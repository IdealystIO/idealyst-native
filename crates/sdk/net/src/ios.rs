//! iOS / macOS / tvOS transport — placeholder for Phase 3.
//!
//! The full implementation will dispatch through
//! `NSURLSession.sharedSession.dataTaskWithRequest:completionHandler:`
//! with a `block2::Block`, bridging completion back to Rust via a
//! `futures-channel::oneshot`.

use std::time::Duration;

use crate::cancel::CancelToken;
use crate::error::Error;
use crate::headers::Headers;
use crate::method::Method;
use crate::response::Response;

pub(crate) struct Transport;

impl Transport {
    pub(crate) fn new() -> Self {
        Self
    }
}

pub(crate) async fn send(
    _transport: &Transport,
    _method: Method,
    _url: String,
    _headers: Headers,
    _body: Vec<u8>,
    _timeout: Option<Duration>,
    _cancel: Option<CancelToken>,
) -> Result<Response, Error> {
    Err(Error::Other(
        "net::ios transport not yet implemented (Phase 3)".into(),
    ))
}
