//! Android haptics — `Vibrator` / `VibrationEffect` via JNI.
//!
//! **Compile-checked only ⚠️** — the JNI calls compile, but the physical
//! buzz has not been verified on a device/emulator from this crate. The
//! method signatures below are the documented `android.os.Vibrator` /
//! `VibrationEffect` API.
//!
//! ## Getting the Vibrator
//!
//! - **API 31+ (S):** `context.getSystemService(VibratorManager.class)` then
//!   `manager.getDefaultVibrator()`.
//! - **API < 31:** `context.getSystemService(Context.VIBRATOR_SERVICE)`.
//!
//! We try the API-31 path first and fall back, so one binary covers both. The
//! `Context` + host `JavaVM` come from `ndk_context`, mirroring the `storage`
//! / `microphone` SDKs; JNI work runs on the calling thread (attached per
//! call — a vibrate is cheap and rare, so per-call attach is fine).
//!
//! ## Mapping the API to effects
//!
//! On **API 26+ (O)** we use predefined / one-shot `VibrationEffect`s:
//! - `notify` → `createPredefined(EFFECT_*)` where available; impact styles →
//!   `createOneShot(durationMs, DEFAULT_AMPLITUDE)` with a per-style duration.
//! - `selection` → a very short `createOneShot` tick.
//!
//! On **API < 26** there's only the deprecated `vibrate(long milliseconds)`,
//! so every effect collapses to a duration-only buzz.
//!
//! ## Permission
//!
//! Requires `<uses-permission android:name="android.permission.VIBRATE"/>`.
//! That's a normal (install-time) permission — no runtime prompt — declared
//! by this crate's `capabilities = ["haptics"]` and injected by the CLI.
//! Without it the system silently ignores the calls (still a safe no-op).

use jni::objects::{JObject, JValue};
use jni::JavaVM;

use crate::{ImpactStyle, NotificationFeedback};

// VibrationEffect predefined constants (API 29+ for these; EFFECT_CLICK is
// API 29). Raw ints from android.os.VibrationEffect.
const EFFECT_CLICK: i32 = 0;
const EFFECT_DOUBLE_CLICK: i32 = 1;
const EFFECT_HEAVY_CLICK: i32 = 5;
// VibrationEffect.DEFAULT_AMPLITUDE.
const DEFAULT_AMPLITUDE: i32 = -1;

// Per-style one-shot durations (ms) for the API 26 createOneShot fallback and
// for the API<26 deprecated path. Tuned to read as light→heavy.
const DUR_LIGHT: i64 = 10;
const DUR_MEDIUM: i64 = 20;
const DUR_HEAVY: i64 = 40;
const DUR_SOFT: i64 = 12;
const DUR_RIGID: i64 = 16;
const DUR_SELECTION: i64 = 5;

fn impact_duration(style: ImpactStyle) -> i64 {
    match style {
        ImpactStyle::Light => DUR_LIGHT,
        ImpactStyle::Medium => DUR_MEDIUM,
        ImpactStyle::Heavy => DUR_HEAVY,
        ImpactStyle::Soft => DUR_SOFT,
        ImpactStyle::Rigid => DUR_RIGID,
    }
}

/// Run `f` with an attached `JNIEnv` and the resolved `Vibrator` object.
/// Returns `None` (silent no-op) on any JNI failure — haptics are
/// best-effort and a missing service / denied permission must never panic.
fn with_vibrator<R>(
    f: impl FnOnce(&mut jni::JNIEnv, &JObject) -> Result<R, jni::errors::Error>,
) -> Option<R> {
    let ctx = ndk_context::android_context();
    let vm = unsafe { JavaVM::from_raw(ctx.vm() as *mut jni::sys::JavaVM) }.ok()?;
    let mut env = vm.attach_current_thread().ok()?;
    let activity = unsafe { JObject::from_raw(ctx.context().cast()) };

    let vibrator = resolve_vibrator(&mut env, &activity)?;
    f(&mut env, &vibrator).ok()
}

/// Resolve the `Vibrator`, trying the API-31 `VibratorManager` first and
/// falling back to the legacy `VIBRATOR_SERVICE` lookup.
fn resolve_vibrator<'a>(
    env: &mut jni::JNIEnv<'a>,
    activity: &JObject<'a>,
) -> Option<JObject<'a>> {
    // --- API 31+: getSystemService(VibratorManager.class).getDefaultVibrator()
    if let Some(manager) = system_service_by_class(env, activity, "android/os/VibratorManager") {
        if let Ok(v) = env
            .call_method(
                &manager,
                "getDefaultVibrator",
                "()Landroid/os/Vibrator;",
                &[],
            )
            .and_then(|r| r.l())
        {
            if !v.is_null() {
                return Some(v);
            }
        }
    }

    // --- API < 31: getSystemService("vibrator")
    let name = env.new_string("vibrator").ok()?;
    let v = env
        .call_method(
            activity,
            "getSystemService",
            "(Ljava/lang/String;)Ljava/lang/Object;",
            &[JValue::Object(&JObject::from(name))],
        )
        .ok()?
        .l()
        .ok()?;
    (!v.is_null()).then_some(v)
}

