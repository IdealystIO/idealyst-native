//! `snapshot-condition` — the hoisted-snapshot trap, caught at edit time.
//!
//! ```ignore
//! #[component]
//! fn A() -> Element {
//!     let too_short = name.get().len() < 3;   // runs ONCE at build
//!     ui! { if too_short { … } }              // static branch — never updates
//! }
//! ```
//!
//! A component body runs once, so a `let` whose initializer performs a
//! bare `.get()` (outside any closure) is a frozen snapshot; using that
//! binding as a `ui!`/`jsx!` `if` condition makes a branch that silently
//! never updates — and the same for a `match NAME { … }` scrutinee, which
//! builds once and never switches arms. This is the edit-time twin of the runtime
//! kernel's run-once component body — the read is untracked by
//! construction and nothing at runtime says so, so this catches it in the
//! editor.
//!
//! Detection is deliberately narrow (high precision over recall):
//! - only fns annotated `#[component]`;
//! - only top-level `let <ident> = <init>` where `<init>` contains a
//!   zero-arg `.get()` call NOT inside a closure (a `memo(move || …)` /
//!   `rx!(…)` initializer keeps its reads inside a closure and never
//!   matches; `.peek()` — and `.get_untracked()` on a `Reactive<T>` prop
//!   — are different names: declared intent, never matched);
//! - only bindings later used as a bare `if [!]NAME {` condition or a
//!   bare `match NAME {` scrutinee inside a `ui!` / `jsx!` token stream in
//!   the same fn.
//!
//! The `match` half matters because a bare identifier is exactly where
//! `ui!`'s reactivity gate stops seeing a read: any call or method call
//! in a condition (`.get()`, `f(sig)`, `x.len()`) lowers reactively, and
//! a bare `Signal` / `Memo` is reactive by type in an `if` and a type
//! error in a `match` — but a bare plain-value binding is static, so a
//! hoisted snapshot there freezes silently.
//!
//! `HashMap::get(k)` and friends take arguments, so the zero-arg match
//! skips them; `Cell::get()` is the known benign false positive, and the
//! inline `// idealyst-lint-disable` directives cover it (same escape as
//! an intentional snapshot).

use proc_macro2::{Delimiter, TokenStream, TokenTree};
use syn::spanned::Spanned;
use syn::visit::{self, Visit};

use crate::diagnostic::RawDiag;

pub(crate) const RULE: &str = "snapshot-condition";

pub(crate) fn check_fn(item: &syn::ItemFn, out: &mut Vec<RawDiag>) {
    if !item.attrs.iter().any(|a| a.path().is_ident("component")) {
        return;
    }

    // Pass 1: snapshot candidates among the body's top-level lets.
    let mut candidates: Vec<(String, proc_macro2::Span)> = Vec::new();
    for stmt in &item.block.stmts {
        let syn::Stmt::Local(local) = stmt else { continue };
        let Some(name) = binding_ident(&local.pat) else { continue };
        let Some(init) = &local.init else { continue };
        if has_bare_get(&init.expr) {
            candidates.push((name, local.span()));
        }
    }
    if candidates.is_empty() {
        return;
    }

    // Pass 2: is a candidate used as `if [!]NAME {` inside a ui!/jsx!
    // invocation in this fn?
    let mut finder = UiMacroFinder { streams: Vec::new() };
    finder.visit_block(&item.block);
    for stream in finder.streams {
        scan_conditions(stream, &mut |cond_ident, kw| {
            if let Some((name, span)) =
                candidates.iter().find(|(n, _)| n == cond_ident)
            {
                out.push(
                    RawDiag::new(
                        RULE,
                        format!(
                            "`{name}` is a build-time snapshot used as a reactive-looking \
                             `{kw}` {} — the branch will never update",
                            if kw == "match" { "scrutinee" } else { "condition" }
                        ),
                        *span,
                    )
                    .with_help(
                        "a component body runs once, so this `.get()` is frozen. For a \
                         live condition: `let … = memo(move || …)`, or inline the \
                         `.get()` into the `if` / `match`. If the snapshot is intentional, say \
                         so: `.peek()` (on a `Reactive<T>` prop, `.get_untracked()`).",
                    ),
                );
            }
        });
    }
}

/// `let x = …` / `let x: T = …` → `x`.
fn binding_ident(pat: &syn::Pat) -> Option<String> {
    match pat {
        syn::Pat::Ident(pi) => Some(pi.ident.to_string()),
        syn::Pat::Type(pt) => binding_ident(&pt.pat),
        _ => None,
    }
}

/// True when the expression contains a zero-arg `.get()` call outside
/// any closure (closure bodies are live-read positions, not snapshots).
fn has_bare_get(expr: &syn::Expr) -> bool {
    struct GetFinder {
        found: bool,
    }
    impl<'ast> Visit<'ast> for GetFinder {
        fn visit_expr_closure(&mut self, _: &'ast syn::ExprClosure) {
            // Don't descend: reads inside a closure are deferred, tracked
            // at run time by whoever calls the closure.
        }
        fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
            if node.method == "get" && node.args.is_empty() {
                self.found = true;
            }
            visit::visit_expr_method_call(self, node);
        }
    }
    let mut f = GetFinder { found: false };
    f.visit_expr(expr);
    f.found
}

/// Collects the token streams of every `ui!` / `jsx!` invocation in a
/// block (macro bodies are opaque to `syn`'s visitor, but the invocation
/// node exposes its raw tokens).
struct UiMacroFinder {
    streams: Vec<TokenStream>,
}
impl<'ast> Visit<'ast> for UiMacroFinder {
    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        if let Some(seg) = node.path.segments.last() {
            let name = seg.ident.to_string();
            if name == "ui" || name == "jsx" {
                self.streams.push(node.tokens.clone());
            }
        }
        visit::visit_macro(self, node);
    }
}

