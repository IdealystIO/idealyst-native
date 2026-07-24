//! Native Win32 backend — single-surface GDI+ scene renderer.
//!
//! Implements `runtime_core::Backend` with a **painted scene model**: the
//! host's top-level HWND is one canvas, and the view / text / icon /
//! image tree is painted with GDI+ into a double-buffered memory DC in
//! tree order (see [`scene`]). Only genuinely-native interactive
//! controls — button, edit, checkbox, trackbar, progress — are real
//! child HWNDs, positioned on top by the layout pass.
//!
//! ## Why not HWND-per-view
//!
//! The first cut of this backend gave every primitive its own child
//! HWND. That model cannot express the framework's visual semantics:
//! Win32 sibling child windows are opaque rectangles that never
//! alpha-composite, so overlapping translucent layers (the welcome
//! scene's vignette bands + radial sun-glare + full-window content
//! layer) blank each other out regardless of what their `WM_PAINT`
//! draws. Every other backend composites a retained tree on one surface
//! (CALayer on Apple, GSK on GTK, the DOM compositor on web) — the
//! painted scene is the Win32 equivalent, and it makes opacity,
//! transforms, gradient-stop animation, and alpha-over-anything
//! compositing all fall out of one code path instead of per-window
//! hacks (repo rule §7).
//!
//! ## Threading
//!
//! HWND calls and painting are single-threaded — the host shell invokes
//! everything on the thread that created the window. The backend assumes
//! it's on that thread.
//!
//! ## Build gating
//!
//! The lib body is gated on `cfg(target_os = "windows")`. On other hosts
//! this crate compiles to an empty rlib.

#![cfg(target_os = "windows")]

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::animation::AnimProp;
use runtime_core::assets::{
    AssetId, AssetSource, AssetTag, SystemFallback, TypefaceFace, TypefaceId,
};
use runtime_core::color::Rgba;
use runtime_core::primitives::navigator::{NavigatorHandler, NavigatorHost, RegisterNavigator};
use runtime_core::{
    Action, Backend, Color, ColorScheme, Gradient, GradientKind, Length, NavigatorRegistry,
    Overflow, Platform, RadialExtent, RegisterExternal, StyleRules, Transform,
};
use runtime_layout::LayoutTree;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontIndirectW, DeleteObject, GetDC, GetTextExtentPoint32W, HFONT, HGDIOBJ,
    InvalidateRect, ReleaseDC, SelectObject, SetWindowRgn, HDC,
};
use windows::Win32::Graphics::GdiPlus::{
    GdipDeleteFont, GdiplusStartup, GdiplusStartupInput, GdiplusStartupOutput,
};
use windows::Win32::UI::Controls::{
    InitCommonControlsEx, ICC_BAR_CLASSES, ICC_PROGRESS_CLASS, INITCOMMONCONTROLSEX,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BeginDeferWindowPos, CreateWindowExW, DeferWindowPos, DestroyWindow, EndDeferWindowPos,
    GetClientRect, GetWindowTextLengthW, GetWindowTextW,
    SendMessageW, SetWindowTextW, ShowWindow, SystemParametersInfoW,
    BS_DEFPUSHBUTTON, HMENU, NONCLIENTMETRICSW, SPI_GETNONCLIENTMETRICS, SWP_NOACTIVATE,
    SWP_NOZORDER, SW_SHOW, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WINDOW_EX_STYLE, WM_SETFONT,
    WS_BORDER, WS_CHILD, WS_VISIBLE,
};

mod code;
mod dcomp;
mod font;
mod graphics;
mod handles;
mod icon;
mod image;
mod scene;
mod wrap;

// =========================================================================
// Win32 control constants
// =========================================================================

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

/// `EDIT` window class (single-line text input).
fn class_edit() -> PCWSTR {
    PCWSTR(windows::core::w!("EDIT").as_ptr())
}
fn class_button() -> PCWSTR {
    PCWSTR(windows::core::w!("BUTTON").as_ptr())
}
// Common controls (comctl32) — need `InitCommonControlsEx` before use.
fn class_trackbar() -> PCWSTR {
    PCWSTR(windows::core::w!("msctls_trackbar32").as_ptr())
}
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

/// Owning wrapper around a UTF-16 buffer so the `PCWSTR` reference
/// stays valid for the duration of a Win32 call.
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

// =========================================================================
// Node — opaque wrapper so the Backend trait's `type Node` is Clone
// =========================================================================

/// Backend-internal handle for a mounted node. `hwnd` is null for
/// painted nodes (view / text / icon / image); it's a real window only
/// for native controls and SDK externals.
#[derive(Clone)]
pub struct WindowsNode {
    pub(crate) id: u64,
    pub(crate) hwnd: HWND,
}

impl WindowsNode {
    /// Access the underlying HWND for SDK extensions that need to send
    /// Win32 messages directly (e.g. a toolbar leaf's `SendMessageW`).
    /// Null for painted (non-control) nodes.
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
// Paint-tree node model
// =========================================================================

/// Per-frame animated transform slots, written by
/// [`Backend::set_animated_f32`]. `scale` and `scale_x`/`scale_y`
/// multiply (matching the Linux backend's composition).
#[derive(Clone, Copy)]
pub(crate) struct AnimTransform {
    pub tx: f32,
    pub ty: f32,
    pub scale: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    pub rotate_deg: f32,
}

impl Default for AnimTransform {
    fn default() -> Self {
        AnimTransform { tx: 0.0, ty: 0.0, scale: 1.0, scale_x: 1.0, scale_y: 1.0, rotate_deg: 0.0 }
    }
}

/// One resolved CSS border side. `color: None` means the author set a
/// width but no color for this side; the paint path falls back to the
/// first side that *does* carry a color (matching the Apple backends'
/// `uniform_border` fallback).
#[derive(Clone, Copy, Default, PartialEq)]
pub(crate) struct BorderSide {
    pub width: f32,
    pub color: Option<Rgba>,
}

/// Shape of a resolved gradient, mirroring [`GradientKind`] with the
/// `extent` enum flattened to a `farthest` bool.
#[derive(Clone, Copy)]
pub(crate) enum GradKind {
    Linear { angle_deg: f32 },
    Radial { center: (f32, f32), radius: f32, farthest: bool },
}

/// A gradient resolved for painting: shape + `(offset, sRGB [r,g,b,a])`
/// stops in ascending-offset order. Stops stay in float sRGB — not
/// packed ARGB — because [`Backend::set_animated_color`] overwrites
/// individual stop colors per frame (the welcome sun pulse).
pub(crate) struct GradientPaint {
    pub kind: GradKind,
    pub stops: Vec<(f32, [f32; 4])>,
}

/// Per-scroll-view state (offset in device px + author callback).
pub(crate) struct ScrollInfo {
    pub horizontal: bool,
    pub offset_x: f32,
    pub offset_y: f32,
    pub on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
}

/// Visuals of a painted container (View / Pressable / ScrollView).
#[derive(Default)]
pub(crate) struct ViewVisual {
    /// Style background (sRGB float). `None` = transparent.
    pub background: Option<[f32; 4]>,
    /// `AnimProp::BackgroundColor` override (the welcome page's dark
    /// wash). Wins over `background` when set.
    pub anim_background: Option<[f32; 4]>,
    pub gradient: Option<GradientPaint>,
    /// `[top, right, bottom, left]`.
    pub borders: [BorderSide; 4],
    /// `[top-left, top-right, bottom-right, bottom-left]` corner radii.
    pub radii: [f32; 4],
    /// Pressable click handler; fired by the host's hit-tested
    /// `WM_LBUTTONUP` routing.
    pub on_click: Option<Rc<dyn Fn()>>,
    pub scroll: Option<ScrollInfo>,
}

impl ViewVisual {
    pub(crate) fn effective_background(&self) -> Option<[f32; 4]> {
        self.anim_background.or(self.background)
    }
}

/// Visuals of a painted text run.
pub(crate) struct TextVisual {
    pub content: String,
    /// Style color (opaque black default, like every other backend).
    pub color: Rgba,
    /// `AnimProp::ForegroundColor` override (the welcome headline's
    /// dark→light tween).
    pub anim_color: Option<[f32; 4]>,
    /// Resolved typography; `None` = the shell font.
    pub font_key: Option<font::FontKey>,
    /// Measured word/space runs for (content, font) — the shared input
    /// both the Taffy measure fn and the painter break lines from.
    /// Rebuilt by `set_text_measure` whenever content or font changes.
    pub plan: Option<Rc<wrap::WrapPlan>>,
    /// Line breaks at the node's current frame width; filled by
    /// `layout_pass` (paint only reads). `None` until first layout or
    /// after a plan rebuild.
    pub lines: Option<wrap::WrappedLines>,
    /// Per-line alignment inside the node's box. Matters once frames
    /// can be wider than their longest line (wrapping); `Justify`
    /// draws as `Left`.
    pub align: runtime_core::TextAlign,
    /// Style `line-height` in px (the css crate emits it as px). Line
    /// advance + CSS half-leading; `None` = the font's natural height.
    pub line_height: Option<f32>,
}

impl TextVisual {
    pub(crate) fn new(content: &str) -> Self {
        Self {
            content: content.to_string(),
            color: Rgba::BLACK,
            anim_color: None,
            font_key: None,
            plan: None,
            lines: None,
            align: runtime_core::TextAlign::Left,
            line_height: None,
        }
    }
}

/// What a node *is* — painted visual or native control.
pub(crate) enum NodeKind {
    View(ViewVisual),
    Text(TextVisual),
    /// Painted colored-runs leaf (the `codeblock` SDK's single-node
    /// realization) — see `code.rs`.
    Code(code::CodeVisual),
    Icon(icon::IconPaint),
    Image(image::ImagePaint),
    /// Native child HWND control (button / edit / checkbox / trackbar /
    /// progress). Positioned in `finish`; paints itself.
    Control { hwnd: HWND },
    /// SDK-external child HWND registered via `register_external_view`.
    External { hwnd: HWND },
}

pub(crate) struct NodeMeta {
    pub kind: NodeKind,
    /// Parent-relative Taffy frame `(x, y, w, h)`, written by `finish`.
    pub frame: (f32, f32, f32, f32),
    /// Window-relative origin, accumulated in `finish` (used to position
    /// control HWNDs and for `absolute_frame` reads).
    pub abs: (f32, f32),
    /// Style opacity (default 1.0).
    pub style_opacity: f32,
    /// `AnimProp::Opacity` override; wins over `style_opacity`.
    pub anim_opacity: Option<f32>,
    /// Author `transform:` chain from the style, resolved at paint.
    pub author_transform: Vec<Transform>,
    /// Animated transform slots.
    pub anim: AnimTransform,
    /// Sibling z-order (`AnimProp::ZIndex`); ties keep insertion order.
    pub z: f32,
    /// `overflow: hidden` → children clipped to the node's (rounded) box.
    pub overflow_hidden: bool,
    /// Win32 control id (`WM_COMMAND` routing) for control nodes.
    pub control_id: Option<u16>,
    /// Portal-hidden (navigation off a portal's screen without
    /// teardown): the painter and hit tester skip the whole subtree.
    pub hidden: bool,
}

impl NodeMeta {
    pub(crate) fn new(kind: NodeKind) -> Self {
        NodeMeta {
            kind,
            frame: (0.0, 0.0, 0.0, 0.0),
            abs: (0.0, 0.0),
            style_opacity: 1.0,
            anim_opacity: None,
            author_transform: Vec::new(),
            anim: AnimTransform::default(),
            z: 0.0,
            overflow_hidden: false,
            control_id: None,
            hidden: false,
        }
    }

