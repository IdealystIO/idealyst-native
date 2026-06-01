//! Server-side machinery: walk the `inventory` of registered server
//! functions at startup, build an axum router, bind a TCP listener.
//!
//! Compiled only when the `server` feature is ON. Pulls in axum +
//! tokio + tower; the client build sees none of this.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};

use axum::body::Bytes;
use axum::extract::Path;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use serde::{Deserialize, Serialize};

use crate::__private::ServerFnEntry;
use crate::error::{ServerError, TransportError, VersionMismatch};
use crate::extract::Context;
use crate::extractors::CURRENT_CONTEXT;

/// Request/response header carrying the wire schema hash (hex). Mirrors
/// the client's `batch::SCHEMA_HEADER`.
const SCHEMA_HEADER: &str = "x-srv-schema";

fn parse_request_schema(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(SCHEMA_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| u64::from_str_radix(s, 16).ok())
}

/// A `426 Upgrade Required` carrying the schema details. The client maps
/// it back to `ServerError::IncompatibleVersion`.
fn version_mismatch_response(path: &str, client_schema: u64, server_schema: u64) -> Response {
    let body = serde_json::to_vec(&VersionMismatch {
        path: path.to_string(),
        client_schema,
        server_schema,
    })
    .unwrap_or_default();
    let mut resp = (
        StatusCode::UPGRADE_REQUIRED,
        [("content-type", "application/json")],
        body,
    )
        .into_response();
    if let Ok(v) = HeaderValue::from_str(&format!("{server_schema:x}")) {
        resp.headers_mut().insert(HeaderName::from_static(SCHEMA_HEADER), v);
    }
    resp
}

/// Build an `axum::Router` containing every `#[server]` function
/// linked into the current binary.
///
/// The router exposes each function at `POST /_srv/<path>`. The
/// `<path>` defaults to the function's identifier; override via
/// `#[server(path = "...")]`.
///
/// All registered functions are dispatched through a single
/// catch-all route — the dispatcher looks up `path` against the
/// inventory at request time. That's a hash-map probe per call (not
/// a per-call route table rebuild), and keeps the router size flat
/// regardless of how many server fns the app declares.
pub fn router() -> Router {
    // Force the dispatch map to build now, so a duplicate-path collision
    // fails loudly at server startup (router build) rather than silently
    // shadowing one handler at request time.
    let _ = entry_map();
    let mut app = Router::new()
        // Batch route is declared first so axum's path matcher prefers
        // the exact `/_srv/_batch` over the catch-all `/_srv/*path`.
        .route("/_srv/_batch", post(batch_dispatch));

    // Mount each #[channel]'s WebSocket route (`GET /_srv/_ws/<path>`)
    // before the catch-all so the specific paths win.
    for entry in inventory::iter::<crate::__private::WsEntry> {
        app = (entry.register)(app);
    }

    app.route("/_srv/*path", post(dispatch))
    // Intentionally no `.fallback(...)`: composing this router with
    // a static-file `ServeDir` (the demo's typical setup) needs the
    // caller to install their own fallback. Unknown server-fn paths
    // are still handled — the catch-all `/_srv/*path` matches them,
    // and `dispatch` returns 404 when no inventory entry is found.
}

// ---------------------------------------------------------------------------
// #[channel] (WebSocket) upgrade helpers, used by the macro's generated
// handler. They reuse the same Context / middleware / extractor machinery
// as the HTTP dispatch, run at upgrade time.
// ---------------------------------------------------------------------------

/// Build the request [`Context`] for a channel upgrade from its headers.
pub fn ws_open_context(headers: HeaderMap, path: &'static str) -> Context {
    Context::new(Arc::new(headers), path)
}

/// Run the middleware chain at upgrade; a short-circuit becomes the HTTP
/// response (so e.g. an auth guard rejects with 401 *without* upgrading).
pub async fn ws_run_middlewares(ctx: &mut Context) -> Result<(), Response> {
    crate::middleware::run_middlewares(ctx)
        .await
        .map_err(transport_error_response)
}

/// Map an extractor-resolution failure at upgrade to an HTTP response.
pub fn ws_error_response(e: TransportError) -> Response {
    transport_error_response(e)
}

