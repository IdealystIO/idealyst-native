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
use runtime_core::{
    Action, Backend, Color, ColorScheme, Platform, StyleRules,
};
use runtime_layout::LayoutTree;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetWindowLongPtrW,
    RegisterClassExW, SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow,
    BS_DEFPUSHBUTTON, CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA, HMENU, SWP_NOACTIVATE,
    SWP_NOZORDER, SW_SHOW, WINDOW_EX_STYLE, WM_MOUSEWHEEL, WM_NCDESTROY, WNDCLASSEXW,
    WS_CHILD, WS_VISIBLE,
};
use windows::Win32::Foundation::RECT;

// STATIC control style constants. The `windows` crate dropped these
// from its `WindowsAndMessaging` re-exports somewhere between 0.5x and
// 0.58 (they're not part of the metadata Microsoft ships anymore;
// only the BS_* button-style family survived). Per Win32 headers
// (winuser.h): SS_LEFT = 0x00000000, SS_NOTIFY = 0x00000100. These
// are plain bit-flags, not WINDOW_STYLE struct constants, so they
// don't need `.0` field access — just `|` them into the style u32.
const SS_LEFT: i32 = 0x0000_0000;
const SS_NOTIFY: i32 = 0x0000_0100;

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
    command_handlers: HashMap<u16, Rc<dyn Fn()>>,
    /// Third-party `Primitive::External` registry. Populated by
    /// `register_external::<T>(...)` calls from per-platform leaf
    /// crates (e.g. `toolbar::register_windows`). `create_external`
    /// looks up the handler by payload TypeId; unregistered kinds
    /// fall through to a "not supported" placeholder. Mirrors the
    /// iOS / macOS pattern.
    pub(crate) external_handlers: runtime_core::ExternalRegistry<WindowsBackend>,
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
}

