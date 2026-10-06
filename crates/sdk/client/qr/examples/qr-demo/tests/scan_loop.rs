//! Regression: the demo's scan loop never read a code.
//!
//! The camera opened and the preview ran, but "Last code" stayed "—". The
//! start callback did `scanner_sig.set(Some(scanner))` and then started the
//! loop by reading `scanner_sig.get()` in the SAME turn. `set` only stages a
//! write until the driver flushes, so that read returned the old `None` and
//! the loop ended before its first `next()`. `start_scan` now takes the first
//! `next()` from the local scanner.
//!
//! Driven here exactly as the start callback drives it — `start_scan` inside
//! one turn — with a producer thread feeding a real QR frame through a
//! `MediaStream`, and a hand-pumped executor standing in for the async
//! driver. Own process: the executor goes in the global first-install-wins
//! slot (same rationale as idea-ui's `tests/loading_button_spawn.rs`).

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::Context;
use std::thread;
use std::time::{Duration, Instant};

use camera::MediaStream;
use host_mock::Harness;
use qr_demo::start_scan;
use qr::QrScanner;
use runtime_core::{signal, Signal};
use runtime_shared::driver::{install_async_executor, AsyncExecutor};

thread_local! {
    static TASKS: RefCell<Vec<Pin<Box<dyn Future<Output = ()> + 'static>>>> =
        const { RefCell::new(Vec::new()) };
}

struct TestExecutor;
// SAFETY: zero-sized; all live state is thread-local (each test thread pumps
// only its own queue) — the precedent in runtime-vocabulary's
// tests/async_reactive.rs.
unsafe impl Send for TestExecutor {}
unsafe impl Sync for TestExecutor {}

impl AsyncExecutor for TestExecutor {
    fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + 'static>>) {
        TASKS.with(|t| t.borrow_mut().push(future));
    }
}

/// Poll every queued future once; keep the pending ones.
fn pump_tasks() {
    let mut tasks = TASKS.with(|t| std::mem::take(&mut *t.borrow_mut()));
    let mut cx = Context::from_waker(std::task::Waker::noop());
    tasks.retain_mut(|f| f.as_mut().poll(&mut cx).is_pending());
    TASKS.with(|t| {
        let mut q = t.borrow_mut();
        tasks.append(&mut q);
        *q = tasks;
    });
}

fn pending_tasks() -> usize {
    TASKS.with(|t| t.borrow().len())
}

/// 320×240 white frame with `payload` as a QR at 4 px/module.
fn qr_frame(payload: &str) -> Vec<u8> {
    let (w, h) = (320u32, 240u32);
    let code = qrcode::QrCode::new(payload).unwrap();
    let n = code.width() as u32;
    let colors = code.to_colors();
    let mut rgba = [255u8; 4].repeat((w * h) as usize);
    for my in 0..n {
        for mx in 0..n {
            if colors[(my * n + mx) as usize] != qrcode::Color::Dark {
                continue;
            }
            for y in 40 + my * 4..40 + (my + 1) * 4 {
                for x in 80 + mx * 4..80 + (mx + 1) * 4 {
                    let i = ((y * w + x) * 4) as usize;
                    rgba[i..i + 3].copy_from_slice(&[0, 0, 0]);
                }
            }
        }
    }
    rgba
}

struct Signals {
    stream: Signal<Option<MediaStream>>,
    scanner: Signal<Option<QrScanner>>,
    last: Signal<String>,
}

#[test]
fn regression_scan_loop_reads_a_code_when_started_in_the_turn_that_stores_the_scanner() {
    install_async_executor(Box::new(TestExecutor));
    let h = Harness::new();

    let (stream, writer) = MediaStream::new();
    let stop = Arc::new(AtomicBool::new(false));
    let producer = {
        let (stop, writer, frame) = (stop.clone(), writer.clone(), qr_frame("from the loop"));
        thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                writer.write_rgba8(320, 240, &frame);
                thread::sleep(Duration::from_millis(10));
            }
        })
    };

    // One turn: create the signals and start, exactly as the camera-open
    // callback does.
    let sigs = h.world.enter(|| {
        let s = Signals {
            stream: signal(None),
            scanner: signal(None),
            last: signal("—".to_string()),
        };
        start_scan(stream, s.stream, s.scanner, signal(None), s.last);
        s
    });
    h.world.flush();

    // Two reads prove the loop re-arms from the committed signal, not just
    // that the first `next()` ran.
    let mut reads = 0;
    let deadline = Instant::now() + Duration::from_secs(10);
    while reads < 2 && Instant::now() < deadline {
        pump_tasks();
        h.world.flush();
        if sigs.last.get() != "—" {
            assert_eq!(sigs.last.get(), "from the loop");
            reads += 1;
            // Reset so the next read is a visible change.
            h.world.enter(|| sigs.last.set("—".to_string()));
            h.world.flush();
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(reads, 2, "the loop must read a code, then re-arm and read it again");

    // Stop, as the Stop button does: the loop ends and the tap is released.
    h.world.enter(|| {
        sigs.scanner.set(None);
        sigs.stream.set(None);
    });
    h.world.flush();
    let deadline = Instant::now() + Duration::from_secs(10);
    while pending_tasks() > 0 && Instant::now() < deadline {
        pump_tasks();
        h.world.flush();
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(pending_tasks(), 0, "Stop must end the scan loop");
    assert!(!writer.wants_cpu_frames(), "Stop must release the scanner's frame tap");

    stop.store(true, Ordering::Relaxed);
    producer.join().unwrap();
}
