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

// The 4x4 matrix ABI is shared with the iOS backend — one definition, one
// host-testable encoding (see `backend_apple_core::cg`).
use backend_apple_core::cg::CATransform3D;
use std::cell::RefCell;
use std::collections::HashMap;

use runtime_shared::animation::AnimProp;
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
/// from `StyleRules`) are kept as percents and resolved against the
/// view's own bounds every time the matrix is built
/// ([`compose_layer_transform`]); the layout pass calls
/// [`sync_transform_after_layout`] after each `setFrame:` so they follow
/// the laid-out size. They live in the layer transform like every other
/// translate, so a `transitions { transform }` animates them.
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
    /// Last animated `AnimProp::ZIndex` (`layer.zPosition`). While set,
    /// `apply_style` leaves `zPosition` alone, so a restyle can't cancel
    /// the animated value with the static `stacking_z()` — the animated
    /// value overrides the static `z_index`, as on web.
    pub(crate) anim_z_index: Option<f32>,
    /// Static percent translateX in 0..=100 units, `None` when the style
    /// has none. Resolved against the view's width when the matrix is
    /// built, and added to the px `translate_x` (CSS sums the entries of
    /// a `translateX(100%) translateX(-24px)` list).
    pub(crate) static_translate_pct_x: Option<f32>,
    pub(crate) static_translate_pct_y: Option<f32>,
    /// The opacity the last style stated, `Some(1.0)` once it has been dropped;
    /// `None` while no style has stated one. See
    /// `backend_apple_core::style_reset::static_opacity_update`.
    pub(crate) static_opacity: Option<f32>,
    /// `AnimProp::Opacity` has written `alphaValue`, which is the one field for
    /// static and animated opacity alike. The animated value overrides the
    /// stylesheet from then on, as it does on web.
    pub(crate) anim_opacity: bool,
    /// A presence state stating an opacity is in force on this view.
    pub(crate) presence_opacity: bool,
    /// `AnimProp::ForegroundColor` has written `textColor`.
    pub(crate) anim_text_color: bool,
    /// True after the first `apply_static_transform` on this view.
    /// Gates `transitions { transform: … }`: the initial apply snaps
    /// (CSS parity — transitions fire on *changes*), later applies
    /// animate via a CABasicAnimation (see
    /// [`crate::transform_transition_policy`]).
    pub(crate) static_transform_seen: bool,
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
            anim_z_index: None,
            static_translate_pct_x: None,
            static_translate_pct_y: None,
            static_opacity: None,
            anim_opacity: false,
            presence_opacity: false,
            anim_text_color: false,
            static_transform_seen: false,
        }
    }
}

/// Per-backend cache keyed by NSView pointer. Owned by the backend.
pub(crate) type AnimatedStateMap = HashMap<usize, RefCell<AnimatedState>>;

/// Apply `style.opacity` to the view's `alphaValue`, going back to opaque when a
/// style drops it. `NSView.setAlphaValue:` matches iOS's `UIView.setAlpha:`, and the
/// animated path writes the same field, so a sheet resting at `opacity: 0.0` that a
/// raf body fades in reaches the value it is given.
pub(crate) fn apply_static_opacity(
    view: &NSView,
    style: &runtime_shared::StyleRules,
    states: &mut AnimatedStateMap,
) {
    let key = view as *const NSView as usize;
    let state = states.entry(key).or_insert_with(|| RefCell::new(AnimatedState::new()));
    let mut s = state.borrow_mut();
    let update = backend_apple_core::style_reset::static_opacity_update(
        style.opacity.as_ref().map(|t| t.resolve()),
        s.static_opacity,
        s.anim_opacity || s.presence_opacity,
    );
    s.static_opacity = update.remember;
    if let Some(alpha) = update.write {
        let _: () = unsafe { msg_send![view, setAlphaValue: alpha as f64] };
    }
}

