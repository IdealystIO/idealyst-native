//! Windows camera capture — Media Foundation.
//!
//! - **Device**: `MFEnumDeviceSources` (video capture sources), listed by
//!   [`devices`] with each camera's symbolic link as its id and its friendly
//!   name as its label. A pinned id opens that camera. Otherwise desktop
//!   webcams report no reliable front/back, so `Default` and `Front` open the
//!   first camera and `Back` is [`CameraError::NoCamera`] — the same rule as
//!   desktop Linux. None attached → [`CameraError::NoCamera`].
//! - **Frames**: an `IMFSourceReader` with video processing on, asked for
//!   `RGB32` (BGRX) at the native mode closest to the requested size / fps.
//!   A capture thread reads samples synchronously and writes each frame to the
//!   stream's `FrameWriter` (which is `Send`) — see [`write_frame`] for the
//!   stride, row-order and alpha handling. The thread owns the reader; the
//!   [`StreamHandle`]'s `Drop` stops and joins it.
//! - **Permission**: Windows' camera privacy setting, through the
//!   `permissions` SDK (`AppCapability("webcam")`, which can show Windows'
//!   prompt). A denial that only shows when the device opens
//!   (`E_ACCESSDENIED`) maps to [`CameraError::PermissionDenied`] too.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use media_stream::FrameWriter;
use windows::core::{GUID, PWSTR};
use windows::Win32::Foundation::E_ACCESSDENIED;
use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFAttributes, IMFMediaSource, IMFMediaType, IMFSourceReader, MFCreateAttributes,
    MFCreateMediaType, MFCreateSourceReaderFromMediaSource, MFEnumDeviceSources, MFMediaType_Video, MFStartup,
    MFVideoFormat_RGB32, MFSTARTUP_FULL, MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
    MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
    MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING,
    MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_VERSION,
};
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};

use crate::{CameraConfig, CameraDevice, CameraError, CameraFacing, DeviceFacing, NativeSource};

