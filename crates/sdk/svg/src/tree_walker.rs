//! Shared usvg-tree-to-native-vector walker.
//!
//! Parses SVG markup with `usvg` and replays the resulting tree
//! against an [`SvgPainter`] implementation that knows how to talk to
//! a single backend's vector primitives.
//!
//! # Coverage
//!
//! - Paths with solid / linear / radial gradient fills, solid strokes
//! - Group opacity (offscreen-composited via `with_opacity`)
//! - ClipPaths (accumulated into one native path, then `with_clip`)
//! - Masks (luminance + alpha, offscreen-rendered via `with_mask`)
//! - Embedded raster images (PNG / JPEG / GIF — `draw_image`)
//! - Embedded nested SVG (recursive walk via the image's own Tree)
//! - Text (when usvg's `text` feature is on, we walk
//!   `Text::flattened()` like any other group)
//!
//! # Still not supported
//!
//! - SVG **filters** (`feGaussianBlur`, etc.) — these need an
//!   offscreen rasterize-and-process pipeline that's per-platform
//!   (CIFilter on iOS, RenderEffect on Android API 31+, with no
//!   pre-31 fallback that isn't deprecated). Groups carrying
//!   `filter` attributes still render their children, just without
//!   the filter effect applied. The trait surface deliberately
//!   leaves this slot open — adding `apply_filter` later means one
//!   new trait method, not a tree-walker rewrite.
//!
//! # What's pre-resolved by usvg
//!
//! - Each `Path` carries `abs_transform()`, the full ancestor-chain
//!   composed transform. The walker applies that *per path* —
//!   group transforms are NOT pushed/popped during walk.
//! - Gradient coordinates are pre-resolved into user-space.
//! - Mask + clipPath transforms are baked into the child paths'
//!   `abs_transform()` too.

use usvg::tiny_skia_path::{PathSegment, Point};
use usvg::{Color, FillRule, LineCap, LineJoin, Node, Paint, Transform};

/// Per-backend painter that the walker drives. Implementations bridge
/// trait calls to UIBezierPath/CGContext (iOS) or
/// `android.graphics.Path`/Canvas (Android). The trait surface is
/// designed so adding a new SVG feature == adding one method here
/// plus one impl per backend — no walker rewrite.
pub trait SvgPainter {
    /// Native path type. iOS = `UIBezierPath`; Android = a `GlobalRef`
    /// to `android.graphics.Path`.
    type Path;

    /// Build a fresh native path object from `segments`. The walker
    /// calls this once per `usvg::Path`. An empty iterator must
    /// produce an empty-but-valid path (used as the seed for clip
    /// accumulation).
    fn build_path<I: Iterator<Item = PathSegment>>(&mut self, segments: I) -> Self::Path;

    /// Append `segments` (each point transformed by `t`) to `dest`.
    /// Used by the walker to merge multiple child paths of a
    /// `clipPath` into a single native path before pushing it as
    /// the clip region.
    fn extend_path<I: Iterator<Item = PathSegment>>(
        &mut self,
        dest: &mut Self::Path,
        segments: I,
        t: Transform,
    );

    fn fill_solid(&mut self, path: &Self::Path, color: Color, opacity: f32, rule: FillRule);
    fn fill_linear_gradient(
        &mut self,
        path: &Self::Path,
        gradient: &usvg::LinearGradient,
        opacity: f32,
        rule: FillRule,
    );
    fn fill_radial_gradient(
        &mut self,
        path: &Self::Path,
        gradient: &usvg::RadialGradient,
        opacity: f32,
        rule: FillRule,
    );
    fn stroke_solid(
        &mut self,
        path: &Self::Path,
        color: Color,
        opacity: f32,
        params: StrokeParams,
    );

    /// Decode + draw an embedded raster image. The destination rect
    /// is in canvas (post-transform) coordinates — usvg's
    /// `Image::abs_bounding_box()`. The backend handles aspect
    /// preservation (the image is already laid out in `dst_rect` by
    /// usvg's parser per the `<image>` element's preserveAspectRatio).
    fn draw_image(&mut self, kind: &usvg::ImageKind, dst_rect: Rect);

