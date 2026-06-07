//! Android `Logger`: forwards `runtime-core` log messages to the `log`
//! crate facade, which `android_logger` (initialized in `JNI_OnLoad`)
//! routes to **logcat** under the `idealyst` tag.
//!
//! Without this, `runtime_core::log` / `log_info!` fall back to
//! `StderrLogger` — and Android discards a native library's stdout/stderr
//! by default, so every app-level log line silently vanishes. Installing
//! this forwarder is what makes `log_info!("…")` from author code (and
//! the E2E demo's console output) actually show up under
//! `adb logcat -s idealyst`.

use runtime_core::logging::{LogLevel, Logger};

struct AndroidLogger;

impl Logger for AndroidLogger {
    fn log(&self, level: LogLevel, msg: &str) {
        // Map runtime-core levels onto the `log` crate's, then emit
        // through the facade `android_logger` is already draining. The
        // `idealyst` tag + `Info` max-level filter set up in `JNI_OnLoad`
        // apply uniformly to these and the backend's own `log::*` calls.
        let level = match level {
            LogLevel::Debug => log::Level::Debug,
            LogLevel::Info => log::Level::Info,
            LogLevel::Warn => log::Level::Warn,
            LogLevel::Error => log::Level::Error,
        };
        log::log!(level, "{}", msg);
    }
}

/// Register the runtime-core → logcat forwarder. Idempotent (the
/// `OnceLock` in `runtime-core` is first-install-wins). Called from
/// `JNI_OnLoad`, right after `android_logger::init_once`, so it's in
/// place before any app code runs.
pub(crate) fn install_logger() {
    runtime_core::logging::install_logger(Box::new(AndroidLogger));
}