/// Query string of a channel/subscription upgrade: the open (wire) args,
/// hex-encoded JSON in `?args=<hex>`. `Option` so a no-arg endpoint
/// (no query) decodes to the unit tuple.
#[derive(Deserialize)]
pub struct WsArgsQuery {
    #[serde(default)]
    pub args: Option<String>,
}

/// Decode the open-args tuple `T` from the connect URL's hex-encoded
/// JSON. Absent args decode as `null` (→ the unit tuple). A bad hex /
/// JSON payload is a 400.
pub fn decode_ws_args<T: serde::de::DeserializeOwned>(args: Option<String>) -> Result<T, Response> {
    let bytes = match &args {
        Some(hex) => from_hex(hex)
            .ok_or_else(|| (StatusCode::BAD_REQUEST, "malformed ws args (hex)").into_response())?,
        None => b"null".to_vec(),
    };
    serde_json::from_slice(&bytes)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("ws args decode: {e}")).into_response())
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Bind a TCP listener on `addr` and serve the registered server
/// functions. Convenience for the common "just run the server"
/// case; authors who need to compose with their own routes should
/// reach for [`router`] directly.
pub async fn serve(addr: SocketAddr) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let app = router();
    axum::serve(listener, app)
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
}

/// The dispatcher. Walks the inventory once per request to find the
/// handler matching the requested path — fast enough at v0 sizes; if
/// startup-cost matters we can hash into a `HashMap` once at first
/// request and cache it in a `OnceLock`.
///
/// Wraps the handler's future in a [`REQUEST_CONTEXT`] scope so the
/// handler body can call [`use_request_headers`] /
/// [`use_request_header`] (and future per-request extractors) and
/// see the values for *this* request.
async fn dispatch(headers: HeaderMap, Path(path): Path<String>, body: Bytes) -> Response {
    let Some(entry) = find_entry(&path) else {
        return (
            StatusCode::NOT_FOUND,
            format!("no server fn registered at path '{path}'"),
        )
            .into_response();
    };

    let client_schema = parse_request_schema(&headers);

    // strict_version: reject a mismatched (or absent) client schema up
    // front, before decoding or running the body.
    if entry.strict && client_schema != Some(entry.schema) {
        return version_mismatch_response(&path, client_schema.unwrap_or(0), entry.schema);
    }

    let body_vec = body.to_vec();
    let mut ctx = Context::new(Arc::new(headers), path.clone());

    // Seed the per-request cookie jar so handler `set_cookie` calls have
    // somewhere to land; keep a handle to drain into response headers.
    let jar = crate::cookie::CookieJar::default();
    ctx.insert(jar.clone());

    // Run the middleware chain (auth guards, etc.) before the handler.
    // A short-circuit becomes the HTTP response; the handler never runs.
    if let Err(e) = crate::middleware::run_middlewares(&mut ctx).await {
        return transport_error_response(e);
    }

    let result = CURRENT_CONTEXT
        .scope(ctx, (entry.handler)(body_vec))
        .await;

    match result {
        Ok(bytes) => {
            let mut resp = (
                StatusCode::OK,
                [("content-type", "application/json")],
                bytes,
            )
                .into_response();
            if let Ok(v) = HeaderValue::from_str(&format!("{:x}", entry.schema)) {
                resp.headers_mut().insert(HeaderName::from_static(SCHEMA_HEADER), v);
            }
            apply_set_cookies(&mut resp, jar.take());
            resp
        }
        // A decode failure the schemas disagree on is version drift, not a
        // same-version bug → a precise 426 the client can act on.
        Err(TransportError::Codec(_))
            if client_schema.is_some_and(|c| c != entry.schema) =>
        {
            version_mismatch_response(&path, client_schema.unwrap_or(0), entry.schema)
        }
        Err(e) => transport_error_response(e),
    }
}

/// Append accumulated `Set-Cookie` headers (one per cookie, never folded)
/// to a response. No-op when the handler set none.
fn apply_set_cookies(resp: &mut Response, cookies: Vec<String>) {
    for c in cookies {
        if let Ok(v) = HeaderValue::from_str(&c) {
            resp.headers_mut()
                .append(axum::http::header::SET_COOKIE, v);
        }
    }
}