/// Apply the static `transform: [...]` from a `StyleRules` to the
/// view's transform state and its layer matrix. Called from
/// `apply_style`. Percent translates resolve against the view's current
/// bounds (0×0 before its first layout, which then resolves them in
/// [`sync_transform_after_layout`]).
///
/// Resets the static slots first so removing the transform reverts
/// to identity — matches the iOS behavior in
/// `backend-ios-mobile/src/imp/animated.rs::impl_apply_static_transform`.
pub(crate) fn apply_static_transform(
    node: &MacosNode,
    style: &runtime_shared::StyleRules,
    states: &mut AnimatedStateMap,
) {
    use runtime_shared::{Length, Transform};
    let view = node.as_view();
    let key = view as *const NSView as usize;
    let state = states
        .entry(key)
        .or_insert_with(|| RefCell::new(AnimatedState::new()));
    // Snapshot for `transitions { transform: … }`: the state BEFORE
    // this apply (change detection) and the layer's CURRENT on-screen
    // matrix (the tween's `from` — read from the presentation layer so a
    // mid-flight reversal retargets smoothly instead of jumping). First
    // apply snaps — CSS parity.
    let (seen_before, old_state) = {
        let mut s = state.borrow_mut();
        let seen = s.static_transform_seen;
        s.static_transform_seen = true;
        (seen, *s)
    };
    let from_matrix = if seen_before && style.transform_transition.is_some() {
        current_layer_transform(view)
    } else {
        None
    };
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
                    // Neither `Auto` nor `Full` means anything on a
                    // translate: `Auto` defers to layout, which a transform
                    // has no part in, and `Full` is the corner-radius pill.
                    // Both leave the identity translate.
                    Transform::TranslateX(Length::Auto | Length::Full | Length::FitContent)
                    | Transform::TranslateY(Length::Auto | Length::Full | Length::FitContent) => {}
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
    // Apply with current bounds (0×0 at mount — the layout pass
    // resolves the percents again via `sync_transform_after_layout`).
    let new_state = *state.borrow();
    rebuild_transform(view, &new_state);

    // `transitions { transform: … }` — animate the change instead of
    // snapping, matching web's CSS `transition: transform` (the
    // AppShell drawer slide). `rebuild_transform` above already set
    // the final MODEL matrix; the CABasicAnimation drives the
    // presentation from the captured on-screen matrix to it.
    if let (Some(t), Some(from)) = (style.transform_transition.as_ref(), from_matrix) {
        let (w, h) = view_size(view);
        if crate::transform_transition_policy::should_animate_static_transform(
            seen_before,
            true,
            static_transform_changed(&old_state, &new_state, w, h),
        ) {
            animate_layer_transform(view, from, t);
        }
    }
}

/// Whether going from `old` to `new` moves the layer matrix of a `w`×`h`
/// view. Compares the composed matrices, so a percent-only change
/// (`translateX(100%)` → `translateX(0%)`) counts, and a percent change on
/// a view with no size yet does not: both resolve to the same matrix, and
/// the first layout places it without a slide.
fn static_transform_changed(old: &AnimatedState, new: &AnimatedState, w: f64, h: f64) -> bool {
    compose_layer_transform(old, w, h) != compose_layer_transform(new, w, h)
}

fn view_size(view: &NSView) -> (f64, f64) {
    let bounds: objc2_foundation::CGRect = unsafe { msg_send![view, bounds] };
    (bounds.size.width as f64, bounds.size.height as f64)
}

/// Read the CURRENT on-screen transform of `view`'s layer: the
/// presentation layer's matrix while an animation is in flight
/// (drawer reversed mid-slide), else the model value.
fn current_layer_transform(view: &NSView) -> Option<CATransform3D> {
    let layer: Option<Retained<NSObject>> = unsafe { msg_send_id![view, layer] };
    let layer = layer?;
    let pres: Option<Retained<NSObject>> = unsafe { msg_send_id![&layer, presentationLayer] };
    let src = pres.unwrap_or(layer);
    Some(unsafe { msg_send![&src, transform] })
}

