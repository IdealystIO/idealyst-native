//! Panic guard for ObjC-dispatched entry points.
//!
//! objc2 0.5's `declare_class!` lowers each `#[method(...)]` to a plain
//! `extern "C"` IMP. A panic unwinding out of one of those method bodies
//! propagates into UIKit's `objc_msgSend` dispatch frame — crossing an
//! `extern "C"` boundary while unwinding is undefined behavior.
//!
//! Every declared-class method whose body runs Rust/framework/author code
//! (touch handlers, control action targets, key handling, collection-view
//! data source, view-lifecycle observers, display-link ticks) wraps its
//! body in [`guard_ffi`]: catch the unwind, log it, and `abort`. Project
//! policy is crash-loud (see `feedback_crash_loud_on_panic`) — we never
//! let a panic silently cross into ObjC, and we never substitute a
//! made-up return value.
//!
//! This mirrors the libdispatch trampoline in [`super::portal`] and the
//! `extern "C"` entry guards in [`crate::runtime_server`].

use std::panic::{catch_unwind, AssertUnwindSafe};

/// Run `f` (an ObjC-dispatched method body) with a panic firewall. On a
/// normal return, returns `f`'s value. On panic, logs `label` + the
/// payload and aborts the process — it never returns `Err` or a default,
/// because there is no safe value to hand back to UIKit.
#[inline]
pub(crate) fn guard_ffi<R>(label: &'static str, f: impl FnOnce() -> R) -> R {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => value,
        Err(payload) => {
            let msg = if let Some(s) = payload.downcast_ref::<String>() {
                s.as_str()
            } else if let Some(s) = payload.downcast_ref::<&'static str>() {
                s
            } else {
                "<non-string panic payload>"
            };
            eprintln!("[backend-ios] panic crossing ObjC boundary in {label}: {msg}");
            std::process::abort();
        }
    }
}
