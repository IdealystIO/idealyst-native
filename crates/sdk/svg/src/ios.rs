//! iOS implementation of the SVG SDK — native vector renderer.
//!
//! Parses the markup with `usvg`, stashes the resulting tree on a
//! `UIView` subclass, and replays it into the current `CGContext`
//! from the view's `drawRect:`. No rasterization step — the view
//! re-draws at the device's pixel resolution every time UIKit asks,
//! so the output stays crisp through resize, scroll, transform, and
//! retina-vs-non-retina screen scale changes.
//!
//! # View subclass: [`IdealystSvgView`]
//!
//! - Overrides `drawRect:` to walk the parsed `usvg::Tree` against an
//!   `SvgPainter` implementation that emits `UIBezierPath` +
//!   `CGContext` calls.
//! - Overrides `layoutSubviews` so a bounds change triggers a redraw
//!   (UIView's default doesn't redraw on resize; `contentMode =
//!   UIViewContentModeRedraw` would also work but `layoutSubviews` is
//!   more explicit).
//! - Stores the parsed tree + intrinsic size in ivars (`SvgViewIvars`).
//!
//! # Re-parsing
//!
//! The Effect closure parses on every markup change. Parsing usvg
//! costs a few ms for typical icons; the result is cached on the view
//! until the next markup change, so `drawRect:` itself only walks
//! the tree (no parse). For high-frequency reactive markup (e.g. an
//! animated SVG string assembled per frame), this would dominate;
//! the v2 fix is a markup-hash cache.

use crate::tree_walker::{map_point, render_tree, MaskKind, Rect as SvgRect, StrokeParams, SvgPainter};
use crate::{SvgOps, SvgProps};
use backend_ios::{IosBackend, IosNode};
use runtime_core::Effect;

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, NSObject};
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{CGFloat, CGPoint, CGRect, CGSize, MainThreadMarker};
use objc2_ui_kit::UIView;
use usvg::tiny_skia_path::{PathSegment, Point as SvgPoint};
use usvg::{Color as SvgColor, FillRule, LineCap, LineJoin, Transform};

use std::any::Any;
use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;

// ============================================================================
// Core Graphics + UIKit C-level bindings
// ============================================================================
//
// CoreGraphics ships as a system framework on iOS — the static
// linker resolves these symbols automatically; no `#[link(name)]`
// attributes are needed. UIBezierPath / UIColor go through Obj-C
// `msg_send`; the rest of the rasterizer (gradients, raw CTM concat,
// clip-by-current-path) needs the C entry points.

/// CoreGraphics's affine transform — same memory layout as
/// `tiny_skia::Transform` (six contiguous floats), so we just reorder
/// fields when handing one to a CG function. Defined locally rather
/// than depending on `core-graphics` because the rest of the repo
/// already does `extern "C"` for Core Graphics functions when it
/// needs them (see `imp/icon.rs`, `imp/portal.rs`).
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

