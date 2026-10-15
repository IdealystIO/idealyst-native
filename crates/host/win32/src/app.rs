//! Win32 window + message pump + `backend_windows::newcore::start` driver.
//!
//! `run_with` registers a window class, opens one top-level window,
//! constructs a [`WindowsBackend`] rooted at its HWND, installs the
//! scheduler, mounts the app, and pumps messages until the window
//! closes. Per-window state (the shared backend + the mounted
//! [`NewCoreApp`]) lives in a heap [`HostState`] whose pointer is stashed
//! in the window's `GWLP_USERDATA` so the `WndProc` can reach it.
//!
//! The pump also feeds the app-level keyboard: every retrieved message is
//! offered to the backend's [`KeyboardSlot`] before
//! `TranslateMessage`/`DispatchMessageW` (keys addressed to a focused
//! child control never reach this window's `WndProc`), and
//! `WM_ACTIVATE(WA_INACTIVE)` reports focus loss. See
//! `backend_windows::app_keys`.
//!
//! The app-level pointer rides the same two seams: the pump offers every
//! message to the backend's [`PointerSlot`] (mouse messages for child
//! controls never reach this `WndProc`; raw input for a locked pointer
//! arrives as `WM_INPUT`), and the `WndProc` routes host-window messages
//! through it first (`WM_SETCURSOR` for a hidden/locked cursor,
//! deactivation dropping a lock). Host-window messages also feed
//! `backend_windows::app_situation` (focus, minimize, settings changes).
//! See `backend_windows::app_pointer`.
//!
//! ## DPI
//!
//! The process is per-monitor-v2 DPI aware
//! ([`backend_windows::enable_per_monitor_dpi_awareness`], before any
//! window exists). `RunOptions` sizes are LOGICAL pixels: the window is
//! created, its DPI read, and its client area sized to the options × the
//! scale. Mouse coordinates are handed to the backend in device pixels
//! (it converts); the viewport signal is published in logical pixels.
//! `WM_DPICHANGED` (the window crossed to a monitor with another scale)
//! re-scales the backend, then applies the suggested window rect. See
//! `backend_windows::dpi`.

use std::cell::RefCell;
use std::rc::Rc;

use backend_windows::app_keys::KeyboardSlot;
use backend_windows::app_pointer::PointerSlot;
use backend_windows::newcore::{self, NewCoreApp};
use backend_windows::dpi;
use backend_windows::view_input;
use runtime_shared::PointerButton;
use backend_windows::WindowsBackend;
use runtime_core::Element;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
// `ScreenToClient` ships under Graphics::Gdi in windows 0.58, not
// WindowsAndMessaging where the C headers put it.
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, ScreenToClient, UpdateWindow, HBRUSH, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, SetWindowPos, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER,
    WM_DPICHANGED, DispatchMessageW, GetClientRect,
    GetMessageW, GetWindowLongPtrW, LoadCursorW, RegisterClassExW,
    SetWindowLongPtrW, ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA,
    IDC_ARROW, MSG, SW_SHOW, WA_INACTIVE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_ACTIVATE, WM_APP, WM_COMMAND,
    WM_DESTROY, WM_ERASEBKGND,
    WM_HSCROLL, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL,
    WM_MOUSEMOVE, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_CAPTURECHANGED, WM_MOUSEWHEEL, WM_PAINT, WM_SIZE, WNDCLASSEXW, WS_CLIPCHILDREN,
    WS_OVERLAPPEDWINDOW,
};

use crate::RunOptions;

/// Private wake message the scheduler worker `PostMessageW`s to the
/// host window; the `WndProc` responds by draining due timers/rafs.
/// `WM_APP + 1` sits in the app-private message range so it can't
/// collide with a system or common-control notification.
pub(crate) const WM_IDEALYST_SCHED: u32 = WM_APP + 1;

/// `WM_MOUSELEAVE` (winuser.h). windows 0.58 files it under
/// `Win32::UI::Controls`, a feature this crate does not otherwise need.
const WM_MOUSELEAVE: u32 = 0x02A3;

/// Window-class name for the host's single top-level window.
const HOST_CLASS_NAME: PCWSTR = PCWSTR(windows::core::w!("IdealystHostWindow").as_ptr());

/// Per-window state reachable from the `WndProc` via `GWLP_USERDATA`.
/// Boxed on the heap; the raw pointer is stored on the window and the
/// box is never reclaimed: the process `TerminateProcess`es from
/// `WM_DESTROY` (see `hard_exit`), so nothing here is dropped.
struct HostState {
    backend: Rc<RefCell<WindowsBackend>>,
    /// The mounted app returned by `newcore::start`. Kept alive so the UI
    /// stays reactive. `RefCell<Option<>>` because the pointer is
    /// published to the window *before* mount runs (so a `WM_SIZE` during
    /// `ShowWindow` finds the backend), and the app is slotted in
    /// afterwards.
    app: RefCell<Option<NewCoreApp>>,
    /// The backend's app-level keyboard slot, cloned once so the
    /// `WndProc` reports focus loss without borrowing the backend.
    keyboard: KeyboardSlot,
    /// The backend's app-level pointer slot, same sharing as `keyboard`.
    pointer: PointerSlot,
    /// A `TrackMouseEvent(TME_LEAVE)` request is armed: Windows sends
    /// `WM_MOUSELEAVE` once per request, so it is re-armed on the next
    /// move after each leave.
    tracking_leave: std::cell::Cell<bool>,
}

