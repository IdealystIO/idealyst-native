//! `prefer-keyed-list` — a child list assembled as a `Vec<Element>` by hand
//! instead of through the `ui!` / `jsx!` macro's keyed `for … , key = …`.
//!
//! ```ignore
//! // WRONG — keys erased, positional reconciliation
//! let mut kids = Vec::new();
//! for row in rows { kids.push(ui! { Row(data = row) }); }
//! ui! { view() { { kids } } }
//!
//! // WRONG — same, via map/collect
//! let kids = rows.iter().map(|r| ui! { Row(data = r.clone()) }).collect();
//!
//! // RIGHT — the keyed loop lives inside the macro
//! ui! { view() { for row in rows, key = row.id { Row(data = row) } } }
//! ```
//!
//! Keys live **only** inside `Element::Each`, which the reactive `for …
//! , key = …` lowering is the sole constructor of. The moment elements are
//! collected into a `Vec<Element>` in plain Rust and splatted as children,
//! the key information is erased before the macro ever sees them, and the
//! walker reconciles them positionally by slot index. If the list is
//! dynamic (reorders, grows, shrinks), per-row state — component-local
//! signals, text-input focus, scroll position — silently attaches to the
//! wrong row. This is the "index as key" trap, caught at edit time.
//!
//! Detection is deliberately narrow (high precision over recall), keying on
//! a **visible `ui!` / `jsx!` macro node** used as the element source — the
//! same "no guessing from bare idents" bar as `prefer-ui-macro`:
//!
//! - `VEC.push(ui! { … })` / `.push(jsx! { … })` — pushing a macro-built
//!   element into a collection (`CLAUDE.md` §9.3's forbidden shape).
//! - `ITER.map(|x| ui! { … })` / `.map(|x| jsx! { … })` — mapping items to
//!   macro-built elements (the `rows.iter().map(…).collect()` shape). Only
//!   when the map is visibly over an ITERATOR: its receiver chain has an
//!   iterator source or iterator-only adapter (`.iter()`, `.into_iter()`,
//!   `.iter_mut()`, `.values()`, `.chars()`, `.enumerate()`, `.rev()`, …),
//!   is a range (`(0..n).map(…)`), or a `std::iter::repeat(…)`-style
//!   constructor — or the mapped result is `.collect()`ed. `Option::map` /
//!   `Result::map` (`props.icon.map(|i| ui! { … })`, `user.get().map(…)`)
//!   yield zero or one element: there is no list order to lose, and that is
//!   the idiomatic optional-child shape, so it is never flagged.
//!
//! Because `syn` never descends into `ui! { … }` token streams, a `.push` /
//! `.map` written *inside* a `ui!` block is invisible here — only genuinely
//! out-of-macro construction is flagged, so a keyed `for … , key = …`
//! inside the macro never trips it.
//!
//! **Out of scope (by design):** a list built from a helper that returns
//! `Element` without a visible macro at the call site — e.g.
//! `rows.iter().map(|r| gallery_card(r.clone())).collect()`. The helper's
//! return type is invisible to `syn`, so this rule cannot see it without
//! type inference. That fuzzier case is the `keyed-list-rendering` audit's
//! job (`.claude/audits/`), which reads with full context.

use crate::diagnostic::RawDiag;

pub(crate) const RULE: &str = "prefer-keyed-list";

/// Shared remediation help for both shapes.
const HELP: &str = "assemble child lists inside the macro with a keyed loop: \
`for item in items, key = item.id { … }`. A hand-built `Vec<Element>` is reconciled \
positionally, so per-row state (input focus, component-local signals, scroll) attaches to \
the wrong row when the list changes. If the list is genuinely static and never reorders, \
suppress with `// idealyst-lint-disable-line prefer-keyed-list`.";

