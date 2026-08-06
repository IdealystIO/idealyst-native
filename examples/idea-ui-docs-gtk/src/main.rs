//! Runs `idea-ui-docs` as a native Linux GTK4 window.
//!
//! Same platform-agnostic tree the web build mounts — no `#[cfg]`, no
//! per-target code. Exists to exercise the native Linux path end to end:
//! the GTK backend, the `codeblock` SDK's GTK leaf, and the builtin
//! `swap_navigator`, none of which the web build touches.
//!
//! Run with: `cargo run -p idea-ui-docs-gtk`
//! Set `IDEALYST_GTK_DUMP_LAYOUT=1` to dump the realized tree per layout pass.

#[cfg(target_os = "linux")]
fn main() {
    let opts = host_gtk::RunOptions {
        title: "idea-ui Docs — GTK".to_string(),
        width: 1280,
        height: 860,
    };
    let code = host_gtk::run(opts, idea_ui_docs::app);
    std::process::exit(code);
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("idea-ui-docs-gtk is a Linux-only harness (native GTK4 backend).");
}
