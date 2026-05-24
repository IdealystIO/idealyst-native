//! NSApplication boot + window setup. macOS-only.
//!
//! Boots a UI app (`NSApplicationActivationPolicyRegular`), opens a
//! standard NSWindow with traffic lights + resize + title bar,
//! hands the backend a flipped content NSView, runs
//! `framework_core::render(app)` into that view, and starts the
//! AppKit run loop.

use std::cell::RefCell;
use std::rc::Rc;

use backend_macos::MacosBackend;
use framework_core::Primitive;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSView, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::{CGSize, MainThreadMarker, NSPoint, NSRect, NSSize, NSString};

#[derive(Clone, Debug)]
pub struct RunOptions {
    /// Initial window title shown in the title bar + on the Dock.
    pub title: String,
    /// Initial window width in points.
    pub width: f64,
    /// Initial window height in points.
    pub height: f64,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            title: "Idealyst".to_string(),
            width: 1024.0,
            height: 768.0,
        }
    }
}

#[derive(Debug)]
pub enum RunError {
    /// `MainThreadMarker::new()` returned `None` — the host was
    /// called off the main thread. AppKit can only boot on the main
    /// thread, so the wrapper binary must call `host_appkit::run`
    /// from `main` directly.
    NotMainThread,
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::NotMainThread => write!(
                f,
                "host-appkit::run must be called from the main thread \
                 (move it to `fn main()`'s body)"
            ),
        }
    }
}

impl std::error::Error for RunError {}

