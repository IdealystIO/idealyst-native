//! `denoise-demo` — the `denoise` SDK end to end, with a real-time A/B.
//!
//! 1. **Start** opens the `microphone` SDK's live [`AudioStream`](denoise::AudioStream)
//!    and feeds it through [`Denoiser::process`], which returns a second
//!    (48 kHz mono) stream of the DeepFilterNet-enhanced signal. Two live
//!    [`Progress`](idea_ui::Progress) meters show the **raw** input vs. the
//!    **denoised** output peak — make noise without speaking and the denoised
//!    bar drops while the raw bar stays up.
//! 2. **Record** captures BOTH streams to two `.m4a` files via the
//!    `media-writer` SDK (`raw.m4a` + `denoised.m4a`) — the exact same moment of
//!    audio, one cleaned and one not.
//! 3. **Stop** finalizes the files and mounts the **A/B monitor**: two `video`
//!    SDK players (AVPlayer on macOS) of equal length, started together and kept
//!    in lockstep. A `SegmentedControl` flips which one is *audible* in real time
//!    (the other is muted via the new [`VideoHandle::set_muted`]), so you hear
//!    the cut between raw and denoised at the same playhead — the whole point of
//!    an A/B.
//!
//! The audio→UI meter bridge is the canonical one (see `mic-demo`):
//! capture/processing callbacks run off the main thread, so they write peaks
//! into lock-free [`std::sync::atomic`] globals; a main-thread
//! [`raf_loop`](runtime_core::raf_loop) folds those into `Signal`s.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};

use denoise::{AudioStream, Denoiser};
use idea_ui::{
    install_idea_theme, light_theme, tone, typography_kind, Badge, Button, Card, CardPadding,
    Divider, Progress, SegmentOption, SegmentedControl, Stack, StackAlign, StackAxis, StackGap,
    StackJustify, StackPadding, ToneRef, Typography,
};
use media_writer::{MediaInputs, MediaWriter, RecordConfig, Recording};
use microphone::{AudioStreamConfig, MicError, Microphone};
use runtime_core::{
    rx, signal, switch, Element, IntoElement, Length, Ref, Signal, StyleRules, StyleSheet,
};
use video::{Video, VideoBind, VideoHandle, VideoProps};

// Where the two recordings live (./Library/Application Support/denoise-recordings/).
const STORE: &str = "denoise-recordings";
const RAW_FILE: &str = "raw.m4a";
const DENOISED_FILE: &str = "denoised.m4a";

// Segment ids for the A/B monitor's source toggle.
const SEG_RAW: &str = "raw";
const SEG_DENOISED: &str = "denoised";

// Audio→UI bridge: capture/processing callbacks write peaks here; the
// main-thread `raf_loop` reads. f32 peaks are stored as bit patterns.
static RAW_PEAK_BITS: AtomicU32 = AtomicU32::new(0);
static DENOISED_PEAK_BITS: AtomicU32 = AtomicU32::new(0);

