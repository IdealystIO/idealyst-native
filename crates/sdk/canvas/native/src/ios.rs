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
use canvas_core::{CanvasProps, Color};
use runtime_core::Effect;

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, NSObject};
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{CGFloat, CGPoint, CGRect, CGSize, MainThreadMarker};
use objc2_ui_kit::UIView;

use std::cell::RefCell;
use std::rc::Rc;

use crate::apple::{ApplePainter, CGContextRef};

extern "C" {
    fn UIGraphicsGetCurrentContext() -> CGContextRef;
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
        painter().paint_scene(ctx, &scene);
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
