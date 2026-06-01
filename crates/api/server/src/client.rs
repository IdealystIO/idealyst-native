//! Client-side machinery: configuration + the `call_impl` the macro's
//! client-side stub ultimately routes through.
//!
//! Compiled only when the `server` feature is OFF — i.e. on every
//! client target. The server build never instantiates this module.

use std::sync::{Arc, OnceLock, RwLock};

use serde::{de::DeserializeOwned, Serialize};

use crate::error::{ServerError, ServerFnReturn, TransportError};

/// Author-supplied configuration for the client side of server
/// functions. Install once at app start via [`configure`].
#[derive(Clone, Debug)]
pub struct ClientConfig {
    /// Base URL of the server (no trailing `/_srv/`; e.g.
    /// `https://api.example.com` or `http://localhost:3000`). The
    /// SDK appends `/_srv/<path>` per call.
    pub base_url: String,
}

/// Process-wide config slot. `OnceLock<RwLock<...>>` so the value is
/// settable exactly once at startup *and* replaceable later in tests
/// (configure-then-reconfigure is a valid pattern when a test runs
/// against a hyper server bound to a random port).
static CONFIG: OnceLock<RwLock<Arc<ClientConfig>>> = OnceLock::new();

/// Install or replace the client config. Typically called once at
/// app startup; can be called again to point the client at a
/// different server (useful for tests / dev tools).
pub fn configure(config: ClientConfig) {
    let arc = Arc::new(config);
    match CONFIG.get() {
        Some(slot) => *slot.write().unwrap() = arc,
        None => {
            let _ = CONFIG.set(RwLock::new(arc));
        }
    }
}

/// Snapshot the active config. Returns a `ServerError::Network` if
/// [`configure`] was never called — that's a programmer error and we
/// surface it loudly through the same error channel a real network
/// failure would use.
pub(crate) fn snapshot_config() -> Result<Arc<ClientConfig>, TransportError> {
    let slot = CONFIG.get().ok_or_else(|| {
        TransportError::Network(
            "server::configure(...) was never called; the client doesn't know where the server lives".into(),
        )
    })?;
    Ok(slot.read().unwrap().clone())
}

/// Lazily-constructed shared [`net::Client`]. One per process — the
/// inner reqwest pool reuses connections across calls.
static NET_CLIENT: OnceLock<net::Client> = OnceLock::new();

pub(crate) fn net_client() -> &'static net::Client {
    NET_CLIENT.get_or_init(net::Client::new)
}

/// The client-side dispatch. The macro's stub funnels every call
/// through here; this routes through the batch queue, which coalesces
/// calls within one executor tick into a single `POST /_srv/_batch`.
///
/// Solo calls (only one in the queue at flush time) still use the
/// `POST /_srv/<path>` single-call wire — the queue's only cost is
/// one task yield. See [`crate::batch`] for the full design.
pub(crate) async fn call_impl<Args, Ret>(path: &str, args: &Args) -> Ret
where
    Args: Serialize,
    Ret: DeserializeOwned + ServerFnReturn,
{
    // Type-erase the args via `serde_json::Value`. The batch path
    // re-emits all of them in one `to_vec` pass; doing the encode
    // once here avoids reparsing per call in the flusher.
    let args_value = match serde_json::to_value(args) {
        Ok(v) => v,
        Err(e) => return Ret::from_server_error(ServerError::Codec(e.to_string())),
    };

    // Direct single call by default; coalesce only inside a `batch(...)`
    // scope. Either way the queue/HTTP layer fails only in transport ways,
    // folded into the caller's domain error type `Ret::Error`.
    let outcome = if crate::batch::in_scope() {
        crate::batch::enqueue(path, args_value).await
    } else {
        crate::batch::send_direct(path, args_value).await
    };
    let response_value = match outcome {
        Ok(v) => v,
        Err(e) => return Ret::from_server_error(e.into_domain()),
    };

    match serde_json::from_value::<Ret>(response_value) {
        Ok(r) => r,
        Err(e) => Ret::from_server_error(ServerError::Codec(e.to_string())),
    }
}

pub(crate) fn map_net_error(e: net::Error) -> TransportError {
    match e {
        net::Error::Timeout => TransportError::Network("timeout".into()),
        net::Error::Offline => TransportError::Network("device offline".into()),
        net::Error::InvalidUrl(s) => TransportError::Network(format!("invalid url: {s}")),
        net::Error::Network(s) | net::Error::Other(s) => TransportError::Network(s),
        net::Error::Serialize(s) | net::Error::Deserialize(s) => TransportError::Codec(s),
        net::Error::Status { code, body } => TransportError::Server {
            status: code,
            message: body.unwrap_or_default(),
        },
        net::Error::Cancelled => TransportError::Cancelled,
    }
}
