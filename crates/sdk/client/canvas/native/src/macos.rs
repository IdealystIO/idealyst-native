//! macOS renderer for the canvas SDK — native CoreGraphics via AppKit.
//!
//! An `NSView` subclass ([`IdealystCanvasMacView`]) holds the current
//! [`Scene`](canvas_core::Scene) and replays its [`DrawOp`]s into the
//! `CGContext` from `drawRect:`. The painting itself is the *shared*
//! CoreGraphics painter in [`crate::apple`] — byte-for-byte the same
//! op-replay iOS uses; only the mechanism differs:
//!
//! - **Context acquisition**: `NSGraphicsContext.currentContext.CGContext`
//!   (AppKit) instead of `UIGraphicsGetCurrentContext()` (UIKit).
//! - **Coordinate origin**: the view overrides `isFlipped` to return
//!   `YES`, so AppKit installs a top-left-origin CTM into the
//!   `drawRect:` context — identical to UIKit. No axis flip is applied
//!   in the painter (matching iOS). If strokes ever render upside-down,
//!   this `isFlipped` override is the thing to check.
//! - **Bezier paths**: AppKit's `NSBezierPath` renames several
//!   UIKit selectors (`addLineToPoint:` → `lineToPoint:`, even-odd via
//!   `setWindingRule:`, no quad-curve method). Rather than fork the
//!   shared painter, [`IdealystBezierShim`] — a tiny `NSBezierPath`
//!   subclass — re-adds the UIKit-named selectors so the shared
//!   op-replay dispatches identically on both platforms.

use backend_macos::MacosBackend;
use canvas_core::{CanvasPrim, Color, TextureLayer};
use runtime_scene::{Element, MountCx};

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, NSObject};
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_app_kit::{NSBezierPath, NSColor, NSGraphicsContext, NSView};
use objc2_foundation::{CGFloat, CGRect, MainThreadMarker, NSPoint};

use std::cell::RefCell;

use crate::apple::{ApplePainter, CGContextRef};

/// Opaque stand-in for CoreGraphics' `CGContext`, defined solely so a
/// `*mut CGContextOpaque` carries the objc2 type-encoding `^{CGContext=}` —
/// which is exactly what AppKit's runtime registers `-[NSGraphicsContext
/// CGContext]` as returning. The painter works in `CGContextRef` (a
/// `*mut c_void`, encoding `^v`); receiving the msg_send result as that type
/// trips objc2's encoding verifier and SIGABRTs inside `drawRect:`. We receive
/// into this typed pointer to satisfy the check, then cast to `CGContextRef`.
#[repr(C)]
struct CGContextOpaque {
    _private: [u8; 0],
}

// SAFETY: `CGContextOpaque` is never instantiated — only `*mut CGContextOpaque`
// is used, as the return type of a single msg_send. Its ref-encoding is the
// pointer-to-struct form `^{CGContext=}` the AppKit method advertises.
unsafe impl objc2::encode::RefEncode for CGContextOpaque {
    const ENCODING_REF: objc2::encode::Encoding =
        objc2::encode::Encoding::Pointer(&objc2::encode::Encoding::Struct("CGContext", &[]));
}

// ============================================================================
// NSBezierPath shim — re-adds the UIKit-named selectors the shared painter
// dispatches, mapping them onto AppKit's NSBezierPath equivalents.
// ============================================================================

