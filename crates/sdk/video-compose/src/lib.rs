//! Real-time cross-platform video compositing.
//!
//! [`VideoPipeline`] takes an input video [`MediaStream`], applies GPU
//! compositing ops each frame, and returns a NEW output [`MediaStream`] — "the
//! product." The input is never modified; the ops live only on the emitted
//! stream, which any consumer (a preview `video`, the `media-writer` recorder,
//! WebRTC) uses like any other stream.
//!
//! ```ignore
//! use video_compose::{VideoPipeline, Corner};
//! use canvas_core::ImageSource;
//!
//! let out = VideoPipeline::new(camera)
//!     .crop(move || crop_rect.get())                 // reactive src sub-rect (input px)
//!     .watermark(logo, Corner::BottomRight, 16.0, move || opacity.get())
//!     .overlay_stream(screen, move || pip_rect.get())// picture-in-picture
//!     .draw(move |s| { /* text / graphics, output px, on top */ })
//!     .build();                                      // -> MediaStream (the product)
//!
//! // `out` is a normal stream: preview it, record it, send it.
//! ```
//!
//! Every param is a reactive `Fn` closure, re-read each composited frame, so a
//! moving watermark / live crop / dragged PiP updates without rebuilding the
//! pipeline. Op call order is z-order (crop applies to the base input; watermark
//! / overlay stack above; `draw` graphics composite on top of everything).
//!
//! # Platforms
//!
//! The compositor is a headless `canvas` (see the crate's `Cargo.toml`): input
//! stream = base layer, watermark/PiP above, drawn graphics on top, composited by
//! `canvas-vello`'s `HeadlessCompositor` into the output stream. **macOS** is the
//! implemented, hardware-verified backend (zero-copy IOSurface in and out).
//! iOS/Android/web are sequenced next; on those targets (and where no GPU adapter
//! is available) [`build`](VideoPipeline::build) returns a live-but-empty output
//! stream rather than failing, so callers compile and run everywhere.
#![deny(missing_docs)]

use canvas_core::{Fit, ImageSource, Scene, TextureLayer};
use media_stream::MediaStream;
use std::rc::Rc;
use std::sync::Arc;

mod driver;

/// Upload-cache id for the CPU-input base layer (one texture slot, overwritten
/// each frame via a bumped generation). Kept distinct from any watermark id.
const BASE_IMAGE_ID: u64 = u64::MAX;

/// A corner of the output frame — where a [`watermark`](VideoPipeline::watermark)
/// is pinned, with a margin inset.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Corner {
    /// Top-left.
    TopLeft,
    /// Top-right.
    TopRight,
    /// Bottom-left.
    BottomLeft,
    /// Bottom-right.
    BottomRight,
}

/// One compositing operation, stored in call (z) order. Resolved every frame so
/// its reactive params update live.
pub(crate) enum Op {
    /// Crop the base (input) to a normalized-later source rect (given in input
    /// pixels), fit into the output.
    Crop {
        fit: Fit,
        rect: Rc<dyn Fn() -> (f32, f32, f32, f32)>,
    },
    /// A static image pinned to a corner with a margin + reactive opacity.
    Watermark {
        image: Arc<ImageSource>,
        corner: Corner,
        margin: f32,
        opacity: Rc<dyn Fn() -> f32>,
    },
    /// A second live stream composited at a reactive output rect (picture-in-picture).
    Overlay {
        stream: MediaStream,
        rect: Rc<dyn Fn() -> (f32, f32, f32, f32)>,
        corner_radius: Rc<dyn Fn() -> f32>,
    },
    /// Draw text / vector graphics on top, in output-logical coordinates.
    Draw(Rc<dyn Fn(&mut Scene)>),
}

/// A reactive GPU compositing pipeline. Build it up with the fluent methods, then
/// [`build`](Self::build) to get the output [`MediaStream`].
pub struct VideoPipeline {
    input: MediaStream,
    output_size: Option<(u32, u32)>,
    ops: Vec<Op>,
}

