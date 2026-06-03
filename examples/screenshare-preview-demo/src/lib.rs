//! `screenshare-preview-demo` — the `MediaStream` consumer path for the
//! *other* producer: the `screen-recorder` SDK yields a `MediaStream`, and the
//! `video` SDK *displays* it. Sibling of `camera-preview-demo`.
//!
//! On iOS the source is **ReplayKit in-app capture**: pressing **Start screen
//! share** records the app's own rendered screen. Because we then show that
//! stream in a `Video` inside the same app, you get a recursive "hall of
//! mirrors" — which is exactly what proves the capture → `MediaStream` →
//! display path works end to end on a real device. On web it's
//! `getDisplayMedia` (the browser's source picker), attached zero-copy to the
//! `<video>` element's `srcObject`.

use idea_ui::{install_idea_theme, light_theme, Stack, StackGap, StackPadding, Typography};
use runtime_core::{
    signal, text, view, ui, Element, IntoElement, Length, Signal, StyleRules, StyleSheet,
};
use screen_recorder::{MediaStream, RecorderError, RecordingConfig, ScreenRecorder};
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

pub fn app() -> Element {
    install_idea_theme(light_theme());

    // The live source, once capture starts. `MediaStream` is `Clone` (Rc); the
    // signal holds it (keeping capture alive) and the `Video` clones it to
    // display.
    let stream_sig: Signal<Option<MediaStream>> = signal!(None);
    let status: Signal<String> = signal!("Idle — press Start screen share".to_string());
    let started: Signal<bool> = signal!(false);

    let status_text = text(move || status.get()).into_element();

    // Always-mounted Video with a REACTIVE stream source: `stream(|| ..)`'s
    // `resolve()` reads `stream_sig`, so when capture starts and sets the
    // signal, the video re-populates with no remount.
    //
    // The Video is an `Element::External` with NO intrinsic size — on native
    // it lays out at main-axis size 0 and collapses. So we give it an explicit
    // size: a fixed-height box, with the Video filling it. (Same fix as
    // camera-preview-demo.)
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
        status.set("Requesting screen capture…".to_string());
        runtime_core::driver::spawn_async(async move {
            // `ThisApp` (the default) → ReplayKit in-app capture on iOS.
            match ScreenRecorder::new().start(RecordingConfig::new()).await {
                Ok(stream) => {
                    status.set("Live — screen feed via Video(source = stream)".to_string());
                    stream_sig.set(Some(stream));
                }
                Err(e) => {
                    started.set(false);
                    status.set(match e {
                        RecorderError::PermissionDenied => {
                            "Screen capture permission denied".to_string()
                        }
                        RecorderError::Unsupported => {
                            "Screen capture isn't supported on this platform".to_string()
                        }
                        RecorderError::UnsupportedSource(s) => {
                            format!("Source not available here: {s}")
                        }
                        other => format!("Error: {other}"),
                    });
                }
            }
        });
    };

    let body: Vec<Element> = vec![
        ui! { Typography(content = "Screen → Video".to_string(), kind = idea_ui::typography_kind::H1) },
        ui! {
            Typography(
                content = "The screen-recorder SDK yields a `MediaStream`; the video SDK displays \
                    it. On iOS that's ReplayKit in-app capture — so you'll see a recursive mirror \
                    of this very screen, which is what proves the path works."
                    .to_string(),
                muted = true,
            )
        },
        status_text,
        preview,
        ui! { button(label = "Start screen share".to_string(), on_click = on_start) },
    ];

    ui! {
        Stack(gap = StackGap::Md, padding = StackPadding::Lg) { body }
    }
}
