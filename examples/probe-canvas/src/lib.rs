//! `probe-canvas` — the thread-B experiment.
//!
//! Instead of registering canvas-vello's handler in the backend's
//! `ExternalRegistry` (a main-resident `Rc<dyn Fn>` dispatched via
//! `call_indirect`, which anchors the renderer in `main.wasm`), the lazy chunk
//! calls the handler DIRECTLY — `canvas_vello::build_canvas` — using the ambient
//! `WebBackend`. That reference lives only in the chunk body (a `#[wasm_split]`
//! boundary), and main never registers or dispatches the handler. So
//! canvas-vello (→ vello → wgpu → naga) should be reachable ONLY from the chunk.
//!
//! Measurement: compare `main.wasm` against `examples/lazy-canvas` (registry
//! path) and `examples/eager-canvas` (main-registered). If main drops toward the
//! floor here, the registry `call_indirect` was the anchor.

use runtime_core::{lazy, ui, Element, IntoElement};

pub fn app() -> Element {
    ui! {
        view {
            text { "probe: in-chunk canvas build (no registry)" }
            {
                lazy! {
                    // The handler reference lives ONLY here, in the chunk. It's
                    // reachable for the linker (compiled into the chunk) whether
                    // or not the ambient backend is present at runtime — that's
                    // all the size probe needs.
                    #[cfg(target_arch = "wasm32")]
                    let _node = backend_web::with_ambient_backend(|b| {
                        canvas_vello::build_canvas(&probe_props(), b)
                    });
                    ui! { view { text { "in-chunk canvas (probe)" } } }
                }
                .placeholder(|| ui! { text { "loading…" } })
                .into_element()
            }
        }
    }
}

/// Build the props the in-chunk handler renders. Cross-platform (canvas-core).
#[cfg(target_arch = "wasm32")]
fn probe_props() -> std::rc::Rc<canvas::prelude::CanvasProps> {
    use canvas::prelude::*;
    std::rc::Rc::new(CanvasProps {
        draw: canvas::draw(|s: &mut Scene| {
            s.path().add_path(Path::rect(0.0, 0.0, 300.0, 200.0));
            s.fill(Color::new(40, 120, 240, 255));
        }),
        ..Default::default()
    })
}

/// No registration — the handler is reached only from the chunk (see `app`).
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}
