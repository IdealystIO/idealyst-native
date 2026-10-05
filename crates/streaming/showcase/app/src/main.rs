//! The showcase's desktop window (macOS). The app is `src/lib.rs`, whose
//! `app()` is also what the CLI's iOS/Android shells mount.
//!
//! ```sh
//! cargo run --release -p stream-spike --bin stream-serve   # optional: live reload
//! cargo run --release -p remote-showcase                   # this window
//! idealyst build --ios --release                           # from this directory: the simulator
//! ```

#[cfg(target_os = "macos")]
fn main() {
    let opts = host_appkit::RunOptions { title: "Remote showcase".to_string(), width: 520.0, height: 860.0 };
    if let Err(e) = host_appkit::newcore::run(remote_showcase::app, opts) {
        eprintln!("[remote-showcase] failed to boot: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("remote-showcase's desktop window is macOS-only; use `idealyst build --ios` / `--android`");
}
