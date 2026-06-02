//! `screen-recorder-demo` — exercises the `screen-recorder` SDK's capture
//! capability end to end on the web backend.
//!
//! Press **Start recording** → the SDK calls `getDisplayMedia`, the browser
//! shows its source picker (the app's own tab is the default via the
//! `preferCurrentTab` hint), and raw RGBA frames begin arriving in the
//! `on_frame` callback. The demo bridges that callback to reactive UI the
//! same canonical way `mic-demo` does — the job the SDK deliberately leaves
//! to a higher layer:
//!
//! 1. The callback runs on the capture thread (native) or the main thread
//!    (web). It can't touch the reactive runtime off-thread, so it writes
//!    a frame count + the last frame's dimensions into lock-free
//!    [`std::sync::atomic`] globals.
//! 2. A [`raf_loop`](runtime_core::raf_loop) on the main thread folds those
//!    atomics into `Signal`s the UI binds to, writing only on change.
//!
//! **Stop recording** drops the [`RecordingHandle`], which clears the
//! frame pump and stops the capture tracks (the browser's "sharing" chrome
//! disappears).
//!
//! Note: the SDK hands you raw frames and nothing else — no file is
//! written. Encoding/persistence is a separate higher-level crate.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};

use idea_ui::{install_idea_theme, light_theme, Stack, StackGap, StackPadding, Typography};
use runtime_core::{signal, text, ui, Element, IntoElement, Signal};
use screen_recorder::{RecorderError, RecordingConfig, RecordingHandle, ScreenRecorder, VideoFrame};

// Capture→UI bridge. The frame callback writes here; the main-thread
// `raf_loop` reads. A single global set is fine for a one-stream demo.
static FRAME_COUNT: AtomicU32 = AtomicU32::new(0);
static LAST_WIDTH: AtomicU32 = AtomicU32::new(0);
static LAST_HEIGHT: AtomicU32 = AtomicU32::new(0);

/// No third-party `Element::External` SDKs to register — this demo uses the
/// capture capability, which is a plain object API, not a rendered
/// primitive. (When you render a `screen_recorder::PrivateLayer`, that's
/// where you'd call `screen_recorder::register(backend)`.)
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(_backend: &mut dev_server::WireRecordingBackend) {}

pub fn app() -> Element {
    install_idea_theme(light_theme());

    let frames: Signal<u32> = signal!(0);
    let dims: Signal<(u32, u32)> = signal!((0, 0));
    let status: Signal<String> = signal!("Idle — press Start recording".to_string());
    let started: Signal<bool> = signal!(false);

    // The live recording handle, shared between the start and stop buttons.
    // `!Send`, but the web app is single-threaded so an `Rc<RefCell<_>>` is
    // the right holder.
    let handle: Rc<RefCell<Option<RecordingHandle>>> = Rc::new(RefCell::new(None));

    // Fold the atomic bridge into signals once per frame, only on change.
    {
        let last_frames = Cell::new(u32::MAX);
        let last_dims = Cell::new((u32::MAX, u32::MAX));
        let raf = runtime_core::raf_loop(move || {
            let f = FRAME_COUNT.load(Ordering::Relaxed);
            if f != last_frames.get() {
                last_frames.set(f);
                frames.set(f);
            }
            let d = (
                LAST_WIDTH.load(Ordering::Relaxed),
                LAST_HEIGHT.load(Ordering::Relaxed),
            );
            if d != last_dims.get() {
                last_dims.set(d);
                dims.set(d);
            }
        });
        // Page-lifetime loop; keep the handle alive for the app's duration.
        std::mem::forget(raf);
    }

    let status_text = text(move || status.get()).into_element();
    let frames_text = text(move || format!("Frames captured: {}", frames.get())).into_element();
    let dims_text = text(move || {
        let (w, h) = dims.get();
        if w == 0 {
            "Resolution: —".to_string()
        } else {
            format!("Resolution: {w}×{h}")
        }
    })
    .into_element();

    // Starting is user-gesture–gated: getDisplayMedia can only prompt from a
    // user interaction, so it must run from the button press.
    let on_start = {
        let handle = handle.clone();
        move || {
            if started.get() {
                return;
            }
            started.set(true);
            status.set("Requesting screen capture — pick a source…".to_string());
            FRAME_COUNT.store(0, Ordering::Relaxed);
            let handle = handle.clone();
            runtime_core::driver::spawn_async(async move {
                let recorder = ScreenRecorder::new();
                let result = recorder
                    .start(RecordingConfig::new(), |frame: &VideoFrame| {
                        FRAME_COUNT.fetch_add(1, Ordering::Relaxed);
                        LAST_WIDTH.store(frame.width, Ordering::Relaxed);
                        LAST_HEIGHT.store(frame.height, Ordering::Relaxed);
                    })
                    .await;
                match result {
                    Ok(h) => {
                        status.set("Recording — frames streaming. Press Stop to end.".to_string());
                        *handle.borrow_mut() = Some(h);
                    }
                    Err(e) => {
                        started.set(false);
                        status.set(match e {
                            RecorderError::PermissionDenied => {
                                "Screen capture cancelled or denied".to_string()
                            }
                            RecorderError::Unsupported => {
                                "Screen recording isn't supported on this platform".to_string()
                            }
                            other => format!("Error: {other}"),
                        });
                    }
                }
            });
        }
    };

    let on_stop = {
        let handle = handle.clone();
        move || {
            // Bind `take()` into a let so the `RefMut` is released before we
            // move the handle into the async block (avoids holding the borrow
            // across the await).
            let taken = handle.borrow_mut().take();
            if let Some(h) = taken {
                started.set(false);
                status.set("Stopped".to_string());
                runtime_core::driver::spawn_async(async move {
                    let _ = h.stop().await;
                });
            }
        }
    };

    let body: Vec<Element> = vec![
        ui! { Typography(content = "Screen recorder SDK demo".to_string(), kind = idea_ui::typography_kind::H1) },
        ui! {
            Typography(
                content = "Captures the current tab/window/screen via \
                    `getDisplayMedia` and streams raw frames through the \
                    `screen-recorder` SDK. Press Start, pick a source, and \
                    watch the frame counter tick. No file is written — the \
                    SDK hands you raw frames and stops there."
                    .to_string(),
                muted = true,
            )
        },
        status_text,
        frames_text,
        dims_text,
        ui! { button(label = "Start recording".to_string(), on_click = on_start) },
        ui! { button(label = "Stop recording".to_string(), on_click = on_stop) },
    ];

    ui! {
        Stack(gap = StackGap::Md, padding = StackPadding::Lg) { body }
    }
}
