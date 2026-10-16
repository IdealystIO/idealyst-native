//! Windows capture backend — `Windows.Graphics.Capture`.
//!
//! - **Source → capture item**: `ThisApp` is this process's main window and
//!   `Window(title_hint)` the first visible top-level window whose title
//!   contains the hint, both through `IGraphicsCaptureItemInterop::
//!   CreateForWindow`; `FullScreen` is the primary monitor
//!   (`CreateForMonitor`); `UserChoice` is the system capture picker
//!   (`GraphicsCapturePicker`, parented to the app's window), and
//!   cancelling it is [`RecorderError::PermissionDenied`].
//! - **Frames**: a free-threaded `Direct3D11CaptureFramePool`; its
//!   `FrameArrived` runs on a thread-pool thread, copies the frame's
//!   texture to a CPU-readable staging texture and publishes it as BGRA on
//!   the stream's (`Send`) `FrameWriter`, paced to `config.fps`. When the
//!   source changes size (a window resized) the pool is recreated at the new
//!   size.
//! - **Permission**: none to grant for a desktop app — Windows marks the
//!   captured window or screen with a border instead. Where the OS has no
//!   capture support, [`RecorderError::Unsupported`].
//! - **Audio**: not carried (like the Linux portal backend) — a requested
//!   audio source records video only, and says so.
//!
//! The `Recording`'s `Drop` closes the session and the pool, which stops
//! frame delivery.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use media_stream::FrameWriter;
use windows::core::{Interface, BOOL};
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCapturePicker, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Foundation::TypedEventHandler;
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, POINT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTOPRIMARY};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::Shell::IInitializeWithWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
};

use crate::{NativeSource, RecorderError, RecordingConfig, Source};

fn platform(e: windows::core::Error) -> RecorderError {
    RecorderError::Platform(format!("{e}"))
}

pub(crate) async fn request_permission(_source: &Source) -> Result<(), RecorderError> {
    if GraphicsCaptureSession::IsSupported().unwrap_or(false) {
        Ok(())
    } else {
        Err(RecorderError::Unsupported)
    }
}

/// A running capture; dropping it stops frame delivery.
pub(crate) struct Recording {
    session: GraphicsCaptureSession,
    pool: Arc<Mutex<Direct3D11CaptureFramePool>>,
}

impl Drop for Recording {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.lock().unwrap_or_else(|p| p.into_inner()).Close();
    }
}

pub(crate) async fn start(
    config: RecordingConfig,
    writer: FrameWriter,
) -> Result<(Recording, Option<NativeSource>), RecorderError> {
    request_permission(&config.source).await?;
    if !matches!(config.audio, crate::AudioSource::None) {
        eprintln!(
            "screen-recorder(windows): audio capture is not wired on Windows.Graphics.Capture; \
             capturing video only."
        );
    }
    let item = capture_item(&config.source).await?;
    let (d3d, device) = unsafe { create_device() }.map_err(platform)?;
    let size = item.Size().map_err(platform)?;
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&device, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, size)
        .map_err(platform)?;
    let session = pool.CreateCaptureSession(&item).map_err(platform)?;
    let pool = Arc::new(Mutex::new(pool));

    let interval = Duration::from_secs_f64(1.0 / config.fps.max(1) as f64);
    let state = Mutex::new(Copier { d3d, device, staging: None, size, last: None, bgra: Vec::new() });
    let pool_for_handler = pool.clone();
    let handler = TypedEventHandler::<Direct3D11CaptureFramePool, windows::core::IInspectable>::new(move |_, _| {
        let pool = pool_for_handler.lock().unwrap_or_else(|p| p.into_inner());
        let Ok(frame) = pool.TryGetNextFrame() else { return Ok(()) };
        let mut copier = state.lock().unwrap_or_else(|p| p.into_inner());
        let content = frame.ContentSize()?;
        if content.Width != copier.size.Width || content.Height != copier.size.Height {
            // The source changed size: deliver at the new size from now on.
            copier.size = content;
            copier.staging = None;
            pool.Recreate(&copier.device, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, content)?;
            return Ok(());
        }
        let now = Instant::now();
        if copier.last.is_some_and(|t| now.duration_since(t) < interval) {
            return Ok(());
        }
        copier.last = Some(now);
        let texture: ID3D11Texture2D = unsafe { frame.Surface()?.cast::<IDirect3DDxgiInterfaceAccess>()?.GetInterface()? };
        unsafe { copier.publish(&texture, &writer) };
        Ok(())
    });
    pool.lock().unwrap_or_else(|p| p.into_inner()).FrameArrived(&handler).map_err(platform)?;
    // Optional on older builds; a capture without the cursor is the norm for
    // recordings of app content.
    let _ = session.SetIsCursorCaptureEnabled(true);
    session.StartCapture().map_err(platform)?;
    Ok((Recording { session, pool }, None))
}

