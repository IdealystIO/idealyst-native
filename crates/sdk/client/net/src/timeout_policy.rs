//! Timeout decisions for the NSURLSession (`ios.rs`) and HttpURLConnection
//! (`android.rs`) transports, kept free of objc/JNI so their regression
//! coverage runs on any host. The transports are FFI plumbing; what they
//! decide about a timeout lives here.
//!
//! # The contract every arm meets
//!
//! A request `timeout` is ONE deadline on the whole exchange — connect,
//! request, response head and body — and expiry resolves
//! [`Error::Timeout`]. reqwest's `RequestBuilder::timeout` and the web arm's
//! `setTimeout` + `AbortController` already mean exactly that.
//!
//! # NSURLSession
//!
//! `NSURLRequest.timeoutInterval` and the session's
//! `timeoutIntervalForRequest` are IDLE timeouts: the clock restarts on
//! every packet, so a body that trickles a byte at a time never expires.
//! `timeoutIntervalForResource` is the whole-transfer limit, i.e. the
//! contract. It is a session property, so a request with a timeout runs on
//! its own session (configured from `defaultSessionConfiguration`, which
//! shares the process-wide cookie store, credential store and URL cache the
//! shared session uses) whose three intervals are all set to the deadline.
//! Expiry completes the task with `NSURLErrorTimedOut`, mapped here.
//!
//! # HttpURLConnection
//!
//! `setConnectTimeout` / `setReadTimeout` bound one connect and one blocking
//! read each — a read timeout restarts per read, so they cannot bound the
//! total. A [`Watchdog`] thread enforces the deadline: at expiry it settles
//! the request with `Error::Timeout` and disconnects the connection, which
//! unblocks the worker thread. The per-phase timeouts are still set (to the
//! same deadline) so the worker also unblocks itself; their
//! `SocketTimeoutException` maps to `Error::Timeout` too.

use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use futures_channel::oneshot;

use crate::error::Error;

/// `NSURLErrorCancelled` (`NSURLErrorDomain`): the task was sent `cancel`.
pub(crate) const NS_URL_ERROR_CANCELLED: isize = -999;

/// `NSURLErrorTimedOut` (`NSURLErrorDomain`): a session or request timeout
/// interval elapsed.
pub(crate) const NS_URL_ERROR_TIMED_OUT: isize = -1001;

/// The smallest deadline the platform timers are given.
///
/// Both platforms read a zero interval as "no timeout of my own": Java's
/// `setConnectTimeout(0)` / `setReadTimeout(0)` mean infinite, and a
/// non-positive NSURLSession interval falls back to the system default.
/// `Duration::ZERO` must mean "expire immediately", so it is raised to the
/// smallest value each API can express.
const MIN_DEADLINE: Duration = Duration::from_millis(1);

/// A deadline as an `NSTimeInterval` (seconds), never zero.
#[cfg_attr(
    not(any(target_os = "ios", target_os = "macos", target_os = "tvos")),
    allow(dead_code)
)]
pub(crate) fn ns_time_interval(deadline: Duration) -> f64 {
    deadline.max(MIN_DEADLINE).as_secs_f64()
}

/// A deadline as Java's `int` milliseconds for `setConnectTimeout` /
/// `setReadTimeout`: rounded UP (a 1.5 ms deadline must not become 1 ms
/// early or, for sub-millisecond ones, 0 = infinite) and saturated at
/// `i32::MAX` (~24.8 days).
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn java_timeout_millis(deadline: Duration) -> i32 {
    let deadline = deadline.max(MIN_DEADLINE);
    let millis = deadline.as_nanos().div_ceil(1_000_000);
    i32::try_from(millis).unwrap_or(i32::MAX)
}

/// Map an `NSError` from `NSURLErrorDomain` to the crate error.
#[cfg_attr(
    not(any(target_os = "ios", target_os = "macos", target_os = "tvos")),
    allow(dead_code)
)]
pub(crate) fn ns_url_error(code: isize, description: String) -> Error {
    match code {
        NS_URL_ERROR_CANCELLED => Error::Cancelled,
        NS_URL_ERROR_TIMED_OUT => Error::Timeout,
        _ => Error::Network(description),
    }
}