declare_class!(
    /// `NSBezierPath` subclass that adds the `UIBezierPath`-style
    /// selectors the shared [`ApplePainter`] expects. Each method
    /// forwards to the AppKit equivalent — `addLineToPoint:` →
    /// `lineToPoint:`, quad-curve → `curveToPoint:controlPoint:`,
    /// cubic-curve → `curveToPoint:controlPoint1:controlPoint2:`, and
    /// even-odd fill → `setWindingRule:NSWindingRuleEvenOdd`.
    ///
    /// `+bezierPath` (inherited) returns an instance of the receiving
    /// class, so `+[IdealystBezierShim bezierPath]` yields a shim.
    struct IdealystBezierShim;

    unsafe impl ClassType for IdealystBezierShim {
        type Super = NSBezierPath;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystBezierShim";
    }

    impl DeclaredClass for IdealystBezierShim {
        type Ivars = ();
    }

    unsafe impl IdealystBezierShim {
        #[method(addLineToPoint:)]
        fn add_line_to_point(&self, point: NSPoint) {
            let p: &NSBezierPath = self.as_super();
            unsafe { p.lineToPoint(point) };
        }

        // Canvas quad curves map to NSBezierPath's quadratic
        // `curveToPoint:controlPoint:` (single control point) — exact,
        // no degree elevation needed.
        #[method(addQuadCurveToPoint:controlPoint:)]
        fn add_quad_curve(&self, end_point: NSPoint, control_point: NSPoint) {
            let p: &NSBezierPath = self.as_super();
            unsafe { p.curveToPoint_controlPoint(end_point, control_point) };
        }

        #[method(addCurveToPoint:controlPoint1:controlPoint2:)]
        fn add_cubic_curve(&self, end_point: NSPoint, cp1: NSPoint, cp2: NSPoint) {
            let p: &NSBezierPath = self.as_super();
            unsafe { p.curveToPoint_controlPoint1_controlPoint2(end_point, cp1, cp2) };
        }

        // UIKit's `setUsesEvenOddFillRule:YES` ⇒ AppKit's
        // `setWindingRule:NSWindingRuleEvenOdd` (= 1). NO ⇒ NonZero (0).
        #[method(setUsesEvenOddFillRule:)]
        fn set_uses_even_odd(&self, uses: bool) {
            use objc2_app_kit::NSWindingRule;
            let p: &NSBezierPath = self.as_super();
            let rule = if uses {
                NSWindingRule::EvenOdd
            } else {
                NSWindingRule::NonZero
            };
            unsafe { p.setWindingRule(rule) };
        }
    }
);

impl IdealystBezierShim {
    fn as_super(&self) -> &NSBezierPath {
        // SAFETY: IdealystBezierShim's superclass is NSBezierPath.
        unsafe { &*(self as *const Self as *const NSBezierPath) }
    }

    /// The `IdealystBezierShim` class object — handed to the shared
    /// painter so it builds paths via `+[IdealystBezierShim bezierPath]`.
    fn class_ref() -> &'static AnyClass {
        <IdealystBezierShim as ClassType>::class()
    }
}

/// Build the macOS painter vtable: shim bezier class + `NSColor` factory.
fn painter() -> ApplePainter {
    ApplePainter {
        bezier_class: IdealystBezierShim::class_ref(),
        make_color: ns_color,
    }
}

/// `NSColor` responding to `setFill` / `setStroke`, in the device-RGB
/// space (so component values map 1:1 to what the gradient builder uses).
fn ns_color(c: Color) -> Retained<NSObject> {
    let r = c.r as CGFloat / 255.0;
    let g = c.g as CGFloat / 255.0;
    let b = c.b as CGFloat / 255.0;
    let a = c.a as CGFloat / 255.0;
    let col: Retained<NSColor> =
        unsafe { NSColor::colorWithDeviceRed_green_blue_alpha(r, g, b, a) };
    // SAFETY: NSColor is an NSObject subclass; the painter only sends it
    // `setFill` / `setStroke`, both inherited NSColor instance methods.
    unsafe { Retained::cast(col) }
}

// ============================================================================
// View subclass
// ============================================================================

pub(crate) struct CanvasViewIvars {
    /// The current scene to replay. `RefCell` so the Effect closure can
    /// swap it without `&mut self`.
    scene: RefCell<canvas_core::Scene>,
    /// Texture layers (camera, image, …) the scene's `DrawOp::Texture` ops
    /// composite, installed together with the scene so the indices agree.
    /// Their `source`/`rect` closures are re-read per paint, so a live camera
    /// and a reactive drag position both follow.
    layers: RefCell<Vec<TextureLayer>>,
    /// One throwaway CPU-frame subscription per live-stream layer, so a camera
    /// producer keeps feeding the frames our `latest()` pull reads (see
    /// [`canvas_core::sync_layer_subscriptions`]).
    layer_subs: RefCell<Vec<Option<canvas_core::Subscription>>>,
    /// Reports the view's bounds to the canvas (`Scene::size`). Set at
    /// mount; read on every `drawRect:`, which runs whenever the bounds change.
    sizing: RefCell<Option<canvas_core::SizeReporter>>,
}

