//! `Primitive::Icon` — vector icon rendering on iOS.
//!
//! Two rendering strategies depending on context:
//!
//! 1. **Standalone `icon()` primitive** → `CAShapeLayer` in a UIView.
//!    Vector, resolution-independent, instant color changes via
//!    `strokeColor`, animatable, zero rasterization cost.
//!
//! 2. **Native UIKit controls** (nav bar, tab bar, UIButton) →
//!    `UIImage` rendered from paths via `UIGraphicsImageRenderer`.
//!    Cached by icon identity + size so repeated use is free.
//!    Template rendering mode lets UIKit apply tintColor.
//!
//! `create_icon` uses strategy 1. Strategy 2 is exposed as
//! `render_to_uiimage` for use by the navigator/tab implementations.
//!
//! Both strategies share the same SVG-path parser from
//! [`backend_apple_core::icon_path`]; the iOS-specific code here
//! is just the UIBezierPath adapter for the parser's emitter trait
//! plus the CAShapeLayer / UIGraphicsImageRenderer wiring.

use runtime_core::primitives::icon::{FillRule, IconData};
use runtime_core::Color;
use objc2::msg_send;
use objc2::msg_send_id;
use objc2::rc::Retained;
use objc2_foundation::{CGFloat, CGPoint, CGRect, CGSize, MainThreadMarker, NSObject, NSString};
use objc2_ui_kit::{UIColor, UIView};

use backend_apple_core::icon_path::{parse_svg_path, PathEmitter};
use backend_ios_core::style::color_to_uicolor;
use super::IosNode;

// =========================================================================
// UIBezierPath PathEmitter adapter — bridges the shared
// apple/core SVG parser to UIKit's path-construction selectors.
// macOS uses a parallel CGPath emitter (see backend-macos/imp/icon.rs);
// the parser itself is identical for both.
// =========================================================================

/// Wraps a `UIBezierPath` (held by-reference) so the shared SVG
/// parser can append path commands by calling `PathEmitter` trait
/// methods. The lifetime mirrors a `&'a NSObject` borrow — the
/// emitter doesn't outlive the bezier path's owning scope.
struct UIBezierEmitter<'a> {
    bezier: &'a NSObject,
}

impl<'a> UIBezierEmitter<'a> {
    fn new(bezier: &'a NSObject) -> Self {
        Self { bezier }
    }
}

impl<'a> PathEmitter for UIBezierEmitter<'a> {
    fn move_to(&mut self, x: f64, y: f64) {
        let pt = CGPoint::new(x as CGFloat, y as CGFloat);
        let _: () = unsafe { msg_send![self.bezier, moveToPoint: pt] };
    }

    fn line_to(&mut self, x: f64, y: f64) {
        let pt = CGPoint::new(x as CGFloat, y as CGFloat);
        let _: () = unsafe { msg_send![self.bezier, addLineToPoint: pt] };
    }

    fn curve_to(&mut self, c1x: f64, c1y: f64, c2x: f64, c2y: f64, x: f64, y: f64) {
        let cp1 = CGPoint::new(c1x as CGFloat, c1y as CGFloat);
        let cp2 = CGPoint::new(c2x as CGFloat, c2y as CGFloat);
        let pt = CGPoint::new(x as CGFloat, y as CGFloat);
        let _: () = unsafe {
            msg_send![
                self.bezier,
                addCurveToPoint: pt
                controlPoint1: cp1
                controlPoint2: cp2
            ]
        };
    }

    fn quad_to(&mut self, cx: f64, cy: f64, x: f64, y: f64) {
        // UIBezierPath has a native quadratic — route through
        // `addQuadCurveToPoint:controlPoint:` so we skip the
        // parser's default cubic lift and let UIKit handle the
        // curve directly.
        let cp = CGPoint::new(cx as CGFloat, cy as CGFloat);
        let pt = CGPoint::new(x as CGFloat, y as CGFloat);
        let _: () = unsafe {
            msg_send![self.bezier, addQuadCurveToPoint: pt controlPoint: cp]
        };
    }

    fn close(&mut self) {
        let _: () = unsafe { msg_send![self.bezier, closePath] };
    }
}

// ==========================================================================
// Strategy 1: CAShapeLayer (standalone icon primitive)
// ==========================================================================

