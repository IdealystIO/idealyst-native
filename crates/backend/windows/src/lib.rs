//! Native Win32 backend — single-surface painted scene.
//!
//! Implements the scene seam — `runtime_scene::Host` plus the `caps::*Ops`
//! capability traits, both in [`newcore`] — with a **painted scene model**:
//! the host's top-level HWND is one canvas, and the view / text / icon /
//! image tree is painted in tree order by one of two renderers
//! ([`RendererKind`]): the GDI+ painter ([`scene`], a double-buffered memory
//! DC; the default) or the Direct2D + DirectWrite renderer (`d2d.rs`, a
//! DirectComposition swapchain; `IDEALYST_WINDOWS_RENDERER=d2d`). Only
//! genuinely-native interactive controls — button, edit, checkbox,
//! trackbar, progress — are real child HWNDs, positioned on top by the layout pass.
//! See `docs/windows.md` for choosing a renderer and running under Wine.
//!
//! The per-primitive bodies live in this module as **inherent** methods on
//! [`WindowsBackend`]; `newcore`'s caps impls delegate to them. That is the
//! same split the GTK backend uses. They were an `impl Backend` before the
//! v2 port — that mega-trait, and the per-backend External/Navigator
//! registries alongside it, are gone (CLAUDE.md §3): third-party leaves
//! register on the scene `Registry` instead.
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
//! ## App-level input and situation
//!
//! The app-wide keyboard (`app_keys.rs`) and pointer (`app_pointer.rs` —
//! buttons, movement, wheel, `ClipCursor` + raw-input pointer lock,
//! `WM_SETCURSOR` cursor hiding) are fed from the HOST's message pump,
//! because input for a focused / hovered child control never reaches the
//! host `WndProc`. Focus, minimize, reduce-motion and pointer-kind facts
//! come from host-window messages (`app_situation.rs`). The
//! `user_activity()` heartbeat (`user_activity.rs`) rides the same pump:
//! both slots report every key / button down, pointer contact and wheel
//! before looking for a sink. The pure message → event rules live in
//! `keymap.rs` / `pointer_policy.rs` / `user_activity.rs`, testable on any
//! host.
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

use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::animation::AnimProp;
use runtime_shared::assets::{
    AssetId, AssetSource, AssetTag, SystemFallback, TypefaceFace, TypefaceId,
};
use runtime_shared::color::Rgba;

use runtime_shared::{
    Action, Color, Gradient, GradientKind, Length, Overflow, RadialExtent, StyleRules, Transform,
};
use runtime_layout::LayoutTree;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontIndirectW, DeleteObject, GetDC, GetTextExtentPoint32W, HFONT, HGDIOBJ,
    InvalidateRect, ReleaseDC, SelectObject, SetWindowRgn, HDC, LOGFONTW,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
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

// Post-dispatch flush-hook slot (new-core flush driver). Unconditional
// — the fire sites live in the out-of-repo host shell, which cannot
// see this crate's features; no-op default so the old core never pays.
pub mod dispatch_hook;

/// `runtime_scene::Host` + the 30 capability traits on
/// [`WindowsBackend`], plus the boot entry and flush driver.
pub mod newcore;

pub mod app_keys;
pub mod app_pointer;
pub mod app_situation;
mod border_pattern;
mod code;
mod d2d;
mod dcomp;
/// Logical ↔ device pixel policy (per-monitor DPI). Public so the host
/// shell sizes its window and handles `WM_DPICHANGED` with the same rules.
pub mod dpi;
mod dwrite_text;
mod file_drop;
mod font;
mod gradient_raster;
mod graphics;
mod layers;
mod overlay;
mod layout_subs;
mod handles;
mod icon;
mod image;
mod image_fetch;
mod image_policy;
mod keymap;
mod pointer_policy;
mod painted_geom;
#[cfg(feature = "robot")]
mod introspect;
#[cfg(feature = "robot")]
mod screenshot;
pub mod user_activity;
mod scene;
pub mod view_input;
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

/// Make the process per-monitor DPI aware (v2, falling back to v1 on
/// Windows 10 before 1703, then to system-aware on older systems).
/// Process-wide and settable once, so the HOST calls it before creating
/// any window; a manifest that already declared awareness makes every
/// call fail harmlessly. Returns whether this call set it.
///
/// Without it Windows treats the process as DPI-unaware and bitmap-
/// stretches the whole window on a >96-DPI monitor: blurry text and
/// SVGs, `GetDpiForWindow` stuck at 96. See `dpi.rs`.
pub fn enable_per_monitor_dpi_awareness() -> bool {
    use windows::Win32::UI::HiDpi::{
        SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE,
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    use windows::Win32::UI::WindowsAndMessaging::SetProcessDPIAware;
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_ok()
            || SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE).is_ok()
            || SetProcessDPIAware().as_bool()
    }
}

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
/// `AnimationOps::set_animated_f32`. `scale` and `scale_x`/`scale_y`
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
/// packed ARGB — because `AnimationOps::set_animated_color` overwrites
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
    /// Line pattern for every side (`StyleRules::border_style`). Solid
    /// takes the original paint path; dashed / dotted are planned by
    /// [`border_pattern`].
    pub border_style: runtime_shared::BorderStyle,
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
    pub align: runtime_shared::TextAlign,
    /// Style `line-height` in px (the css crate emits it as px). Line
    /// advance + CSS half-leading; `None` = the font's natural height.
    pub line_height: Option<f32>,
    /// `StyleRules::max_lines` with `0` normalised to `None` (no limit).
    /// Feeds both the measure fn and the `layout_pass` line fill through
    /// `WrapPlan::lines_limited`, so the measured height and the drawn
    /// (cut, `…`-ended) lines agree. Re-set on every `apply_style`, so a
    /// restyle without it restores unlimited wrapping.
    pub max_lines: Option<u32>,
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
            align: runtime_shared::TextAlign::Left,
            line_height: None,
            max_lines: None,
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
    /// The author's padding `[top, right, bottom, left]` on a painted LEAF
    /// (text / code / icon / image), which paints its own content inside
    /// it. Taffy already sizes the frame to content + padding, but a leaf
    /// has no child to offset — so without this inset its content painted
    /// from the frame's top-left corner, flush against the padding edge
    /// (the docs sidebar's section labels sat on the item above them with
    /// the padding gap below). The GTK backend solves the same problem
    /// with widget margins. Always zero for views: their children are
    /// positioned by Taffy.
    pub content_inset: [f32; 4],
    /// `StyleRules::pointer_events` as authored (`None` = not set, so the
    /// nearest explicit ancestor decides). Read by the hit walk
    /// (`scene::hit_chain`).
    pub pointer_events: Option<runtime_shared::PointerEvents>,
    /// A reactive anchor (`create_anchor`): layout- and hit-transparent.
    pub contents: bool,
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
            content_inset: [0.0; 4],
            pointer_events: None,
            contents: false,
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
    pub(crate) comp: Option<dcomp::CompositionTree>,
    /// Last window region applied per child HWND (key: hwnd as isize)
    /// so `layout_pass` only calls `SetWindowRgn` on actual changes —
    /// every set forces a repaint of the child.
    hwnd_regions: HashMap<isize, Option<RegionSpec>>,
    /// trackbar HWND (as isize) → its `on_change` fire closure.
    slider_handlers: HashMap<isize, Rc<dyn Fn()>>,
    /// Registered image assets keyed by `AssetId`.
    assets: HashMap<u64, image_policy::AssetEntry>,
    /// Per-image-node load generation, outcome and `on_load`/`on_error`
    /// handlers (`image_policy::ImageEvents`).
    image_events: HashMap<u64, image_policy::ImageEvents>,
    /// Per-view `on_touch` / `on_wheel` / `on_hover` handlers and the live
    /// gesture, fed by the host's mouse messages (see `view_input`).
    pub(crate) view_input: view_input::ViewInput,
    /// The host window's OLE drop target, registered with the first
    /// `on_file_drop` handler (`file_drop.rs`).
    drop_target: Option<windows::Win32::System::Ole::IDropTarget>,
    /// Visuals showing painted content lifted above graphics surfaces
    /// (`layers`, `overlay`). A `RefCell` because the Direct2D frame
    /// presents them from `&self`.
    overlays: RefCell<overlay::Overlays>,
    /// Hidden message-only window that brings image fetches and deferred
    /// image events back to the UI thread (`image_fetch.rs`). Created on
    /// the first image load.
    image_pump: Option<image_fetch::ImagePump>,
    /// The shell message font (Segoe UI) at its LOGICAL size — what text
    /// with no style typography is measured in. Never handed to a native
    /// control: controls draw in device pixels (`control_font`).
    ui_font: HFONT,
    /// The shell message font recipe at logical size; `control_font` is
    /// rebuilt from it whenever the DPI changes.
    ui_logfont: LOGFONTW,
    /// `ui_font` scaled to the window's DPI — the `WM_SETFONT` font of
    /// every native control (button / edit / checkbox …), whose text the
    /// control itself draws in device pixels.
    control_font: HFONT,
    /// Single-line height of `ui_font` in logical px.
    line_height: i32,
    /// The host window's DPI. Everything stored on the backend (frames,
    /// fonts, scroll offsets) is logical; this converts at the Win32
    /// boundary. See `dpi.rs`.
    pub(crate) dpi: dpi::DpiScale,
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
    /// The app-level keyboard sink, in a slot the host shell shares
    /// ([`WindowsBackend::keyboard_slot`]) so its message pump can deliver
    /// keys without borrowing the backend. See `app_keys.rs`.
    keyboard: app_keys::KeyboardSlot,
    /// The app-level pointer sink + lock / cursor state, shared with the
    /// host shell ([`WindowsBackend::pointer_slot`]) like the keyboard
    /// slot. See `app_pointer.rs`.
    pointer: app_pointer::PointerSlot,
    /// Which renderer paints the scene (see [`RendererKind`]).
    renderer: RendererKind,
    /// The Direct2D renderer, created at the first paint (it needs the
    /// window). A `RefCell` so it can draw while borrowing the scene it
    /// reads from `self`. Declared BEFORE `dwrite` so it drops first: its
    /// cached text layouts reference DirectWrite's registered fonts.
    d2d: RefCell<Option<d2d::D2dRenderer>>,
    /// DirectWrite measurement, present iff `renderer` is `D2d`: text must
    /// be measured by the engine that draws it (`dwrite_text.rs`).
    pub(crate) dwrite: Option<dwrite_text::DwText>,
}

