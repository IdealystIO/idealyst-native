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
//!   macro-built elements (the `rows.iter().map(…).collect()` shape).
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
    // macro (directly, or as a block's tail expression).
    if method == "map" && node.args.len() == 1 {
        if let syn::Expr::Closure(closure) = &node.args[0] {
            if let Some(kind) = tail_ui_macro_kind(&closure.body) {
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
        }
    }
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