/// Attach a `CABasicAnimation(keyPath: "transform")` running `from` →
/// the layer's (already-set) model matrix. AppKit layer-backed views
/// suppress implicit CALayer animations, so an explicit animation is
/// the AppKit-native way to tween a layer transform — the macOS
/// counterpart of iOS's `UIView animateWithDuration:` wrap and web's
/// CSS `transition: transform`.
fn animate_layer_transform(
    view: &NSView,
    from: CATransform3D,
    transition: &runtime_shared::Transition,
) {
    use objc2_foundation::NSString;
    let layer: Option<Retained<NSObject>> = unsafe { msg_send_id![view, layer] };
    let Some(layer) = layer else { return };

    let keypath = NSString::from_str("transform");
    let anim: Option<Retained<NSObject>> = unsafe {
        msg_send_id![objc2::class!(CABasicAnimation), animationWithKeyPath: &*keypath]
    };
    let Some(anim) = anim else { return };
    let from_val: Option<Retained<NSObject>> = unsafe {
        msg_send_id![objc2::class!(NSValue), valueWithCATransform3D: from]
    };
    let Some(from_val) = from_val else { return };
    let _: () = unsafe { msg_send![&anim, setFromValue: &*from_val] };
    // `toValue` stays nil → animates to the layer's model value
    // (the final matrix `rebuild_transform` just set).
    let duration = transition.duration_ms as f64 / 1000.0;
    let _: () = unsafe { msg_send![&anim, setDuration: duration] };
    let name = NSString::from_str(
        backend_apple_core::ca_timing::ca_timing_name(transition.easing),
    );
    let tf: Option<Retained<NSObject>> = unsafe {
        msg_send_id![objc2::class!(CAMediaTimingFunction), functionWithName: &*name]
    };
    if let Some(tf) = tf {
        let _: () = unsafe { msg_send![&anim, setTimingFunction: &*tf] };
    }
    let key = NSString::from_str("idealyst-transform-transition");
    let _: () = unsafe { msg_send![&layer, addAnimation: &*anim, forKey: &*key] };
}

/// Rebuild `view`'s layer matrix against the bounds the layout pass just
/// gave it: percent translates resolve against the new size, and the
/// scale/rotate center-pivot compensation follows the new center. Called
/// after every `setFrame:` (see `sync_view_after_frame`). Writing the
/// model value leaves an in-flight transition running toward it.
pub(crate) fn sync_transform_after_layout(
    view: &NSView,
    states: &AnimatedStateMap,
) {
    let key = view as *const NSView as usize;
    let Some(state) = states.get(&key) else { return };
    rebuild_transform(view, &state.borrow());
}

