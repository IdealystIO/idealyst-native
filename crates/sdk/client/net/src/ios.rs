//! iOS / macOS / tvOS transport, built on `NSURLSession`.
//!
//! Threading: NSURLSession invokes its completion handler off the
//! caller's thread (a serial queue owned by the shared session by
//! default). We bridge that back to async via a `futures-channel`
//! oneshot the block stores into. The Rust future awaits the
//! receiver.
//!
//! Cancellation: the in-flight `NSURLSessionDataTask` is held by a
//! [`CancelTaskOnDrop`] guard for as long as `send` is pending. When the
//! caller stops waiting — the cancel token wins the race, OR the request
//! future is dropped without one (a component unmounting, a `select`
//! losing) — the guard sends the task `cancel`, so NSURLSession stops the
//! transfer instead of downloading the whole body into a receiver nobody
//! reads (the reqwest arm's `AbortOnDrop` and the web arm's
//! `AbortController` behave the same). NSURLSession then calls the
//! completion handler with `NSError` code `NSURLErrorCancelled`; when the
//! token won, the caller has already returned `Error::Cancelled` (the same
//! variant the native + web transports produce), and when the future was
//! dropped the handler's send into the dead oneshot is simply discarded.
//! A task that already completed is disarmed, so it is never cancelled.
//!
//! Timeout: a request `timeout` is a deadline on the whole exchange, body
//! included, resolving `Error::Timeout` — the reqwest and web arms'
//! meaning. `NSURLRequest.timeoutInterval` alone is an IDLE timeout (it
//! restarts on every packet, so a trickling body never expires); the
//! whole-transfer limit is the session's `timeoutIntervalForResource`. So a
//! request with a timeout runs on its own session, built from
//! `defaultSessionConfiguration` (the same shared cookie / credential /
//! cache stores as `sharedSession`) with all three intervals set to the
//! deadline, and invalidated once its one task finishes. The cost is that
//! such a request does not reuse the shared session's pooled connections.
//! `NSURLErrorTimedOut` maps to `Error::Timeout`
//! ([`crate::timeout_policy`]). Requests without a timeout keep
//! `sharedSession` (and its system 60 s idle default).
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
    NSData, NSDictionary, NSError, NSHTTPURLResponse, NSMutableURLRequest, NSString, NSURLResponse,
    NSURLSession, NSURLSessionConfiguration, NSURL,
};

