//! Windows recording — Media Foundation's Sink Writer (H.264 + AAC in MP4).
//!
//! # Flow
//!
//! The backend subscribes to the input streams; their callbacks (on the
//! producers' threads) copy each video frame / audio chunk into a channel.
//! A writer thread owns the `IMFSinkWriter`:
//!
//! 1. It waits until every input's format is known — the first video frame
//!    gives the size, the first audio chunk the rate and channel count —
//!    buffering what arrives meanwhile, then creates the writer. (Starting
//!    on the first video frame alone, before the audio format was known, is
//!    the bug the macOS backend once had.)
//! 2. Video goes in as top-down `RGB32` (the RGBA frames swizzled to BGRX)
//!    and out as H.264; the Sink Writer inserts the colour converter.
//!    Audio goes in as 16-bit PCM (the AAC encoder's input; the f32 chunks
//!    are converted here) and out as AAC.
//! 3. Each sample is timed from its `pts_micros` on the shared capture clock,
//!    relative to the first sample written, so audio and video from
//!    different producers stay in sync.
//! 4. `stop` finalizes the file and reports the result; dropping the
//!    recording without `stop` discards it.
//!
//! # Container
//!
//! Media Foundation writes MP4 (it has no WebM sink), so
//! [`Container::WebM`](crate::Container::WebM) is recorded as MP4 with the
//! file's extension changed to match — the Linux backend's rule for a
//! container it cannot honour. H.264 wants even frame sizes; an odd size
//! loses its last column / row.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use media_stream::{AudioSubscription, Subscription};
use windows::core::HSTRING;
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFMediaType, IMFSinkWriter, MFAudioFormat_AAC, MFAudioFormat_PCM, MFCreateAttributes,
    MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL, MFMediaType_Audio,
    MFMediaType_Video, MFStartup, MFVideoFormat_H264, MFVideoFormat_RGB32, MFVideoInterlace_Progressive,
    MFSTARTUP_FULL, MF_MT_AUDIO_AVG_BYTES_PER_SECOND, MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_BLOCK_ALIGNMENT,
    MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE,
    MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_PIXEL_ASPECT_RATIO,
    MF_MT_SUBTYPE, MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, MF_VERSION,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

use crate::{Container, MediaInputs, MediaWriterError, RecordConfig};

/// What the producers send the writer thread.
enum Msg {
    Video { w: u32, h: u32, rgba: Vec<u8>, pts: u64 },
    Audio { rate: u32, channels: u16, samples: Vec<f32>, pts: u64 },
    /// Finalize and report (to an async channel: `stop` awaits it without
    /// blocking the caller's thread while the encoder drains).
    Stop(async_channel::Sender<Result<(), MediaWriterError>>),
}

pub(crate) struct RecordingHandle {
    tx: Option<Sender<Msg>>,
    thread: Option<JoinHandle<()>>,
    // Held so the inputs keep delivering. Their callbacks hold senders too,
    // so they are dropped before the channel can close.
    video: Option<Subscription>,
    audio: Option<AudioSubscription>,
    path: PathBuf,
}

impl RecordingHandle {
    pub(crate) async fn stop(mut self) -> Result<(), MediaWriterError> {
        // No more input: unsubscribe first, then ask for the finalize.
        self.video.take();
        self.audio.take();
        let (tx, rx) = async_channel::bounded(1);
        if let Some(sender) = self.tx.take() {
            let _ = sender.send(Msg::Stop(tx));
        }
        let result = rx
            .recv()
            .await
            .unwrap_or_else(|_| Err(MediaWriterError::Backend("the writer thread exited".into())));
        // The thread has replied and returns now; nothing left to wait on.
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        result
    }
}

