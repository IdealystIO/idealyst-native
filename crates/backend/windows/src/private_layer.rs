//! The screen recorder's private layer: painted content the user sees but
//! a recording of the app leaves out.
//!
//! Re-review: this exists for parity with the iOS / Android / macOS
//! private layer (`screen_recorder::PrivateLayer`). Its only user is the
//! screen-recorder's own `screenshare-preview-demo`; whether the feature
//! earns its keep across the backends is to be reviewed again.
//!
//! # How it works
//!
//! [`WindowsBackend::create_private_layer_window`] makes a view node that
//! is a layout root of its own, laid out against the host window's client
//! area like the main root, and an owned popup window over the host. The
//! node is detached: `insert` never adds it to its parent, so the main
//! scene's paint, hit test and layer plan never reach it.
//!
//! After each frame the backend paints every private root into
//! premultiplied pixels for the box its content covers (the same
//! transparent-layer render the lifted overlays use) and hands them to the
//! popup with `UpdateLayeredWindow`. The popup is per-pixel-alpha layered,
//! so where the layer draws nothing (alpha 0) clicks fall through to the
//! app beneath; where it draws, the popup routes the mouse into the
//! backend's input handling with the private root as the hit-test root
//! ([`hit_root`]).
//!
//! # Leaving it out of a recording
//!
//! A capture of the app's window (`CreateForWindow` on the host HWND)
//! never includes another top-level window, so the popup is absent from
//! it as is. A capture of a monitor (or a window the user picks) would
//! include it, so while any recording runs the popups carry
//! `WDA_EXCLUDEFROMCAPTURE` ([`exclude_from_capture`]). Only while a
//! recording runs: the flag hides the window from every capture —
//! screenshots, another app's screen share — which is more than the
//! layer promises.
//!
//! # Limits
//!
//! - Native controls (`button`, `text_input`, `toggle`, `slider`,
//!   `activity_indicator`) are child windows of the host, not painted
//!   content, so inside a private layer they stay in the host window, in
//!   place, and ARE recorded. A dev warning names it the first time.
//! - The popup is repainted with every frame of the host; the cost is the
//!   pixels of the layer's content box (a badge, a control strip).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use runtime_shared::PointerButton;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, ScreenToClient,
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HGDIOBJ,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowLongPtrW, LoadCursorW, RegisterClassW,
    SetWindowDisplayAffinity, SetWindowLongPtrW, SetWindowPos, ShowWindow, UpdateLayeredWindow, GWLP_USERDATA,
    IDC_ARROW, MA_NOACTIVATE, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SW_HIDE, SW_SHOWNOACTIVATE,
    ULW_ALPHA, WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CAPTURECHANGED,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEHWHEEL,
    WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCDESTROY, WM_RBUTTONDOWN, WM_RBUTTONUP, WNDCLASSW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
};

use crate::{layers, scene, view_input, NodeKind, ViewVisual, WindowsBackend, WindowsNode};

/// `WM_MOUSELEAVE` (winuser.h); windows-rs files it under `UI::Controls`,
/// a feature this crate has no other use for (the host defines it too).
const WM_MOUSELEAVE: u32 = 0x02A3;

/// A device-pixel box in host client coordinates: `(x, y, w, h)`.
pub(crate) type PixelBox = (i32, i32, u32, u32);

thread_local! {
    /// The root a forwarded mouse message hit-tests against: a private
    /// layer's, while its popup routes the message; `None` = the main root.
    static INPUT_ROOT: Cell<Option<u64>> = const { Cell::new(None) };
}

/// The root hit testing starts from: the private root whose popup is
/// routing the current message, else the main scene's.
pub(crate) fn hit_root(b: &WindowsBackend) -> Option<u64> {
    INPUT_ROOT.with(|r| r.get()).or(b.root_id)
}

/// Every root a node can be found under: the main scene's, then each
/// private layer's.
pub(crate) fn all_roots(b: &WindowsBackend) -> impl Iterator<Item = u64> + '_ {
    b.root_id.into_iter().chain(b.private.roots())
}

