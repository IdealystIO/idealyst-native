//! The single-surface scene painter.
//!
//! ## Why one surface instead of HWND-per-view
//!
//! Win32 sibling child windows are opaque rectangles — they cannot
//! alpha-composite, so any later-z sibling blanks whatever sits under it
//! regardless of what its `WM_PAINT` draws. A scene built from
//! overlapping translucent layers (the welcome app: vignette bands, a
//! radial sun-glare, an absolutely-positioned content layer) is
//! *unrepresentable* in that model. Every other backend composites a
//! retained tree on one surface (CALayer on Apple, GSK nodes on GTK, the
//! DOM compositor on web); this module is the Win32 equivalent: the host
//! window is ONE canvas, and the view/text/icon/image tree is painted
//! with GDI+ into a double-buffered memory DC in tree order. Only
//! genuinely-native interactive controls (button / edit / trackbar /
//! checkbox / progress) remain child HWNDs, positioned on top by the
//! layout pass — the host's `WS_CLIPCHILDREN` keeps the blit from
//! painting over them.
//!
//! ## Paint algorithm
//!
//! [`paint_scene`] clears the back buffer to the app background, then
//! recurses from the root: each node saves the graphics state, prepends
//! its frame translation + transforms (animated translate, then
//! author-style + animated scale/rotate about the node's center — the
//! CSS `transform-origin: center` composition), multiplies its opacity
//! into the cumulative alpha, draws its own visual, optionally clips
//! children (`overflow: hidden`, honoring corner radii), recurses into
//! children stably sorted by z-index, and restores. Group opacity is
//! approximated by multiplying the cumulative alpha into each drawn
//! color (exact for non-overlapping content; the standard immediate-mode
//! trade-off vs. offscreen subtree compositing).
//!
//! ## Gradients
//!
//! A linear gradient under an axis-aligned world transform (every node
//! that isn't rotated, skewed or mirrored) is computed on the CPU straight
//! onto the device-pixel grid and copied 1:1 (`gradient_raster`) — GDI+'s
//! per-pixel brush shading made the welcome vignettes ~85 ms of a ~90 ms
//! frame under Wine, the copy costs ~3 ms. The output reproduces the line
//! brush (same preset-blend ramp, same pixel grid, anti-aliased rounded
//! shape); rotated nodes and radial gradients still use GDI+ brushes.
//!
//! ## Hit testing
//!
//! With no per-view HWNDs, clicks land on the host window. [`hit_chain`]
//! walks the tree top-most-first (reverse paint order) applying the same
//! frame + translate offsets, finds the deepest node containing the point
//! (the hit target) and returns it with its ancestors, deepest first —
//! the responder chain every input channel bubbles along: pressables
//! ([`pressable_at`]), touch, wheel and hover (`crate::view_input`).
//! `pointer_events: none` makes a node transparent as a TARGET (the hit
//! falls to whatever is painted behind it) without removing it from a
//! descendant's bubble path; the nearest explicit value up the tree wins,
//! so a descendant's `auto` re-enables it — the same verdict as CSS and
//! `backend_apple_core::pointer_events_policy`. Scale/rotate are ignored
//! in hit tests — documented approximation; nothing interactive animates
//! transforms today.

use std::rc::Rc;

use runtime_shared::{Length, PointerEvents, TextAlign, Transform};

use windows::core::PCWSTR;
use windows::Win32::Foundation::COLORREF;
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, SelectObject,
    HBITMAP, HDC, HGDIOBJ, SRCCOPY,
};
use windows::Win32::Graphics::GdiPlus::{
    CombineModeIntersect, FillModeWinding, GdipAddPathArc, GdipAddPathLine, GdipAddPathRectangle,
    GdipClosePathFigure, GdipCreateFromHDC, GdipCreatePath, GdipCreatePen1, GdipCreateSolidFill,
    GdipDeleteBrush, GdipDeleteGraphics, GdipDeletePath, GdipDeletePen, GdipDrawPath,
    GdipDrawString, GdipFillPath, GdipGraphicsClear, GdipRestoreGraphics, GdipRotateWorldTransform,
    GdipSaveGraphics, GdipScaleWorldTransform, GdipSetClipPath, GdipSetSmoothingMode,
    GdipSetTextRenderingHint, GdipStringFormatGetGenericTypographic, GdipTranslateWorldTransform,
    GpBrush, GpGraphics, GpPath, GpPen, GpSolidFill, GpStringFormat, MatrixOrderPrepend, RectF,
    SmoothingModeAntiAlias, TextRenderingHintAntiAlias, UnitPixel, GdipCreateBitmapFromScan0,
    GdipDisposeImage, GdipGetImageGraphicsContext, GpBitmap, GpImage,
};

use crate::font;
use crate::layers::{self, LayerPlan};
use crate::border_pattern::{self, BorderMark};
use crate::{
    icon, image, uniform_border, AnimTransform, BorderSide, GradKind, GradientPaint, NodeKind, NodeMeta,
    ViewVisual, WindowsBackend,
};

// =========================================================================
// Color helpers
// =========================================================================

/// sRGB `[r, g, b, a]` (all 0..=1) → GDI+ `0xAARRGGBB`, with `alpha`
/// multiplied into the color's own alpha.
#[inline]
pub(crate) fn argb_from_srgba(c: [f32; 4], alpha: f32) -> u32 {
    let ch = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    (ch(c[3] * alpha) << 24) | (ch(c[0]) << 16) | (ch(c[1]) << 8) | ch(c[2])
}

/// Multiply `alpha` into an already-packed `0xAARRGGBB`.
#[inline]
pub(crate) fn argb_with_alpha(argb: u32, alpha: f32) -> u32 {
    let a = ((argb >> 24) as f32 * alpha.clamp(0.0, 1.0)).round() as u32;
    (a << 24) | (argb & 0x00FF_FFFF)
}

// =========================================================================
// Transform composition (pure — unit-tested)
// =========================================================================

pub(crate) use crate::painted_geom::XOp;

/// Resolve a node's transform chain against its `w × h` box:
/// animated translate first (origin-independent), then the
/// origin-center wrap around author-style ops + animated scale/rotate.
/// Applying these sequentially with `MatrixOrderPrepend` yields
/// `T(anim) · T(c) · authors · S(anim) · R(anim) · T(-c)` — the CSS
/// `transform-origin: center` composition, matching the Linux backend's
/// `rebuild_transform` (position ∘ author ∘ animated).
pub(crate) fn transform_ops(
    w: f32,
    h: f32,
    author: &[Transform],
    anim: &AnimTransform,
) -> Vec<XOp> {
    let mut ops = Vec::new();
    if anim.tx != 0.0 || anim.ty != 0.0 {
        ops.push(XOp::Translate(anim.tx, anim.ty));
    }
    let sx = anim.scale * anim.scale_x;
    let sy = anim.scale * anim.scale_y;
    let needs_wrap =
        !author.is_empty() || sx != 1.0 || sy != 1.0 || anim.rotate_deg != 0.0;
    if !needs_wrap {
        return ops;
    }
    let (cx, cy) = (w / 2.0, h / 2.0);
    ops.push(XOp::Translate(cx, cy));
    for t in author {
        match t {
            Transform::TranslateX(l) => ops.push(XOp::Translate(resolve_len(l, w), 0.0)),
            Transform::TranslateY(l) => ops.push(XOp::Translate(0.0, resolve_len(l, h))),
            Transform::Scale(s) => ops.push(XOp::Scale(*s, *s)),
            Transform::ScaleXY { x, y } => ops.push(XOp::Scale(*x, *y)),
            Transform::Rotate(deg) => ops.push(XOp::Rotate(*deg)),
            // Skew isn't expressible as translate/scale/rotate; no
            // current author code uses it. Documented no-op.
            Transform::SkewX(_) | Transform::SkewY(_) => {}
        }
    }
    if sx != 1.0 || sy != 1.0 {
        ops.push(XOp::Scale(sx, sy));
    }
    if anim.rotate_deg != 0.0 {
        ops.push(XOp::Rotate(anim.rotate_deg));
    }
    ops.push(XOp::Translate(-cx, -cy));
    ops
}

/// Percent lengths resolve against the node's own box (CSS `translate`
/// semantics — the sun-glare wrapper's `translate(50%, -50%)` corner
/// anchor depends on this).
fn resolve_len(l: &Length, basis: f32) -> f32 {
    match l {
        Length::Px(v) => *v,
        Length::Percent(p) => basis * p / 100.0,
        _ => 0.0,
    }
}

/// Stable z-sort of sibling ids: ascending z, insertion order on ties —
/// the same tie-break every other backend uses (document order), which
/// the welcome planets' binary z-flip depends on.
pub(crate) fn z_sorted(pairs: &[(u64, f32)]) -> Vec<u64> {
    let mut v: Vec<(u64, f32)> = pairs.to_vec();
    v.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    v.into_iter().map(|(id, _)| id).collect()
}

// =========================================================================
// Rounded-rect path
// =========================================================================

/// Append a rounded-rect figure (per-corner radii `[tl, tr, br, bl]`)
/// to `path`, inset by `inset` on every side — half the border width,
/// so a centered stroke stays inside the node rect. Square corners
/// (all radii ~0) use a plain rectangle; otherwise each corner is a
/// 90° arc and GDI+ connects consecutive arcs with the straight edges.
///
/// A square corner among rounded ones (radius < 0.5, the same threshold
/// as the all-square case) is a single POINT, never a zero-size arc:
/// `AddArc` rejects a zero width/height, so the corner was silently
/// dropped and the figure cut diagonally between its neighbours.
pub(crate) unsafe fn build_round_rect_path(
    path: *mut GpPath,
    w: f32,
    h: f32,
    radii: [f32; 4],
    inset: f32,
) {
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

    // Clamp radii to half the box so arcs never overlap (a 999px
    // "max radius" clamps to a circle/capsule — CSS behavior).
    let rmax = bw.min(bh) / 2.0;
    let c = |i: usize| radii[i].clamp(0.0, rmax);
    let (tl, tr, br, bl) = (c(0), c(1), c(2), c(3));
    let corner = |r: f32, arc_x: f32, arc_y: f32, start: f32, at: (f32, f32)| {
        if r < 0.5 {
            let _ = GdipAddPathLine(path, at.0, at.1, at.0, at.1);
        } else {
            let _ = GdipAddPathArc(path, arc_x, arc_y, 2.0 * r, 2.0 * r, start, 90.0);
        }
    };
    corner(tl, left, top, 180.0, (left, top));
    corner(tr, right - 2.0 * tr, top, 270.0, (right, top));
    corner(br, right - 2.0 * br, bottom - 2.0 * br, 0.0, (right, bottom));
    corner(bl, left, bottom - 2.0 * bl, 90.0, (left, bottom));
    let _ = GdipClosePathFigure(path);
}

