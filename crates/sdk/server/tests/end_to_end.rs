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