/// Map a Java exception (its `toString()`, e.g.
/// `"java.net.SocketTimeoutException: Read timed out"`) to the crate
/// error. `SocketTimeoutException` is the connect/read timeout firing;
/// everything else is a network failure.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn java_exception_error(description: String) -> Error {
    // `toString()` is `<class name>` or `<class name>: <message>`.
    let class = description.split(':').next().unwrap_or_default().trim();
    if class == "java.net.SocketTimeoutException" {
        Error::Timeout
    } else {
        Error::Network(description)
    }
}

/// A one-shot timer thread: runs `on_expire` once `after` has elapsed,
/// unless the `Watchdog` was dropped first (dropping disarms it — the
/// thread wakes and exits without running anything).
///
/// A thread rather than an async timer because the crate has no runtime
/// (no async runtime is introduced anywhere, per the execution-model
/// invariant), and the Android arm already pays a worker thread per
/// request.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) struct Watchdog {
    /// Dropping this sender is the disarm signal: the timer thread's
    /// `recv_timeout` returns `Disconnected` instead of `Timeout`.
    _disarm: mpsc::Sender<()>,
}

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
impl Watchdog {
    pub(crate) fn arm(after: Duration, on_expire: impl FnOnce() + Send + 'static) -> Self {
        let (disarm, armed) = mpsc::channel::<()>();
        std::thread::spawn(move || {
            if let Err(mpsc::RecvTimeoutError::Timeout) = armed.recv_timeout(after) {
                on_expire();
            }
        });
        Self { _disarm: disarm }
    }
}

/// The sending half of a request's result channel, shared by everyone who
/// may settle it — the worker with the real result, the [`Watchdog`] with
/// `Error::Timeout`. The first `settle` wins; later ones are dropped, so a
/// worker that unblocks after the deadline cannot overwrite the timeout.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) struct FirstResult<T>(Arc<Mutex<Option<oneshot::Sender<T>>>>);

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
impl<T> FirstResult<T> {
    pub(crate) fn new() -> (Self, oneshot::Receiver<T>) {
        let (tx, rx) = oneshot::channel();
        (Self(Arc::new(Mutex::new(Some(tx)))), rx)
    }

    /// Deliver `value` if nobody has yet. Returns whether this call won.
    pub(crate) fn settle(&self, value: T) -> bool {
        // A poisoned lock only means another settler panicked mid-take;
        // the slot itself is still a valid `Option`.
        let sender = self.0.lock().unwrap_or_else(|p| p.into_inner()).take();
        match sender {
            Some(tx) => {
                let _ = tx.send(value);
                true
            }
            None => false,
        }
    }
}