impl CGAffineTransform {
    fn from_usvg(t: Transform) -> Self {
        // tiny_skia stores column-major-column-vector. CGAffineTransform
        // uses the same convention with field names a/b/c/d/tx/ty.
        // The element-wise mapping is sx→a, ky→b, kx→c, sy→d.
        CGAffineTransform {
            a: t.sx as CGFloat,
            b: t.ky as CGFloat,
            c: t.kx as CGFloat,
            d: t.sy as CGFloat,
            tx: t.tx as CGFloat,
            ty: t.ty as CGFloat,
        }
    }
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
type CGImageRef = *mut c_void;

/// CGGradientDrawingOptions — bit 0 = `kCGGradientDrawsBeforeStartLocation`,
/// bit 1 = `kCGGradientDrawsAfterEndLocation`. We set both so the
/// gradient covers the whole clipped region; without this the area
/// outside the gradient's t=0..1 range stays transparent.
const CG_GRADIENT_DRAWS_BEFORE_START: u32 = 1;
const CG_GRADIENT_DRAWS_AFTER_END: u32 = 2;
const CG_GRADIENT_DRAWS_EXTEND: u32 =
    CG_GRADIENT_DRAWS_BEFORE_START | CG_GRADIENT_DRAWS_AFTER_END;

extern "C" {
    /// Returns the current CGContextRef in the active drawing stack.
    /// Inside `drawRect:` UIKit has already pushed the view's context;
    /// outside drawing entry points this returns NULL.
    fn UIGraphicsGetCurrentContext() -> CGContextRef;

    fn CGContextSaveGState(c: CGContextRef);
    fn CGContextRestoreGState(c: CGContextRef);
    fn CGContextConcatCTM(c: CGContextRef, transform: CGAffineTransform);
    fn CGContextScaleCTM(c: CGContextRef, sx: CGFloat, sy: CGFloat);
    fn CGContextTranslateCTM(c: CGContextRef, tx: CGFloat, ty: CGFloat);

    // (`CGContextClip` / `CGContextEOClip` aren't called directly —
    // `-[UIBezierPath addClip]` already invokes the right one based
    // on the path's `usesEvenOddFillRule` flag, so we save a couple
    // of FFI hops by going through the bezier surface.)
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

    // Transparency layers — CGContext-managed offscreen buffer that
    // composites into the parent context on `End`. Used for group
    // opacity (between Begin and End we set CGContextSetAlpha to the
    // group's alpha so children render at full strength into the
    // layer, then composite once).
    fn CGContextBeginTransparencyLayer(c: CGContextRef, aux_info: *const c_void);
    fn CGContextEndTransparencyLayer(c: CGContextRef);
    fn CGContextSetAlpha(c: CGContextRef, alpha: CGFloat);

    // Bitmap context for offscreen mask rendering. `CGBitmapContextCreate`
    // allocates a buffer + returns a CGContext that draws into it;
    // `CGBitmapContextCreateImage` snapshots the buffer as a
    // CGImage. We use this pair to capture mask content for
    // `CGContextClipToMask`.
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
    fn CGImageRelease(img: CGImageRef);

    // Apply a CGImage as an alpha-mask clip on the current context.
    // For luminance masks (non-mask CGImages) Quartz uses the
    // source samples as a luminance mask. For our purposes both work
    // through the same call; the iOS implementation always renders
    // the mask to a regular RGB context and lets Quartz do
    // luminance extraction.
    fn CGContextClipToMask(c: CGContextRef, rect: CGRect, mask: CGImageRef);

    // CTM access — needed when building the offscreen mask context
    // so it inherits the current world transform.
    fn CGContextGetCTM(c: CGContextRef) -> CGAffineTransform;
    fn CGContextDrawImage(c: CGContextRef, rect: CGRect, image: CGImageRef);
}

// CGBitmapInfo flags. `kCGImageAlphaPremultipliedLast` (= 1) =
// RGBA with premultiplied alpha. `kCGBitmapByteOrder32Little` (=
// 8192) = little-endian byte order, matching iOS's native pixel
// layout. UInt32-typed.
const CG_BITMAP_ALPHA_PREMUL_LAST: u32 = 1;
const CG_BITMAP_BYTE_ORDER_32_LITTLE: u32 = 2 << 12;
const CG_BITMAP_INFO_RGBA8: u32 = CG_BITMAP_ALPHA_PREMUL_LAST | CG_BITMAP_BYTE_ORDER_32_LITTLE;

// ============================================================================
// Per-view state (lives in ivars)
// ============================================================================

pub(crate) struct SvgViewIvars {
    /// Parsed tree. `RefCell` so the Effect closure can swap it on
    /// markup change without mutating through `&self`. `Option`
    /// because the view may be allocated before the first parse
    /// lands.
    tree: RefCell<Option<usvg::Tree>>,
    /// Cached natural size from `tree.size()`. Read by the
    /// `intrinsic_size` op via the OPS table — saves a re-parse just
    /// to query dimensions.
    intrinsic: RefCell<Option<(f32, f32)>>,
}

// ============================================================================
// UIView subclass
// ============================================================================

declare_class!(
    /// Custom UIView subclass that walks a parsed SVG tree against
    /// the current CGContext in its `drawRect:`. Created by
    /// [`build_svg`]; the framework registers it as a regular
    /// `IosNode::View` so layout/touch/animation paths see it as a
    /// generic UIView.
    pub(crate) struct IdealystSvgView;

    unsafe impl ClassType for IdealystSvgView {
        type Super = UIView;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystSvgView";
    }

    impl DeclaredClass for IdealystSvgView {
        type Ivars = SvgViewIvars;
    }

    unsafe impl IdealystSvgView {
        // Override drawRect: to paint the SVG tree. UIKit invokes
        // this whenever the view's content is invalidated — initial
        // display, `setNeedsDisplay`, bounds change (because we set
        // `contentMode = UIViewContentModeRedraw` at init), etc.
        #[method(drawRect:)]
        fn draw_rect(&self, _dirty_rect: CGRect) {
            self.paint_now();
        }

        // contentMode = Redraw triggers drawRect on bounds change but
        // not directly on transform changes. Force a redraw from
        // layoutSubviews too — covers the case where the parent uses
        // sublayer transforms and our bounds technically stay the
        // same while our visual size changes.
        #[method(layoutSubviews)]
        fn layout_subviews(&self) {
            let _: () = unsafe { msg_send![super(self), layoutSubviews] };
            let _: () = unsafe { msg_send![self, setNeedsDisplay] };
        }
    }
);

impl IdealystSvgView {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this: Allocated<Self> = mtm.alloc();
        let this = this.set_ivars(SvgViewIvars {
            tree: RefCell::new(None),
            intrinsic: RefCell::new(None),
        });
        let this: Retained<Self> = unsafe {
            msg_send_id![
                super(this),
                initWithFrame: CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(0.0, 0.0))
            ]
        };
        // Opaque = NO so the SVG's transparent regions show the
        // parent's background. UIView's default is YES for performance,
        // which would composite over an undefined background and
        // produce garbage in the see-through areas.
        let _: () = unsafe { msg_send![&*this, setOpaque: false] };
        // backgroundColor = nil — same reason. Without this UIView
        // fills its bounds with white before drawRect: runs.
        let _: () = unsafe { msg_send![&*this, setBackgroundColor: std::ptr::null::<AnyObject>()] };
        // UIViewContentModeRedraw (= 4) — invalidates content on
        // bounds change so the SVG re-rasters at the new pixel
        // resolution instead of stretching its previous output.
        let _: () = unsafe { msg_send![&*this, setContentMode: 4i64] };
        this
    }

    /// Swap the cached tree + intrinsic size and invalidate the
    /// view's content so UIKit re-runs `drawRect:` on the next pass.
    fn install_tree(&self, tree: usvg::Tree) {
        let size = tree.size();
        *self.ivars().intrinsic.borrow_mut() = Some((size.width(), size.height()));
        *self.ivars().tree.borrow_mut() = Some(tree);
        let _: () = unsafe { msg_send![self, setNeedsDisplay] };
    }

    fn intrinsic_size(&self) -> Option<(f32, f32)> {
        *self.ivars().intrinsic.borrow()
    }

    /// Walk the cached tree into the current `CGContext`. Called
    /// from `drawRect:`. Bails silently if no tree is installed yet
    /// (the Effect closure hasn't fired) — better than an
    /// `unwrap()` half-second after view alloc.
    fn paint_now(&self) {
        let ctx = unsafe { UIGraphicsGetCurrentContext() };
        if ctx.is_null() {
            return;
        }
        let tree_borrow = self.ivars().tree.borrow();
        let Some(tree) = tree_borrow.as_ref() else {
            return;
        };
        let bounds: CGRect = unsafe { msg_send![self, bounds] };
        let size = tree.size();
        let svg_w = size.width().max(1e-6);
        let svg_h = size.height().max(1e-6);

        // Aspect-fit scale: same scale factor on both axes,
        // centered in the view bounds. Matches the SVG `<svg
        // preserveAspectRatio="xMidYMid meet">` default plus
        // UIImageView's `scaleAspectFit` behavior — what authors
        // expect from a drop-in image element.
        let scale_x = bounds.size.width as f32 / svg_w;
        let scale_y = bounds.size.height as f32 / svg_h;
        let scale = scale_x.min(scale_y);
        let draw_w = svg_w * scale;
        let draw_h = svg_h * scale;
        let offset_x = (bounds.size.width as f32 - draw_w) * 0.5;
        let offset_y = (bounds.size.height as f32 - draw_h) * 0.5;

        unsafe {
            CGContextSaveGState(ctx);
            CGContextTranslateCTM(ctx, offset_x as CGFloat, offset_y as CGFloat);
            CGContextScaleCTM(ctx, scale as CGFloat, scale as CGFloat);
        }

        let mut painter = IosSvgPainter { ctx };
        render_tree(&mut painter, tree);

        unsafe {
            CGContextRestoreGState(ctx);
        }
    }
}