declare_class!(
    /// `NSView` subclass that replays a canvas [`Scene`](canvas_core::Scene)
    /// into the current `CGContext` in `drawRect:`. `isFlipped` ⇒ top-left
    /// origin so the shared painter needs no axis flip (matches iOS).
    pub(crate) struct IdealystCanvasMacView;

    unsafe impl ClassType for IdealystCanvasMacView {
        type Super = NSView;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystCanvasMacView";
    }

    impl DeclaredClass for IdealystCanvasMacView {
        type Ivars = CanvasViewIvars;
    }

    unsafe impl IdealystCanvasMacView {
        // Top-left origin — same coordinate space as the canvas Scene
        // (logical points, top-left). With this, the AppKit drawRect: CTM
        // matches UIKit's, so the shared painter needs no flip.
        #[method(isFlipped)]
        fn is_flipped(&self) -> bool {
            true
        }

        #[method(drawRect:)]
        fn draw_rect(&self, _dirty_rect: CGRect) {
            self.paint_now();
        }
    }
);

impl IdealystCanvasMacView {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this: Allocated<Self> = mtm.alloc();
        let this = this.set_ivars(CanvasViewIvars {
            scene: RefCell::new(canvas_core::Scene::new()),
            layers: RefCell::new(Vec::new()),
            layer_subs: RefCell::new(Vec::new()),
            sizing: RefCell::new(None),
        });
        let this: Retained<Self> = unsafe { msg_send_id![super(this), init] };
        // Layer-backed so AppKit invalidates + repaints the whole view on
        // resize; transparent so see-through regions show the parent.
        let _: () = unsafe { msg_send![&*this, setWantsLayer: true] };
        this
    }

    /// Swap the scene + layers and invalidate so AppKit re-runs `drawRect:`.
    fn install(&self, scene: canvas_core::Scene, layers: Vec<TextureLayer>) {
        // Keep CPU-frame subscriptions in step with the live layers so a camera
        // producer keeps delivering frames to `latest()` (main thread only).
        canvas_core::sync_layer_subscriptions(&layers, &mut self.ivars().layer_subs.borrow_mut());
        *self.ivars().scene.borrow_mut() = scene;
        *self.ivars().layers.borrow_mut() = layers;
        let _: () = unsafe { msg_send![self, setNeedsDisplay: true] };
    }

    /// Replay the cached scene into the active `CGContext` from
    /// `NSGraphicsContext.currentContext`.
    fn paint_now(&self) {
        // AppKit redraws a layer-backed view on resize, so `drawRect:` is where
        // this view learns its size. The reporter dedupes.
        if let Some(sizing) = &*self.ivars().sizing.borrow() {
            let bounds: CGRect = unsafe { msg_send![self, bounds] };
            sizing.report(bounds.size.width as f32, bounds.size.height as f32);
        }
        let Some(gc) = (unsafe { NSGraphicsContext::currentContext() }) else {
            return;
        };
        // `-[NSGraphicsContext CGContext]` is the AppKit analogue of
        // `UIGraphicsGetCurrentContext()`. CRITICAL: AppKit's runtime registers
        // this method as returning a TYPED `CGContextRef` (objc2 encoding
        // `^{CGContext=}`), NOT a plain `void*` (`^v`). objc2's msg_send
        // encoding check rejects a `*mut c_void`/`CGContextRef` return — and
        // because `drawRect:` is a non-unwinding Obj-C callback, that mismatch
        // aborts as a hard SIGABRT (the "no window on macOS" crash). So receive
        // it into the `CGContextOpaque` typed pointer (which carries the
        // `^{CGContext=}` encoding) and cast to the painter's `CGContextRef`
        // (a `*mut c_void`) after. The iOS path avoids all this because its
        // context comes from the `extern "C"` `UIGraphicsGetCurrentContext`
        // (no msg_send encoding check).
        let ctx: *mut CGContextOpaque = unsafe { msg_send![&gc, CGContext] };
        if ctx.is_null() {
            return;
        }
        let scene = self.ivars().scene.borrow();
        // Texture layers composite at their `DrawOp::Texture` positions inside
        // the replay (see `ApplePainter::paint_scene`).
        painter().paint_scene(ctx as CGContextRef, &scene, &self.ivars().layers.borrow());
    }
}

// ============================================================================
// register + build
// ============================================================================

