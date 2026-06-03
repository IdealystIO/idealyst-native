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
    signal, text, view, ui, Color, Element, IntoElement, Length, Position, Signal, StyleRules,
    StyleSheet, Tokenized,
};
use screen_recorder::{MediaStream, PrivateLayer, RecorderError, RecordingConfig, ScreenRecorder};
use std::rc::Rc;

// Register the per-platform external handlers — the CLI-generated
// wrapper hands us the concrete backend. `video::register` is needed to
// display the captured stream; `screen_recorder::register` installs the
// `PrivateLayer` (the ReplayKit/PixelCopy-excluded overlay window) so
// the 🔴 REC badge renders but stays out of the recording.

#[cfg(target_arch = "wasm32")]
pub fn register_extensions(backend: &mut backend_web::WebBackend) {
    video::register(backend);
    screen_recorder::register(backend);
}

#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub fn register_extensions(backend: &mut backend_ios::IosBackend) {
    video::register(backend);
    screen_recorder::register(backend);
}

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub fn register_extensions(backend: &mut backend_android::AndroidBackend) {
    video::register(backend);
    screen_recorder::register(backend);
}

#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
pub fn register_extensions(backend: &mut backend_macos::MacosBackend) {
    video::register(backend);
    // macOS private-layer exclusion (SCContentFilter) isn't wired yet;
    // `register` installs the inline no-op so the call is uniform.
    screen_recorder::register(backend);
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

    // The 🔴 REC badge lives inside a `PrivateLayer` — on iOS/Android it
    // renders in a separate, capture-excluded overlay window, so the
    // user sees it but it does NOT appear in the recorded `MediaStream`
    // (proving the private layer works: the Video preview above is the
    // recording, and the badge must be absent from it). The badge is
    // absolutely positioned top-right inside the full-screen layer.
    let badge_sheet = StyleRules {
        position: Some(Position::Absolute),
        top: Some(Length::Px(48.0).into()),
        right: Some(Length::Px(16.0).into()),
        background: Some(Tokenized::Literal(Color("rgba(220, 38, 38, 0.92)".into()))),
        padding_top: Some(Length::Px(6.0).into()),
        padding_bottom: Some(Length::Px(6.0).into()),
        padding_left: Some(Length::Px(12.0).into()),
        padding_right: Some(Length::Px(12.0).into()),
        border_top_left_radius: Some(Length::Px(14.0).into()),
        border_top_right_radius: Some(Length::Px(14.0).into()),
        border_bottom_left_radius: Some(Length::Px(14.0).into()),
        border_bottom_right_radius: Some(Length::Px(14.0).into()),
        ..Default::default()
    };
    let rec_badge = view(vec![text("🔴 REC").into_element()])
        .with_style(Rc::new(StyleSheet::r#static(badge_sheet)))
        .into_element();
    let private_layer = PrivateLayer(vec![rec_badge]).into_element();

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

    // Root view holds the page Stack plus the PrivateLayer. On native
    // the PrivateLayer escapes into its own (capture-excluded) window —
    // its position in the tree is irrelevant — but keeping it a sibling
    // of the page content keeps the author model uniform across
    // backends (on web it would render inline as a DOM sibling).
    let fill_root = StyleRules {
        width: Some(Length::pct(100.0).into()),
        height: Some(Length::pct(100.0).into()),
        ..Default::default()
    };
    let page = ui! {
        Stack(gap = StackGap::Md, padding = StackPadding::Lg) { body }
    };
    view(vec![page, private_layer])
        .with_style(Rc::new(StyleSheet::r#static(fill_root)))
        .into_element()
}
