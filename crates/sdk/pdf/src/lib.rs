//! Render PDF pages onto a [`canvas`](canvas_core) `Scene` — pure Rust, GPU
//! when `canvas-vello` is active, CPU (`canvas-native`) otherwise.
//!
//! PDF parsing + interpretation is [`hayro`](hayro_interpret); this crate is the
//! bridge. [`SceneDevice`] implements [`hayro_interpret::Device`] and records
//! every drawing instruction into a renderer-agnostic [`canvas_core::Scene`]:
//! text → [`DrawOp::Glyphs`](canvas_core::DrawOp::Glyphs) runs (the GPU glyph
//! pipeline), vectors → `Fill`/`Stroke`, images → `Image`. The same scene then
//! drives whichever canvas renderer is registered, so one interpretation renders
//! identically on Metal, Vulkan, WebGPU, or the CPU fallback.
//!
//! ```no_run
//! let doc = pdf::Document::load(std::fs::read("doc.pdf").unwrap()).unwrap();
//! let page = doc.render_page(0).unwrap();
//! // page.scene is a canvas_core::Scene sized page.width × page.height (points).
//! ```
//!
//! # Current approximations
//!
//! Tracked per render in [`Warnings`] (not silently dropped): pattern/shading
//! fills (gradients, tiling) draw as nothing; soft masks are ignored; blend
//! modes outside {Normal, Multiply, Screen} downgrade to Normal; dashed strokes
//! render solid. These mirror gaps in `hayro` itself and shrink as both mature.

mod device;
mod image;
mod shading;
mod view;

pub use device::{SceneDevice, Warnings, GLYPH_UPEM};
pub use view::{Pdf, PdfReactive, PdfView};

use canvas_core::Scene;
use hayro_interpret::hayro_syntax::Pdf;
use hayro_interpret::{
    interpret_page, BlendMode, ClipPath, Context, Device, FillRule, InterpreterCache,
    InterpreterSettings, RectExt, TransformExt,
};
use kurbo::{Rect, Shape};

/// A loaded PDF document. Owns the file bytes; cheap to render pages from.
pub struct Document {
    pdf: Pdf,
    settings: InterpreterSettings,
}

/// One rendered page: its [`Scene`] plus the page's intrinsic size in PDF points
/// (the scene's logical-coordinate extent) and any [`Warnings`].
pub struct RenderedPage {
    /// The recorded scene, in page-point logical coordinates (origin top-left).
    pub scene: Scene,
    /// Page width in PDF points (1/72 inch).
    pub width: f32,
    /// Page height in PDF points.
    pub height: f32,
    /// Approximations made while interpreting this page.
    pub warnings: Warnings,
}

/// Loading / rendering failure.
#[derive(Debug)]
pub enum PdfError {
    /// The bytes could not be parsed as a PDF (corrupt, or encrypted — hayro
    /// does not support encrypted documents).
    Load,
    /// No page exists at the requested index.
    NoPage(usize),
}

impl std::fmt::Display for PdfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PdfError::Load => write!(f, "failed to parse PDF (corrupt or encrypted)"),
            PdfError::NoPage(i) => write!(f, "no page at index {i}"),
        }
    }
}

impl std::error::Error for PdfError {}

impl Document {
    /// Parse PDF bytes. The default [`InterpreterSettings`] bundles the
    /// standard-14 fallback fonts and predefined cmaps (the `embed-fonts` /
    /// `embed-cmaps` features), so non-embedded standard fonts still render.
    pub fn load(bytes: Vec<u8>) -> Result<Self, PdfError> {
        let pdf = Pdf::new(bytes).map_err(|_| PdfError::Load)?;
        Ok(Self { pdf, settings: InterpreterSettings::default() })
    }

    /// Number of pages.
    pub fn page_count(&self) -> usize {
        self.pdf.pages().len()
    }

    /// Interpret page `index` into a [`Scene`] at 1 unit = 1 PDF point.
    pub fn render_page(&self, index: usize) -> Result<RenderedPage, PdfError> {
        let pages = self.pdf.pages();
        let page = pages.get(index).ok_or(PdfError::NoPage(index))?;
        let (width, height) = page.render_dimensions();

        // The page's y-up space flipped to y-down logical pixels at scale 1 (the
        // canvas applies device-pixel-ratio + fit scaling later). Matches the
        // reference `render()` with x_scale = y_scale = 1.
        let initial = page.initial_transform(true).to_kurbo();

        let cache = InterpreterCache::new();
        let mut ctx = Context::new(
            initial,
            Rect::new(0.0, 0.0, width as f64, height as f64),
            &cache,
            page.xref(),
            self.settings.clone(),
        );

        let mut dev = SceneDevice::new();

        // Crop-box clip, pre-transformed to device space (mirrors `render()`).
        let mut clip = page.intersected_crop_box().to_kurbo().to_path(0.1);
        clip.apply_affine(initial);
        dev.push_clip_path(&ClipPath { path: clip, fill: FillRule::NonZero });

        // The root transparency group (always opaque/normal → splices flat).
        dev.push_transparency_group(1.0, None, BlendMode::Normal);
        interpret_page(page, &mut ctx, &mut dev);
        dev.pop_transparency_group();

        dev.pop_clip_path();

        let (scene, warnings) = dev.finish();
        Ok(RenderedPage { scene, width, height, warnings })
    }
}