/// Serializes tests that use DirectWrite / Direct2D. The backend only uses
/// them from the UI thread; the test harness runs tests on parallel
/// threads, and Wine's DirectWrite/Direct2D are not safe for that (page
/// faults inside `dwrite.dll` when tests overlapped, never when run one at a
/// time). Windows' shared factory is documented thread-safe — the lock
/// keeps the tests to the backend's real, single-threaded use.
#[cfg(test)]
pub(crate) fn under_wine() -> bool {
    // Wine exports `wine_get_version` from ntdll; Windows doesn't.
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    unsafe {
        GetModuleHandleW(windows::core::w!("ntdll.dll"))
            .ok()
            .and_then(|m| GetProcAddress(m, windows::core::s!("wine_get_version")))
            .is_some()
    }
}

#[cfg(test)]
pub(crate) fn render_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Which renderer draws the painted scene.
///
/// - `Gdi`: the GDI+ painter (`scene.rs`) — CPU rasterization into a back
///   buffer blitted on `WM_PAINT`. The default.
/// - `D2d`: Direct2D + DirectWrite on the GPU, presented through
///   DirectComposition (`d2d.rs`). Requires Direct3D 11 + DirectComposition:
///   any Windows 10+, or under Wine, Wine-Staging with DXVK.
///
/// Native controls, layout, input and hit testing are the same either way;
/// only how the scene reaches pixels — and therefore how text is measured —
/// differs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RendererKind {
    Gdi,
    D2d,
}

impl RendererKind {
    /// `IDEALYST_WINDOWS_RENDERER=d2d` selects Direct2D; unset or `gdi`
    /// selects GDI+. Any other value is a configuration error and panics
    /// (a misspelt renderer silently falling back would hide which one is
    /// being measured).
    pub fn from_env() -> Self {
        match std::env::var("IDEALYST_WINDOWS_RENDERER").as_deref() {
            Err(_) | Ok("") | Ok("gdi") => RendererKind::Gdi,
            Ok("d2d") => RendererKind::D2d,
            Ok(other) => panic!("IDEALYST_WINDOWS_RENDERER={other:?}: expected \"gdi\" or \"d2d\""),
        }
    }
}

impl WindowsBackend {
    /// Construct a backend rooted at `host_hwnd`. The host shell owns
    /// the window; the backend paints its client area and creates child
    /// controls underneath.
    pub fn new(host_hwnd: HWND) -> Self {
        Self::with_renderer(host_hwnd, RendererKind::from_env())
    }

