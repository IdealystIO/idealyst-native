//! Cross-platform Server-Sent Events client — the receive-only sibling
//! of [`WebSocket`](crate::WebSocket), for consuming an HTTP
//! `text/event-stream` (what a `#[sse]` endpoint serves).
//!
//! Same execution model as the WebSocket arm (no async runtime). Every
//! target reads the stream as raw bytes from its platform-native HTTP
//! source and feeds them to the shared [`SseDecoder`] (the only part with
//! real framing subtlety, written and tested once), then bridges each
//! event's `data:` payload to the async `recv()`:
//!
//! | target                | byte source                                  |
//! |-----------------------|----------------------------------------------|
//! | desktop / terminal    | `reqwest::blocking` on an I/O worker thread  |
//! | iOS                   | `NSURLSession` + `NSURLSessionDataDelegate`  |
//! | Android               | `HttpURLConnection.getInputStream()` via JNI |
//! | web (wasm32)          | the browser's `EventSource` (pre-parses)     |
//!
//! The web arm gets events already parsed by the browser, so it doesn't use
//! [`SseDecoder`]; the three native arms all do.
//!
//! `recv()` yields the raw `data:` payload string; the typed consumer
//! (`server::use_sse`) deserializes it.

use crate::error::Error;

/// A connected SSE stream. Closes on drop.
pub struct EventSource {
    inner: imp::EventSourceImpl,
}

impl EventSource {
    /// Open an `http(s)://…` event stream. Resolves once connected.
    pub async fn connect(url: &str) -> Result<EventSource, Error> {
        Ok(EventSource {
            inner: imp::connect(url).await?,
        })
    }

    /// Await the next event's `data:` payload. `None` = stream closed.
    pub async fn recv(&mut self) -> Option<Result<String, Error>> {
        self.inner.recv().await
    }

    /// Close the stream. Idempotent; also runs on drop.
    pub fn close(&self) {
        self.inner.close();
    }

    /// A cloneable close handle — lets a UI scope close the stream while
    /// a read loop owns the `EventSource`. Powers `use_sse`'s teardown.
    pub fn closer(&self) -> EventSourceCloser {
        EventSourceCloser {
            inner: self.inner.closer(),
        }
    }
}

/// Closes an [`EventSource`] (see [`EventSource::closer`]).
#[derive(Clone)]
pub struct EventSourceCloser {
    inner: imp::CloserImpl,
}

impl EventSourceCloser {
    /// Close the associated [`EventSource`]. Idempotent.
    pub fn close(&self) {
        self.inner.close();
    }
}

// ---------------------------------------------------------------------------
// Shared SSE byte→event decoder. Compiled for every native target (desktop
// reqwest, iOS NSURLSession, Android HttpURLConnection); the web arm gets
// pre-parsed events from the browser's `EventSource` and doesn't use this.
//
// All three native arms read the stream as raw bytes (each from a different
// I/O source) and feed them here, so the framing logic — the only part with
// real parsing subtlety — is written and tested once.
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use codec::SseDecoder;

#[cfg(not(target_arch = "wasm32"))]
mod codec {
    /// Stateful decoder: feed it arbitrary byte chunks (which may split a
    /// frame anywhere, or carry several frames at once) and it yields the
    /// `data:` payload of each frame that has completed, retaining any
    /// partial trailing frame for the next `push`.
    pub(crate) struct SseDecoder {
        buf: Vec<u8>,
    }

    impl SseDecoder {
        pub(crate) fn new() -> Self {
            Self { buf: Vec::new() }
        }

        /// Append `bytes`, then drain every complete frame, returning its
        /// data payload (frames with no `data:` line — e.g. keep-alive
        /// comments — produce nothing).
        pub(crate) fn push(&mut self, bytes: &[u8]) -> Vec<String> {
            self.buf.extend_from_slice(bytes);
            let mut out = Vec::new();
            while let Some(pos) = find_frame_end(&self.buf) {
                let frame: Vec<u8> = self.buf.drain(..pos).collect();
                // drop the blank-line separator that followed the frame
                let sep = separator_len(&self.buf);
                self.buf.drain(..sep);
                let text = String::from_utf8_lossy(&frame);
                if let Some(data) = parse_sse_data(&text) {
                    out.push(data);
                }
            }
            out
        }
    }

    /// Parse the `data:` payload out of one SSE frame (the text before a
    /// blank line). Multiple `data:` lines are joined with `\n` per the SSE
    /// spec; other fields (`event:`, `id:`, `retry:`, comments) are ignored.
    /// Returns `None` for a frame with no data (e.g. a keep-alive comment).
    fn parse_sse_data(frame: &str) -> Option<String> {
        let mut data: Option<String> = None;
        for line in frame.lines() {
            if let Some(rest) = line.strip_prefix("data:") {
                let value = rest.strip_prefix(' ').unwrap_or(rest);
                match &mut data {
                    Some(d) => {
                        d.push('\n');
                        d.push_str(value);
                    }
                    None => data = Some(value.to_string()),
                }
            }
        }
        data
    }

