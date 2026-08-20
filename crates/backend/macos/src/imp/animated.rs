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
            static_translate_pct_x: None,
            static_translate_pct_y: None,
            static_transform_seen: false,
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
    style: &runtime_shared::StyleRules,
    states: &mut AnimatedStateMap,
) {
    use runtime_shared::{Length, Transform};
    let view = node.as_view();
    let key = view as *const NSView as usize;
    let state = states
        .entry(key)
        .or_insert_with(|| RefCell::new(AnimatedState::new()));
    // Snapshot for `transitions { transform: … }`: the static tuple
    // BEFORE this apply (change detection) and the layer's CURRENT
    // on-screen matrix (the tween's `from` — read from the
    // presentation layer so a mid-flight reversal retargets smoothly
    // instead of jumping). First apply snaps — CSS parity.
    let (seen_before, old_static) = {
        let mut s = state.borrow_mut();
        let seen = s.static_transform_seen;
        s.static_transform_seen = true;
        (seen, (s.translate_x, s.translate_y, s.scale_x, s.scale_y, s.rotate_z))
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
                    Transform::TranslateX(Length::Auto | Length::Full)
                    | Transform::TranslateY(Length::Auto | Length::Full) => {}
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

    // `transitions { transform: … }` — animate the change instead of
    // snapping, matching web's CSS `transition: transform` (the
    // AppShell drawer slide). `rebuild_transform` above already set
    // the final MODEL matrix; the CABasicAnimation drives the
    // presentation from the captured on-screen matrix to it.
    if let (Some(t), Some(from)) = (style.transform_transition.as_ref(), from_matrix) {
        let s = state.borrow();
        let new_static = (s.translate_x, s.translate_y, s.scale_x, s.scale_y, s.rotate_z);
        if crate::transform_transition_policy::should_animate_static_transform(
            seen_before,
            true,
            new_static != old_static,
        ) {
            animate_layer_transform(view, from, t);
        }
    }
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
        crate::transform_transition_policy::easing_to_ca_timing_name(transition.easing),
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
    // part but shifts translation by (cx*(1-a) - c*cy,
    // cy*(1-d) - b*cx). Note the cross terms: CA's row-vector map is
    // x' = a*x + c*y, y' = b*x + d*y, so the x-offset carries `c` and
    // the y-offset carries `b`. For a pure scale b = c = 0, so the
    // choice is invisible there; a rotation has b, c ≠ 0 (and opposite
    // sign), so swapping them pivots off-center — the bug this fixes.
    let (center_tx, center_ty) = center_pivot_offset(cx, cy, a, b, c, d);

    let m = CATransform3D {
        m11: a, m12: b, m13: 0.0, m14: 0.0,
        m21: c, m22: d, m23: 0.0, m24: 0.0,
        m31: 0.0, m32: 0.0, m33: 1.0, m34: 0.0,
        m41: tx + center_tx, m42: ty + center_ty, m43: 0.0, m44: 1.0,
    };
    let _: () = unsafe { msg_send![&layer, setTransform: m] };
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

#[cfg(test)]
mod tests {
    //! Pure center-pivot math — no AppKit. The native `setTransform:` write is
    //! exercised by the robot screenshot pass, not here.
    use super::center_pivot_offset;

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

