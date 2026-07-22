//! Split Canvas — a two-screen drawing app. See `app.rs`.
//!
//! Starting state for the arena `split-canvas` scenario: the GPU canvas engine
//! (the `canvas` SDK's vello renderer) is registered EAGERLY below, so on web
//! the whole vello/wgpu engine is statically reachable from `main.wasm` and
//! ships in the initial download — even though the canvas is only ever shown on
//! the Draw screen.

mod app;
pub use app::app;

// SDK-handler registration hook the CLI-generated wrappers invoke before mount.
//
// EAGER canvas registration (the naive shape this scenario starts from): calling
// `canvas_vello::register` at boot makes the vello GPU renderer — and the whole
// wgpu graphics stack it reaches — statically reachable from `main.wasm`, so it
// rides in the initial bundle regardless of whether the Draw screen is ever
// opened. `register` is generic over the concrete backend the wrapper passes.
pub fn register_extensions<B: runtime_core::RegisterExternal>(backend: &mut B) {
    #[cfg(target_arch = "wasm32")]
    canvas_vello::register(backend);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = backend;
}

// Recorder-side registration for the runtime-server sidecar. Gated by `sidecar`
// (set only by the generated sidecar wrapper) so device/web builds never pull
// `dev-server`.
#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(_backend: &mut dev_server::WireRecordingBackend) {}