/// Runs `f` with `root` as the hit-test root (see [`hit_root`]).
fn routed<R>(root: u64, f: impl FnOnce() -> R) -> R {
    struct Reset(Option<u64>);
    impl Drop for Reset {
        fn drop(&mut self) {
            INPUT_ROOT.with(|r| r.set(self.0));
        }
    }
    let _reset = Reset(INPUT_ROOT.with(|r| r.replace(Some(root))));
    f()
}

/// The private layers of one backend: root node id → its popup.
#[derive(Default)]
pub(crate) struct PrivateLayers {
    windows: HashMap<u64, Popup>,
}

struct Popup {
    hwnd: HWND,
    /// The box last shown, `None` while hidden (nothing drawn).
    shown: Option<PixelBox>,
}

impl PrivateLayers {
    pub(crate) fn contains(&self, id: u64) -> bool {
        self.windows.contains_key(&id)
    }

    pub(crate) fn roots(&self) -> impl Iterator<Item = u64> + '_ {
        self.windows.keys().copied()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    /// The popup window of private root `id` (tests, introspection).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn hwnd(&self, id: u64) -> Option<HWND> {
        self.windows.get(&id).map(|p| p.hwnd)
    }

    /// Show `frame` (`(box, premultiplied BGRA)`) in root `id`'s popup over
    /// `host`, or hide the popup when there is nothing to show.
    pub(crate) fn show(&mut self, id: u64, host: HWND, frame: Option<(PixelBox, Vec<u8>)>) {
        let Some(popup) = self.windows.get_mut(&id) else { return };
        match frame {
            None => {
                if popup.shown.take().is_some() {
                    unsafe {
                        let _ = ShowWindow(popup.hwnd, SW_HIDE);
                    }
                }
            }
            Some((bx, pixels)) => {
                if let Err(e) = unsafe { update_layered(popup.hwnd, host, bx, &pixels) } {
                    eprintln!("[backend-windows] private layer: UpdateLayeredWindow failed ({e})");
                    return;
                }
                set_origin(popup.hwnd, (bx.0, bx.1));
                if popup.shown.replace(bx).is_none() {
                    unsafe {
                        let _ = ShowWindow(popup.hwnd, SW_SHOWNOACTIVATE);
                    }
                }
            }
        }
    }

    /// The host window moved: move every shown popup with it.
    pub(crate) fn follow_host(&self, host: HWND) {
        for popup in self.windows.values() {
            let Some((x, y, _, _)) = popup.shown else { continue };
            let mut pt = POINT { x, y };
            unsafe {
                let _ = ClientToScreen(host, &mut pt);
                let _ = SetWindowPos(
                    popup.hwnd,
                    None,
                    pt.x,
                    pt.y,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }
    }

    fn remove(&mut self, id: u64) {
        if let Some(popup) = self.windows.remove(&id) {
            forget_window(popup.hwnd);
            unsafe {
                let _ = DestroyWindow(popup.hwnd);
            }
        }
    }
}

impl Drop for PrivateLayers {
    fn drop(&mut self) {
        let ids: Vec<u64> = self.windows.keys().copied().collect();
        for id in ids {
            self.remove(id);
        }
    }
}

impl WindowsBackend {
    /// A private-layer root: a view laid out against the host's client
    /// area and painted into its own popup over the host, never into the
    /// main scene. The screen recorder's `PrivateLayer` handler realizes
    /// its children into the returned node. Mirrors
    /// `MacosBackend::create_private_layer_window`.
    pub fn create_private_layer_window(&mut self) -> WindowsNode {
        let node = self.add_node(NodeKind::View(ViewVisual::default()));
        // A root that never paints its own box: the popup shows only what
        // the children draw, so a transparent root keeps the rest of the
        // window clickable.
        let hwnd = unsafe { create_popup(self.host_hwnd, self.self_ref.clone(), node.id) };
        match hwnd {
            Some(hwnd) => {
                remember_window(hwnd);
                self.private.windows.insert(node.id, Popup { hwnd, shown: None });
            }
            None => eprintln!(
                "[backend-windows] private layer: popup window creation failed; its content is not shown"
            ),
        }
        self.layout_dirty = true;
        node
    }

    /// Tear down a private-layer root made by
    /// [`Self::create_private_layer_window`]: its subtree, its layout
    /// root and its popup.
    pub fn release_private_layer_window(&mut self, node: &WindowsNode) {
        if !self.private.contains(node.id) {
            return;
        }
        self.private.remove(node.id);
        self.remove_subtree(node.id);
        self.invalidate();
    }

    /// Paint each private root into its popup (after the main frame).
    pub(crate) fn present_private_layers(&mut self) {
        if self.private.is_empty() {
            return;
        }
        let roots: Vec<u64> = self.private.roots().collect();
        for root in roots {
            let frame = content_box(self, root).and_then(|bx| self.render_root_pixels(root, bx).map(|px| (bx, px)));
            let host = self.host_hwnd;
            self.private.show(root, host, frame);
        }
    }

    /// Root `root`'s subtree as premultiplied BGRA pixels for the device
    /// box `bx`, in whichever renderer paints the scene.
    pub(crate) fn render_root_pixels(&self, root: u64, bx: PixelBox) -> Option<Vec<u8>> {
        match self.renderer {
            crate::RendererKind::Gdi => unsafe { scene::paint_root_pixels(self, root, bx) },
            crate::RendererKind::D2d => {
                let mut slot = self.d2d.borrow_mut();
                if slot.is_none() {
                    // The main frame creates it; a layer can be asked first.
                    *slot = crate::d2d::D2dRenderer::new()
                        .map_err(|e| eprintln!("[backend-windows] private layer: Direct2D unavailable ({e})"))
                        .ok();
                }
                let r = slot.as_mut()?.render_root(self, root, bx);
                r.map_err(|e| eprintln!("[backend-windows] private layer: render failed ({e})")).ok()
            }
        }
    }

    /// The host window moved (`WM_MOVE`): keep the private layers over it.
    pub fn host_moved(&self) {
        self.private.follow_host(self.host_hwnd);
    }

    /// Native controls under a private root stay in the host window (they
    /// are its child windows) and are recorded — said once per process.
    pub(crate) fn warn_control_in_private_layer(&self) {
        static WARNED: OnceLock<()> = OnceLock::new();
        if cfg!(debug_assertions) {
            WARNED.get_or_init(|| {
                eprintln!(
                    "[backend-windows] a native control (button / text_input / toggle / slider / \
                     activity_indicator) inside a screen_recorder::PrivateLayer is a child window of \
                     the app's window on Windows: it shows in place but is NOT left out of recordings. \
                     Use painted content (a pressable view) for private controls."
                )
            });
        }
    }
}

/// The device box root `root`'s painted content covers, in host client
/// coordinates; `None` when it draws nothing. Placement follows the paint
/// walk with the hit test's translation-only transforms (`layers` makes
/// the same approximation), grown by a pixel for anti-aliased edges.
pub(crate) fn content_box(b: &WindowsBackend, root: u64) -> Option<PixelBox> {
    fn visit(b: &WindowsBackend, id: u64, ox: f32, oy: f32, clip: layers::Rect, acc: &mut Option<layers::Rect>) {
        let Some((nx, ny, w, h)) = scene::node_origin(b, id, ox, oy) else { return };
        let Some(meta) = b.nodes.get(&id) else { return };
        if meta.hidden || meta.effective_opacity() <= 0.003 {
            return;
        }
        let s = b.dpi.factor();
        let rect = (
            (nx * s).max(clip.0),
            (ny * s).max(clip.1),
            ((nx + w) * s).min(clip.2),
            ((ny + h) * s).min(clip.3),
        );
        let visible = rect.0 < rect.2 && rect.1 < rect.3;
        if visible && layers::draws(meta) {
            *acc = Some(match *acc {
                None => rect,
                Some(r) => (r.0.min(rect.0), r.1.min(rect.1), r.2.max(rect.2), r.3.max(rect.3)),
            });
        }
        let Some(children) = b.children.get(&id) else { return };
        let child_clip = if meta.clips_children() { rect } else { clip };
        let (cx, cy) = scene::content_origin(meta, nx, ny);
        for c in children {
            visit(b, *c, cx, cy, child_clip, acc);
        }
    }
    let mut acc = None;
    visit(b, root, 0.0, 0.0, (f32::MIN, f32::MIN, f32::MAX, f32::MAX), &mut acc);
    let (x0, y0, x1, y1) = acc?;
    Some(layers::pixel_box((x0 - 1.0, y0 - 1.0, x1 + 1.0, y1 + 1.0)))
}

// ---------------------------------------------------------------------------
// Capture exclusion
// ---------------------------------------------------------------------------

/// Every live popup (as `isize`, so the set is `Send`), for the recorder.
fn windows_set() -> &'static Mutex<Vec<isize>> {
    static SET: OnceLock<Mutex<Vec<isize>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(Vec::new()))
}

