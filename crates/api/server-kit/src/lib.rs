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
pub struct Auth<T>(pub T);

impl<T> Deref for Auth<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

#[cfg(feature = "server")]
mod server_impl;
#[cfg(feature = "server")]
pub use server_impl::{
    csrf_guard, from_fn, install_middleware, require, run_chain, FnMiddleware, Middleware,
    MiddlewareFuture,
};
