//! `denoise-demo` — the `denoise` SDK end to end, with a waveform A/B.
//!
//! Two states, nothing in between:
//!
//! - **Recording** — pressing **Record** opens the `microphone` SDK's live
//!   [`AudioStream`](denoise::AudioStream), feeds it into the **already-warm**
//!   DeepFilterNet pipeline (built once at startup — see [`WarmPipeline`]) to
//!   get a cleaned stream, captures BOTH to `.m4a` files via `media-writer`, and
//!   shows live raw-vs-denoised level meters. The mic is open *only* while
//!   recording — **Stop** finalizes the files and tears the mic down (the
//!   denoise pipeline stays warm for the next take).
//! - **Not recording** — if a clip exists, the **A/B monitor** is mounted: both
//!   takes play in lockstep, the two **waveforms are the selector** (tap a track
//!   to make it the audible one, via [`VideoHandle::set_muted`]), and a GPU
//!   canvas draws an Audacity-style waveform per track with a live played-region
//!   highlight + playhead ([`VideoHandle::position`]).
//!
//! The audio→UI bridge is the canonical one (see `mic-demo`): capture/processing
//! callbacks run off the main thread and write peaks into lock-free
//! [`std::sync::atomic`] globals; a main-thread [`raf_loop`](runtime_core::raf_loop)
//! folds those into `Signal`s and, while recording, samples them into the
//! waveform envelopes.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};

use denoise::{AudioStream, Denoiser};
use idea_ui::{
    install_idea_theme, light_theme, size, tone, typography_kind, Badge, Button, Card, CardPadding,
    Divider, Progress, Spinner, Stack, StackAlign, StackAxis, StackGap, StackJustify, StackPadding,
    ToneRef, Typography,
};
use media_writer::{MediaInputs, MediaWriter, RecordConfig, Recording};
use microphone::{AudioStreamConfig, MicError, Microphone};
use runtime_core::{
    rx, signal, switch, ui, Element, IntoElement, Length, Position, Ref, Signal, StyleRules,
    StyleSheet, ViewHandle,
};
use video::{Video, VideoBind, VideoHandle, VideoProps};
use canvas::{color, CanvasProps, Paint, Path, Scene, Stroke};
// Link anchor: `canvas-native` self-registers its CPU renderer via `inventory`
// at backend construction, but ONLY if the crate is actually linked. Nothing
// else references it by name (we draw through the `canvas` SDK), so without this
// `use … as _` the linker drops it and the macOS canvas renders nothing
// ("external not supported"). `canvas-vello` has no inventory hook and is
// registered explicitly in `register_extensions`. See [[project_inventory_self_registration]].
use canvas_native as _;

// Where the two recordings live (./Library/Application Support/denoise-recordings/).
const STORE: &str = "denoise-recordings";
const RAW_FILE: &str = "raw.m4a";
const DENOISED_FILE: &str = "denoised.m4a";

// Track ids for the A/B selector (the waveforms themselves).
const SEG_RAW: &str = "raw";
const SEG_DENOISED: &str = "denoised";

// Waveform rendering + capture (Audacity-style, drawn on the GPU canvas).
const SCRUB_H: f32 = 140.0; // total scrubber canvas height — two stacked lanes
const WAVE_DECIMATE: u32 = 2; // sample the envelope every Nth frame while recording
const WAVE_MAX_GAIN: f32 = 8.0; // cap normalization so a near-silent take isn't blown up to noise
const RAW_WAVE_COLOR: &str = "#475569"; // dark slate — the noisy original (high contrast on white)
const DEN_WAVE_COLOR: &str = "#16a34a"; // green — the cleaned signal
const PLAYHEAD_COLOR: &str = "#6366f1"; // indigo playhead line

// Audio→UI bridge: capture/processing callbacks write peaks here; the
// main-thread `raf_loop` reads. f32 peaks are stored as bit patterns.
static RAW_PEAK_BITS: AtomicU32 = AtomicU32::new(0);
static DENOISED_PEAK_BITS: AtomicU32 = AtomicU32::new(0);

