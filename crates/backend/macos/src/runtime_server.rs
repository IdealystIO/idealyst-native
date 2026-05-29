//! macOS-side runtime-server-client entry point.
//!
//! Only compiled when the `runtime-server` feature is on. Provides the
//! Rust helpers `host-appkit::run_aas` uses to run the AppKit app
//! as a thin client of an runtime-server dev-server.
//!
//! Almost everything cross-platform lives in `runtime-server-shell-native`:
//!
//! - [`RuntimeServerShell::tick`] does report-viewport + drain + run-layout
//!   in one call.
//! - [`runtime_server_shell_native::apple::start_dispatch_main_tick`] handles
//!   the `dispatch_async_f` background-thread pump (shared with the
//!   iOS shell — both ABIs use the same libdispatch).
//!
//! This module is the macOS-specific glue: building the
//! [`MacosBackend`], stashing the shell in a main-thread-local, and
//! sampling the host NSView's bounds on every tick so the sidecar's
//! viewport stays in sync with window resizes.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_server_shell_native::{RuntimeServerShell, RuntimeServerShellOptions, WirePlatform, WireViewport};
use objc2::msg_send;
use objc2::rc::Retained;
use objc2_app_kit::NSView;
use objc2_foundation::CGRect;

use crate::MacosBackend;

thread_local! {
    /// The shell lives on the main thread for the life of the app.
    /// The dispatched tick callbacks reach it through here; the
    /// worker thread doesn't touch this (it talks to the shell only
    /// via channels established at [`RuntimeServerShell::spawn`] time).
    static SHELL: RefCell<Option<Rc<RuntimeServerShell<MacosBackend>>>> = const { RefCell::new(None) };
    /// Strong reference to the host NSView for per-tick bounds
    /// re-sampling — keeps the sidecar's viewport synced through
    /// NSWindow resizes.
    static HOST_VIEW: RefCell<Option<Retained<NSView>>> = const { RefCell::new(None) };
}

/// Connect to the dev-server at `url`, spawn the runtime-server
/// worker, and stash the shell handle in the main-thread-local
/// [`SHELL`] slot. Returns the shell handle for the caller to
/// additionally retain if it wants to drive layout, send outbound
/// events, etc.
///
/// Must be called on the main thread (the shell holds the backend
/// by `Rc<RefCell<...>>` and the drain timer is main-thread-only).
///
/// `url` is `ws://host:port` — the CLI bakes this into the spawned
/// macOS binary via the `IDEALYST_DEV_ENDPOINT` env var; the wrapper
/// reads it via [`runtime_server_shell_native::endpoint_or_panic`].
///
/// `device_label` is an optional human label the dev-server logs
/// next to the platform tag — useful when multiple desktop clients
/// connect to the same server.
///
/// `host_root` is the AppKit `NSView` we'll sample bounds from on
/// every tick to keep the sidecar's viewport synced. If `None`,
/// per-tick viewport reporting is skipped and only the initial
/// `viewport_size` (in `RuntimeServerShellOptions.viewport`) is used.
pub fn spawn_runtime_server_shell(
    backend: MacosBackend,
    url: impl Into<String>,
    device_label: Option<String>,
    viewport_size: Option<(f32, f32)>,
    host_root: Option<Retained<NSView>>,
) -> Rc<RuntimeServerShell<MacosBackend>> {
    let viewport = viewport_size.map(|(w, h)| WireViewport { width: w, height: h });
    let shell = Rc::new(RuntimeServerShell::spawn_with_options(
        backend,
        url.into(),
        RuntimeServerShellOptions {
            // `WirePlatform::Macos` doesn't exist yet on the shared
            // platform enum; use `Other` so the server's session-
            // assignment logic treats us as a generic native client
            // until the enum grows a `Macos` variant.
            platform: WirePlatform::Other,
            device_label,
            viewport,
        },
    ));
    SHELL.with(|slot| *slot.borrow_mut() = Some(shell.clone()));
    HOST_VIEW.with(|slot| *slot.borrow_mut() = host_root);
    shell
}

/// Start the libdispatch-backed main-thread tick. Each tick re-
/// samples the host NSView's bounds (so `NSWindow` resizes
/// propagate to the sidecar) and calls [`RuntimeServerShell::tick`], which
/// internally handles `report_viewport` + `drain` (which sends
/// `RequestFrame`) + `run_layout` (no-op on macOS — `finish()`
/// applies frames synchronously).
pub fn start_main_thread_drain_timer() {
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
}

/// Tear down the active mount. Wired by `host-appkit`'s app
/// delegate (`applicationWillTerminate:` or equivalent). Idempotent
/// — second call is a no-op.
pub fn teardown() {
    SHELL.with(|slot| slot.borrow_mut().take());
    HOST_VIEW.with(|slot| slot.borrow_mut().take());
}

/// Read the NSView's current bounds as a `WireViewport`. Returns
/// `None` when bounds are zero (pre-`makeKeyAndOrderFront`) so the
/// shell's deduper doesn't spam `ViewportChanged` with garbage
/// values before the window is realized.
fn sample_viewport(view: &Retained<NSView>) -> Option<WireViewport> {
    let bounds: CGRect = unsafe { msg_send![view, bounds] };
    if bounds.size.width > 0.0 && bounds.size.height > 0.0 {
        let width = bounds.size.width as f32;
        let height = bounds.size.height as f32;
        // Mirror the wire-side report into the framework's reactive
        // viewport signal. `set_viewport_size` dedupes by equality so
        // the per-tick sample only re-fires subscribers on actual
        // resize.
        runtime_core::set_viewport_size(runtime_core::ViewportSize { width, height });
        Some(WireViewport { width, height })
    } else {
        None
    }
}
