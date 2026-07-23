//! Runs the `welcome` app as a native Linux GTK4 window.
//!
//! `welcome::app` is the exact same platform-agnostic tree the web /
//! iOS / Android builds mount — no `#[cfg]`, no per-target code. Here it
//! renders through `backend-linux` (real GTK widgets, GSK-painted views,
//! Pango text) driven by the `host-gtk` shell. This is the demonstration
//! that idealyst's "one author tree, native output" holds on Linux.
//!
//! Run with: `cargo run -p welcome-gtk`

#[cfg(target_os = "linux")]
fn main() {
    let opts = host_gtk::RunOptions {
        title: "Welcome — Idealyst (GTK)".to_string(),
        // Landscape desktop window; the welcome scene is full-bleed and
        // reflows to whatever size the window is given.
        width: 1000,
        height: 720,
    };
    let code = host_gtk::run(opts, welcome::app);
    std::process::exit(code);
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("welcome-gtk is a Linux-only demo (native GTK4 backend).");
}
