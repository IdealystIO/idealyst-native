//! Server-build machinery: the middleware chain mounted on the
//! primitive's [`server::DispatchHook`] slot, the `Auth<T>` resolution,
//! and the stock guards.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock, RwLock};

use server::{Context, DispatchHook, FromContext, HookFuture, Next, OpenFuture, TransportError};

use crate::{Auth, Authenticated, Role};

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

impl<M: Clone + Send + Sync + 'static> FromContext for Role<M> {
    fn from_context(ctx: &Context) -> impl Future<Output = Result<Self, TransportError>> + Send {
        let found = ctx.get::<M>();
        let authed = ctx.get::<Authenticated>().is_some();
        async move {
            match found {
                Some(m) => Ok(Role(m)),
                // Signed in, but the guard did not grant this capability.
                None if authed => Err(TransportError::Server {
                    status: 403,
                    message: format!(
                        "Role<{}>: authenticated, but this capability was not granted",
                        std::any::type_name::<M>()
                    ),
                }),
                // No session at all.
                None => Err(TransportError::Server {
                    status: 401,
                    message: format!(
                        "Role<{}>: request is not authenticated",
                        std::any::type_name::<M>()
                    ),
                }),
            }
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
/// handler; for stream opens, just the chain. Times the whole
/// invocation and reports it to any installed observers (see
/// [`crate::install_observer`]).
struct ChainHook;

impl DispatchHook for ChainHook {
    fn around<'a>(&'a self, ctx: &'a mut Context, next: Next) -> HookFuture<'a> {
        Box::pin(async move {
            let start = std::time::Instant::now();
            let result = async {
                run_chain(ctx).await?;
                next.run(ctx).await
            }
            .await;
            crate::observe::report(ctx, crate::RequestKind::Call, &result, start.elapsed());
            result
        })
    }

    fn on_open<'a>(&'a self, ctx: &'a mut Context) -> OpenFuture<'a> {
        Box::pin(async move {
            let start = std::time::Instant::now();
            let result = run_chain(ctx).await;
            // Adapt to the reporting shape (opens have no reply bytes).
            let as_reply = match &result {
                Ok(()) => Ok(Vec::new()),
                Err(e) => Err(e.clone()),
            };
            crate::observe::report(ctx, crate::RequestKind::Open, &as_reply, start.elapsed());
            result
        })
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

/// The declarative sibling of [`require`]: reject (403) any endpoint that
/// declared `tag` in its `#[server(tags(...))]` metadata unless the
/// context carries a `T`.
///
/// Where [`require`] scopes by a path-prefix *naming convention*,
/// `require_tag` scopes by the endpoint's own self-description — no path
/// list to maintain, and renaming a route can't silently un-guard it:
///
/// ```ignore
/// #[server(tags(admin))]
/// async fn rotate_keys(admin: Auth<Admin>) -> Result<(), ServerError> { … }
///
/// server_kit::install_middleware(server_kit::require_tag::<Admin>("admin"));
/// ```
///
/// Covers streams too: `#[channel]` / `#[subscription]` / `#[sse]` accept
/// the same `tags(...)` and are gated at open. Install AFTER the
/// context-producing guard (registration order is run order).
pub fn require_tag<T: Clone + Send + Sync + 'static>(tag: &'static str) -> impl Middleware {
    from_fn(move |ctx| {
        let blocked = ctx.has_tag(tag) && ctx.get::<T>().is_none();
        Box::pin(async move {
            if blocked {
                return Err(TransportError::Server {
                    status: 403,
                    message: format!(
                        "tagged '{tag}': forbidden (missing {})",
                        std::any::type_name::<T>()
                    ),
                });
            }
            Ok(())
        })
    })
}

/// The deny-by-default perimeter for `tags(role = "...")` endpoints.
///
/// The kit owns the enforcement semantics; the app registers its role
/// *vocabulary* — which tag values map to which capability types — via
/// [`RoleGuard::role`]. That registration is the extension surface:
///
/// ```ignore
/// server_kit::install_middleware(
///     server_kit::role_guard()
///         .role::<Employer>("employer")
///         .role::<Employee>("employee")
///         .role::<Member>("member"),
/// );
/// ```
///
/// Enforcement, per request (unary, each batch entry, stream opens):
/// - endpoint has no `role` tag → pass (not this guard's business).
/// - `role = "x"` and `x` is registered → require the mapped capability
///   in the context: present → pass; absent with [`Authenticated`] →
///   **403**; absent without → **401**.
/// - `role = "x"` and `x` is NOT registered → **500, fail closed**. A
///   typo'd or forgotten registration must never silently open a route;
///   the error names the tag and the fix.
///
/// Install AFTER the session guard (registration order is run order).
/// Endpoints typically pair the tag with a [`Role`] extractor param —
/// the tag protects the set, the param gives the body typed access.
pub fn role_guard() -> RoleGuard {
    RoleGuard { roles: Vec::new() }
}

/// See [`role_guard`].
pub struct RoleGuard {
    /// tag value → "is the mapped capability present in this context?"
    #[allow(clippy::type_complexity)]
    roles: Vec<(&'static str, Arc<dyn Fn(&Context) -> bool + Send + Sync>)>,
}

impl RoleGuard {
    /// Map `tags(role = "<tag_value>")` to the capability type `M` your
    /// session guard inserts for qualifying sessions.
    pub fn role<M: Clone + Send + Sync + 'static>(mut self, tag_value: &'static str) -> Self {
        self.roles
            .push((tag_value, Arc::new(|ctx: &Context| ctx.get::<M>().is_some())));
        self
    }
}

impl Middleware for RoleGuard {
    fn handle<'a>(&'a self, ctx: &'a mut Context) -> MiddlewareFuture<'a> {
        let outcome = match ctx.tag("role") {
            None => Ok(()),
            Some(value) => match self.roles.iter().find(|(tag, _)| *tag == value) {
                None => Err(TransportError::Server {
                    status: 500,
                    message: format!(
                        "role tag '{value}' is not registered with role_guard(); add \
                         .role::<YourCapability>(\"{value}\") — unknown roles fail closed"
                    ),
                }),
                Some((_, has_capability)) => {
                    if has_capability(ctx) {
                        Ok(())
                    } else if ctx.get::<Authenticated>().is_some() {
                        Err(TransportError::Server {
                            status: 403,
                            message: format!("role '{value}' required"),
                        })
                    } else {
                        Err(TransportError::Server {
                            status: 401,
                            message: "unauthenticated".to_string(),
                        })
                    }
                }
            },
        };
        Box::pin(async move { outcome })
    }
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
    async fn require_tag_blocks_tagged_without_marker_and_ignores_untagged() {
        let guard = require_tag::<Principal>("admin");

        // Tagged, no marker → 403.
        let mut ctx = ContextBuilder::new().tags(&[("admin", "")]).build();
        match guard.handle(&mut ctx).await {
            Err(TransportError::Server { status: 403, .. }) => {}
            other => panic!("expected 403, got {other:?}"),
        }

        // Tagged, marker present → pass.
        let mut ctx = ContextBuilder::new()
            .tags(&[("admin", "")])
            .extension(Principal("root"))
            .build();
        assert!(guard.handle(&mut ctx).await.is_ok());

        // Untagged → pass without marker (the tag defines the set).
        let mut ctx = ContextBuilder::new().build();
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

    #[derive(Clone, Debug, PartialEq)]
    struct Employer(&'static str);

    #[tokio::test]
    async fn role_extractor_distinguishes_401_from_403() {
        // Capability present → resolves.
        let ctx = ContextBuilder::new()
            .extension(Authenticated)
            .extension(Employer("acme"))
            .build();
        let Role(e) = <Role<Employer> as FromContext>::from_context(&ctx)
            .await
            .expect("granted capability must resolve");
        assert_eq!(e, Employer("acme"));

        // Authenticated but capability not granted → 403.
        let ctx = ContextBuilder::new().extension(Authenticated).build();
        match <Role<Employer> as FromContext>::from_context(&ctx).await {
            Err(TransportError::Server { status: 403, .. }) => {}
            Err(other) => panic!("signed-in without capability must 403, got {other:?}"),
            Ok(Role(e)) => panic!("signed-in without capability must 403, resolved {e:?}"),
        }

        // No session at all → 401.
        let ctx = ContextBuilder::new().build();
        match <Role<Employer> as FromContext>::from_context(&ctx).await {
            Err(TransportError::Server { status: 401, .. }) => {}
            Err(other) => panic!("anonymous must 401, got {other:?}"),
            Ok(Role(e)) => panic!("anonymous must 401, resolved {e:?}"),
        }
    }

    #[tokio::test]
    async fn role_guard_enforces_registered_roles_and_fails_closed_on_unknown() {
        let guard = role_guard().role::<Employer>("employer");

        // Untagged endpoint → not this guard's business.
        let mut ctx = ContextBuilder::new().build();
        assert!(guard.handle(&mut ctx).await.is_ok());

        // Tagged, capability granted → pass.
        let mut ctx = ContextBuilder::new()
            .tags(&[("role", "employer")])
            .extension(Authenticated)
            .extension(Employer("acme"))
            .build();
        assert!(guard.handle(&mut ctx).await.is_ok());

        // Tagged, signed in, capability missing → 403.
        let mut ctx = ContextBuilder::new()
            .tags(&[("role", "employer")])
            .extension(Authenticated)
            .build();
        match guard.handle(&mut ctx).await {
            Err(TransportError::Server { status: 403, .. }) => {}
            other => panic!("expected 403, got {other:?}"),
        }

        // Tagged, anonymous → 401.
        let mut ctx = ContextBuilder::new().tags(&[("role", "employer")]).build();
        match guard.handle(&mut ctx).await {
            Err(TransportError::Server { status: 401, .. }) => {}
            other => panic!("expected 401, got {other:?}"),
        }

        // Unregistered role value → 500, fail closed, names the fix.
        let mut ctx = ContextBuilder::new()
            .tags(&[("role", "sudoer")])
            .extension(Authenticated)
            .extension(Employer("acme"))
            .build();
        match guard.handle(&mut ctx).await {
            Err(TransportError::Server { status: 500, message }) => {
                assert!(
                    message.contains("sudoer") && message.contains("role_guard"),
                    "error must name the tag and the fix, got: {message}"
                );
            }
            other => panic!("unknown role must fail closed with 500, got {other:?}"),
        }
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
