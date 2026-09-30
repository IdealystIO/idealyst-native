//! Entry point for the live-CALayer dashed-border test. Same shape as
//! `shadow_layer_calayer.rs`: the body only compiles on an Apple host (every
//! line talks to Core Animation), and `harness = false` keeps it on the main
//! thread.

#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
#[path = "apple/border_dash_body.rs"]
mod body;

fn main() {
    #[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
    body::run();
    #[cfg(not(any(target_os = "ios", target_os = "tvos", target_os = "macos")))]
    println!("border_dash_calayer: Apple targets only — skipped");
}
