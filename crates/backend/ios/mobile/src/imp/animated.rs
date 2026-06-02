//! iOS backend implementation of `Backend::set_animated_*`.
//!
//! Per-frame writes from the framework's animation clock land
//! here keyed by `(IosNode, AnimProp)`. We:
//!
//! 1. Cache the latest value of every transform component in
//!    [`AnimatedTransformState`], keyed by the node's
//!    [`IosNode::view_key`] (pointer-derived `usize`).
//! 2. Recompose the affected UIKit setter on every update.
//!
//! Opacity → `UIView.alpha`, colors → `UIView.backgroundColor` /
//! `UIView.tintColor`. Transform components (translate / scale /
//! rotate) compose into a single [`CGAffineTransform`] which is
//! re-emitted via `UIView.setTransform:` on every component
//! update — UIView only exposes the combined matrix, so we hold
//! the per-axis state on the backend.
//!
//! # Composition order
//!
//! `setTransform:` applies the matrix to the view's anchor point
//! (default `(0.5, 0.5)` — view centre). For a natural feel:
//!
//! - **Scale** scales around centre.
//! - **Rotate** rotates around centre.
//! - **Translate** moves the post-scale-and-rotate view in
//!   screen-space pixels.
//!
//! Matrix form `T(tx,ty) * R(theta) * S(sx,sy)` — *apply* scale
//! first to the source point, then rotate, then translate. This
//! matches CSS's `transform: translate(...) rotate(...) scale(...)`
//! left-to-right convention.

use std::collections::HashMap;

use runtime_core::animation::AnimProp;
use runtime_core::Color;
use objc2::encode::{Encode, Encoding};
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::ClassType;
use objc2_foundation::{CGFloat, NSObject};
use objc2_ui_kit::UIView;

use backend_ios_core::style::color_to_uicolor;

use super::IosBackend;
use super::IosNode;

/// Mutable per-node animation state. Lives in
/// [`IosBackend::animated_states`] keyed by `IosNode::view_key`.
#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub(crate) struct AnimatedTransformState {
    pub opacity: Option<f32>,
    pub translate_x: Option<f32>,
    pub translate_y: Option<f32>,
    pub scale_x: Option<f32>,
    pub scale_y: Option<f32>,
    /// Rotation in degrees, converted to radians at composition.
    pub rotate_z: Option<f32>,
    pub background_color: Option<[f32; 4]>,
    pub foreground_color: Option<[f32; 4]>,
    /// `CAGradientLayer` reference stashed by `apply_gradient`, plus
    /// the current sRGB stop colors. Per-frame `GradientStopColor`
    /// writes mutate `stops[idx]` and re-call `setColors:` on the
    /// stored layer — no need to walk the parent layer's sublayers
    /// or rebuild the whole gradient every frame.
    pub gradient_layer: Option<Retained<NSObject>>,
    pub gradient_stops: Vec<[f32; 4]>,
    /// Static `transform: translate(N%, …)` requests parked here at
    /// apply-style time and resolved against the view's actual
    /// pixel dimensions in the layout pass. CSS-spec translate-% is
    /// BOX-relative — the px shift can't be computed until Taffy
    /// produces a frame. Once resolved, the px value is written
    /// into `translate_x` / `translate_y` so `compose()` sees it.
    pub static_translate_pct_x: Option<f32>,
    pub static_translate_pct_y: Option<f32>,
}

impl AnimatedTransformState {
    fn any_transform_set(&self) -> bool {
        self.translate_x.is_some()
            || self.translate_y.is_some()
            || self.scale_x.is_some()
            || self.scale_y.is_some()
            || self.rotate_z.is_some()
    }

    /// Compose the affine matrix from current state, treating unset
    /// axes as identity defaults.
    fn compose(&self) -> CGAffineTransform {
        let tx = self.translate_x.unwrap_or(0.0) as CGFloat;
        let ty = self.translate_y.unwrap_or(0.0) as CGFloat;
        let sx = self.scale_x.unwrap_or(1.0) as CGFloat;
        let sy = self.scale_y.unwrap_or(1.0) as CGFloat;
        let theta_rad = (self.rotate_z.unwrap_or(0.0) as CGFloat).to_radians();
        let cos_t = theta_rad.cos();
        let sin_t = theta_rad.sin();

        // T(tx, ty) * R(theta) * S(sx, sy):
        // S = [sx 0 0; 0 sy 0]
        // R = [cos -sin 0; sin cos 0]
        // R*S = [sx*cos -sy*sin 0; sx*sin sy*cos 0]
        // T*R*S = [sx*cos -sy*sin tx; sx*sin sy*cos ty]
        //
        // CGAffineTransform layout: [a b c d tx ty] where the
        // transformation of (x, y) is
        //   x' = a*x + c*y + tx
        //   y' = b*x + d*y + ty
        // So  a = sx*cos, b = sx*sin, c = -sy*sin, d = sy*cos.
        CGAffineTransform {
            a: sx * cos_t,
            b: sx * sin_t,
            c: -sy * sin_t,
            d: sy * cos_t,
            tx,
            ty,
        }
    }
}

