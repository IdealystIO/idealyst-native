use crate::body::FromBody;
use crate::error::Error;
use crate::headers::Headers;

/// A completed HTTP response. The body is already buffered in memory
/// (v0; streaming is a follow-up).
#[derive(Debug)]
pub struct Response {
    pub(crate) status: u16,
    pub(crate) headers: Headers,
    pub(crate) body: Vec<u8>,
}

impl Response {
    pub fn status(&self) -> u16 {
        self.status
    }

    /// True iff the status is in `200..300`.
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name)
    }

    pub fn headers(&self) -> &Headers {
        &self.headers
    }

    /// Turn a 4xx/5xx response into [`Error::Status`], otherwise pass
    /// through. Useful to short-circuit error handling with `?` in
    /// happy-path code that only deals with 2xx responses.
    pub fn error_for_status(self) -> Result<Self, Error> {
        if self.is_success() {
            Ok(self)
        } else {
            let body = String::from_utf8(self.body).ok();
            Err(Error::Status {
                code: self.status,
                body,
            })
        }
    }

    /// Consume the response and decode the body via a [`FromBody`] impl.
    pub async fn body<B: FromBody>(self) -> Result<B, Error> {
        let ct = self.headers.get("content-type").map(|s| s.to_string());
        B::from_body(self.body, ct.as_deref())
    }

    /// Consume the response and return its body bytes.
    pub async fn bytes(self) -> Result<Vec<u8>, Error> {
        Ok(self.body)
    }

    /// Consume the response and decode the body as UTF-8 text.
    pub async fn text(self) -> Result<String, Error> {
        String::from_utf8(self.body).map_err(|e| Error::Deserialize(e.to_string()))
    }

    /// Consume the response and deserialise the body as JSON into `T`.
    #[cfg(feature = "json")]
    pub async fn json<T: serde::de::DeserializeOwned>(self) -> Result<T, Error> {
        serde_json::from_slice(&self.body).map_err(|e| Error::Deserialize(e.to_string()))
    }
}
