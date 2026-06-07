//! NSApplication boot + window setup. macOS-only.
//!
//! Boots a UI app (`NSApplicationActivationPolicyRegular`), opens a
//! standard NSWindow with traffic lights + resize + title bar,
//! hands the backend a flipped content NSView, runs
//! `runtime_core::render(app)` into that view, and starts the
//! AppKit run loop.

use std::cell::RefCell;
use std::rc::Rc;

use backend_macos::MacosBackend;
use runtime_core::Element;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSView, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::{
    CGSize, MainThreadMarker, NSActivityOptions, NSPoint, NSProcessInfo, NSRect, NSSize, NSString,
};

/// Env var carrying the PID of the `idealyst dev` launcher that spawned
/// this app. Set ONLY by the dev orchestrator's background macOS launch
/// (`cli/cmd/dev.rs::launch_macos`); absent for a standalone `.app` or a
/// foreground `idealyst run macos`, so the watchdog below stays dormant
/// outside dev.
const LAUNCHER_PID_ENV: &str = "IDEALYST_LAUNCHER_PID";

/// `true` while `pid` is still a live process we could signal. Uses
/// `kill(pid, 0)`, which sends no signal — it only probes existence:
/// `0` = alive; `ESRCH` = gone; `EPERM` = exists but not ours (treat as
/// alive — never false-positive a teardown). Split out so it's unit-
/// testable without spinning the run loop.
#[cfg(unix)]
fn launcher_alive(pid: i32) -> bool {
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    !matches!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(e) if e == libc::ESRCH
    )
}

/// In dev, exit the app when the `idealyst dev` launcher dies — even via
/// SIGKILL / force-quit, which no signal handler in the launcher could
/// forward. Without this the app is orphaned to launchd and lingers after
/// the terminal is gone. Mirrors the dev-host's own parent-pid watchdog
/// (`dev/server/src/host.rs`), but watches the *explicit* launcher pid
/// (via `LAUNCHER_PID_ENV`) rather than `getppid()`, so it's robust to the
/// app being reparented (e.g. launched through a `.app` bundle).
///
/// No-op when `LAUNCHER_PID_ENV` is unset — a standalone app has no
/// launcher to follow.
#[cfg(unix)]
fn spawn_launcher_watchdog() {
    let Ok(raw) = std::env::var(LAUNCHER_PID_ENV) else {
        return;
    };
    let Ok(pid) = raw.parse::<i32>() else {
        return;
    };
    // pid 1 (launchd) / 0 are never a real launcher to follow.
    if pid <= 1 {
        return;
    }
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if !launcher_alive(pid) {
            eprintln!("[host-appkit] dev launcher (pid {pid}) is gone — exiting app.");
            std::process::exit(0);
        }
    });
}

#[cfg(not(unix))]
fn spawn_launcher_watchdog() {}

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
///
/// Equivalent to [`run_with`] with a no-op `register_extensions`
/// callback. Apps that need to install third-party SDK extensions
/// (`toolbar::register`, `webview::register`, …) should use
/// [`run_with`] so they get a `&mut MacosBackend` to register against
/// before the render path starts.
pub fn run<F: FnOnce() -> Element>(app: F, opts: RunOptions) -> Result<(), RunError> {
    run_with(app, opts, |_| {})
}