/// CGAffineTransform — the 2D affine matrix UIView's transform
/// property uses. Same shape as the one in `tab_drawer.rs`; kept
/// local to keep the modules independent.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct CGAffineTransform {
    pub a: CGFloat,
    pub b: CGFloat,
    pub c: CGFloat,
    pub d: CGFloat,
    pub tx: CGFloat,
    pub ty: CGFloat,
}

unsafe impl Encode for CGAffineTransform {
    const ENCODING: Encoding = Encoding::Struct(
        "CGAffineTransform",
        &[
            CGFloat::ENCODING,
            CGFloat::ENCODING,
            CGFloat::ENCODING,
            CGFloat::ENCODING,
            CGFloat::ENCODING,
            CGFloat::ENCODING,
        ],
    );
}

/// Resolve any `static_translate_pct_x` / `_y` requests parked on
/// the per-view animation state against the view's just-laid-out
/// pixel dimensions (CSS spec: translate-% is BOX-relative — the
/// shift is a fraction of the box's own size). Writes the resolved
/// px into `translate_x` / `translate_y` and re-emits
/// `setTransform` so the static shift takes effect.
///
/// Free function (not a method) so it can be called from inside
/// `run_layout_pass`'s borrow on `view_to_layout` without a
/// `&mut self` borrow conflict — callers pass only the slice of
/// state they need.
pub(crate) fn sync_static_transform_percent(
    states: &mut HashMap<usize, AnimatedTransformState>,
    view_ptr: usize,
    view: &UIView,
    width_px: f32,
    height_px: f32,
) {
    let Some(state) = states.get_mut(&view_ptr) else {
        return;
    };
    if state.static_translate_pct_x.is_none() && state.static_translate_pct_y.is_none() {
        return;
    }
    if let Some(pct_x) = state.static_translate_pct_x {
        state.translate_x = Some(width_px * (pct_x / 100.0));
    }
    if let Some(pct_y) = state.static_translate_pct_y {
        state.translate_y = Some(height_px * (pct_y / 100.0));
    }
    let matrix = state.compose();
    let _: () = unsafe { msg_send![view, setTransform: matrix] };
}

