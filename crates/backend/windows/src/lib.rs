//! Native Win32 backend — scaffold.
//!
//! Implements `runtime_core::Backend` over raw HWND child controls.
//! Author code that mounts on Windows gets real `View` containers
//! (parent HWNDs) plus `Text` (`STATIC` class) and `Button` (`BUTTON`
//! class) controls; every other primitive renders a placeholder text
//! label so the missing widget is visible at run-time rather than
//! panicking via the framework's `unimplemented!()` defaults.
//!
//! The placeholder posture follows the same convention as
//! `backend-cpu` — silent no-ops hide the gap, visible placeholders
//! surface it. See `feedback_cpu_unsupported_placeholders` for the
//! design rationale.
//!
//! ## Threading
//!
//! HWND methods (`CreateWindowExW`, `SendMessageW`, etc.) are
//! single-threaded — they must be invoked from the thread that
//! created the parent window. The host shell is responsible for
//! enforcing this; the backend assumes it's running on the right
//! thread and calls Win32 inline.
//!
//! ## Build gating
//!
//! The lib body is gated on `cfg(target_os = "windows")`. On other
//! hosts (the workspace's day-to-day macOS dev environment, CI's
//! Linux runners) this crate compiles to an empty rlib. Don't add
//! cross-platform code outside the cfg — keep the Win32 surface
//! isolated.

#![cfg(target_os = "windows")]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::color::Rgba;
use runtime_core::{
    Action, Backend, Color, ColorScheme, Length, Platform, StyleRules,
};
use runtime_layout::LayoutTree;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontIndirectW, CreatePen, CreateRoundRectRgn, CreateSolidBrush, DeleteObject,
    EndPaint, FillRect, FillRgn, GetDC, GetStockObject, GetTextExtentPoint32W, InvalidateRect,
    Rectangle, ReleaseDC, RoundRect, SelectObject, SetBkColor, SetTextColor, HBRUSH, HFONT,
    HGDIOBJ, HPEN, HRGN, NULL_BRUSH, PAINTSTRUCT, PS_SOLID,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetAncestor, GetClientRect, GetWindowLongPtrW,
    GetWindowTextLengthW, GetWindowTextW, RegisterClassExW, SendMessageW, SetWindowLongPtrW,
    SetWindowPos, SetWindowTextW, ShowWindow, SystemParametersInfoW, BS_DEFPUSHBUTTON, CS_HREDRAW,
    CS_VREDRAW, GA_ROOT, WM_COMMAND, WM_HSCROLL, WM_VSCROLL,
    GWLP_USERDATA, HMENU, NONCLIENTMETRICSW, SPI_GETNONCLIENTMETRICS, SWP_NOACTIVATE, SWP_NOZORDER,
    SW_SHOW, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WINDOW_EX_STYLE, WM_CTLCOLORSTATIC,
    WM_ERASEBKGND, WM_LBUTTONUP, WM_MOUSEWHEEL, WM_NCDESTROY, WM_PAINT, WM_SETFONT, WNDCLASSEXW,
    WS_BORDER, WS_CHILD, WS_CLIPCHILDREN, WS_VISIBLE,
};
use windows::Win32::UI::Controls::{
    InitCommonControlsEx, ICC_BAR_CLASSES, ICC_PROGRESS_CLASS, INITCOMMONCONTROLSEX,
};
use windows::Win32::Foundation::RECT;

// STATIC control style constant. The `windows` crate dropped the SS_*
// family from its `WindowsAndMessaging` re-exports between 0.5x and
// 0.58 (only the BS_* button-style family survived). Per Win32 headers
// (winuser.h): SS_LEFT = 0x00000000. A plain bit-flag, not a
// WINDOW_STYLE struct constant, so it doesn't need `.0` field access —
// just `|` it into the style u32.
const SS_LEFT: i32 = 0x0000_0000;

// Win32 control constants the `windows` crate doesn't re-export in a
// convenient shape (or that are clearer inlined). Values are from the
// Win32 headers (winuser.h / commctrl.h) and are ABI-stable.
//
// WM_COMMAND notification codes (HIWORD of wParam):
const BN_CLICKED: u16 = 0; // button / checkbox pressed
const EN_CHANGE: u16 = 0x0300; // edit control text changed
// Checkbox: BS_AUTOCHECKBOX toggles its own check state on click.
const BS_AUTOCHECKBOX: u32 = 0x0000_0003;
const BM_GETCHECK: u32 = 0x00F0;
const BM_SETCHECK: u32 = 0x00F1;
const BST_CHECKED: isize = 1;
// Edit control styles.
const ES_AUTOHSCROLL: u32 = 0x0080;
const ES_PASSWORD: u32 = 0x0020;

/// Flag bit OR-ed into a Text STATIC's `GWLP_USERDATA` to mark "a text
/// color is stored in the low 24 bits". Bit 24 sits just above a
/// `COLORREF`'s 0x00BBGGRR payload, so `0x000000` (black) is still
/// distinguishable from "unset" (slot == 0).
const TEXT_COLOR_FLAG: isize = 1 << 24;

/// `EDIT` window class (single-line text input).
fn class_edit() -> PCWSTR {
    PCWSTR(windows::core::w!("EDIT").as_ptr())
}

// Common controls (comctl32) — need `InitCommonControlsEx` before use.
/// `msctls_trackbar32` — the Slider control.
fn class_trackbar() -> PCWSTR {
    PCWSTR(windows::core::w!("msctls_trackbar32").as_ptr())
}
/// `msctls_progress32` — used as an indeterminate ActivityIndicator
/// (marquee mode).
fn class_progress() -> PCWSTR {
    PCWSTR(windows::core::w!("msctls_progress32").as_ptr())
}
const TBS_HORZ: u32 = 0x0000;
const TBM_GETPOS: u32 = 0x0400; // WM_USER
const TBM_SETPOS: u32 = 0x0405; // WM_USER+5
const TBM_SETRANGE: u32 = 0x0406; // WM_USER+6; lParam = MAKELONG(min, max)
/// Trackbars are integer-positioned; we map `[0, SLIDER_RESOLUTION]`
/// onto the author's `[min, max]` float range so drags read back with
/// ~0.1% precision regardless of the value range.
const SLIDER_RESOLUTION: i32 = 1000;
const PBS_MARQUEE: u32 = 0x08;
const PBM_SETMARQUEE: u32 = 0x040A; // WM_USER+10

/// Initialize the comctl32 trackbar + progress window classes once per
/// process. Required before `CreateWindowExW` on `msctls_*` classes.
fn ensure_common_controls() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| unsafe {
        let icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_BAR_CLASSES | ICC_PROGRESS_CLASS,
        };
        let _ = InitCommonControlsEx(&icc);
    });
}

/// Map an author `value` in `[min, max]` to an integer trackbar
/// position in `[0, SLIDER_RESOLUTION]`, clamped.
fn value_to_slider_pos(value: f32, min: f32, max: f32) -> i32 {
    if max <= min {
        return 0;
    }
    let frac = ((value - min) / (max - min)).clamp(0.0, 1.0);
    (frac * SLIDER_RESOLUTION as f32).round() as i32
}

/// Read an HWND's window text (an EDIT control's current contents)
/// into a `String`. Returns empty for no text.
fn window_text(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        // +1 for the NUL `GetWindowTextW` writes; it returns the count
        // of chars copied (excluding the NUL).
        let mut buf = vec![0u16; (len + 1) as usize];
        let n = GetWindowTextW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}

// =========================================================================
// Node — opaque HWND wrapper so the Backend trait's `type Node` is Clone
// =========================================================================

/// Backend-internal handle for a mounted Win32 widget. Wraps the HWND
/// plus a stable monotonic id so framework `Clone` semantics work
/// (HWND itself is `Copy`, but we wrap it to keep the per-node
/// metadata reachable from a single key).
#[derive(Clone)]
pub struct WindowsNode {
    pub(crate) id: u64,
    pub(crate) hwnd: HWND,
}

impl WindowsNode {
    /// Access the underlying HWND for SDK extensions that need to
    /// send Win32 messages directly (e.g. toolbar leaf's
    /// `SendMessageW` for TB_ADDBUTTONS).
    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// Internal id allocator output. Useful for SDK code that needs
    /// to correlate a node back to backend-side metadata.
    pub fn id(&self) -> u64 {
        self.id
    }
}