impl VideoPipeline {
    /// Start a pipeline from an input video stream.
    pub fn new(input: MediaStream) -> Self {
        Self { input, output_size: None, ops: Vec::new() }
    }

    /// Fixed output dimensions in pixels. Default: track the input frame's size.
    pub fn output_size(mut self, w: u32, h: u32) -> Self {
        self.output_size = Some((w, h));
        self
    }

    /// Crop the base (input) video to a reactive source rectangle `(x, y, w, h)`
    /// in INPUT pixels, scaled to the output with [`Fit::Cover`]. The input stream
    /// itself is untouched — only the output is cropped.
    pub fn crop(self, rect: impl Fn() -> (f32, f32, f32, f32) + 'static) -> Self {
        self.crop_fit(Fit::Cover, rect)
    }

    /// [`crop`](Self::crop) with an explicit [`Fit`] for the scale into the output.
    pub fn crop_fit(mut self, fit: Fit, rect: impl Fn() -> (f32, f32, f32, f32) + 'static) -> Self {
        self.ops.push(Op::Crop { fit, rect: Rc::new(rect) });
        self
    }

    /// Overlay a static `image` (a logo/watermark) pinned to `corner` with a
    /// `margin` inset (logical px), drawn at the image's native pixel size, with a
    /// reactive `opacity` (`0.0..=1.0`). Transparent PNG regions blend through.
    pub fn watermark(
        mut self,
        image: ImageSource,
        corner: Corner,
        margin: f32,
        opacity: impl Fn() -> f32 + 'static,
    ) -> Self {
        self.ops.push(Op::Watermark {
            image: Arc::new(image),
            corner,
            margin,
            opacity: Rc::new(opacity),
        });
        self
    }

    /// Composite a second live `stream` (picture-in-picture) at a reactive output
    /// rectangle `(x, y, w, h)` in output pixels, cover-fit, square corners.
    pub fn overlay_stream(
        self,
        stream: MediaStream,
        rect: impl Fn() -> (f32, f32, f32, f32) + 'static,
    ) -> Self {
        self.overlay_stream_rounded(stream, rect, || 0.0)
    }

    /// [`overlay_stream`](Self::overlay_stream) with a reactive corner radius
    /// (logical px) — e.g. a circular camera bubble over a screen share.
    pub fn overlay_stream_rounded(
        mut self,
        stream: MediaStream,
        rect: impl Fn() -> (f32, f32, f32, f32) + 'static,
        corner_radius: impl Fn() -> f32 + 'static,
    ) -> Self {
        self.ops.push(Op::Overlay {
            stream,
            rect: Rc::new(rect),
            corner_radius: Rc::new(corner_radius),
        });
        self
    }

    /// Draw text / vector graphics ON TOP of the video (and everything else), via
    /// a [`Scene`] painter in output-logical coordinates. Reactive: signals read
    /// inside re-paint on change.
    pub fn draw(mut self, painter: impl Fn(&mut Scene) + 'static) -> Self {
        self.ops.push(Op::Draw(Rc::new(painter)));
        self
    }

    /// Build the pipeline and return the OUTPUT [`MediaStream`] — the product.
    /// Compositing runs while any clone of the returned stream is alive; dropping
    /// the last clone tears the driver down. The input stream is unaffected.
    pub fn build(self) -> MediaStream {
        let (stream, writer) = MediaStream::with_surface_capture();
        let handle = driver::spawn(self.input, writer, self.output_size, self.ops);
        stream.attach_stopper(move || drop(handle));
        stream
    }
}

