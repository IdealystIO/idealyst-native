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
#[derive(Clone)]
pub struct ClientConfig {
    /// Base URL of the server (no trailing `/_srv/`; e.g.
    /// `https://api.example.com` or `http://localhost:3000`). The
    /// SDK appends `/_srv/<path>` per call.
    pub base_url: String,
    /// Optional credential source whose headers are attached to every
    /// outgoing server-fn request (e.g. a bearer token). `None` means
    /// no credentials are injected (cookie-based auth, or unauthenticated).
    pub(crate) credentials: Option<Arc<dyn CredentialProvider>>,
}

impl ClientConfig {
    /// Config pointing at `base_url`, with no credentials.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            credentials: None,
        }
    }

    /// Attach a credential source (see [`bearer`], [`credentials_from_fn`]).
    pub fn with_credentials(mut self, provider: impl CredentialProvider) -> Self {
        self.credentials = Some(Arc::new(provider));
        self
    }
}

/// Supplies authentication headers attached to every outgoing server-fn
/// request. The SDK stays auth-scheme-agnostic — bearer / JWT / custom
/// header schemes compose from this; cookie-based auth needs no provider
/// (the platform's cookie jar handles it).
pub trait CredentialProvider: Send + Sync + 'static {
    /// Headers to attach to the next request. Called per request, so a
    /// rotated / refreshed token (e.g. read from secure storage) is
    /// picked up without reconfiguring.
    fn headers(&self) -> Vec<(String, String)>;
}

/// A bearer-token credential: attaches `Authorization: Bearer <token>`
/// when the closure yields a token. The closure runs per request.
pub fn bearer<F>(token: F) -> BearerCredentials<F>
where
    F: Fn() -> Option<String> + Send + Sync + 'static,
{
    BearerCredentials(token)
}

/// Credential source from [`bearer`].
pub struct BearerCredentials<F>(F);

impl<F> CredentialProvider for BearerCredentials<F>
where
    F: Fn() -> Option<String> + Send + Sync + 'static,
{
    fn headers(&self) -> Vec<(String, String)> {
        match (self.0)() {
            Some(token) => vec![("authorization".to_string(), format!("Bearer {token}"))],
            None => Vec::new(),
        }
    }
}

/// Adapt a closure returning arbitrary headers into a [`CredentialProvider`].
pub fn credentials_from_fn<F>(f: F) -> FnCredentials<F>
where
    F: Fn() -> Vec<(String, String)> + Send + Sync + 'static,
{
    FnCredentials(f)
}

/// Credential source from [`credentials_from_fn`].
pub struct FnCredentials<F>(F);

impl<F> CredentialProvider for FnCredentials<F>
where
    F: Fn() -> Vec<(String, String)> + Send + Sync + 'static,
{
    fn headers(&self) -> Vec<(String, String)> {
        (self.0)()
    }
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

/// Build the `ws(s)://…/_srv/_ws/<path>` URL for a `#[channel]` client
/// stub from the configured base URL (swapping the http→ws scheme). If
/// unconfigured, the empty base yields a malformed URL that fails to
/// connect — the same loud-failure posture as [`snapshot_config`].
pub(crate) fn ws_url(path: &str) -> String {
    let base = snapshot_config()
        .map(|c| c.base_url.clone())
        .unwrap_or_default();
    let ws_base = if let Some(rest) = base.strip_prefix("https") {
        format!("wss{rest}")
    } else if let Some(rest) = base.strip_prefix("http") {
        format!("ws{rest}")
    } else {
        base
    };
    format!("{}/_srv/_ws/{}", ws_base.trim_end_matches('/'), path)
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
pub(crate) async fn call_impl<Args, Ret>(path: &str, schema: u64, args: &Args) -> Ret
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
    // scope. The direct path also surfaces the server's return-schema
    // hash for the drift diagnostic below; the batch path doesn't thread
    // per-slot schemas, so it falls back to a plain codec error.
    let outcome = if crate::batch::in_scope() {
        crate::batch::enqueue(path, schema, args_value)
            .await
            .map(|v| (v, None))
    } else {
        crate::batch::send_direct(path, schema, args_value).await
    };
    let (response_value, server_schema) = match outcome {
        Ok(x) => x,
        // Transport-layer failure (incl. a 426 IncompatibleVersion from a
        // strict / drifted server); fold into the caller's `Ret::Error`.
        Err(e) => return Ret::from_server_error(e.into_domain()),
    };

    match serde_json::from_value::<Ret>(response_value) {
        Ok(r) => r,
        // The response didn't decode into `Ret`. If the server advertised
        // a different return schema, this is version drift — report it
        // precisely; otherwise it's a genuine same-version codec bug.
        Err(e) => match server_schema {
            Some(s) if s != schema => Ret::from_server_error(ServerError::IncompatibleVersion {
                path: path.to_string(),
                client_schema: schema,
                server_schema: s,
            }),
            _ => Ret::from_server_error(ServerError::Codec(e.to_string())),
        },
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