// =========================================================================
// Gradient painting
// =========================================================================

/// Build `(colors, positions)` arrays for a GDI+ preset blend from
/// ascending-offset sRGB stops, multiplying `alpha` into each stop.
/// GDI+ requires `positions[0] == 0.0` and `positions[last] == 1.0`, so
/// the extremes are padded with the end-stop colors when the author's
/// stops don't reach them. With `reverse` (radial/path gradients, whose
/// blend runs boundary→center rather than the framework's center→edge),
/// each position becomes `1 - offset` and the arrays are reversed to
/// stay ascending.
pub(crate) fn blend_arrays(
    stops: &[(f32, [f32; 4])],
    alpha: f32,
    reverse: bool,
) -> (Vec<u32>, Vec<f32>) {
    let mut pts: Vec<(f32, u32)> = stops
        .iter()
        .map(|(o, c)| (o.clamp(0.0, 1.0), argb_from_srgba(*c, alpha)))
        .collect();
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

/// Fill `fill_path` (the node's rounded-rect region) with the resolved
/// gradient. Linear uses a line-gradient brush along the CSS axis; radial
/// uses a path-gradient brush over an ellipse of the computed radius,
/// clamped outside so box corners beyond the radius keep the edge color.
/// Multi-stop ramps go through the preset-blend APIs.
///
/// A linear gradient under an axis-aligned transform skips the brush:
/// [`paint_linear_raster`] draws the same pixels ~6× faster under Wine
/// (see `gradient_raster`). `radii` is the shape `fill_path` was built
/// with, which the raster reproduces.
unsafe fn paint_gradient(
    g: *mut GpGraphics,
    gp: &GradientPaint,
    fill_path: *mut GpPath,
    w: f32,
    h: f32,
    radii: [f32; 4],
    alpha: f32,
) {
    use windows::Win32::Graphics::GdiPlus::{
        GdipAddPathEllipse, GdipCreatePathGradientFromPath, GdipSetPathGradientCenterColor,
        GdipSetPathGradientCenterPoint, GdipSetPathGradientPresetBlend,
        GdipSetPathGradientSurroundColorsWithCount, GdipSetPathGradientWrapMode, GpPathGradient,
        PointF, WrapModeClamp,
    };

    match gp.kind {
        GradKind::Linear { angle_deg } => {
            let (s, e) = crate::linear_points(angle_deg, w, h);
            if !paint_linear_raster(g, gp, s, e, w, h, radii, alpha) {
                paint_linear_brush(g, gp, s, e, fill_path, alpha);
            }
        }
        GradKind::Radial { center, radius, farthest } => {
            let cx = center.0 * w;
            let cy = center.1 * h;
            let r = crate::radial_radius(center, radius, farthest, w, h).max(1.0);
            let mut ellipse: *mut GpPath = std::ptr::null_mut();
            if GdipCreatePath(FillModeWinding, &mut ellipse).0 != 0 || ellipse.is_null() {
                return;
            }
            let _ = GdipAddPathEllipse(ellipse, cx - r, cy - r, 2.0 * r, 2.0 * r);
            let mut brush: *mut GpPathGradient = std::ptr::null_mut();
            if GdipCreatePathGradientFromPath(ellipse, &mut brush).0 == 0 && !brush.is_null() {
                let inner = gp.stops.first().map(|s| argb_from_srgba(s.1, alpha)).unwrap_or(0);
                let outer = gp.stops.last().map(|s| argb_from_srgba(s.1, alpha)).unwrap_or(inner);
                let cp = PointF { X: cx, Y: cy };
                let _ = GdipSetPathGradientCenterPoint(brush, &cp);
                let _ = GdipSetPathGradientCenterColor(brush, inner);
                let mut cnt = 1i32;
                let _ = GdipSetPathGradientSurroundColorsWithCount(brush, &outer, &mut cnt);
                let (colors, positions) = blend_arrays(&gp.stops, alpha, true);
                if colors.len() >= 2 {
                    let _ = GdipSetPathGradientPresetBlend(
                        brush,
                        colors.as_ptr(),
                        positions.as_ptr(),
                        colors.len() as i32,
                    );
                }
                // Clamp: don't tile the ramp. A path-gradient brush paints
                // nothing OUTSIDE its path whatever the wrap mode, so the
                // box beyond the ellipse is filled separately below.
                let _ = GdipSetPathGradientWrapMode(brush, WrapModeClamp);
                let _ = GdipFillPath(g, brush as *mut GpBrush, fill_path);
                let _ = GdipDeleteBrush(brush as *mut GpBrush);
                paint_beyond_ellipse(g, fill_path, ellipse, (cx, cy, r), w, h, outer);
            }
            let _ = GdipDeletePath(ellipse);
        }
    }
}

/// CSS paints a radial gradient's last stop beyond its ending shape; a
/// GDI+ path-gradient brush leaves everything outside its path blank (the
/// corners of a `closest-side` circle in a box were unpainted). Fill the
/// box minus the ellipse with `outer`: one path holding the shape and the
/// ellipse, alternate fill, so the overlap cancels — never painting under
/// the ellipse, where a translucent outer stop would composite twice. When
/// the ellipse leaves the box its outside part would fill too, so the box
/// then clips the fill.
unsafe fn paint_beyond_ellipse(
    g: *mut GpGraphics,
    fill_path: *mut GpPath,
    ellipse: *mut GpPath,
    (cx, cy, r): (f32, f32, f32),
    w: f32,
    h: f32,
    outer: u32,
) {
    use windows::Win32::Graphics::GdiPlus::{
        FillModeAlternate, GdipAddPathPath, GdipClonePath, GdipSetPathFillMode,
    };
    if outer >> 24 == 0 {
        return;
    }
    // The ellipse covers the whole box: nothing lies beyond it.
    let covers = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)]
        .iter()
        .all(|(x, y)| (x - cx).powi(2) + (y - cy).powi(2) <= r * r);
    if covers {
        return;
    }
    let mut ring: *mut GpPath = std::ptr::null_mut();
    if GdipClonePath(fill_path, &mut ring).0 != 0 || ring.is_null() {
        return;
    }
    let _ = GdipAddPathPath(ring, ellipse, windows::Win32::Foundation::BOOL(0));
    let _ = GdipSetPathFillMode(ring, FillModeAlternate);
    let escapes = cx - r < 0.0 || cy - r < 0.0 || cx + r > w || cy + r > h;
    let mut state: u32 = 0;
    if escapes {
        let _ = GdipSaveGraphics(g, &mut state);
        let _ = GdipSetClipPath(g, fill_path, CombineModeIntersect);
    }
    let mut brush: *mut GpSolidFill = std::ptr::null_mut();
    if GdipCreateSolidFill(outer, &mut brush).0 == 0 {
        let _ = GdipFillPath(g, brush as *mut GpBrush, ring);
        let _ = GdipDeleteBrush(brush as *mut GpBrush);
    }
    if escapes {
        let _ = GdipRestoreGraphics(g, state);
    }
    let _ = GdipDeletePath(ring);
}