pub(crate) fn check_method_call(node: &syn::ExprMethodCall, out: &mut Vec<RawDiag>) {
    use syn::spanned::Spanned;
    let method = node.method.to_string();

    // `VEC.push(ui!{…})` — one arg, and it is a `ui!` / `jsx!` macro.
    if method == "push" && node.args.len() == 1 {
        if let Some(kind) = ui_macro_kind(&node.args[0]) {
            out.push(
                RawDiag::new(
                    RULE,
                    format!(
                        "pushing a `{kind}` element into a `Vec` builds a child list \
                         outside the macro — reconciliation keys are erased"
                    ),
                    node.args[0].span(),
                )
                .with_help(HELP),
            );
        }
        return;
    }

    // `ITER.map(|x| ui!{…})` — one closure arg whose body is a `ui!` / `jsx!`
    // macro (directly, or as a block's tail expression), on a receiver that
    // is visibly an iterator. `Option::map` / `Result::map` share the
    // spelling but produce zero-or-one element — nothing to key (#27).
    if method == "map" {
        if let Some((closure, kind)) = ui_map_closure(node) {
            if is_iterator_chain(&node.receiver) {
                push_map_diag(closure, kind, out);
            }
        }
        return;
    }

    // `X.map(|x| ui!{…}).collect()` — the receiver gave no iterator
    // evidence (a bare `rows`), but collecting the mapped elements is a
    // list by construction. Only the map WITHOUT receiver evidence is
    // reported here; one that has it was already reported by the `map`
    // arm above when the visitor reached it.
    if method == "collect" {
        if let syn::Expr::MethodCall(map) = strip_parens(&node.receiver) {
            if map.method == "map" && !is_iterator_chain(&map.receiver) {
                if let Some((closure, kind)) = ui_map_closure(map) {
                    push_map_diag(closure, kind, out);
                }
            }
        }
    }
}

/// `.map(|x| ui!{…})` → its closure and the macro kind the body evaluates to.
fn ui_map_closure(node: &syn::ExprMethodCall) -> Option<(&syn::ExprClosure, &'static str)> {
    if node.args.len() != 1 {
        return None;
    }
    let syn::Expr::Closure(closure) = &node.args[0] else { return None };
    tail_ui_macro_kind(&closure.body).map(|kind| (closure, kind))
}

fn push_map_diag(closure: &syn::ExprClosure, kind: &'static str, out: &mut Vec<RawDiag>) {
    use syn::spanned::Spanned;
    out.push(
        RawDiag::new(
            RULE,
            format!(
                "mapping items to `{kind}` elements builds a child list \
                 outside the macro — reconciliation keys are erased"
            ),
            closure.body.span(),
        )
        .with_help(HELP),
    );
}

/// Methods that only an iterator-producing chain has: the collection →
/// iterator sources (`iter`, `into_iter`, `values`, `chars`, …) and the
/// adapters `Option` / `Result` do NOT define. Deliberately absent, because
/// `Option` has them too: `filter`, `cloned`, `copied`, `take`, `zip`,
/// `inspect`, `flatten`, `map`, `as_ref` — a chain made only of those
/// (`opt.cloned().map(…)`) is not evidence of a list.
const ITERATOR_METHODS: &[&str] = &[
    "iter",
    "iter_mut",
    "into_iter",
    "into_values",
    "into_keys",
    "keys",
    "values",
    "values_mut",
    "drain",
    "chars",
    "char_indices",
    "bytes",
    "lines",
    "split",
    "split_whitespace",
    "windows",
    "chunks",
    "chunks_exact",
    "enumerate",
    "rev",
    "skip",
    "skip_while",
    "take_while",
    "step_by",
    "chain",
    "peekable",
    "cycle",
    "flat_map",
    "filter_map",
];

/// `std::iter::{repeat, once, …}` — free constructors that yield iterators.
const ITER_CONSTRUCTORS: &[&str] =
    &["repeat", "repeat_with", "once", "once_with", "from_fn", "successors", "empty"];

/// Is `expr` visibly an iterator? Walks the receiver chain
/// (`a.b().c()` → `a.b()` → `a`) looking for an [`ITERATOR_METHODS`] call,
/// a range (`0..n`, `(0..=n)`), or a `iter::repeat(…)`-style constructor.
/// Syntactic only — `syn` has no types — so it errs toward "not an
/// iterator"; the `.collect()` arm recovers the untyped-receiver lists.
fn is_iterator_chain(expr: &syn::Expr) -> bool {
    let mut cur = strip_parens(expr);
    loop {
        match cur {
            syn::Expr::Range(_) => return true,
            syn::Expr::MethodCall(m) => {
                if ITERATOR_METHODS.contains(&m.method.to_string().as_str()) {
                    return true;
                }
                cur = strip_parens(&m.receiver);
            }
            syn::Expr::Call(c) => {
                let syn::Expr::Path(p) = &*c.func else { return false };
                let segs = &p.path.segments;
                let n = segs.len();
                return n >= 2
                    && segs[n - 2].ident == "iter"
                    && ITER_CONSTRUCTORS.contains(&segs[n - 1].ident.to_string().as_str());
            }
            syn::Expr::Reference(r) => cur = strip_parens(&r.expr),
            _ => return false,
        }
    }
}

