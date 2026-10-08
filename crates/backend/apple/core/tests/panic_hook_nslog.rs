//! Regression: on a device, a Rust panic's message was lost. The std hook
//! writes to stderr, which nothing captures on iOS; the crash report only
//! showed the later `panic_cannot_unwind` abort in `after_ms_inner`'s NSTimer
//! block (kiosk crash, 2026-10-07). `install_scheduler` now installs a hook
//! that routes the report through NSLog. This test drives the hook with a
//! capturing sink (the NSLog sink is the same closure with `apple_log`
//! plugged in) and asserts the report carries the message, the panic's
//! source location and a backtrace — the three things the device crash
//! report could not give us.
//!
//! Its own test binary because the panic hook is process-global.

#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
#[test]
fn regression_ios_local_panic_message_reaches_unified_log() {
    use std::sync::Mutex;

    static CAPTURED: Mutex<Vec<String>> = Mutex::new(Vec::new());
    fn sink(report: &str) {
        CAPTURED.lock().unwrap().push(report.to_string());
    }

    backend_apple_core::crash::install_panic_hook_with_sink(sink);
    // Later installs (every boot path calls `install_scheduler`) must not
    // replace the first hook.
    backend_apple_core::crash::install_panic_hook();

    let panic_line = line!() + 2;
    let result = std::panic::catch_unwind(|| {
        panic!("kiosk timer body exploded: {}", 42);
    });
    assert!(result.is_err());

    let captured = CAPTURED.lock().unwrap();
    assert_eq!(captured.len(), 1, "exactly one report per panic: {captured:?}");
    let report = &captured[0];
    assert!(report.starts_with("RUST PANIC"), "{report}");
    assert!(report.contains("kiosk timer body exploded: 42"), "message missing: {report}");
    assert!(
        report.contains(&format!("panic_hook_nslog.rs:{panic_line}:")),
        "location missing: {report}"
    );
    // A forced backtrace, not "disabled backtrace" — RUST_BACKTRACE is never
    // set on a device.
    assert!(!report.contains("disabled backtrace"), "backtrace not captured: {report}");
}
