//! Sessions: the login-demo recipe productized — cookie (web BFF) +
//! bearer (native) session auth over any [`cache::Cache`].
//!
//! **No database opinion.** A session store is a KV-with-TTL, which is
//! exactly the `Cache` contract — so sessions ride whatever the app
//! already provides (`MemoryCache` for dev, `RedisCache` over the
//! app-installed client, or the app's own `Cache` impl over its
//! database). The app supplies only its principal type (serde) and the
//! credential check in its login fn; everything mechanical lives here:
//!
//! ```ignore
//! // boot — one Sessions handle over the provided cache:
//! let sessions = server_kit::Sessions::new(cache_handle);
//! server::install_state(sessions.clone());
//! server_kit::install_middleware(sessions.guard::<Principal>());
//!
//! // login server fn — the app's half is just the credential check:
//! #[server]
//! async fn login(creds: Credentials, sessions: State<Sessions>)
//!     -> Result<LoginOk, ServerError>
//! {
//!     let user = verify(&creds).await.ok_or(ServerError::failed("bad credentials"))?;
//!     let ticket = sessions.start(&Principal { username: user.name }).await
//!         .map_err(|e| ServerError::failed(e.to_string()))?;
//!     // web: an httpOnly cookie was set, ticket.token is None.
//!     // native (x-idealyst-client: native): ticket.token carries the
//!     // bearer for the OS keystore.
//!     Ok(LoginOk { token: ticket.token })
//! }
//!
//! #[server]
//! async fn me(user: Auth<Principal>) -> Result<String, ServerError> { … }
//! ```
//!
//! The guard accepts the session from the cookie **or** an
//! `Authorization: Bearer` header (one server serves web + native), and
//! inserts [`Authenticated`](crate::Authenticated) + the decoded
//! principal — so `Auth<P>` / `Role<M>` compose unchanged. Web security
//! posture matches login-demo: httpOnly/Secure/SameSite=Lax cookie, the
//! secret never enters JS; pair with [`csrf_guard`](crate::csrf_guard).

use std::sync::Arc;
use std::time::Duration;

use cache::{Cache, CacheError};
use serde::de::DeserializeOwned;
use serde::Serialize;
use server::Context;

use crate::server_impl::{from_fn, Middleware};
use crate::Authenticated;

/// What [`Sessions::start`] hands back. `token` is `Some` only for
/// native callers (announced via `x-idealyst-client: native`), which
/// store it in the OS keystore; web callers get an httpOnly cookie and
/// `None` — the secret must never reach JS.
#[derive(Debug)]
pub struct SessionTicket {
    pub token: Option<String>,
}

/// The session system. Cheap to clone (shares the cache handle);
/// install one via `server::install_state` so login/logout fns can take
/// it as `State<Sessions>`.
#[derive(Clone)]
pub struct Sessions {
    cache: Arc<dyn Cache>,
    cookie_name: String,
    namespace: String,
    ttl: Duration,
    /// When set, each authenticated request re-arms the TTL.
    sliding: bool,
}

impl Sessions {
    pub fn new(cache: impl Cache) -> Self {
        Self {
            cache: Arc::new(cache),
            cookie_name: "session".to_string(),
            namespace: "sess".to_string(),
            ttl: Duration::from_secs(30 * 24 * 3600), // 30 days
            sliding: false,
        }
    }

    /// Cookie name (default `session`).
    pub fn cookie_name(mut self, name: impl Into<String>) -> Self {
        self.cookie_name = name.into();
        self
    }

    /// Cache-key namespace (default `sess`).
    pub fn namespace(mut self, ns: impl Into<String>) -> Self {
        self.namespace = ns.into();
        self
    }

    /// Session lifetime (default 30 days).
    pub fn ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Sliding expiration: every authenticated request re-arms the TTL
    /// (default: fixed — a session lives `ttl` from login).
    pub fn sliding(mut self) -> Self {
        self.sliding = true;
        self
    }

    fn key(&self, token: &str) -> String {
        format!("{}:{token}", self.namespace)
    }

