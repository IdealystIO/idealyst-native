//! Extractor parameters: the keystone that lets a `#[server]` function
//! declare injected server-side dependencies (app state, request
//! headers, middleware-set context values) as *parameters* instead of
//! fetching them ad-hoc inside the body.
//!
//! ```ignore
//! #[server]
//! async fn create_todo(
//!     input: CreateTodo,      // wire arg — serialized, present in the client stub
//!     db: State<Db>,          // injected server-side, absent from the client stub
//!     headers: Headers,       // injected server-side
//! ) -> Result<Todo, ServerError<E>> { ... }
//! ```
//!
//! The macro classifies each parameter (see `server-macros`): a
//! parameter is an **injected extractor** if it is annotated `#[ctx]`
//! *or* its type is one of the reserved wrapper names (`State`,
//! `Headers`, `Extension`); otherwise it is a **wire arg**. Injected
//! params are resolved on the server via [`FromContext`] and stripped
//! from the client stub's signature.
//!
//! # Build split
//!
//! The wrapper *types* ([`State`], [`Extension`], [`Headers`]) exist on
//! both client and server builds, because they appear in the author's
//! shared function signature. Their resolution machinery ([`Context`],
//! [`FromContext`]) is server-only — the client stub never resolves
//! anything; it just omits these params. Authors should import the
//! wrappers under `#[cfg(feature = "server")]` (the body that uses them
//! is server-only anyway), which also avoids an unused-import warning on
//! client builds.

use std::ops::Deref;

// ---------------------------------------------------------------------------
// Wrapper types — present on BOTH builds (they appear in the author's
// shared `#[server]` fn signature). Only their `FromContext` impls are
// server-only.
// ---------------------------------------------------------------------------

/// App-level state injected from the process-wide registry populated by
/// [`crate::install_state`]. Derefs to the inner `T`.
///
/// Resolution fails (HTTP 500) if no value of type `T` was installed —
/// a server-misconfiguration, surfaced to the client as
/// `ServerError::Server { status: 500, .. }`.
pub struct State<T>(pub T);

impl<T> Deref for State<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

/// A value placed into the per-request [`Context`] by middleware (e.g.
/// an authenticated principal set by an auth guard). Derefs to `T`.
///
/// Resolution fails (HTTP 500) if no value of type `T` is present — the
/// middleware that should have inserted it didn't run.
pub struct Extension<T>(pub T);

impl<T> Deref for Extension<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

/// The incoming request's HTTP headers.
///
/// On client builds this is a placeholder (it is never constructed —
/// the stub omits the param); on server builds it carries the real
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
    use super::{Extension, Headers, State};

    use std::any::{Any, TypeId};
    use std::collections::HashMap;
    use std::future::Future;
    use std::sync::Arc;

    use axum::http::{HeaderMap, HeaderName, HeaderValue};

    use crate::error::TransportError;

    /// Per-request context handed to every [`FromContext`] resolver.
    ///
    /// Holds the request headers plus a typed extension map (an
    /// `anymap` keyed by `TypeId`). Middleware writes into the map via
    /// [`ContextBuilder`]; extractors read from it. Cheap to clone — the
    /// headers and the map are both `Arc`-shared — which the batch
    /// dispatcher relies on to re-enter the same context per entry.
    #[derive(Clone)]
    pub struct Context {
        headers: Arc<HeaderMap>,
        extensions: Arc<HashMap<TypeId, Box<dyn Any + Send + Sync>>>,
    }

    impl Context {
        /// Build a context from request headers with an empty extension
        /// map. The single/batch dispatchers use this; middleware that
        /// needs to seed extensions uses [`ContextBuilder`].
        pub fn new(headers: Arc<HeaderMap>) -> Self {
            Self {
                headers,
                extensions: Arc::new(HashMap::new()),
            }
        }

        /// An empty context (no headers, no extensions). Returned by
        /// `current_context()` when called outside a request scope.
        pub fn empty() -> Self {
            Self::new(Arc::new(HeaderMap::new()))
        }

        /// The request's headers.
        pub fn headers(&self) -> &HeaderMap {
            &self.headers
        }

        /// Shared handle to the headers (used by the legacy
        /// `use_request_headers` accessor).
        pub(crate) fn headers_arc(&self) -> Arc<HeaderMap> {
            self.headers.clone()
        }

        /// Read a cloned `T` out of the extension map, or `None` if no
        /// value of that type was inserted.
        pub fn get<T: Clone + 'static>(&self) -> Option<T> {
            self.extensions
                .get(&TypeId::of::<T>())?
                .downcast_ref::<T>()
                .cloned()
        }
    }

    /// Builder for a [`Context`]. Middleware and tests use it to seed
    /// headers and extension values before dispatch.
    #[derive(Default)]
    pub struct ContextBuilder {
        headers: HeaderMap,
        extensions: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    }

    impl ContextBuilder {
        pub fn new() -> Self {
            Self::default()
        }

        /// Replace the whole header map.
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

        /// Insert an extension value, keyed by its type. A later insert
        /// of the same type replaces the earlier one.
        pub fn extension<T: Send + Sync + 'static>(mut self, value: T) -> Self {
            self.extensions.insert(TypeId::of::<T>(), Box::new(value));
            self
        }

        pub fn build(self) -> Context {
            Context {
                headers: Arc::new(self.headers),
                extensions: Arc::new(self.extensions),
            }
        }
    }

    /// Resolve `Self` from the per-request [`Context`].
    ///
    /// Implemented by every injected extractor type. The error half is a
    /// [`TransportError`] (no domain payload): an extraction failure is
    /// infrastructure — missing state, missing header, failed auth — and
    /// surfaces to the client as `ServerError::Server { status, .. }`,
    /// distinct from the body's own domain `Failed`.
    ///
    /// The returned future is `Send` so the boxed handler future stays
    /// `Send`. Resolvers that touch the borrowed `ctx` should do so
    /// synchronously and move owned data into the async block (see the
    /// built-in impls) to avoid borrowing `ctx` across the await.
    pub trait FromContext: Sized {
        fn from_context(
            ctx: &Context,
        ) -> impl Future<Output = Result<Self, TransportError>> + Send;
    }

    impl<T: Clone + Send + Sync + 'static> FromContext for State<T> {
        fn from_context(
            _ctx: &Context,
        ) -> impl Future<Output = Result<Self, TransportError>> + Send {
            // Read from the global registry synchronously; the future is
            // immediately ready.
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
}