    /// Index of the end of the next complete frame (before its blank-line
    /// separator), handling both `\n\n` and `\r\n\r\n`.
    fn find_frame_end(buf: &[u8]) -> Option<usize> {
        buf.windows(2)
            .position(|w| w == b"\n\n")
            .or_else(|| buf.windows(4).position(|w| w == b"\r\n\r\n"))
    }

    fn separator_len(buf: &[u8]) -> usize {
        if buf.starts_with(b"\r\n\r\n") {
            4
        } else if buf.starts_with(b"\n\n") {
            2
        } else {
            0
        }
    }

    #[cfg(test)]
    mod tests {
        use super::SseDecoder;

        #[test]
        fn single_frame() {
            let mut d = SseDecoder::new();
            assert_eq!(d.push(b"data: hello\n\n"), vec!["hello".to_string()]);
        }

        #[test]
        fn frame_split_across_pushes() {
            let mut d = SseDecoder::new();
            assert!(d.push(b"data: par").is_empty());
            assert!(d.push(b"tial").is_empty());
            assert_eq!(d.push(b"\n\n"), vec!["partial".to_string()]);
        }

        #[test]
        fn multiple_frames_in_one_push() {
            let mut d = SseDecoder::new();
            assert_eq!(
                d.push(b"data: one\n\ndata: two\n\n"),
                vec!["one".to_string(), "two".to_string()]
            );
        }

        #[test]
        fn multi_data_lines_joined_with_newline() {
            let mut d = SseDecoder::new();
            assert_eq!(
                d.push(b"data: a\ndata: b\n\n"),
                vec!["a\nb".to_string()]
            );
        }

        #[test]
        fn crlf_framing() {
            let mut d = SseDecoder::new();
            assert_eq!(d.push(b"data: x\r\n\r\n"), vec!["x".to_string()]);
        }

        #[test]
        fn keepalive_comment_yields_nothing() {
            let mut d = SseDecoder::new();
            assert!(d.push(b": keep-alive\n\n").is_empty());
        }

        #[test]
        fn event_and_id_fields_ignored_data_kept() {
            let mut d = SseDecoder::new();
            assert_eq!(
                d.push(b"event: tick\nid: 7\ndata: payload\n\n"),
                vec!["payload".to_string()]
            );
        }

        #[test]
        fn value_without_leading_space_preserved() {
            // Only the single optional space after the colon is stripped.
            let mut d = SseDecoder::new();
            assert_eq!(d.push(b"data:no-space\n\n"), vec!["no-space".to_string()]);
        }

        #[test]
        fn partial_trailing_frame_retained() {
            let mut d = SseDecoder::new();
            assert_eq!(d.push(b"data: done\n\ndata: more"), vec!["done".to_string()]);
            assert_eq!(d.push(b"\n\n"), vec!["more".to_string()]);
        }
    }
}

// ---------------------------------------------------------------------------
// Native (desktop) arm: blocking reqwest read on a worker thread, bytes fed
// to the shared `SseDecoder`.
// ---------------------------------------------------------------------------

#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
mod imp {
    use super::SseDecoder;
    use crate::error::Error;

    use std::io::Read;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use futures_channel::mpsc as fut_mpsc;
    use futures_channel::oneshot;
    use futures_util::StreamExt;

    pub struct EventSourceImpl {
        inbound: fut_mpsc::UnboundedReceiver<Result<String, Error>>,
        closed: Arc<AtomicBool>,
    }

    #[derive(Clone)]
    pub struct CloserImpl {
        closed: Arc<AtomicBool>,
    }

    impl CloserImpl {
        pub fn close(&self) {
            self.closed.store(true, Ordering::Relaxed);
        }
    }

