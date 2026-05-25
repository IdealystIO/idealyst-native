//! Log shim for `idealyst dev`.
//!
//! In default (non-interactive) mode this delegates to `eprintln!` —
//! the CLI's historical behavior. When `--interactive` boots the
//! dev-tui panel, [`install`] stashes the panel's [`DevBus`] in this
//! module; subsequent [`dlog`] calls publish to the bus instead so
//! the panel renders them. Direct `eprintln!` during a TUI session
//! would corrupt the cell grid (host-terminal redirects fd 2 to a
//! log file for exactly that reason).
//!
//! Worker call sites should prefer [`dlog`] over `eprintln!` when
//! emitting messages the user might want to see in the panel. Lines
//! that don't go through here still land in `.idealyst/terminal.log`
//! (host-terminal's StderrRedirect) and are recoverable via `tail
//! -f`, just not rendered live.

use std::sync::OnceLock;

static BUS: OnceLock<dev_tui::DevBus> = OnceLock::new();

/// Stash the panel's bus. Idempotent — subsequent calls are ignored
/// (the CLI only enters interactive mode once per process).
pub fn install(bus: dev_tui::DevBus) {
    let _ = BUS.set(bus);
}

/// Emit a log line. Routes to the panel bus when one is installed,
/// otherwise to `eprintln!` in the `[tag] message` shape the CLI has
/// always used so non-interactive output stays unchanged.
pub fn dlog(tag: &str, message: impl AsRef<str>) {
    let m = message.as_ref();
    if let Some(bus) = BUS.get() {
        bus.log(tag, m);
    } else {
        eprintln!("[{}] {}", tag, m);
    }
}

/// `dlog!("tag", "fmt {}", arg)` — shorthand for the common
/// `format!`-then-`dlog` pattern at call sites converted from
/// `eprintln!`.
#[macro_export]
macro_rules! dlog {
    ($tag:expr, $($arg:tt)*) => {
        $crate::dev_log::dlog($tag, format!($($arg)*))
    };
}
