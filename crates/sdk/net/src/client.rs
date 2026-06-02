use std::sync::Arc;
use std::time::Duration;

use crate::headers::Headers;
use crate::method::Method;
use crate::request::RequestBuilder;
use crate::transport;

/// Async HTTP client. Cheap to clone (internally an `Arc`).
///
/// Construct a default client with [`Client::new`] or configure base URL,
/// default headers, and a default timeout via [`Client::builder`].
#[derive(Clone)]
pub struct Client {
    pub(crate) inner: Arc<ClientInner>,
}

/// Shared, immutable client state. Held inside an `Arc` so cloning a
/// `Client` is one atomic increment.
pub(crate) struct ClientInner {
    pub base_url: Option<String>,
    pub default_headers: Headers,
    pub default_timeout: Option<Duration>,
    pub transport: transport::Transport,
}

impl Client {
    /// Construct a default client (no base URL, no default headers,
    /// no default timeout).
    pub fn new() -> Self {
        Self::builder().build()
    }

    /// Begin configuring a client.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    /// Begin building a `GET` request to `url`.
    pub fn get(&self, url: impl AsRef<str>) -> RequestBuilder {
        self.request(Method::Get, url)
    }

    /// Begin building a `POST` request to `url`.
    pub fn post(&self, url: impl AsRef<str>) -> RequestBuilder {
        self.request(Method::Post, url)
    }

    /// Begin building a `PUT` request to `url`.
    pub fn put(&self, url: impl AsRef<str>) -> RequestBuilder {
        self.request(Method::Put, url)
    }

    /// Begin building a `PATCH` request to `url`.
    pub fn patch(&self, url: impl AsRef<str>) -> RequestBuilder {
        self.request(Method::Patch, url)
    }

    /// Begin building a `DELETE` request to `url`.
    pub fn delete(&self, url: impl AsRef<str>) -> RequestBuilder {
        self.request(Method::Delete, url)
    }

    /// Begin building a `HEAD` request to `url`.
    pub fn head(&self, url: impl AsRef<str>) -> RequestBuilder {
        self.request(Method::Head, url)
    }

    /// Begin building an `OPTIONS` request to `url`.
    pub fn options(&self, url: impl AsRef<str>) -> RequestBuilder {
        self.request(Method::Options, url)
    }

    /// Begin building a request with any [`Method`].
    pub fn request(&self, method: Method, url: impl AsRef<str>) -> RequestBuilder {
        RequestBuilder::new(self.clone(), method, url.as_ref().to_string())
    }
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for a [`Client`]. Construct via [`Client::builder`].
#[derive(Default)]
pub struct ClientBuilder {
    base_url: Option<String>,
    default_headers: Headers,
    default_timeout: Option<Duration>,
}

impl ClientBuilder {
    /// Set a base URL. Any relative request URL is resolved against it.
    /// Absolute request URLs (e.g. `https://other.example.com/...`)
    /// always win — `base_url` only applies when the request URL has
    /// no scheme.
    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }

    /// Add a default header applied to every request from the resulting
    /// client. Per-request headers can override these.
    pub fn default_header(
        mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.default_headers.append(name, value);
        self
    }

    /// Default per-request timeout. Per-request `timeout(...)` overrides.
    pub fn timeout(mut self, duration: Duration) -> Self {
        self.default_timeout = Some(duration);
        self
    }

    /// Finish configuration and produce a [`Client`].
    pub fn build(self) -> Client {
        Client {
            inner: Arc::new(ClientInner {
                base_url: self.base_url,
                default_headers: self.default_headers,
                default_timeout: self.default_timeout,
                transport: transport::Transport::new(),
            }),
        }
    }
}
