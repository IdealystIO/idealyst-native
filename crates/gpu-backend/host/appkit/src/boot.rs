//! The windowed boot. macOS-only.
//!
//! [`run`] does the NSApplication / NSWindow / content-view sequence and
//! mounts through `backend_macos::newcore::start` — World + Registry +
//! `realize` + `Backend::finish` + post-mount flush. The window's
//! content view is handed to the backend via `create_host_root` →
//! `setContentView` → `set_host_root`.
//!
//! Mount buffering: this function opens the buffering window
//! (`begin_mount_buffering`) before calling `start` and closes it after
//! — `newcore::start` performs the synchronous pre-`finish` drain (see
//! its module docs, step 3), so deferred build microtasks land in the
//! first paint. Compare `host_winit::run`, whose scheduler has no
//! buffering window.

use std::cell::RefCell;
use std::rc::Rc;

use backend_macos::newcore::{NewCoreApp, SceneElement, SceneRegistry};
use backend_macos::MacosBackend;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSView, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::{
    CGSize, MainThreadMarker, NSActivityOptions, NSPoint, NSProcessInfo, NSRect, NSSize, NSString,
};

use crate::app::{install_main_menu, spawn_launcher_watchdog};
use crate::{RunError, RunOptions};

/// Boot NSApplication, open the host window, install the backend, mount
/// `build()`'s tree, and enter the AppKit run loop. Returns only when
/// the user quits.
///
/// Equivalent to [`run_with`] with a no-op registry seam.
pub fn run<F: FnOnce() -> SceneElement>(build: F, opts: RunOptions) -> Result<(), RunError> {
    run_with(build, opts, |_| {})
}

/// Like [`run`], but invokes `register` with the scene registry after
/// `register_builtins`, so apps/SDKs can add their own payload handlers
/// before the tree realizes.
pub fn run_with<F, R>(build: F, opts: RunOptions, register: R) -> Result<(), RunError>
where
    F: FnOnce() -> SceneElement,
    R: FnOnce(&mut SceneRegistry<MacosBackend>),
{
    let Some(mtm) = MainThreadMarker::new() else {
        return Err(RunError::NotMainThread);
    };

    spawn_launcher_watchdog();

    let nsapp = NSApplication::sharedApplication(mtm);
    nsapp.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    install_main_menu(mtm, &nsapp);

    // App Nap exemption (robot bridge stays responsive backgrounded).
    let _app_nap_assertion = {
        let reason = NSString::from_str(
            "idealyst dev: keep the robot bridge responsive in the background",
        );
        unsafe {
            NSProcessInfo::processInfo().beginActivityWithOptions_reason(
                NSActivityOptions::NSActivityUserInitiatedAllowingIdleSystemSleep,
                &reason,
            )
        }
    };

    // Window-close == app-quit delegate; retained for the run loop's
    // lifetime (setDelegate: is a weak assign).
    let app_delegate = crate::app_delegate::IdealystAppDelegate::new(mtm);
    let delegate_proto: &objc2::runtime::ProtocolObject<dyn objc2_app_kit::NSApplicationDelegate> =
        objc2::runtime::ProtocolObject::from_ref(&*app_delegate);
    nsapp.setDelegate(Some(delegate_proto));

    // Scheduler BEFORE anything that schedules; the flush driver rides
    // schedule_microtask/raf_loop, so this is load-bearing for the
    // driver too, not just after_ms.
    backend_apple_core::scheduler::install_scheduler();
    backend_macos::install_render_loop();
    backend_apple_core::log::install_logger();

    // ── Window (IDEALYST_WINDOW_SIZE override) ────────────────────────
    let (win_w, win_h) = std::env::var("IDEALYST_WINDOW_SIZE")
        .ok()
        .and_then(|s| {
            let (w, h) = s.split_once(['x', 'X', ','])?;
            Some((w.trim().parse::<f64>().ok()?, h.trim().parse::<f64>().ok()?))
        })
        .filter(|(w, h)| *w > 0.0 && *h > 0.0)
        .unwrap_or((opts.width, opts.height));
    let frame = NSRect {
        origin: NSPoint { x: 200.0, y: 200.0 },
        size: NSSize {
            width: win_w,
            height: win_h,
        },
    };
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Resizable
        | NSWindowStyleMask::Miniaturizable;
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            mtm.alloc(),
            frame,
            style,
            NSBackingStoreType::NSBackingStoreBuffered,
            false,
        )
    };
    window.setTitle(&NSString::from_str(&opts.title));

    // ── Content view handoff — the root-attachment mechanism ──────────
    // Backend-created FlippedView as contentView + explicit pre-realize
    // frame so the first Taffy pass sees a non-zero viewport (AppKit
    // only sizes the content view after the window orders front, so a
    // pre-realize frame is what keeps the first pass off 0×0).
    let mut backend = MacosBackend::new(mtm);
    let host_root: Retained<NSView> = match backend.create_host_root() {
        Some(v) => v,
        None => unsafe {
            NSView::initWithFrame(
                mtm.alloc(),
                NSRect {
                    origin: NSPoint { x: 0.0, y: 0.0 },
                    size: CGSize {
                        width: opts.width,
                        height: opts.height,
                    },
                },
            )
        },
    };
    let content_rect = NSRect {
        origin: NSPoint { x: 0.0, y: 0.0 },
        size: CGSize {
            width: win_w,
            height: win_h,
        },
    };
    let _: () = unsafe { objc2::msg_send![&host_root, setFrame: content_rect] };
    let host_root_ref: &NSView = &*host_root;
    window.setContentView(Some(host_root_ref));
    backend.set_host_root(host_root.clone());

    // Global weak self-handle: animation writes, deferred syncs, and
    // navigator closures reach the backend through this.
    let backend = Rc::new(RefCell::new(backend));
    backend_macos::install_global_self(Rc::downgrade(&backend));

    // ── Mount, bracketed by the mount-buffering window ────────────────
    // (module docs above; `start` drains synchronously before `finish`).
    backend_apple_core::scheduler::begin_mount_buffering();
    let app_state: NewCoreApp = backend_macos::newcore::start(backend, register, build);
    backend_apple_core::scheduler::end_mount_buffering();
    // The app lives until the process exits with the run loop.
    std::mem::forget(app_state);

    // ── Show window + run loop ────────────────────────────────────────
    window.makeKeyAndOrderFront(None);
    #[allow(deprecated)]
    nsapp.activateIgnoringOtherApps(true);
    unsafe { nsapp.run() };

    Ok(())
}
