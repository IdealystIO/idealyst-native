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
//! The op-replay itself lives in the shared [`crate::apple`] painter
//! (identical CoreGraphics calls on iOS + macOS). This module owns only
//! the iOS-specific glue: the `UIView` subclass, `UIGraphicsGetCurrent
//! Context()` acquisition, and the `UIBezierPath` + `UIColor` vtable.
//! Canvas coordinates are logical points, top-left origin — UIKit's
//! `drawRect:` CTM already matches, so no axis flip is needed.

use backend_ios::{IosBackend, IosNode};
use canvas_core::{CanvasPrim, Color, TextureLayer};
use runtime_scene::{Element, MountCx};

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, NSObject};
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{CGFloat, CGPoint, CGRect, CGSize, MainThreadMarker};
use objc2_ui_kit::UIView;

use std::cell::RefCell;
#[cfg(target_abi = "sim")]
use std::ffi::c_void;
use std::rc::Rc;

// Self-capture (recording) is a CPU read-back path. On iOS it's compiled ONLY
// for the Simulator (`target_abi = "sim"`), where vello can't run (its Metal
// lacks INDIRECT_EXECUTION) so canvas-native is the active renderer. On real
// devices vello owns the canvas and captures on-GPU, so none of this compiles.
#[cfg(target_abi = "sim")]
use canvas_core::FrameWriter;
#[cfg(target_abi = "sim")]
use std::sync::atomic::{AtomicBool, Ordering};

use crate::apple::{ApplePainter, CGContextRef};

extern "C" {
    fn UIGraphicsGetCurrentContext() -> CGContextRef;
}

// Offscreen-rasterization bindings for the Simulator-only CPU self-capture path.
#[cfg(target_abi = "sim")]
type CGColorSpaceRef = *mut c_void;

/// `kCGImageAlphaPremultipliedLast | kCGBitmapByteOrderDefault` — RGBA byte
/// order, alpha last: the RGBA8 layout a `CGBitmapContext` render target
/// supports.
#[cfg(target_abi = "sim")]
const RGBA_BITMAP_INFO: u32 = 1;

#[cfg(target_abi = "sim")]
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
    fn UIGraphicsPushContext(ctx: CGContextRef);
    fn UIGraphicsPopContext();
}

// ============================================================================
// Painter vtable — UIBezierPath + UIColor
// ============================================================================

