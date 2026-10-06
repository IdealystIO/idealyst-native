//! Reading QR codes out of a live [`MediaStream`] — or a still image.
//!
//! The scanner is a **consumer** of a stream, not a camera. Open the camera
//! once, show it with the `video` SDK, and hand the *same* stream to
//! [`QrScanner::new`]: there is no second capture session and no
//! scanner-owned preview, so the app keeps full control of what's on screen.
//! Any producer works — a `screen-recorder` stream scans the same way.
//!
//! ```no_run
//! use qr::{QrScanner, ScanConfig, ScanError};
//! # async fn demo(stream: media_stream::MediaStream) -> Result<(), ScanError> {
//! let scanner = QrScanner::new(&stream, ScanConfig::default());
//! let scan = scanner.next().await?;           // next frame with ≥1 code
//! for code in &scan.codes {
//!     println!("{:?} at {:?}", code.text(), code.corners);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # How a frame becomes a result
//!
//! 1. While a [`next`](QrScanner::next) future is waiting, the stream's CPU
//!    tap ([`MediaStream::subscribe`]) converts the **next** frame to
//!    greyscale — box-filtered down so its longer edge is at most
//!    [`ScanConfig::max_dimension`] — and parks it. Frames that arrive while
//!    nobody is waiting, or while a decode is running, are skipped untouched:
//!    the scanner always decodes the freshest frame and never queues a
//!    backlog.
//! 2. The parked frame is decoded **off the main thread** through `offload`
//!    (a Web Worker on web, a `std::thread` on native) with `rqrr`, a
//!    pure-Rust decoder — the same decoder on every target.
//! 3. A frame with no readable code loops back to 1; the first frame with
//!    one resolves the future with a [`Scan`].
//!
//! Scanning therefore costs CPU only while a `next()` is pending. Throttle by
//! waiting between calls; stop by dropping the scanner.
//!
//! # Lifetime
//!
//! [`QrScanner`] is a cloneable handle, like [`MediaStream`]. It holds a clone
//! of the stream, so capture keeps running while any scanner clone is alive;
//! dropping the **last** clone unsubscribes, releases the stream, and resolves
//! every pending `next()` with [`ScanError::Stopped`] — a scan loop exits
//! instead of waiting forever on a camera that's gone. A `next()` future holds
//! only the mailbox, never the stream, so an orphaned future cannot keep the
//! camera on.
//!
//! In an idealyst app, keep the scanner in a signal next to the stream and
//! drive the loop with `spawn_then`; clearing the signal (or unmounting) stops
//! the scan and frees the camera. Take the FIRST `next()` from the local
//! scanner before storing it: a staged `set` isn't visible to a `get` in the
//! same turn, so a loop started from the just-set signal never runs. See the
//! README for the full pattern.
//!
//! # Still images
//!
//! [`decode_rgba8`] / [`decode_luma8`] decode one image synchronously on the
//! calling thread — for a photo from `file-picker`, a screenshot, a test
//! fixture. Large images are CPU-heavy; call them inside your own
//! `offload::job` if the image comes from user input on the main thread.

mod decode;

use std::future::{poll_fn, Future};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use media_stream::{MediaStream, Subscription, VideoFrame};
use serde::{Deserialize, Serialize};

use decode::LumaFrame;

// ---------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------

/// A point in **source-frame** pixel coordinates (the stream frame's own
/// `width × height`, top-left origin), even when the frame was downscaled for
/// decoding.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Point {
    /// Horizontal pixel coordinate.
    pub x: f32,
    /// Vertical pixel coordinate.
    pub y: f32,
}

/// One QR code read from an image.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScannedCode {
    /// The decoded payload. Most codes carry UTF-8 text (see
    /// [`text`](Self::text)); binary-mode codes carry arbitrary bytes.
    pub bytes: Vec<u8>,
    /// The code's outline in source-frame pixels, ordered
    /// `[top-left, top-right, bottom-right, bottom-left]` relative to the
    /// code itself — so a rotated code reports rotated corners. Map these onto
    /// your preview to draw a highlight.
    pub corners: [Point; 4],
    /// QR version (1–40), i.e. the module grid size `17 + 4 × version`.
    pub version: u32,
}

impl ScannedCode {
    /// The payload as UTF-8 text, or `None` if it isn't valid UTF-8.
    pub fn text(&self) -> Option<&str> {
        std::str::from_utf8(&self.bytes).ok()
    }
}

