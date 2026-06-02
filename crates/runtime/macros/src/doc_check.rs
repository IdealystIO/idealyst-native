//! `strict-docs` — compile-time documentation enforcement.
//!
//! When the `strict-docs` feature is on (forwarded from
//! `runtime-core/strict-docs`), `#[component]` requires a doc comment on
//! the component fn, and `#[derive(IdealystSchema)]` requires one on
//! every prop field / enum variant. A missing doc becomes a
//! `compile_error!` spanned at the offending item, so the *compiler*
//! catches undocumented surface — no guessing, no after-the-fact audit.
//!
//! These helpers are always compiled (not feature-gated); the call sites
//! in `lib.rs` invoke them only under `#[cfg(feature = "strict-docs")]`,
//! so there's zero cost — and zero generated tokens — when the feature
//! is off.

use proc_macro2::TokenStream as TokenStream2;

/// True when `attrs` carries at least one non-empty `///` /
/// `#[doc = "..."]`. A doc comment that is present but blank (e.g. a
/// lone `///`) does NOT count — the point is real documentation.
pub(crate) fn has_doc(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if !attr.path().is_ident("doc") {
            return false;
        }
        if let syn::Meta::NameValue(nv) = &attr.meta {
            if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) = &nv.value {
                return !s.value().trim().is_empty();
            }
        }
        false
    })
}

/// Require a doc comment on a `#[component]` fn. Returns a spanned
/// `compile_error!` token stream when absent; empty otherwise.
pub(crate) fn require_component_doc(item_fn: &syn::ItemFn) -> TokenStream2 {
    if has_doc(&item_fn.attrs) {
        return TokenStream2::new();
    }
    let name = &item_fn.sig.ident;
    let msg = format!(
        "component `{name}` is missing documentation. Add a `///` doc comment \
         describing it — required by the `strict-docs` feature. Disable the \
         feature to relax this to a non-error."
    );
    syn::Error::new_spanned(name, msg).to_compile_error()
}

/// Require a doc comment on every named struct field / every enum
/// variant of an `#[derive(IdealystSchema)]` type. Emits one
/// `compile_error!` per undocumented item, each spanned at the item, so
/// the compiler points right at the prop/variant that needs a comment.
/// Tuple/unit structs have no named props to document, so they're
/// exempt.
pub(crate) fn require_schema_docs(input: &syn::DeriveInput) -> TokenStream2 {
    let mut errs = TokenStream2::new();
    match &input.data {
        syn::Data::Struct(s) => {
            if let syn::Fields::Named(named) = &s.fields {
                for f in &named.named {
                    if has_doc(&f.attrs) {
                        continue;
                    }
                    let ident = f.ident.as_ref().expect("named field has an ident");
                    let msg = format!(
                        "prop `{ident}` is missing documentation. Add a `///` doc \
                         comment — required by the `strict-docs` feature."
                    );
                    errs.extend(syn::Error::new_spanned(ident, msg).to_compile_error());
                }
            }
        }
        syn::Data::Enum(e) => {
            for v in &e.variants {
                if has_doc(&v.attrs) {
                    continue;
                }
                let ident = &v.ident;
                let msg = format!(
                    "variant `{ident}` is missing documentation. Add a `///` doc \
                     comment — required by the `strict-docs` feature."
                );
                errs.extend(syn::Error::new_spanned(ident, msg).to_compile_error());
            }
        }
        syn::Data::Union(_) => {}
    }
    errs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contains_compile_error(ts: &TokenStream2) -> bool {
        ts.to_string().contains("compile_error")
    }

    #[test]
    fn has_doc_distinguishes_documented_from_blank_and_absent() {
        let documented: syn::ItemFn = syn::parse_quote! {
            /// Greets the user.
            fn greet() {}
        };
        assert!(has_doc(&documented.attrs));

        let absent: syn::ItemFn = syn::parse_quote! {
            fn greet() {}
        };
        assert!(!has_doc(&absent.attrs));

        let blank: syn::ItemFn = syn::parse_quote! {
            ///
            fn greet() {}
        };
        assert!(!has_doc(&blank.attrs), "a blank /// is not documentation");
    }

    #[test]
    fn component_doc_required_only_when_absent() {
        let documented: syn::ItemFn = syn::parse_quote! {
            /// A button.
            fn Button() {}
        };
        assert!(require_component_doc(&documented).is_empty());

        let undocumented: syn::ItemFn = syn::parse_quote! {
            fn Button() {}
        };
        let err = require_component_doc(&undocumented);
        assert!(contains_compile_error(&err));
        assert!(err.to_string().contains("Button"));
    }

    #[test]
    fn schema_docs_required_per_undocumented_field() {
        let input: syn::DeriveInput = syn::parse_quote! {
            struct ButtonProps {
                /// The label.
                label: String,
                on_click: Rc<dyn Fn()>,
                disabled: bool,
            }
        };
        let errs = require_schema_docs(&input).to_string();
        assert!(errs.contains("compile_error"));
        // The two undocumented fields are named; the documented one isn't.
        assert!(errs.contains("on_click"));
        assert!(errs.contains("disabled"));
        assert!(!errs.contains("label"));
    }

    #[test]
    fn fully_documented_struct_passes() {
        let input: syn::DeriveInput = syn::parse_quote! {
            struct P {
                /// a
                a: u8,
                /// b
                b: u8,
            }
        };
        assert!(require_schema_docs(&input).is_empty());
    }

    #[test]
    fn enum_variants_require_docs() {
        let input: syn::DeriveInput = syn::parse_quote! {
            enum Tone { Primary, Secondary }
        };
        let errs = require_schema_docs(&input).to_string();
        assert!(errs.contains("compile_error"));
        assert!(errs.contains("Primary"));

        let documented: syn::DeriveInput = syn::parse_quote! {
            enum Tone {
                /// The primary tone.
                Primary,
            }
        };
        assert!(require_schema_docs(&documented).is_empty());
    }
}
