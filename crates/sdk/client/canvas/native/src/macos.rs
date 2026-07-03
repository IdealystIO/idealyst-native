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
use canvas_core::{CanvasProps, Color};
use runtime_core::effect;

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
        let this =
            this.set_ivars(CanvasViewIvars { scene: RefCell::new(canvas_core::Scene::new()) });
        let this: Retained<Self> = unsafe { msg_send_id![super(this), init] };
        // Layer-backed so AppKit invalidates + repaints the whole view on
        // resize; transparent so see-through regions show the parent.
        let _: () = unsafe { msg_send![&*this, setWantsLayer: true] };
        this
    }

    /// Swap the scene and invalidate so AppKit re-runs `drawRect:`.
    fn install_scene(&self, scene: canvas_core::Scene) {
        *self.ivars().scene.borrow_mut() = scene;
        let _: () = unsafe { msg_send![self, setNeedsDisplay: true] };
    }

    /// Replay the cached scene into the active `CGContext` from
    /// `NSGraphicsContext.currentContext`.
    fn paint_now(&self) {
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
        painter().paint_scene(ctx as CGContextRef, &scene);
    }
}

// ============================================================================
// register + build
// ============================================================================

/// Register the macOS canvas renderer against a `MacosBackend`.
pub fn register(backend: &mut MacosBackend) {
    canvas_core::ensure_wire_serde();
    backend.register_external::<CanvasProps, _>(|props, b| build_canvas(props, b));
}

// Self-register at backend construction (no app-side `register` call needed).
// See [[project_inventory_self_registration]].
inventory::submit! {
    backend_macos::MacosExternalRegistrar(register)
}

fn build_canvas(
    props: &std::rc::Rc<CanvasProps>,
    b: &mut MacosBackend,
) -> backend_macos::MacosNode {
    let view = IdealystCanvasMacView::new(b.mtm());
    // Cast to NSView for layout registration; Obj-C dispatch still reaches
    // IdealystCanvasMacView's drawRect on the same pointer.
    let view_nsview: Retained<NSView> = unsafe { Retained::cast(view) };
    b.register_external_view(&view_nsview);
    let view_canvas: Retained<IdealystCanvasMacView> =
        unsafe { Retained::cast(view_nsview.clone()) };

    let view_for_effect = view_canvas.clone();
    let props_clone = props.clone();
    effect!({
        let scene = canvas_core::paint_scene(&props_clone);
        view_for_effect.install_scene(scene);
    });

    backend_macos::MacosNode::View(view_nsview)
}
