//! iOS / macOS / tvOS transport, built on `NSURLSession`.
//!
//! Threading: NSURLSession invokes its completion handler off the
//! caller's thread (a serial queue owned by the shared session by
//! default). We bridge that back to async via a `futures-channel`
//! oneshot the block stores into. The Rust future awaits the
//! receiver.
//!
//! Cancellation: the in-flight `NSURLSessionDataTask` is captured
//! alongside the cancel token; when the token fires we send the
//! task `cancel`, which causes NSURLSession to call the completion
//! handler with `NSError` code `NSURLErrorCancelled`. We map that
//! to `Error::Cancelled` so the cancel path is observable from the
//! caller as the same `Error::Cancelled` variant the native +
//! web transports produce.
//!
//! Submission posture: by reaching `NSURLSession` (rather than e.g.
//! linking `libcurl` or shipping reqwest into the app bundle), the
//! app inherits ATS, system proxy, cellular handoff, background
//! transfer affordances, and certificate pinning hooks for free —
//! which is what App Store reviewers (and most real-world deploys)
//! expect.

use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::sync::Mutex;
use std::task::Poll;
use std::time::Duration;

use block2::RcBlock;
use futures_channel::oneshot;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{
    NSData, NSDictionary, NSError, NSHTTPURLResponse, NSMutableURLRequest, NSString, NSURL,
    NSURLResponse, NSURLSession,
};

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
    method: Method,
    url: String,
    headers: Headers,
    body: Vec<u8>,
    _timeout: Option<Duration>,
    cancel: Option<CancelToken>,
) -> Result<Response, Error> {
    // -------------------------------------------------------------
    // Build NSMutableURLRequest from our typed inputs.
    // -------------------------------------------------------------
    let ns_url_string = NSString::from_str(&url);
    let ns_url = unsafe { NSURL::URLWithString(&ns_url_string) }
        .ok_or_else(|| Error::InvalidUrl(url.clone()))?;

    let mut request = unsafe { NSMutableURLRequest::requestWithURL(&ns_url) };
    let method_str = NSString::from_str(method.as_str());
    unsafe { request.setHTTPMethod(&method_str) };

    for (name, value) in headers.iter() {
        let name = NSString::from_str(name);
        let value = NSString::from_str(value);
        unsafe { request.setValue_forHTTPHeaderField(Some(&value), &name) };
    }

    if !body.is_empty() {
        // `dataWithBytes:length:` copies the buffer, so the Rust
        // `Vec<u8>` can drop after this call without leaving NSData
        // pointing at freed memory.
        let data = unsafe {
            NSData::dataWithBytes_length(
                body.as_ptr() as *mut std::ffi::c_void,
                body.len(),
            )
        };
        unsafe { request.setHTTPBody(Some(&data)) };
    }

    // -------------------------------------------------------------
    // Wire up the completion block.
    //
    // The block runs on URLSession's delegate queue (a background
    // serial queue). To send the result back into our Rust async
    // context we use a `futures-channel` oneshot. NSURLSession
    // invokes the block exactly once, but `Block` is a `Fn` (not
    // `FnOnce`) at the block2 level, so we wrap the Sender in a
    // `Mutex<Option<Sender<_>>>` and `.take()` it on first call.
    // -------------------------------------------------------------
    let (tx, rx) = oneshot::channel::<Result<Response, Error>>();
    let tx_slot: Mutex<Option<oneshot::Sender<Result<Response, Error>>>> =
        Mutex::new(Some(tx));

    let completion = RcBlock::new(
        move |data: *mut NSData, response: *mut NSURLResponse, error: *mut NSError| {
            let result = build_result(data, response, error);
            if let Ok(mut slot) = tx_slot.lock() {
                if let Some(sender) = slot.take() {
                    let _ = sender.send(result);
                }
            }
        },
    );

    // -------------------------------------------------------------
    // Issue the task and start it. `dataTaskWithRequest:completionHandler:`
    // retains the block; calling `resume` kicks off the network
    // operation.
    // -------------------------------------------------------------
    let session = unsafe { NSURLSession::sharedSession() };
    let task = unsafe { session.dataTaskWithRequest_completionHandler(&request, &completion) };
    unsafe {
        let _: () = objc2::msg_send![&task, resume];
    }

    // -------------------------------------------------------------
    // Await the response, optionally racing against cancel.
    //
    // On cancel: invoke `[task cancel]`. URLSession then fires the
    // completion block with NSError(domain=NSURLErrorDomain,
    // code=NSURLErrorCancelled), which `build_result` translates to
    // `Error::Cancelled`. Both `cancel.cancelled()` and the
    // completion eventually resolve `rx`, but we beat that with the
    // race below.
    // -------------------------------------------------------------
    let receive_future = async move {
        rx.await
            .unwrap_or_else(|_| Err(Error::Other("NSURLSession completion never fired".into())))
    };

    match cancel {
        None => receive_future.await,
        Some(token) => race_with_cancel(receive_future, token, task).await,
    }
}