/// Like [`run`], but invokes `register_extensions` with a mutable
/// reference to the freshly constructed `MacosBackend` before the
/// render pass starts. Third-party SDKs whose `register(&mut B)` adds
/// `Element::External` handlers (`toolbar`, `webview`, future
/// `maps-macos`) must run before render so the framework sees the
/// handler when it first walks the tree.
///
/// ```ignore
/// host_appkit::run_with(
///     app,
///     host_appkit::RunOptions::default(),
///     |backend| {
///         toolbar::register(backend);
///         // webview::register(backend);
///     },
/// )?;
/// ```
pub fn run_with<F, R>(app: F, opts: RunOptions, register_extensions: R) -> Result<(), RunError>
where
    F: FnOnce() -> Element,
    R: FnOnce(&mut MacosBackend),
{
    let Some(mtm) = MainThreadMarker::new() else {
        return Err(RunError::NotMainThread);
    };

    // Dev-mode lifecycle link: die when the `idealyst dev` launcher dies.
    spawn_launcher_watchdog();

    // ── NSApplication boot ────────────────────────────────────────
    let nsapp = NSApplication::sharedApplication(mtm);
    nsapp.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    // ── Disable App Nap ───────────────────────────────────────────
    // A dev app runs the robot bridge (a TCP server polled on the UI
    // thread). When the app is backgrounded, macOS App Nap suspends the
    // process — the run loop stops pumping, the bridge poll never fires,
    // and the bridge goes silent. That breaks the Inspector, which reads a
    // *backgrounded* target from the foreground. Holding a "user-initiated"
    // activity assertion for the app's lifetime keeps it scheduled (and out
    // of nap) so the bridge stays responsive in the background. The token
    // must be retained for as long as we want the assertion held — bind it
    // for the whole `run_with` body, which spans the run loop below.
    // SAFETY: standard NSProcessInfo activity API; the returned token is a
    // retained NSObject we simply hold and drop on app exit.
    let _app_nap_assertion = {
        let reason = NSString::from_str("idealyst dev: keep the robot bridge responsive in the background");
        unsafe {
            // `…UserInitiatedAllowingIdleSystemSleep` exempts the app from
            // App Nap (it's a user-initiated activity) WITHOUT pinning the
            // whole system awake — the Mac can still idle-sleep normally.
            NSProcessInfo::processInfo().beginActivityWithOptions_reason(
                NSActivityOptions::NSActivityUserInitiatedAllowingIdleSystemSleep,
                &reason,
            )
        }
    };

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
    // Route runtime-core `log_*` through NSLog so they reach the macOS system
    // log / stderr the same way iOS and web now do — otherwise Rust-side logs
    // (e.g. an in-app E2E suite's `[E2E-RESULT]`) hit the StderrLogger fallback
    // and may be missed by log scrapers. Idempotent (first install wins).
    backend_apple_core::log::install_logger();

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

    // ── Third-party SDK registration ──────────────────────────────
    // Fired AFTER `set_host_root` (so `host_root.window` is reachable
    // from inside SDK handlers — the toolbar SDK walks up that chain
    // to attach NSToolbar) and BEFORE the Rc<RefCell> wrap (so the
    // callback gets a plain `&mut MacosBackend` without re-borrow
    // gymnastics). Defaults to a no-op for callers of `run` that
    // don't need any extensions.
    register_extensions(&mut backend);

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
    // See `runtime_core::walker::mount` docs for the rationale.
    let owner = runtime_core::mount(backend.clone(), app);
    std::mem::forget(owner);

    // ── Show window + start run loop ──────────────────────────────
    window.makeKeyAndOrderFront(None);
    #[allow(deprecated)]
    nsapp.activateIgnoringOtherApps(true);
    unsafe { nsapp.run() };

    Ok(())
}