    /// [`Self::new`] with an explicit renderer. Selecting `D2d` on a system
    /// without DirectWrite panics: the caller asked for it by name.
    pub fn with_renderer(host_hwnd: HWND, renderer: RendererKind) -> Self {
        ensure_gdiplus();
        let dwrite = match renderer {
            RendererKind::Gdi => None,
            RendererKind::D2d => Some(
                dwrite_text::DwText::new()
                    .unwrap_or_else(|e| panic!("Direct2D renderer: DirectWrite unavailable: {e}")),
            ),
        };
        let (ui_logfont, ui_font, line_height, base_size, base_family) = create_ui_font();
        // `GetDpiForWindow` answers 0 for a null/invalid window (tests
        // build headless backends) — `from_dpi` reads that as 96.
        let dpi = dpi::DpiScale::from_dpi(unsafe { GetDpiForWindow(host_hwnd) });
        let control_font = create_control_font(&ui_logfont, dpi);
        let pointer = app_pointer::PointerSlot::new(host_hwnd);
        pointer.set_scale(dpi.factor());
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
            image_events: HashMap::new(),
            view_input: view_input::ViewInput::default(),
            drop_target: None,
            overlays: RefCell::new(overlay::Overlays::default()),
            image_pump: None,
            ui_font,
            ui_logfont,
            control_font,
            line_height,
            dpi,
            font_cache: font::FontCache::new(),
            default_font_key,
            installed_typefaces: HashSet::new(),
            app_background: None,
            back: scene::BackBuffer::new(),
            layout_dirty: false,
            keyboard: app_keys::KeyboardSlot::default(),
            pointer,
            renderer,
            d2d: RefCell::new(None),
            dwrite,
        }
    }

    /// The renderer this backend paints with.
    pub fn renderer(&self) -> RendererKind {
        self.renderer
    }

    /// The window's current DPI scale (device px per logical px).
    pub fn scale(&self) -> f32 {
        self.dpi.factor()
    }

    /// The host client area in LOGICAL pixels — what layout runs against
    /// and what the host publishes as the viewport signal.
    pub fn logical_client_size(&self) -> (f32, f32) {
        let mut rect = RECT::default();
        if unsafe { GetClientRect(self.host_hwnd, &mut rect) }.is_err() {
            return (0.0, 0.0);
        }
        self.dpi.client_to_logical(rect.right - rect.left, rect.bottom - rect.top)
    }

    /// Adopt a new window DPI — the host calls this from `WM_DPICHANGED`
    /// BEFORE applying the suggested window rect, so the `WM_SIZE` that
    /// rect triggers already lays out at the new scale. Rebuilds the
    /// native controls' device font, re-scales pointer reporting, and
    /// re-lays out (control HWNDs, window regions and composition visuals
    /// move to their new device rects; graphics surfaces get an
    /// `on_resize` with the new physical size + scale). Painted content
    /// needs nothing: the next paint's root transform and SVG raster
    /// density read the new scale.
    pub fn set_dpi(&mut self, dpi: u32) {
        let next = dpi::DpiScale::from_dpi(dpi);
        if next == self.dpi {
            return;
        }
        self.dpi = next;
        self.pointer.set_scale(next.factor());
        let font = create_control_font(&self.ui_logfont, next);
        if !font.is_invalid() {
            for meta in self.nodes.values() {
                if let NodeKind::Control { hwnd } = meta.kind {
                    if !hwnd.is_invalid() {
                        unsafe {
                            SendMessageW(hwnd, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
                        }
                    }
                }
            }
            // Every control now holds the new font; the old one is free.
            let old = std::mem::replace(&mut self.control_font, font);
            if !old.is_invalid() {
                unsafe {
                    let _ = DeleteObject(HGDIOBJ(old.0));
                }
            }
        }
        self.layout_dirty = true;
        self.relayout();
    }

    /// The shared app-level keyboard slot. The host shell clones it once at
    /// boot, offers every retrieved message to
    /// [`KeyboardSlot::pre_dispatch`](app_keys::KeyboardSlot::pre_dispatch)
    /// before `TranslateMessage`/`DispatchMessageW`, and calls
    /// [`KeyboardSlot::focus_lost`](app_keys::KeyboardSlot::focus_lost) on
    /// `WM_ACTIVATE(WA_INACTIVE)`.
    pub fn keyboard_slot(&self) -> app_keys::KeyboardSlot {
        self.keyboard.clone()
    }

    /// The shared app-level pointer slot. The host shell clones it once at
    /// boot, offers every retrieved message to
    /// [`PointerSlot::pre_dispatch`](app_pointer::PointerSlot::pre_dispatch)
    /// before `TranslateMessage`/`DispatchMessageW` (after the keyboard),
    /// routes host-window messages through
    /// [`PointerSlot::window_message`](app_pointer::PointerSlot::window_message),
    /// and calls [`PointerSlot::shutdown`](app_pointer::PointerSlot::shutdown)
    /// before the process exits.
    pub fn pointer_slot(&self) -> app_pointer::PointerSlot {
        self.pointer.clone()
    }

    /// Install / remove the app-level pointer sink (`None` removes, also
    /// releasing a lock and showing the cursor).
    pub(crate) fn set_pointer_sink_impl(
        &mut self,
        sink: Option<runtime_shared::pointer_input::PointerSink>,
    ) {
        self.pointer.set_sink(sink);
    }

    pub(crate) fn set_pointer_lock_impl(&mut self, locked: bool) {
        self.pointer.set_pointer_lock(locked);
    }

    /// Installs an `on_file_drop` handler, registering the window's drop
    /// target the first time. A window that cannot take one (no OLE in this
    /// apartment) keeps working; its drop views just never fire.
    pub(crate) fn install_file_drop_handler_impl(&mut self, id: u64, handler: runtime_shared::FileDropHandler) {
        self.view_input.file_drop.insert(id, handler);
        if self.drop_target.is_none() && !self.host_hwnd.is_invalid() {
            match file_drop::register(self.host_hwnd, self.self_ref.clone()) {
                Ok(t) => self.drop_target = Some(t),
                Err(e) => eprintln!("[backend-windows] file drop unavailable: RegisterDragDrop failed: {e}"),
            }
        }
    }

    pub(crate) fn set_cursor_hidden_impl(&mut self, hidden: bool) {
        self.pointer.set_cursor_hidden(hidden);
    }

    /// Install / remove the app-level keyboard sink (`None` removes). The
    /// native source is the host's message pump, which is always running,
    /// so installing is just publishing the sink into the shared slot.
    pub(crate) fn set_keyboard_sink_impl(
        &mut self,
        sink: Option<runtime_shared::primitives::key::KeyboardSink>,
    ) {
        self.keyboard.set(sink);
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
        spans: &[(String, runtime_shared::Color)],
    ) -> WindowsNode {
        // Cascadia Mono ships on Win11, Consolas everywhere back to
        // Vista; probe like any CSS stack so the key is drawable.
        let family = font::resolve_family_stack(
            "Cascadia Mono, Consolas, Courier New, monospace",
            &self.default_font_key.family,
        );
        // 13px matches the sibling handlers' 13pt mono size.
        let key = font::FontKey { family, size_px: 13, weight: 400, italic: false };
        let (visual, (w, h)) = match &self.dwrite {
            Some(dw) => {
                let line_height = dw.line_height(&key).unwrap_or(self.line_height as f32);
                let measure_key = key.clone();
                code::build_visual(spans, key, line_height, |s| dw.width(s, &measure_key))
            }
            None => {
                let hfont = font::entry_for(&mut self.font_cache, &key)
                    .map(|e| e.hfont)
                    .unwrap_or(self.ui_font);
                code::build_gdi(spans, hfont, key, self.line_height as f32)
            }
        };
        let node = self.add_node(NodeKind::Code(visual));
        self.set_intrinsic(&node, w, h);
        node
    }

    /// Request a repaint of the scene. Cheap — Windows coalesces
    /// invalidations into one `WM_PAINT` per message-loop pass.
    pub(crate) fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.host_hwnd), None, false);
        }
    }

    /// Paint the scene into `hdc` at `w × h`. The host calls this from
    /// its `WM_PAINT`.
    pub fn paint(&mut self, hdc: HDC, w: i32, h: i32) {
        match self.renderer {
            RendererKind::Gdi => unsafe { scene::paint_scene(self, hdc, w, h) },
            RendererKind::D2d => self.paint_d2d(w, h),
        }
    }

    /// The Direct2D frame. `hdc` is unused: the scene reaches the screen
    /// through the composition swapchain, not the window's DC.
    fn paint_d2d(&mut self, w: i32, h: i32) {
        // Same contract as the GDI painter: paint against current geometry.
        if self.layout_dirty {
            self.layout_pass();
        }
        // The scene visual lives in the backend's one composition tree,
        // under the graphics visuals (see `dcomp::CompositionTree::new`).
        if self.comp.is_none() {
            self.comp = dcomp::CompositionTree::new(self.host_hwnd);
        }
        let mut slot = self.d2d.borrow_mut();
        if slot.is_none() {
            *slot = Some(
                d2d::D2dRenderer::new()
                    .unwrap_or_else(|e| panic!("Direct2D renderer: device creation failed: {e}")),
            );
        }
        let result = slot.as_mut().expect("just created").render_window(self, w, h);
        if let Err(e) = result {
            // A lost device (driver update, GPU reset) invalidates every
            // resource: drop the renderer and rebuild it on the next paint.
            eprintln!("backend-windows: Direct2D frame failed ({e}); recreating the renderer");
            *slot = None;
            drop(slot);
            self.invalidate();
        }
    }

    /// The Direct2D renderer's pixels for the current scene at `w × h`
    /// device px (top-down premultiplied BGRA), or `None` on the GDI
    /// renderer. Screenshots and tests.
    #[cfg_attr(not(any(test, feature = "robot")), allow(dead_code))]
    pub(crate) fn render_d2d_offscreen(&self, w: i32, h: i32) -> Option<Result<Vec<u8>, String>> {
        if self.renderer != RendererKind::D2d {
            return None;
        }
        let mut slot = self.d2d.borrow_mut();
        if slot.is_none() {
            match d2d::D2dRenderer::new() {
                Ok(r) => *slot = Some(r),
                Err(e) => return Some(Err(format!("Direct2D device creation failed: {e}"))),
            }
        }
        Some(
            slot.as_mut()
                .expect("just created")
                .render_offscreen(self, w, h)
                .map_err(|e| format!("Direct2D offscreen frame failed: {e}")),
        )
    }

    /// The pressable handler under the client-area point `(x, y)` — in
    /// DEVICE pixels, exactly as the mouse message's `lParam` carries it;
    /// hit-testing runs in logical pixels. The host fires the handler
    /// AFTER releasing the backend borrow (the handler writes signals
    /// whose effects re-borrow the backend).
    pub fn pressable_action(&self, x: f32, y: f32) -> Option<Rc<dyn Fn()>> {
        let (x, y) = self.dpi.point_to_logical(x, y);
        scene::pressable_at(self, x, y)
    }

    /// Route a wheel tick (`delta_px` > 0 scrolls content down/right, in
    /// LOGICAL pixels) to the scroll view under the client-area point
    /// `(x, y)` (DEVICE pixels, as the message carries it). Returns true
    /// when consumed.
    pub fn wheel_scroll(&mut self, x: f32, y: f32, delta_px: f32) -> bool {
        let (x, y) = self.dpi.point_to_logical(x, y);
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

    // `register_external` / `has_external` lived here. Third-party leaves
    // now register on the scene `Registry` (`registry.register::<T, _>`),
    // which is typed and collision-free per payload `TypeId` — see
    // CLAUDE.md §3. The backend keeps no external table of its own.

    /// SDK extension helper: allocate a fresh Win32 control id + install
    /// `on_click` as its WM_COMMAND handler.
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
                Some(self.host_hwnd),
                Some(hmenu),
                None,
                None,
            )
        }
        .unwrap_or(HWND(std::ptr::null_mut()));
        let _ = unsafe { ShowWindow(hwnd, SW_SHOW) };
        // Modern shell font — without it controls render in the legacy
        // bitmap "System" font.
        if !hwnd.is_invalid() && !self.control_font.is_invalid() {
            unsafe {
                SendMessageW(hwnd, WM_SETFONT, Some(WPARAM(self.control_font.0 as usize)), Some(LPARAM(1)));
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
        if let Some(dw) = &self.dwrite {
            return dw.measure_line(text, key.unwrap_or(&self.default_font_key), self.line_height);
        }
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
        let max_lines = t.max_lines;
        let plan = Rc::new(match &self.dwrite {
            // DirectWrite measures in the font it draws — with no key, the
            // default key the painter draws with.
            Some(dw) => dw.build_plan(
                &content,
                key.as_ref().unwrap_or(&self.default_font_key),
                self.line_height as f32,
            ),
            None => {
                let hfont = match &key {
                    Some(k) => font::entry_for(&mut self.font_cache, k)
                        .map(|e| e.hfont)
                        .unwrap_or(self.ui_font),
                    None => self.ui_font,
                };
                wrap::build_gdi(&content, hfont, self.line_height as f32)
            }
        });
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
                Rc::new(move |known, avail| {
                    wrap::measure_size(&plan, advance, max_lines, known, avail)
                }),
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
                    let _ = SetWindowRgn(hwnd, Some(rgn), true);
                }
                None => {
                    let _ = SetWindowRgn(hwnd, None, true);
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
            Some(runtime_shared::FontFamily::System(name)) => {
                font::resolve_family_stack(name, &self.default_font_key.family)
            }
            // Registered typefaces are process-private (AddFontMemResourceEx)
            // and invisible to GDI+'s system collection — do NOT probe
            // them, trust the registered name.
            Some(runtime_shared::FontFamily::Typeface(tf)) => tf.family_name.to_string(),
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
                runtime_shared::scheduling::after_ms_detached(0, move || lost());
            }
        }
        // A fetch still in flight for a removed image finds no events
        // entry and is dropped.
        self.image_events.remove(&id);
        self.view_input.forget(id);
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
        // The client rect is device pixels; layout runs in logical ones.
        let (width, height) = self.logical_client_size();
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        self.compute_layout_roots(root_layout, width, height);

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
                // (read-only) never re-wraps. Same pure `lines_limited`
                // the measure fn used → breaks (and `max_lines` cuts)
                // can't disagree with the height Taffy was told.
                // Wrap at the CONTENT width: the measure fn reported the
                // content box, and Taffy added the padding around it.
                let (_, _, content_w, _) =
                    painted_geom::content_box(frame.width, frame.height, meta.content_inset);
                if let NodeKind::Text(t) = &mut meta.kind {
                    if let Some(plan) = t.plan.clone() {
                        let stale = t
                            .lines
                            .as_ref()
                            .map(|l| (l.width - content_w).abs() > 0.25)
                            .unwrap_or(true);
                        if stale {
                            t.lines = Some(wrap::WrappedLines {
                                width: content_w,
                                lines: plan.lines_limited(content_w, t.max_lines),
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
            // Surfaces are sized in PHYSICAL pixels: a DPI change with an
            // unchanged logical frame is a real resize (bigger swapchain).
            let ev = graphics::layout_event(
                st.on_ready.is_none(),
                st.last_size,
                (self.dpi.to_device(meta.frame.2), self.dpi.to_device(meta.frame.3)),
            );
            if let Some(ev) = ev {
                gfx_events.push((*id, ev));
            }
        }
        if !gfx_events.is_empty() {
            let weak = self.self_ref.clone();
            runtime_shared::scheduling::after_ms_detached(0, move || {
                dispatch_graphics_events(&weak, &gfx_events);
            });
        }

        // `on_layout` subscribers whose view changed size — on a fresh
        // turn, for the same reason as the graphics events.
        let due = self.layout_subs_due();
        if !due.is_empty() {
            runtime_shared::scheduling::after_ms_detached(0, move || layout_subs::fire(due));
        }
    }

    /// Which painted nodes go above which graphics surfaces this frame
    /// (`layers`). Nothing to plan without graphics nodes.
    pub(crate) fn layer_plan(&self) -> layers::LayerPlan {
        if self.graphics.is_empty() {
            return layers::LayerPlan::default();
        }
        layers::plan(self, |id| {
            self.graphics.get(&id).is_some_and(|st| st.visible.load(std::sync::atomic::Ordering::Relaxed))
        })
    }

    /// Show this frame's lifted layers (and drop last frame's that are
    /// gone).
    pub(crate) fn present_overlays(&self, lifted: Vec<overlay::LayerPixels>) {
        let Some(comp) = &self.comp else { return };
        let graphics = &self.graphics;
        self.overlays.borrow_mut().present(
            comp,
            |id| graphics.get(&id).map(|st| st.visuals.container.clone()),
            lifted,
        );
    }

    /// Lay out from `node`, filling the window. A root that is itself a
    /// reactive anchor has no box: its children (through any nested
    /// anchors) are the layout roots.
    fn compute_layout_roots(&mut self, node: runtime_layout::LayoutNode, width: f32, height: f32) {
        if self.layout.is_contents(node) {
            for child in self.layout.logical_children_of(node) {
                self.compute_layout_roots(child, width, height);
            }
        } else {
            self.layout.compute(node, width, height);
        }
    }

    /// The `on_layout` callbacks owed a size by the current frames
    /// (`layout_subs::due`). Hidden nodes keep their last size.
    pub(crate) fn layout_subs_due(&self) -> layout_subs::Due {
        if !layout_subs::any() {
            return Vec::new();
        }
        layout_subs::due(|id| self.nodes.get(&id).filter(|m| !m.hidden).map(|m| (m.frame.2, m.frame.3)))
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
                // Child windows live in DEVICE pixels: place the
                // edge-snapped device rect, and cut the region from the
                // clip in device space too (radii scale with it).
                // Painted-scene clips (`overflow: hidden`, scroll
                // boxes, rounded wrappers) can't touch a native child
                // HWND — express the nearest clipping ancestor as a
                // WINDOW REGION instead, so a control inside a scroll
                // view stops at the viewport edge. Regions can only
                // hold ONE shape, so controls get the chain's
                // single-clip approximation; graphics visuals get the
                // full two-channel chain.
                let dpi = self.dpi;
                let (dx, dy, dw, dh) = dpi.rect_to_device((ax, ay, w, h));
                let region = hwnd_clip_region(
                    (dx as f32, dy as f32, dw as f32, dh as f32),
                    clip.legacy().map(|(r, rad)| (dpi.rect_to_device_f(r), dpi.to_device(rad))),
                );
                control_moves.push((hwnd, dx, dy, dw, dh, region));
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
            // Composition visuals are positioned/clipped in DEVICE pixels.
            let placement = dcomp::visual_placement(
                self.dpi.rect_to_device_f(abs),
                clip.scaled(self.dpi.factor()),
                hidden,
            );
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
        let (w, h) = self.dpi.physical_size(meta.frame.2, meta.frame.3);
        if w <= 1 || h <= 1 {
            return None;
        }
        let scale = self.dpi.factor();
        let st = self.graphics.get_mut(&id)?;
        st.last_size = Some((w, h));
        match ev {
            graphics::GfxEvent::Ready => st
                .on_ready
                .take()
                .map(|cb| TakenGfx::Ready(cb, st.surface.clone(), (w, h), scale)),
            graphics::GfxEvent::Resize => {
                st.on_resize.take().map(|cb| TakenGfx::Resize(cb, (w, h), scale))
            }
        }
    }

    // ---------------------------------------------------------------
    // image() loading — `docs/image-src.md`. Routing / decoding policy is
    // `image_policy` (host-tested in `tests/image_src.rs`); GDI+ is
    // `image.rs`; the HTTP fetch + UI-thread pump is `image_fetch.rs`.
    // ---------------------------------------------------------------

    /// Start loading `src` into image node `id`: decode synchronously
    /// (`data:`, assets), start a fetch (`http(s)`), or fail (rejected /
    /// malformed / missing). Every outcome reaches `on_load` / `on_error`
    /// through a deferred delivery — this runs inside a backend borrow.
    fn load_image_src(&mut self, id: u64, src: &str) {
        let current = match self.nodes.get(&id).map(|m| &m.kind) {
            Some(NodeKind::Image(p)) => p.src.as_deref(),
            _ => return,
        };
        let resolution = image_policy::resolve(
            current,
            src,
            &self.assets,
            runtime_shared::image_src::read_bundled_asset,
        );
        if matches!(resolution, image_policy::Resolution::Unchanged) {
            return;
        }
        if let Some(NodeKind::Image(p)) = self.nodes.get_mut(&id).map(|m| &mut m.kind) {
            p.src = Some(src.to_string());
        }
        let generation = self.image_events.entry(id).or_default().begin();
        let result = match resolution {
            image_policy::Resolution::Unchanged => return,
            image_policy::Resolution::Fetch(url) => {
                // The previous image stays up until the new one settles
                // (a browser `<img>` does the same), so a src swap doesn't
                // flash an empty box.
                self.ensure_image_pump();
                match &self.image_pump {
                    Some(pump) => {
                        pump.fetch(id, generation, url);
                        return;
                    }
                    None => Err(image_policy::Failure::Network("no UI-thread pump".into())),
                }
            }
            image_policy::Resolution::Ready(decoded) => {
                image::realize(decoded).ok_or(image_policy::Failure::Undecodable)
            }
            image_policy::Resolution::Failed(f) => Err(f),
        };
        self.settle_image(id, generation, result);
        self.queue_image_delivery(id);
    }

    /// Install load `generation`'s result on node `id` — unless a newer
    /// `src` superseded it. Sets the bitmap, the intrinsic size (natural
    /// size on success, 0×0 on failure, like every other backend), and the
    /// outcome the next delivery reports.
    fn settle_image(
        &mut self,
        id: u64,
        generation: u64,
        result: Result<image::ImageSource, image_policy::Failure>,
    ) {
        let outcome = match &result {
            Ok(source) => {
                let (width, height) = source.natural();
                image_policy::Outcome::Loaded { width, height }
            }
            Err(_) => image_policy::Outcome::Failed,
        };
        let Some(events) = self.image_events.get_mut(&id) else { return };
        if !events.settle(generation, outcome) {
            return;
        }
        let Some(NodeKind::Image(p)) = self.nodes.get_mut(&id).map(|m| &mut m.kind) else {
            return;
        };
        if let Err(failure) = &result {
            failure.report(p.src.as_deref().unwrap_or(""));
        }
        // Assigning drops the previous image, disposing its GpImage.
        p.source = result.ok();
        let (w, h) = match outcome {
            image_policy::Outcome::Loaded { width, height } => (width.round() as i32, height.round() as i32),
            image_policy::Outcome::Failed => (0, 0),
        };
        let node = WindowsNode { id, hwnd: HWND(std::ptr::null_mut()) };
        self.set_intrinsic(&node, w, h);
        self.invalidate();
    }

    fn ensure_image_pump(&mut self) {
        if self.image_pump.is_none() {
            self.image_pump = image_fetch::ImagePump::new(self.self_ref());
        }
    }

    /// Fire node `id`'s owed image event from the pump, outside this borrow.
    fn queue_image_delivery(&mut self, id: u64) {
        self.ensure_image_pump();
        if let Some(pump) = &self.image_pump {
            pump.queue_delivery(id);
        }
    }

    /// Pump side of [`Self::queue_image_delivery`]: run the handler call
    /// node `id` owes with NO backend borrow held (author handlers write
    /// signals → effects → re-enter the backend). `false` when the backend
    /// is borrowed right now (a nested message loop inside a backend call)
    /// — the pump re-posts and tries again.
    pub(crate) fn deliver_image_events(me: &Weak<RefCell<Self>>, id: u64) -> bool {
        let Some(rc) = me.upgrade() else { return true };
        let fire = match rc.try_borrow_mut() {
            Ok(mut b) => b.image_events.get_mut(&id).and_then(|e| e.take_fire()),
            Err(_) => return false,
        };
        if let Some(fire) = fire {
            fire.call();
        }
        true
    }

    /// Pump side of a finished fetch: decode, settle (dropped if
    /// superseded), then fire the owed handler with no borrow held. Hands
    /// the result back when the backend is borrowed (the pump re-posts).
    pub(crate) fn apply_image_fetch(
        me: &Weak<RefCell<Self>>,
        done: image_fetch::FetchDone,
    ) -> Result<(), image_fetch::FetchDone> {
        let Some(rc) = me.upgrade() else { return Ok(()) };
        let Ok(mut b) = rc.try_borrow_mut() else { return Err(done) };
        let image_fetch::FetchDone { node_id, generation, result } = done;
        let result = result.and_then(|bytes| {
            image_policy::decode(std::borrow::Cow::Owned(bytes))
                .and_then(|d| image::realize(d).ok_or(image_policy::Failure::Undecodable))
        });
        b.settle_image(node_id, generation, result);
        let fire = b.image_events.get_mut(&node_id).and_then(|e| e.take_fire());
        drop(b);
        if let Some(fire) = fire {
            fire.call();
        }
        Ok(())
    }

    pub(crate) fn install_image_load_handler(
        &mut self,
        node: &WindowsNode,
        handler: runtime_shared::primitives::image::ImageLoadHandler,
    ) {
        self.image_events.entry(node.id).or_default().install_load(handler);
        // Replay: a data:/asset source decoded inside `create_image`,
        // before the walker installed this.
        if self.image_events[&node.id].outcome().is_some() {
            self.queue_image_delivery(node.id);
        }
    }

    pub(crate) fn install_image_error_handler(
        &mut self,
        node: &WindowsNode,
        handler: runtime_shared::primitives::image::ImageErrorHandler,
    ) {
        self.image_events.entry(node.id).or_default().install_error(handler);
        if self.image_events[&node.id].outcome().is_some() {
            self.queue_image_delivery(node.id);
        }
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
        runtime_shared::primitives::graphics::OnReady,
        runtime_shared::primitives::graphics::GraphicsSurface,
        (u32, u32),
        f32,
    ),
    Resize(runtime_shared::primitives::graphics::OnResize, (u32, u32), f32),
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
/// `size` is the surface's PHYSICAL pixel size and `scale` the window's
/// DPI scale (the `OnReadyEvent` contract) — a renderer drawing a logical
/// scene applies `scale` as its base transform.
fn dispatch_graphics_events(
    weak: &Weak<RefCell<WindowsBackend>>,
    events: &[(u64, graphics::GfxEvent)],
) {
    use runtime_shared::primitives::graphics::{GraphicsTarget, OnReadyEvent, OnResizeEvent};
    for &(id, ev) in events {
        let Some(backend) = weak.upgrade() else {
            return;
        };
        let taken = backend.borrow_mut().take_graphics_callback(id, ev);
        match taken {
            Some(TakenGfx::Ready(mut cb, surface, size, scale)) => {
                cb(OnReadyEvent {
                    target: GraphicsTarget::RawWindow(surface),
                    size,
                    scale,
                });
            }
            Some(TakenGfx::Resize(mut cb, size, scale)) => {
                cb(OnResizeEvent { size, scale });
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
        let dc = GetDC(None);
        if dc.is_invalid() {
            return (0, line_height);
        }
        let prev = SelectObject(dc, HGDIOBJ(hfont.0));
        let mut size = SIZE::default();
        let ok = GetTextExtentPoint32W(dc, &wide, &mut size).as_bool();
        SelectObject(dc, prev);
        ReleaseDC(None, dc);
        if ok {
            (size.cx, size.cy)
        } else {
            (0, line_height)
        }
    }
}

/// The shell message font (Segoe UI on modern Windows) at its LOGICAL
/// (96-DPI) size, returning `(logfont, hfont, line_height, em_size,
/// family_name)` — all metrics in logical px.
///
/// A per-monitor-aware process gets `SPI_GETNONCLIENTMETRICS` pre-scaled
/// for the SYSTEM DPI (the primary monitor's at login), which would bake
/// one monitor's scale into every logical text size. Ask for the 96-DPI
/// metrics directly (`SystemParametersInfoForDpi`, Windows 10 1607+); on
/// older systems normalize the plain query by the system DPI instead.
fn create_ui_font() -> (LOGFONTW, HFONT, i32, i32, String) {
    use windows::Win32::UI::HiDpi::{GetDpiForSystem, SystemParametersInfoForDpi};
    const FALLBACK: (i32, i32) = (16, 12);
    unsafe {
        let mut ncm = NONCLIENTMETRICSW {
            cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32,
            ..Default::default()
        };
        let ptr = &mut ncm as *mut _ as *mut std::ffi::c_void;
        let ok = if SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS.0,
            ncm.cbSize,
            Some(ptr),
            0,
            dpi::BASE_DPI,
        )
        .is_ok()
        {
            true
        } else if SystemParametersInfoW(
            SPI_GETNONCLIENTMETRICS,
            ncm.cbSize,
            Some(ptr),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .is_ok()
        {
            let h = ncm.lfMessageFont.lfHeight;
            let logical = dpi::metric_to_logical(h.abs(), GetDpiForSystem());
            ncm.lfMessageFont.lfHeight = if h < 0 { -logical } else { logical };
            true
        } else {
            false
        };
        if !ok {
            return (
                LOGFONTW::default(),
                HFONT(std::ptr::null_mut()),
                FALLBACK.0,
                FALLBACK.1,
                String::new(),
            );
        }
        let logfont = ncm.lfMessageFont;
        // `lfHeight` is negative for an em height; take its magnitude
        // as the base font size.
        let base_size = logfont.lfHeight.abs().max(1);
        let face = &logfont.lfFaceName;
        let face_len = face.iter().position(|&c| c == 0).unwrap_or(face.len());
        let base_family = String::from_utf16_lossy(&face[..face_len]);
        let font = CreateFontIndirectW(&logfont);
        if font.is_invalid() {
            return (logfont, HFONT(std::ptr::null_mut()), FALLBACK.0, FALLBACK.1, base_family);
        }
        let mut line_height = 16;
        let dc = GetDC(None);
        if !dc.is_invalid() {
            let prev = SelectObject(dc, HGDIOBJ(font.0));
            let sample: Vec<u16> = "Mg".encode_utf16().collect();
            let mut size = SIZE::default();
            if GetTextExtentPoint32W(dc, &sample, &mut size).as_bool() {
                line_height = size.cy;
            }
            SelectObject(dc, prev);
            ReleaseDC(None, dc);
        }
        (logfont, font, line_height, base_size, base_family)
    }
}

/// The shell font at `dpi`'s DEVICE size — native controls draw their own
/// text in device pixels, so their `WM_SETFONT` font must be the logical
/// font scaled (a logical-size font renders half-size text inside a
/// correctly-sized 2× button). Null when the shell font is unavailable
/// (controls then keep their class default).
fn create_control_font(logical: &LOGFONTW, dpi: dpi::DpiScale) -> HFONT {
    if logical.lfFaceName[0] == 0 && logical.lfHeight == 0 {
        return HFONT(std::ptr::null_mut());
    }
    let mut lf = *logical;
    let device = dpi.font_px_to_device(lf.lfHeight.abs());
    lf.lfHeight = if logical.lfHeight < 0 { -device } else { device };
    unsafe { CreateFontIndirectW(&lf) }
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
        for font in [self.ui_font, self.control_font] {
            if !font.is_invalid() {
                let _ = unsafe { DeleteObject(HGDIOBJ(font.0)) };
            }
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
fn resolve_color(t: &Option<runtime_shared::Tokenized<Color>>) -> Option<Rgba> {
    t.as_ref()
        .map(|c| runtime_shared::color::parse_or(&c.resolve().0, Rgba::TRANSPARENT))
}

/// Corner radius in px, or 0.
fn resolve_radius(t: &Option<runtime_shared::Tokenized<Length>>) -> f32 {
    match t.as_ref().map(|x| x.resolve()) {
        Some(Length::Px(v)) => v,
        // Resolved before the box is known; Direct2D clamps the rounded rect.
        // See `Length::FULL_RADIUS_FALLBACK_PX`.
        Some(Length::Full) => Length::FULL_RADIUS_FALLBACK_PX,
        _ => 0.0,
    }
}

/// A padding side in px, or 0. Px only, like the GTK backend's leaf
/// margins: a percentage resolves against the PARENT's width, which Taffy
/// knows and the painter does not.
fn resolve_padding(t: &Option<runtime_shared::Tokenized<Length>>) -> f32 {
    match t.as_ref().map(|x| x.resolve()) {
        Some(Length::Px(v)) => v.max(0.0),
        _ => 0.0,
    }
}

/// Border-width slot in px, or 0.
fn resolve_width(t: &Option<runtime_shared::Tokenized<f32>>) -> f32 {
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
            let rgba = runtime_shared::color::parse_or(&s.color.0, Rgba::TRANSPARENT);
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
///
/// Direction components within float noise of zero snap TO zero, so the
/// axis angles are exactly axis-aligned: `sin(180°)` in f32 is −8.7e-8,
/// and Wine's GDI+ line brush fills a NEARLY-vertical axis with one solid
/// color (the whole 180° gradient painted as its first stop).
pub(crate) fn linear_points(angle_deg: f32, w: f32, h: f32) -> ((f32, f32), (f32, f32)) {
    let rad = angle_deg.to_radians();
    let snap = |v: f32| if v.abs() < 1e-6 { 0.0 } else { v };
    let dx = snap(rad.sin());
    let dy = snap(-rad.cos());
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

// The pre-v2 `RegisterNavigator` / `RegisterExternal` impls lived here.
// Both tables are gone in v2: third-party leaves register on the scene
// `Registry` instead (CLAUDE.md §3), and navigators are an extension
// rather than a backend table. The inherent `register_external` above
// is what the scene Registry mount path uses.

// =========================================================================
// Primitive bodies — inherent, not a trait impl
// =========================================================================
//
// These were `impl Backend for WindowsBackend` before the v2 port. The
// `Backend` mega-trait is gone; `newcore.rs` implements
// `runtime_scene::Host` plus the `caps::*Ops` traits and delegates here.
// Same split the GTK backend uses (`port(linux): make the GTK bodies
// inherent`) — one body per primitive, reachable from the caps impls
// without a trait in between.

impl WindowsBackend {

    fn create_view(&mut self, _a11y: &AccessibilityProps) -> WindowsNode {
        self.add_node(NodeKind::View(ViewVisual::default()))
    }

    /// A reactive region's anchor over a `display: contents` layout node
    /// (`LayoutTree::new_contents_node`): its children lay out in the
    /// nearest real ancestor's flex context, and its frame is that
    /// ancestor's box at the origin, so the painter and hit walk place them
    /// with no translation. It paints nothing and is never a hit target.
    pub(crate) fn create_anchor(&mut self) -> WindowsNode {
        let id = self.alloc_id();
        let layout = self.layout.new_contents_node();
        self.layout_for_id.insert(id, layout);
        let mut meta = NodeMeta::new(NodeKind::View(ViewVisual::default()));
        meta.contents = true;
        self.nodes.insert(id, meta);
        WindowsNode { id, hwnd: HWND(std::ptr::null_mut()) }
    }

    fn create_pressable(
        &mut self,
        on_click: Rc<dyn Fn()>,
        _a11y: &AccessibilityProps,
    ) -> WindowsNode {
        // A Pressable is a styleable painted container carrying an
        // on_click; the host's hit-tested WM_LBUTTONUP fires it.
        self.add_node(NodeKind::View(ViewVisual {
            on_click: Some(on_click),
            ..Default::default()
        }))
    }

    fn create_link(
        &mut self,
        config: runtime_shared::primitives::link::LinkConfig,
        _a11y: &AccessibilityProps,
    ) -> WindowsNode {
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

    fn create_text(&mut self, content: &str, _a11y: &AccessibilityProps) -> WindowsNode {
        let node = self.add_node(NodeKind::Text(TextVisual::new(content)));
        self.set_text_measure(node.id);
        node
    }

    fn create_button(
        &mut self,
        label: &str,
        on_click: &Action,
        _leading_icon: Option<&runtime_shared::primitives::icon::IconData>,
        _trailing_icon: Option<&runtime_shared::primitives::icon::IconData>,
        _a11y: &AccessibilityProps,
    ) -> WindowsNode {
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

    fn insert(&mut self, parent: &mut WindowsNode, child: WindowsNode) {
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

    fn clear_children(&mut self, node: &WindowsNode) {
        let Some(child_ids) = self.children.remove(&node.id) else {
            return;
        };
        for child_id in child_ids {
            self.remove_subtree(child_id);
        }
        self.layout_dirty = true;
        self.invalidate();
    }

    fn update_text(&mut self, node: &WindowsNode, content: &str) {
        if let Some(meta) = self.nodes.get_mut(&node.id) {
            if let NodeKind::Text(t) = &mut meta.kind {
                t.content = content.to_string();
            }
        }
        self.set_text_measure(node.id);
        self.invalidate();
    }

    fn update_button_label(&mut self, node: &WindowsNode, label: &str) {
        let wide = to_pcwstr(label);
        let _ = unsafe { SetWindowTextW(node.hwnd, wide.as_pcwstr()) };
        let (tw, th) = self.measure_with_key(label, None);
        self.set_intrinsic(node, tw + 24, th + 8);
    }

    fn finish(&mut self, root: WindowsNode) {
        self.root_id = Some(root.id);
        self.layout_pass();
        self.invalidate();
    }

    fn create_image(
        &mut self,
        src: &str,
        _alt: Option<&str>,
        _a11y: &AccessibilityProps,
    ) -> WindowsNode {
        let node = self.add_node(NodeKind::Image(image::ImagePaint::default()));
        // Natural size becomes the intrinsic size once decoded (explicit
        // style width/height override it via `set_style`).
        self.load_image_src(node.id, src);
        node
    }

    fn create_icon(
        &mut self,
        data: &runtime_shared::primitives::icon::IconData,
        color: Option<&Color>,
        _a11y: &AccessibilityProps,
    ) -> WindowsNode {
        // No color → opaque black (the native analogue of web's
        // `currentColor`, matching the Linux default).
        let rgba = color
            .map(|c| runtime_shared::color::parse_or(&c.0, Rgba::BLACK))
            .unwrap_or(Rgba::BLACK);
        let paint = icon::IconPaint::from_data(data, rgba);
        let (vw, vh) = paint.view_box;
        let node = self.add_node(NodeKind::Icon(paint));
        // Icons have no intrinsic content size; default to the view-box
        // so a bare `icon(X)` is visible under flex (`.size(n)` overrides).
        self.set_intrinsic(&node, vw as i32, vh as i32);
        node
    }

    fn update_image_src(&mut self, node: &WindowsNode, src: &str) {
        self.load_image_src(node.id, src);
    }

    fn update_icon_color(&mut self, node: &WindowsNode, color: &Color) {
        if let Some(meta) = self.nodes.get_mut(&node.id) {
            if let NodeKind::Icon(p) = &mut meta.kind {
                p.color = runtime_shared::color::parse_or(&color.0, Rgba::BLACK);
            }
        }
        self.invalidate();
    }

    fn update_icon_data(
        &mut self,
        node: &WindowsNode,
        data: &runtime_shared::primitives::icon::IconData,
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
        _on_key_down: Option<runtime_shared::primitives::key::KeyDownHandler>,
        _on_blur: Option<runtime_shared::primitives::text_input::BlurHandler>,
        secure: bool,
        _a11y: &AccessibilityProps,
    ) -> WindowsNode {
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
        _on_key_down: Option<runtime_shared::primitives::key::KeyDownHandler>,
        _a11y: &AccessibilityProps,
    ) -> WindowsNode {
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
    ) -> WindowsNode {
        // A `BUTTON` with `BS_AUTOCHECKBOX` maintains its own checked
        // state on click, then fires `BN_CLICKED`.
        let control_id = self.alloc_control_id();
        let hwnd = self.create_control_hwnd(class_button(), "", BS_AUTOCHECKBOX, Some(control_id));
        unsafe {
            SendMessageW(
                hwnd,
                BM_SETCHECK,
                Some(WPARAM(if initial_value { BST_CHECKED as usize } else { 0 })),
                Some(LPARAM(0)),
            );
        }
        let action: Rc<dyn Fn()> = Rc::new(move || {
            let checked =
                unsafe { SendMessageW(hwnd, BM_GETCHECK, Some(WPARAM(0)), Some(LPARAM(0))) }.0 == BST_CHECKED;
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

    /// Native inert state for the `disabled` prop (`StyleOps::set_disabled`).
    /// Was the trait's no-op default, so a disabled `text_input` (a real
    /// `EDIT` control) still took focus on click and accepted typing — the
    /// vocabulary's gate only dropped the resulting `on_change`.
    ///
    /// Child-window controls (`EDIT`, `BUTTON`, checkbox, trackbar) get
    /// `EnableWindow(FALSE)`: Win32's own disabled state, which refuses
    /// mouse + keyboard input, can't be focused by click or Tab, greys the
    /// control and reports disabled to UIA. Focus the control holds is
    /// handed back to its parent first — `EnableWindow` does not move the
    /// focus off a window it disables, and a disabled window holding focus
    /// leaves the app's keyboard shortcuts dead until the user clicks
    /// elsewhere. Painted nodes (`view` / `pressable` / `link`) are never
    /// focus targets on this backend and the vocabulary's press block
    /// already gates their click, so there is nothing native to change.
    fn set_disabled(&mut self, node: &WindowsNode, disabled: bool) {
        use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, GetFocus, SetFocus};
        use windows::Win32::UI::WindowsAndMessaging::GetParent;
        let Some(NodeKind::Control { hwnd }) = self.nodes.get(&node.id).map(|m| &m.kind) else {
            return;
        };
        let hwnd = *hwnd;
        unsafe {
            if disabled && GetFocus() == hwnd {
                if let Ok(parent) = GetParent(hwnd) {
                    let _ = SetFocus(Some(parent));
                }
            }
            let _ = EnableWindow(hwnd, !disabled);
        }
    }

    fn update_toggle_value(&mut self, node: &WindowsNode, value: bool) {
        unsafe {
            SendMessageW(
                node.hwnd,
                BM_SETCHECK,
                Some(WPARAM(if value { BST_CHECKED as usize } else { 0 })),
                Some(LPARAM(0)),
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
    ) -> WindowsNode {
        ensure_common_controls();
        let hwnd = self.create_control_hwnd(class_trackbar(), "", TBS_HORZ, None);
        unsafe {
            // TBM_SETRANGE takes MAKELONG(min, max) in lParam.
            let range = (SLIDER_RESOLUTION as isize) << 16;
            SendMessageW(hwnd, TBM_SETRANGE, Some(WPARAM(1)), Some(LPARAM(range)));
            let pos = value_to_slider_pos(initial_value, min, max);
            SendMessageW(hwnd, TBM_SETPOS, Some(WPARAM(1)), Some(LPARAM(pos as isize)));
        }
        let action: Rc<dyn Fn()> = Rc::new(move || {
            let pos = unsafe { SendMessageW(hwnd, TBM_GETPOS, Some(WPARAM(0)), Some(LPARAM(0))) }.0 as i32;
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
    ) -> WindowsNode {
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

    fn set_node_scroll(&mut self, node: &WindowsNode, x: f32, y: f32) {
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
        _size: runtime_shared::primitives::activity_indicator::ActivityIndicatorSize,
        _color: Option<&Color>,
        _a11y: &AccessibilityProps,
    ) -> WindowsNode {
        // A marquee progress bar is the native indeterminate-activity
        // affordance; comctl32 animates it on its own timer.
        ensure_common_controls();
        let hwnd = self.create_control_hwnd(class_progress(), "", PBS_MARQUEE, None);
        unsafe {
            SendMessageW(hwnd, PBM_SETMARQUEE, Some(WPARAM(1)), Some(LPARAM(30)));
        }
        let node = self.add_node(NodeKind::Control { hwnd });
        self.set_intrinsic(&node, 160, 16);
        node
    }

    fn create_virtualizer(
        &mut self,
        _callbacks: runtime_shared::VirtualizerCallbacks<WindowsNode>,
        _overscan: f32,
        _layout: runtime_shared::VirtualLayout,
        _a11y: &AccessibilityProps,
    ) -> WindowsNode {
        self.placeholder("Virtualizer not yet implemented on Windows backend")
    }

    fn create_graphics(
        &mut self,
        on_ready: runtime_shared::primitives::graphics::OnReady,
        on_resize: runtime_shared::primitives::graphics::OnResize,
        on_lost: runtime_shared::primitives::graphics::OnLost,
        _a11y: &AccessibilityProps,
    ) -> WindowsNode {
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


    fn create_portal(
        &mut self,
        _target: runtime_shared::primitives::portal::PortalTarget,
        _on_dismiss: Option<Rc<dyn Fn()>>,
        _trap_focus: bool,
        _a11y: &AccessibilityProps,
    ) -> WindowsNode {
        self.placeholder("Portal not yet implemented on Windows backend")
    }

    fn set_portal_hidden(&mut self, node: &WindowsNode, hidden: bool) {
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
            // GDI: native controls and the GDI painter.
            font::install_face(&face.source);
            // DirectWrite can't see GDI-private fonts (`dwrite_text`).
            if let Some(dw) = &self.dwrite {
                if let Err(e) = dw.register_face(&face.source) {
                    eprintln!("backend-windows: DirectWrite could not load a face of typeface {}: {e}", id.0);
                }
            }
        }
    }

    fn register_asset(&mut self, id: AssetId, _kind: AssetTag, source: &AssetSource) {
        self.assets.insert(id.0, image_policy::asset_entry(source));
    }

    fn set_app_background(&mut self, color: &runtime_shared::Tokenized<Color>) {
        self.app_background =
            Some(runtime_shared::color::parse_or(&color.resolve().0, Rgba::TRANSPARENT));
        self.invalidate();
    }

    fn set_animated_f32(&mut self, node: &WindowsNode, prop: AnimProp, value: f32) {
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

    fn set_animated_color(&mut self, node: &WindowsNode, prop: AnimProp, value: [f32; 4]) {
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

    fn make_view_handle(&self, node: &WindowsNode) -> runtime_shared::ViewHandle {
        handles::make_view_handle(self, node)
    }

    fn make_text_handle(&self, node: &WindowsNode) -> runtime_shared::TextHandle {
        handles::make_text_handle(self, node)
    }

    fn apply_style(&mut self, node: &WindowsNode, style: &Rc<StyleRules>) {
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
            meta.pointer_events = style.pointer_events;
            meta.content_inset = match &meta.kind {
                NodeKind::Text(_) | NodeKind::Code(_) | NodeKind::Icon(_) | NodeKind::Image(_) => [
                    resolve_padding(&style.padding_top),
                    resolve_padding(&style.padding_right),
                    resolve_padding(&style.padding_bottom),
                    resolve_padding(&style.padding_left),
                ],
                _ => [0.0; 4],
            };

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
                    v.border_style = style.border_style.unwrap_or_default();
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
                    // Assigned unconditionally (unlike the fields above):
                    // a style WITHOUT `max_lines` must lift a limit an
                    // earlier style set. The measure fn captures it.
                    let max_lines = style.max_lines.filter(|n| *n > 0);
                    if t.max_lines != max_lines {
                        t.max_lines = max_lines;
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
        runtime_shared::color::parse_or(s, Rgba::TRANSPARENT)
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

    #[test]
    fn regression_axis_angles_are_exactly_axis_aligned() {
        // A float-noise tilt (sin 180° = -8.7e-8 in f32) made Wine's line
        // brush paint the whole 180° gradient as one solid color.
        for angle in [0.0, 180.0, 360.0, -180.0] {
            let (s, e) = linear_points(angle, 120.0, 90.0);
            assert_eq!(s.0, e.0, "{angle}deg must be exactly vertical");
        }
        for angle in [90.0, 270.0, -90.0] {
            let (s, e) = linear_points(angle, 120.0, 90.0);
            assert_eq!(s.1, e.1, "{angle}deg must be exactly horizontal");
        }
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

#[cfg(test)]
mod disabled_tests {
    use super::*;
    use runtime_vocabulary::caps::StyleOps;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, IsWindowEnabled, SetFocus};
    use windows::Win32::UI::WindowsAndMessaging::{WS_OVERLAPPEDWINDOW, WS_VISIBLE};

    /// Regression: `set_disabled` was the trait's no-op default here, so a
    /// disabled `text_input` — a real `EDIT` control — stayed enabled:
    /// clickable, focusable, typeable, with only the vocabulary's gate
    /// dropping the `on_change`. Fails pre-fix: the EDIT is still enabled
    /// and keeps the focus. Needs a desktop session (real HWNDs), so it
    /// only runs on a Windows host.
    #[test]
    fn regression_windows_disabled_text_input_still_editable() {
        let title = to_pcwstr("disabled-test");
        let host = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(windows::core::w!("STATIC").as_ptr()),
                title.as_pcwstr(),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                0,
                0,
                400,
                300,
                None,
                None,
                None,
                None,
            )
        }
        .expect("host window");
        let mut be = WindowsBackend::new(host);
        let input = be.create_text_input("ab", None, Rc::new(|_| {}), None, None, false, &Default::default());
        let edit = input.hwnd;
        unsafe {
            let _ = SetFocus(Some(edit));
        }
        assert_eq!(unsafe { GetFocus() }, edit, "baseline: the enabled EDIT takes focus");

        StyleOps::set_disabled(&mut be, &input, true);
        assert!(!unsafe { IsWindowEnabled(edit) }.as_bool(), "Win32's own disabled state");
        assert_ne!(unsafe { GetFocus() }, edit, "a field that goes disabled drops its focus");

        StyleOps::set_disabled(&mut be, &input, false);
        assert!(unsafe { IsWindowEnabled(edit) }.as_bool(), "re-enabled: a live toggle, not a latch");
        unsafe {
            let _ = DestroyWindow(host);
        }
    }

    /// `toggle` (`BS_AUTOCHECKBOX`) and `slider` (trackbar) are child
    /// controls too: `set_disabled` puts them in Win32's disabled state and
    /// takes focus off them. Before the vocabulary bound `disabled` on these
    /// primitives, `set_disabled` never reached them. Needs a desktop
    /// session (real HWNDs), so it only runs on a Windows host.
    #[test]
    fn disabled_toggle_and_slider_are_not_enabled() {
        use runtime_vocabulary::caps::{SliderOps, ToggleOps};
        let title = to_pcwstr("disabled-controls-test");
        let host = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(windows::core::w!("STATIC").as_ptr()),
                title.as_pcwstr(),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                0,
                0,
                400,
                300,
                None,
                None,
                None,
                None,
            )
        }
        .expect("host window");
        let mut be = WindowsBackend::new(host);
        let toggle = be.create_toggle(false, Rc::new(|_| {}), &Default::default());
        let slider = be.create_slider(0.5, 0.0, 1.0, None, Rc::new(|_| {}), &Default::default());
        for node in [&toggle, &slider] {
            let hwnd = node.hwnd;
            unsafe {
                let _ = SetFocus(Some(hwnd));
            }
            StyleOps::set_disabled(&mut be, node, true);
            assert!(!unsafe { IsWindowEnabled(hwnd) }.as_bool(), "Win32's own disabled state");
            assert_ne!(unsafe { GetFocus() }, hwnd, "a control that goes disabled drops its focus");
            StyleOps::set_disabled(&mut be, node, false);
            assert!(unsafe { IsWindowEnabled(hwnd) }.as_bool(), "re-enabled");
        }
        unsafe {
            let _ = DestroyWindow(host);
        }
    }
}

/// Per-monitor DPI on a real window + GDI. Needs a desktop session (real
/// HWNDs, GDI+), so these run on a Windows host or under Wine
/// (`x86_64-pc-windows-gnu`). The pure conversion rules are host-tested
/// in `tests/dpi.rs`.
#[cfg(test)]
mod dpi_tests {
    use super::*;
    use runtime_shared::Position;
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, GetObjectW, MapWindowPoints,
        BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, WM_GETFONT, WS_POPUP, WS_VISIBLE};

    /// Device client size of the test window.
    const CLIENT: (i32, i32) = (800, 600);

    /// A frameless popup whose client area is exactly `CLIENT` device px.
    fn host_window() -> HWND {
        let title = to_pcwstr("dpi-test");
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(windows::core::w!("STATIC").as_ptr()),
                title.as_pcwstr(),
                WS_POPUP | WS_VISIBLE,
                0,
                0,
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

    /// Paint the scene at the window's device size into a 32-bpp DIB and
    /// return its pixels (`0x00RRGGBB`, row-major, top-down).
    fn render(b: &mut WindowsBackend) -> Vec<u32> {
        let (w, h) = CLIENT;
        unsafe {
            let dc = CreateCompatibleDC(None);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
            let bmp = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
                .expect("DIB section");
            let old = SelectObject(dc, HGDIOBJ(bmp.0));
            b.paint(dc, w, h);
            let px = std::slice::from_raw_parts(bits as *const u32, (w * h) as usize)
                .iter()
                .map(|p| p & 0x00FF_FFFF)
                .collect();
            SelectObject(dc, old);
            let _ = DeleteObject(HGDIOBJ(bmp.0));
            let _ = DeleteDC(dc);
            px
        }
    }

    fn at(px: &[u32], x: i32, y: i32) -> u32 {
        px[(y * CLIENT.0 + x) as usize]
    }

    const RED: u32 = 0x00FF_0000;

    /// A native control's rect in the host's client area, device px.
    fn control_client_rect(host: HWND, ctl: HWND) -> (i32, i32, i32, i32) {
        let mut r = RECT::default();
        unsafe {
            let _ = GetWindowRect(ctl, &mut r);
            let mut pts = [POINT { x: r.left, y: r.top }, POINT { x: r.right, y: r.bottom }];
            MapWindowPoints(None, Some(host), &mut pts);
            (pts[0].x, pts[0].y, pts[1].x - pts[0].x, pts[1].y - pts[0].y)
        }
    }

    fn control_font_height(ctl: HWND) -> i32 {
        unsafe {
            let font = SendMessageW(ctl, WM_GETFONT, Some(WPARAM(0)), Some(LPARAM(0)));
            let mut lf = LOGFONTW::default();
            let n = GetObjectW(
                HGDIOBJ(font.0 as *mut std::ffi::c_void),
                std::mem::size_of::<LOGFONTW>() as i32,
                Some(&mut lf as *mut LOGFONTW as *mut std::ffi::c_void),
            );
            assert!(n > 0, "control has a font");
            lf.lfHeight
        }
    }

    /// The backend had no DPI handling: layout ran against the DEVICE
    /// client rect and everything was drawn 1:1 in device pixels, so on a
    /// 200% monitor a `100×50` view covered 100×50 device px (half its
    /// size on screen, next to correctly-sized OS chrome), controls'
    /// fonts stayed 96-DPI tiny, and clicks hit-tested in the wrong units.
    /// At 192 DPI a 100×50 logical view must occupy 200×100 device px —
    /// painted, native, and hit-tested — and dropping back to 96 DPI
    /// (`WM_DPICHANGED`) must restore 1:1.
    #[test]
    /// A chart's tooltip — painted content over a graphics surface — showed
    /// UNDER the surface: everything painted sat below every composition
    /// visual. What the user sees (the desktop's composited pixels): the
    /// painted box is on top where it overlaps the canvas, the canvas
    /// everywhere else. Needs the real desktop compositor, so it skips
    /// under Wine (screen capture is black there).
    fn regression_painted_content_over_a_canvas_is_shown_above_it() {
        use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
        use windows::Win32::Graphics::Direct3D11::{
            D3D11CreateDevice, ID3D11Device, ID3D11RenderTargetView, ID3D11Texture2D,
            D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
        };
        use windows::Win32::Graphics::Dxgi::Common::{DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
        use windows::Win32::Graphics::Dxgi::{
            IDXGIDevice, IDXGIFactory2, DXGI_PRESENT, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            DXGI_USAGE_RENDER_TARGET_OUTPUT,
        };
        use windows::Win32::Graphics::Gdi::{GetDC, GetPixel, ReleaseDC};
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, PeekMessageW, SetWindowPos, HWND_TOPMOST, MSG, PM_REMOVE, SWP_SHOWWINDOW,
        };
        use windows::core::Interface;
        if crate::under_wine() {
            eprintln!("SKIP regression_painted_content_over_a_canvas_is_shown_above_it: needs the real desktop compositor");
            return;
        }
        let _serial = crate::render_test_lock();
        let host = host_window();
        unsafe {
            let _ = SetWindowPos(host, Some(HWND_TOPMOST), 0, 0, CLIENT.0, CLIENT.1, SWP_SHOWWINDOW);
        }
        let mut b = WindowsBackend::new(host);
        let a11y = AccessibilityProps::default();
        let mut root = b.create_view(&a11y);
        // A 400×300 canvas, then a red box overlapping it (absolute, so
        // it lies over the canvas) — after it in paint order.
        let canvas = runtime_vocabulary::caps::GraphicsOps::create_graphics(
            &mut b,
            Box::new(|_| {}),
            Box::new(|_| {}),
            Box::new(|| {}),
            &a11y,
        );
        b.apply_style(&canvas, &sized(400.0, 300.0, None));
        let canvas_id = canvas.id;
        let tip = b.create_view(&a11y);
        let mut ts = (*sized(100.0, 40.0, Some("#ff0000"))).clone();
        ts.position = Some(Position::Absolute.into());
        ts.left = Some(Length::Px(50.0).into());
        ts.top = Some(Length::Px(50.0).into());
        b.apply_style(&tip, &Rc::new(ts));
        b.insert(&mut root, canvas);
        b.insert(&mut root, tip);
        b.finish(root);

        unsafe {
            // Fill the canvas's visual with opaque magenta.
            let mut d3d: Option<ID3D11Device> = None;
            D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, windows::Win32::Foundation::HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut d3d), None, None)
                .expect("d3d11");
            let d3d = d3d.unwrap();
            let factory: IDXGIFactory2 = d3d.cast::<IDXGIDevice>().unwrap().GetAdapter().unwrap().GetParent().unwrap();
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: 400,
                Height: 300,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                ..Default::default()
            };
            let swap = factory.CreateSwapChainForComposition(&d3d, &desc, None).expect("swapchain");
            let back: ID3D11Texture2D = swap.GetBuffer(0).unwrap();
            let mut rtv: Option<ID3D11RenderTargetView> = None;
            d3d.CreateRenderTargetView(&back, None, Some(&mut rtv)).unwrap();
            d3d.GetImmediateContext().unwrap().ClearRenderTargetView(&rtv.unwrap(), &[1.0, 0.0, 1.0, 1.0]);
            swap.Present(0, DXGI_PRESENT(0)).ok().unwrap();
            b.graphics[&canvas_id].visuals.content.SetContent(&swap).unwrap();
            b.comp.as_ref().unwrap().commit();

            // Paint the scene (GDI+ draws into the window; the lifted box
            // goes to its overlay visual).
            let dc = GetDC(Some(host));
            b.paint(dc, CLIENT.0, CLIENT.1);
            ReleaseDC(Some(host), dc);

            let start = std::time::Instant::now();
            let mut msg = MSG::default();
            while start.elapsed().as_millis() < 700 {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    DispatchMessageW(&msg);
                }
                std::thread::sleep(std::time::Duration::from_millis(15));
            }
            let screen = GetDC(None);
            let on_tip = GetPixel(screen, 100, 70).0 & 0x00FF_FFFF;
            let on_canvas = GetPixel(screen, 300, 200).0 & 0x00FF_FFFF;
            ReleaseDC(None, screen);
            let _ = DestroyWindow(host);
            const MAGENTA: u32 = 0x00FF_00FF; // COLORREF 0x00BBGGRR
            const RED_REF: u32 = 0x0000_00FF;
            assert_eq!(on_canvas, MAGENTA, "the canvas shows where nothing covers it");
            assert_eq!(on_tip, RED_REF, "the painted box shows over the canvas");
        }
    }

    #[test]
    /// Reactive anchors were plain views — flex items the author never
    /// wrote — so a `height: 100%` child under an `if` laid out at 0 (the
    /// charts demo's plot stayed blank) and a row's children under a `for`
    /// stacked into a column, losing the gap. They are `display: contents`
    /// now: children lay out in the anchor's parent.
    fn regression_reactive_anchor_is_layout_transparent() {
        use runtime_scene::Host;
        let host = host_window();
        let mut b = WindowsBackend::new(host);
        let a11y = AccessibilityProps::default();
        let mut root = b.create_view(&a11y);

        // A 300px-tall box → anchor → a child filling it by percentage.
        let mut boxed = b.create_view(&a11y);
        b.apply_style(&boxed, &sized(800.0, 300.0, None));
        let mut anchor = Host::create_anchor(&mut b);
        let fill = b.create_view(&a11y);
        let mut fs = StyleRules::default();
        fs.width = Some(Length::pct(100.0).into());
        fs.height = Some(Length::pct(100.0).into());
        b.apply_style(&fill, &Rc::new(fs));
        let fill_id = fill.id;
        b.insert(&mut anchor, fill);
        b.insert(&mut boxed, anchor);

        // A row with a gap → anchor → two 50px chips.
        let mut row = b.create_view(&a11y);
        let mut rs = StyleRules::default();
        rs.flex_direction = Some(runtime_shared::FlexDirection::Row.into());
        rs.gap = Some(Length::Px(10.0).into());
        b.apply_style(&row, &Rc::new(rs));
        let mut chips = Host::create_anchor(&mut b);
        let a = b.create_view(&a11y);
        let c = b.create_view(&a11y);
        b.apply_style(&a, &sized(50.0, 50.0, None));
        b.apply_style(&c, &sized(50.0, 50.0, None));
        let (a_id, c_id) = (a.id, c.id);
        b.insert(&mut chips, a);
        b.insert(&mut chips, c);
        b.insert(&mut row, chips);

        b.insert(&mut root, boxed);
        b.insert(&mut root, row);
        b.finish(root);

        assert_eq!(b.node_frame(fill_id), Some((0.0, 0.0, 800.0, 300.0)), "the percentage resolves through the anchor");
        assert_eq!(b.node_frame(a_id), Some((0.0, 0.0, 50.0, 50.0)));
        assert_eq!(b.node_frame(c_id), Some((60.0, 0.0, 50.0, 50.0)), "the row and its gap reach through the anchor");
    }

    #[test]
    fn regression_layout_units_scale_with_dpi() {
        let host = host_window();
        let mut b = WindowsBackend::new(host);
        let a11y = AccessibilityProps::default();
        let mut root = b.create_view(&a11y);
        let red = b.create_pressable(Rc::new(|| {}), &a11y);
        b.apply_style(&red, &sized(100.0, 50.0, Some("#ff0000")));
        let red_id = red.id;
        let button = b.create_button("OK", &runtime_shared::derive::IntoAction::into_action(|| {}), None, None, &a11y);
        b.apply_style(&button, &sized(100.0, 50.0, None));
        let button_hwnd = button.hwnd;
        b.insert(&mut root, red);
        b.insert(&mut root, button);
        let root_id = root.id;
        b.set_dpi(192);
        b.finish(root);

        // Layout is logical: the 800×600 device client is a 400×300 viewport
        // the root lays out in.
        assert_eq!(b.scale(), 2.0);
        assert_eq!(b.logical_client_size(), (400.0, 300.0));
        assert_eq!(b.node_frame(root_id), Some((0.0, 0.0, 400.0, 300.0)));
        assert_eq!(b.node_frame(red_id), Some((0.0, 0.0, 100.0, 50.0)));

        // Painted: the red view fills device (0..200, 0..100).
        let px = render(&mut b);
        assert_eq!(at(&px, 10, 10), RED);
        assert_eq!(at(&px, 150, 75), RED, "inside 200×100 device px");
        assert_eq!(at(&px, 199, 99), RED);
        assert_ne!(at(&px, 205, 75), RED, "nothing beyond 200 device px");

        // Native: the button below it sits at logical (0, 50) = device (0, 100).
        assert_eq!(control_client_rect(host, button_hwnd), (0, 100, 200, 100));
        // …and draws its label in a device-scaled font.
        let logical_font = b.default_font_key.size_px;
        assert_eq!(control_font_height(button_hwnd), -(logical_font * 2));

        // Hit-testing takes device coordinates and tests logical frames.
        assert!(b.pressable_action(150.0, 75.0).is_some(), "device (150,75) = logical (75,37)");
        assert!(b.pressable_action(150.0, 120.0).is_none(), "device y 120 = logical 60: the button row");

        // The window moves to a 96-DPI monitor: back to 1:1, same logical layout.
        b.set_dpi(96);
        assert_eq!(b.logical_client_size(), (800.0, 600.0));
        assert_eq!(b.node_frame(root_id), Some((0.0, 0.0, 800.0, 600.0)));
        assert_eq!(b.node_frame(red_id), Some((0.0, 0.0, 100.0, 50.0)));
        assert_eq!(control_client_rect(host, button_hwnd), (0, 50, 100, 50));
        assert_eq!(control_font_height(button_hwnd), -logical_font);
        let px = render(&mut b);
        assert_eq!(at(&px, 50, 25), RED);
        assert_ne!(at(&px, 150, 75), RED, "1:1 again");
        unsafe {
            let _ = DestroyWindow(host);
        }
    }

    /// A painted text leaf drew its glyphs from its frame's top-left
    /// corner, ignoring its own padding: Taffy sized the frame to content +
    /// padding, but a leaf has no child to offset, so the docs sidebar's
    /// section labels (`padding-top`) sat flush on the item above with the
    /// whole gap below them. The glyphs must start inside the padding box.
    #[test]
    fn regression_text_leaf_paints_inside_its_padding() {
        const TOP: f32 = 24.0;
        const LEFT: f32 = 30.0;
        let host = host_window();
        let mut b = WindowsBackend::new(host);
        let a11y = AccessibilityProps::default();
        let mut root = b.create_view(&a11y);
        let label = b.create_text("Hg", &a11y);
        let mut st = StyleRules::default();
        st.padding_top = Some(Length::Px(TOP).into());
        st.padding_left = Some(Length::Px(LEFT).into());
        st.align_self = Some(runtime_shared::AlignSelf::FlexStart);
        b.apply_style(&label, &Rc::new(st));
        b.insert(&mut root, label);
        b.set_dpi(96);
        b.finish(root);

        let px = render(&mut b);
        let inked = |x: i32, y: i32| {
            let p = at(&px, x, y);
            // Any visibly non-white pixel: the default text is near-black.
            ((p >> 16) & 0xFF) < 0xC0 || ((p >> 8) & 0xFF) < 0xC0 || (p & 0xFF) < 0xC0
        };
        let ink: Vec<(i32, i32)> = (0..120)
            .flat_map(|y| (0..200).map(move |x| (x, y)))
            .filter(|&(x, y)| inked(x, y))
            .collect();
        assert!(!ink.is_empty(), "the label must paint");
        let first_row = ink.iter().map(|p| p.1).min().unwrap();
        let first_col = ink.iter().map(|p| p.0).min().unwrap();
        assert!(first_row >= TOP as i32, "glyphs start at y={first_row}, inside the {TOP}px top padding");
        assert!(first_col >= LEFT as i32, "glyphs start at x={first_col}, inside the {LEFT}px left padding");
        unsafe {
            let _ = DestroyWindow(host);
        }
    }

    /// 150% (144 DPI) — a fractional scale: native controls tile without
    /// seams (edges snapped, not origin + size rounded independently).
    #[test]
    fn fractional_dpi_snaps_control_edges() {
        let host = host_window();
        let mut b = WindowsBackend::new(host);
        let a11y = AccessibilityProps::default();
        let mut root = b.create_view(&a11y);
        let mut hwnds = Vec::new();
        for _ in 0..3 {
            let btn = b.create_button("x", &runtime_shared::derive::IntoAction::into_action(|| {}), None, None, &a11y);
            b.apply_style(&btn, &sized(33.0, 21.0, None));
            hwnds.push(btn.hwnd);
            b.insert(&mut root, btn);
        }
        b.set_dpi(144);
        b.finish(root);
        let rects: Vec<_> = hwnds.iter().map(|h| control_client_rect(host, *h)).collect();
        for pair in rects.windows(2) {
            assert_eq!(pair[0].1 + pair[0].3, pair[1].1, "no seam/overlap: {rects:?}");
        }
        // 3 × 21 logical = 63 logical = 94.5 → 95 device px in total.
        assert_eq!(rects[2].1 + rects[2].3, 95);
        unsafe {
            let _ = DestroyWindow(host);
        }
    }

    /// An SVG `image()` was rasterized at scale 1 ("the backend doesn't
    /// scale for DPI") and stretched by the paint: blurry on any HiDPI
    /// monitor. Drawn at its 32×16 intrinsic size on a 2× monitor it must
    /// be rasterized at 64×32 device px and paint exactly that area.
    #[test]
    fn regression_svg_rasterizes_at_monitor_scale() {
        let host = host_window();
        let mut b = WindowsBackend::new(host);
        let a11y = AccessibilityProps::default();
        let mut root = b.create_view(&a11y);
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="16"><rect width="32" height="16" fill="red"/></svg>"#;
        let src = format!("data:image/svg+xml,{}", percent_encode(svg));
        let img = b.create_image(&src, None, &a11y);
        let img_id = img.id;
        let mut s = StyleRules::default();
        s.position = Some(Position::Absolute);
        s.left = Some(Length::Px(0.0).into());
        s.top = Some(Length::Px(0.0).into());
        b.apply_style(&img, &Rc::new(s));
        b.insert(&mut root, img);
        b.set_dpi(192);
        b.finish(root);
        assert_eq!(b.node_frame(img_id), Some((0.0, 0.0, 32.0, 16.0)), "laid out at its logical size");

        let px = render(&mut b);
        let raster = match &b.nodes[&img_id].kind {
            NodeKind::Image(p) => match &p.source {
                Some(image::ImageSource::Svg { raster, .. }) => {
                    raster.borrow().as_ref().map(|(scale, d)| (*scale, d.natural_px()))
                }
                _ => None,
            },
            _ => None,
        };
        assert_eq!(raster, Some((2.0, (64, 32))), "rasterized at the monitor's 2× density");
        assert_eq!(at(&px, 2, 2), RED);
        assert_eq!(at(&px, 62, 30), RED, "covers 64×32 device px");
        assert_ne!(at(&px, 66, 10), RED);

        // Moving to a 1.5× monitor re-rasterizes at the new density.
        b.set_dpi(144);
        let _ = render(&mut b);
        let scale = match &b.nodes[&img_id].kind {
            NodeKind::Image(p) => match &p.source {
                Some(image::ImageSource::Svg { raster, .. }) => raster.borrow().as_ref().map(|r| r.0),
                _ => None,
            },
            _ => None,
        };
        assert_eq!(scale, Some(1.5));
        unsafe {
            let _ = DestroyWindow(host);
        }
    }

    /// The backend adopts the DPI Windows reports for its window, with no
    /// `set_dpi` call. Run in a Wine prefix with `LogPixels` = 192 (or on a
    /// 200% monitor) this sees 2.0 and the 100×50 view paints 200×100
    /// device px; at 96 it sees 1.0 and 100×50 — the assertions are
    /// relative to the reported DPI, so they hold on any monitor.
    #[test]
    fn backend_adopts_the_window_dpi() {
        enable_per_monitor_dpi_awareness();
        let host = host_window();
        let mut b = WindowsBackend::new(host);
        let reported = unsafe { GetDpiForWindow(host) };
        eprintln!("[dpi_tests] GetDpiForWindow = {reported}");
        assert_eq!(b.dpi, dpi::DpiScale::from_dpi(reported));
        let s = b.scale();
        let (w, h) = b.logical_client_size();
        assert_eq!((w * s).round() as i32, CLIENT.0);
        assert_eq!((h * s).round() as i32, CLIENT.1);

        let a11y = AccessibilityProps::default();
        let mut root = b.create_view(&a11y);
        let red = b.create_view(&a11y);
        b.apply_style(&red, &sized(100.0, 50.0, Some("#ff0000")));
        b.insert(&mut root, red);
        b.finish(root);
        let (dw, dh) = ((100.0 * s).round() as i32, (50.0 * s).round() as i32);
        let px = render(&mut b);
        assert_eq!(at(&px, dw - 2, dh - 2), RED, "{dw}×{dh} device px at {reported} DPI");
        assert_ne!(at(&px, dw + 3, dh / 2), RED);
        assert_ne!(at(&px, dw / 2, dh + 3), RED);
        unsafe {
            let _ = DestroyWindow(host);
        }
    }

    fn percent_encode(s: &str) -> String {
        s.bytes()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    (c as char).to_string()
                } else {
                    format!("%{c:02X}")
                }
            })
            .collect()
    }
}
