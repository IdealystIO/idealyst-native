//! Compile-checked usage **recipes** for the audio engine.
//!
//! Each `recipe!(Target, fn ...)` is a real, type-checked example. `recipe!`
//! self-gates on the `catalog` feature: with it off (every production build)
//! these expand to nothing. Recipes are self-contained (imports inside each
//! fn) so the captured source reads as a complete, copy-pasteable example.

use runtime_core::recipe;

recipe!(
    AudioEngine,
    /// Rapid-fire gunshots that layer instead of cutting each other off.
    /// The engine starts once (async) and the shot is rendered once into a
    /// shared `SoundBuffer`; every press starts a new fire-and-forget voice
    /// with a little random pan and pitch so repeats don't sound robotic.
    /// `resume()` in the handler unlocks audio on the web (browsers start it
    /// suspended until a gesture).
    pub fn gunfire() -> ::runtime_core::Element {
        use crate::{AudioEngine, SoundBuffer, VoiceParams};
        use ::runtime_core::driver::spawn_async;
        use ::runtime_core::ui;
        use ::std::cell::{Cell, RefCell};
        use ::std::rc::Rc;

        let shot = SoundBuffer::from_pcm(&::synth::presets::hit());
        let engine: Rc<RefCell<Option<AudioEngine>>> = Rc::new(RefCell::new(None));
        let slot = engine.clone();
        spawn_async(async move {
            if let Ok(e) = AudioEngine::start().await {
                *slot.borrow_mut() = Some(e);
            }
        });

        let shots = Cell::new(0u32);
        let fire = move || {
            let Some(e) = engine.borrow().clone() else {
                return;
            };
            e.resume();
            // Cheap deterministic variation: ±0.2 pan, ±6% pitch.
            let n = shots.get().wrapping_mul(2_654_435_761);
            shots.set(shots.get() + 1);
            let jitter = (n >> 16) as f32 / 65_536.0 - 0.5;
            e.play(
                &shot,
                VoiceParams::new()
                    .gain(0.7)
                    .pan(jitter * 0.4)
                    .rate(1.0 + jitter * 0.12),
            );
        };

        ui! {
            button(label = "Fire", on_click = fire)
        }
    }
);

recipe!(
    Voice,
    /// A looping engine/wake sound steered live: the speed slider opens the
    /// low-pass and raises the pitch while it plays, each change ramped over
    /// 100 ms so it glides instead of stepping. The loop starts when the
    /// engine is up and is stopped (faded) by "Stop".
    pub fn wake_follows_speed() -> ::runtime_core::Element {
        use crate::{AudioEngine, SoundBuffer, Voice, VoiceParams};
        use ::runtime_core::driver::spawn_async;
        use ::runtime_core::ui;
        use ::std::cell::RefCell;
        use ::std::rc::Rc;

        // Brown noise reads as water / engine rumble.
        let wake = SoundBuffer::from_pcm(
            &::synth::noise(::synth::Noise::Brown, 2.0, ::synth::DEFAULT_FORMAT, 11).normalize(0.8),
        );
        let voice: Rc<RefCell<Option<(AudioEngine, Voice)>>> = Rc::new(RefCell::new(None));
        let slot = voice.clone();
        spawn_async(async move {
            if let Ok(e) = AudioEngine::start().await {
                let v = e.play(
                    &wake,
                    VoiceParams::new().looping(true).lowpass(300.0).gain(0.6),
                );
                *slot.borrow_mut() = Some((e, v));
            }
        });

        let steer = voice.clone();
        let on_speed = move |speed: f32| {
            if let Some((e, v)) = steer.borrow().as_ref() {
                e.resume();
                v.set_lowpass(Some(300.0 + speed * 6_000.0), 0.1);
                v.set_rate(0.8 + speed * 0.5, 0.1);
            }
        };
        let stop = move || {
            if let Some((_, v)) = voice.borrow_mut().take() {
                v.stop(0.4);
            }
        };

        ui! {
            view() {
                slider(value = 0.0f32, on_change = on_speed, min = 0.0, max = 1.0)
                button(label = "Stop", on_click = stop)
            }
        }
    }
);

recipe!(
    spatial,
    /// A positional one-shot: an explosion somewhere in the world, heard from
    /// the listener's position. `VoiceParams::spatial` turns the geometry into
    /// gain (distance), pan (left/right) and low-pass (far = muffled).
    pub fn spatial_explosion() -> ::runtime_core::Element {
        use crate::spatial::{Attenuation, Listener};
        use crate::{AudioEngine, SoundBuffer, VoiceParams};
        use ::runtime_core::driver::spawn_async;
        use ::runtime_core::ui;
        use ::std::cell::RefCell;
        use ::std::rc::Rc;

        let boom = SoundBuffer::from_pcm(&::synth::presets::explosion());
        let engine: Rc<RefCell<Option<AudioEngine>>> = Rc::new(RefCell::new(None));
        let slot = engine.clone();
        spawn_async(async move {
            if let Ok(e) = AudioEngine::start().await {
                *slot.borrow_mut() = Some(e);
            }
        });

        let near_right = {
            let engine = engine.clone();
            let boom = boom.clone();
            move || {
                if let Some(e) = engine.borrow().as_ref() {
                    e.resume();
                    let p = VoiceParams::spatial(
                        &Listener::default(),
                        [4.0, 0.0, -2.0],
                        &Attenuation::default(),
                    );
                    e.play(&boom, p);
                }
            }
        };
        let far_left = move || {
            if let Some(e) = engine.borrow().as_ref() {
                e.resume();
                let p = VoiceParams::spatial(
                    &Listener::default(),
                    [-30.0, 0.0, -25.0],
                    &Attenuation::default(),
                );
                e.play(&boom, p);
            }
        };

        ui! {
            view() {
                button(label = "Explosion: near right", on_click = near_right)
                button(label = "Explosion: far left", on_click = far_left)
            }
        }
    }
);

recipe!(
    play_stream,
    /// Play a live `AudioStream` — here a `synth::Mixer` bus — through the
    /// engine, with the engine's own gain / filter on top. The same call
    /// plays a microphone stream (`microphone::open_stream`). The stream
    /// voice runs until stopped; silence plays while the stream is idle.
    pub fn play_synth_mixer() -> ::runtime_core::Element {
        use crate::{AudioEngine, VoiceParams};
        use ::runtime_core::driver::spawn_async;
        use ::runtime_core::ui;
        use ::std::cell::RefCell;
        use ::std::rc::Rc;

        let bus = ::synth::Mixer::new(::synth::AudioFormat {
            sample_rate: 48_000,
            channels: 2,
        });
        let stream = bus.stream();
        let engine: Rc<RefCell<Option<AudioEngine>>> = Rc::new(RefCell::new(None));
        let slot = engine.clone();
        spawn_async(async move {
            if let Ok(e) = AudioEngine::start().await {
                // A muffled, slightly-left bus.
                e.play_stream(
                    &stream,
                    VoiceParams::new().gain(0.8).pan(-0.2).lowpass(5_000.0),
                );
                *slot.borrow_mut() = Some(e);
            }
        });

        let coin = ::synth::presets::coin();
        let ding = move || {
            if let Some(e) = engine.borrow().as_ref() {
                e.resume();
            }
            bus.play(&coin);
        };

        ui! {
            button(label = "Coin", on_click = ding)
        }
    }
);