    /// The session token this request presented, cookie or bearer.
    fn token_from(&self, ctx: &Context) -> Option<String> {
        let headers = ctx.headers();
        let from_cookie = headers
            .get("cookie")
            .and_then(|v| v.to_str().ok())
            .and_then(|raw| cookie_value(raw, &self.cookie_name));
        from_cookie.or_else(|| {
            headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|a| a.strip_prefix("Bearer ").map(|s| s.to_string()))
        })
    }

    /// The context-producing middleware: resolves a presented session to
    /// the principal `P` and inserts `Authenticated` + `P`. NEVER
    /// rejects — an absent/invalid session just inserts nothing, and the
    /// route's own `Auth<P>` / `Role<M>` / guards decide the answer. A
    /// cache outage also inserts nothing (protected routes fail closed)
    /// and warns on stderr.
    pub fn guard<P>(&self) -> impl Middleware
    where
        P: DeserializeOwned + Clone + Send + Sync + 'static,
    {
        let sessions = self.clone();
        from_fn(move |ctx| {
            let token = sessions.token_from(ctx);
            let sessions = sessions.clone();
            Box::pin(async move {
                let Some(token) = token else { return Ok(()) };
                let key = sessions.key(&token);
                match sessions.cache.get(&key).await {
                    Ok(Some(bytes)) => {
                        if let Ok(principal) = serde_json::from_slice::<P>(&bytes) {
                            ctx.insert(Authenticated);
                            ctx.insert(principal);
                            if sessions.sliding {
                                // Best-effort re-arm; a failure here must
                                // not fail the request.
                                let _ = sessions.cache.set(&key, bytes, Some(sessions.ttl)).await;
                            }
                        }
                    }
                    Ok(None) => {} // unknown/expired token → anonymous
                    Err(CacheError(why)) => {
                        eprintln!(
                            "[server-kit] session store unavailable ({why}); request proceeds \
                             unauthenticated"
                        );
                    }
                }
                Ok(())
            })
        })
    }

    /// Create a session for `principal` (call from a login fn after
    /// verifying credentials). Web callers get an httpOnly cookie set on
    /// this response; native callers (`x-idealyst-client: native`) get
    /// the token in the ticket for the OS keystore.
    pub async fn start<P: Serialize>(&self, principal: &P) -> Result<SessionTicket, CacheError> {
        let token = random_token()?;
        let payload = serde_json::to_vec(principal)
            .map_err(|e| CacheError(format!("encode principal: {e}")))?;
        self.cache.set(&self.key(&token), payload, Some(self.ttl)).await?;

        let native = server::use_request_header("x-idealyst-client").as_deref() == Some("native");
        if native {
            Ok(SessionTicket { token: Some(token) })
        } else {
            server::set_cookie(
                server::Cookie::new(self.cookie_name.clone(), token)
                    .max_age_secs(self.ttl.as_secs() as i64),
            );
            Ok(SessionTicket { token: None })
        }
    }

    /// End the current request's session (logout): delete the stored
    /// session and clear the cookie. A no-op when no session was
    /// presented.
    pub async fn end(&self) -> Result<(), CacheError> {
        let token = server::use_request_headers().and_then(|headers| {
            let from_cookie = headers
                .get("cookie")
                .and_then(|v| v.to_str().ok())
                .and_then(|raw| cookie_value(raw, &self.cookie_name));
            from_cookie.or_else(|| {
                headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|a| a.strip_prefix("Bearer ").map(|s| s.to_string()))
            })
        });
        if let Some(token) = token {
            self.cache.delete(&self.key(&token)).await?;
        }
        server::set_cookie(server::clear_cookie(self.cookie_name.clone()));
        Ok(())
    }
}

/// Pull `name`'s value out of a raw `Cookie` header.
fn cookie_value(raw: &str, name: &str) -> Option<String> {
    raw.split(';').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k.trim() == name).then(|| v.trim().to_string())
    })
}

/// 32 CSPRNG bytes, hex — an unguessable opaque session id.
fn random_token() -> Result<String, CacheError> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes)
        .map_err(|e| CacheError(format!("session token entropy: {e}")))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cache::MemoryCache;
    use server::ContextBuilder;

    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Principal {
        name: String,
    }

    fn sessions() -> Sessions {
        Sessions::new(MemoryCache::new()).ttl(Duration::from_secs(60))
    }

    /// Seed a session directly through the cache (start() needs a
    /// request scope for its cookie/native branch; the guard does not).
    async fn seed(s: &Sessions, token: &str, p: &Principal) {
        use cache::CacheExt;
        s.cache.set_json(&s.key(token), p, Some(s.ttl)).await.unwrap();
    }

    #[tokio::test]
    async fn guard_resolves_cookie_session_and_inserts_facts() {
        let s = sessions();
        seed(&s, "tok1", &Principal { name: "ada".into() }).await;
        let guard = s.guard::<Principal>();

        let mut ctx = ContextBuilder::new()
            .header("cookie", "theme=dark; session=tok1")
            .build();
        guard.handle(&mut ctx).await.unwrap();
        assert_eq!(ctx.get::<Principal>(), Some(Principal { name: "ada".into() }));
        assert!(ctx.get::<Authenticated>().is_some());
    }

    #[tokio::test]
    async fn guard_resolves_bearer_session() {
        let s = sessions();
        seed(&s, "tok2", &Principal { name: "eve".into() }).await;
        let guard = s.guard::<Principal>();

        let mut ctx = ContextBuilder::new()
            .header("authorization", "Bearer tok2")
            .build();
        guard.handle(&mut ctx).await.unwrap();
        assert_eq!(ctx.get::<Principal>(), Some(Principal { name: "eve".into() }));
    }

    #[tokio::test]
    async fn guard_stays_anonymous_for_missing_or_unknown_tokens() {
        let s = sessions();
        let guard = s.guard::<Principal>();

        // No token at all.
        let mut ctx = ContextBuilder::new().build();
        guard.handle(&mut ctx).await.unwrap();
        assert!(ctx.get::<Principal>().is_none());
        assert!(ctx.get::<Authenticated>().is_none());

        // A token nobody issued.
        let mut ctx = ContextBuilder::new()
            .header("authorization", "Bearer forged")
            .build();
        guard.handle(&mut ctx).await.unwrap();
        assert!(ctx.get::<Principal>().is_none(), "forged token must stay anonymous");
    }

    #[tokio::test]
    async fn expired_session_is_anonymous() {
        let s = Sessions::new(MemoryCache::new()).ttl(Duration::ZERO);
        seed(&s, "tok3", &Principal { name: "old".into() }).await;
        let guard = s.guard::<Principal>();
        let mut ctx = ContextBuilder::new()
            .header("authorization", "Bearer tok3")
            .build();
        guard.handle(&mut ctx).await.unwrap();
        assert!(ctx.get::<Principal>().is_none(), "expired session must not authenticate");
    }

    #[test]
    fn tokens_are_long_and_unique() {
        let a = random_token().unwrap();
        let b = random_token().unwrap();
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
    }
}
