//! Text-style application for `NSTextField` (label mode) and
//! `NSTextView` (text-area). Mirrors `backend_ios_core::style::
//! apply_text_style` — same shape, AppKit setters.

use backend_apple_core::font::FontRegistry;
use runtime_core::{FontFamily, FontStyle, FontWeight, StyleRules};
use objc2::rc::Retained;
use objc2::{msg_send, msg_send_id};
use objc2_app_kit::NSView;
use objc2_foundation::{CGFloat, NSObject, NSString};

use crate::imp::color_to_nscolor;

/// AppKit's NSTextAlignment enum values. The `objc2-app-kit`
/// generated bindings define them; we mirror raw values here so
/// `msg_send!` can hand the right integer without pulling in a
/// feature that drags more code into the build.
const NS_TEXT_ALIGNMENT_LEFT: isize = 0;
const NS_TEXT_ALIGNMENT_RIGHT: isize = 1;
const NS_TEXT_ALIGNMENT_CENTER: isize = 2;
const NS_TEXT_ALIGNMENT_JUSTIFIED: isize = 3;

/// Apply text-related style props to an NSTextField (label) or
/// NSTextView. Reads `style.color`, `style.font_*`, `style.text_align`.
///
/// `is_label`: true for NSTextField in label mode (different
/// `setStringValue:` path); false for NSTextView (uses `setString:`
/// and behaves like a UITextView).
pub(crate) fn apply_text_style(
    view: &NSView,
    style: &StyleRules,
    is_label: bool,
    font_registry: &FontRegistry,
) {
    // Text color
    if let Some(color) = &style.color {
        let color_val = color.resolve();
        let c = color_to_nscolor(&color_val);
        let _: () = unsafe { msg_send![view, setTextColor: &*c] };
    }

    // Font: route through the registry first (custom typefaces),
    // fall back to system font.
    let has_typography = style.font_family.is_some()
        || style.font_size.is_some()
        || style.font_weight.is_some()
        || style.font_style.is_some();
    if has_typography {
        let weight = style
            .font_weight
            .as_ref()
            .copied()
            .unwrap_or(FontWeight::Normal);
        let fstyle = style
            .font_style
            .as_ref()
            .copied()
            .unwrap_or(FontStyle::Normal);
        let size = match style.font_size.as_ref().map(|t| t.resolve()) {
            Some(len) => {
                let px = length_to_px(&len);
                if px > 0.0 { px } else { 13.0 as CGFloat }
            }
            None => 13.0 as CGFloat,
        };
        let font = resolve_nsfont(font_registry, style.font_family.as_ref(), weight, fstyle, size);
        if let Some(f) = font {
            let _: () = unsafe { msg_send![view, setFont: &*f] };
        }
    }

    // Text alignment
    if let Some(ta) = &style.text_align {
        let align: isize = match ta {
            runtime_core::TextAlign::Left => NS_TEXT_ALIGNMENT_LEFT,
            runtime_core::TextAlign::Right => NS_TEXT_ALIGNMENT_RIGHT,
            runtime_core::TextAlign::Center => NS_TEXT_ALIGNMENT_CENTER,
            runtime_core::TextAlign::Justify => NS_TEXT_ALIGNMENT_JUSTIFIED,
        };
        let _: () = unsafe { msg_send![view, setAlignment: align] };
    }

    let _ = is_label;
}

/// Build an `NSFont` for the given style. `family` is the optional
/// `font_family` from `StyleRules`; `weight`/`style` are the
/// resolved typography knobs.
///
/// Routes through the cross-Apple font registry first (custom
/// typefaces registered via `register_asset`); falls through to
/// `+[NSFont fontWithName:size:]` for `FontFamily::System(name)`;
/// falls through finally to `+[NSFont systemFontOfSize:weight:]`.
fn resolve_nsfont(
    registry: &FontRegistry,
    family: Option<&FontFamily>,
    weight: FontWeight,
    style: FontStyle,
    size: CGFloat,
) -> Option<Retained<NSObject>> {
    let family = family?;
    match family {
        FontFamily::Typeface(t) => {
            let resolved = registry.resolve_typeface(t, weight, style);
            if let Some(face) = resolved {
                ns_font_with_name(face.postscript_name, size)
                    .or_else(|| ns_font_with_name(face.family_name, size))
                    .or_else(|| resolve_system_fallback(t.fallback, weight, size))
            } else {
                resolve_system_fallback(t.fallback, weight, size)
            }
        }
        FontFamily::System(name) => ns_font_with_name(name, size)
            .or_else(|| Some(system_font(weight, size))),
    }
}

/// `+[NSFont fontWithName:size:]` — returns `None` if AppKit
/// doesn't recognize the name.
fn ns_font_with_name(name: &str, size: CGFloat) -> Option<Retained<NSObject>> {
    let ns_name = NSString::from_str(name);
    let font: Option<Retained<NSObject>> = unsafe {
        msg_send_id![
            objc2::class!(NSFont),
            fontWithName: &*ns_name,
            size: size
        ]
    };
    font
}

/// `+[NSFont systemFontOfSize:weight:]`. The weight axis is the
/// same -1.0..1.0 NSFontWeight as `UIFont.Weight` (both bridge to
/// `CGFloat`), so the iOS weight mapping is reusable here.
fn system_font(weight: FontWeight, size: CGFloat) -> Retained<NSObject> {
    let w = font_weight_to_nsfont(weight);
    let font: Retained<NSObject> = unsafe {
        msg_send_id![
            objc2::class!(NSFont),
            systemFontOfSize: size,
            weight: w
        ]
    };
    font
}

/// Generic-role fallback for a typeface that couldn't be resolved.
/// Same mapping iOS uses (serif → Times New Roman, monospace →
/// Menlo, sans → system).
fn resolve_system_fallback(
    fallback: runtime_core::assets::SystemFallback,
    weight: FontWeight,
    size: CGFloat,
) -> Option<Retained<NSObject>> {
    use runtime_core::assets::SystemFallback;
    match fallback {
        SystemFallback::Serif => ns_font_with_name("Times New Roman", size)
            .or_else(|| Some(system_font(weight, size))),
        SystemFallback::Monospace => ns_font_with_name("Menlo", size)
            .or_else(|| Some(system_font(weight, size))),
        SystemFallback::SansSerif | SystemFallback::None => Some(system_font(weight, size)),
    }
}

/// Map framework `FontWeight` to NSFontWeight (same -1.0..1.0 axis
/// UIFont uses). Mirrors `backend_ios_core::style::font_weight_to_uikit`.
fn font_weight_to_nsfont(weight: FontWeight) -> CGFloat {
    match weight {
        FontWeight::Thin => -0.6,
        FontWeight::ExtraLight => -0.5,
        FontWeight::Light => -0.4,
        FontWeight::Normal => 0.0,
        FontWeight::Medium => 0.23,
        FontWeight::SemiBold => 0.3,
        FontWeight::Bold => 0.4,
        FontWeight::ExtraBold => 0.56,
        FontWeight::Black => 0.62,
    }
}

fn length_to_px(len: &runtime_core::Length) -> CGFloat {
    match len {
        runtime_core::Length::Px(v) => *v as CGFloat,
        runtime_core::Length::Percent(_) | runtime_core::Length::Auto => 0.0,
    }
}