/// Map a handler-level [`TransportError`] to an HTTP response.
///
/// An extractor failure carries an intended status in
/// `Server { status, .. }` (e.g. 500 for missing `State`, 401 for a
/// failed auth guard); honour it. A `Codec` failure is a malformed
/// request/response — a 400. Other transport variants can't originate
/// server-side; treat them as 500.
fn transport_error_response(e: TransportError) -> Response {
    match e {
        TransportError::Server { status, message } => (
            StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            message,
        )
            .into_response(),
        TransportError::Codec(message) => (StatusCode::BAD_REQUEST, message).into_response(),
        other => (StatusCode::INTERNAL_SERVER_ERROR, other.to_string()).into_response(),
    }
}

async fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "not a server fn route").into_response()
}

/// Build a path → entry map, rejecting duplicate paths. Factored out of
/// [`entry_map`] so the collision logic is unit-testable without two
/// real same-path `#[server]` fns (which would panic the whole suite).
fn build_entry_map<'a>(
    entries: impl Iterator<Item = &'a ServerFnEntry>,
) -> Result<HashMap<&'a str, &'a ServerFnEntry>, String> {
    let mut map: HashMap<&'a str, &'a ServerFnEntry> = HashMap::new();
    for entry in entries {
        if map.insert(entry.path, entry).is_some() {
            return Err(format!(
                "duplicate server fn path '{}': two #[server] functions registered the same \
                 wire path. Disambiguate with #[server(path = \"...\")].",
                entry.path
            ));
        }
    }
    Ok(map)
}

/// The process-wide path → handler map, built once from the inventory.
/// Panics on a duplicate path (a collision) — fail-fast at startup.
fn entry_map() -> &'static HashMap<&'static str, &'static ServerFnEntry> {
    static MAP: OnceLock<HashMap<&'static str, &'static ServerFnEntry>> = OnceLock::new();
    MAP.get_or_init(|| {
        build_entry_map(inventory::iter::<ServerFnEntry>.into_iter())
            .unwrap_or_else(|msg| panic!("{msg}"))
    })
}

fn find_entry(path: &str) -> Option<&'static ServerFnEntry> {
    entry_map().get(path).copied()
}

