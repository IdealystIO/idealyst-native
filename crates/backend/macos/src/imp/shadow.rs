//! Box shadow (`StyleRules.shadow`) for the macOS backend.
//!
//! Thin AppKit adapter over [`backend_apple_core::shadow_layer`]: this file
//! resolves `NSView → CALayer`, converts the color through `NSColor`, and
//! settles the one genuinely AppKit-shaped question (the Y sign, below). Every
//! layer-level decision — where the shadow gets painted, the `shadowPath`, the
//! sibling layer's whole lifecycle — is shared with iOS so the two backends
//! cannot drift (Rule #7).
//!
//! Before this module macOS honored only `text_shadow` (the GLYPH shadow on
//! labels, see [`super::text_style`]) and dropped `shadow` on the floor — so an
//! idea-ui `Elevated` Card rendered with a drop shadow on iOS and web, and
//! perfectly flat on macOS, from the same author tree.
//!
//! ## The one non-obvious invariant: the Y sign follows `isFlipped`
//!
//! `Shadow`'s `(x, y)` is web semantics: `+y` casts DOWNWARD. A CALayer's
//! `shadowOffset` is expressed in the layer's own coordinate space, and for a
//! layer-backed `NSView` AppKit flips the backing layer's geometry to match the
//! view. The framework's container views (`view` / `pressable` / `link`) are
//! `IdealystFlippedView` — `isFlipped == true`, y-down, exactly like UIKit — so
//! they take `+y` directly, the same value iOS passes. A *non-flipped* view
//! (`NSTextField` and the other native controls) is y-up and must negate, which
//! is why [`super::text_style::text_shadow_offset`] negates unconditionally: it
//! only ever runs on labels. Getting this wrong flips the shadow to the wrong
//! side of the box on macOS only — precisely the per-platform divergence Rule
//! #7 bans. [`box_shadow_offset`] is pure so the sign convention is unit-tested
//! off the main thread.

use backend_apple_core::shadow::{shadow_placement, ShadowPlacement};
use backend_apple_core::shadow_layer;
use objc2::msg_send;
use objc2::runtime::NSObject;
use objc2_app_kit::NSView;
use objc2_foundation::{CGFloat, CGSize};
use runtime_shared::StyleRules;

use super::color_to_nscolor;

/// Translate a `Shadow`'s `(x, y)` (web semantics: `+x` right, `+y` DOWN) into
/// a CALayer `shadowOffset` for a view with the given flipped-ness.
///
/// See the module docs: a flipped view's backing layer is y-down (pass `+y`
/// straight through, matching iOS); a non-flipped view's is y-up (negate).
pub(crate) fn box_shadow_offset(x: f32, y: f32, is_flipped: bool) -> (CGFloat, CGFloat) {
    let h = if is_flipped { y as CGFloat } else { -(y as CGFloat) };
    (x as CGFloat, h)
}

/// The view's backing layer, or `None`.
///
/// `NSView` is layer-*optional* (unlike `UIView`, which is layer-mandatory), so
/// `[view layer]` is nil until something set `wantsLayer`. A view with no layer
/// carries no shadow, which makes every routine here a no-op for it — hence the
/// raw `msg_send!` + null check rather than `msg_send_id!`, which asserts
/// non-nil and would panic when the layout pass reaches an unstyled NSView.
fn layer_of(view: &NSView) -> Option<&NSObject> {
    let ptr: *mut NSObject = unsafe { msg_send![view, layer] };
    (!ptr.is_null()).then(|| unsafe { &*ptr })
}

/// Apply — or clear — the view's box shadow. Call from `apply_style_to_view`
/// after the border, on a view that is already layer-backed.
pub(crate) fn apply_box_shadow(view: &NSView, style: &StyleRules) {
    let Some(layer) = layer_of(view) else { return };

    match shadow_placement(style) {
        ShadowPlacement::None => {
            // Clear whatever a previous apply left, so a reactively removed
            // shadow actually stops painting.
            shadow_layer::clear_own_shadow(layer);
            shadow_layer::drop_sibling(layer);
        }
        ShadowPlacement::OwnLayer => {
            // A style may have flipped from clipped to unclipped; the sibling
            // would otherwise double the shadow.
            shadow_layer::drop_sibling(layer);
            write(view, layer, style);
            shadow_layer::sync_own_shadow_path(layer);
        }
        ShadowPlacement::Sibling => {
            // The view's own layer is about to be bounds-masked, which would
            // clip its shadow away — paint on the peer instead.
            shadow_layer::clear_own_shadow(layer);
            let sib = shadow_layer::ensure_sibling(layer);
            write(view, &sib, style);
            // Parents it if the view is already in the hierarchy; the layout
            // pass calls `sync_shadow_path` again once it has a real frame.
            shadow_layer::sync_sibling(layer);
        }
    }
}

/// Push the authored shadow's appearance onto `target`, converting the color
/// through `NSColor` and the offset through the flipped-ness rule.
fn write(view: &NSView, target: &NSObject, style: &StyleRules) {
    let Some(shadow) = &style.shadow else { return };
    let ns_color = color_to_nscolor(&shadow.color);
    let cg: super::CGColorRef = unsafe { msg_send![&*ns_color, CGColor] };
    let is_flipped: bool = unsafe { msg_send![view, isFlipped] };
    let (width, height) = box_shadow_offset(shadow.x, shadow.y, is_flipped);
    shadow_layer::write_shadow_props(target, cg, CGSize { width, height }, shadow.blur);
}

/// Re-trace the box shadow against the view's current bounds and corner radius,
/// and keep any sibling shadow layer parented, positioned and pathed.
///
/// Call from the post-frame hook AFTER `sync_corner_radius` so the path follows
/// the final clamped curve. Both halves are no-ops for a view without a shadow,
/// so the layout pass calls this blindly.
pub(crate) fn sync_shadow_path(view: &NSView) {
    let Some(layer) = layer_of(view) else { return };
    shadow_layer::sync_own_shadow_path(layer);
    shadow_layer::sync_sibling(layer);
}

/// Unparent a view's sibling shadow layer as the view leaves its parent.
///
/// **Required on every removal path.** The sibling lives in the *parent's*
/// layer, so `removeFromSuperview` alone leaves the shadow painting where the
/// card used to be. Removing an *ancestor* needs no call: the sibling sits
/// inside that ancestor's subtree and goes with it. The handle survives, so a
/// reparent (remove + insert) re-attaches the same layer on the next layout.
pub(crate) fn detach_shadow_sibling(view: &NSView) {
    if let Some(layer) = layer_of(view) {
        shadow_layer::detach_sibling(layer);
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

    /// X never depends on flipped-ness — only the vertical axis is mirrored.
    #[test]
    fn x_is_unaffected_by_flippedness() {
        let (fx, _) = box_shadow_offset(7.5, 1.0, true);
        let (nx, _) = box_shadow_offset(7.5, 1.0, false);
        assert_eq!(fx, nx);
        assert_eq!(fx, 7.5 as CGFloat);
    }
}
