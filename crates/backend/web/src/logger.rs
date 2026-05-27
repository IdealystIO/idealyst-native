//! Web `Logger`: routes through the browser's `console` object, with
//! each [`LogLevel`] mapped to the matching `console` method
//! (`debug` / `info` / `warn` / `error`) so DevTools surfaces the
//! native level styling, filter chips, and stack traces.

use runtime_core::logging::{LogLevel, Logger};
use wasm_bindgen::JsValue;
use web_sys::console;

/// Register this backend's logger with `runtime-core`. Idempotent —
/// first install wins. Hosts typically call this from the same
/// bootstrap that installs the scheduler and time source.
pub fn install_logger() {
    runtime_core::logging::install_logger(Box::new(WebLogger));
}

struct WebLogger;

// SAFETY: wasm32 is single-threaded; the `Send`/`Sync` bounds on
// `Logger` exist only to satisfy the `OnceLock<Box<dyn Logger>>`
// storage. `WebLogger` is a zero-sized marker — no JsValue is ever
// stashed in it — so there's nothing to actually move across threads.
unsafe impl Send for WebLogger {}
unsafe impl Sync for WebLogger {}

impl Logger for WebLogger {
    fn log(&self, level: LogLevel, msg: &str) {
        let js = JsValue::from_str(msg);
        match level {
            LogLevel::Debug => console::debug_1(&js),
            LogLevel::Info => console::info_1(&js),
            LogLevel::Warn => console::warn_1(&js),
            LogLevel::Error => console::error_1(&js),
        }
    }
}
