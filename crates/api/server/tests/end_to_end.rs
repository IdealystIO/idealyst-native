//! End-to-end regression tests for the `#[server]` macro + runtime.
//!
//! Because the macro expansion is feature-gated, this file runs in two
//! independent modes:
//!
//! - `cargo test -p server`                         → client mode
//!   The macro emits RPC stubs. Tests boot a hand-rolled hyper
//!   server that emulates the wire protocol, configure the client to
//!   point at it, and call the stubs to verify request shape +
//!   response decoding.
//!
//! - `cargo test -p server --features server`       → server mode
//!   The macro emits the real bodies + inventory registrations.
//!   Tests bind `server::router()` to a random port and hit it with
//!   `net::Client` to verify dispatcher behaviour: path routing,
//!   args decoding, result encoding, error mapping.
//!
//! Both modes share the same `#[server]` fixture definitions below,
//! which proves the symmetry the macro is supposed to guarantee:
//! identical source compiles for either side.

use serde::{Deserialize, Serialize};
use server::{server, ServerError};

// =============================================================================
// Fixtures: server functions exercised by both modes.
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Echo {
    pub name: String,
    pub n: i32,
}

#[server]
pub async fn add(a: i32, b: i32) -> Result<i32, ServerError> {
    Ok(a + b)
}

#[server]
pub async fn divide(a: f64, b: f64) -> Result<f64, ServerError> {
    if b == 0.0 {
        return Err(ServerError::failed("division by zero"));
    }
    Ok(a / b)
}

#[server]
pub async fn echo_struct(input: Echo) -> Result<Echo, ServerError> {
    Ok(Echo {
        name: format!("hello {}", input.name),
        n: input.n * 2,
    })
}

#[server(path = "v1/ping")]
pub async fn ping() -> Result<String, ServerError> {
    Ok("pong".into())
}

/// Demonstrates app-level state extraction. Reads an `Arc<AppName>`
/// out of the state registry and formats a greeting with it.
///
/// The body references `server::use_state`, which only exists when
/// the `server` feature is enabled — but the `#[server]` macro
/// discards this body entirely on the client build (replacing it
/// with an RPC stub), so the symbol is never referenced in client
/// compilation. No cfg gymnastics inside the body required.
#[derive(Debug, Clone)]
pub struct AppName(pub String);

#[server]
pub async fn greet(who: String) -> Result<String, ServerError> {
    let name = server::use_state::<std::sync::Arc<AppName>>()
        .ok_or_else(|| ServerError::failed("AppName not installed"))?;
    Ok(format!("{}: hello {}", name.0, who))
}

/// Demonstrates per-request header extraction.
#[server]
pub async fn whoami() -> Result<String, ServerError> {
    let auth = server::use_request_header("authorization")
        .ok_or_else(|| ServerError::failed("missing authorization header"))?;
    Ok(auth)
}

// -----------------------------------------------------------------------------
// Typed-domain-error fixture (Phase 0).
//
// Proves a `#[server]` fn can return a rich `Result<T, ServerError<E>>`
// whose error half round-trips across the wire as a *structured* value,
// not a stringified blob — the same source compiling on both sides.
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StockError {
    OutOfStock { requested: u32, available: u32 },
    Discontinued,
}

impl std::fmt::Display for StockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StockError::OutOfStock {
                requested,
                available,
            } => write!(
                f,
                "out of stock: requested {requested}, only {available} available"
            ),
            StockError::Discontinued => write!(f, "discontinued"),
        }
    }
}

/// Reserve `qty` units. Returns the reserved count on success, or a
/// typed [`StockError`] when the request can't be satisfied.
#[server]
pub async fn reserve_stock(qty: u32) -> Result<u32, ServerError<StockError>> {
    const AVAILABLE: u32 = 5;
    if qty > AVAILABLE {
        return Err(ServerError::Failed(StockError::OutOfStock {
            requested: qty,
            available: AVAILABLE,
        }));
    }
    Ok(qty)
}

// -----------------------------------------------------------------------------
// Extractor-param fixtures (Phase 1).
//
// `State<T>` / `Headers` are injected server-side and dropped from the
// client stub. Recommended pattern (mirrors a real app): state types +
// extractor imports are gated `#[cfg(feature = "server")]`, since the
// client build never names them — the macro strips the params.
// -----------------------------------------------------------------------------

#[cfg(feature = "server")]
use server::{Auth, Cookies, Headers, State};

#[cfg(feature = "server")]
#[derive(Debug, Clone)]
pub struct Greeting(pub String);

/// `State<Greeting>` is recognized by name (no `#[ctx]` needed) and
/// injected; `who` is the only wire arg. Client stub: `greet_state(who)`.
#[server]
pub async fn greet_state(who: String, cfg: State<Greeting>) -> Result<String, ServerError> {
    // `cfg` derefs `State<Greeting>` → `Greeting`; destructure its string.
    let Greeting(text) = &*cfg;
    Ok(format!("{} {}", text, who))
}

// A distinct state type so this fixture's test doesn't race
// `greet_state` over the shared global `State<Greeting>` registry slot.
#[cfg(feature = "server")]
#[derive(Debug, Clone)]
pub struct Mid(pub String);

/// Mixes wire + injected params in a non-trivial order (wire, ctx, wire,
/// ctx) to exercise positional call ordering in the generated handler.
#[server]
pub async fn decorate(
    prefix: String,
    cfg: State<Mid>,
    suffix: String,
    headers: Headers,
) -> Result<String, ServerError> {
    let tag = headers
        .get("x-tag")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("none")
        .to_string();
    let Mid(text) = &*cfg;
    Ok(format!("{}|{}|{}|{}", prefix, text, suffix, tag))
}

#[cfg(feature = "server")]
#[derive(Debug, Clone)]
pub struct Missing;

/// Reads `State<Missing>` which is never installed — resolution must
/// fail as infrastructure (HTTP 500), not a domain `Failed`.
#[server]
pub async fn needs_missing(x: i32, _m: State<Missing>) -> Result<i32, ServerError> {
    Ok(x)
}

