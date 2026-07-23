//! Minimal host smoke test: a static tree (no signals, no animation)
//! to isolate the host's window/mount/teardown from any app-level
//! reactive state.
//!
//! ```text
//! cargo run -p host-win32 --example smoke_min
//! ```

use runtime_core::{text, view, Element};

fn app() -> Element {
    view(vec![text("Hello from the native Win32 backend").into()]).into()
}

fn main() {
    let opts = host_win32::RunOptions {
        title: "Idealyst — Win32 minimal smoke".to_string(),
        width: 640,
        height: 400,
    };
    std::process::exit(host_win32::run(opts, app));
}