pub(crate) fn mount_canvas(
    cx: &mut MountCx<'_, MacosBackend>,
    prim: &std::rc::Rc<CanvasPrim>,
    _children: Vec<Element>,
) -> backend_macos::MacosNode {
    let backend = cx.backend().clone();
    let node = {
        let mut b = backend.borrow_mut();
        build_canvas(prim, &mut b)
    };
    crate::finish_mount(&backend, &node, prim);
    node
}

fn build_canvas(
    prim: &std::rc::Rc<CanvasPrim>,
    b: &mut MacosBackend,
) -> backend_macos::MacosNode {
    let props = &prim.props;
    let view = IdealystCanvasMacView::new(b.mtm());
    // Cast to NSView for layout registration; Obj-C dispatch still reaches
    // IdealystCanvasMacView's drawRect on the same pointer.
    let view_nsview: Retained<NSView> = unsafe { Retained::cast(view) };
    b.register_external_view(&view_nsview);
    let view_canvas: Retained<IdealystCanvasMacView> =
        unsafe { Retained::cast(view_nsview.clone()) };

    *view_canvas.ivars().sizing.borrow_mut() = Some(prim.size_reporter());

    // Reactive repaint. Realize runs world-entered, so this effect is
    // collected into the mounting subtree and dies at unmount.
    let view_for_effect = view_canvas.clone();
    let props_clone = props.clone();
    let paint_prim = prim.clone();
    runtime_world::effect(move || {
        let scene = paint_prim.paint();
        // Clone the layer descriptors (cheap — Rc closures); their sources are
        // resolved per `drawRect:` so the live camera + drag rect stay current.
        view_for_effect.install(scene, props_clone.layers.clone());
    });

    backend_macos::MacosNode::View(view_nsview)
}

// ============================================================================
// Tests — the shared CoreGraphics replay, rendered headlessly
// ============================================================================