    pub async fn connect(url: &str) -> Result<EventSourceImpl, Error> {
        let (in_tx, in_rx) = fut_mpsc::unbounded::<Result<String, Error>>();
        let (ready_tx, ready_rx) = oneshot::channel::<Result<(), Error>>();
        let closed = Arc::new(AtomicBool::new(false));

        let url = url.to_string();
        let closed_thread = closed.clone();
        std::thread::Builder::new()
            .name("net-sse".into())
            .spawn(move || io_loop(url, in_tx, ready_tx, closed_thread))
            .map_err(|e| Error::Other(format!("sse thread spawn failed: {e}")))?;

        match ready_rx.await {
            Ok(Ok(())) => Ok(EventSourceImpl {
                inbound: in_rx,
                closed,
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(Error::Network("sse worker dropped during connect".into())),
        }
    }

    impl EventSourceImpl {
        pub async fn recv(&mut self) -> Option<Result<String, Error>> {
            self.inbound.next().await
        }
        pub fn close(&self) {
            self.closed.store(true, Ordering::Relaxed);
        }
        pub fn closer(&self) -> CloserImpl {
            CloserImpl {
                closed: self.closed.clone(),
            }
        }
    }

    impl Drop for EventSourceImpl {
        fn drop(&mut self) {
            self.closed.store(true, Ordering::Relaxed);
        }
    }

    fn io_loop(
        url: String,
        in_tx: fut_mpsc::UnboundedSender<Result<String, Error>>,
        ready_tx: oneshot::Sender<Result<(), Error>>,
        closed: Arc<AtomicBool>,
    ) {
        // `reqwest::blocking` runs its own internal runtime on this
        // thread, so no async runtime is required of the caller.
        let client = match reqwest::blocking::Client::builder().build() {
            Ok(c) => c,
            Err(e) => {
                let _ = ready_tx.send(Err(Error::Network(e.to_string())));
                return;
            }
        };
        let mut resp = match client.get(&url).header("accept", "text/event-stream").send() {
            Ok(r) => r,
            Err(e) => {
                let _ = ready_tx.send(Err(Error::Network(e.to_string())));
                return;
            }
        };
        if !resp.status().is_success() {
            let _ = ready_tx.send(Err(Error::Status {
                code: resp.status().as_u16(),
                body: None,
            }));
            return;
        }
        if ready_tx.send(Ok(())).is_err() {
            return; // caller went away
        }

        // Read chunks, feed the shared decoder, emit each completed frame's
        // data payload.
        let mut decoder = SseDecoder::new();
        let mut chunk = [0u8; 4096];
        loop {
            if closed.load(Ordering::Relaxed) {
                break;
            }
            let n = match resp.read(&mut chunk) {
                Ok(0) => break, // EOF: server ended the stream
                Ok(n) => n,
                Err(e) => {
                    let _ = in_tx.unbounded_send(Err(Error::Network(e.to_string())));
                    break;
                }
            };
            for data in decoder.push(&chunk[..n]) {
                if in_tx.unbounded_send(Ok(data)).is_err() {
                    return; // receiver dropped
                }
            }
        }
        // Dropping `in_tx` resolves the consumer's `recv()` to `None`.
    }
}

// ---------------------------------------------------------------------------
// Web arm: the browser's EventSource.
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
mod imp {
    use crate::error::Error;

    use std::cell::RefCell;
    use std::rc::Rc;

    use futures_channel::mpsc as fut_mpsc;
    use futures_channel::oneshot;
    use futures_util::StreamExt;
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::{JsCast, JsValue};
    use web_sys::{EventSource as WebEventSource, MessageEvent};

    type SenderCell = Rc<RefCell<Option<fut_mpsc::UnboundedSender<Result<String, Error>>>>>;

    pub struct EventSourceImpl {
        es: WebEventSource,
        inbound: fut_mpsc::UnboundedReceiver<Result<String, Error>>,
        _onmessage: Closure<dyn FnMut(MessageEvent)>,
        _onerror: Closure<dyn FnMut(web_sys::Event)>,
    }

    #[derive(Clone)]
    pub struct CloserImpl {
        es: WebEventSource,
    }

    impl CloserImpl {
        pub fn close(&self) {
            self.es.close();
        }
    }

    pub async fn connect(url: &str) -> Result<EventSourceImpl, Error> {
        let es = WebEventSource::new(url).map_err(js_err)?;
        let (in_tx, in_rx) = fut_mpsc::unbounded::<Result<String, Error>>();
        let sender: SenderCell = Rc::new(RefCell::new(Some(in_tx)));
        let (open_tx, open_rx) = oneshot::channel::<Result<(), Error>>();
        let open_tx = Rc::new(RefCell::new(Some(open_tx)));

        let onmessage = {
            let sender = sender.clone();
            let open_tx = open_tx.clone();
            Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
                // First message also confirms the connection is open.
                if let Some(t) = open_tx.borrow_mut().take() {
                    let _ = t.send(Ok(()));
                }
                if let Some(data) = e.data().as_string() {
                    if let Some(tx) = sender.borrow().as_ref() {
                        let _ = tx.unbounded_send(Ok(data));
                    }
                }
            })
        };
        es.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));

        let onopen = {
            let open_tx = open_tx.clone();
            Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
                if let Some(t) = open_tx.borrow_mut().take() {
                    let _ = t.send(Ok(()));
                }
            })
        };
        es.set_onopen(Some(onopen.as_ref().unchecked_ref()));

        // EventSource fires `error` both on transient reconnects and on
        // fatal failure; treat it as fatal here (close + end the stream).
        let onerror = {
            let sender = sender.clone();
            let open_tx = open_tx.clone();
            Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
                if let Some(t) = open_tx.borrow_mut().take() {
                    let _ = t.send(Err(Error::Network("event source error".into())));
                }
                *sender.borrow_mut() = None;
            })
        };
        es.set_onerror(Some(onerror.as_ref().unchecked_ref()));

        let result = open_rx
            .await
            .unwrap_or_else(|_| Err(Error::Network("event source open cancelled".into())));
        drop(onopen);
        result?;

        Ok(EventSourceImpl {
            es,
            inbound: in_rx,
            _onmessage: onmessage,
            _onerror: onerror,
        })
    }

    impl EventSourceImpl {
        pub async fn recv(&mut self) -> Option<Result<String, Error>> {
            self.inbound.next().await
        }
        pub fn close(&self) {
            self.es.close();
        }
        pub fn closer(&self) -> CloserImpl {
            CloserImpl { es: self.es.clone() }
        }
    }

    impl Drop for EventSourceImpl {
        fn drop(&mut self) {
            self.es.close();
        }
    }

    fn js_err(e: JsValue) -> Error {
        Error::Network(format!("{e:?}"))
    }
}