/// The GDI+ line-brush linear gradient — the path for transforms the
/// raster can't express (rotation, skew, mirroring).
unsafe fn paint_linear_brush(
    g: *mut GpGraphics,
    gp: &GradientPaint,
    s: (f32, f32),
    e: (f32, f32),
    fill_path: *mut GpPath,
    alpha: f32,
) {
    use windows::Win32::Graphics::GdiPlus::{
        GdipCreateLineBrush, GdipSetLinePresetBlend, GpLineGradient, PointF, WrapModeTileFlipX,
    };
    let p1 = PointF { X: s.0, Y: s.1 };
    let p2 = PointF { X: e.0, Y: e.1 };
    let first = gp.stops.first().map(|s| argb_from_srgba(s.1, alpha)).unwrap_or(0);
    let last = gp.stops.last().map(|s| argb_from_srgba(s.1, alpha)).unwrap_or(first);
    let mut brush: *mut GpLineGradient = std::ptr::null_mut();
    // The axis spans the box's full projected extent (see
    // `linear_points`), so pixels project into [0, 1] — but a pixel
    // centre exactly ON the far edge projects to t = 1.0, which
    // `WrapModeTile` wraps to 0: a 1-px seam of the START color along the
    // far edge (GDI+ centres pixels on integers, so any whole-pixel edge
    // hits it). Mirroring keeps t = 1.0 at the end color.
    if GdipCreateLineBrush(&p1, &p2, first, last, WrapModeTileFlipX, &mut brush).0 == 0
        && !brush.is_null()
    {
        let (colors, positions) = blend_arrays(&gp.stops, alpha, false);
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

/// The fast linear path: rasterize onto the device grid
/// ([`gradient_raster::linear`]) and copy it 1:1. `false` when the world
/// transform isn't axis-aligned — the caller then uses the brush. `true`
/// covers "drawn" and "nothing visible".
#[allow(clippy::too_many_arguments)]
unsafe fn paint_linear_raster(
    g: *mut GpGraphics,
    gp: &GradientPaint,
    start: (f32, f32),
    end: (f32, f32),
    w: f32,
    h: f32,
    radii: [f32; 4],
    alpha: f32,
) -> bool {
    use crate::gradient_raster::{self, Raster};
    use windows::Win32::Graphics::GdiPlus::{
        GdipCreateBitmapFromScan0, GdipCreateMatrix, GdipDeleteMatrix, GdipDisposeImage,
        GdipDrawImageRectRectI, GdipGetMatrixElements, GdipGetVisibleClipBoundsI,
        GdipGetWorldTransform, GdipResetWorldTransform, GdipSetInterpolationMode,
        GdipSetPixelOffsetMode, GpBitmap, GpImage, InterpolationModeNearestNeighbor, Matrix,
        PixelOffsetModeHalf, Rect, UnitPixel,
    };
    /// `PixelFormat32bppPARGB` from `gdipluspixelformats.h` — a `#define`,
    /// so the `windows` crate carries no constant for it.
    const PIXEL_FORMAT_32BPP_PARGB: i32 = 0x000E_200B;

    let mut m: *mut Matrix = std::ptr::null_mut();
    if GdipCreateMatrix(&mut m).0 != 0 || m.is_null() {
        return false;
    }
    let mut el = [0f32; 6];
    let ok = GdipGetWorldTransform(g, m).0 == 0 && GdipGetMatrixElements(m, el.as_mut_ptr()).0 == 0;
    let _ = GdipDeleteMatrix(m);
    let Some(map) = ok.then(|| gradient_raster::axis_aligned(el)).flatten() else {
        return false;
    };
    // GDI+'s default `PixelOffsetModeNone` centres device pixel p at p, not
    // p + 0.5 — every other shape in the scene is rasterized that way. The
    // raster centres pixels at p + 0.5, so shift the box by half a pixel to
    // land on the same grid; otherwise each gradient sits half a pixel
    // down-right of its own background fill and border.
    let map = gradient_raster::DeviceMap { dx: map.dx + 0.5, dy: map.dy + 0.5, ..map };

    // Everything below draws in DEVICE space; the save/restore puts the
    // world transform (and the modes) back. The clip region is held in
    // device space, so resetting the transform leaves ancestor clips intact
    // and the visible bounds read here are device pixels.
    let mut state: u32 = 0;
    let _ = GdipSaveGraphics(g, &mut state);
    let _ = GdipResetWorldTransform(g);
    let mut vis = Rect::default();
    if GdipGetVisibleClipBoundsI(g, &mut vis).0 != 0 {
        let _ = GdipRestoreGraphics(g, state);
        return false;
    }
    let (colors, positions) = blend_arrays(&gp.stops, alpha, false);
    let raster = gradient_raster::linear(
        start,
        end,
        &colors,
        &positions,
        w,
        h,
        radii,
        map,
        (vis.X, vis.Y, vis.Width, vis.Height),
    );
    if let Some(Raster { x, y, width, height, mut pixels }) = raster {
        let mut bmp: *mut GpBitmap = std::ptr::null_mut();
        // Wraps `pixels` without copying; disposed before `pixels` drops.
        if GdipCreateBitmapFromScan0(
            width,
            height,
            width * 4,
            PIXEL_FORMAT_32BPP_PARGB,
            Some(pixels.as_mut_ptr() as *const u8),
            &mut bmp,
        )
        .0 == 0
            && !bmp.is_null()
        {
            // Nearest + half-pixel offset: an integer-aligned, unscaled
            // copy, no resampling blur at the raster's edges.
            let _ = GdipSetInterpolationMode(g, InterpolationModeNearestNeighbor);
            let _ = GdipSetPixelOffsetMode(g, PixelOffsetModeHalf);
            let _ = GdipDrawImageRectRectI(
                g,
                bmp as *mut GpImage,
                x,
                y,
                width,
                height,
                0,
                0,
                width,
                height,
                UnitPixel,
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
            );
            let _ = GdipDisposeImage(bmp as *mut GpImage);
        }
    }
    let _ = GdipRestoreGraphics(g, state);
    true
}

/// Paint a view's border. Solid borders take the original path
/// unchanged — a uniform border strokes the rounded path, anything else
/// draws straight per-side bars. Dashed / dotted borders draw the marks
/// [`border_pattern::plan`] lays out (see that module for why they are
/// explicit marks rather than a GDI+ dashed pen).
pub(crate) unsafe fn paint_border(g: *mut GpGraphics, v: &ViewVisual, w: f32, h: f32, alpha: f32) {
    if let Some(marks) = border_pattern::plan(&v.borders, v.radii, v.border_style, w, h) {
        paint_border_marks(g, &marks, alpha);
        return;
    }
    match uniform_border(&v.borders) {
        Some((width, color)) => {
            let mut stroke: *mut GpPath = std::ptr::null_mut();
            if GdipCreatePath(FillModeWinding, &mut stroke).0 == 0 && !stroke.is_null() {
                build_round_rect_path(stroke, w, h, v.radii, width / 2.0);
                let mut pen: *mut GpPen = std::ptr::null_mut();
                if GdipCreatePen1(
                    argb_with_alpha(color.to_argb_u32(), alpha),
                    width,
                    UnitPixel,
                    &mut pen,
                )
                .0 == 0
                {
                    let _ = GdipDrawPath(g, pen, stroke);
                    let _ = GdipDeletePen(pen);
                }
                let _ = GdipDeletePath(stroke);
            }
        }
        None if v.borders.iter().any(|s| s.width > 0.5) => {
            paint_side_borders(g, &v.borders, w, h, alpha);
        }
        None => {}
    }
}

/// Draw [`BorderMark`]s: each dash an open polyline under a flat-capped
/// pen (flat so a dash's visible length is exactly its run — a round or
/// square cap would add half the width at each end), each dot a filled
/// circle.
unsafe fn paint_border_marks(g: *mut GpGraphics, marks: &[BorderMark], alpha: f32) {
    use windows::Win32::Graphics::GdiPlus::{
        GdipDrawLines, GdipFillEllipse, GdipSetPenLineJoin, LineJoinRound, PointF,
    };
    for mark in marks {
        match mark {
            BorderMark::Dash { points, width, color } => {
                if points.len() < 2 {
                    continue;
                }
                let pts: Vec<PointF> = points.iter().map(|(x, y)| PointF { X: *x, Y: *y }).collect();
                let mut pen: *mut GpPen = std::ptr::null_mut();
                if GdipCreatePen1(argb_with_alpha(color.to_argb_u32(), alpha), *width, UnitPixel, &mut pen).0 == 0
                    && !pen.is_null()
                {
                    // Round joins: a dash sampled round a rounded corner
                    // has many shallow joins, and a miter at each would
                    // spike outward. GDI+'s default caps are already flat.
                    let _ = GdipSetPenLineJoin(pen, LineJoinRound);
                    let _ = GdipDrawLines(g, pen, pts.as_ptr(), pts.len() as i32);
                    let _ = GdipDeletePen(pen);
                }
            }
            BorderMark::Dot { cx, cy, radius, color } => {
                let mut brush: *mut GpSolidFill = std::ptr::null_mut();
                if GdipCreateSolidFill(argb_with_alpha(color.to_argb_u32(), alpha), &mut brush).0 == 0 {
                    let d = radius * 2.0;
                    let _ = GdipFillEllipse(g, brush as *mut GpBrush, cx - radius, cy - radius, d, d);
                    let _ = GdipDeleteBrush(brush as *mut GpBrush);
                }
            }
        }
    }
}

/// Draw asymmetric borders as straight per-side bars (`[top, right,
/// bottom, left]`), each a filled rectangle of its own width + color. A
/// side with no explicit color falls back to the first side that has
/// one. Corners aren't mitered and a rounded corner isn't traced — the
/// documented limitation shared with the Apple backends' per-side bars
/// (a straight bar can't follow a curve).
unsafe fn paint_side_borders(
    g: *mut GpGraphics,
    sides: &[BorderSide; 4],
    w: f32,
    h: f32,
    alpha: f32,
) {
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
            0 => (0.0, 0.0, w, bw),    // top
            1 => (w - bw, 0.0, bw, h), // right
            2 => (0.0, h - bw, w, bw), // bottom
            _ => (0.0, 0.0, bw, h),    // left
        };
        let mut brush: *mut GpSolidFill = std::ptr::null_mut();
        if GdipCreateSolidFill(argb_with_alpha(color.to_argb_u32(), alpha), &mut brush).0 == 0 {
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

// =========================================================================
// String format — shared GenericTypographic
// =========================================================================

/// The shared GenericTypographic string format: no extra em-padding at
/// the string edges, so GDI+ drawing lines up with the GDI-measured
/// intrinsic box. GDI+ documents this as a cached built-in object —
/// fetched once, never deleted.
fn typographic_format() -> *mut GpStringFormat {
    use std::sync::atomic::{AtomicPtr, Ordering};
    static FMT: AtomicPtr<GpStringFormat> = AtomicPtr::new(std::ptr::null_mut());
    let cur = FMT.load(Ordering::Relaxed);
    if !cur.is_null() {
        return cur;
    }
    let mut fmt: *mut GpStringFormat = std::ptr::null_mut();
    unsafe {
        let _ = GdipStringFormatGetGenericTypographic(&mut fmt);
    }
    FMT.store(fmt, Ordering::Relaxed);
    fmt
}

// =========================================================================
// Back buffer + scene entry
// =========================================================================

/// Owned double-buffer target. Persisted across frames (recreating a
/// full-window bitmap 60×/s is wasteful); the bitmap is rebuilt on
/// resize. Freed in the backend's `Drop`.
pub(crate) struct BackBuffer {
    pub dc: HDC,
    pub bmp: HBITMAP,
    pub size: (i32, i32),
}

impl BackBuffer {
    pub(crate) fn new() -> Self {
        BackBuffer { dc: HDC(std::ptr::null_mut()), bmp: HBITMAP(std::ptr::null_mut()), size: (0, 0) }
    }

    /// Make the buffer compatible with `target` at `w × h`.
    unsafe fn ensure(&mut self, target: HDC, w: i32, h: i32) -> bool {
        if self.dc.is_invalid() {
            self.dc = CreateCompatibleDC(target);
            if self.dc.is_invalid() {
                return false;
            }
        }
        if self.size != (w, h) || self.bmp.is_invalid() {
            let bmp = CreateCompatibleBitmap(target, w.max(1), h.max(1));
            if bmp.is_invalid() {
                return false;
            }
            // Selecting the new bitmap returns the previous one (our old
            // frame bitmap, or the DC's 1×1 stock bitmap on first use —
            // DeleteObject on a stock object is a harmless no-op).
            let old = SelectObject(self.dc, HGDIOBJ(bmp.0));
            let _ = DeleteObject(old);
            self.bmp = bmp;
            self.size = (w, h);
        }
        true
    }

    pub(crate) unsafe fn release(&mut self) {
        if !self.dc.is_invalid() {
            let _ = DeleteDC(self.dc);
            self.dc = HDC(std::ptr::null_mut());
        }
        // The selected bitmap is destroyed with the DC's last reference;
        // delete explicitly for tidiness if still live.
        if !self.bmp.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(self.bmp.0));
            self.bmp = HBITMAP(std::ptr::null_mut());
        }
    }
}

/// Paint the whole scene into `target` (the host's `WM_PAINT` DC) at
/// `w × h` DEVICE pixels (the client rect). Renders into the retained
/// back buffer, then blits — one flicker-free flip.
pub(crate) unsafe fn paint_scene(b: &mut WindowsBackend, target: HDC, w: i32, h: i32) {
    if w <= 0 || h <= 0 {
        return;
    }
    // Post-mount tree mutations (a navigator swapping its outlet, a
    // reactive branch toggling, text growing) mark layout dirty; run
    // the pass here so every frame paints against current geometry.
    if b.layout_dirty {
        b.layout_pass();
    }
    let plan = b.layer_plan();
    // Split-borrow the back buffer out so `b`'s other fields stay
    // reachable below.
    let mut back = std::mem::replace(&mut b.back, BackBuffer::new());
    if !back.ensure(target, w, h) {
        b.back = back;
        return;
    }

    // Prep phase: make sure every text node's GpFont exists (creation
    // wants an HDC, which we have now). Mutates only the font cache.
    let keys: Vec<font::FontKey> = b
        .nodes
        .values()
        .filter_map(|m| match &m.kind {
            NodeKind::Text(t) => {
                Some(t.font_key.clone().unwrap_or_else(|| b.default_font_key.clone()))
            }
            NodeKind::Code(c) => Some(c.font_key.clone()),
            _ => None,
        })
        .collect();
    for key in keys {
        if let Some(entry) = font::entry_for(&mut b.font_cache, &key) {
            let _ = font::gpfont_for(entry, back.dc);
        }
    }

    let mut g: *mut GpGraphics = std::ptr::null_mut();
    if GdipCreateFromHDC(back.dc, &mut g).0 == 0 && !g.is_null() {
        let _ = GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
        // ClearType subpixel rendering assumes an opaque background and
        // fringes badly over animated/alpha fills; plain AA composites
        // correctly everywhere.
        let _ = GdipSetTextRenderingHint(g, TextRenderingHintAntiAlias);

        // Clear to the app background (white default) so unmounted /
        // transparent regions are a clean backdrop.
        let clear = b
            .app_background
            .map(|c| c.to_argb_u32())
            .unwrap_or(0xFFFF_FFFF);
        let _ = GdipGraphicsClear(g, clear);

        // The ONE logical → device conversion for everything painted:
        // frames, borders, radii, text (GDI+ fonts are logical-px
        // `UnitPixel` fonts, rasterized at device size under this
        // transform), icons and images all draw in logical pixels and
        // land on the DPI-sized back buffer. See `dpi.rs`.
        let scale = b.dpi.factor();
        if scale != 1.0 {
            let _ = GdipScaleWorldTransform(g, scale, scale, MatrixOrderPrepend);
        }

        if let Some(root) = b.root_id {
            paint_node(b, g, root, 1.0, LayerFilter { plan: &plan, layer: 0 });
        }
        let _ = GdipDeleteGraphics(g);
    }

    let _ = BitBlt(target, 0, 0, w, h, back.dc, 0, 0, SRCCOPY);
    b.back = back;

    // Content over a graphics surface, in composition visuals above it
    // (`crate::layers`).
    let lifted: Vec<crate::overlay::LayerPixels> = plan
        .lifted()
        .filter_map(|(k, o)| {
            let bounds = layers::pixel_box(o.bounds?);
            let pixels = paint_layer_pixels(b, &plan, k, bounds)?;
            Some(crate::overlay::LayerPixels { above: o.above, bounds, pixels })
        })
        .collect();
    b.present_overlays(lifted);
}

/// Which nodes a paint walk draws: those `plan` puts in `layer`. The walk
/// still visits every node, so transforms, clips and opacity apply.
#[derive(Clone, Copy)]
pub(crate) struct LayerFilter<'a> {
    pub plan: &'a LayerPlan,
    pub layer: usize,
}

