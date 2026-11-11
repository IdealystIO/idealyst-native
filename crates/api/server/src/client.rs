//! Client-side machinery: configuration + the `call_impl` the macro's
//! client-side stub ultimately routes through.
//!
//! Compiled on every build, `server` feature or not (see the note on the
//! client surface in `lib.rs`: gating it made the feature non-additive).

use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use serde::{de::DeserializeOwned, Serialize};

use crate::error::{ServerError, ServerFnReturn, TransportError};

/// Author-supplied configuration for the client side of server
/// functions. Install once at app start via [`configure`].
///
/// Built with [`ClientConfig::new`] plus the `with_*` methods; the
/// optional parts are private fields, so new options arrive as new
/// builder methods.
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
    /// Deadline on each `#[server]` HTTP exchange. `None` leaves the
    /// platform transport's own behaviour in place.
    pub(crate) timeout: Option<Duration>,
    /// Told about every failed `#[server]` call. `None` means nobody is.
    pub(crate) reply_observer: Option<Arc<dyn ReplyObserver>>,
}

impl ClientConfig {
    /// Config pointing at `base_url`, with no credentials, no timeout and
    /// no reply observer.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            credentials: None,
            timeout: None,
            reply_observer: None,
        }
    }

    /// Attach a credential source (see [`bearer`], [`credentials_from_fn`]).
    pub fn with_credentials(mut self, provider: impl CredentialProvider) -> Self {
        self.credentials = Some(Arc::new(provider));
        self
    }

    /// Fail any `#[server]` call whose HTTP exchange takes longer than
    /// `timeout` with `ServerError::Network("timeout")`.
    ///
    /// The deadline covers the whole exchange (connect, request,
    /// response head and body) and is enforced by `net` on every
    /// target: reqwest on desktop, `setTimeout` + `AbortController` on
    /// web, NSURLSession's resource timeout on Apple, a watchdog on
    /// Android. Calls coalesced by [`batch`](crate::batch) share one
    /// request, so they share one deadline. `#[channel]`,
    /// `#[subscription]` and `#[sse]` streams are long-lived and are not
    /// bounded by it.
    ///
    /// Without this there is no deadline: a call waits as long as the
    /// platform transport does, which on a link that drops packets can
    /// be minutes (Apple's 60 s idle timeout) or indefinitely (desktop,
    /// Android).
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Attach an observer told about every failed `#[server]` call (see
    /// [`ReplyObserver`], [`reply_observer_from_fn`]) — one place to react
    /// to, say, any "not signed in" refusal without checking each call
    /// site.
    pub fn with_reply_observer(mut self, observer: impl ReplyObserver) -> Self {
        self.reply_observer = Some(Arc::new(observer));
        self
    }
}

/// The dev backend URL the `idealyst` CLI advertised for this run, if any.
///
/// `idealyst dev` sets the `IDEALYST_SERVER_URL` env var on the native app
/// processes it spawns (macOS / terminal / iOS-sim) when the project
/// declares a server, so the client can point [`configure`] at the dev
/// backend without hardcoding host/port:
///
/// ```ignore
/// let base = server::dev_base_url().unwrap_or_else(|| "http://127.0.0.1:3000".into());
/// server::configure(server::ClientConfig::new(base));
/// ```
///
/// Returns `None` when unset — i.e. in production, and on web (the bundle
/// is served same-origin there, so use the document's origin instead).
/// Reads nothing on `wasm32`: the env var has no meaning in the browser.
pub fn dev_base_url() -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::var("IDEALYST_SERVER_URL")
            .ok()
            .filter(|s| !s.is_empty())
    }
    #[cfg(target_arch = "wasm32")]
    {
        None
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

/// Observes the outcome of `#[server]` calls: told about every call that
/// ends in an error, before the caller sees it. The response-side
/// counterpart of [`CredentialProvider`].
///
/// A failure is any call whose result is `Err`: a domain error the server
/// fn returned (`ServerError::Failed`), a non-2xx status (`Server`), a
/// transport failure or timeout (`Network`), a decode failure (`Codec`)
/// or schema drift (`IncompatibleVersion`). A call cancelled through
/// [`with_cancel`](crate::with_cancel) got no reply and is not reported.
///
/// The error arrives as a plain [`ServerError`] whatever the fn's domain
/// error type: a typed `Failed(E)` is reported as `Failed` carrying `E`'s
/// JSON, or the string itself when `E` serializes as a JSON string (so
/// `ServerError::failed("Not signed in")` arrives as
/// `Failed("Not signed in")`).
pub trait ReplyObserver: Send + Sync + 'static {
    /// Called once per failed call, on the task that made it, with the
    /// fn's path (`"add"`, or the `#[server(path = ...)]` override).
    fn on_failure(&self, path: &str, error: &ServerError);
}

