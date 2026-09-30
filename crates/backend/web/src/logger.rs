//! Web `Logger` and panic hook on web-glue: messages go to the browser's
//! `console`, each [`LogLevel`] mapped to the matching method (`debug` /
//! `info` / `warn` / `error`) so DevTools surfaces the native level
//! styling, filter chips and stack traces.

use runtime_shared::logging::{LogLevel, Logger};
use web_glue::string;

web_glue::import! {
    // level: 0 debug, 1 info, 2 warn, 3 error.
    fn js_console(level: u32, p: usize, l: usize) =
        "(v, p, l) => { const m = G.str(p, l); \
           if (v === 0) console.debug(m); else if (v === 1) console.info(m); \
           else if (v === 2) console.warn(m); else console.error(m); }";
    // What `console_error_panic_hook` printed: the message plus a JS stack,
    // which names the wasm frames (with the `name` section, their Rust
    // symbols) the panic unwound from.
    fn js_console_panic(p: usize, l: usize) =
        "(p, l) => { console.error(G.str(p, l) + '\\n\\nStack:\\n\\n' + new Error().stack + '\\n\\n'); }";
}

/// Register this backend's logger with `runtime-core`. Idempotent —
/// first install wins. Hosts typically call this from the same
/// bootstrap that installs the scheduler and time source.
pub fn install_logger() {
    runtime_shared::logging::install_logger(Box::new(WebLogger));
}

/// Route Rust panics to `console.error` with a JS stack — the boot's
/// replacement for `console_error_panic_hook::set_once`. Without a hook a
/// wasm32 panic message goes to the no-op stderr and the page only sees
/// `RuntimeError: unreachable`. Idempotent: installs once per thread.
pub fn install_panic_hook() {
    use std::sync::Once;
    static SET: Once = Once::new();
    SET.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            let msg = info.to_string();
            let (p, l) = string::abi(&msg);
            unsafe { js_console_panic(p, l) }
        }));
    });
}

struct WebLogger;

impl Logger for WebLogger {
    fn log(&self, level: LogLevel, msg: &str) {
        let level = match level {
            LogLevel::Debug => 0,
            LogLevel::Info => 1,
            LogLevel::Warn => 2,
            LogLevel::Error => 3,
        };
        let (p, l) = string::abi(msg);
        unsafe { js_console(level, p, l) }
    }
}
