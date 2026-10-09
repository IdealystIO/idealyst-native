//! `doc_scope!(Marker = "Title", …)` emission.
//!
//! Declares a documentation **scope** — the organizing node of the
//! catalog's spine (see `docs/catalog-scopes-spec.md`). The macro emits
//! an `inventory::submit!(ScopeEntry)` carrying the scope's stable slug,
//! title, optional docs/parent/order, and `module_path!()` (used for the
//! ambient proximity join that assigns entities to scopes).
//!
//! This whole module compiles only under the `catalog` feature; the
//! `doc_scope!` proc-macro emits NOTHING when `catalog` is off (see
//! `lib.rs`), so scopes cost zero in production — write `doc_scope!(...)`
//! with no `#[cfg]` of your own.
//!
//! Scopes are flat (no parent/child hierarchy — granularity comes from
//! module nesting, see `docs/catalog-scopes-spec.md`). The macro emits
//! only the registration; the marker ident serves to derive the default
//! slug.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::{Parse, ParseStream};

struct ScopeInput {
    /// Marker ident — the scope's name; default slug is its lowercase.
    marker: syn::Ident,
    title: syn::LitStr,
    slug: Option<syn::LitStr>,
    docs: Option<syn::LitStr>,
    order: Option<syn::LitInt>,
}

impl Parse for ScopeInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let marker: syn::Ident = input.parse()?;
        input.parse::<syn::Token![=]>()?;
        let title: syn::LitStr = input.parse()?;
        let mut out = ScopeInput {
            marker,
            title,
            slug: None,
            docs: None,
            order: None,
        };
        while input.peek(syn::Token![,]) {
            input.parse::<syn::Token![,]>()?;
            if input.is_empty() {
                break; // tolerate a trailing comma
            }
            let key: syn::Ident = input.parse()?;
            input.parse::<syn::Token![=]>()?;
            match key.to_string().as_str() {
                "slug" => out.slug = Some(input.parse()?),
                "docs" => out.docs = Some(input.parse()?),
                "order" => out.order = Some(input.parse()?),
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!(
                            "unknown doc_scope! key `{other}` \
                             (expected `slug`, `docs`, or `order`)"
                        ),
                    ))
                }
            }
        }
        Ok(out)
    }
}

pub(crate) fn emit(input: TokenStream2) -> TokenStream2 {
    let parsed = match syn::parse2::<ScopeInput>(input) {
        Ok(p) => p,
        Err(e) => return e.to_compile_error(),
    };

    let title = parsed.title.value();
    let slug = parsed
        .slug
        .map(|s| s.value())
        .unwrap_or_else(|| parsed.marker.to_string().to_lowercase());
    let docs = parsed.docs.map(|d| d.value()).unwrap_or_default();
    let order: u32 = parsed
        .order
        .and_then(|o| o.base10_parse().ok())
        .unwrap_or(0);

    quote! {
        ::runtime_core::__mcp::inventory::submit! {
            ::runtime_core::__mcp::ScopeEntry {
                slug: #slug,
                title: #title,
                docs: #docs,
                module_path: module_path!(),
                order: #order,
            }
        }
    }
}
