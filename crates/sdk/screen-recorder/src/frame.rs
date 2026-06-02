//! The raw frame this SDK hands to the host. This is the crate's *only*
//! output — there is no file/encoder path here by design.

/// Pixel layout of a [`VideoFrame`]'s `data`. The capture backends each
/// deliver one native layout; we surface it rather than forcing a
/// convert-on-capture cost the host may not want.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// 8-bit BGRA, 4 bytes/pixel. The common Apple / Windows layout
    /// (`CVPixelBuffer` BGRA, DXGI `B8G8R8A8`).
    Bgra8,
    /// 8-bit RGBA, 4 bytes/pixel. The common web / GL layout.
    Rgba8,
    /// Bi-planar Y′ + interleaved CbCr (4:2:0). The common hardware
    /// video-encoder layout; `data` holds the Y plane followed by the
    /// CbCr plane, `bytes_per_row` describes the Y plane.
    Nv12,
}

/// One captured frame, CPU-mapped. Real backends may later add a
/// zero-copy GPU-handle variant for the encode-on-GPU path; for now the
/// uniform contract is mapped bytes so every host can consume frames
/// without per-platform GPU knowledge.
pub struct VideoFrame {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Pixel layout of `data`.
    pub format: PixelFormat,
    /// Bytes per row (row stride) of the first/primary plane. May exceed
    /// `width * bytes_per_pixel` when the backend pads rows for alignment.
    pub bytes_per_row: usize,
    /// The mapped pixel bytes.
    pub data: Vec<u8>,
    /// Capture timestamp in microseconds, on the backend's monotonic
    /// clock. Use deltas between frames; the epoch is not portable.
    pub timestamp_micros: u64,
}