/// Create a UIView with a CAShapeLayer sublayer rendering the icon paths.
/// Color is applied via `strokeColor` — instant, no rasterization.
pub(crate) fn create_icon(
    mtm: MainThreadMarker,
    data: &IconData,
    color: Option<&Color>,
) -> IosNode {
    let size: CGFloat = 24.0;
    let (vw, vh) = data.view_box;
    let sx = size / vw as CGFloat;
    let sy = size / vh as CGFloat;

    let view = unsafe { UIView::new(mtm) };
    let _: () = unsafe {
        msg_send![&view, setTranslatesAutoresizingMaskIntoConstraints: false]
    };

    // Default 24x24 size constraints (priority 750 — style can override).
    let width_anchor: Retained<NSObject> = unsafe { msg_send_id![&view, widthAnchor] };
    let height_anchor: Retained<NSObject> = unsafe { msg_send_id![&view, heightAnchor] };
    let w_c: Retained<NSObject> =
        unsafe { msg_send_id![&width_anchor, constraintEqualToConstant: size] };
    let h_c: Retained<NSObject> =
        unsafe { msg_send_id![&height_anchor, constraintEqualToConstant: size] };
    let _: () = unsafe { msg_send![&w_c, setPriority: 750.0f32] };
    let _: () = unsafe { msg_send![&h_c, setPriority: 750.0f32] };
    let _: () = unsafe { msg_send![&w_c, setActive: true] };
    let _: () = unsafe { msg_send![&h_c, setActive: true] };

    // Build UIBezierPath from icon path data via the shared
    // apple/core SVG parser. The same parser drives macOS's
    // CGPath-backed path so iOS + macOS render identical icons
    // without parser duplication.
    let bezier: Retained<NSObject> = unsafe {
        let cls = objc2::class!(UIBezierPath);
        msg_send_id![cls, bezierPath]
    };
    {
        let mut emitter = UIBezierEmitter::new(&bezier);
        for path_d in data.paths {
            parse_svg_path(path_d, sx as f64, sy as f64, &mut emitter);
        }
    }

    // Create CAShapeLayer.
    let shape_layer: Retained<NSObject> = unsafe {
        let cls = objc2::class!(CAShapeLayer);
        msg_send_id![cls, new]
    };

    // Set path.
    let cg_path: *const std::ffi::c_void = unsafe { msg_send![&bezier, CGPath] };
    let _: () = unsafe { msg_send![&shape_layer, setPath: cg_path] };

    // Stroke color.
    let stroke_color = match color {
        Some(c) => color_to_uicolor(c),
        None => unsafe {
            let cls = objc2::class!(UIColor);
            msg_send_id![cls, labelColor]
        },
    };
    let cg_stroke: *const std::ffi::c_void =
        unsafe { msg_send![&stroke_color, CGColor] };
    let _: () = unsafe { msg_send![&shape_layer, setStrokeColor: cg_stroke] };

    // No fill — stroke-only (Lucide / outlined icon style).
    let clear: Retained<UIColor> = unsafe { UIColor::clearColor() };
    let cg_clear: *const std::ffi::c_void = unsafe { msg_send![&clear, CGColor] };
    let _: () = unsafe { msg_send![&shape_layer, setFillColor: cg_clear] };

    // Stroke width scaled to target size.
    let line_width: CGFloat = 2.0 * sx;
    let _: () = unsafe { msg_send![&shape_layer, setLineWidth: line_width] };

    // Round line caps and joins.
    let round = NSString::from_str("round");
    let _: () = unsafe { msg_send![&shape_layer, setLineCap: &*round] };
    let _: () = unsafe { msg_send![&shape_layer, setLineJoin: &*round] };

    // Fill rule.
    if data.fill_rule == FillRule::EvenOdd {
        let rule = NSString::from_str("even-odd");
        let _: () = unsafe { msg_send![&shape_layer, setFillRule: &*rule] };
    }

    // Layer frame.
    let frame = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(size, size));
    let _: () = unsafe { msg_send![&shape_layer, setFrame: frame] };

    // Add as sublayer.
    let layer: Retained<NSObject> = unsafe { msg_send_id![&view, layer] };
    let _: () = unsafe { msg_send![&layer, addSublayer: &*shape_layer] };

    IosNode::View(view)
}

/// Update the icon's stroke color. Grabs the first sublayer (our
/// CAShapeLayer) and sets `strokeColor`.
pub(crate) fn update_icon_color(node: &IosNode, color: &Color) {
    if let Some(shape) = get_shape_layer(node) {
        let ui_color = color_to_uicolor(color);
        let cg_color: *const std::ffi::c_void =
            unsafe { msg_send![&ui_color, CGColor] };
        let _: () = unsafe { msg_send![&shape, setStrokeColor: cg_color] };
    }
}

/// Set stroke progress immediately (no animation).
/// Maps to `CAShapeLayer.strokeEnd`.
pub(crate) fn update_icon_stroke(node: &IosNode, progress: f32) {
    if let Some(shape) = get_shape_layer(node) {
        let val = progress.clamp(0.0, 1.0) as CGFloat;
        let _: () = unsafe { msg_send![&shape, setStrokeEnd: val] };
    }
}