/// The app's two states (Recording / not), plus the two transient screens in the
/// not-recording half: `Finalizing` while files flush, `Preview` once a clip is
/// ready. `Idle` is the not-recording-no-clip start screen.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Recording,
    Finalizing,
    Preview,
}

fn peak_of(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |m, &s| m.max(s.abs()))
}

/// `secs` → `M:SS` for the scrubber readout.
fn fmt_time(secs: f32) -> String {
    let s = secs.max(0.0) as u32;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Bucket a variable-length peak envelope down to exactly `n` points, taking the
/// max of each bucket so transients survive the downsample. Empty → flat.
fn downsample(env: &[f32], n: usize) -> Vec<f32> {
    if env.is_empty() {
        return vec![0.0; n];
    }
    (0..n)
        .map(|i| {
            let start = i * env.len() / n;
            let end = (((i + 1) * env.len() / n).max(start + 1)).min(env.len());
            env[start..end].iter().fold(0.0f32, |m, &v| m.max(v))
        })
        .collect()
}

/// The **preloaded** denoise pipeline, built once at startup and kept warm for
/// the app's lifetime. The model build (the expensive step) happens here, off
/// the record path — so pressing Record no longer waits on it, and the denoised
/// stream starts within its inherent ~30 ms latency rather than after a
/// per-record model build (which was offsetting the two recordings).
///
/// `_relay` is the stable input the pipeline processes; while recording, the mic
/// is forwarded into it (`relay_writer`). `clean` is the denoised output stream
/// the recorder/meter tap. Both are held purely to keep the worker alive.
struct WarmPipeline {
    #[allow(dead_code)]
    _relay: AudioStream,
    clean: AudioStream,
    #[allow(dead_code)]
    _sub: Box<dyn std::any::Any>,
}

/// Per-recording handles, retained while recording. Holding the mic stream keeps
/// the OS mic open; the subscriptions keep the meter firing and forward the mic
/// into the warm pipeline. Dropping it (at Stop) closes the mic — the warm
/// pipeline is untouched and ready for the next take.
struct Live {
    #[allow(dead_code)]
    _mic: AudioStream,
    #[allow(dead_code)]
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
/// downloads only when [`load`] is first awaited (the startup preload), not at
/// page load. Awaiting `LOADER.load()` fetches + links the chunk; `.call` then
/// returns the now-resident `&'static [u8]`.
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

/// `video` and `canvas-native` self-register via inventory at backend
/// construction (the latter only because the `use canvas_native as _` anchor
/// above keeps it linked). `canvas-vello` (the GPU renderer) has no inventory
/// hook, so register it explicitly here — last-registration-wins over native on
/// the targets where vello is viable; it self-gates off where the GPU can't run
/// it (iOS simulator, Android emulator). `microphone`/`denoise`/`media-writer`
/// are plain capability crates.
pub fn register_extensions<B: runtime_core::RegisterExternal>(backend: &mut B) {
    #[cfg(any(
        target_os = "ios",
        target_os = "android",
        target_os = "macos",
        target_arch = "wasm32"
    ))]
    canvas_vello::register(backend);
    #[cfg(not(any(
        target_os = "ios",
        target_os = "android",
        target_os = "macos",
        target_arch = "wasm32"
    )))]
    let _ = backend; // desktop (linux/windows): canvas-native (inventory) only
}

#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(_backend: &mut dev_server::WireRecordingBackend) {}