/// Recordings running (see [`exclude_from_capture`]).
static RECORDINGS: AtomicUsize = AtomicUsize::new(0);

fn remember_window(hwnd: HWND) {
    windows_set().lock().unwrap_or_else(|p| p.into_inner()).push(hwnd.0 as isize);
    if RECORDINGS.load(Ordering::SeqCst) > 0 {
        set_affinity(hwnd, true);
    }
}

fn forget_window(hwnd: HWND) {
    windows_set().lock().unwrap_or_else(|p| p.into_inner()).retain(|h| *h != hwnd.0 as isize);
}

fn set_affinity(hwnd: HWND, excluded: bool) {
    let affinity = if excluded { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE };
    if let Err(e) = unsafe { SetWindowDisplayAffinity(hwnd, affinity) } {
        // Needs Windows 10 2004; before that the popup is recorded.
        eprintln!("[backend-windows] private layer: SetWindowDisplayAffinity failed ({e})");
    }
}

/// Keeps every private-layer popup out of screen captures until dropped.
/// The screen recorder holds one for each running recording.
#[must_use = "the exclusion ends when this is dropped"]
pub struct CaptureExclusion(());

/// Leave the private layers out of captures for as long as the returned
/// guard lives — the windows that exist now and any created meanwhile.
pub fn exclude_from_capture() -> CaptureExclusion {
    if RECORDINGS.fetch_add(1, Ordering::SeqCst) == 0 {
        apply_affinity(true);
    }
    CaptureExclusion(())
}