/// Every code read from one frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scan {
    /// The codes found, in no particular order. Never empty when returned by
    /// [`QrScanner::next`]; may be empty from [`QrScanner::next_frame`] and
    /// [`decode_rgba8`].
    pub codes: Vec<ScannedCode>,
    /// Width of the frame the codes were read from, in pixels — the space
    /// [`ScannedCode::corners`] is in.
    pub width: u32,
    /// Height of that frame, in pixels.
    pub height: u32,
    /// The frame's capture timestamp on the `media_stream::clock` timeline,
    /// in microseconds (`0` for still images).
    pub pts_micros: u64,
}

/// Why [`QrScanner::next`] produced no scan.
#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    /// The last [`QrScanner`] clone was dropped while (or before) this
    /// `next()` was waiting. The scan loop should exit.
    #[error("the QR scanner was stopped")]
    Stopped,
    /// The off-thread decode did not return: the decoder panicked, or (web)
    /// its Web Worker failed to start — e.g. a bundle built without worker
    /// support. The cause is logged to the console on web.
    #[error("the QR decoder failed: {0}")]
    Decoder(#[from] offload::OffloadError),
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// The longest edge, in pixels, a frame is decoded at by default.
///
/// 960 keeps a 720p frame at full resolution and halves 1080p, which is where
/// decode time stops being negligible on a phone. A code needs roughly 2–3
/// decoded pixels per module to read, so at 960 a version-4 code (33 modules
/// plus quiet zone) still reads when it fills ~10 % of the frame width.
pub const DEFAULT_MAX_DIMENSION: u32 = 960;

/// Scanner tuning. `ScanConfig::default()` suits a phone or laptop camera.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanConfig {
    /// Frames whose longer edge exceeds this are box-filtered down by the
    /// smallest integer factor that fits before decoding. Lower is faster;
    /// higher reads smaller / more distant codes. Corners are always reported
    /// in source-frame pixels regardless.
    pub max_dimension: u32,
}

impl Default for ScanConfig {
    fn default() -> Self {
        ScanConfig {
            max_dimension: DEFAULT_MAX_DIMENSION,
        }
    }
}

impl ScanConfig {
    /// Set [`max_dimension`](Self::max_dimension).
    pub fn max_dimension(mut self, px: u32) -> Self {
        self.max_dimension = px;
        self
    }
}

// ---------------------------------------------------------------------------
// The live scanner
// ---------------------------------------------------------------------------

/// The hand-off between the stream's frame tap (a capture thread on native,
/// the main thread on web) and the waiting `next()` futures. `Send`, because
/// the tap is.
#[derive(Default)]
struct Mailbox {
    /// A `next()` is waiting for a frame; the tap converts only while set.
    want: bool,
    /// The one parked frame. Never more than one: a newer frame is only
    /// converted after this one is taken and `want` is set again.
    frame: Option<LumaFrame>,
    wakers: Vec<Waker>,
    /// The last scanner clone dropped.
    closed: bool,
}

type Shared = Arc<Mutex<Mailbox>>;

struct Inner {
    shared: Shared,
    // Field order is drop order: unsubscribe before releasing the stream, so
    // the stream's stopper never runs with our tap still registered.
    _subscription: Subscription,
    _stream: MediaStream,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Close and collect the wakers under the lock, wake outside it — a
        // woken future may be polled inline by a run-to-completion executor.
        let wakers = {
            let mut m = self.shared.lock().unwrap();
            m.closed = true;
            m.want = false;
            m.frame = None;
            std::mem::take(&mut m.wakers)
        };
        for w in wakers {
            w.wake();
        }
    }
}

/// A live QR scan over a [`MediaStream`]. See the [module docs](self) for the
/// frame pipeline and lifetime rules.
///
/// Cloneable; clones share one tap. Scanning stops — and the stream clone it
/// holds is released — when the last clone drops.
#[derive(Clone)]
pub struct QrScanner {
    inner: Rc<Inner>,
}

/// Pointer identity, like [`MediaStream`]: a scanner is a handle to one live
/// tap, so clones are equal and two scanners never are. This is what lets an
/// `Option<QrScanner>` sit in a signal.
impl PartialEq for QrScanner {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.inner, &other.inner)
    }
}

impl std::fmt::Debug for QrScanner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QrScanner").finish_non_exhaustive()
    }
}

