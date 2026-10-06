//! End-to-end scanner behavior against a real `MediaStream`, fed by a
//! producer thread exactly as a camera backend feeds it (the `FrameWriter`
//! on a capture thread, the scanner on the "main" thread). Native only — it
//! needs threads; `web_scanner.rs` is the browser counterpart.

#![cfg(not(target_arch = "wasm32"))]

use std::future::Future;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::thread;
use std::time::Duration;

use media_stream::{FrameWriter, MediaStream};
use qr::{decode_luma8, decode_rgba8, QrScanner, ScanConfig, ScanError};

const WHITE: [u8; 4] = [255, 255, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];

/// A white RGBA canvas with `payload` encoded as a QR code whose top-left
/// module (excluding the quiet zone) sits at `(ox, oy)`, `module` px per module.
fn render(payload: &[u8], width: u32, height: u32, ox: u32, oy: u32, module: u32) -> Vec<u8> {
    let code = qrcode::QrCode::new(payload).unwrap();
    let n = code.width() as u32;
    let colors = code.to_colors();
    let mut rgba = WHITE.repeat((width * height) as usize);
    for my in 0..n {
        for mx in 0..n {
            if colors[(my * n + mx) as usize] != qrcode::Color::Dark {
                continue;
            }
            for y in oy + my * module..oy + (my + 1) * module {
                for x in ox + mx * module..ox + (mx + 1) * module {
                    let i = ((y * width + x) * 4) as usize;
                    rgba[i..i + 4].copy_from_slice(&BLACK);
                }
            }
        }
    }
    rgba
}

