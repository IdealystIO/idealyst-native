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
use server_kit::{Auth, Role};

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

/// A tag-guarded endpoint OUTSIDE the `admin/` path prefix and with no
/// `Auth` param: only `require_tag::<Principal>("admin")` stands between
/// an anonymous caller and the body — declarative group membership, no
/// path convention.
#[server::server(tags(admin))]
pub async fn tagged_admin_op() -> Result<String, ServerError> {
    Ok("tagged admin thing".to_string())
}

/// A guarded server→client stream. SSE rides plain HTTP, so this
/// exercises the chain's `on_open` half (the same `ws_open_hook` call
/// site `#[channel]` / `#[subscription]` upgrades run through) without
/// needing a WebSocket client. Gated purely by its TAG (path is outside
/// the `admin/` prefix), proving stream opens carry tags too.
#[server::sse(path = "guarded_ticks", tags(admin))]
pub async fn guarded_ticks() -> impl futures_util::Stream<Item = u32> {
    futures_util::stream::iter(vec![1u32, 2, 3])
}

// ---------------------------------------------------------------------------
// Disjoint role classes (Employee / Employer): neither implies the other,
// both imply Member (the named union). Different logins insert different
// facts; routes declare which facts they need.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Employee(pub String);
#[derive(Clone, Debug, PartialEq)]
pub struct Employer(pub String);
#[derive(Clone, Debug, PartialEq)]
pub struct Member(pub String);

/// Employer-only: the `role` tag protects the set (via role_guard), the
/// `Role` param gives the body typed access.
#[server::server(tags(role = "employer"))]
pub async fn payroll(who: Role<Employer>) -> Result<String, ServerError> {
    Ok(format!("payroll for {}", who.0 .0))
}

/// Employee-only, extractor ONLY (no tag): proves `Role<M>`'s own
/// 401/403 semantics with no perimeter in front.
#[server::server]
pub async fn my_timesheet(who: Role<Employee>) -> Result<String, ServerError> {
    Ok(format!("timesheet for {}", who.0 .0))
}

/// Either class: the union is a named capability both logins insert.
#[server::server(tags(role = "member"))]
pub async fn member_lobby(who: Role<Member>) -> Result<String, ServerError> {
    Ok(format!("welcome {}", who.0 .0))
}

/// A role value nobody registered with role_guard: must fail CLOSED
/// (500 naming the fix), never silently open.
#[server::server(tags(role = "sudoer"))]
pub async fn sudo_op() -> Result<String, ServerError> {
    Ok("must never run".to_string())
}

// ---------------------------------------------------------------------------
// Rate-limited endpoints: the limit is the route's own declaration; the
// key ("one caller") is registered on the middleware.
// ---------------------------------------------------------------------------

/// Two calls per minute per caller (keyed by x-client-id in this suite).
#[server::server(tags(limit = "2/min"))]
pub async fn limited_op() -> Result<String, ServerError> {
    Ok("ok".to_string())
}

/// A limit nobody can parse: must fail CLOSED (500), never unlimited.
#[server::server(tags(limit = "nonsense"))]
pub async fn bad_limit_op() -> Result<String, ServerError> {
    Ok("must never run".to_string())
}

/// A rate-limited stream: the refused OPEN must carry Retry-After via
/// the primitive's response-header jar drain on the rejection path.
#[server::sse(path = "limited_ticks", tags(limit = "1/min"))]
pub async fn limited_ticks() -> impl futures_util::Stream<Item = u32> {
    futures_util::stream::iter(vec![1u32])
}

// App-scoped context resources (the `context!` mechanism): `Motd` is
// installed at boot and resolves; `NeverInstalled` is not, and must fail
// as a clear 500 (server misconfiguration), not a domain error.
server_kit::context! {
    /// The app's message-of-the-day "database".
    pub struct Motd(std::sync::Arc<String>);
    pub struct NeverInstalled(std::sync::Arc<u64>);
}

/// Reads the `Motd` context resource — the `db: Db` pattern in miniature.
#[server::server]
pub async fn read_motd(#[ctx] motd: Motd) -> Result<String, ServerError> {
    Ok(motd.as_str().to_string())
}

