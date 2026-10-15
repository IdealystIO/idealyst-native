//! Win32 desktop host for the native [`backend-windows`] backend.
//!
//! [`run`] opens one top-level window (title bar + resize + minimize +
//! maximize), builds a [`WindowsBackend`](backend_windows::WindowsBackend)
//! rooted at its HWND, mounts the app tree via `backend_windows::newcore::start`,
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
    /// Initial client width in LOGICAL pixels (96-DPI units, like every
    /// framework size); the window is sized × the monitor's DPI scale.
    pub width: i32,
    /// Initial client height in LOGICAL pixels.
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

impl RunOptions {
    /// Apply an `IDEALYST_WINDOW_SIZE` value (`1280x800`; `X` or `,` also
    /// accepted) over the requested client size. This is how
    /// `idealyst test --parity` pins every platform to the SAME viewport —
    /// without it the Win32 window opened at the app's own 1024x768 while the
    /// headless browser ran at the parity viewport, so every responsive-layout
    /// difference between the two read as a divergence. Same contract as
    /// `host-gtk`. Unparseable or non-positive values leave the size alone.
    pub(crate) fn with_size_override(mut self, raw: Option<&str>) -> Self {
        if let Some((w, h)) = raw.and_then(parse_window_size) {
            self.width = w;
            self.height = h;
        }
        self
    }
}

pub(crate) fn parse_window_size(raw: &str) -> Option<(i32, i32)> {
    let (w, h) = raw.split_once(['x', 'X', ','])?;
    let w = w.trim().parse::<i32>().ok()?;
    let h = h.trim().parse::<i32>().ok()?;
    (w > 0 && h > 0).then_some((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regression_parity_viewport_pins_the_win32_window() {
        let base = RunOptions::default();
        let pinned = base.clone().with_size_override(Some("1280x800"));
        assert_eq!((pinned.width, pinned.height), (1280, 800));
        let pinned = base.clone().with_size_override(Some(" 390 , 844 "));
        assert_eq!((pinned.width, pinned.height), (390, 844));
        for bad in [None, Some("garbage"), Some("0x800"), Some("-5x5")] {
            let o = base.clone().with_size_override(bad);
            assert_eq!((o.width, o.height), (base.width, base.height), "{bad:?}");
        }
    }
}

#[cfg(target_os = "windows")]
mod app;
#[cfg(target_os = "windows")]
mod scheduler;
#[cfg(target_os = "windows")]
mod async_executor;

#[cfg(target_os = "windows")]
pub use app::{on_main_loop_start, run, run_with};

#[cfg(not(target_os = "windows"))]
mod stub;
#[cfg(not(target_os = "windows"))]
pub use stub::{on_main_loop_start, run, run_with};
