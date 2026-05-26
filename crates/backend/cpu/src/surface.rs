//! Pixel output abstraction for the CPU backend.
//!
//! The backend writes pre-blended opaque RGBA8 samples (one per
//! pixel) into a `Surface`. Concrete surfaces decide what to do with
//! the bytes — keep them in a `Vec<u32>`, push them to an SPI display
//! line-by-line, copy them into an OS window, or hand them to
//! `embedded-graphics`.
//!
//! ## Why bytes in, bytes out (no Color enum)
//!
//! ESP32-class targets often render in **RGB565** (16 bpp) to halve
//! the framebuffer's RAM cost and to match the wire format of common
//! displays (ST7789, ILI9341). Doing the RGB888→RGB565 quantization
//! inside the surface — instead of the backend — means the backend's
//! rasterizer can keep a single uniform internal representation and
//! the surface controls per-pixel cost.
//!
//! ## Memory-conscious by default
//!
//! `put_pixel` is the only required method; everything else
//! (`fill_rect`, `present`, `flush_rect`) has a default implementation
//! in terms of it. Surfaces backed by SPI displays will usually want
//! to override `fill_rect` (one command for a whole rectangle) and
//! `flush_rect` (push a dirty region to the panel in one DMA hop).

/// A pixel destination the CPU backend rasterizes into.
///
/// Coordinates are in framebuffer pixels with `(0, 0)` at the top-left.
/// All pixel values handed to the surface are **opaque** sRGB
/// `[r, g, b, a]` — the backend has already alpha-blended against the
/// underlying scene, so a surface may ignore `a` (treat as 255).
///
/// Surfaces are *not* required to maintain a full back-buffer. A
/// line-buffer scheme that holds a single row, paints it, and pushes
/// it to a display can implement `Surface` by accumulating writes
/// into its row buffer and flushing on Y-change in `put_pixel`. The
/// CPU backend's render walker emits pixels in roughly top-to-bottom
/// raster order which makes that scheme cheap.
pub trait Surface {
    /// Width of the surface in pixels.
    fn width(&self) -> u32;

    /// Height of the surface in pixels.
    fn height(&self) -> u32;

    /// Write a single pre-blended opaque pixel. Out-of-bounds writes
    /// are silently dropped — implementers don't need to bounds-check.
    /// The backend always clips before calling.
    fn put_pixel(&mut self, x: u32, y: u32, rgba: [u8; 4]);

    /// Fill an axis-aligned rectangle with a solid color. The
    /// rectangle may extend outside the surface — implementations
    /// must clip. Default implementation walks `put_pixel`; surfaces
    /// backed by SPI displays that support a `SET_WINDOW + FILL`
    /// pair should override this for a large speedup.
    fn fill_rect(&mut self, x: i32, y: i32, w: u32, h: u32, rgba: [u8; 4]) {
        let (sw, sh) = (self.width() as i32, self.height() as i32);
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + w as i32).min(sw);
        let y1 = (y + h as i32).min(sh);
        for py in y0..y1 {
            for px in x0..x1 {
                self.put_pixel(px as u32, py as u32, rgba);
            }
        }
    }

    /// Called by the backend once per frame after the last pixel has
    /// been written. Implementations targeting deferred-flush
    /// hardware (SPI displays, double-buffered windows) should
    /// trigger their flush here. Default is a no-op.
    fn present(&mut self) {}

    /// Optional: hint that the given rect is the only region the
    /// backend touched this frame. Surfaces that DMA partial
    /// updates to a panel can use this for damage-aware flushes.
    /// Default is a no-op — full-frame flush via `present` is fine.
    #[allow(unused_variables)]
    fn flush_rect(&mut self, x: u32, y: u32, w: u32, h: u32) {}
}

// ---------------------------------------------------------------------------
// Memory-backed framebuffer
// ---------------------------------------------------------------------------

/// A heap-allocated RGBA8 framebuffer. The simplest possible
/// `Surface` implementation: one `Vec<u8>` of length `4 * w * h` in
/// row-major order, channels in `[R, G, B, A]`.
///
/// Used in headless tests, by the desktop preview host (which copies
/// `pixels()` into a window-system surface), and as the reference
/// implementation against which other surfaces are checked.
///
/// Not appropriate for ESP32 directly — at 320×240×4 = 300 KB it
/// blows past the chip's SRAM budget. Use a line-buffered or
/// RGB565-quantizing `Surface` impl in that environment.
pub struct MemSurface {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl MemSurface {
    /// Allocate a fresh framebuffer of the given size, fully cleared
    /// to opaque black `(0, 0, 0, 255)`.
    pub fn new(width: u32, height: u32) -> Self {
        let len = (width as usize) * (height as usize) * 4;
        let mut pixels = vec![0u8; len];
        // Initialize alpha to opaque so dumps and visual inspection
        // don't show a "transparent" frame when no view has painted
        // a particular pixel.
        for i in (3..len).step_by(4) {
            pixels[i] = 255;
        }
        Self { width, height, pixels }
    }

    /// Borrow the raw `[R, G, B, A, R, G, B, A, …]` pixel bytes.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Borrow the raw pixel bytes mutably. Provided for hosts that
    /// want to memcpy into a window-system buffer without going
    /// through a getter — e.g. `softbuffer`'s `&mut [u32]` cast.
    pub fn pixels_mut(&mut self) -> &mut [u8] {
        &mut self.pixels
    }

    /// Read back a single pixel. Used by tests; not on the render
    /// hot path. Returns opaque black for out-of-bounds reads.
    pub fn get_pixel(&self, x: u32, y: u32) -> [u8; 4] {
        if x >= self.width || y >= self.height {
            return [0, 0, 0, 255];
        }
        let i = ((y * self.width + x) * 4) as usize;
        [self.pixels[i], self.pixels[i + 1], self.pixels[i + 2], self.pixels[i + 3]]
    }

    /// Erase the framebuffer to `rgba`. Called by the CPU backend
    /// at the start of each frame; exposed publicly so tests and
    /// hosts can pre-clear to a known color before rendering.
    pub fn clear(&mut self, rgba: [u8; 4]) {
        for px in self.pixels.chunks_exact_mut(4) {
            px[0] = rgba[0];
            px[1] = rgba[1];
            px[2] = rgba[2];
            px[3] = rgba[3];
        }
    }
}

impl Surface for MemSurface {
    fn width(&self) -> u32 { self.width }
    fn height(&self) -> u32 { self.height }

    fn put_pixel(&mut self, x: u32, y: u32, rgba: [u8; 4]) {
        if x >= self.width || y >= self.height {
            return;
        }
        let i = ((y * self.width + x) * 4) as usize;
        self.pixels[i] = rgba[0];
        self.pixels[i + 1] = rgba[1];
        self.pixels[i + 2] = rgba[2];
        self.pixels[i + 3] = rgba[3];
    }

    fn fill_rect(&mut self, x: i32, y: i32, w: u32, h: u32, rgba: [u8; 4]) {
        // Override the default per-pixel fill — the contiguous-row
        // memset is ~10× faster on typical L1 cache lines. Matters
        // for backgrounds that cover the full viewport.
        let sw = self.width as i32;
        let sh = self.height as i32;
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + w as i32).min(sw);
        let y1 = (y + h as i32).min(sh);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let row_bytes = ((x1 - x0) * 4) as usize;
        for py in y0..y1 {
            let row_start = ((py * sw + x0) * 4) as usize;
            let row = &mut self.pixels[row_start..row_start + row_bytes];
            for chunk in row.chunks_exact_mut(4) {
                chunk.copy_from_slice(&rgba);
            }
        }
    }
}