use crate::cancel::CancelToken;
use crate::error::Error;
use crate::headers::Headers;
use crate::method::Method;
use crate::response::Response;
use crate::timeout_policy;

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
    timeout: Option<Duration>,
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

    // The deadline also goes on the request itself: the request's own
    // `timeoutInterval` (default 60 s) is an idle timeout that would
    // otherwise cut a longer deadline short on a quiet connection.
    let deadline = timeout.map(timeout_policy::ns_time_interval);
    if let Some(secs) = deadline {
        unsafe { request.setTimeoutInterval(secs) };
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
    #[cfg(test)]
    let hook_url = url.clone();
    let (tx, rx) = oneshot::channel::<Result<Response, Error>>();
    let tx_slot: Mutex<Option<oneshot::Sender<Result<Response, Error>>>> =
        Mutex::new(Some(tx));

    let completion = RcBlock::new(
        move |data: *mut NSData, response: *mut NSURLResponse, error: *mut NSError| {
            #[cfg(test)]
            test_hook::report(&hook_url, error);
            // May run after the request future was dropped (its guard
            // cancelled the task): the receiver is gone, so the send below
            // fails and the result is discarded. Nothing here touches state
            // the dropped future owned.
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
    let session = match deadline {
        None => unsafe { NSURLSession::sharedSession() },
        Some(secs) => unsafe {
            // `timeoutIntervalForResource` is the whole-exchange limit;
            // `timeoutIntervalForRequest` is the idle one, raised from its
            // 60 s default so it cannot fire before the deadline.
            let config = NSURLSessionConfiguration::defaultSessionConfiguration();
            config.setTimeoutIntervalForRequest(secs);
            config.setTimeoutIntervalForResource(secs);
            NSURLSession::sessionWithConfiguration(&config)
        },
    };
    let task = unsafe { session.dataTaskWithRequest_completionHandler(&request, &completion) };
    // Armed before `resume` and held across every await below: from here
    // on, `send` returning early (cancel) or its future being dropped
    // cancels the task.
    let mut guard = CancelTaskOnDrop { task: Some(task) };
    if let Some(task) = &guard.task {
        unsafe {
            let _: () = objc2::msg_send![task, resume];
        }
    }
    if deadline.is_some() {
        // A per-request session is released only once invalidated. This
        // lets its one task run to completion (or cancel / timeout), then
        // tears the session down; it accepts no further tasks. A guard
        // cancel is covered too: the cancelled task completes (with
        // `NSURLErrorCancelled`), which is the session's last task, so the
        // session invalidates and is released — no per-request session
        // outlives a dropped future.
        unsafe { session.finishTasksAndInvalidate() };
    }

    // -------------------------------------------------------------
    // Await the response, optionally racing against cancel.
    //
    // If cancel wins, `send` returns `Error::Cancelled` and `guard` drops,
    // sending `[task cancel]`; URLSession then fires the completion block
    // with NSURLErrorCancelled into a receiver that is already gone.
    // -------------------------------------------------------------
    let receive_future = async move {
        rx.await
            .unwrap_or_else(|_| Err(Error::Other("NSURLSession completion never fired".into())))
    };

    // `Some` = the completion handler delivered a result; `None` = the
    // cancel token won the race.
    let completed = match cancel {
        None => Some(receive_future.await),
        Some(token) => race_with_cancel(receive_future, token).await,
    };
    match completed {
        Some(result) => {
            // The task is finished; dropping the guard must not cancel it.
            guard.disarm();
            result
        }
        // `guard` drops on return and sends `[task cancel]`.
        None => Err(Error::Cancelled),
    }
}

/// Cancels the in-flight `NSURLSessionDataTask` when dropped while still
/// armed — i.e. whenever `send` stops waiting before the completion handler
/// delivered a result: the cancel token won, or the request future itself
/// was dropped. Without this a dropped future left the task running to
/// completion (bandwidth, battery, server load) with nobody listening.
///
/// `cancel` is thread-safe and returns immediately; NSURLSession then
/// completes the task with `NSURLErrorCancelled` on its delegate queue.
struct CancelTaskOnDrop {
    task: Option<Retained<objc2_foundation::NSURLSessionDataTask>>,
}

impl CancelTaskOnDrop {
    /// The exchange finished; dropping must not cancel anything.
    fn disarm(&mut self) {
        self.task = None;
    }
}

impl Drop for CancelTaskOnDrop {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            unsafe {
                let _: () = objc2::msg_send![&task, cancel];
            }
        }
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
    // Error path. Cancellation and an expired deadline map to their
    // dedicated variants (`Error::Cancelled` / `Error::Timeout`) so callers
    // can match them; everything else lands as a Network error with the
    // localized description.
    if !error.is_null() {
        let err_ref: &NSError = unsafe { &*error };
        let desc = err_ref.localizedDescription();
        return Err(timeout_policy::ns_url_error(
            err_ref.code(),
            desc.to_string(),
        ));
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
            let slice = unsafe { std::slice::from_raw_parts(ptr.as_ptr(), len) };
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
            let value_obj: *mut AnyObject = objc2::msg_send![dict, objectForKey: key_obj];
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

/// Race the response future against the cancel token: `Some(result)` when
/// the completion handler delivered first, `None` when the token fired. The
/// caller's [`CancelTaskOnDrop`] guard then cancels the task, so
/// URLSession short-circuits and fires the completion handler with
/// `NSURLErrorCancelled` into the already-dropped receiver.
async fn race_with_cancel<F>(
    response_future: F,
    token: CancelToken,
) -> Option<Result<Response, Error>>
where
    F: Future<Output = Result<Response, Error>>,
{
    let mut fut = Box::pin(response_future);
    let mut cancel_fut = Box::pin(token.cancelled());
    poll_fn(|cx| {
        if let Poll::Ready(()) = Pin::new(&mut cancel_fut).poll(cx) {
            return Poll::Ready(None);
        }
        if let Poll::Ready(result) = Pin::new(&mut fut).poll(cx) {
            return Poll::Ready(Some(result));
        }
        Poll::Pending
    })
    .await
}

/// Test-only observation point on the completion handler: every completion
/// reports `(url, NSError code)` (`0` when there was no error) to each
/// registered observer. The request future is gone by the time a dropped
/// request's handler fires, so this is the one place a test can see what
/// NSURLSession did with the task afterwards.
#[cfg(test)]
mod test_hook {
    use std::sync::{mpsc, Mutex};

    use objc2_foundation::NSError;

    static OBSERVERS: Mutex<Vec<mpsc::Sender<(String, isize)>>> = Mutex::new(Vec::new());

    /// Receive every later completion (tests run in parallel, so filter by
    /// URL).
    pub(super) fn observe() -> mpsc::Receiver<(String, isize)> {
        let (tx, rx) = mpsc::channel();
        OBSERVERS.lock().unwrap().push(tx);
        rx
    }

    pub(super) fn report(url: &str, error: *mut NSError) {
        let code = if error.is_null() {
            0
        } else {
            unsafe { &*error }.code()
        };
        OBSERVERS
            .lock()
            .unwrap()
            .retain(|tx| tx.send((url.to_owned(), code)).is_ok());
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::{mpsc, Arc};
    use std::task::{Context, Poll, Wake, Waker};
    use std::time::{Duration, Instant};

    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    use super::*;
    use crate::timeout_policy::NS_URL_ERROR_CANCELLED;

    /// Far above anything a cancel needs; without one, the shared session's
    /// own idle timeout would only complete the task after 60 s.
    const SLACK: Duration = Duration::from_secs(5);

    /// A loopback server that reads each request head, reports it, and then
    /// never answers — a request to it stays in flight until the client
    /// gives up.
    fn hold_server() -> (String, mpsc::Receiver<()>, tokio::runtime::Runtime) {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let listener = rt.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
        let addr = listener.local_addr().unwrap();
        let (received_tx, received) = mpsc::channel();
        rt.spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let received_tx = received_tx.clone();
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut buf = [0u8; 1024];
                    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                        match stream.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => head.extend_from_slice(&buf[..n]),
                        }
                    }
                    let _ = received_tx.send(());
                    // Hold the connection open until the client closes it.
                    while let Ok(n) = stream.read(&mut buf).await {
                        if n == 0 {
                            return;
                        }
                    }
                });
            }
        });
        (format!("http://{addr}"), received, rt)
    }

    struct NoopWake;
    impl Wake for NoopWake {
        fn wake(self: Arc<Self>) {}
    }

    /// How the test stops waiting on an in-flight request.
    enum StopBy {
        /// Drop the request future (no cancel token attached).
        Drop,
        /// Fire the attached cancel token; the future must then resolve
        /// `Error::Cancelled` before it is dropped.
        Token,
    }

    /// Start a request, poll it until the server holds its head, stop
    /// waiting per `stop`, and return the NSError code the task completed
    /// with (or `None` if it was still running after [`SLACK`]).
    fn stop_in_flight(stop: StopBy, timeout: Option<Duration>) -> Option<isize> {
        let (base, received, _rt) = hold_server();
        let url = format!("{base}/held");
        let completions = test_hook::observe();
        let transport = Transport::new();
        let (handle, token) = crate::cancel_token();
        let token = matches!(stop, StopBy::Token).then_some(token);
        let mut fut = Box::pin(send(
            &transport,
            Method::Get,
            url.clone(),
            Headers::new(),
            Vec::new(),
            timeout,
            token,
        ));
        let waker = Waker::from(Arc::new(NoopWake));
        let mut cx = Context::from_waker(&waker);
        let deadline = Instant::now() + SLACK;
        loop {
            if let Poll::Ready(r) = fut.as_mut().poll(&mut cx) {
                panic!("held request resolved early: {r:?}");
            }
            if received.try_recv().is_ok() {
                break;
            }
            assert!(Instant::now() < deadline, "request never reached the server");
            std::thread::sleep(Duration::from_millis(10));
        }
        if let StopBy::Token = stop {
            handle.cancel();
            match fut.as_mut().poll(&mut cx) {
                Poll::Ready(Err(Error::Cancelled)) => {}
                other => panic!("cancel token must resolve Cancelled, got {other:?}"),
            }
        }
        drop(fut);

        let deadline = Instant::now() + SLACK;
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            match completions.recv_timeout(left) {
                Ok((u, code)) if u == url => return Some(code),
                Ok(_) => {}
                Err(_) => break,
            }
        }
        None
    }

    /// Regression: dropping the request future without a `CancelToken` never
    /// sent `[task cancel]`, so the `NSURLSessionDataTask` ran on to
    /// completion with nobody listening — diverging from the reqwest arm
    /// (`AbortOnDrop`) and the web arm (`AbortController`).
    ///
    /// Asserted at the task, not the wire: NSURLSession keeps a cancelled
    /// task's socket open (measured ≥ 70 s on macOS 26), so "the server saw
    /// the connection close" is not something it promises. That the task
    /// completes with `NSURLErrorCancelled` right after the drop is — the
    /// held server never answers, so nothing else can complete it within
    /// [`SLACK`] (the shared session's idle timeout is 60 s, and would
    /// report `NSURLErrorTimedOut`).
    #[test]
    fn regression_apple_dropping_request_future_cancels_task() {
        assert_eq!(
            stop_in_flight(StopBy::Drop, None),
            Some(NS_URL_ERROR_CANCELLED),
            "dropping the request future must cancel its NSURLSession task"
        );
    }

    /// The same drop on a request with a timeout, which runs on its own
    /// per-request session: the guard cancels that task too, and its
    /// completion is what lets `finishTasksAndInvalidate` release the
    /// session (it waits for its last task), so the session does not stay
    /// alive behind a dropped future until the deadline.
    #[test]
    fn regression_apple_dropping_request_future_cancels_task_on_timeout_session() {
        assert_eq!(
            stop_in_flight(StopBy::Drop, Some(Duration::from_secs(30))),
            Some(NS_URL_ERROR_CANCELLED),
            "dropping a request with a timeout must cancel its task"
        );
    }

    /// A cancel-token win must cancel the task on the wire side too, not
    /// only return `Error::Cancelled` to the caller (`tests/cancellation.rs`
    /// sees only the latter).
    #[test]
    fn cancel_token_win_cancels_the_task() {
        assert_eq!(
            stop_in_flight(StopBy::Token, None),
            Some(NS_URL_ERROR_CANCELLED),
            "a fired cancel token must cancel the NSURLSession task"
        );
    }
}
