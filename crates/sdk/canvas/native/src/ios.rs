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
use canvas_core::{CanvasProps, Color, TextureLayer};
use runtime_core::effect;

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, NSObject};
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{CGFloat, CGPoint, CGRect, CGSize, MainThreadMarker};
use objc2_ui_kit::UIView;

use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr::{null, null_mut};
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
extern "C" {
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
// CoreGraphics bindings for CPU texture-layer compositing (camera-in-canvas)
// ============================================================================

/// Opaque `CGImage`. A pointer to it (`CGImageRef`) encodes as `^{CGImage=}`,
/// which is what `+[UIImage imageWithCGImage:]`'s runtime signature expects —
/// passing a bare `*mut c_void` (`^v`) would trip objc2's encoding check.
#[repr(C)]
struct CGImageOpaque {
    _private: [u8; 0],
}
// `RefEncode` (not `Encode`): it's only ever used behind a pointer, and objc2's
// blanket impl gives `*mut CGImageOpaque` an `Encode` of `^{CGImage=}` from this.
unsafe impl objc2::RefEncode for CGImageOpaque {
    const ENCODING_REF: objc2::Encoding =
        objc2::Encoding::Pointer(&objc2::Encoding::Struct("CGImage", &[]));
}
type CGImageRef = *mut CGImageOpaque;

type CGDataProviderRef = *mut c_void;
type CGColorSpaceRef = *mut c_void;

/// `kCGImageAlphaPremultipliedLast | kCGBitmapByteOrderDefault` — RGBA byte
/// order, alpha last. Camera frames are opaque, so premultiplied vs straight is
/// moot; this is the widely-supported combination for 8-bit RGBA.
const RGBA_BITMAP_INFO: u32 = 1;

extern "C" {
    fn CGColorSpaceCreateDeviceRGB() -> CGColorSpaceRef;
    fn CGColorSpaceRelease(cs: CGColorSpaceRef);
    fn CGDataProviderCreateWithData(
        info: *mut c_void,
        data: *const c_void,
        size: usize,
        release: *const c_void,
    ) -> CGDataProviderRef;
    fn CGDataProviderRelease(p: CGDataProviderRef);
    #[allow(clippy::too_many_arguments)]
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
        intent: u32,
    ) -> CGImageRef;
    fn CGImageCreateWithImageInRect(image: CGImageRef, rect: CGRect) -> CGImageRef;
    fn CGImageRelease(image: CGImageRef);
    fn CGContextSaveGState(c: CGContextRef);
    fn CGContextRestoreGState(c: CGContextRef);
    fn CGContextSetAlpha(c: CGContextRef, alpha: CGFloat);
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
    /// Texture layers (camera, …) composited over the scene each `drawRect:`.
    /// Their `source`/`rect` closures are re-evaluated per paint so a live
    /// camera and a reactive drag position both follow.
    layers: RefCell<Vec<TextureLayer>>,
    /// One throwaway CPU-frame subscription per active layer, so a camera
    /// producer keeps feeding the frames our `latest()` pull reads (see
    /// [`canvas_core::sync_layer_subscriptions`]).
    layer_subs: RefCell<Vec<Option<canvas_core::Subscription>>>,
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

    /// Replay the cached scene, then composite the texture layers over it.
    fn paint_now(&self) {
        let ctx = unsafe { UIGraphicsGetCurrentContext() };
        if ctx.is_null() {
            return;
        }
        let scene = self.ivars().scene.borrow();
        painter().paint_scene(ctx, &scene);
        for layer in self.ivars().layers.borrow().iter() {
            composite_layer(ctx, layer);
        }
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
            painter().paint_scene(ctx, scene);
            for layer in self.ivars().layers.borrow().iter() {
                composite_layer(ctx, layer);
            }
            UIGraphicsPopContext();

            CGContextRelease(ctx);
            CGColorSpaceRelease(cs);
        }

        writer.write_rgba8(w_px as u32, h_px as u32, &buf);
    }
}

