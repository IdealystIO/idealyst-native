//! Shared CoreGraphics painter for the Apple canvas renderers (iOS +
//! macOS).
//!
//! Walks a canvas [`Scene`](canvas_core::Scene) and replays its
//! [`DrawOp`]s into a `CGContext`. The op-replay is pure CoreGraphics
//! (CGContext C functions + `UIBezierPath`/`NSBezierPath` path
//! construction) and is therefore IDENTICAL across iOS (`UIKit`) and
//! macOS (`AppKit`) — only the *context acquisition* differs
//! (`UIGraphicsGetCurrentContext()` vs
//! `NSGraphicsContext.currentContext.CGContext`), which lives in each
//! platform's view subclass.
//!
//! Canvas coordinates are logical points, top-left origin. The iOS
//! `drawRect:` CTM already matches; on macOS the view's `isFlipped`
//! returns `true`, giving the same top-left CTM — so NO axis flip is
//! applied here, on either platform.
//!
//! Bezier-path construction uses Obj-C dispatch against a configurable
//! class name (`UIBezierPath` on iOS, our `IdealystBezierShim` on
//! macOS — see [`macos`](crate::macos)). The selectors and color class
//! are passed in per platform so the op-replay stays toolkit-agnostic.

use canvas_core::{
    BlendMode, Color, DrawOp, FillRule, ImageSource, LineCap, LineJoin, Paint, PaintKind, Path,
};

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, NSObject};
use objc2::{msg_send, msg_send_id};
use objc2_foundation::{CGFloat, CGPoint, CGRect, CGSize};

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;

// ============================================================================
// CoreGraphics C bindings (system framework — linker resolves automatically)
// ============================================================================

/// CoreGraphics affine transform. Same `(a, b, c, d, tx, ty)` convention
/// as the framework's `Transform`: maps `(x, y)` to
/// `(a·x + c·y + tx, b·x + d·y + ty)`.
#[repr(C)]
#[derive(Copy, Clone)]
pub(crate) struct CGAffineTransform {
    pub(crate) a: CGFloat,
    pub(crate) b: CGFloat,
    pub(crate) c: CGFloat,
    pub(crate) d: CGFloat,
    pub(crate) tx: CGFloat,
    pub(crate) ty: CGFloat,
}

unsafe impl objc2::Encode for CGAffineTransform {
    const ENCODING: objc2::Encoding = objc2::Encoding::Struct(
        "CGAffineTransform",
        &[
            <CGFloat as objc2::Encode>::ENCODING,
            <CGFloat as objc2::Encode>::ENCODING,
            <CGFloat as objc2::Encode>::ENCODING,
            <CGFloat as objc2::Encode>::ENCODING,
            <CGFloat as objc2::Encode>::ENCODING,
            <CGFloat as objc2::Encode>::ENCODING,
        ],
    );
}

pub(crate) type CGContextRef = *mut c_void;
type CGGradientRef = *mut c_void;
type CGColorSpaceRef = *mut c_void;
type CGImageRef = *mut c_void;
type CGDataProviderRef = *mut c_void;
type CFDataRef = *mut c_void;

/// `kCGImageAlphaLast` (CGImage.h): straight (non-premultiplied) RGBA, alpha
/// in the last byte — the layout [`ImageSource`] guarantees.
const CG_IMAGE_ALPHA_LAST: u32 = 3;
/// `kCGImageAlphaPremultipliedLast`: the only RGBA8 layout a `CGBitmapContext`
/// supports as a render target (used for persistent layer surfaces).
const CG_IMAGE_ALPHA_PREMULTIPLIED_LAST: u32 = 1;
/// `kCGRenderingIntentDefault`.
const CG_RENDERING_INTENT_DEFAULT: i32 = 0;

/// Cover the whole clipped region (before-start + after-end), matching
/// Canvas2D gradient extension semantics.
const CG_GRADIENT_DRAWS_EXTEND: u32 = 1 | 2;

/// `CGBlendMode` raw values (CoreGraphics/CGContext.h). Only the modes
/// `BlendMode` exposes are named here; see [`cg_blend_mode`].
const CG_BLEND_NORMAL: i32 = 0;
const CG_BLEND_MULTIPLY: i32 = 1;
const CG_BLEND_SCREEN: i32 = 2;
const CG_BLEND_OVERLAY: i32 = 3;
const CG_BLEND_DARKEN: i32 = 4;
const CG_BLEND_LIGHTEN: i32 = 5;
const CG_BLEND_COLOR_DODGE: i32 = 6;
const CG_BLEND_COLOR_BURN: i32 = 7;
// NB: CoreGraphics orders SoftLight(8) BEFORE HardLight(9) — the reverse of
// peniko/W3C — so these values are deliberately not symmetric with the enum.
const CG_BLEND_SOFT_LIGHT: i32 = 8;
const CG_BLEND_HARD_LIGHT: i32 = 9;
const CG_BLEND_DIFFERENCE: i32 = 10;
const CG_BLEND_EXCLUSION: i32 = 11;
const CG_BLEND_HUE: i32 = 12;
const CG_BLEND_SATURATION: i32 = 13;
const CG_BLEND_COLOR: i32 = 14;
const CG_BLEND_LUMINOSITY: i32 = 15;
const CG_BLEND_DESTINATION_OUT: i32 = 23;

