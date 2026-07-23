//! Win32 window + message pump + `runtime_core::mount` driver.
//!
//! `run_with` registers a window class, opens one top-level window,
//! constructs a [`WindowsBackend`] rooted at its HWND, installs the
//! scheduler, mounts the app, and pumps messages until the window
//! closes. Per-window state (the shared backend + the reactive
//! [`Owner`]) lives in a heap [`HostState`] whose pointer is stashed
//! in the window's `GWLP_USERDATA` so the `WndProc` can reach it.

use std::cell::RefCell;
use std::rc::Rc;

use backend_windows::WindowsBackend;
use runtime_core::{Element, Owner};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW,
    GetWindowLongPtrW, LoadCursorW, RegisterClassExW, SetWindowLongPtrW, ShowWindow,
    TranslateMessage, CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA, IDC_ARROW, MSG, SW_SHOW,
    WINDOW_EX_STYLE, WM_APP, WM_COMMAND, WM_DESTROY, WM_HSCROLL, WM_SIZE, WNDCLASSEXW,
    WS_OVERLAPPEDWINDOW,
};

use crate::RunOptions;

/// Private wake message the scheduler worker `PostMessageW`s to the
/// host window; the `WndProc` responds by draining due timers/rafs.
/// `WM_APP + 1` sits in the app-private message range so it can't
/// collide with a system or common-control notification.
pub(crate) const WM_IDEALYST_SCHED: u32 = WM_APP + 1;

/// Window-class name for the host's single top-level window.
const HOST_CLASS_NAME: PCWSTR = PCWSTR(windows::core::w!("IdealystHostWindow").as_ptr());