/// Lexical scan for `if [!]* IDENT {` and `match IDENT {` in a token
/// stream, recursing into every group so nested blocks are covered. Calls
/// `hit` with the condition identifier and the keyword.
fn scan_conditions(stream: TokenStream, hit: &mut impl FnMut(&str, &str)) {
    let tokens: Vec<TokenTree> = stream.into_iter().collect();
    let mut i = 0;
    while i < tokens.len() {
        if let TokenTree::Group(g) = &tokens[i] {
            scan_conditions(g.stream(), hit);
            i += 1;
            continue;
        }
        let is_match = matches!(&tokens[i], TokenTree::Ident(id) if id == "match");
        if is_match {
            if let (Some(TokenTree::Ident(cond)), Some(TokenTree::Group(body))) =
                (tokens.get(i + 1), tokens.get(i + 2))
            {
                if body.delimiter() == Delimiter::Brace {
                    hit(&cond.to_string(), "match");
                }
            }
        }
        let is_if = matches!(&tokens[i], TokenTree::Ident(id) if id == "if");
        if is_if {
            let mut j = i + 1;
            // Skip any leading `!` negations.
            while j < tokens.len()
                && matches!(&tokens[j], TokenTree::Punct(p) if p.as_char() == '!')
            {
                j += 1;
            }
            if let (Some(TokenTree::Ident(cond)), Some(TokenTree::Group(body))) =
                (tokens.get(j), tokens.get(j + 1))
            {
                if body.delimiter() == Delimiter::Brace && *cond != "let" {
                    hit(&cond.to_string(), "if");
                }
            }
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn diags(fn_tokens: proc_macro2::TokenStream) -> Vec<RawDiag> {
        let item: syn::ItemFn = syn::parse2(fn_tokens).unwrap();
        let mut out = Vec::new();
        check_fn(&item, &mut out);
        out
    }

    #[test]
    fn flags_the_canonical_trap() {
        let out = diags(quote! {
            #[component]
            fn A() -> Element {
                let too_short = name.get().len() < 3;
                ui! { if too_short { text { "x" } } }
            }
        });
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].message.contains("too_short"));
    }

    /// Regression: a hoisted snapshot as a `match` scrutinee lowers to a
    /// static `match` that builds once — the `if`-only scan missed it.
    #[test]
    fn regression_flags_a_snapshot_match_scrutinee() {
        let out = diags(quote! {
            #[component]
            fn A() -> Element {
                let gate = mode.get();
                ui! { view() { match gate { Mode::A => { text { "a" } } _ => { view() {} } } } }
            }
        });
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].message.contains("`match` scrutinee"), "{out:?}");
    }

    /// Inline reads and memos are live — clean.
    #[test]
    fn inline_get_and_memo_match_are_clean() {
        let out = diags(quote! {
            #[component]
            fn A() -> Element {
                let gate = memo(move || mode.get());
                ui! { view() { match mode.get() { _ => { view() {} } } match gate { _ => { view() {} } } } }
            }
        });
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn flags_negated_condition() {
        let out = diags(quote! {
            #[component]
            fn A() -> Element {
                let ok = name.get().len() >= 3;
                ui! { if !ok { text { "x" } } }
            }
        });
        assert_eq!(out.len(), 1, "{out:?}");
    }

    #[test]
    fn flags_nested_if_inside_ui_tree() {
        let out = diags(quote! {
            #[component]
            fn A() -> Element {
                let visible = flags.get().visible;
                ui! { view() { view() { if visible { text { "x" } } } } }
            }
        });
        assert_eq!(out.len(), 1, "{out:?}");
    }

    #[test]
    fn memo_initializer_is_clean() {
        // The reads live inside the closure — a live derivation.
        let out = diags(quote! {
            #[component]
            fn A() -> Element {
                let too_short = memo(move || name.get().len() < 3);
                ui! { if too_short { text { "x" } } }
            }
        });
        assert!(out.is_empty(), "{out:?}");
    }

    /// Declared intent, both spellings: `.peek()` is the untracked read
    /// on a `Signal`; `.get_untracked()` survives only on the
    /// `Reactive<T>` prop wrapper. The help text names `.peek()` first,
    /// so it had better be clean.
    #[test]
    fn peek_and_get_untracked_are_clean() {
        for read in [quote! { open.peek() }, quote! { props.mode.get_untracked() }] {
            let out = diags(quote! {
                #[component]
                fn A() -> Element {
                    let mode = #read;
                    ui! { if mode { text { "x" } } }
                }
            });
            assert!(out.is_empty(), "{read} → {out:?}");
        }
    }

    #[test]
    fn snapshot_not_used_as_condition_is_clean() {
        // Snapshots used for non-condition purposes are the legitimate
        // structural-choice idiom — out of scope for this rule.
        let out = diags(quote! {
            #[component]
            fn A() -> Element {
                let label = props.label.get();
                ui! { text { label } }
            }
        });
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn non_component_fn_is_ignored() {
        let out = diags(quote! {
            fn plain() -> Element {
                let ok = x.get();
                ui! { if ok { text { "x" } } }
            }
        });
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn arged_get_is_not_a_signal_read() {
        // HashMap::get(key) & co. take arguments — not the zero-arg
        // signal read shape.
        let out = diags(quote! {
            #[component]
            fn A() -> Element {
                let ok = map.get(&key).is_some();
                ui! { if ok { text { "x" } } }
            }
        });
        assert!(out.is_empty(), "{out:?}");
    }
}