/// Composite one [`TextureLayer`] over the canvas: pull the stream's latest RGBA
/// frame, wrap it as a `CGImage`, crop to the source rect, and draw it into a
/// rounded, alpha-blended destination rect using the shared
/// [`canvas_core::Fit::map_rects`] geometry. Mirrors the web/Android paths.
/// No-op when the stream has no frame yet.
fn composite_layer(ctx: CGContextRef, layer: &TextureLayer) {
    let Some(stream) = (layer.source)() else { return };
    let mut rgba: Vec<u8> = Vec::new();
    let Some((vw, vh)) = stream.latest(&mut rgba) else { return };
    if vw == 0 || vh == 0 || rgba.len() < (vw as usize) * (vh as usize) * 4 {
        return;
    }
    let (dx, dy, dw, dh) = (layer.rect)();
    if dw < 1.0 || dh < 1.0 {
        return;
    }
    let ((sx, sy, sw, sh), (ox, oy, ow, oh)) =
        layer.fit.map_rects(vw as f32, vh as f32, dx, dy, dw, dh);

    // SAFETY: `rgba` outlives the whole block; the data provider references it
    // without copying and every pixel read happens inside `drawInRect:` below,
    // before `rgba` is dropped. All CG objects created here are released here.
    unsafe {
        let cs = CGColorSpaceCreateDeviceRGB();
        let provider = CGDataProviderCreateWithData(
            null_mut(),
            rgba.as_ptr() as *const c_void,
            rgba.len(),
            null(),
        );
        let img = CGImageCreate(
            vw as usize,
            vh as usize,
            8,
            32,
            (vw as usize) * 4,
            cs,
            RGBA_BITMAP_INFO,
            provider,
            null(),
            false,
            0,
        );
        CGColorSpaceRelease(cs);
        CGDataProviderRelease(provider);
        if img.is_null() {
            return;
        }
        // Crop the source to the fit rect, then release the full image.
        let src = CGRect::new(
            CGPoint::new(sx as CGFloat, sy as CGFloat),
            CGSize::new(sw as CGFloat, sh as CGFloat),
        );
        let cropped = CGImageCreateWithImageInRect(img, src);
        CGImageRelease(img);
        if cropped.is_null() {
            return;
        }

        let dst = CGRect::new(
            CGPoint::new(ox as CGFloat, oy as CGFloat),
            CGSize::new(ow as CGFloat, oh as CGFloat),
        );

        CGContextSaveGState(ctx);
        CGContextSetAlpha(ctx, layer.opacity.clamp(0.0, 1.0) as CGFloat);
        // Round the drawn (letterboxed for Contain) rect so corners clip the image.
        let r = layer.corner_radius.clamp(0.0, ow.min(oh) * 0.5) as CGFloat;
        if r > 0.0 {
            if let Some(cls) = AnyClass::get("UIBezierPath") {
                let path: Retained<NSObject> =
                    msg_send_id![cls, bezierPathWithRoundedRect: dst, cornerRadius: r];
                let _: () = msg_send![&path, addClip];
            }
        }
        // CGImage → UIImage → drawInRect: so UIKit handles the top-left
        // orientation (a raw CGContextDrawImage would render flipped here).
        if let Some(uiimage_cls) = AnyClass::get("UIImage") {
            let image: Retained<NSObject> = msg_send_id![uiimage_cls, imageWithCGImage: cropped];
            let _: () = msg_send![&image, drawInRect: dst];
        }
        CGImageRelease(cropped);
        CGContextRestoreGState(ctx);
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

// Self-register at backend construction (no app-side `register` call needed).
// See [[project_inventory_self_registration]].
inventory::submit! {
    backend_ios::IosExternalRegistrar(register)
}

fn build_canvas(props: &Rc<CanvasProps>, b: &mut IosBackend) -> IosNode {
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

    let view_for_effect = view_canvas.clone();
    let props_clone = props.clone();
    effect!({
        let scene = canvas_core::paint_scene(&props_clone);
        // Clone the layer descriptors (cheap — Rc closures); their sources are
        // resolved per `drawRect:` so the live camera + drag rect stay current.
        view_for_effect.install(scene, props_clone.layers.clone());
    });

    IosNode::View(view_uiview)
}
