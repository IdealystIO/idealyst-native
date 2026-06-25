//! Apple haptics — iOS feedback generators / macOS `NSHapticFeedbackManager`.
//!
//! **Compile-checked only ⚠️** — the objc2 message sends compile and link,
//! but the physical effect has not been verified on a device/trackpad from
//! this crate. The class names + selectors below are the documented UIKit /
//! AppKit API.
//!
//! ## iOS (`target_os = "ios"`)
//!
//! UIKit ships three generators:
//! - `UIImpactFeedbackGenerator(style:)` → `impactOccurred` — physical taps.
//! - `UINotificationFeedbackGenerator` → `notificationOccurred:` — patterns.
//! - `UISelectionFeedbackGenerator` → `selectionChanged` — the value tick.
//!
//! Best practice is `prepare()` then fire: `prepare` warms the Taptic Engine
//! so the tap lands with minimal latency. We create a fresh generator per
//! call (cheap), `prepare`, fire, and let it drop — fine for one-shots; a
//! caller doing rapid repeated feedback would hold a generator itself (a
//! future ergonomic layer, out of scope here).
//!
//! These are `UIKit` classes, so this whole impl is `#[cfg(target_os =
//! "ios")]`. macOS uses the AppKit path below instead.
//!
//! ## macOS (`target_os = "macos"`)
//!
//! AppKit has only `NSHapticFeedbackManager.defaultPerformer
//! performFeedbackPattern:performanceTime:` with three coarse patterns
//! (`Generic` = 0, `Alignment` = 1, `LevelChange` = 2). There's no
//! impact-style concept, so we map onto the nearest available pattern:
//! - `notify` → the pattern that best fits the outcome.
//! - `selection` → `LevelChange` (the "value moved a notch" feel).
//! - `impact` → `Generic` (the only general-purpose tap).
//!
//! macOS haptics only fire on a Force Touch trackpad; on other hardware the
//! performer is present but produces nothing — exactly the best-effort
//! contract. `performanceTime: NSHapticFeedbackPerformanceTimeDefault` (= 0)
//! lets AppKit pick the moment.

use crate::{ImpactStyle, NotificationFeedback};

// ===========================================================================
// iOS — UIKit feedback generators.
// ===========================================================================
#[cfg(target_os = "ios")]
mod ios {
    use super::*;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    // UIImpactFeedbackStyle raw values (UIKit).
    const STYLE_LIGHT: i64 = 0;
    const STYLE_MEDIUM: i64 = 1;
    const STYLE_HEAVY: i64 = 2;
    const STYLE_SOFT: i64 = 3;
    const STYLE_RIGID: i64 = 4;

    // UINotificationFeedbackType raw values (UIKit).
    const NOTIFY_SUCCESS: i64 = 0;
    const NOTIFY_WARNING: i64 = 1;
    const NOTIFY_ERROR: i64 = 2;

    fn impact_style_raw(style: ImpactStyle) -> i64 {
        match style {
            ImpactStyle::Light => STYLE_LIGHT,
            ImpactStyle::Medium => STYLE_MEDIUM,
            ImpactStyle::Heavy => STYLE_HEAVY,
            ImpactStyle::Soft => STYLE_SOFT,
            ImpactStyle::Rigid => STYLE_RIGID,
        }
    }

    fn notify_type_raw(feedback: NotificationFeedback) -> i64 {
        match feedback {
            NotificationFeedback::Success => NOTIFY_SUCCESS,
            NotificationFeedback::Warning => NOTIFY_WARNING,
            NotificationFeedback::Error => NOTIFY_ERROR,
        }
    }

    pub(crate) fn impact(style: ImpactStyle) {
        unsafe {
            // `[[UIImpactFeedbackGenerator alloc] initWithStyle: style]`.
            let alloc: *mut AnyObject = msg_send![class!(UIImpactFeedbackGenerator), alloc];
            let gen: *mut AnyObject = msg_send![alloc, initWithStyle: impact_style_raw(style)];
            if gen.is_null() {
                return;
            }
            let _: () = msg_send![gen, prepare];
            let _: () = msg_send![gen, impactOccurred];
            // Balance the +1 from alloc/init; ARC isn't doing it for us here.
            let _: () = msg_send![gen, release];
        }
    }

