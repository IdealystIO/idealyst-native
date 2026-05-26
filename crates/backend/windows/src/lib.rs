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
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GetClientRect, SetWindowPos, SetWindowTextW,
    ShowWindow, BS_DEFPUSHBUTTON, SS_LEFT, SWP_NOACTIVATE, SWP_NOZORDER, SW_SHOW,
    WINDOW_EX_STYLE, WS_CHILD, WS_VISIBLE,
};
use windows::Win32::Foundation::RECT;

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
        }
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
            log::warn!(
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
        // hMenu carries the control id for WS_CHILD windows. The
        // `windows` crate types it as `Option<HMENU>`; HMENU is a
        // `HANDLE` newtype wrapping a pointer. Casting a u16
        // through `usize` to a pointer gives Win32 the value it
        // wants in the low word.
        let hmenu = control_id.map(|cid| {
            windows::Win32::UI::WindowsAndMessaging::HMENU(
                cid as usize as *mut std::ffi::c_void,
            )
        });
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
                Some(self.host_hwnd),
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
        self.create_child(class_static(), message, SS_LEFT.0 as u32, None)
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
        self.create_child(class_static(), "", SS_LEFT.0 as u32, None)
    }

    fn create_text(&mut self, content: &str, _a11y: &AccessibilityProps) -> Self::Node {
        self.create_child(class_static(), content, SS_LEFT.0 as u32, None)
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
            BS_DEFPUSHBUTTON.0 as u32,
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
            (SS_LEFT.0
                | windows::Win32::UI::WindowsAndMessaging::SS_NOTIFY.0)
                as u32,
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
            let _ = windows::Win32::UI::WindowsAndMessaging::SetParent(
                child.hwnd,
                Some(parent.hwnd),
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
        self.create_child(class_static(), initial_value, SS_LEFT.0 as u32, None)
    }

    fn create_text_area(
        &mut self,
        initial_value: &str,
        _placeholder: Option<&str>,
        _on_change: Rc<dyn Fn(String)>,
        _on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.create_child(class_static(), initial_value, SS_LEFT.0 as u32, None)
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
        _horizontal: bool,
        _on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // STATIC container — no clipping or scroll affordance yet.
        // Real impl needs a custom WNDCLASS with WM_VSCROLL handling.
        self.create_child(class_static(), "", SS_LEFT.0 as u32, None)
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
        _type_id: std::any::TypeId,
        type_name: &'static str,
        _payload: &Rc<dyn std::any::Any>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.placeholder(&format!(
            "External \"{type_name}\" not yet implemented on Windows backend"
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
