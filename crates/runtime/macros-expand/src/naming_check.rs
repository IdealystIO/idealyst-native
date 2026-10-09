//! `strict-naming` — compile-time component-naming enforcement.
//!
//! When the `strict-naming` feature is on (forwarded from
//! `runtime-core/strict-naming`), `#[component]` errors if the component
//! fn name isn't PascalCase. The convention is load-bearing: inside
//! `ui!`/`jsx!`, a PascalCase tag routes to `#[component]` dispatch while a
//! lowercase tag is a framework primitive (`view`, `text`, …). A
//! snake_case component fn would have to be spelled `IconButton` at the tag
//! site anyway, so the mismatch is a guaranteed source of confusion — this
//! catches it at the definition.
//!
//! This is the build-failing sibling of the `component-pascal-case` rule
//! in the `lint` crate: the lint warns while you type, the feature stops
//! the build. The helper is always compiled; the call site in `lib.rs`
//! invokes it only under `#[cfg(feature = "strict-naming")]`, so it costs
//! nothing — and emits nothing — when the feature is off.

use proc_macro2::TokenStream as TokenStream2;

/// PascalCase = starts with an ASCII uppercase letter and contains no
/// underscores. Mirrors `lint`'s `component_case::is_pascal_case` so the
/// lint and the hard gate agree on exactly what passes.
pub(crate) fn is_pascal_case(name: &str) -> bool {
    match name.chars().next() {
        Some(c) if c.is_ascii_uppercase() => !name.contains('_'),
        _ => false,
    }
}

/// Require a PascalCase name on a `#[component]` fn. Returns a spanned
/// `compile_error!` when the name isn't PascalCase; empty otherwise.
pub(crate) fn require_component_pascal_case(item_fn: &syn::ItemFn) -> TokenStream2 {
    let ident = &item_fn.sig.ident;
    let name = ident.to_string();
    if is_pascal_case(&name) {
        return TokenStream2::new();
    }
    let msg = format!(
        "component `{name}` must be PascalCase — components route through `ui!`/`jsx!` as \
         PascalCase tags (a lowercase tag is a primitive). Rename it (e.g. `{}`). Required by \
         the `strict-naming` feature; disable the feature to relax this to the \
         `component-pascal-case` lint instead.",
        to_pascal_case(&name)
    );
    syn::Error::new_spanned(ident, msg).to_compile_error()
}

/// Best-effort `snake_case` → `PascalCase` for the error hint.
fn to_pascal_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut capitalize_next = true;
    for ch in name.chars() {
        if ch == '_' {
            capitalize_next = true;
            continue;
        }
        if capitalize_next {
            out.extend(ch.to_uppercase());
            capitalize_next = false;
        } else {
            out.push(ch);
        }
    }
    if out.is_empty() {
        name.to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contains_compile_error(ts: &TokenStream2) -> bool {
        ts.to_string().contains("compile_error")
    }

    #[test]
    fn pascal_case_recognition() {
        assert!(is_pascal_case("Button"));
        assert!(is_pascal_case("IconButton"));
        assert!(!is_pascal_case("button"));
        assert!(!is_pascal_case("icon_button"));
        assert!(!is_pascal_case(""));
    }

    #[test]
    fn errors_only_on_non_pascal() {
        let ok: syn::ItemFn = syn::parse_quote! {
            fn IconButton() -> Element { todo!() }
        };
        assert!(require_component_pascal_case(&ok).is_empty());

        let bad: syn::ItemFn = syn::parse_quote! {
            fn icon_button() -> Element { todo!() }
        };
        let err = require_component_pascal_case(&bad);
        assert!(contains_compile_error(&err));
        assert!(err.to_string().contains("IconButton"), "hint suggests the fix");
    }
}
