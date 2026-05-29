//! Native runtime-server-shell: the sync blocking WebSocket transport
//! (`tungstenite`) and the worker-thread `RuntimeServerShell` that
//! drives it.
//!
//! Sits under `crates/dev/` rather than `crates/runtime/` because
//! everything in here is a *dev-mode implementation* — choices about
//! which WebSocket crate to use, which threading model. Per the
//! framework-purity audit the framework layer declares the protocol
//! (`wire`) and the replay engine (`dev-client`); this crate is one
//! of the implementations of "drive that engine over a native socket."
//!
//! Discovery: there is no discovery. The CLI bakes the dev-server URL
//! into the wrapper build via `IDEALYST_DEV_ENDPOINT=ws://host:port`;
//! the wrapper passes it straight to [`RuntimeServerShell::spawn`].
//! See [`shell::resolve_endpoint`] and [`shell::endpoint_or_panic`].
//!
//! Web hosts use a different impl (`backend-web`'s `dev_transport`
//! module against `web_sys::WebSocket`).

#![cfg(feature = "runtime-server")]

pub mod shell;
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub mod apple;
pub mod transport;

pub use shell::{
    endpoint_or_panic, resolve_endpoint, RuntimeServerShell, RuntimeServerShellOptions,
    ENDPOINT_ENV,
};
pub use transport::{connect_and_run, ClientError};

// Re-export the wire identity types so platform shells (backend-ios,
// backend-android) don't need a direct `wire` dependency to populate
// their Hello identity.
pub use wire::{ClientIdentity, WirePlatform, WireViewport};