impl Drop for RecordingHandle {
    fn drop(&mut self) {
        // Dropped without `stop`: abandon the file. Closing every sender (the
        // subscriptions' and ours) ends the thread without finalizing.
        if let Some(t) = self.thread.take() {
            self.video.take();
            self.audio.take();
            self.tx.take();
            let _ = t.join();
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

pub(crate) async fn start(
    inputs: MediaInputs<'_>,
    config: &RecordConfig,
) -> Result<(RecordingHandle, String), MediaWriterError> {
    // MP4 is what Media Foundation writes; say so in the file name.
    let rel = match config.container {
        Container::Mp4 => config.path.clone(),
        _ => with_extension(&config.path, "mp4"),
    };
    let path = config.store.local_path(&rel).ok_or(MediaWriterError::NoLocalPath)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| MediaWriterError::Backend(format!("{}: {e}", dir.display())))?;
    }
    let (tx, rx) = mpsc::channel::<Msg>();
    let video = inputs.video.map(|s| {
        let tx = tx.clone();
        s.subscribe(move |f: &media_stream::VideoFrame| {
            let _ = tx.send(Msg::Video { w: f.width, h: f.height, rgba: f.data.to_vec(), pts: f.pts_micros });
        })
    });
    let audio = inputs.audio.map(|s| {
        let tx = tx.clone();
        s.subscribe(move |a: &media_stream::AudioFrame| {
            let _ = tx.send(Msg::Audio {
                rate: a.sample_rate,
                channels: a.channels,
                samples: a.samples.to_vec(),
                pts: a.pts_micros,
            });
        })
    });
    let plan = Plan {
        path: path.clone(),
        want_video: video.is_some(),
        want_audio: audio.is_some(),
        fps: config.fps.max(1),
        video_bitrate: config.video_bitrate,
        audio_bitrate: config.audio_bitrate,
    };
    let thread = std::thread::Builder::new()
        .name("media-writer".into())
        .spawn(move || run(plan, rx))
        .map_err(|e| MediaWriterError::Backend(format!("writer thread: {e}")))?;
    Ok((RecordingHandle { tx: Some(tx), thread: Some(thread), video, audio, path }, rel))
}

fn with_extension(rel: &str, ext: &str) -> String {
    let p = Path::new(rel).with_extension(ext);
    p.to_string_lossy().replace('\\', "/")
}

struct Plan {
    path: PathBuf,
    want_video: bool,
    want_audio: bool,
    fps: u32,
    video_bitrate: Option<u32>,
    audio_bitrate: Option<u32>,
}

/// The open writer and its streams.
struct Writer {
    sink: IMFSinkWriter,
    video: Option<(u32, (u32, u32))>,
    audio: Option<(u32, u16)>,
    /// The first written sample's capture time — time zero in the file.
    origin: Option<u64>,
    frame_duration: i64,
}

fn encode(e: windows::core::Error) -> MediaWriterError {
    MediaWriterError::Backend(format!("Media Foundation: {e}"))
}

fn run(plan: Plan, rx: Receiver<Msg>) {
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let mut pending: Vec<Msg> = Vec::new();
    let mut video_fmt: Option<(u32, u32)> = None;
    let mut audio_fmt: Option<(u32, u16)> = None;
    let mut writer: Option<Writer> = None;
    let mut failure: Option<MediaWriterError> = None;
    loop {
        let msg = match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(m) => m,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            // Dropped without stop: abandon.
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        match msg {
            Msg::Stop(reply) => {
                let result = match (failure.take(), writer.take()) {
                    (Some(e), _) => Err(e),
                    (None, Some(w)) => unsafe { w.sink.Finalize() }.map_err(encode),
                    (None, None) => Err(MediaWriterError::Backend("no media arrived before stop".into())),
                };
                let _ = reply.try_send(result);
                return;
            }
            m => {
                if failure.is_some() {
                    continue;
                }
                match &m {
                    Msg::Video { w, h, .. } if video_fmt.is_none() => video_fmt = Some((*w & !1, *h & !1)),
                    Msg::Audio { rate, channels, .. } if audio_fmt.is_none() => audio_fmt = Some((*rate, *channels)),
                    _ => {}
                }
                if writer.is_none() {
                    pending.push(m);
                    let ready = (!plan.want_video || video_fmt.is_some()) && (!plan.want_audio || audio_fmt.is_some());
                    if !ready {
                        continue;
                    }
                    match unsafe { open(&plan, video_fmt, audio_fmt) } {
                        Ok(w) => writer = Some(w),
                        Err(e) => {
                            failure = Some(e);
                            continue;
                        }
                    }
                    for m in pending.drain(..) {
                        if let Err(e) = unsafe { write(writer.as_mut().expect("opened"), m) } {
                            failure = Some(e);
                            break;
                        }
                    }
                } else if let Err(e) = unsafe { write(writer.as_mut().expect("open"), m) } {
                    failure = Some(e);
                }
            }
        }
    }
}

unsafe fn open(plan: &Plan, video: Option<(u32, u32)>, audio: Option<(u32, u16)>) -> Result<Writer, MediaWriterError> {
    MFStartup(MF_VERSION, MFSTARTUP_FULL).map_err(encode)?;
    let mut attrs: Option<IMFAttributes> = None;
    MFCreateAttributes(&mut attrs, 1).map_err(encode)?;
    let attrs = attrs.ok_or_else(|| MediaWriterError::Backend("no attributes".into()))?;
    attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1).map_err(encode)?;
    let sink = MFCreateSinkWriterFromURL(&HSTRING::from(plan.path.as_os_str()), None, &attrs).map_err(encode)?;

