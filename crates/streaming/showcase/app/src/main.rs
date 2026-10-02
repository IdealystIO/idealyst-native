//! The showcase's window (macOS). The app is `src/lib.rs`.
//!
//! ```sh
//! cargo run --release -p stream-spike --bin stream-serve   # optional: live reload
//! cargo run --release -p remote-showcase
//! ```

#[cfg(target_os = "macos")]
fn main() {
    use runtime_core::ui;
    remote_showcase::install();
    let opts = host_appkit::RunOptions { title: "Remote showcase".to_string(), width: 520.0, height: 860.0 };
    if let Err(e) = host_appkit::newcore::run(|| ui! { remote_showcase::App() }, opts) {
        eprintln!("[remote-showcase] failed to boot: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("remote-showcase's window is macOS-only (host-appkit)");
}