extern "C" {
    fn CGContextSaveGState(c: CGContextRef);
    fn CGContextRestoreGState(c: CGContextRef);
    fn CGContextSetBlendMode(c: CGContextRef, mode: i32);
    fn CGContextConcatCTM(c: CGContextRef, transform: CGAffineTransform);
    fn CGContextDrawLinearGradient(
        c: CGContextRef,
        gradient: CGGradientRef,
        start_point: CGPoint,
        end_point: CGPoint,
        options: u32,
    );
    fn CGContextDrawRadialGradient(
        c: CGContextRef,
        gradient: CGGradientRef,
        start_center: CGPoint,
        start_radius: CGFloat,
        end_center: CGPoint,
        end_radius: CGFloat,
        options: u32,
    );
    fn CGColorSpaceCreateDeviceRGB() -> CGColorSpaceRef;
    fn CGColorSpaceRelease(cs: CGColorSpaceRef);
    fn CGGradientCreateWithColorComponents(
        space: CGColorSpaceRef,
        components: *const CGFloat,
        locations: *const CGFloat,
        count: usize,
    ) -> CGGradientRef;
    fn CGGradientRelease(g: CGGradientRef);
    // Image blit (`DrawOp::Image`).
    fn CGContextDrawImage(c: CGContextRef, rect: CGRect, image: CGImageRef);
    fn CGContextTranslateCTM(c: CGContextRef, tx: CGFloat, ty: CGFloat);
    fn CGContextScaleCTM(c: CGContextRef, sx: CGFloat, sy: CGFloat);
    fn CGContextSetAlpha(c: CGContextRef, alpha: CGFloat);
    fn CGImageCreate(
        width: usize,
        height: usize,
        bits_per_component: usize,
        bits_per_pixel: usize,
        bytes_per_row: usize,
        space: CGColorSpaceRef,
        bitmap_info: u32,
        provider: CGDataProviderRef,
        decode: *const CGFloat,
        should_interpolate: bool,
        intent: i32,
    ) -> CGImageRef;
    fn CGImageRelease(image: CGImageRef);
    fn CGImageRetain(image: CGImageRef) -> CGImageRef;
    fn CGDataProviderCreateWithCFData(data: CFDataRef) -> CGDataProviderRef;
    fn CGDataProviderRelease(provider: CGDataProviderRef);
    // Persistent layer (`DrawOp::Layer`): an offscreen RGBA bitmap context.
    fn CGContextGetClipBoundingBox(c: CGContextRef) -> CGRect;
    fn CGContextGetCTM(c: CGContextRef) -> CGAffineTransform;
    fn CGContextClearRect(c: CGContextRef, rect: CGRect);
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: CGColorSpaceRef,
        bitmap_info: u32,
    ) -> CGContextRef;
    fn CGBitmapContextCreateImage(c: CGContextRef) -> CGImageRef;
    fn CGContextRelease(c: CGContextRef);
}

extern "C" {
    // CoreFoundation — `CFData` owns a *copy* of the pixels, so the cached
    // `CGImage` keeps its bytes alive after the per-frame `Scene` (and its
    // `Vec<u8>`) is dropped.
    fn CFDataCreate(allocator: *const c_void, bytes: *const u8, length: isize) -> CFDataRef;
    fn CFRelease(cf: *const c_void);
}

// ============================================================================
// Platform vtable
// ============================================================================

/// Per-platform glue the op-replay needs: which bezier class to build
/// paths with, and how to make a color object that responds to
/// `setFill` / `setStroke`. Everything else (the CGContext C calls) is
/// platform-identical.
///
/// iOS supplies `UIBezierPath` + `UIColor`; macOS supplies a small
/// `NSBezierPath` shim (which adds the `addLineToPoint:` /
/// `setLineCapStyle:` selectors iOS-style code expects) + `NSColor`.
pub(crate) struct ApplePainter {
    /// Class to instantiate per path via `+bezierPath` and build via the
    /// UIKit-style selectors (`moveToPoint:`, `addLineToPoint:`, …).
    pub(crate) bezier_class: &'static AnyClass,
    /// `fn(Color) -> id` producing an object responding to `setFill` /
    /// `setStroke` (a `UIColor` on iOS, `NSColor` on macOS).
    pub(crate) make_color: fn(Color) -> Retained<NSObject>,
}

