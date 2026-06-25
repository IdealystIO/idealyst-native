//! Android playback via `android.media.MediaPlayer`, driven over JNI.
//!
//! `MediaPlayer` is the standard player for tracks and longer sounds:
//! `setDataSource(path)` → `prepare()` → `start()`, with `pause()` /
//! `stop()` / `release()`, `setVolume(left, right)`, `setLooping(bool)`, and
//! `isPlaying()`. We use the file/path data source: a `Bytes` source is
//! written to a temp file under the app's cache dir at `prepare` time and
//! played from there.
//!
//! ## SoundPool (the low-latency-SFX alternative, out of scope here)
//!
//! `MediaPlayer` carries per-instance setup cost and is single-voice, so
//! it's the wrong tool for many tiny overlapping sound effects (UI clicks,
//! game hits). Android's `SoundPool` pre-decodes short clips into memory and
//! plays many concurrently with low latency — the natural backend for a
//! future SFX-pool layer. This crate ships the `MediaPlayer` path (tracks +
//! one-shot sounds) and names `SoundPool` as the place polyphonic SFX go.
//! Concretely: on Android, a second [`Sound::play`](crate::Sound::play)
//! while the first voice is alive *restarts* this player rather than layering
//! a second copy (one `MediaPlayer` per `Sound`).
//!
//! Threading: a `MediaPlayer` is bound to the thread that created it. We
//! attach the calling thread to the JavaVM and keep all JNI touches on it,
//! mirroring the `microphone` Android backend's attach pattern.
//!
//! **Compile-checked only** — typed against the `MediaPlayer` JNI surface
//! and compiles for the Android target, but not exercised on a device from
//! this repo. The threading + global-ref invariants below are documented so
//! the device bring-up has the expectations written down.

use std::path::PathBuf;

use jni::objects::{GlobalRef, JObject, JValue};
use jni::JavaVM;

use crate::{AudioError, AudioSource};

/// Map the host `JavaVM` pointer. Panics-free; a bad pointer is a bootstrap
/// bug surfaced as a [`AudioError::Backend`].
fn java_vm() -> Result<JavaVM, AudioError> {
    let ctx = ndk_context::android_context();
    let vm_ptr = ctx.vm() as *mut jni::sys::JavaVM;
    unsafe { JavaVM::from_raw(vm_ptr) }
        .map_err(|e| AudioError::Backend(format!("invalid JavaVM pointer: {e}")))
}

/// The app's cache dir (`Context.getCacheDir().getAbsolutePath()`), used to
/// stage a `Bytes` source as a temp file MediaPlayer can read.
fn cache_dir(vm: &JavaVM) -> Result<PathBuf, AudioError> {
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| AudioError::Backend(format!("JNI attach failed: {e}")))?;
    let ctx = ndk_context::android_context();
    let activity = unsafe { JObject::from_raw(ctx.context().cast()) };

    let file = env
        .call_method(&activity, "getCacheDir", "()Ljava/io/File;", &[])
        .and_then(|v| v.l())
        .map_err(|e| AudioError::Backend(format!("getCacheDir failed: {e}")))?;
    let path = env
        .call_method(&file, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .and_then(|v| v.l())
        .map_err(|e| AudioError::Backend(format!("getAbsolutePath failed: {e}")))?;
    let s: String = env
        .get_string(&path.into())
        .map_err(|e| AudioError::Backend(format!("cache path decode failed: {e}")))?
        .into();
    Ok(PathBuf::from(s))
}

/// What `play()` re-creates a `MediaPlayer` from: a local file path. A
/// `Bytes` source is staged to a temp file at `prepare` time; that temp file
/// is owned by the `PreparedSound` and removed on `Drop`.
pub(crate) struct PreparedSound {
    path: String,
    vm: JavaVM,
    /// `Some` when `path` points at a temp file we created (from `Bytes`)
    /// and must clean up on drop.
    temp_file: Option<PathBuf>,
}

impl Drop for PreparedSound {
    fn drop(&mut self) {
        if let Some(ref p) = self.temp_file {
            let _ = std::fs::remove_file(p);
        }
    }
}

impl PreparedSound {
    pub(crate) fn play(&self) -> PlaybackHandle {
        match build_player(&self.vm, &self.path) {
            Ok(player) => {
                // start() begins playback.
                let _ = with_player(&self.vm, &player, |env, p| {
                    env.call_method(p, "start", "()V", &[]).map(|_| ())
                });
                PlaybackHandle {
                    vm: clone_vm(&self.vm),
                    player: Some(player),
                }
            }
            Err(e) => {
                // prepare() validated the source, so a failure here is
                // unexpected; return an inert handle rather than panic.
                eprintln!("audio(android): play build failed: {e}");
                PlaybackHandle {
                    vm: clone_vm(&self.vm),
                    player: None,
                }
            }
        }
    }
}

/// A running playback owning a `MediaPlayer` global ref. `Drop` stops and
/// releases it. RAII stop.
pub(crate) struct PlaybackHandle {
    vm: JavaVM,
    player: Option<GlobalRef>,
}

impl Drop for PlaybackHandle {
    fn drop(&mut self) {
        if let Some(ref player) = self.player {
            let _ = with_player(&self.vm, player, |env, p| {
                let _ = env.call_method(p, "stop", "()V", &[]);
                env.call_method(p, "release", "()V", &[]).map(|_| ())
            });
        }
    }
}

