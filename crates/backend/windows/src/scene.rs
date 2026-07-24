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
//! ## Hit testing
//!
//! With no per-view HWNDs, clicks land on the host window. [`pressable_at`]
//! walks the tree top-most-first (reverse paint order) applying the same
//! frame + translate offsets and returns the deepest pressable containing
//! the point. Scale/rotate are ignored in hit tests — documented
//! approximation; nothing interactive animates transforms today.

use std::rc::Rc;

use runtime_core::{Length, Transform};

use windows::core::PCWSTR;
use windows::Win32::Foundation::COLORREF;
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, SelectObject,
    HBITMAP, HDC, HGDIOBJ, SRCCOPY,
};
use windows::Win32::Graphics::GdiPlus::{
    CombineModeIntersect, FillModeWinding, GdipAddPathArc, GdipAddPathRectangle,
    GdipClosePathFigure, GdipCreateFromHDC, GdipCreatePath, GdipCreatePen1, GdipCreateSolidFill,
    GdipDeleteBrush, GdipDeleteGraphics, GdipDeletePath, GdipDeletePen, GdipDrawPath,
    GdipDrawString, GdipFillPath, GdipGraphicsClear, GdipRestoreGraphics, GdipRotateWorldTransform,
    GdipSaveGraphics, GdipScaleWorldTransform, GdipSetClipPath, GdipSetSmoothingMode,
    GdipSetTextRenderingHint, GdipStringFormatGetGenericTypographic, GdipTranslateWorldTransform,
    GpBrush, GpGraphics, GpPath, GpPen, GpSolidFill, GpStringFormat, MatrixOrderPrepend, RectF,
    SmoothingModeAntiAlias, TextRenderingHintAntiAlias, UnitPixel,
};

use crate::font;
use crate::{
    icon, image, uniform_border, AnimTransform, BorderSide, GradKind, GradientPaint, NodeKind,
    WindowsBackend,
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

/// One resolved world-transform op, in application order.
#[derive(Debug, PartialEq)]
pub(crate) enum XOp {
    Translate(f32, f32),
    Scale(f32, f32),
    Rotate(f32),
}

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
    let _ = GdipAddPathArc(path, left, top, 2.0 * tl, 2.0 * tl, 180.0, 90.0);
    let _ = GdipAddPathArc(path, right - 2.0 * tr, top, 2.0 * tr, 2.0 * tr, 270.0, 90.0);
    let _ = GdipAddPathArc(path, right - 2.0 * br, bottom - 2.0 * br, 2.0 * br, 2.0 * br, 0.0, 90.0);
    let _ = GdipAddPathArc(path, left, bottom - 2.0 * bl, 2.0 * bl, 2.0 * bl, 90.0, 90.0);
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
unsafe fn paint_gradient(
    g: *mut GpGraphics,
    gp: &GradientPaint,
    fill_path: *mut GpPath,
    w: f32,
    h: f32,
    alpha: f32,
) {
    use windows::Win32::Graphics::GdiPlus::{
        GdipAddPathEllipse, GdipCreateLineBrush, GdipCreatePathGradientFromPath,
        GdipSetLinePresetBlend, GdipSetPathGradientCenterColor, GdipSetPathGradientCenterPoint,
        GdipSetPathGradientPresetBlend, GdipSetPathGradientSurroundColorsWithCount,
        GdipSetPathGradientWrapMode, GpLineGradient, GpPathGradient, PointF, WrapModeClamp,
        WrapModeTile,
    };

    match gp.kind {
        GradKind::Linear { angle_deg } => {
            let (s, e) = crate::linear_points(angle_deg, w, h);
            let p1 = PointF { X: s.0, Y: s.1 };
            let p2 = PointF { X: e.0, Y: e.1 };
            let first = gp.stops.first().map(|s| argb_from_srgba(s.1, alpha)).unwrap_or(0);
            let last = gp.stops.last().map(|s| argb_from_srgba(s.1, alpha)).unwrap_or(first);
            let mut brush: *mut GpLineGradient = std::ptr::null_mut();
            // The axis already spans the box's full projected extent (see
            // `linear_points`), so no pixel projects outside [0, 1] and
            // the wrap mode is never exercised — `WrapModeTile` is just
            // the required non-null argument.
            if GdipCreateLineBrush(&p1, &p2, first, last, WrapModeTile, &mut brush).0 == 0
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
/// `w × h`. Renders into the retained back buffer, then blits — one
/// flicker-free flip.
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

        if let Some(root) = b.root_id {
            paint_node(b, g, root, 1.0);
        }
        let _ = GdipDeleteGraphics(g);
    }

    let _ = BitBlt(target, 0, 0, w, h, back.dc, 0, 0, SRCCOPY);
    b.back = back;
}

/// Paint one node + its subtree. `alpha` is the cumulative ancestor
/// opacity.
unsafe fn paint_node(b: &WindowsBackend, g: *mut GpGraphics, id: u64, alpha: f32) {
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

    // Own visual.
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
                        paint_gradient(g, gr, fill_path, w, h, alpha);
                    }
                    let _ = GdipDeletePath(fill_path);
                }
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
                            let wide: Vec<u16> = t.content.encode_utf16().collect();
                            // A zero-size layout rect = draw from the
                            // origin, no wrap, no clip (GDI+ contract).
                            // The node's box is exactly the GDI-measured
                            // string, so origin-anchored drawing lines up.
                            let layout = RectF { X: 0.0, Y: 0.0, Width: 0.0, Height: 0.0 };
                            let _ = GdipDrawString(
                                g,
                                PCWSTR(wide.as_ptr()),
                                wide.len() as i32,
                                entry.gpfont,
                                &layout,
                                typographic_format(),
                                brush as *mut GpBrush,
                            );
                            let _ = GdipDeleteBrush(brush as *mut GpBrush);
                        }
                    }
                }
            }
        }
        NodeKind::Icon(p) => icon::paint_into(g, p, w, h, alpha),
        NodeKind::Image(p) => image::paint_into(g, p, w, h),
        // Native controls paint themselves in their own HWNDs (the
        // host's WS_CLIPCHILDREN keeps our blit off them).
        NodeKind::Control { .. } | NodeKind::External { .. } => {}
    }

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

    if meta.overflow_hidden {
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
        paint_node(b, g, cid, alpha);
    }
    let _ = GdipRestoreGraphics(g, state);
}