fn strip_parens(mut expr: &syn::Expr) -> &syn::Expr {
    while let syn::Expr::Paren(p) = expr {
        expr = &p.expr;
    }
    expr
}

/// `ui! { … }` → `"ui!"`, `jsx! { … }` → `"jsx!"`, anything else → `None`.
fn ui_macro_kind(expr: &syn::Expr) -> Option<&'static str> {
    let syn::Expr::Macro(m) = expr else { return None };
    match m.mac.path.segments.last()?.ident.to_string().as_str() {
        "ui" => Some("ui!"),
        "jsx" => Some("jsx!"),
        _ => None,
    }
}

/// The macro kind an expression *evaluates to*: the expression itself when
/// it is a `ui!` / `jsx!` macro, or a block's final tail expression when it
/// is a `{ …; ui! { … } }` body. One level of block unwrapping covers the
/// common `|x| { let y = …; ui! { … } }` closure shape without chasing
/// arbitrary control flow (kept narrow for precision).
fn tail_ui_macro_kind(expr: &syn::Expr) -> Option<&'static str> {
    if let Some(kind) = ui_macro_kind(expr) {
        return Some(kind);
    }
    let syn::Expr::Block(block) = expr else { return None };
    match block.block.stmts.last()? {
        syn::Stmt::Expr(tail, _) => ui_macro_kind(tail),
        // A tail `ui! { … }` with no trailing `;` parses as a statement
        // macro, not an `Expr::Macro`.
        syn::Stmt::Macro(m) => match m.mac.path.segments.last()?.ident.to_string().as_str() {
            "ui" => Some("ui!"),
            "jsx" => Some("jsx!"),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn diags(fn_tokens: proc_macro2::TokenStream) -> Vec<RawDiag> {
        // Route through the shared visitor so the wiring in `mod.rs`
        // (visit_expr_method_call → check_method_call) is exercised too.
        let item: syn::ItemFn = syn::parse2(fn_tokens).unwrap();
        let file = syn::File { shebang: None, attrs: Vec::new(), items: vec![item.into()] };
        crate::rules::collect(&file)
            .into_iter()
            .filter(|d| d.rule == RULE)
            .collect()
    }

    #[test]
    fn flags_push_of_ui_macro() {
        let out = diags(quote! {
            fn build() -> Vec<Element> {
                let mut kids = Vec::new();
                for row in rows {
                    kids.push(ui! { Row(data = row) });
                }
                kids
            }
        });
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].message.contains("ui!"), "{out:?}");
    }

    #[test]
    fn flags_push_of_jsx_macro() {
        let out = diags(quote! {
            fn build() -> Vec<Element> {
                let mut kids = Vec::new();
                kids.push(jsx! { <Row /> });
                kids
            }
        });
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].message.contains("jsx!"), "{out:?}");
    }

    #[test]
    fn flags_map_to_ui_macro() {
        let out = diags(quote! {
            fn build() -> Vec<Element> {
                rows.iter().map(|r| ui! { Row(data = r.clone()) }).collect()
            }
        });
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].message.contains("mapping"), "{out:?}");
    }

    /// The reported bug (#27): ANY `.map(|x| ui!{…})` was flagged, so the
    /// idiomatic optional-element shape — `Option::map` producing zero or
    /// one element — was reported as a hand-built child list. An `Option`
    /// has no order to lose; only an iterator chain builds a list.
    #[test]
    fn regression_option_map_to_ui_macro_is_clean() {
        let out = diags(quote! {
            fn build() -> Option<Element> {
                let a = props.icon.map(|i| ui! { icon(data = i) });
                let b = user.get().map(|u| ui! { text { "{u.name}" } });
                let c = self.subtitle.as_ref().map(|s| { let s = s.clone(); ui! { text { "{s}" } } });
                let d = opt.cloned().map(|x| ui! { Row(data = x) }).unwrap_or_else(|| ui! { view() {} });
                let e = result.ok().map(|v| jsx! { <Row data={v} /> });
                a
            }
        });
        assert!(out.is_empty(), "{out:?}");
    }

    /// Iterator evidence anywhere in the receiver chain — an iterator
    /// source (`iter`, `into_iter`, `iter_mut`, `chars`, `values`, …), an
    /// iterator-only adapter (`enumerate`, `rev`, `skip`, …), a range, a
    /// `std::iter` constructor — still flags.
    #[test]
    fn iterator_receiver_chains_are_flagged() {
        for src in [
            quote! { fn f() { rows.into_iter().map(|r| ui! { Row(data = r) }); } },
            quote! { fn f() { rows.iter_mut().map(|r| ui! { Row(data = r) }); } },
            quote! { fn f() { rows.iter().enumerate().map(|(i, r)| ui! { Row(i = i) }); } },
            quote! { fn f() { rows.iter().filter(|r| r.on).cloned().map(|r| ui! { Row(data = r) }); } },
            quote! { fn f() { map.values().map(|r| ui! { Row(data = r.clone()) }); } },
            quote! { fn f() { (0..n).map(|i| ui! { Cell(i = i) }); } },
            quote! { fn f() { (0..=n).rev().map(|i| ui! { Cell(i = i) }); } },
            quote! { fn f() { std::iter::repeat(x).take(3).map(|x| ui! { Cell(x = x) }); } },
        ] {
            let out = diags(src.clone());
            assert_eq!(out.len(), 1, "{src} → {out:?}");
        }
    }

    /// A receiver with no iterator evidence (`rows` could be anything) is
    /// still a list when the result is `.collect()`ed — one finding, on
    /// the map, not two.
    #[test]
    fn map_then_collect_without_iterator_evidence_is_flagged() {
        let out = diags(quote! {
            fn build() -> Vec<Element> {
                let a: Vec<Element> = rows.map(|r| ui! { Row(data = r) }).collect();
                let b = rows.map(|r| ui! { Row(data = r) }).collect::<Vec<_>>();
                let c: Vec<Element> = rows.iter().map(|r| ui! { Row(data = r) }).collect();
                a
            }
        });
        assert_eq!(out.len(), 3, "{out:?}");
    }

    #[test]
    fn flags_map_with_block_body_tail_macro() {
        let out = diags(quote! {
            fn build() -> Vec<Element> {
                rows.iter().map(|r| {
                    let data = r.clone();
                    ui! { Row(data = data) }
                }).collect()
            }
        });
        assert_eq!(out.len(), 1, "{out:?}");
    }

    #[test]
    fn keyed_for_inside_macro_is_clean() {
        // The `.push` / `.map` here live INSIDE `ui!`, which `syn` never
        // descends into — so nothing is visible to flag. This is the
        // correct form and must not trip the rule.
        let out = diags(quote! {
            fn build() -> Element {
                ui! {
                    view() {
                        for row in rows, key = row.id {
                            Row(data = row)
                        }
                    }
                }
            }
        });
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn push_of_non_element_is_clean() {
        // Pushing a plain value (not a `ui!` / `jsx!` macro) is ordinary
        // Rust — e.g. collecting data, ids, tuples.
        let out = diags(quote! {
            fn build() -> Vec<u32> {
                let mut ids = Vec::new();
                ids.push(row.id);
                ids
            }
        });
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn map_to_non_macro_is_clean() {
        // The `gallery_card` helper case: no visible macro at the map site,
        // so this rule stays silent (it is the audit's domain, not the
        // linter's — see the module docs).
        let out = diags(quote! {
            fn build() -> Vec<Element> {
                rows.iter().map(|r| gallery_card(r.clone())).collect()
            }
        });
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn received_children_flatten_is_clean() {
        // The canonical legit `Vec<Element>` shape (Card / Center):
        // flatten a RECEIVED children param via `append_to`. No `ui!`
        // macro, no `.push(ui!)`, no `.map(|_| ui!)` — must not flag.
        let out = diags(quote! {
            fn Center(props: CenterProps) -> Element {
                let mut children = Vec::with_capacity(props.children.len());
                for c in props.children {
                    ChildList::append_to(c, &mut children);
                }
                ui! { view() { children } }
            }
        });
        assert!(out.is_empty(), "{out:?}");
    }
}
