//! Server-side machinery: walk the `inventory` of registered server
//! functions at startup, build an axum router, bind a TCP listener.
//!
//! Compiled only when the `server` feature is ON. Pulls in axum +
//! tokio + tower; the client build sees none of this.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::Path;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use serde::{Deserialize, Serialize};

use crate::__private::ServerFnEntry;
use crate::error::ServerError;
use crate::extractors::{RequestContext, REQUEST_CONTEXT};

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
    Router::new()
        // Batch route is declared first so axum's path matcher prefers
        // the exact `/_srv/_batch` over the catch-all `/_srv/*path`.
        // Without the explicit ordering axum still matches correctly
        // (more-specific wins) but being explicit avoids subtle
        // regressions if the route table grows.
        .route("/_srv/_batch", post(batch_dispatch))
        .route("/_srv/*path", post(dispatch))
    // Intentionally no `.fallback(...)`: composing this router with
    // a static-file `ServeDir` (the demo's typical setup) needs the
    // caller to install their own fallback. Unknown server-fn paths
    // are still handled — the catch-all `/_srv/*path` matches them,
    // and `dispatch` returns 404 when no inventory entry is found.
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

    let body_vec = body.to_vec();
    let ctx = RequestContext {
        headers: Arc::new(headers),
    };
    let result = REQUEST_CONTEXT
        .scope(ctx, (entry.handler)(body_vec))
        .await;

    match result {
        Ok(bytes) => (
            StatusCode::OK,
            [("content-type", "application/json")],
            bytes,
        )
            .into_response(),
        // Codec failure inside the handler (decoding args, encoding
        // result) is a 400 — the client sent something the server
        // couldn't make sense of (or vice versa for encoding,
        // technically a 500, but we treat both as schema-drift and
        // surface them under 400 for v0).
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

async fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "not a server fn route").into_response()
}

fn find_entry(path: &str) -> Option<&'static ServerFnEntry> {
    inventory::iter::<ServerFnEntry>
        .into_iter()
        .find(|e| e.path == path)
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
}

#[derive(Serialize)]
#[serde(untagged)]
enum BatchOutputEntry {
    /// Always emitted; carries either `Ok(value)` or `Err(error)`
    /// depending on the handler's outcome. Serialised flat
    /// (untagged) so the wire format matches what
    /// `serde_json::to_vec(&Result<Value, ServerError>::Ok(v))`
    /// would produce: `{"Ok": v}` / `{"Err": ...}`.
    Result(Result<serde_json::Value, ServerError>),
}

async fn batch_dispatch(headers: HeaderMap, body: Bytes) -> Response {
    let calls: Vec<BatchInputEntry> = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("malformed batch body: {e}"))
                .into_response();
        }
    };

    // Build the request context once and re-enter it per handler.
    // Sharing the `Arc<HeaderMap>` avoids cloning headers N times
    // for a batch of N entries.
    let ctx = RequestContext {
        headers: Arc::new(headers),
    };

    let mut results: Vec<BatchOutputEntry> = Vec::with_capacity(calls.len());
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

        let arg_bytes = match serde_json::to_vec(&call.args) {
            Ok(b) => b,
            Err(e) => {
                results.push(BatchOutputEntry::Result(Err(ServerError::Codec(
                    e.to_string(),
                ))));
                continue;
            }
        };

        let handler_outcome = REQUEST_CONTEXT
            .scope(ctx.clone(), (entry.handler)(arg_bytes))
            .await;
        let slot = match handler_outcome {
            Ok(result_bytes) => {
                // The handler returned a JSON-encoded
                // `Result<T, ServerError>`. Re-parse it as
                // `Result<Value, ServerError>` so we can embed it into
                // the outer array without re-stringifying.
                match serde_json::from_slice::<Result<serde_json::Value, ServerError>>(
                    &result_bytes,
                ) {
                    Ok(r) => r,
                    Err(e) => Err(ServerError::Codec(e.to_string())),
                }
            }
            Err(codec_err) => Err(codec_err),
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

    (
        StatusCode::OK,
        [("content-type", "application/json")],
        body,
    )
        .into_response()
}
