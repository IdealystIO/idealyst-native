//! `video-compose-demo` — real-time compositing, the "just the product" story
//! made visible.
//!
//! The `camera` SDK yields an **input** `MediaStream`. [`VideoPipeline`] overlays
//! a watermark image (bottom-right) plus a drawn brand bar (top-left) and emits a
//! **new output** `MediaStream`. Two live `video::Video` previews sit side by
//! side: the UNTOUCHED input and the composited output. The watermark is only on
//! the output — the input preview shows the raw camera — which is the whole point
//! of the SDK. A button drives the watermark opacity reactively (the pipeline
//! re-reads it every frame; no rebuild).
//!
//! macOS is the implemented compositor backend; on other targets the output
//! stream is live but empty for now (see the `video-compose` crate docs).

use camera::{Camera, CameraConfig, CameraError, MediaStream};
use canvas_core::{Color, ImageSource, Path};
use idea_ui::{install_idea_theme, light_theme, Stack, StackGap, StackPadding, Typography};
use runtime_core::{
    signal, text, ui, view, Element, IntoElement, Length, Signal, StyleRules, StyleSheet,
};
use std::rc::Rc;
use video_compose::{Corner, VideoPipeline};

/// A font for the text watermark (web has no system fonts, so it must be bundled).
static FONT: &[u8] =
    include_bytes!("../../../../../../../examples/welcome/fonts/Inter-Bold.ttf");

/// Web registration seam — registry-CONCRETE: `video::register` takes a
/// `Registry<WebBackend>` on wasm32. `camera` and the compositor render
/// nothing of their own (the compositor owns its GPU device), so the two
/// `Video` previews are the only payloads needing a handler.
///
/// Registration is MANDATORY: an unregistered payload panics at realize.
#[cfg(target_arch = "wasm32")]
pub fn register_scene_extensions(registry: &mut runtime_scene::Registry<backend_web::WebBackend>) {
    video::register(registry);
}

/// Native registration seam. `video::register` is registry-GENERIC off web
/// and type-dispatches ONCE at registration (macOS / iOS / Android get the
/// real player; every other host gets the External placeholder).
#[cfg(not(target_arch = "wasm32"))]
pub fn register_scene_extensions<H>(registry: &mut runtime_scene::Registry<H>)
where
    H: runtime_vocabulary::caps::ExternalOps
        + runtime_vocabulary::style_attach::StyleServices
        + 'static,
{
    video::register(registry);
}

/// Android entry: the generated wrapper's `attach` mounts `scene_app()`
/// through `backend_android::newcore::start`.
pub fn scene_app() -> Element {
    app()
}

/// A procedurally-built watermark: a soft magenta dot with a translucent core, so
/// the source-alpha blend (transparent PNG regions reading through) is visible on
/// the output. Real apps pass a decoded logo via `ImageSource::decode`.
fn make_watermark() -> ImageSource {
    let (w, h) = (88u32, 88u32);
    let (cx, cy, r) = (44.0f32, 44.0, 38.0);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let d = (dx * dx + dy * dy).sqrt();
            if d <= r {
                let edge = (r - d).clamp(0.0, 1.0); // ~1px anti-aliased rim
                let i = ((y * w + x) * 4) as usize;
                rgba[i] = 232;
                rgba[i + 1] = 46;
                rgba[i + 2] = 150;
                rgba[i + 3] = (edge * 210.0) as u8; // translucent, so it blends
            }
        }
    }
    ImageSource::from_rgba8(1, w, h, rgba)
}

/// A sized preview box wrapping a live-stream `Video`, filling its parent.
///
/// `MediaStream` compares by pointer identity (see its `PartialEq`), so
/// `Option<MediaStream>` is directly a legal signal payload — the guarded
/// `set` stays quiet only when the SAME stream is stored again.
fn preview(stream: Signal<Option<MediaStream>>) -> Element {
    let fill = StyleRules {
        width: Some(Length::pct(100.0).into()),
        height: Some(Length::pct(100.0).into()),
        ..Default::default()
    };
    let box_rules = StyleRules {
        width: Some(Length::pct(100.0).into()),
        height: Some(Length::Px(260.0).into()),
        ..Default::default()
    };
    view(vec![video::Video(video::VideoProps {
        source: video::stream(move || stream.get()),
        autoplay: true,
        ..Default::default()
    })
    .with_style(Rc::new(StyleSheet::r#static(fill)))
    .into_element()])
    .with_style(Rc::new(StyleSheet::r#static(box_rules)))
    .into_element()
}

pub fn app() -> Element {
    install_idea_theme(light_theme());

    let input_sig: Signal<Option<MediaStream>> = signal(None);
    let output_sig: Signal<Option<MediaStream>> = signal(None);
    let status: Signal<String> = signal("Idle — press Start camera".to_string());
    let started: Signal<bool> = signal(false);
    // Reactive watermark opacity — the pipeline re-reads it every composited frame.
    let opacity: Signal<f32> = signal(1.0);

    let input_preview = preview(input_sig);
    let output_preview = preview(output_sig);

    let on_start = move || {
        if started.get() {
            return;
        }
        started.set(true);
        status.set("Requesting camera…".to_string());
        runtime_core::driver::spawn_async(async move {
            match Camera::new().open(CameraConfig::default()).await {
                Ok(input) => {
                    // Build the pipeline: input → watermark + drawn brand bar → output.
                    // The input stream stays untouched; only `output` carries the ops.
                    let out = VideoPipeline::new(input.clone())
                        .watermark(make_watermark(), Corner::BottomRight, 18.0, move || opacity.get())
                        // A TEXT watermark — rasterized from the bundled font, so it
                        // shows on macOS AND web (unlike the `.draw()` glyph path).
                        .watermark_text(
                            "© 2026 idealyst",
                            FONT,
                            26.0,
                            Color::new(255, 255, 255, 210),
                            Corner::TopRight,
                            16.0,
                            move || opacity.get(),
                        )
                        .draw(|s| {
                            // A drawn "LIVE" bar in the top-left, ON TOP of the video
                            // (macOS only for now — `.draw()` isn't rendered on web).
                            s.path().add_path(Path::rounded_rect(14.0, 14.0, 92.0, 30.0, 8.0));
                            s.fill(Color::new(232, 46, 150, 220));
                        })
                        .build();
                    input_sig.set(Some(input));
                    output_sig.set(Some(out));
                    status.set("Live — left: input (untouched) · right: composited output".to_string());
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

    let on_toggle = move || opacity.set(if opacity.get() > 0.5 { 0.15 } else { 1.0 });

    ui! {
        Stack(gap = StackGap::Md, padding = StackPadding::Lg) {
            Typography(
                content = "Video compositing".to_string(),
                kind = idea_ui::typography_kind::H1,
            )
            Typography(
                content = "The camera yields an input stream; `video-compose` overlays a watermark \
                    + drawn label and emits a NEW output stream. The input is never touched — only \
                    the output (right) carries the ops."
                    .to_string(),
                muted = true,
            )
            text { move || status.get() }
            Typography(content = "Input (untouched)".to_string(), muted = true)
            input_preview
            Typography(content = "Output (watermarked)".to_string(), muted = true)
            output_preview
            button(label = "Start camera".to_string(), on_click = on_start)
            button(label = "Toggle watermark opacity".to_string(), on_click = on_toggle)
        }
    }
}
