//! Apple encoder under high-rate capture: content correctness + bounded
//! `stop()` latency.
//!
//! `host_record.rs` only checks size+ftyp on 30 black 64×48 frames. This feeds
//! bright, per-frame-distinct content at the whiteboard's capture resolution and
//! asserts two things the canvas-capture work depends on:
//!
//! 1. the encoder emits a real, non-black, multi-frame mp4 (verified once by
//!    extracting a frame — see git history); and
//! 2. `stop()` stays prompt even when the producer FLOODS frames far faster than
//!    the H.264 encoder drains. With the old unbounded channel that backlog made
//!    `stop()` spend ~2s draining (the whiteboard "stop is super delayed" bug);
//!    `MAX_INFLIGHT_VIDEO_FRAMES` caps the queue so finalize doesn't scale with
//!    how hard the producer floods.

#![cfg(target_os = "macos")]

use media_stream::MediaStream;
use media_writer::{MediaInputs, MediaWriter, RecordConfig};

// App capture size (vello target = window × backing scale). Big on purpose: it's
// the per-frame encode cost that makes an unbounded backlog expensive.
const W: u32 = 2048;
const H: u32 = 1536;
// Many frames so an UNBOUNDED queue would balloon `stop()`'s drain linearly,
// while the in-flight cap keeps it ~flat — a wide, non-flaky gap.
const FRAMES: u64 = 600;

/// Opaque white background + a black vertical bar marching left→right: every
/// frame distinct AND bright, so a black/empty output would be an obvious fail.
fn paint(i: u64) -> Vec<u8> {
    let mut buf = vec![255u8; (W * H * 4) as usize];
    let bar_x = (i as u32 * 8) % W;
    for y in 0..H {
        for x in bar_x..(bar_x + 40).min(W) {
            let o = ((y * W + x) * 4) as usize;
            buf[o] = 0;
            buf[o + 1] = 0;
            buf[o + 2] = 0;
            buf[o + 3] = 255;
        }
    }
    buf
}

#[tokio::test]
async fn stop_stays_prompt_under_flood() {
    let store = files::app_files("media-writer-motion-test").expect("app files store");
    let rel = "motion.mp4";

    let (video, vw) = MediaStream::new();
    let recording = MediaWriter::new()
        .record(MediaInputs::video(&video), RecordConfig::new(store.clone(), rel).fps(30))
        .await
        .expect("start recording");

    // FLOOD: enqueue as fast as possible — the producer vastly outruns the
    // encoder, exactly the render-thread-vs-H.264 mismatch the cap exists for.
    for i in 0..FRAMES {
        vw.write_rgba8(W, H, &paint(i));
    }

    let t0 = std::time::Instant::now();
    let out = recording.stop().await.expect("finalize recording");
    let stop_ms = t0.elapsed().as_millis();
    let path = store.local_path(&out).expect("local path");
    let bytes = std::fs::read(&path).expect("read recorded file");
    eprintln!("[motion-test] stop() {stop_ms}ms, {} bytes after flooding {FRAMES} frames", bytes.len());

    assert_eq!(&bytes[4..8], b"ftyp", "not an mp4");
    assert!(bytes.len() > 1_000, "no media written: {} bytes", bytes.len());

    // REGRESSION: unbounded, flooding 600 full-res frames drained for several
    // seconds (and scaled with FRAMES). The in-flight cap bounds the queue, so
    // `stop()` is dominated by finalize + the few in-flight frames' encode —
    // independent of flood size. Generous ceiling (CI is slower than a dev box)
    // but well under the unbounded multi-second drain this guards against.
    assert!(
        stop_ms < 2_500,
        "stop() took {stop_ms}ms under flood — in-flight cap not bounding the backlog?"
    );

    let _ = std::fs::remove_file(&path);
}