impl ApplePainter {
    /// Replay every op of `scene` into the active `CGContext`.
    pub(crate) fn paint_scene(&self, ctx: CGContextRef, scene: &canvas_core::Scene) {
        for op in scene.ops() {
            self.apply_op(ctx, op);
        }
    }

    fn apply_op(&self, ctx: CGContextRef, op: &DrawOp) {
        match op {
            DrawOp::Save => unsafe { CGContextSaveGState(ctx) },
            DrawOp::Restore => unsafe { CGContextRestoreGState(ctx) },
            DrawOp::Transform(t) => {
                let m = CGAffineTransform {
                    a: t.a as CGFloat,
                    b: t.b as CGFloat,
                    c: t.c as CGFloat,
                    d: t.d as CGFloat,
                    tx: t.e as CGFloat,
                    ty: t.f as CGFloat,
                };
                unsafe { CGContextConcatCTM(ctx, m) };
            }
            DrawOp::Fill { path, paint, fill_rule } => {
                let bezier = self.build_path(path);
                if *fill_rule == FillRule::EvenOdd {
                    let _: () = unsafe { msg_send![&bezier, setUsesEvenOddFillRule: true] };
                }
                // Blend mode is CGContext gstate; set it for this op and
                // reset to Normal after so the next op isn't affected.
                set_blend(ctx, paint.blend);
                match &paint.kind {
                    PaintKind::Solid(c) => {
                        let col = (self.make_color)(*c);
                        let _: () = unsafe { msg_send![&col, setFill] };
                        let _: () = unsafe { msg_send![&bezier, fill] };
                    }
                    _ => self.fill_gradient(ctx, &bezier, paint),
                }
                reset_blend(ctx, paint.blend);
            }
            DrawOp::Stroke { path, paint, stroke } => {
                let bezier = self.build_path(path);
                let _: () = unsafe { msg_send![&bezier, setLineWidth: stroke.width as CGFloat] };
                // `setLineCapStyle:` / `setLineJoinStyle:` are the one place the
                // two toolkits' arg types genuinely diverge: UIKit's
                // `UIBezierPath` takes `CGLineCap`/`CGLineJoin` (`int32_t`,
                // encoding `'i'`), AppKit's `NSBezierPath` takes
                // `NSLineCapStyle`/`NSLineJoinStyle` (`NSUInteger`, `'Q'`). The
                // numeric values are identical (butt/miter=0, round=1,
                // square/bevel=2) — only the integer width/signedness differs —
                // so widen to `usize` on macOS. objc2's msg_send encoding check
                // requires the exact type; the shim can't absorb it because an
                // override must match `NSBezierPath`'s `'Q'` signature.
                #[cfg(target_os = "macos")]
                unsafe {
                    let _: () =
                        msg_send![&bezier, setLineCapStyle: cg_line_cap(stroke.cap) as usize];
                    let _: () =
                        msg_send![&bezier, setLineJoinStyle: cg_line_join(stroke.join) as usize];
                }
                #[cfg(not(target_os = "macos"))]
                unsafe {
                    let _: () = msg_send![&bezier, setLineCapStyle: cg_line_cap(stroke.cap)];
                    let _: () = msg_send![&bezier, setLineJoinStyle: cg_line_join(stroke.join)];
                }
                let _: () = unsafe { msg_send![&bezier, setMiterLimit: stroke.miter_limit as CGFloat] };
                // Dash pattern: `setLineDash:count:phase:` on the bezier path.
                if !stroke.dash.is_empty() {
                    let pattern: Vec<CGFloat> = stroke.dash.iter().map(|&d| d as CGFloat).collect();
                    unsafe {
                        let _: () = msg_send![
                            &bezier,
                            setLineDash: pattern.as_ptr(),
                            count: pattern.len() as isize,
                            phase: stroke.dash_offset as CGFloat,
                        ];
                    }
                }
                // Gradient strokes are approximated by their first stop color
                // (CoreGraphics has no direct gradient-stroke; clipping to a
                // stroked outline needs CGPathCreateCopyByStrokingPath — a v2
                // refinement). Solid strokes are exact.
                set_blend(ctx, paint.blend);
                let col = (self.make_color)(stroke_color(paint));
                let _: () = unsafe { msg_send![&col, setStroke] };
                let _: () = unsafe { msg_send![&bezier, stroke] };
                reset_blend(ctx, paint.blend);
            }
            DrawOp::Clip { path, fill_rule } => {
                let bezier = self.build_path(path);
                if *fill_rule == FillRule::EvenOdd {
                    let _: () = unsafe { msg_send![&bezier, setUsesEvenOddFillRule: true] };
                }
                // `addClip` intersects the current context's clip; it
                // persists until the enclosing CGContextRestoreGState
                // (i.e. the author's matching `restore()`), matching Canvas2D.
                let _: () = unsafe { msg_send![&bezier, addClip] };
            }
            DrawOp::Image { image, dst, alpha, blend } => {
                if let Some(cg_image) = cached_cg_image(image) {
                    unsafe {
                        CGContextSaveGState(ctx);
                        CGContextSetBlendMode(ctx, cg_blend_mode(*blend));
                        CGContextSetAlpha(ctx, *alpha as CGFloat);
                        // The canvas context is top-left-origin (iOS drawRect /
                        // macOS isFlipped). CoreGraphics draws images
                        // bottom-left-origin, so flip vertically within `dst` to
                        // land the image upright. Save/restore reverts the CTM,
                        // blend, and alpha together.
                        CGContextTranslateCTM(ctx, dst.x as CGFloat, (dst.y + dst.h) as CGFloat);
                        CGContextScaleCTM(ctx, 1.0, -1.0);
                        let r = CGRect::new(
                            CGPoint::new(0.0, 0.0),
                            CGSize::new(dst.w as CGFloat, dst.h as CGFloat),
                        );
                        CGContextDrawImage(ctx, r, cg_image);
                        CGContextRestoreGState(ctx);
                    }
                }
            }
            DrawOp::Layer { id, clear, ops: nested, alpha, blend } => {
                self.draw_layer(ctx, *id, *clear, nested, *alpha, *blend);
            }
            DrawOp::LayerCached { id, dirty, transform, ops: nested, alpha, blend } => {
                self.draw_cached_layer(ctx, *id, *dirty, transform, nested, *alpha, *blend);
            }
            DrawOp::Shapes { shapes, blend } => {
                // CPU painter has no instanced fast path: expand the batch to
                // per-shape fills, in array order, replaying each through the
                // Fill arm so a batched shape and a hand-authored fill produce
                // identical pixels (CLAUDE.md §7).
                for sh in shapes {
                    self.apply_op(ctx, &sh.to_fill_op(*blend));
                }
            }
            DrawOp::Glyphs { font, glyphs, paint } => {
                // No glyph engine on CoreGraphics: outline each glyph and fill it,
                // matching the GPU (vello) path's geometry (CLAUDE.md §7).
                for op in crate::glyphs::expand_run(font, glyphs, paint) {
                    self.apply_op(ctx, &op);
                }
            }
            DrawOp::MaskGroup { content, .. } => {
                // No soft-mask primitive wired on CoreGraphics yet: draw the
                // content unmasked so it doesn't vanish (the GPU path masks
                // correctly). canvas-native is the sim/emulator fallback.
                for op in content {
                    self.apply_op(ctx, op);
                }
            }
            // `DrawOp` is `#[non_exhaustive]`; future ops no-op until wired.
            _ => {}
        }
    }

