//! Android capture via `android.media.AudioRecord`, read on a dedicated
//! JNI worker thread.
//!
//! Threading: `AudioRecord.read(...)` blocks, so it must run off the main
//! looper. `open()` spawns one worker thread that attaches to the JavaVM
//! (via `ndk_context`, the same entry point the `net` SDK uses), builds
//! and starts the `AudioRecord`, then loops reading 16-bit PCM until the
//! stream's stop flag is set. Every JNI touch of the `AudioRecord` — build,
//! start, read, stop, release — happens on that one attached thread.
//!
//! Init result (granted+initialized / permission-denied / build-failed) is
//! reported back to the awaiting `open()` over a `futures-channel` oneshot
//! before the read loop begins.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use futures_channel::oneshot;
use jni::objects::{JObject, JValue};
use jni::JavaVM;

use crate::{AudioBuffer, AudioStreamConfig, BoxedCallback, MicError};

// android.media / AudioFormat constants (stable platform values).
const AUDIO_SOURCE_MIC: i32 = 1; // MediaRecorder.AudioSource.MIC
const CHANNEL_IN_MONO: i32 = 16; // AudioFormat.CHANNEL_IN_MONO (0x10)
const CHANNEL_IN_STEREO: i32 = 12; // AudioFormat.CHANNEL_IN_STEREO (0x0C)
const ENCODING_PCM_16BIT: i32 = 2; // AudioFormat.ENCODING_PCM_16BIT
const STATE_INITIALIZED: i32 = 1; // AudioRecord.STATE_INITIALIZED
const PERMISSION_GRANTED: i32 = 0; // PackageManager.PERMISSION_GRANTED
const RECORD_AUDIO: &str = "android.permission.RECORD_AUDIO";

/// Default capture rate when the caller doesn't pin one. 44.1 kHz is the
/// one rate every Android device is guaranteed to support for input.
const DEFAULT_SAMPLE_RATE: u32 = 44_100;

/// Stops the reader thread on drop. Setting the flag unblocks the loop
/// within one `read()` (~100 ms); the worker then stops + releases the
/// `AudioRecord` and detaches.
pub(crate) struct StreamHandle {
    running: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl StreamHandle {
    /// `AudioRecord` exposes no zero-copy native source; PCM flows through the
    /// CPU tap. A future `AudioTrack` playback path would publish a handle here.
    pub(crate) fn native_source(&self) -> Option<crate::NativeSource> {
        None
    }
}

/// Map the host `JavaVM` pointer. Panics-free; a bad pointer is a
/// bootstrap bug surfaced as a [`MicError::Backend`].
fn java_vm() -> Result<JavaVM, MicError> {
    let ctx = ndk_context::android_context();
    let vm_ptr = ctx.vm() as *mut jni::sys::JavaVM;
    unsafe { JavaVM::from_raw(vm_ptr) }
        .map_err(|e| MicError::Backend(format!("invalid JavaVM pointer: {e}")))
}

pub(crate) async fn request_permission() -> Result<(), MicError> {
    let vm = java_vm()?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| MicError::Backend(format!("JNI attach failed: {e}")))?;

    let ctx = ndk_context::android_context();
    let activity = unsafe { JObject::from_raw(ctx.context().cast()) };

    let granted = check_self_permission(&mut env, &activity)?;
    if granted {
        return Ok(());
    }

    // Not yet granted: surface the runtime dialog. Its result arrives in
    // the Activity's `onRequestPermissionsResult`, which this SDK doesn't
    // hook — so we fire the request and report the current (not-granted)
    // state. The caller re-checks (or retries `open`) after the user
    // responds. Documented in the crate README.
    let perm = env
        .new_string(RECORD_AUDIO)
        .map_err(map_jni)?;
    let arr = env
        .new_object_array(1, "java/lang/String", &perm)
        .map_err(map_jni)?;
    let _ = env.call_method(
        &activity,
        "requestPermissions",
        "([Ljava/lang/String;I)V",
        &[JValue::Object(&JObject::from(arr)), JValue::Int(0)],
    );
    Err(MicError::PermissionDenied)
}