/// These drive the REAL shared painter (`crate::apple`, the code both iOS and
/// macOS `drawRect:` run) into an offscreen `CGBitmapContext` set as AppKit's
/// current graphics context — exactly the setup `drawRect:` gets, minus the
/// window. They live here rather than in `tests/` because the painter is
/// crate-private, and in the macOS module because the host only runs macOS;
/// iOS's `UIBezierPath` vtable can't be exercised off-device, but the op
/// replay and texture compositing under test are the same code.
#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::{draw, CanvasProps, Fit, ImageSource, Path, Scene};
    use std::ffi::c_void;
    use std::rc::Rc;
    use std::sync::Arc;

    type CGColorSpaceRef = *mut c_void;
    extern "C" {
        fn CGColorSpaceCreateDeviceRGB() -> CGColorSpaceRef;
        fn CGColorSpaceRelease(cs: CGColorSpaceRef);
        fn CGBitmapContextCreate(
            data: *mut c_void,
            width: usize,
            height: usize,
            bits_per_component: usize,
            bytes_per_row: usize,
            space: CGColorSpaceRef,
            bitmap_info: u32,
        ) -> CGContextRef;
        fn CGContextRelease(c: CGContextRef);
        fn CGContextTranslateCTM(c: CGContextRef, tx: CGFloat, ty: CGFloat);
        fn CGContextScaleCTM(c: CGContextRef, sx: CGFloat, sy: CGFloat);
    }

    /// `kCGImageAlphaPremultipliedLast` — the RGBA8 render-target layout.
    const PREMULTIPLIED_LAST: u32 = 1;
    const SIZE: usize = 20;

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    const GREEN: [u8; 4] = [0, 255, 0, 255];

    /// Render `props` the way `IdealystCanvasMacView` does — `paint_scene`
    /// normalization, then the shared painter with the props' layers — into a
    /// `SIZE × SIZE` top-left-origin bitmap; returns RGBA rows, row 0 on top.
    fn render(props: &CanvasProps) -> Vec<u8> {
        let mut buf = vec![0u8; SIZE * SIZE * 4];
        unsafe {
            let cs = CGColorSpaceCreateDeviceRGB();
            let ctx = CGBitmapContextCreate(
                buf.as_mut_ptr() as *mut c_void,
                SIZE,
                SIZE,
                8,
                SIZE * 4,
                cs,
                PREMULTIPLIED_LAST,
            );
            CGColorSpaceRelease(cs);
            assert!(!ctx.is_null(), "bitmap context");
            // Top-left origin, like the flipped view's drawRect: CTM; buffer
            // row 0 is then the top scanline.
            CGContextTranslateCTM(ctx, 0.0, SIZE as CGFloat);
            CGContextScaleCTM(ctx, 1.0, -1.0);
            // NSBezierPath fill/stroke/addClip target the CURRENT AppKit
            // context, so wrap ours and make it current for the replay.
            let cls = AnyClass::get("NSGraphicsContext").expect("NSGraphicsContext");
            let gc: Retained<NSGraphicsContext> = msg_send_id![
                cls,
                graphicsContextWithCGContext: ctx as *mut CGContextOpaque,
                flipped: true
            ];
            NSGraphicsContext::saveGraphicsState_class();
            NSGraphicsContext::setCurrentContext(Some(&gc));
            let scene = canvas_core::paint_scene(props);
            painter().paint_scene(ctx, &scene, &props.layers);
            NSGraphicsContext::restoreGraphicsState_class();
            drop(gc);
            CGContextRelease(ctx);
        }
        buf
    }

    fn px(buf: &[u8], x: usize, y: usize) -> [u8; 4] {
        let i = (y * SIZE + x) * 4;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    fn assert_px(buf: &[u8], x: usize, y: usize, want: [u8; 4]) {
        let got = px(buf, x, y);
        let close = got.iter().zip(want).all(|(g, w)| (*g as i32 - w as i32).abs() <= 2);
        assert!(close, "pixel ({x},{y}) = {got:?}, want {want:?}");
    }

    /// An image texture layer of `w × h` pixels (`rgba`), drawn in `rect`.
    fn image_layer(
        w: u32,
        h: u32,
        rgba: Vec<u8>,
        rect: (f32, f32, f32, f32),
    ) -> canvas_core::TextureLayer {
        let img = Arc::new(ImageSource::from_rgba8(1, w, h, rgba));
        canvas_core::TextureLayer::image(Rc::new(move || Some(img.clone())), Rc::new(move || rect))
            .fit(Fit::Fill)
    }

    fn solid(c: [u8; 4]) -> Vec<u8> {
        c.to_vec()
    }

    fn fill(s: &mut Scene, x: f32, y: f32, w: f32, h: f32, c: [u8; 4]) {
        s.fill_path(Path::rect(x, y, w, h), Color::new(c[0], c[1], c[2], c[3]));
    }

    /// Regression: the CoreGraphics renderers composited every layer AFTER the
    /// scene, so vector content could never sit over a camera frame. A fill
    /// recorded after `s.texture(0)` must land on top of the texture.
    #[test]
    fn regression_fill_after_texture_op_draws_over_the_texture() {
        let props = CanvasProps {
            draw: draw(|s| {
                s.texture(0);
                fill(s, 0.0, 0.0, 10.0, 10.0, RED);
            }),
            layers: vec![image_layer(1, 1, solid(BLUE), (0.0, 0.0, 20.0, 20.0))],
            ..Default::default()
        };
        let buf = render(&props);
        assert_px(&buf, 5, 5, RED); // the fill, over the texture
        assert_px(&buf, 15, 15, BLUE); // texture where nothing covers it
    }

    /// A layer the scene never places still composites over all scene content
    /// (the pre-`Texture`-op behavior). On macOS this is also the parity fix:
    /// the CPU macOS renderer used to drop texture layers entirely.
    #[test]
    fn regression_unplaced_layer_composites_over_the_scene() {
        let props = CanvasProps {
            draw: draw(|s| fill(s, 0.0, 0.0, 20.0, 20.0, RED)),
            layers: vec![image_layer(1, 1, solid(BLUE), (0.0, 0.0, 10.0, 10.0))],
            ..Default::default()
        };
        let buf = render(&props);
        assert_px(&buf, 5, 5, BLUE);
        assert_px(&buf, 15, 15, RED);
    }

    /// The texture ignores the author's transform at its op (it is drawn in
    /// canvas coordinates), and the transform resumes after it.
    #[test]
    fn texture_ignores_the_author_transform_which_resumes_after_it() {
        let props = CanvasProps {
            draw: draw(|s| {
                s.translate(10.0, 10.0);
                s.texture(0);
                fill(s, 0.0, 0.0, 5.0, 5.0, RED); // lands at (10..15, 10..15)
            }),
            layers: vec![image_layer(1, 1, solid(BLUE), (0.0, 0.0, 10.0, 10.0))],
            ..Default::default()
        };
        let buf = render(&props);
        assert_px(&buf, 5, 5, BLUE); // NOT shifted by the translate
        assert_px(&buf, 12, 12, RED); // the fill IS translated
        assert_px(&buf, 17, 17, [0, 0, 0, 0]);
    }

    /// Images are top-row-first; the composite must not draw them upside-down
    /// in the top-left-origin context.
    #[test]
    fn texture_is_drawn_upright() {
        let rgba = [RED, GREEN].concat(); // 1×2: red top, green bottom
        let props = CanvasProps {
            layers: vec![image_layer(1, 2, rgba, (0.0, 0.0, 20.0, 20.0))],
            ..Default::default()
        };
        let buf = render(&props);
        assert_px(&buf, 10, 2, RED);
        assert_px(&buf, 10, 17, GREEN);
    }

    /// Regression: the CPU renderers ignored `src_crop`. Cropping the right
    /// half of a red|green image must show only green.
    #[test]
    fn regression_src_crop_is_honored() {
        // 20×1: ten red pixels then ten green. Several pixels per side so the
        // bilinear filter at the crop edge (it samples the neighboring source
        // pixel, as the GPU path's UV crop does) only touches the first canvas
        // column, not the pixels checked.
        let rgba = [vec![RED; 10], vec![GREEN; 10]].concat().concat();
        let layer =
            image_layer(20, 1, rgba, (0.0, 0.0, 20.0, 20.0)).src_crop((0.5, 0.0, 0.5, 1.0));
        let props = CanvasProps { layers: vec![layer], ..Default::default() };
        let buf = render(&props);
        assert_px(&buf, 2, 10, GREEN);
        assert_px(&buf, 17, 10, GREEN);
    }

    /// Regression: texture pixels are STRAIGHT alpha (`TextureLayer::resolve_rgba`),
    /// but the CoreGraphics texture image was created as
    /// `kCGImageAlphaPremultipliedLast`, so a translucent texel was composited
    /// as if its color were already multiplied by its alpha — too bright. A
    /// straight red at alpha 128 over black must land at about half red.
    ///
    /// The backdrop is black, not white, on purpose: over white, pure red
    /// gives (255,127,127) under BOTH readings (the misread's red channel
    /// saturates at 255 and green/blue are 0 in both), so it can't tell them
    /// apart. Black and the half-red-over-white case below separate them.
    #[test]
    fn regression_translucent_texture_is_composited_as_straight_alpha() {
        let half_red = [255, 0, 0, 128];
        let props = CanvasProps {
            draw: draw(|s| fill(s, 0.0, 0.0, 20.0, 20.0, [0, 0, 0, 255])),
            layers: vec![image_layer(1, 1, solid(half_red), (0.0, 0.0, 20.0, 20.0))],
            ..Default::default()
        };
        let buf = render(&props);
        // Straight: 255 × 128/255 + 0 = 128. Premultiplied misread: 255.
        assert_px(&buf, 10, 10, [128, 0, 0, 255]);

        // Over white with a color whose misread does not saturate: straight
        // (128,0,0,128) → 128×0.502 + 255×0.498 ≈ (191,127,127); the misread
        // gives 128 + 127 = 255 in red.
        let props = CanvasProps {
            draw: draw(|s| fill(s, 0.0, 0.0, 20.0, 20.0, [255, 255, 255, 255])),
            layers: vec![image_layer(1, 1, solid([128, 0, 0, 128]), (0.0, 0.0, 20.0, 20.0))],
            ..Default::default()
        };
        let buf = render(&props);
        assert_px(&buf, 10, 10, [191, 127, 127, 255]);
    }

    /// `Fit::Contain` letterboxes: outside the fitted rect nothing is drawn.
    #[test]
    fn contain_fit_leaves_the_letterbox_empty() {
        // 2×1 source into a 20×20 rect → 20×10 band at y 5..15.
        let layer = image_layer(2, 1, [BLUE, BLUE].concat(), (0.0, 0.0, 20.0, 20.0))
            .fit(Fit::Contain);
        let props = CanvasProps { layers: vec![layer], ..Default::default() };
        let buf = render(&props);
        assert_px(&buf, 10, 2, [0, 0, 0, 0]);
        assert_px(&buf, 10, 10, BLUE);
        assert_px(&buf, 10, 17, [0, 0, 0, 0]);
    }
}
