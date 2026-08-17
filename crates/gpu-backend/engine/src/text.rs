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
    cosmic_text::Align as GAlign, Attrs, Buffer, Cache, Color as GColor, Family, FontSystem,
    Metrics, Resolution, Shaping, Stretch as GStretch, Style as GStyle, SwashCache, TextArea,
    TextAtlas, TextBounds, TextRenderer as GRenderer, Viewport, Weight as GWeight, Wrap,
};
use runtime_layout::LayoutNode;
use runtime_shared::{FontStyle, FontWeight, TextAlign};

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
    /// `Some` for styled-text nodes (`Backend::create_styled_text`):
    /// the resolved per-run deltas, kept so every re-shape path
    /// (`set_attrs` base change, theme re-realize) rebuilds the buffer
    /// via `set_rich_text` instead of flattening to plain text. The
    /// span index is stamped into each glyph's `metadata`, which is
    /// how the renderer finds run background rects at stage time.
    pub rich: Option<Vec<RichSpan>>,
}

/// One resolved styled-run for the GPU text engine. Colors are
/// resolved sRGB at realize time (theme swaps re-realize through
/// `Backend::update_styled_text`).
#[derive(Clone, Debug, Default)]
pub struct RichSpan {
    pub text: String,
    pub family: Option<RichFamily>,
    pub weight: Option<FontWeight>,
    pub size: Option<f32>,
    /// Foreground, resolved sRGB 0-255.
    pub color: Option<[u8; 4]>,
    /// Background, resolved sRGB 0.0-1.0 — painted by the renderer as
    /// a rect behind the span's glyphs (cosmic-text has no background
    /// attribute).
    pub background: Option<[f32; 4]>,
    /// Italic delta for this run.
    pub style: Option<runtime_shared::FontStyle>,
    /// Underline, painted by the renderer as rect(s) below the
    /// baseline off the same glyph geometry the background chips use
    /// — cosmic-text has no underline attribute either. `None` in the
    /// colour means "the run's own foreground".
    pub underline: Option<(runtime_shared::UnderlineStyle, Option<[f32; 4]>)>,
}

/// Family choice for one rich span. First classifiable entry of the
/// author's font stack wins: generics map to cosmic-text's built-in
/// roles, a leading named face passes through (cosmic-text falls back
/// per-glyph if the name isn't loaded).
#[derive(Clone, Debug)]
pub enum RichFamily {
    Monospace,
    SansSerif,
    Serif,
    Named(String),
}

