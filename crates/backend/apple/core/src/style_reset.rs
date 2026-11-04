//! What a view goes back to when a style drops a prop.
//!
//! `apply_style` hands over a node's full merged rules, so a prop they leave
//! unset was dropped, and the view returns to the value it would have with no
//! style. Web gets this from the class swap. UIKit and AppKit keep whatever was
//! last written, so the backends write it back, and only on a real change: these
//! paths run on every hover, press and focus flip, and a prop no style ever
//! stated must not be touched, since the same native field may belong to an
//! animation.
//!
//! The no-style value is read from the view rather than hard-coded where the
//! platform has one (a text field's font), because it differs per control.

/// What to do with a view's static opacity on one restyle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StaticOpacityUpdate {
    /// The value to remember as the view's static opacity. `None` while no style
    /// has ever stated one.
    pub remember: Option<f32>,
    /// The value to write to the native field, if any.
    pub write: Option<f32>,
}

/// One restyle's opacity decision. `stated` is the style's `opacity`, `last` the
/// remembered static value, and `animated` whether an animation owns the native
/// field: `alpha` is one field on both Apple toolkits, and an animated value
/// overrides the stylesheet, as it does on web, where it is an inline style.
///
/// A drop remembers `1.0` rather than forgetting, so the next stated value
/// transitions from where the view is.
pub fn static_opacity_update(
    stated: Option<f32>,
    last: Option<f32>,
    animated: bool,
) -> StaticOpacityUpdate {
    let remember = stated.or(last.map(|_| OPAQUE));
    let write = match (stated, last) {
        (Some(v), _) => Some(v),
        (None, Some(prev)) if prev != OPAQUE => Some(OPAQUE),
        (None, _) => None,
    };
    StaticOpacityUpdate { remember, write: if animated { None } else { write } }
}

const OPAQUE: f32 = 1.0;

#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
pub use natural::*;