    fn with_transform<R>(&mut self, transform: Transform, f: impl FnOnce(&mut Self) -> R) -> R;

    /// Clip subsequent drawing to `clip` (with the given fill rule)
    /// for the duration of `f`. On `f` return the clip is removed.
    /// `clip` was built by the walker via `build_path` + repeated
    /// `extend_path` over a `usvg::ClipPath`'s contents.
    fn with_clip<R>(
        &mut self,
        clip: &Self::Path,
        rule: FillRule,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R;

    /// Wrap `f` in an offscreen layer composited at `alpha` at end.
    /// Required for group opacity to render correctly — applying
    /// opacity per-child instead of via a layer would double up
    /// alpha on overlapping children.
    fn with_opacity<R>(&mut self, alpha: f32, f: impl FnOnce(&mut Self) -> R) -> R;

    /// SVG `mask` element. The mask is rendered first (via
    /// `render_mask` — a closure the painter invokes inside its
    /// offscreen pass), then `f` runs against the masked
    /// destination. `kind` selects whether luminance or alpha
    /// channel of the mask is the source. `dst_rect` is the mask's
    /// user-space extent (from `usvg::Mask::rect()`); the painter
    /// uses it to size the offscreen buffer correctly.
    ///
    /// Two-callback shape is intentional: the painter controls the
    /// offscreen render order (push offscreen → render_mask → capture
    /// → push masked-content → f → composite), and the walker doesn't
    /// need to know any of that machinery — it just provides what to
    /// paint for each role.
    fn with_mask<R>(
        &mut self,
        kind: MaskKind,
        dst_rect: Rect,
        render_mask: impl FnOnce(&mut Self),
        f: impl FnOnce(&mut Self) -> R,
    ) -> R;
}

/// Stroke parameters extracted from a `usvg::Stroke` so the painter
/// trait can stay agnostic of the usvg type.
#[derive(Clone)]
pub struct StrokeParams<'a> {
    pub width: f32,
    pub linecap: LineCap,
    pub linejoin: LineJoin,
    pub miter_limit: f32,
    pub dasharray: Option<&'a [f32]>,
    pub dashoffset: f32,
}

/// Axis-aligned rectangle in canvas (user-space) coordinates. Used
/// for image destination rects.
#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Mask source channel — selects how the offscreen-rendered mask
/// content becomes an alpha value for the masked content.
#[derive(Clone, Copy, Debug)]
pub enum MaskKind {
    /// Use the perceptual luminance of the mask's RGB output (default
    /// per SVG spec). A white mask region = fully visible content,
    /// black region = fully hidden.
    Luminance,
    /// Use the alpha channel of the mask's output directly. Opaque
    /// mask regions = visible content.
    Alpha,
}

impl From<usvg::MaskType> for MaskKind {
    fn from(t: usvg::MaskType) -> Self {
        match t {
            usvg::MaskType::Luminance => MaskKind::Luminance,
            usvg::MaskType::Alpha => MaskKind::Alpha,
        }
    }
}

/// Walk a parsed SVG tree, painting it into `painter`.
pub fn render_tree<P: SvgPainter>(painter: &mut P, tree: &usvg::Tree) {
    walk_group(painter, tree.root());
}

fn walk_group<P: SvgPainter>(painter: &mut P, group: &usvg::Group) {
    let opacity = group.opacity().get();
    let clip = group.clip_path();
    let mask = group.mask();

    // Composition order (per the SVG spec): the masked-and-clipped
    // children render at the group's alpha. So opacity wraps clip
    // wraps mask wraps the child walk.
    let paint_children = |painter: &mut P| walk_group_children(painter, group);
    let paint_with_mask = |painter: &mut P| {
        if let Some(m) = mask {
            apply_mask(painter, m, paint_children);
        } else {
            paint_children(painter);
        }
    };
    let paint_with_clip = |painter: &mut P| {
        if let Some(c) = clip {
            apply_clip(painter, c, paint_with_mask);
        } else {
            paint_with_mask(painter);
        }
    };

    if opacity < 1.0 {
        painter.with_opacity(opacity, paint_with_clip);
    } else {
        paint_with_clip(painter);
    }

    // `group.filters()` are NOT applied here — see module-level
    // doc. The children render as if the filter were the identity.
}

