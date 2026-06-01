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
/// - `endpoint_utf8` must be a non-null pointer to a NUL-terminated
///   UTF-8 string of the form `ws://host:port`. The Swift host reads
///   it from the Info.plist `IdealystDevEndpoint` key the CLI bakes
///   in at `idealyst dev` time.
// The `#[no_mangle]` C entry symbols (`ios_main` / `ios_teardown`)
// are gated behind `entry-symbols` (OFF by default). The RS-shell
// crate (`backend-ios-rs-shell`) defines its OWN `ios_main` /
// `ios_teardown` C symbols (calling `ios_main_with_register` +
// `ios_teardown_impl` below), so if THIS crate also emitted them the
// final swiftc link would hit a duplicate-symbol error. The shell is
// the sole linked staticlib now, so it owns the C symbols; this
// feature exists only for a hypothetical consumer that links
// `backend-ios-mobile` directly without the shell.
#[cfg(feature = "entry-symbols")]
#[no_mangle]
pub unsafe extern "C" fn ios_main(
    root_view: *mut std::ffi::c_void,
    endpoint_utf8: *const c_char,
) {
    // Back-compat entry: no SDK handlers registered.
    unsafe { ios_main_with_register(root_view, endpoint_utf8, |_backend| {}) }
}

/// Like [`ios_main`] but lets the caller register SDK extension
/// handlers on the freshly-built [`IosBackend`] before the shell
/// spawns. This is the seam the RS-shell crate uses to bundle the
/// first-party SDKs into the (otherwise app-agnostic) runtime-server
/// client: the web RS client registers SDKs via the generated
/// per-app wrapper's `register_extensions`, but the iOS RS client is
/// a fixed staticlib that links no user code — so the SDK set is
/// compiled in here, fixed at build time (like React Native native
/// modules). `register` runs after `IosBackend::new` and before
/// `RuntimeServerShell::spawn_with_options`, matching the local-mount
/// ordering where `register_extensions` runs before the first mount.
///
/// `backend-ios-mobile` itself stays SDK-free (the SDKs depend on it,
/// so it can't depend back without a cycle); the `register` closure
/// is supplied by a crate above both.
///
/// Not a `#[no_mangle] extern "C"` symbol — the only caller is the
/// Rust-side RS-shell crate, and the `fn(&mut IosBackend)` argument
/// isn't FFI-safe. Swift never calls this directly; it calls the
/// shell's `ios_main`, which forwards here.
///
/// # Safety
/// Same contract as [`ios_main`].
pub unsafe fn ios_main_with_register(
    root_view: *mut std::ffi::c_void,
    endpoint_utf8: *const c_char,
    register: fn(&mut IosBackend),
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

        if endpoint_utf8.is_null() {
            eprintln!("[backend-ios::aas] ios_main called with null endpoint; aborting");
            return;
        }
        let endpoint = unsafe { CStr::from_ptr(endpoint_utf8) }
            .to_string_lossy()
            .into_owned();
        if endpoint.is_empty() {
            eprintln!(
                "[backend-ios::aas] ios_main called with empty IDEALYST_DEV_ENDPOINT; aborting. \
                 Rebuild via `idealyst dev` to bake the dev-server URL into the wrapper."
            );
            return;
        }
        eprintln!(
            "[backend-ios::aas] starting; connecting to dev-server at {:?}",
            endpoint
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

        // Install the libdispatch-backed scheduler on the main thread
        // BEFORE anything that can defer work. Without it,
        // `runtime_core::schedule_microtask` / `after_ms` fall back to
        // running their closure SYNCHRONOUSLY (scheduling.rs: no
        // installed scheduler → `f()` inline). The SDK drawer handler
        // defers its sidebar build via `schedule_microtask` precisely
        // so the `backend.borrow_mut()` window held across
        // `create_navigator` is released before the walker re-enters
        // via `build_node` → `build_detached` → `walker/view::build` →
        // `backend.borrow_mut()`. Synchronous fallback re-enters that
        // borrow mid-`create_navigator` → `RefCell already borrowed`
        // panic → `catch_unwind` aborts the batch mid-apply →
        // incomplete tree → blank white. With the AppleScheduler
        // installed, `schedule_microtask` routes through
        // `dispatch_async(main_q)`; since `apply_batch` runs on the
        // main queue's serial FIFO, the deferred build runs in a LATER
        // turn, after the borrow releases. Tokens (installed on the
        // apply thread by the `InstallThemeVariables` replay) stay
        // valid: the deferred build runs on the SAME main thread, so
        // the thread-local token registry is the same one.
        crate::install_scheduler();

        let mut backend = IosBackend::new(mtm);
        backend.set_host_root(view.clone());

        // Register the compiled-in SDK handlers (no-op for the
        // back-compat `ios_main`). Must run before the shell spawns so
        // the first wire `create_navigator` / `create_external` finds
        // the handler + the wire presentation factory.
        register(&mut backend);

        let shell = Rc::new(RuntimeServerShell::spawn_with_options(
            backend,
            endpoint,
            RuntimeServerShellOptions {
                platform: WirePlatform::Ios,
                device_label: None,
                viewport: initial_viewport,
            },
        ));

        // Install the backend self-handle so SDK code reached outside
        // the framework's normal call paths can drive a layout pass.
        // The runtime-server `IosBackend` is owned by the client (an
        // `Rc<RefCell<IosBackend>>`), NOT moved by value as the
        // surrounding comments imply — so a usable weak ref DOES exist.
        // Without this, `with_backend(...)` (e.g. the drawer handler's
        // deferred `drawer_attach_sidebar` → `b.run_layout()` at
        // ios-navigator-helpers/tab_drawer.rs) returns `None` and the
        // sidebar's standalone (parentless) Taffy node is never
        // computed → its UIView frame stays 0×0 → the drawer scrim
        // darkens on open but the sidebar panel is invisible. The
        // sidebar is attached in a `schedule_microtask`-deferred build
        // that runs in a LATER main-queue turn, AFTER the per-batch
        // `run_layout` the shell tick performs — so only an explicit
        // post-attach layout pass (via this handle) sizes it.
        let backend_rc = shell.client.borrow().backend().clone();
        crate::install_global_self(Rc::downgrade(&backend_rc));
        drop(backend_rc);

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
        std::process::abort();
    }
}

/// Tear down the active mount. The shared implementation; the C entry
/// symbol (gated `ios_teardown`) and the RS-shell's `ios_teardown`
/// both call this so there's only ever ONE `#[no_mangle] ios_teardown`
/// in the final link.
pub unsafe fn ios_teardown_impl() {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        SHELL.with(|slot| slot.borrow_mut().take());
        HOST_VIEW.with(|slot| slot.borrow_mut().take());
    }));
    if let Err(payload) = result {
        let msg = panic_payload_message(payload);
        eprintln!("[backend-ios::aas] ios_teardown panicked: {msg}");
        std::process::abort();
    }
}

/// Tear down the active mount. Called by the Swift host from
/// `applicationWillTerminate` or wherever the app shuts down. Gated —
/// see the note on `ios_main`.
#[cfg(feature = "entry-symbols")]
#[no_mangle]
pub unsafe extern "C" fn ios_teardown() {
    unsafe { ios_teardown_impl() }
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
