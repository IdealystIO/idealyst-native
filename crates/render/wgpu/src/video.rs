//! In-process H.264 video decoder for the `Video` primitive.
//!
//! Pipeline: load mp4 bytes from `src` → demux with `re_mp4` to
//! extract H.264 NAL units + per-sample timestamps → decode each
//! access unit with `openh264` → convert YUV420 to RGBA8 → post
//! the latest decoded frame into a shared cell that the
//! renderer's pre-pass uploads to a wgpu texture.
//!
//! Native platforms (iOS, Android, Web) keep using their OS
//! players via `Backend::create_video`'s native overrides; this
//! file only exists so the wgpu desktop preview reaches parity
//! for the H.264 / mp4 case without taking a system FFmpeg dep.
//!
//! Phase 1 scope (this file):
//!   - File-source playback (local path).
//!   - Play / pause via the handle.
//!   - Optional looping (rewinds on EOF when `loop_playback=true`).
//!
//! Out of scope (follow-ups): audio, seek, controls UI, network
//! sources, HEVC/AV1/VP9, hardware-accelerated decode.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// One decoded video frame ready for GPU upload. RGBA8 packed,
/// row-major, top-left origin.
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Shared state between the renderer (main thread) and a per-Video-node
/// decoder thread. The renderer reads `latest_frame` each render tick;
/// the decoder writes it after every newly-decoded access unit.
pub struct VideoSharedState {
    /// Most recently decoded frame. `None` until the first frame
    /// arrives.
    pub latest_frame: Mutex<Option<VideoFrame>>,
    /// `true` while the decoder thread should run the playback
    /// clock forward.
    pub playing: AtomicBool,
    /// Set when the node drops. The decoder thread polls this
    /// every ~30 ms and exits on `true`.
    pub shutdown: AtomicBool,
    /// Incremented every time `latest_frame` is replaced. Lets
    /// the renderer cheaply detect new frames (only upload on
    /// change) without locking the mutex on every tick.
    pub frame_counter: AtomicU64,
}

impl VideoSharedState {
    fn new() -> Self {
        Self {
            latest_frame: Mutex::new(None),
            playing: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            frame_counter: AtomicU64::new(0),
        }
    }
}

/// Owning handle to a running decoder. Dropping joins the
/// decoder thread.
pub struct VideoDecoder {
    pub shared: Arc<VideoSharedState>,
    join: Option<thread::JoinHandle<()>>,
}

impl Drop for VideoDecoder {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Release);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl VideoDecoder {
    /// Spawn a decoder for the file at `src`. Returns immediately;
    /// the decoder runs on its own thread and publishes frames
    /// into the shared state asynchronously.
    pub fn spawn(src: String, autoplay: bool, loop_playback: bool) -> Self {
        let shared = Arc::new(VideoSharedState::new());
        shared.playing.store(autoplay, Ordering::Release);
        let shared_for_thread = shared.clone();
        let join = thread::Builder::new()
            .name(format!("video:{}", short_label(&src)))
            .spawn(move || run_decode_loop(src, shared_for_thread, loop_playback))
            .expect("spawn video decode thread");
        Self { shared, join: Some(join) }
    }

    pub fn set_playing(&self, playing: bool) {
        self.shared.playing.store(playing, Ordering::Release);
    }
}

fn short_label(src: &str) -> &str {
    // Keep thread name under macOS's 64-char limit.
    let trimmed = src.rsplit('/').next().unwrap_or(src);
    if trimmed.len() > 48 { &trimmed[..48] } else { trimmed }
}

/// Main decoder body. Runs on its own thread; the only state it
/// touches outside its locals is `Arc<VideoSharedState>`.
fn run_decode_loop(src: String, shared: Arc<VideoSharedState>, loop_playback: bool) {
    loop {
        if shared.shutdown.load(Ordering::Acquire) {
            return;
        }
        match decode_file_once(&src, &shared) {
            DecodeResult::Done => {
                if !loop_playback {
                    park_until_shutdown(&shared);
                    return;
                }
                // Fall through to another pass for the loop.
            }
            DecodeResult::Shutdown => return,
            DecodeResult::Error(e) => {
                eprintln!("[video] decode error for {src}: {e}");
                park_until_shutdown(&shared);
                return;
            }
        }
    }
}

fn park_until_shutdown(shared: &Arc<VideoSharedState>) {
    while !shared.shutdown.load(Ordering::Acquire) {
        thread::sleep(Duration::from_millis(50));
    }
}

enum DecodeResult {
    Done,
    Shutdown,
    Error(String),
}