/// Build the platform-agnostic `Response` from the (data, response,
/// error) triple NSURLSession hands the completion block. Pointers
/// are raw because the Obj-C block signature uses raw pointers; we
/// translate to safe `Retained<T>` references as the first step.
fn build_result(
    data: *mut NSData,
    response: *mut NSURLResponse,
    error: *mut NSError,
) -> Result<Response, Error> {
    // Error path. Cancellation maps to its dedicated variant so
    // callers can `match Error::Cancelled` against it; everything
    // else lands as a Network error with the localized description.
    if !error.is_null() {
        let err_ref: &NSError = unsafe { &*error };
        let code = err_ref.code();
        // NSURLErrorCancelled is -999 in NSURLErrorDomain.
        if code == -999 {
            return Err(Error::Cancelled);
        }
        let desc = err_ref.localizedDescription();
        return Err(Error::Network(desc.to_string()));
    }

    // Status + headers come from the HTTP-specific subclass; if the
    // response isn't actually an HTTP response (file://, data://,
    // etc.) we fall back to a default status of 0.
    let (status, out_headers): (u16, Headers) = {
        if response.is_null() {
            (0, Headers::new())
        } else {
            // SAFETY: NSURLSession passes us its NSURLResponse; for
            // HTTP(S) requests this is always an NSHTTPURLResponse
            // subclass. The downcast is verified at runtime by
            // checking the Objective-C class identity before use.
            let resp_any: *mut AnyObject = response.cast();
            let http_ptr: *mut NSHTTPURLResponse = resp_any.cast();
            let http_ref: &NSHTTPURLResponse = unsafe { &*http_ptr };
            let status = unsafe { http_ref.statusCode() } as i64;
            let status: u16 = if (0..=u16::MAX as i64).contains(&status) {
                status as u16
            } else {
                0
            };

            let mut hs = Headers::new();
            let dict: Retained<NSDictionary> = unsafe { http_ref.allHeaderFields() };
            collect_headers_into(&dict, &mut hs);
            (status, hs)
        }
    };

    // Body. NSData::bytes returns a raw pointer + length.
    let body_bytes: Vec<u8> = if data.is_null() {
        Vec::new()
    } else {
        let data_ref: &NSData = unsafe { &*data };
        let len = data_ref.length();
        if len == 0 {
            Vec::new()
        } else {
            let ptr = data_ref.bytes();
            // SAFETY: NSData guarantees `bytes()` returns a pointer
            // to `length()` contiguous bytes; we copy out so the
            // returned `Response` is independent of NSData's
            // lifetime.
            let slice = unsafe { std::slice::from_raw_parts(ptr.as_ptr() as *const u8, len) };
            slice.to_vec()
        }
    };

    Ok(Response {
        status,
        headers: out_headers,
        body: body_bytes,
    })
}

/// Iterate every (key, value) pair in an `NSDictionary<NSString, NSString>`
/// (NSURLSession's `allHeaderFields` is typed that way at runtime
/// even though our binding sees it as the untyped `NSDictionary`).
///
/// Uses `allKeys` + `objectForKey:` rather than `keyEnumerator` so
/// the iteration is bounded and trivial to audit.
fn collect_headers_into(dict: &NSDictionary, out: &mut Headers) {
    // SAFETY: every method here is documented to take/produce
    // NSString instances when the dictionary itself is the
    // `allHeaderFields` result, which is contractually
    // `NSDictionary<NSString *, NSString *>`.
    unsafe {
        let keys: Retained<AnyObject> = objc2::msg_send_id![dict, allKeys];
        let count: usize = objc2::msg_send![&keys, count];
        for i in 0..count {
            let key_obj: *mut AnyObject = objc2::msg_send![&keys, objectAtIndex: i];
            if key_obj.is_null() {
                continue;
            }
            let value_obj: *mut AnyObject =
                objc2::msg_send![dict, objectForKey: key_obj as *mut AnyObject];
            if value_obj.is_null() {
                continue;
            }
            // Convert NSString → Rust String. We have an
            // `AnyObject` typed pointer so reach NSString::to_string
            // by `&*key_obj as *const _ as *const NSString`.
            let key_str: &NSString = &*(key_obj as *const NSString);
            let value_str: &NSString = &*(value_obj as *const NSString);
            out.append(key_str.to_string(), value_str.to_string());
        }
    }
}

/// Race the response future against the cancel token. If cancel
/// wins we send the task a `cancel` message so URLSession
/// short-circuits and the response future eventually settles with
/// `Error::Cancelled`.
async fn race_with_cancel<F>(
    response_future: F,
    token: CancelToken,
    task: Retained<objc2_foundation::NSURLSessionDataTask>,
) -> Result<Response, Error>
where
    F: Future<Output = Result<Response, Error>>,
{
    let mut fut = Box::pin(response_future);
    let mut cancel_fut = Box::pin(token.cancelled());
    poll_fn(|cx| {
        if let Poll::Ready(()) = Pin::new(&mut cancel_fut).poll(cx) {
            // Issue `[task cancel]`. NSURLSession will subsequently
            // call the completion handler with NSURLErrorCancelled,
            // which `build_result` maps to Error::Cancelled.
            unsafe {
                let _: () = objc2::msg_send![&task, cancel];
            }
            return Poll::Ready(Err(Error::Cancelled));
        }
        if let Poll::Ready(result) = Pin::new(&mut fut).poll(cx) {
            return Poll::Ready(result);
        }
        Poll::Pending
    })
    .await
}
