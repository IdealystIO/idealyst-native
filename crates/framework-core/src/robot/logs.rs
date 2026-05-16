//! Log capture for the robot.
//!
//! Two sources feed a shared ring buffer:
//!
//! 1. **Direct calls** — framework / backend code calls
//!    [`push`] (or the [`robot_log!`] macro) to record a structured
//!    log entry. This is the path with the richest context (level,
//!    source label).
//!
//! 2. **Stdout / stderr capture** — when [`start_stdio_capture`] is
//!    called early in startup, the platform's stdout and stderr file
//!    descriptors are spliced through a pipe. A background thread
//!    reads from the pipe, splits on newlines, pushes each line into
//!    the ring buffer, AND writes the bytes back to the original
//!    fd (so the Xcode console / terminal still shows them).
//!
//!    This catches anything that goes through stdio — Rust's
//!    `eprintln!` / `println!`, C `printf`, iOS `NSLog`'s mirror to
//!    stderr on the simulator, etc.
//!
//! The bridge dispatches `get_logs` to [`recent`] / [`since`].

use std::collections::VecDeque;
use std::io::Read;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_ENTRIES: usize = 4096;

/// A single captured log line.
#[derive(Clone, Debug)]
pub struct LogEntry {
    /// Milliseconds since UNIX epoch.
    pub timestamp_ms: u64,
    /// Where the entry came from: "stdout", "stderr", or a label
    /// supplied by [`push`] (e.g. "ios", "framework", "robot").
    pub source: String,
    /// The line content (no trailing newline).
    pub text: String,
}

static BUFFER: Mutex<VecDeque<LogEntry>> = Mutex::new(VecDeque::new());

/// Push a structured log entry into the buffer. The framework /
/// backend uses this to record events that aren't otherwise emitted
/// via stdout / stderr.
pub fn push(source: impl Into<String>, text: impl Into<String>) {
    let entry = LogEntry {
        timestamp_ms: now_ms(),
        source: source.into(),
        text: text.into(),
    };
    if let Ok(mut buf) = BUFFER.lock() {
        buf.push_back(entry);
        while buf.len() > MAX_ENTRIES {
            buf.pop_front();
        }
    }
}

/// Return the N most recent entries.
pub fn recent(limit: usize) -> Vec<LogEntry> {
    let buf = match BUFFER.lock() {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };
    let start = buf.len().saturating_sub(limit);
    buf.iter().skip(start).cloned().collect()
}

/// Return all entries with `timestamp_ms > since`. Useful for
/// polling: pass the last seen timestamp to get just new lines.
pub fn since(since_ms: u64) -> Vec<LogEntry> {
    let buf = match BUFFER.lock() {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };
    buf.iter()
        .filter(|e| e.timestamp_ms > since_ms)
        .cloned()
        .collect()
}

/// Drop all entries from the buffer.
pub fn clear() {
    if let Ok(mut buf) = BUFFER.lock() {
        buf.clear();
    }
}

/// Convenience macro: `robot_log!(source, "fmt {}", arg)`.
#[macro_export]
macro_rules! robot_log {
    ($source:expr, $($arg:tt)*) => {{
        $crate::robot::logs::push($source, format!($($arg)*));
    }};
}

// =============================================================================
// Stdio capture (Unix-only — iOS, macOS, Android, Linux)
// =============================================================================

/// Splice stdout and stderr through pipes so that everything written
/// to them is also captured into the buffer. Safe to call from any
/// thread; idempotent (subsequent calls are no-ops).
///
/// Call this once at app startup, BEFORE any logging happens, so the
/// redirection is in place before the first write.
#[cfg(unix)]
pub fn start_stdio_capture() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        capture_fd(libc::STDOUT_FILENO, "stdout");
        capture_fd(libc::STDERR_FILENO, "stderr");
    });
}

#[cfg(not(unix))]
pub fn start_stdio_capture() {}

#[cfg(unix)]
fn capture_fd(fd: libc::c_int, source: &'static str) {
    // Save the original target so we can mirror bytes back to it
    // (otherwise the Xcode console / terminal would see nothing).
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
        // Build a File from the raw fd; on drop it'll close it.
        use std::os::unix::io::FromRawFd;
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

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
