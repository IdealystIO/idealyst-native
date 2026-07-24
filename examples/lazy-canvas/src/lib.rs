//! `lazy-canvas` — a minimal app that lazy-loads a vello (wgpu) canvas.
//!
//! Purpose: measure how much of `canvas-vello` (vello + wgpu + the font stack)
//! actually leaves `main.wasm` when the canvas is code-split into a chunk. The
//! renderer is registered from *inside* the lazy component's body (via
//! `defer_external_registration`), so wasm-split classifies its code as
//! chunk-only — the question is whether its large static DATA follows the code
//! out of main, or gets stranded there.
//!
//! Compare `main.wasm` size against `examples/baseline` (the framework floor).

use runtime_core::{component, ui, Element, IntoElement};

/// The lazily-split canvas screen. On web the body (and its transitive
/// vello/wgpu deps) ships as a separate chunk; on native it compiles inline.
#[component(lazy)]
fn LazyCanvas() -> Element {
    // Register the vello renderer from INSIDE the chunk so its
    // code is reachable only here — never statically from main.
    #[cfg(target_arch = "wasm32")]
    runtime_core::defer_external_registration::<backend_web::WebBackend, _>(|b| {
        canvas_vello::register(b);
    });
    canvas_screen()
}

/// A tiny always-loaded shell around the lazily-loaded canvas screen.
pub fn app() -> Element {
    ui! {
        view {
            text { "lazy canvas demo" }
            LazyCanvas(loading = || ui! { text { "loading canvas…" } })
        }
    }
}

/// The canvas itself: a fixed-size surface with a trivial drawing.
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

/// No eager SDK registration — the renderer registers itself lazily, inside the
/// chunk (see [`LazyCanvas`]). Keeping this empty is what lets the split work.
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}
