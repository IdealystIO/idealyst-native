//! server-kit end-to-end: the middleware chain mounted on the
//! primitive's DispatchHook slot, exercised over real HTTP.
//!
//! Everything here goes through PUBLIC seams only — `#[server]` fns,
//! `server::router()`, and server-kit's chain — which is the proof the
//! primitive's hook surface is sufficient to build the policy layer
//! outside the primitive.
//!
//! Server-feature only: the fixtures and the router exist on the server
//! build (`cargo test -p server-kit --features server`).
#![cfg(feature = "server")]

use serde::{Deserialize, Serialize};
use server::ServerError;
use server_kit::Auth;

// ---------------------------------------------------------------------------
// Shared fixtures.
// ---------------------------------------------------------------------------

/// The principal the session guard inserts for `x-token: sesame`.
#[derive(Clone, Debug, PartialEq)]
pub struct Principal {
    pub name: String,
}

/// Public endpoint: must keep working with no credentials even though
/// guards are installed (the guard inserts, it never rejects).
#[server::server]
pub async fn public_add(a: i32, b: i32) -> Result<i32, ServerError> {
    Ok(a + b)
}

/// Extractor-protected endpoint: 401s via `Auth<Principal>` when the
/// guard inserted nothing.
#[server::server]
pub async fn whoami_secure(user: Auth<Principal>) -> Result<String, ServerError> {
    Ok(user.name.clone())
}

/// The "forgot the Auth param" case: an admin-group endpoint with NO
/// extractor. The `require::<Principal>("admin/")` perimeter must block
/// it anyway — that is the deny-by-default property the perimeter exists
/// for.
#[server::server(path = "admin/forgot_guard")]
pub async fn admin_forgot_guard() -> Result<String, ServerError> {
    Ok("did the admin thing".to_string())
}

/// A guarded server→client stream. SSE rides plain HTTP, so this
/// exercises the chain's `on_open` half (the same `ws_open_hook` call
/// site `#[channel]` / `#[subscription]` upgrades run through) without
/// needing a WebSocket client.
#[server::sse(path = "admin/guarded_ticks")]
pub async fn guarded_ticks() -> impl futures_util::Stream<Item = u32> {
    futures_util::stream::iter(vec![1u32, 2, 3])
}

// ---------------------------------------------------------------------------
// Boot: install the chain once, serve a fresh listener per test.
// ---------------------------------------------------------------------------

/// Install the session guard + admin perimeter exactly once per process.
/// Registration order is run order: the context-producing guard must
/// precede the context-checking perimeter.
fn install_chain_once() {
    use std::sync::OnceLock;
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        // Session guard: validate + INSERT, never reject.
        server_kit::install_middleware(server_kit::from_fn(|ctx| {
            let authed = ctx
                .headers()
                .get("x-token")
                .and_then(|v| v.to_str().ok())
                .map(|t| t == "sesame")
                .unwrap_or(false);
            Box::pin(async move {
                if authed {
                    ctx.insert(Principal { name: "alice".to_string() });
                }
                Ok(())
            })
        }));
        // Perimeter: deny-by-default for the whole admin/ group.
        server_kit::install_middleware(server_kit::require::<Principal>("admin/"));
    });
}

async fn boot() -> std::net::SocketAddr {
    install_chain_once();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let app = server::router();
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    addr
}

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

async fn post(
    addr: std::net::SocketAddr,
    path: &str,
    args: &str,
    token: Option<&str>,
) -> reqwest::Response {
    let mut req = client()
        .post(format!("http://{addr}/_srv/{path}"))
        .header("content-type", "application/json")
        .body(args.to_string());
    if let Some(t) = token {
        req = req.header("x-token", t);
    }
    req.send().await.expect("request")
}

