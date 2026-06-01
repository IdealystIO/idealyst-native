//! Cross-platform Server-Sent Events client — the receive-only sibling
//! of [`WebSocket`](crate::WebSocket), for consuming an HTTP
//! `text/event-stream` (what a `#[sse]` endpoint serves).
//!
//! Same execution model as the WebSocket arm (no async runtime): on
//! native a blocking I/O worker thread reads the stream and parses SSE
//! frames, bridging each event's `data:` payload to the async `recv()`
//! via `futures-channel`; on web the browser's `EventSource` does it and
//! callbacks marshal in. iOS/Android stub for now — WS `#[subscription]`
//! already covers server→client streaming on mobile.
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
    pub fn close(&self) {
        self.inner.close();
    }
}

/// Parse the `data:` payload out of one SSE frame (the text before a
/// blank line). Multiple `data:` lines are joined with `\n` per the SSE
/// spec; other fields (`event:`, `id:`, `retry:`, comments) are ignored.
/// Returns `None` for a frame with no data (e.g. a keep-alive comment).
/// Native-only — the web arm gets parsed events from the browser.
#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
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

// ---------------------------------------------------------------------------
// Native arm: blocking reqwest read on a worker thread + SSE parse.
// ---------------------------------------------------------------------------

#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
mod imp {
    use super::parse_sse_data;
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

        // Read chunks, split on the blank-line frame boundary, emit each
        // frame's data payload.
        let mut buf: Vec<u8> = Vec::new();
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
            buf.extend_from_slice(&chunk[..n]);
            while let Some(pos) = find_frame_end(&buf) {
                let frame: Vec<u8> = buf.drain(..pos).collect();
                // drop the blank-line separator that followed the frame
                let sep = separator_len(&buf);
                buf.drain(..sep);
                let text = String::from_utf8_lossy(&frame);
                if let Some(data) = parse_sse_data(&text) {
                    if in_tx.unbounded_send(Ok(data)).is_err() {
                        return; // receiver dropped
                    }
                }
            }
        }
        // Dropping `in_tx` resolves the consumer's `recv()` to `None`.
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
// iOS / Android: stub. WS `#[subscription]` covers mobile streaming; a
// native SSE arm (streaming NSURLSession / OkHttp read) is a follow-on.
// ---------------------------------------------------------------------------

#[cfg(any(target_os = "ios", target_os = "android"))]
mod imp {
    use crate::error::Error;

    pub async fn connect(_url: &str) -> Result<EventSourceImpl, Error> {
        Err(Error::Other(
            "EventSource (SSE) is not yet implemented on this platform; use a WebSocket \
             #[subscription] for server→client streaming on mobile"
                .into(),
        ))
    }

    pub struct EventSourceImpl;

    impl EventSourceImpl {
        pub async fn recv(&mut self) -> Option<Result<String, Error>> {
            None
        }
        pub fn close(&self) {}
        pub fn closer(&self) -> CloserImpl {
            CloserImpl
        }
    }

    #[derive(Clone)]
    pub struct CloserImpl;

    impl CloserImpl {
        pub fn close(&self) {}
    }
}
