//! Apple (iOS / macOS / tvOS) playback via `AVAudioPlayer`.
//!
//! `AVAudioPlayer` is AVFoundation's simple file/data player — the right
//! tool for "play a loaded sound": init it from in-memory data
//! (`initWithData:error:`) or a file/URL (`initWithContentsOfURL:error:`),
//! then `prepareToPlay` / `play` / `pause` / `stop`, with `volume` and
//! `numberOfLoops` for our volume + looping controls and `isPlaying` for the
//! status query.
//!
//! Each [`Sound::play`](crate::Sound::play) builds a *fresh* `AVAudioPlayer`
//! from the prepared source, so overlapping voices from one `Sound` work —
//! AVAudioPlayer is single-voice per instance, but multiple instances mix.
//!
//! **Compile-checked only.** The objc2 message sends are typed against the
//! AVFoundation surface and this compiles for the Apple targets, but the
//! playback path has not been exercised on a device/simulator from this
//! repo. The invariants below (data must be copied/retained, init can fail
//! → `Decode`) are the AVFoundation contract, documented so the device
//! bring-up has the expectations written down.

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::{msg_send, msg_send_id};
use objc2_foundation::{NSObject, NSString};

use crate::{AudioError, AudioSource};

/// Resolve an Obj-C class by name at runtime, returning `None` if it isn't
/// registered. Unlike `class!(...)` (which panics on a missing class), this
/// lets a host where AVFoundation isn't linked — e.g. the `cargo test`
/// binary on a CI macOS box — surface a typed error instead of aborting the
/// process. On a real app the frameworks are linked (the CLI adds
/// `AVFoundation`), so this always resolves.
fn class_named(name: &str) -> Option<&'static AnyClass> {
    AnyClass::get(name)
}

/// `numberOfLoops` value meaning "loop forever" per the AVAudioPlayer docs
/// (any negative number loops indefinitely).
const LOOP_FOREVER: isize = -1;
/// `numberOfLoops` value meaning "play once" (0 additional loops).
const PLAY_ONCE: isize = 0;

/// How a prepared sound recreates its player. We keep the *source*, not a
/// live player, so each `play()` mints an independent `AVAudioPlayer` (one
/// voice per instance; instances mix for concurrent playback).
enum Prepared {
    /// Encoded audio held as an `NSData` (from a `Bytes` source). Retained
    /// for the `Sound`'s lifetime so every `play()` can re-init from it.
    Data(Retained<NSObject>),
    /// A file path / URL string; each `play()` builds an `NSURL` and inits
    /// from contents.
    Url(String),
}

/// A prepared sound. Holds the source so [`play`](PreparedSound::play) can
/// build a fresh player per call.
pub(crate) struct PreparedSound {
    prepared: Prepared,
}

impl PreparedSound {
    pub(crate) fn play(&self) -> PlaybackHandle {
        let player = unsafe { build_player(&self.prepared) };
        if let Some(ref p) = player {
            unsafe {
                let _: Bool = msg_send![&**p, prepareToPlay];
                let _: Bool = msg_send![&**p, play];
            }
        }
        // `player` is `None` only if init failed *here* (it succeeded in
        // `prepare`, so this is unexpected) — the handle then no-ops, never
        // panics.
        PlaybackHandle { player }
    }
}

/// A running playback owning one `AVAudioPlayer`. `Drop` stops it; the
/// `Retained` releases the player. RAII stop.
pub(crate) struct PlaybackHandle {
    player: Option<Retained<NSObject>>,
}

impl Drop for PlaybackHandle {
    fn drop(&mut self) {
        if let Some(ref p) = self.player {
            unsafe {
                let _: () = msg_send![&**p, stop];
            }
        }
    }
}

impl PlaybackHandle {
    pub(crate) fn pause(&self) {
        if let Some(ref p) = self.player {
            unsafe {
                let _: () = msg_send![&**p, pause];
            }
        }
    }

    pub(crate) fn resume(&self) {
        if let Some(ref p) = self.player {
            unsafe {
                let _: Bool = msg_send![&**p, play];
            }
        }
    }

    pub(crate) fn set_volume(&self, volume: f32) {
        if let Some(ref p) = self.player {
            unsafe {
                // -[AVAudioPlayer setVolume:] takes a float.
                let _: () = msg_send![&**p, setVolume: volume];
            }
        }
    }

