/// HTTP request method.
///
/// Kept as a closed enum (rather than `&'static str`) so the per-platform
/// transports can lower to whatever native representation they need
/// (`reqwest::Method`, `gloo_net::http::Method`, an `NSString`, a
/// Java string) without each one re-parsing strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    /// HTTP `GET` — retrieve a resource.
    Get,
    /// HTTP `POST` — submit data, typically creating a resource.
    Post,
    /// HTTP `PUT` — replace a resource at the target URL.
    Put,
    /// HTTP `PATCH` — apply a partial update to a resource.
    Patch,
    /// HTTP `DELETE` — remove the target resource.
    Delete,
    /// HTTP `HEAD` — like `GET` but returns headers only, no body.
    Head,
    /// HTTP `OPTIONS` — query the communication options for the target.
    Options,
}

impl Method {
    /// Uppercase wire spelling used by HTTP/1.1 and HTTP/2.
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
            Method::Head => "HEAD",
            Method::Options => "OPTIONS",
        }
    }
}

impl std::fmt::Display for Method {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
