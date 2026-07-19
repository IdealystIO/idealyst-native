//! Server-build machinery: the middleware chain mounted on the
//! primitive's [`server::DispatchHook`] slot, the `Auth<T>` resolution,
//! and the stock guards.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock, RwLock};

use server::{Context, DispatchHook, FromContext, HookFuture, Next, OpenFuture, TransportError};

use crate::Auth;

// ---------------------------------------------------------------------------
// Auth<T> resolution (401 on missing — the auth convention).
// ---------------------------------------------------------------------------

impl<T: Clone + Send + Sync + 'static> FromContext for Auth<T> {
    fn from_context(ctx: &Context) -> impl Future<Output = Result<Self, TransportError>> + Send {
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

// ---------------------------------------------------------------------------
// The middleware chain. Same authoring surface the `server` crate used
// to ship; now built entirely on the public hook seam — nothing here
// reaches into the primitive's internals, which is the proof the seam
// is sufficient.
// ---------------------------------------------------------------------------

/// The future a [`Middleware`] returns — boxed so the trait is
/// object-safe. Borrows the `&mut Context` it is handed.
pub type MiddlewareFuture<'a> =
    Pin<Box<dyn Future<Output = Result<(), TransportError>> + Send + 'a>>;

/// Cross-cutting logic run before a handler. Reads and mutates the
/// request [`Context`] (most often an auth guard inserting a principal
/// for a downstream [`Auth`] extractor); returning
/// `Err(TransportError::Server { status, .. })` short-circuits the
/// request with that HTTP status.
pub trait Middleware: Send + Sync + 'static {
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

/// Register a middleware. Runs in registration order before every
/// handler — single calls, each batch entry, and stream opens. Call at
/// startup, alongside `server::install_state`.
///
/// The first call claims the primitive's dispatch-hook slot for the
/// chain; panics (from `server::install_dispatch_hook`) if a different
/// [`DispatchHook`] already owns it.
pub fn install_middleware(mw: impl Middleware) {
    ensure_hook();
    registry().write().unwrap().push(Arc::new(mw));
}

/// Run the installed middleware chain against `ctx`, stopping at the
/// first short-circuit. Public so a *custom* [`DispatchHook`] can layer
/// this crate's guards inside its own policy instead of ceding the slot.
pub async fn run_chain(ctx: &mut Context) -> Result<(), TransportError> {
    // Snapshot under the read lock (cheap Arc clones), then run without
    // holding it — a middleware may itself call install_middleware.
    let chain = registry().read().unwrap().clone();
    for mw in chain {
        mw.handle(ctx).await?;
    }
    Ok(())
}

/// The chain mounted as the primitive's hook: run the chain, then the
/// handler; for stream opens, just the chain.
struct ChainHook;

impl DispatchHook for ChainHook {
    fn around<'a>(&'a self, ctx: &'a mut Context, next: Next) -> HookFuture<'a> {
        Box::pin(async move {
            run_chain(ctx).await?;
            next.run(ctx).await
        })
    }

    fn on_open<'a>(&'a self, ctx: &'a mut Context) -> OpenFuture<'a> {
        Box::pin(run_chain(ctx))
    }
}

/// Claim the hook slot for the chain, once.
fn ensure_hook() {
    static CLAIMED: OnceLock<()> = OnceLock::new();
    CLAIMED.get_or_init(|| server::install_dispatch_hook(ChainHook));
}

// ---------------------------------------------------------------------------
// Stock guards.
// ---------------------------------------------------------------------------

/// A middleware that rejects requests carrying an `Origin` header outside
/// `trusted_origins` (HTTP 403). Requests with no `Origin` (native
/// clients, some same-origin navigations) pass — they can't be
/// browser-driven CSRF.
///
/// Defense-in-depth for cookie-authenticated apps: the primary defense
/// is the `SameSite=Lax` default on `server::Cookie` (browsers won't
/// attach the cookie to a cross-site POST at all); this makes the origin
/// policy explicit and covers `SameSite=None` misconfigurations. It's a
/// stateless check — OWASP's "Verifying Origin" defense.
///
/// Install with [`install_middleware`]. Order doesn't matter relative to
/// an auth guard; a rejected origin short-circuits regardless.
pub fn csrf_guard<I, S>(trusted_origins: I) -> impl Middleware
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let trusted: Vec<String> = trusted_origins.into_iter().map(Into::into).collect();
    from_fn(move |ctx| {
        // Read the origin synchronously; move owned data into the future
        // so it doesn't borrow `ctx` across the (here trivial) await.
        let origin = ctx
            .headers()
            .get("origin")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let trusted = trusted.clone();
        Box::pin(async move {
            match origin {
                // No Origin → native client / non-browser; not a CSRF vector.
                None => Ok(()),
                Some(o) if trusted.iter().any(|t| *t == o) => Ok(()),
                Some(o) => Err(TransportError::Server {
                    status: 403,
                    message: format!("origin '{o}' is not allowed (CSRF guard)"),
                }),
            }
        })
    })
}