    pub(crate) fn notify(feedback: NotificationFeedback) {
        unsafe {
            let gen: *mut AnyObject = msg_send![class!(UINotificationFeedbackGenerator), new];
            if gen.is_null() {
                return;
            }
            let _: () = msg_send![gen, prepare];
            let _: () = msg_send![gen, notificationOccurred: notify_type_raw(feedback)];
            let _: () = msg_send![gen, release];
        }
    }

    pub(crate) fn selection() {
        unsafe {
            let gen: *mut AnyObject = msg_send![class!(UISelectionFeedbackGenerator), new];
            if gen.is_null() {
                return;
            }
            let _: () = msg_send![gen, prepare];
            let _: () = msg_send![gen, selectionChanged];
            let _: () = msg_send![gen, release];
        }
    }

    pub(crate) fn is_supported() -> bool {
        // Every iOS device since the iPhone 7 has a Taptic Engine; older
        // devices simply produce nothing. The generators always exist, and
        // there is no public "does this device vibrate" query, so we report
        // supported and let the no-op-on-old-hardware behavior stand.
        true
    }
}

// ===========================================================================
// macOS — NSHapticFeedbackManager (Force Touch trackpad).
// ===========================================================================
#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::msg_send;

    // NSHapticFeedbackPattern raw values (AppKit).
    const PATTERN_GENERIC: i64 = 0;
    const PATTERN_ALIGNMENT: i64 = 1;
    const PATTERN_LEVEL_CHANGE: i64 = 2;
    // NSHapticFeedbackPerformanceTimeDefault — let AppKit choose the moment.
    const PERFORM_TIME_DEFAULT: u64 = 0;

    /// `[NSHapticFeedbackManager defaultPerformer]`, or null if AppKit's
    /// haptics class isn't available. We resolve the class at runtime with
    /// `AnyClass::get` (NOT the `class!` macro): `class!` *panics* if the
    /// class can't be found, which happens whenever AppKit isn't linked into
    /// the process — e.g. a headless `cargo test` binary. Best-effort haptics
    /// must degrade to a no-op there, never abort.
    fn default_performer() -> *mut AnyObject {
        let Some(cls) = AnyClass::get("NSHapticFeedbackManager") else {
            return std::ptr::null_mut();
        };
        unsafe { msg_send![cls, defaultPerformer] }
    }

    fn perform(pattern: i64) {
        let performer = default_performer();
        if performer.is_null() {
            return;
        }
        unsafe {
            let _: () = msg_send![
                performer,
                performFeedbackPattern: pattern,
                performanceTime: PERFORM_TIME_DEFAULT
            ];
        }
    }

    pub(crate) fn impact(_style: ImpactStyle) {
        // AppKit has no impact-weight concept — `Generic` is the only
        // general-purpose tap. All five styles collapse onto it.
        perform(PATTERN_GENERIC);
    }

    pub(crate) fn notify(feedback: NotificationFeedback) {
        // Map the three outcomes onto AppKit's three patterns as best fits:
        // success/error are discrete "it's done" events → Generic; a warning
        // reads more like an alignment nudge.
        let pattern = match feedback {
            NotificationFeedback::Success => PATTERN_GENERIC,
            NotificationFeedback::Warning => PATTERN_ALIGNMENT,
            NotificationFeedback::Error => PATTERN_GENERIC,
        };
        perform(pattern);
    }

    pub(crate) fn selection() {
        // The "value moved a notch" feel is exactly LevelChange.
        perform(PATTERN_LEVEL_CHANGE);
    }

    pub(crate) fn is_supported() -> bool {
        // The performer exists on every Mac (with AppKit linked); it only
        // produces a physical effect on a Force Touch trackpad. There's no
        // clean runtime query for that, and a non-Force-Touch Mac is the
        // best-effort no-op case, so a non-null performer = supported.
        !default_performer().is_null()
    }
}

// --- Re-export the active platform's impl under flat names so `lib.rs`'s
// `use apple as backend` sees `apple::impact` / `notify` / `selection` /
// `is_supported` regardless of which Apple OS this is. ---

#[cfg(target_os = "ios")]
pub(crate) use ios::{impact, is_supported, notify, selection};

#[cfg(target_os = "macos")]
pub(crate) use mac::{impact, is_supported, notify, selection};
