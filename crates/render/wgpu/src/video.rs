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
    /// Wall-clock-style playback position in microseconds since
    /// the start of the clip. The decoder updates this after
    /// every publish so the controls UI can show a current time
    /// without locking the mutex.
    pub current_time_micros: AtomicU64,
    /// Total clip duration in microseconds, populated once the
    /// decoder has parsed the track. `0` means unknown / still
    /// loading.
    pub duration_micros: AtomicU64,
    /// Seek request: target position in microseconds + `Some`-ness
    /// is the "is requested" flag. The decoder picks it up at the
    /// top of each sample iteration, restarts at the nearest
    /// sync sample, and clears the field. Holding it inside a
    /// mutex (rather than two atomics for u64 + bool) keeps the
    /// "race-free clear after consume" cheap.
    pub seek_request: Mutex<Option<u64>>,
}

impl VideoSharedState {
    fn new() -> Self {
        Self {
            latest_frame: Mutex::new(None),
            playing: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            frame_counter: AtomicU64::new(0),
            current_time_micros: AtomicU64::new(0),
            duration_micros: AtomicU64::new(0),
            seek_request: Mutex::new(None),
        }
    }
}

/// Owning handle to a running decoder. Dropping joins the
/// decoder thread.
pub struct VideoDecoder {
    pub shared: Arc<VideoSharedState>,
    /// Audio side, if the file had a decodable audio track and the
    /// audio subsystem opened successfully. Drop here also drops
    /// the audio source registration.
    audio: Option<crate::audio::DecodedAudioHandle>,
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
    /// into the shared state asynchronously. If the file has a
    /// decodable audio track and the platform's audio subsystem
    /// opened, a sibling decoder thread also runs for audio.
    pub fn spawn(src: String, autoplay: bool, loop_playback: bool) -> Self {
        let shared = Arc::new(VideoSharedState::new());
        shared.playing.store(autoplay, Ordering::Release);

        // Try to open the audio path. Failures here (file gone,
        // no AAC track, audio device missing) just leave the
        // video silent — playback is still valid.
        let audio = build_audio_handle(&src, autoplay, loop_playback);

        let shared_for_thread = shared.clone();
        let join = thread::Builder::new()
            .name(format!("video:{}", short_label(&src)))
            .spawn(move || run_decode_loop(src, shared_for_thread, loop_playback))
            .expect("spawn video decode thread");
        Self { shared, audio, join: Some(join) }
    }

    pub fn set_playing(&self, playing: bool) {
        self.shared.playing.store(playing, Ordering::Release);
        // Forward to the audio side so the user-facing
        // play/pause toggles both streams. The audio ring's
        // `paused` flag silences the mixer; the decode loop on
        // the audio thread sees the same flag and parks too.
        if let Some(audio) = self.audio.as_ref() {
            audio.ring.paused.store(!playing, std::sync::atomic::Ordering::Release);
        }
    }

    pub fn set_volume(&self, vol: f32) {
        if let Some(audio) = self.audio.as_ref() {
            if let Ok(mut v) = audio.volume.lock() {
                *v = vol.clamp(0.0, 1.0);
            }
        }
    }

    pub fn set_muted(&self, muted: bool) {
        if let Some(audio) = self.audio.as_ref() {
            audio.ring.muted.store(muted, std::sync::atomic::Ordering::Release);
        }
    }

    /// Request a seek to `target_secs` from the start of the
    /// clip. The decode loop notices the request at the top of
    /// the next sample iteration and restarts at the nearest
    /// preceding sync sample. Audio is not seek-synced in this
    /// pass — if it matters for the demo, follow up with a
    /// "rewind audio decoder + flush ring" hook.
    pub fn seek(&self, target_secs: f64) {
        let target_micros = (target_secs.max(0.0) * 1_000_000.0) as u64;
        if let Ok(mut g) = self.shared.seek_request.lock() {
            *g = Some(target_micros);
        }
    }
}