impl std::fmt::Debug for WindowsNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowsNode")
            .field("id", &self.id)
            .field("hwnd", &(self.hwnd.0 as usize))
            .finish()
    }
}

// =========================================================================
// Backend
// =========================================================================

pub struct WindowsBackend {
    /// Parent HWND every child control hangs off of. Owned by the
    /// host shell and handed in at construction.
    host_hwnd: HWND,
    /// Monotonic id allocator for `WindowsNode`.
    next_id: u64,
    /// Per-node metadata (children list, on-click handler, etc.).
    #[allow(dead_code)]
    nodes: HashMap<u64, NodeMeta>,
    /// Parallel Taffy layout tree. Same model as iOS / macOS /
    /// Android — child layout nodes parented under the host's root
    /// layout node so flex semantics work uniformly across backends.
    pub(crate) layout: LayoutTree,
    /// `id → LayoutNode` mapping so finish() can walk every
    /// registered HWND and assign its frame.
    layout_for_id: HashMap<u64, runtime_layout::LayoutNode>,
    /// The root node the walker last handed to [`Self::finish`].
    /// Stashed so the host shell can re-run the layout pass on a
    /// window resize (`WM_SIZE`) without the walker being in the
    /// loop — the framework only calls `finish` after a build/
    /// re-render, but a resize changes the client rect with no
    /// tree change. See [`Self::relayout`].
    root_node: Option<WindowsNode>,
    /// Next available Win32 control id. Win32 reserves 0..100
    /// (the IDOK / IDCANCEL / IDABORT family + standard control
    /// ids); we start at 100 so our buttons don't collide with
    /// anything the host might handle separately. u16 because
    /// `WM_COMMAND` carries the control id in the low word of
    /// wParam.
    next_control_id: u16,
    /// Click handlers keyed by Win32 control id. The host's
    /// `WndProc` calls [`Self::dispatch_command`] from its
    /// `WM_COMMAND` arm; we look up the closure and invoke it.
    /// Stored as `Rc` so the same closure can also live on the
    /// `NodeMeta`'s `on_click` slot for diagnostics + future
    /// keyboard-activation routing.
    command_handlers: HashMap<u16, CommandEntry>,
    /// parent node id → child node ids, in insertion order. Populated
    /// by [`Backend::insert`]; consumed by [`Backend::clear_children`]
    /// to know which HWNDs + metadata to tear down (Win32 has no cheap
    /// "logical children of this node" query that matches the
    /// framework's tree).
    children: HashMap<u64, Vec<u64>>,
    /// trackbar HWND (as `isize`) → its `on_change` fire closure.
    /// Keyed by HWND because `WM_HSCROLL` identifies the source control
    /// by handle (in `lParam`), not by a `WM_COMMAND`-style id.
    slider_handlers: HashMap<isize, Rc<dyn Fn()>>,
    /// Third-party `Element::External` registry. Populated by
    /// `register_external::<T>(...)` calls from per-platform leaf
    /// crates (e.g. `toolbar::register_windows`). `create_external`
    /// looks up the handler by payload TypeId; unregistered kinds
    /// fall through to a "not supported" placeholder. Mirrors the
    /// iOS / macOS pattern.
    pub(crate) external_handlers: runtime_core::ExternalRegistry<WindowsBackend>,
    /// The UI font applied to every control (`WM_SETFONT`) and used to
    /// measure text for layout. Without an explicit font, Win32
    /// controls fall back to the ancient bitmap "System" font; we pull
    /// the shell's message font (Segoe UI on modern Windows) so
    /// controls look native. Owned — deleted in `Drop`.
    font: HFONT,
    /// Cached single-line height of [`Self::font`], in pixels. Used as
    /// the intrinsic height for text so a text leaf lays out with a
    /// real height even before per-glyph measurement.
    line_height: i32,
}

/// A registered `WM_COMMAND` handler plus the notification code that
/// should trigger it. Buttons/checkboxes/menu items fire on
/// `BN_CLICKED` (0); edit controls fire on `EN_CHANGE`. Filtering on
/// the code stops an edit's focus/update notifications (all delivered
/// as `WM_COMMAND` under the same control id) from spuriously firing
/// its `on_change`.
struct CommandEntry {
    code: u16,
    action: Rc<dyn Fn()>,
}

struct NodeMeta {
    /// HWND we created for this node.
    #[allow(dead_code)]
    hwnd: HWND,
    /// On-click handler. Same `Rc` is also registered in
    /// [`WindowsBackend::command_handlers`] under
    /// `command_id`. Held here too so Drop can clean both
    /// stores in one place.
    #[allow(dead_code)]
    on_click: Option<Rc<dyn Fn()>>,
    /// Win32 control id passed to `CreateWindowExW` via the
    /// `hMenu` slot. `None` for non-interactive widgets
    /// (Text labels, container Statics). Used by `Drop` to
    /// retire the matching entry in `command_handlers`.
    control_id: Option<u16>,
    /// `true` when the HWND is an `IdealystView` window (View /
    /// Pressable) whose `GWLP_USERDATA` holds a `Box<ViewPaint>`.
    /// `apply_style` reads this to know it may push visual style
    /// (background / border / radius) into the paint state and
    /// repaint. Other node kinds (Text/Button STATIC, scroll views,
    /// placeholders) have no `ViewPaint` and skip that path.
    is_view: bool,
    /// `true` for a `Text` STATIC control. `apply_style` packs the
    /// resolved text color into this HWND's `GWLP_USERDATA` (with a
    /// flag bit), and the parent `IdealystView`'s `WM_CTLCOLORSTATIC`
    /// reads it back to `SetTextColor`. Gated so we never overwrite
    /// the `GWLP_USERDATA` a scroll view (`ScrollState`) or view
    /// (`ViewPaint`) keeps there.
    is_text: bool,
}

impl WindowsBackend {
    /// Construct a backend rooted at `host_hwnd`. The host shell
    /// owns the parent window; the backend creates all child controls
    /// underneath. Drop `WindowsBackend` to release every child HWND
    /// the backend has created.
    pub fn new(host_hwnd: HWND) -> Self {
        let (font, line_height) = create_ui_font();
        Self {
            host_hwnd,
            next_id: 1,
            nodes: HashMap::new(),
            layout: LayoutTree::new(),
            layout_for_id: HashMap::new(),
            root_node: None,
            next_control_id: 100,
            command_handlers: HashMap::new(),
            children: HashMap::new(),
            slider_handlers: HashMap::new(),
            external_handlers: runtime_core::ExternalRegistry::new(),
            font,
            line_height,
        }
    }

    /// Borrow the host HWND. Third-party SDK extensions (the `menu`
    /// SDK's HMENU attach via `SetMenu`, future toolbar leaf's
    /// child-window parent) reach the host window through this.
    pub fn host_hwnd(&self) -> HWND {
        self.host_hwnd
    }

    /// Register a handler for the third-party external primitive whose
    /// payload type is `T`. Called by per-platform leaf crates during
    /// app bootstrap (`toolbar::register(&mut backend)`). The handler
    /// receives the typed payload + a mutable borrow of the backend
    /// and produces the `WindowsNode` to mount. Mirrors the iOS /
    /// macOS pattern.
    pub fn register_external<T, F>(&mut self, handler: F)
    where
        T: 'static,
        F: Fn(&Rc<T>, &mut WindowsBackend) -> WindowsNode + 'static,
    {
        self.external_handlers.register::<T, _>(handler);
    }

