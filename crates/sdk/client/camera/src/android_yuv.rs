//! Android: `YUV_420_888` → tightly-packed top-down `RGBA8`, rotated.
//!
//! Runs on the camera's `ImageReader` thread once per frame, in Rust because a
//! per-pixel loop of `ByteBuffer.get` / `put` in Kotlin pins a core at 1080p
//! and lets through about one frame a second on a mid-range tablet. Pure, so it
//! is unit-tested on the host; `android.rs` hands it the plane buffers.
//!
//! BT.601 full-range, in integer arithmetic. The rotation is baked into the
//! destination index, so there is no second pass. Never mirrors: the front
//! camera comes out unmirrored, matching the web and iOS backends.

/// One plane as `Image.Plane` describes it. A `YUV_420_888` image's U and V
/// planes can share memory (NV12 / NV21, `pixel_stride == 2`), and a plane's
/// last row can stop short of `row_stride`.
pub(crate) struct Plane<'a> {
    pub data: &'a [u8],
    pub row_stride: usize,
    pub pixel_stride: usize,
}

impl Plane<'_> {
    /// Whether the plane holds a sample at `rows - 1`, `cols - 1`.
    fn covers(&self, rows: usize, cols: usize) -> bool {
        (rows - 1)
            .checked_mul(self.row_stride)
            .and_then(|at| (cols - 1).checked_mul(self.pixel_stride)?.checked_add(at))
            .is_some_and(|last| last < self.data.len())
    }
}

/// A frame's three planes and its size before rotation.
pub(crate) struct Yuv420<'a> {
    pub y: Plane<'a>,
    pub u: Plane<'a>,
    pub v: Plane<'a>,
    pub width: usize,
    pub height: usize,
}

/// The output size for `rotation` degrees clockwise: 90° and 270° swap it.
pub(crate) fn rotated_size(width: usize, height: usize, rotation: i32) -> (usize, usize) {
    if rotation == 90 || rotation == 270 {
        (height, width)
    } else {
        (width, height)
    }
}