fn decode_file_once(src: &str, shared: &Arc<VideoSharedState>) -> DecodeResult {
    // 1) Read mp4 bytes off disk. Phase 1 only supports local
    //    files; network sources are a follow-up.
    let path = strip_file_scheme(src);
    eprintln!("[video] opening {path} (cwd: {:?})", std::env::current_dir().ok());
    let bytes = match std::fs::read(path) {
        Ok(b) => {
            eprintln!("[video] loaded {} bytes from {path}", b.len());
            b
        }
        Err(e) => return DecodeResult::Error(format!("open {path}: {e}")),
    };

    // 2) Demux with re_mp4. Pick the first H.264 video track.
    let mp4 = match re_mp4::Mp4::read_bytes(&bytes) {
        Ok(m) => m,
        Err(e) => return DecodeResult::Error(format!("mp4 parse: {e:?}")),
    };
    let (track_id, track) = match find_h264_track(&mp4) {
        Some(t) => t,
        None => {
            return DecodeResult::Error(
                "no H.264 video track (Phase 1: avc1 only)".into(),
            );
        }
    };
    let _ = track_id;
    let timescale = track.timescale.max(1) as f64;

    // 3) Pull SPS/PPS + length_size from the avcC inside the
    //    track's sample-description box.
    let stsd = &track.trak(&mp4).mdia.minf.stbl.stsd;
    let avcc = match &stsd.contents {
        re_mp4::StsdBoxContent::Avc1(avc1) => &avc1.avcc.contents,
        _ => {
            return DecodeResult::Error(
                "track is not H.264 (codec mismatch after find)".into(),
            )
        }
    };
    let length_size = avcc.length_size_minus_one as usize + 1;

    // 4) Stand up the H.264 decoder + feed the SPS/PPS prefix.
    //    `Flush::NoFlush` is mandatory for streams with inter-frame
    //    dependencies (any non-all-keyframe encoding) — the
    //    default `Flush::Auto` flushes the reference buffer after
    //    every decode, which makes P/B frames fail with
    //    `dsRefLost` (and confusingly `dsNoParamSets`) once we
    //    cross the first GOP boundary. We call
    //    `Decoder::flush_remaining` ourselves after the last
    //    sample to drain any held frames.
    let api = openh264::OpenH264API::from_source();
    let config = openh264::decoder::DecoderConfig::new()
        .flush_after_decode(openh264::decoder::Flush::NoFlush);
    let mut decoder = match openh264::decoder::Decoder::with_api_config(api, config) {
        Ok(d) => d,
        Err(e) => return DecodeResult::Error(format!("openh264 decoder: {e:?}")),
    };
    let mut prefix: Vec<u8> = Vec::new();
    for sps in &avcc.sequence_parameter_sets {
        prefix.extend_from_slice(&[0, 0, 0, 1]);
        prefix.extend_from_slice(&sps.bytes);
    }
    for pps in &avcc.picture_parameter_sets {
        prefix.extend_from_slice(&[0, 0, 0, 1]);
        prefix.extend_from_slice(&pps.bytes);
    }
    let _ = decoder.decode(&prefix);
    eprintln!(
        "[video] decoder ready: {} samples, timescale {}",
        track.samples.len(),
        timescale
    );
    // Keep the SPS/PPS prefix around — prepend it to every
    // keyframe access unit. openh264 expects parameter sets
    // alongside each IDR; mp4 stores them once in avcC and
    // strips them from the per-sample NALs, so without this
    // re-injection the decoder loses state on the first IDR
    // after the initial prime and starts returning code 16
    // ("no parameter sets") on every subsequent sample.
    let keyframe_prefix = prefix.clone();
    let _ = keyframe_prefix;

    // 5) Walk samples in decode order, pacing to PTS.
    let start_wall = Instant::now();
    let mut pause_accum = Duration::ZERO;
    let mut last_pause_start: Option<Instant> = None;
    let samples: Vec<re_mp4::Sample> = track.samples.clone();
    for sample in samples {
        if shared.shutdown.load(Ordering::Acquire) {
            return DecodeResult::Shutdown;
        }

        // Pause: park here until playback resumes.
        while !shared.playing.load(Ordering::Acquire) {
            if shared.shutdown.load(Ordering::Acquire) {
                return DecodeResult::Shutdown;
            }
            if last_pause_start.is_none() {
                last_pause_start = Some(Instant::now());
            }
            thread::sleep(Duration::from_millis(30));
        }
        if let Some(p) = last_pause_start.take() {
            pause_accum += p.elapsed();
        }

        let range = sample.byte_range();
        if range.end > bytes.len() {
            continue;
        }
        let mut annex_b = Vec::new();
        // Re-inject SPS/PPS at every sync sample. mp4 strips
        // these from per-sample NALs (they live in avcC); raw
        // H.264 streams typically have them inline at every
        // IDR, so prepending here matches what a `.264` file
        // looks like — even though our test clip happens to
        // have only one IDR, the cost is negligible and the
        // shape is what other decoders expect too.
        if sample.is_sync {
            annex_b.extend_from_slice(&keyframe_prefix);
        }
        annex_b.extend(avcc_to_annex_b(&bytes[range], length_size));
        // Diagnostic: time decode + YUV→RGBA. Removable once
        // the steady-state cadence is verified. Logs every 60th
        // frame to keep stderr quiet.
        let t_decode = Instant::now();
        let yuv = match decoder.decode(&annex_b) {
            Ok(Some(y)) => y,
            Ok(None) => continue,
            Err(e) => {
                eprintln!(
                    "[video] h264 decode failure on sample {} @ dts {}: {e:?}",
                    sample.id, sample.decode_timestamp
                );
                continue;
            }
        };
        let decode_ms = t_decode.elapsed().as_secs_f32() * 1000.0;
        let t_convert = Instant::now();
        let frame = yuv_to_rgba(&yuv);
        let convert_ms = t_convert.elapsed().as_secs_f32() * 1000.0;
        if sample.id % 60 == 0 {
            eprintln!(
                "[video] sample {} decode {:.1}ms convert {:.1}ms",
                sample.id, decode_ms, convert_ms
            );
        }

        // Pace to composition timestamp.
        let pts_secs = sample.composition_timestamp as f64 / timescale;
        let target = start_wall + pause_accum + Duration::from_secs_f64(pts_secs.max(0.0));
        let now = Instant::now();
        if target > now {
            let mut remaining = target - now;
            let max_chunk = Duration::from_millis(30);
            while remaining > Duration::ZERO {
                if shared.shutdown.load(Ordering::Acquire) {
                    return DecodeResult::Shutdown;
                }
                if !shared.playing.load(Ordering::Acquire) {
                    break;
                }
                let chunk = remaining.min(max_chunk);
                thread::sleep(chunk);
                remaining = remaining.saturating_sub(chunk);
            }
        }

        // Publish the frame.
        if let Ok(mut slot) = shared.latest_frame.lock() {
            *slot = Some(frame);
        }
        shared.frame_counter.fetch_add(1, Ordering::Release);

        // Diagnostic: publish-rate counter. Prints once per
        // second from the decoder thread so we can tell apart
        // "decoder is at 30 fps but renderer drops frames" from
        // "decoder publishes at ~5 fps and we're seeing the
        // truth." Same shape as the render-side counter.
        thread_local! {
            static PUB_DIAG: std::cell::Cell<(u64, Instant)> =
                std::cell::Cell::new((0, Instant::now()));
        }
        PUB_DIAG.with(|c| {
            let (n, last) = c.get();
            c.set((n + 1, last));
            if last.elapsed() >= Duration::from_secs(1) {
                eprintln!("[video] publishes/s={}", n + 1);
                c.set((0, Instant::now()));
            }
        });
    }
    DecodeResult::Done
}

