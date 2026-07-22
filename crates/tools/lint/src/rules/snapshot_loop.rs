//! `snapshot-loop` — `for … in <expr>.get()` inside a `ui!` / `jsx!` body.
//!
//! ```ignore
//! ui! {
//!     // WRONG — `.get()` runs ONCE at build; the loop iterates a frozen
//!     // Vec snapshot, so later signal mutations never re-render the list
//!     for item in items.get() { Row(data = item) }
//!
//!     // RIGHT — iterate the Signal ITSELF, with a reconciliation key
//!     for item in items, key = item.id { Row(data = item) }
//! }
//! ```
//!
//! The macro's reactive `for` lowering subscribes to the *signal* it is
//! handed; hand it `items.get()` and it receives a plain `Vec` — a
//! build-time snapshot with nothing left to subscribe to. The list renders
//! once and silently never updates. This is the loop-shaped twin of
//! `snapshot-condition` (same frozen-`.get()` trap, `for` header instead of
//! `if` condition), and it is exactly the miss observed live in the arena's
//! debug-fix run: the agent *named* the hoisted-snapshot pitfall correctly,
//! then wrote `for item in items.get()` anyway.
//!
//! Detection: `syn` never descends into macro token streams, so — like
//! `snapshot-condition`'s pass 2 — this rule TOKENIZES every visible
//! `ui!` / `jsx!` invocation body and lexically scans loop headers. A header
//! runs from `for <pat> in` to the body brace group (or the `, key = …`
//! clause); a zero-arg `. get ( )` token sequence anywhere in the header
//! expression is the snapshot. Zero-arg keeps `HashMap::get(&k)` and
//! friends out (they take arguments); `.get_untracked()` is a different
//! name — declared intent — and never matches. Plain-Rust `for` loops
//! outside the macros are ordinary iteration and are not this rule's
//! business.

use proc_macro2::{Delimiter, Span, TokenStream, TokenTree};
use syn::visit::{self, Visit};

use crate::diagnostic::RawDiag;

pub(crate) const RULE: &str = "snapshot-loop";

const HELP: &str = "iterate the Signal itself and add `key = …` (see the \
`keyed_list_add_remove` recipe); `.get()` in a for header freezes a build-time snapshot";

pub(crate) fn check_ui_macro(node: &syn::Macro, out: &mut Vec<RawDiag>) {
    let Some(seg) = node.path.segments.last() else { return };
    let name = seg.ident.to_string();
    if name != "ui" && name != "jsx" {
        return;
    }
    scan_for_headers(node.tokens.clone(), &mut |span| {
        out.push(
            RawDiag::new(
                RULE,
                "`.get()` in a `for` header iterates a build-time snapshot — \
                 the list renders once and never updates",
                span,
            )
            .with_help(HELP),
        );
    });
}

