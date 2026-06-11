//! Rules that flag the *raw* reactive primitives where an idealyst macro
//! is the intended authoring surface:
//!
//! - `Signal::new(v)` → `signal!(v)`   (`prefer-signal-macro`)
//! - `Effect::new(|| …)` → `effect!{ … }` (`prefer-effect-macro`)
//! - `memo(|| …)` → `memo!(…)`          (`prefer-memo-macro`)
//!
//! The macros aren't sugar — they anchor the created handle to the owning
//! reactive scope (so it isn't dropped early) and let the reactivity
//! rewrite see the dependency. Calling the raw constructor skips that, the
//! classic "my signal stopped updating" footgun.
//!
//! Detection is path-shaped and works regardless of import prefix:
//! `Signal::new`, `runtime_core::Signal::new`, and `reactive::Signal::new`
//! all match. Anything written inside a `signal!( … )` invocation is a
//! token blob `syn` never descends into, so legitimate macro use is never
//! flagged.

use crate::diagnostic::RawDiag;
use crate::rules::{last_segment, nth_from_end};

pub(crate) const SIGNAL_RULE: &str = "prefer-signal-macro";
pub(crate) const EFFECT_RULE: &str = "prefer-effect-macro";
pub(crate) const MEMO_RULE: &str = "prefer-memo-macro";

pub(crate) fn check_call(call: &syn::ExprCall, out: &mut Vec<RawDiag>) {
    let syn::Expr::Path(path_expr) = &*call.func else {
        return;
    };
    let path = &path_expr.path;
    let last = last_segment(path);
    let owner = nth_from_end(path, 1);

    match (owner.as_deref(), last.as_deref()) {
        (Some("Signal"), Some("new")) => {
            out.push(
                RawDiag::new(
                    SIGNAL_RULE,
                    "creating a signal with `Signal::new` bypasses the `signal!` macro",
                    span_of(path_expr),
                )
                .with_help(
                    "use `signal!(value)` — it anchors the signal to the owning scope so it \
                     isn't dropped before its subscribers run",
                ),
            );
        }
        (Some("Effect"), Some("new")) => {
            out.push(
                RawDiag::new(
                    EFFECT_RULE,
                    "creating an effect with `Effect::new` bypasses the `effect!` macro",
                    span_of(path_expr),
                )
                .with_help(
                    "use `effect! { … }` — it binds the effect handle to the surrounding scope \
                     so it keeps running instead of being dropped immediately",
                ),
            );
        }
        // The `memo(…)` free function — match the trailing segment so an
        // imported `memo` and a qualified `reactive::memo` both fire. The
        // `memo!` macro itself expands to this call, but macro bodies are
        // never visited, so only hand-written calls reach here.
        (_, Some("memo")) => {
            out.push(
                RawDiag::new(
                    MEMO_RULE,
                    "calling the `memo` function directly bypasses the `memo!` macro",
                    span_of(path_expr),
                )
                .with_help("use `memo!(|| … )` so the derived value tracks its dependencies"),
            );
        }
        _ => {}
    }
}

/// Point the span at the callee path (the `Signal::new` tokens) rather
/// than the whole call including its argument list.
fn span_of(path_expr: &syn::ExprPath) -> proc_macro2::Span {
    use syn::spanned::Spanned;
    path_expr.path.span()
}
