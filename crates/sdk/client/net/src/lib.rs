//! Cross-platform async networking — HTTP, WebSocket, and Server-Sent
//! Events.
//!
//! One async API ([`Client`] for request/response, [`WebSocket`] for
//! bidirectional, [`EventSource`] for receive-only SSE) that compiles to
//! each platform's native networking stack. The platform-agnostic shell —
//! the builders, body codecs, header map, error type, cancellation — lives
//! in this crate; one cfg-gated transport module supplies the substrate per
//! target, so author code is identical everywhere.
//!
//! This is the foundation the `server` SDK (server functions) is built on,
//! but it's independently useful for any author hitting an external API.
//!
//! ```no_run
//! use net::{Client, Error};
//! # #[derive(serde::Deserialize)] struct User { id: u64, name: String }
//! # async fn demo() -> Result<(), Error> {
//! let client = Client::new();
//! let user: User = client
//!     .get("https://api.example.com/users/1")
//!     .header("Authorization", "Bearer xyz")
//!     .send()
//!     .await?
//!     .json()
//!     .await?;
//! # let _ = user;
//! # Ok(())
//! # }
//! ```
//!
//! # Per-platform mechanism
//!
//! Each target lowers to its native HTTP stack; the observable behavior is
//! the same everywhere (same [`Response`], same [`Error`] variants):
//!
//! | Target | HTTP transport |
//! | --- | --- |
//! | Windows / Linux / terminal | [`reqwest`] (rustls — no native-tls / OpenSSL) on `net`'s own I/O runtime thread |
//! | iOS / macOS / tvOS | `NSURLSession` via `objc2` |
//! | Android | `HttpURLConnection` via JNI on a worker thread |
//! | web (wasm32) | `fetch` via web-glue bindings (AbortController for cancel and timeout) |
//!
//! [`WebSocket`] and [`EventSource`] follow the same split — see their
//! module docs for the per-target byte sources. No arm needs an async runtime
//! from its caller, so any executor can await it: WebSocket / desktop SSE
//! drive a blocking I/O worker thread and bridge to `.await` via
//! `futures-channel`; web/Apple/Android HTTP use the OS event loop; desktop
//! HTTP runs reqwest on one process-wide runtime thread that `net` owns.
//!
//! # Body traits
//!
//! Request and response bodies are pluggable via [`IntoBody`] and
//! [`FromBody`]. Built-in impls cover `Vec<u8>`, `String`, `&'static str`,
//! `()`, and (under default features) the [`Json`] and [`Form`]
//! wrappers. Downstream crates (e.g. server functions) implement their
//! own wrappers for postcard / protobuf / etc. without touching this
//! crate.
//!
//! # Cancellation
//!
//! [`cancel_token`] returns a paired `(CancelHandle, CancelToken)`. Attach
//! the token to one or more requests via
//! [`RequestBuilder::cancel_on`]; firing the handle aborts every in-flight
//! request sharing that token with [`Error::Cancelled`]. Self-contained
//! (no `tokio` / `runtime-core` dependency) so the SDK stays standalone.
//!
//! Dropping a request future (a component unmounting, a `select` losing)
//! aborts its exchange on every arm too, with or without a token: reqwest
//! aborts its I/O task, web aborts the `fetch`, NSURLSession sends the task
//! `cancel`, and HttpURLConnection is disconnected (or never connects).
//!
//! # Permissions
//!
//! HTTP needs Android's `INTERNET` permission. This crate declares the
//! `internet` capability in its `Cargo.toml`, so the CLI injects the
//! `<uses-permission>` automatically for any app that (transitively)
//! depends on `net` — no hand-editing `AndroidManifest.xml`. No other
//! platform requires a declared permission for outbound networking.
//!
//! [`reqwest`]: https://crates.io/crates/reqwest

#![deny(missing_docs)]

mod body;
mod cancel;
mod client;
// The HttpURLConnection arm's worker/aborter hand-off; compiled for tests
// on every host so its coverage runs anywhere.
#[cfg(any(test, target_os = "android"))]
mod conn_slot;
mod error;
mod eventsource;
mod headers;
mod method;
mod request;
mod response;
// Pure timeout decisions for the NSURLSession and HttpURLConnection arms;
// compiled for tests on every host so its coverage runs anywhere.
#[cfg(any(
    test,
    target_os = "ios",
    target_os = "macos",
    target_os = "tvos",
    target_os = "android"
))]
mod timeout_policy;
mod websocket;
// `wss://` on Android: the platform TLS socket, and the reader thread that
// makes its reads non-blocking for the WebSocket I/O loop.
#[cfg(target_os = "android")]
mod android_tls;
#[cfg(any(target_os = "android", test))]
mod ws_pump;

pub use body::{FromBody, IntoBody};
#[cfg(feature = "form")]
pub use body::Form;
#[cfg(feature = "json")]
pub use body::Json;
pub use cancel::{cancel_token, CancelHandle, CancelToken, Cancelled};
pub use client::{Client, ClientBuilder};
pub use error::Error;
pub use headers::Headers;
pub use method::Method;
pub use request::RequestBuilder;
pub use response::Response;
pub use eventsource::{EventSource, EventSourceCloser};
pub use websocket::{WebSocket, WsClose, WsMessage, WsSender};

// Platform-specific transport. Exactly one of these is compiled per
// target; each one supplies the `transport` submodule that `client.rs`
// and `request.rs` reach into.
// Desktop reqwest path is Linux/Windows only — Apple platforms use the
// NSURLSession transport below (the platform's own stack). The reqwest arm
// runs its tokio I/O on a runtime thread it owns (see `native.rs`), so no
// host — AppKit, GTK, Win32, winit — has to provide a Tokio runtime.
#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "ios",
    target_os = "macos",
    target_os = "tvos",
    target_os = "android"
)))]
mod native;
#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "ios",
    target_os = "macos",
    target_os = "tvos",
    target_os = "android"
)))]
use native as transport;

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
mod web_glue_events;
#[cfg(target_arch = "wasm32")]
use web as transport;

// NSURLSession transport for every Apple platform (the module's own doc covers
// iOS / macOS / tvOS; its objc2 deps are already gated to the same set).
#[cfg(any(target_os = "ios", target_os = "macos", target_os = "tvos"))]
mod ios;
#[cfg(any(target_os = "ios", target_os = "macos", target_os = "tvos"))]
use ios as transport;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
use android as transport;
