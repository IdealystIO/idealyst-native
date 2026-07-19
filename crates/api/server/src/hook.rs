//! The dispatch hook — the primitive's single interception seam.
//!
//! The server-functions crate deliberately ships **no middleware system,
//! no guards, no auth opinion**. What it ships instead is one hook slot
//! that wraps every handler invocation, so a policy layer (a middleware
//! chain, a guard SDK, a bespoke API framework) can be built *on top of*
//! the primitive rather than *inside* it. The conventional implementation
//! is the `server-kit` crate; installing a custom [`DispatchHook`] is how
//! an implementor replaces that convention wholesale.
//!
//! # Rationale: one slot, not a list
//!
//! A list with ordering semantics *is* a middleware system — exactly the
//! opinion this crate refuses to hold. Keeping the slot singular pushes
//! composition (chains, ordering, scoping, short-circuit conventions) to
//! the layer above, and turns "two policy layers fought over dispatch"
//! from a silent ordering bug into a loud boot-time panic.
//!
//! # Coverage
//!
//! The installed hook runs at every entry point into author code:
//!
//! - [`DispatchHook::around`] wraps each **unary** invocation — the
//!   single `POST /_srv/<path>` dispatch and *each entry* of a batched
//!   request (so a call cannot dodge policy by riding a batch).
//! - [`DispatchHook::on_open`] runs before each **stream** is accepted —
//!   `#[channel]` / `#[subscription]` WebSocket upgrades and `#[sse]`
//!   requests. There is nothing to "wrap" for a stream (the connection
//!   outlives dispatch), so the open hook is pre-only: `Ok(())` accepts,
//!   `Err` refuses with the error's HTTP status.
//!
//! With no hook installed, dispatch invokes the handler directly — the
//! primitive alone imposes zero per-request policy overhead.
//!
//! ```ignore
//! struct RequireHeader;
//! impl server::DispatchHook for RequireHeader {
//!     fn around<'a>(&'a self, ctx: &'a mut server::Context, next: server::Next)
//!         -> server::HookFuture<'a>
//!     {
//!         Box::pin(async move {
//!             if ctx.headers().get("x-api-key").is_none() {
//!                 return Err(server::TransportError::Server {
//!                     status: 401, message: "missing api key".into(),
//!                 });
//!             }
//!             ctx.insert(Caller::from(ctx.headers())); // visible to extractors
//!             next.run(ctx).await
//!         })
//!     }
//! }
//! server::install_dispatch_hook(RequireHeader);
//! ```

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock, RwLock};

use crate::error::TransportError;
use crate::extract::Context;

/// Future returned by [`DispatchHook::around`] — resolves to the wire
/// reply bytes (or a transport error the dispatcher maps to an HTTP
/// status). Boxed so the trait is object-safe.
pub type HookFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<u8>, TransportError>> + Send + 'a>>;

/// Future returned by [`DispatchHook::on_open`] — `Ok(())` accepts the
/// stream, `Err` refuses it before any upgrade happens.
pub type OpenFuture<'a> = Pin<Box<dyn Future<Output = Result<(), TransportError>> + Send + 'a>>;

/// The registered handler shape (mirrors `ServerFnEntry::handler`).
type HandlerFn =
    fn(Vec<u8>) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, TransportError>> + Send>>;

/// The continuation a hook receives: "the rest of dispatch" for one
/// unary call — decode the args, resolve the extractors, run the body,
/// encode the `Result`.
///
/// Consuming it with [`Next::run`] invokes the handler under the given
/// context; dropping it without running is the short-circuit (the hook
/// then returns its own `Err` — or its own bytes, though synthesizing a
/// reply is exotic).
pub struct Next {
    handler: HandlerFn,
    body: Vec<u8>,
}

impl Next {
    pub(crate) fn new(handler: HandlerFn, body: Vec<u8>) -> Self {
        Self { handler, body }
    }

    /// The raw request body (the JSON-encoded args tuple), for hooks
    /// that want to observe it (logging, size limits). Decoding is the
    /// handler's job — a hook normally never parses this.
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// Run the handler. The context is cloned into the request-scoped
    /// task-local at this moment, so every mutation the hook made
    /// *before* calling `run` (inserted principals, extensions) is what
    /// the handler's extractors and `use_request_*` readers observe.
    pub async fn run(self, ctx: &Context) -> Result<Vec<u8>, TransportError> {
        crate::extractors::CURRENT_CONTEXT
            .scope(ctx.clone(), (self.handler)(self.body))
            .await
    }
}

/// The interception seam. Implement and install (once, at boot) via
/// [`install_dispatch_hook`]. Both methods default to pass-through, so a
/// hook overrides only the surface it cares about.
pub trait DispatchHook: Send + Sync + 'static {
    /// Wrap one unary invocation (single dispatch + each batch entry).
    /// Mutate `ctx` to communicate with extractors; call `next.run(ctx)`
    /// to proceed; return `Err(TransportError::Server { status, .. })`
    /// without running `next` to short-circuit with that status.
    fn around<'a>(&'a self, ctx: &'a mut Context, next: Next) -> HookFuture<'a> {
        Box::pin(async move { next.run(ctx).await })
    }

    /// Gate one stream open (`#[channel]` / `#[subscription]` upgrade,
    /// `#[sse]` request) before it is accepted. Pre-only by design —
    /// see the module docs.
    fn on_open<'a>(&'a self, ctx: &'a mut Context) -> OpenFuture<'a> {
        let _ = ctx;
        Box::pin(async { Ok(()) })
    }
}

