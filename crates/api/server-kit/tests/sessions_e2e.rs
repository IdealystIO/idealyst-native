//! Sessions + observability end-to-end: the productized login-demo flow
//! (cookie web BFF + bearer native) over real HTTP, riding a
//! MemoryCache session store, with observer records asserted alongside.
#![cfg(feature = "server")]

use std::sync::{Arc, Mutex, OnceLock};

use server::{ServerError, State};
use server_kit::{Auth, Outcome, Sessions};

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Principal {
    pub username: String,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct LoginOk {
    pub token: Option<String>,
}

// ---------------------------------------------------------------------------
// Fixtures: the whole auth surface an app writes with kit sessions.
// ---------------------------------------------------------------------------

/// The app's half of login is ONLY the credential check.
#[server::server]
pub async fn login(
    username: String,
    password: String,
    sessions: State<Sessions>,
) -> Result<LoginOk, ServerError> {
    if !(username == "demo" && password == "password") {
        return Err(ServerError::failed("invalid credentials"));
    }
    let ticket = sessions
        .start(&Principal { username })
        .await
        .map_err(|e| ServerError::failed(e.to_string()))?;
    Ok(LoginOk { token: ticket.token })
}

#[server::server]
pub async fn me(user: Auth<Principal>) -> Result<String, ServerError> {
    Ok(user.username.clone())
}

#[server::server]
pub async fn logout(sessions: State<Sessions>) -> Result<(), ServerError> {
    sessions.end().await.map_err(|e| ServerError::failed(e.to_string()))
}

// ---------------------------------------------------------------------------
// Boot: sessions over a MemoryCache + an observer capturing records.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Seen {
    path: String,
    outcome: Outcome,
}

fn records() -> &'static Mutex<Vec<Seen>> {
    static R: OnceLock<Mutex<Vec<Seen>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Vec::new()))
}

fn install_once() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let sessions = Sessions::new(cache::MemoryCache::new());
        server::install_state(sessions.clone());
        server_kit::install_middleware(sessions.guard::<Principal>());
        server_kit::install_observer(|r| {
            records()
                .lock()
                .unwrap()
                .push(Seen { path: r.path.to_string(), outcome: r.outcome });
        });
    });
}

async fn boot() -> std::net::SocketAddr {
    install_once();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let app = server::router();
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    addr
}

async fn post(
    addr: std::net::SocketAddr,
    path: &str,
    body: &str,
    headers: &[(&str, &str)],
) -> reqwest::Response {
    let mut req = reqwest::Client::new()
        .post(format!("http://{addr}/_srv/{path}"))
        .header("content-type", "application/json")
        .body(body.to_string());
    for (name, value) in headers {
        req = req.header(*name, *value);
    }
    req.send().await.expect("request")
}

// ---------------------------------------------------------------------------
// Web flow: httpOnly cookie, secret never in the body.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn web_cookie_login_me_logout_round_trip() {
    let addr = boot().await;

    // Login (web: no native header) → cookie set, NO token in the body.
    let resp = post(addr, "login", r#"["demo","password"]"#, &[]).await;
    assert_eq!(resp.status(), 200);
    let set_cookie = resp
        .headers()
        .get("set-cookie")
        .expect("web login must set the session cookie")
        .to_str()
        .unwrap()
        .to_string();
    assert!(set_cookie.starts_with("session="), "got: {set_cookie}");
    assert!(set_cookie.contains("HttpOnly"), "session cookie must be httpOnly");
    let body: Result<LoginOk, ServerError> =
        serde_json::from_slice(&resp.bytes().await.unwrap()).unwrap();
    assert_eq!(body.unwrap().token, None, "web must never receive the secret in the body");

    // Replay the cookie → authenticated.
    let cookie_pair = set_cookie.split(';').next().unwrap().to_string();
    let resp = post(addr, "me", "null", &[("cookie", &cookie_pair)]).await;
    assert_eq!(resp.status(), 200);
    let body: Result<String, ServerError> =
        serde_json::from_slice(&resp.bytes().await.unwrap()).unwrap();
    assert_eq!(body.unwrap(), "demo");

    // Logout → session deleted server-side and the cookie cleared.
    let resp = post(addr, "logout", "null", &[("cookie", &cookie_pair)]).await;
    assert_eq!(resp.status(), 200);
    let cleared = resp.headers().get("set-cookie").expect("logout must clear the cookie");
    assert!(cleared.to_str().unwrap().contains("Max-Age=0"), "got: {cleared:?}");

    // The old cookie is dead: the deleted session no longer authenticates.
    let resp = post(addr, "me", "null", &[("cookie", &cookie_pair)]).await;
    assert_eq!(resp.status(), 401, "a logged-out session must not authenticate");
}

// ---------------------------------------------------------------------------
// Native flow: bearer token for the OS keystore.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn native_bearer_login_me_logout_round_trip() {
    let addr = boot().await;

    let resp = post(
        addr,
        "login",
        r#"["demo","password"]"#,
        &[("x-idealyst-client", "native")],
    )
    .await;
    assert_eq!(resp.status(), 200);
    let body: Result<LoginOk, ServerError> =
        serde_json::from_slice(&resp.bytes().await.unwrap()).unwrap();
    let token = body.unwrap().token.expect("native login must return the bearer token");

    let bearer = format!("Bearer {token}");
    let resp = post(addr, "me", "null", &[("authorization", &bearer)]).await;
    assert_eq!(resp.status(), 200);

    let resp = post(addr, "logout", "null", &[("authorization", &bearer)]).await;
    assert_eq!(resp.status(), 200);

    let resp = post(addr, "me", "null", &[("authorization", &bearer)]).await;
    assert_eq!(resp.status(), 401, "a revoked bearer must not authenticate");
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_credentials_are_a_domain_error_not_a_session() {
    let addr = boot().await;
    let resp = post(addr, "login", r#"["demo","wrong"]"#, &[]).await;
    assert_eq!(resp.status(), 200, "domain failure rides the Result, not the status");
    assert!(resp.headers().get("set-cookie").is_none(), "no session on failed login");
    let body: Result<LoginOk, ServerError> =
        serde_json::from_slice(&resp.bytes().await.unwrap()).unwrap();
    assert!(matches!(body, Err(ServerError::Failed(_))));
}

// ---------------------------------------------------------------------------
// Observability: the chain hook reported the invocations above.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn observer_saw_successes_and_rejections() {
    let addr = boot().await;
    // Drive one success and one rejection deterministically.
    let resp = post(addr, "me", "null", &[]).await;
    assert_eq!(resp.status(), 401);
    let login = post(addr, "login", r#"["demo","password"]"#, &[]).await;
    let cookie = login.headers().get("set-cookie").unwrap().to_str().unwrap()
        .split(';').next().unwrap().to_string();
    let resp = post(addr, "me", "null", &[("cookie", &cookie)]).await;
    assert_eq!(resp.status(), 200);

    let seen = records().lock().unwrap();
    assert!(
        seen.iter().any(|s| s.path == "me" && s.outcome == Outcome::Error { status: 401 }),
        "observer must record the 401 rejection; saw {seen:?}"
    );
    assert!(
        seen.iter()
            .any(|s| s.path == "me" && matches!(s.outcome, Outcome::Ok { reply_bytes } if reply_bytes > 0)),
        "observer must record the successful call with its reply size; saw {seen:?}"
    );
}