impl IosBackend {
    /// Walk a stylesheet's `transform` Vec and apply each op to the
    /// view via the cached [`AnimatedTransformState`]. Px translates,
    /// scale, and rotate get written directly into `translate_x` /
    /// `translate_y` / `scale_x` / `scale_y` / `rotate_z` (the same
    /// slots the animation system uses, so a later animated write
    /// will naturally override — CSS semantics: animated wins).
    /// Percent translates are stashed in `static_translate_pct_x` /
    /// `static_translate_pct_y`; the box-relative px shift can't be
    /// computed until Taffy hands us a frame in the layout pass —
    /// see [`sync_static_transform_percent`].
    ///
    /// Called from `apply_style` whether or not the stylesheet
    /// includes a `transform` block — when it doesn't, the slots
    /// are reset to `None` so a style change that *removes* the
    /// transform reverts the view to identity.
    pub(crate) fn apply_static_transform(
        &mut self,
        node: &IosNode,
        style: &runtime_core::StyleRules,
    ) {
        use runtime_core::{Length, Transform};
        let key = node.view_key();
        let view = node.as_view();
        let state = self.animated_states.entry(key).or_default();

        // Pre-scan: which axes does the style's transform block
        // ACTUALLY drive? Only those get cleared+overwritten below.
        // We can't blanket-clear translate_x/y/scale_x/y/rotate_z
        // because those slots are shared with the animation system —
        // a `AnimatedValue<f32>` bound to `AnimProp::TranslateX`
        // writes the same field. Pre-fix bug: idea-ui's Switch
        // animates its thumb's TranslateX via that path; a theme
        // toggle re-fires `apply_style` from the cohort for the
        // thumb's static stylesheet, which has NO transform: op,
        // so the old code cleared `translate_x` and never restored
        // it \u{2014} thumb snapped from "on" position back to 0
        // while the track color update still ran. Same trap applies
        // to scale_x/y / rotate_z any time author code animates
        // those props.
        let mut style_writes_tx = false;
        let mut style_writes_ty = false;
        let mut style_writes_pct_x = false;
        let mut style_writes_pct_y = false;
        let mut style_writes_sx = false;
        let mut style_writes_sy = false;
        let mut style_writes_rot = false;
        if let Some(ops) = style.transform.as_ref() {
            for op in ops {
                match op {
                    Transform::TranslateX(Length::Px(_)) => style_writes_tx = true,
                    Transform::TranslateY(Length::Px(_)) => style_writes_ty = true,
                    Transform::TranslateX(Length::Percent(_)) => style_writes_pct_x = true,
                    Transform::TranslateY(Length::Percent(_)) => style_writes_pct_y = true,
                    Transform::TranslateX(Length::Auto) | Transform::TranslateY(Length::Auto) => {}
                    Transform::Scale(_) => {
                        style_writes_sx = true;
                        style_writes_sy = true;
                    }
                    Transform::ScaleXY { .. } => {
                        style_writes_sx = true;
                        style_writes_sy = true;
                    }
                    Transform::Rotate(_) => style_writes_rot = true,
                    Transform::SkewX(_) | Transform::SkewY(_) => {}
                }
            }
        }

        // Clear only the axes the style explicitly controls. If the
        // style USED to set X and now doesn't, that's "removing the
        // transform" \u{2014} the call site should explicitly animate
        // back to 0 in that case (CSS-transition parity); we don't
        // clobber an animation-owned slot on the assumption that the
        // style change "removed" what it never wrote in the first
        // place.
        if style_writes_tx || style_writes_pct_x {
            state.translate_x = None;
            state.static_translate_pct_x = None;
        }
        if style_writes_ty || style_writes_pct_y {
            state.translate_y = None;
            state.static_translate_pct_y = None;
        }
        if style_writes_sx {
            state.scale_x = None;
        }
        if style_writes_sy {
            state.scale_y = None;
        }
        if style_writes_rot {
            state.rotate_z = None;
        }

        if let Some(ops) = style.transform.as_ref() {
            for op in ops {
                match op {
                    Transform::TranslateX(Length::Px(v)) => state.translate_x = Some(*v),
                    Transform::TranslateY(Length::Px(v)) => state.translate_y = Some(*v),
                    Transform::TranslateX(Length::Percent(v)) => {
                        state.static_translate_pct_x = Some(*v)
                    }
                    Transform::TranslateY(Length::Percent(v)) => {
                        state.static_translate_pct_y = Some(*v)
                    }
                    Transform::TranslateX(Length::Auto)
                    | Transform::TranslateY(Length::Auto) => {
                        // Auto doesn't make sense for translate — leave at identity.
                    }
                    Transform::Scale(v) => {
                        state.scale_x = Some(*v);
                        state.scale_y = Some(*v);
                    }
                    Transform::ScaleXY { x, y } => {
                        state.scale_x = Some(*x);
                        state.scale_y = Some(*y);
                    }
                    Transform::Rotate(deg) => state.rotate_z = Some(*deg),
                    // Skew not representable as a flat CGAffineTransform
                    // here (would conflict with rotation matrix math).
                    Transform::SkewX(_) | Transform::SkewY(_) => {}
                }
            }
        }

        // Percent translates need the view's box-relative size to
        // convert % → px. On the initial apply (pre-first-layout)
        // the view's `bounds.size` is 0×0 and we leave the resolved
        // slots `None`; the layout pass's `sync_static_transform_percent`
        // fills them in once Taffy hands us a frame.
        //
        // On every SUBSEQUENT apply (theme swap re-firing the cohort
        // driver, reactive style change, etc.) the view DOES have
        // real bounds — we resolve immediately. Without this, the
        // applied_frames cache in `run_layout_pass` short-circuits
        // the post-frame sync block (frame unchanged → skip), so the
        // % values stay at `None` and the disc snaps to the
        // un-translated raw position. Visible bug: hero sun-glare
        // disc shifts from `translate(45%, -45%)` to identity on
        // every theme toggle.
        let need_resolve = state.static_translate_pct_x.is_some()
            || state.static_translate_pct_y.is_some();
        if need_resolve {
            let bounds: objc2_foundation::CGRect = unsafe { msg_send![&*view, bounds] };
            let w = bounds.size.width as f32;
            let h = bounds.size.height as f32;
            if w > 0.0 && h > 0.0 {
                if let Some(pct_x) = state.static_translate_pct_x {
                    state.translate_x = Some(w * (pct_x / 100.0));
                }
                if let Some(pct_y) = state.static_translate_pct_y {
                    state.translate_y = Some(h * (pct_y / 100.0));
                }
            }
        }

        let matrix = state.compose();
        let _: () = unsafe { msg_send![&*view, setTransform: matrix] };
    }