fn walk_group_children<P: SvgPainter>(painter: &mut P, group: &usvg::Group) {
    for node in group.children() {
        walk_node(painter, node);
    }
}

fn walk_node<P: SvgPainter>(painter: &mut P, node: &Node) {
    match node {
        Node::Group(g) => walk_group(painter, g),
        Node::Path(p) => paint_path(painter, p),
        Node::Image(img) => paint_image(painter, img),
        // `Text::flattened()` is a `Group` of paths that usvg
        // pre-computed during parsing (when the `text` feature is on).
        // Walking it is identical to walking any other group — no
        // text-specific code lives in the painter.
        Node::Text(t) => walk_group(painter, t.flattened()),
    }
}

fn paint_path<P: SvgPainter>(painter: &mut P, path: &usvg::Path) {
    if !path.is_visible() {
        return;
    }
    let segments = path.data().segments();
    let abs_transform = path.abs_transform();

    painter.with_transform(abs_transform, |painter| {
        let native = painter.build_path(segments);
        match path.paint_order() {
            usvg::PaintOrder::FillAndStroke => {
                if let Some(fill) = path.fill() {
                    apply_fill(painter, &native, fill);
                }
                if let Some(stroke) = path.stroke() {
                    apply_stroke(painter, &native, stroke);
                }
            }
            usvg::PaintOrder::StrokeAndFill => {
                if let Some(stroke) = path.stroke() {
                    apply_stroke(painter, &native, stroke);
                }
                if let Some(fill) = path.fill() {
                    apply_fill(painter, &native, fill);
                }
            }
        }
    });
}

fn paint_image<P: SvgPainter>(painter: &mut P, img: &usvg::Image) {
    if !img.is_visible() {
        return;
    }
    match img.kind() {
        // Nested SVG: render via the same walker. usvg's parser
        // already placed the inner tree at the right transform via
        // the wrapping group, so we just recurse.
        usvg::ImageKind::SVG(inner) => {
            render_tree(painter, inner);
        }
        // Raster image: hand the bytes to the backend's image
        // decoder. `abs_bounding_box` is in canvas coords — the
        // backend draws at that rect directly, no with_transform
        // wrap needed.
        _ => {
            let bb = img.abs_bounding_box();
            let rect = Rect {
                x: bb.x(),
                y: bb.y(),
                width: bb.width(),
                height: bb.height(),
            };
            painter.draw_image(img.kind(), rect);
        }
    }
}

fn apply_fill<P: SvgPainter>(painter: &mut P, native: &P::Path, fill: &usvg::Fill) {
    let opacity = fill.opacity().get();
    let rule = fill.rule();
    match fill.paint() {
        Paint::Color(c) => painter.fill_solid(native, *c, opacity, rule),
        Paint::LinearGradient(g) => painter.fill_linear_gradient(native, g, opacity, rule),
        Paint::RadialGradient(g) => painter.fill_radial_gradient(native, g, opacity, rule),
        Paint::Pattern(_) => painter.fill_solid(native, Color::black(), opacity, rule),
    }
}

fn apply_stroke<P: SvgPainter>(painter: &mut P, native: &P::Path, stroke: &usvg::Stroke) {
    let opacity = stroke.opacity().get();
    let params = StrokeParams {
        width: stroke.width().get(),
        linecap: stroke.linecap(),
        linejoin: stroke.linejoin(),
        miter_limit: stroke.miterlimit().get(),
        dasharray: stroke.dasharray(),
        dashoffset: stroke.dashoffset(),
    };
    let color = match stroke.paint() {
        Paint::Color(c) => *c,
        Paint::LinearGradient(g) => first_stop_color(g.stops()).unwrap_or(Color::black()),
        Paint::RadialGradient(g) => first_stop_color(g.stops()).unwrap_or(Color::black()),
        Paint::Pattern(_) => Color::black(),
    };
    painter.stroke_solid(native, color, opacity, params);
}

