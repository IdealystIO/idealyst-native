//! Per-property animation writers for the macOS backend.
//!
//! Cross-platform animation drives `set_animated_f32` /
//! `set_animated_color` per frame for each animated property.
//! This module routes the `AnimProp` enum to the matching
//! NSView / CALayer setter. AppKit + Core Animation overlap a lot
//! with UIKit + Core Animation, so the shape mirrors
//! `backend-ios-mobile/src/imp/animated.rs` — main differences are
//! `NSView.setAlphaValue:` instead of `setAlpha:` and writing
//! transforms via the CALayer (NSView itself has no transform).

use std::cell::RefCell;
use std::collections::HashMap;

use runtime_core::animation::AnimProp;
use objc2::rc::Retained;
use objc2::{msg_send, msg_send_id};
use objc2_app_kit::NSView;
use objc2_foundation::NSObject;

use crate::imp::MacosNode;

/// Per-view cached transform state. Mirrors the iOS pattern — we
/// hold the components separately so writing TranslateX doesn't
/// destroy the previously-set ScaleX, and vice versa. CALayer's
/// `transform` is a single `CATransform3D`, so we rebuild it from
/// the cached components on each write.
///
/// Percent translates (`Transform::TranslateX(Length::Percent(_))`
/// from `StyleRules`) can't be resolved at apply-style time because
/// the view's bounds aren't known yet — we stash them separately
/// and the layout pass calls [`sync_transform_after_layout`] to fold
/// them in once Taffy has assigned a frame.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct AnimatedState {
    // STATIC transform components — written by `apply_static_transform` from
    // `style.transform`. Reset to identity on every restyle.
    pub(crate) translate_x: f32,
    pub(crate) translate_y: f32,
    pub(crate) scale_x: f32,
    pub(crate) scale_y: f32,
    pub(crate) rotate_z: f32,
    // ANIMATED transform components — written by `set_animated_f32` from a bound
    // `AnimatedValue`. Kept SEPARATE from the static slots so a restyle (e.g. a
    // theme swap re-running `apply_style`) doesn't clobber an in-flight
    // animation: the Switch thumb's `TranslateX` survived as a static slot until
    // a theme toggle reset it to 0 and slammed the thumb back to "off".
    // `rebuild_transform` composes static ∘ animated.
    pub(crate) anim_translate_x: f32,
    pub(crate) anim_translate_y: f32,
    pub(crate) anim_scale_x: f32,
    pub(crate) anim_scale_y: f32,
    pub(crate) anim_rotate_z: f32,
    /// Pending percent translateX in 0..=100 units. `None` if no
    /// percent translate is set; the layout pass resolves it
    /// against the view's width on each frame.
    pub(crate) static_translate_pct_x: Option<f32>,
    pub(crate) static_translate_pct_y: Option<f32>,
}

impl AnimatedState {
    pub(crate) fn new() -> Self {
        Self {
            translate_x: 0.0,
            translate_y: 0.0,
            scale_x: 1.0,
            scale_y: 1.0,
            rotate_z: 0.0,
            anim_translate_x: 0.0,
            anim_translate_y: 0.0,
            anim_scale_x: 1.0,
            anim_scale_y: 1.0,
            anim_rotate_z: 0.0,
            static_translate_pct_x: None,
            static_translate_pct_y: None,
        }
    }
}

/// Per-backend cache keyed by NSView pointer. Owned by the backend.
pub(crate) type AnimatedStateMap = HashMap<usize, RefCell<AnimatedState>>;

/// Apply the static `transform: [...]` from a `StyleRules` to the
/// view's transform state. Called from `apply_style`. Percent
/// translates are stashed for resolution in the layout pass;
/// everything else (px translate, scale, rotate) goes straight into
/// the matrix.
///
/// Resets the static slots first so removing the transform reverts
/// to identity — matches the iOS behavior in
/// `backend-ios-mobile/src/imp/animated.rs::impl_apply_static_transform`.
pub(crate) fn apply_static_transform(
    node: &MacosNode,
    style: &runtime_core::StyleRules,
    states: &mut AnimatedStateMap,
) {
    use runtime_core::{Length, Transform};
    let view = node.as_view();
    let key = view as *const NSView as usize;
    let state = states
        .entry(key)
        .or_insert_with(|| RefCell::new(AnimatedState::new()));
    {
        let mut s = state.borrow_mut();
        s.translate_x = 0.0;
        s.translate_y = 0.0;
        s.scale_x = 1.0;
        s.scale_y = 1.0;
        s.rotate_z = 0.0;
        s.static_translate_pct_x = None;
        s.static_translate_pct_y = None;

        if let Some(ops) = style.transform.as_ref() {
            for op in ops {
                match op {
                    Transform::TranslateX(Length::Px(v)) => s.translate_x = *v,
                    Transform::TranslateY(Length::Px(v)) => s.translate_y = *v,
                    Transform::TranslateX(Length::Percent(v)) => {
                        s.static_translate_pct_x = Some(*v)
                    }
                    Transform::TranslateY(Length::Percent(v)) => {
                        s.static_translate_pct_y = Some(*v)
                    }
                    Transform::TranslateX(Length::Auto)
                    | Transform::TranslateY(Length::Auto) => {
                        // Auto on translate is meaningless — leave identity.
                    }
                    Transform::Scale(v) => {
                        s.scale_x = *v;
                        s.scale_y = *v;
                    }
                    Transform::ScaleXY { x, y } => {
                        s.scale_x = *x;
                        s.scale_y = *y;
                    }
                    Transform::Rotate(deg) => s.rotate_z = *deg,
                    // Skew not representable in our 2D affine setup.
                    Transform::SkewX(_) | Transform::SkewY(_) => {}
                }
            }
        }
    }
    // Apply with current bounds (likely 0 at apply-style time for
    // percent-sized views — the layout pass calls this again with
    // real bounds via `sync_transform_after_layout`).
    rebuild_transform(view, &state.borrow());
}