// ---------------------------------------------------------------------------
// iOS arm: NSURLSession streaming via an `NSURLSessionDataDelegate`.
//
// Gated to `target_os = "ios"` to match the HTTP transport split (`ios.rs` is
// the iOS transport; macOS/tvOS use the reqwest arm above) and avoid a
// duplicate `imp` on macOS.
//
// Unlike the one-shot HTTP transport (`ios.rs`), which uses the buffering
// `dataTaskWithRequest:completionHandler:` block API, SSE needs every chunk as
// it arrives — that requires a delegate. We don't implement
// `didReceiveResponse:completionHandler:` (its only job would be to return
// `.allow`, which is already the default when the method is absent), so the
// delegate has no Obj-C block parameters: just `didReceiveData:` and
// `didCompleteWithError:`. The delegate runs on URLSession's background serial
// queue and reaches the async consumer through `Send + Sync` shared state +
// `futures-channel`, exactly like the WebSocket and one-shot HTTP arms.
//
// `connect()` resolves on the first body byte (where we read the status off
// `dataTask.response`) rather than on headers, since we skip the response
// delegate method. For an SSE endpoint the server flushes headers and its
// first event together, so this is the same instant in practice.
// ---------------------------------------------------------------------------

#[cfg(target_os = "ios")]
mod imp {
    use super::SseDecoder;
    use crate::error::Error;

    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use futures_channel::mpsc as fut_mpsc;
    use futures_channel::oneshot;
    use futures_util::StreamExt;

    use objc2::rc::Retained;
    use objc2::runtime::{NSObjectProtocol, ProtocolObject};
    use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
    use objc2_foundation::{
        NSData, NSError, NSHTTPURLResponse, NSMutableURLRequest, NSObject, NSString, NSURL,
        NSURLResponse, NSURLSession, NSURLSessionConfiguration, NSURLSessionDataDelegate,
        NSURLSessionDataTask, NSURLSessionDelegate, NSURLSessionTask, NSURLSessionTaskDelegate,
    };

    /// Inactivity timeout for the request. SSE streams are long-lived and may
    /// idle between events; the default 60s would tear them down, so we set a
    /// day. (Servers that keep-alive with `:` comments stay well under this.)
    const SSE_REQUEST_TIMEOUT_SECS: f64 = 86_400.0;

    /// State shared between the delegate (background delegate queue) and the
    /// async consumer. Holds no Obj-C handle, so it is `Send + Sync`.
    struct Shared {
        /// Inbound payloads → `recv()`. Dropped (`None`) to end the stream.
        in_tx: Mutex<Option<fut_mpsc::UnboundedSender<Result<String, Error>>>>,
        /// Resolves `connect()` exactly once.
        ready: Mutex<Option<oneshot::Sender<Result<(), Error>>>>,
        decoder: Mutex<SseDecoder>,
        closed: AtomicBool,
    }

    impl Shared {
        fn resolve_ready(&self, r: Result<(), Error>) {
            if let Some(tx) = self.ready.lock().unwrap().take() {
                let _ = tx.send(r);
            }
        }
        fn end_stream(&self) {
            *self.in_tx.lock().unwrap() = None;
        }
        fn emit(&self, payloads: Vec<String>) {
            if let Some(tx) = self.in_tx.lock().unwrap().as_ref() {
                for p in payloads {
                    let _ = tx.unbounded_send(Ok(p));
                }
            }
        }
    }

    pub(crate) struct DelegateIvars {
        shared: Arc<Shared>,
    }

