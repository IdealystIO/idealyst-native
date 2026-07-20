//! Response headers for server-fn handlers and dispatch hooks — the
//! generalization of the cookie jar.
//!
//! A handler (or, more commonly, a hook/middleware) attaches arbitrary
//! headers to the HTTP response of the *current* call. The canonical
//! consumer is a rate limiter putting `Retry-After` on its 429 — which is
//! why, unlike cookies, the jar is drained on **error responses too**: a
//! short-circuiting policy layer must be able to annotate its rejection.
//!
//! Mechanism (mirrors `cookie.rs`): the dispatcher seeds each request's
//! [`Context`](crate::extract::Context) with a [`ResponseHeaderJar`]
//! before the hook runs, and drains it into response headers afterward —
//! on the success path, the error path, per batch entry (headers
//! accumulate onto the single batch response), and on a refused stream
//! open (`#[channel]` / `#[subscription]` / `#[sse]`). Headers on an
//! *accepted* stream's response are not supported yet — the upgrade
//! response is built by the transport.
//!
//! Two access styles:
//! - **Hooks / middleware** hold `&mut Context` directly:
//!   `ctx.get::<ResponseHeaderJar>().unwrap().append("retry-after", "30")`.
//! - **Handler bodies** use the free fn [`append_response_header`], which
//!   finds the jar through the task-local context (mirrors
//!   [`set_cookie`](crate::cookie::set_cookie)).

use std::sync::{Arc, Mutex};

use crate::extractors::CURRENT_CONTEXT;

/// Per-request accumulator of `(name, value)` response headers. Seeded
/// into the request `Context` by the dispatcher; drained by it into the
/// HTTP response. `Clone` shares the inner buffer (the dispatcher's
/// retained handle sees what hooks and the handler pushed).
#[derive(Clone, Default)]
pub struct ResponseHeaderJar(Arc<Mutex<Vec<(String, String)>>>);

impl ResponseHeaderJar {
    /// Append a header. Appending (not replacing) keeps the mechanism
    /// policy-free; last-write-wins vs. multi-value is the HTTP layer's
    /// per-header semantics.
    pub fn append(&self, name: impl Into<String>, value: impl Into<String>) {
        self.0.lock().unwrap().push((name.into(), value.into()));
    }

    /// Take the accumulated headers, leaving the jar empty.
    pub(crate) fn take(&self) -> Vec<(String, String)> {
        std::mem::take(&mut self.0.lock().unwrap())
    }
}

/// Attach a header to the current call's HTTP response. A no-op outside
/// a server-fn handler context. Hooks/middleware should prefer reading
/// the [`ResponseHeaderJar`] off the `Context` they already hold.
pub fn append_response_header(name: impl Into<String>, value: impl Into<String>) {
    let (name, value) = (name.into(), value.into());
    let _ = CURRENT_CONTEXT.try_with(|c| {
        if let Some(jar) = c.get::<ResponseHeaderJar>() {
            jar.append(name.clone(), value.clone());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Clones share the buffer — the property the dispatcher relies on
    /// (it keeps one clone, hooks/handlers push via another).
    #[test]
    fn jar_clone_shares_buffer_and_take_drains() {
        let jar = ResponseHeaderJar::default();
        let handle = jar.clone();
        jar.append("retry-after", "30");
        handle.append("x-note", "hi");
        let drained = handle.take();
        assert_eq!(
            drained,
            vec![
                ("retry-after".to_string(), "30".to_string()),
                ("x-note".to_string(), "hi".to_string())
            ]
        );
        assert!(jar.take().is_empty(), "take leaves the jar empty");
    }
}