impl WindowsBackend {
    /// Construct a backend rooted at `host_hwnd`. The host shell
    /// owns the parent window; the backend creates all child controls
    /// underneath. Drop `WindowsBackend` to release every child HWND
    /// the backend has created.
    pub fn new(host_hwnd: HWND) -> Self {
        Self {
            host_hwnd,
            next_id: 1,
            nodes: HashMap::new(),
            layout: LayoutTree::new(),
            layout_for_id: HashMap::new(),
            next_control_id: 100,
            command_handlers: HashMap::new(),
            external_handlers: runtime_core::ExternalRegistry::new(),
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
        self.command_handlers.insert(id, on_click);
        id
    }

    /// Install a `WM_COMMAND` handler under a caller-supplied id.
    /// Used by SDK leaves (`toolbar::flush_pending`) that allocate
    /// ids out of their own namespace — e.g. the toolbar SDK uses
    /// 40000+ to avoid colliding with the backend's own 100+ pool
    /// for buttons. Overwrites any existing handler at that id.
    pub fn install_command_handler_with_id(&mut self, id: u16, on_click: Rc<dyn Fn()>) {
        self.command_handlers.insert(id, on_click);
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
        if let Some(handler) = self.command_handlers.get(&control_id) {
            (handler)();
            return true;
        }
        false
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

        let id = self.alloc_id();
        let layout = self.layout.new_node();
        self.layout_for_id.insert(id, layout);
        self.nodes.insert(
            id,
            NodeMeta { hwnd, on_click: None, control_id },
        );
        WindowsNode { id, hwnd }
    }

    /// Placeholder node — same shape as a real child but with the
    /// "X not supported" text in a `STATIC` HWND. Routes through
    /// `create_child` so the layout tree picks it up too.
    fn placeholder(&mut self, message: &str) -> WindowsNode {
        self.create_child(class_static(), message, SS_LEFT as u32, None)
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
// IdealystScroll — custom WNDCLASS for `Primitive::ScrollView`
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
    /// User-supplied `Primitive::ScrollView::on_scroll`. `None`
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
        // STATIC class with no text — acts as a transparent
        // container. Real Win32 apps typically use a custom WNDCLASS
        // for layout containers; STATIC is fine as a scaffold.
        self.create_child(class_static(), "", SS_LEFT as u32, None)
    }

    fn create_text(&mut self, content: &str, _a11y: &AccessibilityProps) -> Self::Node {
        self.create_child(class_static(), content, SS_LEFT as u32, None)
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
        self.command_handlers.insert(control_id, handler.clone());
        let node = self.create_child(
            class_button(),
            label,
            BS_DEFPUSHBUTTON as u32,
            Some(control_id),
        );
        if let Some(meta) = self.nodes.get_mut(&node.id) {
            meta.on_click = Some(handler);
        }
        node
    }

    fn create_pressable(
        &mut self,
        on_click: Rc<dyn Fn()>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // STATIC controls don't fire WM_COMMAND natively, so the
        // control-id approach doesn't help for Pressable as it
        // does for BUTTON. A proper Pressable needs an `STN_*`
        // notification (via `SS_NOTIFY` style + `WM_COMMAND`
        // with `STN_CLICKED`) or a `WM_LBUTTONDOWN`-subclassed
        // wndproc on the static. For the scaffold we allocate a
        // control id and install `SS_NOTIFY` so the host's
        // dispatcher path treats it like Button; the actual
        // subclassing is a follow-up the host owns.
        let control_id = self.alloc_control_id();
        self.command_handlers.insert(control_id, on_click.clone());
        let node = self.create_child(
            class_static(),
            "",
            (SS_LEFT | SS_NOTIFY) as u32,
            Some(control_id),
        );
        if let Some(meta) = self.nodes.get_mut(&node.id) {
            meta.on_click = Some(on_click);
        }
        node
    }

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        let Some(parent_layout) = self.layout_for_id.get(&parent.id).copied() else {
            return;
        };
        let Some(child_layout) = self.layout_for_id.get(&child.id).copied() else {
            return;
        };
        self.layout.add_child(parent_layout, child_layout);
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

    fn clear_children(&mut self, _node: &Self::Node) {
        // Placeholder: walk children HWNDs and DestroyWindow each.
        // The full implementation needs a parent → children map so
        // we can iterate efficiently. Skipped here so author code
        // doesn't panic on a clear pass.
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        let wide = to_pcwstr(content);
        let _ = unsafe { SetWindowTextW(node.hwnd, wide.as_pcwstr()) };
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
        _on_change: Rc<dyn Fn(String)>,
        _on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Real Win32 EDIT control would land here. For the scaffold,
        // use a STATIC with the initial value so the field is at
        // least visible.
        self.create_child(class_static(), initial_value, SS_LEFT as u32, None)
    }

    fn create_text_area(
        &mut self,
        initial_value: &str,
        _placeholder: Option<&str>,
        _on_change: Rc<dyn Fn(String)>,
        _on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.create_child(class_static(), initial_value, SS_LEFT as u32, None)
    }

    fn create_toggle(
        &mut self,
        _initial_value: bool,
        _on_change: Rc<dyn Fn(bool)>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.placeholder("Toggle not yet implemented on Windows backend")
    }

    fn create_slider(
        &mut self,
        _initial_value: f32,
        _min: f32,
        _max: f32,
        _step: Option<f32>,
        _on_change: Rc<dyn Fn(f32)>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.placeholder("Slider not yet implemented on Windows backend")
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
        self.placeholder("ActivityIndicator not yet implemented on Windows backend")
    }

    fn create_virtualizer(
        &mut self,
        _callbacks: runtime_core::VirtualizerCallbacks<Self::Node>,
        _overscan: f32,
        _horizontal: bool,
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

    fn apply_style(&mut self, _node: &Self::Node, _style: &Rc<StyleRules>) {
        // No-op until we wire Taffy-driven SetWindowPos in finish().
        // Author code calling apply_style today shouldn't crash; the
        // style is silently dropped.
    }
}

// `RefCell` import kept for the eventual wm_command_dispatch state.
#[allow(dead_code)]
type _KeepRefCell = RefCell<()>;