// ============================================================================
// SvgPainter impl
// ============================================================================

struct IosSvgPainter {
    ctx: CGContextRef,
}

impl IosSvgPainter {
    /// `+[UIBezierPath bezierPath]` — fresh empty path. The other
    /// constructors (`bezierPathWithCGPath:`, etc.) would also work
    /// but require a CGPath we'd build manually; staying in
    /// UIBezierPath the whole way keeps the call sites uniform.
    fn new_bezier_path() -> Retained<NSObject> {
        let cls = objc2::class!(UIBezierPath);
        unsafe { msg_send_id![cls, bezierPath] }
    }
}

impl SvgPainter for IosSvgPainter {
    type Path = Retained<NSObject>;

    fn build_path<I: Iterator<Item = PathSegment>>(&mut self, segments: I) -> Self::Path {
        let path = Self::new_bezier_path();
        // tiny_skia_path only emits MoveTo / LineTo / QuadTo /
        // CubicTo / Close. Mapping is one-to-one with UIBezierPath.
        for seg in segments {
            match seg {
                PathSegment::MoveTo(p) => {
                    let pt = CGPoint::new(p.x as CGFloat, p.y as CGFloat);
                    let _: () = unsafe { msg_send![&path, moveToPoint: pt] };
                }
                PathSegment::LineTo(p) => {
                    let pt = CGPoint::new(p.x as CGFloat, p.y as CGFloat);
                    let _: () = unsafe { msg_send![&path, addLineToPoint: pt] };
                }
                PathSegment::QuadTo(c, p) => {
                    let ctrl = CGPoint::new(c.x as CGFloat, c.y as CGFloat);
                    let end = CGPoint::new(p.x as CGFloat, p.y as CGFloat);
                    let _: () =
                        unsafe { msg_send![&path, addQuadCurveToPoint: end, controlPoint: ctrl] };
                }
                PathSegment::CubicTo(c1, c2, p) => {
                    let cp1 = CGPoint::new(c1.x as CGFloat, c1.y as CGFloat);
                    let cp2 = CGPoint::new(c2.x as CGFloat, c2.y as CGFloat);
                    let end = CGPoint::new(p.x as CGFloat, p.y as CGFloat);
                    let _: () = unsafe {
                        msg_send![
                            &path,
                            addCurveToPoint: end,
                            controlPoint1: cp1,
                            controlPoint2: cp2
                        ]
                    };
                }
                PathSegment::Close => {
                    let _: () = unsafe { msg_send![&path, closePath] };
                }
            }
        }
        path
    }

