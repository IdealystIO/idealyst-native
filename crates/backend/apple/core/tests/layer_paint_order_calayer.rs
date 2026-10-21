//! Entry point for the live-CALayer paint-order test. Body in
//! `apple/layer_paint_order_body.rs` for the same reason as
//! `shadow_layer_calayer`: it only compiles on an Apple host.

#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
#[path = "apple/layer_paint_order_body.rs"]
mod body;

fn main() {
    #[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
    body::run();
    #[cfg(not(any(target_os = "ios", target_os = "tvos", target_os = "macos")))]
    println!("layer_paint_order_calayer: Apple targets only — skipped");
}
