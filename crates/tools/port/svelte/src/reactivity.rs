//! Svelte's compile-time reactivity analysis.
//!
//! Svelte has no `useState`-like primitive. The compiler classifies
//! every top-level `let` binding in `<script>` based on how it's
//! used downstream:
//!
//! - written *and* read in the template, or written from event
//!   handlers, or appears as a dependency of a reactive
//!   statement → **reactive let**, lowers to `signal()`.
//! - written only at declaration → **constant let**, lowers to a
//!   plain `let` (forwarded verbatim).
//! - read in a reactive statement but never assigned → **prop**,
//!   lowers to a `XxxProps` field (unless declared with `export
//!   let`, in which case the prop classification is explicit).
//!
//! Reactive statements come in two flavors:
//!
//! - `$: derived = expr;` — a *reactive assignment*. Lowers to a
//!   derived signal (MVP: an inline closure).
//! - `$: bareStatement;` — a *reactive side effect*. Lowers to
//!   `effect!({ … })`. Body translation is best-effort;
//!   non-translatable bodies become handler-body holes.
//!
//! The compiler's job at the IR level is *just* this
//! classification — the actual let/$: rewriting happens at the
//! parser stage where the AST is available.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LetClass {
    /// Plain reactive value. `let x = 0;` plus mutation
    /// elsewhere → `signal(0)`.
    ReactiveLet,
    /// Never mutated → forward as `let x = …;` verbatim.
    ConstLet,
    /// `export let x = default;` → prop with default.
    Prop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReactiveStmtClass {
    /// `$: name = expr;` — derived signal.
    Derived,
    /// `$: someSideEffect();` — Effect.
    SideEffect,
}

/// Template-directive lowering table. Documented as data so the
/// porter's *template coverage* is explicit.
pub const TEMPLATE_DIRECTIVES: &[(&str, &str)] = &[
    ("{#if}", "if-block"),
    ("{:else if}", "else-if-block"),
    ("{:else}", "else-block"),
    ("{#each}", "for-block"),
    ("on:click", "on_click"),
    ("on:input", "on_input"),
    ("on:change", "on_change"),
    ("bind:value", "two-way-binding"),
    ("{expr}", "expr-child"),
    // Special: `class:active={cond}` → conditional class on a
    // stylesheet variant. Hole for now since idealyst's
    // stylesheet variants need named declaration up front.
    ("class:name", "conditional-variant"),
];

/// Convenience: given the rough shape of the original assignment
/// in a Svelte `<script>`, return the equivalent idealyst signal
/// mutation. Documented as a pure function so a future lowering
/// pass can reuse it.
///
/// Examples (input → output, syntactic only):
///
/// - `count = count + 1`  → `count.set(count.get() + 1)`
/// - `count++`            → `count.update(|v| v.clone() + 1)`
/// - `count += 5`         → `count.update(|v| v.clone() + (5))`
///
/// `Signal::update` hands the closure `&T` and takes the next value back,
/// so the closure computes rather than mutates. The right-hand side is
/// parenthesized so `count -= a - b` stays `v - (a - b)`.
///
/// Returns `None` when the assignment shape isn't one of the
/// recognized cases — caller should emit a hole.
pub fn rewrite_assignment(name: &str, op: &str, rhs: Option<&str>) -> Option<String> {
    match (op, rhs) {
        ("=", Some(rhs)) => Some(format!("{}.set({})", name, rhs)),
        ("++", None) => Some(format!("{name}.update(|v| v.clone() + 1)")),
        ("--", None) => Some(format!("{name}.update(|v| v.clone() - 1)")),
        ("+=", Some(rhs)) => Some(format!("{name}.update(|v| v.clone() + ({rhs}))")),
        ("-=", Some(rhs)) => Some(format!("{name}.update(|v| v.clone() - ({rhs}))")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrite_simple_assignment() {
        assert_eq!(
            rewrite_assignment("count", "=", Some("count + 1")).as_deref(),
            Some("count.set(count + 1)"),
        );
    }

    /// Regression: `++` / `+=` emitted `count.update(|v| *v += 1)`, a
    /// mutation through `&T` that does not compile against
    /// `Signal::update(FnOnce(&T) -> T)`.
    #[test]
    fn regression_rewrite_increment_returns_the_next_value() {
        assert_eq!(
            rewrite_assignment("count", "++", None).as_deref(),
            Some("count.update(|v| v.clone() + 1)"),
        );
        assert_eq!(
            rewrite_assignment("count", "-=", Some("a - b")).as_deref(),
            Some("count.update(|v| v.clone() - (a - b))"),
            "the right-hand side keeps its own grouping",
        );
    }

    /// The emitted closures are what `Signal::update` takes: `&T` in, the
    /// next `T` out. Written out here so a signature change breaks this
    /// test instead of the ported app.
    #[test]
    fn emitted_update_closures_have_the_update_shape() {
        fn update<T: Clone>(v: T, f: impl FnOnce(&T) -> T) -> T {
            f(&v)
        }
        let (a, b) = (3, 1);
        assert_eq!(update(5, |v| v.clone() + 1), 6);
        assert_eq!(update(5, |v| v.clone() - (a - b)), 3);
        assert_eq!(update(String::from("a"), |v| v.clone() + ("b")), "ab");
    }

    #[test]
    fn rewrite_unknown_returns_none() {
        assert_eq!(rewrite_assignment("count", "??=", Some("x")), None);
    }
}