    declare_class!(
        struct SseDelegate;

        unsafe impl ClassType for SseDelegate {
            type Super = NSObject;
            type Mutability = mutability::InteriorMutable;
            const NAME: &'static str = "IdealystSseDelegate";
        }

        impl DeclaredClass for SseDelegate {
            type Ivars = DelegateIvars;
        }

        unsafe impl NSObjectProtocol for SseDelegate {}
        unsafe impl NSURLSessionDelegate for SseDelegate {}
        unsafe impl NSURLSessionTaskDelegate for SseDelegate {}
        unsafe impl NSURLSessionDataDelegate for SseDelegate {}

        unsafe impl SseDelegate {
            #[method(URLSession:dataTask:didReceiveData:)]
            fn did_receive_data(
                &self,
                _session: &NSURLSession,
                data_task: &NSURLSessionDataTask,
                data: &NSData,
            ) {
                let shared = &self.ivars().shared;

                // First chunk doubles as the connect confirmation: read the
                // HTTP status off the task's response and accept/reject.
                let still_pending = shared.ready.lock().unwrap().is_some();
                if still_pending {
                    match http_status(data_task) {
                        Some(code) if (200..300).contains(&code) => {
                            shared.resolve_ready(Ok(()));
                        }
                        Some(code) => {
                            shared.resolve_ready(Err(Error::Status { code, body: None }));
                            shared.closed.store(true, Ordering::Relaxed);
                            shared.end_stream();
                            unsafe {
                                let _: () = msg_send![data_task, cancel];
                            }
                            return;
                        }
                        // Non-HTTP response (file://, data://): treat as open.
                        None => shared.resolve_ready(Ok(())),
                    }
                }

                if shared.closed.load(Ordering::Relaxed) {
                    unsafe {
                        let _: () = msg_send![data_task, cancel];
                    }
                    return;
                }

                let bytes = nsdata_to_vec(data);
                let payloads = shared.decoder.lock().unwrap().push(&bytes);
                if !payloads.is_empty() {
                    shared.emit(payloads);
                }
            }

            #[method(URLSession:task:didCompleteWithError:)]
            fn did_complete_with_error(
                &self,
                _session: &NSURLSession,
                _task: &NSURLSessionTask,
                error: Option<&NSError>,
            ) {
                let shared = &self.ivars().shared;
                if let Some(err_ref) = error {
                    // -999 == NSURLErrorCancelled: our own close(), not a fault.
                    if err_ref.code() != -999 {
                        let desc = err_ref.localizedDescription().to_string();
                        shared.resolve_ready(Err(Error::Network(desc.clone())));
                        if let Some(tx) = shared.in_tx.lock().unwrap().as_ref() {
                            let _ = tx.unbounded_send(Err(Error::Network(desc)));
                        }
                    }
                } else {
                    // Clean EOF before any data still counts as "connected".
                    shared.resolve_ready(Ok(()));
                }
                shared.end_stream();
            }
        }
    );

    /// Read the HTTP status code off a data task's response, if it has one
    /// and it is an HTTP(S) response. Mirrors the pointer downcast in
    /// `ios.rs::build_result` — for http(s) requests the response is always
    /// an `NSHTTPURLResponse`.
    fn http_status(task: &NSURLSessionDataTask) -> Option<u16> {
        let resp: Retained<NSURLResponse> = unsafe { task.response() }?;
        let ptr: *const NSURLResponse = &*resp;
        let http: *const NSHTTPURLResponse = ptr.cast();
        let code = unsafe { (*http).statusCode() };
        u16::try_from(code).ok()
    }

    /// Copy an `NSData` into an owned `Vec<u8>` (mirrors `ios.rs`).
    fn nsdata_to_vec(data: &NSData) -> Vec<u8> {
        let len = data.length();
        if len == 0 {
            return Vec::new();
        }
        let ptr = data.bytes();
        // SAFETY: NSData guarantees `bytes()` points at `length()` contiguous
        // bytes; we copy out so the result outlives the NSData.
        let slice = unsafe { std::slice::from_raw_parts(ptr.as_ptr() as *const u8, len) };
        slice.to_vec()
    }

    pub(crate) struct EventSourceImpl {
        // The session strongly retains the delegate and the task; holding it
        // keeps both alive and gives us `invalidateAndCancel` for teardown.
        session: Retained<NSURLSession>,
        _task: Retained<NSURLSessionDataTask>,
        inbound: fut_mpsc::UnboundedReceiver<Result<String, Error>>,
        shared: Arc<Shared>,
    }