impl QrScanner {
    /// Start tapping `stream`'s frames. Cheap: no frame is converted or
    /// decoded until a [`next`](Self::next) future is awaited.
    ///
    /// The scanner keeps a clone of `stream`, so the source keeps capturing
    /// for as long as the scanner is alive even if the caller drops its own
    /// copy.
    pub fn new(stream: &MediaStream, config: ScanConfig) -> QrScanner {
        let shared: Shared = Arc::default();
        let tap = shared.clone();
        let max_dimension = config.max_dimension;
        let subscription = stream.subscribe(move |frame: &VideoFrame| {
            let wakers = {
                let mut m = tap.lock().unwrap();
                if !m.want || m.closed {
                    // Nobody waiting, or a decode is in flight: skip this
                    // frame without touching its pixels.
                    return;
                }
                let Some(luma) = LumaFrame::from_rgba(
                    frame.width,
                    frame.height,
                    frame.data,
                    max_dimension,
                    frame.pts_micros,
                ) else {
                    return;
                };
                m.frame = Some(luma);
                m.want = false;
                std::mem::take(&mut m.wakers)
            };
            for w in wakers {
                w.wake();
            }
        });
        QrScanner {
            inner: Rc::new(Inner {
                shared,
                _subscription: subscription,
                _stream: stream.clone(),
            }),
        }
    }

    /// Wait for the next frame that contains at least one readable QR code.
    ///
    /// The returned future is `'static` and holds no reference to the
    /// scanner or the stream, so it can be handed to `spawn_then` /
    /// `spawn_async`. It resolves:
    ///
    /// - `Ok(scan)` with every code in that frame;
    /// - `Err(ScanError::Stopped)` once the last scanner clone is dropped;
    /// - `Err(ScanError::Decoder(_))` if the off-thread decode failed.
    ///
    /// It keeps decoding frames (one at a time, always the freshest) until
    /// one reads, so it uses a decoder thread's worth of CPU while pending.
    /// The same code is reported again on every call while it stays in view —
    /// de-duplicate in the caller if you only want changes.
    ///
    /// Several `next()` futures may be pending at once; each frame goes to
    /// whichever polls first.
    pub fn next(&self) -> impl Future<Output = Result<Scan, ScanError>> + 'static {
        let shared = self.inner.shared.clone();
        async move {
            loop {
                let scan = decode_next_frame(&shared).await?;
                if !scan.codes.is_empty() {
                    return Ok(scan);
                }
            }
        }
    }

    /// Like [`next`](Self::next), but resolves for EVERY decoded frame,
    /// including one with no readable code (`scan.codes` empty).
    ///
    /// Use it when you track what's in view rather than wait for a read —
    /// e.g. drawing an outline around a code that must disappear on the first
    /// frame the code is gone. Same lifetime and error rules as `next()`.
    pub fn next_frame(&self) -> impl Future<Output = Result<Scan, ScanError>> + 'static {
        let shared = self.inner.shared.clone();
        async move { decode_next_frame(&shared).await }
    }
}

/// Take the next parked frame and decode it off the main thread.
async fn decode_next_frame(shared: &Shared) -> Result<Scan, ScanError> {
    let frame = poll_fn(|cx| poll_frame(shared, cx)).await?;
    let scan = offload::run(offload::handle!(decode::decode_job), &frame).await?;
    // Stopped while the decode ran: the result belongs to a scan the caller
    // already abandoned.
    if shared.lock().unwrap().closed {
        return Err(ScanError::Stopped);
    }
    Ok(scan)
}

fn poll_frame(shared: &Shared, cx: &mut Context<'_>) -> Poll<Result<LumaFrame, ScanError>> {
    let mut m = shared.lock().unwrap();
    if m.closed {
        return Poll::Ready(Err(ScanError::Stopped));
    }
    if let Some(frame) = m.frame.take() {
        return Poll::Ready(Ok(frame));
    }
    m.want = true;
    if !m.wakers.iter().any(|w| w.will_wake(cx.waker())) {
        m.wakers.push(cx.waker().clone());
    }
    Poll::Pending
}

// ---------------------------------------------------------------------------
// Still images
// ---------------------------------------------------------------------------

/// Decode every QR code in one tightly-packed, top-down `RGBA8` image, at
/// full resolution, on the calling thread. Returns no codes if `data` is
/// shorter than `width * height * 4`.
pub fn decode_rgba8(width: u32, height: u32, data: &[u8]) -> Vec<ScannedCode> {
    match LumaFrame::from_rgba(width, height, data, u32::MAX, 0) {
        Some(f) => decode::decode_luma(f.width as usize, f.height as usize, &f.data, 1),
        None => Vec::new(),
    }
}

/// Decode every QR code in one 8-bit greyscale image (`width * height`
/// bytes, top-down) on the calling thread. Returns no codes if `data` is
/// shorter than `width * height`.
pub fn decode_luma8(width: u32, height: u32, data: &[u8]) -> Vec<ScannedCode> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || data.len() < w * h {
        return Vec::new();
    }
    decode::decode_luma(w, h, data, 1)
}