pub fn app() -> Element {
    install_idea_theme(light_theme());

    let raw_level: Signal<f32> = signal!(0.0);
    let denoised_level: Signal<f32> = signal!(0.0);
    let status: Signal<String> = signal!("Preparing denoiser…".to_string());
    let phase: Signal<Phase> = signal!(Phase::Idle);
    let ready: Signal<bool> = signal!(false); // warm pipeline built?

    // Filled when a clip is finalized; the A/B monitor's players read these.
    let raw_url: Signal<String> = signal!(String::new());
    let denoised_url: Signal<String> = signal!(String::new());
    // Captured peak envelopes, snapshotted at Stop and drawn as waveforms.
    let raw_wave: Signal<Vec<f32>> = signal!(Vec::new());
    let den_wave: Signal<Vec<f32>> = signal!(Vec::new());

    // A/B monitor state: which track is audible, playing, and where we are.
    let ab: Signal<String> = signal!(SEG_DENOISED.to_string());
    let playing: Signal<bool> = signal!(false);
    let progress: Signal<f32> = signal!(0.0); // 0..1 playback position
    let dur_secs: Signal<f32> = signal!(0.0);
    let wave_w: Signal<f32> = signal!(0.0); // laid-out width of the waveform canvas, px
    // Handles to the two synced players, filled when the monitor mounts.
    let raw_player: Ref<VideoHandle> = Ref::new();
    let den_player: Ref<VideoHandle> = Ref::new();
    // The first waveform track's wrapper view — polled for its laid-out width so
    // the GPU draw can map the envelope across the available pixels.
    let wave_box: Ref<ViewHandle> = Ref::new();

    // Per-recording handles + in-progress envelopes. `!Send`, main-thread only.
    let live: Rc<RefCell<Option<Live>>> = Rc::new(RefCell::new(None));
    let recs: Rc<RefCell<Option<(Recording, Recording)>>> = Rc::new(RefCell::new(None));
    let raw_env: Rc<RefCell<Vec<f32>>> = Rc::new(RefCell::new(Vec::new()));
    let den_env: Rc<RefCell<Vec<f32>>> = Rc::new(RefCell::new(Vec::new()));
    // True only between writers-started and Stop — gates envelope capture so the
    // waveform aligns with the recorded file (not the mic-open lead-in).
    let capturing: Rc<Cell<bool>> = Rc::new(Cell::new(false));

    // ---- Preload: build the denoise pipeline ONCE, kept warm ------------------
    // The mic is forwarded into `relay` while recording; `clean` is the denoised
    // output. Building it here (not on the record path) removes the per-record
    // model-build delay that was offsetting the denoised recording.
    let (relay, relay_writer) = AudioStream::new();
    let warm: Rc<RefCell<Option<WarmPipeline>>> = Rc::new(RefCell::new(None));
    {
        let warm = warm.clone();
        runtime_core::driver::spawn_async(async move {
            let denoiser = match make_denoiser().await {
                Ok(d) => d,
                Err(e) => {
                    status.set(format!("Model load error: {e}"));
                    return;
                }
            };
            let clean = match denoiser.process(&relay).await {
                Ok(c) => c,
                Err(e) => {
                    status.set(format!("Denoiser error: {e}"));
                    return;
                }
            };
            let sub = clean.subscribe(|f| {
                DENOISED_PEAK_BITS.store(peak_of(f.samples).to_bits(), Ordering::Relaxed);
            });
            *warm.borrow_mut() = Some(WarmPipeline {
                _relay: relay,
                clean,
                _sub: Box::new(sub),
            });
            // Status first, then `ready` — so the Record button swaps in already
            // showing the prompt rather than the stale "Preparing…" line.
            status.set("Press Record to capture an A/B clip.".to_string());
            ready.set(true);
        });
    }

    // One per-frame loop: pump meters, sample the waveform envelopes while
    // capturing, and (in Preview) poll the player for the scrubber position.
    {
        let raw_env = raw_env.clone();
        let den_env = den_env.clone();
        let capturing = capturing.clone();
        let raw_player = raw_player.clone();
        let wave_box = wave_box.clone();
        let last_raw = Cell::new(u32::MAX);
        let last_den = Cell::new(u32::MAX);
        let frame = Cell::new(0u32);
        let raf = runtime_core::raf_loop(move || {
            // The plain `raf_loop` (unlike the `_scoped` variants) does NOT guard
            // the reactive arena: an OS-dispatched frame can land while a reactive
            // mutation is mid-flight. Writing a signal then panics with "RefCell
            // already borrowed". Skip the frame — exactly what `raf_loop_scoped` does.
            if runtime_core::is_reactive_busy() {
                return;
            }
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

            if capturing.get() {
                let f = frame.get().wrapping_add(1);
                frame.set(f);
                if f % WAVE_DECIMATE == 0 {
                    raw_env.borrow_mut().push(raw_level.get());
                    den_env.borrow_mut().push(denoised_level.get());
                }
            }

            if phase.get() == Phase::Preview {
                // Read everything OUT of the handles first: `Ref::with` holds the
                // reactive arena borrowed across its closure, so a `signal.set()`
                // inside it would re-borrow and panic. Set after `with` returns.
                if let Some((pos, dur)) = raw_player.with(|h| (h.position(), h.duration())) {
                    if dur > 0.0 {
                        dur_secs.set(dur);
                        progress.set((pos / dur).clamp(0.0, 1.0));
                    }
                }
                if let Some(Some(w)) = wave_box.with(|h| h.frame().map(|r| r.width)) {
                    if w > 0.0 && (w - wave_w.get()).abs() > 0.5 {
                        wave_w.set(w);
                    }
                }
            }
        });
        std::mem::forget(raf); // page-lifetime loop
    }

    // ---- Record: open mic, forward into the warm pipeline, capture both -------
    let on_record = {
        let warm = warm.clone();
        let live = live.clone();
        let recs = recs.clone();
        let raw_env = raw_env.clone();
        let den_env = den_env.clone();
        let capturing = capturing.clone();
        let relay_writer = relay_writer.clone();
        move || {
            if matches!(phase.get(), Phase::Recording | Phase::Finalizing) {
                return;
            }
            if !ready.get() {
                status.set("Denoiser still loading — one moment…".to_string());
                return;
            }
            phase.set(Phase::Recording);
            status.set("Starting microphone…".to_string());
            raw_env.borrow_mut().clear();
            den_env.borrow_mut().clear();
            raw_level.set(0.0);
            denoised_level.set(0.0);
            progress.set(0.0);
            capturing.set(false);

            let warm = warm.clone();
            let live = live.clone();
            let recs = recs.clone();
            let capturing = capturing.clone();
            let relay_writer = relay_writer.clone();
            runtime_core::driver::spawn_async(async move {
                let mic = Microphone::new();
                let mic = match mic.open_stream(AudioStreamConfig::default().mono()).await {
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
                // Meter the raw input.
                let raw_sub = mic.subscribe(|f| {
                    RAW_PEAK_BITS.store(peak_of(f.samples).to_bits(), Ordering::Relaxed);
                });
                // Drive the warm pipeline: forward every mic chunk into the relay.
                let fwd_writer = relay_writer.clone();
                let fwd_sub = mic.subscribe(move |f| {
                    let fmt = f.format();
                    fwd_writer.write_pcm_f32(fmt.sample_rate, fmt.channels, f.samples);
                });

                let Some(clean) = warm.borrow().as_ref().map(|w| w.clean.clone()) else {
                    phase.set(Phase::Idle);
                    status.set("Denoiser not ready".to_string());
                    return;
                };

                let store = match files::app_files(STORE) {
                    Ok(s) => s,
                    Err(e) => {
                        phase.set(Phase::Idle);
                        status.set(format!("Storage error: {e}"));
                        return;
                    }
                };
                let writer = MediaWriter::new();
                let raw_rec = writer
                    .record(MediaInputs::audio(&mic), RecordConfig::new(store.clone(), RAW_FILE))
                    .await;
                let clean_rec = writer
                    .record(MediaInputs::audio(&clean), RecordConfig::new(store, DENOISED_FILE))
                    .await;
                match (raw_rec, clean_rec) {
                    (Ok(a), Ok(b)) => {
                        *recs.borrow_mut() = Some((a, b));
                    }
                    (a, b) => {
                        phase.set(Phase::Idle);
                        let err = a
                            .err()
                            .map(|e| e.to_string())
                            .or_else(|| b.err().map(|e| e.to_string()))
                            .unwrap_or_default();
                        status.set(format!("Record error: {err}"));
                        return;
                    }
                }

                *live.borrow_mut() = Some(Live {
                    _mic: mic,
                    _subs: vec![Box::new(raw_sub), Box::new(fwd_sub)],
                });
                capturing.set(true);
                status.set("Recording — press Stop to finish.".to_string());
            });
        }
    };

    // ---- Stop: finalize files, tear down the mic, resolve playback URLs -------
    let on_stop = {
        let live = live.clone();
        let recs = recs.clone();
        let raw_env = raw_env.clone();
        let den_env = den_env.clone();
        let capturing = capturing.clone();
        move || {
            if phase.get() != Phase::Recording {
                return;
            }
            let Some((raw_rec, clean_rec)) = recs.borrow_mut().take() else {
                return;
            };
            capturing.set(false);
            phase.set(Phase::Finalizing);
            status.set("Finalizing…".to_string());
            // Snapshot the full peak envelopes for the waveform; the canvas
            // downsamples them to its pixel width at draw time.
            raw_wave.set(raw_env.borrow().clone());
            den_wave.set(den_env.borrow().clone());

            let live = live.clone();
            runtime_core::driver::spawn_async(async move {
                let raw_path = raw_rec.stop().await;
                let clean_path = clean_rec.stop().await;
                // Drop the per-recording handles → mic closes + forwarding stops.
                // The warm denoise pipeline is untouched (ready for the next take).
                *live.borrow_mut() = None;
                raw_level.set(0.0);
                denoised_level.set(0.0);

                let (raw_path, clean_path) = match (raw_path, clean_path) {
                    (Ok(a), Ok(b)) => (a, b),
                    (a, b) => {
                        phase.set(Phase::Idle);
                        let err = a
                            .err()
                            .map(|e| e.to_string())
                            .or_else(|| b.err().map(|e| e.to_string()))
                            .unwrap_or_default();
                        status.set(format!("Finalize error: {err}"));
                        return;
                    }
                };
                let store = match files::app_files(STORE) {
                    Ok(s) => s,
                    Err(e) => {
                        phase.set(Phase::Idle);
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
                ab.set(SEG_DENOISED.to_string());
                playing.set(false);
                progress.set(0.0);
                phase.set(Phase::Preview);
                status.set("Compare — tap a track to hear it while it plays.".to_string());
            });
        }
    };

    // ---- A/B monitor handlers ------------------------------------------------
    // Apply the audible/muted split: the chosen track unmutes, the other mutes.
    // Both players keep running, so the toggle is heard at the same playhead.
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
                apply_ab(&ab.get()); // re-assert the split on (re)start
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

    // ---- The main panel, one screen per phase --------------------------------
    let panel = switch(
        move || phase.get(),
        {
            let on_record = on_record.clone();
            let on_stop = on_stop.clone();
            let on_ab = on_ab.clone();
            let on_play_pause = on_play_pause.clone();
            let on_replay = on_replay.clone();
            let raw_player = raw_player.clone();
            let den_player = den_player.clone();
            let wave_box = wave_box.clone();
            move |p: &Phase| match p {
                Phase::Idle => {
                    // The Record button IS the model-loader: while the warm
                    // pipeline builds, it shows a spinner and is inert; once ready
                    // it becomes the live Record button. No card — it's a single
                    // call-to-action, just centered. Nested `switch` on `ready`.
                    let on_record = on_record.clone();
                    switch(move || ready.get(), move |&is_ready| {
                        if is_ready {
                            let on_record: Rc<dyn Fn()> = Rc::new(on_record.clone());
                            ui! {
                                Stack(gap = StackGap::Sm, align = StackAlign::Center) {
                                    Typography(content = status, muted = true)
                                    Button(
                                        label = "● Record".to_string(),
                                        on_click = on_record,
                                        tone = tone::Danger,
                                        size = size::Lg,
                                    )
                                }
                            }
                        } else {
                            ui! {
                                Stack(align = StackAlign::Center) {
                                    Button(
                                        label = "Loading model…".to_string(),
                                        loading = true,
                                        disabled = true,
                                        tone = tone::Danger,
                                        size = size::Lg,
                                    )
                                }
                            }
                        }
                    })
                }
                Phase::Recording => {
                    // Meters are content → a panel (card); the Stop button is a
                    // single centered action, not full-width.
                    let on_stop: Rc<dyn Fn()> = Rc::new(on_stop.clone());
                    let raw_meter = meter("Raw input", raw_level, tone::Neutral.into());
                    let den_meter = meter("Denoised", denoised_level, tone::Success.into());
                    ui! {
                        Card(padding = CardPadding::Lg) {
                            Stack(gap = StackGap::Md) {
                                Stack(axis = StackAxis::Row, align = StackAlign::Center, gap = StackGap::Sm) {
                                    Badge(label = "● REC".to_string(), tone = tone::Danger)
                                    Typography(content = status, muted = true)
                                }
                                raw_meter
                                den_meter
                                Divider()
                                Stack(align = StackAlign::Center) {
                                    Button(
                                        label = "■ Stop".to_string(),
                                        on_click = on_stop,
                                        tone = tone::Danger,
                                    )
                                }
                            }
                        }
                    }
                }
                Phase::Finalizing => {
                    // Transient — just a centered spinner + label, no card.
                    ui! {
                        Stack(gap = StackGap::Sm, axis = StackAxis::Row, align = StackAlign::Center, justify = StackJustify::Center) {
                            Spinner()
                            Typography(content = status, muted = true)
                        }
                    }
                }
                Phase::Preview => {
                    let on_record: Rc<dyn Fn()> = Rc::new(on_record.clone());
                    let on_play_pause: Rc<dyn Fn()> = Rc::new(on_play_pause.clone());
                    let on_replay: Rc<dyn Fn()> = Rc::new(on_replay.clone());
                    let raw_p = hidden_player(raw_url, true, raw_player.clone());
                    let den_p = hidden_player(denoised_url, false, den_player.clone());
                    let scrub = scrubber(
                        raw_wave,
                        den_wave,
                        progress,
                        dur_secs,
                        wave_w,
                        wave_box.clone(),
                        ab,
                        on_ab.clone(),
                    );
                    ui! {
                        Card(padding = CardPadding::Lg) {
                            Stack(gap = StackGap::Md) {
                                Typography(content = "A/B monitor".to_string(), kind = typography_kind::H2)
                                Typography(content = status, muted = true)
                                scrub
                                Stack(gap = StackGap::Sm, axis = StackAxis::Row, justify = StackJustify::Center) {
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
                                Divider()
                                Stack(align = StackAlign::Center) {
                                    Button(
                                        label = "● Record again".to_string(),
                                        on_click = on_record,
                                        tone = tone::Danger,
                                    )
                                }
                                raw_p
                                den_p
                            }
                        }
                    }
                }
            }
        },
    );

    // Constrain the content to a readable, centered column rather than letting
    // it span the whole window.
    let content = ui! {
        Stack(gap = StackGap::Lg) {
            Stack(gap = StackGap::Xs) {
                Typography(content = "Noise suppression".to_string(), kind = typography_kind::H1)
                Typography(
                    content = "Record a clip and the `denoise` SDK (DeepFilterNet 3) captures \
                        it twice at once — raw and cleaned. Then tap either waveform to A/B them."
                        .to_string(),
                    muted = true,
                )
            }
            panel
        }
    };
    let bounded = runtime_core::view(vec![content])
        .with_style(Rc::new(StyleSheet::r#static(StyleRules {
            width: Some(Length::pct(100.0).into()),
            max_width: Some(Length::Px(720.0).into()),
            ..Default::default()
        })))
        .into_element();

    ui! {
        Stack(padding = StackPadding::Lg, align = StackAlign::Center) {
            bounded
        }
    }
}

/// The playback scrubber: **one** GPU canvas spanning both takes — Raw lane on
/// top, Denoised below — with a **single continuous playhead** through both, a
/// per-lane played-region highlight, and a `M:SS / M:SS` readout. Two transparent
/// `pressable` overlays (top half / bottom half) make each lane tappable to
/// select it as the audible one. The overlay-over-canvas layering is deliberate:
/// a native canvas view can swallow clicks, so the always-on-top pressables own
/// the gesture. Shared normalization gain is computed once (envelopes are set
/// before the monitor mounts); `progress`/`wave_w`/`ab` drive the redraw.
#[allow(clippy::too_many_arguments)]
fn scrubber(
    raw_wave: Signal<Vec<f32>>,
    den_wave: Signal<Vec<f32>>,
    progress: Signal<f32>,
    dur_secs: Signal<f32>,
    wave_w: Signal<f32>,
    wave_box: Ref<ViewHandle>,
    ab: Signal<String>,
    on_ab: Rc<dyn Fn(String)>,
) -> Element {
    let peak = raw_wave
        .get()
        .iter()
        .chain(den_wave.get().iter())
        .fold(0.0f32, |m, &v| m.max(v));
    let gain = if peak > 1e-4 {
        (1.0 / peak).min(WAVE_MAX_GAIN)
    } else {
        1.0
    };

    let canvas_el = canvas::Canvas(CanvasProps {
        draw: canvas::draw(move |s: &mut Scene| {
            draw_scrubber(
                s,
                &raw_wave.get(),
                &den_wave.get(),
                gain,
                progress.get(),
                wave_w.get(),
                &ab.get(),
            );
        }),
        ..Default::default()
    })
    .with_style(Rc::new(StyleSheet::r#static(StyleRules {
        width: Some(Length::pct(100.0).into()),
        height: Some(Length::pct(100.0).into()),
        ..Default::default()
    })))
    .into_element();

    let raw_ov = lane_overlay(SEG_RAW, "Raw", ab, on_ab.clone(), 0.0);
    let den_ov = lane_overlay(SEG_DENOISED, "Denoised", ab, on_ab, 50.0);

    let container = runtime_core::view(vec![canvas_el, raw_ov, den_ov])
        .bind(wave_box)
        .with_style(Rc::new(StyleSheet::r#static(StyleRules {
            position: Some(Position::Relative),
            width: Some(Length::pct(100.0).into()),
            height: Some(Length::Px(SCRUB_H).into()),
            ..Default::default()
        })))
        .into_element();

    ui! {
        Stack(gap = StackGap::Xs) {
            container
            Stack(axis = StackAxis::Row, justify = StackJustify::Between) {
                Typography(content = rx!(fmt_time(progress.get() * dur_secs.get())), muted = true)
                Typography(content = rx!(fmt_time(dur_secs.get())), muted = true)
            }
        }
    }
}