    pub(crate) fn effective_opacity(&self) -> f32 {
        self.anim_opacity.unwrap_or(self.style_opacity).clamp(0.0, 1.0)
    }

    /// Whether this node bounds its children to its own box — for the
    /// painter's clip AND for hit testing (content outside the box must
    /// be neither visible nor clickable). `overflow: hidden` by style;
    /// scroll views ALWAYS, regardless of style: a scroll container that
    /// doesn't clip paints its scrolled-out content over the rest of the
    /// scene (the sidebar-links-over-the-header bug).
    pub(crate) fn clips_children(&self) -> bool {
        self.overflow_hidden
            || matches!(&self.kind, NodeKind::View(v) if v.scroll.is_some())
    }

    fn hwnd(&self) -> Option<HWND> {
        match &self.kind {
            NodeKind::Control { hwnd } | NodeKind::External { hwnd } => Some(*hwnd),
            _ => None,
        }
    }
}

/// A registered `WM_COMMAND` handler plus the notification code that
/// should trigger it. Buttons/checkboxes fire on `BN_CLICKED` (0); edit
/// controls fire on `EN_CHANGE`. Filtering on the code stops an edit's
/// focus/update notifications (delivered as `WM_COMMAND` under the same
/// control id) from spuriously firing its `on_change`.
struct CommandEntry {
    code: u16,
    action: Rc<dyn Fn()>,
}

/// A registered asset resolved to the form the image loader needs.
enum AssetEntry {
    /// Build-tool-resolved local file path (`AssetSource::Bundled`).
    File(String),
    /// Compile-time-embedded bytes + extension.
    Bytes { bytes: &'static [u8], ext: String },
    /// Runtime URL — unsupported by the native image loader.
    Remote,
}

// =========================================================================
// Backend
// =========================================================================

pub struct WindowsBackend {
    /// Host HWND every control hangs off of and whose client area the
    /// scene paints. Owned by the host shell.
    host_hwnd: HWND,
    /// Weak self-reference so node handles (animation writes) can reach
    /// back into the shared `Rc<RefCell<Self>>`. Installed by the host
    /// via [`Self::set_self_ref`] right after construction.
    self_ref: Weak<RefCell<WindowsBackend>>,
    next_id: u64,
    pub(crate) nodes: HashMap<u64, NodeMeta>,
    /// Parallel Taffy layout tree — same model as every other backend.
    pub(crate) layout: LayoutTree,
    layout_for_id: HashMap<u64, runtime_layout::LayoutNode>,
    /// Root node id, stashed by `finish` so `relayout` (window resize)
    /// and the painter can re-enter without the walker.
    pub(crate) root_id: Option<u64>,
    /// Next available Win32 control id (100.. — below is reserved).
    next_control_id: u16,
    command_handlers: HashMap<u16, CommandEntry>,
    /// parent node id → child node ids, in insertion order.
    pub(crate) children: HashMap<u64, Vec<u64>>,
    /// Live `Element::Graphics` nodes: surface + author callbacks +
    /// last reported size. See `graphics.rs` for the dispatch rules.
    graphics: HashMap<u64, graphics::GraphicsState>,
    /// DirectComposition device/target/root over the host window —
    /// graphics surfaces are visuals in this tree (see `dcomp.rs`).
    /// Lazily created by the first `create_graphics`; `None` before
    /// that or if DComp init failed.
    comp: Option<dcomp::CompositionTree>,
    /// Last window region applied per child HWND (key: hwnd as isize)
    /// so `layout_pass` only calls `SetWindowRgn` on actual changes —
    /// every set forces a repaint of the child.
    hwnd_regions: HashMap<isize, Option<RegionSpec>>,
    /// trackbar HWND (as isize) → its `on_change` fire closure.
    slider_handlers: HashMap<isize, Rc<dyn Fn()>>,
    /// Registered image/font assets keyed by `AssetId`.
    assets: HashMap<u64, AssetEntry>,
    pub(crate) external_handlers: runtime_core::ExternalRegistry<WindowsBackend>,
    /// Navigator handler factories keyed by presentation `TypeId`,
    /// populated via [`RegisterNavigator`] (e.g.
    /// `swap_navigator::register_generic`). `create_navigator`
    /// instantiates one per mounted navigator element.
    navigator_handlers: NavigatorRegistry<WindowsBackend>,
    /// Live navigator handler instances keyed by their container node
    /// id, so `navigator_attach_initial` / slot-style / release calls
    /// reach the same handler `create_navigator` built.
    nav_handlers: HashMap<u64, Rc<RefCell<Box<dyn NavigatorHandler<WindowsBackend>>>>>,
    /// The shell message font (Segoe UI) applied to native controls and
    /// used when a text style names no typography.
    ui_font: HFONT,
    /// Single-line height of `ui_font` in px.
    line_height: i32,
    /// Font cache + the key describing the shell font (painted text with
    /// no style typography draws with this).
    pub(crate) font_cache: font::FontCache,
    pub(crate) default_font_key: font::FontKey,
    /// TypefaceIds already installed into the process font table.
    installed_typefaces: HashSet<u64>,
    /// App background behind everything (`set_app_background`); the
    /// scene clears to it. White default.
    pub(crate) app_background: Option<Rgba>,
    /// Retained double-buffer target for the scene painter.
    pub(crate) back: scene::BackBuffer,
    /// Set by any structural mutation (insert / clear / style /
    /// intrinsic-size change) AFTER the mount-time `finish`; consumed
    /// at paint time by re-running the layout pass. This is what keeps
    /// post-mount tree edits laid out — a navigator swapping a screen
    /// into its outlet, a reactive `when` toggling a branch, text
    /// growing. (The web backend gets this from the DOM's own layout;
    /// GTK from its per-frame allocate loop.) Animated transform /
    /// color writes deliberately do NOT set it — they're paint-only.
    pub(crate) layout_dirty: bool,
}

impl WindowsBackend {
    /// Construct a backend rooted at `host_hwnd`. The host shell owns
    /// the window; the backend paints its client area and creates child
    /// controls underneath.
    pub fn new(host_hwnd: HWND) -> Self {
        ensure_gdiplus();
        let (ui_font, line_height, base_size, base_family) = create_ui_font();
        let default_font_key = font::FontKey {
            family: base_family,
            size_px: base_size,
            weight: 400,
            italic: false,
        };
        Self {
            host_hwnd,
            self_ref: Weak::new(),
            next_id: 1,
            nodes: HashMap::new(),
            layout: LayoutTree::new(),
            layout_for_id: HashMap::new(),
            root_id: None,
            next_control_id: 100,
            command_handlers: HashMap::new(),
            children: HashMap::new(),
            graphics: HashMap::new(),
            comp: None,
            hwnd_regions: HashMap::new(),
            slider_handlers: HashMap::new(),
            assets: HashMap::new(),
            external_handlers: runtime_core::ExternalRegistry::new(),
            navigator_handlers: NavigatorRegistry::new(),
            nav_handlers: HashMap::new(),
            ui_font,
            line_height,
            font_cache: font::FontCache::new(),
            default_font_key,
            installed_typefaces: HashSet::new(),
            app_background: None,
            back: scene::BackBuffer::new(),
            layout_dirty: false,
        }
    }

    /// Install the backend's weak self-reference. The host calls this
    /// immediately after wrapping the backend in `Rc<RefCell<..>>`, so
    /// node handles built during mount can reach back into it.
    pub fn set_self_ref(&mut self, me: Weak<RefCell<WindowsBackend>>) {
        self.self_ref = me;
    }

    pub(crate) fn self_ref(&self) -> Weak<RefCell<WindowsBackend>> {
        self.self_ref.clone()
    }

    /// Borrow the host HWND (SDK extensions reach the window here).
    pub fn host_hwnd(&self) -> HWND {
        self.host_hwnd
    }

    /// SDK extension point (the painted-scene analogue of Linux's
    /// `register_external_view`): ONE painted leaf that draws
    /// pre-tokenized `(text, css-color)` runs in the platform
    /// monospace font, line structure preserved, no wrapping — the
    /// `codeblock` SDK's single-node contract. The runs are measured
    /// once here; the intrinsic size is (longest line, line count ×
    /// line height). Box styling (background, radius, padding)
    /// belongs on a wrapping view the caller styles.
    pub fn create_colored_code_leaf(
        &mut self,
        spans: &[(String, runtime_core::Color)],
    ) -> WindowsNode {
        // Cascadia Mono ships on Win11, Consolas everywhere back to
        // Vista; probe like any CSS stack so the key is drawable.
        let family = font::resolve_family_stack(
            "Cascadia Mono, Consolas, Courier New, monospace",
            &self.default_font_key.family,
        );
        // 13px matches the sibling handlers' 13pt mono size.
        let key = font::FontKey { family, size_px: 13, weight: 400, italic: false };
        let hfont = font::entry_for(&mut self.font_cache, &key)
            .map(|e| e.hfont)
            .unwrap_or(self.ui_font);
        let (visual, (w, h)) =
            code::build_gdi(spans, hfont, key, self.line_height as f32);
        let node = self.add_node(NodeKind::Code(visual));
        self.set_intrinsic(&node, w, h);
        node
    }

