//! Terminal boot — what the generated `terminal/wrapper/src/main.rs`
//! used to be.
//!
//! The whole crossterm loop lives in `host-terminal`; this is the seam
//! that hands it the app. Note there is no runtime-server variant here:
//! in that mode the process must NOT link the app at all (the framework
//! runtime lives in a sidecar and this end is a thin replay client), so
//! it isn't an app entry point and doesn't belong in the app's binary.
//! That mode ships as its own binary in the framework.

use runtime_scene::Element;

use super::{AppConfig, SceneExtensions};

/// Boot the app into the terminal.
/// `S` (the builtin set) is accepted for signature parity and ignored:
/// it exists to shrink a shipped wasm bundle, and a native binary has
/// no equivalent to trim.
pub fn run<E: SceneExtensions, S: runtime_vocabulary::BuiltinSet>(
    app: impl FnOnce() -> Element,
    config: AppConfig,
) {
    let mut opts = host_terminal::RunOptions::default();
    // px per character cell. `None` leaves host-terminal's natural
    // 1px = 1 cell, which is what terminal-only apps are authored
    // against; apps that also target a pixel platform set it so their
    // px-based styles translate to a sane character grid.
    opts.cell_size = config.cell_size;

    if let Err(e) = host_terminal::run(app, opts, E::register) {
        // host-terminal redirects stderr to a log file for the duration
        // of raw mode and restores it on drop, so by the time `run`
        // returns an error this reaches the real terminal.
        eprintln!("[{}] runtime error: {e}", config.name);
        std::process::exit(1);
    }
}