    pub(crate) fn impl_set_animated_f32(
        &mut self,
        node: &IosNode,
        prop: AnimProp,
        value: f32,
    ) {
        let key = node.view_key();
        let view = node.as_view();
        let state = self.animated_states.entry(key).or_default();

        match prop {
            AnimProp::Opacity => {
                state.opacity = Some(value);
                unsafe { view.setAlpha(value as CGFloat) };
            }
            AnimProp::TranslateX
            | AnimProp::TranslateY
            | AnimProp::Scale
            | AnimProp::ScaleX
            | AnimProp::ScaleY
            | AnimProp::RotateZ => {
                match prop {
                    AnimProp::TranslateX => state.translate_x = Some(value),
                    AnimProp::TranslateY => state.translate_y = Some(value),
                    AnimProp::Scale => {
                        state.scale_x = Some(value);
                        state.scale_y = Some(value);
                    }
                    AnimProp::ScaleX => state.scale_x = Some(value),
                    AnimProp::ScaleY => state.scale_y = Some(value),
                    AnimProp::RotateZ => state.rotate_z = Some(value),
                    _ => unreachable!(),
                }
                if state.any_transform_set() {
                    let matrix = state.compose();
                    let _: () = unsafe { msg_send![&*view, setTransform: matrix] };
                }
            }
            AnimProp::ZIndex => {
                // `layer.zPosition` reorders sibling CALayers within
                // their superlayer — Core Animation's analog of
                // `style.zIndex` on web and `View.setTranslationZ`
                // on Android. Only the relative ordering vs siblings
                // matters; the absolute value is unbounded. The
                // layer pointer is owned by the view, so we don't
                // retain it — just dispatch to its setter and let
                // the borrow end at the end of this arm.
                let layer: *mut NSObject = unsafe { msg_send![&*view, layer] };
                if !layer.is_null() {
                    let _: () =
                        unsafe { msg_send![layer, setZPosition: value as CGFloat] };
                }
            }
            AnimProp::MaxHeight => {
                // TODO: native animation API path. For v1, snap-only —
                // the value lands on the Taffy node as `max_size.height`
                // (no per-frame interpolation). Animating layout-
                // affecting properties on UIKit needs either per-frame
                // Taffy re-layout (jank risk) or a `UIView.animate`
                // block driven by the framework's animator. The right
                // shape is a new `Backend::animate_property` method
                // that lets each backend use its native animator;
                // until that lands, iOS's `Smooth` collapsible
                // degrades to snap.
                let _ = value;
            }
            AnimProp::BackgroundColor
            | AnimProp::ForegroundColor
            | AnimProp::GradientStopColor(_) => {
                // Wrong family; silently ignored. Same posture as
                // the web backend's f32-path: mis-routing is a
                // diagnostic concern, not a runtime crash.
            }
        }
    }