    /// Replay `nested` into the persistent layer `id`'s offscreen bitmap
    /// context (wiping first if `clear`), then composite it into `ctx` at
    /// `alpha`/`blend`. The CPU-raster counterpart of the vello retained
    /// op-log layer — same observable pixels (CLAUDE.md §7).
    ///
    /// Size + scale are read from `ctx` itself (clip bounding box + CTM), so
    /// no canvas dimensions need threading through the painter. The bitmap is
    /// drawn with a scale-only CTM so its `CGImage` is row-0-top (matching the
    /// `DrawOp::Image` path), then composited with the same vertical flip the
    /// image blit uses.
    fn draw_layer(
        &self,
        ctx: CGContextRef,
        id: u32,
        clear: bool,
        nested: &[DrawOp],
        alpha: f32,
        blend: BlendMode,
    ) {
        let clip = unsafe { CGContextGetClipBoundingBox(ctx) };
        let ctm = unsafe { CGContextGetCTM(ctx) };
        let sx = (ctm.a as f64).abs();
        let sy = (ctm.d as f64).abs();
        let lw = clip.size.width as f64;
        let lh = clip.size.height as f64;
        let w = (lw * sx).round() as usize;
        let h = (lh * sy).round() as usize;
        if w == 0 || h == 0 || sx == 0.0 || sy == 0.0 {
            return;
        }
        let Some(bmp) = layer_bitmap(id, w, h) else { return };

        if clear {
            let full =
                CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(w as CGFloat, h as CGFloat));
            unsafe { CGContextClearRect(bmp, full) };
        }
        // Replay nested ops at logical scale (no flip): logical-top lands in the
        // bitmap's bottom data row, so the resulting CGImage is row-0-top.
        unsafe {
            CGContextSaveGState(bmp);
            CGContextScaleCTM(bmp, sx as CGFloat, sy as CGFloat);
            CGContextTranslateCTM(bmp, -(clip.origin.x), -(clip.origin.y));
        }
        for op in nested {
            self.apply_op(bmp, op);
        }
        unsafe { CGContextRestoreGState(bmp) };

