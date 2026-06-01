//! Middleware: cross-cutting server logic that runs before each handler.
//!
//! A middleware reads and mutates the request [`Context`] — most often an
//! **auth guard** that validates credentials and inserts a principal for
//! a downstream `Auth<Principal>` extractor — and may **short-circuit**
//! by returning an error, in which case the handler never runs and the
//! error's status becomes the HTTP response.
//!
//! Middleware is registered globally via [`install_middleware`] and runs
//! in registration order, per request (and per batch entry), before the
//! handler resolves its extractor params. A guard scopes itself with
//! [`Context::path`](crate::extract::Context::path).
//!
//! ```ignore
//! // Reject anything but `/_srv/public::*` without a valid token.
//! server::install_middleware(server::from_fn(|ctx| Box::pin(async move {
//!     if ctx.path().starts_with("public::") { return Ok(()); }
//!     match ctx.headers().get("authorization").and_then(|v| v.to_str().ok()) {
//!         Some(tok) if let Some(user) = verify(tok) => { ctx.insert(user); Ok(()) }
//!         _ => Err(server::TransportError::Server { status: 401, message: "unauthorized".into() }),
//!     }
//! })));
//! ```
//!
//! This is the pre-handler / context-producing half of middleware (the
//! auth-critical part). Post-handler wrapping (timing/logging around the
//! response) is a planned extension and would layer around the handler
//! call in the dispatcher; the guard model here covers authentication
//! and request-scoped context injection.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock, RwLock};

use crate::error::TransportError;
use crate::extract::Context;

/// The future a [`Middleware`] returns — boxed so the trait is
/// object-safe and the dispatcher can hold a `Vec<Arc<dyn Middleware>>`.
/// Borrows the `&mut Context` it is handed for its lifetime.
pub type MiddlewareFuture<'a> = Pin<Box<dyn Future<Output = Result<(), TransportError>> + Send + 'a>>;

/// Cross-cutting logic run before a handler. See the module docs.
pub trait Middleware: Send + Sync + 'static {
    /// Inspect and mutate the request context. Returning
    /// `Err(TransportError::Server { status, .. })` short-circuits the
    /// request — the handler does not run and `status` becomes the HTTP
    /// response (e.g. 401 for a failed auth guard).
    fn handle<'a>(&'a self, ctx: &'a mut Context) -> MiddlewareFuture<'a>;
}

/// Adapt a closure into a [`Middleware`]. The closure takes `&mut
/// Context` and returns a boxed future (use `Box::pin(async move { … })`).
pub fn from_fn<F>(f: F) -> FnMiddleware<F>
where
    F: for<'a> Fn(&'a mut Context) -> MiddlewareFuture<'a> + Send + Sync + 'static,
{
    FnMiddleware(f)
}

/// A [`Middleware`] backed by a closure (see [`from_fn`]).
pub struct FnMiddleware<F>(F);

impl<F> Middleware for FnMiddleware<F>
where
    F: for<'a> Fn(&'a mut Context) -> MiddlewareFuture<'a> + Send + Sync + 'static,
{
    fn handle<'a>(&'a self, ctx: &'a mut Context) -> MiddlewareFuture<'a> {
        (self.0)(ctx)
    }
}

fn registry() -> &'static RwLock<Vec<Arc<dyn Middleware>>> {
    static REGISTRY: OnceLock<RwLock<Vec<Arc<dyn Middleware>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(Vec::new()))
}

/// Register a middleware. Runs in registration order, before every
/// handler. Call at startup, alongside [`install_state`](crate::install_state).
pub fn install_middleware(mw: impl Middleware) {
    registry().write().unwrap().push(Arc::new(mw));
}

/// Snapshot the installed middleware (cheap: clones a `Vec` of `Arc`s).
fn middlewares() -> Vec<Arc<dyn Middleware>> {
    registry().read().unwrap().clone()
}

/// Run the middleware chain against `ctx`. Returns the first
/// short-circuit error, or `Ok(())` if all pass.
pub(crate) async fn run_middlewares(ctx: &mut Context) -> Result<(), TransportError> {
    for mw in middlewares() {
        mw.handle(ctx).await?;
    }
    Ok(())
}

/// Clear all registered middleware. Test-only — lets a test start from a
/// clean chain regardless of what earlier tests installed.
#[cfg(test)]
pub(crate) fn clear_middlewares() {
    registry().write().unwrap().clear();
}
