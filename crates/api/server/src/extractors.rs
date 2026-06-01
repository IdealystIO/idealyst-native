//! Server-side context surface: app-wide state + per-request data.
//!
//! Two distinct lifetimes:
//!
//! - **App-level state** (DB pool, config, an HTTP outbound client,
//!   etc.) — installed once at startup via [`install_state`],
//!   retrieved from inside any server fn via [`use_state`]. Stays
//!   alive for the process lifetime.
//!
//! - **Per-request data** (the request's HTTP headers, eventually
//!   the authenticated user, the trace id, etc.) — set by the
//!   dispatcher right before invoking a handler, read inside the
//!   handler via [`use_request_headers`] (and friends to come).
//!   Available only while the handler's future is being polled
//!   on a tokio task that was scoped by the dispatcher.
//!
//! Implementation notes:
//!
//! - The state registry is a `TypeId`-keyed map of `Box<dyn Any +
//!   Send + Sync>`. Lookups are `O(1)` and just clone the stored
//!   value, so the API requires `T: Clone`. Common pattern: install
//!   `Arc<MyThing>` so cloning is cheap.
//!
//! - Per-request data lives in a `tokio::task_local`, which means
//!   handlers must be polled on a task that's been entered via
//!   `REQUEST_CONTEXT.scope(...).await` (the dispatcher does this).
//!   Outside a request — utility code, background tasks — the
//!   readers return `None` rather than panicking.
//!
//! Compiled only when the `server` feature is ON. Authors who want
//! to share types between server and client should put those types
//! in a shared crate; this module's surface is server-only because
//! its readers don't make sense on the client.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use axum::http::HeaderMap;

// ---------------------------------------------------------------------------
// App-level state
// ---------------------------------------------------------------------------

type StateMap = HashMap<TypeId, Box<dyn Any + Send + Sync>>;

fn state_map() -> &'static RwLock<StateMap> {
    static STATE: OnceLock<RwLock<StateMap>> = OnceLock::new();
    STATE.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Register `value` as the canonical instance of `T` in the
/// process-wide state map. Later calls to [`use_state::<T>`] return
/// clones of this value.
///
/// Idempotent for the same `T`: a second `install_state::<T>(...)`
/// replaces the prior registration. That makes it safe for tests
/// to reconfigure between cases.
pub fn install_state<T: Send + Sync + 'static>(value: T) {
    state_map()
        .write()
        .unwrap()
        .insert(TypeId::of::<T>(), Box::new(value));
}

/// Read `T` out of the state map. Returns `None` if no value of
/// that exact type was installed.
///
/// Requires `T: Clone` because the registry hands out clones —
/// `Arc<MyThing>` is the typical install shape, with `use_state`
/// callers receiving cheap `Arc` clones.
pub fn use_state<T: Clone + Send + Sync + 'static>() -> Option<T> {
    let map = state_map().read().unwrap();
    map.get(&TypeId::of::<T>())?.downcast_ref::<T>().cloned()
}

// ---------------------------------------------------------------------------
// Per-request context
// ---------------------------------------------------------------------------

use crate::extract::Context;

tokio::task_local! {
    /// The current request's [`Context`], set by the dispatcher around
    /// each handler future. Read by `use_request_*` (legacy convenience
    /// accessors) and by [`current_context`] (which the macro's handler
    /// uses to resolve `FromContext` extractor params). The task-local
    /// lookup is O(1) and confined to the dispatching task — concurrent
    /// requests on different tasks see their own scope.
    pub(crate) static CURRENT_CONTEXT: Context;
}

/// Clone the current request's [`Context`]. Returns
/// [`Context::empty`](crate::extract::Context::empty) when called
/// outside a request scope (utility code, background tasks). The macro's
/// generated handler calls this once, then resolves each injected
/// extractor against the result.
pub(crate) fn current_context() -> Context {
    CURRENT_CONTEXT
        .try_with(|c| c.clone())
        .unwrap_or_else(|_| Context::empty())
}

/// Read the current request's HTTP headers, or `None` if called
/// outside an active handler context (e.g. from app startup or a
/// background task).
pub fn use_request_headers() -> Option<Arc<HeaderMap>> {
    CURRENT_CONTEXT.try_with(|c| c.headers_arc()).ok()
}

/// Read a single header by name from the current request.
/// Convenience over [`use_request_headers`] for the common case.
pub fn use_request_header(name: &str) -> Option<String> {
    let headers = use_request_headers()?;
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}