thread_local! {
    /// Callbacks queued by [`on_main_loop_start`], drained once just before
    /// the message pump starts.
    static ON_LOOP_START: RefCell<Vec<Box<dyn FnOnce()>>> = const { RefCell::new(Vec::new()) };
    /// Set once the pump is about to run; later callers run immediately.
    static LOOP_STARTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `f` on the UI thread once the host is up — scheduler installed, tree
/// mounted — immediately before the message loop starts pumping. Called
/// after that point, `f` runs at once.
///
/// Exists for the generated wrapper's Robot bridge: the bridge's poll is
/// SCHEDULER-driven (`schedule_periodic_poll` bails when no scheduler is
/// installed), and the scheduler is installed inside [`run_with`], so a
/// wrapper cannot start the bridge before handing over control and has
/// nowhere to do it after. Without this the Windows app came up with no
/// bridge listener and no registration, and `--parity …,windows` waited out
/// its timeout. Mirrors `host_gtk::on_main_loop_start`, and is generic for
/// the same reason: the host needs no dependency on the robot surface.
pub fn on_main_loop_start<F: FnOnce() + 'static>(f: F) {
    if LOOP_STARTED.with(|s| s.get()) {
        f();
    } else {
        ON_LOOP_START.with(|q| q.borrow_mut().push(Box::new(f)));
    }
}

fn drain_loop_start() {
    LOOP_STARTED.with(|s| s.set(true));
    let queued = ON_LOOP_START.with(|q| std::mem::take(&mut *q.borrow_mut()));
    for f in queued {
        f();
    }
}

/// Run `app` on the Win32 backend with no extension registration —
/// the common case. Blocks until the window closes; returns the
/// process exit code (0 = clean).
pub fn run<F>(opts: RunOptions, build_ui: F) -> i32
where
    F: FnOnce() -> Element + 'static,
{
    run_with(opts, |_| {}, build_ui)
}

/// As [`run`], but invokes `register` on the scene [`Registry`] before the
/// tree realizes — the seam where an app installs its SDK payload handlers.
/// Runs after `register_builtins`. Mirrors `host_gtk::run_with`.
///
/// [`Registry`]: runtime_scene::Registry
pub fn run_with<R, F>(opts: RunOptions, register: R, build_ui: F) -> i32
where
    R: FnOnce(&mut runtime_scene::Registry<WindowsBackend>) + 'static,
    F: FnOnce() -> Element + 'static,
{
    match unsafe { run_inner(opts, register, build_ui) } {
        Ok(code) => code,
        Err(e) => {
            eprintln!("host-win32: {e}");
            1
        }
    }
}

unsafe fn run_inner<R, F>(opts: RunOptions, register: R, build_ui: F) -> Result<i32, String>
where
    R: FnOnce(&mut runtime_scene::Registry<WindowsBackend>),
    F: FnOnce() -> Element,
{
    let pinned = std::env::var("IDEALYST_WINDOW_SIZE").ok();
    let opts = opts.with_size_override(pinned.as_deref());
    PINNED_CLIENT.with(|p| p.set(pinned.as_deref().and_then(crate::parse_window_size)));
    // Per-monitor DPI awareness is process-wide and must precede the
    // first window (see `backend_windows::dpi`).
    backend_windows::enable_per_monitor_dpi_awareness();

    // hInstance for the window class — the current executable's module.
    let hmodule = GetModuleHandleW(None).map_err(|e| format!("GetModuleHandleW: {e}"))?;
    let hinstance = HINSTANCE(hmodule.0);

    register_host_class(hinstance)?;
    let hwnd = create_host_window(hinstance, &opts)?;

    // Build the backend rooted at the window and share it behind an
    // Rc<RefCell<>> — the WndProc, the scheduler drains, and the reactive
    // effects all borrow the same instance.
    let backend = Rc::new(RefCell::new(WindowsBackend::new(hwnd)));
    let keyboard = backend.borrow().keyboard_slot();
    let pointer = backend.borrow().pointer_slot();
    // Node handles (animation writes, frame reads) reach the backend
    // through a weak self-reference — install it before mount so
    // handles built during the first walk are live.
    backend.borrow_mut().set_self_ref(Rc::downgrade(&backend));

    // Publish per-window state to the WndProc before anything can
    // dispatch a message to this window.
    let state = Box::new(HostState {
        backend: backend.clone(),
        app: RefCell::new(None),
        keyboard: keyboard.clone(),
        pointer: pointer.clone(),
        tracking_leave: std::cell::Cell::new(false),
    });
    let state_ptr = Box::into_raw(state);
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize);

    // Install the scheduler BEFORE mount: author `effect!` blocks fire
    // `after_ms`/`raf_loop` during the first render, and without a
    // scheduler those collapse to the synchronous / inert fallbacks
    // (animations freeze).
    crate::scheduler::install(hwnd);

    // Show before mount so `finish()` computes against the real client
    // rect. The `WM_SIZE` this triggers finds `root_node` unset
    // (nothing mounted yet) and is a harmless no-op.
    let _ = ShowWindow(hwnd, SW_SHOW);

    // Seed the reactive viewport signal with the initial client size so
    // breakpoint-driven layout (responsive sidebars etc.) resolves
    // correctly on the FIRST build, not only after the first resize.
    publish_viewport(&backend);

    // Mount: `newcore::start` installs the time source, builds the scene
    // `Registry` (builtins, then the app's `register`), realizes the tree
    // and calls `finish(root)`, laying it out against the shown window.
    // It replaces `runtime_core::mount`, which went away with the old
    // walker.
    let app = newcore::start(backend.clone(), register, build_ui);
    (*state_ptr).app.borrow_mut().replace(app);

    // Belt-and-suspenders initial layout in case the show-time
    // `WM_SIZE` raced ahead of mount.
    backend.borrow_mut().relayout();

    // Scheduler + tree are live: run what the wrapper queued (the Robot
    // bridge start). No backend borrow is held here.
    drain_loop_start();

    // Pump until `WM_QUIT` (posted by `WM_DESTROY`).
    let exit_code = pump(
        |msg| {
            // App-level keyboard first, for EVERY window of the thread: a
            // key addressed to a focused child control (the `EDIT` behind
            // `text_input`) never reaches the host `WndProc`. A swallowed
            // key (`PreventDefault`) skips translation too, so no
            // `WM_CHAR` types it. No backend borrow is held here — the
            // slot is shared.
            if keyboard.pre_dispatch(msg) {
                return;
            }
            // App-level pointer, same reasoning: a click / move / wheel
            // over a child control goes to that control's window. A
            // swallowed message (`PreventDefault`, or movement while
            // locked) is never dispatched.
            if pointer.pre_dispatch(msg) {
                return;
            }
            let _ = TranslateMessage(msg);
            DispatchMessageW(msg);
        },
        crate::scheduler::drain_due,
    );
    // Fallback: the (single) window `_exit`s from `WM_DESTROY`, so the
    // loop normally never returns here. If it does (a stray `WM_QUIT`
    // posted some other way), hard-exit for the same reason — never
    // fall through into destructor-running process teardown.
    hard_exit(exit_code);
}

