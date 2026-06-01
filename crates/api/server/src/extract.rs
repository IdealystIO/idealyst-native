//! Extractor parameters: the keystone that lets a `#[server]` function
//! declare injected server-side dependencies (app state, request
//! headers, cookies, middleware-set values) as *parameters* instead of
//! fetching them ad-hoc inside the body.
//!
//! ```ignore
//! #[server]
//! async fn create_todo(
//!     input: CreateTodo,      // wire arg — serialized, present in the client stub
//!     db: State<Db>,          // injected server-side, absent from the client stub
//!     user: Auth<Principal>,  // injected by an auth guard (middleware)
//! ) -> Result<Todo, ServerError<E>> { ... }
//! ```
//!
//! The macro classifies each parameter (see `server-macros`): a
//! parameter is an **injected extractor** if it is annotated `#[ctx]`
//! *or* its type is one of the reserved wrapper names (`State`,
//! `Headers`, `Extension`, `Auth`, `Cookies`); otherwise it is a **wire
//! arg**. Injected params are resolved on the server via [`FromContext`]
//! and stripped from the client stub's signature.
//!
//! # Build split
//!
//! The wrapper *types* exist on both builds (they appear in the author's
//! shared signature). Their resolution machinery ([`Context`],
//! [`FromContext`]) is server-only. Authors should import the wrappers
//! under `#[cfg(feature = "server")]` (the body that uses them is
//! server-only anyway), which also avoids unused-import warnings on
//! client builds.

use std::ops::Deref;

// ---------------------------------------------------------------------------
// Wrapper types — present on BOTH builds. Only their `FromContext` impls
// are server-only.
// ---------------------------------------------------------------------------

/// App-level state injected from the process-wide registry populated by
/// [`crate::install_state`]. Derefs to the inner `T`. Missing → HTTP 500
/// (server misconfiguration).
pub struct State<T>(pub T);

impl<T> Deref for State<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

/// A value placed into the per-request [`Context`] by middleware. Derefs
/// to `T`. Missing → HTTP 500 (the middleware that should have inserted
/// it didn't run).
pub struct Extension<T>(pub T);

impl<T> Deref for Extension<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

/// An authenticated principal placed into the [`Context`] by an auth
/// guard (middleware). Like [`Extension`], but a missing value is HTTP
/// **401** — the request is unauthenticated — rather than 500. Derefs to
/// `T`.
pub struct Auth<T>(pub T);

impl<T> Deref for Auth<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

/// Parsed request cookies. Always resolves (empty when there is no
/// `Cookie` header). Holds only `String`s, so it is portable across both
/// builds.
pub struct Cookies(pub std::collections::HashMap<String, String>);

impl Cookies {
    /// The value of cookie `name`, if present.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(|s| s.as_str())
    }
}

/// The incoming request's HTTP headers. On client builds this is a
/// placeholder (never constructed); on server builds it carries the real
/// [`HeaderMap`](axum::http::HeaderMap) and derefs to it.
#[cfg(feature = "server")]
pub struct Headers(pub std::sync::Arc<axum::http::HeaderMap>);

#[cfg(feature = "server")]
impl Deref for Headers {
    type Target = axum::http::HeaderMap;
    fn deref(&self) -> &axum::http::HeaderMap {
        &self.0
    }
}

/// Client-build placeholder — see the server-gated definition above.
#[cfg(not(feature = "server"))]
pub struct Headers;

// ---------------------------------------------------------------------------
// Server-only machinery: the typed context map + the resolution trait.
// ---------------------------------------------------------------------------

#[cfg(feature = "server")]
mod server_impl {
    use super::{Auth, Cookies, Extension, Headers, State};

    use std::any::{Any, TypeId};
    use std::collections::HashMap;
    use std::future::Future;
    use std::sync::Arc;

    use axum::http::{HeaderMap, HeaderName, HeaderValue};

    use crate::error::TransportError;

    /// Per-request context handed to every [`FromContext`] resolver and
    /// to middleware.
    ///
    /// Holds the request headers, the matched wire path, and a typed
    /// extension map (an `anymap` keyed by `TypeId`). Middleware mutates
    /// the map via [`Context::insert`] before the handler resolves its
    /// extractors. Cloning clones the (small) extension map plus two
    /// `Arc`s — cheap enough for the batch dispatcher to re-enter the
    /// same context per entry.
    #[derive(Clone)]
    pub struct Context {
        headers: Arc<HeaderMap>,
        path: Arc<str>,
        extensions: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
    }

