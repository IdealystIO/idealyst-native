//! Pluggable request and response body codecs.
//!
//! The two traits are intentionally symmetric — server-fn (and any
//! other consumer of this crate) ship a single wrapper type that
//! implements both, so the same `Postcard<T>` / `Cbor<T>` /
//! `Protobuf<T>` value works on either side of the call.

use crate::Error;

/// Convert a value into a request body.
///
/// Implementations return the serialized bytes plus an optional default
/// `Content-Type`. The request builder only applies the default when
/// the caller hasn't set `Content-Type` explicitly via `.header(...)`,
/// so the trait can't silently overwrite a user-chosen value.
pub trait IntoBody {
    fn into_body(self) -> Result<(Vec<u8>, Option<&'static str>), Error>;
}

/// Decode a response body.
///
/// Receives the raw bytes and the response's `Content-Type` (if any).
/// Most impls ignore the content-type and trust the caller chose the
/// right `FromBody` — it's available for wrappers that need to
/// dispatch (e.g. an `Either<Json<T>, Cbor<T>>`).
pub trait FromBody: Sized {
    fn from_body(bytes: Vec<u8>, content_type: Option<&str>) -> Result<Self, Error>;
}

// ---------------------------------------------------------------------------
// Built-in impls
// ---------------------------------------------------------------------------

impl IntoBody for () {
    fn into_body(self) -> Result<(Vec<u8>, Option<&'static str>), Error> {
        Ok((Vec::new(), None))
    }
}

impl FromBody for () {
    fn from_body(_bytes: Vec<u8>, _content_type: Option<&str>) -> Result<Self, Error> {
        Ok(())
    }
}

impl IntoBody for Vec<u8> {
    fn into_body(self) -> Result<(Vec<u8>, Option<&'static str>), Error> {
        Ok((self, Some("application/octet-stream")))
    }
}

impl FromBody for Vec<u8> {
    fn from_body(bytes: Vec<u8>, _content_type: Option<&str>) -> Result<Self, Error> {
        Ok(bytes)
    }
}

impl IntoBody for String {
    fn into_body(self) -> Result<(Vec<u8>, Option<&'static str>), Error> {
        Ok((self.into_bytes(), Some("text/plain; charset=utf-8")))
    }
}

impl FromBody for String {
    fn from_body(bytes: Vec<u8>, _content_type: Option<&str>) -> Result<Self, Error> {
        String::from_utf8(bytes).map_err(|e| Error::Deserialize(e.to_string()))
    }
}

impl IntoBody for &'static str {
    fn into_body(self) -> Result<(Vec<u8>, Option<&'static str>), Error> {
        Ok((self.as_bytes().to_vec(), Some("text/plain; charset=utf-8")))
    }
}

// ---------------------------------------------------------------------------
// Json<T> — gated by `json` feature (on by default).
// ---------------------------------------------------------------------------

/// JSON request/response wrapper.
///
/// On the request side, serialises `T` with `serde_json` and sets
/// `Content-Type: application/json`. On the response side, deserialises
/// the body bytes as JSON.
///
/// Most authors will reach for the convenience methods
/// `RequestBuilder::json(&value)` and `Response::json::<T>()` instead
/// of constructing this wrapper directly — but the wrapper is the
/// canonical path through [`IntoBody`] / [`FromBody`], used by
/// pluggable callers like server-fn.
#[cfg(feature = "json")]
pub struct Json<T>(pub T);

#[cfg(feature = "json")]
impl<T: serde::Serialize> IntoBody for Json<T> {
    fn into_body(self) -> Result<(Vec<u8>, Option<&'static str>), Error> {
        let bytes = serde_json::to_vec(&self.0).map_err(|e| Error::Serialize(e.to_string()))?;
        Ok((bytes, Some("application/json")))
    }
}

#[cfg(feature = "json")]
impl<T: serde::de::DeserializeOwned> FromBody for Json<T> {
    fn from_body(bytes: Vec<u8>, _content_type: Option<&str>) -> Result<Self, Error> {
        serde_json::from_slice(&bytes)
            .map(Json)
            .map_err(|e| Error::Deserialize(e.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Form<T> — application/x-www-form-urlencoded.
// ---------------------------------------------------------------------------

/// `application/x-www-form-urlencoded` request/response wrapper.
///
/// Symmetric counterpart to [`Json`] for HTML-form-style payloads.
#[cfg(feature = "form")]
pub struct Form<T>(pub T);

#[cfg(feature = "form")]
impl<T: serde::Serialize> IntoBody for Form<T> {
    fn into_body(self) -> Result<(Vec<u8>, Option<&'static str>), Error> {
        let s = serde_urlencoded::to_string(&self.0)
            .map_err(|e| Error::Serialize(e.to_string()))?;
        Ok((s.into_bytes(), Some("application/x-www-form-urlencoded")))
    }
}

#[cfg(feature = "form")]
impl<T: serde::de::DeserializeOwned> FromBody for Form<T> {
    fn from_body(bytes: Vec<u8>, _content_type: Option<&str>) -> Result<Self, Error> {
        let s = std::str::from_utf8(&bytes).map_err(|e| Error::Deserialize(e.to_string()))?;
        serde_urlencoded::from_str(s)
            .map(Form)
            .map_err(|e| Error::Deserialize(e.to_string()))
    }
}
