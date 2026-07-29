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
//! this project is pure platform-agnostic Rust; the per-target entry
//! points are in the wrapper crates the CLI generates at build time.

// idea-lite core migration: under `new-core` this alias shadows the
// extern-prelude `runtime-core` for the WHOLE crate, so the same source
// compiles against `runtime_vocabulary::glue`'s mirrors of the old
// author surface (the idea-ui / website pattern). The default build has
// no alias and is byte-identical old-core.
#[cfg(feature = "new-core")]
extern crate runtime_facade as runtime_core;

mod color;
#[macro_use]
mod components;
mod app;
mod constants;
mod coordinator;
mod style_helpers;
mod typeface;

pub use app::app;

// SDK-handler registration hook the CLI-generated wrappers invoke before
// mount. `welcome` registers no third-party SDKs, so it's an empty generic
// over `Backend` — backend-agnostic (no per-target `#[cfg]`, no `backend-*`
// dep), matching the scaffold's platform-agnostic app crate. The wrappers
// pass the concrete backend per platform (web/iOS by value, android via
// `&mut *b`), so `B` resolves to that backend. A project that adds a
// navigator / external SDK specializes this to that backend's concrete type.
// Old-core only: the `Backend` mega-trait doesn't exist on the new
// core — a new-core embedding passes its own register seam to the
// scene `Registry` instead (this app registers no third-party SDKs
// either way, so there is nothing to mirror).
#[cfg(not(feature = "new-core"))]
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

// Recorder-side registration for the runtime-server sidecar. Gated by
// `sidecar` (set only by the generated sidecar wrapper) so device/web
// builds never pull `dev-server`.
#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(_backend: &mut dev_server::WireRecordingBackend) {
    // No SDK navigator/external needs recorder-side registration in this app.
}

// New-core recorder registration seam for the `idealyst dev --new-core`
// sidecar wrapper (`dev_server::sidecar::run_newcore`) — the scene-
// registry twin of `register_extensions_recorder`, mirroring the web
// wrapper's `register_scene_extensions` convention. This app registers
// no third-party scene handlers.
#[cfg(all(feature = "sidecar", feature = "new-core"))]
pub fn register_scene_extensions_recorder(_registry: &mut dev_server::newcore::SceneRegistry) {}

// New-core scene-registry registration seam — the runtime-v2 twin of
// `register_extensions`, invoked by the CLI-generated new-core wrappers
// (web `start_in`/`hydrate_in`, macOS `newcore::run_with`, iOS
// `newcore::run_in_view`, terminal `newcore::run`) after
// `runtime_vocabulary::register_builtins`. Registry-generic over the
// scene `Host` so ONE seam serves every backend (each wrapper's call
// site pins `H` to its concrete backend); a project that adds an SDK
// with a caps-generic handler (codeblock, table, markdown, …) calls
// its `register` here, and one with a backend-CONCRETE handler
// specializes `H` to that backend instead.
#[cfg(feature = "new-core")]
pub fn register_scene_extensions<H: runtime_scene::Host>(
    _registry: &mut runtime_scene::Registry<H>,
) {
}

// New-core Android entry: the generated Android wrapper's `new-core`
// attach branch mounts `scene_app()` through
// `backend_android::newcore::start` (see crates/tools/build/android).
// Under the facade alias `app()` already returns the scene `Element`,
// so this is a plain re-export shim with the conventional name.
#[cfg(feature = "new-core")]
pub fn scene_app() -> runtime_core::Element {
    app()
}
