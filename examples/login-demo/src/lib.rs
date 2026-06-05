//! `login-demo` — full-stack auth with server functions, showing the
//! **web BFF pattern** end to end:
//!
//! 1. `login` validates credentials and calls [`server::set_cookie`] to set
//!    an **httpOnly** session cookie. The session id never enters JS — XSS
//!    can't read it.
//! 2. An **auth-guard middleware** reads that cookie on every request and
//!    injects a `Principal`, which protected fns receive via
//!    `Auth<Principal>`.
//! 3. **CSRF defense**: the cookie is `SameSite=Lax` (browsers won't send it
//!    on cross-site POSTs) *and* a [`server::csrf_guard`] rejects untrusted
//!    origins — belt and suspenders.
//! 4. `logout` clears the cookie and drops the server session.
//!
//! The browser sends the cookie automatically on same-origin server-fn
//! calls, so the client never touches the secret.
//!
//! # Native: bearer token in the OS keystore
//!
//! Native clients have no browser cookie jar, so they authenticate the way
//! the same architecture does everywhere else: `login` returns a bearer
//! token (instead of setting a cookie), the client stores it in the
//! `credentials` Keychain/Keystore, and a credential provider attaches it as
//! `Authorization: Bearer …` on every later call. One `login` serves both —
//! it branches on a client header so web never receives the token in JS and
//! native never relies on a cookie. The same server, the same session store,
//! the same `Auth<Principal>` guard (which accepts the session from *either*
//! the cookie or the bearer).

#[cfg(not(feature = "server"))]
use idea_ui::{install_idea_theme, light_theme, Stack, StackGap, StackPadding, Typography};
#[cfg(not(feature = "server"))]
use runtime_core::{effect, signal, text, ui, Element, IntoElement, Signal};
use serde::{Deserialize, Serialize};
use server::{server, ServerError};

/// Login form payload — shared between the client stub and the server body.
#[derive(Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

/// The authenticated user an auth guard injects into the request context,
/// read back by protected fns via `Auth<Principal>`. Defined on BOTH builds
/// because it appears in the shared `me` signature (the client stub names
/// the type even though it never constructs one — `Auth<T>` is a
/// server-injected extractor, absent from the client call site).
#[derive(Clone)]
pub struct Principal {
    pub username: String,
}

/// What `login` returns. `token` is `Some` only for native (bearer) clients,
/// which store it in the OS keystore; it's `None` for web, whose auth rides
/// the httpOnly cookie — the secret must never reach JS.
#[derive(Clone, Serialize, Deserialize)]
pub struct LoginOk {
    pub username: String,
    pub token: Option<String>,
}

// ===========================================================================
// Server-only: the (demo) user check, the in-memory session store, and the
// `Principal` an auth guard injects. Gated behind `feature = "server"` so the
// wasm client never compiles any of it.
// ===========================================================================

#[cfg(feature = "server")]
pub mod srv {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, OnceLock};

    /// session id → username. In-memory and per-process: fine for a demo.
    /// A real app uses a signed/encrypted cookie or a shared store
    /// (Redis/DB), and a CSPRNG for the id.
    fn sessions() -> &'static Mutex<HashMap<String, String>> {
        static S: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
        S.get_or_init(|| Mutex::new(HashMap::new()))
    }

    /// The demo's only valid credentials.
    pub fn check_password(username: &str, password: &str) -> bool {
        username == "demo" && password == "password"
    }

    /// Create a session for `username`, returning its opaque id.
    pub fn new_session(username: &str) -> String {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        // Demo id — unique per process. NOT unguessable; a real app uses a
        // CSPRNG token (e.g. 32 random bytes, base64) or a signed cookie.
        let id = format!("sess-{n}");
        sessions()
            .lock()
            .unwrap()
            .insert(id.clone(), username.to_string());
        id
    }

    /// The username for a session id, if the session is live.
    pub fn lookup(session_id: &str) -> Option<String> {
        sessions().lock().unwrap().get(session_id).cloned()
    }

    /// Drop a session (logout).
    pub fn end(session_id: &str) {
        sessions().lock().unwrap().remove(session_id);
    }

    /// Pull the `session` cookie value out of a raw `Cookie` header.
    pub fn session_from_cookie_header(cookie_header: &str) -> Option<String> {
        cookie_header.split(';').find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            (k.trim() == "session").then(|| v.trim().to_string())
        })
    }
}

