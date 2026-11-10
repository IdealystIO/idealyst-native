//! Component-preview emission for generated docs (`previews` feature).
//!
//! Three pieces, each empty with the feature off so dev and release builds
//! compile none of it:
//!
//! - [`props_impl`] — `impl previews::PreviewProps for <Props>`, shared by
//!   `#[props]` and the inline-props `#[component]` form: one
//!   `PreviewField` per field whose type has a control, picked by the
//!   autoref probe in `runtime_vocabulary::previews::__probe`, plus the
//!   `#[prop(example = …)]` starting values.
//! - [`component_symbol`] — the exported `#[no_mangle]` fn returning the
//!   component's `Preview`, and its symbol name.
//! - [`recipe_symbol`] — the same for a zero-argument `recipe!`.
//! - [`registration`] — the catalog `PreviewEntry` recording a symbol.
//!
//! The docs app reaches each preview through an `extern "Rust"` block
//! generated from the catalog's recorded symbol names, so the symbol is
//! the contract between this emission and `crates/tools/docs-app/build.rs`.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Expr, Ident, Type};

/// One props-struct field as the preview emission sees it.
pub(crate) struct FieldIn<'a> {
    pub(crate) name: &'a Ident,
    /// Post-wrap type (what the struct declares).
    pub(crate) ty: &'a Type,
    pub(crate) example: Option<&'a Expr>,
}

/// The text a component's children preview starts with.
const CHILDREN_PLACEHOLDER: &str = "Child content";

