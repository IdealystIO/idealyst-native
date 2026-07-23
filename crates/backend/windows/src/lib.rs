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
use runtime_core::assets::{
    AssetId, AssetSource, AssetTag, SystemFallback, TypefaceFace, TypefaceId,
};
use runtime_core::{
    Action, Backend, Color, ColorScheme, Gradient, GradientKind, Length, Platform, RadialExtent,
    StyleRules,
};
use runtime_layout::LayoutTree;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontIndirectW, CreateSolidBrush, DeleteObject, EndPaint, FillRect, GetDC,
    GetStockObject, GetTextExtentPoint32W, InvalidateRect, ReleaseDC, SelectObject, SetBkColor,
    SetTextColor, HBRUSH, HFONT, HGDIOBJ, NULL_BRUSH, PAINTSTRUCT, WHITE_BRUSH,
};
use windows::Win32::Graphics::GdiPlus::{
    GdipAddPathArc, GdipAddPathEllipse, GdipAddPathRectangle, GdipClosePathFigure,
    GdipCreateFromHDC, GdipCreateLineBrush, GdipCreatePath, GdipCreatePathGradientFromPath,
    GdipCreatePen1, GdipCreateSolidFill, GdipDeleteBrush, GdipDeleteGraphics, GdipDeletePath,
    GdipDeletePen, GdipDrawPath, GdipFillPath, GdipSetLinePresetBlend,
    GdipSetPathGradientCenterColor, GdipSetPathGradientCenterPoint, GdipSetPathGradientPresetBlend,
    GdipSetPathGradientSurroundColorsWithCount, GdipSetPathGradientWrapMode, GdipSetSmoothingMode,
    GdiplusStartup, GdiplusStartupInput, GdiplusStartupOutput, FillModeWinding, GpBrush, GpGraphics,
    GpLineGradient, GpPath, GpPathGradient, GpPen, GpSolidFill, PointF, SmoothingModeAntiAlias,
    UnitPixel, WrapModeClamp, WrapModeTile,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetAncestor, GetClassNameW, GetClientRect,
    GetParent, GetWindowLongPtrW,
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

// Vector-icon rendering (SVG path parser + GDI+ emitter) and bitmap image
// rendering (GDI+ decode + object-fit) live in their own modules to keep
// this file focused on the Backend trait surface.
mod font;
mod icon;
mod image;

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
    /// Registered image/font assets keyed by `AssetId`. Populated by
    /// [`Backend::register_asset`] (the walker calls it before
    /// `create_image` for an `image_asset`), consumed when resolving an
    /// `asset://{id}` src to a loadable file path or embedded bytes.
    assets: HashMap<u64, AssetEntry>,
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
    /// Em size (px) of the shell message font — the fallback when a text
    /// style sets a weight/family but no explicit `font_size`.
    base_font_size: i32,
    /// Family name of the shell message font (Segoe UI on modern
    /// Windows), used when a style sets no `font_family`.
    base_font_family: String,
    /// `HFONT` cache keyed by resolved (family, size, weight, italic).
    /// Text nodes re-apply their style on every reactive pass; creating a
    /// font per apply would exhaust the process GDI handle quota.
    font_cache: font::FontCache,
    /// `TypefaceId`s already installed into the process font table, so a
    /// repeated `register_typeface` doesn't re-add the same faces.
    installed_typefaces: std::collections::HashSet<u64>,
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

/// A registered asset resolved to the form the image loader needs.
/// `Remote` records that the asset is a runtime URL the native backend
/// can't fetch — resolution yields no bitmap (blank image), the same
/// posture as a bare `http(s)://` src.
enum AssetEntry {
    /// Build-tool-resolved local file path (`AssetSource::Bundled`).
    File(String),
    /// Compile-time-embedded bytes + extension (`AssetSource::Embedded`
    /// or the byte half of `BundledEmbedded`).
    Bytes { bytes: &'static [u8], ext: String },
    /// Runtime URL — unsupported by the native image loader.
    Remote,
}

/// Read the background color of `hwnd`'s parent `IdealystView`, so an icon
/// / image child can erase to it and read as transparent over a
/// solid-colored card. Returns opaque white when the parent isn't an
/// `IdealystView` (e.g. the top-level host during initial mount) or
/// carries no paint state yet. The class-name check gates the
/// `GWLP_USERDATA`-as-`ViewPaint` cast so we never misread another window
/// kind's USERDATA.
pub(crate) unsafe fn parent_view_bk_color(hwnd: HWND) -> COLORREF {
    // windows 0.58: `GetParent` returns `Result<HWND>` (Err / null when
    // the window has no parent).
    let parent = match GetParent(hwnd) {
        Ok(p) if !p.is_invalid() => p,
        _ => return WHITE,
    };
    let mut buf = [0u16; 32];
    let n = GetClassNameW(parent, &mut buf);
    if n <= 0 {
        return WHITE;
    }
    let cls = String::from_utf16_lossy(&buf[..n as usize]);
    if cls != "IdealystView" {
        return WHITE;
    }
    let ud = GetWindowLongPtrW(parent, GWLP_USERDATA);
    if ud == 0 {
        return WHITE;
    }
    // SAFETY: an `IdealystView` window always stores a `Box<ViewPaint>`
    // raw pointer in its USERDATA (set in `create_view_window`), live
    // until its `WM_NCDESTROY`.
    (*(ud as *const ViewPaint)).bk_color
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
    /// `true` for an `IdealystIcon` window whose `GWLP_USERDATA` holds a
    /// `Box<icon::IconPaint>`. `update_icon_color`/`update_icon_data`
    /// reach it by HWND; the flag isn't strictly required for dispatch
    /// but documents the node kind alongside the others.
    #[allow(dead_code)]
    is_icon: bool,
    /// `true` for an `IdealystImage` window whose `GWLP_USERDATA` holds a
    /// `Box<image::ImagePaint>`. `apply_style` reads this to push
    /// `object_fit` into the paint state.
    is_image: bool,
    /// Current string of a Text / Button node. Kept so a later font
    /// change (`apply_style`) can re-measure the intrinsic size without
    /// reading it back out of the HWND.
    text: String,
    /// The `HFONT` currently set on this control (`WM_SETFONT`). Text is
    /// measured with THIS font, not the shell default — otherwise a
    /// 56 px headline lays out at ~12 px and clips.
    font: HFONT,
}

impl WindowsBackend {
    /// Construct a backend rooted at `host_hwnd`. The host shell
    /// owns the parent window; the backend creates all child controls
    /// underneath. Drop `WindowsBackend` to release every child HWND
    /// the backend has created.
    pub fn new(host_hwnd: HWND) -> Self {
        let (font, line_height, base_font_size, base_font_family) = create_ui_font();
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
            assets: HashMap::new(),
            external_handlers: runtime_core::ExternalRegistry::new(),
            font,
            line_height,
            base_font_size,
            base_font_family,
            font_cache: font::FontCache::new(),
            installed_typefaces: std::collections::HashSet::new(),
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
                is_icon: false,
                is_image: false,
                text: String::new(),
                font: HFONT(std::ptr::null_mut()),
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
                is_icon: false,
                is_image: false,
                text: text.to_string(),
                font: self.font,
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
        self.measure_text_with(text, self.font)
    }

    /// Measure `text` in an explicit `font`. Text must be measured in the
    /// font it will actually be DRAWN in — measuring a 56 px headline in
    /// the 12 px shell font lays it out ~4× too narrow and clips it.
    fn measure_text_with(&self, text: &str, font: HFONT) -> (i32, i32) {
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
            let prev = SelectObject(dc, HGDIOBJ(font.0));
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

    /// Resolve a text style's `font_size` / `font_weight` / `font_family`
    /// to a cached `HFONT`, or `None` when the style sets no font
    /// properties at all (leave the control on the shell font).
    ///
    /// A `FontFamily::Typeface` resolves to the typeface's `family_name`,
    /// which `register_typeface` has already installed into the process
    /// font table — so `CreateFontIndirectW` finds the bundled face.
    fn font_for_style(&mut self, style: &StyleRules) -> Option<HFONT> {
        if style.font_size.is_none() && style.font_weight.is_none() && style.font_family.is_none()
        {
            return None;
        }
        let size_px = match style.font_size.as_ref().map(|t| t.resolve()) {
            Some(Length::Px(v)) if v >= 1.0 => v.round() as i32,
            _ => self.base_font_size,
        };
        let weight = font::weight_to_gdi(style.font_weight.unwrap_or_default());
        let family = match &style.font_family {
            Some(runtime_core::FontFamily::System(name)) => name.clone(),
            Some(runtime_core::FontFamily::Typeface(tf)) => tf.family_name.to_string(),
            None => self.base_font_family.clone(),
        };
        let key = font::FontKey { family, size_px, weight, italic: false };
        if let Some(f) = self.font_cache.get(&key) {
            return Some(*f);
        }
        let f = font::create_hfont(&key);
        if f.is_invalid() {
            return None;
        }
        self.font_cache.insert(key, f);
        Some(f)
    }

    /// Apply `hfont` to a text-bearing control and re-measure its
    /// intrinsic size in that font.
    fn set_node_font(&mut self, node: &WindowsNode, hfont: HFONT) {
        unsafe {
            SendMessageW(node.hwnd, WM_SETFONT, WPARAM(hfont.0 as usize), LPARAM(1));
        }
        let content = match self.nodes.get_mut(&node.id) {
            Some(meta) => {
                meta.font = hfont;
                meta.text.clone()
            }
            None => return,
        };
        let (w, h) = self.measure_text_with(&content, hfont);
        eprintln!("[dbg set_node_font] id={} text={:?} measured={}x{}", node.id, content, w, h);
        self.set_intrinsic(node, w, h);
        unsafe {
            let _ = InvalidateRect(node.hwnd, None, true);
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
        ensure_gdiplus();
        // WS_CLIPCHILDREN so our WM_PAINT of the background never paints
        // over child controls (text, buttons) mounted on top.
        let node = self.create_child(VIEW_CLASS_NAME, "", WS_CLIPCHILDREN.0, None);
        // Construct via `default()` + field assignment rather than
        // functional-record-update: `ViewPaint` implements `Drop` (it
        // owns an HBRUSH), so `..ViewPaint::default()` would try to move
        // the non-`Copy` `gradient` field out of a `Drop` value (E0509).
        let mut vp = ViewPaint::default();
        vp.on_click = on_click;
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

    /// Resolve an image `src` to a decoded GDI+ bitmap. Handles
    /// `asset://{id}` (via the registered [`AssetEntry`]), `data:` base64
    /// URLs, `file://` + bare local paths, and returns `None` for remote
    /// `http(s)://` URLs (no native fetch) or any decode failure — the
    /// caller renders `None` as a blank box.
    fn decode_src(&self, src: &str) -> Option<image::DecodedImage> {
        if let Some(rest) = src.strip_prefix("asset://") {
            let id: u64 = rest.parse().ok()?;
            return match self.assets.get(&id) {
                Some(AssetEntry::File(path)) => image::load_image_file(path),
                Some(AssetEntry::Bytes { bytes, ext }) => image::load_image_bytes(bytes, ext),
                _ => None,
            };
        }
        if src.starts_with("data:") {
            let (bytes, ext) = image::decode_data_url(src)?;
            return image::load_image_bytes(&bytes, &ext);
        }
        if let Some(path) = src.strip_prefix("file://") {
            // `file:///C:/x.png` → strip the leading slash(es) before the
            // drive letter so GDI+ gets a plain Windows path.
            return image::load_image_file(path.trim_start_matches('/'));
        }
        if src.starts_with("http://") || src.starts_with("https://") {
            // No native HTTP client — blank, matching the framework's
            // "Android ImageView has no URL loader" posture.
            return None;
        }
        image::load_image_file(src)
    }
}

/// Build the UI font (the shell's message font — Segoe UI on modern
/// Windows) and its single-line height. Falls back to a null `HFONT`
/// (controls keep their default font) if the metrics query fails.
fn create_ui_font() -> (HFONT, i32, i32, String) {
    // Fallbacks if the metrics query fails: no font (controls keep their
    // default) and a conservative 16 px line / 12 px em.
    const FALLBACK: (i32, i32) = (16, 12);
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
            return (HFONT(std::ptr::null_mut()), FALLBACK.0, FALLBACK.1, String::new());
        }
        // `lfHeight` is negative for an em height; take its magnitude as
        // the base font size.
        let base_size = ncm.lfMessageFont.lfHeight.abs().max(1);
        let face = &ncm.lfMessageFont.lfFaceName;
        let face_len = face.iter().position(|&c| c == 0).unwrap_or(face.len());
        let base_family = String::from_utf16_lossy(&face[..face_len]);
        let font = CreateFontIndirectW(&ncm.lfMessageFont);
        if font.is_invalid() {
            return (HFONT(std::ptr::null_mut()), FALLBACK.0, FALLBACK.1, base_family);
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
        (font, line_height, base_size, base_family)
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
// diverging only in mechanism (GDI+ paint vs GSK nodes).
//
// Painting uses **GDI+** (not raw GDI): anti-aliased rounded corners,
// per-corner radii, alpha-blended fills (GDI's `RoundRect` is aliased
// and drops alpha), linear + radial gradient fills, and per-side
// borders (uniform → one AA stroke that follows the radius; asymmetric
// → straight per-side bars). GDI+ binds to the `WM_PAINT` HDC, so it
// honors the WS_CLIPCHILDREN clip and never paints over child controls
// — the reason we don't use a Direct2D HwndRenderTarget (which ignores
// child windows). Remaining gaps: an asymmetric border on a *rounded*
// box can't trace the corner (straight bars, notched corners — same
// limitation as the Apple backends' per-side bars), and child controls
// aren't clipped to the corner radius (HWND clipping is rectangular).

const VIEW_CLASS_NAME: PCWSTR = PCWSTR(windows::core::w!("IdealystView").as_ptr());

/// Opaque white — the fallback child-text background for a view with
/// no explicit background color.
const WHITE: COLORREF = COLORREF(0x00FF_FFFF);

/// One resolved CSS border side. `color: None` means the author set a
/// width but no color for this side; the paint path falls back to the
/// first side that *does* carry a color (matching the Apple backends'
/// `uniform_border` fallback, so a `border_width` set without a color
/// still renders).
#[derive(Clone, Copy, Default, PartialEq)]
struct BorderSide {
    width: f32,
    color: Option<Rgba>,
}

/// Shape of a resolved gradient, mirroring [`GradientKind`] with the
/// `extent` enum flattened to a `farthest` bool (the only distinction
/// the radius math needs).
#[derive(Clone, Copy)]
enum GradKind {
    Linear { angle_deg: f32 },
    Radial { center: (f32, f32), radius: f32, farthest: bool },
}

/// A gradient resolved for GDI+ painting: shape + `(offset, ARGB)`
/// stops in ascending-offset order. Held on [`ViewPaint`] so the
/// `WM_PAINT` handler can fill the view's rounded-rect path with a
/// GDI+ gradient brush.
struct GradientPaint {
    kind: GradKind,
    /// `(offset 0..=1, 0xAARRGGBB)` — ascending by offset, matching the
    /// framework's [`Gradient::stops`] contract.
    stops: Vec<(f32, u32)>,
}

/// Visual paint state for one `IdealystView` window, stored behind its
/// `GWLP_USERDATA`.
struct ViewPaint {
    /// Fill color; `None` = don't paint a background (transparent).
    background: Option<Rgba>,
    /// Gradient fill, painted OVER the solid `background` (framework
    /// contract: when both are set the gradient z-replaces the solid
    /// fill). `None` = no gradient. Clipped to the corner radius.
    gradient: Option<GradientPaint>,
    /// Per-side borders in `[top, right, bottom, left]` order. A uniform
    /// set (all widths + effective colors equal) is stroked as one
    /// anti-aliased path that follows the corner radius; an asymmetric
    /// set (e.g. a bottom-only underline) is drawn as straight per-side
    /// bars — which, like the Apple backends, can't trace a rounded
    /// corner (documented limitation).
    borders: [BorderSide; 4],
    /// Per-corner radius in px: `[top_left, top_right, bottom_right,
    /// bottom_left]` (0 = square corner). GDI+ arcs give each corner
    /// its own radius with anti-aliasing.
    radii: [f32; 4],
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
            gradient: None,
            borders: [BorderSide::default(); 4],
            radii: [0.0; 4],
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

/// Resolve a `Tokenized<f32>` border-width slot to px, or 0.
fn resolve_width(t: &Option<runtime_core::Tokenized<f32>>) -> f32 {
    t.as_ref().map(|x| x.resolve()).unwrap_or(0.0)
}

/// Resolve the framework's [`Gradient`] into a paint-ready
/// [`GradientPaint`] (shape + ARGB stops). Stop colors parse through the
/// same `runtime_core::color` path as every other color; unparseable
/// stops fall back to transparent so a typo degrades to a hole rather
/// than a panic.
fn resolve_gradient(g: &Gradient) -> GradientPaint {
    let kind = match g.kind {
        GradientKind::Linear { angle_deg } => GradKind::Linear { angle_deg },
        GradientKind::Radial { center, radius, extent } => GradKind::Radial {
            center,
            radius,
            farthest: matches!(extent, RadialExtent::FarthestCorner),
        },
    };
    let stops = g
        .stops
        .iter()
        .map(|s| {
            let rgba = runtime_core::color::parse_or(&s.color.0, Rgba::TRANSPARENT);
            (s.offset, rgba.to_argb_u32())
        })
        .collect();
    GradientPaint { kind, stops }
}

/// Decide whether the four border sides collapse to a single uniform
/// stroke. Mirrors `backend_apple_core::border::uniform_border` (same
/// routing so Windows converges with the Apple backends per Rule #7),
/// operating on already-resolved [`Rgba`] rather than color tokens. A
/// `None` side color falls back to the first side that carries one, so a
/// `border_width` set without an explicit color still counts as uniform.
///
/// `Some((width, color))` → stroke one anti-aliased rounded path;
/// `None` → the asymmetric case, drawn as straight per-side bars.
fn uniform_border(sides: &[BorderSide; 4]) -> Option<(f32, Rgba)> {
    if !sides.iter().all(|s| (s.width - sides[0].width).abs() < f32::EPSILON) {
        return None;
    }
    if sides[0].width <= 0.0 {
        return None;
    }
    let fallback = sides.iter().find_map(|s| s.color);
    let eff: [Option<Rgba>; 4] = [
        sides[0].color.or(fallback),
        sides[1].color.or(fallback),
        sides[2].color.or(fallback),
        sides[3].color.or(fallback),
    ];
    let first = eff[0]?;
    if eff.iter().all(|c| *c == Some(first)) {
        Some((sides[0].width, first))
    } else {
        None
    }
}

/// Start/end points (in device pixels) of a linear gradient's axis for a
/// `w × h` box. CSS angle convention (matching [`GradientKind::Linear`]):
/// `0°` = bottom→top, `90°` = left→right, `180°` = top→bottom. Solved in
/// Win32's y-down client space, where the unit direction from the
/// `offset 0` end toward the `offset 1` end is `(sin θ, −cos θ)`. The
/// axis is centered on the box; its half-length is the axis-projected box
/// extent, so an axis-aligned band fills its short dimension exactly and
/// no pixel projects outside `[0, 1]` (hence the line brush's wrap mode
/// is never exercised). Identical formula to the Linux backend's
/// `gradient::linear_points`, so the two backends place stops identically.
fn linear_points(angle_deg: f32, w: f32, h: f32) -> ((f32, f32), (f32, f32)) {
    let rad = angle_deg.to_radians();
    let dx = rad.sin();
    let dy = -rad.cos();
    let half = (dx.abs() * w + dy.abs() * h) / 2.0;
    let (cx, cy) = (w / 2.0, h / 2.0);
    let start = (cx - dx * half, cy - dy * half);
    let end = (cx + dx * half, cy + dy * half);
    (start, end)
}

/// Pixel radius of a radial gradient's `offset 1.0` stop. `ClosestSide`
/// (`farthest == false`) references half the shorter box side; `farthest`
/// references the distance from the center to the farthest corner. Same
/// formula as the Linux backend's `gradient::radial_radius`.
fn radial_radius(center: (f32, f32), radius: f32, farthest: bool, w: f32, h: f32) -> f32 {
    let reference = if farthest {
        let cx = center.0 * w;
        let cy = center.1 * h;
        [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)]
            .iter()
            .map(|(x, y)| ((x - cx).powi(2) + (y - cy).powi(2)).sqrt())
            .fold(0.0_f32, f32::max)
    } else {
        w.min(h) / 2.0
    };
    (reference * radius).max(0.0)
}

/// Initialize GDI+ once per process — required before any `Gdip*`
/// call. The startup token is intentionally leaked: GDI+ stays live
/// for the process lifetime (no `GdiplusShutdown`; the app exits via
/// `TerminateProcess` anyway).
fn ensure_gdiplus() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| unsafe {
        let input = GdiplusStartupInput {
            GdiplusVersion: 1,
            DebugEventCallback: 0,
            SuppressBackgroundThread: Default::default(),
            SuppressExternalCodecs: Default::default(),
        };
        let mut token: usize = 0;
        let mut output: GdiplusStartupOutput = std::mem::zeroed();
        let _ = GdiplusStartup(&mut token, &input, &mut output);
    });
}

/// Append a rounded-rect figure (per-corner radii `[tl, tr, br, bl]`)
/// to `path`, inset by `inset` on every side — half the border width,
/// so a centered stroke stays inside the client rect. Square corners
/// (all radii ~0) use a plain rectangle; otherwise each corner is a
/// 90° arc and GDI+ connects consecutive arcs with the straight edges.
unsafe fn build_round_rect_path(path: *mut GpPath, w: f32, h: f32, radii: [f32; 4], inset: f32) {
    let left = inset;
    let top = inset;
    let right = (w - inset).max(left);
    let bottom = (h - inset).max(top);
    let bw = right - left;
    let bh = bottom - top;

    if radii.iter().all(|r| *r < 0.5) {
        let _ = GdipAddPathRectangle(path, left, top, bw, bh);
        return;
    }

    // Clamp radii to half the box so arcs never overlap.
    let rmax = bw.min(bh) / 2.0;
    let c = |i: usize| radii[i].clamp(0.0, rmax);
    let (tl, tr, br, bl) = (c(0), c(1), c(2), c(3));
    // Arc bounding boxes are 2r × 2r at each corner; sweeps run
    // clockwise starting from the corner's outer quadrant.
    let _ = GdipAddPathArc(path, left, top, 2.0 * tl, 2.0 * tl, 180.0, 90.0);
    let _ = GdipAddPathArc(path, right - 2.0 * tr, top, 2.0 * tr, 2.0 * tr, 270.0, 90.0);
    let _ = GdipAddPathArc(path, right - 2.0 * br, bottom - 2.0 * br, 2.0 * br, 2.0 * br, 0.0, 90.0);
    let _ = GdipAddPathArc(path, left, bottom - 2.0 * bl, 2.0 * bl, 2.0 * bl, 90.0, 90.0);
    let _ = GdipClosePathFigure(path);
}

/// Build `(colors, positions)` arrays for a GDI+ preset blend from
/// ascending-offset `(offset, ARGB)` stops. GDI+ requires
/// `positions[0] == 0.0` and `positions[last] == 1.0`, so the extremes
/// are padded with the end-stop colors when the author's stops don't
/// reach them. With `reverse` (radial/path gradients, whose blend runs
/// boundary→center rather than the framework's center→edge), each
/// position becomes `1 - offset` and the arrays are reversed to stay
/// ascending.
fn blend_arrays(stops: &[(f32, u32)], reverse: bool) -> (Vec<u32>, Vec<f32>) {
    let mut pts: Vec<(f32, u32)> =
        stops.iter().map(|(o, c)| (o.clamp(0.0, 1.0), *c)).collect();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    if let Some(&(first_o, first_c)) = pts.first() {
        if first_o > 0.0 {
            pts.insert(0, (0.0, first_c));
        }
    }
    if let Some(&(last_o, last_c)) = pts.last() {
        if last_o < 1.0 {
            pts.push((1.0, last_c));
        }
    }
    if reverse {
        let mut rev: Vec<(f32, u32)> = pts.iter().map(|(o, c)| (1.0 - o, *c)).collect();
        rev.reverse();
        (rev.iter().map(|(_, c)| *c).collect(), rev.iter().map(|(o, _)| *o).collect())
    } else {
        (pts.iter().map(|(_, c)| *c).collect(), pts.iter().map(|(o, _)| *o).collect())
    }
}

/// Fill `fill_path` (the view's rounded-rect region) with the resolved
/// gradient. Linear uses a line-gradient brush along the CSS axis; radial
/// uses a path-gradient brush over an ellipse of the computed radius,
/// clamped outside so box corners beyond the radius keep the edge color.
/// Multi-stop ramps go through the preset-blend APIs.
unsafe fn paint_gradient(
    g: *mut GpGraphics,
    gp: &GradientPaint,
    fill_path: *mut GpPath,
    w: f32,
    h: f32,
) {
    match gp.kind {
        GradKind::Linear { angle_deg } => {
            let (s, e) = linear_points(angle_deg, w, h);
            let p1 = PointF { X: s.0, Y: s.1 };
            let p2 = PointF { X: e.0, Y: e.1 };
            let first = gp.stops.first().map(|s| s.1).unwrap_or(0);
            let last = gp.stops.last().map(|s| s.1).unwrap_or(first);
            let mut brush: *mut GpLineGradient = std::ptr::null_mut();
            // The axis already spans the box's full projected extent (see
            // `linear_points`), so no pixel projects outside [0, 1] and
            // the wrap mode is never exercised — `WrapModeTile` is just
            // the required non-null argument.
            if GdipCreateLineBrush(&p1, &p2, first, last, WrapModeTile, &mut brush).0 == 0
                && !brush.is_null()
            {
                let (colors, positions) = blend_arrays(&gp.stops, false);
                if colors.len() >= 2 {
                    let _ = GdipSetLinePresetBlend(
                        brush,
                        colors.as_ptr(),
                        positions.as_ptr(),
                        colors.len() as i32,
                    );
                }
                let _ = GdipFillPath(g, brush as *mut GpBrush, fill_path);
                let _ = GdipDeleteBrush(brush as *mut GpBrush);
            }
        }
        GradKind::Radial { center, radius, farthest } => {
            let cx = center.0 * w;
            let cy = center.1 * h;
            let r = radial_radius(center, radius, farthest, w, h).max(1.0);
            let mut ellipse: *mut GpPath = std::ptr::null_mut();
            if GdipCreatePath(FillModeWinding, &mut ellipse).0 != 0 || ellipse.is_null() {
                return;
            }
            let _ = GdipAddPathEllipse(ellipse, cx - r, cy - r, 2.0 * r, 2.0 * r);
            let mut brush: *mut GpPathGradient = std::ptr::null_mut();
            if GdipCreatePathGradientFromPath(ellipse, &mut brush).0 == 0 && !brush.is_null() {
                let inner = gp.stops.first().map(|s| s.1).unwrap_or(0);
                let outer = gp.stops.last().map(|s| s.1).unwrap_or(inner);
                let cp = PointF { X: cx, Y: cy };
                let _ = GdipSetPathGradientCenterPoint(brush, &cp);
                let _ = GdipSetPathGradientCenterColor(brush, inner);
                let mut cnt = 1i32;
                let _ = GdipSetPathGradientSurroundColorsWithCount(brush, &outer, &mut cnt);
                let (colors, positions) = blend_arrays(&gp.stops, true);
                if colors.len() >= 2 {
                    let _ = GdipSetPathGradientPresetBlend(
                        brush,
                        colors.as_ptr(),
                        positions.as_ptr(),
                        colors.len() as i32,
                    );
                }
                // Clamp so pixels outside the ellipse (box corners past
                // the radius) keep the outermost stop color rather than
                // tiling the ramp.
                let _ = GdipSetPathGradientWrapMode(brush, WrapModeClamp);
                let _ = GdipFillPath(g, brush as *mut GpBrush, fill_path);
                let _ = GdipDeleteBrush(brush as *mut GpBrush);
            }
            let _ = GdipDeletePath(ellipse);
        }
    }
}

/// Draw asymmetric borders as straight per-side bars (`[top, right,
/// bottom, left]`), each a filled rectangle of its own width + color. A
/// side with no explicit color falls back to the first side that has
/// one. Corners aren't mitered and a rounded corner isn't traced — the
/// documented limitation shared with the Apple backends' per-side bars
/// (a straight bar can't follow a curve).
unsafe fn paint_side_borders(g: *mut GpGraphics, sides: &[BorderSide; 4], w: f32, h: f32) {
    let fallback = sides.iter().find_map(|s| s.color);
    for (idx, side) in sides.iter().enumerate() {
        if side.width <= 0.5 {
            continue;
        }
        let Some(color) = side.color.or(fallback) else {
            continue;
        };
        if color.a == 0 {
            continue;
        }
        let bw = side.width;
        let (x, y, rw, rh) = match idx {
            0 => (0.0, 0.0, w, bw),      // top
            1 => (w - bw, 0.0, bw, h),   // right
            2 => (0.0, h - bw, w, bw),   // bottom
            _ => (0.0, 0.0, bw, h),      // left
        };
        let mut brush: *mut GpSolidFill = std::ptr::null_mut();
        if GdipCreateSolidFill(color.to_argb_u32(), &mut brush).0 == 0 {
            let mut path: *mut GpPath = std::ptr::null_mut();
            if GdipCreatePath(FillModeWinding, &mut path).0 == 0 && !path.is_null() {
                let _ = GdipAddPathRectangle(path, x, y, rw, rh);
                let _ = GdipFillPath(g, brush as *mut GpBrush, path);
                let _ = GdipDeletePath(path);
            }
            let _ = GdipDeleteBrush(brush as *mut GpBrush);
        }
    }
}

/// Paint the styled box for `hwnd` from `p` via GDI+ — anti-aliased,
/// alpha-blended, per-corner radius, gradient + per-side borders. Called
/// from `WM_PAINT`. GDI+ binds to the paint DC, so it honors the
/// WS_CLIPCHILDREN clip and never paints over child controls. Paint
/// order: solid background, then gradient over it (framework contract),
/// then the border on top.
unsafe fn paint_view(hwnd: HWND, p: &ViewPaint) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let w = (rc.right - rc.left) as f32;
    let h = (rc.bottom - rc.top) as f32;

    let has_bg = p.background.map(|c| c.a > 0).unwrap_or(false);
    let has_gradient = p.gradient.as_ref().map(|g| !g.stops.is_empty()).unwrap_or(false);
    let uniform = uniform_border(&p.borders);
    let has_any_border = p.borders.iter().any(|s| s.width > 0.5);

    if (has_bg || has_gradient || has_any_border) && w > 0.0 && h > 0.0 {
        let mut g: *mut GpGraphics = std::ptr::null_mut();
        if GdipCreateFromHDC(hdc, &mut g).0 == 0 && !g.is_null() {
            let _ = GdipSetSmoothingMode(g, SmoothingModeAntiAlias);

            // Background + gradient share the rounded-rect fill path, so
            // the gradient is clipped to the corner radius for free.
            if has_bg || has_gradient {
                let mut fill_path: *mut GpPath = std::ptr::null_mut();
                if GdipCreatePath(FillModeWinding, &mut fill_path).0 == 0 && !fill_path.is_null() {
                    build_round_rect_path(fill_path, w, h, p.radii, 0.0);
                    if let Some(bg) = p.background.filter(|c| c.a > 0) {
                        let mut brush: *mut GpSolidFill = std::ptr::null_mut();
                        // GDI+ colors are 0xAARRGGBB — alpha honored.
                        if GdipCreateSolidFill(bg.to_argb_u32(), &mut brush).0 == 0 {
                            let _ = GdipFillPath(g, brush as *mut GpBrush, fill_path);
                            let _ = GdipDeleteBrush(brush as *mut GpBrush);
                        }
                    }
                    if let Some(gp) = p.gradient.as_ref().filter(|g| !g.stops.is_empty()) {
                        paint_gradient(g, gp, fill_path, w, h);
                    }
                    let _ = GdipDeletePath(fill_path);
                }
            }

            match uniform {
                // Uniform border → one AA stroke that follows the radius.
                // Inset by half the stroke width so the centered pen stays
                // inside the client rect.
                Some((width, color)) => {
                    let mut stroke_path: *mut GpPath = std::ptr::null_mut();
                    if GdipCreatePath(FillModeWinding, &mut stroke_path).0 == 0
                        && !stroke_path.is_null()
                    {
                        build_round_rect_path(stroke_path, w, h, p.radii, width / 2.0);
                        let mut pen: *mut GpPen = std::ptr::null_mut();
                        if GdipCreatePen1(color.to_argb_u32(), width, UnitPixel, &mut pen).0 == 0 {
                            let _ = GdipDrawPath(g, pen, stroke_path);
                            let _ = GdipDeletePen(pen);
                        }
                        let _ = GdipDeletePath(stroke_path);
                    }
                }
                // Asymmetric → straight per-side bars.
                None if has_any_border => paint_side_borders(g, &p.borders, w, h),
                None => {}
            }

            let _ = GdipDeleteGraphics(g);
        }
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
        // Clear the client to a clean WHITE backdrop before WM_PAINT
        // draws the styled box on top. Without this, an unstyled /
        // transparent view (or the anti-aliased corner gaps outside a
        // rounded fill) shows uninitialized window pixels (desktop
        // bleed-through). White is the neutral app backdrop; a fully
        // opaque square view (radius 0) overpaints it entirely in
        // WM_PAINT, so white is only ever visible where the view is
        // genuinely transparent or rounded. (True transparency over a
        // *colored* parent isn't possible without compositing — a
        // documented Win32-layering limitation; nested rounded cards
        // get white corners rather than the parent's color.)
        WM_ERASEBKGND => {
            let hdc = windows::Win32::Graphics::Gdi::HDC(wparam.0 as *mut std::ffi::c_void);
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let white = HBRUSH(GetStockObject(WHITE_BRUSH).0);
            FillRect(hdc, &rc, white);
            LRESULT(1)
        }
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
        // Re-measure in the node's CURRENT font (which `apply_style` may
        // have changed from the shell default) so the new string's
        // intrinsic size is right.
        let hfont = match self.nodes.get_mut(&node.id) {
            Some(meta) => {
                meta.text = content.to_string();
                meta.font
            }
            None => self.font,
        };
        let (w, h) = self.measure_text_with(content, hfont);
        self.set_intrinsic(node, w, h);
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
        for (id, hwnd, x, y, w, h) in updates {
            if hwnd.is_invalid() {
                continue;
            }
            if let Some(meta) = self.nodes.get(&id) {
                if meta.is_text {
                    eprintln!("[dbg finish] id={} text={:?} frame={},{} {}x{}", id, meta.text, x, y, w, h);
                }
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
        src: &str,
        _alt: Option<&str>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        ensure_gdiplus();
        image::ensure_image_class_registered();
        let node = self.create_child(image::IMAGE_CLASS_NAME, "", 0, None);
        let decoded = self.decode_src(src);
        // Natural pixel size as the intrinsic size, so an image without an
        // explicit width/height still lays out at its own size (like web's
        // `naturalWidth`); an explicit style overrides it via `set_style`.
        if let Some(d) = &decoded {
            let (w, h) = d.natural();
            self.set_intrinsic(&node, w as i32, h as i32);
        }
        let paint = image::ImagePaint { image: decoded, ..Default::default() };
        unsafe {
            SetWindowLongPtrW(node.hwnd, GWLP_USERDATA, Box::into_raw(Box::new(paint)) as isize);
        }
        if let Some(meta) = self.nodes.get_mut(&node.id) {
            meta.is_image = true;
        }
        node
    }

    fn create_icon(
        &mut self,
        data: &runtime_core::primitives::icon::IconData,
        color: Option<&Color>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        ensure_gdiplus();
        icon::ensure_icon_class_registered();
        let node = self.create_child(icon::ICON_CLASS_NAME, "", 0, None);
        // No color → opaque black (the native analogue of web's
        // `currentColor` / nearest text color, matching the Linux
        // `DEFAULT_ICON_COLOR`).
        let rgba = color
            .map(|c| runtime_core::color::parse_or(&c.0, Rgba::BLACK))
            .unwrap_or(Rgba::BLACK);
        let paint = icon::IconPaint::from_data(data, rgba);
        // Icons have no intrinsic content size; default the intrinsic to
        // the view-box so a bare `icon(X)` is visible under flex.
        // `.size(n)` / an explicit width/height style overrides it.
        let (vw, vh) = paint.view_box;
        self.set_intrinsic(&node, vw as i32, vh as i32);
        unsafe {
            SetWindowLongPtrW(node.hwnd, GWLP_USERDATA, Box::into_raw(Box::new(paint)) as isize);
        }
        if let Some(meta) = self.nodes.get_mut(&node.id) {
            meta.is_icon = true;
        }
        node
    }

    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        let decoded = self.decode_src(src);
        let ud = unsafe { GetWindowLongPtrW(node.hwnd, GWLP_USERDATA) };
        if ud != 0 {
            // Update the intrinsic to the new bitmap's natural size before
            // swapping (the reactive `src` change may resize the leaf).
            if let Some(d) = &decoded {
                let (w, h) = d.natural();
                self.set_intrinsic(node, w as i32, h as i32);
            }
            // SAFETY: an image node's USERDATA holds the `Box<ImagePaint>`
            // from `create_image`. Assigning `image` drops the previous
            // `DecodedImage`, disposing the old `GpImage`.
            let paint = unsafe { &mut *(ud as *mut image::ImagePaint) };
            paint.image = decoded;
            unsafe {
                let _ = InvalidateRect(node.hwnd, None, true);
            }
        }
    }

    fn update_icon_color(&mut self, node: &Self::Node, color: &Color) {
        let ud = unsafe { GetWindowLongPtrW(node.hwnd, GWLP_USERDATA) };
        if ud != 0 {
            let paint = unsafe { &mut *(ud as *mut icon::IconPaint) };
            paint.color = runtime_core::color::parse_or(&color.0, Rgba::BLACK);
            unsafe {
                let _ = InvalidateRect(node.hwnd, None, true);
            }
        }
    }

    fn update_icon_data(&mut self, node: &Self::Node, data: &runtime_core::primitives::icon::IconData) {
        let ud = unsafe { GetWindowLongPtrW(node.hwnd, GWLP_USERDATA) };
        if ud != 0 {
            let (vw, vh) = {
                let paint = unsafe { &mut *(ud as *mut icon::IconPaint) };
                paint.set_data(data);
                paint.view_box
            };
            // A new glyph may carry a different view-box → refresh intrinsic.
            self.set_intrinsic(node, vw as i32, vh as i32);
            unsafe {
                let _ = InvalidateRect(node.hwnd, None, true);
            }
        }
    }

    fn register_typeface(
        &mut self,
        id: TypefaceId,
        _family_name: &str,
        faces: &[TypefaceFace],
        _fallback: SystemFallback,
    ) {
        // Install each face into the process font table so a later
        // `CreateFontIndirectW` with the typeface's family name resolves
        // to the bundled TTF. Idempotent — the framework may re-register
        // on a theme/token pass.
        if !self.installed_typefaces.insert(id.0) {
            return;
        }
        for face in faces {
            font::install_face(&face.source);
        }
    }

    fn register_asset(&mut self, id: AssetId, _kind: AssetTag, source: &AssetSource) {
        let entry = match source {
            AssetSource::Bundled { path } => AssetEntry::File(path.to_string()),
            AssetSource::Embedded { bytes, extension } => {
                AssetEntry::Bytes { bytes, ext: extension.to_string() }
            }
            AssetSource::BundledEmbedded { bytes, extension, .. } => {
                AssetEntry::Bytes { bytes, ext: extension.to_string() }
            }
            AssetSource::Remote { .. } => AssetEntry::Remote,
        };
        self.assets.insert(id.0, entry);
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
                paint.gradient = style.background_gradient.as_ref().map(resolve_gradient);
                // Per-side borders in [top, right, bottom, left] order.
                // `paint_view` collapses a uniform set to one AA stroke and
                // renders an asymmetric set as per-side bars.
                paint.borders = [
                    BorderSide {
                        width: resolve_width(&style.border_top_width),
                        color: resolve_color(&style.border_top_color),
                    },
                    BorderSide {
                        width: resolve_width(&style.border_right_width),
                        color: resolve_color(&style.border_right_color),
                    },
                    BorderSide {
                        width: resolve_width(&style.border_bottom_width),
                        color: resolve_color(&style.border_bottom_color),
                    },
                    BorderSide {
                        width: resolve_width(&style.border_left_width),
                        color: resolve_color(&style.border_left_color),
                    },
                ];
                paint.radii = [
                    resolve_radius(&style.border_top_left_radius),
                    resolve_radius(&style.border_top_right_radius),
                    resolve_radius(&style.border_bottom_right_radius),
                    resolve_radius(&style.border_bottom_left_radius),
                ];
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
            // Typography: size / weight / family. Re-measures the leaf in
            // the resolved font so layout matches what's drawn.
            if let Some(hfont) = self.font_for_style(style) {
                self.set_node_font(node, hfont);
            }
        }

        // Image object-fit: push the resolved `object_fit` into the
        // ImagePaint so `WM_PAINT` scales the bitmap correctly. Gated on
        // `is_image` so we only ever cast an `IdealystImage`'s USERDATA.
        let is_image = self.nodes.get(&node.id).map(|m| m.is_image).unwrap_or(false);
        if is_image {
            if let Some(fit) = style.object_fit {
                let ud = unsafe { GetWindowLongPtrW(node.hwnd, GWLP_USERDATA) };
                if ud != 0 {
                    let paint = unsafe { &mut *(ud as *mut image::ImagePaint) };
                    paint.object_fit = fit;
                    unsafe {
                        let _ = InvalidateRect(node.hwnd, None, true);
                    }
                }
            }
        }
    }
}

// `RefCell` import kept for the eventual wm_command_dispatch state.
#[allow(dead_code)]
type _KeepRefCell = RefCell<()>;

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba(s: &str) -> Rgba {
        runtime_core::color::parse_or(s, Rgba::TRANSPARENT)
    }

    fn side(width: f32, color: Option<&str>) -> BorderSide {
        BorderSide { width, color: color.map(rgba) }
    }

    // --- uniform_border routing (mirrors backend_apple_core::border) ---

    #[test]
    fn uniform_all_sides_equal_collapses() {
        let c = Some("#e5e5e5");
        let sides = [side(1.0, c), side(1.0, c), side(1.0, c), side(1.0, c)];
        assert_eq!(uniform_border(&sides), Some((1.0, rgba("#e5e5e5"))));
    }

    #[test]
    fn uniform_width_without_per_side_color_falls_back() {
        // Width on every side, color only on top → fallback fills the
        // rest, so it's still one uniform stroke. This is the case the
        // `smoke_styled` cards rely on for a full border.
        let sides = [
            side(2.0, Some("#000000")),
            side(2.0, None),
            side(2.0, None),
            side(2.0, None),
        ];
        assert_eq!(uniform_border(&sides), Some((2.0, rgba("#000000"))));
    }

    #[test]
    fn asymmetric_bottom_only_stays_per_side() {
        // A bottom-only underline must NOT collapse.
        let sides = [
            side(0.0, None),
            side(0.0, None),
            side(1.0, Some("#000000")),
            side(0.0, None),
        ];
        assert_eq!(uniform_border(&sides), None);
    }

    #[test]
    fn asymmetric_differing_colors_stays_per_side() {
        let sides = [
            side(1.0, Some("#ff0000")),
            side(1.0, Some("#00ff00")),
            side(1.0, Some("#ff0000")),
            side(1.0, Some("#00ff00")),
        ];
        assert_eq!(uniform_border(&sides), None);
    }

    #[test]
    fn no_border_when_all_widths_zero() {
        let sides = [side(0.0, Some("#000")), side(0.0, None), side(0.0, None), side(0.0, None)];
        assert_eq!(uniform_border(&sides), None);
    }

    // --- linear gradient axis geometry (CSS angle convention) ---

    fn approx(a: (f32, f32), b: (f32, f32)) {
        assert!((a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3, "{a:?} != {b:?}");
    }

    #[test]
    fn linear_0deg_runs_bottom_to_top() {
        let (s, e) = linear_points(0.0, 100.0, 100.0);
        approx(s, (50.0, 100.0)); // offset 0 at the bottom
        approx(e, (50.0, 0.0)); //   offset 1 at the top
    }

    #[test]
    fn linear_90deg_runs_left_to_right() {
        let (s, e) = linear_points(90.0, 100.0, 100.0);
        approx(s, (0.0, 50.0));
        approx(e, (100.0, 50.0));
    }

    #[test]
    fn linear_180deg_runs_top_to_bottom() {
        let (s, e) = linear_points(180.0, 100.0, 100.0);
        approx(s, (50.0, 0.0));
        approx(e, (50.0, 100.0));
    }

    // --- radial radius reference ---

    #[test]
    fn radial_closest_side_is_half_shorter_side() {
        let r = radial_radius((0.5, 0.5), 1.0, false, 100.0, 200.0);
        assert!((r - 50.0).abs() < 1e-3);
    }

    #[test]
    fn radial_farthest_corner_reaches_the_corner() {
        // Center (50,100); farthest corner is any at distance
        // sqrt(50^2 + 100^2) ≈ 111.8.
        let r = radial_radius((0.5, 0.5), 1.0, true, 100.0, 200.0);
        assert!((r - 111.803).abs() < 1e-2, "got {r}");
    }

    // --- preset-blend array construction ---

    #[test]
    fn blend_pads_missing_endpoints() {
        let stops = [(0.3, 0xAA), (0.7, 0xBB)];
        let (colors, positions) = blend_arrays(&stops, false);
        assert_eq!(positions.first().copied(), Some(0.0));
        assert_eq!(positions.last().copied(), Some(1.0));
        assert_eq!(colors, vec![0xAA, 0xAA, 0xBB, 0xBB]);
    }

    #[test]
    fn blend_reverse_flips_positions_for_path_gradient() {
        // center→edge stops [(0,inner),(1,outer)] become boundary→center
        // [(0,outer),(1,inner)] for the path gradient.
        let stops = [(0.0, 0x11), (1.0, 0x22)];
        let (colors, positions) = blend_arrays(&stops, true);
        assert_eq!(positions, vec![0.0, 1.0]);
        assert_eq!(colors, vec![0x22, 0x11]);
    }
}
