//! Native AAS-shell: the sync blocking WebSocket transport
//! (`tungstenite`), mDNS service discovery (`mdns-sd`), and the
//! worker-thread `AasShell` that ties them together.
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

#![cfg(feature = "aas-shell")]

pub mod aas_shell;
pub mod discover;
pub mod transport;

pub use aas_shell::AasShell;
pub use discover::{discover, discover_blocking, SERVICE_TYPE};
pub use transport::{connect_and_run, ClientError};
