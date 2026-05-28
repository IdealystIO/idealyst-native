//! Demo for `idealyst build --web` code-splitting (dioxus wasm-split).
//!
//! The shell `Text` lives in the main bundle. The `lazy! { … }` body runs
//! ARBITRARY bindgen-heavy code — real web-sys creating a WebGL2 canvas
//! (the exact object/Result-returning web-sys that can't go in a PIC
//! dynamic-link side) — to prove the dioxus splitter handles arbitrary code
//! in the chunk uniformly. The web-sys code is reachable only from the lazy
//! body, so the splitter hoists it (+ its glue) into the on-demand chunk.

use runtime_core::{lazy, ui, Element, IntoElement};

pub fn app() -> Element {
    ui! {
        View {
            Text { "Always loaded shell (main bundle)" }
            {
                lazy! {
                    {
                        gpu_preview();
                        ui! {
                            View {
                                Text { "lazy chunk: WebGL2 canvas created via web-sys" }
                            }
                        }
                    }
                }
                .into_element()
            }
        }
    }
}

/// Arbitrary bindgen-heavy code that lives ONLY in the lazy body (so the
/// wasm-split chunk owns it): create a `<canvas>`, get a WebGL2 context, and
/// clear it to a color. This is the object/`Result`-returning web-sys surface
/// that wgpu uses and that broke the PIC dynamic-link approach.
#[cfg(target_arch = "wasm32")]
fn gpu_preview() {
    use wasm_bindgen::JsCast;
    let Some(win) = web_sys::window() else { return };
    let Some(doc) = win.document() else { return };
    let Some(slot) = doc.get_element_by_id("gpu-slot") else { return };
    let Ok(el) = doc.create_element("canvas") else { return };
    let Ok(canvas) = el.dyn_into::<web_sys::HtmlCanvasElement>() else { return };
    canvas.set_width(240);
    canvas.set_height(160);
    let _ = canvas.set_attribute("id", "gpu-canvas");
    if slot.append_child(&canvas).is_err() {
        return;
    }
    let Ok(Some(ctx)) = canvas.get_context("webgl2") else { return };
    let Ok(gl) = ctx.dyn_into::<web_sys::WebGl2RenderingContext>() else { return };
    gl.clear_color(0.10, 0.55, 0.85, 1.0);
    gl.clear(web_sys::WebGl2RenderingContext::COLOR_BUFFER_BIT);
}

#[cfg(not(target_arch = "wasm32"))]
fn gpu_preview() {}

/// SDK-handler registration hook the CLI-generated wrapper invokes before
/// mount. This demo registers no third-party SDKs.
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}
