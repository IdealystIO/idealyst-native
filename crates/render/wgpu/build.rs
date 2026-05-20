//! Surfaces two custom cfgs derived from the active feature set +
//! target so `#[cfg(...)]` gates in the source stay readable.
//!
//! - `webview_node` — the `WebView` NodeKind variant + create/handle
//!   methods exist. True on wasm32 unconditionally (the iframe path
//!   is always available there) and on native when `feature =
//!   "webview"` is on.
//! - `blitz_active` — the Blitz worker + wgpu upload path is active.
//!   True only when `feature = "webview"` is on AND the target isn't
//!   wasm32. Use this for `web_view.rs` and the renderer's
//!   `web_view_cache` / pre-pass / `take_web_view_requests` glue.
//!
//! Both names are reserved on stable Rust's `unexpected_cfgs` lint
//! via the `cargo::rustc-check-cfg` directives below.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(webview_node)");
    println!("cargo::rustc-check-cfg=cfg(blitz_active)");

    let webview = std::env::var_os("CARGO_FEATURE_WEBVIEW").is_some();
    let wasm = std::env::var("CARGO_CFG_TARGET_ARCH")
        .map(|a| a == "wasm32")
        .unwrap_or(false);

    if wasm || (webview && !wasm) {
        println!("cargo::rustc-cfg=webview_node");
    }
    if webview && !wasm {
        println!("cargo::rustc-cfg=blitz_active");
    }
}