        // Composite into the main (flipped, top-left) context like an image blit.
        let image = unsafe { CGBitmapContextCreateImage(bmp) };
        if image.is_null() {
            return;
        }
        unsafe {
            CGContextSaveGState(ctx);
            CGContextSetBlendMode(ctx, cg_blend_mode(blend));
            CGContextSetAlpha(ctx, alpha as CGFloat);
            CGContextTranslateCTM(ctx, clip.origin.x, clip.origin.y + clip.size.height as CGFloat);
            CGContextScaleCTM(ctx, 1.0, -1.0);
            let r = CGRect::new(
                CGPoint::new(0.0, 0.0),
                CGSize::new(lw as CGFloat, lh as CGFloat),
            );
            CGContextDrawImage(ctx, r, image);
            CGContextRestoreGState(ctx);
            CGImageRelease(image);
        }
    }

    /// Replay `nested` into the cached layer `id`'s offscreen bitmap (only when
    /// `dirty` — or the first time it's seen / after a resize), then composite it
    /// into `ctx` under the camera `transform` at `alpha`/`blend`. The CPU
    /// **fallback** counterpart of the vello `TransformCompositor` (on real Apple
    /// devices the GPU vello path handles this; this runs on the iOS simulator).
    ///
    /// Same bake as [`draw_layer`](Self::draw_layer) (logical scale, row-0-top
    /// image), but the composite concatenates the logical camera `transform`
    /// before the place-and-flip, so the cached raster moves with the camera at
    /// `O(1)`. A `dirty: false` pan reuses the retained bitmap.
    fn draw_cached_layer(
        &self,
        ctx: CGContextRef,
        id: u32,
        dirty: bool,
        transform: &canvas_core::Transform,
        nested: &[DrawOp],
        alpha: f32,
        blend: BlendMode,
    ) {
        let clip = unsafe { CGContextGetClipBoundingBox(ctx) };
        let ctm = unsafe { CGContextGetCTM(ctx) };
        let sx = (ctm.a as f64).abs();
        let sy = (ctm.d as f64).abs();
        let lw = clip.size.width as f64;
        let lh = clip.size.height as f64;
        let w = (lw * sx).round() as usize;
        let h = (lh * sy).round() as usize;
        if w == 0 || h == 0 || sx == 0.0 || sy == 0.0 {
            return;
        }
        let Some((bmp, fresh)) = cached_layer_bitmap(id, w, h) else { return };

        // Bake only on `dirty` (or first sight / post-resize) — a not-dirty pan
        // reuses the retained bitmap, which is the whole point.
        if dirty || fresh {
            let full =
                CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(w as CGFloat, h as CGFloat));
            unsafe { CGContextClearRect(bmp, full) };
            unsafe {
                CGContextSaveGState(bmp);
                CGContextScaleCTM(bmp, sx as CGFloat, sy as CGFloat);
                CGContextTranslateCTM(bmp, -(clip.origin.x), -(clip.origin.y));
            }
            for op in nested {
                self.apply_op(bmp, op);
            }
            unsafe { CGContextRestoreGState(bmp) };
        }

        let image = unsafe { CGBitmapContextCreateImage(bmp) };
        if image.is_null() {
            return;
        }
        // The camera transform in logical (top-left) coords — same convention as
        // the `DrawOp::Transform` arm.
        let m = CGAffineTransform {
            a: transform.a as CGFloat,
            b: transform.b as CGFloat,
            c: transform.c as CGFloat,
            d: transform.d as CGFloat,
            tx: transform.e as CGFloat,
            ty: transform.f as CGFloat,
        };
        unsafe {
            CGContextSaveGState(ctx);
            CGContextSetBlendMode(ctx, cg_blend_mode(blend));
            CGContextSetAlpha(ctx, alpha as CGFloat);
            // Apply the camera transform before placing the layer (logical space).
            CGContextConcatCTM(ctx, m);
            CGContextTranslateCTM(ctx, clip.origin.x, clip.origin.y + lh as CGFloat);
            CGContextScaleCTM(ctx, 1.0, -1.0);
            let r = CGRect::new(
                CGPoint::new(0.0, 0.0),
                CGSize::new(lw as CGFloat, lh as CGFloat),
            );
            CGContextDrawImage(ctx, r, image);
            CGContextRestoreGState(ctx);
            CGImageRelease(image);
        }
    }

    /// Fill `bezier` with a gradient paint: clip to the path, draw the
    /// gradient over the clipped region, restore. Mirrors the svg painter.
    fn fill_gradient(&self, ctx: CGContextRef, bezier: &Retained<NSObject>, paint: &Paint) {
        unsafe { CGContextSaveGState(ctx) };
        let _: () = unsafe { msg_send![bezier, addClip] };
        match &paint.kind {
            PaintKind::Linear(g) => {
                let (grad, cs) = build_gradient(&g.stops);
                unsafe {
                    CGContextDrawLinearGradient(
                        ctx,
                        grad,
                        CGPoint::new(g.x0 as CGFloat, g.y0 as CGFloat),
                        CGPoint::new(g.x1 as CGFloat, g.y1 as CGFloat),
                        CG_GRADIENT_DRAWS_EXTEND,
                    );
                    CGGradientRelease(grad);
                    CGColorSpaceRelease(cs);
                }
            }
            PaintKind::Radial(g) => {
                let (grad, cs) = build_gradient(&g.stops);
                unsafe {
                    CGContextDrawRadialGradient(
                        ctx,
                        grad,
                        CGPoint::new(g.cx as CGFloat, g.cy as CGFloat),
                        0.0,
                        CGPoint::new(g.cx as CGFloat, g.cy as CGFloat),
                        g.r as CGFloat,
                        CG_GRADIENT_DRAWS_EXTEND,
                    );
                    CGGradientRelease(grad);
                    CGColorSpaceRelease(cs);
                }
            }
            PaintKind::Solid(_) | _ => {}
        }
        unsafe { CGContextRestoreGState(ctx) };
    }

    fn build_path(&self, path: &Path) -> Retained<NSObject> {
        use canvas_core::PathSeg;
        let bezier: Retained<NSObject> = unsafe { msg_send_id![self.bezier_class, bezierPath] };
        for seg in &path.segs {
            match seg {
                PathSeg::MoveTo { x, y } => {
                    let p = CGPoint::new(*x as CGFloat, *y as CGFloat);
                    let _: () = unsafe { msg_send![&bezier, moveToPoint: p] };
                }
                PathSeg::LineTo { x, y } => {
                    let p = CGPoint::new(*x as CGFloat, *y as CGFloat);
                    let _: () = unsafe { msg_send![&bezier, addLineToPoint: p] };
                }
                PathSeg::QuadTo { cx, cy, x, y } => {
                    let ctrl = CGPoint::new(*cx as CGFloat, *cy as CGFloat);
                    let end = CGPoint::new(*x as CGFloat, *y as CGFloat);
                    let _: () = unsafe { msg_send![&bezier, addQuadCurveToPoint: end, controlPoint: ctrl] };
                }
                PathSeg::CubicTo { c1x, c1y, c2x, c2y, x, y } => {
                    let cp1 = CGPoint::new(*c1x as CGFloat, *c1y as CGFloat);
                    let cp2 = CGPoint::new(*c2x as CGFloat, *c2y as CGFloat);
                    let end = CGPoint::new(*x as CGFloat, *y as CGFloat);
                    let _: () = unsafe {
                        msg_send![&bezier, addCurveToPoint: end, controlPoint1: cp1, controlPoint2: cp2]
                    };
                }
                PathSeg::Close => {
                    let _: () = unsafe { msg_send![&bezier, closePath] };
                }
            }
        }
        bezier
    }
}

