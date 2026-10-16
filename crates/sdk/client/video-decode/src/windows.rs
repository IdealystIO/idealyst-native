//! Windows clip decode — Media Foundation's Media Engine.
//!
//! The engine plays the clip (its clock, its audio output, the transport:
//! play / pause / seek / rate / mute) in frame-server mode; a decode thread
//! pulls each new frame (`OnVideoStreamTick`) with `TransferVideoFrame` into
//! a GPU texture at the clip's size (bounded by `max_dimension`), copies it
//! to a CPU-readable staging texture and publishes it as BGRA on the clip's
//! `MediaStream` (`FrameWriter::write_bgra8`). Decode runs on the GPU where
//! the adapter has video decode, in software otherwise.
//!
//! `open` waits for the engine's `LOADEDMETADATA` (the natural size is known
//! then) or `ERROR` (→ [`VideoDecodeError::Backend`]); those events arrive on
//! Media Foundation's threads and reach the awaiting task over a channel.
//!
//! **PCM tap**: the Media Engine plays the clip's audio but exposes no
//! samples, so a second decoder — a Media Foundation source reader over the
//! same file, audio stream only, decoding to interleaved `f32` — feeds
//! `DecodedVideo::audio()`. Its thread follows the engine's clock: each tick
//! it delivers the samples between what it last delivered and the engine's
//! current position, so the stream carries what is being played, in real
//! time, as the AVPlayer tap and the web `MediaElementSource` tap do. Paused
//! or ended, nothing is delivered; a seek or a loop (the position jumping by
//! more than [`RESYNC`]) repositions the reader. The player's mute doesn't
//! touch the tap. At a playback rate other than 1 the samples are the clip's
//! own, unstretched (about `rate` seconds of audio per second).
//!
//! Why a second decoder rather than one pipeline: the Media Engine owns
//! decode, clock, audio output and transport, and has no sample hook;
//! replacing it with a source reader for both streams would mean writing the
//! clock, the audio output and A/V sync ourselves. Two decoders of one file
//! cost a second audio decode — small next to video — and the tap stays
//! within a tick of the engine's position.
//!
//! The engine is free-threaded, so the UI thread's transport calls and the
//! decode thread's frame pulls share it; the Direct3D immediate context is
//! used only by the decode thread.

use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use media_stream::{AudioWriter, FrameWriter};
use windows::core::{implement, Interface, Result as WinResult, BSTR, GUID, HRESULT, HSTRING};
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0};
use windows::Win32::System::Variant::VT_I8;
use windows::Win32::Foundation::{E_FAIL, HMODULE, RECT};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_BIND_RENDER_TARGET,
    D3D11_BIND_SHADER_RESOURCE, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_FLAG,
    D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFDXGIDeviceManager, IMFMediaEngine, IMFMediaEngineClassFactory, IMFMediaEngineNotify,
    IMFMediaEngineNotify_Impl, IMFSample, IMFSourceReader, MFAudioFormat_Float, MFCreateAttributes,
    MFCreateDXGIDeviceManager, MFCreateMediaType, MFCreateSourceReaderFromURL, MFMediaType_Audio, MFStartup,
    MFVideoNormalizedRect, MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE,
    MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READER_ALL_STREAMS, MF_SOURCE_READER_FIRST_AUDIO_STREAM,
    CLSID_MFMediaEngineClassFactory, MFSTARTUP_FULL, MF_MEDIA_ENGINE_CALLBACK, MF_MEDIA_ENGINE_DXGI_MANAGER,
    MF_MEDIA_ENGINE_EVENT_ERROR, MF_MEDIA_ENGINE_EVENT_LOADEDMETADATA, MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT,
    MF_VERSION, MFARGB,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};

use crate::{DecodeConfig, DecodeSource, Opened, TransportControl, VideoDecodeError};

/// How often the decode thread asks the engine for a new frame.
const TICK: Duration = Duration::from_millis(8);

/// How often the audio thread delivers the samples played since its last
/// delivery — the tap's chunk length.
const AUDIO_TICK: Duration = Duration::from_millis(10);