impl PlaybackHandle {
    pub(crate) fn pause(&self) {
        self.call_void("pause");
    }

    pub(crate) fn resume(&self) {
        // MediaPlayer has no "resume" — start() after pause() resumes from
        // the paused position.
        self.call_void("start");
    }

    pub(crate) fn set_volume(&self, volume: f32) {
        if let Some(ref player) = self.player {
            let _ = with_player(&self.vm, player, |env, p| {
                env.call_method(
                    p,
                    "setVolume",
                    "(FF)V",
                    &[JValue::Float(volume), JValue::Float(volume)],
                )
                .map(|_| ())
            });
        }
    }

    pub(crate) fn set_looping(&self, looping: bool) {
        if let Some(ref player) = self.player {
            let _ = with_player(&self.vm, player, |env, p| {
                env.call_method(p, "setLooping", "(Z)V", &[JValue::Bool(looping as u8)])
                    .map(|_| ())
            });
        }
    }

    pub(crate) fn is_playing(&self) -> bool {
        match self.player {
            Some(ref player) => with_player(&self.vm, player, |env, p| {
                env.call_method(p, "isPlaying", "()Z", &[]).and_then(|v| v.z())
            })
            .unwrap_or(false),
            None => false,
        }
    }

    fn call_void(&self, method: &str) {
        if let Some(ref player) = self.player {
            let _ = with_player(&self.vm, player, |env, p| {
                env.call_method(p, method, "()V", &[]).map(|_| ())
            });
        }
    }
}

/// Re-derive a `JavaVM` handle (it's a thin pointer wrapper; cloning the
/// pointer is cheap and the VM outlives the process).
fn clone_vm(vm: &JavaVM) -> JavaVM {
    unsafe { JavaVM::from_raw(vm.get_java_vm_pointer()) }
        .expect("re-wrapping a valid JavaVM pointer cannot fail")
}

/// Attach the current thread, run `f` against the player object, detach via
/// the guard drop. Centralizes the attach + global-ref-deref boilerplate.
fn with_player<R>(
    vm: &JavaVM,
    player: &GlobalRef,
    f: impl FnOnce(&mut jni::JNIEnv, &JObject) -> Result<R, jni::errors::Error>,
) -> Result<R, AudioError> {
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| AudioError::Backend(format!("JNI attach failed: {e}")))?;
    let obj = player.as_obj();
    f(&mut env, obj).map_err(|e| AudioError::Backend(format!("JNI call failed: {e}")))
}

/// Build a `MediaPlayer`, set its data source to `path`, prepare it, and
/// return a global ref. The player is left prepared but not started.
fn build_player(vm: &JavaVM, path: &str) -> Result<GlobalRef, AudioError> {
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| AudioError::Backend(format!("JNI attach failed: {e}")))?;

    let player = env
        .new_object("android/media/MediaPlayer", "()V", &[])
        .map_err(|e| AudioError::Backend(format!("MediaPlayer construct failed: {e}")))?;

    let jpath = env
        .new_string(path)
        .map_err(|e| AudioError::Backend(format!("path encode failed: {e}")))?;
    env.call_method(
        &player,
        "setDataSource",
        "(Ljava/lang/String;)V",
        &[JValue::Object(&jpath)],
    )
    .map_err(|e| AudioError::Decode(format!("setDataSource failed: {e}")))?;

    // prepare() decodes/validates the source synchronously; a bad source
    // throws here → surfaced as Decode.
    env.call_method(&player, "prepare", "()V", &[])
        .map_err(|e| AudioError::Decode(format!("prepare failed: {e}")))?;

    env.new_global_ref(&player)
        .map_err(|e| AudioError::Backend(format!("global ref failed: {e}")))
}

/// Prepare a sound: resolve the source to a local file path MediaPlayer can
/// read (staging `Bytes` to a temp file), then validate by building one
/// player (and immediately releasing it).
pub(crate) async fn prepare(source: AudioSource) -> Result<PreparedSound, AudioError> {
    let vm = java_vm()?;

    let (path, temp_file) = match source {
        AudioSource::Path(p) => (p.to_string_lossy().into_owned(), None),
        // MediaPlayer.setDataSource(String) accepts an http(s) URL directly
        // (it streams), so pass remote URLs straight through.
        AudioSource::Url(u) => (u, None),
        AudioSource::Bytes(bytes) => {
            // Stage to a uniquely-named temp file under the app cache dir.
            let dir = cache_dir(&vm)?;
            let _ = std::fs::create_dir_all(&dir);
            let name = format!("idealyst_audio_{}.snd", next_temp_id());
            let file = dir.join(name);
            std::fs::write(&file, &bytes)
                .map_err(|e| AudioError::Backend(format!("temp write failed: {e}")))?;
            (file.to_string_lossy().into_owned(), Some(file))
        }
    };

    // Validate: build one player (which prepares + may throw Decode), then
    // release it. play() builds fresh players from the same path.
    match build_player(&vm, &path) {
        Ok(player) => {
            let _ = with_player(&vm, &player, |env, p| {
                env.call_method(p, "release", "()V", &[]).map(|_| ())
            });
        }
        Err(e) => {
            if let Some(ref f) = temp_file {
                let _ = std::fs::remove_file(f);
            }
            return Err(e);
        }
    }

    Ok(PreparedSound {
        path,
        vm,
        temp_file,
    })
}

/// Monotonic counter for unique temp-file names within the process.
fn next_temp_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}