/// `impl PreviewProps for #props`. Empty for a generic props struct (no
/// single type to preview) and with the feature off.
pub(crate) fn props_impl(props: &Ident, generic: bool, fields: &[FieldIn<'_>]) -> TokenStream2 {
    if !cfg!(feature = "previews") || generic {
        return TokenStream2::new();
    }
    let pushes = fields.iter().map(|f| {
        let name = f.name;
        let name_str = name.to_string();
        let ty = f.ty;
        if is_element_vec(ty) {
            // A children example is shown as written, not as editable text.
            if f.example.is_some() {
                return quote!();
            }
            return quote! {
                fields.push(::runtime_core::previews::PreviewField::children(
                    #name_str,
                    |p: &mut Self| &mut p.#name,
                    #CHILDREN_PLACEHOLDER,
                ));
            };
        }
        let ty_str = crate::token_text::token_text(&quote!(#ty));
        quote! {
            if let ::core::option::Option::Some(adapter) =
                (&&&&::runtime_core::previews::__probe::Probe::<#ty>::new()).__adapter()
            {
                fields.push(::runtime_core::previews::PreviewField::new(
                    #name_str,
                    #ty_str,
                    adapter,
                    // Annotated: a closure body is checked before the
                    // push unifies `P`, and `p.field` needs the type.
                    |p: &mut Self| &mut p.#name,
                    |p: &Self| &p.#name,
                ).with_imports(
                    (&&&&::runtime_core::previews::__probe::Probe::<#ty>::new()).__imports(),
                ));
            }
        }
    });
    let examples: Vec<TokenStream2> = fields
        .iter()
        .filter_map(|f| {
            let example = f.example?;
            let name = f.name;
            if is_element_vec(f.ty) {
                return Some(quote! {
                    props.#name = ::runtime_core::previews::IntoPreviewChildren::into_preview_children(#example);
                });
            }
            let value = crate::float_lit::coerce_prop_value(example);
            Some(quote! { props.#name = #value; })
        })
        .collect();
    let example_entries: Vec<TokenStream2> = fields
        .iter()
        .filter_map(|f| {
            let example = f.example?;
            let name = f.name.to_string();
            let code = example_source(example);
            let children = is_element_vec(f.ty);
            Some(quote! {
                ::runtime_core::previews::PreviewExample { name: #name, code: #code, children: #children }
            })
        })
        .collect();
    let base = if examples.is_empty() {
        quote!()
    } else {
        quote! {
            fn preview_base() -> Self {
                let mut props = <Self as ::runtime_core::BuildElement>::defaults();
                #(#examples)*
                props
            }
            fn preview_examples() -> &'static [::runtime_core::previews::PreviewExample] {
                &[#(#example_entries),*]
            }
        }
    };
    quote! {
        #[automatically_derived]
        impl ::runtime_core::previews::PreviewProps for #props {
            fn preview_fields() -> ::std::vec::Vec<::runtime_core::previews::PreviewField<Self>> {
                #[allow(unused_imports)]
                use ::runtime_core::previews::__probe::{
                    ViaNone as _, ViaPropControl as _, ViaReactiveVariant as _, ViaVariant as _,
                };
                #[allow(unused_mut)]
                let mut fields = ::std::vec::Vec::new();
                #(#pushes)*
                fields
            }
            #base
        }
    }
}

/// `example` as its author wrote it (`source_text::verbatim`), or
/// formatted by `prettyplease` when its tokens have no source (built by
/// another macro): wrapped in a fn body, which is then peeled off.
fn example_source(example: &Expr) -> String {
    if let Some(text) = crate::source_text::verbatim(&quote!(#example)) {
        return text;
    }
    #[cfg(feature = "previews")]
    {
        let file: syn::File = syn::parse_quote! { fn __example() { #example } };
        let text = prettyplease::unparse(&file);
        let lines: Vec<&str> = text.lines().collect();
        // `fn __example() {`, the body, `}`.
        if lines.len() >= 3 {
            return lines[1..lines.len() - 1]
                .iter()
                .map(|l| l.strip_prefix("    ").unwrap_or(l))
                .collect::<Vec<_>>()
                .join("\n");
        }
    }
    crate::token_text::token_text(&quote!(#example))
}

/// `Vec<Element>` — a children slot, previewed as editable text.
fn is_element_vec(ty: &Type) -> bool {
    let Type::Path(tp) = ty else { return false };
    let Some(seg) = tp.path.segments.last() else { return false };
    if seg.ident != "Vec" {
        return false;
    }
    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else { return false };
    matches!(
        args.args.first(),
        Some(syn::GenericArgument::Type(Type::Path(inner)))
            if inner.path.segments.last().is_some_and(|s| s.ident == "Element")
    )
}

/// The catalog registration of an exported preview `symbol`: a
/// `PreviewEntry` keyed to the component or recipe `name` in this module.
/// Empty when `symbol` is (no preview emitted, which is every build
/// without the feature).
///
/// Its own `inventory::submit!`, never a field on `ComponentEntry` /
/// `RecipeEntry`: those are struct literals in this output, and a field
/// there ties every macro-crate version to one catalog version — older
/// macros stop compiling against a newer catalog and the reverse. This
/// names `PreviewEntry` only when `previews` is on, which only a
/// runtime-core that ships the type can turn on.
pub(crate) fn registration(kind: &str, name: &str, symbol: &str) -> TokenStream2 {
    if symbol.is_empty() {
        return TokenStream2::new();
    }
    let kind = Ident::new(kind, proc_macro2::Span::call_site());
    quote! {
        ::runtime_core::__mcp::inventory::submit! {
            ::runtime_core::__mcp::PreviewEntry {
                kind: ::runtime_core::__mcp::PreviewKind::#kind,
                module_path: module_path!(),
                name: #name,
                symbol: #symbol,
            }
        }
    }
}

/// The exported symbol for an item named `name` declared at `ident`.
///
/// Crate name + item name + the declaring ident's line and column: unique
/// within a link (two same-named components in one crate sit at different
/// positions), stable between the catalog extraction and the docs build
/// (both compile the same source), and a valid identifier. A collision is
/// a duplicate-symbol link error, never a wrong preview.
fn symbol_name(kind: &str, ident: &Ident) -> String {
    let krate = std::env::var("CARGO_CRATE_NAME").unwrap_or_default();
    let start = ident.span().start();
    format!("__idealyst_{kind}__{krate}__{ident}__{}_{}", start.line, start.column)
}

/// A component's preview export: `(fn tokens, symbol)`. The symbol is
/// empty when no preview is emitted (feature off), and then nothing is
/// registered ([`registration`]).
///
/// `tag` names the component's `BuildElement` type: the tag alias / no-arg
/// marker struct `#[component]` declares under the fn's own name.
pub(crate) fn component_symbol(tag: &Ident) -> (TokenStream2, String) {
    if !cfg!(feature = "previews") {
        return (TokenStream2::new(), String::new());
    }
    let symbol = symbol_name("preview", tag);
    let fn_ident = Ident::new(&symbol, tag.span());
    let tokens = quote! {
        #[doc(hidden)]
        #[unsafe(no_mangle)]
        #[allow(non_snake_case)]
        pub fn #fn_ident() -> ::runtime_core::previews::Preview {
            #[allow(unused_imports)]
            use ::runtime_core::previews::__probe::{ViaBuildElement as _, ViaPreviewProps as _};
            (&&::runtime_core::previews::__probe::PropsProbe::<#tag>::new()).__preview()
        }
    };
    (tokens, symbol)
}

/// A zero-argument recipe's export: `(fn tokens, symbol)`, empty as for
/// [`component_symbol`]. A recipe with parameters defines props rather
/// than rendering on its own, so it gets none.
pub(crate) fn recipe_symbol(item_fn: &syn::ItemFn) -> (TokenStream2, String) {
    if !cfg!(feature = "previews") || !item_fn.sig.inputs.is_empty() || !item_fn.sig.generics.params.is_empty() {
        return (TokenStream2::new(), String::new());
    }
    let name = &item_fn.sig.ident;
    let symbol = symbol_name("recipe", name);
    let fn_ident = Ident::new(&symbol, name.span());
    let tokens = quote! {
        #[doc(hidden)]
        #[unsafe(no_mangle)]
        #[allow(non_snake_case)]
        pub fn #fn_ident() -> ::runtime_core::Element {
            ::runtime_core::IntoElement::into_element(#name())
        }
    };
    (tokens, symbol)
}

#[cfg(all(test, feature = "previews"))]
mod tests {
    use super::*;

    fn squash(t: TokenStream2) -> String {
        t.to_string().chars().filter(|c| !c.is_whitespace()).collect()
    }

    #[test]
    fn props_impl_probes_each_field_and_treats_element_vecs_as_children() {
        let label: Ident = syn::parse_quote!(label);
        let label_ty: Type = syn::parse_quote!(::runtime_core::Reactive<String>);
        let example: Expr = syn::parse_quote!("Click me");
        let children: Ident = syn::parse_quote!(children);
        let children_ty: Type = syn::parse_quote!(Vec<Element>);
        let props: Ident = syn::parse_quote!(CardProps);
        let out = squash(props_impl(
            &props,
            false,
            &[
                FieldIn { name: &label, ty: &label_ty, example: Some(&example) },
                FieldIn { name: &children, ty: &children_ty, example: None },
            ],
        ));
        assert!(out.contains("impl::runtime_core::previews::PreviewPropsforCardProps"), "{out}");
        assert!(
            out.contains("(&&&&::runtime_core::previews::__probe::Probe::<::runtime_core::Reactive<String>>::new()).__adapter()"),
            "label goes through the probe: {out}"
        );
        assert!(out.contains("PreviewField::children(\"children\""), "{out}");
        assert!(out.contains("fnpreview_base()"), "an example overrides the base: {out}");
        assert!(out.contains("props.label=(\"Clickme\").into();"), "{out}");
    }

    /// Every example is recorded as written (for the Usage snippet of a
    /// field with no control), and a children example replaces the
    /// editable children text: the slot gets no control and starts at the
    /// example, converted from an element or a list.
    #[test]
    fn examples_are_recorded_as_source_and_a_children_example_is_fixed() {
        let items: Ident = syn::parse_quote!(items);
        let items_ty: Type = syn::parse_quote!(Vec<Crumb>);
        let items_example: Expr = syn::parse_quote!(vec![Crumb::new("Home"), Crumb::new("Docs")]);
        let children: Ident = syn::parse_quote!(children);
        let children_ty: Type = syn::parse_quote!(Vec<Element>);
        let children_example: Expr = syn::parse_quote!(ui! { Badge(label = "One") });
        let props: Ident = syn::parse_quote!(TrailProps);
        let tokens = props_impl(
            &props,
            false,
            &[
                FieldIn { name: &items, ty: &items_ty, example: Some(&items_example) },
                FieldIn { name: &children, ty: &children_ty, example: Some(&children_example) },
            ],
        );
        let text = tokens.to_string();
        let out = squash(tokens);
        assert!(!out.contains("PreviewField::children("), "an example fixes the children slot: {out}");
        assert!(
            out.contains("props.children=::runtime_core::previews::IntoPreviewChildren::into_preview_children(ui!{Badge(label=\"One\")});"),
            "{out}"
        );
        assert!(out.contains("fnpreview_examples()"), "{out}");
        assert!(
            text.contains(r#"code : "vec![Crumb::new(\"Home\"), Crumb::new(\"Docs\")]""#),
            "the example's formatted source: {text}"
        );
        assert!(out.contains("name:\"children\",code:"), "{out}");
        assert!(out.contains("children:true"), "{out}");
    }

    #[test]
    fn props_impl_without_examples_keeps_the_trait_default_base() {
        let props: Ident = syn::parse_quote!(EmptyProps);
        let out = squash(props_impl(&props, false, &[]));
        assert!(!out.contains("preview_base"), "{out}");
    }

    #[test]
    fn generic_props_get_no_preview_impl() {
        let props: Ident = syn::parse_quote!(ListProps);
        assert!(props_impl(&props, true, &[]).is_empty());
    }

    #[test]
    fn component_symbol_is_exported_and_named_after_the_tag() {
        let tag: Ident = syn::parse_quote!(Button);
        let (tokens, symbol) = component_symbol(&tag);
        assert!(symbol.starts_with("__idealyst_preview__"), "{symbol}");
        assert!(symbol.contains("__Button__"), "{symbol}");
        let out = squash(tokens);
        assert!(out.contains("#[unsafe(no_mangle)]"), "{out}");
        assert!(out.contains(&format!("pubfn{symbol}()->::runtime_core::previews::Preview")), "{out}");
        assert!(out.contains("(&&::runtime_core::previews::__probe::PropsProbe::<Button>::new()).__preview()"), "{out}");
    }

    #[test]
    fn only_zero_argument_recipes_export_a_symbol() {
        let plain: syn::ItemFn = syn::parse_quote!(fn button_basic() -> Element { todo!() });
        let (tokens, symbol) = recipe_symbol(&plain);
        assert!(symbol.contains("__button_basic__"), "{symbol}");
        assert!(squash(tokens).contains("into_element(button_basic())"));
        let with_args: syn::ItemFn = syn::parse_quote!(fn custom(label: String) -> Element { todo!() });
        let (tokens, symbol) = recipe_symbol(&with_args);
        assert!(tokens.is_empty() && symbol.is_empty());
    }
}