/// Passive permission read via `checkSelfPermission` (no prompt). It only
/// distinguishes granted vs not-granted, and a not-granted result may be either
/// undetermined or denied — so we report `Granted` or `Undetermined` (the safe
/// "may still prompt" state), never a definitive `Denied`. Any JNI failure →
/// [`MicPermission::Unknown`](crate::MicPermission::Unknown).
pub(crate) async fn permission_status() -> crate::MicPermission {
    let Ok(vm) = java_vm() else {
        return crate::MicPermission::Unknown;
    };
    let Ok(mut env) = vm.attach_current_thread() else {
        return crate::MicPermission::Unknown;
    };
    let ctx = ndk_context::android_context();
    let activity = unsafe { JObject::from_raw(ctx.context().cast()) };
    match check_self_permission(&mut env, &activity) {
        Ok(true) => crate::MicPermission::Granted,
        Ok(false) => crate::MicPermission::Undetermined,
        Err(_) => crate::MicPermission::Unknown,
    }
}

pub(crate) async fn open(
    config: AudioStreamConfig,
    callback: BoxedCallback,
) -> Result<StreamHandle, MicError> {
    let vm = java_vm()?;
    let running = Arc::new(AtomicBool::new(true));
    let running_worker = running.clone();
    let (tx, rx) = oneshot::channel::<Result<(), MicError>>();

    let join = std::thread::spawn(move || {
        reader_thread(vm, config, callback, running_worker, tx);
    });

    // Wait for the worker to report whether the AudioRecord came up.
    match rx.await {
        Ok(Ok(())) => Ok(StreamHandle {
            running,
            join: Some(join),
        }),
        Ok(Err(e)) => {
            let _ = join.join();
            Err(e)
        }
        Err(_) => {
            let _ = join.join();
            Err(MicError::Backend("Android reader thread dropped".into()))
        }
    }
}

/// The worker body: attach, build + start the `AudioRecord`, report init,
/// then loop reading until the stop flag clears.
fn reader_thread(
    vm: JavaVM,
    config: AudioStreamConfig,
    mut callback: BoxedCallback,
    running: Arc<AtomicBool>,
    init_tx: oneshot::Sender<Result<(), MicError>>,
) {
    let mut env = match vm.attach_current_thread() {
        Ok(env) => env,
        Err(e) => {
            let _ = init_tx.send(Err(MicError::Backend(format!("JNI attach failed: {e}"))));
            return;
        }
    };

    let ctx = ndk_context::android_context();
    let activity = unsafe { JObject::from_raw(ctx.context().cast()) };

    // Permission gate — without RECORD_AUDIO the AudioRecord builds but
    // yields silence (or throws on newer APIs). Fail fast and clearly.
    match check_self_permission(&mut env, &activity) {
        Ok(true) => {}
        Ok(false) => {
            let _ = init_tx.send(Err(MicError::PermissionDenied));
            return;
        }
        Err(e) => {
            let _ = init_tx.send(Err(e));
            return;
        }
    }

    let sample_rate = config.sample_rate.unwrap_or(DEFAULT_SAMPLE_RATE);
    let channels = config.channels.unwrap_or(1).max(1);
    let channel_config = if channels >= 2 {
        CHANNEL_IN_STEREO
    } else {
        CHANNEL_IN_MONO
    };

    let record = match build_audio_record(&mut env, sample_rate, channels, channel_config) {
        Ok((record, frames_per_read)) => {
            let _ = init_tx.send(Ok(()));
            (record, frames_per_read)
        }
        Err(e) => {
            let _ = init_tx.send(Err(e));
            return;
        }
    };
    let (record, shorts_per_read) = record;

    // One reusable Java short[] for reads, and one reusable f32 scratch.
    let java_buf = match env.new_short_array(shorts_per_read as i32) {
        Ok(a) => a,
        Err(e) => {
            // Init already reported Ok; tear down rather than capture.
            let _ = env.call_method(&record, "stop", "()V", &[]);
            let _ = env.call_method(&record, "release", "()V", &[]);
            eprintln!("microphone: failed to allocate read buffer: {e}");
            return;
        }
    };
    let mut shorts = vec![0i16; shorts_per_read];
    let mut scratch = vec![0f32; shorts_per_read];

    while running.load(Ordering::SeqCst) {
        let read = env
            .call_method(
                &record,
                "read",
                "([SII)I",
                &[
                    JValue::Object(&java_buf),
                    JValue::Int(0),
                    JValue::Int(shorts_per_read as i32),
                ],
            )
            .and_then(|v| v.i());
        let count = match read {
            Ok(n) if n > 0 => n as usize,
            // 0 = nothing this cycle; negative = AudioRecord error code.
            Ok(0) => continue,
            Ok(_) => break,
            Err(_) => break,
        };

        if env
            .get_short_array_region(&java_buf, 0, &mut shorts[..count])
            .is_err()
        {
            break;
        }
        for (dst, &src) in scratch[..count].iter_mut().zip(&shorts[..count]) {
            *dst = src as f32 / 32768.0;
        }
        let buffer = AudioBuffer {
            samples: &scratch[..count],
            sample_rate,
            channels,
        };
        callback(&buffer);
    }

    let _ = env.call_method(&record, "stop", "()V", &[]);
    let _ = env.call_method(&record, "release", "()V", &[]);
    // `record` and `java_buf` local refs drop with the AttachGuard here.
}