/// A custom extractor resolved via explicit `#[ctx]` — proves the
/// attribute path and open `FromContext` extensibility. Server-only:
/// resolution is a server concern, so the type, its impl, and the fn all
/// live behind the feature.
#[cfg(feature = "server")]
mod custom_extractor {
    use super::{server, ServerError};
    use server::{Context, FromContext, TransportError};

    #[derive(Debug, Clone, PartialEq)]
    pub struct RequestId(pub String);

    impl FromContext for RequestId {
        fn from_context(
            ctx: &Context,
        ) -> impl std::future::Future<Output = Result<Self, TransportError>> + Send {
            // Borrow synchronously, move the owned Option into the async
            // block so the future doesn't borrow `ctx` across the await.
            let id = ctx
                .headers()
                .get("x-request-id")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            async move {
                id.map(RequestId).ok_or_else(|| TransportError::Server {
                    status: 400,
                    message: "missing x-request-id header".into(),
                })
            }
        }
    }

    #[server]
    pub async fn echo_request_id(
        label: String,
        #[ctx] rid: RequestId,
    ) -> Result<String, ServerError> {
        Ok(format!("{}:{}", label, rid.0))
    }
}

// -----------------------------------------------------------------------------
// Middleware / Auth / Cookies fixtures (Phase 2).
// -----------------------------------------------------------------------------

#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq)]
pub struct Principal {
    pub id: u64,
    pub name: String,
}

/// `Auth<Principal>` reads a principal an auth guard (middleware) put
/// into the context. Only a ctx param → client stub is `secure_whoami()`.
#[server]
pub async fn secure_whoami(user: Auth<Principal>) -> Result<String, ServerError> {
    Ok(format!("{}#{}", user.name, user.id))
}

/// `Cookies` parses the request `Cookie` header; always resolves.
#[server]
pub async fn current_theme(cookies: Cookies) -> Result<String, ServerError> {
    Ok(cookies.get("theme").unwrap_or("light").to_string())
}

/// Sets an httpOnly session cookie on the response — the server half of
/// the web BFF auth pattern. The `set_cookie` call is only compiled into
/// the server-side body.
#[cfg(feature = "server")]
#[server]
pub async fn issue_session(token: String) -> Result<(), ServerError> {
    server::set_cookie(server::Cookie::new("session", token));
    Ok(())
}

// -----------------------------------------------------------------------------
// Versioning fixture (Phase 3): a strict-version endpoint.
// -----------------------------------------------------------------------------

/// `strict_version` makes the server reject any client whose wire schema
/// hash differs from this fn's, up front (426), before decoding.
#[server(strict_version)]
pub async fn strict_echo(x: i32) -> Result<i32, ServerError> {
    Ok(x)
}

// =============================================================================
// Server-mode tests: macro emitted real bodies + inventory entries.
// =============================================================================

#[cfg(feature = "server")]
mod server_side {
    use super::*;
    use std::net::SocketAddr;
    use tokio::net::TcpListener;