impl LayerFilter<'_> {
    pub(crate) fn draws(&self, id: u64) -> bool {
        self.plan.layer(id) == self.layer
    }
}

/// GDI+ `PixelFormat32bppPARGB` (gdipluspixelformats.h): premultiplied
/// BGRA, the layout a composition surface takes.
const PIXEL_FORMAT_32BPP_PARGB: i32 = 0x000E_200B;

/// Paint lifted layer `layer` into premultiplied BGRA pixels covering the
/// device box `(x, y, w, h)`, transparent where it draws nothing.
unsafe fn paint_layer_pixels(
    b: &WindowsBackend,
    plan: &LayerPlan,
    layer: usize,
    (x, y, w, h): (i32, i32, u32, u32),
) -> Option<Vec<u8>> {
    let root = b.root_id?;
    let stride = w as usize * 4;
    let mut pixels = vec![0u8; stride * h as usize];
    let mut bmp: *mut GpBitmap = std::ptr::null_mut();
    if GdipCreateBitmapFromScan0(w as i32, h as i32, stride as i32, PIXEL_FORMAT_32BPP_PARGB, Some(pixels.as_mut_ptr()), &mut bmp).0 != 0
        || bmp.is_null()
    {
        return None;
    }
    let mut g: *mut GpGraphics = std::ptr::null_mut();
    if GdipGetImageGraphicsContext(bmp as *mut GpImage, &mut g).0 == 0 && !g.is_null() {
        let _ = GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
        let _ = GdipSetTextRenderingHint(g, TextRenderingHintAntiAlias);
        let _ = GdipTranslateWorldTransform(g, -x as f32, -y as f32, MatrixOrderPrepend);
        let scale = b.dpi.factor();
        if scale != 1.0 {
            let _ = GdipScaleWorldTransform(g, scale, scale, MatrixOrderPrepend);
        }
        paint_node(b, g, root, 1.0, LayerFilter { plan, layer });
        let _ = GdipDeleteGraphics(g);
    }
    // The bitmap draws into `pixels` (its scan0); disposing it flushes.
    let _ = GdipDisposeImage(bmp as *mut GpImage);
    Some(pixels)
}

/// Paint one node + its subtree. `alpha` is the cumulative ancestor
/// opacity.
unsafe fn paint_node(b: &WindowsBackend, g: *mut GpGraphics, id: u64, alpha: f32, layer: LayerFilter<'_>) {
    let Some(meta) = b.nodes.get(&id) else {
        return;
    };
    // Portal-hidden subtrees (navigated-away screens) don't paint.
    if meta.hidden {
        return;
    }
    let alpha = alpha * meta.effective_opacity();
    // Fully transparent subtrees skip entirely — this is how welcome's
    // pre-act layers (opacity 0 until their act) stay invisible.
    if alpha <= 0.003 {
        return;
    }
    let (fx, fy, w, h) = meta.frame;

    let mut state: u32 = 0;
    let _ = GdipSaveGraphics(g, &mut state);
    let _ = GdipTranslateWorldTransform(g, fx, fy, MatrixOrderPrepend);
    for op in transform_ops(w, h, &meta.author_transform, &meta.anim) {
        match op {
            XOp::Translate(dx, dy) => {
                let _ = GdipTranslateWorldTransform(g, dx, dy, MatrixOrderPrepend);
            }
            XOp::Scale(sx, sy) => {
                let _ = GdipScaleWorldTransform(g, sx, sy, MatrixOrderPrepend);
            }
            XOp::Rotate(deg) => {
                let _ = GdipRotateWorldTransform(g, deg, MatrixOrderPrepend);
            }
        }
    }

    // Own visual. A painted leaf draws inside its padding (see
    // `NodeMeta::content_inset`); views have a zero inset.
    let (cx, cy, w, h) = crate::painted_geom::content_box(w, h, meta.content_inset);
    if cx != 0.0 || cy != 0.0 {
        let _ = GdipTranslateWorldTransform(g, cx, cy, MatrixOrderPrepend);
    }
    if layer.draws(id) {
        match &meta.kind {
            NodeKind::View(v) => {
                let has_bg = v.effective_background().map(|c| c[3] > 0.0).unwrap_or(false);
                let has_gradient = v.gradient.as_ref().map(|gr| !gr.stops.is_empty()).unwrap_or(false);
                if (has_bg || has_gradient) && w > 0.0 && h > 0.0 {
                    let mut fill_path: *mut GpPath = std::ptr::null_mut();
                    if GdipCreatePath(FillModeWinding, &mut fill_path).0 == 0 && !fill_path.is_null() {
                        build_round_rect_path(fill_path, w, h, v.radii, 0.0);
                        if let Some(bg) = v.effective_background().filter(|c| c[3] > 0.0) {
                            let mut brush: *mut GpSolidFill = std::ptr::null_mut();
                            if GdipCreateSolidFill(argb_from_srgba(bg, alpha), &mut brush).0 == 0 {
                                let _ = GdipFillPath(g, brush as *mut GpBrush, fill_path);
                                let _ = GdipDeleteBrush(brush as *mut GpBrush);
                            }
                        }
                        if let Some(gr) = v.gradient.as_ref().filter(|gr| !gr.stops.is_empty()) {
                            paint_gradient(g, gr, fill_path, w, h, v.radii, alpha);
                        }
                        let _ = GdipDeletePath(fill_path);
                    }
                }
                paint_border(g, v, w, h, alpha);
            }
            NodeKind::Text(t) => {
                if !t.content.is_empty() {
                    let key = t.font_key.clone().unwrap_or_else(|| b.default_font_key.clone());
                    if let Some(entry) = b.font_cache.get(&key) {
                        if !entry.gpfont.is_null() {
                            let color = t
                                .anim_color
                                .map(|c| argb_from_srgba(c, alpha))
                                .unwrap_or_else(|| argb_with_alpha(t.color.to_argb_u32(), alpha));
                            let mut brush: *mut GpSolidFill = std::ptr::null_mut();
                            if GdipCreateSolidFill(color, &mut brush).0 == 0 {
                                // Zero-size layout rects = draw from the
                                // origin, no GDI+ wrap, no clip. Wrapping is
                                // OURS: the line cache `layout_pass` filled
                                // from the node's WrapPlan (same breaks the
                                // measure fn reported to Taffy). Each line is
                                // its own draw at `advance` steps, offset by
                                // the CSS half-leading and per-line align.
                                match (&t.lines, &t.plan) {
                                    (Some(cache), Some(plan)) => {
                                        let advance =
                                            t.line_height.unwrap_or(plan.font_height).max(1.0);
                                        let lead = ((advance - plan.font_height) / 2.0).max(0.0);
                                        for (i, line) in cache.lines.iter().enumerate() {
                                            if line.text16.is_empty() {
                                                continue;
                                            }
                                            // `Justify` unsupported → Left.
                                            let x = match t.align {
                                                TextAlign::Center => {
                                                    ((w - line.width) / 2.0).max(0.0)
                                                }
                                                TextAlign::Right => (w - line.width).max(0.0),
                                                TextAlign::Left | TextAlign::Justify => 0.0,
                                            };
                                            let layout = RectF {
                                                X: x,
                                                Y: lead + i as f32 * advance,
                                                Width: 0.0,
                                                Height: 0.0,
                                            };
                                            let _ = GdipDrawString(
                                                g,
                                                PCWSTR(line.text16.as_ptr()),
                                                line.text16.len() as i32,
                                                entry.gpfont,
                                                &layout,
                                                typographic_format(),
                                                brush as *mut GpBrush,
                                            );
                                        }
                                    }
                                    // No plan/cache yet (paint racing the
                                    // first layout fill) — single-line draw.
                                    _ => {
                                        let wide: Vec<u16> = t.content.encode_utf16().collect();
                                        let layout =
                                            RectF { X: 0.0, Y: 0.0, Width: 0.0, Height: 0.0 };
                                        let _ = GdipDrawString(
                                            g,
                                            PCWSTR(wide.as_ptr()),
                                            wide.len() as i32,
                                            entry.gpfont,
                                            &layout,
                                            typographic_format(),
                                            brush as *mut GpBrush,
                                        );
                                    }
                                }
                                let _ = GdipDeleteBrush(brush as *mut GpBrush);
                            }
                        }
                    }
                }
            }
            NodeKind::Code(c) => {
                if let Some(entry) = b.font_cache.get(&c.font_key) {
                    if !entry.gpfont.is_null() {
                        for (i, line) in c.lines.iter().enumerate() {
                            let y = i as f32 * c.line_height;
                            let mut x = 0.0_f32;
                            for run in line {
                                if !run.text16.is_empty() {
                                    let color = argb_with_alpha(run.color.to_argb_u32(), alpha);
                                    let mut brush: *mut GpSolidFill = std::ptr::null_mut();
                                    if GdipCreateSolidFill(color, &mut brush).0 == 0 {
                                        let layout =
                                            RectF { X: x, Y: y, Width: 0.0, Height: 0.0 };
                                        let _ = GdipDrawString(
                                            g,
                                            PCWSTR(run.text16.as_ptr()),
                                            run.text16.len() as i32,
                                            entry.gpfont,
                                            &layout,
                                            typographic_format(),
                                            brush as *mut GpBrush,
                                        );
                                        let _ = GdipDeleteBrush(brush as *mut GpBrush);
                                    }
                                }
                                x += run.width;
                            }
                        }
                    }
                }
            }
            NodeKind::Icon(p) => icon::paint_into(g, p, w, h, alpha),
            NodeKind::Image(p) => image::paint_into(g, p, w, h, b.dpi.svg_screen_scale()),
            // Native controls paint themselves in their own HWNDs (the
            // host's WS_CLIPCHILDREN keeps our blit off them).
            NodeKind::Control { .. } | NodeKind::External { .. } => {}
        }
    }
    if cx != 0.0 || cy != 0.0 {
        let _ = GdipTranslateWorldTransform(g, -cx, -cy, MatrixOrderPrepend);
    }
    let (_, _, w, h) = meta.frame;

    // Children: clip when overflow hidden (honoring corner radii), apply
    // scroll offset, and paint in stable z order.
    let Some(child_ids) = b.children.get(&id) else {
        let _ = GdipRestoreGraphics(g, state);
        return;
    };
    if child_ids.is_empty() {
        let _ = GdipRestoreGraphics(g, state);
        return;
    }

    if meta.clips_children() {
        let radii = match &meta.kind {
            NodeKind::View(v) => v.radii,
            _ => [0.0; 4],
        };
        let mut clip: *mut GpPath = std::ptr::null_mut();
        if GdipCreatePath(FillModeWinding, &mut clip).0 == 0 && !clip.is_null() {
            build_round_rect_path(clip, w, h, radii, 0.0);
            let _ = GdipSetClipPath(g, clip, CombineModeIntersect);
            let _ = GdipDeletePath(clip);
        }
    }
    if let NodeKind::View(v) = &meta.kind {
        if let Some(s) = &v.scroll {
            let _ = GdipTranslateWorldTransform(g, -s.offset_x, -s.offset_y, MatrixOrderPrepend);
        }
    }

    let pairs: Vec<(u64, f32)> = child_ids
        .iter()
        .filter_map(|cid| b.nodes.get(cid).map(|m| (*cid, m.z)))
        .collect();
    for cid in z_sorted(&pairs) {
        paint_node(b, g, cid, alpha, layer);
    }
    let _ = GdipRestoreGraphics(g, state);
}