/// Open the audio subsystem and start an AAC decoder against the
/// same mp4 bytes the video pipeline consumes. Returns `None`
/// when there's no audio to play (no track, codec disabled, or
/// the OS refused to give us an output device).
fn build_audio_handle(
    src: &str,
    autoplay: bool,
    loop_playback: bool,
) -> Option<crate::audio::DecodedAudioHandle> {
    let path = strip_file_scheme(src);
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[audio] open {path}: {e}");
            return None;
        }
    };
    let subsys = match crate::audio::subsystem() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[audio] subsystem unavailable: {e}");
            return None;
        }
    };
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str());
    let handle = match crate::audio::spawn_decoded_source(
        Arc::new(bytes),
        ext,
        subsys,
        1.0,
        loop_playback,
    ) {
        Some(h) => h,
        None => {
            eprintln!("[audio] no decodable audio track in {path}");
            return None;
        }
    };
    // Audio defaults to "playing" semantically — but the ring is
    // paused if the video itself starts paused, so the decoder
    // doesn't burn through the file before the user hits play.
    handle
        .ring
        .paused
        .store(!autoplay, std::sync::atomic::Ordering::Release);
    Some(handle)
}

fn short_label(src: &str) -> &str {
    // Keep thread name under macOS's 64-char limit.
    let trimmed = src.rsplit('/').next().unwrap_or(src);
    if trimmed.len() > 48 { &trimmed[..48] } else { trimmed }
}