    fn fill_solid(
        &mut self,
        path: &Self::Path,
        color: SvgColor,
        opacity: f32,
        rule: FillRule,
    ) {
        if rule == FillRule::EvenOdd {
            let _: () = unsafe { msg_send![path, setUsesEvenOddFillRule: true] };
        }
        let ui_color = ui_color(color, opacity);
        let _: () = unsafe { msg_send![&ui_color, setFill] };
        let _: () = unsafe { msg_send![path, fill] };
    }

    fn fill_linear_gradient(
        &mut self,
        path: &Self::Path,
        gradient: &usvg::LinearGradient,
        opacity: f32,
        rule: FillRule,
    ) {
        // Strategy: save the CGContext, push the path as a clip
        // mask, draw the gradient (which now only fills the clipped
        // region), restore.
        unsafe { CGContextSaveGState(self.ctx) };
        push_path_clip(self.ctx, path, rule);

        // usvg stores gradient endpoints in user space, but they
        // still need to pass through the gradient's own `transform`
        // (often identity, sometimes non-trivial).
        let t = gradient.transform();
        let start = map_point(t, SvgPoint { x: gradient.x1(), y: gradient.y1() });
        let end = map_point(t, SvgPoint { x: gradient.x2(), y: gradient.y2() });

        let (cg_gradient, color_space) = build_cg_gradient(gradient.stops(), opacity);
        unsafe {
            CGContextDrawLinearGradient(
                self.ctx,
                cg_gradient,
                CGPoint::new(start.x as CGFloat, start.y as CGFloat),
                CGPoint::new(end.x as CGFloat, end.y as CGFloat),
                CG_GRADIENT_DRAWS_EXTEND,
            );
            CGGradientRelease(cg_gradient);
            CGColorSpaceRelease(color_space);
            CGContextRestoreGState(self.ctx);
        }
    }

    fn fill_radial_gradient(
        &mut self,
        path: &Self::Path,
        gradient: &usvg::RadialGradient,
        opacity: f32,
        rule: FillRule,
    ) {
        unsafe { CGContextSaveGState(self.ctx) };
        push_path_clip(self.ctx, path, rule);

        let t = gradient.transform();
        let focal = map_point(t, SvgPoint { x: gradient.fx(), y: gradient.fy() });
        let center = map_point(t, SvgPoint { x: gradient.cx(), y: gradient.cy() });
        // The radius needs the gradient transform's scale applied
        // too. We approximate via the average of the x/y scale
        // magnitudes — exact for non-skewed transforms, close-enough
        // for skewed ones (which are rare in gradients).
        let scale_avg = ((t.sx * t.sx + t.ky * t.ky).sqrt()
            + (t.kx * t.kx + t.sy * t.sy).sqrt())
            * 0.5;
        let radius = gradient.r().get() * scale_avg;

        let (cg_gradient, color_space) = build_cg_gradient(gradient.stops(), opacity);
        unsafe {
            CGContextDrawRadialGradient(
                self.ctx,
                cg_gradient,
                CGPoint::new(focal.x as CGFloat, focal.y as CGFloat),
                0.0,
                CGPoint::new(center.x as CGFloat, center.y as CGFloat),
                radius as CGFloat,
                CG_GRADIENT_DRAWS_EXTEND,
            );
            CGGradientRelease(cg_gradient);
            CGColorSpaceRelease(color_space);
            CGContextRestoreGState(self.ctx);
        }
    }

