//! macOS-side AAS-client entry point.
//!
//! Only compiled when the `aas-shell` feature is on. Provides the
//! Rust helpers `host-appkit::run_aas` uses to run the AppKit app
//! as a thin client of an AAS dev-server:
//!
//! - [`spawn_aas_shell`] — discovers the dev-server via mDNS,
//!   spawns the worker-thread WebSocket loop, returns the shell
//!   handle.
//! - [`start_main_thread_drain_timer`] — schedules a periodic
//!   `dispatch_async_f` onto the main run loop that pops pending
//!   `DevToApp` commands off the shell's inbound channel and applies
//!   them through the [`MacosBackend`].
//!
//! Mirrors [`backend-ios-mobile`'s `aas` module][ios-aas] — same
//! `AasShell<B>` worker, same libdispatch main-thread drain — but
//! exposes pure Rust functions (host-appkit is Rust, no FFI dance)
//! and reaches into the macOS backend's layout pass on each batch.
//!
//! [ios-aas]: ../../../ios/mobile/src/aas.rs
//!
//! Everything not macOS-specific lives in [`aas_shell_native`];
//! this module is just the platform glue.
//!
//! # Threading
//!
//! The shell handle lives on the main thread for the life of the
//! app. The worker thread inside `AasShell::spawn_with_options`
//! runs the WebSocket loop and talks to the shell only via channels
//! established at spawn time — it never touches the [`SHELL`]
//! thread-local. The main-thread drain timer is the only consumer.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use aas_shell_native::{AasShell, AasShellOptions, WirePlatform, WireViewport};

use crate::MacosBackend;

thread_local! {
    /// The shell lives on the main thread for the life of the app.
    /// `dispatch_async_f`-scheduled drain callbacks reach it through
    /// here; the worker thread doesn't touch this (it talks to the
    /// shell only via channels established at [`AasShell::spawn`]
    /// time).
    static SHELL: RefCell<Option<Rc<AasShell<MacosBackend>>>> = const { RefCell::new(None) };
}

/// Discover the dev-server, spawn the AAS worker, and stash the
/// shell handle in the main-thread-local [`SHELL`] slot. Returns
/// the shell handle for the caller to additionally retain if it
/// wants to drive layout, send outbound events, etc.
///
/// Must be called on the main thread (the shell holds the backend
/// by `Rc<RefCell<...>>` and the drain timer is main-thread-only).
///
/// `device_label` is an optional human label the dev-server logs
/// next to the platform tag — useful when multiple desktop clients
/// connect to the same server.
/// Spawn the AAS shell with the given backend, app id, optional
/// device label, and optional initial viewport size in points (the
/// AppKit window's content area).
///
/// The viewport is shipped to the sidecar in
/// `AppToDev::Hello.viewport`; the sidecar uses it to answer
/// `page_ref.frame()` calls so author code's viewport-relative
/// math (welcome's planet orbit) renders against the real window
/// dimensions. Pass `None` for the viewport if you don't know the
/// size; the sidecar falls back to a hardcoded `393×800` mobile-
/// portrait canvas, which is fine for testing but misaligns
/// anything sized to the real host.
pub fn spawn_aas_shell(
    backend: MacosBackend,
    app_id: impl Into<String>,
    device_label: Option<String>,
    viewport_size: Option<(f32, f32)>,
) -> Rc<AasShell<MacosBackend>> {
    let viewport = viewport_size.map(|(w, h)| WireViewport { width: w, height: h });
    let shell = Rc::new(AasShell::spawn_with_options(
        backend,
        app_id.into(),
        AasShellOptions {
            // `WirePlatform::Macos` doesn't exist yet on the shared
            // platform enum; use `Other` so the server's session-
            // assignment logic treats us as a generic native client
            // until the enum grows a `Macos` variant. The dev-server
            // currently keys session decisions off `app_id`, not
            // platform, so this doesn't change behavior.
            platform: WirePlatform::Other,
            device_label,
            // Real native window size — the sidecar feeds this into
            // `RecordingViewOps::frame()` so welcome's planet-orbit
            // math (and any other `page_ref.frame()` reader) uses
            // the actual viewport instead of the 393×800 fallback.
            viewport,
        },
    ));
    SHELL.with(|slot| *slot.borrow_mut() = Some(shell.clone()));
    shell
}

/// Periodic main-thread drain. A background thread sleeps ~16 ms,
/// then `dispatch_async_f`-s a closure onto the main run loop that
/// pops pending `DevToApp` messages from the shell's inbound channel
/// and applies them. Mirrors the iOS implementation — both use
/// libdispatch, which is the same library on macOS.
pub fn start_main_thread_drain_timer() {
    extern "C" {
        static _dispatch_main_q: std::ffi::c_void;
        fn dispatch_async_f(
            queue: *const std::ffi::c_void,
            context: *mut std::ffi::c_void,
            work: extern "C" fn(*mut std::ffi::c_void),
        );
    }
    extern "C" fn do_drain(_ctx: *mut std::ffi::c_void) {
        // libdispatch returns into Apple-side C code that doesn't
        // expect Rust unwinding. Absorb any panic from drain_on_main
        // (e.g. a user-code panic during apply) so it never crosses
        // the FFI boundary as undefined behavior.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            drain_on_main();
        }));
        if let Err(payload) = result {
            let msg = panic_payload_message(payload);
            eprintln!("[backend-macos::aas] drain panic absorbed: {msg}");
        }
    }
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(16));
        unsafe {
            dispatch_async_f(
                &_dispatch_main_q as *const _ as *const std::ffi::c_void,
                std::ptr::null_mut(),
                do_drain,
            );
        }
    });
}

/// Runs on the main thread. Drains the shell's inbound channel,
/// applying every queued `DevToApp` through the backend. macOS's
/// layout pass runs inside `Backend::finish(root)`, which the
/// command stream invokes via `Command::Finish` — so unlike iOS we
/// don't need to kick layout explicitly here.
fn drain_on_main() {
    SHELL.with(|slot| {
        let shell = slot.borrow().clone();
        let Some(shell) = shell else { return };
        let _ = shell.drain();
    });
}

/// Tear down the active mount. Wired by `host-appkit`'s app
/// delegate (`applicationWillTerminate:` or equivalent). Idempotent
/// — second call is a no-op.
pub fn teardown() {
    SHELL.with(|slot| slot.borrow_mut().take());
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
