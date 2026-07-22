//! Task dashboard — a small multi-component app.
//!
//! `app()` builds a [`store::TaskStore`] (the single source of truth) and
//! hands it to [`components::dashboard::Dashboard`], which lays out the
//! Header (derived stats), the Toolbar (filter controls), and the
//! TaskList (the rows). Every component reads the SAME store, so a
//! change to the store's derived `visible` view flows to every consumer
//! without any of them being rebuilt.

mod app;
mod components;
mod store;
mod style_helpers;

pub use app::app;

// SDK-handler registration hook the CLI-generated wrappers invoke before
// mount. This app registers no third-party SDKs.
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

// Recorder-side registration for the runtime-server sidecar. Gated by
// `sidecar` (set only by the generated sidecar wrapper) so device/web
// builds never pull `dev-server`.
#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(_backend: &mut dev_server::WireRecordingBackend) {}
