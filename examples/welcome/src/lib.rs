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
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

// Recorder-side registration for the runtime-server sidecar. Gated by
// `sidecar` (set only by the generated sidecar wrapper) so device/web
// builds never pull `dev-server`.
#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(_backend: &mut dev_server::WireRecordingBackend) {
    // No SDK navigator/external needs recorder-side registration in this app.
}
