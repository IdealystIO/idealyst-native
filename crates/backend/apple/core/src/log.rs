//! NSLog shim. Pure Foundation — works identically on iOS, tvOS,
//! and macOS.
//!
//! Always visible in Xcode console (iOS/tvOS) and Console.app
//! (macOS).

use objc2_foundation::NSString;
use runtime_core::logging::{LogLevel, Logger};

extern "C" {
    fn NSLog(fmt: *const NSString, ...);
}

/// Log `msg` via NSLog. The `%@` format avoids treating `msg` as a
/// format string, so authors can include arbitrary `%` characters
/// without trippping NSLog's formatter.
pub fn apple_log(msg: &str) {
    let ns = NSString::from_str(msg);
    let fmt = NSString::from_str("%@");
    unsafe { NSLog(&*fmt, &*ns) };
}

/// `runtime_core::Logger` that forwards to NSLog. NSLog has no per-level
/// channel, so the level tag is prefixed (matching `runtime-core`'s
/// `StderrLogger` `[LEVEL] msg` shape) to keep severity greppable.
struct AppleLogger;

impl Logger for AppleLogger {
    fn log(&self, level: LogLevel, msg: &str) {
        apple_log(&format!("[{}] {}", level.tag(), msg));
    }
}

/// Route `runtime_core::log` / `log_info!` through NSLog so author-level
/// logs surface in the Xcode console (iOS/tvOS) and Console.app (macOS)
/// instead of only `StderrLogger`. Idempotent (first-install wins);
/// called from [`crate::scheduler::install_scheduler`].
pub fn install_logger() {
    runtime_core::logging::install_logger(Box::new(AppleLogger));
}
