//! iOS-side AAS-client entry point.
//!
//! Only compiled when the `aas-shell` feature is on. Provides the
//! `ios_main` and `ios_teardown` C symbols a Swift host calls, plus
//! the main-thread drain timer that consumes inbound wire commands
//! arriving from a [`dev_client::AasShell`] worker.
//!
//! Everything not iOS-specific lives in
//! [`dev_client::aas_shell`] — discovery, WebSocket connect /
//! reconnect, message dispatch. This module is just the platform
//! glue: building [`IosBackend`], wiring the host UIView, and
//! scheduling work on the main run loop via `dispatch_async_f`.

use std::cell::RefCell;
use std::ffi::{c_char, CStr};
use std::rc::Rc;
use std::time::Duration;

use aas_shell_native::AasShell;
use framework_core::{install_theme, ThemeTokens, TokenEntry};
use objc2::rc::Retained;
use objc2_foundation::MainThreadMarker;
use objc2_ui_kit::UIView;

use crate::IosBackend;

/// Placeholder theme installed on AAS-client startup. Tokens are
/// empty; real token values arrive over the wire as
/// `Command::InstallThemeVariables` and are forwarded to the
/// backend separately. The iOS backend's
/// `install_theme_transition_effect` (invoked from `finish()`)
/// subscribes to `active_theme()` and panics if no theme is
/// installed — in normal native builds, the user's `app()` calls
/// `install_theme(...)` before render. In AAS mode `app()` runs on
/// the dev-server's sidecar, so we need this stub on the client.
struct AasPlaceholderTheme;
impl ThemeTokens for AasPlaceholderTheme {
    fn tokens(&self) -> Vec<TokenEntry> {
        Vec::new()
    }
}

thread_local! {
    /// The shell lives on the main thread for the life of the app.
    /// `dispatch_async_f`-scheduled drain callbacks reach it through
    /// here; the worker thread doesn't touch this (it talks to the
    /// shell only via channels established at [`AasShell::spawn`]
    /// time).
    static SHELL: RefCell<Option<Rc<AasShell<IosBackend>>>> = const { RefCell::new(None) };
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

    // Take a strong reference to the host UIView so it can't be
    // dropped while the AAS pipeline is wiring it up.
    let view: Retained<UIView> = unsafe {
        Retained::retain(root_view as *mut UIView)
            .expect("ios_main: root_view must be non-null")
    };

    // Install the placeholder theme before constructing the
    // backend — the IosBackend's `finish()` later subscribes to
    // `active_theme()` and panics if no theme has been installed
    // by then. The real theme tokens still come in over the wire.
    install_theme(AasPlaceholderTheme);

    let mut backend = IosBackend::new(mtm);
    backend.set_host_root(view);

    // The AAS shell owns the backend after spawn — main-thread
    // access from here on goes through `shell.client.borrow_mut()
    // .backend_mut()`.
    let shell = Rc::new(AasShell::spawn(backend, app_id));
    SHELL.with(|slot| *slot.borrow_mut() = Some(shell));

    start_main_thread_drain_timer();
}

/// Periodic main-thread drain. A background thread sleeps ~16 ms,
/// then `dispatch_async_f`-s a closure onto the main run loop that
/// pops pending DevToApp messages from the shell's inbound channel
/// and applies them. After each non-empty batch we kick the iOS
/// backend's layout pass — in AAS mode there's no `IOS_BACKEND_SELF`
/// global to register against, so layout is driven explicitly here.
fn start_main_thread_drain_timer() {
    extern "C" {
        static _dispatch_main_q: std::ffi::c_void;
        fn dispatch_async_f(
            queue: *const std::ffi::c_void,
            context: *mut std::ffi::c_void,
            work: extern "C" fn(*mut std::ffi::c_void),
        );
    }
    extern "C" fn do_drain(_ctx: *mut std::ffi::c_void) {
        drain_on_main();
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

/// Runs on the main thread. Drains the shell's inbound channel and,
/// if anything was applied, kicks the iOS layout pass.
fn drain_on_main() {
    SHELL.with(|slot| {
        let shell = slot.borrow().clone();
        let Some(shell) = shell else { return };
        if shell.drain() {
            // The iOS backend defers Taffy layout via
            // `IOS_BACKEND_SELF::schedule_layout_pass()` in the
            // normal native flow. In AAS mode that global isn't
            // installed (no `Rc<RefCell<IosBackend>>` to register
            // — the backend lives inside the AasClient), so we
            // drive layout here at the end of each batch.
            shell.client.borrow_mut().backend_mut().run_layout();
        }
    });
}

/// Tear down the active mount. Called by the Swift host from
/// `applicationWillTerminate` or wherever the app shuts down.
#[no_mangle]
pub unsafe extern "C" fn ios_teardown() {
    SHELL.with(|slot| slot.borrow_mut().take());
}
