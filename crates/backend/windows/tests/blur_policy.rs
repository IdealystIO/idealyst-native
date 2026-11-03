//! The rule behind a Windows text field's cancelable `on_blur`.
//!
//! ## The bug this pins
//!
//! `create_text_input` dropped the author's `on_blur` (`_on_blur`), so on
//! Windows it never ran and `BlurOutcome::Keep` could not keep focus, as it
//! does on every other backend.
//!
//! The crate body is `cfg(target_os = "windows")`; this test compiles the
//! policy module directly (`#[path]`) so it runs on every host. The live
//! wiring (`EN_KILLFOCUS` → `on_blur` → `SetFocus` back) is covered by
//! `lib.rs`'s `regression_text_input_on_blur_keep_takes_focus_back`
//! (Windows target / Wine).

#[path = "../src/blur_policy.rs"]
mod blur_policy;

use blur_policy::{retake_focus, FocusNow};
use runtime_shared::primitives::text_input::BlurOutcome;

#[test]
fn regression_windows_on_blur_keep_takes_focus_back_from_a_dismiss() {
    assert!(retake_focus(BlurOutcome::Keep, FocusNow::Elsewhere), "outside click / blur(): kept");
}

#[test]
fn on_blur_allow_lets_the_blur_stand() {
    for now in [FocusNow::Elsewhere, FocusNow::AnotherField, FocusNow::LeftApp, FocusNow::ThisField] {
        assert!(!retake_focus(BlurOutcome::Allow, now), "{now:?}");
    }
}

#[test]
fn on_blur_keep_never_blocks_a_move_to_another_field_or_app() {
    assert!(!retake_focus(BlurOutcome::Keep, FocusNow::AnotherField), "tabbing / clicking into another field moves focus");
    assert!(!retake_focus(BlurOutcome::Keep, FocusNow::LeftApp), "no re-activating over the app the user switched to");
    assert!(!retake_focus(BlurOutcome::Keep, FocusNow::ThisField), "already back");
}