/// `context.getSystemService(<Class>.class)` — looks up the static `.class`
/// field via JNI and calls the Class-overload of `getSystemService`.
fn system_service_by_class<'a>(
    env: &mut jni::JNIEnv<'a>,
    activity: &JObject<'a>,
    class_path: &str,
) -> Option<JObject<'a>> {
    // `FindClass` returns null + throws if the class is absent (API < 31);
    // clear the pending exception and fall back.
    let class = match env.find_class(class_path) {
        Ok(c) => c,
        Err(_) => {
            let _ = env.exception_clear();
            return None;
        }
    };
    let svc = env
        .call_method(
            activity,
            "getSystemService",
            "(Ljava/lang/Class;)Ljava/lang/Object;",
            &[JValue::Object(&JObject::from(class))],
        )
        .ok()?
        .l()
        .ok()?;
    (!svc.is_null()).then_some(svc)
}

/// API level (`Build.VERSION.SDK_INT`). `0` if it can't be read — callers
/// then take the most-conservative (legacy) path.
fn sdk_int(env: &mut jni::JNIEnv) -> i32 {
    env.get_static_field("android/os/Build$VERSION", "SDK_INT", "I")
        .and_then(|v| v.i())
        .unwrap_or(0)
}

/// `VibrationEffect.createOneShot(durationMs, amplitude)`.
fn create_one_shot<'a>(
    env: &mut jni::JNIEnv<'a>,
    duration_ms: i64,
    amplitude: i32,
) -> Option<JObject<'a>> {
    env.call_static_method(
        "android/os/VibrationEffect",
        "createOneShot",
        "(JI)Landroid/os/VibrationEffect;",
        &[JValue::Long(duration_ms), JValue::Int(amplitude)],
    )
    .ok()?
    .l()
    .ok()
}

/// `VibrationEffect.createPredefined(effectId)` (API 29+).
fn create_predefined<'a>(env: &mut jni::JNIEnv<'a>, effect_id: i32) -> Option<JObject<'a>> {
    env.call_static_method(
        "android/os/VibrationEffect",
        "createPredefined",
        "(I)Landroid/os/VibrationEffect;",
        &[JValue::Int(effect_id)],
    )
    .ok()?
    .l()
    .ok()
}

/// `vibrator.vibrate(effect)` (API 26+).
fn vibrate_effect(env: &mut jni::JNIEnv, vibrator: &JObject, effect: &JObject) {
    let _ = env.call_method(
        vibrator,
        "vibrate",
        "(Landroid/os/VibrationEffect;)V",
        &[JValue::Object(effect)],
    );
}

/// Deprecated `vibrator.vibrate(long milliseconds)` (API < 26 fallback).
fn vibrate_legacy(env: &mut jni::JNIEnv, vibrator: &JObject, duration_ms: i64) {
    let _ = env.call_method(vibrator, "vibrate", "(J)V", &[JValue::Long(duration_ms)]);
}

/// Vibrate either via a modern `VibrationEffect` (built by `build`) on API
/// 26+, or the deprecated duration call on older devices.
fn vibrate(
    duration_ms: i64,
    build: impl for<'a> FnOnce(&mut jni::JNIEnv<'a>) -> Option<JObject<'a>>,
) {
    with_vibrator(|env, vibrator| {
        if sdk_int(env) >= 26 {
            if let Some(eff) = build(env) {
                vibrate_effect(env, vibrator, &eff);
                return Ok(());
            }
        }
        vibrate_legacy(env, vibrator, duration_ms);
        Ok(())
    });
}

pub(crate) fn impact(style: ImpactStyle) {
    let dur = impact_duration(style);
    // Heavy/Rigid map to the crisper predefined clicks where available;
    // everything else is a duration-tuned one-shot.
    vibrate(dur, move |env| match style {
        ImpactStyle::Heavy | ImpactStyle::Rigid => create_predefined(env, EFFECT_HEAVY_CLICK)
            .or_else(|| create_one_shot(env, dur, DEFAULT_AMPLITUDE)),
        _ => create_one_shot(env, dur, DEFAULT_AMPLITUDE),
    });
}

pub(crate) fn notify(feedback: NotificationFeedback) {
    // Predefined effects read as distinct "notification" patterns; fall back
    // to a one-shot of a representative duration if predefined is missing.
    let (effect_id, dur) = match feedback {
        NotificationFeedback::Success => (EFFECT_CLICK, DUR_MEDIUM),
        NotificationFeedback::Warning => (EFFECT_DOUBLE_CLICK, DUR_HEAVY),
        NotificationFeedback::Error => (EFFECT_HEAVY_CLICK, DUR_HEAVY),
    };
    vibrate(dur, move |env| {
        create_predefined(env, effect_id).or_else(|| create_one_shot(env, dur, DEFAULT_AMPLITUDE))
    });
}

pub(crate) fn selection() {
    vibrate(DUR_SELECTION, move |env| {
        create_one_shot(env, DUR_SELECTION, DEFAULT_AMPLITUDE)
    });
}

pub(crate) fn is_supported() -> bool {
    // `vibrator.hasVibrator()` is the honest query.
    with_vibrator(|env, vibrator| {
        env.call_method(vibrator, "hasVibrator", "()Z", &[])
            .and_then(|r| r.z())
    })
    .unwrap_or(false)
}
