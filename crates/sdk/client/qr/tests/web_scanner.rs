//! The live scanner in a real browser: frames tapped on the main thread,
//! decoded in a Web Worker (`offload` → `web_glue::worker`), result delivered
//! back to the awaiting future. A decode that failed to reach the worker
//! would surface as `ScanError::Decoder`, not a scan.
//!
//! Run with `cargo test -p qr --target wasm32-unknown-unknown` (the
//! workspace runner supplies the glue JS; a matching chromedriver via
//! `CHROMEDRIVER`).

#![cfg(target_arch = "wasm32")]

use std::cell::Cell;
use std::rc::Rc;

use media_stream::MediaStream;
use qr::{decode_rgba8, QrScanner, ScanConfig, ScanError};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
use web_glue::js::Function;
use web_glue::JsValue;

wasm_bindgen_test_configure!(run_in_browser);

async fn sleep(ms: u32) {
    let p = Function::new_with_args("ms", "return new Promise((r) => setTimeout(r, ms));")
        .call1(&JsValue::UNDEFINED, &JsValue::from_f64(ms as f64))
        .unwrap();
    let _ = web_glue::JsFuture::new(&p).await;
}

const PAYLOAD: &str = "web worker scan";

/// White `size`² RGBA canvas with `PAYLOAD` as a QR code at 4 px/module,
/// offset 20 px (inside its quiet zone).
fn frame(size: u32) -> Vec<u8> {
    let code = qrcode::QrCode::new(PAYLOAD).unwrap();
    let n = code.width() as u32;
    let colors = code.to_colors();
    let mut rgba = [255u8; 4].repeat((size * size) as usize);
    for my in 0..n {
        for mx in 0..n {
            if colors[(my * n + mx) as usize] != qrcode::Color::Dark {
                continue;
            }
            for y in 20 + my * 4..20 + (my + 1) * 4 {
                for x in 20 + mx * 4..20 + (mx + 1) * 4 {
                    let i = ((y * size + x) * 4) as usize;
                    rgba[i..i + 3].copy_from_slice(&[0, 0, 0]);
                }
            }
        }
    }
    rgba
}

#[wasm_bindgen_test]
fn the_fixture_decodes_inline_in_the_browser() {
    let codes = decode_rgba8(140, 140, &frame(140));
    assert_eq!(codes.len(), 1);
    assert_eq!(codes[0].text(), Some(PAYLOAD));
}

#[wasm_bindgen_test]
async fn scans_a_live_stream_through_a_web_worker() {
    let (stream, writer) = MediaStream::new();
    let scanner = QrScanner::new(&stream, ScanConfig::default());
    let done = Rc::new(Cell::new(false));
    let pixels = frame(140);

    let scan = {
        let finished = done.clone();
        let next = async move {
            let r = scanner.next().await;
            finished.set(true);
            r
        };
        // The producer: one frame every 20 ms on the main thread, as the web
        // camera's frame pump delivers them.
        let produce = async {
            while !done.get() {
                writer.write_rgba8(140, 140, &pixels);
                sleep(20).await;
            }
        };
        let (scan, ()) = futures_join(next, produce).await;
        scan
    };
    let scan = scan.expect("the worker decode returned a scan");
    assert_eq!(scan.codes[0].text(), Some(PAYLOAD));
    assert_eq!((scan.width, scan.height), (140, 140));
}

#[wasm_bindgen_test]
async fn dropping_the_scanner_stops_a_pending_next_in_the_browser() {
    let (stream, _writer) = MediaStream::new();
    let scanner = QrScanner::new(&stream, ScanConfig::default());
    let next = scanner.next();
    drop(scanner);
    assert!(matches!(next.await, Err(ScanError::Stopped)));
}

/// Minimal two-future join (no `futures` dependency): poll both until both
/// finish.
async fn futures_join<A: std::future::Future, B: std::future::Future>(a: A, b: B) -> (A::Output, B::Output) {
    use std::pin::pin;
    use std::task::Poll;
    let (mut a, mut b) = (pin!(a), pin!(b));
    let (mut ra, mut rb) = (None, None);
    std::future::poll_fn(|cx| {
        if ra.is_none() {
            if let Poll::Ready(v) = a.as_mut().poll(cx) {
                ra = Some(v);
            }
        }
        if rb.is_none() {
            if let Poll::Ready(v) = b.as_mut().poll(cx) {
                rb = Some(v);
            }
        }
        if ra.is_some() && rb.is_some() {
            Poll::Ready((ra.take().unwrap(), rb.take().unwrap()))
        } else {
            Poll::Pending
        }
    })
    .await
}
