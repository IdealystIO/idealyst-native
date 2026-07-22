//! Parser for the `#[component(...)]` attribute argument list.
//!
//! Recognized shapes (any order, any subset):
//!   `default(field = expr, field = expr, ...)` — per-field defaults
//!   `children`                                  — opt in to children-aware
//!                                                  treatment in future tooling
//!   `external` / `external(tag = "...")`        — mark for `idealyst export`
//!                                                  (Web Component generation)
//! Empty input is valid.

use proc_macro2::TokenStream as TokenStream2;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Expr, Ident, LitStr, Token};

/// One `field = expr` pair declared in `#[component(default(...))]`.
#[derive(Debug)]
pub(crate) struct DefaultEntry {
    pub(crate) name: Ident,
    pub(crate) expr: Expr,
}

/// The `external` marker — opt this component into `idealyst export`.
/// An optional `tag = "..."` overrides the default custom-element tag
/// (`idl-<kebab(name)>`).
#[derive(Debug)]
pub(crate) struct ExternalSpec {
    // Read only under the `catalog` feature (by `external_emit`); allow it
    // to sit unread in non-catalog builds without a dead-code warning.
    #[allow(dead_code)]
    pub(crate) tag: Option<String>,
}

/// Parsed `#[component(...)]` attribute arguments.
#[derive(Debug)]
pub(crate) struct ComponentAttr {
    pub(crate) defaults: Vec<DefaultEntry>,
    /// Recorded for future use: an eventual children-aware shape check or
    /// view-macro dispatch will consult this flag. The per-component
    /// invocation macro is unchanged whether the flag is set or not.
    #[allow(dead_code)]
    pub(crate) has_children: bool,
    /// `Some` when the component is tagged for external (Web Component)
    /// export. Drives the `ExternalEntry` catalog registration.
    pub(crate) external: Option<ExternalSpec>,
    /// `#[component(lazy)]` / `#[component(lazy = true)]` (or the `#[lazy]`
    /// shorthand) — the component's body is compiled into a separate wasm
    /// chunk and its `BuildElement` yields an `Element::Lazy`. The generated
    /// props struct gains `loading` / `error` config fields.
    pub(crate) lazy: bool,
    /// `#[component(lazy, retryable)]` — derive `Clone` on the props so the
    /// error UI's `retry()` can re-invoke the loader. Only meaningful with
    /// `lazy`. Without it the loader is one-shot (props move once, no `Clone`
    /// bound) and the error UI is message-only.
    pub(crate) retryable: bool,
}

impl ComponentAttr {
    fn empty() -> Self {
        ComponentAttr {
            defaults: Vec::new(),
            has_children: false,
            external: None,
            lazy: false,
            retryable: false,
        }
    }
}

pub(crate) fn parse_component_attr(input: TokenStream2) -> syn::Result<ComponentAttr> {
    if input.is_empty() {
        return Ok(ComponentAttr::empty());
    }
    syn::parse2::<ComponentAttr>(input)
}

/// Parse the argument list of the `#[lazy]` shorthand and fold it into a
/// `ComponentAttr` with `lazy` forced on. `#[lazy]` == `#[component(lazy)]`;
/// `#[lazy(retryable)]` == `#[component(lazy, retryable)]`. The same grammar
/// as `#[component(...)]` is accepted (so `#[lazy(default(...))]` works), with
/// `lazy` implied.
pub(crate) fn parse_lazy_attr(input: TokenStream2) -> syn::Result<ComponentAttr> {
    let mut attr = if input.is_empty() {
        ComponentAttr::empty()
    } else {
        syn::parse2::<ComponentAttr>(input)?
    };
    attr.lazy = true;
    Ok(attr)
}

