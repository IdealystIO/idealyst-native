//! `premint-state-keyed-sheet` / `premint-computed-layer` — the premint
//! crawl contract, caught at edit time.
//!
//! The premint dump mounts every literal route but never interacts, so a
//! runtime-registered sheet (`premint_as`, auto-preminted `r#static`) only
//! gets build-time CSS if some crawled code path CONSTRUCTS it. Two
//! author-side shapes defeat that silently and then detonate under
//! `--premint-only` (see docs/styling.md, "The crawl contract"):
//!
//! 1. **Sheet identity keyed on runtime state** — two sheets selected by a
//!    conditional (`if open { "panel.open" } else { "panel.closed" }`):
//!    the crawl constructs only the arm the initial mount takes, so the
//!    other arm's class has no CSS and panics UNCRAWLED on the user's
//!    first interaction (the AppShell drawer bug). Detected as an
//!    `if`/`match` inside the arguments of `premint_as` /
//!    `premint_with_class`, or inside the KEY argument of
//!    `cached_stylesheet`. The fix is ONE sheet with the state as a
//!    variant axis (`.with("open", "on")`), so every arm registers when
//!    the crawl constructs the sheet.
//!
//! 2. **A `with_computed` layer** — always a premint disqualifier (its
//!    rules are produced at runtime under a key the dump cannot
//!    enumerate), so the application panics at MOUNT under
//!    `--premint-only` — and it has exactly one slot, so two layers
//!    silently clobber each other even off premint. Enumerable state
//!    belongs on a variant axis, continuous/open-set per-instance values
//!    on the inline layer (`with_inline`), fully static rules in a
//!    `stylesheet!`.
//!
//! Precision notes: the conditional scan does NOT descend into closures —
//! `cached_stylesheet(KEY, move || { if let Some(axis) = … })` keeps its
//! conditionals inside the constructor body, where they run once per
//! cache key and are construction-order-safe. Only a conditional that
//! selects the identity/key itself is the trap. Both rules are runtime
//! backstopped (the minted-class guard warns under `--premint` and panics
//! under `--premint-only`), but those signals only fire on paths a dev
//! actually exercises — this is the static twin that catches the
//! untested path before it ships.

use syn::spanned::Spanned;
use syn::visit::Visit;

use crate::diagnostic::RawDiag;

pub(crate) const STATE_KEYED_RULE: &str = "premint-state-keyed-sheet";
pub(crate) const COMPUTED_RULE: &str = "premint-computed-layer";

/// Method-call leg: `.premint_as(…)` / `.premint_with_class(…)` with a
/// conditional in the argument, and any `.with_computed(…)`.
pub(crate) fn check_method_call(node: &syn::ExprMethodCall, out: &mut Vec<RawDiag>) {
    let method = node.method.to_string();
    match method.as_str() {
        "premint_as" | "premint_with_class" => {
            if node.args.iter().any(has_conditional_outside_closures) {
                out.push(
                    RawDiag::new(
                        STATE_KEYED_RULE,
                        format!(
                            "sheet identity selected by a runtime conditional — the premint \
                             dump's crawl only constructs the arm the initial mount takes, so \
                             the other arm's class gets no CSS (`{method}` panics UNCRAWLED \
                             under --premint-only on the user's first interaction)"
                        ),
                        node.span(),
                    )
                    .with_help(
                        "make it ONE sheet and move the state onto a variant axis: declare \
                         `.variant(\"open\", \"on\", …)` on the sheet and select it per \
                         evaluation with `.with(\"open\", \"on\")` — both arms then register \
                         while the crawl constructs the sheet. If the selector is a STATIC \
                         prop (never changes after build) and the sheet constructs at mount, \
                         this is safe — suppress with `// idealyst-lint-disable \
                         premint-state-keyed-sheet`. See docs/styling.md, \"The crawl \
                         contract\".",
                    ),
                );
            }
        }
        "with_computed" => {
            out.push(
                RawDiag::new(
                    COMPUTED_RULE,
                    "`with_computed` layer — a premint disqualifier (panics at mount under \
                     --premint-only) with exactly ONE slot (a second layer silently replaces \
                     the first)",
                    node.span(),
                )
                .with_help(
                    "enumerable state → a variant axis on the sheet; continuous or open-set \
                     per-instance values → the inline layer (`with_inline`); fully static \
                     rules → a `stylesheet!` declaration. See docs/styling.md, \"The crawl \
                     contract\".",
                ),
            );
        }
        _ => {}
    }
}