    /// Request a repaint of the scene. Cheap — Windows coalesces
    /// invalidations into one `WM_PAINT` per message-loop pass.
    pub(crate) fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(self.host_hwnd, None, false);
        }
    }

    /// Paint the scene into `hdc` at `w × h`. The host calls this from
    /// its `WM_PAINT`.
    pub fn paint(&mut self, hdc: HDC, w: i32, h: i32) {
        unsafe {
            scene::paint_scene(self, hdc, w, h);
        }
    }

    /// The pressable handler under client-space `(x, y)`, if any. The
    /// host fires it AFTER releasing the backend borrow (the handler
    /// writes signals whose effects re-borrow the backend).
    pub fn pressable_action(&self, x: f32, y: f32) -> Option<Rc<dyn Fn()>> {
        scene::pressable_at(self, x, y)
    }

    /// Route a wheel tick (`delta_px` > 0 scrolls content down/right) to
    /// the scroll view under `(x, y)`. Returns true when consumed.
    pub fn wheel_scroll(&mut self, x: f32, y: f32, delta_px: f32) -> bool {
        let handled = scene::scroll_at(self, x, y, delta_px);
        if handled {
            // Native children (composition visuals, control HWNDs)
            // don't ride the painter's scroll translate — reposition
            // them to the new offsets or they stay pinned while
            // everything painted scrolls past.
            self.position_native_children();
            self.invalidate();
            // NOTE: the synchronous catch-up paint (UpdateWindow) lives
            // in the HOST's wheel handler, after this borrow is
            // released — UpdateWindow dispatches WM_PAINT inline, and
            // paint re-borrows the backend; calling it from here (still
            // inside the host's `borrow_mut`) is a guaranteed RefCell
            // double-borrow panic on every wheel tick.
        }
        handled
    }

    /// Register a handler for the third-party external primitive whose
    /// payload type is `T`. Mirrors the iOS / macOS pattern.
    pub fn register_external<T, F>(&mut self, handler: F)
    where
        T: 'static,
        F: Fn(&Rc<T>, &mut WindowsBackend) -> WindowsNode + 'static,
    {
        self.external_handlers.register::<T, _>(handler);
    }

