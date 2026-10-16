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
//! - **Private layer**: `PrivateLayer` content lives in popup windows over
//!   the app's (`backend_windows::create_private_layer_window`). A
//!   `CreateForWindow` capture of the app never contains another top-level
//!   window; for every source the recording also holds
//!   `backend_windows::exclude_from_capture` (`WDA_EXCLUDEFROMCAPTURE` on
//!   the popups) so a monitor or picked-window capture leaves them out too.
//!
//! The `Recording`'s `Drop` closes the session and the pool, which stops
//! frame delivery, and lifts the private layers' exclusion.

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
    EnumWindows, GetWindow, GetWindowLongW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, GWL_EXSTYLE,
    GW_OWNER, WS_EX_TOOLWINDOW,
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
    /// Keeps the private layers out of captures while this records.
    _private_layers: backend_windows::CaptureExclusion,
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
    // Before the first frame: a private layer must never reach one.
    let private_layers = backend_windows::exclude_from_capture();
    session.StartCapture().map_err(platform)?;
    Ok((Recording { session, pool, _private_layers: private_layers }, None))
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
            let hwnd = find_window(is_app_window).ok_or_else(|| {
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
            if let Some(owner) = find_window(is_app_window) {
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

/// One of this process's own app windows: not a window it owns on top of
/// one (a private-layer popup, a tooltip), which sits above its owner in
/// z-order and would otherwise be found first.
fn is_app_window(hwnd: HWND) -> bool {
    let owned = unsafe { GetWindow(hwnd, GW_OWNER) }.is_ok_and(|o| !o.is_invalid());
    let tool = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32 & WS_EX_TOOLWINDOW.0 != 0;
    owned_by_this_process(hwnd) && !owned && !tool
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

    /// Frames from `source` for about a second, after the first arrives.
    fn record(source: Source) -> Vec<(u32, u32, Vec<u8>)> {
        let (stream, writer) = MediaStream::new();
        let mut config = RecordingConfig::new();
        config.source = source;
        let (recording, _) = pollster::block_on(start(config, writer)).expect("capture starts");
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut frames = Vec::new();
        let mut first: Option<Instant> = None;
        while Instant::now() < deadline && first.is_none_or(|t| t.elapsed() < Duration::from_secs(1)) {
            let mut buf = Vec::new();
            if let Some((w, h)) = stream.latest(&mut buf) {
                first.get_or_insert_with(Instant::now);
                frames.push((w, h, buf));
            }
            pump(Duration::from_millis(50));
        }
        drop(recording);
        assert!(!frames.is_empty(), "frames arrive");
        frames
    }

    /// Dispatch this thread's window messages for `d` (the test window
    /// paints its scene on `WM_PAINT`).
    fn pump(d: Duration) {
        use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE};
        let until = Instant::now() + d;
        while Instant::now() < until {
            let mut msg = MSG::default();
            while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
                unsafe {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    runtime_tls::thread_local! {
        /// The backend the test window paints.
        static PAINTS: std::cell::RefCell<Option<std::rc::Rc<std::cell::RefCell<backend_windows::WindowsBackend>>>> =
            const { std::cell::RefCell::new(None) };
    }

    /// A top-level window that paints `PAINTS` like the host shell does.
    fn scene_window() -> HWND {
        use windows::Win32::Foundation::{LRESULT, WPARAM};
        use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT};
        use windows::Win32::UI::WindowsAndMessaging::{DefWindowProcW, GetClientRect, RegisterClassW, WM_PAINT, WNDCLASSW, WS_POPUP};
        unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
            if msg == WM_PAINT {
                let mut ps = PAINTSTRUCT::default();
                let dc = unsafe { BeginPaint(hwnd, &mut ps) };
                let mut rc = Default::default();
                let _ = unsafe { GetClientRect(hwnd, &mut rc) };
                PAINTS.with(|p| {
                    if let Some(b) = &*p.borrow() {
                        b.borrow_mut().paint(dc, rc.right, rc.bottom);
                    }
                });
                let _ = unsafe { EndPaint(hwnd, &ps) };
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
        unsafe {
            let wc = WNDCLASSW { lpfnWndProc: Some(proc), lpszClassName: w!("ScreenRecorderSceneTest"), ..Default::default() };
            RegisterClassW(&wc);
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("ScreenRecorderSceneTest"),
                w!("private-layer-recording-test"),
                WS_POPUP | WS_VISIBLE,
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
        .expect("window")
    }

    /// Pixels of an RGBA frame (the stream swizzles capture's BGRA) within
    /// 8 of `rgb`.
    fn count(frame: &[u8], rgb: [u8; 3]) -> usize {
        let near = |a: u8, b: u8| a.abs_diff(b) <= 8;
        frame.chunks_exact(4).filter(|p| near(p[0], rgb[0]) && near(p[1], rgb[1]) && near(p[2], rgb[2])).count()
    }

    /// `PrivateLayer` content is a popup owned by the app's window, above it
    /// in z-order. `ThisApp` took the process's first visible window — the
    /// popup — and recorded the private layer alone; and a full-screen
    /// recording would show it. Neither recording contains it now, while
    /// both show the app's own content.
    #[test]
    fn regression_recordings_leave_the_private_layer_out() {
        use backend_windows::WindowsBackend;
        use runtime_scene::Host;
        use runtime_shared::accessibility::AccessibilityProps;
        use runtime_shared::{Color, Length, StyleRules};
        use runtime_vocabulary::caps::{LifecycleOps, StyleOps, ViewOps};
        use std::cell::RefCell;
        use std::rc::Rc;
        use windows::Win32::Graphics::Dwm::DwmFlush;

        // A blue app window with a private layer holding a red box.
        let host = scene_window();
        let cell = Rc::new(RefCell::new(WindowsBackend::new(host)));
        cell.borrow_mut().set_self_ref(Rc::downgrade(&cell));
        PAINTS.with(|p| *p.borrow_mut() = Some(cell.clone()));
        {
            let mut b = cell.borrow_mut();
            let a11y = AccessibilityProps::default();
            let mut root = ViewOps::create_view(&mut *b, &a11y);
            let mut rs = StyleRules::default();
            rs.background = Some(Color::from("#0000ff").into());
            b.apply_style(&root, &Rc::new(rs));
            let mut layer = b.create_private_layer_window();
            let red = ViewOps::create_view(&mut *b, &a11y);
            let mut s = StyleRules::default();
            s.width = Some(Length::Px(120.0).into());
            s.height = Some(Length::Px(80.0).into());
            s.background = Some(Color::from("#ff0000").into());
            b.apply_style(&red, &Rc::new(s));
            Host::insert(&mut *b, &mut layer, red);
            Host::insert(&mut *b, &mut root, layer);
            LifecycleOps::finish(&mut *b, root);
        }
        // `finish` invalidated the window: its WM_PAINT draws the scene and
        // shows the layer's popup.
        pump(Duration::from_millis(200));
        let popups = backend_windows::private_layer_windows();
        assert_eq!(popups.len(), 1, "the layer has its popup");
        let popup = HWND(popups[0] as *mut std::ffi::c_void);
        assert!(unsafe { IsWindowVisible(popup) }.as_bool(), "and it is on screen");
        assert_eq!(find_window(is_app_window), Some(host), "the app's window, not the popup above it");
        unsafe {
            let _ = DwmFlush();
        }

        for (label, source) in [("ThisApp", Source::ThisApp), ("FullScreen", Source::FullScreen)] {
            for (w, h, frame) in record(source) {
                assert!(count(&frame, [0, 0, 255]) > 1000, "{label}: the app's blue is recorded ({w}x{h})");
                assert_eq!(count(&frame, [255, 0, 0]), 0, "{label}: the private layer's red is not ({w}x{h})");
            }
        }
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
