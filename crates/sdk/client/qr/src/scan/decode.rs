//! The pure decode path: RGBA → greyscale (optionally downscaled) → `rqrr`.
//!
//! Everything here is synchronous, allocation-owning, and free of any
//! platform type, so the same code runs inline (the still-image helpers) and
//! inside an `offload` job (the live scanner) on every target.

use serde::{Deserialize, Serialize};

use super::{Point, Scan, ScannedCode};

/// One greyscale frame on its way to the decoder. Crosses the web worker
/// boundary (postcard), so it carries everything the job needs to report
/// results in SOURCE-frame coordinates: the downscale factor and the original
/// frame's size and timestamp.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct LumaFrame {
    /// Greyscale size actually decoded (`source / scale`, floored).
    pub width: u32,
    pub height: u32,
    /// Integer box-filter factor applied to the source; `1` = full resolution.
    pub scale: u32,
    pub source_width: u32,
    pub source_height: u32,
    pub pts_micros: u64,
    /// `width * height` bytes, one luma sample per pixel, top-down.
    pub data: Vec<u8>,
}

impl LumaFrame {
    /// Convert a tightly-packed `RGBA8` frame, box-filtering it down by the
    /// smallest integer factor that brings its longer edge to
    /// `max_dimension` or below. `None` when `rgba` is shorter than
    /// `width * height * 4` or the frame is empty.
    pub fn from_rgba(
        width: u32,
        height: u32,
        rgba: &[u8],
        max_dimension: u32,
        pts_micros: u64,
    ) -> Option<LumaFrame> {
        let (w, h) = (width as usize, height as usize);
        if w == 0 || h == 0 || rgba.len() < w * h * 4 {
            return None;
        }
        let scale = width.max(height).div_ceil(max_dimension.max(1)).max(1);
        let s = scale as usize;
        let (ow, oh) = (w / s, h / s);
        if ow == 0 || oh == 0 {
            return None;
        }

        let mut data = Vec::with_capacity(ow * oh);
        if s == 1 {
            data.extend(rgba[..w * h * 4].chunks_exact(4).map(luma));
        } else {
            // Box average: each output sample is the mean luma of an s×s
            // block. Averaging (not point-sampling) matters — point-sampling a
            // frame whose module width is near the factor aliases the finder
            // patterns away.
            let area = (s * s) as u32;
            for oy in 0..oh {
                for ox in 0..ow {
                    let mut sum = 0u32;
                    for y in oy * s..oy * s + s {
                        let row = &rgba[(y * w + ox * s) * 4..(y * w + ox * s + s) * 4];
                        sum += row.chunks_exact(4).map(|px| luma(px) as u32).sum::<u32>();
                    }
                    data.push((sum / area) as u8);
                }
            }
        }
        Some(LumaFrame {
            width: ow as u32,
            height: oh as u32,
            scale,
            source_width: width,
            source_height: height,
            pts_micros,
            data,
        })
    }
}

/// BT.601 luma in 8-bit fixed point. The weights sum to 256, so white maps to
/// exactly 255 and black to 0.
fn luma(px: &[u8]) -> u8 {
    ((77 * px[0] as u32 + 150 * px[1] as u32 + 29 * px[2] as u32) >> 8) as u8
}

/// Detect and decode every QR code in a greyscale buffer. Corners are mapped
/// back to source coordinates by `scale`. Grids that are found but fail to
/// decode (blurred, cut off, damaged beyond the error correction) are skipped.
pub(crate) fn decode_luma(width: usize, height: usize, data: &[u8], scale: u32) -> Vec<ScannedCode> {
    let mut img = rqrr::PreparedImage::prepare_from_greyscale(width, height, |x, y| {
        data[y * width + x]
    });
    let s = scale as f32;
    img.detect_grids()
        .into_iter()
        .filter_map(|grid| {
            // A fresh buffer per grid: `decode_to` may leave partial output
            // behind when it fails.
            let mut bytes = Vec::new();
            let meta = grid.decode_to(&mut bytes).ok()?;
            Some(ScannedCode {
                bytes,
                corners: grid.bounds.map(|p| Point {
                    x: p.x as f32 * s,
                    y: p.y as f32 * s,
                }),
                version: meta.version.0 as u32,
            })
        })
        .collect()
}

/// The `offload` job the live scanner runs per frame. A free fn with serde
/// argument and result, as `offload` requires: on web it runs in a Web Worker
/// that instantiates the same module, on native on a `std::thread`.
#[offload::job]
pub(crate) fn decode_job(frame: LumaFrame) -> Scan {
    Scan {
        codes: decode_luma(
            frame.width as usize,
            frame.height as usize,
            &frame.data,
            frame.scale,
        ),
        width: frame.source_width,
        height: frame.source_height,
        pts_micros: frame.pts_micros,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_and_black_map_to_the_ends_of_the_range() {
        assert_eq!(luma(&[255, 255, 255, 255]), 255);
        assert_eq!(luma(&[0, 0, 0, 255]), 0);
    }

    #[test]
    fn a_frame_within_the_limit_is_not_downscaled() {
        let rgba = vec![255u8; 640 * 480 * 4];
        let f = LumaFrame::from_rgba(640, 480, &rgba, 960, 7).unwrap();
        assert_eq!((f.width, f.height, f.scale), (640, 480, 1));
        assert_eq!(f.data.len(), 640 * 480);
        assert_eq!(f.pts_micros, 7);
    }

    #[test]
    fn a_large_frame_is_box_filtered_to_the_limit() {
        // 1920×1080 against a 960 limit → factor 2.
        let mut rgba = vec![0u8; 1920 * 1080 * 4];
        // Left half white, right half black.
        for y in 0..1080 {
            for x in 0..960 {
                let i = (y * 1920 + x) * 4;
                rgba[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        let f = LumaFrame::from_rgba(1920, 1080, &rgba, 960, 0).unwrap();
        assert_eq!((f.width, f.height, f.scale), (960, 540, 2));
        assert_eq!((f.source_width, f.source_height), (1920, 1080));
        assert_eq!(f.data[0], 255);
        assert_eq!(f.data[959], 0);
    }

    #[test]
    fn the_box_filter_averages_rather_than_point_samples() {
        // A 2×2 checker at factor 2 must average to mid-grey; point-sampling
        // would return the top-left pixel (white).
        let rgba = [
            255, 255, 255, 255, 0, 0, 0, 255, //
            0, 0, 0, 255, 255, 255, 255, 255,
        ];
        let f = LumaFrame::from_rgba(2, 2, &rgba, 1, 0).unwrap();
        assert_eq!((f.width, f.height), (1, 1));
        assert_eq!(f.data, vec![127]);
    }

    #[test]
    fn a_short_buffer_is_rejected_not_read_past() {
        assert!(LumaFrame::from_rgba(4, 4, &[0; 15], 960, 0).is_none());
        assert!(LumaFrame::from_rgba(0, 4, &[], 960, 0).is_none());
    }
}
