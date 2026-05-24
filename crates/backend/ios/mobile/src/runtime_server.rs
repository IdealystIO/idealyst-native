//! iOS-side runtime-server-client entry point.
//!
//! Only compiled when the `runtime-server` feature is on. Provides the
//! `ios_main` / `ios_teardown` C symbols a Swift host calls and the
//! main-thread tick that bridges the [`dev_client::RuntimeServerShell`] worker
//! to the UIKit run loop.
//!
//! Almost everything cross-platform lives in `runtime-server-shell-native`:
//!
//! - [`RuntimeServerShell::tick`] does report-viewport + drain + run-layout
//!   in one call.
//! - [`runtime_server_shell_native::apple::start_dispatch_main_tick`] handles
//!   the `dispatch_async_f` background-thread pump (shared with the
//!   macOS shell — both ABIs use the same libdispatch).
//!
//! This module is the iOS-specific glue around those: building the
//! [`IosBackend`], wiring the host UIView, sampling its bounds on
//! every tick (so the sidecar's `RecordingViewOps::frame()` tracks
//! rotations + multitasking-resize), and the FFI boundary.

use std::cell::RefCell;
use std::ffi::{c_char, CStr};
use std::rc::Rc;

use runtime_server_shell_native::{RuntimeServerShell, RuntimeServerShellOptions, WirePlatform, WireViewport};
use objc2::rc::Retained;
use objc2_foundation::MainThreadMarker;
use objc2_ui_kit::UIView;

use crate::IosBackend;

thread_local! {
    /// The shell lives on the main thread for the life of the app.
    /// Dispatched tick callbacks reach it through here; the worker
    /// thread doesn't touch this (it talks to the shell only via
    /// channels established at [`RuntimeServerShell::spawn`] time).
    static SHELL: RefCell<Option<Rc<RuntimeServerShell<IosBackend>>>> = const { RefCell::new(None) };
    /// Strong reference to the host UIView so we can re-sample
    /// `.bounds` on every main-thread tick — keeps the sidecar's
    /// viewport synced through rotations + split-screen resizes.
    static HOST_VIEW: RefCell<Option<Retained<UIView>>> = const { RefCell::new(None) };
}

/// C-exported entry point called by the Swift host once from
/// `viewDidLoad`.
///
/// # Safety
/// - Must be called on the main thread.
/// - `root_view` must be a non-null, valid `UIView *`.
/// - `app_id_utf8` must be a non-null pointer to a NUL-terminated
///   UTF-8 string. It must match the dev-server's mDNS TXT record's
///   `app_id` field — typically the iOS bundle id.
#[no_mangle]
pub unsafe extern "C" fn ios_main(
    root_view: *mut std::ffi::c_void,
    app_id_utf8: *const c_char,
) {
    // Wrap the whole body in `catch_unwind` — this is an
    // `extern "C"` boundary into Swift/UIKit code that is not built
    // for Rust unwind ABI. A panic propagating out is undefined
    // behavior. The set_hook below still runs for diagnostics; the
    // catch_unwind absorbs the unwind so we return to Swift normally.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        std::panic::set_hook(Box::new(|info| {
            eprintln!("RUST PANIC: {}", info);
        }));

        // SAFETY: contract requires main-thread invocation.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };

        if app_id_utf8.is_null() {
            eprintln!("[backend-ios::aas] ios_main called with null app_id; aborting");
            return;
        }
        let app_id = unsafe { CStr::from_ptr(app_id_utf8) }
            .to_string_lossy()
            .into_owned();
        eprintln!(
            "[backend-ios::aas] starting; discovering dev-server app_id={:?}",
            app_id
        );

        let view: Retained<UIView> = match unsafe {
            Retained::retain(root_view as *mut UIView)
        } {
            Some(v) => v,
            None => {
                eprintln!(
                    "[backend-ios::aas] ios_main: root_view is null; aborting"
                );
                return;
            }
        };

        let initial_viewport = sample_viewport(&view);

        let mut backend = IosBackend::new(mtm);
        backend.set_host_root(view.clone());

        let shell = Rc::new(RuntimeServerShell::spawn_with_options(
            backend,
            app_id,
            RuntimeServerShellOptions {
                platform: WirePlatform::Ios,
                device_label: None,
                viewport: initial_viewport,
            },
        ));
        SHELL.with(|slot| *slot.borrow_mut() = Some(shell));
        HOST_VIEW.with(|slot| *slot.borrow_mut() = Some(view));

        // Hand the periodic-tick mechanism to the shared apple
        // helper — same libdispatch pump as macOS. Each tick we
        // re-sample the host UIView's bounds so viewport changes
        // (rotation, split-screen) propagate to the sidecar via
        // `RuntimeServerShell::tick`'s built-in `report_viewport` step.
        runtime_server_shell_native::apple::start_dispatch_main_tick(|| {
            let viewport = HOST_VIEW.with(|slot| {
                slot.borrow().as_ref().and_then(sample_viewport)
            });
            SHELL.with(|slot| {
                if let Some(shell) = slot.borrow().as_ref() {
                    shell.tick(viewport);
                }
            });
        });
    }));
    if let Err(payload) = result {
        let msg = panic_payload_message(payload);
        eprintln!("[backend-ios::aas] ios_main panicked: {msg}");
    }
}

/// Tear down the active mount. Called by the Swift host from
/// `applicationWillTerminate` or wherever the app shuts down.
#[no_mangle]
pub unsafe extern "C" fn ios_teardown() {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        SHELL.with(|slot| slot.borrow_mut().take());
        HOST_VIEW.with(|slot| slot.borrow_mut().take());
    }));
    if let Err(payload) = result {
        let msg = panic_payload_message(payload);
        eprintln!("[backend-ios::aas] ios_teardown panicked: {msg}");
    }
}

/// Read the UIView's current bounds as a `WireViewport`. Returns
/// `None` pre-layout (bounds are zero) so the initial Hello falls
/// back to the sidecar's mobile default instead of shipping
/// nonsensical zeros.
fn sample_viewport(view: &Retained<UIView>) -> Option<WireViewport> {
    let bounds = view.bounds();
    if bounds.size.width > 0.0 && bounds.size.height > 0.0 {
        Some(WireViewport {
            width: bounds.size.width as f32,
            height: bounds.size.height as f32,
        })
    } else {
        None
    }
}

fn panic_payload_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else {
        "<non-string panic payload>".to_string()
    }
}
