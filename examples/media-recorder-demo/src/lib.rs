//! `media-recorder-demo` — records **camera video + microphone audio** to an
//! `.mp4` with the `media-writer` SDK.
//!
//! Press **Record** → the app opens a camera [`MediaStream`](camera::MediaStream)
//! and a microphone [`AudioStream`](microphone::AudioStream), hands both to
//! [`MediaWriter::record`], and the writer muxes them — lip-synced by the
//! shared capture clock — into `recordings/clip.mp4`. Press **Stop** to finalize
//! the file; the written path is shown.
//!
//! This is the producer→writer pipeline end to end: two independent capture
//! SDKs produce streams over the shared `media-stream` abstraction, and one
//! consumer SDK writes them to disk, with no platform types named by the app.

use std::cell::RefCell;
use std::rc::Rc;

use camera::{Camera, CameraConfig};
use idea_ui::{
    install_idea_theme, light_theme, Stack, StackGap, StackPadding, Typography,
};
use media_writer::{MediaInputs, MediaWriter, RecordConfig};
use microphone::{AudioStreamConfig, Microphone};
use runtime_core::{signal, text, ui, Element, IntoElement, Signal};

/// The live capture + recording held for the duration of one recording. The
/// camera `MediaStream` and mic `AudioStream` MUST stay alive while recording —
/// the writer's subscriptions keep the *taps* open, but the streams' own
/// stoppers (which keep *capture* running) live with these handles.
struct Active {
    _camera: media_writer::MediaStream,
    _mic: media_writer::AudioStream,
    recording: media_writer::Recording,
}

/// No `Element::External` SDKs to register — `camera`/`microphone`/`media-writer`
/// are capability crates, not rendered primitives.
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(_backend: &mut dev_server::WireRecordingBackend) {}

pub fn app() -> Element {
    install_idea_theme(light_theme());

    let status: Signal<String> = signal!("Idle — press Record".to_string());
    let recording: Signal<bool> = signal!(false);
    let saved: Signal<String> = signal!(String::new());
    let active: Rc<RefCell<Option<Active>>> = Rc::new(RefCell::new(None));

    let on_record = {
        let active = active.clone();
        move || {
            if recording.get() {
                return;
            }
            recording.set(true);
            saved.set(String::new());
            status.set("Opening camera + microphone…".to_string());
            let active = active.clone();
            runtime_core::driver::spawn_async(async move {
                let cam_stream = match Camera::new().open(CameraConfig::default()).await {
                    Ok(s) => s,
                    Err(e) => {
                        status.set(format!("Camera error: {e}"));
                        recording.set(false);
                        return;
                    }
                };
                let mic_stream = match Microphone::new()
                    .open_stream(AudioStreamConfig::default())
                    .await
                {
                    Ok(s) => s,
                    Err(e) => {
                        status.set(format!("Microphone error: {e}"));
                        recording.set(false);
                        return;
                    }
                };
                let store = match files::app_files("recordings") {
                    Ok(s) => s,
                    Err(e) => {
                        status.set(format!("Files error: {e}"));
                        recording.set(false);
                        return;
                    }
                };
                match MediaWriter::new()
                    .record(
                        MediaInputs::av(&cam_stream, &mic_stream),
                        RecordConfig::new(store, "clip.mp4"),
                    )
                    .await
                {
                    Ok(rec) => {
                        status.set("Recording — press Stop to save".to_string());
                        *active.borrow_mut() = Some(Active {
                            _camera: cam_stream,
                            _mic: mic_stream,
                            recording: rec,
                        });
                    }
                    Err(e) => {
                        status.set(format!("Recording error: {e}"));
                        recording.set(false);
                    }
                }
            });
        }
    };

    let on_stop = {
        let active = active.clone();
        move || {
            if !recording.get() {
                return;
            }
            let Some(act) = active.borrow_mut().take() else {
                recording.set(false);
                return;
            };
            status.set("Finalizing file…".to_string());
            runtime_core::driver::spawn_async(async move {
                let Active {
                    _camera,
                    _mic,
                    recording: rec,
                } = act;
                match rec.stop().await {
                    Ok(path) => {
                        saved.set(path.clone());
                        status.set(format!("Saved to {path}"));
                    }
                    Err(e) => status.set(format!("Finalize error: {e}")),
                }
                // Streams drop here, stopping capture now the file is written.
                drop(_camera);
                drop(_mic);
                recording.set(false);
            });
        }
    };

    let status_text = text(move || status.get()).into_element();
    let saved_text = text(move || {
        let p = saved.get();
        if p.is_empty() {
            "No file yet".to_string()
        } else {
            format!("Last file: {p}")
        }
    })
    .into_element();

    let body: Vec<Element> = vec![
        ui! { Typography(content = "Media Recorder".to_string(), kind = idea_ui::typography_kind::H1) },
        ui! {
            Typography(
                content = "Records your camera + microphone to an .mp4 with the \
                    `media-writer` SDK. Press Record (the OS prompts for camera \
                    and mic permission the first time), then Stop to save."
                    .to_string(),
                muted = true,
            )
        },
        status_text,
        saved_text,
        ui! { button(label = "Record".to_string(), on_click = on_record) },
        ui! { button(label = "Stop".to_string(), on_click = on_stop) },
    ];

    ui! {
        Stack(gap = StackGap::Md, padding = StackPadding::Lg) { body }
    }
}
