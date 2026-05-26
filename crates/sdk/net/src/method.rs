/// HTTP request method.
///
/// Kept as a closed enum (rather than `&'static str`) so the per-platform
/// transports can lower to whatever native representation they need
/// (`reqwest::Method`, `gloo_net::http::Method`, an `NSString`, a
/// Java string) without each one re-parsing strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
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
