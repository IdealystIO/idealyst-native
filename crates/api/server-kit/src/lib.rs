//! The conventional policy layer over the server-functions primitive.
//!
//! The `server` crate holds **no policy**: its one interception surface
//! is the single [`server::DispatchHook`] slot. This crate is the
//! conventional occupant of that slot — the middleware chain, auth
//! extractor, and guard helpers most apps want:
//!
//! - [`install_middleware`] / [`from_fn`] — an ordered chain of
//!   [`Middleware`] run before every handler (single calls, each batch
//!   entry, and channel/subscription/SSE opens).
//! - [`Auth<T>`] — the 401-on-missing extractor an auth-guard middleware
//!   feeds via `ctx.insert(principal)`.
//! - [`csrf_guard`] — origin allow-list, defense-in-depth for
//!   cookie-authenticated web apps.
//! - [`require`] — a path-prefix guard that makes a route group
//!   deny-by-default (`require::<Admin>("admin/")`).
//!
//! The first `install_middleware` claims the primitive's hook slot for
//! the chain. That composes with everything else in this crate, but NOT
//! with a hand-rolled `DispatchHook` — the slot is singular by design,
//! so "my custom hook + this crate's chain" panics at boot rather than
//! silently ordering itself. Pick one policy owner; a custom hook that
//! wants these conveniences can call [`run_chain`] itself.
//!
//! ```ignore
//! // server boot
//! server_kit::install_middleware(server_kit::from_fn(|ctx| {
//!     let session = read_session(ctx.headers());
//!     Box::pin(async move {
//!         if let Some(user) = lookup(session).await {
//!             ctx.insert(user);            // Auth<Principal> now resolves
//!         }
//!         Ok(())
//!     })
//! }));
//! server_kit::install_middleware(server_kit::csrf_guard(["https://app.example.com"]));
//!
//! // shared #[server] fn — 401s automatically when unauthenticated
//! #[server]
//! async fn me(user: server_kit::Auth<Principal>) -> Result<String, ServerError> {
//!     Ok(user.username.clone())
//! }
//! ```

use std::ops::Deref;

// ---------------------------------------------------------------------------
// Auth<T> — present on BOTH builds (it appears in shared #[server]
// signatures; the client names the type but never constructs one). The
// `#[server]` macro classifies it as an extractor by its *name* (final
// path segment), so it works from this crate exactly as it did from
// `server` itself.
// ---------------------------------------------------------------------------

/// An authenticated principal placed into the request context by an
/// auth-guard middleware. Like `server::Extension<T>`, but a missing
/// value is HTTP **401** — the request is unauthenticated — rather than
/// 500. Derefs to `T`.
///
/// `Auth<T>` always answers a missing `T` with 401. When "you're signed
/// in but not *this*" should be a 403 instead, use [`Role`].
pub struct Auth<T>(pub T);

impl<T> Deref for Auth<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

/// The kit's domain-free "a session was established" fact.
///
/// Session guards insert this alongside their own principal/capability
/// types whenever credentials validate — regardless of scheme (cookie,
/// bearer, API key, mTLS). It's what lets [`Role`] and
/// [`role_guard`](server_impl) answer **401** (no session at all) vs
/// **403** (a session, but not the required capability) without the kit
/// ever knowing the app's principal type.
///
/// ```ignore
/// // in your session guard, once credentials validate:
/// ctx.insert(server_kit::Authenticated);
/// ctx.insert(Principal { … });          // your domain facts
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Authenticated;

/// A required capability with status-accurate failures: the request
/// context must carry an `M` (inserted by your session guard).
///
/// Resolution, in order:
/// - `M` present → the body runs with the typed capability.
/// - `M` absent but [`Authenticated`] present → **403** — the caller is
///   signed in, and specifically may not do this.
/// - neither → **401** — the caller is not signed in.
///
/// ```ignore
/// #[server(tags(role = "employer"))]
/// async fn payroll(who: Role<Employer>) -> Result<Payroll, ServerError> {
///     run_payroll(&who.0).await
/// }
/// ```
///
/// Derefs to `M`. Requires the session guard to insert [`Authenticated`]
/// for every valid session — without it, `Role` degrades to `Auth`-like
/// 401s for everyone.
pub struct Role<M>(pub M);