    fn stroke_solid(
        &mut self,
        path: &Self::Path,
        color: SvgColor,
        opacity: f32,
        params: StrokeParams,
    ) {
        let _: () = unsafe { msg_send![path, setLineWidth: params.width as CGFloat] };
        let _: () =
            unsafe { msg_send![path, setLineCapStyle: cg_line_cap(params.linecap)] };
        let _: () =
            unsafe { msg_send![path, setLineJoinStyle: cg_line_join(params.linejoin)] };
        let _: () = unsafe { msg_send![path, setMiterLimit: params.miter_limit as CGFloat] };
        if let Some(dash) = params.dasharray {
            apply_dash(path, dash, params.dashoffset);
        }
        let ui_color = ui_color(color, opacity);
        let _: () = unsafe { msg_send![&ui_color, setStroke] };
        let _: () = unsafe { msg_send![path, stroke] };
    }

    fn with_transform<R>(&mut self, transform: Transform, f: impl FnOnce(&mut Self) -> R) -> R {
        unsafe {
            CGContextSaveGState(self.ctx);
            CGContextConcatCTM(self.ctx, CGAffineTransform::from_usvg(transform));
        }
        let result = f(self);
        unsafe { CGContextRestoreGState(self.ctx) };
        result
    }

    fn extend_path<I: Iterator<Item = PathSegment>>(
        &mut self,
        dest: &mut Self::Path,
        segments: I,
        t: Transform,
    ) {
        // Apply `t` to each segment point ourselves rather than
        // calling `[dest applyTransform:]` after the fact —
        // UIBezierPath has no per-segment-batch transform API, and
        // applyTransform applied to a non-empty dest would also move
        // segments that were already appended.
        //
        // msg_send! receivers want `&NSObject`; reborrow `dest`
        // (which is `&mut Retained<NSObject>`) via `&**dest`.
        let path: &NSObject = &**dest;
        for seg in segments {
            match seg {
                PathSegment::MoveTo(p) => {
                    let q = map_point(t, p);
                    let pt = CGPoint::new(q.x as CGFloat, q.y as CGFloat);
                    let _: () = unsafe { msg_send![path, moveToPoint: pt] };
                }
                PathSegment::LineTo(p) => {
                    let q = map_point(t, p);
                    let pt = CGPoint::new(q.x as CGFloat, q.y as CGFloat);
                    let _: () = unsafe { msg_send![path, addLineToPoint: pt] };
                }
                PathSegment::QuadTo(c, p) => {
                    let qc = map_point(t, c);
                    let qp = map_point(t, p);
                    let cpt = CGPoint::new(qc.x as CGFloat, qc.y as CGFloat);
                    let ept = CGPoint::new(qp.x as CGFloat, qp.y as CGFloat);
                    let _: () = unsafe {
                        msg_send![path, addQuadCurveToPoint: ept, controlPoint: cpt]
                    };
                }
                PathSegment::CubicTo(c1, c2, p) => {
                    let qc1 = map_point(t, c1);
                    let qc2 = map_point(t, c2);
                    let qp = map_point(t, p);
                    let cp1 = CGPoint::new(qc1.x as CGFloat, qc1.y as CGFloat);
                    let cp2 = CGPoint::new(qc2.x as CGFloat, qc2.y as CGFloat);
                    let ept = CGPoint::new(qp.x as CGFloat, qp.y as CGFloat);
                    let _: () = unsafe {
                        msg_send![
                            path,
                            addCurveToPoint: ept,
                            controlPoint1: cp1,
                            controlPoint2: cp2
                        ]
                    };
                }
                PathSegment::Close => {
                    let _: () = unsafe { msg_send![path, closePath] };
                }
            }
        }
    }

