//! `denoise-demo` — the `denoise` SDK end to end, with an A/B recording.
//!
//! 1. **Start** opens the `microphone` SDK's live [`AudioStream`](denoise::AudioStream)
//!    and feeds it through [`Denoiser::process`], which returns a second
//!    (48 kHz mono) stream of the DeepFilterNet-enhanced signal. Two live level
//!    meters show the **raw** input vs. the **denoised** output peak.
//! 2. **Record** captures BOTH streams to two `.m4a` files via the
//!    `media-writer` SDK (`raw.m4a` + `denoised.m4a`).
//! 3. **Stop** finalizes the files and mounts two players (the `video` SDK's
//!    AVPlayer on macOS — audio-only files, but the transport bar plays them) so
//!    you can A/B the original against the cleaned version.
//!
//! The audio→UI meter bridge is the canonical one (see `mic-demo`):
//! capture/processing callbacks run off the main thread, so they write peaks
//! into lock-free [`std::sync::atomic`] globals; a main-thread
//! [`raf_loop`](runtime_core::raf_loop) folds those into `Signal`s.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};

use denoise::{AudioStream, Denoiser};
use idea_ui::{install_idea_theme, light_theme, Stack, StackGap, StackPadding, Typography};
use media_writer::{MediaInputs, MediaWriter, RecordConfig, Recording};
use microphone::{AudioStreamConfig, MicError, Microphone};
use runtime_core::{
    signal, text, ui, when, Element, IntoElement, Length, Signal, StyleRules, StyleSheet,
};

// Where the two recordings live (./Library/Application Support/denoise-recordings/).
const STORE: &str = "denoise-recordings";
const RAW_FILE: &str = "raw.m4a";
const DENOISED_FILE: &str = "denoised.m4a";

// Audio→UI bridge: capture/processing callbacks write peaks here; the
// main-thread `raf_loop` reads. f32 peaks are stored as bit patterns.
static RAW_PEAK_BITS: AtomicU32 = AtomicU32::new(0);
static DENOISED_PEAK_BITS: AtomicU32 = AtomicU32::new(0);

fn peak_of(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |m, &s| m.max(s.abs()))
}

/// The live pipeline, retained for the app's lifetime once Start runs. Holding
/// the `AudioStream`s keeps the mic + denoise worker alive; the subscriptions
/// keep the meters (and let `media-writer` tap the same channels) firing.
struct Live {
    raw: AudioStream,
    clean: AudioStream,
    _subs: Vec<Box<dyn std::any::Any>>,
}

/// Build the denoiser, sourcing the DeepFilterNet model per platform.
///
/// - **native**: the model is embedded in the binary — `Denoiser::new()` is
///   instant, no chunk.
/// - **web**: the 7.6 MB model isn't in the main wasm bundle. It lives in a
///   lazily-loaded `wasm-split` chunk (see [`web_model`]); the first call here
///   fetches that chunk, then hands the bytes to `Denoiser::with_weights`.
#[cfg(not(target_arch = "wasm32"))]
async fn make_denoiser() -> Result<Denoiser, String> {
    Ok(Denoiser::new())
}

#[cfg(target_arch = "wasm32")]
async fn make_denoiser() -> Result<Denoiser, String> {
    let bytes = web_model::load().await?;
    Ok(Denoiser::with_weights(bytes))
}

/// Web-only: the DeepFilterNet model, dynamically linked via `wasm-split`.
///
/// `model_bytes` is a `lazy_loader!` boundary, so the splitter hoists it — and
/// the `include_bytes!` data segment it (exclusively) references — out of the
/// main bundle and into a separate `chunk_N_split.wasm`. The 7.6 MB therefore
/// downloads only when [`load`] is first awaited (first denoiser construction),
/// not at page load. Awaiting `LOADER.load()` fetches + links the chunk; `.call`
/// then returns the now-resident `&'static [u8]`.
#[cfg(target_arch = "wasm32")]
mod web_model {
    fn model_bytes(_: ()) -> &'static [u8] {
        include_bytes!("../assets/DeepFilterNet3_onnx.tar.gz")
    }

    static LOADER: wasm_split::LazyLoader<(), &'static [u8]> =
        wasm_split::lazy_loader!(extern "auto" fn model_bytes(args: ()) -> &'static [u8]);

    pub async fn load() -> Result<&'static [u8], String> {
        if !LOADER.load().await {
            return Err("failed to load the denoise model chunk".into());
        }
        LOADER.call(()).map_err(|e| e.to_string())
    }
}

