//! Browser shell for the wgpu render backend.
//!
//! Sits behind a single async `mount(...)` entry. The caller is
//! whoever already owns a framework `Graphics` surface (the docs
//! Simulator component, future preview embeds, an `idealyst dev`
//! web-host shim). On `Graphics` `on_ready` they pass the surface +
//! a build closure here and forget about wgpu, pointer translation,
//! or render-loop bookkeeping.
//!
//! What this crate does, in order:
//!
//! 1. Extracts the underlying `HtmlCanvasElement` from the framework's
//!    `GraphicsSurface` (via `raw_window_handle::WebCanvasWindowHandle`'s
//!    JS-value pointer).
//! 2. Runs the async wgpu init — instance, adapter, device, queue,
//!    surface configure. WebGL2-only — wgpu 22 unconditionally
//!    serializes `maxInterStageShaderComponents` into `requestDevice`,
//!    and modern Chrome rejects any non-undefined value for that
//!    field (the WebGPU spec removed it). The GL backend dodges the
//!    WebGPU code path entirely.
//! 3. Builds the `render_wgpu::Host` + `Renderer` and mounts the
//!    caller's `build_ui` closure.
//! 4. Starts a `runtime_core::driver::render_loop` that draws one
//!    frame per `rAF` tick + advances the host's tween / momentum
//!    / caret state via `Host::tick`.
//! 5. Attaches `pointerdown` / `pointermove` / `pointerup` /
//!    `pointercancel` / `wheel` listeners to the canvas, translates
//!    each event to the `render_api` vocabulary (canvas-relative CSS
//!    px → logical px), and hands them to the host through
//!    `EventSink`. This is the parallel to `host-winit`'s
//!    `WindowEvent → EventSink` translation.
//!
//! The returned [`WebHostHandle`] owns everything: drop it (or call
//! [`WebHostHandle::resize`] on resize) from the caller's lifecycle
//! callbacks. `Drop` removes the JS listeners, cancels the render
//! loop, releases the wgpu surface + device + queue, and tears down
//! the host (which clears every reactive scope the embedded app
//! built).

#![allow(clippy::new_without_default)]

#[cfg(target_arch = "wasm32")]
mod overlay;
#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(target_arch = "wasm32")]
pub use web::{mount, MountError, WebHostHandle};

// Re-export the shapes consumers most often need so they can depend
// on `host-web` alone and not also pull `render-wgpu` / `render-api`
// directly. The Simulator-style consumer only needs `Painter` and
// `DeviceProfile`; richer integrations can still drop down to
// `render_wgpu::*` and `render_api::*` directly.
pub use render_api::DeviceProfile;
pub use render_wgpu::Painter;

// On non-wasm targets the crate is empty — the public surface
// requires `wasm-bindgen` / `web-sys` types that don't exist there.
// We leave it buildable so the workspace's `cargo check` from any
// host platform doesn't trip; anyone calling `mount` from non-wasm
// gets a compile error at the use site.
#[cfg(not(target_arch = "wasm32"))]
mod stub {
    // Intentionally empty.
}