/// Compute a watermark's destination rect from the image size, the chosen corner,
/// a margin, and the output dimensions. Shared by the driver; kept here (pure,
/// no GPU) so it's unit-testable without a device.
pub(crate) fn watermark_rect(
    corner: Corner,
    margin: f32,
    img_w: f32,
    img_h: f32,
    out_w: f32,
    out_h: f32,
) -> (f32, f32, f32, f32) {
    let (x, y) = match corner {
        Corner::TopLeft => (margin, margin),
        Corner::TopRight => (out_w - img_w - margin, margin),
        Corner::BottomLeft => (margin, out_h - img_h - margin),
        Corner::BottomRight => (out_w - img_w - margin, out_h - img_h - margin),
    };
    (x, y, img_w, img_h)
}

/// Normalize an input-pixel crop rect against the input dimensions, clamped to
/// `0.0..=1.0`. Returns `None` (whole source) if the dims are unknown or the rect
/// is degenerate.
pub(crate) fn normalized_crop(
    rect: (f32, f32, f32, f32),
    in_w: f32,
    in_h: f32,
) -> Option<(f32, f32, f32, f32)> {
    if in_w <= 0.0 || in_h <= 0.0 || rect.2 <= 0.0 || rect.3 <= 0.0 {
        return None;
    }
    let x = (rect.0 / in_w).clamp(0.0, 1.0);
    let y = (rect.1 / in_h).clamp(0.0, 1.0);
    let w = (rect.2 / in_w).clamp(0.0, 1.0 - x);
    let h = (rect.3 / in_h).clamp(0.0, 1.0 - y);
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    Some((x, y, w, h))
}

/// Build the texture-layer stack for one composited frame from the ops, the input
/// stream, and the resolved output/input sizes. The base (input) is `layers[0]`;
/// watermark/PiP layers stack above. Draw ops are handled separately (overlay
/// scene). Pure except for reading the reactive op closures — no GPU here, so the
/// z-order + geometry is unit-testable.
pub(crate) fn build_layers(
    input: &MediaStream,
    ops: &[Op],
    out_w: f32,
    out_h: f32,
    in_w: f32,
    in_h: f32,
) -> Vec<TextureLayer> {
    // Base input layer, filling the whole output. Crop op (if any) sets fit + a
    // normalized source sub-rect.
    let (base_fit, base_crop) = ops
        .iter()
        .find_map(|op| match op {
            Op::Crop { fit, rect } => Some((*fit, normalized_crop(rect(), in_w, in_h))),
            _ => None,
        })
        .unwrap_or((Fit::Cover, None));

    let base_rect: Rc<dyn Fn() -> (f32, f32, f32, f32)> = Rc::new(move || (0.0, 0.0, out_w, out_h));
    // Prefer the input's zero-copy native surface (an IOSurface on macOS — the
    // camera fast path). A CPU-only input (no native surface: a synthetic stream,
    // a non-IOSurface source, tests) is uploaded as an image each frame instead,
    // its `generation` tracking the input so the compositor re-uploads new frames.
    let mut base = if input.native_source().is_some() {
        let input_for_base = input.clone();
        TextureLayer::new(Rc::new(move || Some(input_for_base.clone())), base_rect).fit(base_fit)
    } else {
        let mut buf = Vec::new();
        match input.latest(&mut buf) {
            Some((w, h)) => {
                let img = Arc::new(
                    ImageSource::from_rgba8(BASE_IMAGE_ID, w, h, buf)
                        .with_generation(input.generation()),
                );
                TextureLayer::image(Rc::new(move || Some(img.clone())), base_rect).fit(base_fit)
            }
            None => TextureLayer::image(Rc::new(|| None), base_rect).fit(base_fit),
        }
    };
    base.src_crop = base_crop;

    let mut layers = vec![base];

    for op in ops {
        match op {
            Op::Watermark { image, corner, margin, opacity } => {
                let rect = watermark_rect(
                    *corner,
                    *margin,
                    image.width as f32,
                    image.height as f32,
                    out_w,
                    out_h,
                );
                let image = image.clone();
                let o = opacity().clamp(0.0, 1.0);
                layers.push(
                    TextureLayer::image(
                        Rc::new(move || Some(image.clone())),
                        Rc::new(move || rect),
                    )
                    .fit(Fit::Fill)
                    .opacity(o),
                );
            }
            Op::Overlay { stream, rect, corner_radius } => {
                let stream = stream.clone();
                let r = rect();
                let radius = corner_radius();
                layers.push(
                    TextureLayer::new(
                        Rc::new(move || Some(stream.clone())),
                        Rc::new(move || r),
                    )
                    .fit(Fit::Cover)
                    .corner_radius(radius),
                );
            }
            Op::Crop { .. } | Op::Draw(_) => {}
        }
    }
    layers
}

