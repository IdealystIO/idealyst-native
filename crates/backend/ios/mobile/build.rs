//! Link the frameworks whose symbols this crate references by hand (i.e. via
//! raw `extern "C"` declarations, not through an `objc2-*` crate that would emit
//! its own `cargo:rustc-link-lib`).
//!
//! `imp::callbacks` calls `CACurrentMediaTime()` (the tap-gesture settle gate),
//! which lives in **QuartzCore**. No `objc2` dependency pulls QuartzCore in, so
//! without this the iOS link fails with `Undefined symbols: _CACurrentMediaTime`
//! — hit by the simulator wrapper build, which (unlike the device-run path's
//! `run/ios/frameworks.rs`) doesn't add QuartzCore. Declaring it here links it
//! for every iOS consumer of this crate, simulator and device alike.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "ios" {
        println!("cargo:rustc-link-lib=framework=QuartzCore");
    }
}
