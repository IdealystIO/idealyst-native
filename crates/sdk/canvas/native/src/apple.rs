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

use canvas_core::{Color, DrawOp, FillRule, LineCap, LineJoin, Paint, PaintKind, Path};

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, NSObject};
use objc2::{msg_send, msg_send_id};
use objc2_foundation::{CGFloat, CGPoint};

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

/// Cover the whole clipped region (before-start + after-end), matching
/// Canvas2D gradient extension semantics.
const CG_GRADIENT_DRAWS_EXTEND: u32 = 1 | 2;

extern "C" {
    fn CGContextSaveGState(c: CGContextRef);
    fn CGContextRestoreGState(c: CGContextRef);
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
                match &paint.kind {
                    PaintKind::Solid(c) => {
                        let col = (self.make_color)(*c);
                        let _: () = unsafe { msg_send![&col, setFill] };
                        let _: () = unsafe { msg_send![&bezier, fill] };
                    }
                    _ => self.fill_gradient(ctx, &bezier, paint),
                }
            }
            DrawOp::Stroke { path, paint, stroke } => {
                let bezier = self.build_path(path);
                let _: () = unsafe { msg_send![&bezier, setLineWidth: stroke.width as CGFloat] };
                let _: () = unsafe { msg_send![&bezier, setLineCapStyle: cg_line_cap(stroke.cap)] };
                let _: () = unsafe { msg_send![&bezier, setLineJoinStyle: cg_line_join(stroke.join)] };
                let _: () = unsafe { msg_send![&bezier, setMiterLimit: stroke.miter_limit as CGFloat] };
                // Gradient strokes are approximated by their first stop color
                // (CoreGraphics has no direct gradient-stroke; clipping to a
                // stroked outline needs CGPathCreateCopyByStrokingPath — a v2
                // refinement). Solid strokes are exact.
                let col = (self.make_color)(stroke_color(paint));
                let _: () = unsafe { msg_send![&col, setStroke] };
                let _: () = unsafe { msg_send![&bezier, stroke] };
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
            // `DrawOp` is `#[non_exhaustive]`; future ops no-op until wired.
            _ => {}
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