    let mut video_stream = None;
    if let Some((w, h)) = video.filter(|_| plan.want_video) {
        if w == 0 || h == 0 {
            return Err(MediaWriterError::Backend(format!("frame size {w}x{h} too small for H.264")));
        }
        let size = ((w as u64) << 32) | h as u64;
        let rate = ((plan.fps as u64) << 32) | 1;
        let out: IMFMediaType = MFCreateMediaType().map_err(encode)?;
        out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(encode)?;
        out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264).map_err(encode)?;
        // About 0.1 bit per pixel per frame unless told: ~2.8 Mb/s at 720p30.
        let bitrate = plan.video_bitrate.unwrap_or(((w as u64 * h as u64 * plan.fps as u64) / 10).max(500_000) as u32);
        out.SetUINT32(&MF_MT_AVG_BITRATE, bitrate).map_err(encode)?;
        out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).map_err(encode)?;
        out.SetUINT64(&MF_MT_FRAME_SIZE, size).map_err(encode)?;
        out.SetUINT64(&MF_MT_FRAME_RATE, rate).map_err(encode)?;
        out.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1).map_err(encode)?;
        let index = sink.AddStream(&out).map_err(encode)?;
        let input: IMFMediaType = MFCreateMediaType().map_err(encode)?;
        input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(encode)?;
        input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32).map_err(encode)?;
        input.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).map_err(encode)?;
        input.SetUINT64(&MF_MT_FRAME_SIZE, size).map_err(encode)?;
        input.SetUINT64(&MF_MT_FRAME_RATE, rate).map_err(encode)?;
        input.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1).map_err(encode)?;
        // A positive stride: rows top-down (RGB32's default is bottom-up).
        input.SetUINT32(&MF_MT_DEFAULT_STRIDE, w * 4).map_err(encode)?;
        sink.SetInputMediaType(index, &input, None).map_err(encode)?;
        video_stream = Some((index, (w, h)));
    }

    let mut audio_stream = None;
    if let Some((rate, channels)) = audio.filter(|_| plan.want_audio) {
        // The AAC encoder takes 44.1 / 48 kHz, mono or stereo.
        if !matches!(rate, 44_100 | 48_000) || !matches!(channels, 1 | 2) {
            return Err(MediaWriterError::Backend(format!(
                "AAC needs 44.1 or 48 kHz mono/stereo audio; got {rate} Hz × {channels}"
            )));
        }
        let out: IMFMediaType = MFCreateMediaType().map_err(encode)?;
        out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).map_err(encode)?;
        out.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC).map_err(encode)?;
        out.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16).map_err(encode)?;
        out.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate).map_err(encode)?;
        out.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels as u32).map_err(encode)?;
        // The encoder accepts 12000 / 16000 / 20000 / 24000 bytes/s.
        let bytes_per_sec = match plan.audio_bitrate.unwrap_or(128_000) / 8 {
            b if b <= 12_000 => 12_000,
            b if b <= 16_000 => 16_000,
            b if b <= 20_000 => 20_000,
            _ => 24_000,
        };
        out.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, bytes_per_sec).map_err(encode)?;
        let index = sink.AddStream(&out).map_err(encode)?;
        let input: IMFMediaType = MFCreateMediaType().map_err(encode)?;
        input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).map_err(encode)?;
        input.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM).map_err(encode)?;
        input.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16).map_err(encode)?;
        input.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate).map_err(encode)?;
        input.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels as u32).map_err(encode)?;
        input.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 2 * channels as u32).map_err(encode)?;
        input.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, rate * 2 * channels as u32).map_err(encode)?;
        sink.SetInputMediaType(index, &input, None).map_err(encode)?;
        audio_stream = Some((index, channels));
    }
    sink.BeginWriting().map_err(encode)?;
    Ok(Writer {
        sink,
        video: video_stream,
        audio: audio_stream,
        origin: None,
        frame_duration: 10_000_000 / plan.fps as i64,
    })
}

