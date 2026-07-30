//! The `docs` feature end-to-end: the `DocControls` derive plus the
//! doc-controls runtime that renders a component's live control panel.
//!
//! History (the reason this suite exists as its own file): the derive's
//! RefHandle arm holds the picked value in a `Signal<T>`, and the world
//! kernel's signal surface requires `T: PartialEq` even for `get` /
//! `set_always`. The `*Ref` handle types (`ToneRef`, `VariantRef`, …) had
//! none, so `ref_picker_control<T: RefBuiltins>` failed to type-check and
//! `idea-ui/docs` was parked as unbuildable. The fix made `PartialEq` a
//! `RefBuiltins` supertrait with pointer-identity impls in idea-theme (see
//! the rationale there). This suite is what keeps that wired.
//!
//!   cargo test -p idea-ui --features docs --test doc_controls

#![cfg(feature = "docs")]

use idea_theme::extensible::{RefBuiltins, ToneRef};
use idea_ui::doc_controls::DocControls;
use idea_ui::ButtonProps;

/// Reactive test env: signals and effects (the picker controls use
/// `effect!`) need an ambient world.
fn with_reactive_env<R>(f: impl FnOnce() -> R) -> R {
    idea_theme::testing::with_test_world(f)
}

/// Commit staged writes so a following `get()` observes them.
fn commit() {
    idea_theme::testing::commit();
}

/// The derive's generated state/round-trip on a real component props
/// struct (String arm — `#[props]` wraps ButtonProps fields in
/// `Reactive<T>`, and `Reactive<String>` is the controlled shape), plus
/// the generated panel building.
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
        // render arm.
        let _panel = ButtonProps::render_controls(&state);
    });
}

/// The breakage itself: `ref_picker_control<T: RefBuiltins>` holds the
/// picked value in a `Signal<T>` — the world kernel requires
/// `T: PartialEq` for creation, `get`, and `set_always`. Drives the
/// picker's state signal exactly as its `on_change` does.
#[test]
fn ref_picker_signal_surface_type_checks_and_round_trips() {
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
