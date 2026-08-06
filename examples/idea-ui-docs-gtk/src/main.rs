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
    // `run_with`, not `run`: `run` passes a NO-OP registry hook, so every
    // SDK payload the docs use (codeblock, table, svg, …) would be
    // unregistered and realizing one panics —
    // "no handler registered for item payload (TypeId(…))". That panic
    // surfaces inside a GLib source trampoline, which cannot unwind, so
    // it aborts the process instead of reporting cleanly.
    let code = host_gtk::run_with(opts, idea_ui_docs::register_scene_extensions, idea_ui_docs::app);
    std::process::exit(code);
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("idea-ui-docs-gtk is a Linux-only harness (native GTK4 backend).");
}
