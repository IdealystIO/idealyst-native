//! Dual-core gate for the `docs` feature (DocControls derive + the
//! doc-controls runtime).
//!
//! Wave-2b regression: `idea-ui --features docs,new-core` did not
//! compile — the world kernel's signal surface requires
//! `T: PartialEq` even for `get`/`set_always`, and the `*Ref` handle
//! types (`ToneRef`, `VariantRef`, …) had no `PartialEq`, so
//! `ref_picker_control<T: RefBuiltins>` (the derive's RefHandle arm)
//! failed to type-check and a concurrent wave parked `idea-ui/docs` as
//! old-core-only. Fixed by making `PartialEq` a `RefBuiltins`
//! supertrait with pointer-identity impls in idea-theme (see the
//! rationale there). This suite runs the derive + the control helpers
//! on BOTH cores:
//!
//!   cargo test -p idea-ui --features docs --test doc_controls_dual_core
//!   cargo test -p idea-ui --no-default-features \
//!       --features new-core,docs,table --test doc_controls_dual_core

#![cfg(feature = "docs")]

// The new-core alias: same-source `runtime_core::…` paths resolve
// against the glue facade (see idea-ui's lib.rs note).
#[cfg(feature = "new-core")]
extern crate runtime_facade as runtime_core;

use idea_theme::extensible::{RefBuiltins, ToneRef};
use idea_ui::doc_controls::DocControls;
use idea_ui::ButtonProps;

/// Per-core reactive test env. New core: signals/effects need an
/// ambient world (`__with_fresh_world`). Old core: state is ambient
/// thread-local, but `effect!` (used by the picker controls) asserts
/// an active SCOPE — provide one.
#[cfg(feature = "new-core")]
fn with_reactive_env<R>(f: impl FnOnce() -> R) -> R {
    runtime_vocabulary::glue::__with_fresh_world(f)
}
#[cfg(not(feature = "new-core"))]
fn with_reactive_env<R>(f: impl FnOnce() -> R) -> R {
    let mut scope = runtime_core::reactive::Scope::new();
    runtime_core::reactive::with_scope(&mut scope, f)
}

/// Commit staged writes so `get()` observes them (new core stages;
/// old core applies synchronously — no-op there).
fn commit() {
    #[cfg(feature = "new-core")]
    runtime_vocabulary::glue::__flush_test_world();
}

/// The derive's generated state/round-trip on a real component props
/// struct (String arm — `#[props]` wraps ButtonProps fields in
/// `Reactive<T>`, and `Reactive<String>` is the controlled shape), plus
/// the generated panel building on this core.
#[test]
fn button_doc_controls_state_roundtrip_and_panel_build() {
    with_reactive_env(|| {
        // The panel renders themed components (Card/Typography/Field);
        // their sheets resolve against the installed theme.
        idea_ui::install_idea_theme(idea_ui::light_theme());
        let state = ButtonProps::init_state();
        state.label.set("Ship it".to_string());
        commit();

        let props = ButtonProps::from_state(&state);
        assert_eq!(props.label.get(), "Ship it");

        // The generated controls panel builds (Card + Typography +
        // Field rows) — a compile-and-run gate over the derive's
        // render arm on this core.
        let _panel = ButtonProps::render_controls(&state);
    });
}

/// The wave-2b breakage itself: `ref_picker_control<T: RefBuiltins>`
/// holds the picked value in a `Signal<T>` — on the world kernel that
/// requires `T: PartialEq` for creation, `get`, and `set_always`.
/// Drives the picker's state signal exactly as its `on_change` does.
#[test]
fn ref_picker_signal_surface_works_on_both_cores() {
    with_reactive_env(|| {
        let tone = runtime_core::signal(ToneRef::default());
        let _picker = idea_ui::doc_controls::ref_picker_control::<ToneRef>(tone);

        let danger = ToneRef::builtins_list()
            .into_iter()
            .find(|(k, _)| *k == "danger")
            .map(|(_, t)| t)
            .expect("danger tone builtin");
        tone.set_always(danger);
        commit();
        assert_eq!(
            tone.get().current_key(),
            "danger",
            "picked *Ref must land in the state signal"
        );
    });
}

/// The `*Ref` `PartialEq` contract the picker relies on: pointer
/// identity, never key equality (key-equality would let the guarded
/// `set` swallow a change between two distinct modifiers sharing a
/// key — see the impl comment in idea-theme's extensible module).
#[test]
fn ref_partial_eq_is_pointer_identity() {
    let a = ToneRef::builtins_list()
        .into_iter()
        .find(|(k, _)| *k == "primary")
        .map(|(_, t)| t)
        .unwrap();
    let b = ToneRef::builtins_list()
        .into_iter()
        .find(|(k, _)| *k == "primary")
        .map(|(_, t)| t)
        .unwrap();
    let a2 = a.clone();
    // Plain boolean asserts: the *Ref handles deliberately don't
    // implement `Debug` (opaque `Rc<dyn Trait>`).
    assert!(a == a2, "same Rc allocation compares equal");
    assert!(
        a != b,
        "distinct allocations of the same builtin compare UNEQUAL — \
         false negatives only (redundant notify), never a swallowed change"
    );
}