impl<T> Clone for FirstResult<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Instant;

    use super::*;

    #[test]
    fn regression_zero_deadline_is_not_an_infinite_platform_timeout() {
        // Java reads 0 as "wait forever"; NSURLSession reads <= 0 as
        // "use the default". Zero must stay a real, tiny deadline.
        assert_eq!(java_timeout_millis(Duration::ZERO), 1);
        assert_eq!(java_timeout_millis(Duration::from_micros(10)), 1);
        assert!(ns_time_interval(Duration::ZERO) > 0.0);
    }

    #[test]
    fn java_millis_round_up_and_saturate() {
        assert_eq!(java_timeout_millis(Duration::from_millis(500)), 500);
        assert_eq!(java_timeout_millis(Duration::from_micros(1_500)), 2);
        assert_eq!(
            java_timeout_millis(Duration::from_secs(30 * 24 * 3600)),
            i32::MAX
        );
        assert_eq!(java_timeout_millis(Duration::MAX), i32::MAX);
    }

    #[test]
    fn ns_interval_is_seconds() {
        assert_eq!(ns_time_interval(Duration::from_millis(1_500)), 1.5);
    }

    #[test]
    fn regression_ns_timed_out_maps_to_timeout_not_network() {
        assert!(matches!(
            ns_url_error(-1001, "The request timed out.".into()),
            Error::Timeout
        ));
        assert!(matches!(
            ns_url_error(-999, "cancelled".into()),
            Error::Cancelled
        ));
        assert!(matches!(
            ns_url_error(-1004, "Could not connect to the server.".into()),
            Error::Network(m) if m == "Could not connect to the server."
        ));
    }

    #[test]
    fn regression_socket_timeout_exception_maps_to_timeout() {
        for s in [
            "java.net.SocketTimeoutException: Read timed out",
            "java.net.SocketTimeoutException: failed to connect to /10.0.2.2 (port 1) after 500ms",
            "java.net.SocketTimeoutException",
        ] {
            assert!(
                matches!(java_exception_error(s.into()), Error::Timeout),
                "{s}"
            );
        }
        assert!(matches!(
            java_exception_error("java.net.ConnectException: Connection refused".into()),
            Error::Network(_)
        ));
        // A different exception whose MESSAGE mentions the class is not a timeout.
        assert!(matches!(
            java_exception_error("java.io.IOException: java.net.SocketTimeoutException".into()),
            Error::Network(_)
        ));
    }

    #[test]
    fn watchdog_fires_after_its_deadline() {
        let fired = Arc::new(AtomicBool::new(false));
        let (done_tx, done_rx) = mpsc::channel();
        let start = Instant::now();
        let flag = fired.clone();
        let _dog = Watchdog::arm(Duration::from_millis(50), move || {
            flag.store(true, Ordering::SeqCst);
            let _ = done_tx.send(start.elapsed());
        });
        let elapsed = done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("watchdog never fired");
        assert!(fired.load(Ordering::SeqCst));
        assert!(
            elapsed >= Duration::from_millis(50),
            "fired early: {elapsed:?}"
        );
    }

    /// The Android arm's arbitration: the watchdog settles `Timeout` at the
    /// deadline, and the worker's late result (here, the `Network` error a
    /// disconnected socket throws) must not replace it.
    #[test]
    fn regression_late_worker_result_cannot_replace_a_timeout() {
        let (settle, rx) = FirstResult::<Result<u16, Error>>::new();
        let for_dog = settle.clone();
        let _dog = Watchdog::arm(Duration::from_millis(20), move || {
            for_dog.settle(Err(Error::Timeout));
        });
        let worker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            settle.settle(Err(Error::Network("Socket closed".into())))
        });
        let got = block_on(rx);
        assert!(matches!(got, Ok(Err(Error::Timeout))), "{got:?}");
        assert!(!worker.join().unwrap(), "the late worker result must lose");
    }

    #[test]
    fn worker_result_before_the_deadline_wins_and_disarms() {
        let (settle, rx) = FirstResult::<Result<u16, Error>>::new();
        let for_dog = settle.clone();
        let dog = Watchdog::arm(Duration::from_millis(100), move || {
            for_dog.settle(Err(Error::Timeout));
        });
        assert!(settle.settle(Ok(200)));
        drop(dog);
        assert!(matches!(block_on(rx), Ok(Ok(200))));
    }

    /// Block on a oneshot without an executor dependency: poll with a
    /// thread-parking waker.
    fn block_on<T>(mut rx: oneshot::Receiver<T>) -> Result<T, oneshot::Canceled> {
        use std::future::Future;
        use std::pin::Pin;
        use std::task::{Context, Poll, Wake, Waker};
        struct Unpark(std::thread::Thread);
        impl Wake for Unpark {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
        let mut cx = Context::from_waker(&waker);
        loop {
            if let Poll::Ready(v) = Pin::new(&mut rx).poll(&mut cx) {
                return v;
            }
            std::thread::park();
        }
    }

    #[test]
    fn dropped_watchdog_never_fires() {
        let fired = Arc::new(AtomicBool::new(false));
        let flag = fired.clone();
        let dog = Watchdog::arm(Duration::from_millis(50), move || {
            flag.store(true, Ordering::SeqCst)
        });
        drop(dog);
        std::thread::sleep(Duration::from_millis(200));
        assert!(!fired.load(Ordering::SeqCst));
    }
}