/// Install the auth guard. The server bin calls this at startup. The guard
/// runs on every request: if a live `session` cookie is present it injects
/// the `Principal`; otherwise it does nothing (and protected fns 401 via
/// `Auth<Principal>`).
#[cfg(feature = "server")]
pub fn install_auth_guard() {
    server::install_middleware(server::from_fn(|ctx| {
        // Read the session id synchronously, before mutating ctx. Accept it
        // from the cookie (web) OR an `Authorization: Bearer` header (native).
        let session = {
            let headers = ctx.headers();
            let from_cookie = headers
                .get("cookie")
                .and_then(|v| v.to_str().ok())
                .and_then(srv::session_from_cookie_header);
            from_cookie.or_else(|| {
                headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|a| a.strip_prefix("Bearer ").map(|s| s.to_string()))
            })
        };
        Box::pin(async move {
            if let Some(id) = session {
                if let Some(username) = srv::lookup(&id) {
                    ctx.insert(Principal { username });
                }
            }
            Ok(())
        })
    }));
}

// ===========================================================================
// Server functions — one definition, client stub + server body.
// ===========================================================================

/// Validate credentials and start a session. On success sets the httpOnly
/// session cookie; the returned username is for display only (the secret —
/// the session id — stays in the cookie, never in JS).
///
/// The body compiles only into the server build (the `#[server]` macro
/// replaces it with an HTTP stub on the client), so it can freely name the
/// server-only `srv` module and `server::set_cookie`.
#[server]
pub async fn login(creds: Credentials) -> Result<LoginOk, ServerError> {
    if !srv::check_password(&creds.username, &creds.password) {
        return Err(ServerError::Failed("invalid username or password".into()));
    }
    let session_id = srv::new_session(&creds.username);

    // Native clients announce themselves with `x-idealyst-client: native`
    // (set by their credential provider). They get the token in the body to
    // store in the OS keystore — they have no cookie jar. Web clients get an
    // httpOnly cookie and NO token in the body: the secret must never reach
    // JS. The cookie defaults to HttpOnly + Secure + SameSite=Lax.
    let is_native =
        server::use_request_header("x-idealyst-client").as_deref() == Some("native");
    if is_native {
        Ok(LoginOk {
            username: creds.username,
            token: Some(session_id),
        })
    } else {
        server::set_cookie(server::Cookie::new("session", session_id));
        Ok(LoginOk {
            username: creds.username,
            token: None,
        })
    }
}

/// A protected call: returns the current user, or 401 if unauthenticated.
/// `Auth<Principal>` resolves the principal the guard injected from the
/// session cookie.
#[server]
pub async fn me(user: server::Auth<Principal>) -> Result<String, ServerError> {
    Ok(user.username.clone())
}

/// End the session and clear the cookie.
#[server]
pub async fn logout(cookies: server::Cookies) -> Result<(), ServerError> {
    // Find the session from the cookie (web) or the bearer header (native).
    let id = cookies.0.get("session").cloned().or_else(|| {
        server::use_request_header("authorization")
            .and_then(|a| a.strip_prefix("Bearer ").map(String::from))
    });
    if let Some(id) = id {
        srv::end(&id);
    }
    server::clear_cookie("session"); // harmless when there was no cookie
    Ok(())
}

// ===========================================================================
// Client UI.
// ===========================================================================

