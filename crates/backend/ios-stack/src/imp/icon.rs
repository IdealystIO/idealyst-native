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

use runtime_core::primitives::icon::{FillRule, IconData};
use runtime_core::Color;
use objc2::msg_send;
use objc2::msg_send_id;
use objc2::rc::Retained;
use objc2_foundation::{CGFloat, CGPoint, CGRect, CGSize, MainThreadMarker, NSObject, NSString};
use objc2_ui_kit::{UIColor, UIView};

use super::style::color_to_uicolor;
use super::IosNode;

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

    // Build UIBezierPath from icon path data.
    let bezier: Retained<NSObject> = unsafe {
        let cls = objc2::class!(UIBezierPath);
        msg_send_id![cls, bezierPath]
    };
    for path_d in data.paths {
        parse_svg_path_into(&bezier, path_d, sx, sy);
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

    // Build bezier path.
    let bezier: Retained<NSObject> = unsafe {
        let cls = objc2::class!(UIBezierPath);
        msg_send_id![cls, bezierPath]
    };
    for path_d in data.paths {
        parse_svg_path_into(&bezier, path_d, sx, sy);
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

// ==========================================================================
// SVG path parser → UIBezierPath
// ==========================================================================

/// Parse an SVG path `d` string and append commands to `bezier`,
/// scaled by `(sx, sy)`.
fn parse_svg_path_into(
    bezier: &NSObject,
    d: &str,
    sx: CGFloat,
    sy: CGFloat,
) {
    let mut cur_x: CGFloat = 0.0;
    let mut cur_y: CGFloat = 0.0;
    let mut start_x: CGFloat = 0.0;
    let mut start_y: CGFloat = 0.0;
    let mut last_ctrl_x: CGFloat = 0.0;
    let mut last_ctrl_y: CGFloat = 0.0;
    let mut last_cmd: char = ' ';

    let mut chars = d.chars().peekable();

    while chars.peek().is_some() {
        skip_ws_comma(&mut chars);
        if chars.peek().is_none() {
            break;
        }

        let cmd = if chars.peek().map_or(false, |c| c.is_ascii_alphabetic()) {
            chars.next().unwrap()
        } else {
            if last_cmd == 'M' { 'L' }
            else if last_cmd == 'm' { 'l' }
            else { last_cmd }
        };

        match cmd {
            'M' => {
                let x = parse_number(&mut chars) * sx;
                let y = parse_number(&mut chars) * sy;
                let pt = CGPoint::new(x, y);
                let _: () = unsafe { msg_send![bezier, moveToPoint: pt] };
                cur_x = x; cur_y = y;
                start_x = x; start_y = y;
                last_ctrl_x = x; last_ctrl_y = y;
            }
            'm' => {
                let dx = parse_number(&mut chars) * sx;
                let dy = parse_number(&mut chars) * sy;
                let x = cur_x + dx;
                let y = cur_y + dy;
                let pt = CGPoint::new(x, y);
                let _: () = unsafe { msg_send![bezier, moveToPoint: pt] };
                cur_x = x; cur_y = y;
                start_x = x; start_y = y;
                last_ctrl_x = x; last_ctrl_y = y;
            }
            'L' => {
                let x = parse_number(&mut chars) * sx;
                let y = parse_number(&mut chars) * sy;
                let pt = CGPoint::new(x, y);
                let _: () = unsafe { msg_send![bezier, addLineToPoint: pt] };
                cur_x = x; cur_y = y;
                last_ctrl_x = x; last_ctrl_y = y;
            }
            'l' => {
                let dx = parse_number(&mut chars) * sx;
                let dy = parse_number(&mut chars) * sy;
                let x = cur_x + dx;
                let y = cur_y + dy;
                let pt = CGPoint::new(x, y);
                let _: () = unsafe { msg_send![bezier, addLineToPoint: pt] };
                cur_x = x; cur_y = y;
                last_ctrl_x = x; last_ctrl_y = y;
            }
            'H' => {
                let x = parse_number(&mut chars) * sx;
                let pt = CGPoint::new(x, cur_y);
                let _: () = unsafe { msg_send![bezier, addLineToPoint: pt] };
                cur_x = x;
                last_ctrl_x = x; last_ctrl_y = cur_y;
            }
            'h' => {
                let dx = parse_number(&mut chars) * sx;
                let x = cur_x + dx;
                let pt = CGPoint::new(x, cur_y);
                let _: () = unsafe { msg_send![bezier, addLineToPoint: pt] };
                cur_x = x;
                last_ctrl_x = x; last_ctrl_y = cur_y;
            }
            'V' => {
                let y = parse_number(&mut chars) * sy;
                let pt = CGPoint::new(cur_x, y);
                let _: () = unsafe { msg_send![bezier, addLineToPoint: pt] };
                cur_y = y;
                last_ctrl_x = cur_x; last_ctrl_y = y;
            }
            'v' => {
                let dy = parse_number(&mut chars) * sy;
                let y = cur_y + dy;
                let pt = CGPoint::new(cur_x, y);
                let _: () = unsafe { msg_send![bezier, addLineToPoint: pt] };
                cur_y = y;
                last_ctrl_x = cur_x; last_ctrl_y = y;
            }
            'C' => {
                let x1 = parse_number(&mut chars) * sx;
                let y1 = parse_number(&mut chars) * sy;
                let x2 = parse_number(&mut chars) * sx;
                let y2 = parse_number(&mut chars) * sy;
                let x = parse_number(&mut chars) * sx;
                let y = parse_number(&mut chars) * sy;
                let cp1 = CGPoint::new(x1, y1);
                let cp2 = CGPoint::new(x2, y2);
                let pt = CGPoint::new(x, y);
                let _: () = unsafe {
                    msg_send![bezier, addCurveToPoint: pt controlPoint1: cp1 controlPoint2: cp2]
                };
                cur_x = x; cur_y = y;
                last_ctrl_x = x2; last_ctrl_y = y2;
            }
            'c' => {
                let dx1 = parse_number(&mut chars) * sx;
                let dy1 = parse_number(&mut chars) * sy;
                let dx2 = parse_number(&mut chars) * sx;
                let dy2 = parse_number(&mut chars) * sy;
                let dx = parse_number(&mut chars) * sx;
                let dy = parse_number(&mut chars) * sy;
                let cp1 = CGPoint::new(cur_x + dx1, cur_y + dy1);
                let cp2 = CGPoint::new(cur_x + dx2, cur_y + dy2);
                let pt = CGPoint::new(cur_x + dx, cur_y + dy);
                let _: () = unsafe {
                    msg_send![bezier, addCurveToPoint: pt controlPoint1: cp1 controlPoint2: cp2]
                };
                last_ctrl_x = cur_x + dx2; last_ctrl_y = cur_y + dy2;
                cur_x += dx; cur_y += dy;
            }
            'S' => {
                let x1 = 2.0 * cur_x - last_ctrl_x;
                let y1 = 2.0 * cur_y - last_ctrl_y;
                let x2 = parse_number(&mut chars) * sx;
                let y2 = parse_number(&mut chars) * sy;
                let x = parse_number(&mut chars) * sx;
                let y = parse_number(&mut chars) * sy;
                let cp1 = CGPoint::new(x1, y1);
                let cp2 = CGPoint::new(x2, y2);
                let pt = CGPoint::new(x, y);
                let _: () = unsafe {
                    msg_send![bezier, addCurveToPoint: pt controlPoint1: cp1 controlPoint2: cp2]
                };
                cur_x = x; cur_y = y;
                last_ctrl_x = x2; last_ctrl_y = y2;
            }
            's' => {
                let x1 = 2.0 * cur_x - last_ctrl_x;
                let y1 = 2.0 * cur_y - last_ctrl_y;
                let dx2 = parse_number(&mut chars) * sx;
                let dy2 = parse_number(&mut chars) * sy;
                let dx = parse_number(&mut chars) * sx;
                let dy = parse_number(&mut chars) * sy;
                let cp1 = CGPoint::new(x1, y1);
                let cp2 = CGPoint::new(cur_x + dx2, cur_y + dy2);
                let pt = CGPoint::new(cur_x + dx, cur_y + dy);
                let _: () = unsafe {
                    msg_send![bezier, addCurveToPoint: pt controlPoint1: cp1 controlPoint2: cp2]
                };
                last_ctrl_x = cur_x + dx2; last_ctrl_y = cur_y + dy2;
                cur_x += dx; cur_y += dy;
            }
            'Q' => {
                let cx = parse_number(&mut chars) * sx;
                let cy = parse_number(&mut chars) * sy;
                let x = parse_number(&mut chars) * sx;
                let y = parse_number(&mut chars) * sy;
                let cp = CGPoint::new(cx, cy);
                let pt = CGPoint::new(x, y);
                let _: () = unsafe {
                    msg_send![bezier, addQuadCurveToPoint: pt controlPoint: cp]
                };
                cur_x = x; cur_y = y;
                last_ctrl_x = cx; last_ctrl_y = cy;
            }
            'q' => {
                let dcx = parse_number(&mut chars) * sx;
                let dcy = parse_number(&mut chars) * sy;
                let dx = parse_number(&mut chars) * sx;
                let dy = parse_number(&mut chars) * sy;
                let cp = CGPoint::new(cur_x + dcx, cur_y + dcy);
                let pt = CGPoint::new(cur_x + dx, cur_y + dy);
                let _: () = unsafe {
                    msg_send![bezier, addQuadCurveToPoint: pt controlPoint: cp]
                };
                last_ctrl_x = cur_x + dcx; last_ctrl_y = cur_y + dcy;
                cur_x += dx; cur_y += dy;
            }
            'T' => {
                let cx = 2.0 * cur_x - last_ctrl_x;
                let cy = 2.0 * cur_y - last_ctrl_y;
                let x = parse_number(&mut chars) * sx;
                let y = parse_number(&mut chars) * sy;
                let cp = CGPoint::new(cx, cy);
                let pt = CGPoint::new(x, y);
                let _: () = unsafe {
                    msg_send![bezier, addQuadCurveToPoint: pt controlPoint: cp]
                };
                cur_x = x; cur_y = y;
                last_ctrl_x = cx; last_ctrl_y = cy;
            }
            't' => {
                let cx = 2.0 * cur_x - last_ctrl_x;
                let cy = 2.0 * cur_y - last_ctrl_y;
                let dx = parse_number(&mut chars) * sx;
                let dy = parse_number(&mut chars) * sy;
                let cp = CGPoint::new(cx, cy);
                let pt = CGPoint::new(cur_x + dx, cur_y + dy);
                let _: () = unsafe {
                    msg_send![bezier, addQuadCurveToPoint: pt controlPoint: cp]
                };
                last_ctrl_x = cx; last_ctrl_y = cy;
                cur_x += dx; cur_y += dy;
            }
            'A' | 'a' => {
                let rx = parse_number(&mut chars).abs() * sx;
                let ry = parse_number(&mut chars).abs() * sy;
                let _x_rot = parse_number(&mut chars);
                let large_arc = parse_number(&mut chars) != 0.0;
                let sweep = parse_number(&mut chars) != 0.0;
                let raw_x = parse_number(&mut chars);
                let raw_y = parse_number(&mut chars);
                let (ex, ey) = if cmd == 'a' {
                    (cur_x + raw_x * sx, cur_y + raw_y * sy)
                } else {
                    (raw_x * sx, raw_y * sy)
                };
                arc_to_bezier(bezier, cur_x, cur_y, ex, ey, rx, ry, large_arc, sweep);
                cur_x = ex; cur_y = ey;
                last_ctrl_x = ex; last_ctrl_y = ey;
            }
            'Z' | 'z' => {
                let _: () = unsafe { msg_send![bezier, closePath] };
                cur_x = start_x; cur_y = start_y;
                last_ctrl_x = start_x; last_ctrl_y = start_y;
            }
            _ => {}
        }
        last_cmd = cmd;
    }
}

/// Approximate an SVG arc with cubic bezier curves.
fn arc_to_bezier(
    bezier: &NSObject,
    x1: CGFloat, y1: CGFloat,
    x2: CGFloat, y2: CGFloat,
    rx: CGFloat, ry: CGFloat,
    large_arc: bool, sweep: bool,
) {
    if rx < 1e-6 || ry < 1e-6 {
        let pt = CGPoint::new(x2, y2);
        let _: () = unsafe { msg_send![bezier, addLineToPoint: pt] };
        return;
    }

    let dx = (x1 - x2) / 2.0;
    let dy = (y1 - y2) / 2.0;

    let mut rx = rx;
    let mut ry = ry;

    let check = (dx * dx) / (rx * rx) + (dy * dy) / (ry * ry);
    if check > 1.0 {
        let s = check.sqrt();
        rx *= s;
        ry *= s;
    }

    let rxsq = rx * rx;
    let rysq = ry * ry;
    let dxsq = dx * dx;
    let dysq = dy * dy;

    let num = (rxsq * rysq - rxsq * dysq - rysq * dxsq).max(0.0);
    let den = rxsq * dysq + rysq * dxsq;
    let sq = if den < 1e-10 { 0.0 } else { (num / den).sqrt() };

    let sign = if large_arc == sweep { -1.0 } else { 1.0 };
    let cx = sign * sq * (rx * dy / ry) + (x1 + x2) / 2.0;
    let cy = sign * sq * -(ry * dx / rx) + (y1 + y2) / 2.0;

    let theta1 = ((y1 - cy) / ry).atan2((x1 - cx) / rx);
    let mut dtheta = ((y2 - cy) / ry).atan2((x2 - cx) / rx) - theta1;

    if sweep && dtheta < 0.0 {
        dtheta += 2.0 * std::f64::consts::PI as CGFloat;
    } else if !sweep && dtheta > 0.0 {
        dtheta -= 2.0 * std::f64::consts::PI as CGFloat;
    }

    let n_segs = (dtheta.abs() / (std::f64::consts::FRAC_PI_2 as CGFloat)).ceil() as usize;
    if n_segs == 0 { return; }
    let seg_angle = dtheta / n_segs as CGFloat;

    let mut angle = theta1;
    for _ in 0..n_segs {
        let next_angle = angle + seg_angle;
        let alpha = (seg_angle / 2.0).tan() * 4.0 / 3.0;

        let cos_a = angle.cos();
        let sin_a = angle.sin();
        let cos_b = next_angle.cos();
        let sin_b = next_angle.sin();

        let p2x = cx + rx * cos_b;
        let p2y = cy + ry * sin_b;

        let cp1x = cx + rx * cos_a - alpha * rx * sin_a;
        let cp1y = cy + ry * sin_a + alpha * ry * cos_a;
        let cp2x = p2x + alpha * rx * sin_b;
        let cp2y = p2y - alpha * ry * cos_b;

        let cp1 = CGPoint::new(cp1x, cp1y);
        let cp2 = CGPoint::new(cp2x, cp2y);
        let pt = CGPoint::new(p2x, p2y);
        let _: () = unsafe {
            msg_send![bezier, addCurveToPoint: pt controlPoint1: cp1 controlPoint2: cp2]
        };
        angle = next_angle;
    }
}

// --------------------------------------------------------------------------
// Number parsing helpers
// --------------------------------------------------------------------------

fn skip_ws_comma(chars: &mut std::iter::Peekable<std::str::Chars>) {
    while chars.peek().map_or(false, |&c| c == ' ' || c == ',' || c == '\t' || c == '\n' || c == '\r') {
        chars.next();
    }
}

fn parse_number(chars: &mut std::iter::Peekable<std::str::Chars>) -> CGFloat {
    skip_ws_comma(chars);
    let mut s = String::new();

    if chars.peek() == Some(&'-') || chars.peek() == Some(&'+') {
        s.push(chars.next().unwrap());
    }
    while chars.peek().map_or(false, |c| c.is_ascii_digit()) {
        s.push(chars.next().unwrap());
    }
    if chars.peek() == Some(&'.') {
        s.push(chars.next().unwrap());
        while chars.peek().map_or(false, |c| c.is_ascii_digit()) {
            s.push(chars.next().unwrap());
        }
    }
    if chars.peek().map_or(false, |&c| c == 'e' || c == 'E') {
        s.push(chars.next().unwrap());
        if chars.peek() == Some(&'-') || chars.peek() == Some(&'+') {
            s.push(chars.next().unwrap());
        }
        while chars.peek().map_or(false, |c| c.is_ascii_digit()) {
            s.push(chars.next().unwrap());
        }
    }

    s.parse::<CGFloat>().unwrap_or(0.0)
}