/// The message loop. Every retrieved message goes to `dispatch`, except
/// the scheduler's wake: that one only marks a drain pending, and
/// `drain` runs once the queue holds nothing else.
///
/// Windows retrieves posted messages before input, and the wake is a
/// posted message. Draining it as it arrived meant that when frames
/// (scheduler drains) took longer than the 16 ms raf pulse, a wake was
/// always waiting ahead of the mouse and keyboard: input was never read
/// and the window went "Not Responding" while a canvas3d scene rendered.
/// Deferring the drain behind everything queued lets input and paint go
/// first. A wake that a modal loop (window drag, menu) dispatches still
/// drains in the `WndProc`, so animation keeps running during it.
///
/// Returns `WM_QUIT`'s exit code. `GetMessageW` returns 0 on `WM_QUIT` and
/// -1 on error; both end the loop.
unsafe fn pump(mut dispatch: impl FnMut(&MSG), mut drain: impl FnMut()) -> i32 {
    use windows::Win32::UI::WindowsAndMessaging::{PeekMessageW, PM_NOREMOVE};
    let mut msg = MSG::default();
    let mut wake_pending = false;
    loop {
        if wake_pending && !PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE).as_bool() {
            wake_pending = false;
            drain();
            continue;
        }
        if GetMessageW(&mut msg, None, 0, 0).0 <= 0 {
            return msg.wParam.0 as i32;
        }
        if msg.message == WM_IDEALYST_SCHED {
            wake_pending = true;
            continue;
        }
        dispatch(&msg);
    }
}

/// Register the host window class once. `RegisterClassExW` returns 0
/// (a null ATOM) on failure; a re-registration of a known class also
/// returns 0 with `ERROR_CLASS_ALREADY_EXISTS`, which we treat as
/// success (only one host runs per process, but a re-`run` must not
/// spuriously fail).
unsafe fn register_host_class(hinstance: HINSTANCE) -> Result<(), String> {
    use std::sync::atomic::{AtomicBool, Ordering};
    static REGISTERED: AtomicBool = AtomicBool::new(false);
    if REGISTERED.load(Ordering::Relaxed) {
        return Ok(());
    }

    let cursor = LoadCursorW(None, IDC_ARROW).map_err(|e| format!("LoadCursorW: {e}"))?;

    let wcex = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinstance,
        hIcon: Default::default(),
        hCursor: cursor,
        // No class background brush: the scene painter owns the whole
        // client area (WM_ERASEBKGND returns handled), and a system
        // brush here would flash the wrong color behind it.
        hbrBackground: HBRUSH::default(),
        lpszMenuName: PCWSTR::null(),
        lpszClassName: HOST_CLASS_NAME,
        hIconSm: Default::default(),
    };

    if RegisterClassExW(&wcex) == 0 {
        let err = windows::Win32::Foundation::GetLastError();
        // 1410 = ERROR_CLASS_ALREADY_EXISTS.
        if err.0 != 1410 {
            return Err(format!("RegisterClassExW: {err:?}"));
        }
    }
    REGISTERED.store(true, Ordering::Relaxed);
    Ok(())
}

/// Publish the backend's LOGICAL client size to the framework's reactive
/// viewport signal (breakpoint / responsive styling reads it — the web
/// host's resize observer equivalent). Call with no backend borrow held
/// by the caller beyond this read: the write re-runs author effects that
/// may re-borrow the backend.
fn publish_viewport(backend: &Rc<RefCell<WindowsBackend>>) {
    let (width, height) = backend.borrow().logical_client_size();
    runtime_shared::set_viewport_size(runtime_shared::ViewportSize { width, height });
}

