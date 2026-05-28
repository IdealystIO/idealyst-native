//! `Element::Slider` — `android.widget.SeekBar` with the user's
//! f32 range mapped to an integer progress.

use crate::imp::callbacks::{leak, SliderChangeCallback};
use backend_android_core::helpers::apply_default_layout_params;
use crate::imp::{with_env, AndroidBackend};
use jni::objects::{GlobalRef, JValue};
use jni::sys::jlong;
use std::rc::Rc;

pub(crate) fn create(
    b: &AndroidBackend,
    initial_value: f32,
    min: f32,
    max: f32,
    step: Option<f32>,
    on_change: Rc<dyn Fn(f32)>,
) -> GlobalRef {
    // SeekBar is the simplest native slider widget. Its progress is
    // an integer in [0, max], so we scale our f32 value range to that
    // integer range and reverse the mapping in the listener. Step is
    // forwarded too — the framework applies the snap in its on_change
    // wrapper before we get here, so we just pick a reasonable
    // integer resolution.
    //
    // Resolution: 1000 integer steps if continuous; otherwise use
    // enough steps to cover the requested step granularity.
    with_env(|env| {
        let class = env.find_class("android/widget/SeekBar").unwrap();
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .unwrap();
        let resolution = match step {
            Some(s) if s > 0.0 => ((max - min) / s).round().max(1.0) as i32,
            _ => 1000,
        };
        let _ = env.call_method(&local, "setMax", "(I)V", &[JValue::Int(resolution)]);
        let initial_int =
            ((initial_value - min) / (max - min) * resolution as f32).round() as i32;
        let _ = env.call_method(&local, "setProgress", "(I)V", &[JValue::Int(initial_int)]);
        // Wire the listener via Kotlin trampoline.
        let ptr: jlong = leak(SliderChangeCallback {
            on_change,
            min,
            max,
            resolution,
        });
        let listener_class = env
            .find_class("io/idealyst/runtime/RustSliderListener")
            .unwrap();
        let listener = env
            .new_object(&listener_class, "(J)V", &[JValue::Long(ptr)])
            .unwrap();
        let _ = env.call_method(
            &local,
            "setOnSeekBarChangeListener",
            "(Landroid/widget/SeekBar$OnSeekBarChangeListener;)V",
            &[JValue::Object(&listener)],
        );
        apply_default_layout_params(env, &local);
        env.new_global_ref(local).unwrap()
    })
}

pub(crate) fn update_value(_node: &GlobalRef, _value: f32) {
    // Without min/max here we can't convert back to integer progress
    // generically. For v1 we accept that the framework-side
    // controlled write-back doesn't refresh SeekBar visually after
    // on_change — Android's SeekBar tracks user drag visually itself,
    // so this only matters when the parent programmatically `.set()`s
    // a value. Worth revisiting when this becomes an issue.
}
