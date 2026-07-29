//! Smoke app: the canvas SDK on the new-core web boot — the
//! External-SDK wave's live-verification vehicle.
//!
//! Direct vocabulary-builder calls (no `ui!`/`jsx!` — the
//! newcore-web-smoke posture: this crate gates the layer *under* the
//! macro). The tree: a heading, a `canvas::Canvas` whose draw closure
//! READS a signal (bar count + hue), and a button that bumps the
//! signal. Clicking must repaint the canvas through the full new-core
//! chain: wrapped button callback → staged write → dispatch-site flush
//! → the SDK handler's world effect re-runs the author painter →
//! canvas-native's shared Canvas2D rasterizer replays the scene.
//!
//! A `[CANVAS-SMOKE]` console line reports the canvas element's
//! presence + backing-store size after boot so a driver can assert the
//! mount without pixel-reading.

use canvas::prelude::*;
use runtime_core::{Length, StyleRules, Tokenized};
use runtime_scene::Element;
use runtime_vocabulary::glue::IntoElement;
use runtime_vocabulary::{button, text, view};
use runtime_world::{signal, Signal};
use wasm_bindgen::prelude::*;

fn column() -> StyleRules {
    StyleRules {
        padding_top: Some(Tokenized::Literal(Length::Px(16.0))),
        padding_left: Some(Tokenized::Literal(Length::Px(16.0))),
        gap: Some(Tokenized::Literal(Length::Px(8.0))),
        ..StyleRules::default()
    }
}

fn canvas_box() -> StyleRules {
    StyleRules {
        width: Some(Tokenized::Literal(Length::Px(360.0))),
        height: Some(Tokenized::Literal(Length::Px(200.0))),
        ..StyleRules::default()
    }
}

/// The reactive painter: `bars.get()` INSIDE the draw closure is what
/// re-runs the SDK handler's repaint effect on every committed write.
fn bar_canvas(bars: Signal<u32>) -> Element {
    Canvas(CanvasProps {
        draw: draw(move |s: &mut Scene| {
            let n = bars.get().max(1);
            // Backdrop.
            s.path().add_path(Path::rect(0.0, 0.0, 360.0, 200.0));
            s.fill(Paint::solid(Color::new(15, 23, 42, 255)));
            // n bars, hue-stepped.
            for i in 0..n {
                let w = 360.0 / (n as f32) * 0.8;
                let x = 360.0 / (n as f32) * (i as f32) + w * 0.125;
                let h = 40.0 + 120.0 * ((i + 1) as f32 / n as f32);
                s.path().add_path(Path::rect(x, 200.0 - h, w, h));
                let g = (60 + (150 * (i + 1) / n) as u8).min(255);
                s.fill(Paint::solid(Color::new(56, g, 248, 255)));
            }
        }),
        ..Default::default()
    })
    .with_style(canvas_box())
    .into_element()
}

fn app() -> Element {
    let bars = signal(3u32);

    view()
        .style(column())
        .child(text().content("canvas on the new core"))
        .child(text().content(move || format!("bars = {}", bars.get())))
        .child(bar_canvas(bars))
        .child(button().label("Add bar").on_press(move || bars.set(bars.get() + 1)))
        .build()
}

#[wasm_bindgen(start)]
pub fn main() {
    if web_sys::window().is_none() {
        return;
    }
    console_error_panic_hook::set_once();
    backend_web::install_logger();
    backend_web::newcore::start_in(
        "#app",
        |registry| canvas_native::register(registry),
        app,
    );
    report();
}

/// Post-boot self-report: find the mounted `<canvas>` and log its
/// backing-store size (the rasterizer sizes it to CSS box × dpr on the
/// first replay).
fn report() {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let found = doc
        .query_selector("canvas[data-external-kind='canvas_core::CanvasProps']")
        .ok()
        .flatten();
    match found {
        Some(el) => {
            let w = el.get_attribute("width").unwrap_or_default();
            let h = el.get_attribute("height").unwrap_or_default();
            web_sys::console::log_1(&JsValue::from_str(&format!(
                "[CANVAS-SMOKE] mounted=true width={w} height={h}"
            )));
        }
        None => {
            web_sys::console::log_1(&JsValue::from_str("[CANVAS-SMOKE] mounted=false"));
        }
    }
}
