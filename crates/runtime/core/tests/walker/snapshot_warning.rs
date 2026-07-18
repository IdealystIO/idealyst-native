//! The untracked-build-read diagnostic (hoisted-snapshot trap).
//!
//! `let ok = x.get();` at component-body level runs once and freezes,
//! while looking reactive. Debug builds warn at runtime when a
//! `Signal::get` happens during a `#[component]` build with no tracked
//! consumer and no declared snapshot intent. These tests pin the
//! predicate through the `__take_untracked_build_read_warnings` hook:
//! it must FIRE on the trap (at the root AND inside a rebuilt `when`
//! branch — the walker's `untrack_for_build` split exists for exactly
//! that) and stay SILENT for every legitimate read shape.
//!
//! Debug-assertions-only, like the diagnostic itself.
#![cfg(debug_assertions)]

use runtime_core::{
    component, signal, text, ui, untrack, Element, Signal,
    __take_untracked_build_read_warnings as take_warnings,
};

use crate::common::TestRuntime;

fn warned_components() -> Vec<&'static str> {
    take_warnings().into_iter().map(|(c, _)| c).collect()
}

/// THE trap: a body-level `.get()` with no tracked consumer.
#[component]
fn Trap() -> Element {
    let local = signal(7);
    let _snapshot = local.get(); // ← should warn
    ui! {
        text { "trap" }
    }
}

#[test]
fn regression_body_level_get_warns_during_build() {
    let _ = take_warnings(); // drain other tests' noise
    let rt = TestRuntime::new();
    let _owner = rt.render(ui! { Trap() });
    assert_eq!(warned_components(), vec!["Trap"], "the hoisted snapshot must warn");
}

/// Declared intent silences it.
#[component]
fn IntentionalSnapshot() -> Element {
    let local = signal(7);
    let _snapshot = local.get_untracked(); // ← declared: no warning
    ui! {
        text { "intentional" }
    }
}

#[test]
fn get_untracked_is_silent() {
    let _ = take_warnings();
    let rt = TestRuntime::new();
    let _owner = rt.render(ui! { IntentionalSnapshot() });
    assert_eq!(warned_components(), Vec::<&str>::new());
}

/// A read inside a binding closure runs in the binding's Effect
/// (tracked) — the legitimate reactive shape must stay silent.
#[component]
fn ClosureRead() -> Element {
    let local = signal(7);
    ui! {
        text(move || format!("{}", local.get()))
    }
}

#[test]
fn tracked_closure_read_is_silent() {
    let _ = take_warnings();
    let rt = TestRuntime::new();
    let _owner = rt.render(ui! { ClosureRead() });
    assert_eq!(warned_components(), Vec::<&str>::new());
}

/// An explicit user `untrack` during build is declared intent too.
#[component]
fn UserUntracked() -> Element {
    let local = signal(7);
    let _snapshot = untrack(|| local.get());
    ui! {
        text { "untracked" }
    }
}

#[test]
fn user_untrack_is_silent() {
    let _ = take_warnings();
    let rt = TestRuntime::new();
    let _owner = rt.render(ui! { UserUntracked() });
    assert_eq!(warned_components(), Vec::<&str>::new());
}

#[test]
fn reads_outside_component_build_are_silent() {
    // Event handlers, app init, tests — imperative reads with no build
    // on the stack are normal code, not the trap.
    let _ = take_warnings();
    let s: Signal<i32> = signal(1);
    let _ = s.get();
    assert_eq!(warned_components(), Vec::<&str>::new());
}

/// The branch-rebuild case: `when`/`switch` construct their branches
/// inside the walker's `untrack_for_build`, which must NOT count as
/// declared snapshot intent — a Trap component mounted by a flipped
/// branch still warns. (A plain `untrack` in the walker would have
/// silenced the diagnostic exactly here; this is the regression test
/// for that split.)
#[test]
fn regression_trap_inside_when_branch_still_warns_on_rebuild() {
    let _ = take_warnings();
    let rt = TestRuntime::new();
    let show: Signal<bool> = signal(false);

    let _owner = rt.render(ui! {
        if show.get() {
            Trap()
        } else {
            text { "empty" }
        }
    });
    assert_eq!(
        warned_components(),
        Vec::<&str>::new(),
        "branch not mounted yet — nothing to warn about"
    );

    show.set(true); // branch flips; Trap builds inside the when-effect's build region
    assert_eq!(
        warned_components(),
        vec!["Trap"],
        "the trap must still warn when built inside a reactive branch"
    );
}
