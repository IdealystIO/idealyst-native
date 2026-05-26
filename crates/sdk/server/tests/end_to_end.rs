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
                            let body_bytes = body
                                .collect()
                                .await
                                .map(|c| c.to_bytes().to_vec())
                                .unwrap_or_default();
                            let call = MockCall {
                                path,
                                body: body_bytes,
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
        server::configure(server::ClientConfig {
            base_url: format!("http://{addr}"),
        });
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
        // Three calls awaited via `join!` from one task — the inline
        // flusher's yield_once gives all three a chance to enqueue
        // before the queue is drained, so the mock should see ONE
        // POST /_srv/_batch (not three single calls).
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

        let (a, b, c) = tokio::join!(
            add(1, 2),
            add(10, 20),
            echo_struct(Echo {
                name: "alice".into(),
                n: 5,
            }),
        );
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
            "three concurrent server-fn calls must coalesce into 1 HTTP request, got {count}"
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
        let (a, b) = tokio::join!(
            add(0, 0),
            echo_struct(Echo {
                name: "y".into(),
                n: 0,
            }),
        );
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

        let (a, b) = tokio::join!(
            server::with_cancel_token(token_a, add(1, 2)),
            server::with_cancel_token(token_b, add(10, 20)),
        );

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
}
