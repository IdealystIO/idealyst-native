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

use runtime_core::{FontStyle, FontWeight, TextAlign};
use glyphon::{
    cosmic_text::Align as GAlign, Attrs, Buffer, Cache, Color as GColor, Family, FontSystem,
    Metrics, Resolution, Shaping, Stretch as GStretch, Style as GStyle, SwashCache, TextArea,
    TextAtlas, TextBounds, TextRenderer as GRenderer, Viewport, Weight as GWeight,
};
use runtime_layout::LayoutNode;

/// Per-text-node state held in [`TextStore`].
///
/// `content` + `attrs` are duplicated alongside the shaped `buffer`
/// so style updates (font_size, font_family, font_weight,
/// font_style) can re-shape against the most recent text without
/// the backend having to hand us the content + attrs every time —
/// `apply_style` doesn't otherwise need the text content, and
/// stylesheets-only changes (theme swap, animated state overlay)
/// would have to re-route the buffer's text through the API
/// otherwise.
pub struct BufferEntry {
    pub buffer: Buffer,
    pub font_size: f32,
    pub content: String,
    pub attrs: TextAttrs,
}

/// Render-side projection of font attributes derived from the
/// stylesheet's `font_family` / `font_weight` / `font_style`. Cached
/// per-node so we can re-shape the buffer when font_size changes
/// without losing the family/weight/style picked at the previous
/// `apply_style`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextAttrs {
    /// Family name. `None` falls back to cosmic-text's
    /// `Family::SansSerif` — used when the author hasn't set
    /// `font_family` on the stylesheet. `Some(name)` matches by
    /// the family baked into the loaded font file (via
    /// `register_asset`).
    pub family: Option<String>,
    pub weight: FontWeight,
    pub style: FontStyle,
    /// Per-line text alignment. Stored here (rather than as a
    /// per-fragment x-offset at stage time) because cosmic-text
    /// aligns each `BufferLine` independently — multi-line text
    /// gets per-line centering within the buffer's width, which
    /// can't be reproduced by a single staging-side offset on
    /// the buffer as a whole.
    pub align: TextAlign,
}

impl TextAttrs {
    /// Construct a glyphon `Attrs` for shaping. The family slot is
    /// either `Family::Name(...)` (when an explicit family was
    /// resolved) or the generic `Family::SansSerif` fallback.
    pub fn to_glyphon<'a>(&'a self) -> Attrs<'a> {
        let family = match &self.family {
            Some(name) => Family::Name(name.as_str()),
            None => Family::SansSerif,
        };
        Attrs::new()
            .family(family)
            .weight(font_weight_to_glyphon(self.weight))
            .style(font_style_to_glyphon(self.style))
            .stretch(GStretch::Normal)
    }
}

fn font_weight_to_glyphon(w: FontWeight) -> GWeight {
    match w {
        FontWeight::Thin => GWeight::THIN,
        FontWeight::ExtraLight => GWeight::EXTRA_LIGHT,
        FontWeight::Light => GWeight::LIGHT,
        FontWeight::Normal => GWeight::NORMAL,
        FontWeight::Medium => GWeight::MEDIUM,
        FontWeight::SemiBold => GWeight::SEMIBOLD,
        FontWeight::Bold => GWeight::BOLD,
        FontWeight::ExtraBold => GWeight::EXTRA_BOLD,
        FontWeight::Black => GWeight::BLACK,
    }
}

fn font_style_to_glyphon(s: FontStyle) -> GStyle {
    match s {
        FontStyle::Normal => GStyle::Normal,
        FontStyle::Italic => GStyle::Italic,
    }
}

/// Translate the framework's `TextAlign` into cosmic-text's
/// per-line `Align`. `Left` returns `None` so cosmic-text picks
/// LTR-aware defaults (left for LTR, right for RTL); the other
/// variants pin to a specific direction.
fn text_align_to_glyphon(a: TextAlign) -> Option<GAlign> {
    match a {
        TextAlign::Left => None,
        TextAlign::Right => Some(GAlign::Right),
        TextAlign::Center => Some(GAlign::Center),
        TextAlign::Justify => Some(GAlign::Justified),
    }
}

/// Push the current `attrs.align` to every `BufferLine` so each
/// line aligns within the buffer's width. Cosmic-text aligns per
/// line, so multi-line text gets the right look without a
/// staging-side fudge. Called from every TextStore path that
/// changes either the text or the alignment.
fn apply_buffer_align(buffer: &mut Buffer, align: TextAlign) {
    let g = text_align_to_glyphon(align);
    for line in buffer.lines.iter_mut() {
        line.set_align(g);
    }
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
    /// Initial attrs are the default fallback (SansSerif, Normal,
    /// Normal); `apply_style` calls `set_attrs` immediately after
    /// to swap in the resolved family/weight/style.
    pub fn create(
        &mut self,
        font_system: &mut FontSystem,
        id: LayoutNode,
        content: &str,
        font_size: f32,
    ) {
        let attrs = TextAttrs::default();
        let mut buffer = Buffer::new(font_system, Metrics::new(font_size, font_size * 1.3));
        buffer.set_size(font_system, None, None);
        buffer.set_text(
            font_system,
            content,
            &attrs.to_glyphon(),
            Shaping::Advanced,
            None,
        );
        apply_buffer_align(&mut buffer, attrs.align);
        buffer.shape_until_scroll(font_system, false);
        self.buffers.insert(
            id,
            BufferEntry {
                buffer,
                font_size,
                content: content.to_string(),
                attrs,
            },
        );
    }

    /// Replace the text of `id`'s buffer. No-op if `id` isn't in
    /// the store (the node was dropped before this update fired).
    pub fn set_text(&mut self, font_system: &mut FontSystem, id: LayoutNode, content: &str) {
        if let Some(entry) = self.buffers.get_mut(&id) {
            entry.content = content.to_string();
            entry.buffer.set_text(
                font_system,
                content,
                &entry.attrs.to_glyphon(),
                Shaping::Advanced,
                None,
            );
            // set_text resets the lines' alignment to None, so
            // re-stamp it here. Otherwise a text update would
            // silently revert centered headlines to left-aligned.
            apply_buffer_align(&mut entry.buffer, entry.attrs.align);
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

    /// Re-shape `id`'s buffer with new font attributes (family /
    /// weight / style). The text content + size are preserved.
    /// Called from `apply_style` after the stylesheet resolves —
    /// stylesheet-only changes (theme swap, state overlay flip)
    /// re-shape through this without needing to re-issue the
    /// `create_text` content payload.
    pub fn set_attrs(
        &mut self,
        font_system: &mut FontSystem,
        id: LayoutNode,
        attrs: TextAttrs,
    ) {
        if let Some(entry) = self.buffers.get_mut(&id) {
            if entry.attrs == attrs {
                return;
            }
            let family_or_style_changed = entry.attrs.family != attrs.family
                || entry.attrs.weight != attrs.weight
                || entry.attrs.style != attrs.style;
            entry.attrs = attrs;
            if family_or_style_changed {
                // Font family / weight / style affect glyph
                // selection — re-shape the text against the new
                // attrs. Re-stamp alignment afterwards (set_text
                // resets it).
                entry.buffer.set_text(
                    font_system,
                    &entry.content,
                    &entry.attrs.to_glyphon(),
                    Shaping::Advanced,
                    None,
                );
            }
            // Alignment is a per-line property — re-stamp it
            // whether or not the text was re-shaped above.
            apply_buffer_align(&mut entry.buffer, entry.attrs.align);
            entry.buffer.shape_until_scroll(font_system, false);
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
