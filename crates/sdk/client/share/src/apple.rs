//! Apple share UI: `UIActivityViewController` on iOS, `NSSharingServicePicker`
//! on macOS. Driven through the Obj-C runtime, the same posture `file-export`
//! takes.
//!
//! iOS is **delegate/completion-driven**: we build the activity items
//! (`NSString` for text, `NSURL` for the URL, file `NSURL`s for files), present
//! a `UIActivityViewController` from the top view controller, and bridge its
//! `completionWithItemsHandler` to a oneshot — the same callback→future pattern
//! `file-export`'s document picker uses. The `completed:` bool maps to
//! [`ShareOutcome`].
//!
//! macOS's `NSSharingServicePicker` reports its result through per-service
//! delegate callbacks we don't subscribe to (the picker is shown relative to a
//! rect and the user then drives the chosen service's own UI). We present it
//! and report [`ShareOutcome::Completed`] once shown — see the crate docs'
//! best-effort note on `ShareOutcome`.
//!
//! VERIFICATION: compile-checked for iOS/macOS only — presenting the share
//! sheet resolves at runtime on a device/desktop session (same posture as
//! `file-export`). Marked compile-checked-only in the README.

use crate::{ShareContent, ShareError, ShareOutcome};

#[cfg(target_os = "ios")]
pub(crate) async fn share(content: &ShareContent) -> Result<ShareOutcome, ShareError> {
    ios::share(content).await
}

#[cfg(target_os = "macos")]
pub(crate) async fn share(content: &ShareContent) -> Result<ShareOutcome, ShareError> {
    macos::share(content)
}

// ---------------------------------------------------------------------------
// Shared: build the activity items array from the content.
// ---------------------------------------------------------------------------

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod items {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send, msg_send_id};
    use objc2_foundation::{NSString, NSURL};

    use crate::ShareContent;

    /// Collect `content` into an `NSMutableArray` of activity/sharing items:
    /// `NSString` for text, `NSURL` for the URL and each file. The order
    /// (text, url, files) is what both `UIActivityViewController` and
    /// `NSSharingServicePicker` expect.
    ///
    /// Returned as a bare `AnyObject` (the array) because the items are
    /// heterogeneous (`NSString` + `NSURL`) — there's no single `ClassType`
    /// element to give `NSArray::from_slice`, so we build a mutable array and
    /// `addObject:` each item, which both APIs accept as `NSArray *`.
    ///
    /// # Safety
    /// Must run on the main thread (where author code / this async fn runs);
    /// the returned array retains its items.
    pub(super) unsafe fn build(content: &ShareContent) -> Retained<AnyObject> {
        let array: Retained<AnyObject> = msg_send_id![class!(NSMutableArray), array];

        if let Some(text) = &content.text {
            let s = NSString::from_str(text);
            let _: () = msg_send![&*array, addObject: &*s];
        }
        if let Some(url) = &content.url {
            // A web URL → NSURL; if it doesn't parse, fall back to plain text so
            // we still share *something* rather than dropping it.
            let ns = NSString::from_str(url);
            let u: Option<Retained<NSURL>> = NSURL::URLWithString(&ns);
            match u {
                Some(u) => {
                    let _: () = msg_send![&*array, addObject: &*u];
                }
                None => {
                    let _: () = msg_send![&*array, addObject: &*ns];
                }
            }
        }
        for file in &content.files {
            let path = NSString::from_str(&file.to_string_lossy());
            let u = NSURL::fileURLWithPath(&path);
            let _: () = msg_send![&*array, addObject: &*u];
        }

        array
    }
}

// ---------------------------------------------------------------------------
// iOS — UIActivityViewController.
// ---------------------------------------------------------------------------

#[cfg(target_os = "ios")]
mod ios {
    use std::ptr;
    use std::sync::Mutex;

