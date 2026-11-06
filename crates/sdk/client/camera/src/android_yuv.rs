//! Android: `YUV_420_888` → tightly-packed top-down `RGBA8`, rotated.
//!
//! The pure half of the Android frame path. `android.rs` views the three plane
//! buffers of an open `android.media.Image` as slices for the length of one JNI
//! call and hands them here. It lives apart from the JNI so it runs on any host
//! under `cargo test`.
//!
//! **The arithmetic** is integer BT.601 with full-range-style coefficients,
//! 1436 / 352 / 731 / 1814 over `>> 10`, on `y` as is and `u`, `v` centred on
//! zero. `>>` floors, and `g` shifts the sum of its two chroma products, so
//! neither may be replaced by a shift of each term.
//!
//! **Layouts.** `YUV_420_888` promises 4:2:0 and nothing about memory: planar
//! (chroma pixel stride 1) and semi-planar NV12/NV21 (chroma pixel stride 2, the
//! U and V buffers overlapping one byte apart) are the common layouts, and any
//! other stride still has to work. Nothing here looks at where the planes sit
//! relative to each other. Interleaved chroma is two independent stride-2 reads.
//!
//! **Bounds.** [`YuvFrame::new`] refuses a frame whose planes cannot hold every
//! sample, up front, so the conversion's slicing cannot panic: a panic in the
//! JNI trampoline aborts the process. A plane is only as long as its last
//! sample, not `rows * row_stride`, because the last row of a real plane
//! commonly stops short of a full stride.
//!
//! **Speed.** Two loops with one output, because what is fast optimized is slow
//! unoptimized. [`YuvFrame::convert`] picks by `debug_assertions`, the nearest a
//! crate can get at compile time to asking for the optimization level. 1080p on
//! the host:
//!
//! | | `opt-level = 3` | `opt-level = "z"` | `opt-level = 0` |
//! |---|---|---|---|
//! | [`YuvFrame::convert_blocks`] | 1.1–2 ms | 10–12 ms | 96–126 ms |
//! | [`YuvFrame::convert_scalar`] | 3.5–4.2 ms | 3.7–4.4 ms | 38 ms |
//!
//! Debug assertions and overflow checks take the vectorization away too, so a dev
//! build with this crate at `opt-level = 3` is better off on the scalar loop. A
//! release build sized with `"s"` or `"z"` pays for the block loop it cannot
//! vectorize; `profile.release.package.camera` can set this crate to `3`.
//!
//! The block loop gathers up to [`BLOCK`] pixels into fixed arrays so LLVM can
//! vectorize the arithmetic: 16-bit lanes for the per-pixel work, which holds
//! every intermediate value exactly. It does so for `aarch64-linux-android`
//! (`ld2` chroma, `sqxtun` clamps, `st4` pixels) at `opt-level = 3`. Rotation is
//! chosen once per frame, never per pixel. A quarter turn writes each source row
//! into a column of the output, which would touch one cache line per pixel, so
//! those two convert a [`TILE_ROWS`] by [`TILE_COLS`] tile at a time and write it
//! out line by line. It has no `unsafe`.
//!
//! The scalar loop is for a native dev build: raw pointers and plain `while`
//! loops, because at `opt-level = 0` every slice index, `clamp` and iterator step
//! is a real function call, several times the cost per pixel. Its `unsafe` and the
//! JNI view of the planes in `android.rs` are the only `unsafe` in the frame path.

/// Pixels converted per block. Even, so a block starts on a chroma sample.
/// Larger blocks spend less on loop setup: on the host 128 → 256 pixels took a
/// 1080p frame from 1.30 ms to 1.06 ms.
const BLOCK: usize = 256;
/// Chroma samples in a full block.
const CHROMA_BLOCK: usize = BLOCK / 2;
/// Source rows per tile of a quarter turn. One output cache line holds 16
/// `RGBA8` pixels, so a tile fills whole lines rather than revisiting them.
const TILE_ROWS: usize = 16;
/// Source columns per tile of a quarter turn: one output line each, so a tile
/// stays inside a small core's 32 KB L1 alongside its own 8 KB. Even, for the
/// same reason as [`BLOCK`].
const TILE_COLS: usize = 128;

/// One plane of an image: the bytes, and how far apart its samples are.
#[derive(Clone, Copy)]
pub(crate) struct Plane<'a> {
    pub(crate) data: &'a [u8],
    pub(crate) row_stride: usize,
    pub(crate) pixel_stride: usize,
}

impl<'a> Plane<'a> {
    /// `count` samples of row `row` from sample `first`, as the bytes spanning
    /// them. In bounds for any sample [`YuvFrame::new`] checked.
    fn segment(&self, row: usize, first: usize, count: usize) -> &'a [u8] {
        let start = row * self.row_stride + first * self.pixel_stride;
        &self.data[start..start + (count - 1) * self.pixel_stride + 1]
    }

    /// Whether `rows` rows of `cols` samples fit, the last row ending at its
    /// last sample.
    fn holds(&self, rows: usize, cols: usize) -> bool {
        if self.row_stride == 0 || self.pixel_stride == 0 {
            return false;
        }
        let last_row = (rows - 1).checked_mul(self.row_stride);
        let last_col = (cols - 1).checked_mul(self.pixel_stride);
        match (last_row, last_col) {
            (Some(row), Some(col)) => row
                .checked_add(col)
                .and_then(|last| last.checked_add(1))
                .is_some_and(|needed| needed <= self.data.len()),
            _ => false,
        }
    }
}

