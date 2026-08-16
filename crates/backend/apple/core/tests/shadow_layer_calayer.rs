//! Entry point for the live-CALayer shadow test.
//!
//! The body lives in `apple/shadow_layer_body.rs` (a subdirectory, so cargo
//! doesn't pick it up as a second test target) because it can only COMPILE on
//! an Apple host — every line of it talks to Core Animation. A no-op `main`
//! keeps `cargo test --workspace` linking on Linux/Windows, where a
//! `#![cfg]`-emptied harness-less test would have no `main` at all.

#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
#[path = "apple/shadow_layer_body.rs"]
mod body;

fn main() {
    #[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
    body::run();
    #[cfg(not(any(target_os = "ios", target_os = "tvos", target_os = "macos")))]
    println!("shadow_layer_calayer: Apple targets only — skipped");
}