impl RichFamily {
    fn to_glyphon<'a>(&'a self) -> Family<'a> {
        match self {
            RichFamily::Monospace => Family::Monospace,
            RichFamily::SansSerif => Family::SansSerif,
            RichFamily::Serif => Family::Serif,
            RichFamily::Named(n) => Family::Name(n.as_str()),
        }
    }
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
        Self {
            buffers: HashMap::new(),
        }
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
                rich: None,
            },
        );
    }

    /// Build a new RICH buffer for `id`: one shaped paragraph whose
    /// spans carry per-run family/weight/size/color deltas over the
    /// node's base attrs. Replaces any existing entry. Mirrors
    /// [`TextStore::create`]; `apply_style` swaps the base attrs in
    /// right after via `set_attrs`, which re-shapes rich-aware.
    pub fn create_rich(
        &mut self,
        font_system: &mut FontSystem,
        id: LayoutNode,
        spans: Vec<RichSpan>,
        font_size: f32,
    ) {
        let attrs = TextAttrs::default();
        let mut buffer = Buffer::new(font_system, Metrics::new(font_size, font_size * 1.3));
        buffer.set_size(font_system, None, None);
        shape_rich(&mut buffer, font_system, &spans, &attrs, font_size);
        apply_buffer_align(&mut buffer, attrs.align);
        buffer.shape_until_scroll(font_system, false);
        let content: String = spans.iter().map(|sp| sp.text.as_str()).collect();
        self.buffers.insert(
            id,
            BufferEntry {
                buffer,
                font_size,
                content,
                attrs,
                rich: Some(spans),
            },
        );
    }

    /// Replace a rich buffer's spans (theme re-realize / direct
    /// update). No-op for ids that were never rich-created — a plain
    /// text node's content updates route through `set_text`.
    pub fn set_rich(&mut self, font_system: &mut FontSystem, id: LayoutNode, spans: Vec<RichSpan>) {
        if let Some(entry) = self.buffers.get_mut(&id) {
            entry.content = spans.iter().map(|sp| sp.text.as_str()).collect();
            shape_rich(
                &mut entry.buffer,
                font_system,
                &spans,
                &entry.attrs,
                entry.font_size,
            );
            entry.rich = Some(spans);
            apply_buffer_align(&mut entry.buffer, entry.attrs.align);
            entry.buffer.shape_until_scroll(font_system, false);
        }
    }

    /// Replace the text of `id`'s buffer. No-op if `id` isn't in
    /// the store (the node was dropped before this update fired).
    pub fn set_text(&mut self, font_system: &mut FontSystem, id: LayoutNode, content: &str) {
        if let Some(entry) = self.buffers.get_mut(&id) {
            // A plain-text write onto a rich node replaces the runs
            // wholesale (robot `set_text`, etc.) — matching the DOM
            // backend, where `set_text_content` drops the run spans.
            entry.rich = None;
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
    pub fn set_font_size(&mut self, font_system: &mut FontSystem, id: LayoutNode, font_size: f32) {
        if let Some(entry) = self.buffers.get_mut(&id) {
            entry
                .buffer
                .set_metrics(font_system, Metrics::new(font_size, font_size * 1.3));
            entry.font_size = font_size;
        }
    }

    /// Re-shape `id`'s buffer with new font attributes (family /
    /// weight / style). The text content + size are preserved.
    /// Called from `apply_style` after the stylesheet resolves —
    /// stylesheet-only changes (theme swap, state overlay flip)
    /// re-shape through this without needing to re-issue the
    /// `create_text` content payload.
    pub fn set_attrs(&mut self, font_system: &mut FontSystem, id: LayoutNode, attrs: TextAttrs) {
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
                // resets it). Rich buffers re-shape through their
                // spans so run deltas survive a base-style change.
                if let Some(spans) = entry.rich.take() {
                    shape_rich(
                        &mut entry.buffer,
                        font_system,
                        &spans,
                        &entry.attrs,
                        entry.font_size,
                    );
                    entry.rich = Some(spans);
                } else {
                    entry.buffer.set_text(
                        font_system,
                        &entry.content,
                        &entry.attrs.to_glyphon(),
                        Shaping::Advanced,
                        None,
                    );
                }
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
    /// Measure the text wrapped to `max_width` (`None` = unbounded, a
    /// single line). This is the **max-content** / definite-width path:
    /// Taffy's resolved or unconstrained width. Wrap stays `WordOrGlyph`
    /// (long words break rather than overflow), matching the render pass.
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

    /// **Min-content** width: the widest the text could need if it
    /// wrapped at every legal break — i.e. the longest single word (the
    /// longest unbreakable run). This is the value CSS uses as a flex/grid
    /// item's automatic minimum, so Taffy knows the text can't shrink past
    /// it. Returning max-content here instead (the old `_ => None` bug) hid
    /// the real floor: with `min-width:0` items the layout then shrank the
    /// text to width 0, forcing per-glyph wrapping (the tall 1-char
    /// columns).
    ///
    /// Probe with `Wrap::Word` at width 0 so a word that can't fit *stays*
    /// on its line (overflows) rather than breaking to glyphs — the
    /// default `WordOrGlyph` would report ~1 glyph, defeating the floor.
    /// Wrap is restored to `WordOrGlyph` afterward; the next measure call
    /// (Taffy's final resolved-width pass) re-shapes the buffer for the
    /// render, so this leaves no lasting state.
    pub fn measure_min_content(
        &mut self,
        font_system: &mut FontSystem,
        id: LayoutNode,
    ) -> (f32, f32) {
        let Some(entry) = self.buffers.get_mut(&id) else {
            return (0.0, 0.0);
        };
        entry.buffer.set_wrap(font_system, Wrap::Word);
        entry.buffer.set_size(font_system, Some(0.0), None);
        entry.buffer.shape_until_scroll(font_system, false);
        let mut w: f32 = 0.0;
        let mut line_h: f32 = entry.font_size * 1.3;
        for run in entry.buffer.layout_runs() {
            w = w.max(run.line_w);
            line_h = run.line_height;
        }
        // Restore the buffer to the render-time wrap mode AND re-shape it
        // unbounded, so a min-content probe never leaves the buffer shaped
        // at width 0 (one glyph per line). Taffy is free to call this as a
        // node's LAST measure; the renderer draws whatever the buffer was
        // last shaped to, so a collapsed leftover would render as a single
        // glyph. Re-shaping here makes the probe state-neutral.
        entry.buffer.set_wrap(font_system, Wrap::WordOrGlyph);
        entry.buffer.set_size(font_system, None, None);
        entry.buffer.shape_until_scroll(font_system, false);
        // Min-content height is one line tall (the cross-axis size is
        // recomputed once the real width is known).
        (w.ceil(), line_h.ceil())
    }
}

/// Resolve a framework `TextRun` list into engine [`RichSpan`]s:
/// tokenized colors resolve against the CURRENT theme (which is why
/// theme swaps re-enter here via `Backend::update_styled_text`),
/// families classify to cosmic-text roles, sizes flatten to px.
pub fn resolve_rich_spans(runs: &[runtime_shared::TextRun]) -> Vec<RichSpan> {
    runs.iter()
        .map(|run| {
            let style = run.style.as_ref();
            let family = style
                .and_then(|s| s.font_family.as_ref())
                .and_then(RichFamily::classify);
            let color = style.and_then(|s| s.color.as_ref()).map(|t| {
                let [r, g, b, a] = crate::style_convert::parse_color(&t.resolve());
                [
                    (r * 255.0) as u8,
                    (g * 255.0) as u8,
                    (b * 255.0) as u8,
                    (a * 255.0) as u8,
                ]
            });
            let background = style
                .and_then(|s| s.background.as_ref())
                .map(|t| crate::style_convert::parse_color(&t.resolve()));
            let size = style
                .and_then(|s| s.font_size.as_ref())
                .and_then(|t| match t.resolve() {
                    runtime_shared::Length::Px(px) if px > 0.0 => Some(px),
                    _ => None,
                });
            let underline = style.and_then(|s| s.underline.as_ref()).map(|u| {
                (
                    u.style,
                    u.color
                        .as_ref()
                        .map(|t| crate::style_convert::parse_color(&t.resolve())),
                )
            });
            RichSpan {
                text: run.text.clone(),
                family,
                weight: style.and_then(|s| s.font_weight),
                size,
                color,
                background,
                style: style.and_then(|s| s.font_style),
                underline,
            }
        })
        .collect()
}

impl RichFamily {
    /// First classifiable entry of the stack wins. `Typeface` custom
    /// fonts pass their family name through — cosmic-text matches it
    /// against faces loaded via `register_asset` and falls back
    /// per-glyph otherwise.
    pub fn classify(f: &runtime_shared::FontFamily) -> Option<RichFamily> {
        match f {
            runtime_shared::FontFamily::Typeface(tf) => {
                Some(RichFamily::Named(tf.family_name.to_string()))
            }
            runtime_shared::FontFamily::System(stack) => {
                for entry in stack.split(',') {
                    let entry = entry.trim().trim_matches('"').trim_matches('\'');
                    if entry.is_empty() {
                        continue;
                    }
                    return Some(match entry.to_ascii_lowercase().as_str() {
                        "monospace" | "ui-monospace" => RichFamily::Monospace,
                        "sans-serif" | "system-ui" | "ui-sans-serif" => RichFamily::SansSerif,
                        "serif" | "ui-serif" => RichFamily::Serif,
                        _ => RichFamily::Named(entry.to_string()),
                    });
                }
                None
            }
        }
    }
}

/// Shape a rich buffer: base attrs for plain spans, deltas layered
/// for styled ones. The span INDEX rides each glyph's `metadata` so
/// the renderer can find run boundaries (background rects) in the
/// laid-out glyphs.
fn shape_rich(
    buffer: &mut Buffer,
    font_system: &mut FontSystem,
    spans: &[RichSpan],
    base: &TextAttrs,
    base_size: f32,
) {
    let base_attrs = base.to_glyphon();
    let glyphon_spans: Vec<(&str, Attrs<'_>)> = spans
        .iter()
        .enumerate()
        .map(|(i, sp)| {
            let mut a = base.to_glyphon().metadata(i);
            if let Some(f) = &sp.family {
                a = a.family(f.to_glyphon());
            }
            if let Some(w) = sp.weight {
                a = a.weight(font_weight_to_glyphon(w));
            }
            if let Some(s) = sp.style {
                a = a.style(font_style_to_glyphon(s));
            }
            if let Some(px) = sp.size {
                a = a.metrics(Metrics::new(px, px * 1.3));
            } else if sp.family.is_some() || sp.weight.is_some() {
                // A font-delta span keeps the node's size explicitly —
                // per-span metrics also pin the line height so a mono
                // chip doesn't stretch its line.
                a = a.metrics(Metrics::new(base_size, base_size * 1.3));
            }
            if let Some([r, g, b, alpha]) = sp.color {
                a = a.color(GColor::rgba(r, g, b, alpha));
            }
            (sp.text.as_str(), a)
        })
        .collect();
    buffer.set_rich_text(
        font_system,
        glyphon_spans,
        &base_attrs,
        Shaping::Advanced,
        None,
    );
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
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let swash_cache = SwashCache::new();
        let cache = Cache::new(device);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let viewport = Viewport::new(device, &cache);
        let renderer = GRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);

        Self {
            swash_cache,
            cache,
            atlas,
            viewport,
            renderer,
        }
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
    fn from(e: glyphon::PrepareError) -> Self {
        TextRenderError::Prepare(e)
    }
}

impl From<glyphon::RenderError> for TextRenderError {
    fn from(e: glyphon::RenderError) -> Self {
        TextRenderError::Render(e)
    }
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
        Resolution {
            width: viewport_px[0],
            height: viewport_px[1],
        },
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

#[cfg(test)]
mod tests {
    use super::*;

    fn layout_node() -> LayoutNode {
        runtime_layout::LayoutTree::new().new_node()
    }

    /// Rich creation shapes the full concatenated paragraph and stamps
    /// each glyph's `metadata` with its span index — the invariant the
    /// renderer's background-rect pass depends on. Headless: cosmic's
    /// FontSystem shapes without a GPU.
    #[test]
    fn create_rich_stamps_span_metadata_on_glyphs() {
        let mut fs = FontSystem::new();
        let mut store = TextStore::new();
        let id = layout_node();
        store.create_rich(
            &mut fs,
            id,
            vec![
                RichSpan {
                    text: "the ".into(),
                    ..Default::default()
                },
                RichSpan {
                    text: "ui!".into(),
                    family: Some(RichFamily::Monospace),
                    background: Some([0.9, 0.9, 0.9, 1.0]),
                    ..Default::default()
                },
                RichSpan {
                    text: " macro".into(),
                    ..Default::default()
                },
            ],
            14.0,
        );
        let entry = store.buffers.get(&id).expect("entry created");
        assert_eq!(entry.content, "the ui! macro");
        assert!(entry.rich.is_some());
        let metas: Vec<usize> = entry
            .buffer
            .layout_runs()
            .flat_map(|r| r.glyphs.iter().map(|g| g.metadata).collect::<Vec<_>>())
            .collect();
        // 13 glyphs, spans 0 (4 glyphs), 1 (3 glyphs), 2 (6 glyphs).
        assert_eq!(metas.len(), 13, "one glyph per char: {metas:?}");
        assert_eq!(&metas[0..4], &[0, 0, 0, 0]);
        assert_eq!(&metas[4..7], &[1, 1, 1]);
        assert_eq!(&metas[7..13], &[2, 2, 2, 2, 2, 2]);
    }

    /// A plain `set_text` onto a rich node drops the runs (robot
    /// set_text parity with the DOM backend), and a base-attrs change
    /// re-shapes THROUGH the stored spans so run deltas survive.
    #[test]
    fn set_text_drops_rich_and_set_attrs_preserves_it() {
        let mut fs = FontSystem::new();
        let mut store = TextStore::new();
        let id = layout_node();
        store.create_rich(
            &mut fs,
            id,
            vec![RichSpan {
                text: "chip".into(),
                family: Some(RichFamily::Monospace),
                ..Default::default()
            }],
            14.0,
        );
        // Base-attrs change keeps the rich spans.
        store.set_attrs(
            &mut fs,
            id,
            TextAttrs {
                weight: FontWeight::Bold,
                ..Default::default()
            },
        );
        assert!(store.buffers.get(&id).unwrap().rich.is_some());
        // Plain-text write drops them.
        store.set_text(&mut fs, id, "plain");
        let entry = store.buffers.get(&id).unwrap();
        assert!(entry.rich.is_none());
        assert_eq!(entry.content, "plain");
    }

    /// `resolve_rich_spans` maps framework runs into engine spans:
    /// tokenized colors resolve to sRGB, the family stack classifies
    /// via its first entry, sizes flatten to px.
    #[test]
    fn resolve_rich_spans_resolves_colors_and_families() {
        use runtime_shared::{Color, TextRun, TextRunStyle, Tokenized};
        let spans = resolve_rich_spans(&[
            TextRun::plain("a "),
            TextRun::styled(
                "b",
                TextRunStyle {
                    font_family: Some(runtime_shared::FontFamily::System(
                        "ui-monospace, Menlo, monospace".into(),
                    )),
                    background: Some(Tokenized::Literal(Color("#ff0000".into()))),
                    ..Default::default()
                },
            ),
        ]);
        assert_eq!(spans.len(), 2);
        assert!(spans[0].family.is_none() && spans[0].background.is_none());
        assert!(matches!(spans[1].family, Some(RichFamily::Monospace)));
        let bg = spans[1].background.unwrap();
        assert!(
            bg[0] > 0.99 && bg[1] < 0.01 && bg[2] < 0.01,
            "red bg, got {bg:?}"
        );
    }
}
