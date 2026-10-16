//! Shared types for `overlay()` and `anchored_overlay()` — compositions on
//! top of the portal primitive. These aren't framework primitives; the
//! builders (in `runtime_vocabulary::builders::portal`) lower to a portal
//! item, adding the backdrop wiring around the caller's children.
//!
//! Defaults baked in here are deliberate UX choices for the common
//! cases (centered modal with dismiss-on-tap backdrop; popover with
//! no backdrop). Authors who want non-default behavior either chain
//! the builder methods or reach for `portal()` directly and assemble their own backdrop + content children.


// =============================================================================
// Backdrop
// =============================================================================

/// How an overlay's backdrop layer behaves.
///
/// A backdrop is composition-level (`runtime_vocabulary::builders::portal`):
/// its own full-screen portal, mounted behind the overlay's content,
/// holding a `pressable()` that fills the viewport whatever the overlay's
/// placement. It is transparent; `backdrop_style` paints it (a scrim
/// colour). A `Dismiss` backdrop also carries a fill layer whose
/// `on_touch` dismisses on a non-primary press, because a pressable's
/// click is primary-only on every backend.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum BackdropMode {
    /// Covers the viewport. A press of ANY button on it — primary
    /// click/tap, right-click, middle-click — fires the `on_dismiss`
    /// callback and reaches nothing beneath; a secondary press is
    /// consumed, so no native context menu opens over it.
    #[default]
    Dismiss,
    /// Covers the viewport and swallows presses without dismissing; the
    /// host drives open/close itself.
    Opaque,
    /// No backdrop. Only the overlay's content takes presses; the
    /// viewport around it stays interactive.
    None,
}
