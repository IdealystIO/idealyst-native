//! NSLog shim. Pure Foundation — works identically on iOS, tvOS,
//! and macOS.
//!
//! Always visible in Xcode console (iOS/tvOS) and Console.app
//! (macOS).

use objc2_foundation::NSString;

extern "C" {
    fn NSLog(fmt: *const NSString, ...);
}

/// Log `msg` via NSLog. The `%@` format avoids treating `msg` as a
/// format string, so authors can include arbitrary `%` characters
/// without trippping NSLog's formatter.
#[allow(dead_code)]
pub fn apple_log(msg: &str) {
    let ns = NSString::from_str(msg);
    let fmt = NSString::from_str("%@");
    unsafe { NSLog(&*fmt, &*ns) };
}
