//! `welcome` — three-act cinematic intro driven by springs + tweens
//! + a raf-driven sun/vignette/planet pulse.
//!
//! - Act 1: "Welcome to Idealyst" rises into a light frame.
//! - Act 2: frame washes dark, a warm sun blooms from the top-right.
//! - Act 3: subtitle materializes below the shuffled-up headline.
//!
//! Each animated property is an `AnimatedValue` bound to a `Ref` via
//! [`AnimatedValue::bind`] (or `bind_color` / `bind_gradient_stop` /
//! `bind_text_color`). The framework owns all per-platform dispatch —
//! this library is pure platform-agnostic Rust. The entry point is
//! `src/main.rs`, one `idealyst::entry!(welcome)` line whose shell the
//! target triple selects; the iOS / Android wrappers the CLI still
//! generates depend on this crate as an `rlib`.

mod color;
#[macro_use]
mod components;
mod app;
mod constants;
mod coordinator;
mod style_helpers;
mod typeface;

pub use app::app;

// Recorder-side registration seam for the runtime-server sidecar
// (`dev_server::sidecar::run_newcore`) — the recorder's scene-registry
// twin of `register_scene_extensions`. Gated by `sidecar` (set only by
// the generated sidecar wrapper) so device/web builds never pull
// `dev-server`. This app registers no third-party scene handlers.
#[cfg(feature = "sidecar")]
pub fn register_scene_extensions_recorder(_registry: &mut dev_server::newcore::SceneRegistry) {}

// SDK-handler registration seam, invoked by the CLI-generated wrappers
// (web `start_in`/`hydrate_in`, macOS `run_with`, iOS `run_in_view`,
// terminal `run`) after `runtime_vocabulary::register_builtins`.
// Registry-generic over the scene `Host` so ONE seam serves every
// backend (each wrapper's call site pins `H` to its concrete backend);
// a project that adds an SDK with a caps-generic handler (codeblock,
// table, markdown, …) calls its `register` here, and one with a
// backend-CONCRETE handler specializes `H` to that backend instead.
//
// Registration is mandatory for anything the tree renders: an
// unregistered payload panics at realize.
pub fn register_scene_extensions<H: runtime_scene::Host>(
    _registry: &mut runtime_scene::Registry<H>,
) {
}

// Android entry: the generated Android wrapper's `attach` mounts
// `scene_app()` through `backend_android::newcore::start` (see
// crates/tools/build/android). `app()` already returns the scene
// `Element`, so this is a plain re-export shim with the conventional
// name.
pub fn scene_app() -> runtime_core::Element {
    app()
}
