use std::time::Duration;

use crate::body::IntoBody;
use crate::cancel::CancelToken;
use crate::client::Client;
use crate::error::Error;
use crate::headers::Headers;
use crate::method::Method;
use crate::response::Response;
use crate::transport;

/// In-flight request configuration. Construct via `Client::get` /
/// `Client::post` / `Client::request`. Terminate with [`Self::send`].
pub struct RequestBuilder {
    client: Client,
    method: Method,
    url: String,
    headers: Headers,
    body: Vec<u8>,
    timeout: Option<Duration>,
    /// Cancellation token attached via [`Self::cancel_on`]. The
    /// transport races the request future against this token's
    /// `cancelled()` future; if the token fires first the request is
    /// dropped and `Error::Cancelled` is returned.
    cancel: Option<CancelToken>,
    /// Pre-send error captured from the builder chain. We defer
    /// surfacing it until `.send().await` so users can use the
    /// fluent style without a `?` on every step.
    error: Option<Error>,
}

impl RequestBuilder {
    pub(crate) fn new(client: Client, method: Method, url: String) -> Self {
        let headers = client.inner.default_headers.clone();
        let timeout = client.inner.default_timeout;
        Self {
            client,
            method,
            url,
            headers,
            body: Vec::new(),
            timeout,
            cancel: None,
            error: None,
        }
    }

    /// Append a header. Allows duplicates — see [`Headers::append`].
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.append(name, value);
        self
    }

    /// Replace any existing header with this name and set a single value.
    pub fn set_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.set(name, value);
        self
    }

    /// URL-encode `params` and append them as a `?query=string` to the URL.
    #[cfg(feature = "form")]
    pub fn query<T: serde::Serialize>(mut self, params: &T) -> Self {
        match serde_urlencoded::to_string(params) {
            Ok(qs) if !qs.is_empty() => {
                let sep = if self.url.contains('?') { '&' } else { '?' };
                self.url.push(sep);
                self.url.push_str(&qs);
            }
            Ok(_) => {}
            Err(e) => self.error = Some(Error::Serialize(e.to_string())),
        }
        self
    }

    /// Per-request timeout, overrides the client default.
    pub fn timeout(mut self, dur: Duration) -> Self {
        self.timeout = Some(dur);
        self
    }

    /// Attach a cancellation token. When the token's paired
    /// [`CancelHandle`](crate::CancelHandle) fires (before or during
    /// the request) `.send().await` resolves with [`Error::Cancelled`]
    /// and the underlying transport drops the in-flight request.
    pub fn cancel_on(mut self, token: CancelToken) -> Self {
        self.cancel = Some(token);
        self
    }

    /// Set the request body from any [`IntoBody`] value. The body's
    /// default `Content-Type` is applied only if the caller hasn't
    /// already set one via [`Self::header`] / [`Self::set_header`].
    pub fn body<B: IntoBody>(mut self, body: B) -> Self {
        match body.into_body() {
            Ok((bytes, default_ct)) => {
                self.body = bytes;
                if let Some(ct) = default_ct {
                    self.headers.set_if_absent("content-type", ct);
                }
            }
            Err(e) => self.error = Some(e),
        }
        self
    }

    /// Convenience: serialise `value` as JSON.
    ///
    /// Takes `&T` to avoid moving the caller's value (the common case
    /// is serialising a struct held in a signal or app state). The
    /// canonical path through [`IntoBody`] is `.body(Json(value))`.
    #[cfg(feature = "json")]
    pub fn json<T: serde::Serialize + ?Sized>(mut self, value: &T) -> Self {
        match serde_json::to_vec(value) {
            Ok(bytes) => {
                self.body = bytes;
                self.headers.set_if_absent("content-type", "application/json");
            }
            Err(e) => self.error = Some(Error::Serialize(e.to_string())),
        }
        self
    }

    /// Convenience: encode `value` as `application/x-www-form-urlencoded`.
    #[cfg(feature = "form")]
    pub fn form<T: serde::Serialize + ?Sized>(mut self, value: &T) -> Self {
        match serde_urlencoded::to_string(value) {
            Ok(s) => {
                self.body = s.into_bytes();
                self.headers
                    .set_if_absent("content-type", "application/x-www-form-urlencoded");
            }
            Err(e) => self.error = Some(Error::Serialize(e.to_string())),
        }
        self
    }

    /// Send the request and await a [`Response`].
    pub async fn send(self) -> Result<Response, Error> {
        if let Some(e) = self.error {
            return Err(e);
        }
        // Short-circuit if the token is already cancelled — saves a
        // wasted connection attempt.
        if let Some(t) = &self.cancel {
            if t.is_cancelled() {
                return Err(Error::Cancelled);
            }
        }
        let url = resolve_url(self.client.inner.base_url.as_deref(), &self.url)?;
        transport::send(
            &self.client.inner.transport,
            self.method,
            url,
            self.headers,
            self.body,
            self.timeout,
            self.cancel,
        )
        .await
    }
}

/// Resolve a request URL against an optional client base URL.
///
/// - Request URL with a scheme (`http://`, `https://`, `data:`, ...) is
///   used as-is.
/// - Otherwise, if a base URL is set, the request URL is appended to
///   the base. Exactly one `/` is enforced at the join.
/// - Otherwise it's an error.
fn resolve_url(base: Option<&str>, request: &str) -> Result<String, Error> {
    if has_scheme(request) {
        return Ok(request.to_string());
    }
    let Some(base) = base else {
        return Err(Error::InvalidUrl(format!(
            "relative url '{request}' but no base_url configured"
        )));
    };
    let base = base.trim_end_matches('/');
    let req = request.trim_start_matches('/');
    Ok(format!("{base}/{req}"))
}

fn has_scheme(url: &str) -> bool {
    // RFC 3986 scheme: ALPHA *( ALPHA / DIGIT / "+" / "-" / "." ) ":"
    let bytes = url.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_alphabetic() {
        return false;
    }
    for (i, b) in bytes.iter().enumerate().skip(1) {
        if *b == b':' {
            return i > 0;
        }
        if !(b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')) {
            return false;
        }
    }
    false
}