    /// Boot the inventory-driven router on a random port.
    async fn boot() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = server::router();
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        addr
    }

    #[tokio::test]
    async fn regression_dispatcher_routes_add_to_handler() {
        let addr = boot().await;
        let client = net::Client::new();
        let response = client
            .post(format!("http://{addr}/_srv/add"))
            .body(net::Json(&(2i32, 3i32)))
            .send()
            .await
            .unwrap();
        let result: Result<i32, ServerError> = response.json().await.unwrap();
        assert_eq!(result, Ok(5));
    }

    #[tokio::test]
    async fn set_cookie_reaches_http_response_header() {
        // A handler calling `server::set_cookie` must surface a real
        // `Set-Cookie` header on the HTTP response (the web BFF mechanism).
        let addr = boot().await;
        let client = net::Client::new();
        let response = client
            .post(format!("http://{addr}/_srv/issue_session"))
            .body(net::Json(&("tok-123".to_string(),)))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let set_cookie = response
            .header("set-cookie")
            .expect("Set-Cookie header should be present");
        assert!(set_cookie.contains("session=tok-123"), "got: {set_cookie}");
        // Secure session defaults must be applied.
        assert!(set_cookie.contains("HttpOnly"), "got: {set_cookie}");
        assert!(set_cookie.contains("Secure"), "got: {set_cookie}");
        assert!(set_cookie.contains("SameSite=Lax"), "got: {set_cookie}");
    }

    #[tokio::test]
    async fn regression_user_err_round_trips_as_failed_variant() {
        // A server fn's own `Err(_)` is encoded into the success body
        // (status 200, JSON `{"Err": {...}}`), not as a 4xx/5xx.
        let addr = boot().await;
        let client = net::Client::new();
        let response = client
            .post(format!("http://{addr}/_srv/divide"))
            .body(net::Json(&(1.0f64, 0.0f64)))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let result: Result<f64, ServerError> = response.json().await.unwrap();
        match result {
            Err(ServerError::Failed(msg)) => assert_eq!(msg, "division by zero"),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn regression_struct_arg_and_return_round_trip() {
        let addr = boot().await;
        let client = net::Client::new();
        let input = Echo {
            name: "alice".into(),
            n: 7,
        };
        let response = client
            .post(format!("http://{addr}/_srv/echo_struct"))
            .body(net::Json(&(input,)))
            .send()
            .await
            .unwrap();
        let result: Result<Echo, ServerError> = response.json().await.unwrap();
        assert_eq!(
            result,
            Ok(Echo {
                name: "hello alice".into(),
                n: 14,
            })
        );
    }

    #[tokio::test]
    async fn regression_custom_path_attribute_overrides_default() {
        let addr = boot().await;
        let client = net::Client::new();
        // Default path would be "ping" but we set #[server(path = "v1/ping")].
        let response = client
            .post(format!("http://{addr}/_srv/v1/ping"))
            .body(net::Json(&()))
            .send()
            .await
            .unwrap();
        let result: Result<String, ServerError> = response.json().await.unwrap();
        assert_eq!(result, Ok("pong".into()));
    }

    #[tokio::test]
    async fn regression_unknown_path_returns_404() {
        let addr = boot().await;
        let client = net::Client::new();
        let response = client
            .post(format!("http://{addr}/_srv/does_not_exist"))
            .body(net::Json(&()))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 404);
    }

    #[tokio::test]
    async fn regression_malformed_args_yields_400() {
        // Send a JSON shape the function's args tuple can't decode
        // — `add` expects `(i32, i32)`, not a string.
        let addr = boot().await;
        let client = net::Client::new();
        let response = client
            .post(format!("http://{addr}/_srv/add"))
            .body(net::Json(&"not a tuple"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
    }

    #[tokio::test]
    async fn regression_batch_dispatcher_runs_all_entries_in_order() {
        let addr = boot().await;
        let client = net::Client::new();
        let body = serde_json::json!([
            {"path": "add",     "args": [1, 2]},
            {"path": "add",     "args": [10, 20]},
            // Zero-arg fn: serde encodes the 0-tuple `()` as JSON
            // `null`, NOT as `[]`. The dispatcher decodes args
            // against the function's arg tuple type, so `null`
            // matches `()` and `[]` does not.
            {"path": "v1/ping", "args": null},
        ]);
        let response = client
            .post(format!("http://{addr}/_srv/_batch"))
            .body(net::Json(&body))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let results: Vec<Result<serde_json::Value, ServerError>> =
            response.json().await.unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0], Ok(serde_json::json!(3)));
        assert_eq!(results[1], Ok(serde_json::json!(30)));
        assert_eq!(results[2], Ok(serde_json::json!("pong")));
    }

    #[tokio::test]
    async fn regression_batch_isolates_per_entry_failures() {
        // A batch with one good call + one unknown path + one
        // user-Err return. All three slots must be populated; one
        // entry's failure must not poison the others.
        let addr = boot().await;
        let client = net::Client::new();
        let body = serde_json::json!([
            {"path": "add",     "args": [1, 2]},
            // The slot's "args" shape is irrelevant — the path
            // doesn't exist so the dispatcher rejects before
            // decoding. Using `null` (the canonical 0-arg
            // encoding) here for consistency.
            {"path": "no_such", "args": null},
            {"path": "divide",  "args": [1.0, 0.0]},
        ]);
        let response = client
            .post(format!("http://{addr}/_srv/_batch"))
            .body(net::Json(&body))
            .send()
            .await
            .unwrap();
        let results: Vec<Result<serde_json::Value, ServerError>> =
            response.json().await.unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0], Ok(serde_json::json!(3)));
        match &results[1] {
            Err(ServerError::Server { status, .. }) => assert_eq!(*status, 404),
            other => panic!("expected Server(404), got {other:?}"),
        }
        match &results[2] {
            Err(ServerError::Failed(msg)) => assert_eq!(msg, "division by zero"),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn regression_use_state_reads_installed_value() {
        // App-level state: install once at startup, read inside
        // handlers via `use_state::<T>`.
        server::install_state(std::sync::Arc::new(AppName("idealyst".into())));
        let addr = boot().await;
        let client = net::Client::new();
        let response = client
            .post(format!("http://{addr}/_srv/greet"))
            .body(net::Json(&("world",)))
            .send()
            .await
            .unwrap();
        let result: Result<String, ServerError> = response.json().await.unwrap();
        assert_eq!(result, Ok("idealyst: hello world".into()));
    }

    #[tokio::test]
    async fn regression_use_request_header_reads_incoming_header() {
        // Per-request state: handlers see the headers of the
        // current request via `use_request_header(name)`. The
        // dispatcher scopes them into a task-local before
        // invoking the handler's future.
        let addr = boot().await;
        let client = net::Client::new();
        let response = client
            .post(format!("http://{addr}/_srv/whoami"))
            .header("Authorization", "Bearer s3cr3t")
            .body(net::Json(&()))
            .send()
            .await
            .unwrap();
        let result: Result<String, ServerError> = response.json().await.unwrap();
        assert_eq!(result, Ok("Bearer s3cr3t".into()));
    }

    #[tokio::test]
    async fn regression_use_request_header_missing_surfaces_failed() {
        // The handler's `ok_or_else` path should fire when the
        // header isn't on the request — `Failed` (inside a 200
        // body) since it's the function's own Err, not a
        // dispatcher-level failure.
        let addr = boot().await;
        let client = net::Client::new();
        let response = client
            .post(format!("http://{addr}/_srv/whoami"))
            .body(net::Json(&()))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let result: Result<String, ServerError> = response.json().await.unwrap();
        match result {
            Err(ServerError::Failed(msg)) => {
                assert_eq!(msg, "missing authorization header")
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn regression_batch_propagates_headers_to_each_entry() {
        // Headers set on the outer /_srv/_batch request must be
        // visible inside *every* batched handler. Verify by
        // batching two `whoami` calls; both should pick up the
        // same Authorization header.
        let addr = boot().await;
        let client = net::Client::new();
        let body = serde_json::json!([
            {"path": "whoami", "args": null},
            {"path": "whoami", "args": null},
        ]);
        let response = client
            .post(format!("http://{addr}/_srv/_batch"))
            .header("Authorization", "Bearer batched")
            .body(net::Json(&body))
            .send()
            .await
            .unwrap();
        let results: Vec<Result<String, ServerError>> = response.json().await.unwrap();
        assert_eq!(results.len(), 2);
        for r in results {
            assert_eq!(r, Ok("Bearer batched".into()));
        }
    }

    #[tokio::test]
    async fn regression_batch_malformed_outer_body_yields_400() {
        let addr = boot().await;
        let client = net::Client::new();
        let response = client
            .post(format!("http://{addr}/_srv/_batch"))
            .body(net::Json(&"not a batch array"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
    }

    #[tokio::test]
    async fn regression_typed_domain_error_round_trips_structured() {
        // The dispatcher serializes the handler's `Result<u32,
        // ServerError<StockError>>` whole. The error half must arrive
        // as a *structured* `StockError`, not a stringified blob — this
        // is the Phase 0 typed-error guarantee.
        let addr = boot().await;
        let client = net::Client::new();

        // Err path: a structured domain error inside a 200 body (it's
        // the function's own Err, not a dispatcher-level failure).
        let response = client
            .post(format!("http://{addr}/_srv/reserve_stock"))
            .body(net::Json(&(99u32,)))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let result: Result<u32, ServerError<StockError>> = response.json().await.unwrap();
        assert_eq!(
            result,
            Err(ServerError::Failed(StockError::OutOfStock {
                requested: 99,
                available: 5,
            }))
        );

        // Ok path: the same fn returns the reserved count.
        let response = client
            .post(format!("http://{addr}/_srv/reserve_stock"))
            .body(net::Json(&(3u32,)))
            .send()
            .await
            .unwrap();
        let ok: Result<u32, ServerError<StockError>> = response.json().await.unwrap();
        assert_eq!(ok, Ok(3));
    }

    #[tokio::test]
    async fn regression_state_extractor_injects_installed_value() {
        // A `State<T>` ctx param is resolved from the registry and the
        // wire body carries only the wire arg (`who`), not the state.
        server::install_state(Greeting("hi".to_string()));
        let addr = boot().await;
        let client = net::Client::new();
        let response = client
            .post(format!("http://{addr}/_srv/greet_state"))
            .body(net::Json(&("world".to_string(),)))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let result: Result<String, ServerError> = response.json().await.unwrap();
        assert_eq!(result, Ok("hi world".to_string()));
    }

    #[tokio::test]
    async fn regression_mixed_wire_and_ctx_params_preserve_order() {
        // Params declared wire/ctx/wire/ctx must be passed to the body
        // in the original order — verified by a composite output.
        server::install_state(Mid("MID".to_string()));
        let addr = boot().await;
        let client = net::Client::new();
        let response = client
            .post(format!("http://{addr}/_srv/decorate"))
            // Only the two wire args ride the body, in their relative order.
            .body(net::Json(&("A".to_string(), "B".to_string())))
            .header("x-tag", "TAG")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let result: Result<String, ServerError> = response.json().await.unwrap();
        assert_eq!(result, Ok("A|MID|B|TAG".to_string()));
    }

    #[tokio::test]
    async fn regression_missing_state_surfaces_as_500_not_failed() {
        // `Missing` is never installed; the extractor failure is an
        // infrastructure 500, not a 200-wrapped domain `Failed`.
        let addr = boot().await;
        let client = net::Client::new();
        let response = client
            .post(format!("http://{addr}/_srv/needs_missing"))
            .body(net::Json(&(7i32,)))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 500);
        let body = response.text().await.unwrap_or_default();
        assert!(
            body.contains("State<") && body.contains("not installed"),
            "expected a missing-state message, got: {body:?}"
        );
    }

    #[tokio::test]
    async fn regression_custom_ctx_extractor_resolves_and_rejects() {
        let addr = boot().await;
        let client = net::Client::new();

        // Present: resolves from the header.
        let response = client
            .post(format!("http://{addr}/_srv/echo_request_id"))
            .body(net::Json(&("lbl".to_string(),)))
            .header("x-request-id", "abc123")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let result: Result<String, ServerError> = response.json().await.unwrap();
        assert_eq!(result, Ok("lbl:abc123".to_string()));

        // Absent: the custom extractor's 400 surfaces with its status.
        let response = client
            .post(format!("http://{addr}/_srv/echo_request_id"))
            .body(net::Json(&("lbl".to_string(),)))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
    }

    #[tokio::test]
    async fn regression_auth_guard_injects_principal_and_rejects() {
        // A path-scoped guard: only enforces on `secure_whoami`, so the
        // other server-mode tests' endpoints (which share this global
        // middleware chain) pass straight through.
        server::install_middleware(server::from_fn(|ctx| {
            Box::pin(async move {
                if ctx.path() != "secure_whoami" {
                    return Ok(());
                }
                // Own the token before mutating ctx, so the header borrow
                // is released before `ctx.insert`.
                let token = ctx
                    .headers()
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string());
                match token.as_deref() {
                    Some("Bearer good") => {
                        ctx.insert(Principal {
                            id: 42,
                            name: "alice".to_string(),
                        });
                        Ok(())
                    }
                    _ => Err(server::TransportError::Server {
                        status: 401,
                        message: "unauthorized".into(),
                    }),
                }
            })
        }));

        let addr = boot().await;
        let client = net::Client::new();

        // Authenticated → guard injects the principal, handler reads it.
        let response = client
            .post(format!("http://{addr}/_srv/secure_whoami"))
            .body(net::Json(&()))
            .header("authorization", "Bearer good")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let result: Result<String, ServerError> = response.json().await.unwrap();
        assert_eq!(result, Ok("alice#42".to_string()));

        // Unauthenticated → guard short-circuits 401; handler never runs.
        let response = client
            .post(format!("http://{addr}/_srv/secure_whoami"))
            .body(net::Json(&()))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 401);
    }

    #[tokio::test]
    async fn regression_cookies_extractor_reads_request_cookie() {
        let addr = boot().await;
        let client = net::Client::new();
        let response = client
            .post(format!("http://{addr}/_srv/current_theme"))
            .body(net::Json(&()))
            .header("cookie", "theme=dark; session=xyz")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let result: Result<String, ServerError> = response.json().await.unwrap();
        assert_eq!(result, Ok("dark".to_string()));

        // No cookie → handler's default.
        let response = client
            .post(format!("http://{addr}/_srv/current_theme"))
            .body(net::Json(&()))
            .send()
            .await
            .unwrap();
        let result: Result<String, ServerError> = response.json().await.unwrap();
        assert_eq!(result, Ok("light".to_string()));
    }

    #[tokio::test]
    async fn regression_strict_version_gates_on_schema_hash() {
        let addr = boot().await;
        let client = net::Client::new();

        // The fn's real wire schema hash (what a matching client sends).
        let good = server::schema_for("strict_echo").expect("strict_echo registered");

        // Matching hash → 200, runs normally.
        let response = client
            .post(format!("http://{addr}/_srv/strict_echo"))
            .header("x-srv-schema", format!("{good:x}"))
            .body(net::Json(&(5i32,)))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let result: Result<i32, ServerError> = response.json().await.unwrap();
        assert_eq!(result, Ok(5));

        // Mismatched hash → 426 up front (body never decoded).
        let response = client
            .post(format!("http://{addr}/_srv/strict_echo"))
            .header("x-srv-schema", "deadbeef")
            .body(net::Json(&(5i32,)))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 426);

        // Absent hash → also 426 (a strict endpoint requires a match).
        let response = client
            .post(format!("http://{addr}/_srv/strict_echo"))
            .body(net::Json(&(5i32,)))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 426);
    }

    #[tokio::test]
    async fn regression_arg_decode_drift_yields_426_not_400() {
        // Non-strict `add`: malformed args that can't decode. With a
        // mismatched schema header it's attributed to drift (426); with
        // no header it's an ordinary codec error (400).
        let addr = boot().await;
        let client = net::Client::new();

        // Body that won't decode as (i32, i32) + a wrong schema → 426.
        let response = client
            .post(format!("http://{addr}/_srv/add"))
            .header("x-srv-schema", "deadbeef")
            .body(net::Json(&("not", "ints")))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 426);

        // Same malformed body, no schema header → plain 400.
        let response = client
            .post(format!("http://{addr}/_srv/add"))
            .body(net::Json(&("not", "ints")))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
    }
}

