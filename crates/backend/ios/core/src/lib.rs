//! Shared iOS/tvOS substrate.
//!
//! Houses the bits that both `backend-ios-mobile` (touch) and
//! `backend-ios-tv` (focus engine) reuse unchanged: style/color/flex
//! application and the NSTimer-based render loop driver. Higher
//! pieces — the `IosBackend` struct, primitive construction,
//! navigator/tab-drawer chrome — stay in the leaf crates because
//! they bake in input semantics that differ between mobile and TV.
//!
//! Modules are gated on `cfg(any(target_os = "ios", target_os =
//! "tvos"))`; on the host target the crate compiles as an empty rlib
//! so workspace-wide `cargo check` keeps working.

#[cfg(any(target_os = "ios", target_os = "tvos"))]
pub mod style;

#[cfg(all(any(target_os = "ios", target_os = "tvos"), feature = "async-driver"))]
pub mod render_loop;

#[cfg(any(target_os = "ios", target_os = "tvos"))]
pub mod scheduler;

/// Platform log via NSLog. Always visible in Xcode console.
///
/// Lives here (not in the leaf crates) so `style.rs` and any future
/// shared module can log without reaching back into the mobile or
/// TV crate it's hosted by.
#[cfg(any(target_os = "ios", target_os = "tvos"))]
#[allow(dead_code)]
pub fn ios_log(msg: &str) {
    let ns = objc2_foundation::NSString::from_str(msg);
    // NSLog(@"%@", msg) — the %@ format avoids treating msg as a format string.
    extern "C" {
        fn NSLog(fmt: *const objc2_foundation::NSString, ...);
    }
    let fmt = objc2_foundation::NSString::from_str("%@");
    unsafe { NSLog(&*fmt, &*ns) };
}