    /// `true` if a handler for payload type `T` has been registered.
    /// Useful for opt-in graceful degradation in user code.
    pub fn has_external<T: 'static>(&self) -> bool {
        self.external_handlers.has::<T>()
    }

    /// SDK extension helper: allocate a fresh Win32 control id +
    /// install `on_click` as its WM_COMMAND handler. Returns the id
    /// for use as the lParam/hMenu/menu-item-id of whatever native
    /// control fires WM_COMMAND with that id. Used by the `menu`
    /// SDK to wire HMENU command items into the same dispatch path
    /// buttons use — host's WndProc calls
    /// [`Self::dispatch_command`] with `LOWORD(wParam)` and our
    /// stored closure fires.
    pub fn register_command_handler(&mut self, on_click: Rc<dyn Fn()>) -> u16 {
        let id = self.alloc_control_id();
        self.command_handlers
            .insert(id, CommandEntry { code: BN_CLICKED, action: on_click });
        id
    }

    /// Install a `WM_COMMAND` handler under a caller-supplied id.
    /// Used by SDK leaves (`toolbar::flush_pending`) that allocate
    /// ids out of their own namespace — e.g. the toolbar SDK uses
    /// 40000+ to avoid colliding with the backend's own 100+ pool
    /// for buttons. Overwrites any existing handler at that id.
    pub fn install_command_handler_with_id(&mut self, id: u16, on_click: Rc<dyn Fn()>) {
        self.command_handlers
            .insert(id, CommandEntry { code: BN_CLICKED, action: on_click });
    }

    /// SDK extension helper: register an HWND with the backend's
    /// layout tree so flex parents can size + position it. Third-
    /// party `register_external` handlers call this once after
    /// constructing their native child window so the layout pass
    /// picks it up. Without it, the HWND is laid out as 0×0.
    pub fn register_external_view(&mut self, hwnd: HWND) -> WindowsNode {
        let id = self.alloc_id();
        let layout = self.layout.new_node();
        self.layout_for_id.insert(id, layout);
        self.nodes.insert(
            id,
            NodeMeta {
                hwnd,
                on_click: None,
                control_id: None,
                is_view: false,
                is_text: false,
            },
        );
        WindowsNode { id, hwnd }
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Allocate the next Win32 control id. Wraps at `u16::MAX`;
    /// in practice apps allocate a few hundred at most, so the
    /// 65 k id space is plenty. Wrap-around would silently
    /// collide with a still-live handler — log a warning when
    /// it happens so the failure mode is visible.
    fn alloc_control_id(&mut self) -> u16 {
        let id = self.next_control_id;
        self.next_control_id = self.next_control_id.wrapping_add(1);
        if self.next_control_id == 0 {
            eprintln!(
                "[backend-windows] control id allocator wrapped \
                 — distinct controls may now share an id"
            );
            self.next_control_id = 100;
        }
        id
    }

    /// Fire the click handler registered for `control_id`. The
    /// host shell calls this from its `WndProc`'s `WM_COMMAND`
    /// arm with `LOWORD(wParam)` as the id; we look up the
    /// closure and invoke it. Returns `true` if a handler was
    /// found and fired, `false` if the id is unknown (which
    /// means either a button has been released since the message
    /// was queued, or the host received `WM_COMMAND` from
    /// something we didn't create).
    pub fn dispatch_command(&self, control_id: u16) -> bool {
        if let Some(entry) = self.command_handlers.get(&control_id) {
            (entry.action)();
            return true;
        }
        false
    }

    /// Clone out the handler for `control_id` **iff** the WM_COMMAND
    /// notification `code` (HIWORD of wParam) matches the code that
    /// control cares about — `BN_CLICKED` for buttons/checkboxes,
    /// `EN_CHANGE` for edits. The host shell calls this from its
    /// `WM_COMMAND` arm and fires the returned closure *after*
    /// releasing the backend borrow: a handler routinely writes a
    /// signal, whose reactive effects `borrow_mut()` the same backend,
    /// so firing under the lookup borrow would double-borrow-panic.
    /// Returns `None` for an unknown id or a non-matching code (e.g. an
    /// edit's focus/update notifications, which must not fire
    /// `on_change`).
    pub fn command_action(&self, control_id: u16, code: u16) -> Option<Rc<dyn Fn()>> {
        self.command_handlers
            .get(&control_id)
            .filter(|e| e.code == code)
            .map(|e| e.action.clone())
    }

    /// Clone out the `on_change` fire closure for the trackbar (Slider)
    /// whose HWND is `hwnd`. The host calls this from its `WM_HSCROLL`
    /// arm (`lParam` is the source control) and fires it after
    /// releasing the backend borrow — same re-entrancy discipline as
    /// [`Self::command_action`]. `None` if `hwnd` isn't one of our
    /// sliders (e.g. a scroll bar).
    pub fn slider_action(&self, hwnd: HWND) -> Option<Rc<dyn Fn()>> {
        self.slider_handlers.get(&(hwnd.0 as isize)).cloned()
    }

    /// Re-run the Taffy layout pass against the current host client
    /// rect and re-position every child HWND. The host shell calls
    /// this from its `WndProc`'s `WM_SIZE` arm: a resize changes the
    /// viewport with no change to the element tree, so the framework
    /// never calls [`Self::finish`] again on its own. No-op before
    /// the first `finish` (nothing mounted yet). Cheap when the tree
    /// is small; identical work to the tail of `finish`.
    pub fn relayout(&mut self) {
        if let Some(root) = self.root_node.clone() {
            self.finish(root);
        }
    }

    /// Create a child HWND of class `class_name` with `text` as
    /// its initial window text, parented under the host HWND.
    ///
    /// `control_id` is passed via `CreateWindowExW`'s `hMenu` slot
    /// — when the window is a `WS_CHILD`, Win32 reinterprets that
    /// slot as the child's control id, which is what
    /// `WM_COMMAND` reports back in `LOWORD(wParam)`. `None`
    /// means "no command routing needed" (Text labels, containers
    /// that never fire WM_COMMAND).
    fn create_child(
        &mut self,
        class_name: PCWSTR,
        text: &str,
        style: u32,
        control_id: Option<u16>,
    ) -> WindowsNode {
        let text_wide = to_pcwstr(text);
        // hMenu carries the control id for WS_CHILD windows. HMENU is a
        // HANDLE newtype wrapping a pointer; casting a u16 through
        // `usize` to a pointer gives Win32 the value it wants in the
        // low word. When there's no control id (Text labels, etc.)
        // we pass a null HMENU instead — windows 0.58 dropped the
        // `Option<HMENU>` parameter shape in favor of plain `HMENU`
        // with `HMENU(null)` meaning "no menu", same as the Win32 C
        // API takes a literal `NULL` pointer.
        let hmenu: HMENU = match control_id {
            Some(cid) => HMENU(cid as usize as *mut std::ffi::c_void),
            None => HMENU(std::ptr::null_mut()),
        };
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class_name,
                text_wide.as_pcwstr(),
                windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(style)
                    | WS_CHILD
                    | WS_VISIBLE,
                0,
                0,
                0,
                0,
                // windows 0.58 changed CreateWindowExW's parent/hMenu
                // parameters from `Option<HWND>` / `Option<HMENU>` to
                // `Param<HWND>` / `Param<HMENU>`. Plain values now;
                // null when you want "none", not `None`.
                self.host_hwnd,
                hmenu,
                None,
                None,
            )
        }
        .unwrap_or(HWND(std::ptr::null_mut()));
        let _ = unsafe { ShowWindow(hwnd, SW_SHOW) };

        // Give the control the modern shell font. `WM_SETFONT` with
        // `wParam = HFONT` and `lParam = TRUE` (redraw). Without this a
        // STATIC / BUTTON renders in the legacy bitmap "System" font.
        if !hwnd.is_invalid() && !self.font.is_invalid() {
            unsafe {
                SendMessageW(
                    hwnd,
                    WM_SETFONT,
                    WPARAM(self.font.0 as usize),
                    LPARAM(1),
                );
            }
        }

        let id = self.alloc_id();
        let layout = self.layout.new_node();
        self.layout_for_id.insert(id, layout);
        self.nodes.insert(
            id,
            NodeMeta {
                hwnd,
                on_click: None,
                control_id,
                is_view: false,
                is_text: false,
            },
        );
        WindowsNode { id, hwnd }
    }

    /// Placeholder node — same shape as a real child but with the
    /// "X not supported" text in a `STATIC` HWND. Routes through
    /// `create_child` so the layout tree picks it up too.
    fn placeholder(&mut self, message: &str) -> WindowsNode {
        self.create_child(class_static(), message, SS_LEFT as u32, None)
    }

    /// Measure `text` in the backend's UI font, returning its
    /// single-line `(width, height)` in pixels. Used to give text
    /// leaves a real intrinsic size so Taffy lays them out at their
    /// content size instead of collapsing to 0×0 (which is why the
    /// scaffold rendered nothing visible). Multi-line wrapping is a
    /// later refinement — this reports the single-line max-content
    /// width, which Taffy treats as the intrinsic width.
    fn measure_text(&self, text: &str) -> (i32, i32) {
        if text.is_empty() {
            return (0, self.line_height);
        }
        let wide: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            // Screen DC (HWND null) is fine for text metrics; they
            // don't depend on a specific window.
            let dc = GetDC(HWND(std::ptr::null_mut()));
            if dc.is_invalid() {
                return (0, self.line_height);
            }
            let prev = SelectObject(dc, HGDIOBJ(self.font.0));
            let mut size = SIZE::default();
            let ok = GetTextExtentPoint32W(dc, &wide, &mut size).as_bool();
            SelectObject(dc, prev);
            ReleaseDC(HWND(std::ptr::null_mut()), dc);
            if ok {
                (size.cx, size.cy)
            } else {
                (0, self.line_height)
            }
        }
    }

    /// Record a leaf's intrinsic pixel size on its layout node.
    fn set_intrinsic(&mut self, node: &WindowsNode, width: i32, height: i32) {
        if let Some(layout) = self.layout_for_id.get(&node.id).copied() {
            self.layout
                .set_intrinsic_size(layout, width as f32, height as f32);
        }
    }

    /// Recursively drop node `id` and every descendant from the
    /// backend's maps + layout tree. `destroy_hwnd` is `true` only for
    /// the direct child passed to `clear_children`: `DestroyWindow`
    /// cascades to descendant HWNDs, so descendants are pruned from the
    /// maps but not individually destroyed. Also retires any
    /// `WM_COMMAND` handler the node owned.
    fn remove_subtree(&mut self, id: u64, destroy_hwnd: bool) {
        if let Some(kids) = self.children.remove(&id) {
            for k in kids {
                self.remove_subtree(k, false);
            }
        }
        if let Some(meta) = self.nodes.remove(&id) {
            if let Some(cid) = meta.control_id {
                self.command_handlers.remove(&cid);
            }
            // Retire any slider handler keyed by this HWND.
            self.slider_handlers.remove(&(meta.hwnd.0 as isize));
            if destroy_hwnd && !meta.hwnd.is_invalid() {
                unsafe {
                    let _ = DestroyWindow(meta.hwnd);
                }
            }
        }
        if let Some(layout) = self.layout_for_id.remove(&id) {
            self.layout.remove_node(layout);
        }
    }

    /// Create an owner-drawn `IdealystView` container window and attach
    /// a fresh [`ViewPaint`] (carrying `on_click` for `Pressable`).
    /// Both `View` and `Pressable` route here; `apply_style` later
    /// fills the paint state.
    fn create_view_window(&mut self, on_click: Option<Rc<dyn Fn()>>) -> WindowsNode {
        ensure_view_class_registered();
        // WS_CLIPCHILDREN so our WM_PAINT of the background never paints
        // over child controls (text, buttons) mounted on top.
        let node = self.create_child(VIEW_CLASS_NAME, "", WS_CLIPCHILDREN.0, None);
        let mut vp = ViewPaint {
            on_click,
            ..ViewPaint::default()
        };
        // Seed the child-text background brush (white until a
        // background color is applied).
        vp.set_bk(WHITE);
        let paint = Box::new(vp);
        unsafe {
            SetWindowLongPtrW(node.hwnd, GWLP_USERDATA, Box::into_raw(paint) as isize);
        }
        if let Some(meta) = self.nodes.get_mut(&node.id) {
            meta.is_view = true;
        }
        node
    }
}

