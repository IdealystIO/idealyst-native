//! `screenshot` for the robot bridge: the scene painter's last frame as PNG.
//!
//! The host window is ONE GDI+ surface (`scene.rs`), double-buffered: the
//! back buffer holds exactly the pixels the last `WM_PAINT` blitted. That
//! buffer is what this captures — the real rendered frame, at device
//! resolution — rather than a re-render, which could differ from what is on
//! screen, or a read of the window DC, which comes back black under Wine's
//! macOS driver (where this backend is exercised day to day).
//!
//! On the Direct2D renderer there is no back buffer; the capture renders the
//! current scene offscreen through the same walk the window frame takes.
//!
//! Native controls (button / edit / checkbox / trackbar / progress) paint in
//! their own child HWNDs and are NOT in the back buffer, so they are absent
//! from the capture; everything the painter draws is present.
//!
//! The encode uses GDI+'s own PNG encoder, so the backend gains no image
//! dependency. GDI+ must not wrap a bitmap that is selected into a DC (the
//! back buffer's always is), so the frame is first copied into a DIB section
//! whose pixels GDI+ wraps directly.

use runtime_shared::Screenshot;
use windows::core::{GUID, PCWSTR};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GdiFlush, SelectObject,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ, SRCCOPY,
};
use windows::Win32::Graphics::GdiPlus::{
    GdipCreateBitmapFromScan0, GdipDisposeImage, GdipSaveImageToFile, GpBitmap, GpImage,
};

use crate::WindowsBackend;

/// Distinguishes concurrent captures' temp files.
static CAPTURE_SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// GDI+'s built-in PNG encoder (`image/png`), a fixed, documented CLSID.
const PNG_ENCODER: GUID = GUID::from_u128(0x557cf406_1a04_11d3_9a73_0000f81ef32e);
/// `PixelFormat32bppRGB` — 32 bpp BGRx, the layout of a 32-bit BI_RGB DIB.
const PIXEL_FORMAT_32BPP_RGB: i32 = 0x0002_2009;

impl WindowsBackend {
    pub(crate) fn supports_screenshot_impl(&self) -> bool {
        true
    }

    pub(crate) fn capture_screenshot_impl(&self, done: Box<dyn FnOnce(Result<Screenshot, String>)>) {
        done(unsafe { self.encode_last_frame() });
    }

    unsafe fn encode_last_frame(&self) -> Result<Screenshot, String> {
        // The Direct2D renderer has no GDI back buffer: render the current
        // scene offscreen at the client size (the same walk the window
        // frame takes — see `d2d.rs`).
        if self.renderer() == crate::RendererKind::D2d {
            let mut rc = windows::Win32::Foundation::RECT::default();
            let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(self.host_hwnd(), &mut rc);
            let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
            if w <= 0 || h <= 0 {
                return Err("window has no client area — no frame to capture".into());
            }
            let mut pixels = self.render_d2d_offscreen(w, h).expect("renderer is D2d")?;
            return encode_png(pixels.as_mut_ptr(), w, h)
                .map(|png| Screenshot { png, width: w as u32, height: h as u32 });
        }
        let (w, h) = self.back.size;
        if self.back.dc.is_invalid() || w <= 0 || h <= 0 {
            return Err("nothing painted yet — no frame to capture".into());
        }

        // Copy the frame into a top-down 32-bit DIB we can hand to GDI+.
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(self.back.dc, &info, DIB_RGB_COLORS, &mut bits, None, 0)
            .map_err(|e| format!("CreateDIBSection: {e}"))?;
        let mem = CreateCompatibleDC(self.back.dc);
        let old = SelectObject(mem, HGDIOBJ(dib.0));
        let copied = BitBlt(mem, 0, 0, w, h, self.back.dc, 0, 0, SRCCOPY).is_ok();
        let _ = GdiFlush();

        let result = if !copied || bits.is_null() {
            Err("copying the back buffer failed".to_string())
        } else {
            encode_png(bits as *mut u8, w, h)
        };

        SelectObject(mem, old);
        let _ = DeleteDC(mem);
        let _ = DeleteObject(HGDIOBJ(dib.0));
        result.map(|png| Screenshot { png, width: w as u32, height: h as u32 })
    }
}

/// Encode a top-down BGRx buffer to PNG bytes through GDI+. GDI+ encodes to
/// a file or an `IStream`; a temp file keeps this free of COM stream
/// plumbing, and is removed before returning.
unsafe fn encode_png(bits: *mut u8, w: i32, h: i32) -> Result<Vec<u8>, String> {
    let mut bitmap: *mut GpBitmap = std::ptr::null_mut();
    let status = GdipCreateBitmapFromScan0(w, h, w * 4, PIXEL_FORMAT_32BPP_RGB, Some(bits), &mut bitmap);
    if status.0 != 0 || bitmap.is_null() {
        return Err(format!("GdipCreateBitmapFromScan0 failed ({})", status.0));
    }
    let path = std::env::temp_dir().join(format!(
        "idealyst-screenshot-{}-{}.png",
        std::process::id(),
        CAPTURE_SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let wide: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect();
    let status = GdipSaveImageToFile(bitmap as *mut GpImage, PCWSTR(wide.as_ptr()), &PNG_ENCODER, std::ptr::null());
    let _ = GdipDisposeImage(bitmap as *mut GpImage);
    if status.0 != 0 {
        let _ = std::fs::remove_file(&path);
        return Err(format!("GDI+ PNG encode failed ({})", status.0));
    }
    let png = std::fs::read(&path).map_err(|e| format!("read encoded PNG: {e}"));
    let _ = std::fs::remove_file(&path);
    png
}