/// A transparent, full-width pressable covering one lane's half of the scrubber
/// (top half at `top_pct = 0`, bottom half at `top_pct = 50`). Tapping it selects
/// that lane; it carries the `●`/`○` lane label.
fn lane_overlay(
    sel_id: &'static str,
    name: &'static str,
    ab: Signal<String>,
    on_ab: Rc<dyn Fn(String)>,
    top_pct: f32,
) -> Element {
    let label = ui! {
        Typography(
            content = rx!(if ab.get() == sel_id {
                format!("● {name}")
            } else {
                format!("○ {name}")
            }),
            muted = true,
        )
    };
    let on_select = move || (on_ab)(sel_id.to_string());
    runtime_core::pressable(vec![label], on_select)
        .with_style(Rc::new(StyleSheet::r#static(StyleRules {
            position: Some(Position::Absolute),
            top: Some(Length::pct(top_pct).into()),
            left: Some(Length::Px(0.0).into()),
            width: Some(Length::pct(100.0).into()),
            height: Some(Length::pct(50.0).into()),
            padding_left: Some(Length::Px(10.0).into()),
            padding_top: Some(Length::Px(6.0).into()),
            ..Default::default()
        })))
        .into_element()
}

/// Paint both takes into one box: Raw lane (top), Denoised lane (bottom), then a
/// single continuous playhead line through both. Coordinates are logical px over
/// a `w × SCRUB_H` box.
fn draw_scrubber(s: &mut Scene, raw: &[f32], den: &[f32], gain: f32, progress: f32, w: f32, selected: &str) {
    if w <= 1.0 {
        return;
    }
    let h = SCRUB_H;
    let lane_h = h * 0.5;
    let hh = h * 0.20; // per-lane amplitude half-height (lanes stay clear of each other)
    draw_lane(s, raw, gain, progress, w, lane_h * 0.5, hh, RAW_WAVE_COLOR, selected == SEG_RAW, 0.0, lane_h);
    draw_lane(s, den, gain, progress, w, lane_h * 1.5, hh, DEN_WAVE_COLOR, selected == SEG_DENOISED, lane_h, lane_h);

    // One continuous playhead through both lanes.
    let xp = progress.clamp(0.0, 1.0) * w;
    let line = Path::new().move_to(xp, 0.0).line_to(xp, h);
    s.stroke_path(line, Paint::solid(color(PLAYHEAD_COLOR)), Stroke::width(1.5));
}