/// Read the backend for a control notification — `None` when the backend
/// is borrowed, i.e. the notification arrived RE-ENTRANTLY from the backend's
/// own Win32 call. Windows sends some notifications synchronously from
/// inside the call that causes them: creating an `EDIT` with text, or setting
/// a control's value, sends the parent `WM_COMMAND` (EN_UPDATE / EN_CHANGE)
/// before `CreateWindowExW` / `SetWindowTextW` returns — while the mount that
/// made the call still holds the backend mutably. A plain `borrow()` there
/// panicked, and a panic inside a WndProc cannot unwind, so the app aborted
/// (idea-ui docs' theme-editor page on real Windows; Wine doesn't send the
/// notification there). Such notifications come from the backend, not the
/// user, so dropping them is also right for `on_change`, which fires for user
/// edits only.
unsafe fn with_idle_backend<R>(hwnd: HWND, f: impl FnOnce(&WindowsBackend) -> R) -> Option<R> {
    let state = host_state(hwnd)?;
    let backend = state.backend.try_borrow().ok()?;
    Some(f(&backend))
}

/// The host window's styles. WS_CLIPCHILDREN: the scene painter blits the
/// whole client area; clipping children keeps the blit off the native
/// control HWNDs (button / edit / trackbar) positioned on top of it.
const HOST_STYLE: WINDOW_STYLE = WINDOW_STYLE(WS_OVERLAPPEDWINDOW.0 | WS_CLIPCHILDREN.0);
const HOST_EX_STYLE: WINDOW_EX_STYLE = WINDOW_EX_STYLE(0);

thread_local! {
    /// The client size `IDEALYST_WINDOW_SIZE` pinned (logical px), if any.
    /// Set before the window exists: `WM_GETMINMAXINFO` arrives during
    /// `CreateWindowExW`, before the host state is attached.
    static PINNED_CLIENT: std::cell::Cell<Option<(i32, i32)>> = const { std::cell::Cell::new(None) };
}

/// Windows caps a window's size at roughly the screen's (the default
/// `ptMaxTrackSize`), so a pinned parity viewport larger than the display was
/// silently shrunk: a 1280×800 viewport on a 1173×867 VM display laid out
/// 1177 px wide and every comparison against the 1280-px browser failed.
/// When the size is pinned, allow at least that size — the window then
/// overhangs the screen, which parity (it reads the render tree, not the
/// screen) doesn't mind, exactly as the headless browser renders at its
/// `--window-size` whatever the display. Unpinned windows keep the OS cap.
unsafe fn allow_pinned_size(hwnd: HWND, info: &mut windows::Win32::UI::WindowsAndMessaging::MINMAXINFO) -> bool {
    let Some(client) = PINNED_CLIENT.with(|p| p.get()) else { return false };
    let dpi = match GetDpiForWindow(hwnd) {
        0 => dpi::BASE_DPI,
        d => d,
    };
    let opts = RunOptions { width: client.0, height: client.1, ..RunOptions::default() };
    let (w, h) = window_size_for(&opts, HOST_STYLE, HOST_EX_STYLE, dpi);
    info.ptMaxTrackSize.x = info.ptMaxTrackSize.x.max(w);
    info.ptMaxTrackSize.y = info.ptMaxTrackSize.y.max(h);
    true
}

