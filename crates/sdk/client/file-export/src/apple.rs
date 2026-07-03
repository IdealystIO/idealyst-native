//! Apple save UI: `NSSavePanel` on macOS, `UIDocumentPickerViewController`
//! (export) on iOS — "Save to Files". Driven through the Obj-C runtime, the
//! same posture `camera`/`media-writer` take.
//!
//! macOS is **modal and synchronous** (`runModal`), so it just blocks the
//! main thread until dismissed. iOS is **delegate-driven**: we present the
//! picker, bridge its `didPickDocumentsAtURLs:` / `wasCancelled:` callbacks to
//! a oneshot, and await it — the same callback→future pattern the camera
//! permission flow uses.

#[cfg(target_os = "macos")]
use std::path::Path;

use crate::{ExportError, SaveOutcome, SaveRequest, Source};

#[cfg(target_os = "macos")]
pub(crate) async fn save(request: SaveRequest) -> Result<SaveOutcome, ExportError> {
    macos::save(request)
}

#[cfg(target_os = "ios")]
pub(crate) async fn save(request: SaveRequest) -> Result<SaveOutcome, ExportError> {
    ios::save(request).await
}

/// Write a [`Source`] to an absolute filesystem path (macOS, where the panel
/// returns a real destination path we own).
#[cfg(target_os = "macos")]
fn write_source_to(source: &Source, dest: &Path) -> Result<(), ExportError> {
    match source {
        Source::Bytes(bytes) => std::fs::write(dest, bytes).map_err(|e| ExportError::Io(e.to_string())),
        Source::Path(src) => std::fs::copy(src, dest)
            .map(|_| ())
            .map_err(|e| ExportError::Io(e.to_string())),
    }
}

// ---------------------------------------------------------------------------
// macOS — NSSavePanel.
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod macos {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send, msg_send_id};
    use objc2_foundation::NSString;

    use super::*;

    #[link(name = "AppKit", kind = "framework")]
    extern "C" {}

    // NSModalResponseOK.
    const MODAL_RESPONSE_OK: isize = 1;

    pub(super) fn save(request: SaveRequest) -> Result<SaveOutcome, ExportError> {
        // SAFETY: a straight transcription of the documented NSSavePanel flow
        // (savePanel → set suggested name → runModal → read URL). runModal is
        // app-modal and must run on the main thread, which is where author
        // code (and this async fn) runs.
        unsafe {
            let panel: Retained<AnyObject> = msg_send_id![class!(NSSavePanel), savePanel];
            let name = NSString::from_str(&request.suggested_name);
            let _: () = msg_send![&*panel, setNameFieldStringValue: &*name];
            let _: () = msg_send![&*panel, setCanCreateDirectories: true];

            let response: isize = msg_send![&*panel, runModal];
            if response != MODAL_RESPONSE_OK {
                return Ok(SaveOutcome::Cancelled);
            }

            let url: *mut AnyObject = msg_send![&*panel, URL];
            if url.is_null() {
                return Err(ExportError::Backend("save panel returned no URL".into()));
            }
            let path_ns: *mut AnyObject = msg_send![url, path];
            if path_ns.is_null() {
                return Err(ExportError::Backend("save URL has no path".into()));
            }
            let path = nsstring_to_string(path_ns);
            write_source_to(&request.source, Path::new(&path))?;
            Ok(SaveOutcome::Saved {
                location: Some(path),
            })
        }
    }

    /// Copy an `NSString*` into a Rust `String` via its UTF-8 view.
    unsafe fn nsstring_to_string(s: *mut AnyObject) -> String {
        let s = &*(s as *const NSString);
        s.to_string()
    }
}

// ---------------------------------------------------------------------------
// iOS — UIDocumentPickerViewController (export).
// ---------------------------------------------------------------------------

#[cfg(target_os = "ios")]
mod ios {
    use std::ptr;
    use std::sync::Mutex;

