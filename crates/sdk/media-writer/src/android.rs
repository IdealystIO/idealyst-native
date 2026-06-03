//! Android recording via **MediaCodec** + **MediaMuxer**, bridged through a
//! Kotlin shim ([`RustMediaWriter`]) shipped from this crate via
//! `[package.metadata.idealyst.android].runtime_kotlin`.
//!
//! ## Why a Kotlin shim
//!
//! Unlike the Apple backend (which drives `AVAssetWriter` directly through the
//! Obj-C runtime), Android's encode/mux path is a tangle of stateful Java
//! objects — `MediaCodec` input/output buffer dequeue loops, `MediaCodec.Image`
//! plane filling, `MediaMuxer` track-add/start coordination across two codecs —
//! that is far cleaner to express in Kotlin than through raw JNI. The shim owns
//! the codecs and the muxer; Rust forwards each RGBA frame / PCM chunk to it.
//!
//! ## Direction of calls
//!
//! This is the inverse of `camera`'s Android backend: there, Kotlin trampolines
//! frames *into* Rust; here, Rust forwards frames *into* Kotlin. So there are
//! no `#[no_mangle]` trampolines — `start` mints a recorder and returns a
//! `token`, the capture taps call `writeVideo`/`writeAudio` with that token,
//! and `stop` finalizes. Audio is converted from interleaved `f32` to the
//! 16-bit LE PCM the AAC encoder consumes before it crosses the boundary.
//!
//! ## VERIFICATION
//!
//! Compile-checked for `aarch64-linux-android`, but **not yet device-verified**
//! — the JNI signatures and the Kotlin MediaCodec/MediaMuxer path resolve only
//! at runtime on a device (same posture as the `camera` and `biometrics`
//! Android backends). Every failure is surfaced as a typed
//! [`MediaWriterError`] carrying the JNI/Android message.

use jni::objects::{JObject, JValue};
use jni::sys::{jint, jlong};
use jni::{JNIEnv, JavaVM};

use crate::{MediaInputs, MediaWriterError, RecordConfig};
use media_stream::{AudioSubscription, Subscription};

const HELPER: &str = "io/idealyst/mediawriter/RustMediaWriter";

pub(crate) struct RecordingHandle {
    token: u64,
    _video_sub: Option<Subscription>,
    _audio_sub: Option<AudioSubscription>,
}

impl RecordingHandle {
    pub(crate) async fn stop(mut self) -> Result<(), MediaWriterError> {
        // Stop the taps first so no further frames enqueue into the shim.
        self._video_sub = None;
        self._audio_sub = None;

        let vm = java_vm()?;
        let mut env = vm.attach_current_thread().map_err(jni_err)?;
        let ok = env
            .call_static_method(
                HELPER,
                "stop",
                "(J)Z",
                &[JValue::Long(self.token as jlong)],
            )
            .and_then(|v| v.z())
            .map_err(jni_err)?;
        if ok {
            Ok(())
        } else {
            Err(MediaWriterError::Backend(
                "MediaMuxer finalize failed (see logcat)".into(),
            ))
        }
    }
}

impl Drop for RecordingHandle {
    fn drop(&mut self) {
        // If `stop` already ran, `token` is finalized and this `abort` is a
        // no-op on the Kotlin side. Otherwise discard the partial file.
        if let Ok(vm) = java_vm() {
            if let Ok(mut env) = vm.attach_current_thread() {
                let _ = env.call_static_method(
                    HELPER,
                    "abort",
                    "(J)V",
                    &[JValue::Long(self.token as jlong)],
                );
            }
        }
    }
}