/// Build a `CGGradientRef` + its colorspace from canvas gradient stops.
/// Caller releases both.
fn build_gradient(stops: &[canvas_core::GradientStop]) -> (CGGradientRef, CGColorSpaceRef) {
    let mut components: Vec<CGFloat> = Vec::with_capacity(stops.len() * 4);
    let mut locations: Vec<CGFloat> = Vec::with_capacity(stops.len());
    for s in stops {
        let c = s.color;
        components.push(c.r as CGFloat / 255.0);
        components.push(c.g as CGFloat / 255.0);
        components.push(c.b as CGFloat / 255.0);
        components.push(c.a as CGFloat / 255.0);
        locations.push(s.offset as CGFloat);
    }
    unsafe {
        let cs = CGColorSpaceCreateDeviceRGB();
        let grad = CGGradientCreateWithColorComponents(
            cs,
            components.as_ptr(),
            locations.as_ptr(),
            stops.len(),
        );
        (grad, cs)
    }
}

/// A persistent layer's offscreen bitmap context plus the backing buffer that
/// keeps its pixels alive (`CGBitmapContextCreate` writes into `_buf`).
struct BitmapSurface {
    ctx: CGContextRef,
    _buf: Vec<u8>,
    w: usize,
    h: usize,
}

thread_local! {
    /// Persistent [`DrawOp::Layer`] bitmap contexts keyed by layer id, retained
    /// across frames so baked content accumulates. Never evicts (apart from a
    /// resize, which rebuilds the surface); canvas authors use a small, stable
    /// set of layer ids.
    static LAYER_BITMAPS: RefCell<HashMap<u32, BitmapSurface>> = RefCell::new(HashMap::new());

    /// Persistent [`DrawOp::LayerCached`] bitmap contexts keyed by layer id —
    /// baked once (`dirty`) and composited under a camera transform every frame.
    /// Distinct from [`LAYER_BITMAPS`] so a cached and an accumulate layer can
    /// share an id.
    static CACHED_LAYER_BITMAPS: RefCell<HashMap<u32, BitmapSurface>> = RefCell::new(HashMap::new());
}

