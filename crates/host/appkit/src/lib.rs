//! AppKit host shell for the `backend-macos` native backend.
//!
//! Boots NSApplication, opens a single NSWindow with a flipped
//! content view, installs the backend, and runs
//! `framework_core::render(app)`. The window is the host's
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
pub use app::{run, RunError, RunOptions};

#[cfg(not(target_os = "macos"))]
mod stub;

#[cfg(not(target_os = "macos"))]
pub use stub::{run, RunError, RunOptions};
