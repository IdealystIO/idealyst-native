//! Box shadow (`StyleRules.shadow`) for the macOS backend.
//!
//! Mirrors `backend_ios_core::style`'s shadow path so the two Apple backends
//! converge (Rule #7). Before this module macOS honored only `text_shadow`
//! (the GLYPH shadow on labels, see [`super::text_style`]) and dropped
//! `shadow` on the floor — so an idea-ui `Elevated` Card rendered with a drop
//! shadow on iOS and web, and perfectly flat on macOS, from the same author
//! tree.
//!
//! ## Two non-obvious invariants
//!
//! **1. The Y sign depends on the view's `isFlipped`, not on the platform.**
//! `Shadow`'s `(x, y)` is web semantics: `+y` casts DOWNWARD. A CALayer's
//! `shadowOffset` is expressed in the layer's own coordinate space, and for a
//! layer-backed `NSView` AppKit flips the backing layer's geometry to match
//! the view. The framework's container views (`view` / `pressable` / `link`)
//! are `IdealystFlippedView` — `isFlipped == true`, y-down, exactly like
//! UIKit — so they take `+y` directly, the same value iOS passes. A
//! *non-flipped* view (`NSTextField` and the other native controls) is y-up
//! and must negate, which is why [`super::text_style::text_shadow_offset`]
//! negates unconditionally: it only ever runs on labels. Getting this wrong
//! flips the shadow to the wrong side of the box on macOS only — precisely
//! the per-platform divergence Rule #7 bans. [`box_shadow_offset`] is pure so
//! the sign convention is unit-tested off the main thread.
//!
//! **2. Always hand CoreAnimation an explicit `shadowPath`.**
//! With `shadowOpacity > 0` and no path, CoreAnimation derives the shadow's
//! shape from the layer's alpha channel: it renders the subtree into an
//! offscreen buffer, blurs it to build the silhouette, then composites — and
//! redoes that on every recomposite. A screenful of shadowed cards then costs
//! a screenful of offscreen passes per scrolled frame. Tracing the rounded
//! rect ourselves skips the derivation entirely. The path is in layer
//! coordinates and does NOT follow a resize, so [`sync_shadow_path`] re-traces
//! it from the post-layout hook, after `sync_corner_radius` has settled the
//! final curve.

use std::ffi::c_void;

use objc2::encode::{Encoding, RefEncode};
use objc2::msg_send;
use objc2::runtime::NSObject;
use objc2_app_kit::NSView;
use objc2_foundation::{CGFloat, CGRect, CGSize, NSString};
use runtime_shared::StyleRules;

use super::color_to_nscolor;

/// Marks a layer as carrying a BOX shadow, so [`sync_shadow_path`] knows it
/// may trace a rectangular path over it. A label's `text_shadow` deliberately
/// takes the GLYPH silhouette — stamping a rect path there would paint a solid
/// slab behind the text — and never sets this flag.
const BOX_SHADOW_FLAG_KEY: &str = "idealyst_has_box_shadow";

#[repr(C)]
struct CGPathRef(c_void);

// `-[CALayer setShadowPath:]` wants a typed `CGPathRef` (`^{CGPath=}`).
// Passing a bare `*const c_void` (`^v`) trips objc2's debug-mode encoding
// verifier and SIGABRTs — the same trap `icon.rs` documents, and the one that
// crashed the iOS twin mid-layout during development.
unsafe impl RefEncode for CGPathRef {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("CGPath", &[]));
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    /// Rounded-rect path in one call. `NSBezierPath.cgPath` would be simpler
    /// but is macOS 14+, and the framework's floor is 11.0.
    fn CGPathCreateWithRoundedRect(
        rect: CGRect,
        corner_width: CGFloat,
        corner_height: CGFloat,
        transform: *const c_void,
    ) -> *mut CGPathRef;
    fn CGPathRelease(path: *mut CGPathRef);
}

/// Translate a `Shadow`'s `(x, y)` (web semantics: `+x` right, `+y` DOWN) into
/// a CALayer `shadowOffset` for a view with the given flipped-ness.
///
/// See the module docs: a flipped view's backing layer is y-down (pass `+y`
/// straight through, matching iOS); a non-flipped view's is y-up (negate).
pub(crate) fn box_shadow_offset(x: f32, y: f32, is_flipped: bool) -> (CGFloat, CGFloat) {
    let h = if is_flipped { y as CGFloat } else { -(y as CGFloat) };
    (x as CGFloat, h)
}

