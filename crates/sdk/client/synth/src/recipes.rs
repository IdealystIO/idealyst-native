//! Compile-checked usage **recipes** for the synth SDK.
//!
//! Each `recipe!(Target, fn ...)` is a real, type-checked example of how to
//! use the SDK. Because the fn compiles against the live API, a signature
//! change that isn't reflected here is a compile error (whenever the catalog
//! is built), so these examples can't silently rot — and the MCP / docs
//! surface them as trustworthy "how do I use this?" context.
//!
//! `recipe!` self-gates on the `catalog` feature: with it off (every
//! production build) these expand to nothing — the recipes, and the imports
//! inside them, don't compile at all. So there's no `#[cfg]` here and no
//! cost in shipped apps. Recipes are self-contained (imports live inside
//! each fn) so the captured source reads as a complete, copy-pasteable
//! example.

use runtime_core::recipe;

recipe!(
    Pcm,
    /// A button that fires a synthesized laser. The sound is rendered once
    /// (pure Rust, deterministic), encoded to WAV and loaded as an
    /// `audio::Sound` up front via `spawn_async`; each press starts a fresh
    /// `Playback`. The active `Playback` lives in a `RefCell` slot — RAII, no
    /// `mem::forget` — so the sound isn't cut off when the press closure
    /// returns.
    pub fn laser_on_press() -> ::runtime_core::Element {
        use ::runtime_core::driver::spawn_async;
        use ::runtime_core::ui;
        use ::std::cell::RefCell;
        use ::std::rc::Rc;
        use crate::{sweep, Envelope, Sweep, Waveform, DEFAULT_FORMAT};

        // Render the effect: a square wave diving 1.2 kHz -> 180 Hz, shaped
        // by an ADSR envelope. (`crate::presets::laser()` is this same sound.)
        let laser = sweep(Waveform::Square, 1200.0, 180.0, 0.25, Sweep::Exponential, DEFAULT_FORMAT)
            .apply_envelope(Envelope::new(0.003, 0.05, 0.5, 0.12))
            .gain(0.45);

        // Load once (async: WAV -> the platform player).
        let sound: Rc<RefCell<Option<::audio::Sound>>> = Rc::new(RefCell::new(None));
        let ready = sound.clone();
        spawn_async(async move {
            if let Ok(s) = laser.load().await {
                *ready.borrow_mut() = Some(s);
            }
        });

        let active: Rc<RefCell<Option<::audio::Playback>>> = Rc::new(RefCell::new(None));
        let on_click = move || {
            if let Some(s) = sound.borrow().as_ref() {
                *active.borrow_mut() = Some(s.play());
            }
        };

        ui! {
            button(label = "Fire", on_click = on_click)
        }
    }
);

recipe!(
    Mixer,
    /// Record a game's synthesized audio to a file. A `Mixer` is a live,
    /// real-time-paced `AudioStream` (silence between effects, so the
    /// recording's timeline is continuous); `media-writer` records it like
    /// any microphone stream. "Fire" mixes a laser in — fire-and-forget, the
    /// returned `Voice` can be dropped — and "Stop" finalizes the file.
    pub fn record_game_audio() -> ::runtime_core::Element {
        use ::media_writer::{MediaInputs, MediaWriter, RecordConfig, Recording};
        use ::runtime_core::driver::spawn_async;
        use ::runtime_core::ui;
        use ::std::cell::RefCell;
        use ::std::rc::Rc;
        use crate::{presets, AudioFormat, Mixer};

        let mixer = Mixer::new(AudioFormat { sample_rate: 48_000, channels: 2 });

        // Start recording the mixer's stream as soon as the screen mounts.
        let recording: Rc<RefCell<Option<Recording>>> = Rc::new(RefCell::new(None));
        let slot = recording.clone();
        let stream = mixer.stream();
        spawn_async(async move {
            let Ok(store) = ::files::app_files("recordings") else { return };
            let config = RecordConfig::new(store, "game-audio.mp4");
            if let Ok(rec) = MediaWriter::new().record(MediaInputs::audio(&stream), config).await {
                *slot.borrow_mut() = Some(rec);
            }
        });

        let laser = presets::laser();
        let fire = move || {
            mixer.play(&laser);
        };
        let stop = move || {
            if let Some(rec) = recording.borrow_mut().take() {
                spawn_async(async move {
                    let _path = rec.stop().await;
                });
            }
        };

        ui! {
            view() {
                button(label = "Fire", on_click = fire)
                button(label = "Stop recording", on_click = stop)
            }
        }
    }
);