// ---------------------------------------------------------------------------
// Unary dispatch through the chain.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn public_fn_passes_without_credentials() {
    let addr = boot().await;
    let resp = post(addr, "public_add", "[2,3]", None).await;
    assert_eq!(resp.status(), 200);
    let body: Result<i32, ServerError> =
        serde_json::from_slice(&resp.bytes().await.unwrap()).unwrap();
    assert_eq!(body.unwrap(), 5);
}

#[tokio::test(flavor = "multi_thread")]
async fn auth_extractor_401s_without_guard_insert() {
    let addr = boot().await;
    let resp = post(addr, "whoami_secure", "null", None).await;
    assert_eq!(resp.status(), 401, "missing principal must 401 before the body runs");
}

#[tokio::test(flavor = "multi_thread")]
async fn auth_extractor_resolves_guard_inserted_principal() {
    let addr = boot().await;
    let resp = post(addr, "whoami_secure", "null", Some("sesame")).await;
    assert_eq!(resp.status(), 200);
    let body: Result<String, ServerError> =
        serde_json::from_slice(&resp.bytes().await.unwrap()).unwrap();
    assert_eq!(body.unwrap(), "alice");
}

#[tokio::test(flavor = "multi_thread")]
async fn perimeter_blocks_admin_fn_that_forgot_its_auth_param() {
    let addr = boot().await;
    // No Auth param on the fn — only the require::<Principal>("admin/")
    // perimeter stands between an anonymous caller and the body.
    let resp = post(addr, "admin/forgot_guard", "null", None).await;
    assert_eq!(resp.status(), 403, "perimeter must deny the group by default");

    let resp = post(addr, "admin/forgot_guard", "null", Some("sesame")).await;
    assert_eq!(resp.status(), 200, "credentialed caller passes the perimeter");
}

// ---------------------------------------------------------------------------
// Batch entries run the chain per entry.
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct BatchEntry<'a> {
    path: &'a str,
    args: serde_json::Value,
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
enum Slot {
    Result(Result<serde_json::Value, ServerError<serde_json::Value>>),
}

#[tokio::test(flavor = "multi_thread")]
async fn batch_runs_chain_per_entry() {
    let addr = boot().await;
    // One protected entry (should fail 401 in ITS slot) + one public
    // entry (should succeed) in the same anonymous batch: proves a call
    // can't dodge the hook by riding a batch, and that one entry's
    // rejection doesn't poison the other.
    let body = serde_json::to_string(&vec![
        BatchEntry { path: "whoami_secure", args: serde_json::json!(null) },
        BatchEntry { path: "public_add", args: serde_json::json!([2, 3]) },
    ])
    .unwrap();
    let resp = post(addr, "_batch", &body, None).await;
    assert_eq!(resp.status(), 200);
    let slots: Vec<Slot> = serde_json::from_slice(&resp.bytes().await.unwrap()).unwrap();
    let [Slot::Result(first), Slot::Result(second)] = &slots[..] else {
        panic!("expected two slots, got {slots:?}");
    };
    match first {
        Err(ServerError::Server { status: 401, .. }) => {}
        other => panic!("protected entry must 401 in its slot, got {other:?}"),
    }
    assert_eq!(second.as_ref().unwrap(), &serde_json::json!(5));
}

// ---------------------------------------------------------------------------
// Stream opens run the chain (`on_open`).
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn sse_open_is_gated_by_the_chain() {
    let addr = boot().await;

    // Anonymous open inside the admin/ perimeter → refused before any
    // event is produced.
    let resp = client()
        .get(format!("http://{addr}/_srv/_sse/admin/guarded_ticks"))
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), 403, "stream open must be refused by the perimeter");

    // Credentialed open → the stream is accepted and produces events.
    let resp = client()
        .get(format!("http://{addr}/_srv/_sse/admin/guarded_ticks"))
        .header("x-token", "sesame")
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), 200);
    let text = resp.text().await.expect("stream body");
    assert!(text.contains("data:"), "expected SSE events, got: {text}");
}
