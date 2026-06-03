//! iOS renderer for the canvas SDK — native CoreGraphics.
//!
//! A `UIView` subclass ([`IdealystCanvasView`]) holds the current
//! [`Scene`](canvas_core::Scene) and replays its [`DrawOp`]s into the
//! `CGContext` from `drawRect:`. No rasterization step — UIKit re-runs
//! `drawRect:` at the device pixel resolution on every invalidation, so
//! output stays crisp through resize and retina scale. A reactive
//! [`Effect`] swaps the scene and calls `setNeedsDisplay`; an animation
//! signal therefore repaints every frame.
//!
//! Modeled on the `svg` SDK's iOS painter (same CoreGraphics C-bindings,
//! same `UIBezierPath` path construction), narrowed to the canvas op set.
//! Canvas coordinates are logical points, top-left origin — UIKit's
//! `drawRect:` CTM already matches, so no axis flip is needed.

use backend_ios::{IosBackend, IosNode};
use canvas_core::{CanvasProps, Color, DrawOp, FillRule, LineCap, LineJoin, Paint, PaintKind, Path};
use runtime_core::Effect;

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, NSObject};
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{CGFloat, CGPoint, CGRect, CGSize, MainThreadMarker};
use objc2_ui_kit::UIView;

use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;

// ============================================================================
// CoreGraphics C bindings (system framework — linker resolves automatically)
// ============================================================================

/// CoreGraphics affine transform. Same `(a, b, c, d, tx, ty)` convention
/// as the framework's `Transform`: maps `(x, y)` to
/// `(a·x + c·y + tx, b·x + d·y + ty)`.
#[repr(C)]
#[derive(Copy, Clone)]
struct CGAffineTransform {
    a: CGFloat,
    b: CGFloat,
    c: CGFloat,
    d: CGFloat,
    tx: CGFloat,
    ty: CGFloat,
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

type CGContextRef = *mut c_void;
type CGGradientRef = *mut c_void;
type CGColorSpaceRef = *mut c_void;

/// Cover the whole clipped region (before-start + after-end), matching
/// Canvas2D gradient extension semantics.
const CG_GRADIENT_DRAWS_EXTEND: u32 = 1 | 2;

extern "C" {
    fn UIGraphicsGetCurrentContext() -> CGContextRef;
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
// View subclass
// ============================================================================

pub(crate) struct CanvasViewIvars {
    /// The current scene to replay. `RefCell` so the Effect closure can
    /// swap it without `&mut self`.
    scene: RefCell<canvas_core::Scene>,
}

declare_class!(
    /// `UIView` subclass that replays a canvas [`Scene`](canvas_core::Scene)
    /// into the current `CGContext` in `drawRect:`.
    pub(crate) struct IdealystCanvasView;

    unsafe impl ClassType for IdealystCanvasView {
        type Super = UIView;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystCanvasView";
    }

    impl DeclaredClass for IdealystCanvasView {
        type Ivars = CanvasViewIvars;
    }

    unsafe impl IdealystCanvasView {
        #[method(drawRect:)]
        fn draw_rect(&self, _dirty_rect: CGRect) {
            self.paint_now();
        }

        // UIView doesn't redraw on bounds change by default; contentMode
        // = Redraw (set at init) invalidates on resize, and forcing a
        // redraw from layoutSubviews covers sublayer-transform cases.
        #[method(layoutSubviews)]
        fn layout_subviews(&self) {
            let _: () = unsafe { msg_send![super(self), layoutSubviews] };
            let _: () = unsafe { msg_send![self, setNeedsDisplay] };
        }
    }
);

impl IdealystCanvasView {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this: Allocated<Self> = mtm.alloc();
        let this = this.set_ivars(CanvasViewIvars { scene: RefCell::new(canvas_core::Scene::new()) });
        let this: Retained<Self> = unsafe {
            msg_send_id![
                super(this),
                initWithFrame: CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(0.0, 0.0))
            ]
        };
        // Transparent: the painter fills its own background; see-through
        // regions show the parent. clipsToBounds keeps drawing inside the
        // canvas box. contentMode = Redraw (4) re-invalidates on resize.
        let _: () = unsafe { msg_send![&*this, setOpaque: false] };
        let _: () = unsafe { msg_send![&*this, setBackgroundColor: std::ptr::null::<AnyObject>()] };
        let _: () = unsafe { msg_send![&*this, setClipsToBounds: true] };
        let _: () = unsafe { msg_send![&*this, setContentMode: 4i64] };
        this
    }

    /// Swap the scene and invalidate so UIKit re-runs `drawRect:`.
    fn install_scene(&self, scene: canvas_core::Scene) {
        *self.ivars().scene.borrow_mut() = scene;
        let _: () = unsafe { msg_send![self, setNeedsDisplay] };
    }