/// Clockwise turn applied to a frame on its way out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rotation {
    R0,
    R90,
    R180,
    R270,
}

impl Rotation {
    /// The turn for an angle from `android_rotation::frame_rotation`, or `None`
    /// for anything that is not a right angle in `0..360`.
    pub(crate) fn from_degrees(degrees: i32) -> Option<Self> {
        match degrees {
            0 => Some(Self::R0),
            90 => Some(Self::R90),
            180 => Some(Self::R180),
            270 => Some(Self::R270),
            _ => None,
        }
    }
}

/// A `YUV_420_888` frame whose planes are known to hold every sample.
pub(crate) struct YuvFrame<'a> {
    width: usize,
    height: usize,
    y: Plane<'a>,
    u: Plane<'a>,
    v: Plane<'a>,
}

impl<'a> YuvFrame<'a> {
    /// Check the planes against the dimensions and strides. `None` refuses the
    /// frame: empty, a zero stride, a plane too short, or a size that overflows.
    pub(crate) fn new(
        width: usize,
        height: usize,
        y: Plane<'a>,
        u: Plane<'a>,
        v: Plane<'a>,
    ) -> Option<Self> {
        if width == 0 || height == 0 {
            return None;
        }
        width.checked_mul(height)?.checked_mul(4)?;
        let (chroma_cols, chroma_rows) = (width.div_ceil(2), height.div_ceil(2));
        let fits = y.holds(height, width)
            && u.holds(chroma_rows, chroma_cols)
            && v.holds(chroma_rows, chroma_cols);
        fits.then_some(Self { width, height, y, u, v })
    }

    /// Output `(width, height)` under `rotation`: a quarter turn swaps them.
    pub(crate) fn output_size(&self, rotation: Rotation) -> (usize, usize) {
        match rotation {
            Rotation::R0 | Rotation::R180 => (self.width, self.height),
            Rotation::R90 | Rotation::R270 => (self.height, self.width),
        }
    }

    /// Convert into `out`, which must be exactly `width * height * 4` bytes.
    /// A wrong size writes nothing and answers `false`.
    pub(crate) fn convert(&self, rotation: Rotation, out: &mut [u8]) -> bool {
        if cfg!(debug_assertions) {
            self.convert_scalar(rotation, out)
        } else {
            self.convert_blocks(rotation, out)
        }
    }

    /// [`Self::convert`] for an optimized build.
    fn convert_blocks(&self, rotation: Rotation, out: &mut [u8]) -> bool {
        let (pixels, rest) = out.as_chunks_mut::<4>();
        if !rest.is_empty() || pixels.len() != self.width * self.height {
            return false;
        }
        let mut block = Block::new();
        match rotation {
            Rotation::R0 => {
                for (row, line) in pixels.chunks_exact_mut(self.width).enumerate() {
                    self.convert_row(row, line, &mut block);
                }
            }
            Rotation::R180 => {
                for (row, line) in pixels.chunks_exact_mut(self.width).rev().enumerate() {
                    self.convert_row(row, line, &mut block);
                    line.reverse();
                }
            }
            Rotation::R90 => self.convert_quarter_turn::<true>(pixels, &mut block),
            Rotation::R270 => self.convert_quarter_turn::<false>(pixels, &mut block),
        }
        true
    }

    /// [`Self::convert`] for an unoptimized build, one pixel at a time with the
    /// rotation baked into the destination index.
    fn convert_scalar(&self, rotation: Rotation, out: &mut [u8]) -> bool {
        let (width, height) = (self.width, self.height);
        if out.len() != width * height * 4 {
            return false;
        }
        let (out_width, _) = self.output_size(rotation);
        let (w, h, stride) = (width as isize, height as isize, out_width as isize);
        let (y_data, u_data, v_data) = (self.y.data.as_ptr(), self.u.data.as_ptr(), self.v.data.as_ptr());
        let (y_step, u_step, v_step) = (self.y.pixel_stride, self.u.pixel_stride, self.v.pixel_stride);
        let out = out.as_mut_ptr();
        let mut row = 0;
        while row < height {
            let r = row as isize;
            // The pixel a source row starts at, and how far apart its pixels land.
            let (start, step) = match rotation {
                Rotation::R0 => (r * stride, 1),
                Rotation::R90 => (h - 1 - r, stride),
                Rotation::R180 => ((h - 1 - r) * stride + w - 1, -1),
                Rotation::R270 => ((w - 1) * stride + r, -stride),
            };
            let mut dst = start * 4;
            let dst_step = step * 4;
            let mut y_at = row * self.y.row_stride;
            let mut u_at = (row / 2) * self.u.row_stride;
            let mut v_at = (row / 2) * self.v.row_stride;
            let mut col = 0;
            while col < width {
                // SAFETY: `new` proved the last sample of each plane in bounds and
                // the loops stop at it. `out` holds `width * height` pixels, and each
                // rotation maps a source pixel to a distinct one of them. The planes
                // may overlap each other but are only read, and `out` is ours alone.
                let (y, u, v, px) = unsafe {
                    (
                        *y_data.wrapping_add(y_at) as i32,
                        *u_data.wrapping_add(u_at) as i32 - 128,
                        *v_data.wrapping_add(v_at) as i32 - 128,
                        out.wrapping_offset(dst),
                    )
                };
                let mut red = y + ((1436 * v) >> 10);
                let mut green = y - ((352 * u + 731 * v) >> 10);
                let mut blue = y + ((1814 * u) >> 10);
                if red < 0 {
                    red = 0
                } else if red > 255 {
                    red = 255
                }
                if green < 0 {
                    green = 0
                } else if green > 255 {
                    green = 255
                }
                if blue < 0 {
                    blue = 0
                } else if blue > 255 {
                    blue = 255
                }
                // SAFETY: as above; `px` starts a pixel, so its four bytes are in `out`.
                unsafe {
                    *px = red as u8;
                    *px.wrapping_add(1) = green as u8;
                    *px.wrapping_add(2) = blue as u8;
                    *px.wrapping_add(3) = 255;
                }
                dst += dst_step;
                y_at += y_step;
                // Chroma is subsampled: it moves on after every odd column.
                if col & 1 == 1 {
                    u_at += u_step;
                    v_at += v_step;
                }
                col += 1;
            }
            row += 1;
        }
        true
    }

