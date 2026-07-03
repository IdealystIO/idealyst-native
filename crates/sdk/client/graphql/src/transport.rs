//! The transport seam — the single point of decoupling from how a GraphQL
//! request actually reaches the server.
//!
//! [`Transport`] is the only contract the [`crate::GraphqlClient`] depends
//! on. Two ways to satisfy it:
//!
//! 1. [`HttpTransport`] — a plain HTTP POST to any GraphQL endpoint, built
//!    on the cross-platform `net` SDK. Use this when you are *not* on the
//!    server-functions stack.
//! 2. The [`graphql_transport!`](crate::graphql_transport) macro — bridges
//!    an app-authored `#[server]` fn into a `Transport`, so the request
//!    rides the server-functions HTTP path and reuses its auth / CSRF /
//!    credentials / per-platform client config. The crate stays free of any
//!    dependency on the `server` SDK.

use crate::{GraphqlError, GraphqlRequest, GraphqlResponse};
use std::future::Future;
use std::pin::Pin;

/// A future returned by a [`Transport`] method. Boxed so the trait stays
/// object-safe (`Rc<dyn Transport>`). `!Send` — the client runs on the
/// single-threaded UI loop on every target (matching `net` + the reactive
/// runtime).
pub type TransportFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, GraphqlError>> + 'a>>;

/// Carries a [`GraphqlRequest`] to a GraphQL server and returns its
/// [`GraphqlResponse`]. The one abstraction the client is written against.
pub trait Transport: 'static {
    /// Execute one GraphQL operation.
    fn execute(&self, request: GraphqlRequest) -> TransportFuture<'_, GraphqlResponse>;
}

/// A [`Transport`] that POSTs the canonical GraphQL JSON body to a fixed
/// URL over the `net` SDK. The endpoint-agnostic client path: point it at
/// any spec-compliant GraphQL server.
pub struct HttpTransport {
    url: String,
    headers: Vec<(String, String)>,
}

impl HttpTransport {
    /// A transport targeting `url` (the GraphQL endpoint).
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            headers: Vec::new(),
        }
    }

    /// Attach a static header sent with every request (e.g. an API key).
    /// For per-request auth that refreshes, prefer the server-functions
    /// transport, which carries credentials through its own provider.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

impl Transport for HttpTransport {
    fn execute(&self, request: GraphqlRequest) -> TransportFuture<'_, GraphqlResponse> {
        let url = self.url.clone();
        let headers = self.headers.clone();
        Box::pin(async move {
            let client = net::Client::new();
            let mut rb = client.post(&url).json(&request);
            for (name, value) in &headers {
                rb = rb.header(name, value);
            }
            let response = rb
                .send()
                .await
                .map_err(|e| GraphqlError::Transport(e.to_string()))?
                // A GraphQL endpoint returns 200 with an `errors` array for
                // operation failures; a non-2xx is a genuine transport
                // fault (404 wrong path, 500, 401 unauthenticated, …).
                .error_for_status()
                .map_err(|e| GraphqlError::Transport(e.to_string()))?;
            response
                .json::<GraphqlResponse>()
                .await
                .map_err(|e| GraphqlError::Transport(e.to_string()))
        })
    }
}
