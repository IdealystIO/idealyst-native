//! `overlay()` and `anchored_overlay()` — compositions on top of
//! [`primitives::portal`]. These aren't framework primitives; they're
//! builders that lower to `Element::Portal` at conversion time,
//! adding the backdrop wiring around the caller's children.
//!
//! Defaults baked in here are deliberate UX choices for the common
//! cases (centered modal with dismiss-on-tap backdrop; popover with
//! no backdrop). Authors who want non-default behavior either chain
//! the builder methods or reach for [`portal()`](super::portal::portal)
//! directly and assemble their own backdrop + content children.


// =============================================================================
// Backdrop
// =============================================================================

/// How an overlay's backdrop layer behaves.
///
/// Backdrop dismissal is composition-level here — we render a
/// fullscreen `pressable()` as the first child inside the portal and
/// wire its `on_click` to the user's `on_dismiss` (for `Dismiss`)
/// or leave it as a passive scrim (for `Opaque`).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum BackdropMode {
    /// Semi-transparent scrim. Clicks on the scrim fire the
    /// `on_dismiss` callback.
    #[default]
    Dismiss,
    /// Semi-transparent scrim. Clicks on the scrim do NOT dismiss;
    /// the host must drive open/close itself.
    Opaque,
    /// No scrim at all. The viewport behind stays interactive.
    None,
}

// =============================================================================
// overlay() — viewport-anchored composition
// =============================================================================








// =============================================================================
// anchored_overlay() — element-anchored composition
// =============================================================================


// NOTE: `anchored_overlay` has no `click_through` — popovers, tooltips,
// dropdowns and context menus are content-sized (not full-strip), so their
// root only covers what they render; there's no empty band to pass through.
// The shared lowering always receives `click_through: false` for it below.







// =============================================================================
// Lowering — shared between both compositions
// =============================================================================



