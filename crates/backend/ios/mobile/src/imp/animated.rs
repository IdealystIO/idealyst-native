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

use framework_core::animation::AnimProp;
use framework_core::Color;
use objc2::encode::{Encode, Encoding};
use objc2::msg_send;
use objc2::rc::Retained;
use objc2_foundation::{CGFloat, NSObject};

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

impl IosBackend {
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
            | AnimProp::RotateZ => {}
        }
    }

    /// Drop per-node animation state. Called from the existing
    /// view-teardown path so we don't keep stale state alive for
    /// views that have been removed from their parent.
    pub(crate) fn impl_drop_animated_state(&mut self, key: usize) {
        self.animated_states.remove(&key);
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
