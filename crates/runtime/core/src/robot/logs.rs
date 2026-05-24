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
// Stdio capture — backend-provided
// =============================================================================

/// Backend-supplied stdio capturer. The active backend implements
/// this against its platform's stdio facility (POSIX pipe + dup2 on
/// unix-derived backends; equivalent mechanisms elsewhere) and
/// registers an instance via [`install_log_capture`] at init.
///
/// Implementations should call [`push`] with `"stdout"` / `"stderr"`
/// as the source for each captured line. They are also responsible
/// for mirroring bytes back to the original fd so the platform's
/// console (Xcode, adb logcat, terminal) still shows them.
pub trait LogCapture: Send + Sync {
    /// Begin capturing stdout and stderr. Idempotent — backends
    /// should make repeated calls no-ops.
    fn start_stdio_capture(&self);
}

static LOG_CAPTURE: std::sync::OnceLock<Box<dyn LogCapture>> =
    std::sync::OnceLock::new();

/// Register the active backend's log capturer. First call wins;
/// subsequent calls are silently ignored.
pub fn install_log_capture(capture: Box<dyn LogCapture>) {
    let _ = LOG_CAPTURE.set(capture);
}

/// Begin capturing stdout and stderr through the installed
/// [`LogCapture`] backend. Without one, this is a no-op — direct
/// [`push`] calls still work, but `eprintln!` / `println!` output
/// won't reach the robot buffer.
///
/// Call this once at app startup, BEFORE any logging happens, so
/// the redirection is in place before the first write.
pub fn start_stdio_capture() {
    if let Some(c) = LOG_CAPTURE.get() {
        c.start_stdio_capture();
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