// =========================================================================
// Hit testing
// =========================================================================

/// One node on a hit path: its id and window-space (logical px) origin,
/// the point its local coordinates are measured from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PathNode {
    pub id: u64,
    pub origin: (f32, f32),
}

/// The hit target under client-space `(px, py)` followed by its
/// ancestors, deepest first; empty when nothing (not even the root) is
/// hit. See the module docs for the target rules.
pub(crate) fn hit_chain(b: &WindowsBackend, px: f32, py: f32) -> Vec<PathNode> {
    let mut out = Vec::new();
    if let Some(root) = b.root_id {
        hit_walk(b, root, px, py, 0.0, 0.0, false, &mut out);
    }
    out
}

/// The pressable that takes a click at `(px, py)`: the nearest one on the
/// hit chain. A view painted over a pressable takes the click itself (as
/// on web), so the pressable underneath does not fire.
pub(crate) fn pressable_at(b: &WindowsBackend, px: f32, py: f32) -> Option<Rc<dyn Fn()>> {
    hit_chain(b, px, py).into_iter().find_map(|n| match &b.nodes.get(&n.id)?.kind {
        NodeKind::View(v) => v.on_click.clone(),
        _ => None,
    })
}

pub(crate) fn node_origin(b: &WindowsBackend, id: u64, ox: f32, oy: f32) -> Option<(f32, f32, f32, f32)> {
    let meta = b.nodes.get(&id)?;
    let (fx, fy, w, h) = meta.frame;
    // Translation-only approximation of the transform chain.
    let mut dx = meta.anim.tx;
    let mut dy = meta.anim.ty;
    for t in &meta.author_transform {
        match t {
            Transform::TranslateX(l) => dx += resolve_len(l, w),
            Transform::TranslateY(l) => dy += resolve_len(l, h),
            _ => {}
        }
    }
    Some((ox + fx + dx, oy + fy + dy, w, h))
}

/// Where `id`'s children are laid out from: its origin minus its scroll
/// offset.
pub(crate) fn content_origin(meta: &NodeMeta, nx: f32, ny: f32) -> (f32, f32) {
    match &meta.kind {
        NodeKind::View(ViewVisual { scroll: Some(s), .. }) => (nx - s.offset_x, ny - s.offset_y),
        _ => (nx, ny),
    }
}

/// Appends the hit path below `id` (deepest first, `id` last) and returns
/// whether anything under `id` was hit. `none_above` is the
/// `pointer_events` verdict inherited from the nearest explicit ancestor.
#[allow(clippy::too_many_arguments)]
fn hit_walk(
    b: &WindowsBackend,
    id: u64,
    px: f32,
    py: f32,
    ox: f32,
    oy: f32,
    none_above: bool,
    out: &mut Vec<PathNode>,
) -> bool {
    let Some((nx, ny, w, h)) = node_origin(b, id, ox, oy) else {
        return false;
    };
    let Some(meta) = b.nodes.get(&id) else {
        return false;
    };
    if meta.hidden {
        return false;
    }
    if meta.effective_opacity() <= 0.003 {
        // Invisible subtrees don't take clicks (matches web/Apple:
        // opacity 0 still hits, but our pre-act layers overlap the
        // content layer full-screen — treat as transparent to input so
        // the visible content underneath stays clickable).
        return false;
    }
    let inside = px >= nx && px < nx + w && py >= ny && py < ny + h;
    // A clipping node's children are not visible outside its box —
    // they must not be clickable there either (a scrolled-out sidebar
    // link would otherwise take clicks meant for whatever is painted
    // in its place).
    if meta.clips_children() && !inside {
        return false;
    }
    let none = match meta.pointer_events {
        Some(PointerEvents::None) => true,
        Some(PointerEvents::Auto) => false,
        None => none_above,
    };
    // Children first, topmost (highest z, later insertion) first.
    let (cx, cy) = content_origin(meta, nx, ny);
    if let Some(child_ids) = b.children.get(&id) {
        let pairs: Vec<(u64, f32)> = child_ids
            .iter()
            .filter_map(|cid| b.nodes.get(cid).map(|m| (*cid, m.z)))
            .collect();
        for cid in z_sorted(&pairs).into_iter().rev() {
            if hit_walk(b, cid, px, py, cx, cy, none, out) {
                out.push(PathNode { id, origin: (nx, ny) });
                return true;
            }
        }
    }
    // An anchor (`contents`) only passes hits through to its children.
    if inside && !none && !meta.contents {
        out.push(PathNode { id, origin: (nx, ny) });
        return true;
    }
    false
}

/// Window-space origin of `target` as the hit walk computes it (scroll
/// offsets and translations included), whether or not anything is under
/// the pointer: a captured drag keeps measuring local positions from the
/// node it started on even after leaving it. `None` once the node is
/// gone or hidden.
pub(crate) fn window_origin(b: &WindowsBackend, target: u64) -> Option<(f32, f32)> {
    fn walk(b: &WindowsBackend, id: u64, target: u64, ox: f32, oy: f32) -> Option<(f32, f32)> {
        let (nx, ny, _, _) = node_origin(b, id, ox, oy)?;
        let meta = b.nodes.get(&id)?;
        if meta.hidden {
            return None;
        }
        if id == target {
            return Some((nx, ny));
        }
        let (cx, cy) = content_origin(meta, nx, ny);
        b.children.get(&id)?.iter().find_map(|cid| walk(b, *cid, target, cx, cy))
    }
    walk(b, b.root_id?, target, 0.0, 0.0)
}

/// Route a mouse-wheel tick to the deepest scroll view under `(px, py)`.
/// Returns `true` if a scroll node consumed it (offset updated +
/// `on_scroll` fired + scene invalidated by the caller).
pub(crate) fn scroll_at(b: &mut WindowsBackend, px: f32, py: f32, delta_px: f32) -> bool {
    let Some(root) = b.root_id else {
        return false;
    };
    let Some(target) = find_scroll_node(b, root, px, py, 0.0, 0.0) else {
        return false;
    };
    let (max_x, max_y) = max_scroll_offsets(b, target);
    let mut cb: Option<(Rc<dyn Fn(f32, f32)>, f32, f32)> = None;
    if let Some(meta) = b.nodes.get_mut(&target) {
        if let NodeKind::View(v) = &mut meta.kind {
            if let Some(s) = &mut v.scroll {
                if s.horizontal {
                    s.offset_x = (s.offset_x + delta_px).clamp(0.0, max_x);
                } else {
                    s.offset_y = (s.offset_y + delta_px).clamp(0.0, max_y);
                }
                if let Some(f) = &s.on_scroll {
                    cb = Some((f.clone(), s.offset_x, s.offset_y));
                }
            }
        }
    }
    if let Some((f, x, y)) = cb {
        f(x, y);
    }
    true
}