/// A gap between the tap's position and the engine's larger than this is a
/// seek or a loop, not tick jitter: the reader jumps to the engine.
const RESYNC: f64 = 0.25;

/// Media Foundation time unit: 100 ns.
const HNS_PER_SEC: f64 = 10_000_000.0;

type Verdict = std::result::Result<(), HRESULT>;

#[implement(IMFMediaEngineNotify)]
struct Notify {
    tx: Mutex<Option<async_channel::Sender<Verdict>>>,
}

impl IMFMediaEngineNotify_Impl for Notify_Impl {
    fn EventNotify(&self, event: u32, _param1: usize, param2: u32) -> WinResult<()> {
        let verdict = if event == MF_MEDIA_ENGINE_EVENT_LOADEDMETADATA.0 as u32 {
            Ok(())
        } else if event == MF_MEDIA_ENGINE_EVENT_ERROR.0 as u32 {
            Err(HRESULT(param2 as i32))
        } else {
            return Ok(());
        };
        if let Some(tx) = self.tx.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = tx.try_send(verdict);
        }
        Ok(())
    }
}

fn backend(e: windows::core::Error) -> VideoDecodeError {
    VideoDecodeError::Backend(format!("Media Foundation: {e}"))
}

/// Stops the decode thread and shuts the engine down on drop.
struct Handle {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    audio_thread: Option<JoinHandle<()>>,
    engine: IMFMediaEngine,
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        for t in [self.thread.take(), self.audio_thread.take()].into_iter().flatten() {
            let _ = t.join();
        }
        let _ = unsafe { self.engine.Shutdown() };
    }
}

/// `file://` URLs (what a `Bytes` source becomes) as plain paths — the form
/// Media Foundation opens for local files; anything else unchanged.
fn location(url: &str) -> String {
    match url.strip_prefix("file:///").or_else(|| url.strip_prefix("file://")) {
        Some(path) => path.replace('/', "\\"),
        None => url.to_string(),
    }
}

/// The decoded size: the natural size, scaled so the longer side is at most
/// `max` (never up), each side at least 1 and even (video surfaces want even
/// dimensions).
fn target_size(natural: (u32, u32), max: Option<u32>) -> (u32, u32) {
    let (w, h) = natural;
    let scale = match max {
        Some(m) if w.max(h) > m && m > 0 => m as f64 / w.max(h) as f64,
        _ => 1.0,
    };
    let even = |v: f64| ((v.round() as u32).max(2) / 2) * 2;
    (even(w as f64 * scale), even(h as f64 * scale))
}

pub(crate) async fn open(
    source: DecodeSource,
    config: DecodeConfig,
    frames: FrameWriter,
    audio: AudioWriter,
) -> Result<Opened, VideoDecodeError> {
    let DecodeSource::Url(url) = source else {
        return Err(VideoDecodeError::Backend("an in-memory clip reaches the backend as a file URL".into()));
    };
    let (device, manager) = unsafe { create_device() }.map_err(backend)?;
    let (tx, rx) = async_channel::bounded(1);
    let engine = unsafe { create_engine(&manager, &location(&url), &config, tx) }.map_err(backend)?;
    match rx.recv().await {
        Ok(Ok(())) => {}
        Ok(Err(hr)) => {
            let _ = unsafe { engine.Shutdown() };
            return Err(VideoDecodeError::Backend(format!("Media Foundation could not open the clip ({:#010x})", hr.0)));
        }
        Err(_) => return Err(VideoDecodeError::Backend("the engine went away before it opened the clip".into())),
    }
    let natural = unsafe {
        let (mut w, mut h) = (0u32, 0u32);
        let _ = engine.GetNativeVideoSize(Some(&mut w), Some(&mut h));
        (w > 0 && h > 0).then_some((w, h))
    };

    let stop = Arc::new(AtomicBool::new(false));
    let thread = match natural {
        Some(n) => {
            let size = target_size(n, config.max_dimension);
            let (stop_t, engine_t) = (stop.clone(), AgileEngine(engine.clone()));
            Some(
                std::thread::Builder::new()
                    .name("video-decode".into())
                    .spawn(move || pump(engine_t, device, size, frames, stop_t))
                    .map_err(|e| VideoDecodeError::Backend(format!("decode thread: {e}")))?,
            )
        }
        // An audio-only clip: nothing to pull.
        None => None,
    };
    // The tap's reader opens on its own thread; it says whether the clip has
    // an audio track before it starts delivering.
    let (has_tx, has_rx) = async_channel::bounded(1);
    let (stop_a, engine_a, path) = (stop.clone(), AgileEngine(engine.clone()), location(&url));
    let audio_thread = std::thread::Builder::new()
        .name("video-decode-audio".into())
        .spawn(move || tap(engine_a, &path, audio, has_tx, stop_a))
        .map_err(|e| VideoDecodeError::Backend(format!("audio thread: {e}")))?;
    let has_audio = has_rx.recv().await.unwrap_or(false);
    let control: Rc<dyn TransportControl> = Rc::new(Control { engine: engine.clone() });
    Ok(Opened {
        handle: Box::new(Handle { stop, thread, audio_thread: Some(audio_thread), engine }),
        control,
        has_audio,
        natural_size: natural,
    })
}