    /// One whole source row, left to right, into `line`.
    fn convert_row(&self, row: usize, line: &mut [[u8; 4]], block: &mut Block) {
        for (index, part) in line.chunks_mut(BLOCK).enumerate() {
            self.convert_block(row, index * BLOCK, part, block);
        }
    }

    /// A 90° (`CLOCKWISE`) or 270° turn. Source column `c` becomes output row
    /// `c` (or `width - 1 - c`), and source row `r` becomes output column
    /// `height - 1 - r` (or `r`).
    fn convert_quarter_turn<const CLOCKWISE: bool>(
        &self,
        pixels: &mut [[u8; 4]],
        block: &mut Block,
    ) {
        let out_width = self.height;
        let mut tile = [[[0u8; 4]; TILE_COLS]; TILE_ROWS];
        for first_row in (0..self.height).step_by(TILE_ROWS) {
            let rows = TILE_ROWS.min(self.height - first_row);
            let span = if CLOCKWISE {
                let end = out_width - first_row;
                end - rows..end
            } else {
                first_row..first_row + rows
            };
            for first_col in (0..self.width).step_by(TILE_COLS) {
                let cols = TILE_COLS.min(self.width - first_col);
                for (offset, tile_row) in tile[..rows].iter_mut().enumerate() {
                    self.convert_block(first_row + offset, first_col, &mut tile_row[..cols], block);
                }
                for col in 0..cols {
                    let out_row = if CLOCKWISE {
                        first_col + col
                    } else {
                        self.width - 1 - first_col - col
                    };
                    let start = out_row * out_width;
                    let cells = &mut pixels[start + span.start..start + span.end];
                    if CLOCKWISE {
                        for (cell, tile_row) in cells.iter_mut().rev().zip(&tile[..rows]) {
                            *cell = tile_row[col];
                        }
                    } else {
                        for (cell, tile_row) in cells.iter_mut().zip(&tile[..rows]) {
                            *cell = tile_row[col];
                        }
                    }
                }
            }
        }
    }

    /// Up to [`BLOCK`] pixels of `row` from the even column `first_col`.
    fn convert_block(
        &self,
        row: usize,
        first_col: usize,
        out: &mut [[u8; 4]],
        block: &mut Block,
    ) {
        let count = out.len();
        let chroma_count = count.div_ceil(2);
        let (chroma_row, first_chroma) = (row / 2, first_col / 2);
        let Block { y, u, v, red_half, green_half, blue_half, red, green, blue } = block;

        let y = samples(self.y.segment(row, first_col, count), self.y.pixel_stride, &mut y[..count]);
        let u = samples(
            self.u.segment(chroma_row, first_chroma, chroma_count),
            self.u.pixel_stride,
            &mut u[..chroma_count],
        );
        let v = samples(
            self.v.segment(chroma_row, first_chroma, chroma_count),
            self.v.pixel_stride,
            &mut v[..chroma_count],
        );

        // Per chroma sample. The products need 32 bits; the results fit 16.
        for i in 0..chroma_count {
            let (cb, cr) = (i32::from(u[i]) - 128, i32::from(v[i]) - 128);
            red_half[i] = ((1436 * cr) >> 10) as i16;
            green_half[i] = ((352 * cb + 731 * cr) >> 10) as i16;
            blue_half[i] = ((1814 * cb) >> 10) as i16;
        }
        double(&red_half[..chroma_count], red);
        double(&green_half[..chroma_count], green);
        double(&blue_half[..chroma_count], blue);

        // Per pixel: every value is within `-227..=482`, so 16-bit lanes hold it.
        let (red, green, blue) = (&red[..count], &green[..count], &blue[..count]);
        for (i, pixel) in out.iter_mut().enumerate() {
            let luma = i16::from(y[i]);
            *pixel = [
                (luma + red[i]).clamp(0, 255) as u8,
                (luma - green[i]).clamp(0, 255) as u8,
                (luma + blue[i]).clamp(0, 255) as u8,
                255,
            ];
        }
    }
}

