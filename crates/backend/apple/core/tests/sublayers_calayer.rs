//! Entry point for the live-CALayer sublayer-removal test. Body in
//! `apple/sublayers_body.rs` for the same reason as `shadow_layer_calayer`:
//! it only compiles on an Apple host.

#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
#[path = "apple/sublayers_body.rs"]
mod body;

fn main() {
    #[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
    body::run();
    #[cfg(not(any(target_os = "ios", target_os = "tvos", target_os = "macos")))]
    println!("sublayers_calayer: Apple targets only — skipped");
}
