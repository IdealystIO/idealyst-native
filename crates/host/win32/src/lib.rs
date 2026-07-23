//! Win32 desktop host for the native [`backend-windows`] backend.
//!
//! [`run`] opens one top-level window (title bar + resize + minimize +
//! maximize), builds a [`WindowsBackend`](backend_windows::WindowsBackend)
//! rooted at its HWND, mounts the app tree via [`runtime_core::mount`],
//! and pumps the Win32 message loop until the window closes. The
//! framework's scheduler is installed (on the same message loop)
//! *before* the mount so `after_ms` / `raf_loop` — and therefore every
//! `AnimatedValue` — advance.
//!
//! ```no_run
//! use host_win32::RunOptions;
//! use runtime_core::{view, Element};
//!
//! fn app() -> Element {
//!     view(vec![]).into()
//! }
//!
//! fn main() {
//!     host_win32::run(RunOptions { title: "Demo".into(), width: 900, height: 700 }, app);
//! }
//! ```
//!
//! The app tree is host-triple-agnostic — the same `app()` that runs on
//! web/iOS/Android renders here as native Win32 controls. This is the
//! Win32 sibling of `host-gtk` (Linux/GTK4); the two native desktop
//! hosts share the same `run` / `run_with` shape so the CLI's per-
//! platform wrapper generation stays uniform.
//!
//! ## Build gating
//!
//! The real implementation lives in [`app`] under
//! `cfg(target_os = "windows")`. On other hosts the [`stub`] module
//! provides the same surface (returning a non-zero exit code) so a
//! workspace-wide `cargo check` on macOS / Linux compiles this crate
//! without the `windows` crate.

/// Window configuration for [`run`]. Matches `host_gtk::RunOptions`.
#[derive(Clone, Debug)]
pub struct RunOptions {
    /// Window title shown in the title bar + taskbar.
    pub title: String,
    /// Initial client width in pixels.
    pub width: i32,
    /// Initial client height in pixels.
    pub height: i32,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            title: "Idealyst".to_string(),
            width: 1024,
            height: 768,
        }
    }
}

#[cfg(target_os = "windows")]
mod app;
#[cfg(target_os = "windows")]
mod scheduler;

#[cfg(target_os = "windows")]
pub use app::{run, run_with};

#[cfg(not(target_os = "windows"))]
mod stub;
#[cfg(not(target_os = "windows"))]
pub use stub::{run, run_with};