fn first_stop_color(stops: &[usvg::Stop]) -> Option<Color> {
    stops.first().map(|s| s.color())
}

// ----------------------------------------------------------------------------
// ClipPath + Mask machinery
// ----------------------------------------------------------------------------

/// Build a native path from a `usvg::ClipPath` (the union of its
/// child paths, each at its own `abs_transform`) and run `f` inside
/// the clip region.
fn apply_clip<P: SvgPainter>(
    painter: &mut P,
    clip: &usvg::ClipPath,
    f: impl FnOnce(&mut P),
) {
    let clip_path = build_clip_geometry(painter, clip.root());
    // SVG spec: clipPath uses non-zero fill rule by default. The
    // individual children's `fill-rule` is ignored here — the clip
    // is a single union. Honoring per-child fill rules would need
    // multiple successive intersected clips, which is rare enough
    // in practice that the simpler union behavior matches resvg's
    // output on every test SVG.
    let rule = FillRule::NonZero;

    // ClipPaths can themselves carry a `clip_path` attribute (nested
    // clipping). Apply the outer clip first, then the inner. The
    // intersection is what the masked content sees.
    if let Some(inner_clip) = clip.clip_path() {
        painter.with_clip(&clip_path, rule, |painter| {
            apply_clip(painter, inner_clip, f);
        });
    } else {
        painter.with_clip(&clip_path, rule, f);
    }
}

/// Accumulate all paths inside a clipPath's content group (recursing
/// through nested groups) into a single native path.
fn build_clip_geometry<P: SvgPainter>(painter: &mut P, group: &usvg::Group) -> P::Path {
    let mut acc = painter.build_path(std::iter::empty());
    extend_clip_geometry(painter, &mut acc, group);
    acc
}

fn extend_clip_geometry<P: SvgPainter>(
    painter: &mut P,
    acc: &mut P::Path,
    group: &usvg::Group,
) {
    for node in group.children() {
        match node {
            Node::Path(p) => {
                painter.extend_path(acc, p.data().segments(), p.abs_transform());
            }
            Node::Group(g) => extend_clip_geometry(painter, acc, g),
            // `<text>` inside `<clipPath>` is technically supported
            // by SVG (text outlines as clip region). For v2 we can
            // walk text.flattened() the same way; for now skip
            // (text-as-clip is exceedingly rare).
            Node::Text(_) => {}
            // `<image>` inside `<clipPath>` is spec-invalid — skip.
            Node::Image(_) => {}
        }
    }
}

fn apply_mask<P: SvgPainter>(
    painter: &mut P,
    mask: &usvg::Mask,
    f: impl FnOnce(&mut P),
) {
    let kind = MaskKind::from(mask.kind());
    let r = mask.rect();
    let rect = Rect {
        x: r.x(),
        y: r.y(),
        width: r.width(),
        height: r.height(),
    };
    // Masks can chain to another mask (`<mask mask="...">`). usvg
    // 0.42 exposes the chained mask but not a `clip-path` attribute
    // on Mask (which is rare in real SVGs — `clip-path` on a mask
    // is uncommon; chained masks cover the same need).
    let mask_root = mask.root();
    let inner_mask = mask.mask();

    let render_mask_content = move |painter: &mut P| {
        if let Some(inner) = inner_mask {
            apply_mask(painter, inner, |painter| {
                walk_group_children(painter, mask_root);
            });
        } else {
            walk_group_children(painter, mask_root);
        }
    };

    painter.with_mask(kind, rect, render_mask_content, f);
}

// ----------------------------------------------------------------------------
// Helpers shared with backends
// ----------------------------------------------------------------------------

/// Apply a `tiny_skia_path::Transform` to a point. Mirrors
/// `Transform::map_point` on newer tiny-skia, which doesn't ship in
/// the 0.11 path crate we depend on.
pub fn map_point(t: Transform, p: Point) -> Point {
    Point {
        x: t.sx * p.x + t.kx * p.y + t.tx,
        y: t.ky * p.x + t.sy * p.y + t.ty,
    }
}