unsafe fn write(w: &mut Writer, msg: Msg) -> Result<(), MediaWriterError> {
    match msg {
        Msg::Video { w: fw, h: fh, rgba, pts } => {
            let Some((index, (vw, vh))) = w.video else { return Ok(()) };
            if fw < vw || fh < vh || rgba.len() < (fw * fh * 4) as usize {
                return Ok(()); // a frame smaller than the stream: skip it
            }
            let bytes = bgrx(&rgba, fw, (vw, vh));
            let time = timestamp(&mut w.origin, pts);
            put(&w.sink, index, &bytes, time, w.frame_duration)
        }
        Msg::Audio { samples, pts, channels, rate } => {
            let Some((index, ch)) = w.audio else { return Ok(()) };
            if channels != ch {
                return Ok(());
            }
            let pcm = pcm16(&samples);
            let frames = samples.len() as i64 / ch.max(1) as i64;
            let time = timestamp(&mut w.origin, pts);
            put(&w.sink, index, &pcm, time, frames * 10_000_000 / rate.max(1) as i64)
        }
        Msg::Stop(_) => Ok(()),
    }
}

/// 100 ns units since the first sample written; earlier samples clamp to 0.
fn timestamp(origin: &mut Option<u64>, pts_micros: u64) -> i64 {
    let o = *origin.get_or_insert(pts_micros);
    pts_micros.saturating_sub(o) as i64 * 10
}

/// Tightly packed RGBA (`stride_px` wide) → BGRX cropped to `(w, h)`.
fn bgrx(rgba: &[u8], stride_px: u32, (w, h): (u32, u32)) -> Vec<u8> {
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h as usize {
        let row = &rgba[y * stride_px as usize * 4..][..w as usize * 4];
        for px in row.chunks_exact(4) {
            out.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
    }
    out
}

/// Normalized f32 → little-endian 16-bit PCM.
fn pcm16(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

unsafe fn put(sink: &IMFSinkWriter, stream: u32, bytes: &[u8], time: i64, duration: i64) -> Result<(), MediaWriterError> {
    let buffer = MFCreateMemoryBuffer(bytes.len() as u32).map_err(encode)?;
    let mut data = std::ptr::null_mut();
    buffer.Lock(&mut data, None, None).map_err(encode)?;
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), data, bytes.len());
    buffer.Unlock().map_err(encode)?;
    buffer.SetCurrentLength(bytes.len() as u32).map_err(encode)?;
    let sample = MFCreateSample().map_err(encode)?;
    sample.AddBuffer(&buffer).map_err(encode)?;
    sample.SetSampleTime(time).map_err(encode)?;
    sample.SetSampleDuration(duration).map_err(encode)?;
    sink.WriteSample(stream, &sample).map_err(encode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_crop_to_even_and_swizzle() {
        // 3×1 RGBA → 2×1 BGRX (odd width loses its last column).
        let rgba = [1, 2, 3, 4, 5, 6, 7, 8, 9, 9, 9, 9];
        assert_eq!(bgrx(&rgba, 3, (2, 1)), [3, 2, 1, 255, 7, 6, 5, 255]);
    }

    #[test]
    fn pcm_converts_and_clamps() {
        assert_eq!(pcm16(&[0.0, 1.0, -2.0]), [0, 0, 0xff, 0x7f, 0x01, 0x80]);
    }

    #[test]
    fn timestamps_count_from_the_first_sample() {
        let mut o = None;
        assert_eq!(timestamp(&mut o, 1_000_000), 0);
        assert_eq!(timestamp(&mut o, 1_033_333), 333_330);
        assert_eq!(timestamp(&mut o, 900_000), 0, "earlier than the origin clamps");
    }

    #[test]
    fn webm_requests_are_written_as_mp4() {
        assert_eq!(with_extension("clips/a.webm", "mp4"), "clips/a.mp4");
    }
}