/// Copies captured textures to the CPU. Lives behind a mutex: the pool's
/// handler runs on thread-pool threads, and the immediate context is
/// single-threaded.
struct Copier {
    d3d: ID3D11Device,
    device: IDirect3DDevice,
    staging: Option<ID3D11Texture2D>,
    size: SizeInt32,
    last: Option<Instant>,
    bgra: Vec<u8>,
}

// SAFETY: the frame pool's handler runs on thread-pool threads. A D3D11
// device is free-threaded and the WinRT `IDirect3DDevice` wrapper is agile;
// the one thread-affine part, the immediate context, is only touched with the
// `Copier` mutex held, so no two threads use it at once.
unsafe impl Send for Copier {}

impl Copier {
    unsafe fn publish(&mut self, texture: &ID3D11Texture2D, writer: &FrameWriter) {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        let (w, h) = (desc.Width.min(self.size.Width as u32), desc.Height.min(self.size.Height as u32));
        if w == 0 || h == 0 {
            return;
        }
        if self.staging.is_none() {
            let mut s = desc;
            s.Usage = D3D11_USAGE_STAGING;
            s.BindFlags = 0;
            s.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            s.MiscFlags = 0;
            let mut t = None;
            if self.d3d.CreateTexture2D(&s, None, Some(&mut t)).is_err() {
                return;
            }
            self.staging = t;
        }
        let Some(staging) = &self.staging else { return };
        let Ok(ctx): windows::core::Result<ID3D11DeviceContext> = self.d3d.GetImmediateContext() else { return };
        ctx.CopyResource(staging, texture);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        if ctx.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).is_err() {
            return;
        }
        let row = (w * 4) as usize;
        let pitch = mapped.RowPitch as usize;
        let data = std::slice::from_raw_parts(mapped.pData as *const u8, pitch * (h as usize - 1) + row);
        self.bgra.clear();
        for y in 0..h as usize {
            self.bgra.extend_from_slice(&data[y * pitch..y * pitch + row]);
        }
        ctx.Unmap(staging, 0);
        writer.write_bgra8(w, h, &self.bgra);
    }
}