    fn draw_image(&mut self, kind: &usvg::ImageKind, dst_rect: SvgRect) {
        let bytes: &[u8] = match kind {
            usvg::ImageKind::JPEG(b) | usvg::ImageKind::PNG(b) | usvg::ImageKind::GIF(b) => {
                b.as_slice()
            }
            // `ImageKind::SVG` is handled by the walker before reaching here.
            usvg::ImageKind::SVG(_) => return,
        };
        // UIImage(data:) decodes PNG, JPEG, GIF (also TIFF, BMP,
        // WebP, HEIC) natively. Returns nil on malformed input;
        // we tolerate that with a no-op.
        let data: Retained<NSObject> = unsafe {
            msg_send_id![
                objc2::class!(NSData),
                dataWithBytes: bytes.as_ptr() as *const c_void,
                length: bytes.len()
            ]
        };
        let image: Option<Retained<NSObject>> = unsafe {
            msg_send_id![objc2::class!(UIImage), imageWithData: &*data]
        };
        let Some(uiimage) = image else { return };
        // UIImage backs onto a CGImage we can hand to
        // CGContextDrawImage. The selector returns CGImageRef (raw
        // pointer) so we keep it as such — the UIImage retains it
        // for us through the call.
        //
        // CG coordinate-system gotcha: CGContextDrawImage uses
        // BL-origin coordinates *if* the context has the default CTM.
        // UIKit's drawRect: CTM is TL-origin (UIKit flips it for us),
        // and our walker's transforms preserve that — so we
        // additionally flip the image vertically before drawing so it
        // doesn't end up upside-down. The pattern is:
        //   translate(0, h) → scale(1, -1) → drawImage(rect at 0,0)
        // wrapped in save/restoreGState so we don't disturb sibling
        // draws.
        let cg_image: CGImageRef = unsafe { msg_send![&uiimage, CGImage] };
        if cg_image.is_null() {
            return;
        }
        unsafe {
            CGContextSaveGState(self.ctx);
            CGContextTranslateCTM(self.ctx, dst_rect.x as CGFloat, (dst_rect.y + dst_rect.height) as CGFloat);
            CGContextScaleCTM(self.ctx, 1.0, -1.0);
            CGContextDrawImage(
                self.ctx,
                CGRect::new(
                    CGPoint::new(0.0, 0.0),
                    CGSize::new(dst_rect.width as CGFloat, dst_rect.height as CGFloat),
                ),
                cg_image,
            );
            CGContextRestoreGState(self.ctx);
        }
    }

    fn with_clip<R>(
        &mut self,
        clip: &Self::Path,
        rule: FillRule,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        unsafe { CGContextSaveGState(self.ctx) };
        if rule == FillRule::EvenOdd {
            let _: () = unsafe { msg_send![clip, setUsesEvenOddFillRule: true] };
        }
        // `-[UIBezierPath addClip]` reads `usesEvenOddFillRule` and
        // calls the right CG clipper internally.
        let _: () = unsafe { msg_send![clip, addClip] };
        let result = f(self);
        unsafe { CGContextRestoreGState(self.ctx) };
        result
    }

    fn with_opacity<R>(&mut self, alpha: f32, f: impl FnOnce(&mut Self) -> R) -> R {
        // Sequence:
        //   save gstate → set alpha → begin transparency layer →
        //   children render at full alpha into the offscreen layer →
        //   end transparency layer (composites into parent context
        //   modulated by the current alpha) → restore gstate.
        //
        // Without the transparency layer, group opacity applies
        // per-child via CGContextSetAlpha — overlapping children
        // would double up alpha, producing visually wrong output
        // for any group with partial-opacity children that occlude
        // each other.
        unsafe {
            CGContextSaveGState(self.ctx);
            CGContextSetAlpha(self.ctx, alpha.clamp(0.0, 1.0) as CGFloat);
            CGContextBeginTransparencyLayer(self.ctx, std::ptr::null());
        }
        let result = f(self);
        unsafe {
            CGContextEndTransparencyLayer(self.ctx);
            CGContextRestoreGState(self.ctx);
        }
        result
    }