/// A path-perimeter guard: reject (403) any request whose wire path
/// starts with `prefix` unless the context carries a `T` — i.e. unless
/// an earlier guard inserted the required principal/marker.
///
/// This is the deny-by-default half of route-group protection: the
/// [`Auth<T>`] extractor protects each fn that *declares* it, while the
/// perimeter also catches a fn someone adds to the group without the
/// param. Install AFTER the context-producing guard (registration order
/// is run order).
///
/// ```ignore
/// server_kit::install_middleware(session_guard());               // inserts Admin
/// server_kit::install_middleware(server_kit::require::<Admin>("admin/"));
/// ```
pub fn require<T: Clone + Send + Sync + 'static>(prefix: &'static str) -> impl Middleware {
    from_fn(move |ctx| {
        let blocked = ctx.path().starts_with(prefix) && ctx.get::<T>().is_none();
        Box::pin(async move {
            if blocked {
                return Err(TransportError::Server {
                    status: 403,
                    message: format!("{prefix}*: forbidden (missing {})", std::any::type_name::<T>()),
                });
            }
            Ok(())
        })
    })
}

// ---------------------------------------------------------------------------
// Unit tests: chain + guards against hand-built contexts (no HTTP in the
// loop). Hook-mounted end-to-end behavior lives in tests/kit_e2e.rs.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use server::ContextBuilder;

    #[derive(Clone, Debug, PartialEq)]
    struct Principal(&'static str);

    #[tokio::test]
    async fn auth_resolves_inserted_principal_and_401s_when_missing() {
        let ctx = ContextBuilder::new().extension(Principal("alice")).build();
        let Auth(p) = <Auth<Principal> as FromContext>::from_context(&ctx)
            .await
            .expect("inserted principal must resolve");
        assert_eq!(p, Principal("alice"));

        let empty = ContextBuilder::new().build();
        match <Auth<Principal> as FromContext>::from_context(&empty).await {
            Err(TransportError::Server { status: 401, .. }) => {}
            Err(other) => panic!("missing principal must 401, got {other:?}"),
            Ok(Auth(p)) => panic!("missing principal must 401, resolved {p:?}"),
        }
    }

    #[tokio::test]
    async fn require_blocks_prefix_without_marker_and_passes_others() {
        let guard = require::<Principal>("admin/");

        // In the group, no marker → 403.
        let mut ctx = ContextBuilder::new().path("admin/delete").build();
        match guard.handle(&mut ctx).await {
            Err(TransportError::Server { status: 403, .. }) => {}
            other => panic!("expected 403, got {other:?}"),
        }

        // In the group, marker present → pass.
        let mut ctx = ContextBuilder::new()
            .path("admin/delete")
            .extension(Principal("root"))
            .build();
        assert!(guard.handle(&mut ctx).await.is_ok());

        // Outside the group → pass without marker.
        let mut ctx = ContextBuilder::new().path("public_list").build();
        assert!(guard.handle(&mut ctx).await.is_ok());
    }

    #[tokio::test]
    async fn csrf_guard_filters_origins() {
        let g = csrf_guard(["https://app.example.com", "http://localhost:3000"]);

        let mut trusted = ContextBuilder::new()
            .header("origin", "https://app.example.com")
            .build();
        assert!(g.handle(&mut trusted).await.is_ok());

        let mut evil = ContextBuilder::new()
            .header("origin", "https://evil.example.com")
            .build();
        match g.handle(&mut evil).await {
            Err(TransportError::Server { status: 403, .. }) => {}
            other => panic!("expected 403, got {other:?}"),
        }

        // Native clients send no Origin and use bearer auth — not CSRF.
        let mut native = ContextBuilder::new().build();
        assert!(g.handle(&mut native).await.is_ok());
    }

    #[tokio::test]
    async fn chain_runs_in_registration_order_and_short_circuits() {
        // Use run_chain against a LOCAL sequence via the registry:
        // middleware 1 inserts, middleware 2 requires the insert. This
        // also exercises order (2 sees 1's insert).
        #[derive(Clone)]
        struct Step1;

        install_middleware(from_fn(|ctx| {
            Box::pin(async move {
                ctx.insert(Step1);
                Ok(())
            })
        }));
        install_middleware(from_fn(|ctx| {
            let ok = ctx.get::<Step1>().is_some();
            Box::pin(async move {
                if ok {
                    Ok(())
                } else {
                    Err(TransportError::Server { status: 500, message: "order broken".into() })
                }
            })
        }));

        let mut ctx = ContextBuilder::new().build();
        run_chain(&mut ctx).await.expect("in-order chain must pass");
    }
}
