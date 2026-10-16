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
//! **No PCM tap**: the Media Engine plays the clip's audio but exposes no
//! samples, so `DecodedVideo::audio()` is `None` on Windows (the clip is
//! still heard). A recording that needs the clip's sound has no source for it
//! here yet.
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
use windows::core::{implement, Interface, Result as WinResult, BSTR, HRESULT};
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
    IMFMediaEngineNotify_Impl, MFCreateAttributes, MFCreateDXGIDeviceManager, MFStartup, MFVideoNormalizedRect,
    CLSID_MFMediaEngineClassFactory, MFSTARTUP_FULL, MF_MEDIA_ENGINE_CALLBACK, MF_MEDIA_ENGINE_DXGI_MANAGER,
    MF_MEDIA_ENGINE_EVENT_ERROR, MF_MEDIA_ENGINE_EVENT_LOADEDMETADATA, MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT,
    MF_VERSION, MFARGB,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};

use crate::{DecodeConfig, DecodeSource, Opened, TransportControl, VideoDecodeError};

/// How often the decode thread asks the engine for a new frame.
const TICK: Duration = Duration::from_millis(8);

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
    engine: IMFMediaEngine,
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
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
    _audio: AudioWriter,
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
    let control: Rc<dyn TransportControl> = Rc::new(Control { engine: engine.clone() });
    Ok(Opened {
        handle: Box::new(Handle { stop, thread, engine }),
        control,
        has_audio: false,
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

    #[test]
    fn max_dimension_bounds_the_longer_side_and_never_upscales() {
        assert_eq!(target_size((1920, 1080), Some(512)), (512, 288));
        assert_eq!(target_size((640, 360), Some(1024)), (640, 360));
        assert_eq!(target_size((853, 480), None), (852, 480));
    }
}