/// runtime-server variant of [`run`]. Boots NSApplication + opens the host
/// window exactly like local-render mode, but instead of mounting
/// the user's `app()` function it spawns a
/// [`backend_macos::runtime_server::spawn_runtime_server_shell`] worker that connects to
/// the dev-server at `url` and streams the sidecar's render commands
/// onto the AppKit run loop.
///
/// The user crate is NOT a dependency of the wrapper in runtime-server mode —
/// the sidecar process owns it. The wrapper's `main()` only needs
/// to know the dev-server URL, which the CLI bakes in via the
/// `IDEALYST_DEV_ENDPOINT` env var the wrapper resolves at startup
/// with [`runtime_server_shell_native::endpoint_or_panic`].
///
/// Returns only when the user quits the application.
#[cfg(feature = "runtime-server")]
pub fn run_aas(url: &str, opts: RunOptions) -> Result<(), RunError> {
    let Some(mtm) = MainThreadMarker::new() else {
        return Err(RunError::NotMainThread);
    };

    // Dev-mode lifecycle link: die when the `idealyst dev` launcher dies.
    spawn_launcher_watchdog();

    // ── NSApplication boot — identical to local-render path ───────
    let nsapp = NSApplication::sharedApplication(mtm);
    nsapp.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    let app_delegate = crate::app_delegate::IdealystAppDelegate::new(mtm);
    let delegate_proto: &objc2::runtime::ProtocolObject<dyn objc2_app_kit::NSApplicationDelegate> =
        objc2::runtime::ProtocolObject::from_ref(&*app_delegate);
    nsapp.setDelegate(Some(delegate_proto));

    // Required even in runtime-server mode — backend code on this side still
    // hits `after_ms` / `raf_loop` (animation tweens, presence
    // timers) via apply-style; those need the NSTimer scheduler so
    // they dispatch correctly instead of falling through to the
    // synchronous native fallback. Skipping this is what makes the
    // welcome example's intro freeze on `opacity:0` in any mode.
    backend_apple_core::scheduler::install_scheduler();
    // Route runtime-core `log_*` through NSLog so they reach the macOS system
    // log / stderr the same way iOS and web now do — otherwise Rust-side logs
    // (e.g. an in-app E2E suite's `[E2E-RESULT]`) hit the StderrLogger fallback
    // and may be missed by log scrapers. Idempotent (first install wins).
    backend_apple_core::log::install_logger();

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

    // ── Spawn the runtime-server shell ──────────────────────────────────────
    // The shell consumes the backend by value (it owns the
    // `RuntimeServerClient<MacosBackend>` thereafter). The drain timer fires
    // every ~16ms on the main thread; nothing else here needs to
    // hold the backend, so we don't install a global-self Weak ref
    // the way the local-render path does — there's no per-frame
    // re-entrant code path in runtime-server mode that would consult it.
    //
    // Report the host window size as the runtime-server viewport so the
    // sidecar's `page_ref.frame()` reads return the *actual*
    // canvas dimensions. Without this welcome's planet-orbit math
    // (and any other viewport-relative layout) computes against
    // the 393×800 fallback — visually misaligned on a 1024×768
    // desktop window.
    let _shell = backend_macos::runtime_server::spawn_runtime_server_shell(
        backend,
        url,
        None,
        Some((opts.width as f32, opts.height as f32)),
        Some(host_root.clone()),
    );
    backend_macos::runtime_server::start_main_thread_drain_timer();

    // ── Show window + start run loop ─────────────────────────────
    window.makeKeyAndOrderFront(None);
    #[allow(deprecated)]
    nsapp.activateIgnoringOtherApps(true);
    unsafe { nsapp.run() };

    // App quit. Tear down the shell so any worker-thread WebSocket
    // closes cleanly (best-effort — process is exiting anyway).
    backend_macos::runtime_server::teardown();

    Ok(())
}

#[cfg(all(test, unix))]
mod watchdog_tests {
    use super::launcher_alive;

    /// Our own process is, definitionally, alive.
    #[test]
    fn launcher_alive_true_for_self() {
        let me = std::process::id() as i32;
        assert!(launcher_alive(me), "the current process must read as alive");
    }

    /// A child we spawned and reaped is gone — its pid must read dead, so
    /// the watchdog would fire. Spawning+reaping a real process is the
    /// only deterministic way to obtain a known-dead pid.
    #[test]
    fn launcher_alive_false_after_child_reaped() {
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("spawn `true`");
        let pid = child.id() as i32;
        child.wait().expect("reap child");
        // The pid has been reaped (not a zombie holding the slot), so
        // `kill(pid, 0)` returns ESRCH. (Tiny race window before the
        // kernel frees the pid is not observed in practice here.)
        assert!(
            !launcher_alive(pid),
            "a reaped child's pid must read as gone"
        );
    }
}