/// The engine, sent to the decode thread. The Media Engine is free-threaded
/// (its methods may be called from any thread).
struct AgileEngine(IMFMediaEngine);
unsafe impl Send for AgileEngine {}

/// The decode thread: pull each new frame and publish it as BGRA.
fn pump(engine: AgileEngine, device: ID3D11Device, size: (u32, u32), frames: FrameWriter, stop: Arc<AtomicBool>) {
    let engine = engine.0;
    let made = unsafe { textures(&device, size) };
    let Ok((target, staging, ctx)) = made else { return };
    let mut last_pts = -1i64;
    let mut bgra = Vec::new();
    while !stop.load(Ordering::SeqCst) {
        std::thread::sleep(TICK);
        let pts = match unsafe { engine.OnVideoStreamTick() } {
            Ok(pts) if pts != last_pts => pts,
            _ => continue,
        };
        unsafe {
            let src = MFVideoNormalizedRect { left: 0.0, top: 0.0, right: 1.0, bottom: 1.0 };
            let dst = RECT { left: 0, top: 0, right: size.0 as i32, bottom: size.1 as i32 };
            let border = MFARGB { rgbBlue: 0, rgbGreen: 0, rgbRed: 0, rgbAlpha: 255 };
            if engine.TransferVideoFrame(&target, Some(&src), &dst, Some(&border)).is_err() {
                continue;
            }
            last_pts = pts;
            ctx.CopyResource(&staging, &target);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            if ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).is_ok() {
                let row = (size.0 * 4) as usize;
                let pitch = mapped.RowPitch as usize;
                let data = std::slice::from_raw_parts(mapped.pData as *const u8, pitch * (size.1 as usize - 1) + row);
                bgra.clear();
                for y in 0..size.1 as usize {
                    bgra.extend_from_slice(&data[y * pitch..y * pitch + row]);
                }
                ctx.Unmap(&staging, 0);
                frames.write_bgra8(size.0, size.1, &bgra);
            }
        }
    }
}

/// The clip's audio as interleaved `f32`, decoded by a source reader.
struct AudioReader {
    reader: IMFSourceReader,
    sample_rate: u32,
    channels: u16,
}