    /// `true` if a handler for payload type `T` has been registered.
    pub fn has_external<T: 'static>(&self) -> bool {
        self.external_handlers.has::<T>()
    }

    /// SDK extension helper: allocate a fresh Win32 control id + install
    /// `on_click` as its WM_COMMAND handler.
    pub fn register_command_handler(&mut self, on_click: Rc<dyn Fn()>) -> u16 {
        let id = self.alloc_control_id();
        self.command_handlers
            .insert(id, CommandEntry { code: BN_CLICKED, action: on_click });
        id
    }

    /// Install a `WM_COMMAND` handler under a caller-supplied id (SDK
    /// leaves allocating out of their own id namespace).
    pub fn install_command_handler_with_id(&mut self, id: u16, on_click: Rc<dyn Fn()>) {
        self.command_handlers
            .insert(id, CommandEntry { code: BN_CLICKED, action: on_click });
    }

    /// SDK extension helper: register an externally-created HWND with
    /// the backend's layout tree so flex parents can size + position it.
    pub fn register_external_view(&mut self, hwnd: HWND) -> WindowsNode {
        let id = self.alloc_id();
        let layout = self.layout.new_node();
        self.layout_for_id.insert(id, layout);
        self.nodes.insert(id, NodeMeta::new(NodeKind::External { hwnd }));
        WindowsNode { id, hwnd }
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Allocate the next Win32 control id. Wraps at `u16::MAX` with a
    /// visible warning (collision would be silent otherwise).
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

    /// Fire the click handler registered for `control_id`; `true` if a
    /// handler was found.
    pub fn dispatch_command(&self, control_id: u16) -> bool {
        if let Some(entry) = self.command_handlers.get(&control_id) {
            (entry.action)();
            return true;
        }
        false
    }

    /// Clone out the handler for `control_id` **iff** the WM_COMMAND
    /// notification `code` matches the code that control cares about.
    /// The host fires the returned closure after releasing the borrow.
    pub fn command_action(&self, control_id: u16, code: u16) -> Option<Rc<dyn Fn()>> {
        self.command_handlers
            .get(&control_id)
            .filter(|e| e.code == code)
            .map(|e| e.action.clone())
    }

    /// Clone out the `on_change` fire closure for the trackbar whose
    /// HWND is `hwnd` (host `WM_HSCROLL` routing).
    pub fn slider_action(&self, hwnd: HWND) -> Option<Rc<dyn Fn()>> {
        self.slider_handlers.get(&(hwnd.0 as isize)).cloned()
    }

    /// Re-run the layout pass against the current client rect. The host
    /// calls this on `WM_SIZE`.
    pub fn relayout(&mut self) {
        self.layout_pass();
        self.invalidate();
    }

    /// A node's parent-relative Taffy frame, for handle `frame()` reads
    /// (welcome's orbit math reads the page's viewport size).
    pub(crate) fn node_frame(&self, id: u64) -> Option<(f32, f32, f32, f32)> {
        self.nodes.get(&id).map(|m| m.frame)
    }

    /// A node's window-relative frame.
    pub(crate) fn node_abs_frame(&self, id: u64) -> Option<(f32, f32, f32, f32)> {
        self.nodes
            .get(&id)
            .map(|m| (m.abs.0, m.abs.1, m.frame.2, m.frame.3))
    }

    /// Create a native child control HWND parented under the host.
    /// `control_id` rides in the `hMenu` slot (Win32 reinterprets it as
    /// the child id reported by `WM_COMMAND`).
    fn create_control_hwnd(
        &mut self,
        class_name: PCWSTR,
        text: &str,
        style: u32,
        control_id: Option<u16>,
    ) -> HWND {
        let text_wide = to_pcwstr(text);
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
                self.host_hwnd,
                hmenu,
                None,
                None,
            )
        }
        .unwrap_or(HWND(std::ptr::null_mut()));
        let _ = unsafe { ShowWindow(hwnd, SW_SHOW) };
        // Modern shell font — without it controls render in the legacy
        // bitmap "System" font.
        if !hwnd.is_invalid() && !self.ui_font.is_invalid() {
            unsafe {
                SendMessageW(hwnd, WM_SETFONT, WPARAM(self.ui_font.0 as usize), LPARAM(1));
            }
        }
        hwnd
    }

    /// Allocate a node + layout slot for `kind`.
    fn add_node(&mut self, kind: NodeKind) -> WindowsNode {
        let hwnd = match &kind {
            NodeKind::Control { hwnd } | NodeKind::External { hwnd } => *hwnd,
            _ => HWND(std::ptr::null_mut()),
        };
        let id = self.alloc_id();
        let layout = self.layout.new_node();
        self.layout_for_id.insert(id, layout);
        self.nodes.insert(id, NodeMeta::new(kind));
        WindowsNode { id, hwnd }
    }

    /// Painted-text placeholder for unimplemented primitives — visible
    /// at runtime rather than a silent gap (backend-cpu posture).
    fn placeholder(&mut self, message: &str) -> WindowsNode {
        let node = self.add_node(NodeKind::Text(TextVisual::new(message)));
        self.set_text_measure(node.id);
        node
    }

    /// Measure `text` in the font `key` names (shell font when `None`).
    /// Text must be measured in the font it is DRAWN in — measuring a
    /// 56 px headline in the 12 px shell font lays it out ~4× too
    /// narrow and the layout clips it.
    fn measure_with_key(&mut self, text: &str, key: Option<&font::FontKey>) -> (i32, i32) {
        let hfont = match key {
            Some(k) => font::entry_for(&mut self.font_cache, k)
                .map(|e| e.hfont)
                .unwrap_or(self.ui_font),
            None => self.ui_font,
        };
        measure_text_gdi(text, hfont, self.line_height)
    }

    /// (Re)build a painted text node's wrap plan and install its Taffy
    /// measure fn. This is the text-sizing path — `set_intrinsic` is
    /// wrong for text because it writes `min_size`, which pins the box
    /// to its single-line extent and makes wrapping impossible (the
    /// "paragraphs run off the window edge" website bug). Call after
    /// any content, font, or line-height change.
    fn set_text_measure(&mut self, node_id: u64) {
        let Some(meta) = self.nodes.get(&node_id) else {
            return;
        };
        let NodeKind::Text(t) = &meta.kind else {
            return;
        };
        let content = t.content.clone();
        let key = t.font_key.clone();
        let style_advance = t.line_height;
        let hfont = match &key {
            Some(k) => font::entry_for(&mut self.font_cache, k)
                .map(|e| e.hfont)
                .unwrap_or(self.ui_font),
            None => self.ui_font,
        };
        let plan = Rc::new(wrap::build_gdi(&content, hfont, self.line_height as f32));
        if let Some(meta) = self.nodes.get_mut(&node_id) {
            if let NodeKind::Text(t) = &mut meta.kind {
                t.plan = Some(plan.clone());
                t.lines = None;
            }
        }
        if let Some(layout) = self.layout_for_id.get(&node_id).copied() {
            let advance = style_advance.unwrap_or(plan.font_height).max(1.0);
            self.layout.set_measure_fn(
                layout,
                Rc::new(move |known, avail| wrap::measure_size(&plan, advance, known, avail)),
            );
        }
        self.layout_dirty = true;
    }

    /// Apply (or clear) a child HWND's window region, diffed against
    /// the last applied spec — `SetWindowRgn` forces a child repaint,
    /// so identical re-applies on every layout pass would flicker.
    /// The system takes ownership of a set region (never delete it).
    fn apply_hwnd_region(&mut self, hwnd: HWND, region: Option<RegionSpec>) {
        use windows::Win32::Graphics::Gdi::{CreateRectRgn, CreateRoundRectRgn};
        let key = hwnd.0 as isize;
        if self.hwnd_regions.get(&key) == Some(&region) {
            return;
        }
        self.hwnd_regions.insert(key, region);
        unsafe {
            match region {
                Some((x0, y0, x1, y1, ellipse)) => {
                    let rgn = if ellipse > 0 {
                        CreateRoundRectRgn(x0, y0, x1, y1, ellipse, ellipse)
                    } else {
                        CreateRectRgn(x0, y0, x1, y1)
                    };
                    let _ = SetWindowRgn(hwnd, rgn, true);
                }
                None => {
                    let _ = SetWindowRgn(hwnd, windows::Win32::Graphics::Gdi::HRGN::default(), true);
                }
            }
        }
    }

    /// Record a leaf's intrinsic pixel size on its layout node.
    /// Controls only (button/edit/…) — text nodes use
    /// `set_text_measure` (this writes `min_size`, which forbids wrap).
    fn set_intrinsic(&mut self, node: &WindowsNode, width: i32, height: i32) {
        if let Some(layout) = self.layout_for_id.get(&node.id).copied() {
            self.layout
                .set_intrinsic_size(layout, width as f32, height as f32);
        }
        self.layout_dirty = true;
    }

    /// Resolve a text style's typography to a cached font key, or `None`
    /// when the style sets no font properties (shell font).
    fn font_key_for_style(&mut self, style: &StyleRules) -> Option<font::FontKey> {
        if style.font_size.is_none() && style.font_weight.is_none() && style.font_family.is_none()
        {
            return None;
        }
        let size_px = match style.font_size.as_ref().map(|t| t.resolve()) {
            Some(Length::Px(v)) if v >= 1.0 => v.round() as i32,
            _ => self.default_font_key.size_px,
        };
        let weight = font::weight_to_gdi(style.font_weight.unwrap_or_default());
        let family = match &style.font_family {
            // May be a CSS fallback STACK — resolve to a family GDI+
            // can actually draw, or text paints as invisible (see
            // `resolve_family_stack`).
            Some(runtime_core::FontFamily::System(name)) => {
                font::resolve_family_stack(name, &self.default_font_key.family)
            }
            // Registered typefaces are process-private (AddFontMemResourceEx)
            // and invisible to GDI+'s system collection — do NOT probe
            // them, trust the registered name.
            Some(runtime_core::FontFamily::Typeface(tf)) => tf.family_name.to_string(),
            None => self.default_font_key.family.clone(),
        };
        Some(font::FontKey { family, size_px, weight, italic: false })
    }

    /// Recursively drop node `id` and every descendant from the maps +
    /// layout tree, destroying any native control HWNDs.
    fn remove_subtree(&mut self, id: u64) {
        if let Some(kids) = self.children.remove(&id) {
            for k in kids {
                self.remove_subtree(k);
            }
        }
        // A navigator node in the removed subtree drops its live handler.
        self.nav_handlers.remove(&id);
        // A graphics node fires `on_lost` (deferred — remove can run
        // inside author effects that already borrow the backend) so
        // the author drops every wgpu object built on the visual. The
        // visual detaches from the composition tree here; the COM
        // object itself stays alive until wgpu's own reference drops
        // (a detached visual composes nothing, so presenting to it in
        // the interim is harmless).
        if let Some(mut st) = self.graphics.remove(&id) {
            st.visible
                .store(false, std::sync::atomic::Ordering::Relaxed);
            if let Some(comp) = &self.comp {
                comp.remove_visual(&st.visuals);
                comp.commit();
            }
            if let Some(mut lost) = st.on_lost.take() {
                runtime_core::scheduling::after_ms_detached(0, move || lost());
            }
        }
        if let Some(meta) = self.nodes.remove(&id) {
            if let Some(cid) = meta.control_id {
                self.command_handlers.remove(&cid);
            }
            if let Some(hwnd) = meta.hwnd() {
                self.slider_handlers.remove(&(hwnd.0 as isize));
                self.hwnd_regions.remove(&(hwnd.0 as isize));
                if !hwnd.is_invalid() {
                    unsafe {
                        let _ = DestroyWindow(hwnd);
                    }
                }
            }
        }
        if let Some(layout) = self.layout_for_id.remove(&id) {
            self.layout.remove_node(layout);
        }
    }

    /// The shared layout pass: run Taffy against the host client rect,
    /// store every node's parent-relative + window-relative frame, and
    /// position native control HWNDs. Painted nodes are drawn from the
    /// stored frames on the next `WM_PAINT`.
    pub(crate) fn layout_pass(&mut self) {
        self.layout_dirty = false;
        let Some(root_id) = self.root_id else {
            return;
        };
        let Some(root_layout) = self.layout_for_id.get(&root_id).copied() else {
            return;
        };
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

        // Parent-relative frames for every laid-out node.
        let ids: Vec<u64> = self.layout_for_id.keys().copied().collect();
        for id in ids {
            let Some(layout) = self.layout_for_id.get(&id).copied() else {
                continue;
            };
            let frame = self.layout.frame_of(layout);
            if let Some(meta) = self.nodes.get_mut(&id) {
                meta.frame = (frame.x, frame.y, frame.width, frame.height);
                // Break text lines at the final frame width so paint
                // (read-only) never re-wraps. Same pure `lines_at` the
                // measure fn used → breaks can't disagree with the
                // height Taffy was told.
                if let NodeKind::Text(t) = &mut meta.kind {
                    if let Some(plan) = t.plan.clone() {
                        let stale = t
                            .lines
                            .as_ref()
                            .map(|l| (l.width - frame.width).abs() > 0.25)
                            .unwrap_or(true);
                        if stale {
                            t.lines = Some(wrap::WrappedLines {
                                width: frame.width,
                                lines: plan.lines_at(frame.width),
                            });
                        }
                    }
                }
            }
        }

        self.position_native_children();

        // Graphics surfaces: decide which nodes owe an `on_ready` /
        // `on_resize` now that frames are current, and dispatch on a
        // fresh scheduler turn. Author callbacks must NOT run here —
        // `layout_pass` executes while the host holds the backend's
        // RefCell borrow (paint / resize), and `on_ready` runs
        // arbitrary author code (the Simulator blocks on the whole
        // wgpu mount) that may re-enter backend methods.
        let mut gfx_events: Vec<(u64, graphics::GfxEvent)> = Vec::new();
        for (id, st) in self.graphics.iter() {
            let Some(meta) = self.nodes.get(id) else {
                continue;
            };
            if meta.hidden {
                continue;
            }
            let ev = graphics::layout_event(
                st.on_ready.is_none(),
                st.last_size,
                (meta.frame.2, meta.frame.3),
            );
            if let Some(ev) = ev {
                gfx_events.push((*id, ev));
            }
        }
        if !gfx_events.is_empty() {
            let weak = self.self_ref.clone();
            runtime_core::scheduling::after_ms_detached(0, move || {
                dispatch_graphics_events(&weak, &gfx_events);
            });
        }
    }

    /// Recompute window-relative origins and reposition + re-clip every
    /// native child: control HWNDs (buttons, edits) AND graphics
    /// composition visuals (the wgpu canvas). Called from
    /// `layout_pass`, and on EVERY scroll-offset change: painted
    /// children shift via the painter's scroll translate, but native
    /// children only move when repositioned here — without this,
    /// scrolling left them pinned in place. Each stack entry carries
    /// the accumulated clip from clipping ancestors
    /// (`(abs rect, corner radius)`) — realized as window regions for
    /// HWNDs and as composition clips for visuals — plus the inherited
    /// portal-hidden flag.
    pub(crate) fn position_native_children(&mut self) {
        let Some(root_id) = self.root_id else {
            return;
        };
        let mut stack: Vec<(u64, f32, f32, dcomp::ClipChain, bool)> =
            vec![(root_id, 0.0, 0.0, dcomp::ClipChain::default(), false)];
        let mut control_moves: Vec<(HWND, i32, i32, i32, i32, Option<RegionSpec>)> = Vec::new();
        // Graphics visuals collected during the walk, applied after it
        // (placement diff + one batched commit).
        let mut gfx_moves: Vec<(u64, (f32, f32, f32, f32), dcomp::ClipChain, bool)> = Vec::new();
        while let Some((id, ox, oy, clip, hidden_above)) = stack.pop() {
            let is_gfx = self.graphics.contains_key(&id);
            let Some(meta) = self.nodes.get_mut(&id) else {
                continue;
            };
            // Portal-hidden state inherits down the walk: HWND children
            // are ShowWindow-hidden individually, but a graphics VISUAL
            // needs the aggregate flag to blank its clip + visibility
            // vote (see `dcomp::visual_placement`).
            let hidden = hidden_above || meta.hidden;
            let (fx, fy, w, h) = meta.frame;
            let (ax, ay) = (ox + fx, oy + fy);
            meta.abs = (ax, ay);
            if is_gfx {
                gfx_moves.push((id, (ax, ay, w, h), clip, hidden));
            } else if let Some(hwnd) = meta.hwnd() {
                // Painted-scene clips (`overflow: hidden`, scroll
                // boxes, rounded wrappers) can't touch a native child
                // HWND — express the nearest clipping ancestor as a
                // WINDOW REGION instead, so a control inside a scroll
                // view stops at the viewport edge. Regions can only
                // hold ONE shape, so controls get the chain's
                // single-clip approximation; graphics visuals get the
                // full two-channel chain.
                let region = hwnd_clip_region((ax, ay, w, h), clip.legacy());
                control_moves.push((
                    hwnd,
                    ax.round() as i32,
                    ay.round() as i32,
                    w.round() as i32,
                    h.round() as i32,
                    region,
                ));
            }
            let (mut cox, mut coy) = (ax, ay);
            if let NodeKind::View(v) = &meta.kind {
                if let Some(s) = &v.scroll {
                    cox -= s.offset_x;
                    coy -= s.offset_y;
                }
            }
            // A clipping node bounds its subtree — folded into the
            // chain's square or rounded channel by its (max) radius;
            // see `dcomp::ClipChain::push` for the semantics.
            let child_clip = if meta.clips_children() {
                let radius = match &meta.kind {
                    NodeKind::View(v) => v.radii.iter().cloned().fold(0.0_f32, f32::max),
                    _ => 0.0,
                };
                clip.push((ax, ay, ax + w, ay + h), radius)
            } else {
                clip
            };
            if let Some(kids) = self.children.get(&id) {
                for k in kids {
                    stack.push((*k, cox, coy, child_clip, hidden));
                }
            }
        }
        // Apply graphics-visual placements: offset + clip per changed
        // node, then ONE device commit — the DWM picks the whole batch
        // up atomically with its next composition frame, which is what
        // keeps the canvas glued to the painted scene during scrolling.
        let mut comp_dirty = false;
        for (id, abs, clip, hidden) in gfx_moves {
            let Some(st) = self.graphics.get_mut(&id) else {
                continue;
            };
            st.visible
                .store(!hidden, std::sync::atomic::Ordering::Relaxed);
            let placement = dcomp::visual_placement(abs, clip, hidden);
            if st.last_placement == Some(placement) {
                continue;
            }
            st.last_placement = Some(placement);
            if let Some(comp) = &self.comp {
                comp.apply_placement(&st.visuals, &placement);
                comp_dirty = true;
            }
        }
        if comp_dirty {
            if let Some(comp) = &self.comp {
                comp.commit();
            }
        }
        // Batch every move into one `DeferWindowPos` transaction: the
        // system repositions all children in one pass instead of N
        // separate reposition/compose cycles — visibly smoother when a
        // scroll tick moves a wgpu canvas plus a column of controls.
        let moves: Vec<_> = control_moves
            .iter()
            .filter(|(hwnd, ..)| !hwnd.is_invalid())
            .collect();
        if !moves.is_empty() {
            unsafe {
                let mut hdwp = BeginDeferWindowPos(moves.len() as i32).unwrap_or_default();
                if !hdwp.is_invalid() {
                    for (hwnd, x, y, w, h, _) in &moves {
                        match DeferWindowPos(
                            hdwp,
                            *hwnd,
                            None,
                            *x,
                            *y,
                            *w,
                            *h,
                            SWP_NOZORDER | SWP_NOACTIVATE,
                        ) {
                            Ok(next) => hdwp = next,
                            Err(_) => break,
                        }
                    }
                    let _ = EndDeferWindowPos(hdwp);
                }
            }
        }
        for (hwnd, _, _, _, _, region) in control_moves {
            if hwnd.is_invalid() {
                continue;
            }
            self.apply_hwnd_region(hwnd, region);
        }
    }

    /// The graphics-event dispatcher's phase 1: under a short borrow,
    /// re-read the node's CURRENT frame (layout may have moved on since
    /// the event was queued), record it as the dedupe baseline, and
    /// take the callback box out of the state. Returns what phase 2
    /// should call with the backend released.
    fn take_graphics_callback(&mut self, id: u64, ev: graphics::GfxEvent) -> Option<TakenGfx> {
        let meta = self.nodes.get(&id)?;
        let w = meta.frame.2.round().max(0.0) as u32;
        let h = meta.frame.3.round().max(0.0) as u32;
        if w <= 1 || h <= 1 {
            return None;
        }
        let st = self.graphics.get_mut(&id)?;
        st.last_size = Some((w, h));
        match ev {
            graphics::GfxEvent::Ready => st
                .on_ready
                .take()
                .map(|cb| TakenGfx::Ready(cb, st.surface.clone(), (w, h))),
            graphics::GfxEvent::Resize => {
                st.on_resize.take().map(|cb| TakenGfx::Resize(cb, (w, h)))
            }
        }
    }

    /// Resolve an image `src` to a decoded GDI+ bitmap. Handles
    /// `asset://{id}`, `data:` base64 URLs, `file://` + bare local
    /// paths; `None` for remote `http(s)://` (no native fetch) or any
    /// decode failure — rendered as a blank box.
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
            // `file:///C:/x.png` → strip the leading slash(es) before
            // the drive letter so GDI+ gets a plain Windows path.
            return image::load_image_file(path.trim_start_matches('/'));
        }
        if src.starts_with("http://") || src.starts_with("https://") {
            return None;
        }
        image::load_image_file(src)
    }
}

