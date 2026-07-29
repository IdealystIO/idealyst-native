//! Third-party `WebView` SDK for the idealyst framework.
//!
//! Provides a `WebView` primitive backed by the framework's external
//! extension mechanism. Author-facing API mirrors the framework's
//! other reactive primitives — typed props, `.bind(...)`-able handle,
//! `.with_style(...)`.
//!
//! # One authored surface, two cores
//!
//! The default build is the old-core implementation (`Element::External`
//! payload + per-backend `ExternalRegistry`), byte-moved into
//! [`oldcore`]; the `new-core` feature swaps in [`newcore`], which
//! re-expresses the SAME public call shape (`WebView(WebViewProps
//! { .. }).bind(r)` + a bootstrap `register` + the `WebView` `ui!` tag)
//! over the scene registry — the new core's unified primitive==external
//! contract. Mutually exclusive (same names), mirroring the
//! svg/codeblock/table SDK precedents. The new-core web leg reproduces
//! the old web handler's `<iframe>` DOM call-for-call; on non-web
//! new-core hosts the SDK registers the frozen External-placeholder
//! degradation path (the native iOS/Android handlers are old-core-only
//! until the new-core native boots grow an external story — same
//! posture as svg).
//!
//! # Usage (old core)
//!
//! ```ignore
//! // In the app's bootstrap (one line per third-party SDK):
//! let mut backend = WebBackend::new("#app");
//! webview::register(&mut backend);
//!
//! // Inside a `ui!` block — either the PascalCase tag:
//! ui! {
//!     view {
//!         WebView(url = webview::url("https://example.com"))
//!     }
//! }
//! // …or the fn-call form when a handle is needed:
//! let url = signal("https://example.com".to_string());
//! let wv: Ref<WebViewHandle> = Ref::new();
//! ui! {
//!     view {
//!         { webview::WebView(WebViewProps {
//!             url: webview::url(move || url.get()),
//!             on_load: Some(Rc::new(|| log::info!("loaded"))),
//!             ..Default::default()
//!         }).bind(wv.clone()) }
//!     }
//! }
//! // Imperative ops at any later point:
//! wv.with(|h| h.reload());
//! ```
//!
//! On the new core the same author code compiles unchanged; only the
//! bootstrap seam moves — pass [`register`] to the boot entry
//! (`backend_web::newcore::start_in("#app", webview::register, app)`).
#![deny(missing_docs)]

// Shared wasm32 helpers (pure DOM: imperative iframe ops + the event
// listener wiring with its `__wv_state` closure persistence — no core
// types, no feature gates) used by BOTH cores' web legs.
#[cfg(target_arch = "wasm32")]
pub(crate) mod web_util;

#[cfg(all(target_arch = "wasm32", not(feature = "new-core")))]
mod web;

#[cfg(all(
    target_os = "android",
    not(target_arch = "wasm32"),
    not(feature = "new-core")
))]
mod android;
#[cfg(all(target_os = "ios", not(target_arch = "wasm32"), not(feature = "new-core")))]
mod ios;

// One authored surface, two cores: the default build is the old-core
// implementation, byte-moved into `oldcore`; the `new-core` feature
// swaps in `newcore`, which re-expresses the SAME public names over the
// scene registry. Mutually exclusive (same names).
#[cfg(not(feature = "new-core"))]
mod oldcore;
#[cfg(not(feature = "new-core"))]
pub use oldcore::*;

#[cfg(feature = "new-core")]
mod newcore;
#[cfg(feature = "new-core")]
pub use newcore::*;
