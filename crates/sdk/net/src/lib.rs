//! Cross-platform async HTTP client SDK.
//!
//! See the crate's `Cargo.toml` header for the overall architecture and
//! per-platform impl strategy. This module re-exports the public API.
//!
//! # Quick start
//!
//! ```no_run
//! # use net::{Client, Error};
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
//! # Body traits
//!
//! Request and response bodies are pluggable via [`IntoBody`] and
//! [`FromBody`]. Built-in impls cover `Vec<u8>`, `String`, `&'static str`,
//! `()`, and (under default features) the [`Json`] and [`Form`]
//! wrappers. Downstream crates (e.g. server functions) implement their
//! own wrappers for postcard / protobuf / etc. without touching this
//! crate.

mod body;
mod cancel;
mod client;
mod error;
mod headers;
mod method;
mod request;
mod response;
mod websocket;

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
pub use websocket::{WebSocket, WsMessage, WsSender};

// Platform-specific transport. Exactly one of these is compiled per
// target; each one supplies the `transport` submodule that `client.rs`
// and `request.rs` reach into.
#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
mod native;
#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
use native as transport;

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
use web as transport;

#[cfg(target_os = "ios")]
mod ios;
#[cfg(target_os = "ios")]
use ios as transport;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
use android as transport;
