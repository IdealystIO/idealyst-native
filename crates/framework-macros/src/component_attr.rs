//! Parser for the `#[component(...)]` attribute argument list.
//!
//! Recognized shapes (any order, any subset):
//!   `default(field = expr, field = expr, ...)` — per-field defaults
//!   `children`                                  — opt in to children-aware
//!                                                  treatment in future tooling
//! Empty input is valid.

use proc_macro2::TokenStream as TokenStream2;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Expr, Ident, Token};

/// One `field = expr` pair declared in `#[component(default(...))]`.
pub(crate) struct DefaultEntry {
    pub(crate) name: Ident,
    pub(crate) expr: Expr,
}

/// Parsed `#[component(...)]` attribute arguments.
pub(crate) struct ComponentAttr {
    pub(crate) defaults: Vec<DefaultEntry>,
    /// Recorded for future use: an eventual children-aware shape check or
    /// view-macro dispatch will consult this flag. The per-component
    /// invocation macro is unchanged whether the flag is set or not.
    #[allow(dead_code)]
    pub(crate) has_children: bool,
}

pub(crate) fn parse_component_attr(input: TokenStream2) -> syn::Result<ComponentAttr> {
    if input.is_empty() {
        return Ok(ComponentAttr { defaults: Vec::new(), has_children: false });
    }
    syn::parse2::<ComponentAttr>(input)
}

impl Parse for ComponentAttr {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut defaults = Vec::new();
        let mut has_children = false;
        while !input.is_empty() {
            let ident: Ident = input.parse()?;
            match ident.to_string().as_str() {
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
                other => {
                    return Err(syn::Error::new(
                        ident.span(),
                        format!(
                            "unexpected argument `{}`; only `default(...)` and `children` are supported",
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
        Ok(ComponentAttr { defaults, has_children })
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
}
