//! The robot's `set_orientation` on iOS (`IntrospectionOps::set_orientation`):
//! the app turns its own interface with
//! `UIWindowScene.requestGeometryUpdate(_:errorHandler:)` (iOS 16, the
//! framework's floor), as a person turning the phone would. UIKit rotates
//! the scene, the host view's bounds change, and the layout observer
//! (`set_host_root`) pushes the turned size to the viewport — the same path
//! a real rotation takes.
//!
//! An iPad app that supports multitasking can't turn itself (iPadOS ignores
//! the request, without an error): there this answers `None`, and
//! `idealyst test` turns the simulator instead.
//!
//! The request asks for an orientation MASK, so UIKit may pick either
//! landscape side; the scene's view controller must allow it, and the root
//! controller is a plain `UIViewController`, whose default allows portrait
//! and both landscapes on iPhone and every orientation on iPad (the
//! generated Info.plist lists all four). It works on a simulator and on a
//! device alike, with no simulator-side scripting.
//!
//! UIKit answers a refusal asynchronously through the error handler, after
//! the injection has returned, so the refusal is logged; the robot client
//! waits for the turned size and reports the size it saw instead.

use block2::RcBlock;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::msg_send_id;
use objc2_foundation::{NSObject, NSString};
use objc2_ui_kit::UIView;
use runtime_vocabulary::caps::{NativeInjection, ScreenOrientation};

/// Turn the scene holding `view` to `orientation`; `None` where the app
/// can't (a multitasking iPad app: `orientation_policy::app_can_turn`).
pub(crate) fn set_orientation(view: &UIView, orientation: ScreenOrientation) -> Option<NativeInjection> {
    if !crate::orientation_policy::app_can_turn(is_pad(), requires_full_screen()) {
        return None;
    }
    let window: Option<Retained<NSObject>> = unsafe { msg_send_id![view, window] };
    let scene: Option<Retained<NSObject>> = window.as_ref().and_then(|w| unsafe { msg_send_id![w, windowScene] });
    let (window, scene) = match (window, scene) {
        (Some(w), Some(s)) => (w, s),
        _ => return Some(Box::new(|| Err("the app's view is not in a window scene yet".into()))),
    };
    Some(Box::new(move || {
        let mask = crate::orientation_policy::mask(orientation);
        unsafe {
            let prefs: Retained<NSObject> = msg_send_id![
                msg_send_id![objc2::class!(UIWindowSceneGeometryPreferencesIOS), alloc],
                initWithInterfaceOrientations: mask
            ];
            let name = orientation.name();
            let refused = RcBlock::new(move |error: *mut AnyObject| {
                let text: Option<Retained<NSString>> =
                    if error.is_null() { None } else { msg_send_id![error, localizedDescription] };
                eprintln!(
                    "[idealyst] set_orientation({name}): UIKit refused the rotation: {}",
                    text.map(|t| t.to_string()).unwrap_or_default()
                );
            });
            let _: () = msg_send![&scene, requestGeometryUpdateWithPreferences: &*prefs, errorHandler: &*refused];
            // The controller re-reads which orientations it allows, so the
            // request is checked against them now rather than at the next
            // device turn.
            let root: Option<Retained<NSObject>> = msg_send_id![&window, rootViewController];
            if let Some(root) = root {
                let _: () = msg_send![&root, setNeedsUpdateOfSupportedInterfaceOrientations];
            }
        }
        Ok(())
    }))
}

/// `UIDevice.currentDevice.userInterfaceIdiom == .pad`.
fn is_pad() -> bool {
    const IDIOM_PAD: isize = 1;
    let idiom: isize = unsafe {
        let device: Retained<NSObject> = msg_send_id![objc2::class!(UIDevice), currentDevice];
        msg_send![&device, userInterfaceIdiom]
    };
    idiom == IDIOM_PAD
}

/// The app's `UIRequiresFullScreen` Info.plist key.
fn requires_full_screen() -> bool {
    unsafe {
        let bundle: Retained<NSObject> = msg_send_id![objc2::class!(NSBundle), mainBundle];
        let key = NSString::from_str("UIRequiresFullScreen");
        let value: Option<Retained<NSObject>> = msg_send_id![&bundle, objectForInfoDictionaryKey: &*key];
        value.is_some_and(|v| {
            let on: bool = msg_send![&v, boolValue];
            on
        })
    }
}
