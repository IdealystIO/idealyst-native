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
    signal, ui, view, Element, IntoElement, Length, Signal, StyleRules, StyleSheet,
};
use std::rc::Rc;

/// Registration seam — one registry-GENERIC fn for every target.
/// `video::register` type-dispatches ONCE at registration: web gets the real
/// `<video>`, macOS / iOS / Android get the native player, every other host
/// gets the External placeholder. `camera` renders nothing, so it registers
/// nothing.
///
/// Registration is MANDATORY: an unregistered payload panics at realize.
pub fn register_scene_extensions<H>(registry: &mut runtime_scene::Registry<H>)
where
    H: runtime_vocabulary::caps::ExternalOps
        + runtime_vocabulary::style_attach::StyleServices
        + 'static,
{
    video::register(registry);
}

/// Runtime-server (sidecar) recorder seam: the wire recorder's registry gets
/// the External placeholder arm of `video::register`.
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

    // The live source, once opened. `MediaStream` is `Clone` (Rc); the signal
    // holds it (keeping capture alive) and the `Video` clones it to display.
    // `MediaStream` compares by pointer identity (see its `PartialEq`), so
    // `Option<MediaStream>` is directly a legal signal payload: the guarded
    // `set` stays quiet only when the SAME stream is stored again.
    let stream_sig: Signal<Option<MediaStream>> = signal(None);
    let status: Signal<String> = signal("Idle — press Start camera".to_string());
    let started: Signal<bool> = signal(false);

    // Always-mounted Video with a REACTIVE stream source: `stream(|| ..)`'s
    // `resolve()` reads `stream_sig`, so when the camera opens and sets the
    // signal, the video re-populates with no remount.
    //
    // The Video is a handler-backed scene payload with NO intrinsic size — on native
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

    ui! {
        Stack(gap = StackGap::Md, padding = StackPadding::Lg) {
            Typography(content = "Camera → Video".to_string(), kind = idea_ui::typography_kind::H1)
            Typography(
                content = "The camera SDK yields a `MediaStream`; the video SDK displays it. \
                    On web that's a zero-copy `<video srcObject>` — no platform types in app code."
                    .to_string(),
                muted = true,
            )
            text { move || status.get() }
            preview
            button(label = "Start camera".to_string(), on_click = on_start)
        }
    }
}