/// Re-apply transforms for any view with non-identity animated
/// state after the layout pass has assigned frames. Used for animated
/// scale/rotate so the center-pivot compensation re-resolves
/// against the new bounds. (Static percent translates are NOT in
/// the layer transform on macOS — they go through frame-origin
/// adjustment via [`static_translate_offset`].)
pub(crate) fn sync_transform_after_layout(
    view: &NSView,
    states: &AnimatedStateMap,
) {
    let key = view as *const NSView as usize;
    let Some(state) = states.get(&key) else { return };
    rebuild_transform(view, &state.borrow());
}

/// Compute the frame-origin offset for `view`'s static percent
/// translates, resolved against the Taffy-computed frame size. Used
/// by the layout pass to apply CSS-style `translate(50%, -50%)` as
/// a frame shift rather than a layer transform.
///
/// Returns `(0.0, 0.0)` if no state exists or no percent translates
/// are present.
pub(crate) fn static_translate_offset(
    view: &NSView,
    states: &AnimatedStateMap,
    frame_w: f32,
    frame_h: f32,
) -> (f64, f64) {
    let key = view as *const NSView as usize;
    let Some(state) = states.get(&key) else { return (0.0, 0.0) };
    let s = state.borrow();
    let tx = s
        .static_translate_pct_x
        .map(|p| p as f64 / 100.0 * frame_w as f64)
        .unwrap_or(0.0);
    let ty = s
        .static_translate_pct_y
        .map(|p| p as f64 / 100.0 * frame_h as f64)
        .unwrap_or(0.0);
    (tx, ty)
}

/// Write a scalar animation property on `node`. Routes through the
/// CALayer for transforms (NSView itself has no transform property)
/// and through NSView's `setAlphaValue:` for opacity.
pub(crate) fn set_animated_f32(
    node: &MacosNode,
    prop: AnimProp,
    value: f32,
    states: &mut AnimatedStateMap,
) {
    let view = node.as_view();
    let key = view as *const NSView as usize;
    let state = states.entry(key).or_insert_with(|| RefCell::new(AnimatedState::new()));

    match prop {
        AnimProp::Opacity => {
            // NSView's `alphaValue` is the AppKit equivalent of UIView's
            // `alpha`. It cascades through the view hierarchy and is
            // CALayer-independent — works even for layer-less NSViews.
            let _: () = unsafe { msg_send![view, setAlphaValue: value as f64] };
        }
        // Animated transforms write the `anim_*` slots, NOT the static ones, so
        // a concurrent restyle (which resets the static slots) can't wipe them.
        AnimProp::TranslateX => {
            state.borrow_mut().anim_translate_x = value;
            rebuild_transform(view, &state.borrow());
        }
        AnimProp::TranslateY => {
            state.borrow_mut().anim_translate_y = value;
            rebuild_transform(view, &state.borrow());
        }
        AnimProp::Scale => {
            let mut s = state.borrow_mut();
            s.anim_scale_x = value;
            s.anim_scale_y = value;
            rebuild_transform(view, &s);
        }
        AnimProp::ScaleX => {
            state.borrow_mut().anim_scale_x = value;
            rebuild_transform(view, &state.borrow());
        }
        AnimProp::ScaleY => {
            state.borrow_mut().anim_scale_y = value;
            rebuild_transform(view, &state.borrow());
        }
        AnimProp::RotateZ => {
            state.borrow_mut().anim_rotate_z = value;
            rebuild_transform(view, &state.borrow());
        }
        AnimProp::ZIndex => {
            // CALayer's `zPosition` is the closest equivalent. AppKit
            // sibling ordering normally goes by subview index; setting
            // zPosition reorders at draw time.
            let layer: Option<Retained<NSObject>> =
                unsafe { msg_send_id![view, layer] };
            if let Some(layer) = layer {
                let _: () = unsafe { msg_send![&layer, setZPosition: value as f64] };
            }
        }
        // Other props are no-ops on macOS for v1. Width/height/padding
        // /margin animations go through `apply_style` + Taffy
        // re-compute, not direct setters.
        _ => {}
    }
}