    impl Context {
        /// Build a context from request headers + the matched path, with
        /// an empty extension map.
        pub fn new(headers: Arc<HeaderMap>, path: impl Into<Arc<str>>) -> Self {
            Self {
                headers,
                path: path.into(),
                extensions: HashMap::new(),
            }
        }

        /// An empty context. Returned by `current_context()` when called
        /// outside a request scope.
        pub fn empty() -> Self {
            Self::new(Arc::new(HeaderMap::new()), "")
        }

        /// The request's headers.
        pub fn headers(&self) -> &HeaderMap {
            &self.headers
        }

        /// The matched wire path (e.g. `"todos::list"`). Lets middleware
        /// scope itself to particular endpoints.
        pub fn path(&self) -> &str {
            &self.path
        }

        pub(crate) fn headers_arc(&self) -> Arc<HeaderMap> {
            self.headers.clone()
        }

        /// Insert a value into the extension map, keyed by its type. The
        /// primary tool for middleware: an auth guard validates the
        /// request and `ctx.insert(principal)`, which a downstream
        /// `Auth<Principal>` / `Extension<Principal>` extractor reads.
        pub fn insert<T: Send + Sync + 'static>(&mut self, value: T) {
            self.extensions.insert(TypeId::of::<T>(), Arc::new(value));
        }

        /// Read a cloned `T` out of the extension map.
        pub fn get<T: Clone + 'static>(&self) -> Option<T> {
            self.extensions
                .get(&TypeId::of::<T>())?
                .downcast_ref::<T>()
                .cloned()
        }
    }

    /// Builder for a [`Context`]. Tests use it to seed headers and
    /// extension values without a running server.
    #[derive(Default)]
    pub struct ContextBuilder {
        headers: HeaderMap,
        path: String,
        extensions: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
    }

    impl ContextBuilder {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn path(mut self, path: impl Into<String>) -> Self {
            self.path = path.into();
            self
        }

        pub fn headers(mut self, headers: HeaderMap) -> Self {
            self.headers = headers;
            self
        }

        /// Insert one header by name/value. Malformed names/values are
        /// silently skipped (convenience for tests / fixtures).
        pub fn header(mut self, name: &str, value: &str) -> Self {
            if let (Ok(n), Ok(v)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(value),
            ) {
                self.headers.insert(n, v);
            }
            self
        }

        pub fn extension<T: Send + Sync + 'static>(mut self, value: T) -> Self {
            self.extensions.insert(TypeId::of::<T>(), Arc::new(value));
            self
        }

        pub fn build(self) -> Context {
            Context {
                headers: Arc::new(self.headers),
                path: self.path.into(),
                extensions: self.extensions,
            }
        }
    }

    /// Resolve `Self` from the per-request [`Context`].
    ///
    /// The error half is a [`TransportError`] (no domain payload): an
    /// extraction failure is infrastructure — missing state, missing
    /// header, unauthenticated — and surfaces to the client as
    /// `ServerError::Server { status, .. }`, distinct from the body's own
    /// domain `Failed`. The returned future is `Send` so the boxed
    /// handler future stays `Send`; resolvers touching the borrowed
    /// `ctx` should do so synchronously and move owned data into the
    /// async block.
    pub trait FromContext: Sized {
        fn from_context(
            ctx: &Context,
        ) -> impl Future<Output = Result<Self, TransportError>> + Send;
    }

    impl<T: Clone + Send + Sync + 'static> FromContext for State<T> {
        fn from_context(
            _ctx: &Context,
        ) -> impl Future<Output = Result<Self, TransportError>> + Send {
            let found = crate::extractors::use_state::<T>();
            async move {
                found.map(State).ok_or_else(|| TransportError::Server {
                    status: 500,
                    message: format!(
                        "State<{}> not installed; call server::install_state(...) at startup",
                        std::any::type_name::<T>()
                    ),
                })
            }
        }
    }

    impl FromContext for Headers {
        fn from_context(
            ctx: &Context,
        ) -> impl Future<Output = Result<Self, TransportError>> + Send {
            let headers = ctx.headers.clone();
            async move { Ok(Headers(headers)) }
        }
    }

    impl<T: Clone + Send + Sync + 'static> FromContext for Extension<T> {
        fn from_context(
            ctx: &Context,
        ) -> impl Future<Output = Result<Self, TransportError>> + Send {
            let found = ctx.get::<T>();
            async move {
                found.map(Extension).ok_or_else(|| TransportError::Server {
                    status: 500,
                    message: format!(
                        "Extension<{}> not present in request context (no middleware inserted it)",
                        std::any::type_name::<T>()
                    ),
                })
            }
        }
    }

    impl<T: Clone + Send + Sync + 'static> FromContext for Auth<T> {
        fn from_context(
            ctx: &Context,
        ) -> impl Future<Output = Result<Self, TransportError>> + Send {
            let found = ctx.get::<T>();
            async move {
                // Missing principal = unauthenticated → 401, not 500.
                found.map(Auth).ok_or_else(|| TransportError::Server {
                    status: 401,
                    message: format!(
                        "Auth<{}>: request is not authenticated (no guard inserted a principal)",
                        std::any::type_name::<T>()
                    ),
                })
            }
        }
    }

    impl FromContext for Cookies {
        fn from_context(
            ctx: &Context,
        ) -> impl Future<Output = Result<Self, TransportError>> + Send {
            // Parse `Cookie: a=1; b=2` synchronously; always succeeds.
            let mut map = HashMap::new();
            if let Some(raw) = ctx.headers.get("cookie").and_then(|v| v.to_str().ok()) {
                for pair in raw.split(';') {
                    let pair = pair.trim();
                    if let Some((k, v)) = pair.split_once('=') {
                        map.insert(k.trim().to_string(), v.trim().to_string());
                    }
                }
            }
            async move { Ok(Cookies(map)) }
        }
    }
}