/// Write a scalar animation property on `node`. Routes through the
/// CALayer for transforms (NSView itself has no transform property)
/// and through NSView's `setAlphaValue:` for opacity.
/// Install a `CAKeyframeAnimation` on the view's layer so the property animates
/// on the **render server** (off the main thread, no per-frame reactive tick /
/// CALayer commit). Returns `true` when handled natively, `false` to signal the
/// framework to fall back to the per-frame `set_animated_f32` clock path.
///
/// First-class for **opacity** (the spinner pulse — the case where the
/// per-frame full-tree CA commit was measurably stealing scroll frames) and
/// **TranslateX** (the indeterminate-Progress sweep — a forever translate loop
/// with the same per-frame cost profile). Other props return `false` and keep
/// the per-frame path until their keyPath mapping lands. Rationale: a forever
/// animation driven per frame forces a `CA::Transaction::commit`
/// (O(layer-tree)) every frame; the same loop as a CAKeyframeAnimation costs
/// the main thread nothing per frame.
///
/// A keyframe animation only drives the PRESENTATION layer — the model value
/// (alpha / `rebuild_transform`'s model transform) is untouched. Callers must
/// not drive the same prop through the per-frame `set_animated_f32` path on
/// the same view concurrently; the framework's `AnimatedValue` fallback
/// contract (native path taken → no fallback animator started) guarantees
/// this.
pub(crate) fn install_keyframe_animation(
    node: &MacosNode,
    prop: AnimProp,
    keyframes: &[(f32, f32)],
    duration_ms: u32,
    repeat_forever: bool,
    autoreverse: bool,
) -> bool {
    // keyPath mapping; unmapped props fall back to the per-frame clock.
    let key_path = match prop {
        AnimProp::Opacity => "opacity",
        AnimProp::TranslateX => "transform.translation.x",
        _ => return false,
    };
    if keyframes.len() < 2 || duration_ms == 0 {
        return false;
    }
    let view = node.as_view();

    use objc2::class;
    use objc2_foundation::NSString;

    // Layer-back so the layer exists to host the animation. The pulse target
    // (the progress fill / spinner) is already layer-backed for its background,
    // so this is a no-op in practice — but it must be true for `layer` to exist.
    let _: () = unsafe { msg_send![view, setWantsLayer: true] };
    let layer: *mut NSObject = unsafe { msg_send![view, layer] };
    if layer.is_null() {
        return false;
    }

    // values + keyTimes as NSArray<NSNumber>.
    let values: Retained<NSObject> = unsafe { msg_send_id![class!(NSMutableArray), array] };
    let key_times: Retained<NSObject> = unsafe { msg_send_id![class!(NSMutableArray), array] };
    for (t, v) in keyframes {
        let num_v: Retained<NSObject> =
            unsafe { msg_send_id![class!(NSNumber), numberWithDouble: *v as f64] };
        let num_t: Retained<NSObject> = unsafe {
            msg_send_id![class!(NSNumber), numberWithDouble: (*t).clamp(0.0, 1.0) as f64]
        };
        let _: () = unsafe { msg_send![&values, addObject: &*num_v] };
        let _: () = unsafe { msg_send![&key_times, addObject: &*num_t] };
    }

    let key_path_ns = NSString::from_str(key_path);
    let anim: Retained<NSObject> = unsafe {
        msg_send_id![class!(CAKeyframeAnimation), animationWithKeyPath: &*key_path_ns]
    };
    let dur_s = duration_ms as f64 / 1000.0;
    unsafe {
        let _: () = msg_send![&anim, setValues: &*values];
        let _: () = msg_send![&anim, setKeyTimes: &*key_times];
        let _: () = msg_send![&anim, setDuration: dur_s];
        let _: () = msg_send![&anim, setAutoreverses: autoreverse];
        let repeat: f32 = if repeat_forever { f32::INFINITY } else { 1.0 };
        let _: () = msg_send![&anim, setRepeatCount: repeat];
        // Hold the last value for a finite animation; irrelevant when looping.
        let _: () = msg_send![&anim, setRemovedOnCompletion: false];
        let forwards = NSString::from_str("forwards");
        let _: () = msg_send![&anim, setFillMode: &*forwards];
        // Ease between keyframes for a natural pulse (matches the per-frame
        // `ease_in_out` tweens closely enough that the two paths look identical).
        let ease = NSString::from_str("easeInEaseOut");
        let tf: Retained<NSObject> =
            msg_send_id![class!(CAMediaTimingFunction), functionWithName: &*ease];
        let _: () = msg_send![&anim, setTimingFunction: &*tf];
        // Stable per-keyPath key so a re-install (e.g. the Progress sweep
        // re-ranging after a resize) replaces rather than stacks, while
        // animations on different keyPaths coexist.
        let anim_key = NSString::from_str(&format!("idealyst.keyframe.{key_path}"));
        let _: () = msg_send![layer, addAnimation: &*anim, forKey: &*anim_key];
    }
    true
}

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
            state.borrow_mut().anim_opacity = true;
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
            state.borrow_mut().anim_z_index = Some(value);
            // CALayer's `zPosition` is the closest equivalent. AppKit
            // sibling ordering normally goes by subview index; setting
            // zPosition reorders at draw time, and `FlippedView.hitTest:`
            // walks subviews in the same order.
            let layer: Option<Retained<NSObject>> =
                unsafe { msg_send_id![view, layer] };
            if let Some(layer) = layer {
                let _: () = unsafe { msg_send![&layer, setZPosition: value as f64] };
                backend_apple_core::shadow_layer::sync_sibling_z(&layer);
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

/// Translation that keeps the box center `(cx, cy)` fixed under the 2×2
/// linear map `[[a, c], [b, d]]` (Core Animation's row-vector convention:
/// `x' = a*x + c*y`, `y' = b*x + d*y`). Equal to `C − L(C)`, i.e. the
/// translate part of `T(C) · M · T(−C)`. AppKit layer-backed `NSView`s
/// default their layer `anchorPoint` to `(0, 0)` (top-left), so this is what
/// makes scale/rotate pivot around the center like UIKit / web do.
///
/// Kept pure (no ObjC) so the center-invariant is unit-testable — the native
/// `setTransform:` write is exercised by the robot screenshot pass.
fn center_pivot_offset(cx: f64, cy: f64, a: f64, b: f64, c: f64, d: f64) -> (f64, f64) {
    let center_tx = cx * (1.0 - a) - c * cy;
    let center_ty = cy * (1.0 - d) - b * cx;
    (center_tx, center_ty)
}

/// The translate `state` places a `w`×`h` view at: px and percent
/// entries summed (percents resolve against the view's own size, CSS's
/// reference box for `translate`), plus the animated translate.
fn resolved_translate(state: &AnimatedState, w: f64, h: f64) -> (f64, f64) {
    let pct_x = state.static_translate_pct_x.map_or(0.0, |p| p as f64 / 100.0 * w);
    let pct_y = state.static_translate_pct_y.map_or(0.0, |p| p as f64 / 100.0 * h);
    (
        (state.translate_x + state.anim_translate_x) as f64 + pct_x,
        (state.translate_y + state.anim_translate_y) as f64 + pct_y,
    )
}

/// The layer matrix for `state` on a view of `w`×`h` bounds. CATransform3D
/// is a 4x4 matrix; this composes translate × rotate × scale in that order
/// (CSS transform semantics: scale applies first, then rotate, then
/// translate), pivoted at the view's center. Pure, so the composition is
/// unit-testable without AppKit.
fn compose_layer_transform(state: &AnimatedState, w: f64, h: f64) -> CATransform3D {
    let cx = w / 2.0;
    let cy = h / 2.0;

    // Compose static ∘ animated: translates add, scales multiply, rotations
    // add. So a view with no static transform but an animated `TranslateX`
    // (the Switch thumb) keeps that translate across a restyle, and a static
    // transform + an animated one combine rather than overwrite.
    let (tx, ty) = resolved_translate(state, w, h);

    // 2x2 linear part: rotate then scale. The derivation is kept
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
    // part but shifts translation by (cx*(1-a) - c*cy,
    // cy*(1-d) - b*cx). Note the cross terms: CA's row-vector map is
    // x' = a*x + c*y, y' = b*x + d*y, so the x-offset carries `c` and
    // the y-offset carries `b`. For a pure scale b = c = 0, so the
    // choice is invisible there; a rotation has b, c ≠ 0 (and opposite
    // sign), so swapping them pivots off-center.
    let (center_tx, center_ty) = center_pivot_offset(cx, cy, a, b, c, d);

    CATransform3D {
        m11: a, m12: b, m13: 0.0, m14: 0.0,
        m21: c, m22: d, m23: 0.0, m24: 0.0,
        m31: 0.0, m32: 0.0, m33: 1.0, m34: 0.0,
        m41: tx + center_tx, m42: ty + center_ty, m43: 0.0, m44: 1.0,
    }
}

/// Write `state`'s matrix ([`compose_layer_transform`]) to `view`'s layer,
/// resolved against the view's current bounds. The raw `#[repr(C)]`
/// CATransform3D goes through `setTransform:`'s standard ObjC encoding.
fn rebuild_transform(view: &NSView, state: &AnimatedState) {
    // Layer-back the view; transforms only render through the
    // CALayer. `setWantsLayer:true` is a no-op if already set.
    let _: () = unsafe { msg_send![view, setWantsLayer: true] };
    let layer: Option<Retained<NSObject>> = unsafe { msg_send_id![view, layer] };
    let Some(layer) = layer else { return };
    let (w, h) = view_size(view);
    set_view_layer_transform(view, &layer, compose_layer_transform(state, w, h));
}

thread_local! {
    /// Views whose backing layer was given a non-identity transform before
    /// AppKit put that layer in the window's layer tree, with the matrix
    /// last written. See [`set_view_layer_transform`].
    static AWAITING_LAYER_TREE: RefCell<HashMap<usize, (Retained<NSView>, CATransform3D)>> =
        RefCell::new(HashMap::new());
}

/// Write `m` to `view`'s backing `layer`, and remember it while AppKit has
/// not yet parented that layer.
///
/// AppKit builds a new subview's layer into the tree lazily, in the window's
/// display cycle inside the Core Animation commit (`NSWindowUpdateLayerTree`
/// → `-[NSView _updateLayerGeometryFromView]`), and that build writes the
/// view's own geometry onto the layer: `setAffineTransform:` with the view's
/// identity transform. A transform written before the build (a translate set
/// as the view mounts, which is how the drag ghost is placed) is wiped in the
/// commit that first shows the view, and stays wiped until the value next
/// changes, so the ghost painted at the window's top-left. A parented layer
/// keeps its transform across `setFrame:`, so only the unparented case needs
/// [`restore_transforms_awaiting_layer_tree`].
fn set_view_layer_transform(view: &NSView, layer: &NSObject, m: CATransform3D) {
    let _: () = unsafe { msg_send![layer, setTransform: m] };
    let key = view as *const NSView as usize;
    let superlayer: *mut NSObject = unsafe { msg_send![layer, superlayer] };
    AWAITING_LAYER_TREE.with(|w| {
        let mut w = w.borrow_mut();
        if superlayer.is_null() && m != CATransform3D::IDENTITY {
            // SAFETY: `view` is a live reference, so retaining it is sound.
            let view = unsafe { Retained::retain(view as *const NSView as *mut NSView) };
            if let Some(view) = view {
                w.insert(key, (view, m));
            }
        } else {
            w.remove(&key);
        }
    });
}

/// Whether any view is waiting for [`restore_transforms_awaiting_layer_tree`].
pub(crate) fn transforms_await_layer_tree() -> bool {
    AWAITING_LAYER_TREE.with(|w| !w.borrow().is_empty())
}

/// Build the waiting views' layers into their windows' layer trees now, then
/// write their transforms back over what that build wrote, so the commit that
/// first shows them carries the transforms. Runs before the commit (the
/// pre-commit drain in `imp::mod`).
///
/// `-[NSWindow displayIfNeeded]` is the public entry to the display cycle the
/// commit runs later in this same turn. Running it here moves that work
/// earlier without adding to it; the commit's own cycle then finds nothing
/// left to do. It can run drawing code that takes the backend, so the caller
/// must not hold a backend borrow. A view with no window yet is dropped from
/// the list: when it is inserted, the layout pass frames it for the first
/// time, `sync_transform_after_layout` writes the transform again, and that
/// write records it again.
pub(crate) fn restore_transforms_awaiting_layer_tree() {
    let pending = AWAITING_LAYER_TREE.with(|w| std::mem::take(&mut *w.borrow_mut()));
    if pending.is_empty() {
        return;
    }
    let mut windows: Vec<Retained<NSObject>> = Vec::new();
    for (view, _) in pending.values() {
        let window: Option<Retained<NSObject>> = unsafe { msg_send_id![&**view, window] };
        if let Some(window) = window {
            if !windows.iter().any(|w| Retained::as_ptr(w) == Retained::as_ptr(&window)) {
                windows.push(window);
            }
        }
    }
    for window in &windows {
        let _: () = unsafe { msg_send![&**window, displayIfNeeded] };
    }
    for (view, m) in pending.into_values() {
        let layer: Option<Retained<NSObject>> = unsafe { msg_send_id![&*view, layer] };
        let Some(layer) = layer else { continue };
        let superlayer: *mut NSObject = unsafe { msg_send![&layer, superlayer] };
        if !superlayer.is_null() {
            let _: () = unsafe { msg_send![&layer, setTransform: m] };
        }
    }
}

/// Apply a presence transform — uniform `scale` + (`tx`, `ty`) translate,
/// center-pivoted — directly to `view`'s layer, WITHOUT touching the
/// `AnimatedStateMap` cache. Presence drives its own raf tween (see
/// [`crate::imp::presence`]) and must neither clobber nor be clobbered by the
/// static / animated style-transform slots, so it bypasses `rebuild_transform`.
/// The center-pivot compensation mirrors `rebuild_transform`: AppKit
/// layer-backed NSViews default `anchorPoint` to (0, 0), so without it a scale
/// would grow from the top-left corner instead of the center (web / iOS pivot
/// around the center). For a pure translate (`scale == 1`) the compensation is
/// zero, so `m41`/`m42` are exactly the translate — and `view_layer_translate`
/// (read by `hitTest:`) still reports the right offset.
pub(crate) fn apply_presence_transform(view: &NSView, tx: f64, ty: f64, scale: f64) {
    let _: () = unsafe { msg_send![view, setWantsLayer: true] };
    let layer: Option<Retained<NSObject>> = unsafe { msg_send_id![view, layer] };
    let Some(layer) = layer else { return };
    let bounds: objc2_foundation::CGRect = unsafe { msg_send![view, bounds] };
    let cx = bounds.size.width / 2.0;
    let cy = bounds.size.height / 2.0;
    let center_tx = cx * (1.0 - scale);
    let center_ty = cy * (1.0 - scale);
    let m = CATransform3D {
        m11: scale, m12: 0.0, m13: 0.0, m14: 0.0,
        m21: 0.0, m22: scale, m23: 0.0, m24: 0.0,
        m31: 0.0, m32: 0.0, m33: 1.0, m34: 0.0,
        m41: tx + center_tx, m42: ty + center_ty, m43: 0.0, m44: 1.0,
    };
    set_view_layer_transform(view, &layer, m);
}

/// The uniform scale currently on `layer` (its `transform.m11`). The icon
/// backend reads this to skip re-applying an unchanged scale every layout
/// pass.
pub(crate) fn current_layer_scale(layer: &NSObject) -> f64 {
    let t: CATransform3D = unsafe { msg_send![layer, transform] };
    t.m11
}

/// The `(x, y)` translate currently on `view`'s layer (`transform.m41`/`m42`):
/// static px and percent translates, animated and presence translates alike.
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

/// The summed layer translates of `view` and every ancestor: how far the
/// view paints from where its frame chain puts it. `convertRect:` /
/// `convertPoint:` walk frames and ignore layer transforms, so a window
/// rect or a local pointer position adds (or subtracts) this to land on
/// the painted view — including when the translate is an ancestor's (a
/// row inside a card slid by `translateX(100%)`).
pub(crate) fn visual_translate_in_window(view: &NSView) -> (f64, f64) {
    let (mut x, mut y) = view_layer_translate(view);
    let mut cur: Option<Retained<NSView>> = unsafe { msg_send_id![view, superview] };
    while let Some(v) = cur {
        let (tx, ty) = view_layer_translate(&v);
        x += tx;
        y += ty;
        cur = unsafe { msg_send_id![&*v, superview] };
    }
    (x, y)
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

#[cfg(test)]
mod tests {
    //! Pure transform math — no AppKit. The native `setTransform:` write and
    //! its transition are exercised by `tests/percent_translate_transition.rs`.
    use super::{
        center_pivot_offset, compose_layer_transform, resolved_translate,
        static_transform_changed, AnimatedState,
    };

    fn translated(pct_x: Option<f32>, px_x: f32, pct_y: Option<f32>, px_y: f32) -> AnimatedState {
        let mut s = AnimatedState::new();
        s.static_translate_pct_x = pct_x;
        s.translate_x = px_x;
        s.static_translate_pct_y = pct_y;
        s.translate_y = px_y;
        s
    }

    /// The Timely content card: `[TranslateX(Percent(100)), TranslateX(Px(-24))]`
    /// on a 400-wide card sits at 400 − 24. The percent resolves against the
    /// view's own width and the px entry adds to it, as CSS sums a translate list.
    #[test]
    fn percent_and_px_translates_compose() {
        let s = translated(Some(100.0), -24.0, Some(-50.0), 6.0);
        assert_eq!(resolved_translate(&s, 400.0, 200.0), (376.0, -94.0));
        let m = compose_layer_transform(&s, 400.0, 200.0);
        assert_eq!((m.m41, m.m42), (376.0, -94.0), "a pure translate is m41/m42 exactly");
    }

    /// `translate(-50%, -50%)` centering (idea-ui overlays) lands half the
    /// view's own size up and left, in the matrix.
    #[test]
    fn percent_translate_centers_against_own_size() {
        let s = translated(Some(-50.0), 0.0, Some(-50.0), 0.0);
        let m = compose_layer_transform(&s, 300.0, 120.0);
        assert_eq!((m.m41, m.m42), (-150.0, -60.0));
    }

    /// An animated translate (`set_animated_f32`) adds on top of the static
    /// percent one rather than replacing it.
    #[test]
    fn animated_translate_adds_to_percent_translate() {
        let mut s = translated(Some(10.0), 0.0, None, 0.0);
        s.anim_translate_x = 5.0;
        assert_eq!(resolved_translate(&s, 100.0, 50.0), (15.0, 0.0));
    }

    /// REGRESSION (a `transitions { transform }` card sliding by
    /// `translateX(100%)` → `translateX(0%)` snapped on macOS, slid on web):
    /// the change check looked at the px/scale/rotate slots only, so a change
    /// carried by the percent entry never counted as a change.
    #[test]
    fn regression_percent_only_transform_change_counts_as_a_change() {
        let from = translated(Some(100.0), 0.0, None, 0.0);
        let to = translated(Some(0.0), 0.0, None, 0.0);
        assert!(static_transform_changed(&from, &to, 400.0, 300.0));
        assert!(!static_transform_changed(&from, &from, 400.0, 300.0));
    }

    /// Before its first layout a view is 0×0, so a percent change moves
    /// nothing; it snaps, and the first layout places the view (no phantom
    /// slide-in at mount).
    #[test]
    fn percent_change_on_an_unsized_view_is_not_a_change() {
        let from = translated(Some(100.0), 0.0, None, 0.0);
        let to = translated(Some(0.0), 0.0, None, 0.0);
        assert!(!static_transform_changed(&from, &to, 0.0, 0.0));
    }

    /// A translate under a scale still pivots the scale at the center: the
    /// center maps to center + translate.
    #[test]
    fn percent_translate_with_scale_keeps_center_pivot() {
        let mut s = translated(Some(50.0), 0.0, None, 0.0);
        s.scale_x = 2.0;
        s.scale_y = 2.0;
        let (w, h) = (100.0, 40.0);
        let m = compose_layer_transform(&s, w, h);
        let (cx, cy) = (w / 2.0, h / 2.0);
        let x = m.m11 * cx + m.m21 * cy + m.m41;
        let y = m.m12 * cx + m.m22 * cy + m.m42;
        assert!((x - (cx + 50.0)).abs() < 1e-9 && (y - cy).abs() < 1e-9, "({x}, {y})");
    }

    /// Apply the CA row-vector map `x' = a*x + c*y + tx`, `y' = b*x + d*y + ty`
    /// to a point, using the center-pivot offset as the translate.
    fn map_center(cx: f64, cy: f64, a: f64, b: f64, c: f64, d: f64) -> (f64, f64) {
        let (tx, ty) = center_pivot_offset(cx, cy, a, b, c, d);
        let x = a * cx + c * cy + tx;
        let y = b * cx + d * cy + ty;
        (x, y)
    }

    /// The box center must be a fixed point of a pure rotation about center.
    /// Before the b/c-swap fix this held only for scale, so a rotating chevron
    /// pivoted off its center on macOS (unlike iOS / web).
    #[test]
    fn regression_rotation_pivots_around_center() {
        let (cx, cy) = (12.0, 12.0);
        for deg in [30.0_f64, 45.0, 90.0, 135.0, 180.0, 270.0] {
            let r = deg.to_radians();
            let (cos, sin) = (r.cos(), r.sin());
            // Pure rotation, no scale: a=cos, b=sin, c=-sin, d=cos.
            let (x, y) = map_center(cx, cy, cos, sin, -sin, cos);
            assert!(
                (x - cx).abs() < 1e-9 && (y - cy).abs() < 1e-9,
                "rotation {deg}° moved center to ({x}, {y}), expected ({cx}, {cy})"
            );
        }
    }

    /// Rotation combined with a non-uniform scale must also pivot around center.
    #[test]
    fn rotation_with_scale_pivots_around_center() {
        let (cx, cy) = (20.0, 8.0);
        let r = 60.0_f64.to_radians();
        let (cos, sin) = (r.cos(), r.sin());
        let (sx, sy) = (1.5, 0.75);
        // rotate-then-scale: a=cos*sx, b=sin*sx, c=-sin*sy, d=cos*sy.
        let (x, y) = map_center(cx, cy, cos * sx, sin * sx, -sin * sy, cos * sy);
        assert!((x - cx).abs() < 1e-9 && (y - cy).abs() < 1e-9, "center moved to ({x}, {y})");
    }

    /// Pure scale already worked (b = c = 0); guard it so the fix stays correct.
    #[test]
    fn scale_pivots_around_center() {
        let (cx, cy) = (10.0, 10.0);
        let (x, y) = map_center(cx, cy, 2.0, 0.0, 0.0, 2.0);
        assert!((x - cx).abs() < 1e-9 && (y - cy).abs() < 1e-9, "center moved to ({x}, {y})");
    }
}

