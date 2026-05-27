//! AppKit host shell for the `backend-macos` native backend.
//!
//! Boots NSApplication, opens a single NSWindow with a flipped
//! content view, installs the backend, and runs
//! `runtime_core::render(app)`. The window is the host's
//! responsibility (per the macOS spec — host owns, injects content
//! view); the backend never touches NSApplication or NSWindow
//! directly.
//!
//! See `docs/macos-backend-plan.md` for the design.

#[cfg(target_os = "macos")]
mod app;

#[cfg(target_os = "macos")]
mod app_delegate;

#[cfg(target_os = "macos")]
pub use app::{run, run_with, RunError, RunOptions};

// runtime-server variant. Mirrors `run` but, instead of mounting the user's
// app() locally, connects to an runtime-server dev-server and applies the
// command stream the sidecar produces. Only present when the
// `runtime-server` Cargo feature is on (forwards to
// `backend-macos/runtime-server`).
#[cfg(all(target_os = "macos", feature = "runtime-server"))]
pub use app::run_aas;

#[cfg(not(target_os = "macos"))]
mod stub;

#[cfg(not(target_os = "macos"))]
pub use stub::{run, run_with, RunError, RunOptions};

#[cfg(all(not(target_os = "macos"), feature = "runtime-server"))]
pub use stub::run_aas;