/// Animate strokeEnd from→to using CABasicAnimation.
pub(crate) fn animate_icon_stroke(
    node: &IosNode,
    from: f32,
    to: f32,
    duration_ms: u32,
    easing: runtime_core::Easing,
    infinite: bool,
    autoreverses: bool,
) {
    let Some(shape) = get_shape_layer(node) else { return };

    let from_val = from.clamp(0.0, 1.0) as CGFloat;
    let to_val = to.clamp(0.0, 1.0) as CGFloat;
    let duration = duration_ms as CGFloat / 1000.0;

    // Create CABasicAnimation for "strokeEnd".
    let key_path = NSString::from_str("strokeEnd");
    let anim: Retained<NSObject> = unsafe {
        let cls = objc2::class!(CABasicAnimation);
        msg_send_id![cls, animationWithKeyPath: &*key_path]
    };

    // Set from/to values.
    let from_num: Retained<NSObject> = unsafe {
        let cls = objc2::class!(NSNumber);
        msg_send_id![cls, numberWithDouble: from_val as f64]
    };
    let to_num: Retained<NSObject> = unsafe {
        let cls = objc2::class!(NSNumber);
        msg_send_id![cls, numberWithDouble: to_val as f64]
    };
    let _: () = unsafe { msg_send![&anim, setFromValue: &*from_num] };
    let _: () = unsafe { msg_send![&anim, setToValue: &*to_num] };
    let _: () = unsafe { msg_send![&anim, setDuration: duration] };

    // Timing function.
    let timing = easing_to_timing_function(easing);
    let _: () = unsafe { msg_send![&anim, setTimingFunction: &*timing] };

    if infinite {
        let _: () = unsafe { msg_send![&anim, setRepeatCount: f32::INFINITY] };
        let _: () = unsafe { msg_send![&anim, setAutoreverses: autoreverses] };
    }

    // Keep the final value after animation completes.
    let _: () = unsafe { msg_send![&anim, setFillMode: &*NSString::from_str("both")] };
    let _: () = unsafe { msg_send![&anim, setRemovedOnCompletion: false] };

    // Begin time: use CACurrentMediaTime so the animation starts on
    // the next commit even if the layer was just added to the tree.
    extern "C" { fn CACurrentMediaTime() -> f64; }
    let now: f64 = unsafe { CACurrentMediaTime() };
    let _: () = unsafe { msg_send![&anim, setBeginTime: now] };

    // Set the model value to `to` so it persists after animation removal.
    let _: () = unsafe { msg_send![&shape, setStrokeEnd: to_val] };

    // Add animation.
    let anim_key = NSString::from_str("strokeEndAnim");
    let _: () = unsafe { msg_send![&shape, addAnimation: &*anim forKey: &*anim_key] };
}

/// Get the CAShapeLayer (first sublayer) from an icon node.
fn get_shape_layer(node: &IosNode) -> Option<Retained<NSObject>> {
    let view = node.as_view();
    let layer: Retained<NSObject> = unsafe { msg_send_id![view, layer] };
    let sublayers: *const NSObject = unsafe { msg_send![&layer, sublayers] };
    if sublayers.is_null() {
        return None;
    }
    let count: usize = unsafe { msg_send![sublayers, count] };
    if count == 0 {
        return None;
    }
    let shape: Retained<NSObject> = unsafe { msg_send_id![sublayers, firstObject] };
    Some(shape)
}

fn easing_to_timing_function(easing: runtime_core::Easing) -> Retained<NSObject> {
    let name = match easing {
        runtime_core::Easing::Linear => "linear",
        runtime_core::Easing::Ease => "default",
        runtime_core::Easing::EaseIn => "easeIn",
        runtime_core::Easing::EaseOut => "easeOut",
        runtime_core::Easing::EaseInOut => "easeInEaseOut",
        runtime_core::Easing::CubicBezier(_, _, _, _) => "default",
    };
    let ns_name = NSString::from_str(name);
    unsafe {
        let cls = objc2::class!(CAMediaTimingFunction);
        msg_send_id![cls, functionWithName: &*ns_name]
    }
}

// ==========================================================================
// IconHandle / IconOps for iOS
// ==========================================================================

use runtime_core::primitives::icon::{IconHandle, IconOps};

pub(crate) fn make_handle(node: &IosNode) -> IconHandle {
    let view: Retained<UIView> = Retained::clone(match node {
        IosNode::View(v) => v,
        _ => return IconHandle::new(Rc::new(()), &IosIconOps),
    });
    IconHandle::new(Rc::new(view), &IosIconOps)
}