    use futures_channel::oneshot;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObjectProtocol};
    use objc2::{class, declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
    use objc2_foundation::{NSArray, NSObject, NSString, NSURL};

    use super::*;

    #[link(name = "UIKit", kind = "framework")]
    extern "C" {}

    type Outcome = Result<SaveOutcome, ExportError>;

    pub(super) struct DelegateIvars {
        tx: Mutex<Option<oneshot::Sender<Outcome>>>,
    }

    declare_class!(
        struct ExportDelegate;

        unsafe impl ClassType for ExportDelegate {
            type Super = NSObject;
            type Mutability = mutability::InteriorMutable;
            const NAME: &'static str = "IdealystFileExportDelegate";
        }

        impl DeclaredClass for ExportDelegate {
            type Ivars = DelegateIvars;
        }

        unsafe impl NSObjectProtocol for ExportDelegate {}

        // UIDocumentPickerDelegate. We don't import the typed protocol; UIKit
        // calls these selectors by name, so implementing them is enough.
        unsafe impl ExportDelegate {
            #[method(documentPicker:didPickDocumentsAtURLs:)]
            fn did_pick(&self, _picker: *mut AnyObject, urls: *mut AnyObject) {
                // First URL is the exported file's destination.
                let location = unsafe { first_url_path(urls) };
                self.resolve(Ok(SaveOutcome::Saved { location }));
            }

            #[method(documentPickerWasCancelled:)]
            fn was_cancelled(&self, _picker: *mut AnyObject) {
                self.resolve(Ok(SaveOutcome::Cancelled));
            }
        }
    );

    impl ExportDelegate {
        fn resolve(&self, outcome: Outcome) {
            if let Some(tx) = self.ivars().tx.lock().unwrap().take() {
                let _ = tx.send(outcome);
            }
        }
    }

    /// Path of the first `NSURL` in an `NSArray<NSURL*>*`, or `None`.
    unsafe fn first_url_path(urls: *mut AnyObject) -> Option<String> {
        if urls.is_null() {
            return None;
        }
        let count: usize = msg_send![urls, count];
        if count == 0 {
            return None;
        }
        let url: *mut AnyObject = msg_send![urls, objectAtIndex: 0usize];
        if url.is_null() {
            return None;
        }
        let path_ns: *mut AnyObject = msg_send![url, path];
        if path_ns.is_null() {
            return None;
        }
        Some((*(path_ns as *const NSString)).to_string())
    }

    pub(super) async fn save(request: SaveRequest) -> Outcome {
        // The picker exports an on-disk file; for `Bytes`, stage a temp file.
        let src_path = match &request.source {
            Source::Path(p) => p.clone(),
            Source::Bytes(bytes) => {
                let tmp = std::env::temp_dir().join(&request.suggested_name);
                std::fs::write(&tmp, bytes).map_err(|e| ExportError::Io(e.to_string()))?;
                tmp
            }
        };

        let (tx, rx) = oneshot::channel::<Outcome>();

        // SAFETY: documented UIDocumentPickerViewController export flow.
        // Everything runs on the main thread (author code does).
        let (_picker, _delegate) = unsafe {
            let url = NSURL::fileURLWithPath(&NSString::from_str(&src_path.to_string_lossy()));
            let urls = NSArray::from_slice(&[&*url]);

            let alloc = ExportDelegate::alloc().set_ivars(DelegateIvars {
                tx: Mutex::new(Some(tx)),
            });
            let delegate: Retained<ExportDelegate> = msg_send_id![super(alloc), init];

            let picker_alloc: objc2::rc::Allocated<AnyObject> =
                msg_send_id![class!(UIDocumentPickerViewController), alloc];
            // iOS 14+: initForExportingURLs: copies the files to the chosen
            // location (the "Save a copy" flow).
            let picker: Retained<AnyObject> =
                msg_send_id![picker_alloc, initForExportingURLs: &*urls];
            let _: () = msg_send![&*picker, setDelegate: &*delegate];

            let root = root_view_controller().ok_or(ExportError::NoPresenter)?;
            let _: () = msg_send![
                &*root,
                presentViewController: &*picker,
                animated: true,
                completion: ptr::null::<AnyObject>(),
            ];
            (picker, delegate)
        };

        // Hold the picker + delegate alive across the await; the delegate
        // resolves the oneshot from its callback.
        rx.await
            .unwrap_or_else(|_| Err(ExportError::Backend("picker dropped".into())))
    }

    /// The key window's root view controller, to present the picker from.
    unsafe fn root_view_controller() -> Option<Retained<AnyObject>> {
        let app: *mut AnyObject = msg_send![class!(UIApplication), sharedApplication];
        if app.is_null() {
            return None;
        }
        // keyWindow is deprecated on iOS 13+ but honored at the framework's
        // iOS-16 floor; it's the simplest reliable presenter handle.
        let window: *mut AnyObject = msg_send![app, keyWindow];
        let window = if window.is_null() {
            // Fall back to the first window in `windows`.
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
        // Use the topmost presented controller so we don't present on top of
        // an already-modal one.
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
