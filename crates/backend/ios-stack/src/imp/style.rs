use framework_core::{Color, Length, StyleRules};
use objc2::encode::{Encode, Encoding};
use objc2::rc::Retained;
use objc2::{msg_send, msg_send_id};
use objc2_foundation::{CGFloat, CGSize, NSObject};
use objc2_ui_kit::{UIColor, UIView};
use std::rc::Rc;
use block2::ConcreteBlock;

/// Opaque wrapper for CoreGraphics' `CGColorRef` so `msg_send!`'s
/// debug-mode encoding check sees `^{CGColor=}` instead of `^v`.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub(crate) struct CGColorRef(pub(crate) *const std::ffi::c_void);

unsafe impl Encode for CGColorRef {
    const ENCODING: Encoding = Encoding::Pointer(&Encoding::Struct("CGColor", &[]));
}

/// Parse a CSS-style color string into (r, g, b, a) in 0.0..1.0.
/// Parsing logic lives in `framework_core::color`; this wrapper
/// applies opaque black as the fallback for unknown shapes.
pub(crate) fn parse_color(s: &str) -> (CGFloat, CGFloat, CGFloat, CGFloat) {
    let [r, g, b, a] = framework_core::color::parse_or(s, framework_core::color::Rgba::BLACK)
        .to_srgb_f32();
    (r as CGFloat, g as CGFloat, b as CGFloat, a as CGFloat)
}

pub(crate) fn color_to_uicolor(color: &Color) -> Retained<UIColor> {
    let (r, g, b, a) = parse_color(&color.0);
    unsafe { UIColor::colorWithRed_green_blue_alpha(r, g, b, a) }
}

pub(crate) fn length_to_px(len: &Length) -> CGFloat {
    match len {
        Length::Px(v) => *v as CGFloat,
        Length::Percent(_) => 0.0,
        Length::Auto => 0.0,
    }
}

pub(crate) fn font_weight_to_uikit(weight: framework_core::FontWeight) -> CGFloat {
    match weight {
        framework_core::FontWeight::Thin => -0.6,
        framework_core::FontWeight::ExtraLight => -0.5,
        framework_core::FontWeight::Light => -0.4,
        framework_core::FontWeight::Normal => 0.0,
        framework_core::FontWeight::Medium => 0.23,
        framework_core::FontWeight::SemiBold => 0.3,
        framework_core::FontWeight::Bold => 0.4,
        framework_core::FontWeight::ExtraBold => 0.56,
        framework_core::FontWeight::Black => 0.62,
    }
}

/// Map framework Easing to UIView animation options bitmask.
pub(crate) fn easing_to_options(easing: &framework_core::Easing) -> u64 {
    match easing {
        framework_core::Easing::Linear => 3 << 16,
        framework_core::Easing::Ease | framework_core::Easing::EaseInOut => 0 << 16,
        framework_core::Easing::EaseIn => 1 << 16,
        framework_core::Easing::EaseOut => 2 << 16,
        framework_core::Easing::CubicBezier(_, _, _, _) => 0 << 16,
    }
}

/// Run property changes inside a UIView animation block.
pub(crate) fn animate(transition: &framework_core::Transition, changes: Rc<dyn Fn()>) {
    let duration = transition.duration_ms as CGFloat / 1000.0;
    let options = easing_to_options(&transition.easing);
    let block = ConcreteBlock::new(move || {
        changes();
    });
    let block = block.copy();
    let nil: *const NSObject = std::ptr::null();
    unsafe {
        let _: () = msg_send![
            objc2::class!(UIView),
            animateWithDuration: duration,
            delay: 0.0 as CGFloat,
            options: options,
            animations: &*block,
            completion: nil
        ];
    }
}

/// Look for an existing width or height constraint on `view` and
/// update its constant. Returns true if found (no new constraint
/// needed), false if none exists yet.
fn update_dimension_constraint(view: &UIView, is_width: bool, value: CGFloat) -> bool {
    // NSLayoutConstraint attributes: width=7, height=8
    let target_attr: isize = if is_width { 7 } else { 8 };
    let constraints: Retained<objc2_foundation::NSArray<NSObject>> = unsafe {
        msg_send_id![view, constraints]
    };
    for i in 0..constraints.len() {
        let c: &NSObject = &constraints[i];
        let first_attr: isize = unsafe { msg_send![c, firstAttribute] };
        let second_item: *const NSObject = unsafe { msg_send![c, secondItem] };
        // A dimension constraint has firstAttribute == width/height
        // and secondItem == nil (it's a constant constraint, not relative).
        if first_attr == target_attr && second_item.is_null() {
            let _: () = unsafe { msg_send![c, setConstant: value] };
            return true;
        }
    }
    false
}