/// Which phase the app is in. Drives the action button + whether the A/B
/// monitor is mounted. `Copy` so it rides in a `Signal` and closures freely.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Nothing open yet — the Start button is showing.
    Idle,
    /// Mic + denoiser live, meters running, ready to capture.
    Listening,
    /// Capturing both streams to files.
    Recording,
    /// Files finalized; the A/B monitor is mounted below.
    Done,
}

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
    let status: Signal<String> = signal!("Press Start to open your microphone.".to_string());
    let phase: Signal<Phase> = signal!(Phase::Idle);
    // Empty until a recording is finalized; the A/B monitor's players read these.
    let raw_url: Signal<String> = signal!(String::new());
    let denoised_url: Signal<String> = signal!(String::new());

    // A/B monitor state: which source is audible, and whether it's playing.
    let ab: Signal<String> = signal!(SEG_DENOISED.to_string());
    let playing: Signal<bool> = signal!(false);
    // Handles to the two synced players, filled when the monitor mounts.
    let raw_player: Ref<VideoHandle> = Ref::new();
    let den_player: Ref<VideoHandle> = Ref::new();

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

    // ---- Start: open mic, denoise, wire meters -------------------------------
    let on_start = {
        let live = live.clone();
        move || {
            if phase.get() != Phase::Idle {
                return;
            }
            phase.set(Phase::Listening);
            status.set("Requesting microphone…".to_string());
            let live = live.clone();
            runtime_core::driver::spawn_async(async move {
                let mic = Microphone::new();
                let raw = match mic.open_stream(AudioStreamConfig::default().mono()).await {
                    Ok(s) => s,
                    Err(e) => {
                        phase.set(Phase::Idle);
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
                        phase.set(Phase::Idle);
                        status.set(format!("Model load error: {e}"));
                        return;
                    }
                };
                let clean = match denoiser.process(&raw).await {
                    Ok(c) => c,
                    Err(e) => {
                        phase.set(Phase::Idle);
                        status.set(format!("Denoiser error: {e}"));
                        return;
                    }
                };
                let clean_sub = clean.subscribe(|f| {
                    DENOISED_PEAK_BITS.store(peak_of(f.samples).to_bits(), Ordering::Relaxed);
                });

                status.set("Listening — press Record to capture an A/B clip.".to_string());
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
            // Re-recordable from both Listening and Done; ignore mid-record.
            if phase.get() == Phase::Idle || phase.get() == Phase::Recording {
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
            phase.set(Phase::Recording);
            playing.set(false);
            raw_url.set(String::new());
            denoised_url.set(String::new());
            status.set("Recording…".to_string());
            let recs = recs.clone();
            runtime_core::driver::spawn_async(async move {
                let store = match files::app_files(STORE) {
                    Ok(s) => s,
                    Err(e) => {
                        phase.set(Phase::Listening);
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
                        status.set("Recording — press Stop to finish.".to_string());
                    }
                    (a, b) => {
                        phase.set(Phase::Listening);
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
            if phase.get() != Phase::Recording {
                return;
            }
            let Some((raw_rec, clean_rec)) = recs.borrow_mut().take() else {
                return;
            };
            status.set("Finalizing…".to_string());
            runtime_core::driver::spawn_async(async move {
                let raw_path = raw_rec.stop().await;
                let clean_path = clean_rec.stop().await;
                let (raw_path, clean_path) = match (raw_path, clean_path) {
                    (Ok(a), Ok(b)) => (a, b),
                    (a, b) => {
                        phase.set(Phase::Listening);
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
                        phase.set(Phase::Listening);
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
                phase.set(Phase::Done);
                status.set("Compare below — flip Raw / Denoised while it plays.".to_string());
            });
        }
    };

    // ---- A/B monitor handlers ------------------------------------------------
    // Apply the audible/muted split for a given selection: the chosen source
    // unmutes, the other mutes. This is what makes the toggle real-time —
    // both players keep running, we only move the "audible" flag.
    let apply_ab = {
        let raw_player = raw_player.clone();
        let den_player = den_player.clone();
        move |sel: &str| {
            let denoised = sel == SEG_DENOISED;
            raw_player.with(|h| h.set_muted(denoised));
            den_player.with(|h| h.set_muted(!denoised));
        }
    };

    let on_ab: Rc<dyn Fn(String)> = {
        let apply_ab = apply_ab.clone();
        Rc::new(move |v: String| {
            ab.set(v.clone());
            apply_ab(&v);
        })
    };

    let on_play_pause = {
        let raw_player = raw_player.clone();
        let den_player = den_player.clone();
        let apply_ab = apply_ab.clone();
        move || {
            let now = !playing.get();
            playing.set(now);
            if now {
                // Re-assert the mute split each time we start: a fresh mount
                // begins from the construction-time `muted`, and a play after a
                // source swap must respect the current selection.
                apply_ab(&ab.get());
                raw_player.with(|h| h.play());
                den_player.with(|h| h.play());
            } else {
                raw_player.with(|h| h.pause());
                den_player.with(|h| h.pause());
            }
        }
    };

    let on_replay = {
        let raw_player = raw_player.clone();
        let den_player = den_player.clone();
        let apply_ab = apply_ab.clone();
        move || {
            apply_ab(&ab.get());
            raw_player.with(|h| {
                h.seek(0.0);
                h.play();
            });
            den_player.with(|h| {
                h.seek(0.0);
                h.play();
            });
            playing.set(true);
        }
    };

    // ---- A live level meter: a labelled, reactive Progress bar ---------------
    let meter = |label: &'static str, level: Signal<f32>, t: ToneRef| -> Element {
        ui! {
            Stack(gap = StackGap::Xs) {
                Stack(axis = StackAxis::Row, justify = StackJustify::Between) {
                    Typography(content = label.to_string(), muted = true)
                    Typography(
                        content = rx!(format!("{:>3.0}%", level.get().clamp(0.0, 1.0) * 100.0)),
                        muted = true,
                    )
                }
                Progress(value = level, tone = t)
            }
        }
    };

    // ---- A hidden, looping player feeding the A/B monitor --------------------
    // Audio-only (.m4a), so there's nothing to show — collapse it to 0×0. It
    // exists purely to decode + play; `controls = false` keeps the two players
    // from drifting (no independent scrubbers), `loop_playback` keeps the
    // comparison going, and `set_muted` (driven from the toggle) picks which
    // one you hear.
    let hidden_player = |url: Signal<String>, start_muted: bool, r: Ref<VideoHandle>| -> Element {
        let style = Rc::new(StyleSheet::r#static(StyleRules {
            width: Some(Length::Px(0.0).into()),
            height: Some(Length::Px(0.0).into()),
            ..Default::default()
        }));
        Video(VideoProps {
            source: video::url(move || url.get()),
            autoplay: false,
            muted: start_muted,
            controls: false,
            loop_playback: true,
            object_fit: video::ObjectFit::Contain,
        })
        .bind(r)
        .with_style(style)
        .into_element()
    };

    // ---- The action button: one primary action per phase ---------------------
    let action = switch(
        move || phase.get(),
        {
            let on_start = on_start.clone();
            let on_record = on_record.clone();
            let on_stop = on_stop.clone();
            move |p: &Phase| match p {
                Phase::Idle => {
                    let on_start: Rc<dyn Fn()> = Rc::new(on_start.clone());
                    ui! {
                        Button(
                            label = "Start".to_string(),
                            on_click = on_start,
                            tone = tone::Primary,
                            block = true,
                        )
                    }
                }
                Phase::Listening => {
                    let on_record: Rc<dyn Fn()> = Rc::new(on_record.clone());
                    ui! {
                        Button(
                            label = "● Record".to_string(),
                            on_click = on_record,
                            tone = tone::Danger,
                            block = true,
                        )
                    }
                }
                Phase::Recording => {
                    let on_stop: Rc<dyn Fn()> = Rc::new(on_stop.clone());
                    ui! {
                        Stack(gap = StackGap::Sm, axis = StackAxis::Row, align = StackAlign::Center) {
                            Badge(label = "REC".to_string(), tone = tone::Danger)
                            Button(
                                label = "■ Stop".to_string(),
                                on_click = on_stop,
                                tone = tone::Danger,
                                block = true,
                            )
                        }
                    }
                }
                Phase::Done => {
                    let on_record: Rc<dyn Fn()> = Rc::new(on_record.clone());
                    ui! {
                        Button(
                            label = "● Record again".to_string(),
                            on_click = on_record,
                            tone = tone::Danger,
                            block = true,
                        )
                    }
                }
            }
        },
    );

    // ---- The A/B monitor: mounts only once a clip is captured ----------------
    let monitor = switch(
        move || phase.get() == Phase::Done,
        {
            let on_ab = on_ab.clone();
            let on_play_pause = on_play_pause.clone();
            let on_replay = on_replay.clone();
            let raw_player = raw_player.clone();
            let den_player = den_player.clone();
            move |&shown: &bool| {
                if !shown {
                    return ui! { view {} };
                }
                let on_ab = on_ab.clone();
                let on_play_pause: Rc<dyn Fn()> = Rc::new(on_play_pause.clone());
                let on_replay: Rc<dyn Fn()> = Rc::new(on_replay.clone());
                ui! {
                    Card(padding = CardPadding::Lg) {
                        Stack(gap = StackGap::Md) {
                            Typography(content = "A/B monitor".to_string(), kind = typography_kind::H2)
                            Typography(
                                content = "Both takes play in lockstep — switch which one \
                                    you hear without losing your place.".to_string(),
                                muted = true,
                            )
                            SegmentedControl(
                                value = ab,
                                on_change = on_ab,
                                options = vec![
                                    SegmentOption::new(SEG_RAW, "Raw"),
                                    SegmentOption::new(SEG_DENOISED, "Denoised"),
                                ],
                            )
                            Stack(gap = StackGap::Sm, axis = StackAxis::Row) {
                                Button(
                                    label = rx!(if playing.get() {
                                        "⏸ Pause".to_string()
                                    } else {
                                        "▶ Play".to_string()
                                    }),
                                    on_click = on_play_pause,
                                    tone = tone::Primary,
                                )
                                Button(
                                    label = "↺ Restart".to_string(),
                                    on_click = on_replay,
                                    tone = tone::Neutral,
                                )
                            }
                            // The two synced, hidden players. Raw starts muted so
                            // the default-selected "Denoised" is what you hear first.
                            { hidden_player(raw_url, true, raw_player.clone()) }
                            { hidden_player(denoised_url, false, den_player.clone()) }
                        }
                    }
                }
            }
        },
    );

    ui! {
        Stack(gap = StackGap::Lg, padding = StackPadding::Lg) {
            Stack(gap = StackGap::Xs) {
                Typography(content = "Noise suppression".to_string(), kind = typography_kind::H1)
                Typography(
                    content = "Your mic runs through the `denoise` SDK (DeepFilterNet 3). \
                        Watch raw vs. denoised live, then capture both and A/B them.".to_string(),
                    muted = true,
                )
            }
            Card(padding = CardPadding::Lg) {
                Stack(gap = StackGap::Md) {
                    Typography(content = status, muted = true)
                    { meter("Raw input", raw_level, tone::Neutral) }
                    { meter("Denoised", denoised_level, tone::Success) }
                    Divider()
                    action
                }
            }
            monitor
        }
    }
}