    pub(crate) async fn connect(url: &str) -> Result<EventSourceImpl, Error> {
        let ns_url_string = NSString::from_str(url);
        let ns_url = unsafe { NSURL::URLWithString(&ns_url_string) }
            .ok_or_else(|| Error::InvalidUrl(url.to_string()))?;
        let mut request = unsafe { NSMutableURLRequest::requestWithURL(&ns_url) };
        let accept = NSString::from_str("text/event-stream");
        let accept_field = NSString::from_str("Accept");
        unsafe { request.setValue_forHTTPHeaderField(Some(&accept), &accept_field) };

        let (in_tx, in_rx) = fut_mpsc::unbounded::<Result<String, Error>>();
        let (ready_tx, ready_rx) = oneshot::channel::<Result<(), Error>>();
        let shared = Arc::new(Shared {
            in_tx: Mutex::new(Some(in_tx)),
            ready: Mutex::new(Some(ready_tx)),
            decoder: Mutex::new(SseDecoder::new()),
            closed: AtomicBool::new(false),
        });

        let delegate: Retained<SseDelegate> = {
            let this = SseDelegate::alloc().set_ivars(DelegateIvars {
                shared: shared.clone(),
            });
            unsafe { msg_send_id![super(this), init] }
        };

        let config = unsafe { NSURLSessionConfiguration::defaultSessionConfiguration() };
        unsafe { config.setTimeoutIntervalForRequest(SSE_REQUEST_TIMEOUT_SECS) };

        let proto: &ProtocolObject<dyn NSURLSessionDelegate> = ProtocolObject::from_ref(&*delegate);
        // `delegateQueue: nil` → URLSession creates a background serial queue
        // it delivers callbacks on, which is what our `Send + Sync` Shared
        // bridge expects.
        let session = unsafe {
            NSURLSession::sessionWithConfiguration_delegate_delegateQueue(&config, Some(proto), None)
        };
        let task = unsafe { session.dataTaskWithRequest(&request) };
        unsafe {
            let _: () = msg_send![&task, resume];
        }

        match ready_rx.await {
            Ok(Ok(())) => Ok(EventSourceImpl {
                session,
                _task: task,
                inbound: in_rx,
                shared,
            }),
            Ok(Err(e)) => {
                unsafe { session.invalidateAndCancel() };
                Err(e)
            }
            Err(_) => Err(Error::Network("sse delegate dropped during connect".into())),
        }
    }

    impl EventSourceImpl {
        pub(crate) async fn recv(&mut self) -> Option<Result<String, Error>> {
            self.inbound.next().await
        }
        pub(crate) fn close(&self) {
            self.shared.closed.store(true, Ordering::Relaxed);
            self.shared.end_stream();
            // Hard teardown: cancels the in-flight task and releases the
            // delegate. Safe to call repeatedly.
            unsafe { self.session.invalidateAndCancel() };
        }
        pub(crate) fn closer(&self) -> CloserImpl {
            CloserImpl {
                shared: self.shared.clone(),
            }
        }
    }

    impl Drop for EventSourceImpl {
        fn drop(&mut self) {
            self.close();
        }
    }

    /// Cross-handle close. Holds only the `Send + Sync` Shared (no Obj-C
    /// handle), so it stays cheap and thread-agnostic. Flips the closed flag
    /// and ends the stream; the delegate issues `[task cancel]` on its next
    /// callback, and the owning `EventSourceImpl::drop` does the hard
    /// `invalidateAndCancel`.
    #[derive(Clone)]
    pub(crate) struct CloserImpl {
        shared: Arc<Shared>,
    }

    impl CloserImpl {
        pub(crate) fn close(&self) {
            self.shared.closed.store(true, Ordering::Relaxed);
            self.shared.end_stream();
        }
    }
}

// ---------------------------------------------------------------------------
// Android arm: stream `HttpURLConnection.getInputStream()` via JNI on a
// blocking worker thread (no OkHttp — the one-shot HTTP transport in
// `android.rs` picked HttpURLConnection precisely to keep a zero Gradle/JAR
// footprint, and SSE keeps that posture). Bytes feed the shared `SseDecoder`;
// payloads bridge to async via `futures-channel`. Cancellation reuses the
// `android.rs` pattern: the connection is promoted to a JNI `GlobalRef` parked
// in a shared slot, and close/drop calls `disconnect()` on a fresh
// JVM-attached thread, which unblocks the worker's blocking `read()`.
// ---------------------------------------------------------------------------

#[cfg(target_os = "android")]
mod imp {
    use super::SseDecoder;
    use crate::error::Error;

    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use futures_channel::mpsc as fut_mpsc;
    use futures_channel::oneshot;
    use futures_util::StreamExt;
    use jni::objects::{GlobalRef, JObject, JString, JValue};
    use jni::JavaVM;

    /// Shared slot the worker parks the connection's `GlobalRef` into and the
    /// canceller takes it out of — identical role to `android.rs::ConnSlot`.
    type ConnSlot = Arc<Mutex<Option<GlobalRef>>>;

    pub(crate) struct EventSourceImpl {
        inbound: fut_mpsc::UnboundedReceiver<Result<String, Error>>,
        closed: Arc<AtomicBool>,
        conn_slot: ConnSlot,
    }

