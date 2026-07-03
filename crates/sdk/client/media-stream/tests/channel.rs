//! Portable tests for the frame channel — the CPU tap (subscribe + latest),
//! frame normalization (RGBA8 / BGRA8 swizzle), and lifecycle. Run anywhere;
//! no capture device needed.
//!
//! Subscriber callbacks use `Arc<Mutex<_>>` because the host `FrameCallback`
//! bound requires `Send` (frames arrive on a capture thread on native).

use std::sync::{Arc, Mutex};

use media_stream::{MediaStream, PixelFormat, VideoFrame};

#[test]
fn latest_starts_empty() {
    let (stream, _writer) = MediaStream::new();
    let mut buf = Vec::new();
    assert_eq!(stream.latest(&mut buf), None);
    assert_eq!(stream.generation(), 0);
}

#[test]
fn write_rgba8_updates_latest_and_generation() {
    let (stream, writer) = MediaStream::new();
    let data = [1u8, 2, 3, 4, 5, 6, 7, 8]; // 2x1 RGBA
    writer.write_rgba8(2, 1, &data);
    assert_eq!(stream.generation(), 1);
    let mut buf = Vec::new();
    assert_eq!(stream.latest(&mut buf), Some((2, 1)));
    assert_eq!(buf, data);
}

#[test]
fn write_bgra8_swizzles_to_rgba() {
    let (stream, writer) = MediaStream::new();
    // One BGRA pixel B=10 G=20 R=30 A=40 -> RGBA 30,20,10,40.
    writer.write_bgra8(1, 1, &[10, 20, 30, 40]);
    let mut buf = Vec::new();
    assert_eq!(stream.latest(&mut buf), Some((1, 1)));
    assert_eq!(buf, vec![30, 20, 10, 40]);
}

#[test]
fn short_frames_are_ignored() {
    let (stream, writer) = MediaStream::new();
    writer.write_rgba8(4, 4, &[0u8; 10]); // needs 64 bytes
    assert_eq!(stream.generation(), 0);
    let mut buf = Vec::new();
    assert_eq!(stream.latest(&mut buf), None);
}

#[test]
fn subscribe_receives_frames() {
    let (stream, writer) = MediaStream::new();
    let seen: Arc<Mutex<Vec<(u32, u32, usize)>>> = Arc::new(Mutex::new(Vec::new()));
    let seen_cb = seen.clone();
    let sub = stream.subscribe(move |f: &VideoFrame| {
        assert_eq!(f.format, PixelFormat::Rgba8);
        assert_eq!(f.data.len(), f.byte_len());
        seen_cb.lock().unwrap().push((f.width, f.height, f.data.len()));
    });

    writer.write_rgba8(1, 1, &[9, 9, 9, 9]);
    writer.write_rgba8(2, 1, &[1, 2, 3, 4, 5, 6, 7, 8]);

    assert_eq!(&*seen.lock().unwrap(), &[(1, 1, 4), (2, 1, 8)]);
    drop(sub);
}

#[test]
fn dropped_subscription_stops_receiving() {
    let (stream, writer) = MediaStream::new();
    let count = Arc::new(Mutex::new(0usize));
    let count_cb = count.clone();
    let sub = stream.subscribe(move |_| *count_cb.lock().unwrap() += 1);

    writer.write_rgba8(1, 1, &[0, 0, 0, 0]);
    assert_eq!(*count.lock().unwrap(), 1);

    drop(sub);
    writer.write_rgba8(1, 1, &[0, 0, 0, 0]);
    assert_eq!(*count.lock().unwrap(), 1, "no more callbacks after unsubscribe");
}

#[test]
fn stopper_runs_when_last_clone_drops() {
    let (stream, _writer) = MediaStream::new();
    let stopped = Arc::new(Mutex::new(false));
    let stopped_cb = stopped.clone();
    stream.attach_stopper(move || *stopped_cb.lock().unwrap() = true);

    let clone = stream.clone();
    drop(stream);
    assert!(!*stopped.lock().unwrap(), "still alive via the clone");
    drop(clone);
    assert!(*stopped.lock().unwrap(), "stopper ran on last drop");
}
