//! Generates the per-component dispatch glue that `ui! { Foo(...) }`
//! targets — an `impl runtime_core::BuildElement for FooProps`.
//!
//! `ui!` lowers a tag `Foo` to a plain struct literal plus a UFCS call:
//!
//! ```ignore
//! ::runtime_core::BuildElement::build(
//!     FooProps { label: ("x").into(), ..<FooProps as BuildElement>::defaults() }
//! )
//! ```
//!
//! so the only thing `#[component]` has to emit is the trait impl that
//! ties `FooProps` to the component function. This replaces the old
//! per-component `macro_rules!`: dispatch now resolves across crate
//! boundaries by ordinary path rules (no `#[macro_export]` /
//! `#[macro_use]` ordering), and the call site is a real struct literal,
//! so rust-analyzer gives field completion / hover / go-to-def on props.
//!
//! - `build(self)` absorbs the `fn foo(props: &FooProps)` vs `fn
//!   foo(props: FooProps)` split, so the macro never has to know which.
//! - `defaults()` is only overridden when `#[component(default(field =
//!   expr, …))]` declares defaults; otherwise the trait's provided impl
//!   (`Self::default()`) is used.
//! - A no-argument component gets a generated empty marker `FooProps {}`
//!   so it dispatches through the same path as every other tag.
//!
//! ## Synthesized props (multi-param / plain-arg components)
//!
//! The canonical component shape is a single `props: &<Name>Props`. But a
//! component may instead be written with an ordinary parameter list —
//! `fn SummaryCard(count: usize, total: i64, nav: Ref<DrawerHandle>)`.
//! Those can't name a props struct, so we **synthesize** one from the
//! signature: a `<Name>Props { count, total, nav }` whose fields are each
//! `Option<ParamType>`, plus a `build` that unwraps them and calls the fn
//! positionally. `Option`-wrapping is what lets the struct `derive(Default)`
//! regardless of whether the param types do (the `..defaults()` base is
//! all-`None`), while the call-site `.into()` (`field: (v).into()`) still
//! lands via `From<T> for Option<T>`. The synthesized fields mirror the
//! exact param types — no reactive auto-wrap — so a tag call behaves
//! identically to calling the fn (a `Signal<T>` param stays live, a plain
//! `T` is a snapshot). A missing required prop panics in `build` with a
//! named message.
//!
//! The trigger is unambiguous and back-compatible: a **single** param
//! whose type name ends in `Props` is treated as an existing hand-written
//! props struct (the universal convention — every shipped component uses
//! it); anything else (multi-param, or a single non-`Props` param) is
//! synthesized. Synthesis is skipped — falling back to a plain fn with no
//! `ui!` tag — when the fn is generic or any param is a reference / not a
//! simple `ident: Type` (those can't be `'static` owned props fields).