/// Build the UI font (the shell's message font — Segoe UI on modern
/// Windows) and its single-line height. Falls back to a null `HFONT`
/// (controls keep their default font) if the metrics query fails.
fn create_ui_font() -> (HFONT, i32) {
    unsafe {
        let mut ncm = NONCLIENTMETRICSW {
            cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32,
            ..Default::default()
        };
        let ok = SystemParametersInfoW(
            SPI_GETNONCLIENTMETRICS,
            ncm.cbSize,
            Some(&mut ncm as *mut _ as *mut std::ffi::c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .is_ok();
        if !ok {
            return (HFONT(std::ptr::null_mut()), 16);
        }
        let font = CreateFontIndirectW(&ncm.lfMessageFont);
        if font.is_invalid() {
            return (HFONT(std::ptr::null_mut()), 16);
        }
        // Derive the line height by measuring a representative glyph
        // run in this font. Height is font-dependent, not text-
        // dependent, so any non-empty string works.
        let dc = GetDC(HWND(std::ptr::null_mut()));
        let mut line_height = 16;
        if !dc.is_invalid() {
            let prev = SelectObject(dc, HGDIOBJ(font.0));
            let sample: Vec<u16> = "Mg".encode_utf16().collect();
            let mut size = SIZE::default();
            if GetTextExtentPoint32W(dc, &sample, &mut size).as_bool() {
                line_height = size.cy;
            }
            SelectObject(dc, prev);
            ReleaseDC(HWND(std::ptr::null_mut()), dc);
        }
        (font, line_height)
    }
}

impl Drop for WindowsBackend {
    fn drop(&mut self) {
        // DestroyWindow each child we created. The host's HWND is
        // not ours to destroy. Drop the matching command handler
        // entry too — if the host's WndProc fires `dispatch_command`
        // with a now-stale id after the backend has dropped, we
        // simply won't find a handler and the call returns false.
        for (_, meta) in self.nodes.drain() {
            if let Some(cid) = meta.control_id {
                self.command_handlers.remove(&cid);
            }
            if !meta.hwnd.is_invalid() {
                let _ = unsafe { DestroyWindow(meta.hwnd) };
            }
        }
        // Release the UI font we created in `new`.
        if !self.font.is_invalid() {
            let _ = unsafe { DeleteObject(HGDIOBJ(self.font.0)) };
        }
    }
}

// =========================================================================
// Helpers: PCWSTR conversion + class constants
// =========================================================================

/// Owning wrapper around a UTF-16 buffer so the `PCWSTR` reference
/// stays valid for the duration of a Win32 call. `PCWSTR` is a raw
/// pointer; the caller must keep the backing storage alive until
/// after the API returns.
struct PcwstrBuf(Vec<u16>);
impl PcwstrBuf {
    fn as_pcwstr(&self) -> PCWSTR {
        PCWSTR(self.0.as_ptr())
    }
}

fn to_pcwstr(s: &str) -> PcwstrBuf {
    let mut buf: Vec<u16> = s.encode_utf16().collect();
    buf.push(0);
    PcwstrBuf(buf)
}

fn class_button() -> PCWSTR {
    PCWSTR(windows::core::w!("BUTTON").as_ptr())
}

fn class_static() -> PCWSTR {
    PCWSTR(windows::core::w!("STATIC").as_ptr())
}

// =========================================================================
// IdealystScroll — custom WNDCLASS for `Element::ScrollView`
// =========================================================================
//
// Win32 has no first-class scroll-view widget; the canonical pattern
// is to register a custom WNDCLASS and run a WndProc that handles
// `WM_MOUSEWHEEL` (and optionally `WM_VSCROLL` / `WM_HSCROLL` if
// scroll bars are visible). We use the mouse-wheel path here \u{2014}
// modern Windows reports touchpad scroll as `WM_MOUSEWHEEL`, so this
// covers wheel + trackpad scroll without the SCROLLINFO scaffolding
// that scroll bars would need.
//
// Per-window state \u{2014} the `on_scroll` callback plus the live
// scroll offset \u{2014} lives in a `Box<ScrollState>` stored in
// `GWLP_USERDATA`. `WM_NCDESTROY` releases the box; if the host
// quits the process without firing destroy, the leak is bounded to
// the lifetime of the process.

const SCROLL_CLASS_NAME: PCWSTR = PCWSTR(windows::core::w!("IdealystScroll").as_ptr());

/// Per-scroll-view state stashed in the HWND via `GWLP_USERDATA`.
struct ScrollState {
    /// `true` = horizontal scroller; `false` = vertical (the default).
    horizontal: bool,
    /// Current scroll offset in CSS pixels / device-points. Same
    /// unit as web `scrollLeft`/`scrollTop`, iOS `contentOffset`,
    /// Android `getScrollY`/`getScrollX` (after dp conversion).
    offset_x: f32,
    offset_y: f32,
    /// User-supplied `Element::ScrollView::on_scroll`. `None`
    /// means "scroll is observable through input only; author
    /// didn't ask for reactive observation."
    on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
}

/// Convert a `WM_MOUSEWHEEL` `wParam` high word into a signed
/// 16-bit delta. Win32 ships the delta as a positive `i16` for
/// up-scrolls and a negative `i16` for down-scrolls; the standard
/// multiple is `WHEEL_DELTA = 120` per detent.
#[inline]
fn wheel_delta(wparam: WPARAM) -> i32 {
    let hi = ((wparam.0 >> 16) & 0xffff) as u16;
    hi as i16 as i32
}

/// Pixels-per-detent for translating mouse-wheel ticks into scroll
/// movement. `120` Win32 units = one click of a notched wheel; we
/// translate that to 40 px of scroll, matching the OS default of
/// "3 lines per detent" on a typical 13 px line height.
const WHEEL_LINE_PX: f32 = 40.0;

unsafe extern "system" fn scroll_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_MOUSEWHEEL => {
            let user_data = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
            if user_data == 0 {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }
            let state = &mut *(user_data as *mut ScrollState);
            // `WM_MOUSEWHEEL`'s delta is positive when scrolling
            // *up* (away from user); the canonical content scroll
            // direction is the opposite, so we negate.
            let delta = wheel_delta(wparam);
            let scroll_px = -(delta as f32) * (WHEEL_LINE_PX / 120.0);
            let (dx, dy) = if state.horizontal {
                (scroll_px, 0.0)
            } else {
                (0.0, scroll_px)
            };
            state.offset_x = (state.offset_x + dx).max(0.0);
            state.offset_y = (state.offset_y + dy).max(0.0);
            // V1 fires the `on_scroll` callback so reactive author
            // code (the documented use case for this primitive) sees
            // the new offset. Visual scrolling of the container's
            // children \u{2014} `ScrollWindowEx` with
            // `SW_SCROLLCHILDREN`, or per-frame `SetWindowPos` with
            // the offset subtracted \u{2014} lands in a focused
            // follow-up alongside the Taffy-driven `finish()`
            // integration. The callback path is the framework's
            // contract; the visual half is the backend's
            // implementation freedom.
            if let Some(cb) = &state.on_scroll {
                cb(state.offset_x, state.offset_y);
            }
            LRESULT(0)
        }
        WM_NCDESTROY => {
            let user_data = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
            if user_data != 0 {
                drop(Box::from_raw(user_data as *mut ScrollState));
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Register the `IdealystScroll` WNDCLASS exactly once per process.
/// `RegisterClassExW` returns 0 if the class is already registered;
/// we treat that as success. Cursor + hInstance left default;
/// child HWNDs inherit the host's behavior there.
fn ensure_scroll_class_registered() {
    use std::sync::Once;
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| unsafe {
        let wcex = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(scroll_wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: Default::default(),
            hIcon: Default::default(),
            hCursor: Default::default(),
            hbrBackground: HBRUSH::default(),
            lpszMenuName: PCWSTR::null(),
            lpszClassName: SCROLL_CLASS_NAME,
            hIconSm: Default::default(),
        };
        let _ = RegisterClassExW(&wcex);
        // Ignore the return value: a non-zero ATOM means newly
        // registered; zero with `ERROR_CLASS_ALREADY_EXISTS` means
        // a previous call (different host instance in the same
        // process) won the race. Either is fine.
    });
}

// =========================================================================
// IdealystView — owner-drawn custom WNDCLASS for `Element::View` /
// `Element::Pressable`
// =========================================================================
//
// Win32 common controls (STATIC/BUTTON/EDIT/...) can't express the
// framework's box styling — a `View` is an arbitrary styled rectangle
// (background color, border, corner radius). So containers get their
// own registered WNDCLASS whose WndProc paints the styled box in
// `WM_PAINT`. The visual model lives in a `Box<ViewPaint>` stored in
// the window's `GWLP_USERDATA`; `apply_style` updates it and
// invalidates the window. This mirrors the Linux backend's
// `IdealystView` GtkWidget subclass (background / border / radius),
// diverging only in mechanism (GDI paint vs GSK nodes).
//
// v1 scope: solid background fill, a uniform border (top width/color),
// and a uniform corner radius (top-left), all via GDI. Known gaps
// tracked for the Direct2D upgrade: no anti-aliasing on rounded
// corners (GDI `RoundRect` is aliased), no per-corner radius /
// per-side border, no gradient fill, and GDI ignores background alpha
// (semi-transparent fills render opaque). Child controls are NOT
// clipped to the corner radius (HWND clipping is rectangular).

const VIEW_CLASS_NAME: PCWSTR = PCWSTR(windows::core::w!("IdealystView").as_ptr());

/// Opaque white — the fallback child-text background for a view with
/// no explicit background color.
const WHITE: COLORREF = COLORREF(0x00FF_FFFF);

/// Visual paint state for one `IdealystView` window, stored behind its
/// `GWLP_USERDATA`.
struct ViewPaint {
    /// Fill color; `None` = don't paint a background (transparent).
    background: Option<Rgba>,
    /// Uniform border width in px (0 = no border). From
    /// `border_top_width` (per-side widths collapse to this in v1).
    border_width: f32,
    /// Border color (used when `border_width > 0`).
    border_color: Rgba,
    /// Uniform corner radius in px (0 = square). From
    /// `border_top_left_radius`.
    radius: f32,
    /// Pressable click handler. `None` for a plain `View`; `Some` for
    /// `Pressable`. Fired on `WM_LBUTTONUP`.
    on_click: Option<Rc<dyn Fn()>>,
    /// Opaque background color handed to child STATIC (Text) controls
    /// via `WM_CTLCOLORSTATIC` — the view's own background if it has
    /// one, else white. Giving statics an OPAQUE brush (rather than a
    /// hollow one) is what erases stale glyphs when reactive text
    /// updates; a transparent static never clears its old text.
    bk_color: COLORREF,
    /// Cached solid brush for `bk_color`, rebuilt when the background
    /// changes and deleted on drop. Returned from `WM_CTLCOLORSTATIC`.
    bk_brush: Option<HBRUSH>,
}

impl Default for ViewPaint {
    fn default() -> Self {
        ViewPaint {
            background: None,
            border_width: 0.0,
            border_color: Rgba::default(),
            radius: 0.0,
            on_click: None,
            bk_color: WHITE,
            bk_brush: None,
        }
    }
}

impl ViewPaint {
    /// Point the child-text background at `color`, rebuilding the
    /// cached brush. Deletes the previous brush first.
    fn set_bk(&mut self, color: COLORREF) {
        if let Some(old) = self.bk_brush.take() {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(old.0));
            }
        }
        self.bk_color = color;
        self.bk_brush = Some(unsafe { CreateSolidBrush(color) });
    }
}

impl Drop for ViewPaint {
    fn drop(&mut self) {
        if let Some(b) = self.bk_brush.take() {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(b.0));
            }
        }
    }
}