/// Scrollable range of node `id`: content extent minus the viewport
/// (the node's own frame), floored at zero. Without the cap the wheel
/// scrolls forever past the end of the content. Bottom/right container
/// padding isn't in any child frame, so the last child sits flush at
/// the end — the same approximation the extent-from-children approach
/// gives on the other immediate-mode backends.
fn max_scroll_offsets(b: &WindowsBackend, id: u64) -> (f32, f32) {
    let Some(meta) = b.nodes.get(&id) else {
        return (0.0, 0.0);
    };
    let (_, _, vw, vh) = meta.frame;
    let (cw, ch) = content_extent(b, id);
    ((cw - vw).max(0.0), (ch - vh).max(0.0))
}

/// Farthest descendant edge inside `id`'s content space. MUST recurse:
/// a scroll container's direct child is often a viewport-sized wrapper
/// whose own descendants are what actually overflow — measuring only
/// direct-child frames reads "nothing to scroll" and clamps every
/// wheel tick to zero. Recursion stops at nodes that clip (nested
/// scroll views, `overflow: hidden`): their overflow is bounded by
/// their own box and must not inflate the outer scroll range.
fn content_extent(b: &WindowsBackend, id: u64) -> (f32, f32) {
    let (mut cw, mut ch) = (0.0_f32, 0.0_f32);
    let Some(kids) = b.children.get(&id) else {
        return (cw, ch);
    };
    for k in kids {
        let Some(m) = b.nodes.get(k) else {
            continue;
        };
        if m.hidden {
            continue;
        }
        let (fx, fy, w, h) = m.frame;
        let (mut ew, mut eh) = (w, h);
        if !m.clips_children() {
            let (iw, ih) = content_extent(b, *k);
            ew = ew.max(iw);
            eh = eh.max(ih);
        }
        cw = cw.max(fx + ew);
        ch = ch.max(fy + eh);
    }
    (cw, ch)
}

fn find_scroll_node(b: &WindowsBackend, id: u64, px: f32, py: f32, ox: f32, oy: f32) -> Option<u64> {
    let (nx, ny, w, h) = node_origin(b, id, ox, oy)?;
    let meta = b.nodes.get(&id)?;
    if meta.hidden {
        return None;
    }
    let inside = px >= nx && px < nx + w && py >= ny && py < ny + h;
    // Same rule as `hit_node`: content outside a clipping box is not
    // there for input purposes.
    if meta.clips_children() && !inside {
        return None;
    }
    let (mut cx, mut cy) = (nx, ny);
    if let NodeKind::View(v) = &meta.kind {
        if let Some(s) = &v.scroll {
            cx -= s.offset_x;
            cy -= s.offset_y;
        }
    }
    if let Some(child_ids) = b.children.get(&id) {
        for cid in child_ids.iter().rev() {
            if let Some(found) = find_scroll_node(b, *cid, px, py, cx, cy) {
                return Some(found);
            }
        }
    }
    if inside {
        if let NodeKind::View(v) = &meta.kind {
            if v.scroll.is_some() {
                return Some(id);
            }
        }
    }
    None
}

/// Erase-to-background COLORREF used by the host for regions Windows
/// insists on erasing (we return "handled" from WM_ERASEBKGND, so this
/// is currently unused, but kept for the maximize-flash edge).
#[allow(dead_code)]
pub(crate) fn clear_colorref(b: &WindowsBackend) -> COLORREF {
    let c = b.app_background.unwrap_or(runtime_shared::color::Rgba {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    });
    COLORREF((c.r as u32) | ((c.g as u32) << 8) | ((c.b as u32) << 16))
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- transform composition ---

    #[test]
    fn identity_transform_produces_no_ops() {
        let anim = AnimTransform::default();
        assert!(transform_ops(100.0, 50.0, &[], &anim).is_empty());
    }

    #[test]
    fn animated_translate_is_origin_independent() {
        let anim = AnimTransform { tx: 10.0, ty: -4.0, ..Default::default() };
        assert_eq!(
            transform_ops(100.0, 50.0, &[], &anim),
            vec![XOp::Translate(10.0, -4.0)]
        );
    }

    #[test]
    fn animated_scale_wraps_about_center() {
        let anim = AnimTransform { scale: 2.0, ..Default::default() };
        assert_eq!(
            transform_ops(100.0, 50.0, &[], &anim),
            vec![
                XOp::Translate(50.0, 25.0),
                XOp::Scale(2.0, 2.0),
                XOp::Translate(-50.0, -25.0),
            ]
        );
    }

    /// The sun-glare wrapper's corner anchor: `translate(50%, -50%)`
    /// must resolve percentages against the node's own box.
    #[test]
    fn author_percent_translate_resolves_against_own_box() {
        let author = vec![
            Transform::TranslateX(Length::Percent(50.0)),
            Transform::TranslateY(Length::Percent(-50.0)),
        ];
        let anim = AnimTransform::default();
        let ops = transform_ops(200.0, 100.0, &author, &anim);
        assert_eq!(
            ops,
            vec![
                XOp::Translate(100.0, 50.0),
                XOp::Translate(100.0, 0.0),
                XOp::Translate(0.0, -50.0),
                XOp::Translate(-100.0, -50.0),
            ]
        );
    }

    // --- z ordering ---

    /// Planets z-flip between 0 and 1; equal-z siblings must keep
    /// insertion (document) order — that tie-break is what puts the
    /// content layer above the back-half planets.
    #[test]
    fn z_sort_is_stable_on_ties() {
        let pairs = [(1, 0.0), (2, 1.0), (3, 0.0), (4, 0.0)];
        assert_eq!(z_sorted(&pairs), vec![1, 3, 4, 2]);
    }

    // --- alpha helpers ---

    #[test]
    fn argb_from_srgba_packs_and_multiplies() {
        assert_eq!(argb_from_srgba([1.0, 0.0, 0.0, 1.0], 1.0), 0xFFFF_0000);
        assert_eq!(argb_from_srgba([0.0, 1.0, 0.0, 1.0], 0.5), 0x8000_FF00);
        assert_eq!(argb_from_srgba([0.0, 0.0, 1.0, 0.5], 0.5), 0x4000_00FF);
    }

    #[test]
    fn argb_with_alpha_scales_only_the_alpha_byte() {
        assert_eq!(argb_with_alpha(0xFF11_2233, 0.5), 0x8011_2233);
        assert_eq!(argb_with_alpha(0x8011_2233, 1.0), 0x8011_2233);
    }

    // --- preset-blend arrays ---

    #[test]
    fn blend_pads_missing_endpoints() {
        let stops = [(0.3, [1.0, 0.0, 0.0, 1.0]), (0.7, [0.0, 0.0, 1.0, 1.0])];
        let (colors, positions) = blend_arrays(&stops, 1.0, false);
        assert_eq!(positions.first().copied(), Some(0.0));
        assert_eq!(positions.last().copied(), Some(1.0));
        assert_eq!(colors, vec![0xFFFF_0000, 0xFFFF_0000, 0xFF00_00FF, 0xFF00_00FF]);
    }

    #[test]
    fn blend_reverse_flips_positions_for_path_gradient() {
        // center→edge stops [(0,inner),(1,outer)] become boundary→center
        // [(0,outer),(1,inner)] for the path gradient.
        let stops = [(0.0, [1.0, 1.0, 1.0, 1.0]), (1.0, [0.0, 0.0, 0.0, 1.0])];
        let (colors, positions) = blend_arrays(&stops, 1.0, true);
        assert_eq!(positions, vec![0.0, 1.0]);
        assert_eq!(colors, vec![0xFF00_0000, 0xFFFF_FFFF]);
    }

    #[test]
    fn blend_multiplies_cumulative_alpha_into_stops() {
        let stops = [(0.0, [1.0, 1.0, 1.0, 1.0]), (1.0, [1.0, 1.0, 1.0, 0.5])];
        let (colors, _) = blend_arrays(&stops, 0.5, false);
        assert_eq!(colors[0] >> 24, 0x80);
        assert_eq!(colors[1] >> 24, 0x40);
    }

    // --- scroll clamp + clipped hit testing (backend-fixture tests) ---

    use crate::{NodeMeta, ScrollInfo, ViewVisual};
    use windows::Win32::Foundation::HWND;

    fn put(b: &mut WindowsBackend, id: u64, kind: NodeKind, frame: (f32, f32, f32, f32)) {
        let mut m = NodeMeta::new(kind);
        m.frame = frame;
        b.nodes.insert(id, m);
    }

    /// Root(800×600) → scroll box at (0,100) 200×200 → VIEWPORT-SIZED
    /// wrapper (200×200 — the shape Taffy actually produces for the
    /// website sidebar) whose child column overflows to content-y 400
    /// via a pressable at (0,350) 200×50 (visually y 450, below the
    /// scroll box, until scrolled). The wrapper being no taller than
    /// the viewport is the load-bearing part: a direct-children-only
    /// extent reads "nothing to scroll" here and clamps every wheel
    /// tick to zero.
    fn scroll_fixture() -> WindowsBackend {
        let mut b = WindowsBackend::new(HWND(std::ptr::null_mut()));
        put(&mut b, 1, NodeKind::View(ViewVisual::default()), (0.0, 0.0, 800.0, 600.0));
        put(
            &mut b,
            2,
            NodeKind::View(ViewVisual {
                scroll: Some(ScrollInfo {
                    offset_x: 0.0,
                    offset_y: 0.0,
                    horizontal: false,
                    on_scroll: None,
                }),
                ..Default::default()
            }),
            (0.0, 100.0, 200.0, 200.0),
        );
        put(&mut b, 3, NodeKind::View(ViewVisual::default()), (0.0, 0.0, 200.0, 200.0));
        put(
            &mut b,
            4,
            NodeKind::View(ViewVisual { on_click: Some(Rc::new(|| {})), ..Default::default() }),
            (0.0, 350.0, 200.0, 50.0),
        );
        b.root_id = Some(1);
        b.children.insert(1, vec![2]);
        b.children.insert(2, vec![3]);
        b.children.insert(3, vec![4]);
        b
    }

    fn offset_y(b: &WindowsBackend) -> f32 {
        match &b.nodes[&2].kind {
            NodeKind::View(v) => v.scroll.as_ref().unwrap().offset_y,
            _ => unreachable!(),
        }
    }

    /// The wheel scrolled forever: only a `.max(0.0)` floor existed, no
    /// content-extent ceiling, so the page kept moving past its end.
    /// And the first ceiling attempt over-corrected to zero on the
    /// wrapper-shaped tree (see `scroll_fixture`) — both failure modes
    /// pin here.
    #[test]
    fn regression_wheel_scroll_clamps_to_content_extent() {
        let mut b = scroll_fixture();
        assert!(scroll_at(&mut b, 50.0, 150.0, 120.0), "wheel over the box is consumed");
        assert_eq!(offset_y(&b), 120.0, "a scrollable box must actually scroll");
        for _ in 0..10 {
            scroll_at(&mut b, 50.0, 150.0, 120.0);
        }
        // deep content extent 400 − viewport 200 = 200.
        assert_eq!(offset_y(&b), 200.0, "must stop at content − viewport");
        for _ in 0..10 {
            scroll_at(&mut b, 50.0, 150.0, -120.0);
        }
        assert_eq!(offset_y(&b), 0.0, "and at zero on the way back");
    }

    /// Native child HWNDs (wgpu canvas, controls) are positioned by
    /// `SetWindowPos`, not the painter's scroll translate — a scroll
    /// offset change must re-run the origin walk or they stay pinned
    /// while painted content scrolls past (the "canvas doesn't move
    /// with the scroll" bug). The walk's written-back `abs` origins
    /// are the observable the test pins.
    #[test]
    fn regression_scroll_offset_moves_native_child_origins() {
        let mut b = scroll_fixture();
        b.position_native_children();
        assert_eq!(b.nodes[&3].abs, (0.0, 100.0), "content starts at the box top");
        if let NodeKind::View(v) = &mut b.nodes.get_mut(&2).unwrap().kind {
            v.scroll.as_mut().unwrap().offset_y = 50.0;
        }
        b.position_native_children();
        assert_eq!(b.nodes[&3].abs, (0.0, 50.0), "children ride the offset");
        assert_eq!(b.nodes[&4].abs, (0.0, 400.0), "grandchildren too");
    }

    /// Unclipped scroll content wasn't just painted outside the box —
    /// it took clicks there too.
    #[test]
    fn regression_scrolled_out_content_is_not_clickable() {
        let mut b = scroll_fixture();
        // Pressable at content y 350 → visual y 450, below the box
        // (which ends at 300).
        assert!(
            pressable_at(&b, 50.0, 450.0).is_none(),
            "content outside the scroll box must not take clicks"
        );
        // Scrolled to the end (offset 200) it sits at visual y 250..300
        // — clickable.
        if let NodeKind::View(v) = &mut b.nodes.get_mut(&2).unwrap().kind {
            v.scroll.as_mut().unwrap().offset_y = 200.0;
        }
        assert!(pressable_at(&b, 50.0, 275.0).is_some(), "scrolled-in content hits");
    }
}