impl Drop for CaptureExclusion {
    fn drop(&mut self) {
        if RECORDINGS.fetch_sub(1, Ordering::SeqCst) == 1 {
            apply_affinity(false);
        }
    }
}

fn apply_affinity(excluded: bool) {
    let set = windows_set().lock().unwrap_or_else(|p| p.into_inner());
    for h in set.iter() {
        set_affinity(HWND(*h as *mut std::ffi::c_void), excluded);
    }
}

/// The popup windows of every private layer (the recorder's tests check
/// their affinity).
pub fn private_layer_windows() -> Vec<isize> {
    windows_set().lock().unwrap_or_else(|p| p.into_inner()).clone()
}

// ---------------------------------------------------------------------------
// The popup window
// ---------------------------------------------------------------------------

/// Per-popup state behind `GWLP_USERDATA`.
struct PopupState {
    backend: Weak<RefCell<WindowsBackend>>,
    root: u64,
    /// The popup's top-left in host client device px: popup client
    /// coordinates + this = host client coordinates, the space the
    /// backend's input handling takes.
    origin: Cell<(i32, i32)>,
    tracking_leave: Cell<bool>,
}

unsafe fn create_popup(host: HWND, backend: Weak<RefCell<WindowsBackend>>, root: u64) -> Option<HWND> {
    static CLASS: OnceLock<()> = OnceLock::new();
    let class = w!("IdealystPrivateLayer");
    let hinstance = GetModuleHandleW(None).ok()?;
    CLASS.get_or_init(|| {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(popup_proc),
            hInstance: hinstance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);
    });
    // Owned by the host: always above it, hidden with it when minimized,
    // never in the taskbar (tool window), never activated by a click.
    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE(WS_EX_LAYERED.0 | WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0),
        class,
        PCWSTR::null(),
        WINDOW_STYLE(WS_POPUP.0),
        0,
        0,
        0,
        0,
        Some(host),
        None,
        Some(hinstance.into()),
        None,
    )
    .ok()?;
    let state = Box::new(PopupState {
        backend,
        root,
        origin: Cell::new((0, 0)),
        tracking_leave: Cell::new(false),
    });
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
    Some(hwnd)
}

unsafe fn popup_state<'a>(hwnd: HWND) -> Option<&'a PopupState> {
    let raw = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
    (raw != 0).then(|| &*(raw as *const PopupState))
}