impl AudioReader {
    /// `None` when the clip has no audio track (or it can't be decoded).
    unsafe fn open(path: &str) -> Option<AudioReader> {
        let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path), None).ok()?;
        let all = MF_SOURCE_READER_ALL_STREAMS.0 as u32;
        let audio = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
        let _ = reader.SetStreamSelection(all, false);
        reader.SetStreamSelection(audio, true).ok()?;
        let wanted = MFCreateMediaType().ok()?;
        wanted.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).ok()?;
        wanted.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float).ok()?;
        reader.SetCurrentMediaType(audio, None, &wanted).ok()?;
        let got = reader.GetCurrentMediaType(audio).ok()?;
        let sample_rate = got.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).ok()?;
        let channels = got.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).ok()? as u16;
        (sample_rate > 0 && channels > 0).then_some(AudioReader { reader, sample_rate, channels })
    }

    /// The next decoded chunk: `(start in seconds, interleaved samples)`;
    /// `None` at the end of the stream.
    unsafe fn read(&self) -> Option<(f64, Vec<f32>)> {
        loop {
            let mut flags = 0u32;
            let mut ts = 0i64;
            let mut sample: Option<IMFSample> = None;
            self.reader
                .ReadSample(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut sample))
                .ok()?;
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                return None;
            }
            // A gap or format notice carries no sample; read on.
            let Some(sample) = sample else { continue };
            let buffer = sample.ConvertToContiguousBuffer().ok()?;
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut len = 0u32;
            buffer.Lock(&mut data, None, Some(&mut len)).ok()?;
            let floats = std::slice::from_raw_parts(data as *const f32, len as usize / 4).to_vec();
            let _ = buffer.Unlock();
            return Some((ts as f64 / HNS_PER_SEC, floats));
        }
    }

    unsafe fn seek(&self, seconds: f64) {
        // A VT_I8 of 100 ns units; GUID_NULL = that time format.
        let at = PROPVARIANT {
            Anonymous: PROPVARIANT_0 {
                Anonymous: std::mem::ManuallyDrop::new(PROPVARIANT_0_0 {
                    vt: VT_I8,
                    Anonymous: PROPVARIANT_0_0_0 { hVal: (seconds.max(0.0) * HNS_PER_SEC) as i64 },
                    ..Default::default()
                }),
            },
        };
        let _ = self.reader.SetCurrentPosition(&GUID::zeroed(), &at);
    }
}

/// What the tap has decoded but not yet delivered: interleaved samples
/// starting at `start` seconds of the clip.
struct Pending {
    start: f64,
    samples: std::collections::VecDeque<f32>,
}

impl Pending {
    fn clear(&mut self, at: f64) {
        self.start = at;
        self.samples.clear();
    }

    /// Append a chunk decoded at `at`, dropping whatever falls before what
    /// is pending (a seek lands on the packet before the target).
    fn push(&mut self, at: f64, chunk: &[f32], rate: u32, channels: u16) {
        let end = self.start + (self.samples.len() / channels as usize) as f64 / rate as f64;
        let skip_frames = ((end - at) * rate as f64).round().max(0.0) as usize;
        let skip = (skip_frames * channels as usize).min(chunk.len());
        self.samples.extend(&chunk[skip..]);
    }

    /// Take the frames up to `until` seconds (fewer if not decoded yet).
    fn take_until(&mut self, until: f64, rate: u32, channels: u16) -> Vec<f32> {
        // The epsilon keeps a position that is a whole frame count in
        // decimal (0.05 s × 100 Hz) from flooring a frame short in binary.
        let frames = ((until - self.start) * rate as f64 + 1e-6).floor().max(0.0) as usize;
        let n = (frames * channels as usize).min(self.samples.len());
        let n = n - n % channels as usize;
        self.start += (n / channels as usize) as f64 / rate as f64;
        self.samples.drain(..n).collect()
    }

    fn available_until(&self, rate: u32, channels: u16) -> f64 {
        self.start + (self.samples.len() / channels as usize) as f64 / rate as f64
    }
}

