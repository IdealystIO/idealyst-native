//! `Signal::dispose` — the explicit-free path for legitimately-unowned
//! signals (per-item state in dynamic collections, created in handlers /
//! async where no reactive scope is active).
//!
//! Regression context: the arena's adversarial review of todo-app run-3
//! (2026-07-21) found per-item `done` signals created in add-handlers
//! leaking permanently on every add/delete cycle, with no framework-provided
//! way to free them and no diagnostic that it was happening. `dispose()` is
//! the fix's API half; the once-per-site dev warning is the diagnostic half
//! (exercised implicitly here — these tests create unowned signals, and the
//! suite must not spam: one line per creation site).

use runtime_shared::{arena_stats, reactive, signal};

#[test]
fn regression_unowned_signal_leaks_until_disposed() {
    // The bug: signals born outside any scope leaked with no recourse.
    let baseline = arena_stats().signals_in_use;
    let s = signal(41_u32);
    assert_eq!(
        arena_stats().signals_in_use,
        baseline + 1,
        "unowned signal occupies a slot"
    );
    s.dispose();
    assert_eq!(
        arena_stats().signals_in_use,
        baseline,
        "dispose frees the slot immediately"
    );
}

#[test]
fn double_dispose_is_a_silent_noop() {
    // Mirrors the stale-set philosophy: a stale handle's dispose must not
    // touch whatever signal recycled the slot.
    let a = signal(1_u32);
    let a_copy = a; // Copy handle, same slot + generation.
    a.dispose();
    // Recycle the slot (arena reuses freed slots with a bumped generation).
    let b = signal(2_u32);
    a_copy.dispose(); // stale: generation mismatch → must NOT free b's slot
    assert_eq!(b.get_untracked(), 2, "recycled slot's occupant survives a stale dispose");
    b.dispose();
}

#[test]
fn writes_through_a_disposed_handle_noop() {
    let s = signal(7_u32);
    let stale = s;
    s.dispose();
    // Standard stale-handle write semantics: skip, don't panic, don't alias.
    stale.set(99);
    let fresh = signal(3_u32);
    assert_eq!(fresh.get_untracked(), 3);
    fresh.dispose();
}

#[test]
fn scope_owned_signal_freed_by_scope_after_early_dispose_is_safe() {
    // An early dispose of a scope-owned signal must not double-free when the
    // scope later drops: the scope's batched free hits a stale generation
    // and skips, same guard as everything else.
    //
    // RELOCATION NOTE: the old body opened the owning scope by RENDERING a
    // `ui! { view {} }` through the walker's `TestRuntime`. The scope was
    // incidental scenery — the subject is `reactive::Scope`'s batched free,
    // which lives here. Driving `Scope` directly exercises the SAME free
    // path with the walker removed from the picture.
    let mut scope = reactive::Scope::new();
    reactive::with_scope(&mut scope, || {
        let s = signal(10_u32);
        s.dispose(); // early free while the owning scope still lives
    });
    // The critical properties: the scope's batched free hits the disposed
    // slot's stale generation and SKIPS (no double-free, no panic)…
    drop(scope);
    // …and the arena is uncorrupted afterwards: a fresh signal on a recycled
    // slot reads back its own value.
    let canary = signal(123_u32);
    assert_eq!(canary.get_untracked(), 123, "arena intact after early-dispose + scope drop");
    canary.dispose();
}