fn set_origin(hwnd: HWND, origin: (i32, i32)) {
    if let Some(state) = unsafe { popup_state(hwnd) } {
        state.origin.set(origin);
    }
}

/// Show `pixels` (premultiplied BGRA, `w * 4` per row, top-down) as the
/// popup's content at host client box `bx`.
unsafe fn update_layered(hwnd: HWND, host: HWND, (x, y, w, h): PixelBox, pixels: &[u8]) -> windows::core::Result<()> {
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            // Negative: top-down rows, as the renderers produce them.
            biHeight: -(h as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let dc = CreateCompatibleDC(None);
    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    let bitmap = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
        Ok(b) => b,
        Err(e) => {
            let _ = DeleteDC(dc);
            return Err(e);
        }
    };
    let len = (w * h * 4) as usize;
    std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits as *mut u8, len.min(pixels.len()));
    let old = SelectObject(dc, HGDIOBJ(bitmap.0));
    let mut dst = POINT { x, y };
    let _ = ClientToScreen(host, &mut dst);
    let size = SIZE { cx: w as i32, cy: h as i32 };
    let src = POINT { x: 0, y: 0 };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    let r = UpdateLayeredWindow(hwnd, None, Some(&dst), Some(&size), Some(dc), Some(&src), COLORREF(0), Some(&blend), ULW_ALPHA);
    SelectObject(dc, old);
    let _ = DeleteObject(HGDIOBJ(bitmap.0));
    let _ = DeleteDC(dc);
    r
}

fn popup_point(state: &PopupState, lparam: LPARAM) -> (f32, f32) {
    let (ox, oy) = state.origin.get();
    let x = (lparam.0 & 0xffff) as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
    ((x + ox) as f32, (y + oy) as f32)
}

/// Wheel messages carry screen coordinates.
unsafe fn wheel_point(hwnd: HWND, state: &PopupState, lparam: LPARAM) -> (f32, f32) {
    let mut pt = POINT { x: (lparam.0 & 0xffff) as i16 as i32, y: ((lparam.0 >> 16) & 0xffff) as i16 as i32 };
    let _ = ScreenToClient(hwnd, &mut pt);
    let (ox, oy) = state.origin.get();
    ((pt.x + ox) as f32, (pt.y + oy) as f32)
}

fn button_of(msg: u32) -> PointerButton {
    match msg {
        WM_RBUTTONDOWN | WM_RBUTTONUP => PointerButton::Secondary,
        WM_MBUTTONDOWN | WM_MBUTTONUP => PointerButton::Middle,
        _ => PointerButton::Primary,
    }
}

