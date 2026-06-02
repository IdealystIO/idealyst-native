//! The frame of pixels handed to the callback.

/// Pixel layout of a [`VideoFrame`]. There's one variant today —
/// every backend normalizes to it — but it's named (not assumed) so a
/// future zero-copy planar path can be added without breaking the field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PixelFormat {
    /// 8 bits per channel, byte order `R G B A`, straight (non-premultiplied)
    /// alpha. Alpha is `255` for opaque camera frames.
    Rgba8,
}

/// One captured frame, borrowed for the duration of the callback call.
/// Copy out what you need before returning — `data` points at a
/// backend-owned buffer that's reused for the next frame.
///
/// Pixels are **tightly-packed `RGBA8`** (`data.len() == width * height *
/// 4`), in **top-down** row order (row 0 is the top of the image). Every
/// backend converts its native layout (iOS/macOS `BGRA`, Android
/// `YUV_420_888`, the web canvas' `RGBA`) into this one shape, repacking
/// any row padding, so consumer code is identical everywhere — the
/// backends diverge in mechanism, not in what you receive. Upload it to a
/// GPU texture, draw it to a canvas, or run it through your own pipeline.
pub struct VideoFrame<'a> {
    /// Tightly-packed `RGBA8` pixels for this frame, top-down.
    pub data: &'a [u8],
    /// Frame width in pixels. Authoritative — reflects what the device
    /// produced, not what was requested.
    pub width: u32,
    /// Frame height in pixels. Authoritative.
    pub height: u32,
    /// The pixel layout of `data`. Always [`PixelFormat::Rgba8`] today.
    pub format: PixelFormat,
}

impl VideoFrame<'_> {
    /// Number of pixels in this frame (`width * height`).
    pub fn pixel_count(&self) -> usize {
        self.width as usize * self.height as usize
    }

    /// Expected length of `data` in bytes for the frame's dimensions and
    /// format (`width * height * 4`). A correctly-formed frame satisfies
    /// `data.len() == byte_len()`.
    pub fn byte_len(&self) -> usize {
        self.pixel_count() * 4
    }
}