/// Stops the capture thread on drop.
pub(crate) struct StreamHandle {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub(crate) async fn request_permission() -> Result<(), CameraError> {
    if permissions::request(permissions::Permission::Camera).await.is_usable() {
        Ok(())
    } else {
        Err(CameraError::PermissionDenied)
    }
}

/// Every Media Foundation video capture source. Enumerated on a short-lived
/// thread of its own: Media Foundation needs COM, and initializing it as a
/// multithreaded apartment on the caller's thread would change the apartment
/// of an app's UI thread.
pub(crate) async fn devices() -> Result<Vec<CameraDevice>, CameraError> {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::Builder::new()
        .name("camera-devices".into())
        .spawn(move || {
            let _ = tx.try_send(unsafe { list_devices() });
        })
        .map_err(|e| CameraError::Backend(format!("device list thread: {e}")))?;
    rx.recv()
        .await
        .map_err(|_| CameraError::Backend("the device list thread exited without a result".into()))?
}

unsafe fn list_devices() -> Result<Vec<CameraDevice>, CameraError> {
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    MFStartup(MF_VERSION, MFSTARTUP_FULL).map_err(map)?;
    let mut out = Vec::new();
    for activate in camera_activates()? {
        let id = string_attr(&activate, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK).map_err(map)?;
        let label = string_attr(&activate, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME).unwrap_or_default();
        out.push(CameraDevice {
            id,
            label: if label.is_empty() { "Camera".into() } else { label },
            facing: DeviceFacing::Unreported,
        });
    }
    Ok(out)
}

pub(crate) async fn open(
    config: CameraConfig,
    writer: FrameWriter,
) -> Result<(StreamHandle, Option<NativeSource>), CameraError> {
    request_permission().await?;
    // The reader lives on its own thread (a multithreaded apartment, where
    // Media Foundation's synchronous reads belong); it reports whether the
    // camera opened before frames start.
    let (ready_tx, ready_rx) = async_channel::bounded::<Result<(), CameraError>>(1);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let thread = std::thread::Builder::new()
        .name("camera-capture".into())
        .spawn(move || capture(config, writer, stop_thread, ready_tx))
        .map_err(|e| CameraError::Backend(format!("capture thread: {e}")))?;
    let handle = StreamHandle { stop, thread: Some(thread) };
    match ready_rx.recv().await {
        Ok(Ok(())) => Ok((handle, None)),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(CameraError::Backend("the capture thread exited before opening the camera".into())),
    }
}

fn map(e: windows::core::Error) -> CameraError {
    if e.code() == E_ACCESSDENIED {
        CameraError::PermissionDenied
    } else {
        CameraError::Backend(format!("Media Foundation: {e}"))
    }
}

fn capture(
    config: CameraConfig,
    writer: FrameWriter,
    stop: Arc<AtomicBool>,
    ready: async_channel::Sender<Result<(), CameraError>>,
) {
    let opened = unsafe { open_reader(&config) };
    let (reader, size, stride) = match opened {
        Ok(r) => {
            let _ = ready.try_send(Ok(()));
            r
        }
        Err(e) => {
            let _ = ready.try_send(Err(e));
            return;
        }
    };
    let mut rgba = Vec::new();
    while !stop.load(Ordering::SeqCst) {
        let mut flags = 0u32;
        let mut sample = None;
        let read = unsafe {
            reader.ReadSample(
                MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                0,
                None,
                Some(&mut flags),
                None,
                Some(&mut sample),
            )
        };
        if read.is_err() {
            break;
        }
        let Some(sample) = sample else { continue };
        let Ok(buffer) = (unsafe { sample.ConvertToContiguousBuffer() }) else { continue };
        unsafe {
            let mut data = std::ptr::null_mut();
            let mut len = 0u32;
            if buffer.Lock(&mut data, None, Some(&mut len)).is_ok() {
                let bytes = std::slice::from_raw_parts(data, len as usize);
                write_frame(&writer, bytes, size, stride, &mut rgba);
                let _ = buffer.Unlock();
            }
        }
    }
}

/// Copy one BGRX frame (rows `stride` bytes apart; a negative stride means
/// bottom-up rows, which uncompressed RGB from Media Foundation often is)
/// into tightly-packed, top-down RGBA with opaque alpha (`RGB32`'s fourth
/// byte is undefined), and publish it.
fn write_frame(writer: &FrameWriter, bytes: &[u8], (w, h): (u32, u32), stride: i32, rgba: &mut Vec<u8>) {
    let row = (w * 4) as usize;
    let pitch = stride.unsigned_abs() as usize;
    if w == 0 || h == 0 || pitch < row || bytes.len() < pitch * (h as usize - 1) + row {
        return;
    }
    rgba.clear();
    rgba.reserve(row * h as usize);
    for y in 0..h as usize {
        let src_row = if stride < 0 { h as usize - 1 - y } else { y };
        let line = &bytes[src_row * pitch..src_row * pitch + row];
        for px in line.chunks_exact(4) {
            rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
    }
    writer.write_rgba8(w, h, rgba);
}

/// The chosen camera's reader, delivering RGB32 at the native mode closest to
/// the request: `(reader, (width, height), stride)`.
unsafe fn open_reader(config: &CameraConfig) -> Result<(IMFSourceReader, (u32, u32), i32), CameraError> {
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    MFStartup(MF_VERSION, MFSTARTUP_FULL).map_err(map)?;
    let source = match &config.device {
        Some(id) => camera_with_id(id)?,
        None => first_camera(config.facing)?,
    };
    let mut attrs: Option<IMFAttributes> = None;
    MFCreateAttributes(&mut attrs, 1).map_err(map)?;
    let attrs = attrs.ok_or_else(|| CameraError::Backend("no attributes".into()))?;
    // Lets the reader convert the camera's YUV/MJPEG to RGB32.
    attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1).map_err(map)?;
    let reader = MFCreateSourceReaderFromMediaSource(&source, &attrs).map_err(map)?;
    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

    // The native mode closest to the request (size first, then frame rate).
    let want = (config.width.unwrap_or(1280), config.height.unwrap_or(720), config.fps.unwrap_or(30));
    let mut best: Option<((u32, u32, u32), u64)> = None;
    let mut i = 0;
    while let Ok(t) = reader.GetNativeMediaType(stream, i) {
        i += 1;
        let Ok((w, h)) = pair(&t, &MF_MT_FRAME_SIZE) else { continue };
        let fps = pair(&t, &MF_MT_FRAME_RATE).map(|(n, d)| if d == 0 { 0 } else { n / d }).unwrap_or(0);
        let cost = (w as i64 - want.0 as i64).unsigned_abs() + (h as i64 - want.1 as i64).unsigned_abs()
            + (fps as i64 - want.2 as i64).unsigned_abs();
        if best.is_none_or(|(_, c)| cost < c) {
            best = Some(((w, h, fps), cost));
        }
    }
    let ((w, h, fps), _) = best.ok_or_else(|| CameraError::UnsupportedConfig("the camera reports no video modes".into()))?;

    let out: IMFMediaType = MFCreateMediaType().map_err(map)?;
    out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(map)?;
    out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32).map_err(map)?;
    out.SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64).map_err(map)?;
    if fps > 0 {
        out.SetUINT64(&MF_MT_FRAME_RATE, ((fps as u64) << 32) | 1).map_err(map)?;
    }
    reader
        .SetCurrentMediaType(stream, None, &out)
        .map_err(|e| CameraError::UnsupportedConfig(format!("RGB32 {w}x{h}@{fps}: {e}")))?;
    let current = reader.GetCurrentMediaType(stream).map_err(map)?;
    let (w, h) = pair(&current, &MF_MT_FRAME_SIZE).map_err(map)?;
    let stride = current.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|s| s as i32).unwrap_or((w * 4) as i32);
    Ok((reader, (w, h), stride))
}