/// Convert `frame` into `out`, rotated `rotation` degrees clockwise. `out` must
/// be exactly the [`rotated_size`] in `RGBA8`. Returns `false`, writing nothing,
/// for a frame whose planes or output cannot hold it.
pub(crate) fn yuv420_to_rgba(frame: &Yuv420<'_>, rotation: i32, out: &mut [u8]) -> bool {
    let (width, height) = (frame.width, frame.height);
    if width == 0 || height == 0 {
        return false;
    }
    let (chroma_w, chroma_h) = (width.div_ceil(2), height.div_ceil(2));
    if !frame.y.covers(height, width)
        || !frame.u.covers(chroma_h, chroma_w)
        || !frame.v.covers(chroma_h, chroma_w)
        || out.len() != width * height * 4
    {
        return false;
    }
    let (out_w, _) = rotated_size(width, height, rotation);
    let (w, h, stride) = (width as isize, height as isize, out_w as isize);
    let (y_data, u_data, v_data) = (frame.y.data.as_ptr(), frame.u.data.as_ptr(), frame.v.data.as_ptr());
    let (y_step, u_step, v_step) = (frame.y.pixel_stride, frame.u.pixel_stride, frame.v.pixel_stride);
    let out = out.as_mut_ptr();
    // Raw pointers and plain `while` loops, not slices and ranges: a native dev
    // build compiles this at opt-level 0, where every slice index, `clamp` and
    // iterator step is a real function call, several times the cost per pixel.
    let mut row = 0;
    while row < height {
        let r = row as isize;
        // The pixel a source row starts at, and how far apart its pixels land.
        let (start, step) = match rotation {
            90 => (h - 1 - r, stride),
            180 => ((h - 1 - r) * stride + w - 1, -1),
            270 => ((w - 1) * stride + r, -stride),
            _ => (r * stride, 1),
        };
        let mut dst = start * 4;
        let dst_step = step * 4;
        let mut y_at = row * frame.y.row_stride;
        let mut u_at = (row / 2) * frame.u.row_stride;
        let mut v_at = (row / 2) * frame.v.row_stride;
        let mut col = 0;
        while col < width {
            // SAFETY: `covers` proved the last sample of each plane in bounds and
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2×2 frame in NV21's layout, as Android devices commonly deliver it:
    /// V and U interleaved in one buffer, with row padding. Each luma differs,
    /// so a rotation shows in the output.
    fn convert(rotation: i32, lumas: [u8; 4]) -> Vec<u8> {
        let y = [lumas[0], lumas[1], 0, 0, lumas[2], lumas[3], 0, 0];
        let vu = [128u8, 128];
        let frame = Yuv420 {
            y: Plane { data: &y, row_stride: 4, pixel_stride: 1 },
            u: Plane { data: &vu[1..], row_stride: 4, pixel_stride: 2 },
            v: Plane { data: &vu, row_stride: 4, pixel_stride: 2 },
            width: 2,
            height: 2,
        };
        let mut out = vec![0; 16];
        assert!(yuv420_to_rgba(&frame, rotation, &mut out));
        out
    }

    fn lumas_of(rgba: &[u8]) -> Vec<u8> {
        rgba.chunks_exact(4).map(|px| px[0]).collect()
    }

    /// Clockwise: the top-left source pixel goes to the top-right at 90°, the
    /// bottom-right at 180° and the bottom-left at 270°.
    #[test]
    fn each_rotation_is_clockwise() {
        let source = [10, 20, 30, 40];
        assert_eq!(lumas_of(&convert(0, source)), [10, 20, 30, 40]);
        assert_eq!(lumas_of(&convert(90, source)), [30, 10, 40, 20]);
        assert_eq!(lumas_of(&convert(180, source)), [40, 30, 20, 10]);
        assert_eq!(lumas_of(&convert(270, source)), [20, 40, 10, 30]);
    }

    /// A non-square frame swaps its dimensions, which a wrong destination
    /// stride only shows here.
    #[test]
    fn a_wide_frame_rotates_into_a_tall_one() {
        let y = [1u8, 2, 3, 4, 5, 6];
        let chroma = [128u8; 4];
        let plane = |data| Plane { data, row_stride: 2, pixel_stride: 1 };
        let frame = Yuv420 {
            y: Plane { data: &y, row_stride: 3, pixel_stride: 1 },
            u: plane(&chroma),
            v: plane(&chroma),
            width: 3,
            height: 2,
        };
        assert_eq!(rotated_size(3, 2, 90), (2, 3));
        let mut out = vec![0; 24];
        assert!(yuv420_to_rgba(&frame, 90, &mut out));
        assert_eq!(lumas_of(&out), [4, 1, 5, 2, 6, 3]);
        assert!(yuv420_to_rgba(&frame, 270, &mut out));
        assert_eq!(lumas_of(&out), [3, 6, 2, 5, 1, 4]);
    }

    /// The conversion's colours and opaque alpha, at known BT.601 points.
    #[test]
    fn colours_match_bt601_full_range() {
        let pixel = |y: u8, u: u8, v: u8| {
            let (ys, us, vs) = ([y], [u], [v]);
            let plane = |data| Plane { data, row_stride: 1, pixel_stride: 1 };
            let frame = Yuv420 { y: plane(&ys), u: plane(&us), v: plane(&vs), width: 1, height: 1 };
            let mut out = [0u8; 4];
            assert!(yuv420_to_rgba(&frame, 0, &mut out));
            out
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
        let frame = Yuv420 { y: plane(&y), u: plane(&chroma), v: plane(&chroma), width: 2, height: 2 };
        let mut out = vec![0; 16];
        assert!(yuv420_to_rgba(&frame, 0, &mut out));
        assert_eq!(lumas_of(&out), [1, 2, 3, 4]);
    }

    /// A frame the planes cannot hold is dropped rather than panicking in the
    /// JNI callback, which aborts the app.
    #[test]
    fn a_short_plane_or_output_is_refused() {
        let y = [0u8; 3];
        let chroma = [128u8; 2];
        let plane = |data, row_stride| Plane { data, row_stride, pixel_stride: 1 };
        let frame = Yuv420 { y: plane(&y, 2), u: plane(&chroma, 2), v: plane(&chroma, 2), width: 2, height: 2 };
        assert!(!yuv420_to_rgba(&frame, 0, &mut [0; 16]));
        let y = [0u8; 4];
        let frame = Yuv420 { y: plane(&y, 2), u: plane(&chroma, 2), v: plane(&chroma, 2), width: 2, height: 2 };
        assert!(!yuv420_to_rgba(&frame, 0, &mut [0; 15]));
        assert!(yuv420_to_rgba(&frame, 0, &mut [0; 16]));
    }

    /// Host timing for a 1080p frame; run with
    /// `cargo test --release -p camera android_yuv -- --ignored --nocapture`.
    #[test]
    #[ignore = "timing, not a check"]
    fn time_one_1080p_frame() {
        let (w, h) = (1920usize, 1080usize);
        let y: Vec<u8> = (0..w * h).map(|i| i as u8).collect();
        let vu: Vec<u8> = (0..w * h / 2).map(|i| (i * 7) as u8).collect();
        let frame = Yuv420 {
            y: Plane { data: &y, row_stride: w, pixel_stride: 1 },
            u: Plane { data: &vu[1..], row_stride: w, pixel_stride: 2 },
            v: Plane { data: &vu, row_stride: w, pixel_stride: 2 },
            width: w,
            height: h,
        };
        let mut out = vec![0; w * h * 4];
        for rotation in [0, 90, 180, 270] {
            let started = std::time::Instant::now();
            assert!(yuv420_to_rgba(&frame, rotation, &mut out));
            println!("{rotation}°: {:?}", started.elapsed());
        }
    }
}