struct IosIconOps;
impl IconOps for IosIconOps {
    fn set_stroke_progress(&self, node: &dyn std::any::Any, progress: f32) {
        let Some(view) = node.downcast_ref::<Retained<UIView>>() else { return };
        let layer: Retained<NSObject> = unsafe { msg_send_id![view.as_ref(), layer] };
        let sublayers: *const NSObject = unsafe { msg_send![&layer, sublayers] };
        if sublayers.is_null() { return; }
        let count: usize = unsafe { msg_send![sublayers, count] };
        if count == 0 { return; }
        let shape: Retained<NSObject> = unsafe { msg_send_id![sublayers, firstObject] };
        let val = progress.clamp(0.0, 1.0) as CGFloat;
        // Remove any running animation so the snap is immediate.
        let _: () = unsafe { msg_send![&shape, removeAllAnimations] };
        let _: () = unsafe { msg_send![&shape, setStrokeEnd: val] };
    }

    fn animate_stroke(
        &self,
        node: &dyn std::any::Any,
        from: f32,
        to: f32,
        duration_ms: u32,
        easing: runtime_core::Easing,
    ) {
        let Some(view) = node.downcast_ref::<Retained<UIView>>() else { return };
        // Build a temporary IosNode to reuse the existing function.
        let ios_node = IosNode::View(Retained::clone(view));
        animate_icon_stroke(&ios_node, from, to, duration_ms, easing, false, false);
    }
}

use std::rc::Rc;

// ==========================================================================
// Strategy 2: UIImage rasterization (for native UIKit controls)
// ==========================================================================

/// Render icon paths into a template-mode `UIImage` at the given point
/// size, with caching. Use this when feeding icons into UIKit APIs that
/// require `UIImage` (UIBarButtonItem, UITabBarItem, UIButton.setImage).
///
/// Cache key = (pointer address of `data.paths` slice, size as u16).
/// Same icon at same size returns the cached UIImage — no re-rasterization.
///
/// Returns a `UIImage` with rendering mode `.alwaysTemplate` so the
/// host control applies its own `tintColor`.
#[allow(dead_code)]
pub(crate) fn render_to_uiimage(
    data: &IconData,
    size: CGFloat,
    cache: &mut std::collections::HashMap<(usize, u16), Retained<NSObject>>,
) -> Retained<NSObject> {
    let key = (data.paths.as_ptr() as usize, size as u16);
    if let Some(cached) = cache.get(&key) {
        return cached.clone();
    }
    let image = render_to_uiimage_uncached(data, size);
    cache.insert(key, image.clone());
    image
}

/// Uncached rasterization — builds the UIImage from scratch.
fn render_to_uiimage_uncached(data: &IconData, size: CGFloat) -> Retained<NSObject> {
    let (vw, vh) = data.view_box;
    let sx = size / vw as CGFloat;
    let sy = size / vh as CGFloat;

    let cg_size = CGSize::new(size, size);

    // UIGraphicsImageRenderer.
    let renderer: Retained<NSObject> = unsafe {
        let cls = objc2::class!(UIGraphicsImageRenderer);
        msg_send_id![msg_send_id![cls, alloc], initWithSize: cg_size]
    };

    // Build bezier path via the shared SVG parser.
    let bezier: Retained<NSObject> = unsafe {
        let cls = objc2::class!(UIBezierPath);
        msg_send_id![cls, bezierPath]
    };
    {
        let mut emitter = UIBezierEmitter::new(&bezier);
        for path_d in data.paths {
            parse_svg_path(path_d, sx as f64, sy as f64, &mut emitter);
        }
    }

    let line_width: CGFloat = 2.0 * sx;
    let _: () = unsafe { msg_send![&bezier, setLineWidth: line_width] };
    // Line cap round = 1, line join round = 1.
    let _: () = unsafe { msg_send![&bezier, setLineCapStyle: 1i32] };
    let _: () = unsafe { msg_send![&bezier, setLineJoinStyle: 1i32] };

    if data.fill_rule == FillRule::EvenOdd {
        let _: () = unsafe { msg_send![&bezier, setUsesEvenOddFillRule: true] };
    }

    // Render: stroke in black (template mode ignores source color).
    let bezier_clone = bezier.clone();
    let block = block2::StackBlock::new(move |_ctx: *const NSObject| {
        let black: Retained<UIColor> = unsafe { UIColor::blackColor() };
        let _: () = unsafe { msg_send![&black, setStroke] };
        let _: () = unsafe { msg_send![&bezier_clone, stroke] };
    });

    let image: Retained<NSObject> = unsafe {
        msg_send_id![&renderer, imageWithActions: &*block]
    };

    // Set template rendering mode (2 = alwaysTemplate).
    let template: Retained<NSObject> = unsafe {
        msg_send_id![&image, imageWithRenderingMode: 2isize]
    };

    template
}