/// Lexical scan for `for <pat> in <expr…> {` headers in a token stream,
/// recursing into every group so nested trees are covered. Calls `hit`
/// with the span of the offending `.get()` when the header expression
/// contains a zero-arg `. get ( )` sequence.
fn scan_for_headers(stream: TokenStream, hit: &mut impl FnMut(Span)) {
    let tokens: Vec<TokenTree> = stream.into_iter().collect();
    let mut i = 0;
    while i < tokens.len() {
        // Recurse into groups FIRST so a `for` inside a nested block (or a
        // loop body containing another loop) is scanned too. Group tokens
        // that also belong to a header we match below are scanned twice at
        // worst, which is harmless: `hit` only fires on header-level
        // `.get()` sequences found by the header walk itself.
        if let TokenTree::Group(g) = &tokens[i] {
            scan_for_headers(g.stream(), hit);
            i += 1;
            continue;
        }
        if !matches!(&tokens[i], TokenTree::Ident(id) if id == "for") {
            i += 1;
            continue;
        }
        // Find the header's `in` at this nesting level. Pattern tokens
        // (`item`, `(a, b)`, `Foo { .. }` groups) can't contain a bare
        // top-level `in`, so the first match is the loop's `in`.
        let mut j = i + 1;
        while j < tokens.len()
            && !matches!(&tokens[j], TokenTree::Ident(id) if id == "in")
        {
            j += 1;
        }
        // Walk the iterated expression: from after `in` to the body brace
        // group or a top-level `,` (the macro's `key = …` clause). The
        // trailing brace group is the body — expressions in iterate
        // position can't end in a bare `{ … }` block (same restriction as
        // Rust's own for-loop grammar), so the first brace group closes
        // the header.
        let mut k = j + 1;
        while k < tokens.len() {
            match &tokens[k] {
                TokenTree::Group(g) if g.delimiter() == Delimiter::Brace => break,
                TokenTree::Punct(p) if p.as_char() == ',' => break,
                // `.get()` — Punct('.'), Ident("get"), empty parens group.
                TokenTree::Punct(p) if p.as_char() == '.' => {
                    if let (Some(TokenTree::Ident(m)), Some(TokenTree::Group(args))) =
                        (tokens.get(k + 1), tokens.get(k + 2))
                    {
                        if m == "get"
                            && args.delimiter() == Delimiter::Parenthesis
                            && args.stream().is_empty()
                        {
                            hit(m.span());
                        }
                    }
                    k += 1;
                }
                _ => k += 1,
            }
        }
        i = j + 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn diags(fn_tokens: proc_macro2::TokenStream) -> Vec<RawDiag> {
        // Route through the shared visitor so the wiring in `mod.rs`
        // (visit_macro → check_ui_macro) is exercised too.
        let item: syn::ItemFn = syn::parse2(fn_tokens).unwrap();
        let file = syn::File { shebang: None, attrs: Vec::new(), items: vec![item.into()] };
        crate::rules::collect(&file)
            .into_iter()
            .filter(|d| d.rule == RULE)
            .collect()
    }

    /// The arena debug-fix regression: the agent diagnosed the hoisted-
    /// snapshot pitfall, then wrote `for item in items.get()` — the same
    /// pitfall in loop form. Must be flagged.
    #[test]
    fn regression_flags_for_in_get_inside_ui() {
        let out = diags(quote! {
            fn build() -> Element {
                ui! {
                    view() {
                        for item in items.get() {
                            Row(data = item)
                        }
                    }
                }
            }
        });
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].message.contains("snapshot"), "{out:?}");
        assert!(
            out[0].help.as_deref().unwrap_or("").contains("keyed_list_add_remove"),
            "{out:?}"
        );
    }

    #[test]
    fn flags_for_in_get_inside_jsx() {
        let out = diags(quote! {
            fn build() -> Element {
                jsx! { for item in items.get() { Row(data = item) } }
            }
        });
        assert_eq!(out.len(), 1, "{out:?}");
    }

    #[test]
    fn flags_get_followed_by_iterator_adapter() {
        // Still a frozen snapshot, just with adapters chained on it.
        let out = diags(quote! {
            fn build() -> Element {
                ui! { for item in items.get().into_iter().rev() { Row(data = item) } }
            }
        });
        assert_eq!(out.len(), 1, "{out:?}");
    }

    #[test]
    fn flags_deeply_nested_for() {
        let out = diags(quote! {
            fn build() -> Element {
                ui! {
                    view() {
                        view() {
                            for row in state.rows.get() { Row(data = row) }
                        }
                    }
                }
            }
        });
        assert_eq!(out.len(), 1, "{out:?}");
    }

    #[test]
    fn keyed_signal_iteration_is_clean() {
        // The canonical form: the Signal itself + a key.
        let out = diags(quote! {
            fn build() -> Element {
                ui! {
                    view() {
                        for item in items, key = item.id {
                            Row(data = item)
                        }
                    }
                }
            }
        });
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn get_inside_loop_body_is_clean() {
        // `.get()` in the BODY is a live read in a reactive scope — only
        // the header (the iterated expression) freezes a snapshot.
        let out = diags(quote! {
            fn build() -> Element {
                ui! {
                    for item in items, key = item.id {
                        text { "{selected.get()}" }
                    }
                }
            }
        });
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn arged_get_is_clean() {
        // `map.get(&k)` takes an argument — not the zero-arg signal read.
        let out = diags(quote! {
            fn build() -> Element {
                ui! { for item in lookup.get(&key).unwrap() { Row(data = item) } }
            }
        });
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn get_untracked_is_clean() {
        // Different name — declared intent, mirrors `snapshot-condition`.
        let out = diags(quote! {
            fn build() -> Element {
                ui! { for item in items.get_untracked() { Row(data = item) } }
            }
        });
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn plain_rust_for_outside_macro_is_clean() {
        // Ordinary Rust iteration over a snapshot outside ui!/jsx! is not
        // a render-freshness question — out of scope.
        let out = diags(quote! {
            fn build() -> Vec<u64> {
                let mut ids = Vec::new();
                for item in items.get() {
                    ids.push(item.id);
                }
                ids
            }
        });
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn get_in_key_clause_is_not_a_header_hit() {
        // The header scan stops at the `,` before `key = …`; whatever the
        // key expression does is the macro's concern, not this rule's.
        let out = diags(quote! {
            fn build() -> Element {
                ui! { for item in items, key = item.id.get() { Row(data = item) } }
            }
        });
        assert!(out.is_empty(), "{out:?}");
    }
}