/// The arrays a block is gathered into, kept per frame rather than zeroed per
/// block.
struct Block {
    y: [u8; BLOCK],
    u: [u8; CHROMA_BLOCK],
    v: [u8; CHROMA_BLOCK],
    red_half: [i16; CHROMA_BLOCK],
    green_half: [i16; CHROMA_BLOCK],
    blue_half: [i16; CHROMA_BLOCK],
    red: [i16; BLOCK],
    green: [i16; BLOCK],
    blue: [i16; BLOCK],
}

impl Block {
    fn new() -> Self {
        Self {
            y: [0; BLOCK],
            u: [0; CHROMA_BLOCK],
            v: [0; CHROMA_BLOCK],
            red_half: [0; CHROMA_BLOCK],
            green_half: [0; CHROMA_BLOCK],
            blue_half: [0; CHROMA_BLOCK],
            red: [0; BLOCK],
            green: [0; BLOCK],
            blue: [0; BLOCK],
        }
    }
}

/// A plane segment as consecutive samples: the bytes themselves at stride 1,
/// else gathered into `buf`, whose length is the sample count.
fn samples<'a>(segment: &'a [u8], stride: usize, buf: &'a mut [u8]) -> &'a [u8] {
    match stride {
        1 => &segment[..buf.len()],
        // The semi-planar layout, as its own arm so the stride is a constant.
        2 => {
            for (sample, byte) in buf.iter_mut().zip(segment.iter().step_by(2)) {
                *sample = *byte;
            }
            buf
        }
        _ => {
            for (sample, byte) in buf.iter_mut().zip(segment.iter().step_by(stride)) {
                *sample = *byte;
            }
            buf
        }
    }
}