/// Paint all [`Op::Draw`] painters into one overlay [`Scene`] (composited on top).
/// Empty when the pipeline has no draw ops.
pub(crate) fn build_overlay_scene(ops: &[Op]) -> Scene {
    let mut scene = Scene::new();
    for op in ops {
        if let Op::Draw(painter) = op {
            painter(&mut scene);
        }
    }
    scene
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watermark_corners_place_correctly() {
        // 20×10 image, 8px margin, 200×100 output.
        assert_eq!(watermark_rect(Corner::TopLeft, 8.0, 20.0, 10.0, 200.0, 100.0), (8.0, 8.0, 20.0, 10.0));
        assert_eq!(
            watermark_rect(Corner::BottomRight, 8.0, 20.0, 10.0, 200.0, 100.0),
            (200.0 - 20.0 - 8.0, 100.0 - 10.0 - 8.0, 20.0, 10.0)
        );
        assert_eq!(watermark_rect(Corner::TopRight, 8.0, 20.0, 10.0, 200.0, 100.0), (172.0, 8.0, 20.0, 10.0));
        assert_eq!(watermark_rect(Corner::BottomLeft, 8.0, 20.0, 10.0, 200.0, 100.0), (8.0, 82.0, 20.0, 10.0));
    }

    #[test]
    fn crop_normalizes_and_guards_degenerate() {
        // A 160×90 crop at (40,20) of a 320×180 input → centered-ish quarter.
        assert_eq!(normalized_crop((40.0, 20.0, 160.0, 90.0), 320.0, 180.0), Some((0.125, 0.111_111_11, 0.5, 0.5)));
        // Unknown input size or zero-size rect → whole source.
        assert_eq!(normalized_crop((0.0, 0.0, 10.0, 10.0), 0.0, 0.0), None);
        assert_eq!(normalized_crop((0.0, 0.0, 0.0, 0.0), 320.0, 180.0), None);
    }

    #[test]
    fn build_layers_puts_input_at_base_and_watermark_above() {
        let (input, _w) = MediaStream::new();
        let img = ImageSource::from_rgba8(1, 4, 4, vec![255; 64]);
        let ops = vec![Op::Watermark {
            image: Arc::new(img),
            corner: Corner::TopLeft,
            margin: 2.0,
            opacity: Rc::new(|| 0.5),
        }];
        let layers = build_layers(&input, &ops, 100.0, 100.0, 100.0, 100.0);
        assert_eq!(layers.len(), 2, "base input + one watermark");
        // The watermark layer sits above the base and carries the resolved opacity.
        assert_eq!(layers[1].opacity, 0.5);
    }

    #[test]
    fn draw_ops_build_overlay_scene() {
        let ops = vec![Op::Draw(Rc::new(|s: &mut Scene| {
            s.path().add_path(canvas_core::Path::rect(0.0, 0.0, 4.0, 4.0));
            s.fill(canvas_core::Color::new(255, 0, 0, 255));
        }))];
        assert!(!build_overlay_scene(&ops).is_empty());
        // No draw ops → empty overlay (the compositor skips the overlay pass).
        assert!(build_overlay_scene(&[]).is_empty());
    }
}
