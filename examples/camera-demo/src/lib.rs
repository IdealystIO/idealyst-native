//! `camera-demo` — exercises the `camera` SDK end to end, and (by merely
//! depending on it) the capability-based permission injection.
//!
//! Press **Start camera** → the SDK opens a live capture stream (the OS
//! prompts for permission the first time). Raw `RGBA8` frames arrive in the
//! `on_frame` callback. The demo bridges that callback to reactive UI state
//! the canonical way — the exact job the camera SDK leaves to a higher layer
//! (it ships pixels, not a preview widget):
//!
//! 1. The callback runs on a capture thread (native/Android) or the main
//!    thread (web). It can't touch the reactive runtime directly off-thread,
//!    so it averages the frame's color + records its size into lock-free
//!    [`std::sync::atomic`] globals.
//! 2. A [`raf_loop`](runtime_core::raf_loop) on the main thread reads those
//!    atomics once per frame and folds them into reactive state — tinting the
//!    whole page background to the camera's average color and updating the
//!    resolution / frame-count readout (writing only on change).
//!
//! That's the whole pattern: raw frame callback → atomic hand-off →
//! main-thread state. Showing the frames *as video* would instead upload
//! each into a `graphics` surface; this demo deliberately stays simple and
//! does something every backend can do uniformly with the raw bytes.

use std::sync::atomic::{AtomicU32, Ordering};

use camera::{Camera, CameraConfig, CameraError, VideoFrame};
use idea_ui::{install_idea_theme, light_theme, Stack, StackGap, StackPadding, Typography};
use runtime_core::{set_app_background, signal, text, ui, Element, IntoElement, Signal};

// Frame→UI bridge. The capture callback writes here; the main-thread
// `raf_loop` reads. A single global set is fine for a one-stream demo.
/// Average frame color, packed `0x00RRGGBB`.
static COLOR_BITS: AtomicU32 = AtomicU32::new(0x00_80_80_80);
/// Frame size, packed `width << 16 | height`.
static DIMS: AtomicU32 = AtomicU32::new(0);
/// Total frames delivered since capture started.
static FRAME_COUNT: AtomicU32 = AtomicU32::new(0);

/// At most this many pixels are sampled per frame for the average — keeps the
/// per-frame cost bounded regardless of resolution (a 1080p frame has ~2M
/// pixels; we touch a few thousand).
const MAX_SAMPLES: usize = 4096;

/// No third-party `Element::External` SDKs to register — `camera` is a plain
/// capability crate, not a rendered primitive, so this stays empty.
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(_backend: &mut dev_server::WireRecordingBackend) {}

/// Average the frame's color over a bounded sample of pixels, returning
/// packed `0x00RRGGBB`.
fn average_color(frame: &VideoFrame) -> u32 {
    let pixels = frame.pixel_count();
    if pixels == 0 || frame.data.len() < pixels * 4 {
        return 0x00_80_80_80;
    }
    let step = (pixels / MAX_SAMPLES).max(1);
    let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
    let mut i = 0;
    while i < pixels {
        let px = i * 4;
        r += frame.data[px] as u64;
        g += frame.data[px + 1] as u64;
        b += frame.data[px + 2] as u64;
        n += 1;
        i += step;
    }
    let n = n.max(1);
    let (r, g, b) = ((r / n) as u32, (g / n) as u32, (b / n) as u32);
    (r << 16) | (g << 8) | b
}

pub fn app() -> Element {
    install_idea_theme(light_theme());

    let dims: Signal<u32> = signal!(0);
    let frames: Signal<u32> = signal!(0);
    let status: Signal<String> = signal!("Idle — press Start camera".to_string());
    let started: Signal<bool> = signal!(false);

    // Pump the atomic bridge into the UI once per frame. Change guards keep a
    // steady scene from re-rendering text or re-setting the background every
    // frame — only actual movement fires.
    {
        let last_color = std::cell::Cell::new(u32::MAX);
        let last_dims = std::cell::Cell::new(u32::MAX);
        let last_frames = std::cell::Cell::new(u32::MAX);
        let raf = runtime_core::raf_loop(move || {
            let c = COLOR_BITS.load(Ordering::Relaxed);
            if c != last_color.get() {
                last_color.set(c);
                // `#rrggbb` → Tokenized<Color>; tint the whole page.
                set_app_background(format!("#{c:06x}").into());
            }
            let d = DIMS.load(Ordering::Relaxed);
            if d != last_dims.get() {
                last_dims.set(d);
                dims.set(d);
            }
            let f = FRAME_COUNT.load(Ordering::Relaxed);
            if f != last_frames.get() {
                last_frames.set(f);
                frames.set(f);
            }
        });
        // Page-lifetime loop; keep the handle alive for the app's duration.
        std::mem::forget(raf);
    }

    let status_text = text(move || status.get()).into_element();
    let resolution_text = text(move || {
        let d = dims.get();
        if d == 0 {
            "Resolution: —".to_string()
        } else {
            format!("Resolution: {}×{}", d >> 16, d & 0xFFFF)
        }
    })
    .into_element();
    let frames_text = text(move || format!("Frames captured: {}", frames.get())).into_element();

    // Opening is user-gesture–gated: web requires getUserMedia to start from a
    // user interaction, and it's the right UX everywhere.
    let on_start = move || {
        if started.get() {
            return;
        }
        started.set(true);
        status.set("Requesting camera…".to_string());
        runtime_core::driver::spawn_async(async move {
            let cam = Camera::new();
            let result = cam
                .open(CameraConfig::default(), |frame: &VideoFrame| {
                    COLOR_BITS.store(average_color(frame), Ordering::Relaxed);
                    DIMS.store((frame.width << 16) | (frame.height & 0xFFFF), Ordering::Relaxed);
                    FRAME_COUNT.fetch_add(1, Ordering::Relaxed);
                })
                .await;
            match result {
                Ok(stream) => {
                    status.set("Live — the page tints to what the camera sees".to_string());
                    // The demo has no Stop button; keep capturing for the
                    // app's lifetime. Leaking the stream pins it (and avoids
                    // moving the !Send native session).
                    std::mem::forget(stream);
                }
                Err(e) => {
                    started.set(false);
                    status.set(match e {
                        CameraError::PermissionDenied => "Camera permission denied".to_string(),
                        CameraError::NoCamera => "No camera found".to_string(),
                        CameraError::Unsupported => {
                            "Camera capture isn't supported on this platform".to_string()
                        }
                        other => format!("Error: {other}"),
                    });
                }
            }
        });
    };

    let body: Vec<Element> = vec![
        ui! { Typography(content = "Camera SDK demo".to_string(), kind = idea_ui::typography_kind::H1) },
        ui! {
            Typography(
                content = "Live frames captured straight from the platform's camera \
                    via the `camera` SDK. The SDK ships raw RGBA pixels — this demo \
                    averages each frame's color and tints the page to it. Press \
                    Start, then point the camera around."
                    .to_string(),
                muted = true,
            )
        },
        status_text,
        resolution_text,
        frames_text,
        ui! { button(label = "Start camera".to_string(), on_click = on_start) },
    ];

    ui! {
        Stack(gap = StackGap::Md, padding = StackPadding::Lg) { body }
    }
}
