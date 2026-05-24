//! Real POSIX impl. Gated on `cfg(unix)` so this file is never seen
//! by Windows-only consumers (which would fail to resolve the
//! `libc::STDOUT_FILENO` etc. constants).

use std::io::Read;
use std::sync::Once;

use runtime_core::robot::logs::{install_log_capture, push, LogCapture};

pub(super) fn install() {
    install_log_capture(Box::new(PosixLogCapture));
}

struct PosixLogCapture;

impl LogCapture for PosixLogCapture {
    fn start_stdio_capture(&self) {
        // Idempotent: subsequent calls to `start_stdio_capture()`
        // skip the FD-juggling so we don't end up with multiple
        // reader threads competing for the same pipe.
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            capture_fd(libc::STDOUT_FILENO, "stdout");
            capture_fd(libc::STDERR_FILENO, "stderr");
        });
    }
}

fn capture_fd(fd: libc::c_int, source: &'static str) {
    // Save the original target so we can mirror bytes back to it
    // (otherwise the Xcode console / adb logcat / terminal would
    // see nothing).
    let original = unsafe { libc::dup(fd) };
    if original < 0 {
        return;
    }

    // Create a pipe and redirect the original fd to its write end.
    let mut pipefd: [libc::c_int; 2] = [0; 2];
    let r = unsafe { libc::pipe(pipefd.as_mut_ptr()) };
    if r != 0 {
        return;
    }
    let (read_fd, write_fd) = (pipefd[0], pipefd[1]);
    let dup_r = unsafe { libc::dup2(write_fd, fd) };
    if dup_r < 0 {
        // Couldn't take over the fd — clean up.
        unsafe {
            libc::close(read_fd);
            libc::close(write_fd);
            libc::close(original);
        }
        return;
    }
    unsafe { libc::close(write_fd) };

    // Reader thread: read bytes from the pipe, split into lines,
    // push each line into the buffer, and re-emit the bytes to the
    // original fd so the console still works.
    std::thread::spawn(move || {
        use std::os::unix::io::FromRawFd;
        // Build a File from the raw fd; on drop it'll close it.
        let mut reader = unsafe { std::fs::File::from_raw_fd(read_fd) };
        let mut buf = [0u8; 4096];
        let mut leftover: Vec<u8> = Vec::with_capacity(4096);
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let slice = &buf[..n];
                    // Mirror to the original fd so visibility is
                    // preserved. Ignore errors — if the original is
                    // gone (closed app), we just stop mirroring.
                    let _ = unsafe {
                        libc::write(original, slice.as_ptr() as *const _, n)
                    };
                    leftover.extend_from_slice(slice);
                    // Drain complete lines.
                    while let Some(nl) = leftover.iter().position(|&b| b == b'\n') {
                        let line: Vec<u8> = leftover.drain(..=nl).collect();
                        let mut text = String::from_utf8_lossy(&line).into_owned();
                        if text.ends_with('\n') {
                            text.pop();
                        }
                        if text.ends_with('\r') {
                            text.pop();
                        }
                        push(source, text);
                    }
                }
                Err(_) => break,
            }
        }
    });
}