/// Main decoder body. Runs on its own thread; the only state it
/// touches outside its locals is `Arc<VideoSharedState>`.
fn run_decode_loop(src: String, shared: Arc<VideoSharedState>, loop_playback: bool) {
    let mut start_micros: u64 = 0;
    loop {
        if shared.shutdown.load(Ordering::Acquire) {
            return;
        }
        match decode_file_once(&src, &shared, start_micros) {
            DecodeResult::Done => {
                if !loop_playback {
                    park_until_shutdown(&shared);
                    return;
                }
                // Fall through to another pass for the loop.
                start_micros = 0;
            }
            DecodeResult::Shutdown => return,
            DecodeResult::Seek(target) => {
                start_micros = target;
            }
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
    /// User requested a seek; outer loop re-enters `decode_file_once`
    /// with the new start position in microseconds.
    Seek(u64),
    Error(String),
}

fn decode_file_once(
    src: &str,
    shared: &Arc<VideoSharedState>,
    start_micros: u64,
) -> DecodeResult {
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
    //
    // Pacing uses the OUTPUT INDEX, not the input sample's
    // composition_timestamp. With B-frame reordering, decode
    // order ≠ display order: openh264 buffers some inputs and
    // outputs frames in display order. If we paced using
    // sample.cts we'd produce bursty publishes (a P-frame whose
    // input cts is far in the future arrives while the decoder
    // is still emitting earlier B-frames, so its sleep target
    // is wrong) — the renderer would then only catch one frame
    // per burst.
    //
    // Output index pacing assumes constant frame rate, which
    // holds for the typical mp4 we'd preview. For variable-rate
    // content, follow-up work would pair each output with its
    // PTS via a min-heap reorder buffer; for now CFR is fine.
    let total_duration_ticks: u64 = track.samples.iter().map(|s| s.duration as u64).sum();
    let total_duration_secs = total_duration_ticks as f64 / timescale;
    let frame_duration_secs = if track.samples.is_empty() {
        1.0 / 30.0
    } else {
        total_duration_secs / track.samples.len() as f64
    };
    // Publish the total duration so the controls UI can render
    // "0:00 / 0:15" right away even before frame 1 lands.
    shared
        .duration_micros
        .store((total_duration_secs * 1_000_000.0) as u64, Ordering::Release);

    let samples: Vec<re_mp4::Sample> = track.samples.clone();

    // For seeks, find the latest sync sample whose composition
    // time is ≤ the requested start. H.264 must decode forward
    // from an IDR; if `start_micros` falls inside a GOP we step
    // back to its IDR and ramp through the intermediate P/B
    // frames until we hit the target, then start publishing.
    let start_sample_idx = find_start_sample(&samples, start_micros, timescale);
    // Once we're past this PTS, start publishing. Frames between
    // start_sample_idx's PTS and `start_micros` are decoded
    // (needed for reference) but not shown.
    let publish_after_micros = start_micros;

    // Reset our wall-clock anchor so pacing aligns with the new
    // start position. Once we begin publishing at `start_micros`,
    // `Instant::now()` = start_wall + Duration(start_micros).
    let start_wall =
        Instant::now() - Duration::from_micros(start_micros);
    let mut pause_accum = Duration::ZERO;
    let mut last_pause_start: Option<Instant> = None;
    let mut output_index: u64 =
        ((start_micros as f64 / 1_000_000.0) / frame_duration_secs) as u64;

    for sample in samples.iter().skip(start_sample_idx) {
        if shared.shutdown.load(Ordering::Acquire) {
            return DecodeResult::Shutdown;
        }

        // Honor a seek request before anything else this iteration.
        if let Some(target) = shared
            .seek_request
            .lock()
            .ok()
            .and_then(|mut g| g.take())
        {
            return DecodeResult::Seek(target);
        }

        // Pause: park here until playback resumes.
        while !shared.playing.load(Ordering::Acquire) {
            if shared.shutdown.load(Ordering::Acquire) {
                return DecodeResult::Shutdown;
            }
            // Allow seek to break us out of pause too.
            if let Some(target) = shared
                .seek_request
                .lock()
                .ok()
                .and_then(|mut g| g.take())
            {
                return DecodeResult::Seek(target);
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
        let frame = yuv_to_rgba(&yuv);

        // Pace to the wall-clock display time of this OUTPUT
        // frame. See top-of-loop comment for why we use output
        // index instead of sample.composition_timestamp.
        let target = start_wall
            + pause_accum
            + Duration::from_secs_f64(output_index as f64 * frame_duration_secs);
        let frame_pts_micros = (output_index as f64 * frame_duration_secs * 1e6) as u64;
        output_index += 1;
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

        // Skip publishing during the seek ramp-up (frames whose
        // PTS is before the requested target). They still had to
        // be decoded so the IDR's reference state is built up
        // for whatever comes next, but the UI shouldn't flash
        // them.
        if frame_pts_micros + 1 < publish_after_micros {
            continue;
        }
        shared
            .current_time_micros
            .store(frame_pts_micros, Ordering::Release);
        publish_frame(&shared, frame);
    }
    // Drain any frames openh264 still holds in its DPB. With
    // `Flush::NoFlush`, B-frame buffering can leave the trailing
    // frames in the decoder when the input ends; without this
    // call, the last ~half-GOP of every clip never reaches the
    // renderer and looping clips appear to restart early.
    let remaining = decoder.flush_remaining().unwrap_or_default();
    for yuv in remaining {
        if shared.shutdown.load(Ordering::Acquire) {
            return DecodeResult::Shutdown;
        }
        let frame = yuv_to_rgba(&yuv);
        let target = start_wall
            + pause_accum
            + Duration::from_secs_f64(output_index as f64 * frame_duration_secs);
        let frame_pts_micros = (output_index as f64 * frame_duration_secs * 1e6) as u64;
        output_index += 1;
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
        if frame_pts_micros + 1 < publish_after_micros {
            continue;
        }
        shared
            .current_time_micros
            .store(frame_pts_micros, Ordering::Release);
        publish_frame(&shared, frame);
    }
    DecodeResult::Done
}

/// Find the latest sync sample whose composition time is ≤ the
/// requested `target_micros`. Falls back to 0 if the target is
/// before the first sample.
fn find_start_sample(samples: &[re_mp4::Sample], target_micros: u64, timescale: f64) -> usize {
    if target_micros == 0 {
        return 0;
    }
    let target_ticks = (target_micros as f64 / 1_000_000.0) * timescale;
    let mut best = 0;
    for (i, s) in samples.iter().enumerate() {
        if !s.is_sync {
            continue;
        }
        if (s.composition_timestamp as f64) > target_ticks {
            break;
        }
        best = i;
    }
    best
}

fn publish_frame(shared: &Arc<VideoSharedState>, frame: VideoFrame) {
    if let Ok(mut slot) = shared.latest_frame.lock() {
        *slot = Some(frame);
    }
    shared.frame_counter.fetch_add(1, Ordering::Release);
    // Wake the event loop. Without this, the renderer's
    // observation of `frame_counter` is coupled to whatever else
    // is driving redraws (animations, scroll, etc.). The proxy
    // hop is `Send + Sync`, safe to call from any thread.
    crate::scheduler::request_redraw();
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