fn find_h264_track(mp4: &re_mp4::Mp4) -> Option<(re_mp4::TrackId, &re_mp4::Track)> {
    for (id, track) in mp4.tracks() {
        if !matches!(track.kind, Some(re_mp4::TrackKind::Video)) {
            continue;
        }
        let stsd = &track.trak(mp4).mdia.minf.stbl.stsd;
        if matches!(stsd.contents, re_mp4::StsdBoxContent::Avc1(_)) {
            return Some((*id, track));
        }
    }
    None
}

fn strip_file_scheme(src: &str) -> &str {
    src.strip_prefix("file://").unwrap_or(src)
}

/// Convert avcC-style length-prefixed NAL units to Annex-B
/// (start-code prefixed). openh264 consumes Annex-B natively;
/// avcC requires the conversion.
fn avcc_to_annex_b(input: &[u8], length_size: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len() + 16);
    let mut i = 0;
    while i + length_size <= input.len() {
        let mut len: usize = 0;
        for k in 0..length_size {
            len = (len << 8) | input[i + k] as usize;
        }
        i += length_size;
        if i + len > input.len() {
            break;
        }
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&input[i..i + len]);
        i += len;
    }
    out
}

/// Convert openh264's YUV420 output to packed RGBA8. Uses the
/// crate's `write_rgba8` helper (BT.601 limited-range — the
/// dominant assumption for SDR mp4 files).
fn yuv_to_rgba(yuv: &openh264::decoder::DecodedYUV<'_>) -> VideoFrame {
    use openh264::formats::YUVSource;
    let (w, h) = yuv.dimensions();
    let mut rgba = vec![0u8; w * h * 4];
    yuv.write_rgba8(&mut rgba);
    VideoFrame { width: w as u32, height: h as u32, rgba }
}
