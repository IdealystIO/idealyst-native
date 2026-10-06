//! The demo's window (macOS). See README.md.

#[cfg(target_os = "macos")]
fn main() {
    let opts = host_appkit::RunOptions { title: "Over-the-air demo".to_string(), width: 560.0, height: 760.0 };
    if let Err(e) = host_appkit::newcore::run(ota_demo::app, opts) {
        eprintln!("[ota-demo] failed to boot: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("the over-the-air demo's window is macOS-only");
}