/// A packed `UINT64` attribute's high/low halves (frame size, frame rate).
unsafe fn pair(t: &IMFMediaType, key: &GUID) -> windows::core::Result<(u32, u32)> {
    let v = t.GetUINT64(key)?;
    Ok(((v >> 32) as u32, v as u32))
}

/// The first camera, for a facing a webcam satisfies (one reporting no facing
/// counts as user-facing — see `CameraFacing`): `Back` is `NoCamera`.
unsafe fn first_camera(facing: CameraFacing) -> Result<IMFMediaSource, CameraError> {
    if facing == CameraFacing::Back {
        return Err(CameraError::NoCamera);
    }
    let first = camera_activates()?.into_iter().next().ok_or(CameraError::NoCamera)?;
    first.ActivateObject::<IMFMediaSource>().map_err(map)
}

/// The camera whose symbolic link is `id` (an id from [`devices`]).
unsafe fn camera_with_id(id: &str) -> Result<IMFMediaSource, CameraError> {
    let found = camera_activates()?.into_iter().find(|a| {
        string_attr(a, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK).is_ok_and(|link| link == id)
    });
    found.ok_or(CameraError::NoCamera)?.ActivateObject::<IMFMediaSource>().map_err(map)
}

/// A string attribute of a device activation object.
unsafe fn string_attr(activate: &IMFActivate, key: &GUID) -> windows::core::Result<String> {
    let mut value = PWSTR::null();
    let mut len = 0u32;
    activate.GetAllocatedString(key, &mut value, &mut len)?;
    let s = value.to_string().unwrap_or_default();
    CoTaskMemFree(Some(value.0 as *const _));
    Ok(s)
}