/// Each chroma value twice: 4:2:0 shares a sample across two pixels of a row.
fn double(half: &[i16], full: &mut [i16; BLOCK]) {
    for (pair, value) in full.as_chunks_mut::<2>().0.iter_mut().zip(half) {
        *pair = [*value, *value];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame's backing bytes, laid out by the strides a test chose.
    struct Planes {
        width: usize,
        height: usize,
        y: Vec<u8>,
        y_row: usize,
        y_pixel: usize,
        u: Vec<u8>,
        u_row: usize,
        u_pixel: usize,
        v: Vec<u8>,
        v_row: usize,
        v_pixel: usize,
    }

    impl Planes {
        fn frame(&self) -> YuvFrame<'_> {
            YuvFrame::new(
                self.width,
                self.height,
                Plane { data: &self.y, row_stride: self.y_row, pixel_stride: self.y_pixel },
                Plane { data: &self.u, row_stride: self.u_row, pixel_stride: self.u_pixel },
                Plane { data: &self.v, row_stride: self.v_row, pixel_stride: self.v_pixel },
            )
            .expect("a well-formed frame")
        }
    }

    /// The layouts a device can hand out. Samples come from `y_at` / `u_at` /
    /// `v_at`; bytes a sample does not occupy hold a value that would show up
    /// in the output if read by mistake. Each plane ends at its last sample,
    /// as real planes do.
    #[derive(Clone, Copy, Debug)]
    enum Layout {
        /// I420: chroma pixel stride 1, no padding.
        Planar,
        /// I420 with padding at the end of every row.
        PlanarPadded,
        /// NV12: U, V interleaved; the V buffer starts one byte into the U one.
        Nv12,
        /// NV21: V, U interleaved; the U buffer starts one byte into the V one.
        Nv21,
        /// NV21 with an odd row padding, as some ISPs align rows.
        Nv21Padded,
        /// Nothing common: stride 3 chroma, luma pixel stride 2, odd row padding.
        Odd,
    }

    const LAYOUTS: [Layout; 6] = [
        Layout::Planar,
        Layout::PlanarPadded,
        Layout::Nv12,
        Layout::Nv21,
        Layout::Nv21Padded,
        Layout::Odd,
    ];
    const GARBAGE: u8 = 0xA5;

    fn sized(row: usize, pixel: usize, rows: usize, cols: usize) -> Vec<u8> {
        vec![GARBAGE; (rows - 1) * row + (cols - 1) * pixel + 1]
    }

    fn build(
        layout: Layout,
        width: usize,
        height: usize,
        y_at: impl Fn(usize, usize) -> u8,
        u_at: impl Fn(usize, usize) -> u8,
        v_at: impl Fn(usize, usize) -> u8,
    ) -> Planes {
        let (cw, ch) = (width.div_ceil(2), height.div_ceil(2));
        let (y_row, y_pixel) = match layout {
            Layout::Planar | Layout::Nv12 | Layout::Nv21 => (width, 1),
            Layout::PlanarPadded | Layout::Nv21Padded => (width + 7, 1),
            Layout::Odd => (width * 2 + 3, 2),
        };
        let mut y = sized(y_row, y_pixel, height, width);
        for r in 0..height {
            for c in 0..width {
                y[r * y_row + c * y_pixel] = y_at(r, c);
            }
        }

        let (c_row, c_pixel) = match layout {
            Layout::Planar => (cw, 1),
            Layout::PlanarPadded => (cw + 5, 1),
            Layout::Nv12 | Layout::Nv21 => (cw * 2, 2),
            Layout::Nv21Padded => (cw * 2 + 3, 2),
            Layout::Odd => (cw * 3 + 1, 3),
        };
        let mut u = sized(c_row, c_pixel, ch, cw);
        let mut v = sized(c_row, c_pixel, ch, cw);
        for r in 0..ch {
            for c in 0..cw {
                u[r * c_row + c * c_pixel] = u_at(r, c);
                v[r * c_row + c * c_pixel] = v_at(r, c);
            }
        }
        // Interleaved buffers each see the other plane's samples between their
        // own, the way Android's overlapping views do.
        if matches!(layout, Layout::Nv12 | Layout::Nv21 | Layout::Nv21Padded) {
            for r in 0..ch {
                for c in 0..cw - 1 {
                    u[r * c_row + c * c_pixel + 1] = v_at(r, c);
                    v[r * c_row + c * c_pixel + 1] = u_at(r, c + 1);
                }
            }
        }

        Planes {
            width,
            height,
            y,
            y_row,
            y_pixel,
            u,
            u_row: c_row,
            u_pixel: c_pixel,
            v,
            v_row: c_row,
            v_pixel: c_pixel,
        }
    }

    /// `RustCamera2Helper.yuv420ToRgba` as the Kotlin shim wrote it, line for
    /// line and on the same indexing, so the tests hold the Rust path to it.
    fn kotlin_reference(planes: &Planes, rotation: i32) -> Vec<u8> {
        let (width, height) = (planes.width as i32, planes.height as i32);
        let out_w = if rotation == 90 || rotation == 270 { height } else { width };
        let mut out = vec![0u8; (width * height * 4) as usize];
        for row in 0..height {
            let y_row = row * planes.y_row as i32;
            let uv_row = row >> 1;
            for col in 0..width {
                let y = i32::from(planes.y[(y_row + col * planes.y_pixel as i32) as usize]);
                let u_index = uv_row * planes.u_row as i32 + (col >> 1) * planes.u_pixel as i32;
                let v_index = uv_row * planes.v_row as i32 + (col >> 1) * planes.v_pixel as i32;
                let u = i32::from(planes.u[u_index as usize]) - 128;
                let v = i32::from(planes.v[v_index as usize]) - 128;

                let r = (y + ((1436 * v) >> 10)).clamp(0, 255);
                let g = (y - ((352 * u + 731 * v) >> 10)).clamp(0, 255);
                let b = (y + ((1814 * u) >> 10)).clamp(0, 255);

                let (d_row, d_col) = match rotation {
                    90 => (col, height - 1 - row),
                    180 => (height - 1 - row, width - 1 - col),
                    270 => (width - 1 - col, row),
                    _ => (row, col),
                };
                let dst = ((d_row * out_w + d_col) * 4) as usize;
                out[dst] = r as u8;
                out[dst + 1] = g as u8;
                out[dst + 2] = b as u8;
                out[dst + 3] = 255;
            }
        }
        out
    }

    /// A small deterministic generator: frames carrying every extreme.
    fn noise(seed: u32) -> impl Fn(usize, usize) -> u8 {
        move |r, c| {
            let mut x = seed ^ (r as u32).wrapping_mul(2_654_435_761) ^ (c as u32).wrapping_mul(40_503);
            x ^= x >> 13;
            x = x.wrapping_mul(1_274_126_177);
            x ^= x >> 16;
            match x & 0xF {
                0 => 0,
                1 => 255,
                _ => (x >> 8) as u8,
            }
        }
    }

    /// Through both loops, whichever one this build's `convert` takes, so every
    /// test below holds each of them to its expectation and to the other.
    fn convert(planes: &Planes, rotation: Rotation) -> Vec<u8> {
        let frame = planes.frame();
        let mut blocks = vec![0x11; planes.width * planes.height * 4];
        assert!(frame.convert_blocks(rotation, &mut blocks));
        let mut scalar = vec![0x11; planes.width * planes.height * 4];
        assert!(frame.convert_scalar(rotation, &mut scalar));
        assert!(blocks == scalar, "the two loops differ at {rotation:?}");
        let mut built = vec![0x11; planes.width * planes.height * 4];
        assert!(frame.convert(rotation, &mut built));
        assert!(built == blocks, "convert differs from its loops at {rotation:?}");
        blocks
    }

    const ROTATIONS: [(Rotation, i32); 4] = [
        (Rotation::R0, 0),
        (Rotation::R90, 90),
        (Rotation::R180, 180),
        (Rotation::R270, 270),
    ];

    /// A 2×3 luma ramp at neutral chroma, so each output pixel names its source.
    fn ramp(layout: Layout) -> Planes {
        build(layout, 2, 3, |r, c| (10 * (r + 1) + c) as u8, |_, _| 128, |_, _| 128)
    }

    fn grey_levels(rgba: &[u8]) -> Vec<u8> {
        rgba.chunks_exact(4)
            .map(|p| {
                assert_eq!((p[0], p[1], p[3]), (p[2], p[2], 255), "neutral chroma is grey, opaque");
                p[0]
            })
            .collect()
    }

    #[test]
    fn neutral_chroma_passes_luma_through_at_every_rotation() {
        // Source:  10 11
        //          20 21
        //          30 31
        for layout in LAYOUTS {
            let planes = ramp(layout);
            assert_eq!(grey_levels(&convert(&planes, Rotation::R0)), [10, 11, 20, 21, 30, 31]);
            // Clockwise: the left column reads bottom-up along the top row.
            assert_eq!(grey_levels(&convert(&planes, Rotation::R90)), [30, 20, 10, 31, 21, 11]);
            assert_eq!(grey_levels(&convert(&planes, Rotation::R180)), [31, 30, 21, 20, 11, 10]);
            assert_eq!(grey_levels(&convert(&planes, Rotation::R270)), [11, 21, 31, 10, 20, 30]);
        }
    }

    #[test]
    fn quarter_turns_swap_the_output_size() {
        let planes = ramp(Layout::Planar);
        let frame = planes.frame();
        assert_eq!(frame.output_size(Rotation::R0), (2, 3));
        assert_eq!(frame.output_size(Rotation::R90), (3, 2));
        assert_eq!(frame.output_size(Rotation::R180), (2, 3));
        assert_eq!(frame.output_size(Rotation::R270), (3, 2));
    }

    #[test]
    fn known_colours_in_every_layout() {
        // (y, u, v) → rgba worked by hand from the coefficients.
        let cases = [
            ((128, 128, 128), [128, 128, 128, 255]),
            ((0, 128, 128), [0, 0, 0, 255]),
            ((255, 128, 128), [255, 255, 255, 255]),
            // v = 255: r = 100 + (1436 * 127 >> 10) = 278 → 255;
            // g = 100 - (731 * 127 >> 10) = 10.
            ((100, 128, 255), [255, 10, 100, 255]),
            // u = 255: g = 100 - (352 * 127 >> 10) = 57;
            // b = 100 + (1814 * 127 >> 10) = 324 → 255.
            ((100, 255, 128), [100, 57, 255, 255]),
            // v = 0: r = 100 + (1436 * -128 >> 10) = 100 - 180 = -80 → 0;
            // g = 100 - (731 * -128 >> 10) = 100 + 92 = 192.
            ((100, 128, 0), [0, 192, 100, 255]),
            // u = 0: g = 100 - (352 * -128 >> 10) = 144;
            // b = 100 + (1814 * -128 >> 10) = 100 - 227 → 0.
            ((100, 0, 128), [100, 144, 0, 255]),
        ];
        for layout in LAYOUTS {
            for ((y, u, v), want) in cases {
                let planes = build(layout, 2, 2, |_, _| y, |_, _| u, |_, _| v);
                let out = convert(&planes, Rotation::R0);
                for pixel in out.chunks_exact(4) {
                    assert_eq!(pixel, want, "{layout:?} y={y} u={u} v={v}");
                }
            }
        }
    }

    #[test]
    fn green_shifts_the_sum_of_its_chroma_products() {
        // u = 129, v = 129: (352 + 731) >> 10 = 1, where shifting each product
        // alone gives 0 + 0. Separate shifts would come out one level brighter.
        let planes = build(Layout::Planar, 2, 2, |_, _| 100, |_, _| 129, |_, _| 129);
        assert_eq!(convert(&planes, Rotation::R0)[1], 99);
    }

    #[test]
    fn a_chroma_sample_covers_two_pixels_and_two_rows() {
        let planes = build(
            Layout::Nv21,
            4,
            4,
            |_, _| 100,
            |_, _| 128,
            |r, c| if (r, c) == (1, 1) { 255 } else { 128 },
        );
        let out = convert(&planes, Rotation::R0);
        for r in 0..4 {
            for c in 0..4 {
                let red = out[(r * 4 + c) * 4];
                let inside = r >= 2 && c >= 2;
                assert_eq!(red, if inside { 255 } else { 100 }, "pixel {r},{c}");
            }
        }
    }

    #[test]
    fn matches_the_kotlin_arithmetic_on_every_layout_rotation_and_size() {
        // Sizes straddle BLOCK and TILE_ROWS, and include odd dimensions, whose
        // last row or column has a chroma sample to itself.
        let sizes = [
            (1, 1),
            (2, 2),
            (3, 5),
            (5, 3),
            (16, 16),
            (17, 15),
            (127, 3),
            (128, 16),
            (129, 17),
            (130, 33),
            (257, 49),
            (640, 34),
        ];
        for layout in LAYOUTS {
            for (seed, (w, h)) in sizes.into_iter().enumerate() {
                let seed = seed as u32;
                let planes = build(layout, w, h, noise(seed), noise(seed + 100), noise(seed + 200));
                for (rotation, degrees) in ROTATIONS {
                    assert!(
                        convert(&planes, rotation) == kotlin_reference(&planes, degrees),
                        "{layout:?} {w}x{h} at {degrees}"
                    );
                }
            }
        }
    }

    #[test]
    fn regression_last_row_shorter_than_a_stride_is_accepted() {
        // Real planes end at their last sample, not at a full stride. A check
        // of `rows * row_stride` would drop every frame on such a device.
        let planes = build(Layout::Nv21Padded, 6, 4, noise(1), noise(2), noise(3));
        assert!(planes.y.len() < planes.height * planes.y_row);
        assert!(planes.u.len() < 2 * planes.u_row);
        assert_eq!(convert(&planes, Rotation::R0), kotlin_reference(&planes, 0));
    }

    /// Everything `YuvFrame::new` would be given, one field changed by `change`.
    fn refused_when(change: impl Fn(&mut Planes, &mut (usize, usize))) -> bool {
        let mut planes = build(Layout::Nv21Padded, 8, 6, noise(1), noise(2), noise(3));
        let mut size = (planes.width, planes.height);
        change(&mut planes, &mut size);
        YuvFrame::new(
            size.0,
            size.1,
            Plane { data: &planes.y, row_stride: planes.y_row, pixel_stride: planes.y_pixel },
            Plane { data: &planes.u, row_stride: planes.u_row, pixel_stride: planes.u_pixel },
            Plane { data: &planes.v, row_stride: planes.v_row, pixel_stride: planes.v_pixel },
        )
        .is_none()
    }

    #[test]
    fn exact_length_planes_are_accepted_and_one_byte_less_is_refused() {
        assert!(!refused_when(|_, _| {}));
        assert!(refused_when(|p, _| {
            p.y.pop();
        }));
        assert!(refused_when(|p, _| {
            p.u.pop();
        }));
        assert!(refused_when(|p, _| {
            p.v.pop();
        }));
    }

    #[test]
    fn empty_planes_zero_sizes_and_zero_strides_are_refused() {
        assert!(refused_when(|p, _| p.y.clear()));
        assert!(refused_when(|_, size| size.0 = 0));
        assert!(refused_when(|_, size| size.1 = 0));
        assert!(refused_when(|p, _| p.y_row = 0));
        assert!(refused_when(|p, _| p.y_pixel = 0));
        assert!(refused_when(|p, _| p.u_row = 0));
        assert!(refused_when(|p, _| p.v_pixel = 0));
    }

    #[test]
    fn sizes_that_overflow_are_refused_rather_than_wrapped() {
        assert!(refused_when(|_, size| *size = (usize::MAX, 2)));
        assert!(refused_when(|p, _| p.y_row = usize::MAX));
        assert!(refused_when(|p, _| p.u_pixel = usize::MAX / 2));
    }

    #[test]
    fn a_wrong_output_size_writes_nothing() {
        let planes = ramp(Layout::Planar);
        for len in [0, 4, 23, 25, 28] {
            let frame = planes.frame();
            let mut out = vec![0x11; len];
            assert!(!frame.convert(Rotation::R0, &mut out), "{len}");
            assert!(!frame.convert_blocks(Rotation::R0, &mut out), "{len}");
            assert!(!frame.convert_scalar(Rotation::R0, &mut out), "{len}");
            assert!(out.iter().all(|b| *b == 0x11), "{len}");
        }
    }

    #[test]
    fn only_right_angles_are_rotations() {
        assert_eq!(Rotation::from_degrees(0), Some(Rotation::R0));
        assert_eq!(Rotation::from_degrees(90), Some(Rotation::R90));
        assert_eq!(Rotation::from_degrees(180), Some(Rotation::R180));
        assert_eq!(Rotation::from_degrees(270), Some(Rotation::R270));
        for degrees in [-90, 1, 45, 360, 450] {
            assert_eq!(Rotation::from_degrees(degrees), None, "{degrees}");
        }
    }

    /// A 2×2 frame in NV21's layout, as Android devices commonly deliver it:
    /// V and U interleaved in one buffer, with row padding. Each luma differs,
    /// so a rotation shows in the output.
    fn convert_nv21_2x2(rotation: Rotation, lumas: [u8; 4]) -> Vec<u8> {
        let y = [lumas[0], lumas[1], 0, 0, lumas[2], lumas[3], 0, 0];
        let vu = [128u8, 128];
        let frame = YuvFrame::new(
            2,
            2,
            Plane { data: &y, row_stride: 4, pixel_stride: 1 },
            Plane { data: &vu[1..], row_stride: 4, pixel_stride: 2 },
            Plane { data: &vu, row_stride: 4, pixel_stride: 2 },
        )
        .expect("a well-formed frame");
        converted(&frame, rotation)
    }

    /// Through both loops, as [`convert`] does, for a frame no [`Planes`] built.
    fn converted(frame: &YuvFrame<'_>, rotation: Rotation) -> Vec<u8> {
        let (width, height) = frame.output_size(rotation);
        let mut blocks = vec![0; width * height * 4];
        assert!(frame.convert_blocks(rotation, &mut blocks));
        let mut scalar = vec![0; width * height * 4];
        assert!(frame.convert_scalar(rotation, &mut scalar));
        assert_eq!(blocks, scalar, "the two loops differ at {rotation:?}");
        blocks
    }

    fn lumas_of(rgba: &[u8]) -> Vec<u8> {
        rgba.chunks_exact(4).map(|px| px[0]).collect()
    }

    /// Clockwise: the top-left source pixel goes to the top-right at 90°, the
    /// bottom-right at 180° and the bottom-left at 270°.
    #[test]
    fn each_rotation_is_clockwise() {
        let source = [10, 20, 30, 40];
        assert_eq!(lumas_of(&convert_nv21_2x2(Rotation::R0, source)), [10, 20, 30, 40]);
        assert_eq!(lumas_of(&convert_nv21_2x2(Rotation::R90, source)), [30, 10, 40, 20]);
        assert_eq!(lumas_of(&convert_nv21_2x2(Rotation::R180, source)), [40, 30, 20, 10]);
        assert_eq!(lumas_of(&convert_nv21_2x2(Rotation::R270, source)), [20, 40, 10, 30]);
    }

    /// A non-square frame swaps its dimensions, which a wrong destination
    /// stride only shows here.
    #[test]
    fn a_wide_frame_rotates_into_a_tall_one() {
        let y = [1u8, 2, 3, 4, 5, 6];
        let chroma = [128u8; 4];
        let plane = |data| Plane { data, row_stride: 2, pixel_stride: 1 };
        let frame = YuvFrame::new(
            3,
            2,
            Plane { data: &y, row_stride: 3, pixel_stride: 1 },
            plane(&chroma),
            plane(&chroma),
        )
        .expect("a well-formed frame");
        assert_eq!(frame.output_size(Rotation::R90), (2, 3));
        assert_eq!(lumas_of(&converted(&frame, Rotation::R90)), [4, 1, 5, 2, 6, 3]);
        assert_eq!(lumas_of(&converted(&frame, Rotation::R270)), [3, 6, 2, 5, 1, 4]);
    }

    /// The conversion's colours and opaque alpha, at known BT.601 points.
    #[test]
    fn colours_match_bt601_full_range() {
        let pixel = |y: u8, u: u8, v: u8| {
            let (ys, us, vs) = ([y], [u], [v]);
            let plane = |data| Plane { data, row_stride: 1, pixel_stride: 1 };
            let frame = YuvFrame::new(1, 1, plane(&ys), plane(&us), plane(&vs))
                .expect("a well-formed frame");
            converted(&frame, Rotation::R0)
        };
        assert_eq!(pixel(128, 128, 128), [128, 128, 128, 255], "grey");
        assert_eq!(pixel(255, 128, 255), [255, 165, 255, 255], "clamps high");
        assert_eq!(pixel(0, 128, 0), [0, 92, 0, 255], "clamps low");
    }

    /// `Image.Plane` buffers can stop at the last sample, short of a full row.
    #[test]
    fn a_last_row_short_of_its_stride_still_converts() {
        let y = [1u8, 2, 0, 0, 3, 4];
        let chroma = [128u8, 128];
        let plane = |data| Plane { data, row_stride: 4, pixel_stride: 1 };
        let frame = YuvFrame::new(2, 2, plane(&y), plane(&chroma), plane(&chroma))
            .expect("a well-formed frame");
        assert_eq!(lumas_of(&converted(&frame, Rotation::R0)), [1, 2, 3, 4]);
    }

    /// A frame the planes cannot hold is dropped rather than panicking in the
    /// JNI callback, which aborts the app.
    #[test]
    fn a_short_plane_or_output_is_refused() {
        let chroma = [128u8; 2];
        let plane = |data, row_stride| Plane { data, row_stride, pixel_stride: 1 };
        let y = [0u8; 3];
        assert!(YuvFrame::new(2, 2, plane(&y, 2), plane(&chroma, 2), plane(&chroma, 2)).is_none());
        let y = [0u8; 4];
        let frame = YuvFrame::new(2, 2, plane(&y, 2), plane(&chroma, 2), plane(&chroma, 2))
            .expect("a well-formed frame");
        assert!(!frame.convert_blocks(Rotation::R0, &mut [0; 15]));
        assert!(!frame.convert_scalar(Rotation::R0, &mut [0; 15]));
        assert!(frame.convert_blocks(Rotation::R0, &mut [0; 16]));
        assert!(frame.convert_scalar(Rotation::R0, &mut [0; 16]));
    }

    /// Host timing of the block loop on a 1920×1080 frame, which only means
    /// something optimized:
    /// `cargo test -p camera --release android_yuv::tests::timing -- --ignored --nocapture`
    #[test]
    #[ignore = "a timing report, not an assertion"]
    fn timing_1080p() {
        const RUNS: u32 = 40;
        let (w, h) = (1920, 1080);
        for layout in [Layout::Nv21, Layout::Planar] {
            let planes = build(layout, w, h, noise(1), noise(2), noise(3));
            let mut out = vec![0u8; w * h * 4];
            for (rotation, degrees) in ROTATIONS {
                let start = std::time::Instant::now();
                for _ in 0..RUNS {
                    assert!(planes.frame().convert_blocks(rotation, std::hint::black_box(&mut out)));
                }
                let rust = start.elapsed() / RUNS;
                let start = std::time::Instant::now();
                for _ in 0..4 {
                    std::hint::black_box(kotlin_reference(&planes, degrees));
                }
                let reference = start.elapsed() / 4;
                println!("{layout:?} {degrees:>3}°: {rust:>10.2?}  (per-pixel reference {reference:.2?})");
            }
        }
    }

    /// Host timing of the scalar loop on a 1080p frame, in a dev build, which is
    /// where it runs:
    /// `cargo test -p camera android_yuv::tests::time_one -- --ignored --nocapture`
    #[test]
    #[ignore = "timing, not a check"]
    fn time_one_1080p_frame() {
        let (w, h) = (1920usize, 1080usize);
        let y: Vec<u8> = (0..w * h).map(|i| i as u8).collect();
        let vu: Vec<u8> = (0..w * h / 2).map(|i| (i * 7) as u8).collect();
        let frame = YuvFrame::new(
            w,
            h,
            Plane { data: &y, row_stride: w, pixel_stride: 1 },
            Plane { data: &vu[1..], row_stride: w, pixel_stride: 2 },
            Plane { data: &vu, row_stride: w, pixel_stride: 2 },
        )
        .expect("a well-formed frame");
        let mut out = vec![0; w * h * 4];
        for (rotation, degrees) in ROTATIONS {
            let started = std::time::Instant::now();
            assert!(frame.convert_scalar(rotation, &mut out));
            println!("{degrees}°: {:?}", started.elapsed());
        }
    }
}
