//! UIKit-flavored font resolution.
//!
//! Wraps the cross-Apple [`backend_apple_core::font::FontRegistry`]
//! with UIFont construction + UIView application. The registry holds
//! the data (CGFont registration, PS-name lookup, face matching);
//! this file maps the resolved PS name into a `UIFont` and onto a
//! `UIView.font`.

use runtime_core::assets::SystemFallback;
use runtime_core::{FontFamily, FontStyle, FontWeight};
use objc2::rc::Retained;
use objc2::{msg_send, msg_send_id};
use objc2_foundation::{CGFloat, NSObject, NSString};

// Re-export the apple-core registry so callers can keep using
// `backend_ios_core::font::FontRegistry` unchanged.
pub use backend_apple_core::font::FontRegistry;

/// Build a `UIFont` for the given style. `family` is the optional
/// `font_family` from `StyleRules`; `weight`/`style` are the
/// resolved typography knobs the caller has already extracted.
/// Returns `None` if `family` is `None` and no other font lookup
/// is needed — the caller is then expected to fall back to its
/// existing system-font path.
pub fn resolve_uifont(
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
            // Try the resolved PS name first; fall back to the
            // typeface's declared family name (some bundled fonts
            // register under their family name instead of their PS
            // name); finally fall through to the typeface's
            // `SystemFallback` role.
            if let Some(face) = resolved {
                ui_font_with_name(face.postscript_name, size)
                    .or_else(|| ui_font_with_name(face.family_name, size))
                    .or_else(|| resolve_system_fallback(t.fallback, weight, size))
            } else {
                resolve_system_fallback(t.fallback, weight, size)
            }
        }
        FontFamily::System(name) => resolve_named_or_system(name, weight, size),
    }
}

// ---------------------------------------------------------------------------
// UIFont construction
// ---------------------------------------------------------------------------

/// `+[UIFont fontWithName:size:]` — returns `None` if UIKit doesn't
/// recognize the name (e.g. registration failed silently or the
/// caller passed a freeform family from `FontFamily::System` that
/// doesn't match an installed face).
fn ui_font_with_name(name: &str, size: CGFloat) -> Option<Retained<NSObject>> {
    let ns_name = NSString::from_str(name);
    let font: Option<Retained<NSObject>> = unsafe {
        msg_send_id![
            objc2::class!(UIFont),
            fontWithName: &*ns_name,
            size: size
        ]
    };
    font
}

/// Try a `FontFamily::System(name)` literally; if UIKit doesn't know
/// the name (the typical "system-ui, sans-serif" case), drop to
/// `systemFontOfSize:weight:`. Authors who want the system UI font
/// usually get here, so the fallback is the common path.
fn resolve_named_or_system(
    name: &str,
    weight: FontWeight,
    size: CGFloat,
) -> Option<Retained<NSObject>> {
    if let Some(f) = ui_font_with_name(name, size) {
        return Some(f);
    }
    Some(system_font(weight, size))
}

/// Generic-role fallback for a typeface that couldn't be resolved.
/// Maps `SystemFallback` to UIKit's nearest equivalent: serif →
/// `Times New Roman`, monospace → `Menlo`, sans-serif and unknown →
/// the standard system font.
fn resolve_system_fallback(
    fallback: SystemFallback,
    weight: FontWeight,
    size: CGFloat,
) -> Option<Retained<NSObject>> {
    match fallback {
        SystemFallback::Serif => ui_font_with_name("Times New Roman", size)
            .or_else(|| Some(system_font(weight, size))),
        SystemFallback::Monospace => ui_font_with_name("Menlo", size)
            .or_else(|| Some(system_font(weight, size))),
        SystemFallback::SansSerif | SystemFallback::None => Some(system_font(weight, size)),
    }
}

/// `+[UIFont systemFontOfSize:weight:]` — same call the existing
/// system-font path uses.
fn system_font(weight: FontWeight, size: CGFloat) -> Retained<NSObject> {
    let w = crate::style::font_weight_to_uikit(weight);
    let font: Retained<NSObject> = unsafe {
        msg_send_id![
            objc2::class!(UIFont),
            systemFontOfSize: size,
            weight: w
        ]
    };
    font
}

// ---------------------------------------------------------------------------
// Direct-set helper for callers that already have a UIView + size
// ---------------------------------------------------------------------------

/// Convenience: apply the resolved `UIFont` to a view's `font`
/// property in one call. Returns `true` if a font was applied so
/// the caller can fall through to its existing system-font path on
/// `false`.
pub fn apply_resolved_font(
    view: &objc2_ui_kit::UIView,
    registry: &FontRegistry,
    family: Option<&FontFamily>,
    weight: FontWeight,
    style: FontStyle,
    size: CGFloat,
) -> bool {
    let Some(font) = resolve_uifont(registry, family, weight, style, size) else {
        return false;
    };
    let _: () = unsafe { msg_send![view, setFont: &*font] };
    true
}