/// `context.checkSelfPermission("android.permission.RECORD_AUDIO") == GRANTED`.
fn check_self_permission(
    env: &mut jni::JNIEnv<'_>,
    activity: &JObject<'_>,
) -> Result<bool, MicError> {
    let perm = env.new_string(RECORD_AUDIO).map_err(map_jni)?;
    let result = env
        .call_method(
            activity,
            "checkSelfPermission",
            "(Ljava/lang/String;)I",
            &[JValue::Object(&JObject::from(perm))],
        )
        .map_err(map_jni)?
        .i()
        .map_err(map_jni)?;
    Ok(result == PERMISSION_GRANTED)
}

/// Build, validate, and start an `AudioRecord`. Returns it alongside the
/// per-read length in shorts. Errors map device rejections to
/// [`MicError::UnsupportedConfig`].
fn build_audio_record<'a>(
    env: &mut jni::JNIEnv<'a>,
    sample_rate: u32,
    channels: u16,
    channel_config: i32,
) -> Result<(JObject<'a>, usize), MicError> {
    // int minBytes = AudioRecord.getMinBufferSize(rate, chanCfg, fmt);
    let min_bytes = env
        .call_static_method(
            "android/media/AudioRecord",
            "getMinBufferSize",
            "(III)I",
            &[
                JValue::Int(sample_rate as i32),
                JValue::Int(channel_config),
                JValue::Int(ENCODING_PCM_16BIT),
            ],
        )
        .map_err(map_jni)?
        .i()
        .map_err(map_jni)?;
    if min_bytes <= 0 {
        return Err(MicError::UnsupportedConfig(format!(
            "AudioRecord.getMinBufferSize rejected rate={sample_rate} channels={channels}"
        )));
    }

    // Capture buffer: at least the device minimum, but no smaller than
    // ~100 ms so the read loop wakes at a sane cadence.
    let target_bytes = (sample_rate as i32) * (channels as i32) * 2 / 10;
    let buffer_bytes = min_bytes.max(target_bytes);

    // AudioRecord rec = new AudioRecord(MIC, rate, chanCfg, fmt, bytes);
    let record = env
        .new_object(
            "android/media/AudioRecord",
            "(IIIII)V",
            &[
                JValue::Int(AUDIO_SOURCE_MIC),
                JValue::Int(sample_rate as i32),
                JValue::Int(channel_config),
                JValue::Int(ENCODING_PCM_16BIT),
                JValue::Int(buffer_bytes),
            ],
        )
        .map_err(|e| MicError::Backend(format!("new AudioRecord: {e}")))?;

    let state = env
        .call_method(&record, "getState", "()I", &[])
        .map_err(map_jni)?
        .i()
        .map_err(map_jni)?;
    if state != STATE_INITIALIZED {
        let _ = env.call_method(&record, "release", "()V", &[]);
        return Err(MicError::UnsupportedConfig(format!(
            "AudioRecord did not initialize (state={state}) for rate={sample_rate} channels={channels}"
        )));
    }

    env.call_method(&record, "startRecording", "()V", &[])
        .map_err(|e| MicError::Backend(format!("startRecording: {e}")))?;

    // Read a quarter of the buffer per cycle (in shorts = bytes / 2).
    let shorts_per_read = (buffer_bytes as usize / 2 / 4).max(channels as usize);
    Ok((record, shorts_per_read))
}

fn map_jni(e: jni::errors::Error) -> MicError {
    MicError::Backend(format!("JNI: {e}"))
}