/// Adapt a closure into a [`ReplyObserver`].
///
/// ```ignore
/// server::configure(
///     server::ClientConfig::new(base).with_reply_observer(server::reply_observer_from_fn(
///         |path, error| {
///             if matches!(error, server::ServerError::Server { status: 401, .. }) {
///                 log::warn!("{path}: signed out");
///             }
///         },
///     )),
/// );
/// ```
pub fn reply_observer_from_fn<F>(f: F) -> FnReplyObserver<F>
where
    F: Fn(&str, &ServerError) + Send + Sync + 'static,
{
    FnReplyObserver(f)
}

/// Reply observer from [`reply_observer_from_fn`].
pub struct FnReplyObserver<F>(F);

impl<F> ReplyObserver for FnReplyObserver<F>
where
    F: Fn(&str, &ServerError) + Send + Sync + 'static,
{
    fn on_failure(&self, path: &str, error: &ServerError) {
        (self.0)(path, error)
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
///
/// Also sets the image base (`runtime_core::set_image_base_url`), so a
/// root-relative `image()` src such as `/uploads/x.png` loads from this
/// server on every platform. An empty or relative `base_url` leaves no
/// image base, and the browser resolves the path against the page.
pub fn configure(config: ClientConfig) {
    runtime_core::set_image_base_url(&config.base_url);
    let arc = Arc::new(config);
    match CONFIG.get() {
        Some(slot) => *slot.write().unwrap() = arc,
        None => {
            let _ = CONFIG.set(RwLock::new(arc));
        }
    }
}

/// The base URL the client is configured to call — what [`configure`] was
/// last given, with any trailing `/` removed — or `None` before
/// [`configure`] has run.
///
/// For links and requests that go to the app's server but not through a
/// `#[server]` fn: a download route, a file the OS or browser should open
/// itself, an image `src`, a plain-HTTP webhook the app calls directly.
/// Reading it back keeps them pointed wherever the server-fn calls go,
/// instead of repeating the host in a second place:
///
/// ```ignore
/// let base = server::base_url().expect("server::configure ran at startup");
/// let href = format!("{base}/files/{id}/download");
/// ```
///
/// On web, when the bundle is served same-origin, this is whatever was
/// passed to `configure` (often `""` or the page origin).
pub fn base_url() -> Option<String> {
    let config = CONFIG.get()?.read().unwrap().clone();
    Some(config.base_url.trim_end_matches('/').to_string())
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

/// Encode a channel/subscription's open-args tuple as hex JSON for the
/// connect URL's `?args=` param. Hex is URL-safe (no escaping needed)
/// and the args are tiny.
pub(crate) fn encode_ws_args<T: serde::Serialize>(args: &T) -> String {
    let bytes = serde_json::to_vec(args).unwrap_or_default();
    to_hex(&bytes)
}

/// Append the hex-encoded open args to the `ws://…/_srv/_ws/<path>` URL.
pub(crate) fn ws_url_args(path: &str, args_hex: &str) -> String {
    let base = ws_url(path);
    if args_hex.is_empty() {
        base
    } else {
        format!("{base}?args={args_hex}")
    }
}

/// The `http(s)://…/_srv/_sse/<path>` event-stream URL (SSE rides plain
/// HTTP, so no `ws` scheme swap), with optional hex-encoded open args.
pub(crate) fn sse_url_args(path: &str, args_hex: &str) -> String {
    let base = snapshot_config()
        .map(|c| c.base_url.clone())
        .unwrap_or_default();
    let url = format!("{}/_srv/_sse/{}", base.trim_end_matches('/'), path);
    if args_hex.is_empty() {
        url
    } else {
        format!("{url}?args={args_hex}")
    }
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Lazily-constructed shared [`net::Client`]. One per process — the
/// inner reqwest pool reuses connections across calls.
///
/// Carries no default timeout: [`ClientConfig::with_timeout`] is applied
/// per request from the config snapshot, so a later [`configure`] call
/// changes it without rebuilding this client.
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

    // A mocked path never reaches the network (see `crate::mock`).
    match crate::mock::lookup(path) {
        Some(crate::mock::Mock::Pending) => return std::future::pending().await,
        Some(crate::mock::Mock::Reply(handler)) => {
            return match handler(args_value).and_then(|v| serde_json::from_value::<Ret>(v).map_err(|e| format!("reply: {e}"))) {
                Ok(r) => r,
                Err(e) => Ret::from_server_error(ServerError::Codec(format!("mock for `{path}`: {e}"))),
            };
        }
        None => {}
    }

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
    let observer = snapshot_config()
        .ok()
        .and_then(|c| c.reply_observer.clone());
    let (response_value, server_schema) = match outcome {
        Ok(x) => x,
        // Transport-layer failure (incl. a 426 IncompatibleVersion from a
        // strict / drifted server); fold into the caller's `Ret::Error`.
        Err(e) => {
            if let Some(observer) = &observer {
                if !matches!(e, TransportError::Cancelled) {
                    observer.on_failure(path, &e.clone().into_domain());
                }
            }
            return Ret::from_server_error(e.into_domain());
        }
    };

    // Read the failure off the raw reply before the typed decode consumes
    // it: `Ret` is opaque here, so this is the one place a domain error
    // can be seen without knowing `E`.
    let reply_failure = observer
        .as_ref()
        .and_then(|_| failure_in_reply(&response_value));

    match serde_json::from_value::<Ret>(response_value) {
        Ok(r) => {
            if let (Some(observer), Some(failure)) = (&observer, reply_failure) {
                observer.on_failure(path, &failure);
            }
            r
        }
        // The response didn't decode into `Ret`. If the server advertised
        // a different return schema, this is version drift — report it
        // precisely; otherwise it's a genuine same-version codec bug.
        Err(e) => {
            let error: TransportError = match server_schema {
                Some(s) if s != schema => ServerError::IncompatibleVersion {
                    path: path.to_string(),
                    client_schema: schema,
                    server_schema: s,
                },
                _ => ServerError::Codec(e.to_string()),
            };
            if let Some(observer) = &observer {
                observer.on_failure(path, &error.clone().into_domain());
            }
            Ret::from_server_error(error.into_domain())
        }
    }
}

/// The error a reply carries, if it is an `{"Err": …}` result, as the
/// domain-free [`ServerError`] a [`ReplyObserver`] receives. A typed
/// `Failed(E)` becomes `Failed` of `E`'s JSON (the bare string when `E` is
/// a JSON string). `None` for a success, or for a reply that is not
/// `Result`-shaped (a custom [`ServerFnReturn`] type).
fn failure_in_reply(reply: &serde_json::Value) -> Option<ServerError> {
    let err = reply.as_object().filter(|o| o.len() == 1)?.get("Err")?;
    let error = serde_json::from_value::<ServerError<serde_json::Value>>(err.clone()).ok()?;
    Some(match error {
        ServerError::Failed(serde_json::Value::String(s)) => ServerError::Failed(s),
        ServerError::Failed(other) => ServerError::Failed(other.to_string()),
        ServerError::Network(s) => ServerError::Network(s),
        ServerError::Codec(s) => ServerError::Codec(s),
        ServerError::Server { status, message } => ServerError::Server { status, message },
        ServerError::Cancelled => ServerError::Cancelled,
        ServerError::IncompatibleVersion {
            path,
            client_schema,
            server_schema,
        } => ServerError::IncompatibleVersion {
            path,
            client_schema,
            server_schema,
        },
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    /// #81: apps building URLs to their own server (downloads, files the
    /// OS opens) had no way to read the configured base back —
    /// `snapshot_config` is crate-private. This is the only lib test that
    /// touches the process-global `CONFIG`, so the unconfigured step is
    /// deterministic.
    #[test]
    fn base_url_reads_back_the_configured_server() {
        assert_eq!(base_url(), None, "unconfigured → None");

        configure(ClientConfig::new("https://api.example.com"));
        assert_eq!(base_url().as_deref(), Some("https://api.example.com"));

        // Reconfiguring is observed, and a trailing slash is dropped so
        // `format!("{base}/path")` never doubles it.
        configure(ClientConfig::new("http://127.0.0.1:3000/"));
        assert_eq!(base_url().as_deref(), Some("http://127.0.0.1:3000"));
    }
}