    pub(crate) async fn connect(url: &str) -> Result<EventSourceImpl, Error> {
        let (in_tx, in_rx) = fut_mpsc::unbounded::<Result<String, Error>>();
        let (ready_tx, ready_rx) = oneshot::channel::<Result<(), Error>>();
        let closed = Arc::new(AtomicBool::new(false));
        let conn_slot: ConnSlot = Arc::new(Mutex::new(None));

        // SAFETY: documented entry point for the host's JavaVM; the host
        // installs it once at startup (see android.rs).
        let ctx = ndk_context::android_context();
        let vm_ptr = ctx.vm() as *mut jni::sys::JavaVM;
        let vm = unsafe { JavaVM::from_raw(vm_ptr) }
            .map_err(|e| Error::Other(format!("invalid JavaVM pointer: {e}")))?;

        let url = url.to_string();
        let closed_thread = closed.clone();
        let slot_thread = conn_slot.clone();
        std::thread::Builder::new()
            .name("net-sse".into())
            .spawn(move || io_loop(vm, url, in_tx, ready_tx, closed_thread, slot_thread))
            .map_err(|e| Error::Other(format!("sse thread spawn failed: {e}")))?;

        match ready_rx.await {
            Ok(Ok(())) => Ok(EventSourceImpl {
                inbound: in_rx,
                closed,
                conn_slot,
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(Error::Network("sse worker dropped during connect".into())),
        }
    }

    impl EventSourceImpl {
        pub(crate) async fn recv(&mut self) -> Option<Result<String, Error>> {
            self.inbound.next().await
        }
        pub(crate) fn close(&self) {
            self.closed.store(true, Ordering::Relaxed);
            disconnect_slot(&self.conn_slot);
        }
        pub(crate) fn closer(&self) -> CloserImpl {
            CloserImpl {
                closed: self.closed.clone(),
                conn_slot: self.conn_slot.clone(),
            }
        }
    }

    impl Drop for EventSourceImpl {
        fn drop(&mut self) {
            self.close();
        }
    }

    #[derive(Clone)]
    pub(crate) struct CloserImpl {
        closed: Arc<AtomicBool>,
        conn_slot: ConnSlot,
    }

    impl CloserImpl {
        pub(crate) fn close(&self) {
            self.closed.store(true, Ordering::Relaxed);
            disconnect_slot(&self.conn_slot);
        }
    }

    /// Take any parked connection ref and call `disconnect()` on a fresh
    /// JVM-attached thread — this unblocks the worker's blocking `read()`
    /// (it throws IOException, which ends the loop). Mirrors the cancel path
    /// in `android.rs`. No-op if the worker already finished (slot is `None`).
    fn disconnect_slot(slot: &ConnSlot) {
        let Some(global) = slot.lock().unwrap().take() else {
            return;
        };
        std::thread::spawn(move || {
            let ctx = ndk_context::android_context();
            let vm_ptr = ctx.vm() as *mut jni::sys::JavaVM;
            let Ok(vm) = (unsafe { JavaVM::from_raw(vm_ptr) }) else {
                return;
            };
            let Ok(mut env) = vm.attach_current_thread() else {
                return;
            };
            let _ = env.call_method(global.as_obj(), "disconnect", "()V", &[]);
        });
    }

    /// RAII: free any leftover `GlobalRef` in the slot on the JNI-attached
    /// worker thread (single FFI hop), same as `android.rs::SlotGuard`.
    struct SlotGuard<'a>(&'a ConnSlot);
    impl Drop for SlotGuard<'_> {
        fn drop(&mut self) {
            let _ = self.0.lock().unwrap().take();
        }
    }

    fn io_loop(
        vm: JavaVM,
        url: String,
        in_tx: fut_mpsc::UnboundedSender<Result<String, Error>>,
        ready_tx: oneshot::Sender<Result<(), Error>>,
        closed: Arc<AtomicBool>,
        conn_slot: ConnSlot,
    ) {
        // `ready_tx` is consumed when `connect()` resolves (success or an
        // HTTP-status reject). If `run_stream` errors out *before* that point
        // (a JNI/setup failure), the sender is still here — route the error to
        // `connect()` so the caller sees the real cause, not a generic
        // "worker dropped". A failure *after* ready surfaces on the stream.
        let mut ready_tx = Some(ready_tx);
        if let Err(e) = run_stream(&vm, &url, &in_tx, &mut ready_tx, &closed, &conn_slot) {
            match ready_tx.take() {
                Some(tx) => {
                    let _ = tx.send(Err(e));
                }
                None => {
                    let _ = in_tx.unbounded_send(Err(e));
                }
            }
        }
        // Dropping `in_tx` resolves the consumer's `recv()` to `None`.
    }