/// The audio thread: say whether there is audio, then deliver what the
/// engine plays as it plays it (see the module docs).
fn tap(engine: AgileEngine, path: &str, out: AudioWriter, has: async_channel::Sender<bool>, stop: Arc<AtomicBool>) {
    let engine = engine.0;
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let Some(reader) = (unsafe { AudioReader::open(path) }) else {
        let _ = has.try_send(false);
        return;
    };
    let _ = has.try_send(true);
    let (rate, channels) = (reader.sample_rate, reader.channels);
    let mut pending = Pending { start: 0.0, samples: Default::default() };
    let mut ended = false;
    while !stop.load(Ordering::SeqCst) {
        std::thread::sleep(AUDIO_TICK);
        let (position, idle) = unsafe {
            let idle = engine.IsPaused().as_bool() || engine.IsEnded().as_bool() || engine.IsSeeking().as_bool();
            (engine.GetCurrentTime(), idle)
        };
        // (Past the end of an audio track shorter than the video there is
        // nothing to jump to: only a jump back restarts the reader.)
        let jumped = (position - pending.start).abs() > RESYNC;
        if jumped && !(ended && position > pending.start) {
            unsafe { reader.seek(position) };
            pending.clear(position);
            ended = false;
        }
        if idle {
            continue;
        }
        while !ended && pending.available_until(rate, channels) < position {
            match unsafe { reader.read() } {
                Some((at, chunk)) => pending.push(at, &chunk, rate, channels),
                None => ended = true,
            }
        }
        let chunk = pending.take_until(position, rate, channels);
        if !chunk.is_empty() {
            out.write_pcm_f32(rate, channels, &chunk);
        }
    }
}

/// The transfer target (a render target the engine draws into) and its
/// CPU-readable copy, plus the device's immediate context.
unsafe fn textures(device: &ID3D11Device, (w, h): (u32, u32)) -> WinResult<(ID3D11Texture2D, ID3D11Texture2D, ID3D11DeviceContext)> {
    let mut desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut target = None;
    device.CreateTexture2D(&desc, None, Some(&mut target))?;
    desc.Usage = D3D11_USAGE_STAGING;
    desc.BindFlags = 0;
    desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
    let mut staging = None;
    device.CreateTexture2D(&desc, None, Some(&mut staging))?;
    let ctx = device.GetImmediateContext()?;
    let fail = || windows::core::Error::from_hresult(E_FAIL);
    Ok((target.ok_or_else(fail)?, staging.ok_or_else(fail)?, ctx))
}

/// A Direct3D device (video decode where the adapter has it; WARP last) in
/// Media Foundation's DXGI device manager.
unsafe fn create_device() -> WinResult<(ID3D11Device, IMFDXGIDeviceManager)> {
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
    let attempts: [(D3D_DRIVER_TYPE, D3D11_CREATE_DEVICE_FLAG); 3] = [
        (D3D_DRIVER_TYPE_HARDWARE, D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT),
        (D3D_DRIVER_TYPE_HARDWARE, D3D11_CREATE_DEVICE_BGRA_SUPPORT),
        (D3D_DRIVER_TYPE_WARP, D3D11_CREATE_DEVICE_BGRA_SUPPORT),
    ];
    let mut device = None;
    for (driver, flags) in attempts {
        let mut d: Option<ID3D11Device> = None;
        if D3D11CreateDevice(None, driver, HMODULE::default(), flags, None, D3D11_SDK_VERSION, Some(&mut d), None, None).is_ok() {
            device = d;
            break;
        }
    }
    let device = device.ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?;
    if let Ok(mt) = device.cast::<windows::Win32::Graphics::Direct3D10::ID3D10Multithread>() {
        let _ = mt.SetMultithreadProtected(true);
    }
    let mut token = 0u32;
    let mut manager = None;
    MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
    let manager = manager.ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?;
    manager.ResetDevice(&device, token)?;
    Ok((device, manager))
}

unsafe fn create_engine(
    manager: &IMFDXGIDeviceManager,
    location: &str,
    config: &DecodeConfig,
    ready: async_channel::Sender<Verdict>,
) -> WinResult<IMFMediaEngine> {
    let mut attrs: Option<IMFAttributes> = None;
    MFCreateAttributes(&mut attrs, 3)?;
    let attrs = attrs.ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?;
    let notify: IMFMediaEngineNotify = Notify { tx: Mutex::new(Some(ready)) }.into();
    attrs.SetUnknown(&MF_MEDIA_ENGINE_DXGI_MANAGER, manager)?;
    attrs.SetUnknown(&MF_MEDIA_ENGINE_CALLBACK, &notify)?;
    attrs.SetUINT32(&MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM.0 as u32)?;
    let factory: IMFMediaEngineClassFactory = CoCreateInstance(&CLSID_MFMediaEngineClassFactory, None, CLSCTX_INPROC_SERVER)?;
    let engine = factory.CreateInstance(0, &attrs)?;
    engine.SetMuted(config.muted)?;
    engine.SetLoop(config.loop_playback)?;
    engine.SetAutoPlay(config.autoplay)?;
    engine.SetSource(&BSTR::from(location))?;
    Ok(engine)
}

