//! `spawn_async` on GTK must not block the main thread.
//!
//! ## The bug this pins
//!
//! host-gtk installed no async executor, so `spawn_async` fell back to
//! `pollster::block_on` on the GTK main thread. A future that waits on
//! another thread — a `use_socket` receive loop — then owned the main thread
//! until it completed, and a never-ending one froze the window for good at 0%
//! CPU (Idea Chat: the window died the moment its live connection opened).
//!
//! The test spawns a future that waits on a value another thread delivers
//! 200 ms later. With the blocking fallback `spawn_async` returns only after
//! the value arrived; with the executor it returns at the first `.await`, and
//! the continuation runs later, on the main thread, once the main loop turns.
//!
//! One `#[test]`: GTK may be driven only from the thread that ran `gtk::init`.

#![cfg(target_os = "linux")]

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use backend_linux::gtk4;

/// A one-shot value set from another thread; awaiting it parks until then.
#[derive(Default)]
struct Slot {
    value: Option<u32>,
    waker: Option<Waker>,
}

struct Recv(Arc<Mutex<Slot>>);

impl Future for Recv {
    type Output = u32;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u32> {
        let mut s = self.0.lock().unwrap();
        match s.value {
            Some(v) => Poll::Ready(v),
            None => {
                s.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

static FLUSHES: AtomicUsize = AtomicUsize::new(0);
fn count_flush() {
    FLUSHES.fetch_add(1, Ordering::SeqCst);
}

#[test]
fn regression_spawn_async_does_not_block_the_gtk_main_thread() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display / GTK init failed");
        return;
    }
    host_gtk::install_scheduler();
    backend_linux::dispatch_hook::install_dispatch_hook(count_flush);

    let main = std::thread::current().id();
    let slot = Arc::new(Mutex::new(Slot::default()));
    let got: Arc<Mutex<Option<(u32, std::thread::ThreadId)>>> = Arc::default();

    // Deliver from another thread, as a socket's reader thread would.
    {
        let slot = slot.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            let mut s = slot.lock().unwrap();
            s.value = Some(7);
            if let Some(w) = s.waker.take() {
                w.wake();
            }
        });
    }

    let ran_before_await = Arc::new(Mutex::new(false));
    {
        let (slot, got, ran) = (slot.clone(), got.clone(), ran_before_await.clone());
        runtime_core::driver::spawn_async(async move {
            *ran.lock().unwrap() = true;
            let v = Recv(slot).await;
            *got.lock().unwrap() = Some((v, std::thread::current().id()));
        });
    }

    assert!(
        *ran_before_await.lock().unwrap(),
        "spawn_async runs the future up to its first `.await` before returning",
    );
    assert!(
        got.lock().unwrap().is_none(),
        "spawn_async must return at the first `.await`; it returned only after the \
         other thread delivered — the main thread was blocked on the future",
    );

    let flushes_before = FLUSHES.load(Ordering::SeqCst);
    let ctx = gtk4::glib::MainContext::default();
    let deadline = Instant::now() + Duration::from_secs(5);
    while got.lock().unwrap().is_none() && Instant::now() < deadline {
        ctx.iteration(false);
        std::thread::sleep(Duration::from_millis(1));
    }

    let (v, thread) = got.lock().unwrap().expect("the continuation ran once the main loop turned");
    assert_eq!(v, 7);
    assert_eq!(thread, main, "the continuation runs on the GTK main thread, not the waker's thread");
    assert!(
        FLUSHES.load(Ordering::SeqCst) > flushes_before,
        "a resumed poll fires the post-dispatch hook, so signal writes it staged commit",
    );
}
