//! Shared Taffy `measure_fn` for system-rendered native widgets.
//!
//! Android controls like `Button`, `Switch`, and `SeekBar` have a real
//! intrinsic size (Material metrics — a button's text + padding, a switch's
//! ~52×32dp track, a seekbar's ~48dp height), but Taffy doesn't know it
//! unless we hook the view's own `measure()` into the layout leaf. Without
//! this, such a control is a 0×0 flex leaf: it contributes no height to its
//! column and gets clipped or vanishes entirely — text siblings still render
//! (they have `text::create`'s own measure_fn), so the symptom is "everything
//! shows except the button/toggle/slider."
//!
//! This is the Android twin of the iOS intrinsic-content-size measurer
//! ([[project_ios_intrinsic_size_measurer]], which sizes UISwitch/UISlider/etc
//! from `intrinsicContentSize`). It carries no widget-specific logic — it just
//! asks the view to measure itself at the available width and reports the
//! result in dp — so every native control routes through this one helper.

use crate::imp::{with_env, AndroidBackend};
use jni::objects::{GlobalRef, JValue};

/// Install a Taffy `measure_fn` on `view`'s layout leaf that drives the view's
/// own `View.measure(...)` and returns its measured size in dp. Call this for
/// any system-rendered control that would otherwise collapse to a 0×0 leaf.
pub(crate) fn install_intrinsic_measure(b: &mut AndroidBackend, view: &GlobalRef) {
    let layout = b.layout_for_view(view);
    let view_for_measure = view.clone();
    b.layout.set_measure_fn(
        layout,
        std::rc::Rc::new(move |known_dimensions, available_space| {
            let avail_w = known_dimensions
                .width
                .unwrap_or(match available_space.width {
                    runtime_layout::AvailableSpace::Definite(w) => w,
                    runtime_layout::AvailableSpace::MaxContent => f32::INFINITY,
                    runtime_layout::AvailableSpace::MinContent => 0.0,
                });
            with_env(|env| {
                let obj = view_for_measure.as_obj();
                let density = crate::imp::density_of(env, &obj).unwrap_or(1.0);
                let avail_w_px = if avail_w.is_finite() {
                    (avail_w * density).round() as i32
                } else {
                    0
                };
                // MeasureSpec: AT_MOST (0x8000_0000) packs the bounded width;
                // UNSPECIFIED (0) lets the view pick its natural height.
                let at_most: i32 = -2_147_483_648;
                let unspec: i32 = 0;
                let width_spec = if avail_w.is_finite() {
                    at_most | (avail_w_px & 0x3fff_ffff)
                } else {
                    unspec
                };
                let _ = env.call_method(
                    &obj,
                    "measure",
                    "(II)V",
                    &[JValue::Int(width_spec), JValue::Int(unspec)],
                );
                let measured_w_px: i32 = env
                    .call_method(&obj, "getMeasuredWidth", "()I", &[])
                    .and_then(|v| v.i())
                    .unwrap_or(0);
                let measured_h_px: i32 = env
                    .call_method(&obj, "getMeasuredHeight", "()I", &[])
                    .and_then(|v| v.i())
                    .unwrap_or(0);
                let w_dp = measured_w_px as f32 / density;
                let h_dp = measured_h_px as f32 / density;
                runtime_layout::Size {
                    width: known_dimensions.width.unwrap_or(w_dp.ceil()),
                    height: known_dimensions.height.unwrap_or(h_dp.ceil()),
                }
            })
        }),
    );
}
