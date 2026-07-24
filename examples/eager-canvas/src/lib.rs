//! `eager-canvas` — the control app for the External-anchoring experiment.
//! Identical canvas to `examples/lazy-canvas`, but the renderer is registered
//! EAGERLY at boot (in `register_extensions`, main code) with no lazy split.
//! Diffing main.wasm vs lazy-canvas shows how much (if any) lazy registration
//! actually removes from main.

use runtime_core::{ui, Element, IntoElement};

pub fn app() -> Element {
    ui! {
        view {
            text { "eager canvas control" }
            { canvas_screen() }
        }
    }
}

fn canvas_screen() -> Element {
    use canvas::prelude::*;
    canvas::Canvas(CanvasProps {
        draw: canvas::draw(|s: &mut Scene| {
            s.path().add_path(Path::rect(0.0, 0.0, 300.0, 200.0));
            s.fill(Color::new(40, 120, 240, 255));
            s.path().add_path(Path::rect(60.0, 50.0, 180.0, 100.0));
            s.fill(Color::new(240, 200, 40, 255));
        }),
        ..Default::default()
    })
    .into_element()
}

/// Eager registration — anchors canvas-vello in main statically.
pub fn register_extensions<B: runtime_core::RegisterExternal>(backend: &mut B) {
    canvas_vello::register(backend);
}