/// Write a color animation property on `node`. Routes through
/// CALayer's `backgroundColor` for `BackgroundColor`; through the
/// widget's text color (NSTextField.textColor) for `ForegroundColor`
/// on labels; defers other kinds until the matching primitives land.
pub(crate) fn set_animated_color(
    node: &MacosNode,
    prop: AnimProp,
    value: [f32; 4],
) {
    let view = node.as_view();
    let ns_color = unsafe {
        objc2_app_kit::NSColor::colorWithSRGBRed_green_blue_alpha(
            value[0] as f64,
            value[1] as f64,
            value[2] as f64,
            value[3] as f64,
        )
    };

    match prop {
        AnimProp::BackgroundColor => {
            // Ensure layer-backing — animated color writes need the
            // CALayer to exist. `setWantsLayer:` is idempotent.
            let _: () = unsafe { msg_send![view, setWantsLayer: true] };
            let layer: Option<Retained<NSObject>> =
                unsafe { msg_send_id![view, layer] };
            if let Some(layer) = layer {
                let cg: crate::imp::CGColorRef = unsafe { msg_send![&ns_color, CGColor] };
                if !cg.0.is_null() {
                    let _: () = unsafe { msg_send![&layer, setBackgroundColor: cg] };
                }
            }
        }
        AnimProp::ForegroundColor => {
            // Per-widget text-color routing. AppKit's NSTextField (and
            // NSTextView, when we add it) own their text color via
            // `setTextColor:` — neither inherits from the view's
            // layer or window tint. iOS makes the same split (see
            // `IosNode::Label` arm in `backend-ios-mobile/src/imp/animated.rs`).
            match node {
                MacosNode::Label(label) => {
                    let _: () = unsafe { msg_send![label.as_ref(), setTextColor: &*ns_color] };
                }
                MacosNode::View(_) => {
                    // No NSView analogue to UIView's `tintColor` —
                    // skip. Authors targeting icon strokes / interactive
                    // chrome will land here once those primitives exist.
                }
            }
        }
        _ => {}
    }
}

/// Rebuild and apply the per-view CATransform3D from the cached
/// component values. CATransform3D is a 4x4 matrix; we compose
/// translate × rotate × scale in that order (matches CSS transform
/// semantics: scale applies first, then rotate, then translate).
///
/// `CATransform3DMakeAffineTransform` would be one path; building
/// from a `CGAffineTransform` and embedding is another. We use the
/// raw CATransform3D struct directly — it's `#[repr(C)]` 16 doubles
/// — and let `setTransform:` accept it through the standard ObjC
/// type encoding.
fn rebuild_transform(view: &NSView, state: &AnimatedState) {
    // Layer-back the view; transforms only render through the
    // CALayer. `setWantsLayer:true` is a no-op if already set.
    let _: () = unsafe { msg_send![view, setWantsLayer: true] };
    let layer: Option<Retained<NSObject>> = unsafe { msg_send_id![view, layer] };
    let Some(layer) = layer else { return };

    // Read current bounds. Used for center-pivot compensation
    // below. Static percent translates are NOT folded into the
    // layer transform on macOS — they go through the frame.origin
    // adjustment in the layout pass instead ([`static_translate_offset`]),
    // because layer-backed NSViews don't honor pure-static
    // layer.transform translates the same way UIKit does. The
    // layer transform here is reserved for animated transforms
    // (scale, rotate, animated translate via `set_animated_f32`).
    let bounds: objc2_foundation::CGRect = unsafe { msg_send![view, bounds] };
    let w = bounds.size.width as f64;
    let h = bounds.size.height as f64;
    let cx = w / 2.0;
    let cy = h / 2.0;

    // Compose static ∘ animated: translates add, scales multiply, rotations
    // add. So a view with no static transform but an animated `TranslateX`
    // (the Switch thumb) keeps that translate across a restyle, and a static
    // transform + an animated one combine rather than overwrite.
    let tx = (state.translate_x + state.anim_translate_x) as f64;
    let ty = (state.translate_y + state.anim_translate_y) as f64;

    // 2x2 linear part: rotate then scale. Mirrors the matrix
    // `build_transform_matrix` produced — keeping the derivation
    // explicit here so the center-pivot compensation right below
    // can refer to the entries by name.
    let rz_rad = ((state.rotate_z + state.anim_rotate_z) as f64).to_radians();
    let cos = rz_rad.cos();
    let sin = rz_rad.sin();
    let sx = (state.scale_x * state.anim_scale_x) as f64;
    let sy = (state.scale_y * state.anim_scale_y) as f64;
    let a = cos * sx; //  column 1, row 1 — x-basis x
    let b = sin * sx; //  column 1, row 2 — x-basis y
    let c = -sin * sy; // column 2, row 1 — y-basis x
    let d = cos * sy; //  column 2, row 2 — y-basis y

    // Center-pivot compensation. UIKit's `view.transform` pivots
    // around `view.center` (layer.anchorPoint defaults to 0.5, 0.5);
    // AppKit layer-backed NSViews default `anchorPoint` to (0, 0),
    // so a CALayer transform pivots around the top-left corner.
    //
    // We pre/post-compose with translates of ±(cx, cy) so scale and
    // rotate pivot around the view's center — matching UIKit /
    // iOS / web semantics. Without this, the sun glare scales out
    // from the top-left corner instead of growing from its center,
    // and the welcome text entrance translates+scales feel
    // off-axis.
    //
    // Derivation: T(cx, cy) × M × T(-cx, -cy) gives the same linear
    // part but shifts translation by (cx*(1-a) - b*cy,
    // cy*(1-d) - c*cx).
    let center_tx = cx * (1.0 - a) - b * cy;
    let center_ty = cy * (1.0 - d) - c * cx;

    let m = CATransform3D {
        m11: a, m12: b, m13: 0.0, m14: 0.0,
        m21: c, m22: d, m23: 0.0, m24: 0.0,
        m31: 0.0, m32: 0.0, m33: 1.0, m34: 0.0,
        m41: tx + center_tx, m42: ty + center_ty, m43: 0.0, m44: 1.0,
    };
    let _: () = unsafe { msg_send![&layer, setTransform: m] };
}