/// No `Element::External` SDKs to register by hand — `video` self-registers via
/// inventory at backend construction; `microphone`/`denoise`/`media-writer` are
/// plain capability/utility crates.
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(_backend: &mut dev_server::WireRecordingBackend) {}

pub fn app() -> Element {
    install_idea_theme(light_theme());

    let raw_level: Signal<f32> = signal!(0.0);
    let denoised_level: Signal<f32> = signal!(0.0);
    let status: Signal<String> = signal!("Idle — press Start".to_string());
    let started: Signal<bool> = signal!(false);
    let recording: Signal<bool> = signal!(false);
    // Empty until a recording is finalized; the players mount when these fill.
    let raw_url: Signal<String> = signal!(String::new());
    let denoised_url: Signal<String> = signal!(String::new());

    // Live pipeline + active recordings, shared between the button handlers.
    // `!Send`, main-thread only — `Recording`/`AudioStream` are not `Send`.
    let live: Rc<RefCell<Option<Live>>> = Rc::new(RefCell::new(None));
    let recs: Rc<RefCell<Option<(Recording, Recording)>>> = Rc::new(RefCell::new(None));

    // Pump the atomic bridge into the signals once per frame (change-guarded).
    {
        let last_raw = std::cell::Cell::new(u32::MAX);
        let last_den = std::cell::Cell::new(u32::MAX);
        let raf = runtime_core::raf_loop(move || {
            let r = RAW_PEAK_BITS.load(Ordering::Relaxed);
            if r != last_raw.get() {
                last_raw.set(r);
                raw_level.set(f32::from_bits(r));
            }
            let d = DENOISED_PEAK_BITS.load(Ordering::Relaxed);
            if d != last_den.get() {
                last_den.set(d);
                denoised_level.set(f32::from_bits(d));
            }
        });
        std::mem::forget(raf); // page-lifetime loop
    }

    let bar = |level: Signal<f32>, label: &'static str| {
        text(move || {
            let l = level.get().clamp(0.0, 1.0);
            let filled = (l * 24.0).round() as usize;
            format!(
                "{label:<9} [{}{}] {:>3.0}%",
                "#".repeat(filled),
                "-".repeat(24 - filled),
                l * 100.0
            )
        })
        .into_element()
    };

    // ---- Start: open mic, denoise, wire meters -------------------------------
    let on_start = {
        let live = live.clone();
        move || {
            if started.get() {
                return;
            }
            started.set(true);
            status.set("Requesting microphone…".to_string());
            let live = live.clone();
            runtime_core::driver::spawn_async(async move {
                let mic = Microphone::new();
                let raw = match mic.open_stream(AudioStreamConfig::default().mono()).await {
                    Ok(s) => s,
                    Err(e) => {
                        started.set(false);
                        status.set(match e {
                            MicError::PermissionDenied => "Microphone permission denied".into(),
                            MicError::NoInputDevice => "No microphone found".into(),
                            other => format!("Mic error: {other}"),
                        });
                        return;
                    }
                };

                let raw_sub = raw.subscribe(|f| {
                    RAW_PEAK_BITS.store(peak_of(f.samples).to_bits(), Ordering::Relaxed);
                });

                // On web this lazily fetches the model chunk (~7.6 MB) the
                // first time; on native it's instant (embedded model).
                status.set("Loading denoiser model…".to_string());
                let denoiser = match make_denoiser().await {
                    Ok(d) => d,
                    Err(e) => {
                        started.set(false);
                        status.set(format!("Model load error: {e}"));
                        return;
                    }
                };
                let clean = match denoiser.process(&raw).await {
                    Ok(c) => c,
                    Err(e) => {
                        started.set(false);
                        status.set(format!("Denoiser error: {e}"));
                        return;
                    }
                };
                let clean_sub = clean.subscribe(|f| {
                    DENOISED_PEAK_BITS.store(peak_of(f.samples).to_bits(), Ordering::Relaxed);
                });

                status.set("Listening — press Record to capture an A/B clip".to_string());
                *live.borrow_mut() = Some(Live {
                    raw,
                    clean,
                    _subs: vec![Box::new(raw_sub), Box::new(clean_sub)],
                });
            });
        }
    };

    // ---- Record: capture both streams to files -------------------------------
    let on_record = {
        let live = live.clone();
        let recs = recs.clone();
        move || {
            if !started.get() || recording.get() {
                return;
            }
            // Clone the stream handles out before awaiting so we don't hold the
            // `RefCell` borrow across an await point.
            let streams = live
                .borrow()
                .as_ref()
                .map(|l| (l.raw.clone(), l.clean.clone()));
            let Some((raw, clean)) = streams else {
                return;
            };
            recording.set(true);
            raw_url.set(String::new());
            denoised_url.set(String::new());
            status.set("Recording…".to_string());
            let recs = recs.clone();
            runtime_core::driver::spawn_async(async move {
                let store = match files::app_files(STORE) {
                    Ok(s) => s,
                    Err(e) => {
                        recording.set(false);
                        status.set(format!("Storage error: {e}"));
                        return;
                    }
                };
                let writer = MediaWriter::new();
                let raw_rec = writer
                    .record(MediaInputs::audio(&raw), RecordConfig::new(store.clone(), RAW_FILE))
                    .await;
                let clean_rec = writer
                    .record(MediaInputs::audio(&clean), RecordConfig::new(store, DENOISED_FILE))
                    .await;
                match (raw_rec, clean_rec) {
                    (Ok(a), Ok(b)) => {
                        *recs.borrow_mut() = Some((a, b));
                        status.set("Recording — press Stop to finish".to_string());
                    }
                    (a, b) => {
                        recording.set(false);
                        let err = a.err().map(|e| e.to_string())
                            .or_else(|| b.err().map(|e| e.to_string()))
                            .unwrap_or_default();
                        status.set(format!("Record error: {err}"));
                    }
                }
            });
        }
    };

    // ---- Stop: finalize both files, resolve playback URLs --------------------
    let on_stop = {
        let recs = recs.clone();
        move || {
            if !recording.get() {
                return;
            }
            let Some((raw_rec, clean_rec)) = recs.borrow_mut().take() else {
                return;
            };
            status.set("Finalizing…".to_string());
            runtime_core::driver::spawn_async(async move {
                let raw_path = raw_rec.stop().await;
                let clean_path = clean_rec.stop().await;
                recording.set(false);
                let (raw_path, clean_path) = match (raw_path, clean_path) {
                    (Ok(a), Ok(b)) => (a, b),
                    (a, b) => {
                        let err = a.err().map(|e| e.to_string())
                            .or_else(|| b.err().map(|e| e.to_string()))
                            .unwrap_or_default();
                        status.set(format!("Finalize error: {err}"));
                        return;
                    }
                };
                let store = match files::app_files(STORE) {
                    Ok(s) => s,
                    Err(e) => {
                        status.set(format!("Storage error: {e}"));
                        return;
                    }
                };
                if let Ok(Some(u)) = store.loadable_url(&raw_path).await {
                    raw_url.set(u);
                }
                if let Ok(Some(u)) = store.loadable_url(&clean_path).await {
                    denoised_url.set(u);
                }
                status.set("Done — play both below to compare".to_string());
            });
        }
    };

    // A compact audio player (AVPlayer on macOS) bound to a reactive URL signal.
    let player = |url: Signal<String>| {
        let style = Rc::new(StyleSheet::r#static(StyleRules {
            width: Some(Length::pct(100.0).into()),
            height: Some(Length::Px(56.0).into()),
            ..Default::default()
        }));
        video::Video(video::VideoProps {
            source: video::url(move || url.get()),
            autoplay: false,
            muted: false,
            controls: true,
            loop_playback: false,
            object_fit: video::ObjectFit::Contain,
        })
        .with_style(style)
        .into_element()
    };

    // The A/B preview mounts once the first URL resolves (after Stop).
    let preview = when(
        move || !raw_url.get().is_empty() || !denoised_url.get().is_empty(),
        move || {
            let kids: Vec<Element> = vec![
                ui! { Typography(content = "Raw (original)".to_string(), muted = true) },
                player(raw_url),
                ui! { Typography(content = "Denoised".to_string(), muted = true) },
                player(denoised_url),
            ];
            ui! { Stack(gap = StackGap::Sm) { kids } }
        },
        || ui! { view {} },
    );

    let body: Vec<Element> = vec![
        ui! { Typography(content = "Noise-suppression SDK demo".to_string(), kind = idea_ui::typography_kind::H1) },
        ui! {
            Typography(
                content = "Microphone input runs through the `denoise` SDK \
                    (DeepFilterNet 3). Start to meter raw vs. denoised live, then \
                    Record / Stop to capture both and A/B them below."
                    .to_string(),
                muted = true,
            )
        },
        text(move || status.get()).into_element(),
        bar(raw_level, "Raw"),
        bar(denoised_level, "Denoised"),
        ui! {
            Stack(gap = StackGap::Sm) {
                button(label = "Start".to_string(), on_click = on_start)
                button(label = "Record".to_string(), on_click = on_record)
                button(label = "Stop".to_string(), on_click = on_stop)
            }
        },
        preview,
    ];

    ui! {
        Stack(gap = StackGap::Md, padding = StackPadding::Lg) { body }
    }
}