use proc_macro2::{TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::{FnArg, Ident, ItemFn, Pat, Type, Visibility};

use crate::component_attr::ComponentAttr;

/// Describes the props parameter shape so the generated `build` can pick
/// between `func(&self)` and `func(self)`.
struct PropsType {
    /// Tokens naming the type (e.g. `CardProps`).
    path: TokenStream2,
    /// True when the function takes `props: &Type` (the common case).
    /// False when it takes `props: Type` (owned, used for container
    /// components that need to consume their children).
    by_ref: bool,
}

/// Generates `impl BuildElement for <Props>` for a component, or an empty
/// token stream if the signature doesn't fit the expected shape (zero
/// params, or one param typed as `&SomeProps` / `SomeProps`).
pub(crate) fn generate_build_impl(item_fn: &ItemFn, attr: &ComponentAttr) -> TokenStream2 {
    let fn_name = &item_fn.sig.ident;
    let vis = &item_fn.vis;

    // Propagate the component's own doc comment onto the generated tag
    // alias (and the no-arg marker struct), so hovering a `ui!` tag shows
    // the component's docs instead of a bare `type Foo = FooProps`. The
    // props themselves are real struct fields, so they hover individually
    // at the call site, and go-to-def on the tag lands on the props type.
    let docs: Vec<&syn::Attribute> = item_fn
        .attrs
        .iter()
        .filter(|a| a.path().is_ident("doc"))
        .collect();

    // No props: synthesize an empty marker struct named after the tag so
    // `ui! { Foo() }` dispatches through the same `BuildElement::build`
    // path. Lowercase fns can't be `ui!` tags (the parser only treats
    // uppercase-first idents as components), but generating the marker
    // regardless is harmless and keeps the rule uniform.
    if item_fn.sig.inputs.is_empty() {
        return emit_no_args_impl(vis, fn_name, &docs);
    }

    // A single param whose type name ends in `Props` is an existing
    // hand-written props struct — bridge the tag straight to it. This is
    // the universal component shape, so keeping the old behavior gated on
    // the `Props` suffix is fully back-compatible.
    if let Some(props_type) = existing_props_type(&item_fn.sig) {
        return emit_existing_props_impl(vis, fn_name, &docs, &props_type, attr);
    }

    // Otherwise synthesize a props struct from the parameter list, when
    // every param is a simple owned `ident: Type` and the fn is not
    // generic. If the signature doesn't fit, fall back to the historical
    // behavior: no `BuildElement` impl (the fn is still callable directly;
    // it just isn't a `ui!` tag).
    emit_synthesized_props_impl(vis, fn_name, &docs, item_fn, attr).unwrap_or_default()
}

/// Emits the tag alias + `BuildElement` impl that bridges a component fn to
/// its **existing** props struct (`fn Foo(props: &FooProps)` / `Foo(props:
/// FooProps)`).
fn emit_existing_props_impl(
    vis: &Visibility,
    fn_name: &Ident,
    docs: &[&syn::Attribute],
    props_type: &PropsType,
    attr: &ComponentAttr,
) -> TokenStream2 {
    let path = &props_type.path;
    let amp = if props_type.by_ref { quote!(&) } else { quote!() };
    let defaults_method = defaults_method(attr);

    quote! {
        // Tag alias: `ui! { Foo(...) }` uses the tag as the type name, so
        // this bridges `Foo` to its real props struct. The component fn
        // (`fn Foo`, value namespace) and this alias (type namespace)
        // coexist; existing `use …::Foo` imports resolve here. The
        // component's doc comment rides along so hovering the tag is useful.
        #(#docs)*
        #[allow(non_camel_case_types)]
        #vis type #fn_name = #path;

        #[automatically_derived]
        impl ::runtime_core::BuildElement for #path {
            fn build(self) -> ::runtime_core::Element {
                // Coerce via `IntoElement` so a component returning a richer
                // type than bare `Element` still satisfies `-> Element`:
                // identity for `Element`, `.primitive` for `Bound`/`Bindable`
                // (a `methods!` component returns `Bindable<Handle>`). The tag
                // form drops the handle — use the fn-call form to `.bind` it.
                ::runtime_core::IntoElement::into_element(#fn_name(#amp self))
            }
            #defaults_method
        }
    }
}

/// Synthesizes a `<Name>Props` struct + tag alias + `BuildElement` impl
/// from a component's ordinary parameter list. Returns `None` (→ plain fn,
/// no tag) when the signature can't be turned into a `'static` owned props
/// struct: the fn is generic, or a param is a reference / not a plain
/// `ident: Type`.
fn emit_synthesized_props_impl(
    vis: &Visibility,
    fn_name: &Ident,
    docs: &[&syn::Attribute],
    item_fn: &ItemFn,
    attr: &ComponentAttr,
) -> Option<TokenStream2> {
    // Generic components can't have a single concrete `'static` props type.
    if !item_fn.sig.generics.params.is_empty() || item_fn.sig.generics.where_clause.is_some() {
        return None;
    }

    // Each param must be a plain `ident: OwnedType` (no `self`, no `&T`, no
    // destructuring pattern). Collect (ident, type) for every one.
    let mut fields: Vec<(&Ident, &Type)> = Vec::with_capacity(item_fn.sig.inputs.len());
    for arg in &item_fn.sig.inputs {
        let FnArg::Typed(pat_type) = arg else {
            return None; // a `self` receiver — not a component
        };
        if matches!(&*pat_type.ty, Type::Reference(_)) {
            return None; // a borrowed param can't be a `'static` field
        }
        let Pat::Ident(pat_ident) = &*pat_type.pat else {
            return None; // destructured / non-ident param
        };
        if pat_ident.subpat.is_some() {
            return None; // `ident @ subpat`
        }
        fields.push((&pat_ident.ident, &pat_type.ty));
    }

    let props_name = format_ident!("{}Props", fn_name);

    // `pub field: Option<Ty>` per param. `Option` gives the whole struct a
    // `Default` (all `None`) no matter the field types, so `..defaults()`
    // works and unset props default to "missing".
    let field_decls = fields.iter().map(|(name, ty)| {
        quote! { pub #name: ::core::option::Option<#ty>, }
    });

    // `build` unwraps each field back to the owned value and calls the fn
    // positionally. A prop left unset panics with a named message.
    let call_args = fields.iter().map(|(name, _)| {
        let msg = format!(
            "missing required prop `{name}` on component `{fn_name}` (set it at the `ui!` call site)"
        );
        quote! { self.#name.expect(#msg) }
    });

    let defaults_method = defaults_method(attr);

    Some(quote! {
        // Synthesized props struct: one `Option`-wrapped field per fn
        // parameter. `#[doc(hidden)]` — the fn's own doc is the surface;
        // the fields hover individually at the call site.
        #(#docs)*
        #[doc(hidden)]
        #[derive(::core::default::Default)]
        #[allow(non_camel_case_types)]
        #vis struct #props_name {
            #(#field_decls)*
        }

        // Tag alias so `ui! { Foo(..) }` can name the struct as `Foo`; the
        // `fn Foo` (value namespace) and this alias (type namespace) coexist.
        #(#docs)*
        #[allow(non_camel_case_types)]
        #vis type #fn_name = #props_name;

        #[automatically_derived]
        impl ::runtime_core::BuildElement for #props_name {
            fn build(self) -> ::runtime_core::Element {
                // Coerce via `IntoElement` so a `Bound`/`Bindable` return
                // still satisfies `-> Element` (see the props-bearing impl).
                ::runtime_core::IntoElement::into_element(#fn_name(#(#call_args),*))
            }
            #defaults_method
        }
    })
}

/// The `defaults()` override, emitted only when the author declared
/// `#[component(default(field = expr, …))]`; otherwise empty (the trait's
/// provided `Self::default()` is used). Shared by both emission paths.
fn defaults_method(attr: &ComponentAttr) -> TokenStream2 {
    if attr.defaults.is_empty() {
        return quote!();
    }
    let fills = attr.defaults.iter().map(|d| {
        let name = &d.name;
        let expr = &d.expr;
        quote! { #name: (#expr).into(), }
    });
    quote! {
        fn defaults() -> Self {
            Self {
                #(#fills)*
                ..::core::default::Default::default()
            }
        }
    }
}

/// No-arg component: emit an empty marker struct named after the tag
/// (matching the component's visibility) plus its `BuildElement` impl. The
/// struct is braced-empty so `ui!`'s `Foo { ..defaults() }` struct-update
/// syntax is valid, and braced structs have no value-namespace
/// constructor, so the marker struct and the `fn Foo` coexist.
fn emit_no_args_impl(
    vis: &Visibility,
    fn_name: &Ident,
    docs: &[&syn::Attribute],
) -> TokenStream2 {
    quote! {
        #(#docs)*
        #[doc(hidden)]
        #[derive(::core::default::Default)]
        #[allow(non_camel_case_types)]
        #vis struct #fn_name {}

        #[automatically_derived]
        impl ::runtime_core::BuildElement for #fn_name {
            fn build(self) -> ::runtime_core::Element {
                // See the props-bearing impl: coerce so `Bound`/`Bindable`
                // returns satisfy `-> Element`.
                ::runtime_core::IntoElement::into_element(#fn_name())
            }
        }
    }
}

/// If the function is the canonical single-props shape — exactly one
/// parameter typed `&T` or `T` where the type's final path segment ends in
/// `Props` — returns its props type info. The `Props` suffix is what
/// distinguishes an existing hand-written props struct from an ordinary
/// data param (`fn EntryRow(s: SubmissionDto)`), which is synthesized
/// instead. Every shipped component uses the suffix, so this is a
/// no-op-for-existing-code gate.
fn existing_props_type(sig: &syn::Signature) -> Option<PropsType> {
    if sig.inputs.len() != 1 {
        return None;
    }
    let syn::FnArg::Typed(pt) = &sig.inputs[0] else {
        return None;
    };
    let (path_ty, by_ref) = match &*pt.ty {
        syn::Type::Reference(ref_ty) => match &*ref_ty.elem {
            syn::Type::Path(path_ty) => (path_ty, true),
            _ => return None,
        },
        syn::Type::Path(path_ty) => (path_ty, false),
        _ => return None,
    };
    // Final segment must end in `Props` to count as an existing struct.
    let ends_in_props = path_ty
        .path
        .segments
        .last()
        .is_some_and(|seg| seg.ident.to_string().ends_with("Props"));
    if !ends_in_props {
        return None;
    }
    let path = &path_ty.path;
    Some(PropsType { path: quote! { #path }, by_ref })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attr() -> ComponentAttr {
        ComponentAttr { defaults: Vec::new(), has_children: false, external: None }
    }

    fn build_impl(src: &str) -> String {
        let item_fn: ItemFn = syn::parse_str(src).unwrap();
        generate_build_impl(&item_fn, &attr()).to_string()
    }

    #[test]
    fn existing_props_bridges_not_synthesizes() {
        let out = build_impl("fn Badge(props: &BadgeProps) -> Element { todo!() }");
        assert!(out.contains("type Badge = BadgeProps"));
        assert!(out.contains("impl :: runtime_core :: BuildElement for BadgeProps"));
        // No synthesized struct.
        assert!(!out.contains("struct BadgeProps"));
    }

    #[test]
    fn multi_param_synthesizes_option_wrapped_struct() {
        let out = build_impl(
            "fn SummaryCard(count: usize, total: i64, nav: Ref<DrawerHandle>) -> Element { todo!() }",
        );
        assert!(out.contains("struct SummaryCardProps"));
        assert!(out.contains("count : :: core :: option :: Option < usize >"));
        assert!(out.contains("nav : :: core :: option :: Option < Ref < DrawerHandle > >"));
        assert!(out.contains("type SummaryCard = SummaryCardProps"));
        // build() unwraps and calls the fn positionally.
        assert!(out.contains("SummaryCard (self . count . expect"));
        assert!(out.contains("self . total . expect"));
        assert!(out.contains("self . nav . expect"));
    }

    #[test]
    fn single_non_props_param_is_synthesized() {
        // A lone DTO param is data, not a props struct — synthesize.
        let out = build_impl("fn EntryRow(s: SubmissionDto) -> Element { todo!() }");
        assert!(out.contains("struct EntryRowProps"));
        assert!(out.contains("s : :: core :: option :: Option < SubmissionDto >"));
        assert!(out.contains("EntryRow (self . s . expect"));
    }

    #[test]
    fn reference_param_falls_back_to_no_tag() {
        // Borrowed params can't be `'static` fields → no BuildElement impl.
        let out = build_impl("fn StatusPill(status: &str) -> Element { todo!() }");
        assert!(out.is_empty(), "expected no tag glue, got: {out}");
    }

    #[test]
    fn generic_component_falls_back_to_no_tag() {
        let out = build_impl(
            "fn Press<S: IntoStyleSource>(style: S) -> Element { todo!() }",
        );
        assert!(out.is_empty(), "expected no tag glue, got: {out}");
    }

    #[test]
    fn zero_arg_emits_marker_struct() {
        let out = build_impl("fn Header() -> Element { todo!() }");
        assert!(out.contains("struct Header { }"));
        assert!(out.contains("impl :: runtime_core :: BuildElement for Header"));
    }
}
