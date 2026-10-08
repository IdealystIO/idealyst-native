//! Panic reporting for Apple builds — get the panic message into the
//! unified log, then abort loudly.
//!
//! Two pieces:
//!
//! - [`install_panic_hook`] replaces the std panic hook with one that writes
//!   the message, location, thread and a backtrace through NSLog
//!   ([`crate::log::apple_log`]). The std default hook writes to stderr, and
//!   on a device nothing captures stderr: the `.ips` crash report only shows
//!   the final `panic_cannot_unwind` abort from whichever ObjC block / libdispatch
//!   / CFRunLoop callout the unwind tried to cross, and the original panic's
//!   message and frames are gone. NSLog lands in the device syslog
//!   (Console.app, `idevicesyslog`, `log collect`) and in the Xcode console.
//!   The hook runs at the panic site, *before* any unwinding, so it reports
//!   the real panic even when the unwind later aborts at an FFI frame.
//!   Installed from [`crate::scheduler::install_scheduler`], which every Apple
//!   boot path (iOS local, iOS runtime-server sidecar, macOS, tvOS) calls.
//!
//! - [`abort_on_panic`] is the firewall for Rust bodies that run under an
//!   `extern "C"` caller (ObjC blocks, libdispatch drains, CFRunLoop
//!   callouts, ObjC method IMPs). A panic unwinding through such a frame is
//!   undefined behavior; we catch it, name the site in the log, and abort.
//!   Crash-loud is the project policy — there is no safe value to hand back
//!   to the toolkit, so we never swallow the panic and keep running.

use std::any::Any;
use std::panic::{catch_unwind, AssertUnwindSafe, PanicHookInfo};
use std::sync::Once;

static INSTALL: Once = Once::new();

/// Route every Rust panic's report through NSLog. Idempotent (first call
/// wins), so each boot path can call it without coordinating.
pub fn install_panic_hook() {
    install_panic_hook_with_sink(crate::log::apple_log);
}

/// [`install_panic_hook`] with an explicit sink. Exists so the integration
/// test can observe what the hook writes; app code calls
/// [`install_panic_hook`]. Shares its `Once`, so whichever runs first wins.
#[doc(hidden)]
pub fn install_panic_hook_with_sink(sink: fn(&str)) {
    INSTALL.call_once(|| {
        std::panic::set_hook(Box::new(move |info| sink(&panic_report(info))));
    });
}

/// The text the hook logs: `RUST PANIC` marker, thread, location, message,
/// then a forced backtrace. `force_capture` because `RUST_BACKTRACE` is never
/// set on a device, and the frames are the whole point — the crash report's
/// own stack starts at the abort, after the panicking frames unwound away.
pub fn panic_report(info: &PanicHookInfo<'_>) -> String {
    let thread = std::thread::current();
    let thread = thread.name().unwrap_or("<unnamed>");
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "<unknown location>".to_string());
    let message = payload_message(info.payload());
    let backtrace = std::backtrace::Backtrace::force_capture();
    format!("RUST PANIC in thread '{thread}' at {location}: {message}\n{backtrace}")
}

/// Run `f`, a body invoked by an `extern "C"` caller. On panic, log `site`
/// and the payload through NSLog, then abort. The panic hook has already
/// logged the message, location and backtrace by the time we get here; this
/// line adds which callback boundary it was about to cross.
#[inline]
pub fn abort_on_panic<R>(site: &'static str, f: impl FnOnce() -> R) -> R {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => value,
        Err(payload) => {
            crate::log::apple_log(&format!(
                "RUST PANIC crossing {site}: {} — aborting",
                payload_message(&*payload)
            ));
            std::process::abort();
        }
    }
}

fn payload_message(payload: &(dyn Any + Send)) -> &str {
    if let Some(s) = payload.downcast_ref::<String>() {
        s.as_str()
    } else if let Some(s) = payload.downcast_ref::<&'static str>() {
        s
    } else {
        "<non-string panic payload>"
    }
}
