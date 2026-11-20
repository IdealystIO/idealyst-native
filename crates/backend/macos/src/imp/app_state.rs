//! App situation facts for the macOS backend (`runtime_shared::app_state`):
//! focus, visibility and the reduce-motion preference, read at boot and
//! followed live. Installed once per boot by `newcore::start_with`.
//!
//! - **Focused**: `NSApp.isActive`, re-read on
//!   `NSApplicationDidBecomeActive` / `DidResignActive` (the resign side is
//!   the same hook the keyboard and pointer sources use for focus loss).
//! - **Visible**: `!NSApp.isHidden && NSApp.occlusionState & Visible`
//!   ([`crate::pointer_policy::app_visible`]), re-read on app hide / unhide /
//!   occlusion change and window miniaturize / deminiaturize / occlusion
//!   change. App-level occlusion is false when every window is minimized,
//!   covered, on another Space, or the display sleeps. At boot only
//!   `isHidden` is read: the host window is not ordered on screen yet, so
//!   the occlusion state would wrongly read "not visible"; the occlusion
//!   notification follows as soon as the window appears.
//! - **ReduceMotion**: `NSWorkspace.accessibilityDisplayShouldReduceMotion`,
//!   re-read on `NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification`
//!   — posted on the WORKSPACE's notification center, not the default one.
//! - **PrimaryPointer**: `Fine` (every Mac has a mouse or trackpad).
//! - **Color scheme** (`runtime_shared::host::report_color_scheme`, not
//!   `app_state`): `NSApp.effectiveAppearance`'s name ([`read_color_scheme`]),
//!   followed by key-value observing that property ([`observe_appearance`]).
//!   KVO rather than the `AppleInterfaceThemeChangedNotification`
//!   distributed notification: the property is what the read returns, and
//!   AppKit may update it after that notification is delivered.
//! - **UserActivated**: not reported (native apps may play audio at once).

use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use objc2_foundation::{MainThreadMarker, NSObject};
use runtime_shared::app_state::{report, AppStateChange, PointerKind};

fn shared_app() -> *mut AnyObject {
    unsafe { msg_send![class!(NSApplication), sharedApplication] }
}

fn read_focused() -> bool {
    unsafe { msg_send![shared_app(), isActive] }
}

fn read_visible() -> bool {
    let app = shared_app();
    let hidden: bool = unsafe { msg_send![app, isHidden] };
    let occlusion: usize = unsafe { msg_send![app, occlusionState] };
    crate::pointer_policy::app_visible(hidden, occlusion)
}

/// The app's light / dark appearance.
///
/// The *application's* effective appearance, NOT
/// `NSAppearance.currentAppearance`: the latter is a per-draw thread-local
/// that is `nil` outside a drawing pass (e.g. at mount, when the app reads
/// `color_scheme()` to pick its initial theme), so it would always report
/// `Auto`. `NSApp.effectiveAppearance` reflects the system Dark Mode at any
/// time.
pub(crate) fn read_color_scheme() -> runtime_shared::ColorScheme {
    let app = shared_app();
    let appearance: *const NSObject = unsafe {
        if app.is_null() {
            // No app yet — fall back to the drawing-context appearance.
            msg_send![class!(NSAppearance), currentAppearance]
        } else {
            msg_send![app, effectiveAppearance]
        }
    };
    if appearance.is_null() {
        return runtime_shared::ColorScheme::Auto;
    }
    let name: *const objc2_foundation::NSString = unsafe { msg_send![appearance, name] };
    if name.is_null() {
        return runtime_shared::ColorScheme::Auto;
    }
    crate::pointer_policy::color_scheme_from_appearance(&unsafe { &*name }.to_string())
}

/// Follow `NSApp.effectiveAppearance`: report each change of the color
/// scheme, then `after_report` (the flush). `None` before `NSApp` exists.
pub(crate) fn observe_appearance(
    mtm: MainThreadMarker,
    after_report: Rc<dyn Fn()>,
) -> Option<super::callbacks::KeyValueObservation> {
    super::callbacks::observe_key_path(
        mtm,
        shared_app(),
        "effectiveAppearance",
        Rc::new(move || {
            runtime_shared::backend::report_color_scheme(read_color_scheme());
            after_report();
        }),
    )
}

fn workspace() -> *mut AnyObject {
    unsafe { msg_send![class!(NSWorkspace), sharedWorkspace] }
}

fn read_reduce_motion() -> bool {
    unsafe { msg_send![workspace(), accessibilityDisplayShouldReduceMotion] }
}

/// Report the boot reading and install the observers. `after_report` runs
/// after every live report (the backend's flush, so world signals commit).
/// The returned observers must outlive the app; dropping them unregisters
/// nothing explicitly — `NSNotificationCenter` drops deallocated
/// selector-based observers on its own (macOS 10.11+).
pub(crate) fn install(mtm: MainThreadMarker, after_report: Rc<dyn Fn()>) -> Vec<Retained<NSObject>> {
    report(AppStateChange::Focused(read_focused()));
    let hidden: bool = unsafe { msg_send![shared_app(), isHidden] };
    report(AppStateChange::Visible(!hidden));
    report(AppStateChange::ReduceMotion(read_reduce_motion()));
    report(AppStateChange::PrimaryPointer(PointerKind::Fine));

    let mut observers = Vec::new();
    let on = |f: fn() -> AppStateChange| -> Rc<dyn Fn()> {
        let after = after_report.clone();
        Rc::new(move || {
            report(f());
            after();
        })
    };
    let nil: *mut AnyObject = std::ptr::null_mut();
    for name in ["NSApplicationDidBecomeActiveNotification", "NSApplicationDidResignActiveNotification"] {
        observers.push(super::callbacks::observe_notification(
            mtm,
            name,
            nil,
            on(|| AppStateChange::Focused(read_focused())),
        ));
    }
    for name in [
        "NSApplicationDidHideNotification",
        "NSApplicationDidUnhideNotification",
        "NSApplicationDidChangeOcclusionStateNotification",
        "NSWindowDidMiniaturizeNotification",
        "NSWindowDidDeminiaturizeNotification",
        "NSWindowDidChangeOcclusionStateNotification",
    ] {
        observers.push(super::callbacks::observe_notification(
            mtm,
            name,
            nil,
            on(|| AppStateChange::Visible(read_visible())),
        ));
    }
    let ws_center: *mut AnyObject = unsafe { msg_send![workspace(), notificationCenter] };
    observers.push(super::callbacks::observe_notification_on(
        mtm,
        ws_center,
        "NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification",
        nil,
        on(|| AppStateChange::ReduceMotion(read_reduce_motion())),
    ));
    observers
}