/// Create the top-level window sized so its *client* area shows exactly
/// `opts.width × opts.height` LOGICAL pixels at the DPI of the monitor it
/// opens on (via `AdjustWindowRectExForDpi`, which inflates for the frame
/// + title bar at that DPI).
unsafe fn create_host_window(hinstance: HINSTANCE, opts: &RunOptions) -> Result<HWND, String> {
    let (style, ex_style) = (HOST_STYLE, HOST_EX_STYLE);

    // The monitor (and so the DPI) is only known once the window exists:
    // create at the 96-DPI size, then resize for the real DPI before the
    // window is ever shown.
    let (win_w, win_h) = window_size_for(opts, style, ex_style, dpi::BASE_DPI);

    let title = to_wide(&opts.title);
    let hwnd = CreateWindowExW(
        ex_style,
        HOST_CLASS_NAME,
        PCWSTR(title.as_ptr()),
        style,
        // CW_USEDEFAULT for position lets Windows place the window.
        windows::Win32::UI::WindowsAndMessaging::CW_USEDEFAULT,
        windows::Win32::UI::WindowsAndMessaging::CW_USEDEFAULT,
        win_w,
        win_h,
        // No parent (top-level), no menu, our hInstance, no create param.
        None,
        None,
        Some(hinstance),
        None,
    )
    .map_err(|e| format!("CreateWindowExW: {e}"))?;

    if hwnd.is_invalid() {
        return Err("CreateWindowExW returned null".into());
    }
    let window_dpi = GetDpiForWindow(hwnd);
    if window_dpi != 0 && window_dpi != dpi::BASE_DPI {
        let (w, h) = window_size_for(opts, style, ex_style, window_dpi);
        let _ = SetWindowPos(hwnd, None, 0, 0, w, h, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
    Ok(hwnd)
}

/// Outer window size whose client area is `opts`' logical size at `dpi`.
unsafe fn window_size_for(
    opts: &RunOptions,
    style: WINDOW_STYLE,
    ex_style: WINDOW_EX_STYLE,
    window_dpi: u32,
) -> (i32, i32) {
    let scale = dpi::DpiScale::from_dpi(window_dpi);
    let (cw, ch) = scale.logical_size_to_device(opts.width.max(1) as f32, opts.height.max(1) as f32);
    let mut rect = RECT { left: 0, top: 0, right: cw, bottom: ch };
    // Ignore an adjust error (extremely unlikely for a standard style) —
    // fall back to client dims as window dims.
    let _ = AdjustWindowRectExForDpi(&mut rect, style, false, ex_style, window_dpi);
    (rect.right - rect.left, rect.bottom - rect.top)
}

/// Terminate the process immediately without running the CRT's
/// orderly shutdown, so no destructors or thread-local teardown fire.
///
/// This is subtler than it looks on Windows. The obvious analog of the
/// winit host's POSIX `_exit` — the CRT's `_exit` — is **not**
/// equivalent here: on Windows `_exit` still routes through
/// `ExitProcess` → `LdrShutdownProcess`, which runs the loader's
/// thread-local destructor callbacks. Those tear down `runtime_core`'s
/// reactive `Arena` thread-local, and dropping an effect that owns a
/// scheduler handle then reaches into the scheduler's `MAIN_QUEUE`
/// thread-local — already destroyed, since inter-TLS destruction order
/// is undefined — aborting with "cannot access a Thread Local Storage
/// value during or after destruction" (confirmed by backtrace).
///
/// `TerminateProcess` on the current process skips `LdrShutdownProcess`
/// entirely: no TLS callbacks, no DLL detach, no atexit. The OS
/// reclaims memory, threads, and the window's HWNDs. The window is
/// already being destroyed when this runs, so there is nothing to
/// flush. This is the true Windows equivalent of POSIX `_exit`.
fn hard_exit(code: i32) -> ! {
    use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
    unsafe {
        let _ = TerminateProcess(GetCurrentProcess(), code as u32);
    }
    // TerminateProcess tears down the calling thread; not reached.
    loop {
        std::hint::spin_loop();
    }
}

/// UTF-16, NUL-terminated. The buffer must outlive the Win32 call that
/// reads its pointer.
fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Fetch the `HostState` pointer stashed on `hwnd`. `None` before the
/// pointer is published (early class/create messages) or after
/// `WM_NCDESTROY` has cleared it.
unsafe fn host_state<'a>(hwnd: HWND) -> Option<&'a HostState> {
    let raw = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
    if raw == 0 {
        None
    } else {
        Some(&*(raw as *const HostState))
    }
}

/// Host window procedure. Routes paint, input, resize, command,
/// sched-tick, and destroy; forwards everything else to
/// `DefWindowProcW`.
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if let Some(state) = host_state(hwnd) {
        // Pointer first: a hidden / locked cursor answers WM_SETCURSOR, and
        // deactivation must drop a lock before anything else runs.
        if let Some(result) = state.pointer.window_message(hwnd, msg, wparam, lparam) {
            return result;
        }
        // Focus / minimize / settings → app situation (never consumes).
        backend_windows::app_situation::window_message(msg, wparam, lparam);
    }
    match msg {
        WM_PAINT => {
            // The whole scene renders here: the backend paints the
            // view/text/icon/image tree into its double-buffered memory
            // DC and blits. Native controls paint themselves (they're
            // child HWNDs clipped out of the blit).
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            if let Some(state) = host_state(hwnd) {
                state
                    .backend
                    .borrow_mut()
                    .paint(hdc, rc.right - rc.left, rc.bottom - rc.top);
            }
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        windows::Win32::UI::WindowsAndMessaging::WM_GETMINMAXINFO
            if allow_pinned_size(hwnd, &mut *(lparam.0 as *mut windows::Win32::UI::WindowsAndMessaging::MINMAXINFO)) =>
        {
            LRESULT(0)
        }
        WM_ERASEBKGND => {
            // The scene blit covers the entire client area every frame —
            // default erasing would just flash white under animations.
            LRESULT(1)
        }
        // Painted views have no HWNDs, so their `on_touch` / `on_hover` /
        // `on_wheel` handlers are routed here (`backend_windows::view_input`
        // walks the hit chain). A handler that takes a press gets mouse
        // capture until the release, so a drag that leaves the view (or the
        // window) keeps reporting to it.
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN => {
            if let Some(state) = host_state(hwnd) {
                let (x, y) = client_point(lparam);
                let mods = view_input::current_modifiers();
                if view_input::mouse_down(&state.backend, x, y, button_of(msg), mods) {
                    SetCapture(hwnd);
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_MOUSEMOVE => {
            if let Some(state) = host_state(hwnd) {
                if !state.tracking_leave.get() {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    state.tracking_leave.set(TrackMouseEvent(&mut tme).is_ok());
                }
                let (x, y) = client_point(lparam);
                view_input::mouse_moved(&state.backend, x, y, view_input::current_modifiers());
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_MOUSELEAVE => {
            if let Some(state) = host_state(hwnd) {
                state.tracking_leave.set(false);
                view_input::mouse_left(&state.backend);
            }
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            // Another window took capture mid-drag (Alt-Tab, a popup): the
            // gesture is cancelled. Our own `ReleaseCapture` on a release
            // lands here after the gesture already ended — a no-op.
            if let Some(state) = host_state(hwnd) {
                view_input::capture_lost(&state.backend);
            }
            LRESULT(0)
        }
        WM_RBUTTONUP | WM_MBUTTONUP => {
            if let Some(state) = host_state(hwnd) {
                let (x, y) = client_point(lparam);
                let mods = view_input::current_modifiers();
                if view_input::mouse_up(&state.backend, x, y, button_of(msg), mods) {
                    let _ = ReleaseCapture();
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_LBUTTONUP => {
            let (x, y) = client_point(lparam);
            // A press a touch handler took ends here, and fires no
            // pressable (the handler owned it).
            if let Some(state) = host_state(hwnd) {
                let mods = view_input::current_modifiers();
                if view_input::mouse_up(&state.backend, x, y, PointerButton::Primary, mods) {
                    let _ = ReleaseCapture();
                    return LRESULT(0);
                }
            }
            // Painted pressables have no HWNDs; hit-test the scene tree.
            // Clone the handler out before firing so the backend borrow
            // is released (the handler writes signals whose effects
            // re-borrow the backend).
            let action = with_idle_backend(hwnd, |b| b.pressable_action(x, y)).flatten();
            if let Some(action) = action {
                action();
            }
            LRESULT(0)
        }
        WM_MOUSEHWHEEL => {
            let (x, y) = wheel_client_point(hwnd, lparam);
            let units = ((wparam.0 >> 16) & 0xffff) as u16 as i16 as f32;
            if let Some(state) = host_state(hwnd) {
                let mods = view_input::current_modifiers();
                if view_input::wheel(&state.backend, x, y, units, true, mods) {
                    return LRESULT(0);
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_MOUSEWHEEL => {
            // Wheel coordinates arrive in SCREEN space — convert before
            // hit-testing. Delta > 0 = wheel up (scroll content up), so
            // negate into content-scroll pixels (120 units per detent →
            // 40 px, the OS's 3-lines default).
            let (x, y) = wheel_client_point(hwnd, lparam);
            let delta = ((wparam.0 >> 16) & 0xffff) as u16 as i16 as f32;
            // A view's `on_wheel` goes first; one that consumes the tick
            // keeps the scroll view under it still (web `preventDefault`).
            if let Some(state) = host_state(hwnd) {
                if view_input::wheel(&state.backend, x, y, delta, false, view_input::current_modifiers()) {
                    return LRESULT(0);
                }
            }
            let scroll_px = -delta * view_input::WHEEL_PX_PER_UNIT;
            let mut handled = false;
            if let Some(state) = host_state(hwnd) {
                handled = state.backend.borrow_mut().wheel_scroll(x, y, scroll_px);
            }
            // Coalesce: when more wheel notches are already queued, let the
            // last of them paint. Scrolling is ~0.1 ms, a frame is not —
            // painting per notch let a fast wheel queue notches faster than
            // frames drained them, so the page kept scrolling for seconds
            // after the wheel stopped (idea-ui docs under GDI+: ~7 paints/s
            // against 60 notches/s). The invalidation `wheel_scroll` made
            // stays pending, so a dropped paint is never lost.
            if handled && !wheel_message_pending(hwnd) {
                // Synchronous catch-up paint, AFTER the backend borrow
                // above is released (UpdateWindow dispatches WM_PAINT
                // inline, which re-borrows the backend — inside the
                // borrow it double-borrow panics on every wheel tick).
                // Why synchronous at all: WM_PAINT is the lowest-
                // priority message and a fast wheel stream starves it,
                // so the wgpu canvas HWND (moved by SetWindowPos
                // immediately) visibly detached from the painted scene
                // that lagged frames behind. Painting in the same
                // message keeps both on the same DWM compose.
                let _ = unsafe { UpdateWindow(hwnd) };
            }
            LRESULT(0)
        }
        WM_SIZE => {
            // A resize changes the viewport with no tree change, so the
            // framework never re-`finish`es on its own — re-run layout.
            if let Some(state) = host_state(hwnd) {
                state.backend.borrow_mut().relayout();
                // The new client size, in LOGICAL px, into the viewport
                // signal — after the relayout borrow is released.
                publish_viewport(&state.backend);
            }
            LRESULT(0)
        }
        WM_DPICHANGED => {
            // The window moved to a monitor with a different scale (or the
            // user changed it). Re-scale the backend FIRST so the WM_SIZE
            // the suggested rect triggers lays out at the new scale, then
            // take the suggested rect exactly — it keeps the window's
            // logical size (see `dpi::suggested_window_pos`).
            let new_dpi = dpi::dpichanged_dpi(wparam.0);
            if let Some(state) = host_state(hwnd) {
                state.backend.borrow_mut().set_dpi(new_dpi);
            }
            let suggested = &*(lparam.0 as *const RECT);
            let (x, y, w, h) = dpi::suggested_window_pos((
                suggested.left,
                suggested.top,
                suggested.right,
                suggested.bottom,
            ));
            let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
            if let Some(state) = host_state(hwnd) {
                // The device client size can stay put across a change
                // (rounding); the logical viewport still moved.
                publish_viewport(&state.backend);
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            // Control notifications: LOWORD(wParam) = control id,
            // HIWORD(wParam) = notification code. The backend fires only
            // when the code matches what that control cares about
            // (BN_CLICKED for buttons/checkboxes, EN_CHANGE for edits),
            // so an edit's focus/update notifications don't spuriously
            // fire its on_change. Clone the handler OUT before firing so
            // the backend borrow is released — the handler writes
            // signals whose effects `borrow_mut()` the same backend.
            let control_id = (wparam.0 & 0xffff) as u16;
            let code = ((wparam.0 >> 16) & 0xffff) as u16;
            let handler = with_idle_backend(hwnd, |b| b.command_action(control_id, code)).flatten();
            if let Some(handler) = handler {
                handler();
                // SDK command handlers bypass the caps flush wraps (see
                // `backend_windows::newcore`): the hook flushes after them.
                backend_windows::dispatch_hook::fire_dispatch_hook();
            }
            LRESULT(0)
        }
        WM_HSCROLL => {
            // Trackbar (Slider) drags arrive here. `lParam` is the
            // source control's HWND. Clone the action out before firing
            // — same borrow discipline as WM_COMMAND. Non-slider
            // WM_HSCROLL (scroll bars) return None and no-op.
            let ctl = HWND(lparam.0 as *mut std::ffi::c_void);
            let action = with_idle_backend(hwnd, |b| b.slider_action(ctl)).flatten();
            if let Some(action) = action {
                action();
            }
            LRESULT(0)
        }
        WM_ACTIVATE => {
            // LOWORD(wParam) == WA_INACTIVE: the window stopped being the
            // active window and will get no further key messages — not even
            // the releases of keys held right now. Report it so the
            // keyboard dispatcher synthesizes those releases (no stuck
            // movement keys after Alt+Tab). WM_KILLFOCUS is NOT used: it
            // also fires when focus moves into one of our own child
            // controls. DefWindowProc still runs — it restores focus on
            // activation.
            if (wparam.0 & 0xffff) as u32 == WA_INACTIVE {
                if let Some(state) = host_state(hwnd) {
                    state.keyboard.focus_lost();
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_IDEALYST_SCHED => {
            // Scheduler worker woke us: run every due timer + one raf
            // tick. No backend borrow is held across this — the drained
            // closures borrow the backend themselves.
            crate::scheduler::drain_due();
            LRESULT(0)
        }
        WM_DESTROY => {
            // The clip rectangle of a pointer lock is desktop-wide and the
            // process is about to `TerminateProcess` — release it (and the
            // raw-input registration) first.
            if let Some(state) = host_state(hwnd) {
                state.pointer.shutdown();
            }
            // The window is going away — end the process here rather
            // than unwinding the message loop and dropping the reactive
            // tree. Disposing an app that owns `AnimatedValue`s tears
            // down framework-global animation state held in thread-
            // locals, and one of those drops touches an already-being-
            // destroyed TLS slot — aborting with "cannot access a
            // Thread Local Storage value during or after destruction".
            // See `hard_exit` for why this is `TerminateProcess` rather
            // than the CRT's `_exit`. This mirrors the winit host, which
            // force-exits on window close for the same class of reason
            // (`host-winit/src/app.rs`). Single-window only —
            // multi-window would decrement a live-window count and
            // exit on the last close.
            hard_exit(0);
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// The client-area point a mouse message's `lParam` carries (device px).
fn client_point(lparam: LPARAM) -> (f32, f32) {
    ((lparam.0 & 0xffff) as i16 as f32, ((lparam.0 >> 16) & 0xffff) as i16 as f32)
}

/// Wheel messages carry SCREEN coordinates; the client-area point (device
/// px) under them.
unsafe fn wheel_client_point(hwnd: HWND, lparam: LPARAM) -> (f32, f32) {
    let mut pt = POINT {
        x: (lparam.0 & 0xffff) as i16 as i32,
        y: ((lparam.0 >> 16) & 0xffff) as i16 as i32,
    };
    let _ = ScreenToClient(hwnd, &mut pt);
    (pt.x as f32, pt.y as f32)
}

fn button_of(msg: u32) -> PointerButton {
    match msg {
        WM_RBUTTONDOWN | WM_RBUTTONUP => PointerButton::Secondary,
        WM_MBUTTONDOWN | WM_MBUTTONUP => PointerButton::Middle,
        _ => PointerButton::Primary,
    }
}

/// Whether another `WM_MOUSEWHEEL` for `hwnd` is already in the queue.
fn wheel_message_pending(hwnd: HWND) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{PeekMessageW, PM_NOREMOVE};
    let mut msg = MSG::default();
    unsafe { PeekMessageW(&mut msg, Some(hwnd), WM_MOUSEWHEEL, WM_MOUSEWHEEL, PM_NOREMOVE).as_bool() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{DestroyWindow, PeekMessageW, PostMessageW, PM_REMOVE, WS_POPUP};

    /// The scheduler's wake was dispatched the moment it was retrieved,
    /// and Windows retrieves posted messages before input, so a wake
    /// always queued behind a slow frame starved the mouse and keyboard
    /// (canvas3d went "Not Responding"). The drain must wait until every
    /// other queued message has been handled.
    #[test]
    fn regression_scheduler_wake_drains_after_queued_messages() {
        const OTHER: u32 = WM_APP + 7;
        unsafe {
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                windows::core::w!("STATIC"),
                None,
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(windows::Win32::UI::WindowsAndMessaging::HWND_MESSAGE),
                None,
                None,
                None,
            )
            .unwrap();
            PostMessageW(Some(hwnd), WM_IDEALYST_SCHED, WPARAM(0), LPARAM(0)).unwrap();
            PostMessageW(Some(hwnd), OTHER, WPARAM(0), LPARAM(0)).unwrap();
            PostMessageW(Some(hwnd), OTHER, WPARAM(1), LPARAM(0)).unwrap();
            let order = RefCell::new(Vec::new());
            let code = pump(
                |msg| {
                    if msg.message == OTHER {
                        order.borrow_mut().push(format!("msg{}", msg.wParam.0));
                    }
                },
                || {
                    order.borrow_mut().push("drain".to_string());
                    windows::Win32::UI::WindowsAndMessaging::PostQuitMessage(3);
                },
            );
            assert_eq!(code, 3);
            assert_eq!(*order.borrow(), ["msg0", "msg1", "drain"]);
            let _ = DestroyWindow(hwnd);
        }
    }

    /// A host window with a live backend attached, as `run_inner` sets it
    /// up. Its WM_DESTROY ends the process, so tests hide it rather than
    /// destroy it.
    unsafe fn host_with_backend() -> (HWND, Rc<RefCell<WindowsBackend>>) {
        let hinstance = HINSTANCE(GetModuleHandleW(None).unwrap().0);
        register_host_class(hinstance).unwrap();
        let hwnd = create_host_window(hinstance, &RunOptions::default()).unwrap();
        let backend = Rc::new(RefCell::new(WindowsBackend::new(hwnd)));
        let keyboard = backend.borrow().keyboard_slot();
        let pointer = backend.borrow().pointer_slot();
        let state = Box::new(HostState {
            backend: backend.clone(),
            app: RefCell::new(None),
            keyboard,
            pointer,
            tracking_leave: std::cell::Cell::new(false),
        });
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
        (hwnd, backend)
    }

    /// Control notifications arriving while the backend is borrowed (sent
    /// synchronously from inside the backend's own Win32 calls) panicked on
    /// a double borrow inside the WndProc, which aborts the process. This
    /// sends each notification under a held borrow, as Windows does.
    #[test]
    fn regression_reentrant_control_notifications_do_not_abort() {
        unsafe {
            let (hwnd, backend) = host_with_backend();
            let held = backend.borrow_mut();
            for msg in [WM_COMMAND, WM_HSCROLL, WM_LBUTTONUP] {
                windows::Win32::UI::WindowsAndMessaging::SendMessageW(hwnd, msg, Some(WPARAM(0x0300_0064)), Some(LPARAM(0)));
            }
            drop(held);
            let _ = ShowWindow(hwnd, windows::Win32::UI::WindowsAndMessaging::SW_HIDE);
        }
    }

    /// The real trigger: mounting a text input with initial text. Windows
    /// sends EN_UPDATE / EN_CHANGE to the parent while the control is
    /// created, under the mount's borrow (Wine does not, so this only
    /// reproduced on Windows).
    #[test]
    fn regression_text_input_with_text_mounts_under_the_backend_borrow() {
        use runtime_vocabulary::caps::TextInputOps;
        unsafe {
            let (hwnd, backend) = host_with_backend();
            {
                let mut b = backend.borrow_mut();
                let _input = TextInputOps::create_text_input(
                    &mut *b,
                    "#3366ff",
                    None,
                    Rc::new(|_| {}),
                    None,
                    None,
                    false,
                    &runtime_shared::AccessibilityProps::default(),
                );
            }
            let _ = ShowWindow(hwnd, windows::Win32::UI::WindowsAndMessaging::SW_HIDE);
        }
    }

    /// A pinned viewport larger than the display must still get its full
    /// client area (the VM parity run laid out 1177 px of a 1280-px pin).
    /// 6000×4000 exceeds any display this runs on.
    #[test]
    fn regression_pinned_window_size_is_not_capped_to_the_screen() {
        unsafe {
            let hinstance = HINSTANCE(GetModuleHandleW(None).unwrap().0);
            register_host_class(hinstance).unwrap();
            PINNED_CLIENT.with(|p| p.set(Some((6000, 4000))));
            let opts = RunOptions { width: 6000, height: 4000, ..RunOptions::default() };
            let hwnd = create_host_window(hinstance, &opts).unwrap();
            let mut rc = RECT::default();
            GetClientRect(hwnd, &mut rc).unwrap();
            PINNED_CLIENT.with(|p| p.set(None));
            let scale = dpi::DpiScale::from_dpi(GetDpiForWindow(hwnd).max(dpi::BASE_DPI));
            // The host's WM_DESTROY ends the process (app shutdown): hand
            // the window to DefWindowProc before destroying it.
            unsafe extern "system" fn plain(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
                DefWindowProcW(h, m, w, l)
            }
            SetWindowLongPtrW(hwnd, windows::Win32::UI::WindowsAndMessaging::GWLP_WNDPROC, plain as usize as isize);
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd);
            let (want_w, want_h) = scale.logical_size_to_device(6000.0, 4000.0);
            assert_eq!((rc.right - rc.left, rc.bottom - rc.top), (want_w, want_h));
        }
    }

    /// The wheel coalescing test: queued notches are seen (so all but the
    /// last skip their paint) and the last one is not (so it paints). The
    /// paint itself is `UpdateWindow`, which needs the full host; this pins
    /// the decision it hangs on.
    #[test]
    fn regression_queued_wheel_notches_defer_their_paint_to_the_last() {
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                windows::core::w!("STATIC"),
                windows::core::w!("wheel-test"),
                WS_POPUP,
                0,
                0,
                10,
                10,
                None,
                None,
                None,
                None,
            )
        }
        .expect("window");
        assert!(!wheel_message_pending(hwnd), "nothing queued yet");
        unsafe {
            PostMessageW(Some(hwnd), WM_MOUSEWHEEL, WPARAM(0), LPARAM(0)).unwrap();
            PostMessageW(Some(hwnd), WM_MOUSEWHEEL, WPARAM(0), LPARAM(0)).unwrap();
        }
        let mut msg = MSG::default();
        let take = |msg: &mut MSG| unsafe { PeekMessageW(msg, Some(hwnd), WM_MOUSEWHEEL, WM_MOUSEWHEEL, PM_REMOVE).as_bool() };
        assert!(take(&mut msg));
        assert!(wheel_message_pending(hwnd), "the first notch sees the second: it skips its paint");
        assert!(take(&mut msg));
        assert!(!wheel_message_pending(hwnd), "the last notch sees none: it paints");
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }
}