/// Apply — or clear — the view's box shadow. Call from `apply_style_to_view`
/// after the border, on a view that is already layer-backed.
pub(crate) fn apply_box_shadow(view: &NSView, style: &StyleRules) {
    let layer_ptr: *mut NSObject = unsafe { msg_send![view, layer] };
    if layer_ptr.is_null() {
        return;
    }
    let layer: &NSObject = unsafe { &*layer_ptr };
    let key = NSString::from_str(BOX_SHADOW_FLAG_KEY);

    let Some(shadow) = &style.shadow else {
        // No shadow in THIS restyle → clear whatever a previous apply left,
        // including the cached path, so a reactively-removed shadow actually
        // stops painting (the set-then-never-unset hazard the background and
        // text-shadow paths both guard). Only touch layers WE marked: a label
        // carrying a glyph shadow must be left alone.
        let flag_ptr: *mut NSObject = unsafe { msg_send![layer, valueForKey: &*key] };
        if !flag_ptr.is_null() {
            let null_obj: *mut NSObject = std::ptr::null_mut();
            let null_path: *mut CGPathRef = std::ptr::null_mut();
            unsafe {
                let _: () = msg_send![layer, setValue: null_obj, forKey: &*key];
                let _: () = msg_send![layer, setShadowOpacity: 0.0f32];
                let _: () = msg_send![layer, setShadowPath: null_path];
            }
        }
        return;
    };

    // `shadowColor` carries the alpha and `shadowOpacity` is a plain enable
    // multiplier at 1.0 — CALayer multiplies the two, so the effective
    // strength is exactly the author's color alpha. Same split iOS uses.
    let ns_color = color_to_nscolor(&shadow.color);
    let cg: super::CGColorRef = unsafe { msg_send![&*ns_color, CGColor] };
    if !cg.0.is_null() {
        let _: () = unsafe { msg_send![layer, setShadowColor: cg] };
    }

    let is_flipped: bool = unsafe { msg_send![view, isFlipped] };
    let (w, h) = box_shadow_offset(shadow.x, shadow.y, is_flipped);
    let offset = CGSize { width: w, height: h };
    unsafe {
        let _: () = msg_send![layer, setShadowOffset: offset];
        // CSS `blur` is the full blur diameter; CALayer's `shadowRadius` is
        // the standard-deviation-ish half. iOS halves it identically, so the
        // two backends produce the same spread for one authored value.
        let _: () = msg_send![layer, setShadowRadius: (shadow.blur as CGFloat / 2.0)];
        let _: () = msg_send![layer, setShadowOpacity: 1.0f32];
        // Mark it as a box shadow so `sync_shadow_path` will trace it.
        let flag: *mut NSObject = msg_send![objc2::class!(NSNumber), numberWithBool: true];
        let _: () = msg_send![layer, setValue: flag, forKey: &*key];
    }
    // Trace the path now; the layout pass re-traces once bounds are real.
    sync_shadow_path(view);
}

/// Re-trace the box shadow's `shadowPath` against the view's current bounds
/// and corner radius. Call from the post-frame hook AFTER `sync_corner_radius`
/// so the path follows the final clamped curve.
///
/// No-op unless the layer was marked by [`apply_box_shadow`], and on a 0×0
/// view (the path would be empty; the layout pass calls this again once Taffy
/// has assigned a frame).
pub(crate) fn sync_shadow_path(view: &NSView) {
    let layer_ptr: *mut NSObject = unsafe { msg_send![view, layer] };
    if layer_ptr.is_null() {
        return;
    }
    let layer: &NSObject = unsafe { &*layer_ptr };

    let key = NSString::from_str(BOX_SHADOW_FLAG_KEY);
    let flag_ptr: *mut NSObject = unsafe { msg_send![layer, valueForKey: &*key] };
    if flag_ptr.is_null() {
        return;
    }
    let opacity: f32 = unsafe { msg_send![layer, shadowOpacity] };
    if opacity <= 0.0 {
        return;
    }
    let bounds: CGRect = unsafe { msg_send![view, bounds] };
    if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
        return;
    }
    let radius: CGFloat = unsafe { msg_send![layer, cornerRadius] };

    // `CGPathCreateWithRoundedRect` is a Create-rule function: we own the +1.
    // `setShadowPath:` retains it, so release our reference afterwards.
    let path = unsafe { CGPathCreateWithRoundedRect(bounds, radius, radius, std::ptr::null()) };
    if path.is_null() {
        return;
    }
    unsafe {
        let _: () = msg_send![layer, setShadowPath: path];
        CGPathRelease(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: the framework's container views (`IdealystFlippedView`) are
    /// `isFlipped`, so their backing layer is y-down like UIKit and `+y` must
    /// pass through UNCHANGED — the same number iOS sends. Negating here (the
    /// rule the label-only `text_shadow` path follows) would cast every card's
    /// drop shadow UPWARD on macOS while iOS and web cast it down.
    #[test]
    fn flipped_container_passes_y_through_like_ios() {
        assert_eq!(box_shadow_offset(0.0, 12.0, true), (0.0 as CGFloat, 12.0 as CGFloat));
        assert_eq!(box_shadow_offset(2.0, -4.0, true), (2.0 as CGFloat, -4.0 as CGFloat));
    }

    /// A non-flipped view (native controls) is y-up, so the offset negates —
    /// matching `text_style::text_shadow_offset`.
    #[test]
    fn non_flipped_view_negates_y() {
        assert_eq!(box_shadow_offset(0.0, 12.0, false), (0.0 as CGFloat, -12.0 as CGFloat));
        assert_eq!(box_shadow_offset(-3.0, 0.0, false), (-3.0 as CGFloat, 0.0 as CGFloat));
    }

    /// Regression: `-[CALayer setShadowPath:]` takes a `^{CGPath=}`. Handing
    /// it a bare `*const c_void` (`^v`) trips objc2's debug-mode encoding
    /// verifier and SIGABRTs the process the instant a shadowed view lays out
    /// — which is exactly how the iOS twin of this code crashed mid-layout
    /// during development, and the same trap `icon.rs` guards. A full mount
    /// test needs a live AppKit layer + main thread, so pinning the encoding
    /// constant is the closest reachable guard.
    #[test]
    fn cgpath_pointer_encodes_as_typed_cgpath() {
        assert_eq!(
            CGPathRef::ENCODING_REF,
            Encoding::Pointer(&Encoding::Struct("CGPath", &[])),
            "*mut CGPathRef must encode as ^{{CGPath=}} for -[CALayer setShadowPath:]",
        );
    }

    /// X never depends on flipped-ness — only the vertical axis is mirrored.
    #[test]
    fn x_is_unaffected_by_flippedness() {
        let (fx, _) = box_shadow_offset(7.5, 1.0, true);
        let (nx, _) = box_shadow_offset(7.5, 1.0, false);
        assert_eq!(fx, nx);
        assert_eq!(fx, 7.5 as CGFloat);
    }
}
