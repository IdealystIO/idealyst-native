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
pub(crate) fn parse_color(s: &str) -> (CGFloat, CGFloat, CGFloat, CGFloat) {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        let hex = hex.trim();
        let chars: Vec<char> = hex.chars().collect();
        match chars.len() {
            3 => {
                let r = u8::from_str_radix(&format!("{}{}", chars[0], chars[0]), 16).unwrap_or(0);
                let g = u8::from_str_radix(&format!("{}{}", chars[1], chars[1]), 16).unwrap_or(0);
                let b = u8::from_str_radix(&format!("{}{}", chars[2], chars[2]), 16).unwrap_or(0);
                (r as CGFloat / 255.0, g as CGFloat / 255.0, b as CGFloat / 255.0, 1.0)
            }
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
                let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
                let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
                (r as CGFloat / 255.0, g as CGFloat / 255.0, b as CGFloat / 255.0, 1.0)
            }
            8 => {
                let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
                let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
                let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
                let a = u8::from_str_radix(&hex[6..8], 16).unwrap_or(255);
                (r as CGFloat / 255.0, g as CGFloat / 255.0, b as CGFloat / 255.0, a as CGFloat / 255.0)
            }
            _ => (0.0, 0.0, 0.0, 1.0),
        }
    } else if s.starts_with("rgba(") || s.starts_with("RGBA(") {
        let inner = s.trim_start_matches(|c: char| !c.is_ascii_digit() && c != '.')
            .trim_end_matches(')');
        let parts: Vec<&str> = inner.split(',').collect();
        if parts.len() >= 4 {
            let r: f64 = parts[0].trim().parse().unwrap_or(0.0);
            let g: f64 = parts[1].trim().parse().unwrap_or(0.0);
            let b: f64 = parts[2].trim().parse().unwrap_or(0.0);
            let a: f64 = parts[3].trim().parse().unwrap_or(1.0);
            (r / 255.0, g / 255.0, b / 255.0, a)
        } else {
            (0.0, 0.0, 0.0, 1.0)
        }
    } else if s.starts_with("rgb(") || s.starts_with("RGB(") {
        let inner = s.trim_start_matches(|c: char| !c.is_ascii_digit() && c != '.')
            .trim_end_matches(')');
        let parts: Vec<&str> = inner.split(',').collect();
        if parts.len() >= 3 {
            let r: f64 = parts[0].trim().parse().unwrap_or(0.0);
            let g: f64 = parts[1].trim().parse().unwrap_or(0.0);
            let b: f64 = parts[2].trim().parse().unwrap_or(0.0);
            (r / 255.0, g / 255.0, b / 255.0, 1.0)
        } else {
            (0.0, 0.0, 0.0, 1.0)
        }
    } else if s == "transparent" {
        (0.0, 0.0, 0.0, 0.0)
    } else {
        (0.0, 0.0, 0.0, 1.0)
    }
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

pub(crate) fn apply_style_to_view(view: &UIView, style: &StyleRules) {
    // Background color -- skip for Metal-backed views
    let layer: Retained<NSObject> = unsafe { msg_send_id![view, layer] };
    let is_metal_view: bool = unsafe {
        msg_send![&layer, isKindOfClass: objc2::class!(CAMetalLayer)]
    };
    if let Some(bg) = &style.background {
        if !is_metal_view {
            let raw = bg.value().0.clone();
            let c = color_to_uicolor(bg.value());
            view.setBackgroundColor(Some(&c));
            // Debug: read back the property to verify the assignment
            // stuck. Spot the views where `setBackgroundColor:` is
            // silently dropped.
            let read_back: Option<Retained<NSObject>> =
                unsafe { msg_send_id![view, backgroundColor] };
            crate::imp::ios_log(&format!(
                "[bg-paint] view {:p} src=\"{}\" set?={} ",
                view as *const UIView,
                raw,
                read_back.is_some()
            ));
            if let Some(trans) = &style.background_transition {
                let view_ref: Retained<UIView> = unsafe {
                    Retained::retain(view as *const UIView as *mut UIView).unwrap()
                };
                let trans = *trans;
                let c2 = c.clone();
                animate(&trans, Rc::new(move || {
                    view_ref.setBackgroundColor(Some(&c2));
                }));
            }
        }
    }

    // Flex direction, gap, justify_content, align_items, etc. are
    // ALL handled by Taffy now. They flow through
    // `LayoutTree::set_style` → Taffy's flex engine → frame
    // assignment via `apply_frames`. We deliberately do NOT forward
    // them to any UIView property: legacy backends used UIStackView
    // here, but UIStackView's own constraints conflict with Taffy's
    // frame writes (UISV-canvas-connection forces sizes Taffy didn't
    // choose). The framework's flex semantics live entirely in
    // native-layout.

    // Opacity
    if let Some(opacity) = style.opacity.as_ref().map(|t| *t.value()) {
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
    .filter_map(|r| r.map(|t| length_to_px(t.value())))
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
    .filter_map(|w| w.map(|t| *t.value()))
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
        let c = color_to_uicolor(bc.value());
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

    // Padding is handled entirely by Taffy now (writes into the
    // node's `padding` Rect, which insets the content area inside
    // the view's frame). We don't forward to setLayoutMargins
    // because UIView's layoutMargins are only consulted by
    // UIStackView's `layoutMarginsRelativeArrangement`, which we no
    // longer use.

    // Overflow
    if let Some(overflow) = &style.overflow {
        match overflow {
            framework_core::Overflow::Hidden => unsafe { view.setClipsToBounds(true) },
            framework_core::Overflow::Visible => unsafe { view.setClipsToBounds(false) },
        }
    }

    // Width / height: owned entirely by Taffy. Authors' explicit
    // `width` / `height` flow through `translate_style` into Taffy's
    // `size`, then Taffy writes `view.frame` via `apply_frames`. We
    // do NOT install Auto Layout constraints here — the goal of the
    // Taffy migration is to make UIView's Auto Layout system
    // redundant for framework-managed views.
}

pub(crate) fn apply_text_style(view: &UIView, style: &StyleRules, is_label: bool) {
    // Text color
    if let Some(color) = &style.color {
        let c = color_to_uicolor(color.value());
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
        let size = length_to_px(fs.value());
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

    // Number of lines = 0 for wrapping (UILabel only). Also pin
    // lineBreakMode to byWordWrapping (= 0) so wrapping happens
    // instead of mid-line ellipsis when the assigned frame is a
    // hair narrower than the text wants (rounding off `sizeThatFits:`).
    if is_label {
        let _: () = unsafe { msg_send![view, setNumberOfLines: 0isize] };
        let _: () = unsafe { msg_send![view, setLineBreakMode: 0isize] };
    }
}