#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
mod natural {
    use objc2::ffi::{objc_getAssociatedObject, objc_setAssociatedObject, OBJC_ASSOCIATION_RETAIN_NONATOMIC};
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject};
    use objc2_foundation::NSString;

    /// The address is the associated-object key for a view's unstyled font.
    static NATURAL_FONT_KEY: u8 = 0;

    fn associated(view: &NSObject) -> *mut AnyObject {
        let key = &NATURAL_FONT_KEY as *const u8 as *const std::ffi::c_void;
        unsafe { objc_getAssociatedObject(view as *const NSObject as *const _, key) as *mut AnyObject }
    }

    fn associate(view: &NSObject, value: *mut AnyObject) {
        let key = &NATURAL_FONT_KEY as *const u8 as *const std::ffi::c_void;
        unsafe {
            objc_setAssociatedObject(
                view as *const NSObject as *mut _,
                key,
                value as *mut _,
                OBJC_ASSOCIATION_RETAIN_NONATOMIC,
            )
        };
    }

    /// Keep `view`'s font before a style first replaces it. Later calls keep the
    /// first: that is the one the view had with no style. A `UITextView` can have
    /// no font at all, so nil is kept as `NSNull`.
    pub fn remember_natural_font(view: &NSObject) {
        if !associated(view).is_null() {
            return;
        }
        let font: *mut AnyObject = unsafe { msg_send![view, font] };
        if font.is_null() {
            let null: Retained<NSObject> = unsafe { objc2::msg_send_id![objc2::class!(NSNull), null] };
            associate(view, Retained::as_ptr(&null) as *mut AnyObject);
        } else {
            associate(view, font);
        }
    }

    /// Give `view` back the font [`remember_natural_font`] kept, if a style has
    /// replaced it since the last restore. `true` when it wrote.
    pub fn restore_natural_font(view: &NSObject) -> bool {
        let kept = associated(view);
        if kept.is_null() {
            return false;
        }
        let is_nil: bool = unsafe { msg_send![kept, isKindOfClass: objc2::class!(NSNull)] };
        let font: *mut AnyObject = if is_nil { std::ptr::null_mut() } else { kept };
        let _: () = unsafe { msg_send![view, setFont: font] };
        associate(view, std::ptr::null_mut());
        true
    }

    /// The layer key holding what a layer's clip was before a style first touched
    /// it. Kept for the layer's life.
    const NATURAL_CLIP_KEY: &str = "idealyst_natural_clip";

    /// Keep `layer`'s `masksToBounds` before a style first changes it. Later calls
    /// keep the first.
    pub fn remember_natural_clip(layer: &NSObject) {
        let key = NSString::from_str(NATURAL_CLIP_KEY);
        let kept: *mut NSObject = unsafe { msg_send![layer, valueForKey: &*key] };
        if kept.is_null() {
            let current: bool = unsafe { msg_send![layer, masksToBounds] };
            let number: Retained<NSObject> =
                unsafe { objc2::msg_send_id![objc2::class!(NSNumber), numberWithBool: current] };
            let _: () = unsafe { msg_send![layer, setValue: &*number, forKey: &*key] };
        }
    }

    /// Apply `clip` (`backend_apple_core::clip::clips_to_bounds`) to `layer`'s
    /// `masksToBounds`, which is also `UIView.clipsToBounds`. With no `overflow` it
    /// goes back to what [`remember_natural_clip`] kept, not to a flat `false`: a
    /// scroll view and an image view clip with no style at all. A layer no style has
    /// touched is left alone.
    pub fn apply_clip(layer: &NSObject, clip: Option<bool>) {
        if let Some(clip) = clip {
            remember_natural_clip(layer);
            let _: () = unsafe { msg_send![layer, setMasksToBounds: clip] };
            return;
        }
        let key = NSString::from_str(NATURAL_CLIP_KEY);
        let kept: *mut NSObject = unsafe { msg_send![layer, valueForKey: &*key] };
        if kept.is_null() {
            return;
        }
        let natural: bool = unsafe { msg_send![kept, boolValue] };
        let current: bool = unsafe { msg_send![layer, masksToBounds] };
        if current != natural {
            let _: () = unsafe { msg_send![layer, setMasksToBounds: natural] };
        }
    }

    /// The layer key the corner-radius paths stash the requested radius under, for
    /// the layout pass to re-clamp.
    pub const REQUESTED_CORNER_RADIUS_KEY: &str = "idealyst_requested_corner_radius";

    /// A style has no radius any more: square the layer and forget the stash, or
    /// the next layout pass brings the radius back. No write for a layer no style
    /// ever rounded, which may round itself.
    pub fn clear_requested_corner_radius(layer: &NSObject) {
        let key = NSString::from_str(REQUESTED_CORNER_RADIUS_KEY);
        let kept: *mut NSObject = unsafe { msg_send![layer, valueForKey: &*key] };
        if kept.is_null() {
            return;
        }
        let nil: *mut NSObject = std::ptr::null_mut();
        let _: () = unsafe { msg_send![layer, setValue: nil, forKey: &*key] };
        let _: () = unsafe { msg_send![layer, setCornerRadius: 0.0_f64] };
        crate::sublayers::round_named_sublayers(layer, "idealyst_gradient", 0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A variant with `opacity: 0.5` flipping to one without it stayed at 0.5 on
    /// iOS and macOS: the static path only wrote when a style stated a value.
    #[test]
    fn regression_apple_dropped_opacity_kept_its_old_value() {
        let dropped = static_opacity_update(None, Some(0.5), false);
        assert_eq!(dropped.write, Some(1.0));
        assert_eq!(dropped.remember, Some(1.0), "the next stated value transitions from here");
    }

    #[test]
    fn opacity_no_style_ever_stated_is_never_written() {
        let update = static_opacity_update(None, None, false);
        assert_eq!(update, StaticOpacityUpdate { remember: None, write: None });
    }

    #[test]
    fn a_second_restyle_after_the_drop_writes_nothing() {
        assert_eq!(static_opacity_update(None, Some(1.0), false).write, None);
    }

    /// An `AnimatedValue` on the same field keeps it through a restyle, stated or
    /// dropped, and the static value is still tracked for when a presence lets go.
    #[test]
    fn an_animated_opacity_is_not_clobbered() {
        assert_eq!(
            static_opacity_update(None, Some(0.5), true),
            StaticOpacityUpdate { remember: Some(1.0), write: None }
        );
        assert_eq!(
            static_opacity_update(Some(0.25), None, true),
            StaticOpacityUpdate { remember: Some(0.25), write: None }
        );
    }

    #[test]
    fn a_stated_opacity_is_written() {
        assert_eq!(static_opacity_update(Some(0.25), Some(1.0), false).write, Some(0.25));
    }
}
