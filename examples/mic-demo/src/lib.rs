//! `mic-demo` — exercises the `microphone` SDK end to end, and (by merely
//! depending on it) the capability-based permission injection.
//!
//! Press **Start microphone** → the SDK opens a live input stream (the OS
//! prompts for permission the first time). Raw f32 frames arrive in the
//! `on_audio` callback. The demo bridges that callback to reactive UI state
//! the canonical way — the exact job the mic SDK leaves to a higher layer:
//!
//! 1. The callback runs on the audio thread (native) or the main thread
//!    (web). It can't touch the reactive runtime directly off-thread, so it
//!    writes the current peak amplitude + sample rate into lock-free
//!    [`std::sync::atomic`] globals.
//! 2. A [`raf_loop`](runtime_core::raf_loop) on the main thread reads those
//!    atomics once per frame and folds them into `Signal`s the meter binds
//!    to (writing only on change, so an idle/steady meter doesn't re-render
//!    every frame).
//!
//! That's the whole pattern: raw callback → atomic hand-off → main-thread
//! signal. A future "audio state" SDK would package exactly this.

use std::sync::atomic::{AtomicU32, Ordering};

use idea_ui::{install_idea_theme, light_theme, Stack, StackGap, StackPadding, Typography};
use microphone::{AudioBuffer, AudioStreamConfig, MicError, Microphone};
use runtime_core::{signal, text, ui, Element, IntoElement, Signal};

// Audio→UI bridge. The capture callback writes here; the main-thread
// `raf_loop` reads. f32s are stored as their bit pattern (`AtomicF32` isn't
// in std). A single global pair is fine for a one-stream demo.
static PEAK_BITS: AtomicU32 = AtomicU32::new(0);
static SAMPLE_RATE: AtomicU32 = AtomicU32::new(0);

/// No third-party `Element::External` SDKs to register — `microphone` is a
/// plain capability crate, not a rendered primitive, so this stays empty.
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(_backend: &mut dev_server::WireRecordingBackend) {}

pub fn app() -> Element {
    install_idea_theme(light_theme());

    let level: Signal<f32> = signal!(0.0);
    let rate: Signal<u32> = signal!(0);
    let status: Signal<String> = signal!("Idle — press Start microphone".to_string());
    let started: Signal<bool> = signal!(false);

    // Pump the atomic bridge into the signals once per frame. The change
    // guards keep an idle (silent) or steady meter from re-rendering every
    // frame — the signal only fires when the value actually moves.
    {
        let last_peak = std::cell::Cell::new(u32::MAX);
        let last_rate = std::cell::Cell::new(u32::MAX);
        let raf = runtime_core::raf_loop(move || {
            let p = PEAK_BITS.load(Ordering::Relaxed);
            if p != last_peak.get() {
                last_peak.set(p);
                level.set(f32::from_bits(p));
            }
            let r = SAMPLE_RATE.load(Ordering::Relaxed);
            if r != last_rate.get() {
                last_rate.set(r);
                rate.set(r);
            }
        });
        // Page-lifetime loop; keep the handle alive for the app's duration.
        std::mem::forget(raf);
    }

    // Reactive level meter — a 24-cell ASCII bar + percentage.
    let meter = text(move || {
        let l = level.get().clamp(0.0, 1.0);
        let filled = (l * 24.0).round() as usize;
        format!(
            "[{}{}] {:>3.0}%",
            "#".repeat(filled),
            "-".repeat(24 - filled),
            l * 100.0
        )
    })
    .into_element();

    let status_text = text(move || status.get()).into_element();
    let rate_text = text(move || {
        let r = rate.get();
        if r == 0 {
            "Sample rate: —".to_string()
        } else {
            format!("Sample rate: {r} Hz")
        }
    })
    .into_element();

    // Opening is user-gesture–gated: web requires getUserMedia + AudioContext
    // to start from a user interaction, and it's the right UX everywhere.
    let on_start = move || {
        if started.get() {
            return;
        }
        started.set(true);
        status.set("Requesting microphone…".to_string());
        runtime_core::driver::spawn_async(async move {
            let mic = Microphone::new();
            let result = mic
                .open(AudioStreamConfig::default().mono(), |buf: &AudioBuffer| {
                    let mut peak = 0.0f32;
                    for &s in buf.samples {
                        let a = s.abs();
                        if a > peak {
                            peak = a;
                        }
                    }
                    PEAK_BITS.store(peak.to_bits(), Ordering::Relaxed);
                    SAMPLE_RATE.store(buf.sample_rate, Ordering::Relaxed);
                })
                .await;
            match result {
                Ok(stream) => {
                    status.set("Listening — speak into the mic".to_string());
                    // The demo has no Stop button; keep capturing for the
                    // app's lifetime. Leaking the stream is the simplest way
                    // to pin it (and avoids moving the !Send native stream).
                    std::mem::forget(stream);
                }
                Err(e) => {
                    started.set(false);
                    status.set(match e {
                        MicError::PermissionDenied => {
                            "Microphone permission denied".to_string()
                        }
                        MicError::NoInputDevice => "No microphone found".to_string(),
                        other => format!("Error: {other}"),
                    });
                }
            }
        });
    };

    let body: Vec<Element> = vec![
        ui! { Typography(content = "Microphone SDK demo".to_string(), kind = idea_ui::typography_kind::H1) },
        ui! {
            Typography(
                content = "Live input level captured straight from the platform's \
                    microphone via the `microphone` SDK. Press Start, then make \
                    some noise."
                    .to_string(),
                muted = true,
            )
        },
        status_text,
        meter,
        rate_text,
        ui! { button(label = "Start microphone".to_string(), on_click = on_start) },
    ];

    ui! {
        Stack(gap = StackGap::Md, padding = StackPadding::Lg) { body }
    }
}
