//! Cross-platform **tactile feedback** — fire-and-forget device haptics.
//!
//! Three tiny synchronous functions trigger the platform's haptic engine:
//!
//! - [`impact`] — a physical-impact tap, with an [`ImpactStyle`] weight.
//! - [`notify`] — a success / warning / error [`NotificationFeedback`]
//!   pattern.
//! - [`selection`] — a light selection-changed tick.
//!
//! # Best-effort, no error type
//!
//! Haptics are *non-essential* feedback — a nicety, never load-bearing. So
//! every function is **best-effort and infallible**: where the platform
//! can't deliver an exact analog (web has no notion of an "impact style", a
//! Mac without a Force Touch trackpad has no haptics at all), the call maps
//! to the nearest effect or is a silent no-op. There is deliberately no
//! `Result` and no error enum — a caller can't meaningfully recover from "the
//! phone didn't buzz", and threading an error through every press handler
//! would be all cost and no benefit. Use [`is_supported`] if you want to,
//! say, hide a "vibrate on tap" setting on a device that can't.
//!
//! The public surface is identical on every target; only the *mechanism*
//! differs (see the per-platform modules / the README table).
//!
//! ```ignore
//! use haptics::{impact, notify, selection, ImpactStyle, NotificationFeedback};
//!
//! impact(ImpactStyle::Medium);          // a tap when a control engages
//! selection();                          // a tick as a picker value changes
//! notify(NotificationFeedback::Success); // the "it worked" pattern
//! ```
//!
//! # Scope
//!
//! Predefined feedback patterns only. Custom waveform / amplitude
//! composition (Core Haptics `CHHapticEngine` on iOS, amplitude curves on
//! Android) is deliberately a later SDK — this one is the small, universal
//! "give me a standard tap/notification/tick" capability.

#![deny(missing_docs)]

// Exactly one platform backend is compiled per target; every other target
// falls through to the `noop` shim below. Each backend exposes the same three
// `fn`s + `is_supported`, called by the public wrappers at the bottom.
#[cfg(all(not(target_arch = "wasm32"), any(target_os = "ios", target_os = "macos")))]
mod apple;
#[cfg(all(not(target_arch = "wasm32"), target_os = "android"))]
mod android;
#[cfg(target_arch = "wasm32")]
mod web;

/// Compile-checked usage recipes (docs / MCP catalog). Present only under the
/// `catalog` feature — see [`recipes`].
#[cfg(feature = "catalog")]
pub mod recipes;

/// The weight of a physical-impact tap from [`impact`].
///
/// Mirrors iOS's `UIImpactFeedbackStyle`. On platforms with a coarser haptic
/// engine the styles collapse onto the nearest available effect (documented
/// per platform), but the author always picks from the same five.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImpactStyle {
    /// A light, subtle tap — small UI elements engaging.
    Light,
    /// A medium tap — the default for most "something happened" feedback.
    Medium,
    /// A heavy, pronounced tap — large or significant elements.
    Heavy,
    /// A soft, dampened tap — a gentler feel than `Light`.
    Soft,
    /// A rigid, crisp tap — a sharper feel than `Heavy`.
    Rigid,
}

/// The semantic outcome a [`notify`] pattern conveys.
///
/// Mirrors iOS's `UINotificationFeedbackType`: a short, distinct buzz pattern
/// the user learns to recognize without looking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationFeedback {
    /// An operation completed successfully.
    Success,
    /// An operation completed, but something needs attention.
    Warning,
    /// An operation failed.
    Error,
}

/// Trigger a physical-impact tap of the given [`ImpactStyle`].
///
/// Use this when a UI element physically "lands" — a control snapping into
/// place, a draggable hitting a boundary, a button committing. Best-effort:
/// a no-op where the platform has no haptic engine.
pub fn impact(style: ImpactStyle) {
    backend::impact(style);
}

/// Trigger a success / warning / error notification pattern.
///
/// Use this to reinforce the outcome of a discrete operation (a form saved,
/// a payment declined). Best-effort: a no-op where unsupported.
pub fn notify(feedback: NotificationFeedback) {
    backend::notify(feedback);
}

/// Trigger a light "selection changed" tick.
///
/// Use this as a value scrolls past under the finger — a picker wheel, a
/// segmented control, a slider crossing a detent. Best-effort: a no-op where
/// unsupported.
pub fn selection() {
    backend::selection();
}

/// Whether this device/target can produce haptic feedback at all.
///
/// `true` means [`impact`] / [`notify`] / [`selection`] will (best-effort)
/// produce a physical effect; `false` means they're guaranteed no-ops here,
/// so a caller can hide haptics-related UI. The calls remain safe to make
/// regardless — this is purely so an app can present an honest setting.
pub fn is_supported() -> bool {
    backend::is_supported()
}

// ---------------------------------------------------------------------------
// Backend dispatch. Each `#[cfg]` arm aliases `backend` to one module so the
// public wrappers above are platform-agnostic. Non-iOS/macOS/Android/web
// native targets get the `noop` shim — every function present, every one a
// silent no-op — so the crate builds and the API is uniform everywhere.
// ---------------------------------------------------------------------------

#[cfg(all(not(target_arch = "wasm32"), any(target_os = "ios", target_os = "macos")))]
use apple as backend;

#[cfg(all(not(target_arch = "wasm32"), target_os = "android"))]
use android as backend;

#[cfg(target_arch = "wasm32")]
use web as backend;

#[cfg(all(
    not(target_arch = "wasm32"),
    not(target_os = "ios"),
    not(target_os = "macos"),
    not(target_os = "android")
))]
use noop as backend;

/// No-op haptics for native targets with no haptic engine (Windows, Linux,
/// …). Every function is present so the public API is uniform; each does
/// nothing and [`is_supported`] reports `false`.
#[cfg(all(
    not(target_arch = "wasm32"),
    not(target_os = "ios"),
    not(target_os = "macos"),
    not(target_os = "android")
))]
mod noop {
    use crate::{ImpactStyle, NotificationFeedback};

    pub(crate) fn impact(_style: ImpactStyle) {}
    pub(crate) fn notify(_feedback: NotificationFeedback) {}
    pub(crate) fn selection() {}
    pub(crate) fn is_supported() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The public API must be callable on the host (whatever runs `cargo
    // test`) and never panic. On macOS this exercises the real
    // `NSHapticFeedbackManager` path end-to-end; on Linux/Windows CI it's the
    // `noop` shim. Either way the contract is "safe to call, never panics".
    #[test]
    fn calls_never_panic() {
        impact(ImpactStyle::Light);
        impact(ImpactStyle::Medium);
        impact(ImpactStyle::Heavy);
        impact(ImpactStyle::Soft);
        impact(ImpactStyle::Rigid);
        notify(NotificationFeedback::Success);
        notify(NotificationFeedback::Warning);
        notify(NotificationFeedback::Error);
        selection();
    }

    // `is_supported()` is a pure predicate — calling it must be side-effect
    // free and itself never panic. We don't assert a specific value (it's
    // host-dependent), only that it answers.
    #[test]
    fn is_supported_answers() {
        let _ = is_supported();
    }

    // The enums are plain `Copy` value types; assert the derives an author
    // relies on (matching, equality, copy-by-value into the fns) hold.
    #[test]
    fn enums_are_value_types() {
        let s = ImpactStyle::Medium;
        let s2 = s; // Copy, not move
        assert_eq!(s, s2);
        assert_eq!(NotificationFeedback::Error, NotificationFeedback::Error);
        assert_ne!(NotificationFeedback::Success, NotificationFeedback::Warning);
    }
}
