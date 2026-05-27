//! Platform-agnostic logging.
//!
//! [`Logger`] is a single-method trait the active backend implements:
//! "emit a leveled message through whatever the host platform
//! considers its log channel." Hosts register an impl via
//! [`install_logger`] at init.
//!
//! Without an installed logger:
//! - On **native** targets, falls back to `eprintln!` with a
//!   `[LEVEL] msg` prefix so messages still surface in terminal hosts
//!   and test runs.
//! - On **wasm32**, drops the message silently. The web backend
//!   installs a `console.{debug,info,warn,error}`-backed logger early
//!   during bootstrap, so author code that runs after `mount(...)`
//!   sees real console output. Before install, dropping is safer than
//!   panicking — logging must never be the thing that crashes a build.
//!
//! # Pattern parallel
//!
//! Matches [`crate::scheduling`] / [`crate::time`]: trait + `OnceLock`
//! registry + free functions for author code. The Backend trait
//! itself stays free of cross-cutting concerns.

use std::sync::OnceLock;

/// Severity of a log message. Mapped per-backend to whatever native
/// channel matches (`console.debug` vs `console.error` on web, NSLog
/// level on Apple platforms, `__android_log_print` priority on
/// Android, etc.).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    /// Short uppercase tag used by the native fallback (`[INFO]
    /// msg`). Backends emitting through a real channel typically
    /// ignore this and dispatch per-level instead.
    pub fn tag(self) -> &'static str {
        match self {
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
        }
    }
}

/// Backend-supplied log sink. Implementations should be cheap and
/// non-panicking — author code may invoke this from animation
/// callbacks, drop handlers, and error-recovery paths where a panic
/// would compound the original problem.
pub trait Logger: Send + Sync {
    fn log(&self, level: LogLevel, msg: &str);
}

static LOGGER: OnceLock<Box<dyn Logger>> = OnceLock::new();

/// Register the active backend's logger. First call wins; subsequent
/// calls are silently ignored. Backends typically call this from the
/// same bootstrap sequence as [`crate::scheduling::install_scheduler`]
/// and [`crate::time::install_time_source`].
pub fn install_logger(logger: Box<dyn Logger>) {
    let _ = LOGGER.set(logger);
}

/// Returns `true` if a backend has installed a real logger. Author
/// code rarely needs this — the [`log`] free function handles the
/// uninstalled case — but it's useful for tests that want to assert
/// the bootstrap wired everything up.
pub fn is_logger_installed() -> bool {
    LOGGER.get().is_some()
}

/// Emit `msg` at `level`. Uses the installed [`Logger`] if present;
/// otherwise falls back to `eprintln!` on native and a silent drop on
/// wasm32. The per-level macros ([`log_debug!`], [`log_info!`],
/// [`log_warn!`], [`log_error!`]) are the ergonomic entry points and
/// support `format!`-style argument lists; call this function
/// directly when you already have a `&str` in hand.
pub fn log(level: LogLevel, msg: &str) {
    if let Some(l) = LOGGER.get() {
        l.log(level, msg);
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        eprintln!("[{}] {}", level.tag(), msg);
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (level, msg);
    }
}

/// Emit a `Debug`-level message. Accepts `format!`-style arguments.
#[macro_export]
macro_rules! log_debug {
    ($($t:tt)*) => {
        $crate::logging::log($crate::logging::LogLevel::Debug, &format!($($t)*))
    };
}

/// Emit an `Info`-level message. Accepts `format!`-style arguments.
#[macro_export]
macro_rules! log_info {
    ($($t:tt)*) => {
        $crate::logging::log($crate::logging::LogLevel::Info, &format!($($t)*))
    };
}

/// Emit a `Warn`-level message. Accepts `format!`-style arguments.
#[macro_export]
macro_rules! log_warn {
    ($($t:tt)*) => {
        $crate::logging::log($crate::logging::LogLevel::Warn, &format!($($t)*))
    };
}

/// Emit an `Error`-level message. Accepts `format!`-style arguments.
#[macro_export]
macro_rules! log_error {
    ($($t:tt)*) => {
        $crate::logging::log($crate::logging::LogLevel::Error, &format!($($t)*))
    };
}