async fn capture_item(source: &Source) -> Result<GraphicsCaptureItem, RecorderError> {
    let interop = || windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(platform);
    match source {
        Source::ThisApp => {
            let hwnd = find_window(|hwnd| owned_by_this_process(hwnd)).ok_or_else(|| {
                RecorderError::Platform("this app has no visible window to record".into())
            })?;
            unsafe { interop()?.CreateForWindow(hwnd) }.map_err(platform)
        }
        Source::Window(selector) => {
            let hint = selector.title_hint.clone().unwrap_or_default().to_lowercase();
            let hwnd = find_window(|hwnd| !owned_by_this_process(hwnd) && title(hwnd).to_lowercase().contains(&hint))
                .ok_or_else(|| RecorderError::Platform(format!("no visible window titled like {hint:?}")))?;
            unsafe { interop()?.CreateForWindow(hwnd) }.map_err(platform)
        }
        Source::FullScreen => {
            let monitor = unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
            unsafe { interop()?.CreateForMonitor(monitor) }.map_err(platform)
        }
        Source::UserChoice => {
            let picker = GraphicsCapturePicker::new().map_err(platform)?;
            if let Some(owner) = find_window(|hwnd| owned_by_this_process(hwnd)) {
                unsafe { picker.cast::<IInitializeWithWindow>().map_err(platform)?.Initialize(owner) }.map_err(platform)?;
            }
            let picked = picker.PickSingleItemAsync().map_err(platform)?.await;
            // Cancelling the picker completes with no item.
            picked.map_err(|_| RecorderError::PermissionDenied)
        }
    }
}

fn owned_by_this_process(hwnd: HWND) -> bool {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid == unsafe { GetCurrentProcessId() }
}

fn title(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// The first visible top-level window `accept` takes.
fn find_window(accept: impl Fn(HWND) -> bool) -> Option<HWND> {
    struct Search<'a> {
        accept: &'a dyn Fn(HWND) -> bool,
        found: Option<HWND>,
    }
    unsafe extern "system" fn visit(hwnd: HWND, data: LPARAM) -> BOOL {
        let search = unsafe { &mut *(data.0 as *mut Search) };
        if unsafe { IsWindowVisible(hwnd) }.as_bool() && (search.accept)(hwnd) {
            search.found = Some(hwnd);
            return BOOL(0);
        }
        BOOL(1)
    }
    let mut search = Search { accept: &accept, found: None };
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize));
    }
    search.found
}

/// A Direct3D device and its WinRT wrapper (what the frame pool takes).
unsafe fn create_device() -> windows::core::Result<(ID3D11Device, IDirect3DDevice)> {
    let mut d3d = None;
    D3D11CreateDevice(
        None,
        D3D_DRIVER_TYPE_HARDWARE,
        HMODULE::default(),
        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        None,
        D3D11_SDK_VERSION,
        Some(&mut d3d),
        None,
        None,
    )?;
    let d3d: ID3D11Device = d3d.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))?;
    let inspectable = CreateDirect3D11DeviceFromDXGIDevice(&d3d.cast::<IDXGIDevice>()?)?;
    Ok((d3d, inspectable.cast()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use media_stream::MediaStream;
    use windows::core::w;
    use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, WINDOW_EX_STYLE, WS_OVERLAPPEDWINDOW, WS_VISIBLE};

    /// Windows was a skeleton returning `Unsupported`. Now this process's
    /// window is captured with Windows.Graphics.Capture and frames arrive at
    /// its size.
    #[test]
    fn regression_windows_records_this_apps_window() {
        let _hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("screen-recorder-test"),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                100,
                100,
                320,
                240,
                None,
                None,
                None,
                None,
            )
        }
        .expect("window");
        let (stream, writer) = MediaStream::new();
        let (recording, _) = pollster::block_on(start(RecordingConfig::new(), writer)).expect("capture starts");
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut buf = Vec::new();
        let mut got = None;
        while got.is_none() && Instant::now() < deadline {
            got = stream.latest(&mut buf);
            std::thread::sleep(Duration::from_millis(50));
        }
        let (w, h) = got.expect("a captured frame arrives");
        assert!(w > 0 && h > 0 && buf.len() == (w * h * 4) as usize, "{w}x{h}, {} bytes", buf.len());
        drop(recording);
    }

    #[test]
    fn window_titles_read_back() {
        let hwnd = unsafe {
            CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("Title probe"), WS_VISIBLE, 0, 0, 10, 10, None, None, None, None)
        }
        .expect("window");
        assert_eq!(title(hwnd), "Title probe");
        assert!(owned_by_this_process(hwnd));
    }
}
