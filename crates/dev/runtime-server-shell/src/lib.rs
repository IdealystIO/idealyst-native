//! Native runtime-server-shell: the sync blocking WebSocket transport
//! (`tungstenite`), mDNS service discovery (`mdns-sd`), and the
//! worker-thread `RuntimeServerShell` that ties them together.
//!
//! Sits under `crates/backend/` rather than `crates/framework/`
//! because everything in here is a *platform implementation* —
//! choices about which WebSocket crate to use, which discovery
//! mechanism, which threading model. Per the framework-purity
//! audit the framework layer declares the protocol (`wire`) and
//! the replay engine (`dev-client`); this crate is one of the
//! implementations of "drive that engine over a native socket."
//!
//! Web hosts use a different impl (`backend-web`'s `dev_transport`
//! module against `web_sys::WebSocket`).

#![cfg(feature = "runtime-server")]

pub mod shell;
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub mod apple;
pub mod discover;
pub mod transport;

pub use shell::{RuntimeServerShell, RuntimeServerShellOptions};
pub use discover::{discover, discover_blocking, SERVICE_TYPE};
pub use transport::{connect_and_run, ClientError};

// Re-export the wire identity types so platform shells (backend-ios,
// backend-android) don't need a direct `wire` dependency to populate
// their Hello identity.
pub use wire::{ClientIdentity, WirePlatform, WireViewport};