/// Get-or-build the cached-layer bitmap context for `id` at `w × h` device
/// pixels, returning `(ctx, fresh)` where `fresh` is `true` when this call
/// created (or resized → recreated) it — so the caller bakes on first sight even
/// if the frame said not-dirty. A size change releases the old context.
fn cached_layer_bitmap(id: u32, w: usize, h: usize) -> Option<(CGContextRef, bool)> {
    CACHED_LAYER_BITMAPS.with(|m| {
        let mut map = m.borrow_mut();
        if let Some(s) = map.get(&id) {
            if s.w == w && s.h == h {
                return Some((s.ctx, false));
            }
            unsafe { CGContextRelease(s.ctx) };
            map.remove(&id);
        }
        let mut buf = vec![0u8; w * h * 4];
        let cs = unsafe { CGColorSpaceCreateDeviceRGB() };
        let ctx = unsafe {
            CGBitmapContextCreate(
                buf.as_mut_ptr() as *mut c_void,
                w,
                h,
                8,
                w * 4,
                cs,
                CG_IMAGE_ALPHA_PREMULTIPLIED_LAST,
            )
        };
        unsafe { CGColorSpaceRelease(cs) };
        if ctx.is_null() {
            return None;
        }
        map.insert(id, BitmapSurface { ctx, _buf: buf, w, h });
        Some((ctx, true))
    })
}

/// Get-or-build the persistent bitmap context for layer `id` at `w × h`
/// device pixels. A size change releases the old context and builds a fresh
/// (blank) one.
fn layer_bitmap(id: u32, w: usize, h: usize) -> Option<CGContextRef> {
    LAYER_BITMAPS.with(|m| {
        let mut map = m.borrow_mut();
        if let Some(s) = map.get(&id) {
            if s.w == w && s.h == h {
                return Some(s.ctx);
            }
            // Size changed: release the stale context before replacing it.
            unsafe { CGContextRelease(s.ctx) };
            map.remove(&id);
        }
        let mut buf = vec![0u8; w * h * 4];
        let cs = unsafe { CGColorSpaceCreateDeviceRGB() };
        let ctx = unsafe {
            CGBitmapContextCreate(
                buf.as_mut_ptr() as *mut c_void,
                w,
                h,
                8,
                w * 4,
                cs,
                CG_IMAGE_ALPHA_PREMULTIPLIED_LAST,
            )
        };
        unsafe { CGColorSpaceRelease(cs) };
        if ctx.is_null() {
            return None;
        }
        map.insert(id, BitmapSurface { ctx, _buf: buf, w, h });
        Some(ctx)
    })
}

thread_local! {
    /// Per-thread cache of built `CGImage`s keyed by [`ImageSource::id`].
    /// Stored as a raw pointer (a `CGImageRef`, retained on insert) because
    /// `CGImage` is a CoreFoundation type, not an objc object. The render
    /// thread is single-threaded, so a `thread_local` is the right scope.
    /// Never evicts — canvas authors use a small, stable set of image ids.
    static CG_IMAGE_CACHE: RefCell<HashMap<u64, usize>> = RefCell::new(HashMap::new());
}

/// Get-or-build the cached `CGImage` for `src`. Returns `None` for an
/// invalid (mismatched-length) or empty image.
fn cached_cg_image(src: &ImageSource) -> Option<CGImageRef> {
    if !src.is_valid() || src.width == 0 || src.height == 0 {
        return None;
    }
    CG_IMAGE_CACHE.with(|c| {
        if let Some(&ptr) = c.borrow().get(&src.id) {
            return Some(ptr as CGImageRef);
        }
        let img = build_cg_image(src)?;
        // Retain for the cache's ownership; the temporary `img` ref is
        // released by `build_cg_image`'s caller contract below.
        let retained = unsafe { CGImageRetain(img) };
        unsafe { CGImageRelease(img) };
        c.borrow_mut().insert(src.id, retained as usize);
        Some(retained)
    })
}

/// Build a `CGImage` from straight RGBA8. The returned image is +1
/// retained (the caller retains again for the cache and releases this
/// reference). `CFData` owns a copy of the pixels, so the image is
/// self-contained.
fn build_cg_image(src: &ImageSource) -> Option<CGImageRef> {
    unsafe {
        let cf_data = CFDataCreate(
            std::ptr::null(),
            src.rgba.as_ptr(),
            src.rgba.len() as isize,
        );
        if cf_data.is_null() {
            return None;
        }
        let provider = CGDataProviderCreateWithCFData(cf_data);
        // The provider retains the CFData; drop our reference.
        CFRelease(cf_data);
        if provider.is_null() {
            return None;
        }
        let cs = CGColorSpaceCreateDeviceRGB();
        let image = CGImageCreate(
            src.width as usize,
            src.height as usize,
            8,
            32,
            (src.width as usize) * 4,
            cs,
            CG_IMAGE_ALPHA_LAST,
            provider,
            std::ptr::null(),
            true,
            CG_RENDERING_INTENT_DEFAULT,
        );
        // The image retains the colorspace + provider; drop our references.
        CGColorSpaceRelease(cs);
        CGDataProviderRelease(provider);
        if image.is_null() {
            None
        } else {
            Some(image)
        }
    }
}

