//! `media-sources-demo` — two `Video` components, two live-stream
//! producers, one abstraction.
//!
//! - **Camera** comes from the `camera` SDK (`Camera::open() -> MediaStream`).
//! - **Screen share** comes from an inline producer (web `getDisplayMedia()`
//!   wrapped in a `MediaStream` whose `native_source` is the resulting
//!   `web_sys::MediaStream`).
//!
//! Both feed the *same* `Video(source = stream(..))` component — the only
//! difference is which producer made the stream. That's the point: `Video`
//! consumes a platform-agnostic `MediaStream`; it doesn't care who produced
//! it. (The "proper" home for screen capture is the `screen-recorder` SDK,
//! which yields a `MediaStream` the same way — wired here inline to keep the
//! demo self-contained.)

use camera::{Camera, CameraConfig, CameraError};
use idea_ui::{install_idea_theme, light_theme, typography_kind, Stack, StackGap, StackPadding, Typography};
use media_stream::MediaStream;
use runtime_core::{signal, text, ui, Element, IntoElement, Signal};

/// No per-platform registration needed: the `video` external self-registers
/// via `inventory::submit!` at backend construction (see
/// [[project_inventory_self_registration]]). The crate stays linked through
/// the `video::Video` references in `app()`.
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(_backend: &mut dev_server::WireRecordingBackend) {}

pub fn app() -> Element {
    install_idea_theme(light_theme());

    // One stream signal per source. Each `Video` reads its own via a reactive
    // `stream(..)` source, so flipping the signal populates that video in
    // place — no remount.
    let cam_sig: Signal<Option<MediaStream>> = signal!(None);
    let screen_sig: Signal<Option<MediaStream>> = signal!(None);
    let cam_status: Signal<String> = signal!("idle".to_string());
    let screen_status: Signal<String> = signal!("idle".to_string());

    let cam_video = video::Video(video::VideoProps {
        source: video::stream(move || cam_sig.get()),
        autoplay: true,
        ..Default::default()
    })
    .into_element();

    let screen_video = video::Video(video::VideoProps {
        source: video::stream(move || screen_sig.get()),
        autoplay: true,
        ..Default::default()
    })
    .into_element();

    let cam_status_text =
        text(move || format!("Camera: {}", cam_status.get())).into_element();
    let screen_status_text =
        text(move || format!("Screen: {}", screen_status.get())).into_element();

    let on_camera = move || {
        cam_status.set("requesting…".to_string());
        runtime_core::driver::spawn_async(async move {
            match Camera::new().open(CameraConfig::default()).await {
                Ok(stream) => {
                    cam_status.set("live".to_string());
                    cam_sig.set(Some(stream));
                }
                Err(e) => cam_status.set(camera_error(e)),
            }
        });
    };

    let on_screen = move || {
        screen_status.set("requesting…".to_string());
        runtime_core::driver::spawn_async(async move {
            match open_screen_share().await {
                Ok(stream) => {
                    screen_status.set("live".to_string());
                    screen_sig.set(Some(stream));
                }
                Err(e) => screen_status.set(e),
            }
        });
    };

    let body: Vec<Element> = vec![
        ui! { Typography(content = "Camera + Screen share → Video".to_string(), kind = typography_kind::H1) },
        ui! {
            Typography(
                content = "Two Video components, each fed a live MediaStream from a different \
                    producer — a camera and a screen share. Same component, same `source` prop; \
                    only the producer differs."
                    .to_string(),
                muted = true,
            )
        },
        // --- Camera ---
        ui! { Typography(content = "Camera".to_string(), kind = typography_kind::H2) },
        cam_status_text,
        cam_video,
        ui! { button(label = "Start camera".to_string(), on_click = on_camera) },
        // --- Screen share ---
        ui! { Typography(content = "Screen share".to_string(), kind = typography_kind::H2) },
        screen_status_text,
        screen_video,
        ui! { button(label = "Start screen share".to_string(), on_click = on_screen) },
    ];

    ui! {
        Stack(gap = StackGap::Md, padding = StackPadding::Lg) { body }
    }
}

fn camera_error(e: CameraError) -> String {
    match e {
        CameraError::PermissionDenied => "permission denied".to_string(),
        CameraError::NoCamera => "no camera".to_string(),
        CameraError::Unsupported => "unsupported on this platform".to_string(),
        other => format!("error: {other}"),
    }
}

// ---------------------------------------------------------------------------
// Inline screen-share producer. On web, `getDisplayMedia()` yields a
// `web_sys::MediaStream`; we wrap it in a platform-agnostic `MediaStream`
// and publish it as the `native_source` so the `video` SDK attaches it as
// `<video>.srcObject` — the exact same consumer path the camera uses.
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
async fn open_screen_share() -> Result<MediaStream, String> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;

    let window = web_sys::window().ok_or("no window")?;
    let devices = window
        .navigator()
        .media_devices()
        .map_err(|_| "no mediaDevices".to_string())?;
    let promise = devices
        .get_display_media()
        .map_err(|e| format!("getDisplayMedia: {e:?}"))?;
    let value = JsFuture::from(promise)
        .await
        .map_err(|_| "permission denied / cancelled".to_string())?;
    let web_ms: web_sys::MediaStream = value
        .dyn_into()
        .map_err(|_| "getDisplayMedia did not return a MediaStream".to_string())?;

    // Wrap it as a platform-agnostic MediaStream. No CPU frames are pushed —
    // the consumer uses the native_source for zero-copy display.
    let (stream, _writer) = MediaStream::new();
    stream.set_native_source(std::rc::Rc::new(web_ms.clone()));
    // Stop the OS capture when the last stream clone drops.
    stream.attach_stopper(move || {
        for track in web_ms.get_tracks().iter() {
            if let Ok(track) = track.dyn_into::<web_sys::MediaStreamTrack>() {
                track.stop();
            }
        }
    });
    Ok(stream)
}

#[cfg(not(target_arch = "wasm32"))]
async fn open_screen_share() -> Result<MediaStream, String> {
    Err("screen share is web-only in this demo".to_string())
}
