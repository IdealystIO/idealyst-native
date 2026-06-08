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

use proc_macro2::{TokenStream as TokenStream2};
use quote::quote;
use syn::{Ident, ItemFn, Visibility};

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

    let Some(props_type) = props_type_from_sig(&item_fn.sig) else {
        return TokenStream2::new();
    };
    let path = &props_type.path;
    let amp = if props_type.by_ref { quote!(&) } else { quote!() };

    // Only override `defaults()` when the author declared defaults; the
    // trait's provided impl (`Self::default()`) covers the common case.
    let defaults_method = if attr.defaults.is_empty() {
        quote!()
    } else {
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
    };

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

/// If the function has exactly one parameter, returns its props type info.
/// Accepts both `_: &T` and `_: T` (where T is a path type).
fn props_type_from_sig(sig: &syn::Signature) -> Option<PropsType> {
    if sig.inputs.len() != 1 {
        return None;
    }
    let syn::FnArg::Typed(pt) = &sig.inputs[0] else {
        return None;
    };
    match &*pt.ty {
        syn::Type::Reference(ref_ty) => {
            let syn::Type::Path(path_ty) = &*ref_ty.elem else {
                return None;
            };
            let path = &path_ty.path;
            Some(PropsType { path: quote! { #path }, by_ref: true })
        }
        syn::Type::Path(path_ty) => {
            let path = &path_ty.path;
            Some(PropsType { path: quote! { #path }, by_ref: false })
        }
        _ => None,
    }
}