    pub(crate) fn impl_set_animated_color(
        &mut self,
        node: &IosNode,
        prop: AnimProp,
        value: [f32; 4],
    ) {
        let key = node.view_key();
        let view = node.as_view();
        let state = self.animated_states.entry(key).or_default();

        // The framework's `Color` is a CSS-string wrapper; we go
        // through `color_to_uicolor` so the iOS color path stays
        // single-source-of-truth. Encode the rgba as a hex
        // shorthand the parser accepts.
        let css = rgba_to_css_string(value);
        let color_struct = Color(css);
        let ui_color = color_to_uicolor(&color_struct);

        match prop {
            AnimProp::BackgroundColor => {
                state.background_color = Some(value);
                let _: () = unsafe { msg_send![&*view, setBackgroundColor: &*ui_color] };
            }
            AnimProp::ForegroundColor => {
                state.foreground_color = Some(value);
                // Per-widget routing: UILabel and UIButton each
                // own their text color via dedicated properties.
                // Other UIView subclasses fall back to `tintColor`
                // (icon strokes, interactive chrome, etc.).
                //
                // Without this split, animating `ForegroundColor`
                // on a label was a no-op — tintColor doesn't
                // cascade into UILabel's textColor.
                match node {
                    IosNode::Label(label) => {
                        let _: () = unsafe {
                            msg_send![label.as_ref(), setTextColor: &*ui_color]
                        };
                    }
                    IosNode::Button(btn) => {
                        // UIControlStateNormal = 0
                        let _: () = unsafe {
                            msg_send![
                                btn.as_ref(),
                                setTitleColor: &*ui_color,
                                forState: 0u64
                            ]
                        };
                    }
                    _ => {
                        let _: () = unsafe {
                            msg_send![&*view, setTintColor: &*ui_color]
                        };
                    }
                }
            }
            AnimProp::GradientStopColor(idx) => {
                // Look up the gradient layer this node owns (stashed
                // by `apply_style` when it called
                // `install_gradient`). Update the cached stop colors
                // and re-emit `setColors:` — Core Animation
                // composites the new gradient in the next frame.
                let Some(layer) = state.gradient_layer.clone() else {
                    return;
                };
                backend_ios_core::style::set_animated_gradient_stop(
                    &*layer,
                    &mut state.gradient_stops,
                    idx as usize,
                    value,
                );
            }
            AnimProp::Opacity
            | AnimProp::TranslateX
            | AnimProp::TranslateY
            | AnimProp::Scale
            | AnimProp::ScaleX
            | AnimProp::ScaleY
            | AnimProp::RotateZ
            | AnimProp::ZIndex
            | AnimProp::MaxHeight => {}
        }
    }

    /// Drop per-node animation state. Called from the existing
    /// view-teardown path so we don't keep stale state alive for
    /// views that have been removed from their parent.
    pub(crate) fn impl_drop_animated_state(&mut self, key: usize) {
        self.animated_states.remove(&key);
    }

    /// `Backend::apply_presence` for iOS.
    ///
    /// Maps a [`PresenceState`] (opacity + 2D translate + uniform
    /// scale) onto the node's UIView. Missing fields resolve to the
    /// resting identity (opacity 1.0, translate 0, scale 1.0) — this
    /// matches the web backend, where a `None` field clears the inline
    /// override and the element snaps back to its stylesheet/default
    /// value.
    ///
    /// We reuse the same [`AnimatedTransformState`] slots the
    /// `AnimatedValue` path writes (`opacity`, `translate_x/y`,
    /// `scale_x/y`) and the same [`AnimatedTransformState::compose`]
    /// matrix, so presence and `AnimatedValue` agree on the final
    /// `setTransform:`/`setAlpha:` output. The presence walker drives
    /// enter/exit as discrete state writes (apply-before-paint, then
    /// snap-to-rest one frame later for enter; apply exit-state then
    /// drop for exit), so we only ever animate from the *current*
    /// rendered value to the supplied state — no per-frame ticking.
    ///
    /// `transition`:
    /// - `Some((ms, easing))` → wrap the alpha+transform writes in
    ///   `UIView animateWithDuration:delay:options:animations:completion:`
    ///   so UIKit interpolates from the current value over `ms`.
    /// - `None` → write the values directly (instant snap), matching
    ///   the pre-paint enter setup and the snap path on web.
    pub(crate) fn impl_apply_presence(
        &mut self,
        node: &IosNode,
        state: runtime_core::PresenceState,
        transition: Option<(u32, runtime_core::Easing)>,
    ) {
        let key = node.view_key();
        let view = node.as_view().retain();
        let st = self.animated_states.entry(key).or_default();

        // Resolve each presence field to its target, treating a
        // missing field as the resting identity. We write these into
        // the shared transform slots so `compose()` reflects them and
        // a subsequent `AnimatedValue`/`apply_style` sees consistent
        // state.
        let target_alpha = state.opacity.unwrap_or(1.0) as CGFloat;
        st.opacity = Some(target_alpha as f32);
        st.translate_x = Some(state.translate_x.unwrap_or(0.0));
        st.translate_y = Some(state.translate_y.unwrap_or(0.0));
        let s = state.scale.unwrap_or(1.0);
        st.scale_x = Some(s);
        st.scale_y = Some(s);
        let matrix = st.compose();

        // Block applied either inside UIView.animate (transition) or
        // immediately (snap). Clone the retained view into the block.
        let view_for_block = view.clone();
        let apply = move || {
            let _: () = unsafe { msg_send![&*view_for_block, setAlpha: target_alpha] };
            let _: () = unsafe { msg_send![&*view_for_block, setTransform: matrix] };
        };

        match transition {
            None => apply(),
            Some((duration_ms, easing)) => {
                let duration = (duration_ms as f64) / 1000.0;
                let options = easing_to_uiview_options(easing);
                let anim_block = block2::StackBlock::new(move || apply());
                let cls = objc2::class!(UIView);
                // animateWithDuration:delay:options:animations:completion:
                // completion is nullable — pass a null block ptr.
                let null_completion: *const NSObject = std::ptr::null();
                let _: () = unsafe {
                    msg_send![
                        cls,
                        animateWithDuration: duration as CGFloat,
                        delay: 0.0 as CGFloat,
                        options: options,
                        animations: &*anim_block,
                        completion: null_completion
                    ]
                };
            }
        }
    }
}