/// Window-region recipe for a child HWND, in the child's LOCAL
/// coordinates: `(left, top, right, bottom, corner-ellipse-diameter)`.
/// `(0,0,0,0,0)` = fully clipped (empty region hides the child's
/// pixels); ellipse `0` = plain rectangle.
type RegionSpec = (i32, i32, i32, i32, i32);

/// Rect ∩ rect, both `(x, y, w, h)`; empty results collapse to zero
/// size at the intersection origin.
fn intersect_rects(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    let x0 = a.0.max(b.0);
    let y0 = a.1.max(b.1);
    let x1 = (a.0 + a.2).min(b.0 + b.2);
    let y1 = (a.1 + a.3).min(b.1 + b.3);
    (x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
}

/// The window region a native child HWND needs so its nearest
/// clipping ancestor's bounds — and rounded corners — apply to it.
/// `None` = no region needed (no clipping ancestor, or the child sits
/// fully inside a square-cornered clip). The rounded case always gets
/// a region even at full coverage: the corners themselves are what
/// need cutting. (Graphics surfaces use `dcomp::visual_placement`
/// instead — composition clips, antialiased and repaint-free.)
fn hwnd_clip_region(
    child: (f32, f32, f32, f32),
    clip: Option<((f32, f32, f32, f32), f32)>,
) -> Option<RegionSpec> {
    let (rect, radius) = clip?;
    let (ix, iy, iw, ih) = intersect_rects(child, rect);
    if iw <= 0.0 || ih <= 0.0 {
        return Some((0, 0, 0, 0, 0));
    }
    let full = ix <= child.0 + 0.5
        && iy <= child.1 + 0.5
        && ix + iw >= child.0 + child.2 - 0.5
        && iy + ih >= child.1 + child.3 - 0.5;
    if full && radius < 0.5 {
        return None;
    }
    Some((
        (ix - child.0).round() as i32,
        (iy - child.1).round() as i32,
        (ix + iw - child.0).round() as i32,
        (iy + ih - child.1).round() as i32,
        (radius * 2.0).round() as i32,
    ))
}

/// A graphics callback pulled out of the backend for a borrow-free call.
enum TakenGfx {
    Ready(
        runtime_core::primitives::graphics::OnReady,
        runtime_core::primitives::graphics::GraphicsSurface,
        (u32, u32),
    ),
    Resize(runtime_core::primitives::graphics::OnResize, (u32, u32)),
}

/// Deliver deferred graphics events (queued by `layout_pass`). Runs on
/// its own scheduler turn with NO outstanding backend borrow; each
/// author callback executes with the backend fully released — the
/// Simulator's `on_ready` blocks on the entire wgpu mount and its
/// embedded app's effects may re-enter backend methods. `on_resize`'s
/// `FnMut` box is restored afterward; `on_ready` is once-only (the
/// composition visual is stable for the node's lifetime — no
/// Android-style surface recreation on this backend).
///
/// `scale` is 1.0 — the Win32 host is DPI-unaware today; `1.0` is the
/// documented "not yet reported" value.
fn dispatch_graphics_events(
    weak: &Weak<RefCell<WindowsBackend>>,
    events: &[(u64, graphics::GfxEvent)],
) {
    use runtime_core::primitives::graphics::{GraphicsTarget, OnReadyEvent, OnResizeEvent};
    for &(id, ev) in events {
        let Some(backend) = weak.upgrade() else {
            return;
        };
        let taken = backend.borrow_mut().take_graphics_callback(id, ev);
        match taken {
            Some(TakenGfx::Ready(mut cb, surface, size)) => {
                cb(OnReadyEvent {
                    target: GraphicsTarget::RawWindow(surface),
                    size,
                    scale: 1.0,
                });
            }
            Some(TakenGfx::Resize(mut cb, size)) => {
                cb(OnResizeEvent { size, scale: 1.0 });
                if let Some(st) = backend.borrow_mut().graphics.get_mut(&id) {
                    st.on_resize = Some(cb);
                }
            }
            None => {}
        }
    }
}

/// Measure a single line of `text` in `hfont` via GDI. `line_height` is
/// the fallback when measurement fails or the string is empty.
fn measure_text_gdi(text: &str, hfont: HFONT, line_height: i32) -> (i32, i32) {
    if text.is_empty() {
        return (0, line_height);
    }
    let wide: Vec<u16> = text.encode_utf16().collect();
    unsafe {
        let dc = GetDC(HWND(std::ptr::null_mut()));
        if dc.is_invalid() {
            return (0, line_height);
        }
        let prev = SelectObject(dc, HGDIOBJ(hfont.0));
        let mut size = SIZE::default();
        let ok = GetTextExtentPoint32W(dc, &wide, &mut size).as_bool();
        SelectObject(dc, prev);
        ReleaseDC(HWND(std::ptr::null_mut()), dc);
        if ok {
            (size.cx, size.cy)
        } else {
            (0, line_height)
        }
    }
}

/// Build the shell UI font (the message font — Segoe UI on modern
/// Windows), returning `(hfont, line_height, em_size, family_name)`.
fn create_ui_font() -> (HFONT, i32, i32, String) {
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
        // `lfHeight` is negative for an em height; take its magnitude
        // as the base font size.
        let base_size = ncm.lfMessageFont.lfHeight.abs().max(1);
        let face = &ncm.lfMessageFont.lfFaceName;
        let face_len = face.iter().position(|&c| c == 0).unwrap_or(face.len());
        let base_family = String::from_utf16_lossy(&face[..face_len]);
        let font = CreateFontIndirectW(&ncm.lfMessageFont);
        if font.is_invalid() {
            return (HFONT(std::ptr::null_mut()), FALLBACK.0, FALLBACK.1, base_family);
        }
        let mut line_height = 16;
        let dc = GetDC(HWND(std::ptr::null_mut()));
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
        // Destroy the control HWNDs we created (the host's window is not
        // ours to destroy).
        for (_, meta) in self.nodes.drain() {
            if let Some(hwnd) = meta.hwnd() {
                if !hwnd.is_invalid() {
                    let _ = unsafe { DestroyWindow(hwnd) };
                }
            }
        }
        // Release fonts + the back buffer.
        for (_, entry) in self.font_cache.drain() {
            if !entry.hfont.is_invalid() {
                let _ = unsafe { DeleteObject(HGDIOBJ(entry.hfont.0)) };
            }
            if !entry.gpfont.is_null() {
                let _ = unsafe { GdipDeleteFont(entry.gpfont) };
            }
        }
        if !self.ui_font.is_invalid() {
            let _ = unsafe { DeleteObject(HGDIOBJ(self.ui_font.0)) };
        }
        unsafe {
            self.back.release();
        }
    }
}