    pub(crate) fn set_looping(&self, looping: bool) {
        if let Some(ref p) = self.player {
            let loops = if looping { LOOP_FOREVER } else { PLAY_ONCE };
            unsafe {
                let _: () = msg_send![&**p, setNumberOfLoops: loops];
            }
        }
    }

    pub(crate) fn is_playing(&self) -> bool {
        match self.player {
            Some(ref p) => unsafe {
                let playing: Bool = msg_send![&**p, isPlaying];
                playing.as_bool()
            },
            None => false,
        }
    }
}

/// Build an `AVAudioPlayer` from the prepared source. `None` if init fails.
///
/// AVFoundation copies/retains the `NSData` it inits from, but we hold the
/// `Retained<NSData>` in `Prepared::Data` for the `Sound`'s lifetime anyway
/// so re-`play()` after the first player drops still has the bytes.
unsafe fn build_player(prepared: &Prepared) -> Option<Retained<NSObject>> {
    // Resolve AVAudioPlayer dynamically (panic-free): absent only where
    // AVFoundation isn't linked, which the public `prepare` maps to
    // `NotSupported`.
    let player_cls = class_named("AVAudioPlayer")?;
    let allocated: *mut AnyObject = msg_send![player_cls, alloc];
    if allocated.is_null() {
        return None;
    }
    let inited: *mut AnyObject = match prepared {
        Prepared::Data(data) => {
            // initWithData:error: — error out-param left null (we treat a
            // nil return as failure, which is sufficient for our `Decode`).
            msg_send![
                allocated,
                initWithData: &**data,
                error: std::ptr::null_mut::<*mut AnyObject>()
            ]
        }
        Prepared::Url(s) => {
            let ns = NSString::from_str(s);
            let url_cls = class_named("NSURL")?;
            // fileURLWithPath: covers local paths; for a remote http(s) URL
            // a caller should pre-fetch (AVAudioPlayer does not stream). We
            // build a file URL here — the documented scope is local sounds.
            let url: Retained<NSObject> =
                msg_send_id![url_cls, fileURLWithPath: &*ns];
            msg_send![
                allocated,
                initWithContentsOfURL: &*url,
                error: std::ptr::null_mut::<*mut AnyObject>()
            ]
        }
    };
    if inited.is_null() {
        return None;
    }
    Retained::from_raw(inited.cast::<NSObject>())
}

/// Prepare a sound: retain the bytes as `NSData`, or keep the path/URL
/// string. We also do a one-shot init here to surface a [`Decode`] error
/// eagerly if the data can't be decoded, matching the cross-platform
/// contract that `load` validates the audio.
pub(crate) async fn prepare(source: AudioSource) -> Result<PreparedSound, AudioError> {
    // If AVFoundation isn't linked (e.g. a CI host running these tests with
    // no audio framework), report `NotSupported` rather than panic — and
    // before touching any audio class.
    if class_named("AVAudioPlayer").is_none() {
        return Err(AudioError::NotSupported);
    }

    let prepared = match source {
        AudioSource::Bytes(bytes) => {
            let data_cls = class_named("NSData").ok_or(AudioError::NotSupported)?;
            // Copy bytes into an NSData the player can re-init from. The
            // class copies, but we retain so later `play()`s still have it.
            let data: Retained<NSObject> = unsafe {
                msg_send_id![
                    data_cls,
                    dataWithBytes: bytes.as_ptr() as *const std::ffi::c_void,
                    length: bytes.len()
                ]
            };
            Prepared::Data(data)
        }
        AudioSource::Path(path) => Prepared::Url(path.to_string_lossy().into_owned()),
        // AVAudioPlayer does not stream remote URLs; a remote source must be
        // fetched first. We accept the string and treat it as a (file) URL,
        // so a path-style http(s) URL yields a player that fails to init →
        // `Decode`, which is the honest signal for "not a local sound".
        AudioSource::Url(url) => Prepared::Url(url),
    };

    // Eagerly validate by building one player; a nil result means the audio
    // couldn't be decoded / the URL didn't resolve (the framework is present
    // — we checked above).
    let valid = unsafe { build_player(&prepared) };
    if valid.is_none() {
        return Err(AudioError::Decode(
            "AVAudioPlayer could not decode the source".to_string(),
        ));
    }
    drop(valid); // The validation player is discarded; play() makes fresh ones.

    Ok(PreparedSound { prepared })
}
