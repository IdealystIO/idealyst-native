//! Redirect this process's stderr to a log file for the lifetime of
//! a terminal session.
//!
//! crossterm puts the TTY into raw mode + alternate screen and
//! paints by emitting ANSI escapes on stdout (fd 1). Any other code
//! writing to stderr (`eprintln!` in `dev-hot`, `dev-client`,
//! `runtime-server-shell-native`, `mdns-sd`, …) drops bytes into the
//! same TTY — visible cursor jumps, half-painted glyphs, and a
//! grid that never recovers because crossterm's diff-paint assumes
//! the previous frame is still on screen.
//!
//! The fix this module provides: at session start, `dup` the
//! current stderr fd so we can restore it later, open the log file,
//! `dup2` the log fd over fd 2. Every subsequent `eprintln!`
//! anywhere in the process lands in the file instead of the TTY.
//! On drop, restore the original fd. Stdout (fd 1) is left alone —
//! that's where crossterm paints, and we want it on the TTY.
//!
//! No-op on non-unix targets. Crossterm supports Windows but the
//! `libc::dup2` trick is unix-only; the same approach using
//! `SetStdHandle` is possible but not wired here.

use std::path::Path;

pub struct StderrRedirect {
    #[cfg(unix)]
    saved_fd: std::os::raw::c_int,
}

impl StderrRedirect {
    /// Redirect stderr to `log_path`, creating its parent if needed.
    /// On success the file is truncated and every subsequent write
    /// to fd 2 lands there until the returned guard drops.
    ///
    /// Failure modes (open fails, dup fails) downgrade to a no-op
    /// guard — the terminal will look messy on hot-reload but the
    /// session still runs. Logged via `eprintln!` BEFORE we steal
    /// fd 2, so the user sees the warning on their normal terminal.
    pub fn install(log_path: &Path) -> Self {
        #[cfg(unix)]
        unsafe {
            use std::ffi::CString;
            use std::os::raw::c_int;

            if let Some(parent) = log_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }

            let saved_fd = libc::dup(libc::STDERR_FILENO);
            if saved_fd < 0 {
                eprintln!(
                    "[host-terminal] could not dup stderr; hot-reload logs will \
                     corrupt the screen"
                );
                return Self { saved_fd: -1 };
            }

            let c_path = match CString::new(log_path.to_string_lossy().as_bytes()) {
                Ok(p) => p,
                Err(_) => {
                    libc::close(saved_fd);
                    return Self { saved_fd: -1 };
                }
            };
            let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC;
            let mode: libc::mode_t = 0o644;
            let log_fd: c_int = libc::open(c_path.as_ptr(), flags, mode as c_int);
            if log_fd < 0 {
                libc::close(saved_fd);
                eprintln!(
                    "[host-terminal] could not open {} for stderr redirect: {}",
                    log_path.display(),
                    std::io::Error::last_os_error(),
                );
                return Self { saved_fd: -1 };
            }
            if libc::dup2(log_fd, libc::STDERR_FILENO) < 0 {
                libc::close(log_fd);
                libc::close(saved_fd);
                return Self { saved_fd: -1 };
            }
            libc::close(log_fd);
            Self { saved_fd }
        }

        #[cfg(not(unix))]
        {
            let _ = log_path;
            Self {}
        }
    }
}

impl Drop for StderrRedirect {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            if self.saved_fd >= 0 {
                libc::dup2(self.saved_fd, libc::STDERR_FILENO);
                libc::close(self.saved_fd);
                self.saved_fd = -1;
            }
        }
    }
}