impl<M> Deref for Role<M> {
    type Target = M;
    fn deref(&self) -> &M {
        &self.0
    }
}

#[cfg(feature = "server")]
mod observe;
#[cfg(feature = "server")]
mod rate_limit;
#[cfg(feature = "redis")]
mod redis_limit_store;
#[cfg(feature = "server")]
mod server_impl;
#[cfg(feature = "server")]
mod sessions;
#[cfg(feature = "server")]
pub use observe::{install_observer, stderr_logger, Outcome, RequestKind, RequestRecord};
#[cfg(feature = "server")]
pub use sessions::{SessionTicket, Sessions};
#[cfg(feature = "server")]
pub use rate_limit::{
    rate_limit, Admitted, Limit, LimitStore, MemoryLimitStore, RateLimit, StoreFuture,
    StoreUnavailable,
};
#[cfg(feature = "redis")]
pub use redis_limit_store::RedisLimitStore;
#[cfg(feature = "server")]
pub use server_impl::{
    csrf_guard, from_fn, install_middleware, require, require_tag, role_guard, run_chain,
    FnMiddleware, Middleware, MiddlewareFuture, RoleGuard,
};

/// Declare an app-scoped context resource as a named extractor.
///
/// Expands to a wrapper struct (both builds — it may appear in shared
/// `#[server]` signatures), a `Deref` to the inner type, and (server
/// build) a `FromContext` impl that resolves the value from the
/// process-wide state registry (`server::install_state`), failing with a
/// clear HTTP 500 when it was never installed.
///
/// The point: server fns name their dependencies as *domain* types
/// (`db: Db`) rather than `State<PgPool>`. Because the name isn't one of
/// the macro's reserved extractor spellings, annotate the parameter with
/// `#[ctx]`:
///
/// ```ignore
/// server_kit::context! {
///     /// The app's database pool.
///     pub struct Db(sqlx::PgPool);
/// }
///
/// // boot
/// server::install_state(pool);        // installs the PgPool
///
/// #[server]
/// async fn list_todos(filter: Filter, #[ctx] db: Db)
///     -> Result<Vec<Todo>, ServerError>
/// {
///     db.query("select …").fetch_all().await   // Db derefs to PgPool
/// }
/// ```
///
/// The inner type must be `Clone + Send + Sync + 'static` (install an
/// `Arc<T>` / pool handle — the registry hands out clones). Several
/// declarations may share one invocation.
#[macro_export]
macro_rules! context {
    ($( $(#[$meta:meta])* $vis:vis struct $name:ident($inner:ty); )+) => {
        $(
            $(#[$meta])*
            $vis struct $name(pub $inner);

            impl ::std::ops::Deref for $name {
                type Target = $inner;
                fn deref(&self) -> &$inner {
                    &self.0
                }
            }

            #[cfg(feature = "server")]
            impl ::server::FromContext for $name {
                fn from_context(
                    _ctx: &::server::Context,
                ) -> impl ::std::future::Future<
                    Output = ::std::result::Result<Self, ::server::TransportError>,
                > + Send {
                    let found = ::server::use_state::<$inner>();
                    async move {
                        found.map($name).ok_or_else(|| ::server::TransportError::Server {
                            status: 500,
                            message: ::std::format!(
                                concat!(
                                    stringify!($name),
                                    ": no {} installed; call server::install_state(...) at startup"
                                ),
                                ::std::any::type_name::<$inner>()
                            ),
                        })
                    }
                }
            }
        )+
    };
}