/// Paint one take as a centered, mirrored, filled waveform (the Audacity look)
/// with a played-region highlight, no playhead (the scrubber draws one shared
/// line). When `selected`, a faint highlight panel fills the lane's half and the
/// waveform paints at full strength; unselected lanes recede.
#[allow(clippy::too_many_arguments)]
fn draw_lane(
    s: &mut Scene,
    env: &[f32],
    gain: f32,
    progress: f32,
    w: f32,
    yc: f32,
    hh: f32,
    hex: &str,
    selected: bool,
    lane_y0: f32,
    lane_h: f32,
) {
    let base = color(hex);
    if selected {
        let bg = canvas::Color { a: 28, ..base };
        s.fill_path(
            Path::rounded_rect(0.0, lane_y0 + 2.0, w, lane_h - 4.0, 8.0),
            Paint::solid(bg),
        );
    }

    // One outline point per ~2 px for a smooth-but-cheap waveform.
    let n = ((w / 2.0) as usize).clamp(2, 1024);
    let pts = downsample(env, n);
    // Both waveforms are clearly visible at rest; selection reads from the
    // highlight panel + brighter fill, and the played region brightens further.
    let rest_a = if selected { 225 } else { 150 };
    let played_a = if selected { 255 } else { 210 };

    s.fill_path(
        envelope_path(&pts, gain, w, yc, hh, pts.len()),
        Paint::solid(canvas::Color { a: rest_a, ..base }),
    );
    let played = (progress.clamp(0.0, 1.0) * pts.len() as f32).round() as usize;
    if played >= 2 {
        s.fill_path(
            envelope_path(&pts, gain, w, yc, hh, played.min(pts.len())),
            Paint::solid(canvas::Color { a: played_a, ..base }),
        );
    }
}

/// Build a closed, filled, mirrored envelope polygon over the first `count`
/// points of `pts` (x positions still span the full width via `pts.len()`),
/// centered on `yc` with amplitude scaled by `gain` into `±hh`.
fn envelope_path(pts: &[f32], gain: f32, w: f32, yc: f32, hh: f32, count: usize) -> Path {
    let total = pts.len().max(2);
    let xi = |i: usize| i as f32 / (total - 1) as f32 * w;
    let amp = |i: usize| ((pts[i] * gain).clamp(0.0, 1.0) * hh).max(0.6);
    let count = count.clamp(2, total);
    let mut p = Path::new().move_to(xi(0), yc - amp(0));
    for i in 1..count {
        p = p.line_to(xi(i), yc - amp(i));
    }
    for i in (0..count).rev() {
        p = p.line_to(xi(i), yc + amp(i));
    }
    p.close()
}
