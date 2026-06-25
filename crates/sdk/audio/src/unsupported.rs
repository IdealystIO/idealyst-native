//! Desktop (Windows / Linux / other native) fallback.
//!
//! No pure-Rust audio player is pulled in here on purpose — the SDK stays
//! dependency-light, and a half-working desktop path would be worse than an
//! honest one. So [`prepare`] always returns [`AudioError::NotSupported`].
//!
//! The [`PreparedSound`] / [`PlaybackHandle`] types still exist (the public
//! `Sound` / `Playback` wrappers name them) but are uninhabited-by-use:
//! `prepare` never returns `Ok`, so neither is ever constructed at runtime.
//! Adding a real desktop backend later (e.g. `rodio`/`cpal`) means filling
//! these in and is a drop-in — the public API doesn't change.

use crate::{AudioError, AudioSource};

/// A prepared sound. Never constructed on this target ([`prepare`] always
/// errors); present only so the cross-platform `Sound` wrapper has a type
/// to name.
pub(crate) struct PreparedSound {
    // Uninhabited-by-use: `prepare` never returns `Ok`. The field keeps the
    // type from being a unit (so a stray construction would be a compile
    // error pointing here, not a silent no-op handle).
    _never: std::convert::Infallible,
}

impl PreparedSound {
    pub(crate) fn play(&self) -> PlaybackHandle {
        // Unreachable: a `PreparedSound` can't be constructed on this
        // target. Matching `_never` proves it to the compiler.
        match self._never {}
    }
}

/// A running playback. Never constructed on this target — see
/// [`PreparedSound`].
pub(crate) struct PlaybackHandle {
    _never: std::convert::Infallible,
}

impl PlaybackHandle {
    pub(crate) fn pause(&self) {
        match self._never {}
    }
    pub(crate) fn resume(&self) {
        match self._never {}
    }
    pub(crate) fn set_volume(&self, _volume: f32) {
        match self._never {}
    }
    pub(crate) fn set_looping(&self, _looping: bool) {
        match self._never {}
    }
    pub(crate) fn is_playing(&self) -> bool {
        match self._never {}
    }
}

/// Always [`AudioError::NotSupported`] on desktop targets without a player.
pub(crate) async fn prepare(_source: AudioSource) -> Result<PreparedSound, AudioError> {
    Err(AudioError::NotSupported)
}