/// Map a [`BlendMode`] to its `CGBlendMode` raw value. Unknown
/// (`#[non_exhaustive]`) modes fall back to Normal, matching the contract.
fn cg_blend_mode(blend: BlendMode) -> i32 {
    match blend {
        BlendMode::Normal => CG_BLEND_NORMAL,
        BlendMode::DestinationOut => CG_BLEND_DESTINATION_OUT,
        BlendMode::Multiply => CG_BLEND_MULTIPLY,
        BlendMode::Screen => CG_BLEND_SCREEN,
        BlendMode::Overlay => CG_BLEND_OVERLAY,
        BlendMode::Darken => CG_BLEND_DARKEN,
        BlendMode::Lighten => CG_BLEND_LIGHTEN,
        BlendMode::ColorDodge => CG_BLEND_COLOR_DODGE,
        BlendMode::ColorBurn => CG_BLEND_COLOR_BURN,
        BlendMode::HardLight => CG_BLEND_HARD_LIGHT,
        BlendMode::SoftLight => CG_BLEND_SOFT_LIGHT,
        BlendMode::Difference => CG_BLEND_DIFFERENCE,
        BlendMode::Exclusion => CG_BLEND_EXCLUSION,
        BlendMode::Hue => CG_BLEND_HUE,
        BlendMode::Saturation => CG_BLEND_SATURATION,
        BlendMode::Color => CG_BLEND_COLOR,
        BlendMode::Luminosity => CG_BLEND_LUMINOSITY,
        _ => CG_BLEND_NORMAL,
    }
}

/// Set the context blend mode for a blended op. No-op for `Normal`.
fn set_blend(ctx: CGContextRef, blend: BlendMode) {
    if blend != BlendMode::Normal {
        unsafe { CGContextSetBlendMode(ctx, cg_blend_mode(blend)) };
    }
}

/// Restore Normal after a blended op so it doesn't leak into the next.
fn reset_blend(ctx: CGContextRef, blend: BlendMode) {
    if blend != BlendMode::Normal {
        unsafe { CGContextSetBlendMode(ctx, CG_BLEND_NORMAL) };
    }
}

/// First-stop color of a gradient paint, or the solid color — used as the
/// stroke color (gradient strokes are approximated, see `apply_op`).
fn stroke_color(paint: &Paint) -> Color {
    match &paint.kind {
        PaintKind::Solid(c) => *c,
        PaintKind::Linear(g) => g.stops.first().map(|s| s.color).unwrap_or(Color::BLACK),
        PaintKind::Radial(g) => g.stops.first().map(|s| s.color).unwrap_or(Color::BLACK),
        _ => Color::BLACK,
    }
}

// `CGLineCap` / `CGLineJoin` are `int32_t` C enums — the bezier path's
// `setLineCapStyle:` / `setLineJoinStyle:` take them by value, so the
// argument MUST be `i32` (Obj-C type code 'i'). Passing `i64` ('q')
// trips objc2's runtime encoding check and aborts in `drawRect:`.
//
// `NSLineCapStyle` / `NSLineJoinStyle` use the same numeric values as
// `CGLineCap` / `CGLineJoin`, so this mapping is platform-identical.
fn cg_line_cap(c: LineCap) -> i32 {
    match c {
        LineCap::Butt => 0,
        LineCap::Round => 1,
        LineCap::Square => 2,
    }
}

fn cg_line_join(j: LineJoin) -> i32 {
    match j {
        LineJoin::Miter => 0,
        LineJoin::Round => 1,
        LineJoin::Bevel => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cg_blend_mode_constants_match_coregraphics() {
        // These are the raw `CGBlendMode` values from CoreGraphics/CGContext.h.
        // A wrong constant (e.g. 17 = kCGBlendModeCopy instead of 23 =
        // kCGBlendModeDestinationOut) would silently break the macOS/iOS
        // eraser — paint would composite as the wrong Porter-Duff op.
        assert_eq!(cg_blend_mode(BlendMode::Normal), 0);
        assert_eq!(cg_blend_mode(BlendMode::Multiply), 1);
        assert_eq!(cg_blend_mode(BlendMode::Screen), 2);
        assert_eq!(cg_blend_mode(BlendMode::DestinationOut), 23);
    }
}