/// Boot NSApplication, open the host window, install the backend,
/// render `app()`, and enter the AppKit run loop.
///
/// Returns only when the user quits the application (NSApp's
/// `run` returns to the caller cleanly on `terminate:`).
pub fn run<F: FnOnce() -> Primitive>(app: F, opts: RunOptions) -> Result<(), RunError> {
    let Some(mtm) = MainThreadMarker::new() else {
        return Err(RunError::NotMainThread);
    };

    // ── NSApplication boot ────────────────────────────────────────
    let nsapp = NSApplication::sharedApplication(mtm);
    nsapp.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    // ── App delegate ──────────────────────────────────────────────
    // Without a delegate that returns YES from
    // `applicationShouldTerminateAfterLastWindowClosed:`, closing
    // the window (red traffic light / Cmd-W) leaves NSApp running
    // in the background — the user has to Cmd-Q to actually quit.
    // Single-window apps almost always want window close to mean
    // app quit, so make that the default here.
    //
    // Retain the delegate explicitly for the run loop's lifetime —
    // `setDelegate:` doesn't keep a strong reference (it's a weak
    // assign), and if the Retained drops the delegate the run loop
    // would call into a freed object on first window close.
    let app_delegate = crate::app_delegate::IdealystAppDelegate::new(mtm);
    let delegate_proto: &objc2::runtime::ProtocolObject<dyn objc2_app_kit::NSApplicationDelegate> =
        objc2::runtime::ProtocolObject::from_ref(&*app_delegate);
    nsapp.setDelegate(Some(delegate_proto));

    // ── Scheduler ─────────────────────────────────────────────────
    // Required before `render(...)` so `after_ms`/`raf_loop` etc.
    // dispatch through NSTimer instead of synchronously.
    backend_apple_core::scheduler::install_scheduler();

    // ── Window ────────────────────────────────────────────────────
    let frame = NSRect {
        origin: NSPoint { x: 200.0, y: 200.0 },
        size: NSSize {
            width: opts.width,
            height: opts.height,
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
    let title_ns = NSString::from_str(&opts.title);
    window.setTitle(&title_ns);

    // ── Content view ──────────────────────────────────────────────
    // Use the backend's FlippedView so the Taffy layout pass (top-
    // left origin) lands correctly. We can't use `MacosNode` here
    // because the backend hasn't been constructed yet — instead we
    // ask the backend to create the host root via `create_view`
    // after construction. For now create a bare NSView and let the
    // backend layer its own root on top.
    //
    // Simpler path: construct the backend, ask it for a host root,
    // mount that as the window's contentView.
    let mut backend = MacosBackend::new(mtm);
    let host_root: Retained<NSView> = match backend.create_host_root() {
        Some(v) => v,
        None => {
            // Defensive fallback — if the backend doesn't expose
            // `create_host_root` (it should), construct a plain
            // NSView via AppKit.
            unsafe {
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
            }
        }
    };
    // Set the host_root's frame explicitly so the backend's first
    // layout pass has a non-zero viewport. AppKit *will* size the
    // contentView on `setContentView:`, but the resize doesn't take
    // effect until the window is realized (`makeKeyAndOrderFront`),
    // and `render(...)` runs before that — so without an explicit
    // frame, the first Taffy compute sees a zero viewport and
    // produces zero-sized children.
    let content_rect = NSRect {
        origin: NSPoint { x: 0.0, y: 0.0 },
        size: CGSize {
            width: opts.width,
            height: opts.height,
        },
    };
    let _: () = unsafe { objc2::msg_send![&host_root, setFrame: content_rect] };
    let host_root_ref: &NSView = &*host_root;
    window.setContentView(Some(host_root_ref));
    backend.set_host_root(host_root.clone());

    // ── Backend handoff ───────────────────────────────────────────
    // Wrap in Rc<RefCell<>> + install the global self-ref. Mirrors
    // the iOS pattern — closures inside navigators/drawers reach
    // back into the backend through this.
    let backend = Rc::new(RefCell::new(backend));
    backend_macos::install_global_self(Rc::downgrade(&backend));

    // ── Render the user tree ──────────────────────────────────────
    // Use `mount(closure)` not `render(primitive)` — the closure
    // runs INSIDE the root reactive scope, so effects, signals,
    // and refs declared by the user's `app()` adopt that scope.
    // Without this, every `effect!` in the coordinator drops its
    // hidden handle as soon as `app()` returns (its scope is gone),
    // which kills the entrance/animation timers before they ever
    // fire and the welcome example renders zero-opacity, static.
    // See `framework_core::walker::mount` docs for the rationale.
    let owner = framework_core::mount(backend.clone(), app);
    std::mem::forget(owner);

    // ── Show window + start run loop ──────────────────────────────
    window.makeKeyAndOrderFront(None);
    #[allow(deprecated)]
    nsapp.activateIgnoringOtherApps(true);
    unsafe { nsapp.run() };

    Ok(())
}

/// AAS variant of [`run`]. Boots NSApplication + opens the host
/// window exactly like local-render mode, but instead of mounting
/// the user's `app()` function it spawns a
/// [`backend_macos::aas::spawn_aas_shell`] worker that connects to
/// the dev-server (mDNS-discovered via `app_id`) and streams the
/// sidecar's render commands onto the AppKit run loop.
///
/// The user crate is NOT a dependency of the wrapper in AAS mode —
/// the sidecar process owns it. The wrapper's `main()` only needs
/// to know the `app_id` (typically the bundle id from
/// `[package.metadata.idealyst.app]`) so this host can find the
/// matching dev-server.
///
/// Returns only when the user quits the application.
#[cfg(feature = "aas-shell")]
pub fn run_aas(app_id: &str, opts: RunOptions) -> Result<(), RunError> {
    let Some(mtm) = MainThreadMarker::new() else {
        return Err(RunError::NotMainThread);
    };

    // ── NSApplication boot — identical to local-render path ───────
    let nsapp = NSApplication::sharedApplication(mtm);
    nsapp.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    let app_delegate = crate::app_delegate::IdealystAppDelegate::new(mtm);
    let delegate_proto: &objc2::runtime::ProtocolObject<dyn objc2_app_kit::NSApplicationDelegate> =
        objc2::runtime::ProtocolObject::from_ref(&*app_delegate);
    nsapp.setDelegate(Some(delegate_proto));

    // Required even in AAS mode — backend code on this side still
    // hits `after_ms` / `raf_loop` (animation tweens, presence
    // timers) via apply-style; those need the NSTimer scheduler so
    // they dispatch correctly instead of falling through to the
    // synchronous native fallback. Skipping this is what makes the
    // welcome example's intro freeze on `opacity:0` in any mode.
    backend_apple_core::scheduler::install_scheduler();

    // ── Window + host root ─ same as local-render ────────────────
    let frame = NSRect {
        origin: NSPoint { x: 200.0, y: 200.0 },
        size: NSSize {
            width: opts.width,
            height: opts.height,
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
    let title_ns = NSString::from_str(&opts.title);
    window.setTitle(&title_ns);

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
            width: opts.width,
            height: opts.height,
        },
    };
    let _: () = unsafe { objc2::msg_send![&host_root, setFrame: content_rect] };
    let host_root_ref: &NSView = &*host_root;
    window.setContentView(Some(host_root_ref));
    backend.set_host_root(host_root.clone());

    // ── Spawn the AAS shell ──────────────────────────────────────
    // The shell consumes the backend by value (it owns the
    // `AasClient<MacosBackend>` thereafter). The drain timer fires
    // every ~16ms on the main thread; nothing else here needs to
    // hold the backend, so we don't install a global-self Weak ref
    // the way the local-render path does — there's no per-frame
    // re-entrant code path in AAS mode that would consult it.
    //
    // Report the host window size as the AAS viewport so the
    // sidecar's `page_ref.frame()` reads return the *actual*
    // canvas dimensions. Without this welcome's planet-orbit math
    // (and any other viewport-relative layout) computes against
    // the 393×800 fallback — visually misaligned on a 1024×768
    // desktop window.
    let _shell = backend_macos::aas::spawn_aas_shell(
        backend,
        app_id,
        None,
        Some((opts.width as f32, opts.height as f32)),
    );
    backend_macos::aas::start_main_thread_drain_timer();

    // ── Show window + start run loop ─────────────────────────────
    window.makeKeyAndOrderFront(None);
    #[allow(deprecated)]
    nsapp.activateIgnoringOtherApps(true);
    unsafe { nsapp.run() };

    // App quit. Tear down the shell so any worker-thread WebSocket
    // closes cleanly (best-effort — process is exiting anyway).
    backend_macos::aas::teardown();

    Ok(())
}