// The client UI. Gated off the server build: under `--features server` the
// server fns expose their real signatures (with `Auth<Principal>` / `Cookies`
// extractor params), whereas `app()` calls the 0-arg client stubs — those
// only exist on the client build. The server bin never calls `app()`.
#[cfg(not(feature = "server"))]
pub fn app() -> Element {
    install_idea_theme(light_theme());
    configure_server();

    let username: Signal<String> = signal!("demo".to_string());
    let password: Signal<String> = signal!(String::new());
    let user: Signal<Option<String>> = signal!(None); // logged-in username
    let status: Signal<String> = signal!("Enter demo / password".to_string());
    let protected: Signal<String> = signal!(String::new());

    // On mount, ask the server who we are — if a session cookie survives a
    // reload, this restores the logged-in state without re-entering creds.
    {
        effect!({
            runtime_core::driver::spawn_async(async move {
                if let Ok(name) = me().await {
                    user.set(Some(name.clone()));
                    status.set(format!("Welcome back, {name}"));
                }
            });
        });
    }

    let on_login = move || {
        let creds = Credentials {
            username: username.get(),
            password: password.get(),
        };
        status.set("Logging in…".to_string());
        runtime_core::driver::spawn_async(async move {
            match login(creds).await {
                Ok(ok) => {
                    // Native: persist the bearer token in the OS keystore.
                    // Web: `token` is None and this is a no-op.
                    if let Some(token) = &ok.token {
                        store_session_token(token);
                    }
                    user.set(Some(ok.username.clone()));
                    status.set(format!("Logged in as {}", ok.username));
                }
                Err(e) => {
                    user.set(None);
                    status.set(format!("Login failed: {}", err_msg(e)));
                }
            }
        });
    };

    let on_me = move || {
        runtime_core::driver::spawn_async(async move {
            match me().await {
                Ok(name) => protected.set(format!("Protected call says: you are {name}")),
                Err(e) => protected.set(format!("Protected call rejected: {}", err_msg(e))),
            }
        });
    };

    let on_logout = move || {
        runtime_core::driver::spawn_async(async move {
            let _ = logout().await;
            clear_session_token(); // native: drop the keystore token (no-op on web)
            user.set(None);
            protected.set(String::new());
            status.set("Logged out".to_string());
        });
    };

    let status_line = text(move || status.get()).into_element();
    let user_line = text(move || match user.get() {
        Some(name) => format!("Session: logged in as {name}"),
        None => "Session: not logged in".to_string(),
    })
    .into_element();
    let protected_line = text(move || protected.get()).into_element();

    let body: Vec<Element> = vec![
        ui! { Typography(content = "Login demo".to_string(), kind = idea_ui::typography_kind::H1) },
        ui! {
            Typography(
                content = "Server-fn login sets an httpOnly session cookie (BFF). \
                    Try demo / password. Reload the page — the session persists \
                    because the cookie does, not because anything is stored in JS."
                    .to_string(),
                muted = true,
            )
        },
        ui! { text_input(value = username, on_change = move |s| username.set(s), placeholder = "username") },
        ui! { text_input(value = password, on_change = move |s| password.set(s), placeholder = "password") },
        ui! { button(label = "Log in".to_string(), on_click = on_login) },
        ui! { button(label = "Call protected me()".to_string(), on_click = on_me) },
        ui! { button(label = "Log out".to_string(), on_click = on_logout) },
        user_line,
        status_line,
        protected_line,
    ];

    ui! {
        Stack(gap = StackGap::Md, padding = StackPadding::Lg) { body }
    }
}

/// Flatten a `ServerError` to a display string.
#[cfg(not(feature = "server"))]
fn err_msg(e: ServerError) -> String {
    match e {
        ServerError::Failed(m) => m,
        other => format!("{other:?}"),
    }
}

/// Configure the server-fn client per platform.
///
/// - **web**: point at the page origin so calls are same-origin — which is
///   what makes the browser send the httpOnly session cookie automatically.
/// - **native**: point at the dev server and attach a credential provider
///   that (a) always announces `x-idealyst-client: native` so `login` returns
///   a bearer token, and (b) attaches `Authorization: Bearer <token>` once a
///   token is stored in the keystore.
#[cfg(not(feature = "server"))]
fn configure_server() {
    #[cfg(target_arch = "wasm32")]
    {
        let origin = web_sys::window()
            .and_then(|w| w.location().origin().ok())
            .unwrap_or_else(|| "http://127.0.0.1:3000".to_string());
        server::configure(server::ClientConfig::new(origin));
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        server::configure(
            server::ClientConfig::new(NATIVE_SERVER_URL).with_credentials(
                server::credentials_from_fn(|| {
                    let mut headers =
                        vec![("x-idealyst-client".to_string(), "native".to_string())];
                    if let Some(token) = native_creds().get("session").ok().flatten() {
                        headers.push(("authorization".to_string(), format!("Bearer {token}")));
                    }
                    headers
                }),
            ),
        );
    }
}

/// Dev server address for native builds. `10.0.2.2` is the Android emulator's
/// alias for the host machine's `localhost`; for a **physical device** change
/// this to the dev machine's LAN IP (e.g. `http://192.168.1.20:3000`).
#[cfg(all(not(feature = "server"), not(target_arch = "wasm32")))]
const NATIVE_SERVER_URL: &str = "http://10.0.2.2:3000";

/// The process-wide secure store for the native session token (Keychain on
/// iOS/macOS, AndroidKeyStore-backed on Android). Created once.
#[cfg(all(not(feature = "server"), not(target_arch = "wasm32")))]
fn native_creds() -> std::sync::Arc<dyn credentials::Credentials> {
    use std::sync::{Arc, OnceLock};
    static C: OnceLock<Arc<dyn credentials::Credentials>> = OnceLock::new();
    C.get_or_init(|| credentials::platform_credentials("login_demo"))
        .clone()
}

/// Store the session token in the OS keystore. No-op on web (the httpOnly
/// cookie is the web mechanism — nothing is stored client-side).
#[cfg(not(feature = "server"))]
fn store_session_token(token: &str) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = native_creds().set("session", token);
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = token;
    }
}

/// Drop the stored session token (native). No-op on web.
#[cfg(not(feature = "server"))]
fn clear_session_token() {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = native_creds().remove("session");
    }
}

pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(_backend: &mut dev_server::WireRecordingBackend) {}