/// The wire schema hash registered for `path`, or `None` if no server fn
/// is registered there. Useful for diagnostics and for tooling that
/// wants to assert client/server compatibility out of band.
pub fn schema_for(path: &str) -> Option<u64> {
    find_entry(path).map(|e| e.schema)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::TransportError;
    use std::future::Future;
    use std::pin::Pin;

    fn stub_entry(path: &'static str) -> ServerFnEntry {
        fn handler(
            _: Vec<u8>,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, TransportError>> + Send>> {
            Box::pin(async { Ok(Vec::new()) })
        }
        ServerFnEntry {
            path,
            schema: 0,
            strict: false,
            handler,
        }
    }

    #[test]
    fn distinct_paths_build_ok() {
        let entries = [stub_entry("a"), stub_entry("b")];
        let map = build_entry_map(entries.iter()).expect("distinct paths must build");
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn duplicate_paths_are_rejected() {
        let entries = [stub_entry("dup"), stub_entry("dup")];
        let Err(err) = build_entry_map(entries.iter()) else {
            panic!("collision must be rejected");
        };
        assert!(err.contains("duplicate server fn path 'dup'"), "got: {err}");
    }
}

// -----------------------------------------------------------------------------
// Batch dispatcher (POST /_srv/_batch).
//
// Request body shape:  `[{"path":"add","args":[2,3]}, {"path":"v1/ping","args":[]}]`
// Response body shape: `[{"Ok":5}, {"Ok":"pong"}]` — `Vec<Result<Value, ServerError>>`.
//
// Each entry is dispatched independently against the same inventory
// table used by the single-call route; a missing path or a per-entry
// codec failure becomes that slot's `Err` without affecting the
// rest. The whole batch returns 200; only malformed input (the outer
// array itself) yields a 400.
// -----------------------------------------------------------------------------

#[derive(Deserialize)]
struct BatchInputEntry {
    path: String,
    args: serde_json::Value,
    /// Per-entry wire schema hash. `Option` (+ default) so a client that
    /// doesn't send one is treated as "unknown" (no drift diagnostic),
    /// rather than colliding with a real hash of 0.
    #[serde(default)]
    schema: Option<u64>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum BatchOutputEntry {
    /// Always emitted; carries either `Ok(value)` or `Err(error)`
    /// depending on the handler's outcome. Serialised flat
    /// (untagged) so the wire format matches what
    /// `serde_json::to_vec(&Result<Value, ServerError<Value>>::Ok(v))`
    /// would produce: `{"Ok": v}` / `{"Err": ...}`.
    ///
    /// The error half is `ServerError<Value>`: the dispatcher is
    /// type-erased over each fn's domain error `E`, so it re-parses the
    /// handler's already-serialized error into a generic `Value` payload
    /// rather than the concrete `E` (which it can't name here).
    Result(Result<serde_json::Value, ServerError<serde_json::Value>>),
}

async fn batch_dispatch(headers: HeaderMap, body: Bytes) -> Response {
    let calls: Vec<BatchInputEntry> = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("malformed batch body: {e}"))
                .into_response();
        }
    };

    // Share the headers across entries; each entry gets its own
    // `Context` (its own matched path + a fresh extension map for
    // middleware to populate independently).
    let headers = Arc::new(headers);

    let mut results: Vec<BatchOutputEntry> = Vec::with_capacity(calls.len());
    // Cookies from every entry's handler accumulate onto the single batch
    // response (each entry runs in its own context + jar).
    let mut batch_cookies: Vec<String> = Vec::new();
    for call in calls {
        let entry = match find_entry(&call.path) {
            Some(e) => e,
            None => {
                // Unknown path inside an otherwise-valid batch — one
                // slot's failure, not the whole batch's.
                results.push(BatchOutputEntry::Result(Err(ServerError::Server {
                    status: 404,
                    message: format!("no server fn at path '{}'", call.path),
                })));
                continue;
            }
        };

        // strict_version: reject a mismatched schema for this entry up
        // front (its own slot's failure, not the whole batch's).
        if entry.strict && call.schema != Some(entry.schema) {
            results.push(BatchOutputEntry::Result(Err(ServerError::IncompatibleVersion {
                path: call.path.clone(),
                client_schema: call.schema.unwrap_or(0),
                server_schema: entry.schema,
            })));
            continue;
        }

        let arg_bytes = match serde_json::to_vec(&call.args) {
            Ok(b) => b,
            Err(e) => {
                results.push(BatchOutputEntry::Result(Err(ServerError::Codec(
                    e.to_string(),
                ))));
                continue;
            }
        };

        // Run the middleware chain for this entry; a short-circuit is
        // this slot's failure, not the whole batch's.
        let mut ctx = Context::new(headers.clone(), call.path.clone());
        let jar = crate::cookie::CookieJar::default();
        ctx.insert(jar.clone());
        if let Err(e) = crate::middleware::run_middlewares(&mut ctx).await {
            results.push(BatchOutputEntry::Result(Err(e.into_domain())));
            continue;
        }

        let handler_outcome = CURRENT_CONTEXT
            .scope(ctx, (entry.handler)(arg_bytes))
            .await;
        batch_cookies.extend(jar.take());
        let slot = match handler_outcome {
            Ok(result_bytes) => {
                // The handler returned a JSON-encoded
                // `Result<T, ServerError<E>>`. Re-parse it as
                // `Result<Value, ServerError<Value>>` so we can embed it
                // into the outer array without re-stringifying or naming
                // the concrete `E`.
                match serde_json::from_slice::<Result<serde_json::Value, ServerError<serde_json::Value>>>(
                    &result_bytes,
                ) {
                    Ok(r) => r,
                    Err(e) => Err(ServerError::Codec(e.to_string())),
                }
            }
            // A decode failure the schemas disagree on is version drift.
            Err(TransportError::Codec(_))
                if call.schema.is_some_and(|c| c != entry.schema) =>
            {
                Err(ServerError::IncompatibleVersion {
                    path: call.path.clone(),
                    client_schema: call.schema.unwrap_or(0),
                    server_schema: entry.schema,
                })
            }
            // Transport-level codec failure from the handler ABI; lift it
            // into the type-erased `ServerError<Value>` slot.
            Err(codec_err) => Err(codec_err.into_domain()),
        };
        results.push(BatchOutputEntry::Result(slot));
    }

    let body = match serde_json::to_vec(&results) {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("could not encode batch response: {e}"),
            )
                .into_response();
        }
    };

    let mut resp = (
        StatusCode::OK,
        [("content-type", "application/json")],
        body,
    )
        .into_response();
    apply_set_cookies(&mut resp, batch_cookies);
    resp
}