/// Pack an [`Rgba`] into a GDI `COLORREF` (`0x00BBGGRR`). GDI has no
/// alpha channel, so `a` is dropped here (see the alpha limitation in
/// the module header).
#[inline]
fn colorref(c: Rgba) -> COLORREF {
    COLORREF((c.r as u32) | ((c.g as u32) << 8) | ((c.b as u32) << 16))
}

/// Resolve a `StyleRules` color slot to canonical [`Rgba`], defaulting
/// to fully-transparent when unset/unparseable (matches the Linux
/// backend's "silent transparent, not loud magenta" posture — an
/// unset background legitimately means "paint nothing").
fn resolve_color(t: &Option<runtime_core::Tokenized<Color>>) -> Option<Rgba> {
    t.as_ref()
        .map(|c| runtime_core::color::parse_or(&c.resolve().0, Rgba::TRANSPARENT))
}

/// First corner radius (top-left) in px, or 0.
fn resolve_radius(t: &Option<runtime_core::Tokenized<Length>>) -> f32 {
    match t.as_ref().map(|x| x.resolve()) {
        Some(Length::Px(v)) => v,
        _ => 0.0,
    }
}

/// Paint the styled box for `hwnd` from `p`. Called from `WM_PAINT`.
unsafe fn paint_view(hwnd: HWND, p: &ViewPaint) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let w = rc.right - rc.left;
    let h = rc.bottom - rc.top;
    let dia = (p.radius * 2.0).round() as i32;

    if let Some(bg) = p.background {
        if bg.a > 0 {
            let brush = CreateSolidBrush(colorref(bg));
            if p.radius > 0.5 {
                // `+1` because a region's right/bottom edges are
                // exclusive; without it the last row/column stays
                // unpainted.
                let rgn: HRGN = CreateRoundRectRgn(0, 0, w + 1, h + 1, dia, dia);
                let _ = FillRgn(hdc, rgn, brush);
                let _ = DeleteObject(HGDIOBJ(rgn.0));
            } else {
                FillRect(hdc, &rc, brush);
            }
            let _ = DeleteObject(HGDIOBJ(brush.0));
        }
    }

    if p.border_width > 0.5 && p.border_color.a > 0 {
        let pen: HPEN = CreatePen(PS_SOLID, p.border_width.round() as i32, colorref(p.border_color));
        let old_pen = SelectObject(hdc, HGDIOBJ(pen.0));
        // Hollow brush so the border-drawing primitives stroke only.
        let old_brush = SelectObject(hdc, GetStockObject(NULL_BRUSH));
        if p.radius > 0.5 {
            let _ = RoundRect(hdc, 0, 0, w, h, dia, dia);
        } else {
            let _ = Rectangle(hdc, 0, 0, w, h);
        }
        SelectObject(hdc, old_pen);
        SelectObject(hdc, old_brush);
        let _ = DeleteObject(HGDIOBJ(pen.0));
    }

    let _ = EndPaint(hwnd, &ps);
}

