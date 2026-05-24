//! POSIX `LogCapture` impl for the framework's robot log buffer.
//!
//! Splices stdout/stderr through pipes via `libc::dup` + `libc::pipe`
//! + `libc::dup2`, reads each line on a worker thread, pushes it
//! into `runtime_core::robot::logs` via `push("stdout"/"stderr",
//! …)`, and mirrors the bytes back to the original fd so the
//! platform's console (Xcode, adb logcat, terminal) still shows
//! them.
//!
//! # Usage
//!
//! ```ignore
//! // Once at host startup, before any logging happens:
//! posix_log_capture::install();
//! runtime_core::robot::logs::start_stdio_capture();
//! ```
//!
//! On non-unix targets [`install`] is a compiled-in no-op so cross-
//! compile of consumer code still type-checks.

#![cfg(feature = "log-capture")]

#[cfg(unix)]
mod imp;

/// Register the POSIX `LogCapture` impl with `runtime-core`. First
/// install wins; subsequent calls are silently ignored.
///
/// On non-unix targets this is a no-op — `LogCapture` has no
/// portable POSIX-FD implementation and the call compiles to
/// nothing.
pub fn install() {
    #[cfg(unix)]
    imp::install();
}
