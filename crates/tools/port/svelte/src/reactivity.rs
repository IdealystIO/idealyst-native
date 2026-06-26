//! Svelte's compile-time reactivity analysis.
//!
//! Svelte has no `useState`-like primitive. The compiler classifies
//! every top-level `let` binding in `<script>` based on how it's
//! used downstream:
//!
//! - written *and* read in the template, or written from event
//!   handlers, or appears as a dependency of a reactive
//!   statement → **reactive let**, lowers to `signal!`.
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
    /// elsewhere → `signal!(0)`.
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
/// - `count++`            → `count.update(|v| *v += 1)`
/// - `count += 5`         → `count.update(|v| *v += 5)`
///
/// Returns `None` when the assignment shape isn't one of the
/// recognized cases — caller should emit a hole.
pub fn rewrite_assignment(name: &str, op: &str, rhs: Option<&str>) -> Option<String> {
    match (op, rhs) {
        ("=", Some(rhs)) => Some(format!("{}.set({})", name, rhs)),
        ("++", None) => Some(format!("{}.update(|v| *v += 1)", name)),
        ("--", None) => Some(format!("{}.update(|v| *v -= 1)", name)),
        ("+=", Some(rhs)) => Some(format!("{name}.update(|v| *v += {rhs})", name = name, rhs = rhs)),
        ("-=", Some(rhs)) => Some(format!("{name}.update(|v| *v -= {rhs})", name = name, rhs = rhs)),
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

    #[test]
    fn rewrite_increment() {
        assert_eq!(
            rewrite_assignment("count", "++", None).as_deref(),
            Some("count.update(|v| *v += 1)"),
        );
    }

    #[test]
    fn rewrite_unknown_returns_none() {
        assert_eq!(rewrite_assignment("count", "??=", Some("x")), None);
    }
}