/// Per-window state reachable from the `WndProc` via `GWLP_USERDATA`.
/// Boxed on the heap; the raw pointer is stored on the window and the
/// box is reclaimed in `WM_NCDESTROY`. Dropping it disposes the
/// reactive tree (`owner`) and then the backend (whose own `Drop`
/// releases any child HWNDs Windows hasn't already torn down).
struct HostState {
    backend: Rc<RefCell<WindowsBackend>>,
    /// The reactive owner returned by `mount`. Kept alive so the UI
    /// stays reactive; dropped on window teardown. `RefCell<Option<>>`
    /// because the pointer is published to the window *before* mount
    /// runs (so a `WM_SIZE` during `ShowWindow` finds the backend),
    /// and the owner is slotted in afterwards.
    owner: RefCell<Option<Owner>>,
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

/// As [`run`], but invokes `register` on the freshly constructed
/// [`WindowsBackend`] before the app mounts — third-party SDKs whose
/// `register(&mut B)` installs `Element::External` handlers must run
/// before the first tree walk. Mirrors the winit / AppKit / GTK hosts'
/// `run_with`.
pub fn run_with<R, F>(opts: RunOptions, register: R, build_ui: F) -> i32
where
    R: FnOnce(&mut WindowsBackend) + 'static,
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
    R: FnOnce(&mut WindowsBackend),
    F: FnOnce() -> Element,
{
    // hInstance for the window class — the current executable's module.
    let hmodule = GetModuleHandleW(None).map_err(|e| format!("GetModuleHandleW: {e}"))?;
    let hinstance = HINSTANCE(hmodule.0);

    register_host_class(hinstance)?;
    let hwnd = create_host_window(hinstance, &opts)?;

    // Build the backend rooted at the window, run extension
    // registration, then share it behind an Rc<RefCell<>> — the
    // WndProc, the scheduler drains, and the reactive effects all
    // borrow the same instance.
    let mut backend = WindowsBackend::new(hwnd);
    register(&mut backend);
    let backend = Rc::new(RefCell::new(backend));

    // Publish per-window state to the WndProc before anything can
    // dispatch a message to this window.
    let state = Box::new(HostState {
        backend: backend.clone(),
        owner: RefCell::new(None),
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

    // Mount: the walker builds the child-HWND tree and calls
    // `finish(root)`, laying it out against the shown window.
    let owner = runtime_core::mount(backend.clone(), build_ui);
    (*state_ptr).owner.borrow_mut().replace(owner);

    // Belt-and-suspenders initial layout in case the show-time
    // `WM_SIZE` raced ahead of mount.
    backend.borrow_mut().relayout();

    // Pump until `WM_QUIT` (posted by `WM_DESTROY`). `GetMessageW`
    // returns 0 on WM_QUIT and -1 on error; `> 0` exits on both.
    let mut msg = MSG::default();
    while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    // Fallback: the (single) window `_exit`s from `WM_DESTROY`, so the
    // loop normally never returns here. If it does (a stray `WM_QUIT`
    // posted some other way), hard-exit for the same reason — never
    // fall through into destructor-running process teardown.
    hard_exit(msg.wParam.0 as i32);
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
        // No class background brush: the framework's root View paints
        // its own background once styling lands, and a system brush
        // here would flash the wrong color behind it.
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

/// Create the top-level window sized so its *client* area is exactly
/// `opts.width × opts.height` (via `AdjustWindowRectEx`, which inflates
/// for the frame + title bar).
unsafe fn create_host_window(hinstance: HINSTANCE, opts: &RunOptions) -> Result<HWND, String> {
    let ex_style = WINDOW_EX_STYLE(0);
    let style = WS_OVERLAPPEDWINDOW;

    let mut rect = RECT {
        left: 0,
        top: 0,
        right: opts.width.max(1),
        bottom: opts.height.max(1),
    };
    // Ignore an AdjustWindowRectEx error (extremely unlikely for a
    // standard style) — fall back to client dims as window dims.
    let _ = AdjustWindowRectEx(&mut rect, style, false, ex_style);
    let win_w = rect.right - rect.left;
    let win_h = rect.bottom - rect.top;

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
        HWND(std::ptr::null_mut()),
        None,
        hinstance,
        None,
    )
    .map_err(|e| format!("CreateWindowExW: {e}"))?;

    if hwnd.is_invalid() {
        return Err("CreateWindowExW returned null".into());
    }
    Ok(hwnd)
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

/// Host window procedure. Routes the four messages the backend needs
/// (resize → relayout, command → button dispatch, sched-tick → drain,
/// destroy → quit + teardown) and forwards everything else to
/// `DefWindowProcW`.
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_SIZE => {
            // A resize changes the viewport with no tree change, so the
            // framework never re-`finish`es on its own — re-run layout.
            if let Some(state) = host_state(hwnd) {
                state.backend.borrow_mut().relayout();
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
            let handler = host_state(hwnd)
                .and_then(|state| state.backend.borrow().command_action(control_id, code));
            if let Some(handler) = handler {
                handler();
            }
            LRESULT(0)
        }
        WM_HSCROLL => {
            // Trackbar (Slider) drags arrive here (forwarded up from the
            // containing view). `lParam` is the source control's HWND.
            // Clone the action out before firing — same borrow
            // discipline as WM_COMMAND. Non-slider WM_HSCROLL (scroll
            // bars) return None and no-op.
            let ctl = HWND(lparam.0 as *mut std::ffi::c_void);
            let action =
                host_state(hwnd).and_then(|state| state.backend.borrow().slider_action(ctl));
            if let Some(action) = action {
                action();
            }
            LRESULT(0)
        }
        WM_IDEALYST_SCHED => {
            // Scheduler worker woke us: run every due timer + one raf
            // tick. No backend borrow is held across this — the drained
            // closures borrow the backend themselves.
            crate::scheduler::drain_due();
            LRESULT(0)
        }
        WM_DESTROY => {
            // The window is going away — end the process here rather
            // than unwinding the message loop and dropping the reactive
            // tree. Disposing an app that owns `AnimatedValue`s tears
            // down framework-global animation state held in thread-
            // locals, and one of those drops touches an already-being-
            // destroyed TLS slot — aborting with "cannot access a
            // Thread Local Storage value during or after destruction".
            // `_exit` terminates immediately, skipping every
            // destructor; the OS reclaims memory, threads, and the
            // window's HWNDs. See `hard_exit` for why this is
            // `TerminateProcess` rather than the CRT's `_exit`. This
            // mirrors the winit host, which force-exits on window
            // close for the same class of reason
            // (`host-winit/src/app.rs`). Single-window only —
            // multi-window would decrement a live-window count and
            // exit on the last close.
            hard_exit(0);
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