    /// Replay the cached scene into the active `CGContext`.
    fn paint_now(&self) {
        let ctx = unsafe { UIGraphicsGetCurrentContext() };
        if ctx.is_null() {
            return;
        }
        let scene = self.ivars().scene.borrow();
        for op in scene.ops() {
            apply_op(ctx, op);
        }
    }
}

// ============================================================================
// Op replay
// ============================================================================

fn apply_op(ctx: CGContextRef, op: &DrawOp) {
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
            let bezier = build_path(path);
            if *fill_rule == FillRule::EvenOdd {
                let _: () = unsafe { msg_send![&bezier, setUsesEvenOddFillRule: true] };
            }
            match &paint.kind {
                PaintKind::Solid(c) => {
                    let col = ui_color(*c);
                    let _: () = unsafe { msg_send![&col, setFill] };
                    let _: () = unsafe { msg_send![&bezier, fill] };
                }
                _ => fill_gradient(ctx, &bezier, paint),
            }
        }
        DrawOp::Stroke { path, paint, stroke } => {
            let bezier = build_path(path);
            let _: () = unsafe { msg_send![&bezier, setLineWidth: stroke.width as CGFloat] };
            let _: () = unsafe { msg_send![&bezier, setLineCapStyle: cg_line_cap(stroke.cap)] };
            let _: () = unsafe { msg_send![&bezier, setLineJoinStyle: cg_line_join(stroke.join)] };
            let _: () = unsafe { msg_send![&bezier, setMiterLimit: stroke.miter_limit as CGFloat] };
            // Gradient strokes are approximated by their first stop color
            // (CoreGraphics has no direct gradient-stroke; clipping to a
            // stroked outline needs CGPathCreateCopyByStrokingPath — a v2
            // refinement). Solid strokes are exact.
            let col = ui_color(stroke_color(paint));
            let _: () = unsafe { msg_send![&col, setStroke] };
            let _: () = unsafe { msg_send![&bezier, stroke] };
        }
        DrawOp::Clip { path, fill_rule } => {
            let bezier = build_path(path);
            if *fill_rule == FillRule::EvenOdd {
                let _: () = unsafe { msg_send![&bezier, setUsesEvenOddFillRule: true] };
            }
            // `-[UIBezierPath addClip]` intersects the current context's
            // clip; it persists until the enclosing CGContextRestoreGState
            // (i.e. the author's matching `restore()`), matching Canvas2D.
            let _: () = unsafe { msg_send![&bezier, addClip] };
        }
        // `DrawOp` is `#[non_exhaustive]`; future ops no-op until wired.
        _ => {}
    }
}

/// Fill `bezier` with a gradient paint: clip to the path, draw the
/// gradient over the clipped region, restore. Mirrors the svg iOS painter.
fn fill_gradient(ctx: CGContextRef, bezier: &Retained<NSObject>, paint: &Paint) {
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

fn build_path(path: &Path) -> Retained<NSObject> {
    use canvas_core::PathSeg;
    let cls = objc2::class!(UIBezierPath);
    let bezier: Retained<NSObject> = unsafe { msg_send_id![cls, bezierPath] };
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

fn ui_color(c: Color) -> Retained<NSObject> {
    let cls: &AnyClass = AnyClass::get("UIColor").expect("UIColor class not found");
    let r = c.r as CGFloat / 255.0;
    let g = c.g as CGFloat / 255.0;
    let b = c.b as CGFloat / 255.0;
    let a = c.a as CGFloat / 255.0;
    unsafe { msg_send_id![cls, colorWithRed: r, green: g, blue: b, alpha: a] }
}

// `CGLineCap` / `CGLineJoin` are `int32_t` C enums — UIBezierPath's
// `setLineCapStyle:` / `setLineJoinStyle:` take them by value, so the
// argument MUST be `i32` (Obj-C type code 'i'). Passing `i64` ('q')
// trips objc2's runtime encoding check and aborts in `drawRect:`.
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

// ============================================================================
// register + build
// ============================================================================

/// Register the iOS canvas renderer against an `IosBackend`.
pub fn register(backend: &mut IosBackend) {
    canvas_core::ensure_wire_serde();
    backend.register_external::<CanvasProps, _>(|props, b| build_canvas(props, b));
}

fn build_canvas(props: &Rc<CanvasProps>, b: &mut IosBackend) -> IosNode {
    let view = IdealystCanvasView::new(b.mtm());
    // Cast to UIView for layout registration; Obj-C dispatch still reaches
    // IdealystCanvasView's drawRect on the same pointer.
    let view_uiview: Retained<UIView> = unsafe { Retained::cast(view) };
    b.register_external_view(&view_uiview);
    let view_canvas: Retained<IdealystCanvasView> = unsafe { Retained::cast(view_uiview.clone()) };

    let view_for_effect = view_canvas.clone();
    let props_clone = props.clone();
    let _effect = Effect::new(move || {
        let scene = canvas_core::paint_scene(&props_clone);
        view_for_effect.install_scene(scene);
    });

    IosNode::View(view_uiview)
}