/// The single slot. `RwLock<Option<…>>` rather than `OnceLock` so unit
/// tests can reset it; the public surface still enforces install-once.
fn slot() -> &'static RwLock<Option<Arc<dyn DispatchHook>>> {
    static SLOT: OnceLock<RwLock<Option<Arc<dyn DispatchHook>>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

/// Install the process-wide [`DispatchHook`]. Call once at server boot,
/// before serving.
///
/// # Panics
///
/// Panics if a hook is already installed. The primitive holds exactly
/// one slot — compose multiple policies *inside* one hook (that is
/// precisely what a middleware-chain SDK such as `server-kit` is), so a
/// second claimant is a wiring bug worth failing loudly on.
pub fn install_dispatch_hook(hook: impl DispatchHook) {
    // Poison-tolerant: the stored Option is valid at every point, and the
    // double-install panic below must not poison the slot for the whole
    // process (release the guard BEFORE panicking).
    let mut slot = slot().write().unwrap_or_else(|p| p.into_inner());
    if slot.is_some() {
        drop(slot);
        panic!(
            "a DispatchHook is already installed. The server-fn primitive has exactly one \
             hook slot; compose policies inside a single hook (e.g. the server-kit \
             middleware chain) instead of installing several."
        );
    }
    *slot = Some(Arc::new(hook));
}

/// Snapshot the installed hook, if any. Cheap (`Arc` clone under a read
/// lock); called once per dispatch.
pub(crate) fn installed() -> Option<Arc<dyn DispatchHook>> {
    slot().read().unwrap_or_else(|p| p.into_inner()).clone()
}

/// Test-only reset so unit tests can exercise install/around/short-
/// circuit repeatedly in one process.
#[cfg(test)]
pub(crate) fn clear_dispatch_hook() {
    *slot().write().unwrap_or_else(|p| p.into_inner()) = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::ContextBuilder;
    use std::sync::Mutex;

    /// Serializes the tests in this module — they all mutate the one
    /// process-wide slot.
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        static M: OnceLock<Mutex<()>> = OnceLock::new();
        M.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    fn ok_handler(
        body: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, TransportError>> + Send>> {
        Box::pin(async move { Ok(body) }) // echo
    }

    #[tokio::test]
    async fn no_hook_runs_handler_directly() {
        let _g = lock();
        clear_dispatch_hook();
        let ctx = ContextBuilder::new().build();
        let next = Next::new(ok_handler, b"payload".to_vec());
        let out = next.run(&ctx).await.expect("plain run must succeed");
        assert_eq!(out, b"payload");
    }

    #[tokio::test]
    async fn hook_short_circuit_skips_handler() {
        let _g = lock();
        clear_dispatch_hook();

        struct Deny;
        impl DispatchHook for Deny {
            fn around<'a>(&'a self, _ctx: &'a mut Context, _next: Next) -> HookFuture<'a> {
                Box::pin(async {
                    Err(TransportError::Server { status: 403, message: "denied".into() })
                })
            }
        }
        install_dispatch_hook(Deny);

        let hook = installed().expect("just installed");
        let mut ctx = ContextBuilder::new().build();
        let next = Next::new(ok_handler, b"never".to_vec());
        match hook.around(&mut ctx, next).await {
            Err(TransportError::Server { status: 403, .. }) => {}
            other => panic!("expected short-circuit 403, got {other:?}"),
        }
        clear_dispatch_hook();
    }

    #[tokio::test]
    async fn hook_context_mutation_reaches_handler_scope() {
        let _g = lock();
        clear_dispatch_hook();

        #[derive(Clone, PartialEq, Debug)]
        struct Tag(&'static str);

        struct Tagger;
        impl DispatchHook for Tagger {
            fn around<'a>(&'a self, ctx: &'a mut Context, next: Next) -> HookFuture<'a> {
                Box::pin(async move {
                    ctx.insert(Tag("hooked"));
                    next.run(ctx).await
                })
            }
        }
        install_dispatch_hook(Tagger);

        // A handler that reads the tag back out of the request-scoped
        // context — proving pre-run mutations are what the handler sees.
        fn tag_reader(
            _body: Vec<u8>,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, TransportError>> + Send>> {
            Box::pin(async {
                let ctx = crate::extractors::current_context();
                match ctx.get::<Tag>() {
                    Some(Tag(t)) => Ok(t.as_bytes().to_vec()),
                    None => Err(TransportError::Server {
                        status: 500,
                        message: "tag missing in handler scope".into(),
                    }),
                }
            })
        }

        let hook = installed().expect("just installed");
        let mut ctx = ContextBuilder::new().build();
        let next = Next::new(tag_reader, Vec::new());
        let out = hook.around(&mut ctx, next).await.expect("must reach handler");
        assert_eq!(out, b"hooked");
        clear_dispatch_hook();
    }

    #[tokio::test]
    #[should_panic(expected = "already installed")]
    async fn second_install_panics() {
        let _g = lock();
        clear_dispatch_hook();
        struct A;
        impl DispatchHook for A {}
        struct B;
        impl DispatchHook for B {}
        install_dispatch_hook(A);
        install_dispatch_hook(B); // must panic
    }

    #[tokio::test]
    async fn default_on_open_accepts() {
        let _g = lock();
        struct Passive;
        impl DispatchHook for Passive {}
        let mut ctx = ContextBuilder::new().build();
        assert!(Passive.on_open(&mut ctx).await.is_ok());
    }
}
