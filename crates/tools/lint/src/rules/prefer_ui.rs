//! `prefer-ui-macro` — flag elements built by hand instead of through the
//! `ui!` / `jsx!` macro.
//!
//! Three hand-built shapes are caught, all high-precision (no guessing
//! from bare idents that might be unrelated locals):
//!
//! - `builder::view(…)`, `builder::text(…)`, … — any call qualified by a
//!   `builder` path segment.
//! - `BuildElement::build(props)` — the trait call the macro emits for
//!   component dispatch; writing it by hand is the manual form.
//! - `Element::View { … }`, `Element::Text { … }`, … — a struct literal of
//!   a primitive `Element` variant.
//!
//! `Element::External { … }` and `Element::Component { … }` are
//! deliberately **not** flagged: per the framework's core rules, `External`
//! is the blessed third-party-extension construction path and `Component`
//! is the macro's own wrapper — both are legitimate hand-written forms.
//!
//! Because `syn` never descends into `ui! { … }` token streams, every node
//! this rule sees is genuinely outside the macro.

use crate::diagnostic::RawDiag;
use crate::rules::{has_segment, last_segment, nth_from_end};

pub(crate) const RULE: &str = "prefer-ui-macro";

/// Element variants that are legitimate to construct by hand — the
/// extension escape hatches — and so are exempt from this rule.
const EXEMPT_VARIANTS: &[&str] = &["External", "Component"];

pub(crate) fn check_call(call: &syn::ExprCall, out: &mut Vec<RawDiag>) {
    let syn::Expr::Path(path_expr) = &*call.func else {
        return;
    };
    let path = &path_expr.path;

    if has_segment(path, "builder") {
        out.push(
            RawDiag::new(
                RULE,
                "building an element through `builder::…` bypasses the `ui!` macro",
                span_of(path_expr),
            )
            .with_help("compose the tree inside `ui! { … }` (or `jsx! { … }`) instead"),
        );
        return;
    }

    if last_segment(path).as_deref() == Some("build")
        && nth_from_end(path, 1).as_deref() == Some("BuildElement")
    {
        out.push(
            RawDiag::new(
                RULE,
                "calling `BuildElement::build` by hand bypasses the `ui!` macro",
                span_of(path_expr),
            )
            .with_help("render the component inside `ui! { … }` so reconciliation tracks it"),
        );
    }
}

pub(crate) fn check_struct(node: &syn::ExprStruct, out: &mut Vec<RawDiag>) {
    // Looking for `Element::Variant { … }`: the owner segment is `Element`
    // and the last segment is the variant name.
    if nth_from_end(&node.path, 1).as_deref() != Some("Element") {
        return;
    }
    let Some(variant) = last_segment(&node.path) else {
        return;
    };
    if EXEMPT_VARIANTS.contains(&variant.as_str()) {
        return;
    }
    out.push(
        RawDiag::new(
            RULE,
            format!("constructing `Element::{variant}` by hand bypasses the `ui!` macro"),
            span_of_struct(node),
        )
        .with_help("write this element inside `ui! { … }` (or `jsx! { … }`) instead"),
    );
}

fn span_of(path_expr: &syn::ExprPath) -> proc_macro2::Span {
    use syn::spanned::Spanned;
    path_expr.path.span()
}

fn span_of_struct(node: &syn::ExprStruct) -> proc_macro2::Span {
    use syn::spanned::Spanned;
    node.path.span()
}
