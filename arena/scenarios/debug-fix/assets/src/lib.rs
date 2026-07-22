//! Todo app — see `app.rs`. (Arena debug-fix scenario starting state.)

mod app;
pub use app::app;

// SDK-handler registration hook the CLI-generated wrappers invoke before
// mount. This app registers no third-party SDKs.
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(_backend: &mut dev_server::WireRecordingBackend) {}
