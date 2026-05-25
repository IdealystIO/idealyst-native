//! `Primitive::Toggle` — `android.widget.Switch` with a Kotlin-side
//! `RustToggleListener` bridge.

use crate::imp::callbacks::{leak, ToggleChangeCallback};
use backend_android_core::helpers::apply_default_layout_params;
use crate::imp::{with_env, AndroidBackend};
use jni::objects::{GlobalRef, JValue};
use jni::sys::jlong;
use std::rc::Rc;

pub(crate) fn create(
    b: &mut AndroidBackend,
    initial_value: bool,
    on_change: Rc<dyn Fn(bool)>,
) -> GlobalRef {
    let node = with_env(|env| {
        let class = env.find_class("android/widget/Switch").unwrap();
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .unwrap();
        // Initial value.
        let _ = env.call_method(
            &local,
            "setChecked",
            "(Z)V",
            &[JValue::Bool(if initial_value { 1 } else { 0 })],
        );
        // Wire the listener.
        let ptr: jlong = leak(ToggleChangeCallback(on_change));
        let listener_class = env
            .find_class("io/idealyst/runtime/RustToggleListener")
            .unwrap();
        let listener = env
            .new_object(&listener_class, "(J)V", &[JValue::Long(ptr)])
            .unwrap();
        let _ = env.call_method(
            &local,
            "setOnCheckedChangeListener",
            "(Landroid/widget/CompoundButton$OnCheckedChangeListener;)V",
            &[JValue::Object(&listener)],
        );
        // Stash the listener on the Switch's tag so update_value can
        // retrieve it later and flip its `suppress` flag without
        // having to detach + reattach.
        let _ = env.call_method(
            &local,
            "setTag",
            "(Ljava/lang/Object;)V",
            &[JValue::Object(&listener)],
        );
        apply_default_layout_params(env, &local);
        env.new_global_ref(local).unwrap()
    });
    // Install a Taffy `measure_fn` so the flex parent reads the
    // Switch's real Material dimensions (~52dp × 32dp) instead of
    // collapsing it to a 0×0 leaf. Without this the toggle vanishes
    // from layout — the Theme caption above it still renders because
    // the Text has its own measure_fn, but the Switch contributes
    // zero height to the surrounding column and gets clipped /
    // hidden. Same hazard the iOS backend documents in
    // [[project_ios_intrinsic_size_measurer]] for UISwitch/UISlider.
    install_intrinsic_measure(b, &node);
    node
}

/// Hook Taffy up to the view's own `measure()` so multi-line wrap
/// and the system-rendered Material control surface report their
/// real dimensions back to the flex layout. Pattern matches
/// `text::create`'s `set_measure_fn` install but without text-
/// specific bookkeeping (no `getText`, etc.).
fn install_intrinsic_measure(b: &mut AndroidBackend, view: &GlobalRef) {
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
                let density =
                    crate::imp::density_of(env, &obj).unwrap_or(1.0);
                let avail_w_px = if avail_w.is_finite() {
                    (avail_w * density).round() as i32
                } else {
                    0
                };
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

/// Apply a programmatic value to the toggle. Suppresses the
/// `OnCheckedChangeListener` for the duration of the `setChecked`
/// call — without this guard, an runtime-server-driven value update (server
/// emits the authoritative `value` over the wire) would race a
/// recent user tap: `setChecked` fires the listener → listener
/// sends the new value back to the server as an event → server
/// rewrites `is_dark`/etc. and re-emits → loop. The visible
/// symptom is the toggle flipping on its own a few times after a
/// spam-click. We retrieve the listener via the tag set in `create`
/// rather than detach-and-reattach, which would race with other
/// callbacks scheduled on the same JNI thread.
pub(crate) fn update_value(node: &GlobalRef, value: bool) {
    with_env(|env| {
        let tag = env
            .call_method(node.as_obj(), "getTag", "()Ljava/lang/Object;", &[])
            .ok()
            .and_then(|v| v.l().ok());
        if let Some(ref listener) = tag {
            if !listener.is_null() {
                let _ = env.set_field(listener, "suppress", "Z", JValue::Bool(1));
            }
        }
        let _ = env.call_method(
            node.as_obj(),
            "setChecked",
            "(Z)V",
            &[JValue::Bool(if value { 1 } else { 0 })],
        );
        if let Some(ref listener) = tag {
            if !listener.is_null() {
                let _ = env.set_field(listener, "suppress", "Z", JValue::Bool(0));
            }
        }
    });
}
