//! `new-core` retargeting (idea-lite core migration, P3a).
//!
//! When the `new-core` feature is on, every proc-macro entry point that
//! emits core paths pipes its OUTPUT through [`retarget`], which rewrites
//! absolute `::runtime_core::…` path heads to
//! `::runtime_vocabulary::glue::…`. The glue module
//! (`crates/runtime/vocabulary/src/glue.rs`) mirrors the names and call
//! shapes the emission relies on (`view(children)`, `ChildList`,
//! `IntoElement`, `BuildElement`, `when`/`switch`, the dispatch traits,
//! the f-string slot machinery), implemented on the new core.
//!
//! ## Why output rewriting instead of editing ~300 emission sites
//!
//! The alternative — threading a switchable path root through every
//! `quote!` in ui.rs/jsx.rs/inline_props.rs/… — touches every emission
//! site and risks drift between the two lowerings during the migration
//! window. Rewriting the assembled output keeps the OLD lowering
//! byte-identical when the feature is off (the rewriter isn't even
//! compiled) and makes the retarget a single reviewable seam. The
//! emission DIFFERENCES that are more than a path change (structured
//! generator-backend bindings, deferred primitives) are explicit
//! decision points at the emission sites themselves.
//!
//! ## Scope of the rewrite — and its documented edge
//!
//! Only absolute path heads are rewritten: the token sequence
//! `:: runtime_core` (a joint `::` punct pair immediately followed by
//! the ident `runtime_core`), recursively inside groups. Macro-emitted
//! core references are always absolute, so this catches all of them. A
//! USER expression inside a `ui!` body or `#[component]` fn that spells
//! an absolute `::runtime_core::…` path is rewritten too — transitional
//! behavior, documented here on purpose: inside a new-core crate a
//! direct old-core reference is wrong either way, and the remap either
//! lands on the glue equivalent or fails loudly with a
//! `runtime_vocabulary::glue` path in the error. Unqualified
//! `runtime_core::…` user paths are NOT touched.

use proc_macro2::{Delimiter, Group, Ident, Punct, Spacing, TokenStream as TokenStream2, TokenTree};

/// Rewrite absolute `::runtime_core` path heads to
/// `::runtime_vocabulary::glue` throughout `stream` (recursing into
/// groups), preserving spans.
pub(crate) fn retarget(stream: TokenStream2) -> TokenStream2 {
    let tokens: Vec<TokenTree> = stream.into_iter().collect();
    let mut out: Vec<TokenTree> = Vec::with_capacity(tokens.len());
    let mut i = 0;
    while i < tokens.len() {
        // Match `:` (joint) `:` `runtime_core`.
        if i + 2 < tokens.len() {
            if let (TokenTree::Punct(p1), TokenTree::Punct(p2), TokenTree::Ident(id)) =
                (&tokens[i], &tokens[i + 1], &tokens[i + 2])
            {
                // A LEADING `::` only: if the previous token is a
                // path-segment ident, this `::` is a mid-path separator
                // (`some_seg::runtime_core::…`) — a user path rooted
                // elsewhere; leave it alone. Keywords are `Ident`
                // tokens too (`as ::runtime_core::X`,
                // `impl ::runtime_core::T for …`, `use ::runtime_core::…`)
                // but can never be a path segment, so they do NOT
                // suppress the rewrite.
                let prev_is_segment = matches!(
                    out.last(),
                    Some(TokenTree::Ident(prev)) if !is_non_segment_keyword(&prev.to_string())
                );
                if !prev_is_segment
                    && p1.as_char() == ':'
                    && p1.spacing() == Spacing::Joint
                    && p2.as_char() == ':'
                    && id == "runtime_core"
                {
                    let span = id.span();
                    out.push(tokens[i].clone());
                    out.push(tokens[i + 1].clone());
                    out.push(TokenTree::Ident(Ident::new("runtime_vocabulary", span)));
                    let mut c1 = Punct::new(':', Spacing::Joint);
                    c1.set_span(span);
                    let mut c2 = Punct::new(':', Spacing::Alone);
                    c2.set_span(span);
                    out.push(TokenTree::Punct(c1));
                    out.push(TokenTree::Punct(c2));
                    out.push(TokenTree::Ident(Ident::new("glue", span)));
                    i += 3;
                    continue;
                }
            }
        }
        match &tokens[i] {
            TokenTree::Group(g) => {
                let mut ng = Group::new(g.delimiter(), retarget(g.stream()));
                ng.set_span(g.span());
                // Preserve the delimiter kind (None-delimited groups
                // matter for precedence).
                debug_assert!(matches!(
                    g.delimiter(),
                    Delimiter::Parenthesis
                        | Delimiter::Brace
                        | Delimiter::Bracket
                        | Delimiter::None
                ));
                out.push(TokenTree::Group(ng));
            }
            other => out.push(other.clone()),
        }
        i += 1;
    }
    out.into_iter().collect()
}