// =========================================================================
// Style resolution helpers
// =========================================================================

/// `Rgba` → straight sRGB floats `[r, g, b, a]` in 0..=1.
pub(crate) fn srgba_of(c: Rgba) -> [f32; 4] {
    [
        c.r as f32 / 255.0,
        c.g as f32 / 255.0,
        c.b as f32 / 255.0,
        c.a as f32 / 255.0,
    ]
}

/// Resolve a `StyleRules` color slot to canonical [`Rgba`], defaulting
/// to fully-transparent when unset/unparseable (an unset background
/// legitimately means "paint nothing").
fn resolve_color(t: &Option<runtime_core::Tokenized<Color>>) -> Option<Rgba> {
    t.as_ref()
        .map(|c| runtime_core::color::parse_or(&c.resolve().0, Rgba::TRANSPARENT))
}

/// Corner radius in px, or 0.
fn resolve_radius(t: &Option<runtime_core::Tokenized<Length>>) -> f32 {
    match t.as_ref().map(|x| x.resolve()) {
        Some(Length::Px(v)) => v,
        _ => 0.0,
    }
}

/// Border-width slot in px, or 0.
fn resolve_width(t: &Option<runtime_core::Tokenized<f32>>) -> f32 {
    t.as_ref().map(|x| x.resolve()).unwrap_or(0.0)
}

/// Resolve the framework's [`Gradient`] into a paint-ready
/// [`GradientPaint`] (shape + float sRGB stops — floats so animated
/// stop writes can overwrite them per frame).
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
            (s.offset, srgba_of(rgba))
        })
        .collect();
    GradientPaint { kind, stops }
}

