//! One syntactic predicate shared by the split pass and the emission.

use syn::Expr;

/// Does `expr` have the "reactive call" shape — a single-segment
/// function call whose every argument is a bare single-segment path
/// (a signal reference)? This is the SAME syntactic shape
/// `try_emit_derived_call` / `try_emit_structured_match` accept, and
/// it is the shape that, by contract, reads signals: the structured
/// lowering calls `(arg).get()` on each argument.
///
/// `condition_is_reactive` only fires on a literal `.get()` substring,
/// so a scrutinee like `key(state)` (the signal read is hidden inside
/// `key`, the args are bare `Signal`s) is NOT caught by it. For `if`
/// this didn't matter — `if key(state) { … }` is always claimed by the
/// structured `try_emit_derived_call::<bool>` path, which makes it
/// reactive. But `match`'s structured path requires *literal* arm
/// keys, so `match key(state) { Enum::A => …, _ => … }` (enum/non-literal
/// arms) fell through `try_emit_structured_match` AND past
/// `condition_is_reactive`, landing on the static plain-`match` arm —
/// it built once and never re-ran when `state` changed. This predicate
/// closes that gap so the closure-`switch` reactive path also claims
/// the bare-call shape (matching `if`'s behavior).
pub fn is_reactive_call_shape(expr: &Expr) -> bool {
    let call = match expr {
        Expr::Call(c) => c,
        _ => return false,
    };
    // Function position must be a single-segment path with no generic args.
    match &*call.func {
        Expr::Path(syn::ExprPath { qself: None, path, .. }) => {
            if path.segments.len() != 1 || !path.segments[0].arguments.is_empty() {
                return false;
            }
        }
        _ => return false,
    }
    if call.args.is_empty() {
        return false;
    }
    // Every arg must be a bare single-segment path (a signal reference).
    call.args.iter().all(|a| {
        matches!(
            a,
            Expr::Path(syn::ExprPath { qself: None, path, .. })
                if path.segments.len() == 1 && path.segments[0].arguments.is_empty()
        )
    })
}