/// Border painting through REAL GDI+ calls into an offscreen bitmap.
///
/// These need `gdiplus.dll`, so they run only where the Windows test
/// binary runs (the crate is gated on `target_os = "windows"`).
#[cfg(test)]
mod border_render_tests {
    use super::*;
    use runtime_shared::color::Rgba;
    use runtime_shared::BorderStyle;
    use windows::Win32::Graphics::GdiPlus::{
        GdipBitmapGetPixel, GdipCreateBitmapFromScan0, GdipDisposeImage,
        GdipGetImageGraphicsContext, GpBitmap, GpImage,
    };

    /// `PixelFormat32bppARGB` from `gdipluspixelformats.h` — a `#define`,
    /// so the `windows` crate carries no constant for it.
    const PIXEL_FORMAT_32BPP_ARGB: i32 = 0x0026_200A;

    /// Paint `v`'s border on a transparent `w × h` bitmap (anti-aliased,
    /// as the scene paints) and return each pixel's alpha, row-major.
    fn render(v: &ViewVisual, w: i32, h: i32) -> Vec<u8> {
        crate::ensure_gdiplus();
        unsafe {
            let mut bmp: *mut GpBitmap = std::ptr::null_mut();
            assert_eq!(
                GdipCreateBitmapFromScan0(w, h, 0, PIXEL_FORMAT_32BPP_ARGB, None, &mut bmp).0,
                0
            );
            let mut g: *mut GpGraphics = std::ptr::null_mut();
            assert_eq!(GdipGetImageGraphicsContext(bmp as *mut GpImage, &mut g).0, 0);
            let _ = GdipGraphicsClear(g, 0);
            let _ = GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
            paint_border(g, v, w as f32, h as f32, 1.0);
            let _ = GdipDeleteGraphics(g);
            let mut out = Vec::with_capacity((w * h) as usize);
            for y in 0..h {
                for x in 0..w {
                    let mut argb = 0u32;
                    let _ = GdipBitmapGetPixel(bmp, x, y, &mut argb);
                    out.push((argb >> 24) as u8);
                }
            }
            let _ = GdipDisposeImage(bmp as *mut GpImage);
            out
        }
    }

    fn bordered(width: f32, style: BorderStyle) -> ViewVisual {
        let side = BorderSide { width, color: Some(Rgba::new(255, 0, 0, 255)) };
        ViewVisual { borders: [side; 4], border_style: style, ..Default::default() }
    }

    fn alpha(px: &[u8], w: i32, x: i32, y: i32) -> u8 {
        px[(y * w + x) as usize]
    }

    #[test]
    fn dashed_border_leaves_gaps_where_solid_is_inked() {
        // 60×30, 2px: centreline loop 2·(58 + 28) = 172 → 14 fitted dashes
        // (period 172/14 ≈ 12.29). The first is centred on (1, 1), so the
        // top edge is inked on x ∈ [0, 4] and [10.3, 16.3] and clear
        // between.
        let (w, h) = (60, 30);
        let dashed = render(&bordered(2.0, BorderStyle::Dashed), w, h);
        assert!(alpha(&dashed, w, 13, 0) > 200, "inside the second dash");
        assert!(alpha(&dashed, w, 2, 0) > 200, "inside the first dash");
        assert!(alpha(&dashed, w, 7, 0) < 30, "the gap between them");
        assert!(alpha(&dashed, w, 30, 15) < 30, "the interior stays clear");

        let solid = render(&bordered(2.0, BorderStyle::Solid), w, h);
        assert!(alpha(&solid, w, 7, 0) > 200, "solid inks the same pixel");
    }

    #[test]
    fn dotted_border_draws_round_dots_one_width_across() {
        // 60×40, 4px: loop 2·(56 + 36) = 184, dot period 8 → 23 dots,
        // centred on the top edge's centreline (y = 2) at x = 2, 10, 18…
        let (w, h) = (60, 40);
        let px = render(&bordered(4.0, BorderStyle::Dotted), w, h);
        assert!(alpha(&px, w, 9, 2) > 200 && alpha(&px, w, 10, 1) > 200, "dot at x = 10");
        assert!(alpha(&px, w, 5, 2) < 30 && alpha(&px, w, 6, 2) < 30, "gap between dots");
        // Round, not square: the dot's bounding-box corner is outside the
        // circle.
        assert!(alpha(&px, w, 8, 0) < 128, "dot corner is cut: {}", alpha(&px, w, 8, 0));
    }
}

#[cfg(test)]
mod gradient_render_tests {
    //! The raster linear path against the GDI+ line brush it replaces,
    //! pixel for pixel, on real GDI+ (Windows, or Wine off Windows). The
    //! raster exists only for speed (`gradient_raster`), so any visible
    //! difference from the brush is a regression.
    use super::*;
    use crate::{GradKind, GradientPaint};
    use windows::Win32::Graphics::GdiPlus::{
        GdipBitmapGetPixel, GdipCreateBitmapFromScan0, GdipDisposeImage,
        GdipGetImageGraphicsContext, GdipTranslateWorldTransform, GpBitmap, GpImage,
    };

    const PIXEL_FORMAT_32BPP_ARGB: i32 = 0x0026_200A;
    const SURFACE: (i32, i32) = (160, 120);

    /// One linear gradient on a transparent surface, positioned and scaled
    /// the way `paint_scene` + `paint_node` would, through either path.
    /// Returns `[a, r, g, b]` per pixel (straight).
    fn render(gp: &GradientPaint, at: (f32, f32), size: (f32, f32), scale: f32, radii: [f32; 4], raster: bool) -> Vec<[u8; 4]> {
        crate::ensure_gdiplus();
        let (sw, sh) = SURFACE;
        unsafe {
            let mut bmp: *mut GpBitmap = std::ptr::null_mut();
            assert_eq!(GdipCreateBitmapFromScan0(sw, sh, 0, PIXEL_FORMAT_32BPP_ARGB, None, &mut bmp).0, 0);
            let mut g: *mut GpGraphics = std::ptr::null_mut();
            assert_eq!(GdipGetImageGraphicsContext(bmp as *mut GpImage, &mut g).0, 0);
            let _ = GdipGraphicsClear(g, 0);
            let _ = GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
            let _ = GdipScaleWorldTransform(g, scale, scale, MatrixOrderPrepend);
            let _ = GdipTranslateWorldTransform(g, at.0, at.1, MatrixOrderPrepend);
            let (w, h) = size;
            let GradKind::Linear { angle_deg } = gp.kind else { panic!("linear only") };
            let (s, e) = crate::linear_points(angle_deg, w, h);
            if raster {
                assert!(paint_linear_raster(g, gp, s, e, w, h, radii, 1.0), "fast path not taken");
            } else {
                let mut path: *mut GpPath = std::ptr::null_mut();
                assert_eq!(GdipCreatePath(FillModeWinding, &mut path).0, 0);
                build_round_rect_path(path, w, h, radii, 0.0);
                paint_linear_brush(g, gp, s, e, path, 1.0);
                let _ = GdipDeletePath(path);
            }
            let _ = GdipDeleteGraphics(g);
            let mut out = Vec::with_capacity((sw * sh) as usize);
            for y in 0..sh {
                for x in 0..sw {
                    let mut argb = 0u32;
                    let _ = GdipBitmapGetPixel(bmp, x, y, &mut argb);
                    out.push(argb.to_be_bytes());
                }
            }
            let _ = GdipDisposeImage(bmp as *mut GpImage);
            out
        }
    }

