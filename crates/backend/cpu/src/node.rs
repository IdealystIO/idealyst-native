//! Node representation for the CPU backend.
//!
//! Mirrors the terminal backend's split: the framework holds a small
//! `CpuNode { id }` handle, the backend owns a `HashMap<u32, NodeData>`
//! keyed by id. The `HashMap` indirection trades a per-access lookup
//! for `Copy` node handles, which is what the framework's `Backend`
//! contract requires.
//!
//! Anything that wants to render fast on ESP32 (where the system
//! allocator's per-allocation overhead is heavier than on a desktop
//! libc) can swap `HashMap` for `slotmap` or a dense `Vec<Option<_>>`
//! without churning the rest of the backend. The kind+style+children
//! shape stays the same.

use std::rc::Rc;

use runtime_core::color::Rgba;
use runtime_core::{GradientKind, Length, StyleRules};
use runtime_layout::LayoutNode;

/// Opaque handle the framework holds as the backend's `Self::Node`.
/// Just an id — the heavy data lives in
/// [`crate::CpuBackend::nodes`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CpuNode {
    pub id: u32,
}

/// What kind of primitive a node represents. The render walker
/// branches on this to pick a paint strategy.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NodeKind {
    /// Generic container — paints background + border, descends into
    /// children. No content.
    View,
    /// Text leaf. The `NodeData.content` field holds the string;
    /// renderer rasterizes it with the built-in bitmap font.
    Text,
    /// Tappable button — same paint as `View` plus the label text
    /// drawn centered. `NodeData.on_click` fires on hit.
    Button,
    /// Clickable container. Same paint as `View`; click hit-tests
    /// fire `on_click`.
    Pressable,
    /// Scrolling container. Children layout at their natural sizes;
    /// renderer paints them with `(scroll_x, scroll_y)` offset and
    /// clips children to the view's frame.
    ScrollView,
}

