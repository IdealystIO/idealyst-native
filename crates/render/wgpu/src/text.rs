//! Text rendering via glyphon (cosmic-text + wgpu glyph atlas).
//!
//! Text state is split across two `Rc<RefCell<>>`s that the backend
//! and renderer share:
//!
//! - [`FontSystem`] (cosmic-text's font DB + shaper). Owned via
//!   `Rc<RefCell<FontSystem>>` so the framework-side `create_text`
//!   path (running inside `Backend` methods) and the GPU-side
//!   `render_text` (running inside the wgpu render pass) can both
//!   reach it.
//! - [`TextStore`] — the `HashMap<LayoutNode, BufferEntry>` of
//!   per-node buffers. Owned via `Rc<RefCell<TextStore>>` for the
//!   same reason.
//!
//! Splitting these two stores out of `WgpuBackend` lets the Taffy
//! measure closure capture `Weak` handles and `borrow_mut` them
//! safely. The earlier raw-pointer dance is gone.

use std::collections::HashMap;

use glyphon::{
    Attrs, Buffer, Cache, Color as GColor, Family, FontSystem, Metrics, Resolution, Shaping,
    SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer as GRenderer, Viewport,
};
use native_layout::LayoutNode;

/// Per-text-node state held in [`TextStore`].
pub struct BufferEntry {
    pub buffer: Buffer,
    pub font_size: f32,
}

/// Shared text-buffer store. Both the `Backend` impl (writer) and
/// the renderer + measure-fn closures (readers) hold an
/// `Rc<RefCell<TextStore>>` to this.
pub struct TextStore {
    pub buffers: HashMap<LayoutNode, BufferEntry>,
}

impl TextStore {
    pub fn new() -> Self {
        Self { buffers: HashMap::new() }
    }

    /// Build a new buffer for `id` with `content` at `font_size`,
    /// shaped against `font_system`. Replaces any existing entry.
    pub fn create(
        &mut self,
        font_system: &mut FontSystem,
        id: LayoutNode,
        content: &str,
        font_size: f32,
    ) {
        let mut buffer = Buffer::new(font_system, Metrics::new(font_size, font_size * 1.3));
        buffer.set_size(font_system, None, None);
        buffer.set_text(
            font_system,
            content,
            &Attrs::new().family(Family::SansSerif),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(font_system, false);
        self.buffers.insert(id, BufferEntry { buffer, font_size });
    }

    /// Replace the text of `id`'s buffer. No-op if `id` isn't in
    /// the store (the node was dropped before this update fired).
    pub fn set_text(&mut self, font_system: &mut FontSystem, id: LayoutNode, content: &str) {
        if let Some(entry) = self.buffers.get_mut(&id) {
            entry.buffer.set_text(
                font_system,
                content,
                &Attrs::new().family(Family::SansSerif),
                Shaping::Advanced,
                None,
            );
            entry.buffer.shape_until_scroll(font_system, false);
        }
    }

    /// Reset the metrics on `id`'s buffer. Called when the framework
    /// re-applies a style that changed `font_size`.
    pub fn set_font_size(
        &mut self,
        font_system: &mut FontSystem,
        id: LayoutNode,
        font_size: f32,
    ) {
        if let Some(entry) = self.buffers.get_mut(&id) {
            entry.buffer.set_metrics(
                font_system,
                Metrics::new(font_size, font_size * 1.3),
            );
            entry.font_size = font_size;
        }
    }

    /// Drop a buffer (called from `clear_children` /
    /// `on_node_unstyled` so the store doesn't leak entries past
    /// the node's lifetime).
    pub fn remove(&mut self, id: LayoutNode) {
        self.buffers.remove(&id);
    }

    /// Re-shape `id`'s buffer against a width constraint and return
    /// the wrapped extent. Used by the Taffy measure closure.
    pub fn measure(
        &mut self,
        font_system: &mut FontSystem,
        id: LayoutNode,
        max_width: Option<f32>,
    ) -> (f32, f32) {
        let Some(entry) = self.buffers.get_mut(&id) else {
            return (0.0, 0.0);
        };
        entry.buffer.set_size(font_system, max_width, None);
        entry.buffer.shape_until_scroll(font_system, false);
        let mut w: f32 = 0.0;
        let mut h: f32 = 0.0;
        for run in entry.buffer.layout_runs() {
            w = w.max(run.line_w);
            h = h.max(run.line_top + run.line_height);
        }
        (w.ceil(), h.ceil())
    }
}

/// GPU-side text infrastructure: the atlas, the wgpu pipeline, and
/// the per-draw swash cache. Separate from [`TextStore`] because
/// it's GPU-bound and only exists once the surface is up.
pub struct TextCtx {
    pub swash_cache: SwashCache,
    // Held for the same reason the wgpu Gpu retains its instance:
    // glyphon's Atlas + Viewport borrow internally from `cache`.
    #[allow(dead_code)]
    pub cache: Cache,
    pub atlas: TextAtlas,
    pub viewport: Viewport,
    pub renderer: GRenderer,
}

impl TextCtx {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Self {
        let swash_cache = SwashCache::new();
        let cache = Cache::new(device);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let viewport = Viewport::new(device, &cache);
        let renderer =
            GRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);

        Self { swash_cache, cache, atlas, viewport, renderer }
    }
}

/// One staged text draw — built per-frame by the renderer walker.
pub struct StagedText<'a> {
    pub buffer: &'a Buffer,
    pub x: f32,
    pub y: f32,
    pub color: [f32; 4],
    pub clip: TextBounds,
}

/// Errors from a single text-render pass. We unify glyphon's
/// distinct `PrepareError` / `RenderError` into one type so callers
/// can use a single `?` path.
#[derive(Debug)]
pub enum TextRenderError {
    Prepare(#[allow(dead_code)] glyphon::PrepareError),
    Render(#[allow(dead_code)] glyphon::RenderError),
}

impl From<glyphon::PrepareError> for TextRenderError {
    fn from(e: glyphon::PrepareError) -> Self { TextRenderError::Prepare(e) }
}

impl From<glyphon::RenderError> for TextRenderError {
    fn from(e: glyphon::RenderError) -> Self { TextRenderError::Render(e) }
}

pub fn render_text<'a>(
    ctx: &'a mut TextCtx,
    font_system: &mut FontSystem,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pass: &mut wgpu::RenderPass<'a>,
    viewport_px: [u32; 2],
    items: &[StagedText<'a>],
) -> Result<(), TextRenderError> {
    ctx.viewport.update(
        queue,
        Resolution { width: viewport_px[0], height: viewport_px[1] },
    );

    let areas: Vec<TextArea<'_>> = items
        .iter()
        .map(|item| TextArea {
            buffer: item.buffer,
            left: item.x,
            top: item.y,
            scale: 1.0,
            bounds: item.clip,
            default_color: GColor::rgba(
                (item.color[0] * 255.0) as u8,
                (item.color[1] * 255.0) as u8,
                (item.color[2] * 255.0) as u8,
                (item.color[3] * 255.0) as u8,
            ),
            custom_glyphs: &[],
        })
        .collect();

    ctx.renderer.prepare(
        device,
        queue,
        font_system,
        &mut ctx.atlas,
        &ctx.viewport,
        areas,
        &mut ctx.swash_cache,
    )?;
    ctx.renderer.render(&ctx.atlas, &ctx.viewport, pass)?;
    Ok(())
}