/// The uniform scale currently on `layer` (its `transform.m11`). The icon
/// backend reads this to skip re-applying an unchanged scale every layout
/// pass.
pub(crate) fn current_layer_scale(layer: &NSObject) -> f64 {
    let t: CATransform3D = unsafe { msg_send![layer, transform] };
    t.m11
}

/// The `(x, y)` translate currently on `view`'s layer (`transform.m41`/`m42`).
/// `hitTest:` uses this to make transform-positioned views clickable where they
/// VISUALLY render instead of where their untransformed frame sits — AppKit
/// hit-tests by frame and ignores the layer transform, unlike web/iOS. For a
/// pure translate the center-pivot compensation is zero, so `m41`/`m42` are
/// exactly the translate. Returns `(0, 0)` when the view isn't layer-backed.
pub(crate) fn view_layer_translate(view: &NSView) -> (f64, f64) {
    let layer: *mut objc2::runtime::AnyObject = unsafe { msg_send![view, layer] };
    if layer.is_null() {
        return (0.0, 0.0);
    }
    let t: CATransform3D = unsafe { msg_send![layer, transform] };
    (t.m41, t.m42)
}

/// Apply a uniform scale (about the layer's anchor point) to `layer`'s
/// `transform`. Shared with the icon backend, which scales a fixed-size
/// glyph sublayer down to its laid-out box. Uses the same raw
/// `CATransform3D` struct the animated-transform path uses.
pub(crate) fn apply_layer_scale(layer: &NSObject, s: f64) {
    let m = CATransform3D {
        m11: s, m12: 0.0, m13: 0.0, m14: 0.0,
        m21: 0.0, m22: s, m23: 0.0, m24: 0.0,
        m31: 0.0, m32: 0.0, m33: 1.0, m34: 0.0,
        m41: 0.0, m42: 0.0, m43: 0.0, m44: 1.0,
    };
    let _: () = unsafe { msg_send![layer, setTransform: m] };
}

/// CATransform3D layout — 4x4 column-major matrix of f64. Matches
/// the C ABI Core Animation exposes.
#[repr(C)]
#[derive(Copy, Clone, Debug)]
struct CATransform3D {
    m11: f64, m12: f64, m13: f64, m14: f64,
    m21: f64, m22: f64, m23: f64, m24: f64,
    m31: f64, m32: f64, m33: f64, m34: f64,
    m41: f64, m42: f64, m43: f64, m44: f64,
}

unsafe impl objc2::encode::Encode for CATransform3D {
    const ENCODING: objc2::encode::Encoding = objc2::encode::Encoding::Struct(
        "CATransform3D",
        &[
            f64::ENCODING, f64::ENCODING, f64::ENCODING, f64::ENCODING,
            f64::ENCODING, f64::ENCODING, f64::ENCODING, f64::ENCODING,
            f64::ENCODING, f64::ENCODING, f64::ENCODING, f64::ENCODING,
            f64::ENCODING, f64::ENCODING, f64::ENCODING, f64::ENCODING,
        ],
    );
}

