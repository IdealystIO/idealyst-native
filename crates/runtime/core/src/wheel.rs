//! Wheel / magnify event pipeline — the desktop counterpart to the touch
//! pipeline ([`crate::touch`]).
//!
//! Touch screens express zoom as a two-finger pinch, which rides the existing
//! [`TouchEvent`](crate::TouchEvent) stream (see the `pinch` recognizer). The
//! desktop equivalents — a trackpad pinch, a trackpad two-finger scroll, a
//! mouse scroll-wheel — are **not** touches. They arrive through this separate
//! channel: the backend installs a [`WheelHandler`] on a view via
//! [`Backend::install_wheel_handler`](crate::Backend::install_wheel_handler)
//! and fires it for every wheel / magnify event hitting that view.
//!
//! Only the desktop backends source these (web `wheel`, macOS `magnify:` /
//! `scrollWheel:`). iOS / Android leave the trait method at its no-op default —
//! they have no trackpad/wheel, and pinch covers them. This is genuine input
//! availability, not a per-platform hack: an app that wants zoom installs
//! *both* a `pinch` handler (works everywhere) and a wheel handler (fires on
//! desktop), which the zoom SDK pairs for you.

use std::rc::Rc;

use crate::touch::{TouchPoint, TouchResponse};

/// What a [`WheelEvent`] represents. The two intents are sourced differently
/// per platform but converge here: web folds them out of `WheelEvent.ctrlKey`,
/// macOS out of `magnify:` vs `scrollWheel:`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WheelKind {
    /// A two-finger trackpad scroll or a mouse scroll-wheel notch. Carried by
    /// [`WheelEvent::delta_x`] / [`WheelEvent::delta_y`].
    Scroll,
    /// A zoom intent — a trackpad pinch (web `wheel` with `ctrlKey`, macOS
    /// `magnify:`). The amount is in [`WheelEvent::scale`].
    Zoom,
}

/// One wheel / magnify delivery to a subscribed handler.
///
/// The key design point is [`WheelEvent::scale`]: each backend normalizes its
/// native zoom signal (web's `wheel` `deltaY` under `ctrlKey`, macOS's
/// `NSEvent.magnification`) into the same incremental multiplier, so app code
/// — and the zoom SDK — never hardcodes a per-platform constant. This is the
/// "backends diverge in mechanism, converge in observable output" rule applied
/// to input.
#[derive(Clone, Copy, Debug)]
pub struct WheelEvent {
    /// Whether this is a scroll or a zoom. See [`WheelKind`].
    pub kind: WheelKind,
    /// Horizontal scroll delta in CSS pixels (positive = content moves left /
    /// scroll right). `0.0` for [`WheelKind::Zoom`].
    pub delta_x: f32,
    /// Vertical scroll delta in CSS pixels (positive = scroll down). `0.0` for
    /// [`WheelKind::Zoom`].
    pub delta_y: f32,
    /// Incremental zoom multiplier for THIS event, normalized across
    /// platforms: `1.0` = no change, `> 1.0` = zoom in, `< 1.0` = zoom out.
    /// `1.0` for [`WheelKind::Scroll`].
    pub scale: f32,
    /// Cursor position in the subscribed view's local coordinates — the focal
    /// point to zoom about.
    pub position: TouchPoint,
    /// Cursor position in window coordinates.
    pub window_position: TouchPoint,
    /// Platform monotonic timestamp in nanoseconds (velocity / inter-event
    /// timing; not wall-clock).
    pub timestamp_ns: u64,
}

/// Boxed wheel handler installed on a primitive. The framework holds one per
/// subscribed node and invokes it for every wheel / magnify event delivered
/// there. A [`TouchResponse`] with `consumed: true` tells the backend to
/// suppress the platform default (web `preventDefault`, so the page doesn't
/// scroll or browser-zoom).
pub type WheelHandler = Rc<dyn Fn(&WheelEvent) -> TouchResponse>;
