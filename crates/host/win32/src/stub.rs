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
    run_with(opts, |_| {}, build_ui)
}

pub fn run_with<R, F>(_opts: RunOptions, _register: R, _build_ui: F) -> i32
where
    F: FnOnce() -> Element + 'static,
{
    eprintln!("host-win32 only runs on Windows targets");
    1
}