/// Pushes `frame()`'s current frame at ~100 fps until dropped.
struct Producer {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Producer {
    fn start(writer: FrameWriter, w: u32, h: u32, frame: Arc<Mutex<Vec<u8>>>) -> Producer {
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let thread = thread::spawn(move || {
            while !s.load(Ordering::Relaxed) {
                let f = frame.lock().unwrap().clone();
                writer.write_rgba8(w, h, &f);
                thread::sleep(Duration::from_millis(10));
            }
        });
        Producer {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for Producer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}

struct FlagWaker(AtomicBool);
impl Wake for FlagWaker {
    fn wake(self: Arc<Self>) {
        self.0.store(true, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// Still images
// ---------------------------------------------------------------------------

#[test]
fn decodes_a_still_rgba_image_with_corners_in_image_pixels() {
    // Version 1 (21 modules) at 4 px/module, top-left module at (40, 60).
    let rgba = render(b"hello idealyst", 300, 300, 40, 60, 4);
    let codes = decode_rgba8(300, 300, &rgba);
    assert_eq!(codes.len(), 1, "{codes:?}");
    let code = &codes[0];
    assert_eq!(code.text(), Some("hello idealyst"));
    assert_eq!(code.version, 1);
    // The outline spans the 84 px symbol; allow a module of slack for where
    // the detector places the edge.
    let tl = code.corners[0];
    let br = code.corners[2];
    assert!((tl.x - 40.0).abs() <= 4.0 && (tl.y - 60.0).abs() <= 4.0, "{tl:?}");
    assert!((br.x - 124.0).abs() <= 4.0 && (br.y - 144.0).abs() <= 4.0, "{br:?}");
}

#[test]
fn decodes_a_still_greyscale_image() {
    let rgba = render(b"grey", 120, 120, 16, 16, 4);
    let luma: Vec<u8> = rgba.chunks_exact(4).map(|p| p[0]).collect();
    let codes = decode_luma8(120, 120, &luma);
    assert_eq!(codes.len(), 1);
    assert_eq!(codes[0].text(), Some("grey"));
}

#[test]
fn binary_payloads_are_returned_as_bytes() {
    let payload = [0xffu8, 0x00, 0xfe, 0x80];
    let rgba = render(&payload, 160, 160, 20, 20, 4);
    let codes = decode_rgba8(160, 160, &rgba);
    assert_eq!(codes.len(), 1);
    assert_eq!(codes[0].bytes, payload);
    assert_eq!(codes[0].text(), None);
}

#[test]
fn an_image_without_a_code_yields_nothing() {
    assert!(decode_rgba8(64, 64, &WHITE.repeat(64 * 64)).is_empty());
}

#[test]
fn a_short_buffer_yields_nothing_instead_of_panicking() {
    assert!(decode_rgba8(64, 64, &[0; 100]).is_empty());
    assert!(decode_luma8(64, 64, &[0; 100]).is_empty());
}

// ---------------------------------------------------------------------------
// Live scanner
// ---------------------------------------------------------------------------

#[test]
fn scans_a_code_out_of_a_live_stream() {
    let (stream, writer) = MediaStream::new();
    let frame = Arc::new(Mutex::new(render(b"live frame", 320, 240, 100, 60, 4)));
    let _producer = Producer::start(writer, 320, 240, frame);

    let scanner = QrScanner::new(&stream, ScanConfig::default());
    let scan = pollster::block_on(scanner.next()).unwrap();
    assert_eq!(scan.codes[0].text(), Some("live frame"));
    assert_eq!((scan.width, scan.height), (320, 240));
    assert!(scan.pts_micros > 0, "a stream frame carries its capture time");
}

#[test]
fn next_skips_frames_without_a_code_until_one_appears() {
    let (stream, writer) = MediaStream::new();
    let frame = Arc::new(Mutex::new(WHITE.repeat(200 * 200)));
    let _producer = Producer::start(writer, 200, 200, frame.clone());
    let scanner = QrScanner::new(&stream, ScanConfig::default());

    let swap = {
        let frame = frame.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(150));
            *frame.lock().unwrap() = render(b"appeared", 200, 200, 30, 30, 4);
        })
    };
    let scan = pollster::block_on(scanner.next()).unwrap();
    swap.join().unwrap();
    assert_eq!(scan.codes[0].text(), Some("appeared"));
}

/// A frame larger than `max_dimension` is decoded downscaled, but corners
/// come back in SOURCE pixels — the space a preview overlay is drawn in.
#[test]
fn downscaled_frames_report_corners_in_source_pixels() {
    // 1600×1200 at max 400 → factor 4. 8 px/module → 2 px/module decoded.
    let (stream, writer) = MediaStream::new();
    let frame = Arc::new(Mutex::new(render(b"far away", 1600, 1200, 400, 320, 8)));
    let _producer = Producer::start(writer, 1600, 1200, frame);

    let scanner = QrScanner::new(&stream, ScanConfig::default().max_dimension(400));
    let scan = pollster::block_on(scanner.next()).unwrap();
    assert_eq!((scan.width, scan.height), (1600, 1200));
    let code = &scan.codes[0];
    assert_eq!(code.text(), Some("far away"));
    // Symbol spans 21 modules × 8 px = 168 px from (400, 320). One decoded
    // pixel of slack = 4 source px, plus a module.
    let (tl, br) = (code.corners[0], code.corners[2]);
    assert!((tl.x - 400.0).abs() <= 12.0 && (tl.y - 320.0).abs() <= 12.0, "{tl:?}");
    assert!((br.x - 568.0).abs() <= 12.0 && (br.y - 488.0).abs() <= 12.0, "{br:?}");
}

/// Regression guard for the lifetime contract: a scan loop awaiting `next()`
/// must END when the scanner is dropped, not hang on a camera that's gone.
#[test]
fn dropping_the_scanner_wakes_and_stops_a_pending_next() {
    let (stream, _writer) = MediaStream::new(); // no frames ever arrive
    let scanner = QrScanner::new(&stream, ScanConfig::default());
    let mut fut = pin!(scanner.next());

    let flag = Arc::new(FlagWaker(AtomicBool::new(false)));
    let waker = Waker::from(flag.clone());
    let mut cx = Context::from_waker(&waker);
    assert!(fut.as_mut().poll(&mut cx).is_pending());

    drop(scanner);
    assert!(flag.0.load(Ordering::SeqCst), "drop must wake the waiting future");
    assert!(matches!(
        fut.as_mut().poll(&mut cx),
        Poll::Ready(Err(ScanError::Stopped))
    ));
}

#[test]
fn next_after_the_last_clone_dropped_is_stopped() {
    let (stream, _writer) = MediaStream::new();
    let scanner = QrScanner::new(&stream, ScanConfig::default());
    let clone = scanner.clone();
    let fut = scanner.next();
    drop(scanner);
    // A clone is still alive: not stopped yet.
    let mut fut = Box::pin(fut);
    let mut cx = Context::from_waker(Waker::noop());
    assert!(fut.as_mut().poll(&mut cx).is_pending());
    drop(clone);
    assert!(matches!(
        fut.as_mut().poll(&mut cx),
        Poll::Ready(Err(ScanError::Stopped))
    ));
}

/// The scanner keeps the stream alive (the caller may drop its copy), and
/// releases both the CPU tap and the stream when it drops — so a stopped
/// scanner never leaves the camera running.
#[test]
fn the_scanner_holds_the_stream_and_releases_it_on_drop() {
    let (stream, writer) = MediaStream::new();
    let stopped = Arc::new(AtomicBool::new(false));
    {
        let stopped = stopped.clone();
        stream.attach_stopper(move || stopped.store(true, Ordering::SeqCst));
    }
    let scanner = QrScanner::new(&stream, ScanConfig::default());
    assert!(writer.wants_cpu_frames(), "the scanner taps CPU frames");

    drop(stream);
    assert!(!stopped.load(Ordering::SeqCst), "the scanner keeps capture alive");

    // A pending future must not keep the stream alive.
    let orphan = scanner.next();
    drop(scanner);
    assert!(stopped.load(Ordering::SeqCst), "the last scanner drop stops capture");
    assert!(!writer.wants_cpu_frames(), "the tap is removed");
    drop(orphan);
}

#[test]
fn clones_compare_by_identity() {
    let (stream, _w) = MediaStream::new();
    let a = QrScanner::new(&stream, ScanConfig::default());
    let b = QrScanner::new(&stream, ScanConfig::default());
    assert_eq!(a, a.clone());
    assert_ne!(a, b);
}

/// `next_frame` reports every decoded frame, so a caller can tell the code
/// LEFT the view (an outline must clear) — `next` would just keep waiting.
#[test]
fn next_frame_reports_frames_without_a_code_then_the_code() {
    let (stream, writer) = MediaStream::new();
    let frame = Arc::new(Mutex::new(WHITE.repeat(200 * 200)));
    let _producer = Producer::start(writer, 200, 200, frame.clone());
    let scanner = QrScanner::new(&stream, ScanConfig::default());

    let empty = pollster::block_on(scanner.next_frame()).unwrap();
    assert!(empty.codes.is_empty(), "a blank frame is reported, with no codes");
    assert_eq!((empty.width, empty.height), (200, 200));

    *frame.lock().unwrap() = render(b"in view", 200, 200, 30, 30, 4);
    // The frame in flight may still be the blank one; the next ones carry it.
    let mut seen = None;
    for _ in 0..20 {
        let scan = pollster::block_on(scanner.next_frame()).unwrap();
        if let Some(code) = scan.codes.first() {
            seen = Some(code.text().map(str::to_owned));
            break;
        }
    }
    assert_eq!(seen, Some(Some("in view".to_string())));

    *frame.lock().unwrap() = WHITE.repeat(200 * 200);
    let mut cleared = false;
    for _ in 0..20 {
        if pollster::block_on(scanner.next_frame()).unwrap().codes.is_empty() {
            cleared = true;
            break;
        }
    }
    assert!(cleared, "once the code leaves, next_frame reports empty frames again");
}

#[test]
fn next_frame_is_stopped_by_dropping_the_scanner() {
    let (stream, _writer) = MediaStream::new();
    let scanner = QrScanner::new(&stream, ScanConfig::default());
    let fut = scanner.next_frame();
    drop(scanner);
    assert!(matches!(pollster::block_on(fut), Err(ScanError::Stopped)));
}