    use block2::RcBlock;
    use futures_channel::oneshot;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, Bool};
    use objc2::{class, msg_send, msg_send_id};

    use super::*;

    #[link(name = "UIKit", kind = "framework")]
    extern "C" {}

    type Outcome = Result<ShareOutcome, ShareError>;

    pub(super) async fn share(content: &ShareContent) -> Outcome {
        let (tx, rx) = oneshot::channel::<Outcome>();
        // The completion block fires once; guard the sender so a (spec-illegal)
        // double-invoke can't double-send.
        let tx = Mutex::new(Some(tx));

        // SAFETY: documented UIActivityViewController flow. Author code (and
        // this async fn) runs on the main thread, where presentation must occur.
        let (_vc,) = unsafe {
            let items = super::items::build(content);

            let alloc: objc2::rc::Allocated<AnyObject> =
                msg_send_id![class!(UIActivityViewController), alloc];
            // initWithActivityItems:applicationActivities: — nil custom activities.
            let vc: Retained<AnyObject> = msg_send_id![
                alloc,
                initWithActivityItems: &*items,
                applicationActivities: ptr::null::<AnyObject>(),
            ];

            // completionWithItemsHandler: (activityType, completed, items, error).
            // We only need `completed`.
            // The completion handler's `completed` is an Obj-C `BOOL`, which
            // objc2 marshals as `runtime::Bool` (not Rust `bool`).
            let handler = RcBlock::new(
                move |_activity_type: *mut AnyObject,
                      completed: Bool,
                      _returned: *mut AnyObject,
                      _error: *mut AnyObject| {
                    let outcome = if completed.as_bool() {
                        Ok(ShareOutcome::Completed)
                    } else {
                        Ok(ShareOutcome::Dismissed)
                    };
                    if let Some(tx) = tx.lock().unwrap().take() {
                        let _ = tx.send(outcome);
                    }
                },
            );
            let _: () = msg_send![&*vc, setCompletionWithItemsHandler: &*handler];

            let root = root_view_controller().ok_or(ShareError::Backend(
                "no foreground view controller to present the share sheet".into(),
            ))?;

            // On iPad the activity VC is a popover and needs an anchor; point it
            // at the presenter's view so it doesn't assert. (No-op on iPhone.)
            let pop: *mut AnyObject = msg_send![&*vc, popoverPresentationController];
            if !pop.is_null() {
                let view: *mut AnyObject = msg_send![&*root, view];
                if !view.is_null() {
                    let _: () = msg_send![pop, setSourceView: view];
                }
            }

            let _: () = msg_send![
                &*root,
                presentViewController: &*vc,
                animated: true,
                completion: ptr::null::<AnyObject>(),
            ];
            (vc,)
        };

        rx.await
            .unwrap_or_else(|_| Err(ShareError::Backend("share sheet dropped".into())))
    }

    /// The key window's topmost presented view controller — the same presenter
    /// acquisition `file-export`'s iOS backend uses.
    unsafe fn root_view_controller() -> Option<Retained<AnyObject>> {
        let app: *mut AnyObject = msg_send![class!(UIApplication), sharedApplication];
        if app.is_null() {
            return None;
        }
        let window: *mut AnyObject = msg_send![app, keyWindow];
        let window = if window.is_null() {
            let windows: *mut AnyObject = msg_send![app, windows];
            if windows.is_null() {
                return None;
            }
            let count: usize = msg_send![windows, count];
            if count == 0 {
                return None;
            }
            let w: *mut AnyObject = msg_send![windows, objectAtIndex: 0usize];
            w
        } else {
            window
        };
        if window.is_null() {
            return None;
        }
        let root: *mut AnyObject = msg_send![window, rootViewController];
        let mut top = root;
        loop {
            if top.is_null() {
                return None;
            }
            let presented: *mut AnyObject = msg_send![top, presentedViewController];
            if presented.is_null() {
                break;
            }
            top = presented;
        }
        Retained::retain(top)
    }
}

// ---------------------------------------------------------------------------
// macOS — NSSharingServicePicker.
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod macos {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send, msg_send_id};

    use super::*;

    #[link(name = "AppKit", kind = "framework")]
    extern "C" {}

    // `NSRect`/`CGRect` without pulling in objc2-foundation's `NSGeometry`
    // feature: an `NSPoint` + `NSSize` of `CGFloat` (= f64 on 64-bit macOS).
    // Layout matches the C struct, so it's ABI-correct for `bounds` /
    // `showRelativeToRect:`. The hand-written `Encode` mirrors AppKit's
    // `CGRect = {CGPoint={x,y}; CGSize={w,h}}` so objc2 marshals it correctly.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CgRect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    }

    // SAFETY: `#[repr(C)]` with four `CGFloat`s, matching the `CGRect` ABI;
    // the encoding below is the Objective-C type encoding for that struct.
    unsafe impl objc2::Encode for CgRect {
        const ENCODING: objc2::Encoding = objc2::Encoding::Struct(
            "CGRect",
            &[
                objc2::Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]),
                objc2::Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]),
            ],
        );
    }

    pub(super) fn share(content: &ShareContent) -> Result<ShareOutcome, ShareError> {
        // SAFETY: documented NSSharingServicePicker flow — build items, init the
        // picker, show it relative to the key window's content view. Must run on
        // the main thread, where author code runs.
        unsafe {
            let items = super::items::build(content);

            let alloc: objc2::rc::Allocated<AnyObject> =
                msg_send_id![class!(NSSharingServicePicker), alloc];
            let picker: Retained<AnyObject> = msg_send_id![alloc, initWithItems: &*items];

            let view = key_window_content_view().ok_or(ShareError::Backend(
                "no key window to anchor the share picker".into(),
            ))?;

            // showRelativeToRect:ofView:preferredEdge: — anchor at the view's
            // bounds, min-Y edge (NSRectEdgeMinY = 1). The picker reports its
            // result via per-service delegates we don't subscribe to, so this is
            // best-effort Completed (see ShareOutcome's docs).
            let bounds: CgRect = msg_send![&*view, bounds];
            const NS_RECT_EDGE_MIN_Y: usize = 1;
            let _: () = msg_send![
                &*picker,
                showRelativeToRect: bounds,
                ofView: &*view,
                preferredEdge: NS_RECT_EDGE_MIN_Y,
            ];

            Ok(ShareOutcome::Completed)
        }
    }

    /// The key window's content view, to anchor the picker against.
    unsafe fn key_window_content_view() -> Option<Retained<AnyObject>> {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        if app.is_null() {
            return None;
        }
        let mut window: *mut AnyObject = msg_send![app, keyWindow];
        if window.is_null() {
            window = msg_send![app, mainWindow];
        }
        if window.is_null() {
            return None;
        }
        let view: *mut AnyObject = msg_send![window, contentView];
        if view.is_null() {
            return None;
        }
        Retained::retain(view)
    }
}