/// Reads a context resource nobody installed.
#[server::server]
pub async fn read_never(#[ctx] never: NeverInstalled) -> Result<u64, ServerError> {
    Ok(**never)
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
        // Session guard: validate + INSERT, never reject. Different
        // tokens model different login flows; every valid session also
        // gets the kit's domain-free `Authenticated` fact (what lets
        // Role / role_guard answer 403 instead of 401 for signed-in
        // callers missing a capability).
        server_kit::install_middleware(server_kit::from_fn(|ctx| {
            let token = ctx
                .headers()
                .get("x-token")
                .and_then(|v| v.to_str().ok())
                .map(|t| t.to_string());
            Box::pin(async move {
                match token.as_deref() {
                    Some("sesame") => {
                        ctx.insert(server_kit::Authenticated);
                        ctx.insert(Principal { name: "alice".to_string() });
                    }
                    Some("employee-token") => {
                        ctx.insert(server_kit::Authenticated);
                        ctx.insert(Employee("eve".to_string()));
                        ctx.insert(Member("eve".to_string()));
                    }
                    Some("employer-token") => {
                        ctx.insert(server_kit::Authenticated);
                        ctx.insert(Employer("acme".to_string()));
                        ctx.insert(Member("acme".to_string()));
                    }
                    _ => {}
                }
                Ok(())
            })
        }));
        // Perimeter: deny-by-default for the whole admin/ group.
        server_kit::install_middleware(server_kit::require::<Principal>("admin/"));
        // Declarative perimeter: deny-by-default for everything tagged
        // `admin`, wherever its path lives.
        server_kit::install_middleware(server_kit::require_tag::<Principal>("admin"));
        // Role vocabulary: the app registers which tag values map to
        // which capability types; the kit enforces (401/403/fail-closed).
        server_kit::install_middleware(
            server_kit::role_guard()
                .role::<Employer>("employer")
                .role::<Employee>("employee")
                .role::<Member>("member"),
        );
        // Rate limiting: enforcement is the kit's, "one caller" is ours
        // (x-client-id header here; a real app keys by its principal).
        server_kit::install_middleware(server_kit::rate_limit().key_by(|ctx| {
            ctx.headers()
                .get("x-client-id")
                .and_then(|v| v.to_str().ok())
                .map(|s| format!("client:{s}"))
        }));
    });
}

async fn boot() -> std::net::SocketAddr {
    install_chain_once();
    // Context resource for the `context!` tests (idempotent re-install).
    server::install_state(std::sync::Arc::new("hello from motd".to_string()));
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

#[tokio::test(flavor = "multi_thread")]
async fn require_tag_gates_tagged_fn_without_path_convention() {
    let addr = boot().await;
    // The fn lives at a plain path with no Auth param; only its
    // `tags(admin)` metadata + require_tag stand in the way.
    let resp = post(addr, "tagged_admin_op", "null", None).await;
    assert_eq!(resp.status(), 403, "tag perimeter must deny by default");

    let resp = post(addr, "tagged_admin_op", "null", Some("sesame")).await;
    assert_eq!(resp.status(), 200);
}

// ---------------------------------------------------------------------------
// Disjoint roles: Employee / Employer / the Member union.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn role_guard_distinguishes_401_403_and_grants_the_right_class() {
    let addr = boot().await;

    // Anonymous → 401 (no session at all).
    let resp = post(addr, "payroll", "null", None).await;
    assert_eq!(resp.status(), 401);

    // Signed in as the WRONG class → 403 (authenticated, forbidden).
    let resp = post(addr, "payroll", "null", Some("employee-token")).await;
    assert_eq!(resp.status(), 403);

    // The right class → the body runs with the typed capability.
    let resp = post(addr, "payroll", "null", Some("employer-token")).await;
    assert_eq!(resp.status(), 200);
    let body: Result<String, ServerError> =
        serde_json::from_slice(&resp.bytes().await.unwrap()).unwrap();
    assert_eq!(body.unwrap(), "payroll for acme");
}

#[tokio::test(flavor = "multi_thread")]
async fn role_extractor_alone_is_status_accurate() {
    let addr = boot().await;
    // No tag on this route — only the Role<Employee> param guards it.
    let resp = post(addr, "my_timesheet", "null", None).await;
    assert_eq!(resp.status(), 401, "anonymous → 401 from the extractor");

    let resp = post(addr, "my_timesheet", "null", Some("employer-token")).await;
    assert_eq!(resp.status(), 403, "signed-in wrong class → 403 from the extractor");

    let resp = post(addr, "my_timesheet", "null", Some("employee-token")).await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test(flavor = "multi_thread")]
async fn member_union_admits_both_classes() {
    let addr = boot().await;
    for token in ["employee-token", "employer-token"] {
        let resp = post(addr, "member_lobby", "null", Some(token)).await;
        assert_eq!(resp.status(), 200, "{token} must reach the member union route");
    }
    let resp = post(addr, "member_lobby", "null", None).await;
    assert_eq!(resp.status(), 401);
}