/// Decide whether the four border sides collapse to a single uniform
/// stroke. Mirrors `backend_apple_core::border::uniform_border` (same
/// routing so Windows converges with the Apple backends per Rule #7).
/// A `None` side color falls back to the first side that carries one.
///
/// `Some((width, color))` → stroke one anti-aliased rounded path;
/// `None` → the asymmetric case, drawn as straight per-side bars.
pub(crate) fn uniform_border(sides: &[BorderSide; 4]) -> Option<(f32, Rgba)> {
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
/// `w × h` box. CSS angle convention: `0°` = bottom→top, `90°` =
/// left→right, `180°` = top→bottom. Identical formula to the Linux
/// backend's `gradient::linear_points` so the two backends place stops
/// identically.
pub(crate) fn linear_points(angle_deg: f32, w: f32, h: f32) -> ((f32, f32), (f32, f32)) {
    let rad = angle_deg.to_radians();
    let dx = rad.sin();
    let dy = -rad.cos();
    let half = (dx.abs() * w + dy.abs() * h) / 2.0;
    let (cx, cy) = (w / 2.0, h / 2.0);
    let start = (cx - dx * half, cy - dy * half);
    let end = (cx + dx * half, cy + dy * half);
    (start, end)
}

/// Pixel radius of a radial gradient's `offset 1.0` stop. Same formula
/// as the Linux backend's `gradient::radial_radius`.
pub(crate) fn radial_radius(center: (f32, f32), radius: f32, farthest: bool, w: f32, h: f32) -> f32 {
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

// =========================================================================
// SDK registration traits — the backend-neutral `register_generic` paths
// (`swap_navigator::register_generic`, `<sdk>::register`) resolve on
// WindowsBackend through these. Mirrors the Linux backend.
// =========================================================================

impl RegisterNavigator for WindowsBackend {
    fn register_navigator<P, F>(&mut self, factory: F)
    where
        P: 'static,
        F: Fn() -> Box<dyn NavigatorHandler<WindowsBackend>> + 'static,
    {
        self.navigator_handlers.register::<P, _>(factory);
    }
}

impl RegisterExternal for WindowsBackend {
    fn register_external<T, F>(&mut self, handler: F)
    where
        T: 'static,
        F: Fn(&Rc<T>, &mut Self) -> Self::Node + 'static,
    {
        self.external_handlers.register::<T, _>(handler);
    }
}

// =========================================================================
// Backend trait
// =========================================================================

impl Backend for WindowsBackend {
    type Node = WindowsNode;

    fn color_scheme(&self) -> ColorScheme {
        ColorScheme::Auto
    }

    fn platform(&self) -> Platform {
        Platform::Custom("windows")
    }

    fn create_view(&mut self, _a11y: &AccessibilityProps) -> Self::Node {
        self.add_node(NodeKind::View(ViewVisual::default()))
    }

    fn create_pressable(
        &mut self,
        on_click: Rc<dyn Fn()>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // A Pressable is a styleable painted container carrying an
        // on_click; the host's hit-tested WM_LBUTTONUP fires it.
        self.add_node(NodeKind::View(ViewVisual {
            on_click: Some(on_click),
            ..Default::default()
        }))
    }

    fn create_link(
        &mut self,
        config: runtime_core::primitives::link::LinkConfig,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // A Link is "a Pressable that navigates". The trait default
        // collapses to `create_view` and DROPS `on_activate`, so every
        // link renders as inert text and nothing can be navigated to —
        // the same bug the Linux + terminal backends fixed.
        // `config.url` is deliberately ignored (documented web-only);
        // `on_activate` already wraps in-app push/replace dispatch, and
        // `open_url` for `external` links.
        self.add_node(NodeKind::View(ViewVisual {
            on_click: Some(config.on_activate.clone()),
            ..Default::default()
        }))
    }

    fn create_text(&mut self, content: &str, _a11y: &AccessibilityProps) -> Self::Node {
        let node = self.add_node(NodeKind::Text(TextVisual::new(content)));
        self.set_text_measure(node.id);
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
        let control_id = self.alloc_control_id();
        let handler = on_click.fire.clone();
        self.command_handlers.insert(
            control_id,
            CommandEntry { code: BN_CLICKED, action: handler },
        );
        let hwnd =
            self.create_control_hwnd(class_button(), label, BS_DEFPUSHBUTTON as u32, Some(control_id));
        let node = self.add_node(NodeKind::Control { hwnd });
        if let Some(meta) = self.nodes.get_mut(&node.id) {
            meta.control_id = Some(control_id);
        }
        // Label metrics + push-button chrome padding.
        let (tw, th) = self.measure_with_key(label, None);
        self.set_intrinsic(&node, tw + 24, th + 8);
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
        self.children.entry(parent.id).or_default().push(child.id);
        // No HWND re-parenting: painted nodes have no windows, and
        // native controls are direct children of the host, positioned
        // in absolute window coordinates by `layout_pass`.
        self.layout_dirty = true;
    }

    fn clear_children(&mut self, node: &Self::Node) {
        let Some(child_ids) = self.children.remove(&node.id) else {
            return;
        };
        for child_id in child_ids {
            self.remove_subtree(child_id);
        }
        self.layout_dirty = true;
        self.invalidate();
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        if let Some(meta) = self.nodes.get_mut(&node.id) {
            if let NodeKind::Text(t) = &mut meta.kind {
                t.content = content.to_string();
            }
        }
        self.set_text_measure(node.id);
        self.invalidate();
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        let wide = to_pcwstr(label);
        let _ = unsafe { SetWindowTextW(node.hwnd, wide.as_pcwstr()) };
        let (tw, th) = self.measure_with_key(label, None);
        self.set_intrinsic(node, tw + 24, th + 8);
    }

    fn finish(&mut self, root: Self::Node) {
        self.root_id = Some(root.id);
        self.layout_pass();
        self.invalidate();
    }

    fn create_image(
        &mut self,
        src: &str,
        _alt: Option<&str>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let decoded = self.decode_src(src);
        let natural = decoded.as_ref().map(|d| d.natural());
        let node = self.add_node(NodeKind::Image(image::ImagePaint {
            image: decoded,
            ..Default::default()
        }));
        // Natural pixel size as the intrinsic size; explicit style
        // width/height overrides via `set_style`.
        if let Some((w, h)) = natural {
            self.set_intrinsic(&node, w as i32, h as i32);
        }
        node
    }

    fn create_icon(
        &mut self,
        data: &runtime_core::primitives::icon::IconData,
        color: Option<&Color>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // No color → opaque black (the native analogue of web's
        // `currentColor`, matching the Linux default).
        let rgba = color
            .map(|c| runtime_core::color::parse_or(&c.0, Rgba::BLACK))
            .unwrap_or(Rgba::BLACK);
        let paint = icon::IconPaint::from_data(data, rgba);
        let (vw, vh) = paint.view_box;
        let node = self.add_node(NodeKind::Icon(paint));
        // Icons have no intrinsic content size; default to the view-box
        // so a bare `icon(X)` is visible under flex (`.size(n)` overrides).
        self.set_intrinsic(&node, vw as i32, vh as i32);
        node
    }

    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        let decoded = self.decode_src(src);
        let natural = decoded.as_ref().map(|d| d.natural());
        if let Some(meta) = self.nodes.get_mut(&node.id) {
            if let NodeKind::Image(p) = &mut meta.kind {
                // Assigning drops the previous DecodedImage, disposing
                // the old GpImage.
                p.image = decoded;
            }
        }
        if let Some((w, h)) = natural {
            self.set_intrinsic(node, w as i32, h as i32);
        }
        self.invalidate();
    }

    fn update_icon_color(&mut self, node: &Self::Node, color: &Color) {
        if let Some(meta) = self.nodes.get_mut(&node.id) {
            if let NodeKind::Icon(p) = &mut meta.kind {
                p.color = runtime_core::color::parse_or(&color.0, Rgba::BLACK);
            }
        }
        self.invalidate();
    }

    fn update_icon_data(
        &mut self,
        node: &Self::Node,
        data: &runtime_core::primitives::icon::IconData,
    ) {
        let vb = if let Some(meta) = self.nodes.get_mut(&node.id) {
            if let NodeKind::Icon(p) = &mut meta.kind {
                p.set_data(data);
                Some(p.view_box)
            } else {
                None
            }
        } else {
            None
        };
        if let Some((vw, vh)) = vb {
            self.set_intrinsic(node, vw as i32, vh as i32);
        }
        self.invalidate();
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
        let control_id = self.alloc_control_id();
        let mut style = ES_AUTOHSCROLL | WS_BORDER.0;
        if secure {
            style |= ES_PASSWORD;
        }
        let hwnd = self.create_control_hwnd(class_edit(), initial_value, style, Some(control_id));
        // `EN_CHANGE` fires on every content change; the handler reads
        // the live text so it stays correct across IME / paste / undo.
        let action: Rc<dyn Fn()> = Rc::new(move || on_change(window_text(hwnd)));
        self.command_handlers
            .insert(control_id, CommandEntry { code: EN_CHANGE, action });
        let node = self.add_node(NodeKind::Control { hwnd });
        if let Some(meta) = self.nodes.get_mut(&node.id) {
            meta.control_id = Some(control_id);
        }
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
        // Read-only painted text for now (parity with the previous
        // STATIC-based placeholder; a real multi-line EDIT is a later
        // control refinement).
        let node = self.add_node(NodeKind::Text(TextVisual::new(initial_value)));
        self.set_text_measure(node.id);
        node
    }

    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // A `BUTTON` with `BS_AUTOCHECKBOX` maintains its own checked
        // state on click, then fires `BN_CLICKED`.
        let control_id = self.alloc_control_id();
        let hwnd = self.create_control_hwnd(class_button(), "", BS_AUTOCHECKBOX, Some(control_id));
        unsafe {
            SendMessageW(
                hwnd,
                BM_SETCHECK,
                WPARAM(if initial_value { BST_CHECKED as usize } else { 0 }),
                LPARAM(0),
            );
        }
        let action: Rc<dyn Fn()> = Rc::new(move || {
            let checked =
                unsafe { SendMessageW(hwnd, BM_GETCHECK, WPARAM(0), LPARAM(0)) }.0 == BST_CHECKED;
            on_change(checked);
        });
        self.command_handlers
            .insert(control_id, CommandEntry { code: BN_CLICKED, action });
        let node = self.add_node(NodeKind::Control { hwnd });
        if let Some(meta) = self.nodes.get_mut(&node.id) {
            meta.control_id = Some(control_id);
        }
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
        let hwnd = self.create_control_hwnd(class_trackbar(), "", TBS_HORZ, None);
        unsafe {
            // TBM_SETRANGE takes MAKELONG(min, max) in lParam.
            let range = (SLIDER_RESOLUTION as isize) << 16;
            SendMessageW(hwnd, TBM_SETRANGE, WPARAM(1), LPARAM(range));
            let pos = value_to_slider_pos(initial_value, min, max);
            SendMessageW(hwnd, TBM_SETPOS, WPARAM(1), LPARAM(pos as isize));
        }
        let action: Rc<dyn Fn()> = Rc::new(move || {
            let pos = unsafe { SendMessageW(hwnd, TBM_GETPOS, WPARAM(0), LPARAM(0)) }.0 as i32;
            let value = min + (pos as f32 / SLIDER_RESOLUTION as f32) * (max - min);
            on_change(value);
        });
        self.slider_handlers.insert(hwnd.0 as isize, action);
        let node = self.add_node(NodeKind::Control { hwnd });
        self.set_intrinsic(&node, 160, 24);
        node
    }

    fn create_scroll_view(
        &mut self,
        horizontal: bool,
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let node = self.add_node(NodeKind::View(ViewVisual {
            scroll: Some(ScrollInfo {
                horizontal,
                offset_x: 0.0,
                offset_y: 0.0,
                on_scroll,
            }),
            ..Default::default()
        }));
        // Taffy MUST know this node scrolls (`overflow: scroll` on the
        // axis): the content is a Taffy child, so without it the
        // viewport node sizes to its content — the website's page
        // scroll node laid out 2376px tall in an 860px window, leaving
        // `content − viewport = 0` to scroll. See
        // `LayoutTree::set_overflow_scroll`'s docs; iOS/Android/macOS
        // make the same call for the same reason.
        if let Some(layout) = self.layout_for_id.get(&node.id).copied() {
            self.layout.set_overflow_scroll(layout, horizontal);
        }
        node
    }

    fn set_node_scroll(&mut self, node: &Self::Node, x: f32, y: f32) {
        if let Some(meta) = self.nodes.get_mut(&node.id) {
            if let NodeKind::View(v) = &mut meta.kind {
                if let Some(s) = &mut v.scroll {
                    s.offset_x = x.max(0.0);
                    s.offset_y = y.max(0.0);
                }
            }
        }
        // Same as `wheel_scroll`: HWND children must follow the offset.
        // No synchronous paint here — author code calls this under the
        // backend borrow (same double-borrow hazard as wheel_scroll).
        self.position_native_children();
        self.invalidate();
    }

    fn create_activity_indicator(
        &mut self,
        _size: runtime_core::primitives::activity_indicator::ActivityIndicatorSize,
        _color: Option<&Color>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // A marquee progress bar is the native indeterminate-activity
        // affordance; comctl32 animates it on its own timer.
        ensure_common_controls();
        let hwnd = self.create_control_hwnd(class_progress(), "", PBS_MARQUEE, None);
        unsafe {
            SendMessageW(hwnd, PBM_SETMARQUEE, WPARAM(1), LPARAM(30));
        }
        let node = self.add_node(NodeKind::Control { hwnd });
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
        on_ready: runtime_core::primitives::graphics::OnReady,
        on_resize: runtime_core::primitives::graphics::OnResize,
        on_lost: runtime_core::primitives::graphics::OnLost,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // The surface is a DirectComposition visual, not a child HWND
        // — see `dcomp.rs` for the architecture. Tree init is lazy so
        // apps without graphics never touch dcomp.dll.
        if self.comp.is_none() {
            self.comp = dcomp::CompositionTree::new(self.host_hwnd);
        }
        let Some(comp) = &self.comp else {
            return self.placeholder("Graphics surface creation failed (DirectComposition)");
        };
        let Some(visuals) = comp.add_visual() else {
            return self.placeholder("Graphics surface creation failed (visual)");
        };
        let device = comp.device.clone();
        // Graphics nodes carry no HWND; a null handle keeps them out of
        // the native-control move/hide paths (all guarded by
        // `is_invalid`) while the External kind still gives them a
        // frame in the layout tree.
        let node = self.add_node(NodeKind::External { hwnd: HWND(std::ptr::null_mut()) });
        let visible = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let surface = graphics::make_surface(visuals.content.clone(), device, visible.clone());
        self.graphics.insert(
            node.id,
            graphics::GraphicsState {
                surface,
                visuals,
                visible,
                last_placement: None,
                on_ready: Some(on_ready),
                on_resize: Some(on_resize),
                on_lost: Some(on_lost),
                last_size: None,
            },
        );
        node
    }

    fn create_external(
        &mut self,
        type_id: std::any::TypeId,
        type_name: &'static str,
        payload: &Rc<dyn std::any::Any>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
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

    fn set_portal_hidden(&mut self, node: &Self::Node, hidden: bool) {
        // Hide without teardown (navigation off a portal's screen). The
        // painter + hit tester skip hidden subtrees; native control
        // HWNDs inside the subtree hide/show explicitly since they
        // paint themselves.
        if let Some(meta) = self.nodes.get_mut(&node.id) {
            meta.hidden = hidden;
        }
        let mut stack = vec![node.id];
        while let Some(id) = stack.pop() {
            if let Some(meta) = self.nodes.get(&id) {
                if let Some(hwnd) = meta.hwnd() {
                    unsafe {
                        let _ = ShowWindow(
                            hwnd,
                            if hidden {
                                windows::Win32::UI::WindowsAndMessaging::SW_HIDE
                            } else {
                                SW_SHOW
                            },
                        );
                    }
                }
            }
            if let Some(kids) = self.children.get(&id) {
                stack.extend(kids.iter().copied());
            }
        }
        // Graphics visuals don't respond to ShowWindow — re-run the
        // positioning walk, which threads the hidden flag down to each
        // visual's clip + visibility vote (empty clip when hidden).
        self.position_native_children();
        self.invalidate();
    }

    fn create_navigator(
        &mut self,
        type_id: std::any::TypeId,
        _type_name: &'static str,
        presentation: Rc<dyn std::any::Any>,
        host: NavigatorHost<Self::Node>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Dispatch to a registered handler (the backend-neutral swap /
        // stack navigators build their chrome from primitives, which
        // the scene painter draws). With no handler, fall back to a
        // bare container — the walker still mounts the path-matched
        // initial screen into it via `navigator_attach_initial`, so the
        // current page renders even without navigation chrome. Same
        // posture as the Linux backend.
        if let Some(factory) = self.navigator_handlers.get(type_id) {
            let mut handler = factory();
            let node = handler.init(self, host, presentation);
            self.nav_handlers
                .insert(node.id, Rc::new(RefCell::new(handler)));
            node
        } else {
            self.create_view(a11y)
        }
    }

    fn navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: Box<dyn std::any::Any>,
    ) {
        if let Some(handler) = self.nav_handlers.get(&navigator.id).cloned() {
            handler
                .borrow_mut()
                .attach_initial(self, screen, scope_id, options);
        } else {
            // Bare-container fallback: mount the initial screen directly.
            let mut nav = navigator.clone();
            self.insert(&mut nav, screen);
        }
    }

    fn release_navigator(&mut self, node: &Self::Node) {
        if let Some(handler) = self.nav_handlers.remove(&node.id) {
            handler.borrow_mut().release(self);
        }
    }

    fn apply_navigator_slot_style(
        &mut self,
        node: &Self::Node,
        slot: &'static str,
        style: &Rc<StyleRules>,
    ) {
        if let Some(handler) = self.nav_handlers.get(&node.id).cloned() {
            handler.borrow_mut().apply_slot_style(self, slot, style);
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
        // to the bundled TTF. Idempotent per TypefaceId.
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

    fn set_app_background(&mut self, color: &runtime_core::Tokenized<Color>) {
        self.app_background =
            Some(runtime_core::color::parse_or(&color.resolve().0, Rgba::TRANSPARENT));
        self.invalidate();
    }

    fn set_animated_f32(&mut self, node: &Self::Node, prop: AnimProp, value: f32) {
        if let Some(meta) = self.nodes.get_mut(&node.id) {
            match prop {
                AnimProp::Opacity => meta.anim_opacity = Some(value),
                AnimProp::TranslateX => meta.anim.tx = value,
                AnimProp::TranslateY => meta.anim.ty = value,
                AnimProp::Scale => meta.anim.scale = value,
                AnimProp::ScaleX => meta.anim.scale_x = value,
                AnimProp::ScaleY => meta.anim.scale_y = value,
                AnimProp::RotateZ => meta.anim.rotate_deg = value,
                // The painter re-sorts siblings by z every frame, so a
                // z write needs no reorder bookkeeping here.
                AnimProp::ZIndex => meta.z = value,
                // MaxHeight would drive a Taffy reflow; no current
                // author code animates it on Windows — no-op rather
                // than a wrong write.
                _ => return,
            }
        }
        self.invalidate();
    }

    fn set_animated_color(&mut self, node: &Self::Node, prop: AnimProp, value: [f32; 4]) {
        if let Some(meta) = self.nodes.get_mut(&node.id) {
            match prop {
                AnimProp::BackgroundColor => {
                    if let NodeKind::View(v) = &mut meta.kind {
                        v.anim_background = Some(value);
                    }
                }
                AnimProp::ForegroundColor => {
                    if let NodeKind::Text(t) = &mut meta.kind {
                        t.anim_color = Some(value);
                    }
                }
                AnimProp::GradientStopColor(idx) => {
                    if let NodeKind::View(v) = &mut meta.kind {
                        if let Some(gr) = &mut v.gradient {
                            if let Some(stop) = gr.stops.get_mut(idx as usize) {
                                stop.1 = value;
                            }
                        }
                    }
                }
                _ => return,
            }
        }
        self.invalidate();
    }

    fn make_view_handle(&self, node: &Self::Node) -> runtime_core::ViewHandle {
        handles::make_view_handle(self, node)
    }

    fn make_text_handle(&self, node: &Self::Node) -> runtime_core::TextHandle {
        handles::make_text_handle(self, node)
    }

    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        // Box layout via the shared StyleRules→Taffy translator.
        if let Some(layout) = self.layout_for_id.get(&node.id).copied() {
            self.layout.set_style(layout, style);
        }
        self.layout_dirty = true;

        // Typography resolution needs `&mut self` (font cache) — do it
        // before borrowing the node meta.
        let font_key = self.font_key_for_style(style);

        let mut remeasure = false;
        if let Some(meta) = self.nodes.get_mut(&node.id) {
            // Node-level visuals shared by every kind.
            meta.style_opacity = style
                .opacity
                .as_ref()
                .map(|t| t.resolve())
                .unwrap_or(1.0);
            meta.author_transform = style.transform.clone().unwrap_or_default();
            meta.overflow_hidden = matches!(style.overflow, Some(Overflow::Hidden));

            match &mut meta.kind {
                NodeKind::View(v) => {
                    v.background = resolve_color(&style.background).map(srgba_of);
                    v.gradient = style.background_gradient.as_ref().map(resolve_gradient);
                    v.borders = [
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
                    v.radii = [
                        resolve_radius(&style.border_top_left_radius),
                        resolve_radius(&style.border_top_right_radius),
                        resolve_radius(&style.border_bottom_right_radius),
                        resolve_radius(&style.border_bottom_left_radius),
                    ];
                }
                NodeKind::Text(t) => {
                    if let Some(rgba) = resolve_color(&style.color) {
                        t.color = rgba;
                    }
                    if let Some(a) = style.text_align {
                        t.align = a;
                    }
                    // Paint-only: alignment needs no re-measure. Font and
                    // line-height feed the wrap plan / line advance, so
                    // either changing rebuilds the measure fn.
                    let lh = style.line_height.as_ref().map(|v| v.resolve());
                    if lh.is_some() && t.line_height != lh {
                        t.line_height = lh;
                        remeasure = true;
                    }
                    if font_key.is_some() && t.font_key != font_key {
                        t.font_key = font_key.clone();
                        remeasure = true;
                    }
                }
                NodeKind::Image(p) => {
                    if let Some(fit) = style.object_fit {
                        p.object_fit = fit;
                    }
                }
                // Code runs carry their own tokenizer colors + mono
                // font; author style contributes box/layout only.
                NodeKind::Code(_)
                | NodeKind::Icon(_)
                | NodeKind::Control { .. }
                | NodeKind::External { .. } => {}
            }
        }
        // Rebuild the wrap plan + measure fn in the resolved font so
        // layout matches what's drawn.
        if remeasure {
            self.set_text_measure(node.id);
        }
        self.invalidate();
    }
}

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
        approx(s, (50.0, 100.0));
        approx(e, (50.0, 0.0));
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
        let r = radial_radius((0.5, 0.5), 1.0, true, 100.0, 200.0);
        assert!((r - 111.803).abs() < 1e-2, "got {r}");
    }

    // --- animated-value plumbing (pure node-state checks) ---

    /// Painted-scene clips can't touch a native child window, so the
    /// nearest clipping ancestor must become a window REGION on the
    /// child. (Originally hit by the Simulator's wgpu canvas when it
    /// was a child HWND; graphics surfaces are composition visuals now
    /// — see `dcomp::visual_placement`'s tests — but the region path
    /// still guards native controls inside rounded/scrolling wrappers.)
    #[test]
    fn regression_native_child_clips_to_rounded_ancestor() {
        // Canvas exactly filling a 300×649 wrapper with 32px radii →
        // full-cover but ROUNDED: a round-rect region over the whole
        // child (ellipse = 2r).
        let r = hwnd_clip_region(
            (100.0, 50.0, 300.0, 649.0),
            Some(((100.0, 50.0, 300.0, 649.0), 32.0)),
        );
        assert_eq!(r, Some((0, 0, 300, 649, 64)));

        // No clipping ancestor → no region.
        assert_eq!(hwnd_clip_region((0.0, 0.0, 10.0, 10.0), None), None);

        // Fully inside a square-cornered clip → no region needed.
        let r = hwnd_clip_region(
            (10.0, 10.0, 50.0, 20.0),
            Some(((0.0, 0.0, 200.0, 200.0), 0.0)),
        );
        assert_eq!(r, None);
    }

    /// A control half-scrolled out of its scroll viewport keeps only
    /// the visible part (rect region in child-local coords); fully
    /// scrolled out = empty region (nothing shows).
    #[test]
    fn native_child_partial_and_full_scroll_clip() {
        // Viewport y 0..200; control at abs y 150, 60 tall → bottom 10
        // visible rows clipped off.
        let r = hwnd_clip_region(
            (0.0, 150.0, 100.0, 60.0),
            Some(((0.0, 0.0, 100.0, 200.0), 0.0)),
        );
        assert_eq!(r, Some((0, 0, 100, 50, 0)));

        // Fully outside → empty region.
        let r = hwnd_clip_region(
            (0.0, 300.0, 100.0, 60.0),
            Some(((0.0, 0.0, 100.0, 200.0), 0.0)),
        );
        assert_eq!(r, Some((0, 0, 0, 0, 0)));
    }

    /// A scroll viewport must be sized by its PARENT, not its content.
    /// `create_scroll_view` didn't call `set_overflow_scroll`, so Taffy
    /// laid the website's page scroll node out 2376px tall in an 860px
    /// window — `content − viewport = 0`, nothing to clamp against, and
    /// clipping to the (content-sized) box was meaningless.
    #[test]
    fn regression_scroll_view_sizes_to_parent_not_content() {
        let mut b = WindowsBackend::new(HWND(std::ptr::null_mut()));
        let a11y = AccessibilityProps::default();
        let mut root = b.create_view(&a11y);
        let mut sv = b.create_scroll_view(false, None, &a11y);
        let sv_id = sv.id;
        let content = b.create_view(&a11y);
        let mut tall = StyleRules::default();
        tall.height = Some(Length::Px(2000.0).into());
        b.apply_style(&content, &Rc::new(tall));
        b.insert(&mut sv, content);
        b.insert(&mut root, sv);
        let root_layout = b.layout_for_id[&root.id];
        b.layout.compute(root_layout, 800.0, 600.0);
        let f = b.layout.frame_of(b.layout_for_id[&sv_id]);
        assert!(
            f.height <= 600.5,
            "scroll viewport bounded by its parent (600), got {}",
            f.height
        );
    }

    /// Scroll views must bound children to their box even without an
    /// author `overflow: hidden` — unclipped, scrolled-out sidebar
    /// links painted over the header (and stayed clickable there).
    #[test]
    fn regression_scroll_views_always_clip_children() {
        let plain = NodeMeta::new(NodeKind::View(ViewVisual::default()));
        assert!(!plain.clips_children());

        let mut hidden = NodeMeta::new(NodeKind::View(ViewVisual::default()));
        hidden.overflow_hidden = true;
        assert!(hidden.clips_children());

        let scroll = NodeMeta::new(NodeKind::View(ViewVisual {
            scroll: Some(ScrollInfo {
                offset_x: 0.0,
                offset_y: 0.0,
                horizontal: false,
                on_scroll: None,
            }),
            ..Default::default()
        }));
        assert!(scroll.clips_children(), "scroll views clip regardless of style");
    }

    #[test]
    fn effective_opacity_prefers_animated_over_style() {
        let mut m = NodeMeta::new(NodeKind::View(ViewVisual::default()));
        m.style_opacity = 0.0; // welcome layers start style-hidden
        assert_eq!(m.effective_opacity(), 0.0);
        m.anim_opacity = Some(0.75);
        assert!((m.effective_opacity() - 0.75).abs() < 1e-6);
    }

    #[test]
    fn srgba_of_maps_channels_to_unit_floats() {
        let c = srgba_of(Rgba { r: 255, g: 0, b: 51, a: 128 });
        assert!((c[0] - 1.0).abs() < 1e-6);
        assert!((c[1] - 0.0).abs() < 1e-6);
        assert!((c[2] - 0.2).abs() < 1e-2);
        assert!((c[3] - 0.502).abs() < 1e-2);
    }
}
