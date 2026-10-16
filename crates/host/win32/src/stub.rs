//! Non-Windows stub. Keeps the public API (`run` / `run_with`)
//! nameable and the crate compiling under a workspace-wide
//! `cargo check` on macOS / Linux, where `backend-windows` is an
//! empty rlib and the `windows` crate isn't in the graph.
//!
//! `register` is an unconstrained generic here: off-Windows there is
//! no `WindowsBackend` type to bound it to, and the wrapper that names
//! the bound is only ever built for the Windows target. Both entry
//! points log and return a non-zero exit code without running `app`.

use runtime_core::Element;

use crate::RunOptions;

pub fn run<F: FnOnce() -> Element + 'static>(opts: RunOptions, build_ui: F) -> i32 {
    // Off-Windows there is no registry type to name; any concrete
    // closure type satisfies the unconstrained `R`.
    run_with(opts, |_: &mut ()| {}, build_ui)
}

pub fn run_with<R, F>(_opts: RunOptions, _register: R, _build_ui: F) -> i32
where
    F: FnOnce() -> Element + 'static,
{
    eprintln!("host-win32 only runs on Windows targets");
    1
}

/// Off-Windows there is no shell identity to set.
pub fn set_app_identity(_id: &str, _display_name: &str) {}

/// Off-Windows nothing ever runs a loop, so the callback is dropped.
pub fn on_main_loop_start<F: FnOnce() + 'static>(_f: F) {}