impl Parse for ComponentAttr {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut defaults = Vec::new();
        let mut has_children = false;
        let mut external = None;
        let mut lazy = false;
        let mut retryable = false;
        while !input.is_empty() {
            let ident: Ident = input.parse()?;
            match ident.to_string().as_str() {
                "lazy" => {
                    lazy = true;
                    // Accept the `lazy = true` / `lazy = false` spelling too.
                    if input.peek(Token![=]) {
                        let _: Token![=] = input.parse()?;
                        let lit: syn::LitBool = input.parse()?;
                        lazy = lit.value;
                    }
                }
                "retryable" => {
                    retryable = true;
                }
                "default" => {
                    let content;
                    syn::parenthesized!(content in input);
                    let pairs: Punctuated<DefaultPair, Token![,]> =
                        content.parse_terminated(DefaultPair::parse, Token![,])?;
                    defaults.extend(
                        pairs.into_iter().map(|p| DefaultEntry { name: p.name, expr: p.expr }),
                    );
                }
                "children" => {
                    has_children = true;
                }
                "external" => {
                    // Bare `external`, or `external(tag = "...")`.
                    let mut tag = None;
                    if input.peek(syn::token::Paren) {
                        let content;
                        syn::parenthesized!(content in input);
                        while !content.is_empty() {
                            let key: Ident = content.parse()?;
                            if key != "tag" {
                                return Err(syn::Error::new(
                                    key.span(),
                                    format!(
                                        "unexpected `external` argument `{}`; only `tag = \"...\"` is supported",
                                        key
                                    ),
                                ));
                            }
                            let _: Token![=] = content.parse()?;
                            let lit: LitStr = content.parse()?;
                            tag = Some(lit.value());
                            if content.is_empty() {
                                break;
                            }
                            let _: Token![,] = content.parse()?;
                        }
                    }
                    external = Some(ExternalSpec { tag });
                }
                other => {
                    return Err(syn::Error::new(
                        ident.span(),
                        format!(
                            "unexpected argument `{}`; only `default(...)`, `children`, `external`, `lazy`, and `retryable` are supported",
                            other
                        ),
                    ));
                }
            }
            if input.is_empty() {
                break;
            }
            let _: Token![,] = input.parse()?;
        }
        if retryable && !lazy {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "`retryable` only applies to a lazy component; use `#[component(lazy, retryable)]`",
            ));
        }
        Ok(ComponentAttr { defaults, has_children, external, lazy, retryable })
    }
}

struct DefaultPair {
    name: Ident,
    expr: Expr,
}

impl Parse for DefaultPair {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name: Ident = input.parse()?;
        let _: Token![=] = input.parse()?;
        let expr: Expr = input.parse()?;
        Ok(DefaultPair { name, expr })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    #[test]
    fn empty_attr_yields_empty_defaults() {
        let a = parse_component_attr(TokenStream2::new()).unwrap();
        assert!(a.defaults.is_empty());
        assert!(!a.has_children);
    }

    #[test]
    fn parses_single_default() {
        let a = parse_component_attr(quote! { default(step = 1) }).unwrap();
        assert_eq!(a.defaults.len(), 1);
        assert_eq!(a.defaults[0].name.to_string(), "step");
        assert!(!a.has_children);
    }

    #[test]
    fn parses_multiple_defaults() {
        let a = parse_component_attr(quote! { default(step = 1, gap = 10) }).unwrap();
        assert_eq!(a.defaults.len(), 2);
        assert_eq!(a.defaults[0].name.to_string(), "step");
        assert_eq!(a.defaults[1].name.to_string(), "gap");
    }

    #[test]
    fn parses_children_flag() {
        let a = parse_component_attr(quote! { children }).unwrap();
        assert!(a.has_children);
        assert!(a.defaults.is_empty());
    }

    #[test]
    fn parses_combined() {
        let a = parse_component_attr(quote! { children, default(step = 1) }).unwrap();
        assert!(a.has_children);
        assert_eq!(a.defaults.len(), 1);
    }

    #[test]
    fn rejects_unknown_argument() {
        let result = parse_component_attr(quote! { unknown });
        let err = match result {
            Ok(_) => panic!("expected parse error"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("unknown"));
    }

    #[test]
    fn parses_bare_external() {
        let a = parse_component_attr(quote! { external }).unwrap();
        let ext = a.external.expect("external flag set");
        assert!(ext.tag.is_none());
    }

    #[test]
    fn parses_external_with_tag() {
        let a = parse_component_attr(quote! { external(tag = "x-greeter") }).unwrap();
        let ext = a.external.expect("external flag set");
        assert_eq!(ext.tag.as_deref(), Some("x-greeter"));
    }

    #[test]
    fn parses_external_combined_with_default() {
        let a = parse_component_attr(quote! { external, default(step = 1) }).unwrap();
        assert!(a.external.is_some());
        assert_eq!(a.defaults.len(), 1);
    }

    #[test]
    fn rejects_unknown_external_argument() {
        let err = parse_component_attr(quote! { external(name = "x") }).unwrap_err();
        assert!(err.to_string().contains("external"));
    }

    #[test]
    fn no_external_by_default() {
        let a = parse_component_attr(quote! { children }).unwrap();
        assert!(a.external.is_none());
    }
}