/// Call leg: `cached_stylesheet(KEY, ctor)` where the KEY is selected by a
/// conditional. (Conditionals inside the constructor closure are fine —
/// they run once per key; it's the key itself that must not be
/// state-selected.)
pub(crate) fn check_call(node: &syn::ExprCall, out: &mut Vec<RawDiag>) {
    let syn::Expr::Path(p) = &*node.func else { return };
    if super::last_segment(&p.path).as_deref() != Some("cached_stylesheet") {
        return;
    }
    let Some(key) = node.args.first() else { return };
    if has_conditional_outside_closures(key) {
        out.push(
            RawDiag::new(
                STATE_KEYED_RULE,
                "`cached_stylesheet` key selected by a runtime conditional — each key mints \
                 its own sheet, so this is N sheets keyed on state: the premint dump's crawl \
                 only constructs the arm the initial mount takes, and the others panic \
                 UNCRAWLED under --premint-only on the user's first interaction",
                node.span(),
            )
            .with_help(
                "cache ONE sheet under a state-independent key and move the state onto a \
                 variant axis selected per evaluation (`.with(\"open\", \"on\")`). See \
                 docs/styling.md, \"The crawl contract\".",
            ),
        );
    }
}

/// Does this expression contain an `if` / `match` anywhere OUTSIDE a
/// closure body? Closure internals run at constructor time under an
/// already-fixed key and are not the trap.
fn has_conditional_outside_closures(expr: &syn::Expr) -> bool {
    struct Finder {
        found: bool,
    }
    impl<'ast> Visit<'ast> for Finder {
        fn visit_expr_if(&mut self, _: &'ast syn::ExprIf) {
            self.found = true;
        }
        fn visit_expr_match(&mut self, _: &'ast syn::ExprMatch) {
            self.found = true;
        }
        fn visit_expr_closure(&mut self, _: &'ast syn::ExprClosure) {
            // Do not descend.
        }
    }
    let mut f = Finder { found: false };
    f.visit_expr(expr);
    f.found
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn method_diags(tokens: proc_macro2::TokenStream) -> Vec<RawDiag> {
        let expr: syn::Expr = syn::parse2(tokens).unwrap();
        let mut out = Vec::new();
        struct W<'a>(&'a mut Vec<RawDiag>);
        impl<'a, 'ast> Visit<'ast> for W<'a> {
            fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
                check_method_call(node, self.0);
                syn::visit::visit_expr_method_call(self, node);
            }
            fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
                check_call(node, self.0);
                syn::visit::visit_expr_call(self, node);
            }
        }
        W(&mut out).visit_expr(&expr);
        out
    }

    // The AppShell drawer bug, as written: identity picked by `if open`.
    #[test]
    fn flags_conditional_premint_identity() {
        let out = method_diags(quote! {
            sheet.premint_as(&premint_id(
                width,
                pin_axis,
                if open { "scrim.open" } else { "scrim.closed" },
            ))
        });
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].rule, STATE_KEYED_RULE);
    }

    #[test]
    fn flags_conditional_cache_key() {
        let out = method_diags(quote! {
            runtime_core::cached_stylesheet(
                cache_key(width, pin_axis, if open { 1 } else { 2 }),
                move || build(),
            )
        });
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].rule, STATE_KEYED_RULE);
    }

    // The FIXED AppShell shape: constant identity, conditionals only
    // inside the constructor closure (`if let Some(axis) = pin_axis`).
    #[test]
    fn allows_axis_spelling_with_closure_internals() {
        let out = method_diags(quote! {
            runtime_core::cached_stylesheet(cache_key(width, pin_axis, 1), move || {
                let mut sheet = StyleSheet::r#static(rules());
                if let Some(axis) = pin_axis {
                    sheet = sheet.variant(axis, "on", |_| overlay());
                }
                sheet.premint_as(&premint_id(width, pin_axis, "scrim"))
            })
        });
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn flags_with_computed() {
        let out = method_diags(quote! {
            StyleApplication::new(sheet).with_computed("tone", move || rules())
        });
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].rule, COMPUTED_RULE);
    }

    #[test]
    fn allows_with_inline_and_axes() {
        let out = method_diags(quote! {
            StyleApplication::new(sheet)
                .with("open", "on")
                .with_inline(StyleRules { ..Default::default() })
        });
        assert!(out.is_empty(), "{out:?}");
    }

    // `match` counts as a conditional identity selector too.
    #[test]
    fn flags_match_selected_identity() {
        let out = method_diags(quote! {
            sheet.premint_as(match side {
                Side::Start => "panel.start",
                Side::End => "panel.end",
            })
        });
        assert_eq!(out.len(), 1, "{out:?}");
    }
}
