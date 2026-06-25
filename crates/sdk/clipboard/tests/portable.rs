//! Portable host tests for the `clipboard` SDK.
//!
//! The error type and its `Display`/`Error` conformance are exercised on
//! every host. The platform clipboard backends can't run inside a bare
//! `cargo test` process: the Apple backend's `NSPasteboard` lives in
//! AppKit (not Foundation), which the test binary doesn't link — the class
//! resolves at runtime via `objc_getClass`, so it compiles+links but
//! `class!(NSPasteboard)` is absent without a real AppKit app loaded.
//! Likewise web/Android need their host runtimes. So the round-trip is
//! left to the app build, and these host tests cover the portable logic.

use clipboard::ClipboardError;

#[test]
fn error_display_and_eq() {
    let backend = ClipboardError::Backend("boom".into());
    let not_supported = ClipboardError::NotSupported;

    assert_ne!(backend, not_supported);
    assert_eq!(backend, ClipboardError::Backend("boom".into()));

    assert!(format!("{backend}").contains("boom"));
    assert!(format!("{not_supported}").contains("not supported"));

    // It's a real std::error::Error.
    let _: &dyn std::error::Error = &backend;
}

/// On a desktop target with no backend (Windows / Linux / other native),
/// both ops report `NotSupported`. We can only assert this on such a host;
/// on macOS the real backend runs instead (see the macOS test below).
#[cfg(all(
    not(target_arch = "wasm32"),
    not(any(target_os = "ios", target_os = "macos", target_os = "tvos")),
    not(target_os = "android")
))]
#[tokio::test]
async fn unsupported_host_reports_not_supported() {
    assert_eq!(
        clipboard::set_text("x").await,
        Err(ClipboardError::NotSupported)
    );
    assert_eq!(clipboard::text().await, Err(ClipboardError::NotSupported));
}

