//! CSRF protection for cookie-authenticated server functions.
//!
//! # The layered defense
//!
//! Server functions are JSON **POST**s, and the session cookie defaults to
//! `SameSite=Lax` (or `Strict`) — which already means a browser **won't
//! attach the cookie to a cross-site POST** at all. That is the primary
//! CSRF defense, and it's automatic.
//!
//! [`csrf_guard`] is **defense-in-depth** on top: it rejects any request
//! whose `Origin` header isn't in your trusted set, covering older browsers
//! that don't enforce `SameSite`, misconfigured `SameSite=None` cookies,
//! and making the policy explicit. It's a stateless check (no token, no
//! session lookup) — OWASP's "Verifying Origin" defense.
//!
//! Native clients (reqwest / NSURLSession / `HttpURLConnection`) send no
//! `Origin` and authenticate with a bearer token, not a cookie, so they're
//! not a CSRF vector; the guard lets `Origin`-less requests through.
//!
//! ```ignore
//! // At server startup, alongside your auth guard:
//! server::install_middleware(server::csrf_guard([
//!     "https://app.example.com",
//!     "http://localhost:3000", // dev
//! ]));
//! ```

use crate::error::TransportError;
use crate::middleware::{from_fn, Middleware};

/// A middleware that rejects requests carrying an `Origin` header outside
/// `trusted_origins` (HTTP 403). Requests with no `Origin` (native clients,
/// some same-origin navigations) pass — they can't be browser-driven CSRF.
///
/// Install with [`install_middleware`](crate::install_middleware). Order
/// doesn't matter relative to your auth guard; a rejected origin
/// short-circuits before the handler regardless.
pub fn csrf_guard<I, S>(trusted_origins: I) -> impl Middleware
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let trusted: Vec<String> = trusted_origins.into_iter().map(Into::into).collect();
    from_fn(move |ctx| {
        // Read the origin synchronously; move owned data into the future so
        // it doesn't borrow `ctx` across the (here trivial) await.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::ContextBuilder;

    fn guard() -> impl Middleware {
        csrf_guard(["https://app.example.com", "http://localhost:3000"])
    }

    #[tokio::test]
    async fn allows_trusted_origin() {
        let g = guard();
        let mut ctx = ContextBuilder::new()
            .header("origin", "https://app.example.com")
            .build();
        assert!(g.handle(&mut ctx).await.is_ok());
    }

    #[tokio::test]
    async fn rejects_untrusted_origin_with_403() {
        let g = guard();
        let mut ctx = ContextBuilder::new()
            .header("origin", "https://evil.example.com")
            .build();
        match g.handle(&mut ctx).await {
            Err(TransportError::Server { status: 403, .. }) => {}
            other => panic!("expected 403, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn allows_request_without_origin() {
        // Native clients send no Origin and use bearer auth — not CSRF.
        let g = guard();
        let mut ctx = ContextBuilder::new().build();
        assert!(g.handle(&mut ctx).await.is_ok());
    }
}