    /// The paths agree: away from the shape's edge, alpha within 2 and
    /// straight color within rounding; overall coverage within half a
    /// pixel of the perimeter.
    ///
    /// Edge pixels are only compared in aggregate: the raster's are
    /// area-coverage anti-aliased (what the scene's `SmoothingModeAntiAlias`
    /// asks for), but Wine's GDI+ fills paths ALIASED — a pixel is in or out
    /// by its centre — so per-pixel edge values legitimately differ there.
    fn assert_agree(name: &str, gp: &GradientPaint, at: (f32, f32), size: (f32, f32), scale: f32, radii: [f32; 4]) {
        let brush = render(gp, at, size, scale, radii, false);
        let fast = render(gp, at, size, scale, radii, true);
        let (sw, sh) = SURFACE;
        let alpha_at = |v: &Vec<[u8; 4]>, x: i32, y: i32| -> Option<u8> {
            (x >= 0 && y >= 0 && x < sw && y < sh).then(|| v[(y * sw + x) as usize][0])
        };
        let (mut sum_b, mut sum_f, mut interior) = (0f64, 0f64, 0usize);
        for y in 0..sh {
            for x in 0..sw {
                let (b, f) = (brush[(y * sw + x) as usize], fast[(y * sw + x) as usize]);
                sum_b += b[0] as f64;
                sum_f += f[0] as f64;
                let a = b[0];
                let edge = f[0] != a && (f[0] != 0 && f[0] != 255 || a == 0)
                    || [(1, 0), (-1, 0), (0, 1), (0, -1)]
                        .iter()
                        .any(|(dx, dy)| alpha_at(&brush, x + dx, y + dy).map_or(true, |n| n == 0) && a != 0);
                if edge || a == 0 {
                    continue;
                }
                interior += 1;
                let da = (a as i32 - f[0] as i32).abs();
                assert!(da <= 2, "{name}: alpha at ({x}, {y}) brush {b:?} raster {f:?}");
                // Straight color from 8-bit premultiplied storage loses
                // precision as alpha drops. Wine's brush also samples its
                // ramp up to a pixel off the CSS geometry the raster follows
                // exactly, so the raster must match the brush at this pixel
                // or a 4-neighbour.
                let tol = 3 + 255 / a.max(1) as i32;
                let near = |q: [u8; 4]| (1..4).all(|k| (q[k] as i32 - f[k] as i32).abs() <= tol);
                let ok = near(b)
                    || [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dy)| {
                        let (nx, ny) = (x + dx, y + dy);
                        nx >= 0 && ny >= 0 && nx < sw && ny < sh && near(brush[(ny * sw + nx) as usize])
                    });
                assert!(ok, "{name}: color at ({x}, {y}) brush {b:?} raster {f:?}");
            }
        }
        assert!(interior > 100, "{name}: only {interior} interior pixels compared");
        // Aliased vs anti-aliased edges differ by at most half a pixel
        // along the perimeter; a shape or grid error is far larger.
        let perimeter = 2.0 * (size.0 + size.1) as f64 * scale as f64;
        let bound = 0.5 * 255.0 * perimeter;
        assert!((sum_b - sum_f).abs() <= bound, "{name}: coverage {sum_b} vs {sum_f} (bound {bound})");
    }

    fn paint(angle_deg: f32, stops: &[(f32, [f32; 4])]) -> GradientPaint {
        GradientPaint { kind: GradKind::Linear { angle_deg }, stops: stops.to_vec() }
    }

    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
    const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
    const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];

    #[test]
    fn welcome_vignette_raster_matches_the_line_brush() {
        // The welcome vignette band that cost ~21 ms per frame through the
        // brush under Wine: warm color at the edge fading to transparent,
        // at 2× DPI.
        let warm = [1.0, 0.66, 0.24, 0.7];
        let clear = [1.0, 0.66, 0.24, 0.0];
        assert_agree("vignette", &paint(270.0, &[(0.0, clear), (1.0, warm)]), (0.0, 0.0), (40.0, 60.0), 2.0, [0.0; 4]);
    }

    /// Render one radial gradient through `paint_gradient` on a `w × h`
    /// transparent surface; `[a, r, g, b]` per pixel.
    fn render_radial(gp: &GradientPaint, w: i32, h: i32) -> Vec<[u8; 4]> {
        crate::ensure_gdiplus();
        unsafe {
            let mut bmp: *mut GpBitmap = std::ptr::null_mut();
            assert_eq!(GdipCreateBitmapFromScan0(w, h, 0, PIXEL_FORMAT_32BPP_ARGB, None, &mut bmp).0, 0);
            let mut g: *mut GpGraphics = std::ptr::null_mut();
            assert_eq!(GdipGetImageGraphicsContext(bmp as *mut GpImage, &mut g).0, 0);
            let _ = GdipGraphicsClear(g, 0);
            let _ = GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
            let mut path: *mut GpPath = std::ptr::null_mut();
            assert_eq!(GdipCreatePath(FillModeWinding, &mut path).0, 0);
            build_round_rect_path(path, w as f32, h as f32, [0.0; 4], 0.0);
            paint_gradient(g, gp, path, w as f32, h as f32, [0.0; 4], 1.0);
            let _ = GdipDeletePath(path);
            let _ = GdipDeleteGraphics(g);
            let mut out = Vec::with_capacity((w * h) as usize);
            for y in 0..h {
                for x in 0..w {
                    let mut argb = 0u32;
                    let _ = GdipBitmapGetPixel(bmp, x, y, &mut argb);
                    out.push(argb.to_be_bytes());
                }
            }
            let _ = GdipDisposeImage(bmp as *mut GpImage);
            out
        }
    }

    #[test]
    fn regression_radial_gradient_paints_its_last_stop_beyond_the_circle() {
        // `closest-side` on 100×60: the circle (r 30) leaves the box's
        // corners and sides outside it. CSS paints them the last stop; a
        // path-gradient brush painted nothing there.
        let (w, h) = (100, 60);
        let half_blue = [0.0, 0.0, 1.0, 0.5];
        let gp = GradientPaint {
            kind: GradKind::Radial { center: (0.5, 0.5), radius: 1.0, farthest: false },
            stops: vec![(0.0, [1.0, 1.0, 1.0, 1.0]), (1.0, half_blue)],
        };
        let px = render_radial(&gp, w, h);
        let at = |x: i32, y: i32| px[(y * w + x) as usize];
        for (x, y) in [(1, 1), (98, 1), (1, 58), (98, 58), (5, 30)] {
            let p = at(x, y);
            assert!((p[0] as i32 - 128).abs() <= 2 && p[3] > 240, "beyond the circle at ({x}, {y}): {p:?}");
        }
        // Inside, the gradient itself — and no second outer-color layer
        // under it: the centre stays the opaque white first stop.
        let c = at(50, 30);
        assert!(c[0] > 250 && c[1] > 240 && c[2] > 240, "centre {c:?}");
        // Just inside the circle the translucent outer stop is not doubled.
        let rim = at(50, 3);
        assert!(rim[0] < 200, "rim alpha {} (doubling would push it toward opaque)", rim[0]);
    }

    #[test]
    fn regression_square_corner_among_rounded_ones_is_not_cut() {
        // radii [16, 4, 30, 0]: the square bottom-left corner was a
        // zero-size arc, which GDI+ rejects — the figure then ran straight
        // from the bottom-right arc to the top-left one, cutting the box
        // diagonally.
        crate::ensure_gdiplus();
        let (sw, sh) = SURFACE;
        unsafe {
            let mut bmp: *mut GpBitmap = std::ptr::null_mut();
            assert_eq!(GdipCreateBitmapFromScan0(sw, sh, 0, PIXEL_FORMAT_32BPP_ARGB, None, &mut bmp).0, 0);
            let mut g: *mut GpGraphics = std::ptr::null_mut();
            assert_eq!(GdipGetImageGraphicsContext(bmp as *mut GpImage, &mut g).0, 0);
            let _ = GdipGraphicsClear(g, 0);
            let mut path: *mut GpPath = std::ptr::null_mut();
            assert_eq!(GdipCreatePath(FillModeWinding, &mut path).0, 0);
            build_round_rect_path(path, 100.0, 80.0, [16.0, 4.0, 30.0, 0.0], 0.0);
            let mut brush: *mut GpSolidFill = std::ptr::null_mut();
            let _ = GdipCreateSolidFill(0xFF00_0000, &mut brush);
            let _ = GdipFillPath(g, brush as *mut GpBrush, path);
            let _ = GdipDeleteBrush(brush as *mut GpBrush);
            let _ = GdipDeletePath(path);
            let _ = GdipDeleteGraphics(g);
            let at = |x, y| {
                let mut v = 0u32;
                let _ = GdipBitmapGetPixel(bmp, x, y, &mut v);
                v >> 24
            };
            // Inside the square corner, and the middle of the diagonal the
            // broken figure cut away.
            assert_eq!(at(2, 77), 255, "bottom-left corner");
            assert_eq!(at(20, 60), 255, "below the old diagonal");
            // The rounded corners are still rounded.
            assert_eq!(at(0, 0), 0, "top-left arc");
            assert_eq!(at(99, 79), 0, "bottom-right arc");
            let _ = GdipDisposeImage(bmp as *mut GpImage);
        }
    }

    #[test]
    fn regression_line_brush_far_edge_has_no_start_color_seam() {
        // 0deg = to top: the top row is t = 1.0. With `WrapModeTile` that
        // row wrapped to the START color (red) — a 1-px seam the welcome
        // vignettes showed at the screen edge.
        let stops = [(0.0, RED), (1.0, BLUE)];
        let px = render(&paint(0.0, &stops), (8.0, 6.0), (120.0, 90.0), 1.0, [0.0; 4], false);
        let top = px[(6 * SURFACE.0 + 60) as usize];
        assert!(top[3] > 200 && top[1] < 40, "top edge should be blue, got {top:?}");
    }

    #[test]
    fn raster_matches_the_brush_at_every_axis_angle_and_multi_stop() {
        let stops = [(0.0, RED), (0.4, GREEN), (1.0, BLUE)];
        for angle in [0.0, 90.0, 180.0, 270.0, 45.0, 120.0] {
            assert_agree(&format!("{angle}deg"), &paint(angle, &stops), (8.0, 6.0), (120.0, 90.0), 1.0, [0.0; 4]);
        }
    }

    #[test]
    fn raster_matches_the_brush_with_fractional_position_and_rounded_corners() {
        let stops = [(0.0, RED), (1.0, BLUE)];
        assert_agree("rounded", &paint(90.0, &stops), (10.25, 7.5), (100.0, 80.0), 1.0, [16.0, 4.0, 30.0, 0.0]);
        assert_agree("rounded 2x", &paint(0.0, &stops), (3.5, 2.25), (60.0, 40.0), 2.0, [12.0; 4]);
    }
}