/// Keywords that may directly precede an ABSOLUTE `::path` (they can
/// never be path segments themselves). `crate`/`self`/`super`/`Self`
/// are deliberately absent — those ARE segments (`self::runtime_core`
/// would be a user path).
fn is_non_segment_keyword(ident: &str) -> bool {
    matches!(
        ident,
        "as" | "use" | "impl" | "for" | "in" | "dyn" | "where" | "pub" | "fn" | "let" | "mut"
            | "ref" | "move" | "if" | "else" | "match" | "return" | "break" | "continue"
            | "type" | "const" | "static" | "trait" | "enum" | "struct" | "mod" | "unsafe"
            | "box" | "async" | "await" | "yield" | "while" | "loop" | "extern"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn squash(t: TokenStream2) -> String {
        t.to_string().chars().filter(|c| !c.is_whitespace()).collect()
    }

    #[test]
    fn rewrites_absolute_core_paths() {
        let out = retarget(quote! { ::runtime_core::view(::std::vec::Vec::new()) });
        assert_eq!(
            squash(out),
            "::runtime_vocabulary::glue::view(::std::vec::Vec::new())"
        );
    }

    #[test]
    fn rewrites_inside_nested_groups() {
        let out = retarget(quote! {
            f({ let x: ::runtime_core::Element = y; [::runtime_core::text("a")] })
        });
        let s = squash(out);
        assert!(s.contains("::runtime_vocabulary::glue::Element"), "{s}");
        assert!(s.contains("::runtime_vocabulary::glue::text"), "{s}");
        assert!(!s.contains("runtime_core"), "{s}");
    }

    #[test]
    fn leaves_unqualified_user_paths_alone() {
        let out = retarget(quote! { runtime_core::Element::Item });
        assert_eq!(squash(out), "runtime_core::Element::Item");
    }

    #[test]
    fn leaves_mid_path_segments_alone() {
        let out = retarget(quote! { my_crate::runtime_core::thing });
        assert_eq!(squash(out), "my_crate::runtime_core::thing");
    }

    /// Regression (first live compile of newcore-app): keywords are
    /// `Ident` tokens, and `as ::runtime_core::X` / `impl
    /// ::runtime_core::T for` / `use ::runtime_core::…` were wrongly
    /// treated as mid-path separators and left untouched, so every
    /// expansion kept unresolvable `runtime_core` imports.
    #[test]
    fn regression_keyword_before_absolute_path_still_rewrites() {
        for (input, needle) in [
            (
                quote! { <Foo as ::runtime_core::BuildElement>::defaults() },
                "as::runtime_vocabulary::glue::BuildElement",
            ),
            (
                quote! { impl ::runtime_core::BuildElement for FooProps {} },
                "impl::runtime_vocabulary::glue::BuildElement",
            ),
            (
                quote! { use ::runtime_core::{StaticCond as _, ReactiveCond as _}; },
                "use::runtime_vocabulary::glue::",
            ),
        ] {
            let s = squash(retarget(input));
            assert!(s.contains(needle), "{s}");
            assert!(!s.contains("runtime_core"), "{s}");
        }
    }

    #[test]
    fn leaves_other_idents_alone() {
        let out = retarget(quote! { ::runtime_world::signal(0) });
        assert_eq!(squash(out), "::runtime_world::signal(0)");
    }
}