/// Every video capture source's activation object, in Media Foundation's order.
unsafe fn camera_activates() -> Result<Vec<IMFActivate>, CameraError> {
    let mut attrs: Option<IMFAttributes> = None;
    MFCreateAttributes(&mut attrs, 1).map_err(map)?;
    let attrs = attrs.ok_or_else(|| CameraError::Backend("no attributes".into()))?;
    attrs.SetGUID(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID).map_err(map)?;
    let mut devices: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    MFEnumDeviceSources(&attrs, &mut devices, &mut count).map_err(map)?;
    // No cameras: the array may be null, which no slice may point at — even
    // an empty one.
    if devices.is_null() || count == 0 {
        if !devices.is_null() {
            CoTaskMemFree(Some(devices as *const _));
        }
        return Ok(Vec::new());
    }
    let list = std::slice::from_raw_parts_mut(devices, count as usize);
    // `take` moves each reference out, so freeing the array releases nothing twice.
    let out = list.iter_mut().filter_map(|d| d.take()).collect();
    CoTaskMemFree(Some(devices as *const _));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use media_stream::MediaStream;

    /// The frame path without a camera: a 2×2 BGRX frame, bottom-up (negative
    /// stride) with padding, becomes top-down opaque RGBA.
    #[test]
    fn bgrx_frames_become_top_down_opaque_rgba() {
        let (stream, writer) = MediaStream::new();
        // Rows stored bottom-up; each row 2 px (8 bytes) + 4 bytes padding.
        let bottom_row = [1, 2, 3, 0, 4, 5, 6, 0, 9, 9, 9, 9];
        let top_row = [10, 20, 30, 0, 40, 50, 60, 0, 9, 9, 9, 9];
        let bytes: Vec<u8> = [bottom_row, top_row].concat();
        let mut scratch = Vec::new();
        write_frame(&writer, &bytes, (2, 2), -12, &mut scratch);
        let mut out = Vec::new();
        assert_eq!(stream.latest(&mut out), Some((2, 2)));
        assert_eq!(out, [30, 20, 10, 255, 60, 50, 40, 255, 3, 2, 1, 255, 6, 5, 4, 255]);
    }

    /// Guards against Windows falling through to the `Unsupported` stub:
    /// `open` must reach Media Foundation. On the test VM, which has no camera, that is `NoCamera`;
    /// on a machine with one, a stream that delivers a frame.
    #[test]
    fn regression_windows_open_reaches_media_foundation() {
        let (stream, writer) = MediaStream::new();
        match pollster::block_on(open(CameraConfig::default(), writer)) {
            Err(CameraError::NoCamera) => {}
            Ok((handle, _)) => {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                let mut buf = Vec::new();
                while stream.latest(&mut buf).is_none() && std::time::Instant::now() < deadline {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                assert!(stream.latest(&mut buf).is_some(), "a frame arrives");
                drop(handle);
            }
            Err(e) => panic!("expected NoCamera or a stream, got {e:?}"),
        }
    }

    /// Listing never errors for lack of a camera (the VM has none: an empty
    /// list), every listed camera opens by id, and an unknown id is `NoCamera`.
    #[test]
    fn listed_cameras_open_by_id_and_unknown_ids_are_no_camera() {
        let devices = pollster::block_on(devices()).expect("listing succeeds");
        for d in &devices {
            assert!(!d.id.is_empty() && !d.label.is_empty(), "{d:?}");
            let (_stream, writer) = MediaStream::new();
            let opened = pollster::block_on(open(CameraConfig::new().with_device(&d.id), writer));
            assert!(opened.is_ok(), "{} must open by id", d.label);
        }
        let (_stream, writer) = MediaStream::new();
        let missing = pollster::block_on(open(CameraConfig::new().with_device("no-such-camera"), writer));
        assert!(matches!(missing, Err(CameraError::NoCamera)));
    }

    /// `Back` on a desktop webcam (which reports no facing) is `NoCamera`,
    /// as on macOS, Linux and web — never a silent open of the front webcam.
    #[test]
    fn regression_back_on_a_desktop_webcam_is_no_camera() {
        let (_stream, writer) = MediaStream::new();
        let back = pollster::block_on(open(CameraConfig::new().back(), writer));
        assert!(matches!(back, Err(CameraError::NoCamera)));
    }
}
