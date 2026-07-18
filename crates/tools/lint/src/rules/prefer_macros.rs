//! Rules that flag drift from the canonical reactive authoring surface:
//!
//! - `Signal::new(v)` / `signal!(v)` → `signal(v)` (`prefer-signal-fn`)
//! - `memo!(…)` → `memo(move || …)`   (`prefer-memo-fn`)
//! - `Effect::new(|| …)` → `effect!{ … }` (`prefer-effect-macro`)
//!
//! Signal creation and memoization are plain **functions** — `signal(value)`
//! and `memo(move || …)` — because neither needs token manipulation:
//! scope anchoring happens inside the constructors, and the memo author
//! writes the closure anyway. Their macros were removed (`signal!`
//! expanded verbatim to `Signal::new`; `memo!` only inserted `move ||`),
//! so an invocation of either won't compile; `Signal::new` still works
//! but is the redundant spelling. `effect!` remains a macro on merit: it
//! wraps a bare BLOCK in a `move` closure and expands to the scope-owned
//! `Effect::scoped` (the raw `Effect::new` constructor is sealed).
//!
//! Detection is path-shaped and works regardless of import prefix:
//! `Signal::new`, `runtime_core::Signal::new`, and `reactive::Signal::new`
//! all match. Macro *bodies* are token blobs `syn` never descends into,
//! so `ui! { … }` contents are invisible here — but macro *invocations*
//! themselves are visible nodes, which is how leftover `signal!(…)` /
//! `memo!(…)` calls are caught (with a better message than rustc's
//! "cannot find macro").

use crate::diagnostic::RawDiag;
use crate::rules::{last_segment, nth_from_end};

pub(crate) const SIGNAL_RULE: &str = "prefer-signal-fn";
pub(crate) const EFFECT_RULE: &str = "prefer-effect-macro";
pub(crate) const MEMO_RULE: &str = "prefer-memo-fn";
pub(crate) const TEXT_FSTRING_RULE: &str = "prefer-text-fstring";

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
                    "`Signal::new` is the redundant spelling of signal creation",
                    span_of(path_expr),
                )
                .with_help(
                    "use `signal(value)` — the canonical creation form (identical \
                     behavior; `signal` is the plain function re-exported at the \
                     crate root)",
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
                    "inside a component use `effect! { … }` (the scope owns it); outside the \
                     tree use `watch(…)` and hold the returned `Subscription`. The raw \
                     `Effect::new` constructor is sealed.",
                ),
            );
        }
        // No arm for the `memo(…)` free function: it IS the canonical
        // form (the old `prefer-memo-macro` rule flagged it toward the
        // since-removed `memo!` — that direction inverted with the
        // macro's removal; leftover `memo!` invocations are caught in
        // `check_macro` below).
        _ => {}
    }
}

/// Flag invocations of the removed `signal!` / `memo!` macros. They no
/// longer exist in runtime-core, so rustc will also error — but with an
/// unhelpful "cannot find macro `signal` in this scope" (the *function*
/// of the same name IS in scope). These diagnostics supply the actual fix.
pub(crate) fn check_macro(mac: &syn::Macro, out: &mut Vec<RawDiag>) {
    use syn::spanned::Spanned;
    match last_segment(&mac.path).as_deref() {
        Some("signal") => {
            out.push(
                RawDiag::new(
                    SIGNAL_RULE,
                    "the `signal!` macro was removed",
                    mac.path.span(),
                )
                .with_help(
                    "drop the `!` — signal creation is the plain function `signal(value)`",
                ),
            );
        }
        Some("memo") => {
            out.push(
                RawDiag::new(
                    MEMO_RULE,
                    "the `memo!` macro was removed",
                    mac.path.span(),
                )
                .with_help(
                    "write the closure yourself — `memo(move || …)` is the plain-fn form \
                     (the macro only inserted the `move ||`)",
                ),
            );
        }
        Some("text_fmt") => {
            out.push(
                RawDiag::new(
                    TEXT_FSTRING_RULE,
                    "the `text_fmt!` macro was removed",
                    mac.path.span(),
                )
                .with_help(
                    "interpolate in the text literal itself — `text { \"count: {count}\" }`. \
                     Slots are live or static by the value's TYPE (signals subscribe, \
                     `Display` values bake in) and signal slots produce the same optimized \
                     web binding `text_fmt!` did. For positional/Debug formatting use \
                     `text { move || format!(…) }`.",
                ),
            );
        }
        Some("bind") => {
            out.push(
                RawDiag::new(
                    TEXT_FSTRING_RULE,
                    "the `bind!` sentinel was removed with `text_fmt!`",
                    mac.path.span(),
                )
                .with_help(
                    "name the signal in a text f-string instead — `text { \"g={global}\" }` \
                     subscribes because `global` is a signal (reactivity by type, no marker)",
                ),
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