/// The popup's window procedure: the host's mouse routing
/// (`host_win32`'s `wnd_proc`), hit-tested against the private root.
unsafe extern "system" fn popup_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_NCDESTROY {
        let raw = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        if raw != 0 {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Box::from_raw(raw as *mut PopupState));
        }
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    // A click on the layer must not take activation (and keyboard focus)
    // from the app's window.
    if msg == WM_MOUSEACTIVATE {
        return LRESULT(MA_NOACTIVATE as isize);
    }
    let Some(state) = popup_state(hwnd) else { return DefWindowProcW(hwnd, msg, wparam, lparam) };
    let Some(cell) = state.backend.upgrade() else { return DefWindowProcW(hwnd, msg, wparam, lparam) };
    let cell: Rc<RefCell<WindowsBackend>> = cell;
    let root = state.root;
    match msg {
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN => {
            let (x, y) = popup_point(state, lparam);
            let mods = view_input::current_modifiers();
            if routed(root, || view_input::mouse_down(&cell, x, y, button_of(msg), mods)) {
                SetCapture(hwnd);
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if !state.tracking_leave.get() {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                state.tracking_leave.set(TrackMouseEvent(&mut tme).is_ok());
            }
            let (x, y) = popup_point(state, lparam);
            routed(root, || view_input::mouse_moved(&cell, x, y, view_input::current_modifiers()));
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            state.tracking_leave.set(false);
            view_input::mouse_left(&cell);
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            view_input::capture_lost(&cell);
            LRESULT(0)
        }
        WM_RBUTTONUP | WM_MBUTTONUP => {
            let (x, y) = popup_point(state, lparam);
            let mods = view_input::current_modifiers();
            if routed(root, || view_input::mouse_up(&cell, x, y, button_of(msg), mods)) {
                let _ = ReleaseCapture();
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let (x, y) = popup_point(state, lparam);
            let mods = view_input::current_modifiers();
            if routed(root, || view_input::mouse_up(&cell, x, y, PointerButton::Primary, mods)) {
                let _ = ReleaseCapture();
                return LRESULT(0);
            }
            // Clone the pressable's handler out before firing: it writes
            // signals whose effects re-borrow the backend.
            let action = routed(root, || cell.try_borrow().ok().and_then(|b| b.pressable_action(x, y)));
            if let Some(action) = action {
                action();
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let (x, y) = wheel_point(hwnd, state, lparam);
            let units = ((wparam.0 >> 16) & 0xffff) as u16 as i16 as f32;
            let horizontal = msg == WM_MOUSEHWHEEL;
            let mods = view_input::current_modifiers();
            if routed(root, || view_input::wheel(&cell, x, y, units, horizontal, mods)) {
                return LRESULT(0);
            }
            if !horizontal {
                let scroll_px = -units * view_input::WHEEL_PX_PER_UNIT;
                routed(root, || {
                    if let Ok(mut b) = cell.try_borrow_mut() {
                        b.wheel_scroll(x, y, scroll_px);
                    }
                });
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RendererKind;
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::{Color, Length, StyleRules};
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowDisplayAffinity, GetWindowRect, IsWindow, IsWindowVisible, SendMessageW, WS_VISIBLE,
    };

    const CLIENT: (i32, i32) = (400, 300);

    fn host_window() -> HWND {
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("private-layer-test"),
                WS_POPUP | WS_VISIBLE,
                100,
                100,
                CLIENT.0,
                CLIENT.1,
                None,
                None,
                None,
                None,
            )
        }
        .expect("host window")
    }

    fn sized(w: f32, h: f32, bg: Option<&str>) -> Rc<StyleRules> {
        let mut s = StyleRules::default();
        s.width = Some(Length::Px(w).into());
        s.height = Some(Length::Px(h).into());
        s.flex_shrink = Some(0.0.into());
        if let Some(c) = bg {
            s.background = Some(Color::from(c).into());
        }
        Rc::new(s)
    }

    /// A blue app scene, and a private layer holding a 10 px spacer over a
    /// 40×30 red pressable: the red box sits at logical (0, 10).
    struct Scene {
        cell: Rc<RefCell<WindowsBackend>>,
        root: u64,
        layer: WindowsNode,
        clicks: Rc<Cell<u32>>,
    }

    fn scene(renderer: RendererKind) -> Scene {
        let host = host_window();
        let cell = Rc::new(RefCell::new(WindowsBackend::with_renderer(host, renderer)));
        cell.borrow_mut().set_self_ref(Rc::downgrade(&cell));
        let a11y = AccessibilityProps::default();
        let clicks = Rc::new(Cell::new(0));
        let mut b = cell.borrow_mut();
        let mut root = b.create_view(&a11y);
        let mut rs = StyleRules::default();
        rs.background = Some(Color::from("#0000ff").into());
        b.apply_style(&root, &Rc::new(rs));
        let mut layer = b.create_private_layer_window();
        let spacer = b.create_view(&a11y);
        b.apply_style(&spacer, &sized(40.0, 10.0, None));
        let counter = clicks.clone();
        let red = b.create_pressable(Rc::new(move || counter.set(counter.get() + 1)), &a11y);
        b.apply_style(&red, &sized(40.0, 30.0, Some("#ff0000")));
        b.insert(&mut layer, spacer);
        b.insert(&mut layer, red);
        let root_id = root.id;
        b.insert(&mut root, layer.clone());
        b.finish(root);
        drop(b);
        Scene { cell, root: root_id, layer, clicks }
    }

    /// Premultiplied BGRA pixel `(x, y)` of a `w`-wide frame.
    fn bgra(px: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * w + x) * 4) as usize;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    }

    const RED: [u8; 4] = [0, 0, 255, 255];

    fn renderers() -> Vec<RendererKind> {
        let mut kinds = vec![RendererKind::Gdi];
        if !crate::under_wine() {
            kinds.push(RendererKind::D2d);
        }
        kinds
    }

    /// The private layer rendered as an ordinary child: on Windows it was a
    /// placeholder with its children inside, in the app's own window, so a
    /// recording of the app contained it. Now its content paints only into
    /// its popup — never into the app window's scene — and takes input only
    /// through the popup.
    #[test]
    fn regression_private_layer_paints_in_its_own_window_not_the_apps() {
        let _lock = crate::render_test_lock();
        for kind in renderers() {
            let s = scene(kind);
            let b = s.cell.borrow();
            let full = (0, 0, CLIENT.0 as u32, CLIENT.1 as u32);

            // The app's scene: blue where the red box would be.
            let main = b.render_root_pixels(s.root, full).expect("main scene renders");
            assert_eq!(bgra(&main, full.2, 20, 25), [255, 0, 0, 255], "{kind:?}: the app scene shows its own blue there");

            // The layer: only the red box, grown a pixel for anti-aliasing.
            let bx = content_box(&b, s.layer.id).expect("the layer draws");
            assert_eq!(bx, (-1, 9, 42, 32), "{kind:?}");
            let layer = b.render_root_pixels(s.layer.id, bx).expect("layer renders");
            assert_eq!(bgra(&layer, bx.2, 21, 16), RED, "{kind:?}: the box, in the layer");

            // Input: the app's hit test doesn't reach the layer; the
            // layer's does.
            assert!(b.pressable_action(20.0, 25.0).is_none(), "{kind:?}: the app window doesn't take the layer's clicks");
            let action = routed(s.layer.id, || b.pressable_action(20.0, 25.0));
            assert!(action.is_some(), "{kind:?}: the layer's own hit test finds its pressable");
            assert!(routed(s.layer.id, || b.pressable_action(20.0, 100.0)).is_none(), "{kind:?}: nothing below the box");
        }
    }

    /// After a frame the popup shows the layer's content box over the host,
    /// routes a click there to the pressable, carries the capture exclusion
    /// only while a recording holds it, and goes with the layer.
    #[test]
    fn private_layer_popup_shows_routes_and_hides_from_capture() {
        let _lock = crate::render_test_lock();
        let s = scene(RendererKind::Gdi);
        s.cell.borrow_mut().present_private_layers();
        let popup = s.cell.borrow().private.hwnd(s.layer.id).expect("a popup");
        unsafe {
            assert!(IsWindowVisible(popup).as_bool(), "shown once there is content");
            let mut r = RECT::default();
            let _ = GetWindowRect(popup, &mut r);
            // Host client origin is (100, 100) on screen; box (-1, 9, 42, 32).
            assert_eq!((r.left, r.top, r.right - r.left, r.bottom - r.top), (99, 109, 42, 32));

            // A click on the box, in popup client coordinates: host (19, 25).
            let at = LPARAM(((16 << 16) | 20) as isize);
            SendMessageW(popup, WM_LBUTTONDOWN, Some(WPARAM(0)), Some(at));
            SendMessageW(popup, WM_LBUTTONUP, Some(WPARAM(0)), Some(at));
            assert_eq!(s.clicks.get(), 1, "the popup routes the click to the layer's pressable");

            let mut affinity = 0u32;
            let _ = GetWindowDisplayAffinity(popup, &mut affinity);
            assert_eq!(affinity, WDA_NONE.0, "not excluded while nothing records");
            let guard = exclude_from_capture();
            let _ = GetWindowDisplayAffinity(popup, &mut affinity);
            assert_eq!(affinity, WDA_EXCLUDEFROMCAPTURE.0, "excluded while a recording runs");
            drop(guard);
            let _ = GetWindowDisplayAffinity(popup, &mut affinity);
            assert_eq!(affinity, WDA_NONE.0, "and only then");

            s.cell.borrow_mut().release_private_layer_window(&s.layer);
            assert!(!IsWindow(Some(popup)).as_bool(), "the popup goes with the layer");
            assert!(!s.cell.borrow().nodes.contains_key(&s.layer.id));
            assert!(!private_layer_windows().contains(&(popup.0 as isize)));
        }
    }
}