unsafe extern "system" fn view_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            let ud = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
            if ud != 0 {
                paint_view(hwnd, &*(ud as *const ViewPaint));
            } else {
                // No paint state yet — still satisfy the paint cycle so
                // Windows stops resending WM_PAINT.
                let mut ps = PAINTSTRUCT::default();
                let _ = BeginPaint(hwnd, &mut ps);
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        // Control notifications (button/checkbox/edit `WM_COMMAND`,
        // trackbar `WM_HSCROLL`/`WM_VSCROLL`) are sent to the control's
        // *immediate* parent — which, once mounted, is this view, not
        // the host window that owns the backend. Forward them to the
        // top-level host window so its WndProc can route them through
        // the backend. `GA_ROOT` walks straight to the top-level owner,
        // skipping any intermediate nested views.
        WM_COMMAND | WM_HSCROLL | WM_VSCROLL => {
            let root = GetAncestor(hwnd, GA_ROOT);
            if !root.is_invalid() && root != hwnd {
                return SendMessageW(root, msg, wparam, lparam);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        // We paint the whole client area in WM_PAINT; suppressing the
        // default erase avoids a flash of the erase brush on resize.
        WM_ERASEBKGND => LRESULT(1),
        // Child STATIC (Text/Image) controls ask their parent for a
        // background brush. Draw their text transparently over our
        // painted box and hand back a hollow brush so the static
        // doesn't fill an opaque rectangle behind the glyphs.
        WM_CTLCOLORSTATIC => {
            let hdc = windows::Win32::Graphics::Gdi::HDC(wparam.0 as *mut std::ffi::c_void);
            // The view (this window) supplies the opaque background its
            // child statics erase to — so updating a Text control's
            // string clears the old glyphs. Read it from our own
            // ViewPaint.
            let view_ud = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
            let (bk_color, brush) = if view_ud != 0 {
                let vp = &*(view_ud as *const ViewPaint);
                (vp.bk_color, vp.bk_brush)
            } else {
                (WHITE, None)
            };
            SetBkColor(hdc, bk_color);
            // lParam is the child STATIC's HWND; if `apply_style` packed
            // a text color into its USERDATA, honor it.
            let child = HWND(lparam.0 as *mut std::ffi::c_void);
            let raw = GetWindowLongPtrW(child, GWLP_USERDATA);
            if raw & TEXT_COLOR_FLAG != 0 {
                SetTextColor(hdc, COLORREF((raw & 0x00FF_FFFF) as u32));
            }
            match brush {
                Some(b) => LRESULT(b.0 as isize),
                None => LRESULT(GetStockObject(NULL_BRUSH).0 as isize),
            }
        }
        WM_LBUTTONUP => {
            // Clone the handler out before firing so no `&ViewPaint`
            // borrow is held across it (the click may synchronously
            // rebuild — and free — this very window). Same discipline
            // as the host's `WM_COMMAND` routing.
            let ud = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
            let cb = if ud != 0 {
                (*(ud as *const ViewPaint)).on_click.clone()
            } else {
                None
            };
            if let Some(cb) = cb {
                cb();
                return LRESULT(0);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_NCDESTROY => {
            let ud = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
            if ud != 0 {
                drop(Box::from_raw(ud as *mut ViewPaint));
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Register the `IdealystView` WNDCLASS once per process.
fn ensure_view_class_registered() {
    use std::sync::Once;
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| unsafe {
        let wcex = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(view_wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: Default::default(),
            hIcon: Default::default(),
            hCursor: Default::default(),
            // No class brush — the WndProc paints the client area.
            hbrBackground: HBRUSH::default(),
            lpszMenuName: PCWSTR::null(),
            lpszClassName: VIEW_CLASS_NAME,
            hIconSm: Default::default(),
        };
        let _ = RegisterClassExW(&wcex);
    });
}

// =========================================================================
// Backend trait
// =========================================================================

impl Backend for WindowsBackend {
    type Node = WindowsNode;

    fn color_scheme(&self) -> ColorScheme {
        // Win32 doesn't expose a single "dark mode" toggle until very
        // recent builds (UISettings::Background via WinRT). For the
        // scaffold we return Auto and let the framework's theme APIs
        // own the decision. A future revision can read
        // `AppsUseLightTheme` from the registry under
        // `Software\Microsoft\Windows\CurrentVersion\Themes\Personalize`.
        ColorScheme::Auto
    }

    fn platform(&self) -> Platform {
        Platform::Custom("windows")
    }

    fn create_view(&mut self, _a11y: &AccessibilityProps) -> Self::Node {
        // Owner-drawn `IdealystView` container so `apply_style` can
        // paint the box (background / border / radius). No click
        // handler — a plain View.
        self.create_view_window(None)
    }

    fn create_text(&mut self, content: &str, _a11y: &AccessibilityProps) -> Self::Node {
        let node = self.create_child(class_static(), content, SS_LEFT as u32, None);
        let (w, h) = self.measure_text(content);
        self.set_intrinsic(&node, w, h);
        if let Some(meta) = self.nodes.get_mut(&node.id) {
            meta.is_text = true;
        }
        node
    }

    fn create_button(
        &mut self,
        label: &str,
        on_click: &Action,
        _leading_icon: Option<&runtime_core::primitives::icon::IconData>,
        _trailing_icon: Option<&runtime_core::primitives::icon::IconData>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Allocate a control id, install the handler in the
        // dispatch table, and pass the id to `create_child` so
        // CreateWindowExW records it on the HWND. The host's
        // WndProc routes `WM_COMMAND` with `LOWORD(wParam)` ==
        // this id back through `dispatch_command`.
        let control_id = self.alloc_control_id();
        let handler = on_click.fire.clone();
        self.command_handlers.insert(
            control_id,
            CommandEntry { code: BN_CLICKED, action: handler.clone() },
        );
        let node = self.create_child(
            class_button(),
            label,
            BS_DEFPUSHBUTTON as u32,
            Some(control_id),
        );
        if let Some(meta) = self.nodes.get_mut(&node.id) {
            meta.on_click = Some(handler);
        }
        // Intrinsic size = label metrics + push-button chrome padding
        // (~12px each side, ~8px top/bottom) so an unstyled button is
        // large enough to show its label. Explicit width/height in the
        // author's style still override via `set_style`'s `size`.
        let (tw, th) = self.measure_text(label);
        self.set_intrinsic(&node, tw + 24, th + 8);
        node
    }

    fn create_pressable(
        &mut self,
        on_click: Rc<dyn Fn()>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // A Pressable is a styleable container that reports a press —
        // exactly an `IdealystView` carrying an `on_click`. Its
        // `view_wnd_proc` fires the handler on `WM_LBUTTONUP` (cloning
        // the Rc out before invoking, so a press that rebuilds the
        // tree can't free the window under the borrow). This replaces
        // the old STATIC + `SS_NOTIFY` hack, which couldn't paint a
        // background and relied on the host's `WM_COMMAND` routing.
        self.create_view_window(Some(on_click))
    }

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        let Some(parent_layout) = self.layout_for_id.get(&parent.id).copied() else {
            return;
        };
        let Some(child_layout) = self.layout_for_id.get(&child.id).copied() else {
            return;
        };
        self.layout.add_child(parent_layout, child_layout);
        // Track the framework-logical parent→child edge so
        // `clear_children` can tear the subtree down later.
        self.children.entry(parent.id).or_default().push(child.id);
        // SetParent: re-parent the HWND so the host's WM_PAINT
        // walks reach this node. Without it, the framework's
        // logical parent/child differs from Win32's HWND tree.
        unsafe {
            // windows 0.58 dropped the `Option<HWND>` parent param;
            // it's now `Param<HWND>`. Pass the bare HWND.
            let _ = windows::Win32::UI::WindowsAndMessaging::SetParent(
                child.hwnd,
                parent.hwnd,
            );
        }
    }

    fn clear_children(&mut self, node: &Self::Node) {
        // Remove every logical child of `node` (and their descendants):
        // destroy the HWND, drop layout + node metadata, and detach the
        // layout-tree edge. Without this, a reactive rebuild (`when` /
        // `for` swap, component unmount) would leak controls and paint
        // stale ones over the new tree. `DestroyWindow` on a parent
        // recursively destroys its child HWNDs, so we only call it on
        // the direct children and prune the whole subtree from our maps.
        let Some(child_ids) = self.children.remove(&node.id) else {
            return;
        };
        for child_id in child_ids {
            self.remove_subtree(child_id, true);
        }
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        let wide = to_pcwstr(content);
        unsafe {
            let _ = SetWindowTextW(node.hwnd, wide.as_pcwstr());
            // Force an erasing repaint (TRUE) so the parent view's
            // opaque brush clears the previous string before the new
            // one draws — otherwise reactive updates ghost old glyphs.
            let _ = InvalidateRect(node.hwnd, None, true);
        }
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        let wide = to_pcwstr(label);
        let _ = unsafe { SetWindowTextW(node.hwnd, wide.as_pcwstr()) };
    }

    fn finish(&mut self, root: Self::Node) {
        // Run Taffy against the host window's client rect, then
        // walk every registered HWND and call SetWindowPos with
        // its computed frame. Frames in Taffy are relative to the
        // immediate parent; SetWindowPos takes coordinates
        // relative to the parent HWND, and our `insert` reparents
        // each child to its framework parent via `SetParent`, so
        // the two coordinate systems line up directly.
        // Remember the root so a bare window resize can re-run this
        // pass via `relayout()` without the walker rebuilding.
        self.root_node = Some(root.clone());

        let Some(root_layout) = self.layout_for_id.get(&root.id).copied() else {
            return;
        };

        // Host client rect in pixels. GetClientRect can fail if the
        // window has been destroyed; bail rather than feed garbage
        // dimensions to the layout pass.
        let mut rect = RECT::default();
        if unsafe { GetClientRect(self.host_hwnd, &mut rect) }.is_err() {
            return;
        }
        let width = (rect.right - rect.left).max(0) as f32;
        let height = (rect.bottom - rect.top).max(0) as f32;
        if width <= 0.0 || height <= 0.0 {
            return;
        }

        self.layout.compute(root_layout, width, height);

        // Collect (hwnd, frame) pairs first so we can release the
        // borrow on `self.nodes` before issuing the Win32 calls.
        // SetWindowPos is documented as safe to call from the
        // owning thread; we issue them serially so the HWND tree
        // doesn't see partial-state intermediate frames.
        let mut updates: Vec<(HWND, i32, i32, i32, i32)> =
            Vec::with_capacity(self.nodes.len());
        for (id, meta) in &self.nodes {
            let Some(layout) = self.layout_for_id.get(id).copied() else {
                continue;
            };
            let frame = self.layout.frame_of(layout);
            updates.push((
                meta.hwnd,
                frame.x.round() as i32,
                frame.y.round() as i32,
                frame.width.round() as i32,
                frame.height.round() as i32,
            ));
        }
        for (hwnd, x, y, w, h) in updates {
            if hwnd.is_invalid() {
                continue;
            }
            // `HWND_TOP` here would force every child to the top of
            // the z-order on every layout pass — wasteful and
            // visually disruptive. `SWP_NOZORDER` preserves whatever
            // z-order the HWND already has. `SWP_NOACTIVATE` keeps
            // input focus from jumping to whatever child we
            // happen to move first.
            let _ = unsafe {
                SetWindowPos(
                    hwnd,
                    None,
                    x,
                    y,
                    w,
                    h,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                )
            };
        }
    }

    // ---------------------------------------------------------------------
    // Placeholders. See `backend-cpu`'s analogous block — the same
    // posture applies: visible "X not supported on Windows" text
    // beats a silent unimplemented panic.
    // ---------------------------------------------------------------------

    fn create_image(
        &mut self,
        _src: &str,
        _alt: Option<&str>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.placeholder("Image not yet implemented on Windows backend")
    }

    fn create_icon(
        &mut self,
        _data: &runtime_core::primitives::icon::IconData,
        _color: Option<&Color>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.placeholder("Icon not yet implemented on Windows backend")
    }

    fn create_text_input(
        &mut self,
        initial_value: &str,
        _placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
        _on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        _on_blur: Option<runtime_core::primitives::text_input::BlurHandler>,
        secure: bool,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Real single-line `EDIT`. `WS_BORDER` gives the visible box;
        // `ES_AUTOHSCROLL` lets text scroll past the right edge;
        // `ES_PASSWORD` masks input for secure fields.
        let control_id = self.alloc_control_id();
        let mut style = ES_AUTOHSCROLL | WS_BORDER.0;
        if secure {
            style |= ES_PASSWORD;
        }
        let node = self.create_child(class_edit(), initial_value, style, Some(control_id));
        // `EN_CHANGE` fires on every content change; the handler reads
        // the live text and reports it up. Reading in the closure (vs.
        // capturing a value) keeps it correct across IME / paste / undo.
        let hwnd = node.hwnd;
        let action: Rc<dyn Fn()> = Rc::new(move || on_change(window_text(hwnd)));
        self.command_handlers
            .insert(control_id, CommandEntry { code: EN_CHANGE, action });
        // Intrinsic: a reasonable default field size; explicit width in
        // the author's style overrides via `set_style`.
        self.set_intrinsic(&node, 160, self.line_height + 8);
        node
    }

    fn create_text_area(
        &mut self,
        initial_value: &str,
        _placeholder: Option<&str>,
        _wrap: bool,
        _min_rows: Option<u32>,
        _max_rows: Option<u32>,
        _on_change: Rc<dyn Fn(String)>,
        _on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.create_child(class_static(), initial_value, SS_LEFT as u32, None)
    }

    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // A `BUTTON` with `BS_AUTOCHECKBOX` maintains its own checked
        // state on click, then fires `BN_CLICKED`. (Win32 has no
        // switch control; a checkbox is the native two-state toggle —
        // the observable behavior matches the framework's Toggle.)
        let control_id = self.alloc_control_id();
        let node = self.create_child(class_button(), "", BS_AUTOCHECKBOX, Some(control_id));
        unsafe {
            SendMessageW(
                node.hwnd,
                BM_SETCHECK,
                WPARAM(if initial_value { BST_CHECKED as usize } else { 0 }),
                LPARAM(0),
            );
        }
        // On click the control's state has already flipped; read it and
        // report the new boolean.
        let hwnd = node.hwnd;
        let action: Rc<dyn Fn()> = Rc::new(move || {
            let checked =
                unsafe { SendMessageW(hwnd, BM_GETCHECK, WPARAM(0), LPARAM(0)) }.0 == BST_CHECKED;
            on_change(checked);
        });
        self.command_handlers
            .insert(control_id, CommandEntry { code: BN_CLICKED, action });
        // The check glyph is ~13px; give it a small square footprint.
        self.set_intrinsic(&node, 18, 18);
        node
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        unsafe {
            SendMessageW(
                node.hwnd,
                BM_SETCHECK,
                WPARAM(if value { BST_CHECKED as usize } else { 0 }),
                LPARAM(0),
            );
        }
    }

    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        _step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        ensure_common_controls();
        let node = self.create_child(class_trackbar(), "", TBS_HORZ, None);
        // Trackbars use integer positions; map [0, RESOLUTION] onto the
        // author's [min, max]. TBM_SETRANGE takes MAKELONG(min, max) in
        // lParam; wParam = TRUE redraws.
        unsafe {
            let range = (SLIDER_RESOLUTION as isize) << 16; // MAKELONG(0, RESOLUTION)
            SendMessageW(node.hwnd, TBM_SETRANGE, WPARAM(1), LPARAM(range));
            let pos = value_to_slider_pos(initial_value, min, max);
            SendMessageW(node.hwnd, TBM_SETPOS, WPARAM(1), LPARAM(pos as isize));
        }
        // On drag, read the integer position back and map to the float
        // range before reporting up.
        let hwnd = node.hwnd;
        let action: Rc<dyn Fn()> = Rc::new(move || {
            let pos = unsafe { SendMessageW(hwnd, TBM_GETPOS, WPARAM(0), LPARAM(0)) }.0 as i32;
            let value = min + (pos as f32 / SLIDER_RESOLUTION as f32) * (max - min);
            on_change(value);
        });
        self.slider_handlers.insert(hwnd.0 as isize, action);
        // A sensible default track size; explicit style overrides.
        self.set_intrinsic(&node, 160, 24);
        node
    }

    fn create_scroll_view(
        &mut self,
        horizontal: bool,
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Register the `IdealystScroll` WNDCLASS on first use; the
        // call is idempotent across multiple backend instances in
        // the same process.
        ensure_scroll_class_registered();
        let node = self.create_child(SCROLL_CLASS_NAME, "", 0, None);

        // Stash per-window scroll state (callback + offsets) in
        // `GWLP_USERDATA`. The WndProc reads it on every
        // `WM_MOUSEWHEEL`, advances the offset, calls
        // `ScrollWindowEx` to move children, and fires the user
        // callback. `WM_NCDESTROY` releases the box.
        let state = Box::new(ScrollState {
            horizontal,
            offset_x: 0.0,
            offset_y: 0.0,
            on_scroll,
        });
        let raw = Box::into_raw(state) as isize;
        unsafe {
            SetWindowLongPtrW(node.hwnd, GWLP_USERDATA, raw);
        }
        node
    }

    fn create_activity_indicator(
        &mut self,
        _size: runtime_core::primitives::activity_indicator::ActivityIndicatorSize,
        _color: Option<&Color>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Win32 has no spinner primitive; a marquee progress bar is the
        // native indeterminate-activity affordance. `PBM_SETMARQUEE`
        // with wParam=TRUE starts the animation (lParam = update period
        // in ms). Comctl32 animates it on its own timer — no per-frame
        // work from us. (Color/size mapping is a later refinement.)
        ensure_common_controls();
        let node = self.create_child(class_progress(), "", PBS_MARQUEE, None);
        unsafe {
            SendMessageW(node.hwnd, PBM_SETMARQUEE, WPARAM(1), LPARAM(30));
        }
        self.set_intrinsic(&node, 160, 16);
        node
    }

    fn create_virtualizer(
        &mut self,
        _callbacks: runtime_core::VirtualizerCallbacks<Self::Node>,
        _overscan: f32,
        _layout: runtime_core::VirtualLayout,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.placeholder("Virtualizer not yet implemented on Windows backend")
    }

    fn create_graphics(
        &mut self,
        _on_ready: runtime_core::primitives::graphics::OnReady,
        _on_resize: runtime_core::primitives::graphics::OnResize,
        _on_lost: runtime_core::primitives::graphics::OnLost,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.placeholder("Graphics not yet implemented on Windows backend")
    }

    fn create_external(
        &mut self,
        type_id: std::any::TypeId,
        type_name: &'static str,
        payload: &Rc<dyn std::any::Any>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Look up the registered handler for `type_id`. If found,
        // invoke it with the typed payload + `&mut self`; if not,
        // fall through to the labeled placeholder so the missing
        // SDK is visible at runtime (matches the iOS / macOS posture
        // for unregistered externals).
        //
        // Clone the registry slot out before mutably borrowing
        // `self` for the handler call — `ExternalRegistry` stores
        // its handlers as `Rc<dyn ErasedHandler<_>>`, so the clone
        // is cheap and breaks the borrow conflict.
        if let Some(handler) = self.external_handlers.get(type_id) {
            return handler(payload, self);
        }
        self.placeholder(&format!(
            "External \"{type_name}\" not registered on Windows backend"
        ))
    }

    fn create_portal(
        &mut self,
        _target: runtime_core::primitives::portal::PortalTarget,
        _on_dismiss: Option<Rc<dyn Fn()>>,
        _trap_focus: bool,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.placeholder("Portal not yet implemented on Windows backend")
    }

    fn create_navigator(
        &mut self,
        _type_id: std::any::TypeId,
        type_name: &'static str,
        _presentation: Rc<dyn std::any::Any>,
        _host: runtime_core::primitives::navigator::NavigatorHost<Self::Node>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.placeholder(&format!(
            "Navigator \"{type_name}\" not yet implemented on Windows backend"
        ))
    }

    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        // Translate the author's StyleRules into the node's Taffy
        // style so width / height / flex / padding / margin / gap /
        // alignment drive the layout pass in `finish`. `set_style` is
        // the shared StyleRules→Taffy translator in `runtime_layout`,
        // so every backend gets identical box layout for free.
        //
        if let Some(layout) = self.layout_for_id.get(&node.id).copied() {
            self.layout.set_style(layout, style);
        }

        // Visual style: only `IdealystView` windows carry a ViewPaint.
        // Push background / border / radius into it and repaint. (Text
        // color, gradient, per-corner radius, per-side border, and
        // anti-aliased corners are later refinements — see the
        // IdealystView module header.)
        let is_view = self.nodes.get(&node.id).map(|m| m.is_view).unwrap_or(false);
        if is_view {
            let ud = unsafe { GetWindowLongPtrW(node.hwnd, GWLP_USERDATA) };
            if ud != 0 {
                // SAFETY: for an `is_view` node the USERDATA slot holds
                // the `Box<ViewPaint>` raw pointer set in
                // `create_view_window`; it lives until the window's
                // `WM_NCDESTROY`. We hold no other alias here.
                let paint = unsafe { &mut *(ud as *mut ViewPaint) };
                paint.background = resolve_color(&style.background);
                paint.border_width =
                    style.border_top_width.as_ref().map(|t| t.resolve()).unwrap_or(0.0);
                paint.border_color =
                    resolve_color(&style.border_top_color).unwrap_or(Rgba::BLACK);
                paint.radius = resolve_radius(&style.border_top_left_radius);
                // Child text erases to the view's background (or white
                // if none/transparent) so reactive text updates don't
                // leave ghosts.
                let bk = match paint.background {
                    Some(c) if c.a > 0 => colorref(c),
                    _ => WHITE,
                };
                paint.set_bk(bk);
                unsafe {
                    let _ = InvalidateRect(node.hwnd, None, true);
                }
            }
        }

        // Text color: pack the resolved color into the STATIC's own
        // USERDATA so its parent view's WM_CTLCOLORSTATIC can apply it
        // (STATIC controls take their text color from the parent, not
        // a per-control setter). Gated on `is_text` so we never stomp
        // the ViewPaint/ScrollState pointers other node kinds keep in
        // USERDATA.
        let is_text = self.nodes.get(&node.id).map(|m| m.is_text).unwrap_or(false);
        if is_text {
            if let Some(rgba) = resolve_color(&style.color) {
                let packed = colorref(rgba).0 as isize | TEXT_COLOR_FLAG;
                unsafe {
                    SetWindowLongPtrW(node.hwnd, GWLP_USERDATA, packed);
                    let _ = InvalidateRect(node.hwnd, None, true);
                }
            }
        }
    }
}

// `RefCell` import kept for the eventual wm_command_dispatch state.
#[allow(dead_code)]
type _KeepRefCell = RefCell<()>;