pub(crate) fn apply_style_to_view(view: &UIView, style: &StyleRules) {
    // Background color -- skip for Metal-backed views
    let layer: Retained<NSObject> = unsafe { msg_send_id![view, layer] };
    let is_metal_view: bool = unsafe {
        msg_send![&layer, isKindOfClass: objc2::class!(CAMetalLayer)]
    };
    if let Some(bg) = &style.background {
        if !is_metal_view {
            let bg_val = bg.resolve();
            let c = color_to_uicolor(&bg_val);
            if let Some(trans) = &style.background_transition {
                let view_ref: Retained<UIView> = unsafe { Retained::retain(view as *const UIView as *mut UIView).unwrap() };
                let trans = *trans;
                animate(&trans, Rc::new(move || {
                    view_ref.setBackgroundColor(Some(&c));
                }));
            } else {
                view.setBackgroundColor(Some(&c));
            }
        }
    }

    // Flex direction -> UIStackView axis
    if let Some(dir) = &style.flex_direction {
        let is_stack: bool = unsafe {
            msg_send![view, isKindOfClass: objc2::class!(UIStackView)]
        };
        if is_stack {
            let axis: isize = match dir {
                framework_core::FlexDirection::Row
                | framework_core::FlexDirection::RowReverse => 0,
                framework_core::FlexDirection::Column
                | framework_core::FlexDirection::ColumnReverse => 1,
            };
            let _: () = unsafe { msg_send![view, setAxis: axis] };
        }
    }

    // Gap -> UIStackView spacing
    if let Some(gap) = &style.gap {
        let is_stack: bool = unsafe {
            msg_send![view, isKindOfClass: objc2::class!(UIStackView)]
        };
        if is_stack {
            let gap_val = gap.resolve();
            let px = length_to_px(&gap_val);
            let _: () = unsafe { msg_send![view, setSpacing: px] };
        }
    }

    // Opacity
    if let Some(opacity) = style.opacity.as_ref().map(|t| t.resolve()) {
        if let Some(trans) = &style.opacity_transition {
            let view_ref: Retained<UIView> = unsafe { Retained::retain(view as *const UIView as *mut UIView).unwrap() };
            let trans = *trans;
            animate(&trans, Rc::new(move || {
                unsafe { view_ref.setAlpha(opacity as CGFloat) };
            }));
        } else {
            unsafe { view.setAlpha(opacity as CGFloat) };
        }
    }

    // Corner radius
    let radius = [
        style.border_top_left_radius.as_ref(),
        style.border_top_right_radius.as_ref(),
        style.border_bottom_left_radius.as_ref(),
        style.border_bottom_right_radius.as_ref(),
    ]
    .iter()
    .filter_map(|r| r.map(|t| length_to_px(&t.resolve())))
    .fold(0.0_f64, f64::max);
    if radius > 0.0 {
        let _: () = unsafe { msg_send![&layer, setCornerRadius: radius] };
        unsafe { view.setClipsToBounds(true) };
    }

    // Border width
    let border_w = [
        style.border_top_width.as_ref(),
        style.border_right_width.as_ref(),
        style.border_bottom_width.as_ref(),
        style.border_left_width.as_ref(),
    ]
    .iter()
    .filter_map(|w| w.map(|t| t.resolve()))
    .fold(0.0_f32, f32::max);
    if border_w > 0.0 {
        let _: () = unsafe { msg_send![&layer, setBorderWidth: border_w as CGFloat] };
    }

    // Border color
    let border_color = style
        .border_top_color
        .as_ref()
        .or(style.border_right_color.as_ref())
        .or(style.border_bottom_color.as_ref())
        .or(style.border_left_color.as_ref());
    if let Some(bc) = border_color {
        let bc_val = bc.resolve();
        let c = color_to_uicolor(&bc_val);
        let cg: CGColorRef = unsafe { msg_send![&c, CGColor] };
        if !cg.0.is_null() {
            let _: () = unsafe { msg_send![&layer, setBorderColor: cg] };
        }
    }

    // Shadow
    if let Some(shadow) = &style.shadow {
        let shadow_color = color_to_uicolor(&shadow.color);
        let cg: CGColorRef = unsafe { msg_send![&shadow_color, CGColor] };
        if !cg.0.is_null() {
            let _: () = unsafe { msg_send![&layer, setShadowColor: cg] };
        }
        let offset = CGSize {
            width: shadow.x as CGFloat,
            height: shadow.y as CGFloat,
        };
        let _: () = unsafe { msg_send![&layer, setShadowOffset: offset] };
        let _: () = unsafe { msg_send![&layer, setShadowRadius: (shadow.blur as CGFloat / 2.0)] };
        let _: () = unsafe { msg_send![&layer, setShadowOpacity: 1.0_f32] };
        unsafe { view.setClipsToBounds(false) };
    }

    // Padding via layoutMargins
    let pad_top = style.padding_top.as_ref().map(|t| length_to_px(&t.resolve())).unwrap_or(0.0);
    let pad_left = style.padding_left.as_ref().map(|t| length_to_px(&t.resolve())).unwrap_or(0.0);
    let pad_bottom = style.padding_bottom.as_ref().map(|t| length_to_px(&t.resolve())).unwrap_or(0.0);
    let pad_right = style.padding_right.as_ref().map(|t| length_to_px(&t.resolve())).unwrap_or(0.0);
    if pad_top > 0.0 || pad_left > 0.0 || pad_bottom > 0.0 || pad_right > 0.0 {
        let is_stack: bool = unsafe {
            msg_send![view, isKindOfClass: objc2::class!(UIStackView)]
        };
        if is_stack {
            let _: () = unsafe { msg_send![view, setLayoutMarginsRelativeArrangement: true] };
        }
        #[repr(C)]
        #[derive(Clone, Copy)]
        struct UIEdgeInsets {
            top: CGFloat, left: CGFloat, bottom: CGFloat, right: CGFloat,
        }
        unsafe impl Encode for UIEdgeInsets {
            const ENCODING: Encoding = Encoding::Struct(
                "UIEdgeInsets",
                &[
                    CGFloat::ENCODING,
                    CGFloat::ENCODING,
                    CGFloat::ENCODING,
                    CGFloat::ENCODING,
                ],
            );
        }
        let insets = UIEdgeInsets { top: pad_top, left: pad_left, bottom: pad_bottom, right: pad_right };
        let _: () = unsafe { msg_send![view, setLayoutMargins: insets] };
    }

    // Overflow
    if let Some(overflow) = &style.overflow {
        match overflow {
            framework_core::Overflow::Hidden => unsafe { view.setClipsToBounds(true) },
            framework_core::Overflow::Visible => unsafe { view.setClipsToBounds(false) },
        }
    }

    // Height / width via Auto Layout constraints
    // Width / height via Auto Layout constraints. To avoid
    // accumulating duplicate constraints on repeated apply_style
    // calls, we first check if a matching constraint already exists
    // on the view and update its constant rather than adding a new one.
    if let Some(w) = &style.width {
        if let Length::Px(px) = w.resolve() {
            let px_val = px as CGFloat;
            if !update_dimension_constraint(view, true, px_val) {
                let anchor: Retained<NSObject> = unsafe { msg_send_id![view, widthAnchor] };
                let c: Retained<NSObject> = unsafe {
                    msg_send_id![&anchor, constraintEqualToConstant: px_val]
                };
                let _: () = unsafe { msg_send![&c, setActive: true] };
            }
        }
    }
    if let Some(h) = &style.height {
        if let Length::Px(px) = h.resolve() {
            let px_val = px as CGFloat;
            if !update_dimension_constraint(view, false, px_val) {
                let anchor: Retained<NSObject> = unsafe { msg_send_id![view, heightAnchor] };
                let c: Retained<NSObject> = unsafe {
                    msg_send_id![&anchor, constraintEqualToConstant: px_val]
                };
                let _: () = unsafe { msg_send![&c, setActive: true] };
            }
        }
    }
}