// =============================================================================
// Client-mode tests: macro emitted RPC stubs.
// =============================================================================

#[cfg(not(feature = "server"))]
mod client_side {
    use super::*;
    use std::convert::Infallible;
    use std::sync::{Arc, Mutex};

    use http_body_util::{BodyExt, Full};
    use hyper::body::{Bytes, Incoming};
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Request, Response, StatusCode};
    use hyper_util::rt::TokioIo;
    use tokio::net::TcpListener;

    /// Captured invocation against the mock server.
    #[derive(Debug, Clone)]
    struct MockCall {
        path: String,
        body: Vec<u8>,
        headers: Vec<(String, String)>,
    }

    impl MockCall {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        }
    }

    type Replier =
        Arc<dyn Fn(&MockCall) -> (StatusCode, Vec<u8>) + Send + Sync + 'static>;

    struct MockServer {
        addr: std::net::SocketAddr,
        calls: Arc<Mutex<Vec<MockCall>>>,
    }

    impl MockServer {
        fn calls(&self) -> Vec<MockCall> {
            self.calls.lock().unwrap().clone()
        }
    }

    async fn mock_server(reply: Replier) -> MockServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let calls: Arc<Mutex<Vec<MockCall>>> = Arc::new(Mutex::new(Vec::new()));
        let calls_task = calls.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let calls = calls_task.clone();
                let reply = reply.clone();
                tokio::spawn(async move {
                    let svc = service_fn(move |req: Request<Incoming>| {
                        let calls = calls.clone();
                        let reply = reply.clone();
                        async move {
                            let (parts, body) = req.into_parts();
                            // Strip "/_srv/" prefix so tests see just the
                            // server-fn path, matching what the macro
                            // produced.
                            let path = parts
                                .uri
                                .path()
                                .strip_prefix("/_srv/")
                                .unwrap_or(parts.uri.path())
                                .to_string();
                            let headers = parts
                                .headers
                                .iter()
                                .filter_map(|(k, v)| {
                                    v.to_str().ok().map(|v| (k.as_str().to_string(), v.to_string()))
                                })
                                .collect();
                            let body_bytes = body
                                .collect()
                                .await
                                .map(|c| c.to_bytes().to_vec())
                                .unwrap_or_default();
                            let call = MockCall {
                                path,
                                body: body_bytes,
                                headers,
                            };
                            let (status, reply_body) = (reply)(&call);
                            calls.lock().unwrap().push(call);
                            Ok::<_, Infallible>(
                                Response::builder()
                                    .status(status)
                                    .header("content-type", "application/json")
                                    .body(Full::new(Bytes::from(reply_body)))
                                    .unwrap(),
                            )
                        }
                    });
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), svc)
                        .await;
                });
            }
        });
        MockServer { addr, calls }
    }

    /// Serialises client-mode tests against the process-global
    /// `server::CONFIG`. Each test acquires the lock before
    /// configuring + calling, and holds it until the test ends —
    /// otherwise concurrent tests would clobber each other's
    /// base-url and call against the wrong mock.
    static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    async fn configure_for(
        addr: std::net::SocketAddr,
    ) -> tokio::sync::MutexGuard<'static, ()> {
        let guard = TEST_LOCK.lock().await;
        server::configure(server::ClientConfig::new(format!("http://{addr}")));
        guard
    }

    /// Like [`configure_for`] but takes a fully-built config (e.g. one
    /// carrying a credential provider).
    async fn configure_with(config: server::ClientConfig) -> tokio::sync::MutexGuard<'static, ()> {
        let guard = TEST_LOCK.lock().await;
        server::configure(config);
        guard
    }

    #[tokio::test]
    async fn regression_stub_serialises_args_as_json_tuple() {
        let mock = mock_server(Arc::new(|_call| {
            // Reply with Ok(5) as a Result<i32, ServerError>.
            let body = serde_json::to_vec(&Result::<i32, ServerError>::Ok(5)).unwrap();
            (StatusCode::OK, body)
        }))
        .await;
        let _guard = configure_for(mock.addr).await;

        let result = add(2, 3).await;
        assert_eq!(result, Ok(5));

        let call = &mock.calls()[0];
        assert_eq!(call.path, "add");
        // Args wire format is a JSON array (a serde tuple).
        let decoded: (i32, i32) = serde_json::from_slice(&call.body).unwrap();
        assert_eq!(decoded, (2, 3));
    }

    #[tokio::test]
    async fn regression_stub_decodes_user_err_into_result() {
        let mock = mock_server(Arc::new(|_call| {
            let body = serde_json::to_vec(
                &Result::<f64, ServerError>::Err(ServerError::failed("nope")),
            )
            .unwrap();
            (StatusCode::OK, body)
        }))
        .await;
        let _guard = configure_for(mock.addr).await;

        let result = divide(1.0, 0.0).await;
        match result {
            Err(ServerError::Failed(msg)) => assert_eq!(msg, "nope"),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn regression_stub_folds_non_2xx_into_server_error() {
        let mock = mock_server(Arc::new(|_call| {
            (StatusCode::INTERNAL_SERVER_ERROR, b"boom".to_vec())
        }))
        .await;
        let _guard = configure_for(mock.addr).await;

        let result = add(1, 1).await;
        match result {
            Err(ServerError::Server { status, message }) => {
                assert_eq!(status, 500);
                assert_eq!(message, "boom");
            }
            other => panic!("expected Server, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn regression_stub_uses_custom_path_attribute() {
        let mock = mock_server(Arc::new(|_call| {
            let body = serde_json::to_vec(&Result::<String, ServerError>::Ok("pong".into()))
                .unwrap();
            (StatusCode::OK, body)
        }))
        .await;
        let _guard = configure_for(mock.addr).await;

        let _ = ping().await;
        assert_eq!(mock.calls()[0].path, "v1/ping");
    }

    #[tokio::test]
    async fn regression_stub_struct_args_round_trip() {
        let mock = mock_server(Arc::new(|call| {
            // Decode the incoming arg tuple and reply with the
            // transformed Echo to verify both halves of the codec.
            let (input,): (Echo,) = serde_json::from_slice(&call.body).unwrap();
            let body = serde_json::to_vec(&Result::<Echo, ServerError>::Ok(Echo {
                name: format!("hello {}", input.name),
                n: input.n * 2,
            }))
            .unwrap();
            (StatusCode::OK, body)
        }))
        .await;
        let _guard = configure_for(mock.addr).await;

        let result = echo_struct(Echo {
            name: "alice".into(),
            n: 7,
        })
        .await;
        assert_eq!(
            result,
            Ok(Echo {
                name: "hello alice".into(),
                n: 14
            })
        );
    }

    // -----------------------------------------------------------------
    // Reactive integration — feeds the stub through `mutation()` and
    // `resource()` from runtime-core. Proves the user-facing
    // "Signal<NetworkState>" experience actually works.
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn regression_concurrent_calls_coalesce_into_single_batch_request() {
        // Three calls awaited via `join!` INSIDE a `server::batch(...)`
        // scope — the inline flusher's yield_once gives all three a chance
        // to enqueue before the queue is drained, so the mock should see
        // ONE POST /_srv/_batch (not three single calls). Coalescing is
        // opt-in: without the batch scope these would be three direct
        // requests (see regression_direct_by_default_no_coalescing).
        let request_count = Arc::new(Mutex::new(0u32));
        let request_count_clone = request_count.clone();
        let mock = mock_server(Arc::new(move |call| {
            *request_count_clone.lock().unwrap() += 1;
            // Expect the path to be "_batch" (single calls would
            // arrive as their function name).
            assert_eq!(call.path, "_batch", "calls must coalesce to /_srv/_batch");

            // Echo back results in the same order, matching what the
            // server-side dispatcher would produce.
            let entries: Vec<serde_json::Value> = serde_json::from_slice(&call.body).unwrap();
            let results: Vec<Result<serde_json::Value, ServerError>> = entries
                .iter()
                .map(|e| {
                    let path = e["path"].as_str().unwrap();
                    let args = &e["args"];
                    match path {
                        "add" => {
                            let (a, b): (i32, i32) = serde_json::from_value(args.clone()).unwrap();
                            Ok(serde_json::json!(a + b))
                        }
                        "echo_struct" => {
                            let (input,): (Echo,) =
                                serde_json::from_value(args.clone()).unwrap();
                            Ok(serde_json::to_value(Echo {
                                name: format!("hello {}", input.name),
                                n: input.n * 2,
                            })
                            .unwrap())
                        }
                        _ => panic!("unexpected path {path} in batch"),
                    }
                })
                .collect();
            let body = serde_json::to_vec(&results).unwrap();
            (StatusCode::OK, body)
        }))
        .await;
        let _guard = configure_for(mock.addr).await;

        let (a, b, c) = server::batch(async {
            tokio::join!(
                add(1, 2),
                add(10, 20),
                echo_struct(Echo {
                    name: "alice".into(),
                    n: 5,
                }),
            )
        })
        .await;
        assert_eq!(a, Ok(3));
        assert_eq!(b, Ok(30));
        assert_eq!(
            c,
            Ok(Echo {
                name: "hello alice".into(),
                n: 10
            })
        );

        let count = *request_count.lock().unwrap();
        assert_eq!(
            count, 1,
            "three concurrent server-fn calls in a batch scope must coalesce into 1 HTTP request, got {count}"
        );
    }

    #[tokio::test]
    async fn regression_direct_by_default_no_coalescing() {
        // OUTSIDE a batch scope, three concurrent calls must each be their
        // own direct POST /_srv/<path> — never coalesced. Proves batching
        // is opt-in.
        let request_count = Arc::new(Mutex::new(0u32));
        let request_count_clone = request_count.clone();
        let mock = mock_server(Arc::new(move |call| {
            *request_count_clone.lock().unwrap() += 1;
            assert_ne!(call.path, "_batch", "calls must NOT coalesce outside a batch scope");
            let body = serde_json::to_vec(&Result::<i32, ServerError>::Ok(0)).unwrap();
            (StatusCode::OK, body)
        }))
        .await;
        let _guard = configure_for(mock.addr).await;

        let _ = tokio::join!(add(1, 2), add(3, 4), add(5, 6));

        let count = *request_count.lock().unwrap();
        assert_eq!(
            count, 3,
            "three concurrent calls without a batch scope must be 3 direct requests, got {count}"
        );
    }

    #[tokio::test]
    async fn regression_solo_call_uses_single_call_path_not_batch() {
        // A call without siblings flushes alone — the queue has size 1
        // when the flusher takes it, so the wire is POST /_srv/add,
        // not /_srv/_batch. Verifies the solo fast-path.
        let mock = mock_server(Arc::new(|call| {
            assert_eq!(
                call.path, "add",
                "solo call must use single-call wire, not /_srv/_batch"
            );
            let body = serde_json::to_vec(&Result::<i32, ServerError>::Ok(99)).unwrap();
            (StatusCode::OK, body)
        }))
        .await;
        let _guard = configure_for(mock.addr).await;
        let result = add(1, 2).await;
        assert_eq!(result, Ok(99));
    }

    #[tokio::test]
    async fn regression_batch_dispatch_decoded_back_to_typed_returns() {
        // Two calls of DIFFERENT return types (i32 + Echo) are
        // batched; each must be decoded back to its own typed
        // Result<T, ServerError> on the client side.
        let mock = mock_server(Arc::new(|call| {
            assert_eq!(call.path, "_batch");
            let entries: Vec<serde_json::Value> = serde_json::from_slice(&call.body).unwrap();
            // Reply in matching order.
            let results: Vec<Result<serde_json::Value, ServerError>> = entries
                .iter()
                .map(|e| match e["path"].as_str().unwrap() {
                    "add" => Ok(serde_json::json!(7)),
                    "echo_struct" => Ok(serde_json::json!({"name": "x", "n": 42})),
                    _ => panic!(),
                })
                .collect();
            (StatusCode::OK, serde_json::to_vec(&results).unwrap())
        }))
        .await;
        let _guard = configure_for(mock.addr).await;
        let (a, b) = server::batch(async {
            tokio::join!(
                add(0, 0),
                echo_struct(Echo {
                    name: "y".into(),
                    n: 0,
                }),
            )
        })
        .await;
        assert_eq!(a, Ok(7));
        assert_eq!(
            b,
            Ok(Echo {
                name: "x".into(),
                n: 42
            })
        );
    }

    // -----------------------------------------------------------------
    // Cancellation flow — `with_cancel_token` + batch+cancel interop.
    // -----------------------------------------------------------------

    /// A pre-cancelled token short-circuits the call: no HTTP request
    /// is made and the caller receives `ServerError::Cancelled`.
    #[tokio::test]
    async fn regression_cancel_before_enqueue_makes_no_http_request() {
        let request_count = Arc::new(Mutex::new(0u32));
        let request_count_clone = request_count.clone();
        let mock = mock_server(Arc::new(move |_call| {
            *request_count_clone.lock().unwrap() += 1;
            let body = serde_json::to_vec(&Result::<i32, ServerError>::Ok(5)).unwrap();
            (StatusCode::OK, body)
        }))
        .await;
        let _guard = configure_for(mock.addr).await;

        let (handle, token) = net::cancel_token();
        handle.cancel(); // pre-cancel

        let result = server::with_cancel_token(token, add(2, 3)).await;
        match result {
            Err(ServerError::Cancelled) => {}
            other => panic!("expected Cancelled, got {other:?}"),
        }
        assert_eq!(
            *request_count.lock().unwrap(),
            0,
            "pre-cancelled call must not hit the network"
        );
    }

    /// In a two-call concurrent join, pre-cancelling one of them
    /// leaves the other to make a solo HTTP request (single-call
    /// wire, not /_srv/_batch) — the cancelled call is filtered
    /// out before flush.
    #[tokio::test]
    async fn regression_cancelled_call_filtered_from_batch_before_flush() {
        let request_count = Arc::new(Mutex::new(0u32));
        let request_paths: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let request_count_clone = request_count.clone();
        let request_paths_clone = request_paths.clone();
        let mock = mock_server(Arc::new(move |call| {
            *request_count_clone.lock().unwrap() += 1;
            request_paths_clone.lock().unwrap().push(call.path.clone());
            let body = match call.path.as_str() {
                "_batch" => {
                    let entries: Vec<serde_json::Value> =
                        serde_json::from_slice(&call.body).unwrap();
                    let results: Vec<Result<serde_json::Value, ServerError>> = entries
                        .iter()
                        .map(|e| {
                            let (a, b): (i32, i32) =
                                serde_json::from_value(e["args"].clone()).unwrap();
                            Ok(serde_json::json!(a + b))
                        })
                        .collect();
                    serde_json::to_vec(&results).unwrap()
                }
                "add" => {
                    let (a, b): (i32, i32) = serde_json::from_slice(&call.body).unwrap();
                    serde_json::to_vec(&Result::<i32, ServerError>::Ok(a + b)).unwrap()
                }
                other => panic!("unexpected path {other}"),
            };
            (StatusCode::OK, body)
        }))
        .await;
        let _guard = configure_for(mock.addr).await;

        let (handle_a, token_a) = net::cancel_token();
        let (_handle_b, token_b) = net::cancel_token();
        handle_a.cancel(); // A short-circuits before enqueue

        let (a, b) = server::batch(async {
            tokio::join!(
                server::with_cancel_token(token_a, add(1, 2)),
                server::with_cancel_token(token_b, add(10, 20)),
            )
        })
        .await;

        match a {
            Err(ServerError::Cancelled) => {}
            other => panic!("A expected Cancelled, got {other:?}"),
        }
        assert_eq!(b, Ok(30));

        // Only ONE HTTP request should have been made — the
        // solo-call path for B. (Pre-cancelled A never enqueued.)
        assert_eq!(*request_count.lock().unwrap(), 1);
        assert_eq!(
            request_paths.lock().unwrap().as_slice(),
            &["add".to_string()],
        );
    }

    /// Cancelling a solo in-flight HTTP request via the cancel
    /// token aborts the underlying transport — `net`'s
    /// `cancel_on` race wins and the caller sees Cancelled before
    /// the slow mock would have responded.
    #[tokio::test]
    async fn regression_cancel_aborts_solo_in_flight_http() {
        use http_body_util::Full;
        use hyper::body::Bytes;
        use hyper::server::conn::http1;
        use hyper::service::service_fn;
        use hyper::{Response, StatusCode};
        use hyper_util::rt::TokioIo;
        use tokio::net::TcpListener;

        // Slow mock: holds the response for ~5s. The cancel must
        // beat that.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let svc = service_fn(|_req| async {
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        let body = serde_json::to_vec(&Result::<i32, ServerError>::Ok(99))
                            .unwrap();
                        Ok::<_, std::convert::Infallible>(
                            Response::builder()
                                .status(StatusCode::OK)
                                .header("content-type", "application/json")
                                .body(Full::new(Bytes::from(body)))
                                .unwrap(),
                        )
                    });
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), svc)
                        .await;
                });
            }
        });

        let _guard = configure_for(addr).await;
        let (handle, token) = net::cancel_token();

        // Spawn the call so we can fire cancel concurrently. The
        // task captures `token` via with_cancel_token; the handle
        // stays here to trigger cancellation from this test thread.
        let task = tokio::spawn(server::with_cancel_token(token, add(1, 2)));
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        handle.cancel();

        let start = std::time::Instant::now();
        let result = task.await.unwrap();
        let elapsed = start.elapsed();

        match result {
            Err(ServerError::Cancelled) => {}
            other => panic!("expected Cancelled, got {other:?}"),
        }
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "cancel must abort promptly, elapsed = {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn regression_mutation_wraps_server_fn_callback() {
        use runtime_core::{mutation, NetworkState};

        let mock = mock_server(Arc::new(|_call| {
            let body = serde_json::to_vec(&Result::<i32, ServerError>::Ok(42)).unwrap();
            (StatusCode::OK, body)
        }))
        .await;
        let _guard = configure_for(mock.addr).await;

        let m = mutation::<(i32, i32), i32, ServerError, _, _>(|(a, b)| async move {
            add(a, b).await
        });
        assert_eq!(m.network_state(), NetworkState::Idle);

        let _ = m.run((2, 3)).await;
        match m.network_state() {
            NetworkState::Success(v) => assert_eq!(v, 42),
            other => panic!("expected Success, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn regression_stub_decodes_typed_domain_error() {
        // The client stub must decode a *structured* `ServerError<E>`
        // out of the response, reconstructing the exact `StockError`
        // variant + fields — not a `Failed(String)`. This is the
        // client half of the Phase 0 typed-error guarantee.
        let mock = mock_server(Arc::new(|_call| {
            let body = serde_json::to_vec(
                &Result::<u32, ServerError<StockError>>::Err(ServerError::Failed(
                    StockError::OutOfStock {
                        requested: 99,
                        available: 5,
                    },
                )),
            )
            .unwrap();
            (StatusCode::OK, body)
        }))
        .await;
        let _guard = configure_for(mock.addr).await;

        let result = reserve_stock(99).await;
        assert_eq!(
            result,
            Err(ServerError::Failed(StockError::OutOfStock {
                requested: 99,
                available: 5,
            }))
        );
    }

    #[tokio::test]
    async fn regression_client_stub_omits_ctx_params() {
        // `greet_state(who, cfg: State<Greeting>)` compiles on the
        // client to `greet_state(who)` — the extractor param is gone
        // from the signature AND from the wire body. Calling it with a
        // single arg is the compile-time proof; the body assertion is
        // the runtime proof.
        let mock = mock_server(Arc::new(|_call| {
            let body =
                serde_json::to_vec(&Result::<String, ServerError>::Ok("hi world".into())).unwrap();
            (StatusCode::OK, body)
        }))
        .await;
        let _guard = configure_for(mock.addr).await;

        let result = greet_state("world".to_string()).await;
        assert_eq!(result, Ok("hi world".to_string()));

        // The wire body is the one-element tuple of wire args only — no
        // slot for the injected `State<Greeting>`.
        let call = &mock.calls()[0];
        assert_eq!(call.path, "greet_state");
        let decoded: (String,) = serde_json::from_slice(&call.body).unwrap();
        assert_eq!(decoded, ("world".to_string(),));
    }

    #[tokio::test]
    async fn regression_response_schema_drift_yields_incompatible_version() {
        // A server that replies 200 with a return-schema header (0) that
        // differs from what the stub advertised, plus a body that doesn't
        // decode into the stub's Ret. The client must report a precise
        // IncompatibleVersion — the "your app is outdated" signal — not a
        // vague codec error. (mock_server can't set response headers, so
        // this uses a bespoke one that does.)
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let svc = service_fn(|_req: Request<Incoming>| async move {
                        // 200 + a body that's not a valid i32, + a return
                        // schema of 0 (≠ add's real, nonzero hash).
                        let body =
                            serde_json::to_vec(&serde_json::json!({ "Ok": "not-an-int" })).unwrap();
                        Ok::<_, Infallible>(
                            Response::builder()
                                .status(StatusCode::OK)
                                .header("content-type", "application/json")
                                .header("x-srv-schema", "0")
                                .body(Full::new(Bytes::from(body)))
                                .unwrap(),
                        )
                    });
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), svc)
                        .await;
                });
            }
        });
        let _guard = configure_for(addr).await;

        match add(1, 2).await {
            Err(ServerError::IncompatibleVersion {
                client_schema,
                server_schema,
                ..
            }) => {
                assert_ne!(client_schema, server_schema);
                assert_eq!(server_schema, 0);
            }
            other => panic!("expected IncompatibleVersion, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn regression_credential_provider_attaches_bearer_header() {
        // A configured credential source must attach its header to every
        // outgoing request — the client half of the auth story (the
        // server half is the Phase 2 guard).
        let mock = mock_server(Arc::new(|_call| {
            let body = serde_json::to_vec(&Result::<i32, ServerError>::Ok(1)).unwrap();
            (StatusCode::OK, body)
        }))
        .await;
        let config = server::ClientConfig::new(format!("http://{}", mock.addr))
            .with_credentials(server::bearer(|| Some("xyz".to_string())));
        let _guard = configure_with(config).await;

        let _ = add(1, 2).await;

        let call = &mock.calls()[0];
        assert_eq!(call.header("authorization"), Some("Bearer xyz"));
    }
}