    fn with_mask<R>(
        &mut self,
        _kind: MaskKind,
        dst_rect: SvgRect,
        render_mask: impl FnOnce(&mut Self),
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        // Strategy:
        // 1. Read parent context's CTM scale so the offscreen buffer
        //    is sized at the rendering resolution that
        //    CGContextClipToMask will composite against.
        // 2. Allocate an offscreen bitmap context. Apply a CTM that
        //    maps the mask's user-space rect to the offscreen's
        //    pixel rect — translate so `dst_rect.{x,y}` lands at
        //    (0,0), scale so width/height fill the buffer.
        // 3. Swap `self.ctx` to the offscreen, run `render_mask`,
        //    restore.
        // 4. Snapshot offscreen → CGImage, release the bitmap
        //    context.
        // 5. On the parent context, push CGContextClipToMask with
        //    `dst_rect` as the user-space target. Quartz uses the
        //    luminance of color images as the mask channel — for
        //    `MaskKind::Luminance` (default) that's exactly what we
        //    want. For `MaskKind::Alpha`, Quartz's behavior is the
        //    same when the offscreen background is opaque white;
        //    leaving the offscreen transparent black means
        //    `alpha = src_alpha * mask_alpha` is implicit because
        //    Quartz multiplies the mask's alpha into the clip too.
        //    Visual result matches resvg's spec compliance on every
        //    test SVG checked.
        // 6. Run `f`, then restoreGState (pops the mask clip).

        let original_ctx = self.ctx;
        let ctm = unsafe { CGContextGetCTM(original_ctx) };
        let scale_x = ((ctm.a * ctm.a + ctm.b * ctm.b) as f64).sqrt() as f32;
        let scale_y = ((ctm.c * ctm.c + ctm.d * ctm.d) as f64).sqrt() as f32;
        // Cap the offscreen at a sane maximum — even huge SVGs at
        // 3× retina rarely exceed 4096²; clamping at 4096 avoids
        // pathological 100MB+ allocations for malformed masks.
        const MAX_DIM: f32 = 4096.0;
        let buf_w = (dst_rect.width * scale_x).ceil().clamp(1.0, MAX_DIM) as usize;
        let buf_h = (dst_rect.height * scale_y).ceil().clamp(1.0, MAX_DIM) as usize;

        let color_space = unsafe { CGColorSpaceCreateDeviceRGB() };
        let mask_ctx = unsafe {
            CGBitmapContextCreate(
                std::ptr::null_mut(),
                buf_w,
                buf_h,
                8,
                buf_w * 4,
                color_space,
                CG_BITMAP_INFO_RGBA8,
            )
        };
        if mask_ctx.is_null() {
            unsafe { CGColorSpaceRelease(color_space) };
            // Allocation failure — render unmasked rather than
            // dropping the whole subtree.
            return f(self);
        }

        // Set up the offscreen CTM so user-space `dst_rect` maps to
        // the offscreen's pixel rectangle. Operations applied in
        // reverse order (Quartz uses post-multiply): scale first,
        // then translate, then any further child transforms.
        unsafe {
            CGContextScaleCTM(mask_ctx, scale_x as CGFloat, scale_y as CGFloat);
            CGContextTranslateCTM(
                mask_ctx,
                -dst_rect.x as CGFloat,
                -dst_rect.y as CGFloat,
            );
        }

        // Render mask content into the offscreen.
        self.ctx = mask_ctx;
        render_mask(self);
        self.ctx = original_ctx;

        let mask_image = unsafe { CGBitmapContextCreateImage(mask_ctx) };
        unsafe {
            CGContextRelease(mask_ctx);
            CGColorSpaceRelease(color_space);
        }
        if mask_image.is_null() {
            return f(self);
        }

        let result;
        unsafe {
            CGContextSaveGState(self.ctx);
            CGContextClipToMask(
                self.ctx,
                CGRect::new(
                    CGPoint::new(dst_rect.x as CGFloat, dst_rect.y as CGFloat),
                    CGSize::new(dst_rect.width as CGFloat, dst_rect.height as CGFloat),
                ),
                mask_image,
            );
            CGImageRelease(mask_image);
            result = f(self);
            CGContextRestoreGState(self.ctx);
        }
        result
    }
}

// ============================================================================
// Painter helpers
// ============================================================================

/// `+[UIColor colorWithRed:green:blue:alpha:]`. The repo doesn't
/// expose typed `UIColor` through `objc2_ui_kit`'s feature set, so
/// we hit the class directly via runtime lookup.
fn ui_color(c: SvgColor, opacity: f32) -> Retained<NSObject> {
    let cls: &AnyClass =
        AnyClass::get("UIColor").expect("UIColor class not found — UIKit linkage broken?");
    let r = c.red as CGFloat / 255.0;
    let g = c.green as CGFloat / 255.0;
    let b = c.blue as CGFloat / 255.0;
    let a = opacity.clamp(0.0, 1.0) as CGFloat;
    unsafe {
        msg_send_id![cls, colorWithRed: r, green: g, blue: b, alpha: a]
    }
}

fn cg_line_cap(c: LineCap) -> i64 {
    match c {
        LineCap::Butt => 0,
        LineCap::Round => 1,
        LineCap::Square => 2,
    }
}

fn cg_line_join(j: LineJoin) -> i64 {
    // CoreGraphics has no MiterClip — substitute plain Miter
    // (visual difference is invisible at typical stroke widths and
    // CG clamps with miterLimit anyway).
    match j {
        LineJoin::Miter | LineJoin::MiterClip => 0,
        LineJoin::Round => 1,
        LineJoin::Bevel => 2,
    }
}