#[cfg(test)]
mod tests {
    //! `LOGGER` is a process-wide `OnceLock`, so we can only install
    //! one logger per test binary. The tests below share a single
    //! capturing logger; ordering is unspecified, so each assertion
    //! filters by a unique marker rather than relying on absolute
    //! indices.
    //!
    //! The native fallback (`eprintln!`) cannot be exercised here in
    //! the same binary because installing the capturing logger
    //! permanently shadows it. That path is covered by hand-running
    //! `cargo test` without a logger installed — see the
    //! `level_tag_strings` test which exercises the fallback's
    //! formatting helper directly.
    use super::*;
    use std::sync::Mutex;

    static CAPTURED: Mutex<Vec<(LogLevel, String)>> = Mutex::new(Vec::new());

    struct CapturingLogger;
    impl Logger for CapturingLogger {
        fn log(&self, level: LogLevel, msg: &str) {
            CAPTURED.lock().unwrap().push((level, msg.to_string()));
        }
    }

    fn ensure_installed() {
        // First test to run wins; subsequent calls are no-ops.
        install_logger(Box::new(CapturingLogger));
    }

    fn drain_matching(marker: &str) -> Vec<(LogLevel, String)> {
        let mut buf = CAPTURED.lock().unwrap();
        let (matched, rest): (Vec<_>, Vec<_>) =
            buf.drain(..).partition(|(_, m)| m.contains(marker));
        *buf = rest;
        matched
    }

    #[test]
    fn level_tag_strings() {
        // The fallback path's format helper — uppercase, no padding,
        // no level-specific punctuation.
        assert_eq!(LogLevel::Debug.tag(), "DEBUG");
        assert_eq!(LogLevel::Info.tag(), "INFO");
        assert_eq!(LogLevel::Warn.tag(), "WARN");
        assert_eq!(LogLevel::Error.tag(), "ERROR");
    }

    #[test]
    fn install_logger_is_idempotent_first_wins() {
        ensure_installed();
        assert!(is_logger_installed());

        struct Other;
        impl Logger for Other {
            fn log(&self, _: LogLevel, _: &str) {
                panic!("the second install should be silently ignored");
            }
        }
        install_logger(Box::new(Other));

        // If `Other` had won the race, this would panic.
        log(LogLevel::Info, "logging::tests::install_logger_is_idempotent_first_wins marker");
        let got = drain_matching("install_logger_is_idempotent_first_wins");
        assert_eq!(got.len(), 1, "expected exactly one capture, got {got:?}");
    }

    #[test]
    fn log_free_function_forwards_level_and_message_verbatim() {
        ensure_installed();
        log(LogLevel::Warn, "logging::tests::forwards_verbatim marker payload");
        let got = drain_matching("forwards_verbatim");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, LogLevel::Warn);
        assert_eq!(got[0].1, "logging::tests::forwards_verbatim marker payload");
    }

    #[test]
    fn level_macros_dispatch_to_matching_level() {
        ensure_installed();
        crate::log_debug!("logging::tests::macros_match_level debug={}", 1);
        crate::log_info!("logging::tests::macros_match_level info={}", 2);
        crate::log_warn!("logging::tests::macros_match_level warn={}", 3);
        crate::log_error!("logging::tests::macros_match_level error={}", 4);

        let got = drain_matching("macros_match_level");
        assert_eq!(got.len(), 4, "expected 4 captures, got {got:?}");

        // Order isn't guaranteed across parallel tests but within a
        // single test body all four calls run sequentially on the
        // same thread, so the capture order matches the call order.
        assert_eq!(got[0].0, LogLevel::Debug);
        assert!(got[0].1.ends_with("debug=1"));
        assert_eq!(got[1].0, LogLevel::Info);
        assert!(got[1].1.ends_with("info=2"));
        assert_eq!(got[2].0, LogLevel::Warn);
        assert!(got[2].1.ends_with("warn=3"));
        assert_eq!(got[3].0, LogLevel::Error);
        assert!(got[3].1.ends_with("error=4"));
    }

    #[test]
    fn level_equality_is_value_based() {
        // `LogLevel` is `Copy + Eq` — the macros rely on this so a
        // backend can use `match level { … }` without taking
        // ownership.
        let a = LogLevel::Info;
        let b = LogLevel::Info;
        assert_eq!(a, b);
        assert_ne!(LogLevel::Debug, LogLevel::Info);
    }
}
