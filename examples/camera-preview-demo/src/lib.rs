//! `camera-preview-demo` — the phase-2 `MediaStream` consumer path end to
//! end: the `camera` SDK yields a `MediaStream`, and the `video` SDK
//! *displays* it. On web the stream's native source (a `web_sys::MediaStream`)
//! is attached to the `<video>` element's `srcObject` — zero copy, no canvas
//! pump — the developer never names a platform type.
//!
//! Press **Start camera** → `Camera::open()` resolves a `MediaStream`, which
//! is stashed in a signal; a reactive `when(..)` then mounts
//! `Video(source = stream)` to show the live feed.

use camera::{Camera, CameraConfig, CameraError, MediaStream};
use idea_ui::{install_idea_theme, light_theme, Stack, StackGap, StackPadding, Typography};
use runtime_core::{
    signal, text, ui, view, Element, IntoElement, Length, Signal, StyleRules, StyleSheet,
};
use std::rc::Rc;

// Register the `video` external handler per platform — the CLI-generated
// wrapper hands us the concrete backend. Needed on every platform we want to
// display a stream on (the `Video` external isn't registered otherwise).

#[cfg(target_arch = "wasm32")]
pub fn register_extensions(backend: &mut backend_web::WebBackend) {
    video::register(backend);
}

#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub fn register_extensions(backend: &mut backend_ios::IosBackend) {
    video::register(backend);
}

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub fn register_extensions(backend: &mut backend_android::AndroidBackend) {
    video::register(backend);
}

#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
pub fn register_extensions(backend: &mut backend_macos::MacosBackend) {
    video::register(backend);
}

#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "ios",
    target_os = "android",
    target_os = "macos"
)))]
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(_backend: &mut dev_server::WireRecordingBackend) {}

pub fn app() -> Element {
    install_idea_theme(light_theme());

    // The live source, once opened. `MediaStream` is `Clone` (Rc); the signal
    // holds it (keeping capture alive) and the `Video` clones it to display.
    let stream_sig: Signal<Option<MediaStream>> = signal!(None);
    let status: Signal<String> = signal!("Idle — press Start camera".to_string());
    let started: Signal<bool> = signal!(false);

    let status_text = text(move || status.get()).into_element();

    // Always-mounted Video with a REACTIVE stream source: `stream(|| ..)`'s
    // `resolve()` reads `stream_sig`, so when the camera opens and sets the
    // signal, the video re-populates with no remount.
    //
    // The Video is an `Element::External` with NO intrinsic size — on native
    // (iOS UIView / Android FrameLayout) it lays out at main-axis size 0 and
    // collapses, exactly like the `graphics` primitive does. So we give it an
    // explicit size: a fixed-height box, with the Video filling it. (On web
    // the `<video>` had an intrinsic size, so this wasn't needed there.)
    let fill = StyleRules {
        width: Some(Length::pct(100.0).into()),
        height: Some(Length::pct(100.0).into()),
        ..Default::default()
    };
    let box_rules = StyleRules {
        width: Some(Length::pct(100.0).into()),
        height: Some(Length::Px(300.0).into()),
        ..Default::default()
    };
    let preview = view(vec![video::Video(video::VideoProps {
        source: video::stream(move || stream_sig.get()),
        autoplay: true,
        ..Default::default()
    })
    .with_style(Rc::new(StyleSheet::r#static(fill)))
    .into_element()])
    .with_style(Rc::new(StyleSheet::r#static(box_rules)))
    .into_element();

    let on_start = move || {
        if started.get() {
            return;
        }
        started.set(true);
        status.set("Requesting camera…".to_string());
        runtime_core::driver::spawn_async(async move {
            match Camera::new().open(CameraConfig::default()).await {
                Ok(stream) => {
                    status.set("Live — camera feed via Video(source = stream)".to_string());
                    stream_sig.set(Some(stream));
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
        ui! { Typography(content = "Camera → Video".to_string(), kind = idea_ui::typography_kind::H1) },
        ui! {
            Typography(
                content = "The camera SDK yields a `MediaStream`; the video SDK displays it. \
                    On web that's a zero-copy `<video srcObject>` — no platform types in app code."
                    .to_string(),
                muted = true,
            )
        },
        status_text,
        preview,
        ui! { button(label = "Start camera".to_string(), on_click = on_start) },
    ];

    ui! {
        Stack(gap = StackGap::Md, padding = StackPadding::Lg) { body }
    }
}