/// `path.setLineDash:count:phase:`. Builds a Vec<CGFloat> from the
/// f32 dasharray so the pointer hands to ObjC the right element
/// stride on platforms where CGFloat=f64.
fn apply_dash(path: &NSObject, dash: &[f32], offset: f32) {
    let pattern: Vec<CGFloat> = dash.iter().map(|v| *v as CGFloat).collect();
    let _: () = unsafe {
        msg_send![
            path,
            setLineDash: pattern.as_ptr(),
            count: pattern.len() as i64,
            phase: offset as CGFloat
        ]
    };
}

/// Push the bezier path as a clip mask on the *current* CGContext
/// with the requested fill rule. Caller must have already done a
/// `CGContextSaveGState`; the matching `CGContextRestoreGState`
/// unwinds the clip. UIBezierPath's `-addClip` reads the current
/// context internally; we don't pass `ctx` here, but the `_ctx`
/// reminder documents the precondition.
fn push_path_clip(_ctx: CGContextRef, path: &NSObject, rule: FillRule) {
    if rule == FillRule::EvenOdd {
        let _: () = unsafe { msg_send![path, setUsesEvenOddFillRule: true] };
    }
    let _: () = unsafe { msg_send![path, addClip] };
}

/// Build a `CGGradientRef` from a usvg stop list. Caller owns the
/// returned gradient AND the colorspace and must release both via
/// `CGGradientRelease` + `CGColorSpaceRelease` when done.
fn build_cg_gradient(
    stops: &[usvg::Stop],
    opacity: f32,
) -> (CGGradientRef, CGColorSpaceRef) {
    let mut components: Vec<CGFloat> = Vec::with_capacity(stops.len() * 4);
    let mut locations: Vec<CGFloat> = Vec::with_capacity(stops.len());
    for s in stops {
        let c = s.color();
        components.push(c.red as CGFloat / 255.0);
        components.push(c.green as CGFloat / 255.0);
        components.push(c.blue as CGFloat / 255.0);
        components.push((s.opacity().get() * opacity).clamp(0.0, 1.0) as CGFloat);
        locations.push(s.offset().get() as CGFloat);
    }
    unsafe {
        let color_space = CGColorSpaceCreateDeviceRGB();
        let gradient = CGGradientCreateWithColorComponents(
            color_space,
            components.as_ptr(),
            locations.as_ptr(),
            stops.len(),
        );
        (gradient, color_space)
    }
}

// ============================================================================
// Public API: register + build + ops
// ============================================================================

pub(crate) static OPS: &dyn SvgOps = &IosSvgOps;

pub fn register(backend: &mut IosBackend) {
    backend.register_external::<SvgProps, _>(|props, b| build_svg(props, b));
}

fn build_svg(props: &Rc<SvgProps>, b: &mut IosBackend) -> IosNode {
    let view = IdealystSvgView::new(b.mtm());
    // Cast to UIView so we can register it with the backend's
    // layout tree + return a generic IosNode::View. Obj-C dispatch
    // still reaches IdealystSvgView's drawRect on the same pointer.
    let view_uiview: Retained<UIView> = unsafe { Retained::cast(view) };
    b.register_external_view(&view_uiview);
    // Re-cast back to IdealystSvgView for install_tree access. The
    // raw pointer is identical; the cast is a Rust type-system
    // dance, not a runtime conversion.
    let view_svg: Retained<IdealystSvgView> =
        unsafe { Retained::cast(view_uiview.clone()) };

    let view_for_effect = view_svg.clone();
    let props_clone = props.clone();
    let _effect = Effect::new(move || {
        let markup = (props_clone.markup)();
        match usvg::Tree::from_str(&markup, &usvg::Options::default()) {
            Ok(tree) => {
                view_for_effect.install_tree(tree);
                if let Some(cb) = &props_clone.on_load {
                    cb();
                }
            }
            Err(e) => {
                if let Some(cb) = &props_clone.on_error {
                    cb(format!("{e}"));
                }
            }
        }
    });

    IosNode::View(view_uiview)
}

struct IosSvgOps;

impl SvgOps for IosSvgOps {
    fn intrinsic_size(&self, node: &dyn Any) -> Option<(f32, f32)> {
        let ios_node = node.downcast_ref::<IosNode>()?;
        let IosNode::View(view) = ios_node else {
            return None;
        };
        // Downcast the UIView back to our subclass by reading the
        // Obj-C runtime class. Avoids needing a thread-local
        // side-table for intrinsic size — the data lives where the
        // view lives, freed automatically when the view drops.
        let view_class = view.class();
        let target_class = IdealystSvgView::class();
        if view_class != target_class {
            return None;
        }
        let svg_view: &IdealystSvgView =
            unsafe { &*(&**view as *const UIView as *const IdealystSvgView) };
        svg_view.intrinsic_size()
    }
}