pub(crate) async fn start(
    inputs: MediaInputs<'_>,
    config: &RecordConfig,
) -> Result<RecordingHandle, MediaWriterError> {
    let path = config
        .store
        .local_path(&config.path)
        .ok_or(MediaWriterError::NoLocalPath)?;
    let path = path.to_string_lossy().into_owned();

    let vm = java_vm()?;
    let mut env = vm.attach_current_thread().map_err(jni_err)?;

    let jpath = env.new_string(&path).map_err(jni_err)?;
    let token = env
        .call_static_method(
            HELPER,
            "start",
            "(Ljava/lang/String;ZZIII)J",
            &[
                JValue::Object(&JObject::from(jpath)),
                JValue::Bool(inputs.video.is_some() as u8),
                JValue::Bool(inputs.audio.is_some() as u8),
                JValue::Int(config.fps.max(1) as jint),
                JValue::Int(config.video_bitrate.unwrap_or(0) as jint),
                JValue::Int(config.audio_bitrate.unwrap_or(0) as jint),
            ],
        )
        .and_then(|v| v.j())
        .map_err(jni_err)?;
    if token <= 0 {
        return Err(MediaWriterError::Backend(
            "RustMediaWriter.start failed (see logcat)".into(),
        ));
    }
    let token = token as u64;

    let video_sub = inputs.video.map(|stream| {
        stream.subscribe(move |f| {
            forward_video(token, f.width, f.height, f.pts_micros, f.data);
        })
    });
    let audio_sub = inputs.audio.map(|stream| {
        stream.subscribe(move |f| {
            forward_audio(token, f.sample_rate, f.channels, f.pts_micros, f.samples);
        })
    });

    Ok(RecordingHandle {
        token,
        _video_sub: video_sub,
        _audio_sub: audio_sub,
    })
}

/// Forward one RGBA frame to the Kotlin shim. Runs on the capture tap's thread,
/// so it attaches that thread to the JVM for the duration of the call.
fn forward_video(token: u64, width: u32, height: u32, pts_us: u64, rgba: &[u8]) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Ok(vm) = java_vm() else { return };
        let Ok(mut env) = vm.attach_current_thread() else {
            return;
        };
        let Ok(arr) = env.byte_array_from_slice(rgba) else {
            return;
        };
        let _ = env.call_static_method(
            HELPER,
            "writeVideo",
            "(J[BIIJ)V",
            &[
                JValue::Long(token as jlong),
                JValue::Object(&JObject::from(arr)),
                JValue::Int(width as jint),
                JValue::Int(height as jint),
                JValue::Long(pts_us as jlong),
            ],
        );
    }));
}

/// Forward one PCM chunk to the shim, converting interleaved `f32` to the
/// 16-bit little-endian PCM the AAC encoder consumes.
fn forward_audio(token: u64, sample_rate: u32, channels: u16, pts_us: u64, samples: &[f32]) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Ok(vm) = java_vm() else { return };
        let Ok(mut env) = vm.attach_current_thread() else {
            return;
        };
        let mut pcm = Vec::with_capacity(samples.len() * 2);
        for &s in samples {
            let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            pcm.extend_from_slice(&v.to_le_bytes());
        }
        let Ok(arr) = env.byte_array_from_slice(&pcm) else {
            return;
        };
        let _ = env.call_static_method(
            HELPER,
            "writeAudio",
            "(J[BIIJ)V",
            &[
                JValue::Long(token as jlong),
                JValue::Object(&JObject::from(arr)),
                JValue::Int(sample_rate as jint),
                JValue::Int(channels as jint),
                JValue::Long(pts_us as jlong),
            ],
        );
    }));
}

// ---------------------------------------------------------------------------
// JNI helpers.
// ---------------------------------------------------------------------------

fn java_vm() -> Result<JavaVM, MediaWriterError> {
    let ctx = ndk_context::android_context();
    let vm_ptr = ctx.vm() as *mut jni::sys::JavaVM;
    unsafe { JavaVM::from_raw(vm_ptr) }
        .map_err(|e| MediaWriterError::Backend(format!("invalid JavaVM pointer: {e}")))
}

fn jni_err(e: jni::errors::Error) -> MediaWriterError {
    MediaWriterError::Backend(format!("JNI: {e}"))
}

// Silence an unused-import lint on the rare path where `JNIEnv` isn't named
// directly (kept for signature clarity in the helpers above).
#[allow(dead_code)]
fn _env_marker(_: &JNIEnv<'_>) {}
