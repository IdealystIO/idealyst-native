//! Shared AppKit host pieces: [`RunOptions`] / [`RunError`], the main
//! menu, the dev launcher watchdog, and the runtime-server boot. The
//! local-mount windowed boot lives in `boot.rs`. macOS-only.

#[cfg(feature = "runtime-server")]
use std::cell::RefCell;
#[cfg(feature = "runtime-server")]
use std::rc::Rc;

#[cfg(feature = "runtime-server")]
use backend_macos::MacosBackend;
#[cfg(feature = "runtime-server")]
use objc2::rc::Retained;
use objc2::sel;
#[cfg(feature = "runtime-server")]
use objc2_app_kit::{
    NSApplicationActivationPolicy, NSBackingStoreType, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem};
#[cfg(feature = "runtime-server")]
use objc2_foundation::{CGSize, NSActivityOptions, NSPoint, NSProcessInfo, NSRect, NSSize};
use objc2_foundation::{MainThreadMarker, NSString};

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
pub(crate) fn spawn_launcher_watchdog() {
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
pub(crate) fn spawn_launcher_watchdog() {}

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

/// Install a standard main menu (App + Edit) so the app gets the system
/// editing commands.
///
/// On macOS, `Cmd-A` / `Cmd-C` / `Cmd-V` / `Cmd-X` / `Cmd-Z` are NOT delivered
/// to the focused control through key bindings — they're matched as **key
/// equivalents of the main menu's items** and only then dispatched down the
/// responder chain to the first responder. A programmatically-booted
/// `NSApplication` has no menu by default, so a focused `NSTextView` /
/// `NSTextField` never received select-all / copy / paste / cut / undo (the
/// reported "can't do normal textbox functions" bug) even though it already
/// implements every one of those action methods.
///
/// Each Edit item uses a `nil` target (the `initWithTitle:action:keyEquivalent:`
/// default): AppKit then routes the action up the responder chain to whatever
/// is first responder, so the same menu drives editing in any text control with
/// no per-control wiring. The App menu carries `Quit` (`Cmd-Q`) so the standard
/// quit shortcut works too.
pub(crate) fn install_main_menu(mtm: MainThreadMarker, nsapp: &NSApplication) {
    // `nil`-target item whose action travels the responder chain. `cmd_extra`
    // adds modifiers beyond the implicit Command (e.g. Shift for Redo).
    let make_item = |title: &str, action: objc2::runtime::Sel, key: &str, cmd_extra: bool| {
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                mtm.alloc(),
                &NSString::from_str(title),
                Some(action),
                &NSString::from_str(key),
            )
        };
        if cmd_extra {
            item.setKeyEquivalentModifierMask(
                NSEventModifierFlags::NSEventModifierFlagCommand
                    | NSEventModifierFlags::NSEventModifierFlagShift,
            );
        }
        item
    };

    let main_menu = NSMenu::new(mtm);

    // ── Application menu (Quit) ──────────────────────────────────────
    // The first top-level item's submenu is the app menu (its title is
    // replaced by the process name by AppKit).
    let app_item = NSMenuItem::new(mtm);
    let app_menu = NSMenu::new(mtm);
    app_menu.addItem(&make_item("Quit", sel!(terminate:), "q", false));
    app_item.setSubmenu(Some(&app_menu));
    main_menu.addItem(&app_item);

    // ── Edit menu (Undo/Redo, Cut/Copy/Paste, Select All) ────────────
    let edit_item = NSMenuItem::new(mtm);
    let edit_menu = unsafe { NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str("Edit")) };
    edit_menu.addItem(&make_item("Undo", sel!(undo:), "z", false));
    edit_menu.addItem(&make_item("Redo", sel!(redo:), "z", true));
    edit_menu.addItem(&NSMenuItem::separatorItem(mtm));
    edit_menu.addItem(&make_item("Cut", sel!(cut:), "x", false));
    edit_menu.addItem(&make_item("Copy", sel!(copy:), "c", false));
    edit_menu.addItem(&make_item("Paste", sel!(paste:), "v", false));
    edit_menu.addItem(&make_item("Select All", sel!(selectAll:), "a", false));
    edit_item.setSubmenu(Some(&edit_menu));
    main_menu.addItem(&edit_item);

    nsapp.setMainMenu(Some(&main_menu));
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
    // Standard main menu so focused text controls get Cmd-A/C/V/X/Z.
    install_main_menu(mtm, &nsapp);

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
    // Per-frame render-loop driver (NSTimer). Embedded wgpu hosts
    // (`host-macos-desktop`, e.g. the website's Simulator preview) draw
    // via `runtime_core::driver::render_loop`; without an installed
    // driver that call silently returns a no-op handle and the preview
    // never paints. Idempotent (first install wins).
    backend_macos::install_render_loop();
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