// =========================================================================
// Hit testing
// =========================================================================

/// The deepest pressable under client-space `(px, py)`, honoring z order
/// (topmost first) and animated/author translations. Scale/rotate are
/// ignored — nothing interactive animates transforms today.
pub(crate) fn pressable_at(b: &WindowsBackend, px: f32, py: f32) -> Option<Rc<dyn Fn()>> {
    let root = b.root_id?;
    hit_node(b, root, px, py, 0.0, 0.0)
}

fn node_origin(b: &WindowsBackend, id: u64, ox: f32, oy: f32) -> Option<(f32, f32, f32, f32)> {
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

fn hit_node(b: &WindowsBackend, id: u64, px: f32, py: f32, ox: f32, oy: f32) -> Option<Rc<dyn Fn()>> {
    let (nx, ny, w, h) = node_origin(b, id, ox, oy)?;
    let meta = b.nodes.get(&id)?;
    if meta.hidden {
        return None;
    }
    if meta.effective_opacity() <= 0.003 {
        // Invisible subtrees don't take clicks (matches web/Apple:
        // opacity 0 still hits, but our pre-act layers overlap the
        // content layer full-screen — treat as transparent to input so
        // the visible content underneath stays clickable).
        return None;
    }
    // Children first, topmost (highest z, later insertion) first.
    let (mut cx, mut cy) = (nx, ny);
    if let NodeKind::View(v) = &meta.kind {
        if let Some(s) = &v.scroll {
            cx -= s.offset_x;
            cy -= s.offset_y;
        }
    }
    if let Some(child_ids) = b.children.get(&id) {
        let pairs: Vec<(u64, f32)> = child_ids
            .iter()
            .filter_map(|cid| b.nodes.get(cid).map(|m| (*cid, m.z)))
            .collect();
        for cid in z_sorted(&pairs).into_iter().rev() {
            if let Some(f) = hit_node(b, cid, px, py, cx, cy) {
                return Some(f);
            }
        }
    }
    if let NodeKind::View(v) = &meta.kind {
        if let Some(cb) = &v.on_click {
            if px >= nx && px < nx + w && py >= ny && py < ny + h {
                return Some(cb.clone());
            }
        }
    }
    None
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
    let mut cb: Option<(Rc<dyn Fn(f32, f32)>, f32, f32)> = None;
    if let Some(meta) = b.nodes.get_mut(&target) {
        if let NodeKind::View(v) = &mut meta.kind {
            if let Some(s) = &mut v.scroll {
                if s.horizontal {
                    s.offset_x = (s.offset_x + delta_px).max(0.0);
                } else {
                    s.offset_y = (s.offset_y + delta_px).max(0.0);
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

fn find_scroll_node(b: &WindowsBackend, id: u64, px: f32, py: f32, ox: f32, oy: f32) -> Option<u64> {
    let (nx, ny, w, h) = node_origin(b, id, ox, oy)?;
    let meta = b.nodes.get(&id)?;
    if meta.hidden {
        return None;
    }
    let inside = px >= nx && px < nx + w && py >= ny && py < ny + h;
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
    let c = b.app_background.unwrap_or(runtime_core::color::Rgba {
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
}
