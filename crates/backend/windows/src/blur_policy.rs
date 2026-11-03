//! A text field's cancelable `on_blur` on Windows: when the field's `EDIT`
//! loses focus (`EN_KILLFOCUS`) the author's handler runs, and a
//! [`BlurOutcome::Keep`] takes focus back — the cross-platform contract
//! (`runtime_shared::primitives::text_input::BlurOutcome`): `Keep` vetoes a
//! blur that leaves no field focused, never a move to another field.
//!
//! Win32 has no "may focus leave?" query — focus moves first and the field
//! hears about it after — so the veto is a re-take, as on web and Android.
//! The re-take is deferred to the next scheduler turn: calling `SetFocus`
//! from inside the `WM_KILLFOCUS` of the focus change still in progress is
//! undefined (the outer change can land after it and win).
//!
//! Deliberately free of any `windows`-crate type: the crate body is
//! `cfg(target_os = "windows")`. This file is ALSO compiled by
//! `tests/blur_policy.rs` via `#[path]`, which runs on any host; the live
//! wiring is covered by the `lib.rs` tests (Windows target / Wine).

use runtime_shared::primitives::text_input::BlurOutcome;

/// Where keyboard focus sits when the deferred re-take runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FocusNow {
    /// The field itself (focus already came back).
    ThisField,
    /// Another text field (`EDIT`) in the same window: the user moved on,
    /// which `Keep` does not block.
    AnotherField,
    /// Somewhere else in the window — the host window itself (a click on
    /// painted content, a programmatic or robot `blur()`), or a native
    /// button that took focus on press.
    Elsewhere,
    /// Nowhere in this thread: the app was deactivated (Alt+Tab). Taking
    /// focus back would try to re-activate the window over the app the
    /// user switched to.
    LeftApp,
}

/// Whether the field takes focus back after its `on_blur` answered
/// `outcome` and focus moved to `now`.
pub(crate) fn retake_focus(outcome: BlurOutcome, now: FocusNow) -> bool {
    outcome == BlurOutcome::Keep && now == FocusNow::Elsewhere
}