    fn run_stream(
        vm: &JavaVM,
        url: &str,
        in_tx: &fut_mpsc::UnboundedSender<Result<String, Error>>,
        ready_tx: &mut Option<oneshot::Sender<Result<(), Error>>>,
        closed: &Arc<AtomicBool>,
        conn_slot: &ConnSlot,
    ) -> Result<(), Error> {
        let mut env = vm
            .attach_current_thread()
            .map_err(|e| Error::Other(format!("JNI attach failed: {e}")))?;
        // Drops first (declared after `env`) so a leftover GlobalRef is freed
        // on this attached thread.
        let _slot_guard = SlotGuard(conn_slot);

        // URL url = new URL(url_str); conn = url.openConnection();
        let url_class = env.find_class("java/net/URL").map_err(map_jni_err)?;
        let url_str: JString = env.new_string(url).map_err(map_jni_err)?;
        let url_obj = env
            .new_object(
                url_class,
                "(Ljava/lang/String;)V",
                &[JValue::Object(&JObject::from(url_str))],
            )
            .map_err(map_jni_err)?;
        let conn_obj = env
            .call_method(&url_obj, "openConnection", "()Ljava/net/URLConnection;", &[])
            .map_err(map_jni_err)?
            .l()
            .map_err(map_jni_err)?;

        // Publish a global ref so close()/cancel can `disconnect()` it.
        {
            let global = env.new_global_ref(&conn_obj).map_err(map_jni_err)?;
            *conn_slot.lock().unwrap() = Some(global);
        }

        // Accept: text/event-stream; Accept-Encoding: identity so the JVM
        // doesn't transparently gzip-buffer the stream (which would defeat
        // incremental delivery). No read timeout (0 = infinite) for a
        // long-lived stream.
        set_request_property(&mut env, &conn_obj, "Accept", "text/event-stream")?;
        set_request_property(&mut env, &conn_obj, "Accept-Encoding", "identity")?;
        env.call_method(&conn_obj, "setReadTimeout", "(I)V", &[JValue::Int(0)])
            .map_err(map_jni_err)?;

        // int code = conn.getResponseCode();
        let code = env
            .call_method(&conn_obj, "getResponseCode", "()I", &[])
            .map_err(map_jni_err)?
            .i()
            .map_err(map_jni_err)?;
        let status: u16 = u16::try_from(code).unwrap_or(0);
        if !(200..300).contains(&status) {
            if let Some(tx) = ready_tx.take() {
                let _ = tx.send(Err(Error::Status {
                    code: status,
                    body: None,
                }));
            }
            let _ = env.call_method(&conn_obj, "disconnect", "()V", &[]);
            return Ok(());
        }
        if let Some(tx) = ready_tx.take() {
            if tx.send(Ok(())).is_err() {
                // Caller went away before connect resolved.
                let _ = env.call_method(&conn_obj, "disconnect", "()V", &[]);
                return Ok(());
            }
        }

        // InputStream in = conn.getInputStream();
        let stream = env
            .call_method(&conn_obj, "getInputStream", "()Ljava/io/InputStream;", &[])
            .map_err(map_jni_err)?
            .l()
            .map_err(map_jni_err)?;

        const CHUNK: usize = 4096;
        let buf_java = env.new_byte_array(CHUNK as i32).map_err(map_jni_err)?;
        let mut decoder = SseDecoder::new();
        loop {
            if closed.load(Ordering::Relaxed) {
                break;
            }
            // int n = in.read(buf);  (blocks; disconnect() makes it throw)
            let read = match env.call_method(
                &stream,
                "read",
                "([B)I",
                &[JValue::Object(buf_java.as_ref())],
            ) {
                Ok(v) => v.i().map_err(map_jni_err)?,
                Err(_) => {
                    // IOException — typically our own disconnect() (close).
                    let _ = env.exception_clear();
                    break;
                }
            };
            if read < 0 {
                break; // EOF: server ended the stream
            }
            if read == 0 {
                continue;
            }
            let mut chunk = vec![0i8; read as usize];
            env.get_byte_array_region(&buf_java, 0, &mut chunk)
                .map_err(map_jni_err)?;
            let bytes: Vec<u8> = chunk.into_iter().map(|b| b as u8).collect();
            for data in decoder.push(&bytes) {
                if in_tx.unbounded_send(Ok(data)).is_err() {
                    break; // receiver dropped
                }
            }
        }

        let _ = env.call_method(&stream, "close", "()V", &[]);
        let _ = env.call_method(&conn_obj, "disconnect", "()V", &[]);
        Ok(())
    }

    fn set_request_property(
        env: &mut jni::JNIEnv<'_>,
        conn: &JObject<'_>,
        name: &str,
        value: &str,
    ) -> Result<(), Error> {
        let n: JString = env.new_string(name).map_err(map_jni_err)?;
        let v: JString = env.new_string(value).map_err(map_jni_err)?;
        env.call_method(
            conn,
            "setRequestProperty",
            "(Ljava/lang/String;Ljava/lang/String;)V",
            &[
                JValue::Object(&JObject::from(n)),
                JValue::Object(&JObject::from(v)),
            ],
        )
        .map_err(map_jni_err)?;
        Ok(())
    }

    fn map_jni_err(e: jni::errors::Error) -> Error {
        Error::Network(format!("JNI: {e}"))
    }
}