#[cfg(feature = "server")]
pub use server_impl::{Context, ContextBuilder, FromContext};

// ---------------------------------------------------------------------------
// Unit tests: resolve each built-in extractor against a hand-built
// Context, no HTTP server in the loop.
// ---------------------------------------------------------------------------

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    use crate::error::TransportError;

    #[derive(Clone, Debug, PartialEq)]
    struct Pool(u32);

    #[tokio::test]
    async fn state_resolves_installed_value() {
        crate::extractors::install_state(Pool(7));
        let ctx = ContextBuilder::new().build();
        let State(p) = <State<Pool> as FromContext>::from_context(&ctx)
            .await
            .expect("state should resolve");
        assert_eq!(p, Pool(7));
    }

    #[tokio::test]
    async fn state_missing_is_server_500() {
        #[derive(Clone)]
        struct NeverInstalled;
        let ctx = ContextBuilder::new().build();
        let result = <State<NeverInstalled> as FromContext>::from_context(&ctx).await;
        let Err(err) = result else {
            panic!("missing state must fail");
        };
        assert!(
            matches!(err, TransportError::Server { status: 500, .. }),
            "expected Server 500, got {err:?}"
        );
    }

    #[tokio::test]
    async fn headers_extractor_exposes_request_headers() {
        let ctx = ContextBuilder::new().header("x-test", "yes").build();
        let h = <Headers as FromContext>::from_context(&ctx)
            .await
            .expect("headers always resolve");
        assert_eq!(h.get("x-test").unwrap().to_str().unwrap(), "yes");
    }

    #[tokio::test]
    async fn extension_resolves_inserted_and_rejects_missing() {
        #[derive(Clone, Debug, PartialEq)]
        struct Principal(String);

        let ctx = ContextBuilder::new()
            .extension(Principal("alice".to_string()))
            .build();
        let Extension(p) = <Extension<Principal> as FromContext>::from_context(&ctx)
            .await
            .expect("extension should resolve");
        assert_eq!(p, Principal("alice".to_string()));

        let empty = ContextBuilder::new().build();
        let result = <Extension<Principal> as FromContext>::from_context(&empty).await;
        let Err(err) = result else {
            panic!("missing extension must fail");
        };
        assert!(matches!(err, TransportError::Server { status: 500, .. }));
    }

    #[tokio::test]
    async fn auth_missing_is_401() {
        #[derive(Clone)]
        struct User(u64);
        let ctx = ContextBuilder::new().build();
        let result = <Auth<User> as FromContext>::from_context(&ctx).await;
        let Err(err) = result else {
            panic!("missing auth must fail");
        };
        assert!(
            matches!(err, TransportError::Server { status: 401, .. }),
            "expected 401, got {err:?}"
        );
    }

    #[tokio::test]
    async fn cookies_parse_from_header() {
        let ctx = ContextBuilder::new()
            .header("cookie", "session=abc; theme=dark")
            .build();
        let c = <Cookies as FromContext>::from_context(&ctx)
            .await
            .expect("cookies always resolve");
        assert_eq!(c.get("session"), Some("abc"));
        assert_eq!(c.get("theme"), Some("dark"));
        assert_eq!(c.get("missing"), None);
    }
}