/// Build the iOS painter vtable: `UIBezierPath` class + `UIColor` factory.
fn painter() -> ApplePainter {
    ApplePainter {
        bezier_class: objc2::class!(UIBezierPath),
        make_color: ui_color,
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

// ============================================================================
// View subclass
// ============================================================================

pub(crate) struct CanvasViewIvars {
    /// The current scene to replay. `RefCell` so the Effect closure can
    /// swap it without `&mut self`.
    scene: RefCell<canvas_core::Scene>,
    /// Texture layers (camera, …) the scene's `DrawOp::Texture` ops composite,
    /// installed together with the scene so the indices agree. Their
    /// `source`/`rect` closures are re-evaluated per paint so a live camera and
    /// a reactive drag position both follow.
    layers: RefCell<Vec<TextureLayer>>,
    /// One throwaway CPU-frame subscription per active layer, so a camera
    /// producer keeps feeding the frames our `latest()` pull reads (see
    /// [`canvas_core::sync_layer_subscriptions`]).
    layer_subs: RefCell<Vec<Option<canvas_core::Subscription>>>,
    /// Reports the view's bounds to the canvas (`Scene::size`). Set at
    /// mount; read on every `drawRect:`, which runs whenever the bounds change.
    sizing: RefCell<Option<canvas_core::SizeReporter>>,
    /// Self-capture sink (iOS Simulator only — the CPU recording fallback). On a
    /// real device vello owns the canvas + its GPU capture, so this isn't stored.
    #[cfg(target_abi = "sim")]
    capture: RefCell<Option<FrameWriter>>,
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
        let this = this.set_ivars(CanvasViewIvars {
            scene: RefCell::new(canvas_core::Scene::new()),
            layers: RefCell::new(Vec::new()),
            layer_subs: RefCell::new(Vec::new()),
            sizing: RefCell::new(None),
            #[cfg(target_abi = "sim")]
            capture: RefCell::new(None),
        });
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

    /// Swap the scene + layers and invalidate so UIKit re-runs `drawRect:`.
    fn install(&self, scene: canvas_core::Scene, layers: Vec<TextureLayer>) {
        // Keep CPU-frame subscriptions in step with the live layers so a camera
        // producer keeps delivering frames to `latest()` (UI-thread only).
        canvas_core::sync_layer_subscriptions(&layers, &mut self.ivars().layer_subs.borrow_mut());
        *self.ivars().scene.borrow_mut() = scene;
        *self.ivars().layers.borrow_mut() = layers;
        let _: () = unsafe { msg_send![self, setNeedsDisplay] };
    }

    /// Replay the cached scene (texture layers composite at their op positions).
    fn paint_now(&self) {
        // `drawRect:` runs whenever the bounds change (contentMode = Redraw),
        // so it is where this view learns its size. The reporter dedupes.
        if let Some(sizing) = &*self.ivars().sizing.borrow() {
            let bounds: CGRect = unsafe { msg_send![self, bounds] };
            sizing.report(bounds.size.width as f32, bounds.size.height as f32);
        }
        let ctx = unsafe { UIGraphicsGetCurrentContext() };
        if ctx.is_null() {
            return;
        }
        let scene = self.ivars().scene.borrow();
        // Texture layers composite at their `DrawOp::Texture` positions inside
        // the replay (see `ApplePainter::paint_scene`), not after it.
        painter().paint_scene(ctx, &scene, &self.ivars().layers.borrow());
        // Simulator-only: while recording, re-rasterize offscreen and read back.
        #[cfg(target_abi = "sim")]
        self.capture_frame_if_recording(&scene);
    }

    /// Store the self-capture sink (iOS Simulator only).
    #[cfg(target_abi = "sim")]
    fn set_capture(&self, writer: Option<FrameWriter>) {
        *self.ivars().capture.borrow_mut() = writer;
    }

    /// While a recorder is tapping the capture stream, re-render the scene +
    /// layers into an offscreen RGBA bitmap and push it to the `FrameWriter`
    /// (self-capture). The iOS canvas paints straight into the on-screen
    /// `drawRect:` context, which has no readable backing buffer — so recording
    /// needs this second, offscreen rasterization. Simulator-only: it's the CPU
    /// fallback for when vello (which would capture on-GPU, zero re-render) can't
    /// run. Gated on `wants_cpu_frames` so a non-recording canvas does nothing.
    #[cfg(target_abi = "sim")]
    fn capture_frame_if_recording(&self, scene: &canvas_core::Scene) {
        let writer = match self.ivars().capture.borrow().as_ref() {
            Some(w) if w.wants_cpu_frames() => w.clone(),
            _ => return,
        };

        // Announce the slow path ONCE so a developer recording on the simulator
        // knows why it's sluggish and to validate perf on a real device. The
        // `log` crate facade isn't routed to the iOS console, so use NSLog.
        static LOGGED: AtomicBool = AtomicBool::new(false);
        if !LOGGED.swap(true, Ordering::Relaxed) {
            backend_ios_core::ios_log(
                "[canvas] recording via the CoreGraphics CPU renderer (iOS Simulator \
                 fallback — vello can't run here). Expect SEVERE performance loss; \
                 record on a physical device for representative performance.",
            );
        }

        let bounds: CGRect = unsafe { msg_send![self, bounds] };
        let scale: CGFloat = unsafe { msg_send![self, contentScaleFactor] };
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let w_px = (bounds.size.width * scale).round() as usize;
        let h_px = (bounds.size.height * scale).round() as usize;
        if w_px == 0 || h_px == 0 {
            return;
        }

        let mut buf = vec![0u8; w_px * h_px * 4];
        // SAFETY: `buf` outlives the context (released below, before `write_rgba8`
        // reads it). Every CG object created here is released here. The bitmap
        // context is pushed as the current UIGraphics context so the painter's
        // `UIBezierPath.fill/stroke` (which target the *current* context) AND the
        // explicit-`ctx` CGContext calls both land in `buf`.
        unsafe {
            let cs = CGColorSpaceCreateDeviceRGB();
            let ctx = CGBitmapContextCreate(
                buf.as_mut_ptr() as *mut c_void,
                w_px,
                h_px,
                8,
                w_px * 4,
                cs,
                RGBA_BITMAP_INFO,
            );
            if ctx.is_null() {
                CGColorSpaceRelease(cs);
                return;
            }
            // A fresh CGBitmapContext has a bottom-left origin; flip to top-left
            // and scale logical points → device pixels (the same setup
            // `UIGraphicsBeginImageContext` applies). After this, buffer row 0 is
            // the TOP scanline — the order `write_rgba8` expects.
            CGContextTranslateCTM(ctx, 0.0, h_px as CGFloat);
            CGContextScaleCTM(ctx, scale, -scale);

            UIGraphicsPushContext(ctx);
            // The same replay as `paint_now`, so the recording matches the
            // screen (textures at their op positions).
            painter().paint_scene(ctx, scene, &self.ivars().layers.borrow());
            UIGraphicsPopContext();

            CGContextRelease(ctx);
            CGColorSpaceRelease(cs);
        }

        writer.write_rgba8(w_px as u32, h_px as u32, &buf);
    }
}

// ============================================================================
// register + build
// ============================================================================

pub(crate) fn mount_canvas(
    cx: &mut MountCx<'_, IosBackend>,
    prim: &Rc<CanvasPrim>,
    _children: Vec<Element>,
) -> IosNode {
    let backend = cx.backend().clone();
    let node = {
        let mut b = backend.borrow_mut();
        build_canvas(prim, &mut b)
    };
    crate::finish_mount(&backend, &node, prim);
    node
}

fn build_canvas(prim: &Rc<CanvasPrim>, b: &mut IosBackend) -> IosNode {
    let props = &prim.props;
    let view = IdealystCanvasView::new(b.mtm());
    // Cast to UIView for layout registration; Obj-C dispatch still reaches
    // IdealystCanvasView's drawRect on the same pointer.
    let view_uiview: Retained<UIView> = unsafe { Retained::cast(view) };
    b.register_external_view(&view_uiview);
    let view_canvas: Retained<IdealystCanvasView> = unsafe { Retained::cast(view_uiview.clone()) };

    // Simulator-only: hand the view the self-capture sink so `drawRect:` can read
    // frames back for recording (on a device vello captures on-GPU instead).
    #[cfg(target_abi = "sim")]
    view_canvas.set_capture(props.capture.clone());

    *view_canvas.ivars().sizing.borrow_mut() = Some(prim.size_reporter());

    let view_for_effect = view_canvas.clone();
    let props_clone = props.clone();
    let paint_prim = prim.clone();
    // Reactive repaint. Realize runs world-entered, so this effect is
    // collected into the mounting subtree and dies at unmount.
    runtime_world::effect(move || {
        let scene = paint_prim.paint();
        // Clone the layer descriptors (cheap — Rc closures); their sources are
        // resolved per `drawRect:` so the live camera + drag rect stay current.
        view_for_effect.install(scene, props_clone.layers.clone());
    });

    IosNode::View(view_uiview)
}