pub(crate) fn apply_text_style(view: &UIView, style: &StyleRules, is_label: bool) {
    // Text color
    if let Some(color) = &style.color {
        let color_val = color.resolve();
        let c = color_to_uicolor(&color_val);
        if let Some(trans) = &style.color_transition {
            let view_ref: Retained<UIView> = unsafe { Retained::retain(view as *const UIView as *mut UIView).unwrap() };
            let trans = *trans;
            animate(&trans, Rc::new(move || {
                let _: () = unsafe { msg_send![&view_ref, setTextColor: &*c] };
            }));
        } else {
            let _: () = unsafe { msg_send![view, setTextColor: &*c] };
        }
    }

    // Font size
    if let Some(fs) = &style.font_size {
        let fs_val = fs.resolve();
        let size = length_to_px(&fs_val);
        if size > 0.0 {
            let weight = style.font_weight.as_ref().copied().unwrap_or(framework_core::FontWeight::Normal);
            let ui_weight = font_weight_to_uikit(weight);
            let font: Retained<NSObject> = unsafe {
                msg_send_id![
                    objc2::class!(UIFont),
                    systemFontOfSize: size,
                    weight: ui_weight
                ]
            };
            let _: () = unsafe { msg_send![view, setFont: &*font] };
        }
    } else if let Some(weight) = &style.font_weight {
        let ui_weight = font_weight_to_uikit(*weight);
        let font: Retained<NSObject> = unsafe {
            msg_send_id![
                objc2::class!(UIFont),
                systemFontOfSize: 17.0 as CGFloat,
                weight: ui_weight
            ]
        };
        let _: () = unsafe { msg_send![view, setFont: &*font] };
    }

    // Text alignment
    if let Some(ta) = &style.text_align {
        let align: isize = match ta {
            framework_core::TextAlign::Left => 0,
            framework_core::TextAlign::Center => 1,
            framework_core::TextAlign::Right => 2,
            framework_core::TextAlign::Justify => 3,
        };
        let _: () = unsafe { msg_send![view, setTextAlignment: align] };
    }

    // Number of lines = 0 for wrapping (UILabel only)
    if is_label {
        let _: () = unsafe { msg_send![view, setNumberOfLines: 0isize] };
    }
}
