//! `Primitive::Text` — `android.widget.TextView`.

use backend_android_core::helpers::{apply_default_layout_params, set_text};
use crate::imp::{with_env, AndroidBackend};
use jni::objects::{GlobalRef, JValue};

pub(crate) fn create(b: &mut AndroidBackend, content: &str) -> GlobalRef {
    let view = with_env(|env| {
        let class = env.find_class("android/widget/TextView").unwrap();
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .unwrap();
        set_text(env, &local, content);
        apply_default_layout_params(env, &local);
        env.new_global_ref(local).unwrap()
    });
    // Install a Taffy measure function so flex layout can ask the
    // TextView how tall it needs to be for a given available width.
    // Without this, the framework's `Text` collapses to 0×0 and
    // every flex sibling around it gets a 0-height row — the welcome
    // screen's "Welcome to Idealyst" headline was the user-visible
    // symptom.
    let layout = b.layout_for_view(&view);
    let view_for_measure = view.clone();
    b.layout.set_measure_fn(
        layout,
        std::rc::Rc::new(move |known_dimensions, available_space| {
            let avail_w = known_dimensions
                .width
                .unwrap_or(match available_space.width {
                    native_layout::AvailableSpace::Definite(w) => w,
                    native_layout::AvailableSpace::MaxContent => f32::INFINITY,
                    native_layout::AvailableSpace::MinContent => 0.0,
                });
            measure_textview(&view_for_measure, avail_w, known_dimensions)
        }),
    );
    view
}

/// Ask the TextView (via JNI) how big it wants to be for a given
/// `available_width`. Goes through `View.measure(widthSpec,
/// heightSpec)` with AT_MOST/UNSPECIFIED specs depending on what
/// the known-dimensions slot the caller supplied, then reads back
/// `getMeasuredWidth()`/`getMeasuredHeight()` in dp.
fn measure_textview(
    view: &GlobalRef,
    avail_w_dp: f32,
    known_dimensions: native_layout::Size<Option<f32>>,
) -> native_layout::Size<f32> {
    with_env(|env| {
        let view_obj = view.as_obj();
        // dp → px for the MeasureSpec.
        let density = super::super::density_of(env, &view_obj).unwrap_or(1.0);
        let avail_w_px = if avail_w_dp.is_finite() {
            (avail_w_dp * density).round() as i32
        } else {
            // No upper bound — use UNSPECIFIED. `0 | UNSPECIFIED`
            // is the spec value (UNSPECIFIED = 0).
            0
        };
        // MeasureSpec mode constants:
        //   UNSPECIFIED = 0 << 30 = 0
        //   EXACTLY     = 1 << 30 = 0x40000000
        //   AT_MOST     = 2 << 30 = 0x80000000 (as i32: -2147483648)
        let at_most: i32 = -2_147_483_648; // 2 << 30
        let unspec: i32 = 0;
        let width_spec = if avail_w_dp.is_finite() {
            // AT_MOST | avail_w_px (lower 30 bits)
            at_most | (avail_w_px & 0x3fff_ffff)
        } else {
            unspec
        };
        // No height constraint: UNSPECIFIED → TextView picks its
        // natural height for the given width.
        let height_spec = unspec;
        let _ = env.call_method(
            &view_obj,
            "measure",
            "(II)V",
            &[JValue::Int(width_spec), JValue::Int(height_spec)],
        );
        let measured_w_px: i32 = env
            .call_method(&view_obj, "getMeasuredWidth", "()I", &[])
            .and_then(|v| v.i())
            .unwrap_or(0);
        let measured_h_px: i32 = env
            .call_method(&view_obj, "getMeasuredHeight", "()I", &[])
            .and_then(|v| v.i())
            .unwrap_or(0);
        let w_dp = measured_w_px as f32 / density;
        let h_dp = measured_h_px as f32 / density;
        native_layout::Size {
            width: known_dimensions.width.unwrap_or(w_dp.ceil()),
            height: known_dimensions.height.unwrap_or(h_dp.ceil()),
        }
    })
}

pub(crate) fn update_text(node: &GlobalRef, content: &str) {
    with_env(|env| set_text(env, &node.as_obj(), content));
}