/// Map a framework [`Easing`](runtime_core::Easing) to UIKit's
/// `UIViewAnimationOptions` curve bits. The curve lives in bits
/// 16..18 of the options mask:
///
/// - EaseInOut = 0 << 16
/// - EaseIn    = 1 << 16
/// - EaseOut   = 2 << 16
/// - Linear    = 3 << 16
///
/// `Ease` (the CSS default — quick start, slow end) has no dedicated
/// UIKit named curve; its shape is closest to ease-in-out's
/// symmetric softening for the short fades presence drives, so we map
/// it there. `CubicBezier` likewise falls back to ease-in-out — a
/// custom curve would need `CAMediaTimingFunction` + a CATransaction
/// rather than the `UIView.animate` options path, which presence's
/// narrow fade/slide vocabulary doesn't warrant.
fn easing_to_uiview_options(e: runtime_core::Easing) -> u64 {
    use runtime_core::Easing;
    match e {
        Easing::EaseInOut => 0 << 16,
        Easing::EaseIn => 1 << 16,
        Easing::EaseOut => 2 << 16,
        Easing::Linear => 3 << 16,
        Easing::Ease => 0 << 16,
        Easing::CubicBezier(..) => 0 << 16,
    }
}

/// `[f32; 4]` sRGB → CSS `rgba(...)` string. We round to the
/// nearest 0..=255 byte for r/g/b so the parser in
/// `color_to_uicolor` reads the same way the web backend's
/// inline writes do — keeps cross-platform color rendering
/// consistent across animation paths.
fn rgba_to_css_string(value: [f32; 4]) -> String {
    let r = (value[0].clamp(0.0, 1.0) * 255.0).round() as u8;
    let g = (value[1].clamp(0.0, 1.0) * 255.0).round() as u8;
    let b = (value[2].clamp(0.0, 1.0) * 255.0).round() as u8;
    let a = value[3].clamp(0.0, 1.0);
    format!("rgba({}, {}, {}, {})", r, g, b, a)
}

pub(crate) type AnimatedStateMap = HashMap<usize, AnimatedTransformState>;

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_core::Easing;

    // The curve lives in bits 16..18 of the UIViewAnimationOptions
    // mask. These constants are part of UIKit's stable ABI; a
    // regression here would silently flip presence fades to the wrong
    // curve on device (the closest reachable test for an objc2-only
    // crate — device verification covers the actual animation).
    #[test]
    fn easing_maps_to_uiview_curve_bits() {
        assert_eq!(easing_to_uiview_options(Easing::EaseInOut), 0 << 16);
        assert_eq!(easing_to_uiview_options(Easing::EaseIn), 1 << 16);
        assert_eq!(easing_to_uiview_options(Easing::EaseOut), 2 << 16);
        assert_eq!(easing_to_uiview_options(Easing::Linear), 3 << 16);
        // Ease (CSS default) and CubicBezier fall back to ease-in-out.
        assert_eq!(easing_to_uiview_options(Easing::Ease), 0 << 16);
        assert_eq!(
            easing_to_uiview_options(Easing::CubicBezier(0.1, 0.2, 0.3, 0.4)),
            0 << 16
        );
    }

    #[test]
    fn presence_missing_fields_resolve_to_identity() {
        // Mirror the resolution logic in impl_apply_presence without
        // touching UIKit: a rest() state must produce opacity 1,
        // translate 0, scale 1.
        let s = runtime_core::PresenceState::rest();
        assert_eq!(s.opacity.unwrap_or(1.0), 1.0);
        assert_eq!(s.translate_x.unwrap_or(0.0), 0.0);
        assert_eq!(s.translate_y.unwrap_or(0.0), 0.0);
        assert_eq!(s.scale.unwrap_or(1.0), 1.0);
    }
}
