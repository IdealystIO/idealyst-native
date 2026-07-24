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

/// On a desktop target with no backend (Windows / other native), both ops
/// report `NotSupported`. We can only assert this on such a host; on macOS
/// and Linux a real backend runs instead (see the Linux tests below).
#[cfg(all(
    not(target_arch = "wasm32"),
    not(any(target_os = "ios", target_os = "macos", target_os = "tvos")),
    not(target_os = "android"),
    not(target_os = "linux")
))]
#[tokio::test]
async fn unsupported_host_reports_not_supported() {
    assert_eq!(
        clipboard::set_text("x").await,
        Err(ClipboardError::NotSupported)
    );
    assert_eq!(clipboard::text().await, Err(ClipboardError::NotSupported));
}

// --- Linux: real `arboard` backend --------------------------------------
//
// The Linux backend talks to a live X11 server / Wayland compositor. Two
// levels of coverage:
//
//   * `linux_backend_is_wired_up` runs headless in CI. It proves the cfg
//     cascade actually routes Linux to the `arboard` backend and NOT the
//     old `unsupported` stub: the stub returns `NotSupported`, so observing
//     any outcome OTHER than `NotSupported` (a real value, or a `Backend`
//     error from a missing display) is proof the new module is compiled in.
//     This is the regression guard — it fails against the pre-change tree.
//
//   * `linux_round_trip` is a genuine set→get round-trip. It needs a
//     display server + a persistent clipboard owner, which a headless CI
//     box lacks, so it's `#[ignore]`d — run it locally with
//     `cargo test -p clipboard --test portable -- --ignored` under a
//     desktop session.

#[cfg(all(not(target_arch = "wasm32"), target_os = "linux"))]
#[tokio::test]
async fn linux_backend_is_wired_up() {
    // The old `unsupported` stub answered `Err(NotSupported)` here. The
    // `arboard` backend never does: on a desktop session it succeeds, and
    // headless it fails to open a display and returns `Backend(_)`. Either
    // way, `NotSupported` must NOT appear — that's the regression the cfg
    // change must not undo.
    let result = clipboard::text().await;
    assert_ne!(
        result,
        Err(ClipboardError::NotSupported),
        "Linux must route to the arboard backend, not the NotSupported stub; got {result:?}"
    );
}

#[cfg(all(not(target_arch = "wasm32"), target_os = "linux"))]
#[tokio::test]
// NOTE: this needs a real desktop session AND a running clipboard manager
// (GNOME/KDE built-in, klipper, wl-clip-persist, …). Because `set_text`
// drops its arboard `Clipboard` on return — releasing selection ownership —
// the text only survives for this cross-call read if a clipboard manager
// grabbed it. On a bare/nested session with no manager the read-back is
// `None`, which is correct platform behavior, not a bug (see linux.rs).
#[ignore = "needs a live X11/Wayland session + a clipboard manager; run locally with --ignored"]
async fn linux_round_trip() {
    let sample = "idealyst-clipboard-round-trip";
    clipboard::set_text(sample).await.expect("set_text should succeed under a desktop session");
    let read_back = clipboard::text().await.expect("text should succeed under a desktop session");
    assert_eq!(read_back, Some(sample.to_string()));
}
