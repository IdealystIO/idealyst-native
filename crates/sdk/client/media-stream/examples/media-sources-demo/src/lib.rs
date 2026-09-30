//! `media-sources-demo` — two `Video` components, two live-stream
//! producers, one abstraction.
//!
//! - **Camera** comes from the `camera` SDK (`Camera::open() -> MediaStream`).
//! - **Screen share** comes from an inline producer (web `getDisplayMedia()`
//!   wrapped in a `MediaStream` whose `native_source` is the resulting
//!   `web_glue::dom::MediaStream`).
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
use runtime_core::{signal, ui, Element, IntoElement, Signal};

/// SDK-handler registration seam, called by `idealyst::entry!` at boot.
/// There is no inventory self-registration on the scene registry — an
/// UNREGISTERED payload panics at realize — so the `video` handler MUST be
/// composed in here. `video::register` dispatches on the registry TYPE at
/// registration time (the `<video>` renderer on web, the native player on
/// macOS / iOS / Android), so one generic seam covers every target.
pub fn register_scene_extensions<H>(registry: &mut runtime_scene::Registry<H>)
where
    H: runtime_vocabulary::caps::ExternalOps
        + runtime_vocabulary::style_attach::StyleServices
        + 'static,
{
    video::register(registry);
}

/// Recorder-side seam for the runtime-server sidecar
/// (`dev_server::sidecar::run_newcore`).
#[cfg(feature = "sidecar")]
pub fn register_scene_extensions_recorder(registry: &mut dev_server::newcore::SceneRegistry) {
    video::register(registry);
}

/// Android entry: the generated wrapper's `attach` mounts `scene_app()`
/// through `backend_android::newcore::start`.
pub fn scene_app() -> Element {
    app()
}

pub fn app() -> Element {
    install_idea_theme(light_theme());

    // One stream signal per source. Each `Video` reads its own via a reactive
    // `stream(..)` source, so flipping the signal populates that video in
    // place — no remount.
    //
    // `MediaStream` compares by pointer identity (see its `PartialEq`), so
    // `Option<MediaStream>` is directly a legal signal payload: every fresh
    // capture is a distinct instance and notifies, while re-storing the same
    // stream is correctly swallowed by the guard.
    let cam_sig: Signal<Option<MediaStream>> = signal(None);
    let screen_sig: Signal<Option<MediaStream>> = signal(None);
    let cam_status: Signal<String> = signal("idle".to_string());
    let screen_status: Signal<String> = signal("idle".to_string());

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

    // `video::Video` is a builder fn, not a `#[component]`, so the two
    // players are built here and splatted in as children.
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

    ui! {
        Stack(gap = StackGap::Md, padding = StackPadding::Lg) {
            Typography(content = "Camera + Screen share → Video".to_string(), kind = typography_kind::H1)
            Typography(
                content = "Two Video components, each fed a live MediaStream from a different \
                    producer — a camera and a screen share. Same component, same `source` prop; \
                    only the producer differs."
                    .to_string(),
                muted = true,
            )
            // --- Camera ---
            Typography(content = "Camera".to_string(), kind = typography_kind::H2)
            text { move || format!("Camera: {}", cam_status.get()) }
            cam_video
            button(label = "Start camera".to_string(), on_click = on_camera)
            // --- Screen share ---
            Typography(content = "Screen share".to_string(), kind = typography_kind::H2)
            text { move || format!("Screen: {}", screen_status.get()) }
            screen_video
            button(label = "Start screen share".to_string(), on_click = on_screen)
        }
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
// `MediaStream`, bound here through web-glue (the framework-owned JS boundary)
// as a `web_glue::dom::MediaStream` — the type every media consumer
// downcasts. We wrap it in a platform-agnostic `MediaStream` and publish it
// as the `native_source` so the `video` SDK attaches it as
// `<video>.srcObject` — the exact same consumer path the camera uses.
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
web_glue::import! {
    // `navigator.mediaDevices.getDisplayMedia()` → its Promise.
    #[catch]
    fn js_get_display_media() -> u32 = "() => G.add(navigator.mediaDevices.getDisplayMedia())";
}

#[cfg(target_arch = "wasm32")]
async fn open_screen_share() -> Result<MediaStream, String> {
    use web_glue::dom::{MediaStream as WebMediaStream, MediaStreamTrack};
    use web_glue::{JsCast, JsFuture, JsValue};

    // SAFETY: the result is a fresh Promise handle.
    let promise = unsafe { js_get_display_media() }
        .map(|p| unsafe { JsValue::from_raw(p) })
        .map_err(|e| format!("getDisplayMedia: {}", e.message()))?;
    let value = JsFuture::new(&promise)
        .await
        .map_err(|_| "permission denied / cancelled".to_string())?;
    let web_ms: WebMediaStream = value
        .dyn_into()
        .map_err(|_| "getDisplayMedia did not return a MediaStream".to_string())?;

    // Wrap it as a platform-agnostic MediaStream. No CPU frames are pushed —
    // the consumer uses the native_source for zero-copy display. The handle
    // clone is the same JS stream the stopper below ends.
    let (stream, _writer) = MediaStream::new();
    stream.set_native_source(std::rc::Rc::new(web_ms.clone()));
    // Stop the OS capture when the last stream clone drops.
    stream.attach_stopper(move || {
        for track in web_ms.get_tracks().iter() {
            track.unchecked_into::<MediaStreamTrack>().stop();
        }
    });
    Ok(stream)
}

#[cfg(not(target_arch = "wasm32"))]
async fn open_screen_share() -> Result<MediaStream, String> {
    Err("screen share is web-only in this demo".to_string())
}