struct Control {
    engine: IMFMediaEngine,
}

impl TransportControl for Control {
    fn play(&self) {
        let _ = unsafe { self.engine.Play() };
    }
    fn pause(&self) {
        let _ = unsafe { self.engine.Pause() };
    }
    fn seek(&self, seconds: f32) {
        let _ = unsafe { self.engine.SetCurrentTime(seconds.max(0.0) as f64) };
    }
    fn set_muted(&self, muted: bool) {
        let _ = unsafe { self.engine.SetMuted(muted) };
    }
    fn set_rate(&self, rate: f32) {
        unsafe {
            if rate <= 0.0 {
                let _ = self.engine.Pause();
            } else {
                let _ = self.engine.SetPlaybackRate(rate as f64);
            }
        }
    }
    fn position(&self) -> f32 {
        unsafe { self.engine.GetCurrentTime() as f32 }
    }
    fn duration(&self) -> f32 {
        let d = unsafe { self.engine.GetDuration() };
        if d.is_finite() { d as f32 } else { 0.0 }
    }
    fn is_playing(&self) -> bool {
        unsafe { !self.engine.IsPaused().as_bool() && !self.engine.IsEnded().as_bool() }
    }
    fn is_muted(&self) -> bool {
        unsafe { self.engine.GetMuted().as_bool() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_urls_become_paths() {
        assert_eq!(location(r"file://C:\Temp\clip.mp4"), r"C:\Temp\clip.mp4");
        assert_eq!(location("file:///C:/Temp/clip.mp4"), r"C:\Temp\clip.mp4");
        assert_eq!(location("https://x/clip.mp4"), "https://x/clip.mp4");
    }

    fn pending() -> Pending {
        Pending { start: 0.0, samples: Default::default() }
    }

    /// Chunks append in order; delivery takes exactly the frames up to the
    /// engine's position and moves the start with them.
    #[test]
    fn the_tap_delivers_up_to_the_engines_position() {
        let mut p = pending();
        p.push(0.0, &[1.0; 200], 100, 2); // 1 s of stereo at 100 Hz
        assert_eq!(p.available_until(100, 2), 1.0);
        assert_eq!(p.take_until(0.25, 100, 2).len(), 50, "25 frames × 2 channels");
        assert!((p.start - 0.25).abs() < 1e-9);
        assert!(p.take_until(0.25, 100, 2).is_empty(), "nothing new at the same position");
        assert_eq!(p.take_until(5.0, 100, 2).len(), 150, "never more than is decoded");
    }

    /// After a seek the reader lands on the packet before the target: the
    /// part before the target is dropped, not played late.
    #[test]
    fn a_chunk_starting_before_the_seek_target_is_trimmed() {
        let mut p = pending();
        p.clear(2.0);
        let chunk: Vec<f32> = (0..100).map(|i| i as f32).collect(); // 1.9 s .. 2.9 s, mono 100 Hz
        p.push(1.9, &chunk, 100, 1);
        let got = p.take_until(2.05, 100, 1);
        assert_eq!(got, vec![10.0, 11.0, 12.0, 13.0, 14.0], "starts at 2.0 s");
        let mut p = pending();
        p.clear(2.0);
        p.push(0.5, &[0.0; 50], 100, 1);
        assert!(p.samples.is_empty(), "a chunk wholly before the target is dropped");
    }

    #[test]
    fn max_dimension_bounds_the_longer_side_and_never_upscales() {
        assert_eq!(target_size((1920, 1080), Some(512)), (512, 288));
        assert_eq!(target_size((640, 360), Some(1024)), (640, 360));
        assert_eq!(target_size((853, 480), None), (852, 480));
    }
}