/// Per-node mutable state owned by [`crate::CpuBackend::nodes`].
pub(crate) struct NodeData {
    pub kind: NodeKind,
    /// Text content (used by `Text` and `Button` only).
    pub content: String,
    /// Optional press handler. Set by `create_button` (from
    /// `Action.fire`) and `create_pressable`.
    pub on_click: Option<Rc<dyn Fn()>>,
    /// Last applied resolved style. Kept around for the renderer
    /// because not everything cachable into raw `Rgba` lives in the
    /// flat fields below (border widths, corner radii, etc.).
    pub style: Option<Rc<StyleRules>>,
    /// Companion handle in the parallel Taffy tree.
    pub layout: LayoutNode,
    /// Child node ids, in insertion order. Mirrors the Taffy child
    /// list — we keep this here so the paint walker doesn't have to
    /// round-trip through Taffy for every recursion.
    pub children: Vec<u32>,
    /// Cached background color (resolved from `style.background`).
    /// `None` means transparent — the parent's paint shows through.
    pub bg: Option<Rgba>,
    /// Cached foreground / text color. `None` falls back to opaque
    /// black in the text rasterizer.
    pub fg: Option<Rgba>,
    /// Per-frame background override from
    /// `set_animated_color(BackgroundColor, …)`. When `Some`, wins
    /// over `bg` in the paint pass. Mirrors the split the terminal
    /// backend uses — keeps a re-applied `apply_style` from
    /// clobbering an in-flight animation on hot-patch.
    pub animated_bg: Option<Rgba>,
    /// Per-frame foreground override.
    pub animated_fg: Option<Rgba>,
    /// Static opacity from the stylesheet, clamped to `[0, 1]`.
    /// Seeded by `apply_style`; default 1.0 (fully opaque). Composed
    /// multiplicatively with ancestors' opacities during paint.
    pub opacity: f32,
    /// Per-frame opacity override from
    /// `set_animated_f32(Opacity, …)`. When `Some`, wins over the
    /// static `opacity` field. Same separation rationale as
    /// `animated_bg` / `animated_fg`.
    pub animated_opacity: Option<f32>,
    /// Animation-driven translate added on top of the laid-out
    /// frame (and on top of `static_translate_*`). Driven by
    /// `set_animated_f32(TranslateX/Y, …)`.
    pub animated_translate_x: f32,
    pub animated_translate_y: f32,
    /// Sibling-relative z-order. Higher values paint later (in
    /// front). Driven by `set_animated_f32(ZIndex, …)`. Default 0.
    /// Within a parent, paint order is `sort_by(z_index)` so a
    /// single view can change its stacking position per frame
    /// without re-mounting the tree.
    pub z_index: f32,
    /// `scroll_x` for `ScrollView` nodes — current horizontal scroll
    /// offset in pixels, subtracted from child paint coords. Always
    /// 0 for non-scroll kinds.
    pub scroll_x: f32,
    /// `scroll_y` companion.
    pub scroll_y: f32,
    /// Cached border widths in px, per side `[top, right, bottom, left]`.
    /// Read from `style.border_*_width`; zeros mean "no border".
    pub border_widths: [f32; 4],
    /// Border colors per side `[top, right, bottom, left]`. `None`
    /// means "inherit text color" or "no border" depending on width.
    pub border_colors: [Option<Rgba>; 4],
    /// Cached corner radii `[tl, tr, br, bl]` in px. Zeros = sharp
    /// corners. Used by the rounded-rect rasterizer.
    pub corner_radii: [f32; 4],
    /// Cached `font_size` resolved to px. `None` = renderer default
    /// (one bitmap-font glyph cell = 8 px).
    pub font_size_px: Option<f32>,
    /// Cached static transforms. We support TranslateX/Y only in
    /// this MVP — Scale / Rotate would need an inverse-mapping
    /// sampler that's nontrivial to do efficiently on a software
    /// rasterizer. Logged at apply_style time when a non-translate
    /// transform is dropped.
    pub static_translate_x: Option<Length>,
    pub static_translate_y: Option<Length>,
    /// Cached gradient, populated when the stylesheet sets
    /// `background_gradient`. When present, takes precedence over
    /// the solid `bg` color in the paint pass (matches every other
    /// backend in the repo).
    pub gradient: Option<ResolvedGradient>,
}

/// Backend-side gradient with stops pre-parsed to `Rgba` so the
/// per-pixel sampler doesn't reparse strings on every paint.
#[derive(Clone)]
pub(crate) struct ResolvedGradient {
    pub kind: GradientKind,
    pub stops: Vec<(f32, Rgba)>,
    /// Per-stop animated color overrides written by
    /// `set_animated_color(GradientStopColor(idx), …)`. `None`
    /// entries fall back to the stop's static color. Length matches
    /// `stops`; updates preserve unset entries across an
    /// `apply_style` re-fire (the dev-server hot-patch path
    /// re-applies the stylesheet on every change).
    pub animated_stops: Vec<Option<Rgba>>,
}

impl NodeData {
    /// Fresh node with sensible zero defaults. Callers
    /// (`CpuBackend::alloc_node`) supply the `kind` + initial
    /// `content`.
    pub fn new(kind: NodeKind, content: String, layout: LayoutNode) -> Self {
        Self {
            kind,
            content,
            on_click: None,
            style: None,
            layout,
            children: Vec::new(),
            bg: None,
            fg: None,
            animated_bg: None,
            animated_fg: None,
            opacity: 1.0,
            animated_opacity: None,
            animated_translate_x: 0.0,
            animated_translate_y: 0.0,
            z_index: 0.0,
            scroll_x: 0.0,
            scroll_y: 0.0,
            border_widths: [0.0; 4],
            border_colors: [None; 4],
            corner_radii: [0.0; 4],
            font_size_px: None,
            static_translate_x: None,
            static_translate_y: None,
            gradient: None,
        }
    }
}