#[tokio::test(flavor = "multi_thread")]
async fn unregistered_role_tag_fails_closed() {
    let addr = boot().await;
    // Even a fully-credentialed caller must NOT reach a route whose
    // role value was never registered — misconfiguration fails closed.
    let resp = post(addr, "sudo_op", "null", Some("employer-token")).await;
    assert_eq!(resp.status(), 500);
    let text = resp.text().await.unwrap();
    assert!(
        text.contains("sudoer") && text.contains("role_guard"),
        "error must name the tag and the fix, got: {text}"
    );
}

// ---------------------------------------------------------------------------
// Rate limiting.
// ---------------------------------------------------------------------------

/// POST with an x-client-id (the registered rate-limit key).
async fn post_id(addr: std::net::SocketAddr, path: &str, id: &str) -> reqwest::Response {
    client()
        .post(format!("http://{addr}/_srv/{path}"))
        .header("content-type", "application/json")
        .header("x-client-id", id)
        .body("null")
        .send()
        .await
        .expect("request")
}

#[tokio::test(flavor = "multi_thread")]
async fn rate_limit_exhausts_then_429_with_retry_after() {
    let addr = boot().await;
    // Capacity 2: two calls pass…
    assert_eq!(post_id(addr, "limited_op", "c-exhaust").await.status(), 200);
    assert_eq!(post_id(addr, "limited_op", "c-exhaust").await.status(), 200);
    // …the third is refused, annotated with when to come back.
    let resp = post_id(addr, "limited_op", "c-exhaust").await;
    assert_eq!(resp.status(), 429);
    let retry: u64 = resp
        .headers()
        .get("retry-after")
        .expect("429 must carry Retry-After")
        .to_str()
        .unwrap()
        .parse()
        .expect("Retry-After must be integer seconds");
    // 2/min = 1 token per 30s; an empty bucket's wait is ≤ 30s.
    assert!((1..=30).contains(&retry), "got Retry-After: {retry}");
}

#[tokio::test(flavor = "multi_thread")]
async fn rate_limit_isolates_callers() {
    let addr = boot().await;
    // Caller A exhausts its own bucket…
    assert_eq!(post_id(addr, "limited_op", "c-a").await.status(), 200);
    assert_eq!(post_id(addr, "limited_op", "c-a").await.status(), 200);
    assert_eq!(post_id(addr, "limited_op", "c-a").await.status(), 429);
    // …caller B is unaffected.
    assert_eq!(post_id(addr, "limited_op", "c-b").await.status(), 200);
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_limit_tag_fails_closed() {
    let addr = boot().await;
    let resp = post_id(addr, "bad_limit_op", "c-bad").await;
    assert_eq!(resp.status(), 500, "unparseable limit must never mean unlimited");
    let text = resp.text().await.unwrap();
    assert!(text.contains("malformed rate limit"), "got: {text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn rate_limited_stream_open_carries_retry_after() {
    let addr = boot().await;
    // No x-client-id → the per-route global bucket; this test owns the
    // path, so the sequence is deterministic. Capacity 1: first open OK.
    let resp = client()
        .get(format!("http://{addr}/_srv/_sse/limited_ticks"))
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), 200);
    // Second open refused — and the rejection response (built on the
    // ws_open_hook drain path) must carry Retry-After.
    let resp = client()
        .get(format!("http://{addr}/_srv/_sse/limited_ticks"))
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), 429);
    assert!(
        resp.headers().get("retry-after").is_some(),
        "refused stream open must carry Retry-After"
    );
}

// ---------------------------------------------------------------------------
// context! resources.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn context_resource_resolves_installed_value() {
    let addr = boot().await;
    let resp = post(addr, "read_motd", "null", None).await;
    assert_eq!(resp.status(), 200);
    let body: Result<String, ServerError> =
        serde_json::from_slice(&resp.bytes().await.unwrap()).unwrap();
    assert_eq!(body.unwrap(), "hello from motd");
}

#[tokio::test(flavor = "multi_thread")]
async fn context_resource_missing_is_clear_500() {
    let addr = boot().await;
    let resp = post(addr, "read_never", "null", None).await;
    assert_eq!(resp.status(), 500, "uninstalled context resource is infrastructure, not domain");
    let text = resp.text().await.unwrap();
    assert!(
        text.contains("NeverInstalled") && text.contains("install_state"),
        "error must name the resource and the fix, got: {text}"
    );
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
        .get(format!("http://{addr}/_srv/_sse/guarded_ticks"))
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), 403, "stream open must be refused by the tag perimeter");

    // Credentialed open → the stream is accepted and produces events.
    let resp = client()
        .get(format!("http://{addr}/_srv/_sse/guarded_ticks"))
        .header("x-token", "sesame")
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), 200);
    let text = resp.text().await.expect("stream body");
    assert!(text.contains("data:"), "expected SSE events, got: {text}");
}
