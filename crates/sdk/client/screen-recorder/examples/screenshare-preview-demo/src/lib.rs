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

/// Web registration seam — registry-CONCRETE. `video::register` takes a
/// `Registry<WebBackend>` on wasm32 (the real `<video>` handler has no
/// caps-trait expression), so the seam is specialized to that registry here.
///
/// Registration is MANDATORY for anything the tree renders: an unregistered
/// payload panics at realize.
#[cfg(target_arch = "wasm32")]
pub fn register_scene_extensions(registry: &mut runtime_scene::Registry<backend_web::WebBackend>) {
    video::register(registry);
    screen_recorder::register(registry);
}

/// Native registration seam. Both SDKs are registry-GENERIC off web and
/// type-dispatch ONCE at registration: `video::register` downcasts to the
/// macOS / iOS / Android registry and installs the real player, and
/// `screen_recorder::register` installs the capture-excluded overlay
/// window where the platform has one (passthrough container elsewhere).
#[cfg(not(target_arch = "wasm32"))]
pub fn register_scene_extensions<H>(registry: &mut runtime_scene::Registry<H>)
where
    H: runtime_vocabulary::caps::ExternalOps
        + runtime_vocabulary::style_attach::StyleServices
        + 'static,
{
    video::register(registry);
    screen_recorder::register(registry);
}

/// Android entry: the generated wrapper's `attach` mounts `scene_app()`
/// through `backend_android::newcore::start`.
pub fn scene_app() -> Element {
    app()
}

pub fn app() -> Element {
    install_idea_theme(light_theme());

    // The live source, once capture starts. `MediaStream` is `Clone` (Rc); the
    // signal holds it (keeping capture alive) and the `Video` clones it to
    // display.
    // `MediaStream` compares by pointer identity (see its `PartialEq`), so
    // `Option<MediaStream>` is directly a legal signal payload: the guarded
    // `set` stays quiet only when the SAME stream is stored again.
    let stream_sig: Signal<Option<MediaStream>> = signal(None);
    let status: Signal<String> = signal("Idle — press Start screen share".to_string());
    let started: Signal<bool> = signal(false);

    // Always-mounted Video with a REACTIVE stream source: `stream(|| ..)`'s
    // `resolve()` reads `stream_sig`, so when capture starts and sets the
    // signal, the video re-populates with no remount.
    //
    // The Video is a handler-backed scene payload with NO intrinsic size — on native
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
        Stack(gap = StackGap::Md, padding = StackPadding::Lg) {
            Typography(content = "Screen → Video".to_string(), kind = idea_ui::typography_kind::H1)
            Typography(
                content = "The screen-recorder SDK yields a `MediaStream`; the video SDK displays \
                    it. On iOS that's ReplayKit in-app capture — so you'll see a recursive mirror \
                    of this very screen, which is what proves the path works."
                    .to_string(),
                muted = true,
            )
            text { move || status.get() }
            preview
            button(label = "Start screen share".to_string(), on_click = on_start)
        }
    };
    view(vec![page, private_layer])
        .with_style(Rc::new(StyleSheet::r#static(fill_root)))
        .into_element()
}
