//! Wgpu-bound render dispatch. Platform-agnostic (no winit, no
//! browser deps) — anything in here can be reused by the native
//! shell, a future web shell, or any other host that owns a wgpu
//! device + surface.
//!
//! Owns the per-pipeline GPU resources (the rect pipeline and the
//! glyphon text context). The shell creates the [`Renderer`] once
//! the wgpu device is up and drives [`Renderer::render`] per
//! frame against its surface texture.
//!
//! # Coordinate space
//!
//! Everything in this module operates in **logical** CSS pixels —
//! the same space Taffy lays out in and that the framework's
//! styling uses. The shell tells us the logical viewport size on
//! each call; the wgpu surface stays configured in physical px
//! (so HiDPI rasterizes at the right resolution), but the shader's
//! NDC mapping uses the logical viewport so geometry coordinates
//! line up with hit-test coordinates.

use std::rc::Rc;
// `web-time` for wasm32 compat — see `host.rs` for the rationale.
use web_time::Instant;

use glyphon::{Buffer, TextBounds};
use native_layout::LayoutNode;

use crate::animation::{AnimProperty, TweenKey};
use crate::backend_impl::WgpuBackend;
use crate::host::{scrollview_content_extent, Host};
use crate::keyboard;
use crate::node::{
    NodeKind, WgpuNode, ACTIVITY_INDICATOR_SPIN_PERIOD_SEC, CARET_BLINK_PERIOD_SEC,
    SCROLLBAR_INSET, SCROLLBAR_MIN_THUMB, SCROLLBAR_WIDTH,
};
use crate::pipeline::{Instance as RectInstance, RectPipeline};
use crate::style_convert::srgb_rgba_to_linear;
use crate::image_pipeline::{ImageDraw, ImageInstance, ImagePipeline};
use crate::text::{render_text, StagedText, TextCtx, TextStore};

/// GPU-side rendering bundle. Holds the rect pipeline + the
/// glyphon text context + the textured-quad pipeline + per-src
/// image cache. One per surface.
pub struct Renderer {
    pub rect: RectPipeline,
    pub text: TextCtx,
    pub image: ImagePipeline,
    /// Paints opaque black outside the rounded display path.
    /// Runs last in the overlay submit so it composites over
    /// everything else (app, drawer, modals, chrome) — gives
    /// the simulator its "device sitting on a black table"
    /// look without needing per-skin corner-mask rects.
    pub device_frame: crate::device_frame_pipeline::DeviceFramePipeline,
    /// Decoded + uploaded image textures, keyed by the author's
    /// `src` string. Populated lazily on first encounter — load
    /// failures cache as `Failed` so we don't retry every frame.
    image_cache: std::collections::HashMap<String, ImageCacheState>,
    /// Per-`Graphics`-node offscreen render targets. Keyed by the
    /// node's Taffy id (stable across remounts of the same Rc).
    /// Allocated lazily on first encounter at the node's current
    /// pixel size; re-allocated when that size changes by more
    /// than one pixel on either axis. Entries are not currently
    /// evicted — Graphics nodes are rare and long-lived; a future
    /// eviction pass keyed on `Backend::drop_subtree` would close
    /// the leak when the framework supports release_graphics.
    graphics_cache: std::collections::HashMap<
        native_layout::LayoutNode,
        GraphicsTextureEntry,
    >,
    /// Per-`Video`-node GPU texture state. Keyed by Taffy id.
    /// Allocated lazily once the decoder has produced its first
    /// frame (we don't know the source frame size before then);
    /// re-allocated if a later frame's size differs (e.g.
    /// resolution change on a stream — unusual but defensive).
    video_cache: std::collections::HashMap<
        native_layout::LayoutNode,
        VideoTextureEntry,
    >,
}

/// GPU resources backing one `NodeKind::Graphics` node:
/// the offscreen texture, a default 2D view over it, and the
/// pre-built bind group the image pipeline samples it with.
struct GraphicsTextureEntry {
    #[allow(dead_code)]
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    size: (u32, u32),
}

/// GPU resources backing one `NodeKind::Video` node. The shape
/// mirrors `GraphicsTextureEntry`; difference is how the texture
/// gets populated — `queue.write_texture` from a decoder-produced
/// RGBA buffer instead of a `RenderPass` from a user drawer.
struct VideoTextureEntry {
    #[allow(dead_code)]
    texture: wgpu::Texture,
    #[allow(dead_code)]
    view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    size: (u32, u32),
}

/// One cache entry per `Image` `src`. Holds the GPU texture +
/// the bind group the pipeline binds at draw time. The view
/// and texture are stashed so we can rebuild the bind group
/// if the layout ever changes.
pub struct ImageEntry {
    #[allow(dead_code)]
    pub texture: wgpu::Texture,
    #[allow(dead_code)]
    pub view: wgpu::TextureView,
    pub bind_group: wgpu::BindGroup,
    pub size: (u32, u32),
}

/// Cache state for an image src. `Loaded` carries the entry;
/// `Failed` marks "we tried and it didn't decode" so we don't
/// retry the file system every frame.
enum ImageCacheState {
    Loaded(ImageEntry),
    Failed,
}

/// One image draw recorded during the tree walk. `src` is
/// resolved against the renderer's image cache in the pre-render
/// step — successes turn into [`crate::image_pipeline::ImageDraw`]s
/// for the textured-quad batch; failures fall back to the
/// missing-image placeholder rect.
pub struct ImageRequest {
    pub src: String,
    pub alt: Option<String>,
    /// Screen-space rect in logical px: `(x, y, w, h)`.
    pub rect: (f32, f32, f32, f32),
    pub opacity: f32,
}

/// One Graphics node recorded during the tree walk. Resolved
/// in the renderer's pre-pass: ensure the node's offscreen
/// texture exists at the right size, then invoke the user's
/// drawer to encode draw calls into it. The main UI pass then
/// composites the resulting texture as a textured quad through
/// the image pipeline.
pub struct GraphicsRequest {
    /// User's node Rc — needed both as the cache key for the
    /// offscreen texture (via `node.borrow().layout`) and to
    /// reach the registered drawer closure inside the
    /// `NodeKind::Graphics` variant.
    pub node: crate::node::WgpuNode,
    /// Screen-space rect in logical px: `(x, y, w, h)`. The
    /// composite stage paints into this rect; the offscreen
    /// texture is sized to `(w, h)` in physical px (rounded
    /// up to at least 1).
    pub rect: (f32, f32, f32, f32),
    /// Composited alpha multiplier (from the node's style).
    pub opacity: f32,
}

/// One Video node recorded during the tree walk. Resolved in
/// the renderer's pre-pass: if the decoder thread has a new
/// frame, allocate / re-allocate the per-node texture to the
/// decoder's frame size and upload via `queue.write_texture`.
/// The main UI pass composites the texture as a textured quad
/// through the image pipeline — same path Graphics uses.
pub struct VideoRequest {
    pub node: crate::node::WgpuNode,
    pub rect: (f32, f32, f32, f32),
    pub opacity: f32,
}

impl Renderer {
    /// Create the renderer's GPU resources. `format` must match
    /// the surface's color format; the rect + image pipelines
    /// are created against it.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Self {
        Self {
            rect: RectPipeline::new(device, format),
            text: TextCtx::new(device, queue, format),
            image: ImagePipeline::new(device, format),
            device_frame: crate::device_frame_pipeline::DeviceFramePipeline::new(
                device, format,
            ),
            image_cache: std::collections::HashMap::new(),
            graphics_cache: std::collections::HashMap::new(),
            video_cache: std::collections::HashMap::new(),
        }
    }

    /// Ensure a `VideoTextureEntry` exists for `layout` sized to
    /// `(w, h)`. Allocates on first call; re-allocates when the
    /// stored size differs from the requested one (decoder
    /// resolution change). Returns `None` for zero-size inputs.
    fn ensure_video_texture(
        &mut self,
        device: &wgpu::Device,
        layout: native_layout::LayoutNode,
        w: u32,
        h: u32,
    ) -> Option<&VideoTextureEntry> {
        if w == 0 || h == 0 {
            return None;
        }
        let needs_alloc = self
            .video_cache
            .get(&layout)
            .map(|e| e.size != (w, h))
            .unwrap_or(true);
        if needs_alloc {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("video-frame"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                // The decoder writes straight sRGB-encoded RGBA
                // (BT.601 YUV→RGB conversion lands in display-
                // referred sRGB). Tagging the texture as sRGB
                // makes the image-pipeline sampler decode back
                // to linear at sample time, matching the rest
                // of the renderer's color math.
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("video-frame-bg"),
                layout: &self.image.texture_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.image.sampler),
                    },
                ],
            });
            self.video_cache.insert(
                layout,
                VideoTextureEntry { texture, view, bind_group, size: (w, h) },
            );
        }
        self.video_cache.get(&layout)
    }

    /// Ensure the cache holds a `GraphicsTextureEntry` for `layout`
    /// sized to `(w, h)` physical px. Reallocates if the cached
    /// size doesn't match (resize via parent layout). Returns
    /// `None` when `w == 0 || h == 0` — the caller should skip
    /// running the drawer in that case (no drawable surface).
    fn ensure_graphics_texture(
        &mut self,
        device: &wgpu::Device,
        layout: native_layout::LayoutNode,
        w: u32,
        h: u32,
    ) -> Option<&GraphicsTextureEntry> {
        if w == 0 || h == 0 {
            return None;
        }
        let needs_alloc = self
            .graphics_cache
            .get(&layout)
            .map(|e| e.size != (w, h))
            .unwrap_or(true);
        if needs_alloc {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("graphics-target"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                // sRGB so the image pipeline's sampler decodes
                // back to linear (matches the image-loading path
                // at `decode_and_upload`). Authors write straight
                // linear color from their fragment shader; the
                // GPU encodes to sRGB at attachment-write time.
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("graphics-target-bg"),
                layout: &self.image.texture_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.image.sampler),
                    },
                ],
            });
            self.graphics_cache.insert(
                layout,
                GraphicsTextureEntry { texture, view, bind_group, size: (w, h) },
            );
        }
        self.graphics_cache.get(&layout)
    }

    /// Look up an image src in the cache, loading + uploading
    /// it on miss. Returns the cached entry on success, `None`
    /// on decode/IO failure (so the caller falls back to the
    /// placeholder paint). Failures are remembered as
    /// `ImageCacheState::Failed`, so a broken `src` doesn't
    /// hit the file system every frame.
    fn get_or_load_image(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        src: &str,
    ) -> Option<&ImageEntry> {
        if !self.image_cache.contains_key(src) {
            let state = match decode_and_upload(device, queue, &self.image, src) {
                Some(entry) => ImageCacheState::Loaded(entry),
                None => ImageCacheState::Failed,
            };
            self.image_cache.insert(src.to_string(), state);
        }
        match self.image_cache.get(src) {
            Some(ImageCacheState::Loaded(e)) => Some(e),
            _ => None,
        }
    }

    /// Render one frame of `host`'s tree into `target_view`.
    ///
    /// - Runs a Taffy layout pass against `logical_viewport`.
    /// - Walks the node tree, accumulating rect + text draw lists
    ///   (sampling the animator for any in-flight transitions).
    /// - Encodes one render pass that clears + paints + types.
    /// - Submits via `queue`.
    ///
    /// The shell's next step is to:
    /// 1. Present the surface texture.
    /// 2. Tick the animator via [`Host::tick_animations`] — if it
    ///    returns `true`, request another redraw.
    pub fn render(
        &mut self,
        host: &Host,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target_view: &wgpu::TextureView,
        logical_viewport: (f32, f32),
        surface_viewport: (f32, f32, f32, f32),
    ) {
        let viewport = [logical_viewport.0, logical_viewport.1];

        // Run Taffy layout against the logical viewport, then
        // a second compute pass for every overlay subtree.
        // Overlays render at the top z-layer with viewport-
        // sized geometry; doing a fresh compute against the
        // full viewport gives the overlay's children the
        // correct frames (Taffy on the main root would still
        // size absolute-positioned overlays against their
        // parent's containing block, which is the wrong
        // reference frame for a modal).
        let root = host.backend().borrow().root();
        if let Some(root) = root.as_ref() {
            let mut backend = host.backend().borrow_mut();
            let root_layout = root.borrow().layout;
            backend.layout.compute(root_layout, viewport[0], viewport[1]);
            let overlays = collect_overlays(root);
            for overlay in &overlays {
                let id = overlay.borrow().layout;
                backend.layout.compute(id, viewport[0], viewport[1]);
            }
        }

        // Hold the immutable borrows for the rest of the frame.
        // The text store is its own `Rc<RefCell<>>` so glyphon
        // buffer refs stay valid across the encode.
        let backend = host.backend().borrow();
        let text_store = host.text_store().borrow();
        // Hold the chrome-glyph cache for the full render — the
        // skin's `paint_device_chrome` pushes `&Buffer` refs
        // into `overlay_texts` that must outlive the encoder
        // submit below.
        let chrome_glyphs = host.chrome_glyphs.borrow();
        let skin = host.skin().clone();
        let focused_layout = host.focused_input_layout();
        let mut rects: Vec<RectInstance> = Vec::new();
        let mut texts: Vec<StagedText<'_>> = Vec::new();
        let now = Instant::now();
        // Sample the keyboard's slide value once per frame.
        // While the keyboard is visible (even partially), narrow
        // the walker's viewport clip to exclude the area it
        // covers. Glyphon's `TextBounds` then clip app text
        // glyphs at the keyboard's top edge so they don't punch
        // through the keyboard's panel (the bug that was making
        // the keyboard look semi-transparent).
        let keyboard_slide = host.sample_keyboard(now);
        let keyboard_top = if host.keyboard_visible() {
            keyboard::keyboard_rect((viewport[0], viewport[1]), keyboard_slide)
                .map(|r| r.1)
                .unwrap_or(viewport[1])
        } else {
            viewport[1]
        };
        let viewport_clip = (0.0, 0.0, viewport[0], keyboard_top);
        // Caret blink phase (true = visible). Computed once per
        // frame so every visible TextInput's caret blinks in
        // unison — matches iOS.
        let caret_visible = caret_blink_visible(now);
        // Spinner rotation phase in `[0, 1)`, shared across every
        // visible ActivityIndicator so they all stay in lockstep.
        let spinner_phase = spinner_phase(now);
        // Overlays detected during the main walk get hoisted
        // here and painted in a second pass below, so they
        // composite above everything else (above the keyboard
        // too, on purpose — a modal that fired during typing
        // should sit on top of the on-screen keyboard).
        let mut deferred_overlays: Vec<(WgpuNode, (f32, f32))> = Vec::new();
        // Top screens of stack navigators with an in-flight
        // push/pop slide. Deferred so they paint in their own
        // render submit between the main pass and the overlay
        // pass — splitting them out is the only way to keep
        // glyphon's main-pass text writes from bleeding through
        // the slide-in screen's background.
        let mut deferred_nav_tops: Vec<DeferredNavTop> = Vec::new();
        // Drawer navigators in any state other than fully
        // closed. Painted in the overlay pass on top of the
        // body screen — the slide + scrim composite cleanly
        // there without re-batching the main draw.
        let mut deferred_drawers: Vec<DeferredDrawer> = Vec::new();
        // Image draws are collected by tree-walk and resolved
        // (decode + bind-group lookup) in a pre-render pass
        // below — that pass takes `&mut self` for the cache,
        // which the read-only walk can't.
        let mut image_requests: Vec<ImageRequest> = Vec::new();
        // Graphics nodes encountered during the walk; resolved in
        // a pre-pass below (allocate offscreen texture, run user's
        // drawer) before the main render encoder runs.
        let mut graphics_requests: Vec<GraphicsRequest> = Vec::new();
        // Video nodes — same resolution pattern: pre-pass uploads
        // the latest decoded frame from each node's decoder
        // thread; main pass composites via the image pipeline.
        let mut video_requests: Vec<VideoRequest> = Vec::new();
        // Clear the prior frame's header-hit registry before the
        // walk fills it. Pointer dispatch reads from this Vec on
        // the next press; rebuilding it every frame keeps it in
        // sync with layout shifts (orientation, theme reflow,
        // navigator transitions in flight).
        let mut header_hits: Vec<crate::host::HeaderHit> = Vec::new();
        if let Some(root) = root.as_ref() {
            walk(
                &backend,
                &text_store,
                skin.as_ref(),
                focused_layout,
                caret_visible,
                spinner_phase,
                now,
                viewport_clip,
                root,
                0.0,
                0.0,
                &mut rects,
                &mut texts,
                &mut deferred_overlays,
                &mut deferred_nav_tops,
                &mut deferred_drawers,
                &mut image_requests,
                &mut graphics_requests,
                &mut video_requests,
                &mut header_hits,
            );
        }

        // On-screen keyboard overlay. Painted *after* the tree
        // walk so it sits on top of regular content; deferred
        // Overlay subtrees paint after it again (modals stack
        // above the keyboard).
        if host.keyboard_visible() {
            keyboard::paint(
                skin.as_ref(),
                (viewport[0], viewport[1]),
                keyboard_slide,
                host.keyboard_pressed_label.get(),
                &host.keyboard_glyphs,
                &mut rects,
                &mut texts,
            );
        }

        // Nav-slide top-screen pass — same rationale as the
        // overlay split (text bleed-through), but specifically
        // for the incoming/outgoing top screen during a
        // navigator push/pop animation. Renders between the
        // main pass and the overlay pass so the slide composites
        // above content + keyboard but below modals.
        let mut nav_top_rects: Vec<RectInstance> = Vec::new();
        let mut nav_top_texts: Vec<StagedText<'_>> = Vec::new();
        let mut nav_top_image_requests: Vec<ImageRequest> = Vec::new();
        let mut nav_top_graphics_requests: Vec<GraphicsRequest> = Vec::new();
        let mut nav_top_video_requests: Vec<VideoRequest> = Vec::new();
        // Sub-deferred overlays / nav-tops discovered while
        // walking a deferred top screen. Top screens can in
        // principle host overlays of their own; route those into
        // the existing overlay queue so they composite above the
        // slide too.
        let mut sub_deferred_overlays: Vec<(WgpuNode, (f32, f32))> = Vec::new();
        let mut sub_deferred_nav_tops: Vec<DeferredNavTop> = Vec::new();
        let mut sub_deferred_drawers: Vec<DeferredDrawer> = Vec::new();
        for top in &deferred_nav_tops {
            walk(
                &backend,
                &text_store,
                skin.as_ref(),
                focused_layout,
                caret_visible,
                spinner_phase,
                now,
                top.clip,
                &top.node,
                top.origin_x,
                top.origin_y,
                &mut nav_top_rects,
                &mut nav_top_texts,
                &mut sub_deferred_overlays,
                &mut sub_deferred_nav_tops,
                &mut sub_deferred_drawers,
                &mut nav_top_image_requests,
                &mut nav_top_graphics_requests,
                &mut nav_top_video_requests,
                &mut header_hits,
            );
        }
        // Publish the assembled hit-region list to the Host so
        // the next pointer-down can read it.
        *host.header_hits.borrow_mut() = header_hits;
        // Hoist any sub-deferred items into the outer queues
        // before the overlay walk so they get painted in the
        // right submit. Nested nav-tops (an in-flight nav inside
        // an already-sliding screen) are unusual but cheap to
        // support — they share the same pass.
        deferred_overlays.extend(sub_deferred_overlays);
        deferred_nav_tops.extend(sub_deferred_nav_tops);
        deferred_drawers.extend(sub_deferred_drawers);

        // Top-z overlay pass — collects into a *separate* batch
        // so its rects + texts paint AFTER the main batch
        // completes. Without this split, all rects (incl. the
        // overlay backdrop) would draw first, then all texts
        // (incl. underlying content text) would draw on top —
        // text would visibly bleed through the backdrop.
        let mut overlay_rects: Vec<RectInstance> = Vec::new();
        let mut overlay_texts: Vec<StagedText<'_>> = Vec::new();
        let mut overlay_image_requests: Vec<ImageRequest> = Vec::new();
        let mut overlay_graphics_requests: Vec<GraphicsRequest> = Vec::new();
        let mut overlay_video_requests: Vec<VideoRequest> = Vec::new();

        // Drawer pass — paints first so true `Overlay` /
        // `AnchoredOverlay` modals composite *over* the drawer
        // (modal on top of a drawer matches every native
        // platform's convention). For each open / animating
        // drawer: scrim across the navigator's rect, then the
        // sidebar at its sampled slide offset.
        let mut drawer_scrim_hits: Vec<crate::host::HeaderHit> = Vec::new();
        for drawer in &deferred_drawers {
            paint_drawer_overlay(
                &backend,
                &text_store,
                skin.as_ref(),
                focused_layout,
                caret_visible,
                spinner_phase,
                now,
                drawer,
                &mut overlay_rects,
                &mut overlay_texts,
                &mut overlay_image_requests,
                &mut overlay_graphics_requests,
                &mut overlay_video_requests,
                &mut drawer_scrim_hits,
            );
        }
        // Drawer-scrim taps re-use the header-hit dispatch
        // pipeline so a tap on the scrim runs the navigator's
        // CloseDrawer command. Append after the per-frame walk
        // so they're ordered against any normal header hits;
        // pointer dispatch iterates in reverse so the scrim hit
        // (added last) wins against under-screen header taps.
        host.header_hits.borrow_mut().extend(drawer_scrim_hits);

        for (overlay_node, _) in &deferred_overlays {
            walk_overlay(
                &backend,
                &text_store,
                skin.as_ref(),
                focused_layout,
                caret_visible,
                spinner_phase,
                now,
                (viewport[0], viewport[1]),
                &mut overlay_image_requests,
                &mut overlay_graphics_requests,
                &mut overlay_video_requests,
                overlay_node,
                &mut overlay_rects,
                &mut overlay_texts,
            );
        }

        // Device chrome — status bar + home indicator. Painted
        // last so it sits on top of every other batch incl.
        // modals + the keyboard. Real iOS lets full-screen
        // modals optionally hide the status bar; for the
        // simulator we keep it pinned, matching design-tool
        // behavior. The skin reads `chrome_glyphs` (held above
        // for the full render) to grab the pre-shaped clock
        // buffer.
        skin.paint_device_chrome(
            (viewport[0], viewport[1]),
            skin.safe_area_insets(),
            now,
            &chrome_glyphs,
            &mut overlay_rects,
            &mut overlay_texts,
        );

        let (vx, vy, vw, vh) = surface_viewport;
        let viewport_px = [viewport[0] as u32, viewport[1] as u32];

        // Phase 1: load (or fail) every image src referenced
        // this frame. Each call mutably borrows `self.image_cache`,
        // so we do it as a separate pass before phase 2 collects
        // immutable references to the loaded bind groups.
        for req in image_requests
            .iter()
            .chain(nav_top_image_requests.iter())
            .chain(overlay_image_requests.iter())
        {
            let _ = self.get_or_load_image(device, queue, &req.src);
        }
        // Phase 2: resolve each request to either a real
        // textured-quad draw (cache hit / fresh load succeeded)
        // or a placeholder rect (failed load — file not found,
        // unsupported format, etc.). Failed requests reuse
        // the original missing-image stripe so the slot is
        // visible in-place.
        let resolve_draws =
            |reqs: &[ImageRequest], rects: &mut Vec<RectInstance>,
             cache: &std::collections::HashMap<String, ImageCacheState>|
             -> Vec<(ImageInstance, String)> {
                let mut out = Vec::new();
                for req in reqs {
                    match cache.get(&req.src) {
                        Some(ImageCacheState::Loaded(_)) => {
                            out.push((
                                ImageInstance {
                                    rect: [
                                        req.rect.0,
                                        req.rect.1,
                                        req.rect.2,
                                        req.rect.3,
                                    ],
                                    uv_rect: [0.0, 0.0, 1.0, 1.0],
                                    tint: [1.0, 1.0, 1.0, 1.0],
                                    rotation: 0.0,
                                    opacity: req.opacity,
                                    _pad: [0.0; 2],
                                },
                                req.src.clone(),
                            ));
                        }
                        _ => {
                            paint_image_placeholder(
                                req.rect.0,
                                req.rect.1,
                                req.rect.2,
                                req.rect.3,
                                &req.src,
                                req.alt.as_deref(),
                                rects,
                            );
                        }
                    }
                }
                out
            };
        let main_image_specs =
            resolve_draws(&image_requests, &mut rects, &self.image_cache);
        let nav_top_image_specs = resolve_draws(
            &nav_top_image_requests,
            &mut nav_top_rects,
            &self.image_cache,
        );
        let overlay_image_specs = resolve_draws(
            &overlay_image_requests,
            &mut overlay_rects,
            &self.image_cache,
        );

        // ----- Submit 0: Graphics offscreen renders -----
        //
        // For every `NodeKind::Graphics` node in the tree (across
        // main + nav-top + overlay layers), ensure an offscreen
        // texture exists at the node's current pixel size and
        // invoke the user-registered drawer to encode into it.
        // Submitted *before* the main pass so the main batch's
        // image-pipeline sample of the same texture reads a
        // fresh frame. One encoder + one submit total — each
        // drawer encodes its own `begin_render_pass`/`end_pass`
        // bracket on this shared encoder.
        if !graphics_requests.is_empty()
            || !nav_top_graphics_requests.is_empty()
            || !overlay_graphics_requests.is_empty()
        {
            let mut encoder0 = device.create_command_encoder(
                &wgpu::CommandEncoderDescriptor { label: Some("idealyst-graphics-pre") },
            );
            let all = graphics_requests
                .iter()
                .chain(nav_top_graphics_requests.iter())
                .chain(overlay_graphics_requests.iter());
            // Convert the node's logical-px rect to *physical* px
            // so the offscreen texture matches the surface's
            // device pixel ratio. Allocating at logical px and
            // composing into a 2x-or-greater logical rect makes
            // the drawer's output read as pixelated on Retina /
            // hi-DPI displays — bilinear upsampling at sample
            // time can't recover detail that wasn't rendered.
            let scale_x = if logical_viewport.0 > 0.0 {
                surface_viewport.2 / logical_viewport.0
            } else {
                1.0
            };
            let scale_y = if logical_viewport.1 > 0.0 {
                surface_viewport.3 / logical_viewport.1
            } else {
                1.0
            };
            // Cap the dynamic-borrow split: gather a snapshot of
            // (node, layout_id, size, created_at) up front so the
            // `node.borrow()` lifetime doesn't collide with the
            // later `borrow_mut()` on the drawer slot, which lives
            // *inside* the same `NodeData`.
            #[derive(Clone)]
            struct PendingGraphics {
                node: crate::node::WgpuNode,
                layout_id: native_layout::LayoutNode,
                size: (u32, u32),
                created_at: web_time::Instant,
            }
            let mut pending: Vec<PendingGraphics> = Vec::new();
            for req in all {
                let (px_w, px_h) = (
                    (req.rect.2 * scale_x).round().max(1.0) as u32,
                    (req.rect.3 * scale_y).round().max(1.0) as u32,
                );
                let data = req.node.borrow();
                let layout_id = data.layout;
                let created_at = match &data.kind {
                    NodeKind::Graphics { created_at, .. } => *created_at,
                    _ => continue,
                };
                pending.push(PendingGraphics {
                    node: req.node.clone(),
                    layout_id,
                    size: (px_w, px_h),
                    created_at,
                });
            }
            for p in pending {
                // Allocate / resize the offscreen target. Holds an
                // immutable borrow on `self.graphics_cache` for the
                // duration of the drawer call so we can pass
                // `&entry.view` without cloning the un-cloneable
                // `wgpu::TextureView`.
                let entry = match self.ensure_graphics_texture(
                    device,
                    p.layout_id,
                    p.size.0,
                    p.size.1,
                ) {
                    Some(e) => e,
                    None => continue,
                };
                let mut frame = crate::node::GraphicsFrame {
                    device,
                    queue,
                    view: &entry.view,
                    encoder: &mut encoder0,
                    size: p.size,
                    elapsed: web_time::Instant::now().saturating_duration_since(p.created_at),
                };
                if let NodeKind::Graphics { drawer, .. } = &p.node.borrow().kind {
                    if let Some(d) = drawer.borrow_mut().as_mut() {
                        d(&mut frame);
                    }
                }
            }
            queue.submit(std::iter::once(encoder0.finish()));
        }

        // Build textured-quad specs for each Graphics node so the
        // main / nav-top / overlay batches composite them via
        // the image pipeline alongside regular images.
        let mut main_graphics_specs: Vec<(ImageInstance, native_layout::LayoutNode)> = Vec::new();
        let mut nav_top_graphics_specs: Vec<(ImageInstance, native_layout::LayoutNode)> = Vec::new();
        let mut overlay_graphics_specs: Vec<(ImageInstance, native_layout::LayoutNode)> = Vec::new();
        let resolve_graphics_draws =
            |reqs: &[GraphicsRequest],
             out: &mut Vec<(ImageInstance, native_layout::LayoutNode)>,
             cache: &std::collections::HashMap<native_layout::LayoutNode, GraphicsTextureEntry>| {
                for req in reqs {
                    let layout_id = req.node.borrow().layout;
                    if !cache.contains_key(&layout_id) {
                        continue;
                    }
                    out.push((
                        ImageInstance {
                            rect: [req.rect.0, req.rect.1, req.rect.2, req.rect.3],
                            uv_rect: [0.0, 0.0, 1.0, 1.0],
                            tint: [1.0, 1.0, 1.0, 1.0],
                            rotation: 0.0,
                            opacity: req.opacity,
                            _pad: [0.0; 2],
                        },
                        layout_id,
                    ));
                }
            };
        resolve_graphics_draws(&graphics_requests, &mut main_graphics_specs, &self.graphics_cache);
        resolve_graphics_draws(
            &nav_top_graphics_requests,
            &mut nav_top_graphics_specs,
            &self.graphics_cache,
        );
        resolve_graphics_draws(
            &overlay_graphics_requests,
            &mut overlay_graphics_specs,
            &self.graphics_cache,
        );

        // ----- Video frame uploads -----
        //
        // For every `NodeKind::Video` in the tree, check whether
        // the decoder thread has published a fresher frame than
        // we've uploaded; if so, ensure the per-node texture and
        // `queue.write_texture` the RGBA bytes. Upload uses the
        // shared queue, so the bytes land before the upcoming
        // main-pass submit reads from the texture. Lifecycle
        // mirrors the Graphics pre-pass — same data shapes,
        // different population mechanism.
        // Take whatever is in each video's slot. Slot presence
        // is the only signal we need — the decoder thread paces
        // its publishes (one per output frame, ~30 fps), so a
        // 120 Hz render pass reads `Some` ~1 in every 4 ticks
        // and `None` otherwise. No counter snapshot to race
        // against the decoder's overwrite.
        for req in video_requests
            .iter()
            .chain(nav_top_video_requests.iter())
            .chain(overlay_video_requests.iter())
        {
            let (layout_id, frame) = {
                let data = req.node.borrow();
                let decoder = match &data.kind {
                    NodeKind::Video { decoder, .. } => decoder.clone(),
                    _ => continue,
                };
                let layout_id = data.layout;
                drop(data);
                let frame = match decoder.shared.latest_frame.lock() {
                    Ok(mut slot) => slot.take(),
                    Err(_) => None,
                };
                (layout_id, frame)
            };
            let frame = match frame {
                Some(f) => f,
                None => continue,
            };
            let entry = match self.ensure_video_texture(device, layout_id, frame.width, frame.height) {
                Some(e) => e,
                None => continue,
            };
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &entry.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &frame.rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * frame.width),
                    rows_per_image: Some(frame.height),
                },
                wgpu::Extent3d {
                    width: frame.width,
                    height: frame.height,
                    depth_or_array_layers: 1,
                },
            );
        }

        // Composite specs for Video — same shape as Graphics.
        let mut main_video_specs: Vec<(ImageInstance, native_layout::LayoutNode)> = Vec::new();
        let mut nav_top_video_specs: Vec<(ImageInstance, native_layout::LayoutNode)> = Vec::new();
        let mut overlay_video_specs: Vec<(ImageInstance, native_layout::LayoutNode)> = Vec::new();
        let resolve_video_draws =
            |reqs: &[VideoRequest],
             out: &mut Vec<(ImageInstance, native_layout::LayoutNode)>,
             cache: &std::collections::HashMap<native_layout::LayoutNode, VideoTextureEntry>| {
                for req in reqs {
                    let layout_id = req.node.borrow().layout;
                    if !cache.contains_key(&layout_id) {
                        continue;
                    }
                    out.push((
                        ImageInstance {
                            rect: [req.rect.0, req.rect.1, req.rect.2, req.rect.3],
                            uv_rect: [0.0, 0.0, 1.0, 1.0],
                            tint: [1.0, 1.0, 1.0, 1.0],
                            rotation: 0.0,
                            opacity: req.opacity,
                            _pad: [0.0; 2],
                        },
                        layout_id,
                    ));
                }
            };
        resolve_video_draws(&video_requests, &mut main_video_specs, &self.video_cache);
        resolve_video_draws(
            &nav_top_video_requests,
            &mut nav_top_video_specs,
            &self.video_cache,
        );
        resolve_video_draws(
            &overlay_video_requests,
            &mut overlay_video_specs,
            &self.video_cache,
        );

        // ----- Submit 1: main content + keyboard -----
        //
        // Encoded + submitted alone so the overlay batch's
        // `queue.write_buffer` calls land *after* this pass
        // executes on the GPU. wgpu's queue applies all pending
        // buffer writes at the start of each submit, so a
        // shared instance buffer used twice within one submit
        // would only see the latest write — losing the main
        // batch's contents.
        {
            // Build the image-draw list *outside* the pass scope
            // so its borrowed bind-group references outlive the
            // pass borrow that consumes them.
            let mut main_image_draws: Vec<ImageDraw<'_>> = main_image_specs
                .iter()
                .map(|(inst, src)| ImageDraw {
                    instance: *inst,
                    bind_group: match self.image_cache.get(src) {
                        Some(ImageCacheState::Loaded(e)) => &e.bind_group,
                        _ => unreachable!(
                            "resolved-but-missing image src in cache"
                        ),
                    },
                })
                .collect();
            // Append Graphics composites — same image pipeline,
            // bind groups come from `graphics_cache` instead of
            // `image_cache`.
            main_image_draws.extend(main_graphics_specs.iter().map(|(inst, layout_id)| {
                ImageDraw {
                    instance: *inst,
                    bind_group: &self.graphics_cache[layout_id].bind_group,
                }
            }));
            main_image_draws.extend(main_video_specs.iter().map(|(inst, layout_id)| {
                ImageDraw {
                    instance: *inst,
                    bind_group: &self.video_cache[layout_id].bind_group,
                }
            }));
            let mut encoder1 =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("idealyst-main"),
                });
            {
                let mut pass = encoder1.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("idealyst-main-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target_view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            // Clear to white. The display region
                            // shows this when the app's root has no
                            // explicit background, which makes
                            // default text + idea-ui chrome read
                            // naturally instead of being lost on
                            // black. The device silhouette (bezel
                            // + notch + rounded corners) is painted
                            // *opaque black on top* by the skin and
                            // the `device_frame` inverse-SDF pass —
                            // so the bezel still looks bezel-like,
                            // we just don't bleed black into the
                            // app's drawing surface.
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 1.0,
                                g: 1.0,
                                b: 1.0,
                                a: 1.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_viewport(vx, vy, vw.max(1.0), vh.max(1.0), 0.0, 1.0);
                self.rect.render(device, queue, &mut pass, viewport, &rects);
                // Textured-quad batch sits *between* rects and
                // text in the main pass so a text label drawn
                // over an image (e.g. a caption) renders on top.
                if !main_image_draws.is_empty() {
                    self.image.render(
                        device,
                        queue,
                        &mut pass,
                        viewport,
                        &main_image_draws,
                    );
                }
                let mut fs = host.font_system().borrow_mut();
                let _ = render_text(
                    &mut self.text,
                    &mut fs,
                    device,
                    queue,
                    &mut pass,
                    viewport_px,
                    &texts,
                );
            }
            queue.submit(std::iter::once(encoder1.finish()));

            // Video-controls overlay — staged by the walk into a
            // thread-local, drained here. This MUST go in its
            // own submit, not just a second pass in encoder1:
            // `RectPipeline::render` calls `queue.write_buffer`
            // on a shared instance buffer, and writes queued
            // before a submit are coalesced — having two passes
            // in one submit would leave both passes drawing the
            // *latest* write (the controls rects), which would
            // smear the controls geometry over the main UI rects
            // (button backgrounds, screen bg, etc.). Submitting
            // the main pass first commits its draws against the
            // main-rects buffer state; this second submit then
            // queues a fresh write for the controls rects and
            // draws them on top via `LoadOp::Load`.
            let controls_overlay = take_video_controls_rects();
            if !controls_overlay.is_empty() {
                let mut encoder_ctrl =
                    device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("idealyst-video-controls-encoder"),
                    });
                {
                    let mut pass2 =
                        encoder_ctrl.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("idealyst-video-controls-pass"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: target_view,
                                resolve_target: None,
                                depth_slice: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Load,
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            depth_stencil_attachment: None,
                            timestamp_writes: None,
                            occlusion_query_set: None,
                            multiview_mask: None,
                        });
                    pass2.set_viewport(vx, vy, vw.max(1.0), vh.max(1.0), 0.0, 1.0);
                    self.rect.render(
                        device,
                        queue,
                        &mut pass2,
                        viewport,
                        &controls_overlay,
                    );
                }
                queue.submit(std::iter::once(encoder_ctrl.finish()));
            }
        }

        // ----- Submit 1.5: nav-slide top screen (loads pass 1) -----
        //
        // Rendered between main and overlay so the slide
        // composites over the under-screen's content (incl. the
        // on-screen keyboard, which paints into the main batch).
        // Without this dedicated submit, glyphon's per-submit
        // text writes from the under-screen would land *on top*
        // of the slide-in screen's rects in raster order — the
        // exact same bleed-through pattern the overlay split
        // fixes, just for nav screens.
        let has_nav_top_content = !nav_top_rects.is_empty()
            || !nav_top_texts.is_empty()
            || !nav_top_image_specs.is_empty();
        if has_nav_top_content {
            let mut nav_top_image_draws: Vec<ImageDraw<'_>> = nav_top_image_specs
                .iter()
                .map(|(inst, src)| ImageDraw {
                    instance: *inst,
                    bind_group: match self.image_cache.get(src) {
                        Some(ImageCacheState::Loaded(e)) => &e.bind_group,
                        _ => unreachable!("resolved-but-missing image src in cache"),
                    },
                })
                .collect();
            nav_top_image_draws.extend(nav_top_graphics_specs.iter().map(
                |(inst, layout_id)| ImageDraw {
                    instance: *inst,
                    bind_group: &self.graphics_cache[layout_id].bind_group,
                },
            ));
            nav_top_image_draws.extend(nav_top_video_specs.iter().map(
                |(inst, layout_id)| ImageDraw {
                    instance: *inst,
                    bind_group: &self.video_cache[layout_id].bind_group,
                },
            ));
            let mut encoder_mid =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("idealyst-nav-slide"),
                });
            {
                let mut pass =
                    encoder_mid.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("idealyst-nav-slide-pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: target_view,
                            resolve_target: None,
                            depth_slice: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                pass.set_viewport(vx, vy, vw.max(1.0), vh.max(1.0), 0.0, 1.0);
                self.rect.render(device, queue, &mut pass, viewport, &nav_top_rects);
                if !nav_top_image_draws.is_empty() {
                    self.image.render(
                        device,
                        queue,
                        &mut pass,
                        viewport,
                        &nav_top_image_draws,
                    );
                }
                let mut fs = host.font_system().borrow_mut();
                let _ = render_text(
                    &mut self.text,
                    &mut fs,
                    device,
                    queue,
                    &mut pass,
                    viewport_px,
                    &nav_top_texts,
                );
            }
            queue.submit(std::iter::once(encoder_mid.finish()));
        }

        // ----- Submit 2: overlay content (loads pass 1) -----
        // The device frame (rounded-display mask) also paints
        // here, last, so any skin with a non-zero corner radius
        // forces this submit even when no app overlay is in
        // flight.
        let device_corner_radius = skin.device_corner_radius();
        let has_overlay_content = !overlay_rects.is_empty()
            || !overlay_texts.is_empty()
            || !overlay_image_specs.is_empty()
            || device_corner_radius > 0.0;
        if has_overlay_content {
            let mut overlay_image_draws: Vec<ImageDraw<'_>> = overlay_image_specs
                .iter()
                .map(|(inst, src)| ImageDraw {
                    instance: *inst,
                    bind_group: match self.image_cache.get(src) {
                        Some(ImageCacheState::Loaded(e)) => &e.bind_group,
                        _ => unreachable!(
                            "resolved-but-missing image src in cache"
                        ),
                    },
                })
                .collect();
            overlay_image_draws.extend(overlay_graphics_specs.iter().map(
                |(inst, layout_id)| ImageDraw {
                    instance: *inst,
                    bind_group: &self.graphics_cache[layout_id].bind_group,
                },
            ));
            overlay_image_draws.extend(overlay_video_specs.iter().map(
                |(inst, layout_id)| ImageDraw {
                    instance: *inst,
                    bind_group: &self.video_cache[layout_id].bind_group,
                },
            ));
            let mut encoder2 =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("idealyst-overlays"),
                });
            {
                let mut pass = encoder2.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("idealyst-overlay-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target_view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            // Load the main pass's output so the
                            // overlay composites on top instead
                            // of clearing the frame.
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_viewport(vx, vy, vw.max(1.0), vh.max(1.0), 0.0, 1.0);
                if !overlay_rects.is_empty() {
                    self.rect.render(device, queue, &mut pass, viewport, &overlay_rects);
                }
                if !overlay_image_draws.is_empty() {
                    self.image.render(
                        device,
                        queue,
                        &mut pass,
                        viewport,
                        &overlay_image_draws,
                    );
                }
                if !overlay_texts.is_empty() {
                    let mut fs = host.font_system().borrow_mut();
                    let _ = render_text(
                        &mut self.text,
                        &mut fs,
                        device,
                        queue,
                        &mut pass,
                        viewport_px,
                        &overlay_texts,
                    );
                }
                // Device frame — paints opaque black outside
                // the rounded display path. Drawn LAST in this
                // overlay submit so it sits on top of every
                // other layer (app, drawer, modals, status bar,
                // home indicator). With this in place, the
                // skin's chrome doesn't need corner masks or
                // edge strips — the inverse-SDF shader gives a
                // clean rounded device silhouette.
                if device_corner_radius > 0.0 {
                    self.device_frame.render(
                        queue,
                        &mut pass,
                        viewport,
                        device_corner_radius,
                    );
                }
            }
            queue.submit(std::iter::once(encoder2.finish()));
        }

        drop(text_store);
        drop(backend);
    }
}

// ---------------------------------------------------------------------------
// Tree walk
// ---------------------------------------------------------------------------

/// Recursive tree walk. Accumulates draw commands into `rects` and
/// `texts` in tree order (back-to-front).
///
/// - `now` is the frame's reference clock for sampling the animator
///   — passed through so every node in the frame sees the same
///   time and avoids one-pixel jitter from timestamp drift.
/// - `clip` is the current scissor rect in logical screen space —
///   narrowed when entering a `ScrollView`. Glyphs intersect it via
///   `TextBounds`; rects are frustum-culled if entirely outside.
///   Per-fragment partial rect clipping is a shader follow-up.
#[allow(clippy::too_many_arguments)]
fn walk<'a>(
    backend: &WgpuBackend,
    text_store: &'a TextStore,
    skin: &dyn crate::skin::Skin,
    focused_input_layout: Option<LayoutNode>,
    caret_visible: bool,
    spinner_phase: f32,
    now: Instant,
    clip: (f32, f32, f32, f32),
    node: &WgpuNode,
    parent_x: f32,
    parent_y: f32,
    rects: &mut Vec<RectInstance>,
    texts: &mut Vec<StagedText<'a>>,
    deferred_overlays: &mut Vec<(WgpuNode, (f32, f32))>,
    deferred_nav_tops: &mut Vec<DeferredNavTop>,
    deferred_drawers: &mut Vec<DeferredDrawer>,
    image_requests: &mut Vec<ImageRequest>,
    graphics_requests: &mut Vec<GraphicsRequest>,
    video_requests: &mut Vec<VideoRequest>,
    header_hits: &mut Vec<crate::host::HeaderHit>,
) {
    let data = node.borrow();
    let frame = backend.layout.frame_of(data.layout);
    let x = parent_x + frame.x;
    let y = parent_y + frame.y;
    let w = frame.width;
    let h = frame.height;

    // Form inputs paint their own background/border via the
    // platform-skinned widget renderer; skip the generic background
    // path for them. The spinner is included even though it doesn't
    // paint a chrome rect — its `background` style is consumed as
    // a tint override by the widget, so it shouldn't double as the
    // node's own fill.
    let is_native_widget = matches!(
        data.kind,
        NodeKind::Toggle { .. }
            | NodeKind::Slider { .. }
            | NodeKind::TextInput { .. }
            | NodeKind::ActivityIndicator { .. }
    );

    let r = &data.render;
    // Portals are hoisted to the top-z viewport pass — they
    // must be deferred regardless of where their in-flow
    // position lands, otherwise scrolling the host content
    // (which shifts the portal's flow origin off-clip)
    // would silently drop the modal from the frame.
    if matches!(data.kind, NodeKind::Portal { .. }) {
        deferred_overlays.push((node.clone(), (parent_x, parent_y)));
        return;
    }
    // Cull entirely-out-of-clip rects. Doesn't handle partial
    // overlap (a half-visible row will still draw past the
    // scrollview's edge). Real per-fragment clipping is shader work.
    let in_clip = !(x + w < clip.0
        || x > clip.0 + clip.2
        || y + h < clip.1
        || y > clip.1 + clip.3);
    if !is_native_widget && in_clip {
        let has_bg = r.background.is_some();
        let any_border = r.border_width.iter().any(|w| *w > 0.0);
        // Skin-driven press feedback may add a background even
        // when the author's style didn't — M3's filled-button
        // state-layer paints an 8% on-primary overlay regardless
        // of the resolved bg. Resolve the overlay (if any) once
        // here so the `has_bg || any_border || press_overlay`
        // gate decides whether to stage a rect at all.
        let press_overlay = if matches!(data.kind, NodeKind::Button { .. }) {
            let t = backend.animator.sample(
                TweenKey::new(data.layout, AnimProperty::PressProgress),
                0.0,
                now,
            );
            if t > 0.0 {
                skin.button_press_visual(t).bg_overlay
            } else {
                None
            }
        } else {
            None
        };
        if has_bg || any_border || press_overlay.is_some() {
            // Drop shadow gets staged *first* so it paints
            // underneath the main rect. The shadow quad covers
            // the visual rect shifted by `(offset.x, offset.y)`
            // and expanded by `blur` on every side; the fragment
            // shader takes care of the soft falloff via the
            // `shadow_blur > 0` path.
            if let Some(sh) = r.shadow.as_ref() {
                let bw = sh.blur.max(0.0);
                let shadow_color =
                    srgb_rgba_to_linear([sh.color[0], sh.color[1], sh.color[2], sh.color[3] * r.opacity]);
                rects.push(RectInstance {
                    rect: [
                        x + sh.offset[0] - bw,
                        y + sh.offset[1] - bw,
                        w + bw * 2.0,
                        h + bw * 2.0,
                    ],
                    bg: shadow_color,
                    // Same corner radii as the visual rect — the
                    // shader compares against the *inner* SDF
                    // (half-extent inset by `bw`), so the shadow
                    // hugs whatever shape the rect itself has.
                    corner_radius: r.corner_radius,
                    border_color: [0.0; 4],
                    border_width: 0.0,
                    rotation: 0.0,
                    shadow_blur: bw,
                    _pad: 0.0,
                });
            }
            let bg_rest = r.background.unwrap_or([0.0; 4]);
            let bg = backend.animator.sample_color(
                TweenKey::new(data.layout, AnimProperty::BackgroundColor),
                bg_rest,
                now,
            );
            // Composite the skin's press overlay on top of the
            // resolved (possibly-tweening) background using
            // standard source-over alpha. The overlay's alpha is
            // already weighted by `t` inside the skin.
            let bg = if let Some(ov) = press_overlay {
                composite_over(bg, ov)
            } else {
                bg
            };
            let bw = r.border_width[0];
            let bc = backend.animator.sample_color(
                TweenKey::new(data.layout, AnimProperty::BorderTopColor),
                r.border_color[0],
                now,
            );
            let bg_lin = srgb_rgba_to_linear([bg[0], bg[1], bg[2], bg[3] * r.opacity]);
            let bc_lin = srgb_rgba_to_linear(bc);
            rects.push(RectInstance {
                rect: [x, y, w, h],
                bg: bg_lin,
                corner_radius: r.corner_radius,
                border_color: bc_lin,
                border_width: bw,
                rotation: 0.0,
                shadow_blur: 0.0,
                _pad: 0.0,
            });
        }
    }

    if in_clip {
        match &data.kind {
            NodeKind::Text { .. } | NodeKind::Button { .. } => {
                if let Some(entry) = text_store.buffers.get(&data.layout) {
                    let mut color = backend.animator.sample_color(
                        TweenKey::new(data.layout, AnimProperty::TextColor),
                        r.color,
                        now,
                    );
                    // Skin-driven press feedback for Buttons:
                    // sample the press-progress tween and let the
                    // active skin convert it into a text-alpha
                    // factor + optional background overlay. Done
                    // here (post-color-tween, pre-stage) so theme
                    // crossfades and press feedback compose
                    // additively without interfering.
                    if matches!(data.kind, NodeKind::Button { .. }) {
                        let t = backend.animator.sample(
                            TweenKey::new(data.layout, AnimProperty::PressProgress),
                            0.0,
                            now,
                        );
                        if t > 0.0 {
                            let v = skin.button_press_visual(t);
                            color[3] *= v.text_alpha_factor;
                        }
                    }
                    // Center the label within the button's frame.
                    // Taffy gives us the outer frame (content +
                    // padding); the buffer was shaped to the
                    // text's intrinsic width, so we offset by the
                    // remaining slack on each axis. For `Text` we
                    // leave the buffer at the frame origin — the
                    // glyphon buffer is already sized to the
                    // measured wrap width, and centering plain
                    // text would diverge from `Text`'s default
                    // top-left flow.
                    let (text_x, text_y) = if matches!(data.kind, NodeKind::Button { .. }) {
                        let (tw, th) = measured_buffer_size(&entry.buffer);
                        (x + ((w - tw) * 0.5).max(0.0), y + ((h - th) * 0.5).max(0.0))
                    } else {
                        (x, y)
                    };
                    let tb = intersect_rect((x, y, w, h), clip);
                    texts.push(StagedText {
                        buffer: &entry.buffer,
                        x: text_x,
                        y: text_y,
                        color,
                        clip: TextBounds {
                            left: tb.0 as i32,
                            top: tb.1 as i32,
                            right: (tb.0 + tb.2) as i32,
                            bottom: (tb.1 + tb.3) as i32,
                        },
                    });
                }
            }
            NodeKind::Toggle { value, .. } => {
                let rest = if *value { 1.0 } else { 0.0 };
                let t = backend.animator.sample(
                    TweenKey::new(data.layout, AnimProperty::ToggleThumb),
                    rest,
                    now,
                );
                // Author-set `background` styles the ON-state
                // track tint. The current sample picks up any
                // animated background tween too, so theme
                // crossfades on the accent color still work.
                let tint = r.background.map(|bg_rest| {
                    backend.animator.sample_color(
                        TweenKey::new(data.layout, AnimProperty::BackgroundColor),
                        bg_rest,
                        now,
                    )
                });
                skin.paint_toggle(x, y, w, h, t, tint, rects);
            }
            NodeKind::Slider { value, min, max, .. } => {
                let tint = r.background.map(|bg_rest| {
                    backend.animator.sample_color(
                        TweenKey::new(data.layout, AnimProperty::BackgroundColor),
                        bg_rest,
                        now,
                    )
                });
                skin.paint_slider(x, y, w, h, *value, *min, *max, tint, rects);
            }
            NodeKind::ActivityIndicator { color, .. } => {
                skin.paint_activity_indicator(
                    x,
                    y,
                    w,
                    h,
                    spinner_phase,
                    *color,
                    rects,
                );
            }
            NodeKind::TextInput { value, .. } => {
                let is_focused = focused_input_layout == Some(data.layout);
                let is_placeholder = value.is_empty();
                // Only paint the caret on the phase-on half of
                // the blink. Unfocused inputs never paint a caret.
                let draw_caret = is_focused && caret_visible;
                if let Some(entry) = text_store.buffers.get(&data.layout) {
                    let caret_local = entry
                        .buffer
                        .layout_runs()
                        .next()
                        .map(|r| r.line_w)
                        .unwrap_or(0.0);
                    let field_bg = r.background.map(|bg_rest| {
                        backend.animator.sample_color(
                            TweenKey::new(data.layout, AnimProperty::BackgroundColor),
                            bg_rest,
                            now,
                        )
                    });
                    skin.paint_text_input(
                        x,
                        y,
                        w,
                        h,
                        is_focused,
                        draw_caret,
                        is_placeholder,
                        &entry.buffer,
                        caret_local,
                        r.color,
                        field_bg,
                        rects,
                        texts,
                    );
                }
            }
            NodeKind::Image { src, alt } => {
                // The walk only *records* image draws — the
                // actual cache lookup + decode happens before
                // the render pass (see `Renderer::render`,
                // pre-pass). Here we push a request so the
                // post-walk collation can join it with the
                // bind group from the cache. Failures resolve
                // there too and fall back to the placeholder.
                image_requests.push(ImageRequest {
                    src: src.clone(),
                    alt: alt.clone(),
                    rect: (x, y, w, h),
                    opacity: r.opacity,
                });
            }
            NodeKind::Graphics { .. } => {
                // Same shape as Image: just record. The
                // renderer's pre-pass ensures the offscreen
                // texture, invokes the user's drawer to encode
                // their commands into it, and the main pass
                // composites the resulting texture via the
                // image pipeline.
                graphics_requests.push(GraphicsRequest {
                    node: node.clone(),
                    rect: (x, y, w, h),
                    opacity: r.opacity,
                });
            }
            NodeKind::Video {
                decoder,
                controls,
                last_hover,
                play_btn_rect,
                scrubber_rect,
                mute_btn_rect,
                frame_rect,
            } => {
                // Same record-and-resolve-later pattern as
                // Graphics; the pre-pass uploads the latest
                // decoded RGBA frame from the node's decoder
                // thread, the main pass composites via the
                // image pipeline.
                video_requests.push(VideoRequest {
                    node: node.clone(),
                    rect: (x, y, w, h),
                    opacity: r.opacity,
                });
                frame_rect.set((x, y, w, h));
                if *controls {
                    let shared = &decoder.shared;
                    let is_playing = shared.playing.load(std::sync::atomic::Ordering::Acquire);
                    let cur_micros = shared.current_time_micros.load(std::sync::atomic::Ordering::Acquire);
                    let dur_micros = shared.duration_micros.load(std::sync::atomic::Ordering::Acquire);
                    // `is_audio_muted()` returns None for silent
                    // clips — treat as "no mute button to show"
                    // by mapping None to a sentinel the paint
                    // helper recognizes.
                    let muted = decoder.is_audio_muted();
                    paint_video_controls(
                        (x, y, w, h),
                        is_playing,
                        cur_micros,
                        dur_micros,
                        muted,
                        last_hover.get(),
                        now,
                        play_btn_rect,
                        scrubber_rect,
                        mute_btn_rect,
                        rects,
                    );
                }
            }
            NodeKind::Icon { paths, view_box, color, stroke_progress } => {
                let tint = color.unwrap_or(r.color);
                // Sample the animator first — `animate_icon_stroke`
                // drives the IconStroke property; falls back to the
                // node's stored progress if no tween is in flight.
                let progress = backend.animator.sample(
                    TweenKey::new(data.layout, AnimProperty::IconStroke),
                    stroke_progress.get(),
                    now,
                );
                paint_icon(x, y, w, h, paths, *view_box, tint, progress, rects);
            }
            NodeKind::Link { .. } => {
                // Hit-region is wired via Pressable-style press
                // detection in the host; the visible chrome is
                // whatever the user's children render. The Link
                // itself paints nothing extra here.
            }
            // Overlay/AnchoredOverlay are intentionally not
            // matched here — they're handled by the early
            // return above so the in-flow clip-cull doesn't
            // drop them when their flow position scrolls
            // off-screen.
            NodeKind::TabNavigator { active_tab, tab_count, bar_style, .. } => {
                // Paint the tab bar strip; the screens stack
                // through the child walk below. `bar_style` —
                // if the app set one via `.tab_bar_style(...)` —
                // overrides the default neutral gray.
                let bar_bg = bar_style
                    .borrow()
                    .as_ref()
                    .and_then(|s| s.background.as_ref())
                    .map(|t| crate::style_convert::parse_color(&t.resolve()));
                paint_tab_bar(
                    x, y, w, h,
                    active_tab.get(),
                    tab_count.get(),
                    bar_bg,
                    rects,
                );
            }
            NodeKind::DrawerNavigator { is_open, .. } => {
                // Body screens paint through the child walk
                // below; the sidebar (separately retained) is
                // walked in the deferred top-z pass so it
                // composites above the body. The drawer itself
                // paints nothing here.
                let _ = is_open;
            }
            NodeKind::Unsupported { label } => {
                paint_unsupported(x, y, w, h, label, rects);
            }
            _ => {}
        }
    }

    // Capture whether this node owns a navigator header to paint
    // *after* the children walk. We previously painted the
    // header here in the per-kind paint block, but that pre-loaded
    // the header rects into the batch before the screen's own
    // content rects — and because the renderer has no shader-side
    // scissor (clip-culling is whole-rect, not per-fragment),
    // scrolled-up content that geometrically overlaps the header
    // strip ends up painting *over* the header icons. Deferring
    // the header paint until after the children means header
    // rects are appended last and win the draw-order race.
    let paint_header_after_children = data.navigator_screen
        && data
            .screen_options
            .as_ref()
            .and_then(|o| o.header_shown)
            .unwrap_or(true);

    // Determine child origin + clip. A ScrollView shifts children
    // by `-offset` and narrows the clip to its frame (intersected
    // with the inherited clip).
    let (child_origin_x, child_origin_y, child_clip) = match &data.kind {
        NodeKind::ScrollView { offset_x, offset_y, .. } => (
            x - *offset_x,
            y - *offset_y,
            intersect_rect((x, y, w, h), clip),
        ),
        _ => (x, y, clip),
    };

    // Capture scrollview offsets for the post-children scrollbar
    // overlay, computed before the borrow is released.
    let scrollbar_state: Option<(bool, f32, f32)> = match &data.kind {
        NodeKind::ScrollView { horizontal, offset_x, offset_y } => {
            Some((*horizontal, *offset_x, *offset_y))
        }
        _ => None,
    };

    let children: Vec<WgpuNode> = data.children.clone();
    // Navigator-style child filtering: stack navigators only
    // paint the topmost (last) screen; tab navigators only
    // paint the active tab; drawer navigators paint the body
    // (everything except the sidebar — that's mounted via a
    // separate slot and painted in the post-walk overlay pass).
    //
    // Each entry pairs a child with an x-translate (in px from
    // the natural child origin). The translate is non-zero only
    // for stack-navigator screens during a push/pop slide — see
    // `nav_transition_offsets` below.
    let kind_ref = &data.kind;
    // Optional defer for the top screen of a stack navigator
    // mid-transition. Filled below in the Navigator branch and
    // drained at the end of `walk` (after the children loop) so
    // the inline walk only paints the under-screen.
    let mut pending_nav_top: Option<DeferredNavTop> = None;
    // (frame_x_translate, frame_y_translate) per visible child.
    // Filled per match arm; the loop below converts each into
    // an origin offset before recursing.
    let (visible_children, nav_in_transition): (Vec<(WgpuNode, ScreenXform)>, bool) =
        match kind_ref {
            NodeKind::Navigator { transition, transition_anim, .. } => {
                let frame = nav_transition_frame(transition, transition_anim, w, h, now);
                if let Some(frame) = frame {
                    // Painting both screens in one submit would let
                    // the under-screen's text bleed through the top
                    // screen's background (glyphon writes all text
                    // in one queue.submit unless we split). Walk
                    // the under-screen inline with its sampled
                    // transform and defer the top screen to the
                    // dedicated nav-slide pass.
                    let len = children.len();
                    let mut out = Vec::with_capacity(1);
                    if len >= 2 {
                        out.push((children[len - 2].clone(), frame.under));
                    }
                    if let Some(last) = children.last() {
                        pending_nav_top = Some(DeferredNavTop {
                            node: last.clone(),
                            origin_x: x + frame.top.translate_x,
                            origin_y: y + frame.top.translate_y,
                            clip: intersect_rect((x, y, w, h), child_clip),
                        });
                    }
                    (out, true)
                } else {
                    (
                        children
                            .last()
                            .cloned()
                            .map(|c| vec![(c, ScreenXform::default())])
                            .unwrap_or_default(),
                        false,
                    )
                }
            }
        NodeKind::TabNavigator { active_tab, .. } => {
            let idx = active_tab.get().min(children.len().saturating_sub(1));
            (
                children
                    .get(idx)
                    .cloned()
                    .map(|c| vec![(c, ScreenXform::default())])
                    .unwrap_or_default(),
                false,
            )
        }
        // DrawerNavigator: paint only the active body screen
        // in the normal walk. The sidebar (kept out of
        // `children` by `drawer_navigator_attach_sidebar`'s
        // separate slot, but appended into the children Vec
        // too for Taffy parenting) gets hoisted to the
        // deferred drawer-overlay pass so it composites above
        // the body with the slide transform.
        NodeKind::DrawerNavigator { sidebar, active_screen, .. } => {
            let sidebar_node = sidebar.borrow().clone();
            let body: Vec<WgpuNode> = children
                .iter()
                .filter(|c| !sidebar_node.as_ref().is_some_and(|s| Rc::ptr_eq(s, c)))
                .cloned()
                .collect();
            let idx = active_screen.get().min(body.len().saturating_sub(1));
            (
                body.get(idx)
                    .cloned()
                    .map(|c| vec![(c, ScreenXform::default())])
                    .unwrap_or_default(),
                false,
            )
        }
        // Portal never reaches this match — the early-return at
        // the top of `walk` deferred it before any clip /
        // children logic ran. Listed for exhaustiveness so a
        // future variant can't fall into the default arm and
        // accidentally walk portal children inline.
        NodeKind::Portal { .. } => (Vec::new(), false),
        _ => (
            children
                .iter()
                .map(|c| (c.clone(), ScreenXform::default()))
                .collect(),
            false,
        ),
    };
    // While a navigator transition is in flight, narrow the
    // children's clip to the navigator's own rect so neither
    // the under-screen's parallax slide nor the top screen's
    // entry/exit slide paints outside the navigator's bounds.
    let child_clip = if nav_in_transition {
        intersect_rect((x, y, w, h), child_clip)
    } else {
        child_clip
    };
    // Drawer: queue the sidebar + scrim for the post-walk
    // overlay pass whenever the drawer is at least partly open
    // (fully open at rest, or animating in either direction).
    // `is_open` reflects the *target* state set by the
    // dispatcher; the renderer interpolates `progress` between
    // 0 and 1 based on `anim_started_at`.
    if let NodeKind::DrawerNavigator {
        is_open,
        sidebar,
        anim_started_at,
        ..
    } = kind_ref
    {
        if let Some(s) = sidebar.borrow().as_ref().cloned() {
            let target = if is_open.get() { 1.0 } else { 0.0 };
            let (progress, anim_alive) =
                sample_drawer_progress(anim_started_at.get(), target, now);
            // Clear the anim cell once the slide settles so the
            // host's tick doesn't keep redrawing forever.
            if !anim_alive && anim_started_at.get().is_some() {
                anim_started_at.set(None);
            }
            if progress > 0.0 {
                deferred_drawers.push(DeferredDrawer {
                    sidebar: s,
                    nav_rect: (x, y, w, h),
                    progress,
                    anim_alive,
                    navigator: node.clone(),
                });
            }
        }
    }
    drop(data);
    for (child, xform) in &visible_children {
        walk(
            backend,
            text_store,
            skin,
            focused_input_layout,
            caret_visible,
            spinner_phase,
            now,
            child_clip,
            child,
            child_origin_x + xform.translate_x,
            child_origin_y + xform.translate_y,
            rects,
            texts,
            deferred_overlays,
            deferred_nav_tops,
            deferred_drawers,
            image_requests,
            graphics_requests,
            video_requests,
            header_hits,
        );
    }
    if let Some(top) = pending_nav_top {
        deferred_nav_tops.push(top);
    }

    // Scrollbar overlay. Drawn after children so it sits on top
    // of the content. iOS-style overlay scrollbar: thin gray
    // translucent thumb pinned to the trailing edge of the
    // scrollview. Only painted when the content overflows.
    if let Some((horizontal, off_x, off_y)) = scrollbar_state {
        paint_scrollbar(backend, node, x, y, w, h, horizontal, off_x, off_y, rects);
    }

    // Navigator header — painted *last* (see the note where
    // `paint_header_after_children` is decided above) so it
    // composites over any scrolled content that bleeds past the
    // screen's top edge.
    if paint_header_after_children {
        let data = node.borrow();
        paint_screen_header(
            skin, &data, x, y, w, rects, texts, text_store, header_hits,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_scrollbar(
    backend: &WgpuBackend,
    node: &WgpuNode,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    horizontal: bool,
    offset_x: f32,
    offset_y: f32,
    rects: &mut Vec<RectInstance>,
) {
    let (content_w, content_h) = scrollview_content_extent(backend, node);

    // Track color: translucent black overlay (works on light + dark
    // backgrounds without a theme-bound resolve). Drawn in sRGB
    // and linearized below.
    const THUMB_SRGB: [f32; 4] = [0.0, 0.0, 0.0, 0.35];
    let radius = SCROLLBAR_WIDTH * 0.5;

    if horizontal {
        let viewport = w;
        let content = content_w;
        if content <= viewport {
            return;
        }
        let max_offset = content - viewport;
        let thumb_w = (viewport * (viewport / content)).max(SCROLLBAR_MIN_THUMB);
        let travel = (viewport - thumb_w).max(0.0);
        let t = if max_offset > 0.0 {
            (offset_x / max_offset).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let thumb_x = x + travel * t;
        let thumb_y = y + h - SCROLLBAR_WIDTH - SCROLLBAR_INSET;
        rects.push(RectInstance {
            rect: [thumb_x, thumb_y, thumb_w, SCROLLBAR_WIDTH],
            bg: srgb_rgba_to_linear(THUMB_SRGB),
            corner_radius: [radius; 4],
            border_color: [0.0; 4],
            border_width: 0.0,
            rotation: 0.0,
            shadow_blur: 0.0, _pad: 0.0,
        });
    } else {
        let viewport = h;
        let content = content_h;
        if content <= viewport {
            return;
        }
        let max_offset = content - viewport;
        let thumb_h = (viewport * (viewport / content)).max(SCROLLBAR_MIN_THUMB);
        let travel = (viewport - thumb_h).max(0.0);
        let t = if max_offset > 0.0 {
            (offset_y / max_offset).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let thumb_x = x + w - SCROLLBAR_WIDTH - SCROLLBAR_INSET;
        let thumb_y = y + travel * t;
        rects.push(RectInstance {
            rect: [thumb_x, thumb_y, SCROLLBAR_WIDTH, thumb_h],
            bg: srgb_rgba_to_linear(THUMB_SRGB),
            corner_radius: [radius; 4],
            border_color: [0.0; 4],
            border_width: 0.0,
            rotation: 0.0,
            shadow_blur: 0.0, _pad: 0.0,
        });
    }
}

use crate::nav_anim::ScreenXform;

/// A stack-navigator's top screen, queued for the dedicated
/// nav-slide render pass that runs between the main pass and
/// the overlay pass. Held aside so the slide's rects+text
/// composite cleanly on top of the under-screen (which painted
/// in the main pass) instead of interleaving in a single submit
/// — without this split, glyphon writes from the under-screen
/// would visibly bleed through the top screen's background.
pub(crate) struct DeferredNavTop {
    pub node: WgpuNode,
    pub origin_x: f32,
    pub origin_y: f32,
    pub clip: (f32, f32, f32, f32),
}

/// An open (or animating) drawer navigator's sidebar + scrim,
/// queued for the overlay render pass. The scrim is a
/// translucent full-rect fill behind the sliding sidebar;
/// `progress` drives both the scrim alpha and the sidebar's
/// horizontal slide. `nav_rect` is the navigator's own box so
/// the scrim doesn't paint outside the navigator (nested
/// navigators), and `sidebar` is the WgpuNode to recursively
/// walk for the panel's content.
pub(crate) struct DeferredDrawer {
    pub sidebar: WgpuNode,
    pub nav_rect: (f32, f32, f32, f32),
    /// 0.0 = closed (offscreen left, scrim fully transparent),
    /// 1.0 = fully open (sidebar at left edge, scrim at max).
    pub progress: f32,
    /// `true` when an animation is still in flight — the host's
    /// tick uses this to keep redrawing. Once a transition
    /// finishes, the renderer clears the `anim_started_at` cell
    /// on the source node.
    pub anim_alive: bool,
    /// Strong handle so the host's pointer dispatch can find
    /// the navigator a scrim tap belongs to.
    pub navigator: WgpuNode,
}

/// Paint a navigator-screen's header strip via the active
/// skin. Resolves the per-screen `ScreenOptions` (sampled this
/// frame so theme-bound color closures reflect the current
/// theme) and the owning navigator's chrome styles into a
/// [`crate::skin::NavigatorHeaderChrome`] bundle, calls the
/// skin's paint method, and translates each emitted
/// [`crate::skin::NavigatorHeaderHit`] into a
/// [`crate::host::HeaderHit`] tagged with the owning nav so
/// pointer dispatch routes back-button taps to the right stack.
///
/// `data` is the screen node's borrowed `NodeData`; `x` and `y`
/// are the screen's *content* origin (Taffy-laid below the
/// header inset). The header strip sits at `(x, y -
/// NAV_HEADER_HEIGHT, w, NAV_HEADER_HEIGHT)`.
#[allow(clippy::too_many_arguments)]
fn paint_screen_header<'a, 'b>(
    skin: &dyn crate::skin::Skin,
    data: &'b crate::node::NodeData,
    x: f32,
    y: f32,
    w: f32,
    rects: &mut Vec<RectInstance>,
    texts: &mut Vec<StagedText<'a>>,
    text_store: &'a TextStore,
    header_hits: &mut Vec<crate::host::HeaderHit>,
) {
    use crate::skin::{
        NavigatorHeaderAction, NavigatorHeaderChrome, NavigatorHeaderHit,
    };
    let Some(options) = data.screen_options.as_deref() else { return };
    // `owning_navigator` is `Weak` to avoid a parent↔child Rc cycle;
    // upgrade for the duration of this paint pass. If the navigator
    // has been dropped (shouldn't happen while we're painting one of
    // its screens, but be defensive), skip the header — the screen
    // chrome is meaningless without it.
    let Some(navigator) = data.owning_navigator.as_ref().and_then(|w| w.upgrade()) else {
        return;
    };
    let navigator = &navigator;

    let header_y = y - crate::node::NAV_HEADER_HEIGHT;
    let header_rect = (x, header_y, w, crate::node::NAV_HEADER_HEIGHT);

    // Stack depth + style fallbacks come from the owning
    // navigator. Borrow scope kept tight so the navigator's
    // RefCell isn't held while we call into the skin.
    let (depth, header_bg_default, title_color_default, tint_default) = {
        let nav_data = navigator.borrow();
        match &nav_data.kind {
            NodeKind::Navigator {
                scope_ids,
                header_style,
                title_style,
                button_style,
                ..
            } => (
                scope_ids.borrow().len(),
                header_style
                    .borrow()
                    .as_ref()
                    .and_then(|r| r.background.as_ref())
                    .map(|t| crate::style_convert::parse_color(&t.resolve())),
                title_style
                    .borrow()
                    .as_ref()
                    .and_then(|r| r.color.as_ref())
                    .map(|t| crate::style_convert::parse_color(&t.resolve())),
                button_style
                    .borrow()
                    .as_ref()
                    .and_then(|r| r.color.as_ref())
                    .map(|t| crate::style_convert::parse_color(&t.resolve())),
            ),
            _ => (0, None, None, None),
        }
    };

    // Per-screen closure samples (theme-bound) override
    // navigator-level style fallbacks. Skin's default beats
    // them only when both are `None`.
    let background = options
        .header_background
        .as_ref()
        .map(|f| crate::style_convert::parse_color(&f()))
        .or(header_bg_default);
    let title_color = options
        .title_color
        .as_ref()
        .map(|f| crate::style_convert::parse_color(&f()))
        .or(title_color_default);
    let tint = options
        .header_tint
        .as_ref()
        .map(|f| crate::style_convert::parse_color(&f()))
        .or(tint_default);

    let title_buffer = data
        .screen_title_layout
        .and_then(|id| text_store.buffers.get(&id))
        .map(|entry| &entry.buffer);

    // The skin gets first crack at painting (rects + texts) and
    // populates a local Vec of hit regions; we translate those
    // into per-frame Host entries below.
    let mut local_hits: Vec<NavigatorHeaderHit> = Vec::new();
    let header_left_name = options.header_left.as_ref().map(|b| b.icon.as_str());
    let header_right_name = options.header_right.as_ref().map(|b| b.icon.as_str());
    let show_back = depth >= 2 && options.header_left.is_none();
    // The screen's frame starts `safe_area.top + NAV_HEADER_HEIGHT`
    // below the navigator's top; the header rect sits in the
    // `[navigator_top + safe_area, navigator_top + safe_area +
    // NAV_HEADER_HEIGHT]` band. We pass `safe_area.top` so the
    // skin can extend the header's bg upward into the status-
    // bar strip — without this the strip stays the clear color
    // and the bg appears to ignore the slide.
    let safe_area_top = framework_core::safe_area_insets().get().top;
    let chrome = NavigatorHeaderChrome {
        title: title_buffer,
        show_back,
        header_left_icon: header_left_name,
        header_right_icon: header_right_name,
        background,
        title_color,
        tint,
        safe_area_top,
    };
    skin.paint_navigator_header(header_rect, chrome, rects, texts, &mut local_hits);

    // Route each hit through `Host::header_hits`. The press
    // dispatch resolves them in z-order on the next pointer-down.
    for hit in local_hits {
        let action = match hit.action {
            NavigatorHeaderAction::Back => crate::host::HeaderHitAction::Back,
            NavigatorHeaderAction::HeaderLeft => {
                let Some(btn) = options.header_left.as_ref() else { continue };
                crate::host::HeaderHitAction::HeaderLeft(btn.on_press.clone())
            }
            NavigatorHeaderAction::HeaderRight => {
                let Some(btn) = options.header_right.as_ref() else { continue };
                crate::host::HeaderHitAction::HeaderRight(btn.on_press.clone())
            }
        };
        header_hits.push(crate::host::HeaderHit {
            rect: hit.rect,
            // Weak — see `HeaderHit::navigator` doc comment about
            // why the per-frame hit registry must not strong-ref
            // the navigator.
            navigator: Rc::downgrade(navigator),
            action,
        });
    }
}

/// Sample the drawer's `scrim_style.background` for its scrim
/// color. `None` means the renderer should fall back to the
/// default 32%-black Material guideline value. The full
/// `StyleRules` is stored on the navigator node but only
/// `background` is read here — other fields aren't meaningful
/// for the scrim's purpose.
fn scrim_color_from_navigator(navigator: &WgpuNode) -> Option<[f32; 4]> {
    if let NodeKind::DrawerNavigator { scrim_style, .. } = &navigator.borrow().kind {
        return scrim_style
            .borrow()
            .as_ref()
            .and_then(|s| s.background.as_ref())
            .map(|t| crate::style_convert::parse_color(&t.resolve()));
    }
    None
}

/// Paint a deferred drawer into the overlay batch: scrim
/// behind, sidebar in front. The sidebar's content walks
/// through the regular `walk` recursion with a slide-offset
/// applied to its origin, so the panel's own children (text,
/// buttons, theme bg) all paint correctly.
///
/// Scrim is a single rect across the navigator's box with alpha
/// proportional to slide progress. A
/// [`crate::host::HeaderHit`] is registered over the visible
/// portion of the body (the area NOT covered by the sidebar)
/// so a tap there fires `CloseDrawer` — matches Material /
/// iOS convention where tapping outside the panel dismisses
/// it.
#[allow(clippy::too_many_arguments)]
fn paint_drawer_overlay<'a>(
    backend: &WgpuBackend,
    text_store: &'a TextStore,
    skin: &dyn crate::skin::Skin,
    focused_input_layout: Option<LayoutNode>,
    caret_visible: bool,
    spinner_phase: f32,
    now: Instant,
    drawer: &DeferredDrawer,
    rects: &mut Vec<RectInstance>,
    texts: &mut Vec<StagedText<'a>>,
    image_requests: &mut Vec<ImageRequest>,
    graphics_requests: &mut Vec<GraphicsRequest>,
    video_requests: &mut Vec<VideoRequest>,
    scrim_hits: &mut Vec<crate::host::HeaderHit>,
) {
    let (nx, ny, nw, nh) = drawer.nav_rect;
    let sidebar_frame = backend.layout.frame_of(drawer.sidebar.borrow().layout);
    let sidebar_w = sidebar_frame.width;
    // Sidebar slides from the left edge: progress=0 → x = -w
    // (off-screen left), progress=1 → x = 0.
    let slide_x = -sidebar_w * (1.0 - drawer.progress);

    // Scrim rect — full navigator box, alpha scaled by progress.
    // Color comes from the navigator's `scrim_style.background`
    // when the app set one via `.scrim_style(...)`; otherwise
    // the default 32%-black Material guideline value.
    let scrim_rgba = scrim_color_from_navigator(&drawer.navigator)
        .unwrap_or([0.0, 0.0, 0.0, crate::node::DRAWER_SCRIM_MAX_ALPHA]);
    let scrim_alpha = drawer.progress * scrim_rgba[3];
    if scrim_alpha > 0.001 {
        rects.push(crate::widgets::rect_inst(
            nx,
            ny,
            nw,
            nh,
            [scrim_rgba[0], scrim_rgba[1], scrim_rgba[2], scrim_alpha],
            [0.0; 4],
            [0.0; 4],
            0.0,
        ));
    }

    // Tap-outside-to-close: register the area to the *right* of
    // the sidebar (the visible body strip) as a scrim hit. We
    // reuse the existing header-hit dispatch path with a
    // synthetic `CloseDrawer` action that the host dispatches
    // through this navigator's control on press.
    let visible_x = nx + slide_x + sidebar_w;
    let visible_w = (nx + nw - visible_x).max(0.0);
    if visible_w > 0.0 && drawer.progress > 0.0 {
        scrim_hits.push(crate::host::HeaderHit {
            rect: (visible_x, ny, visible_w, nh),
            navigator: Rc::downgrade(&drawer.navigator),
            action: crate::host::HeaderHitAction::CloseDrawer,
        });
    }

    // Sidebar walk. Origin = navigator origin + slide offset;
    // clip = navigator's box so a sidebar wider than the
    // navigator stays inside. The drawer is conceptually the
    // top of the z-stack within its navigator, so it can host
    // its own deferred overlays / header hits but not nested
    // drawers — nested drawers would be a weird design choice
    // and go into a throwaway sink here.
    let mut sub_overlays = Vec::new();
    let mut sub_nav_tops = Vec::new();
    let mut sub_drawers = Vec::new();
    let mut sub_header_hits = Vec::new();
    walk(
        backend,
        text_store,
        skin,
        focused_input_layout,
        caret_visible,
        spinner_phase,
        now,
        (nx, ny, nw, nh),
        &drawer.sidebar,
        nx + slide_x,
        ny,
        rects,
        texts,
        &mut sub_overlays,
        &mut sub_nav_tops,
        &mut sub_drawers,
        image_requests,
        graphics_requests,
        video_requests,
        &mut sub_header_hits,
    );
    // Sidebar header buttons (if any) join the outer hits.
    scrim_hits.extend(sub_header_hits);
    // Sub-overlays/sub-nav-tops/sub-drawers from inside the
    // sidebar are uncommon; surface a warning if dropped so
    // future authors notice.
    if !sub_overlays.is_empty()
        || !sub_nav_tops.is_empty()
        || !sub_drawers.is_empty()
    {
        // Quiet drop — a sidebar with a modal inside is rare;
        // if needed we can lift them into the outer queues.
    }
}

/// Compute the drawer's slide progress at `now`. `target` is
/// the dispatcher's most recent target (1.0 open, 0.0 closed).
/// Returns `(progress, anim_alive)`:
/// - `progress` is the eased visible amount in `[0, 1]`.
/// - `anim_alive` is `true` while the slide is still in
///   flight; the host's tick keeps redrawing on its strength.
///
/// With no `anim_started_at`, the drawer is at rest at `target`.
/// During a slide, progress eases from the *opposite* extreme
/// toward `target` over `DRAWER_ANIM_MS`. Ease-out cubic —
/// matches Material's emphasized-decelerate curve closely.
fn sample_drawer_progress(
    started: Option<web_time::Instant>,
    target: f32,
    now: Instant,
) -> (f32, bool) {
    let Some(start) = started else { return (target, false) };
    let elapsed = now.saturating_duration_since(start).as_millis() as f32;
    let total = crate::node::DRAWER_ANIM_MS as f32;
    if elapsed >= total {
        return (target, false);
    }
    let t = (elapsed / total).clamp(0.0, 1.0);
    let eased = 1.0 - (1.0 - t).powi(3);
    let from = 1.0 - target; // opposite extreme
    let progress = from + (target - from) * eased;
    (progress, true)
}

/// Sample the navigator's transition animator for the current
/// frame. Returns `None` when no transition is in flight (the
/// nav is at rest); otherwise the under-screen and top-screen
/// transforms the renderer should apply.
fn nav_transition_frame(
    transition: &std::cell::RefCell<Option<crate::node::NavTransition>>,
    anim: &std::rc::Rc<dyn crate::nav_anim::ScreenTransition>,
    width: f32,
    height: f32,
    now: web_time::Instant,
) -> Option<crate::nav_anim::TransitionFrame> {
    let t = transition.borrow();
    let nav = t.as_ref()?;
    let duration = anim.duration_ms().max(1) as f32;
    let elapsed = now.saturating_duration_since(nav.start).as_millis() as f32;
    let progress = (elapsed / duration).clamp(0.0, 1.0);
    let direction = match &nav.kind {
        crate::node::NavTransitionKind::Push => {
            crate::nav_anim::TransitionDirection::Push
        }
        crate::node::NavTransitionKind::Pop { .. } => {
            crate::nav_anim::TransitionDirection::Pop
        }
    };
    Some(anim.sample(direction, progress, width, height))
}

/// Intersect two axis-aligned rects, returning a possibly-zero
/// `(x, y, w, h)`. Used to narrow the active clip when descending
/// into a `ScrollView` and to clamp glyph `TextBounds`.
fn intersect_rect(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    let x = a.0.max(b.0);
    let y = a.1.max(b.1);
    let r = (a.0 + a.2).min(b.0 + b.2);
    let bot = (a.1 + a.3).min(b.1 + b.3);
    (x, y, (r - x).max(0.0), (bot - y).max(0.0))
}

/// Source-over alpha composite of `over` onto `under`, both in
/// straight-alpha sRGB. The skin's press overlay carries its
/// own alpha (already scaled by the press-progress `t`), so this
/// is the standard Porter–Duff "over" formula with no extra
/// premultiplication step. Per-channel; opacity from the
/// underlying author style still scales the final alpha
/// downstream of this composite.
fn composite_over(under: [f32; 4], over: [f32; 4]) -> [f32; 4] {
    let a = over[3] + under[3] * (1.0 - over[3]);
    if a <= f32::EPSILON {
        return [0.0; 4];
    }
    let blend = |c_under: f32, c_over: f32| -> f32 {
        (c_over * over[3] + c_under * under[3] * (1.0 - over[3])) / a
    };
    [
        blend(under[0], over[0]),
        blend(under[1], over[1]),
        blend(under[2], over[2]),
        a,
    ]
}

/// Pull the (width, height) of a glyphon `Buffer`'s current
/// shaped layout. Iterates `layout_runs` rather than re-shaping
/// — the buffer was already shaped by the Taffy measure pass,
/// so this is a cheap read of cached state. Used to center
/// labels inside their parent's padded frame (Buttons today;
/// any future centered-text widget can call the same helper).
fn measured_buffer_size(buffer: &Buffer) -> (f32, f32) {
    let mut w: f32 = 0.0;
    let mut h: f32 = 0.0;
    for run in buffer.layout_runs() {
        w = w.max(run.line_w);
        h = h.max(run.line_top + run.line_height);
    }
    (w, h)
}

/// Walk the tree once collecting every `Overlay` /
/// `AnchoredOverlay` subtree root. The walker uses the
/// resulting list to (a) compute each overlay against the full
/// viewport in a second Taffy pass and (b) defer rendering to
/// the top-z layer.
fn collect_overlays(root: &WgpuNode) -> Vec<WgpuNode> {
    fn recurse(node: &WgpuNode, out: &mut Vec<WgpuNode>) {
        let data = node.borrow();
        if matches!(data.kind, NodeKind::Portal { .. }) {
            out.push(node.clone());
        }
        let children: Vec<WgpuNode> = data.children.clone();
        drop(data);
        for child in &children {
            recurse(child, out);
        }
    }
    let mut out = Vec::new();
    recurse(root, &mut out);
    out
}

/// Paint a deferred Overlay subtree at the top-z layer. The
/// overlay's children render against the *viewport*, not the
/// parent's flow position — the overlay's pre-compute pass
/// gave it a viewport-sized frame, so child Taffy frames are
/// already in viewport-local coordinates.
#[allow(clippy::too_many_arguments)]
fn walk_overlay<'a>(
    backend: &WgpuBackend,
    text_store: &'a TextStore,
    skin: &dyn crate::skin::Skin,
    focused_input_layout: Option<LayoutNode>,
    caret_visible: bool,
    spinner_phase: f32,
    now: Instant,
    viewport: (f32, f32),
    image_requests: &mut Vec<ImageRequest>,
    graphics_requests: &mut Vec<GraphicsRequest>,
    video_requests: &mut Vec<VideoRequest>,
    node: &WgpuNode,
    rects: &mut Vec<RectInstance>,
    texts: &mut Vec<StagedText<'a>>,
) {
    let data = node.borrow();
    let frame = backend.layout.frame_of(data.layout);

    // Backdrop is no longer a backend concern — the composition
    // layer emits a backdrop primitive as a child of the portal,
    // so it just paints through the regular child walk below.
    let viewport_clip = (0.0, 0.0, viewport.0, viewport.1);

    // Content origin: derive from the portal's target. `Viewport`
    // uses the placement enum; `Anchor` re-queries the anchor rect
    // each frame (free because we render every frame); `Named`
    // shouldn't reach here — `create_portal` panics on construction.
    let (content_x, content_y) = match &data.kind {
        NodeKind::Portal { target, .. } => match target {
            framework_core::primitives::portal::PortalTarget::Viewport(
                placement,
            ) => place_overlay(
                *placement,
                frame.width,
                frame.height,
                viewport.0,
                viewport.1,
            ),
            framework_core::primitives::portal::PortalTarget::Anchor {
                target,
                side,
                align,
                offset,
            } => match target.rect() {
                Some(vr) => position_anchored(
                    (vr.x, vr.y, vr.width, vr.height),
                    frame.width,
                    frame.height,
                    *side,
                    *align,
                    *offset,
                ),
                None => (0.0, 0.0),
            },
            framework_core::primitives::portal::PortalTarget::Named(_) => {
                (0.0, 0.0)
            }
        },
        _ => (0.0, 0.0),
    };

    let children: Vec<WgpuNode> = data.children.clone();
    drop(data);
    let mut nested_overlays = Vec::new();
    // Throwaway sink — nav-top deferral is meaningful only at the
    // root tree walk; an overlay's child walk has no parent
    // navigator to defer paint for. WIP nav-transition work in
    // backend_impl.rs may revisit this once dispatchers mount
    // screens directly.
    let mut nested_nav_tops = Vec::new();
    // Drawer + header hits inside an overlay are unsupported
    // in V1 — overlays don't host drawer navigators or per-screen
    // headers in practice. Throw them into local sinks so the
    // recursion type-checks.
    let mut nested_drawers = Vec::new();
    let mut nested_header_hits = Vec::new();
    for child in &children {
        walk(
            backend,
            text_store,
            skin,
            focused_input_layout,
            caret_visible,
            spinner_phase,
            now,
            viewport_clip,
            child,
            content_x,
            content_y,
            rects,
            texts,
            &mut nested_overlays,
            &mut nested_nav_tops,
            &mut nested_drawers,
            image_requests,
            graphics_requests,
            video_requests,
            &mut nested_header_hits,
        );
    }
    // Nested overlays paint in this same top-z pass against
    // the same viewport — further deferring would just hoist
    // them out of their declared composite order.
    for (child_overlay, _) in nested_overlays {
        walk_overlay(
            backend,
            text_store,
            skin,
            focused_input_layout,
            caret_visible,
            spinner_phase,
            now,
            viewport,
            image_requests,
            graphics_requests,
            video_requests,
            &child_overlay,
            rects,
            texts,
        );
    }
}

/// Compute the top-left origin for an overlay's content given
/// its `placement` and the viewport size. `Top` / `Bottom` etc.
/// pin to one edge with the cross axis full-width; `Center`
/// centers in both axes; `FullScreen` paints at the origin.
fn place_overlay(
    placement: framework_core::primitives::portal::ViewportPlacement,
    content_w: f32,
    content_h: f32,
    viewport_w: f32,
    viewport_h: f32,
) -> (f32, f32) {
    use framework_core::primitives::portal::ViewportPlacement;
    match placement {
        ViewportPlacement::Center => (
            ((viewport_w - content_w) * 0.5).max(0.0),
            ((viewport_h - content_h) * 0.5).max(0.0),
        ),
        ViewportPlacement::Top => (((viewport_w - content_w) * 0.5).max(0.0), 0.0),
        ViewportPlacement::Bottom => (
            ((viewport_w - content_w) * 0.5).max(0.0),
            (viewport_h - content_h).max(0.0),
        ),
        ViewportPlacement::Left => (0.0, ((viewport_h - content_h) * 0.5).max(0.0)),
        ViewportPlacement::Right => (
            (viewport_w - content_w).max(0.0),
            ((viewport_h - content_h) * 0.5).max(0.0),
        ),
        ViewportPlacement::FullScreen => (0.0, 0.0),
    }
}

/// Resolve an anchored overlay's content origin given its
/// trigger rect, the side/align placement, and the offset gap.
/// Returns `(x, y)` in screen-logical coords.
fn position_anchored(
    trigger: (f32, f32, f32, f32),
    content_w: f32,
    content_h: f32,
    side: framework_core::primitives::portal::ElementSide,
    align: framework_core::primitives::portal::ElementAlign,
    offset: f32,
) -> (f32, f32) {
    use framework_core::primitives::portal::{ElementAlign, ElementSide};
    let (tx, ty, tw, th) = trigger;
    // `Above` / `Below` flow vertically (cross-axis = horizontal).
    // `Start` / `End` flow horizontally (cross-axis = vertical).
    let (cross_x, cross_y) = match (side, align) {
        (ElementSide::Above | ElementSide::Below, ElementAlign::Start) => (tx, 0.0),
        (ElementSide::Above | ElementSide::Below, ElementAlign::Center) => {
            (tx + (tw - content_w) * 0.5, 0.0)
        }
        (ElementSide::Above | ElementSide::Below, ElementAlign::End) => {
            (tx + tw - content_w, 0.0)
        }
        (ElementSide::Start | ElementSide::End, ElementAlign::Start) => (0.0, ty),
        (ElementSide::Start | ElementSide::End, ElementAlign::Center) => {
            (0.0, ty + (th - content_h) * 0.5)
        }
        (ElementSide::Start | ElementSide::End, ElementAlign::End) => {
            (0.0, ty + th - content_h)
        }
    };
    match side {
        ElementSide::Above => (cross_x, ty - content_h - offset),
        ElementSide::Below => (cross_x, ty + th + offset),
        ElementSide::Start => (tx - content_w - offset, cross_y),
        ElementSide::End => (tx + tw + offset, cross_y),
    }
}

// ---------------------------------------------------------------------------
// Placeholder paint helpers for primitives that aren't yet
// implemented end-to-end. Each draws an obvious "stand-in" rect
// so authors can see *where* the primitive lives in the tree
// even before the proper pipeline (texture sampling, SVG path
// rasterization, native overlay shell) lands.
// ---------------------------------------------------------------------------

const PLACEHOLDER_BG: [f32; 4] = [0.94, 0.94, 0.96, 1.0];
const PLACEHOLDER_BORDER: [f32; 4] = [0.78, 0.78, 0.82, 1.0];
const PLACEHOLDER_ACCENT: [f32; 4] = [0.55, 0.55, 0.58, 1.0];

/// Image placeholder — light-gray box with a diagonal accent
/// stripe so it reads as "image missing" instead of an empty
/// rect. `src` and `alt` are accepted so a future build can
/// surface them as labels.
fn paint_image_placeholder(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    _src: &str,
    _alt: Option<&str>,
    rects: &mut Vec<RectInstance>,
) {
    rects.push(RectInstance {
        rect: [x, y, w, h],
        bg: srgb_rgba_to_linear(PLACEHOLDER_BG),
        corner_radius: [4.0; 4],
        border_color: srgb_rgba_to_linear(PLACEHOLDER_BORDER),
        border_width: 1.0,
        rotation: 0.0,
        shadow_blur: 0.0, _pad: 0.0,
    });
    // Diagonal "missing-image" stripe across the box.
    let stripe_w = w.hypot(h);
    let stripe_h = 2.0_f32;
    let cx = x + w * 0.5;
    let cy = y + h * 0.5;
    rects.push(RectInstance {
        rect: [cx - stripe_w * 0.5, cy - stripe_h * 0.5, stripe_w, stripe_h],
        bg: srgb_rgba_to_linear(PLACEHOLDER_ACCENT),
        corner_radius: [0.0; 4],
        border_color: [0.0; 4],
        border_width: 0.0,
        rotation: (h / w).atan(),
        shadow_blur: 0.0, _pad: 0.0,
    });
}

/// Lucide stroke width in its native 24-unit viewBox.
const ICON_STROKE_VB: f32 = 2.0;
/// Bezier subdivision count. Higher = smoother curves; the
/// cost is `2*N` extra rects per curve. 8 segments per curve
/// is visually smooth at typical icon sizes.
const ICON_BEZIER_STEPS: usize = 8;
/// Arc subdivision count. Lucide uses arcs for circles + the
/// rounded corners of every gear/badge/dial icon — needs
/// enough segments that the curvature reads cleanly. 16 covers
/// a full circle (`a r r 0 1 1 ...`) at ~22° per segment.
const ICON_ARC_STEPS: usize = 16;

/// Render an icon by parsing its SVG path strings and stroking
/// each subpath as a sequence of rotated capsule rects. Curves
/// (cubic/quadratic) are sampled into polylines at
/// [`ICON_BEZIER_STEPS`] subdivisions. Path commands we don't
/// support (arcs `A`) get skipped — none of the bundled Lucide
/// icons use them.
///
/// This is a stroke-only renderer (no fill) because Lucide's
/// icons are stroke-2 line art. Fill-based SVG icon packs would
/// need a tessellator (lyon) instead.
pub fn paint_icon(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    paths: &[&str],
    view_box: (u16, u16),
    tint: [f32; 4],
    stroke_progress: f32,
    rects: &mut Vec<RectInstance>,
) {
    if w <= 0.0 || h <= 0.0 || view_box.0 == 0 || view_box.1 == 0 {
        return;
    }
    let progress = stroke_progress.clamp(0.0, 1.0);
    if progress <= 0.0 {
        return;
    }
    let vb_w = view_box.0 as f32;
    let vb_h = view_box.1 as f32;
    // Uniform scale; preserve the icon's aspect by fitting
    // inside the smaller dimension. Centers within the slot.
    let scale = (w / vb_w).min(h / vb_h);
    let stroke_w = ICON_STROKE_VB * scale;
    let draw_w = vb_w * scale;
    let draw_h = vb_h * scale;
    let ox = x + (w - draw_w) * 0.5;
    let oy = y + (h - draw_h) * 0.5;
    let to_screen = |p: (f32, f32)| (ox + p.0 * scale, oy + p.1 * scale);

    // Fast path: fully drawn, no length math required.
    if progress >= 1.0 {
        for path in paths {
            for (a, b) in path_segments(path) {
                stroke_segment(to_screen(a), to_screen(b), stroke_w, tint, rects);
            }
        }
        return;
    }

    // Stroke-reveal: paint segments in order until accumulated
    // path length reaches `progress * total_length`. The final
    // boundary segment is truncated to the partial length so
    // the animation has 1-pixel resolution along the path.
    //
    // Path length is computed in viewBox space; the stroke
    // rects produced are still in screen space via `to_screen`,
    // so the length budget is also in viewBox units (consistent
    // with the segment endpoints).
    let mut total: f32 = 0.0;
    for path in paths {
        for (a, b) in path_segments(path) {
            let dx = b.0 - a.0;
            let dy = b.1 - a.1;
            total += (dx * dx + dy * dy).sqrt();
        }
    }
    if total <= 0.0 {
        return;
    }
    let budget = total * progress;
    let mut consumed: f32 = 0.0;
    'outer: for path in paths {
        for (a, b) in path_segments(path) {
            let dx = b.0 - a.0;
            let dy = b.1 - a.1;
            let seg_len = (dx * dx + dy * dy).sqrt();
            if consumed + seg_len <= budget || seg_len <= 0.0 {
                // Full segment fits within budget.
                stroke_segment(to_screen(a), to_screen(b), stroke_w, tint, rects);
                consumed += seg_len;
            } else {
                // Partial segment ends the reveal.
                let remaining = (budget - consumed).max(0.0);
                let t = remaining / seg_len;
                let cut = (a.0 + dx * t, a.1 + dy * t);
                stroke_segment(to_screen(a), to_screen(cut), stroke_w, tint, rects);
                break 'outer;
            }
        }
    }
}

/// Stroke a single line segment as a rotated capsule rect.
///
/// The capsule extends `stroke_w / 2` beyond each endpoint
/// along its length so the *actual* segment endpoint sits at
/// the capsule's solid interior, not on the SDF's antialiased
/// rim. Without that overshoot, two capsules meeting at a
/// shared vertex each contribute ~0.5 alpha at the join — the
/// blend reads as a stitched seam (the "dashed circle" look on
/// arc subdivisions). With it, both endpoints overlap inside
/// full-coverage interior, so polylines look continuous.
fn stroke_segment(
    a: (f32, f32),
    b: (f32, f32),
    stroke_w: f32,
    color: [f32; 4],
    rects: &mut Vec<RectInstance>,
) {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 0.01 {
        // Zero-length segment — render a dot so isolated
        // M-then-Z paths (e.g. a "dot" subpath) still show.
        let r = stroke_w * 0.5;
        rects.push(RectInstance {
            rect: [a.0 - r, a.1 - r, stroke_w, stroke_w],
            bg: srgb_rgba_to_linear(color),
            corner_radius: [r; 4],
            border_color: [0.0; 4],
            border_width: 0.0,
            rotation: 0.0,
            shadow_blur: 0.0, _pad: 0.0,
        });
        return;
    }
    // Atan2 gives the angle from +x; that matches the
    // pipeline's rotation convention (clockwise = positive
    // in y-down screen space).
    let angle = dy.atan2(dx);
    let cx = (a.0 + b.0) * 0.5;
    let cy = (a.1 + b.1) * 0.5;
    // Extend beyond each endpoint by half the stroke width.
    let extended = len + stroke_w;
    rects.push(RectInstance {
        rect: [cx - extended * 0.5, cy - stroke_w * 0.5, extended, stroke_w],
        bg: srgb_rgba_to_linear(color),
        corner_radius: [stroke_w * 0.5; 4],
        border_color: [0.0; 4],
        border_width: 0.0,
        rotation: angle,
        shadow_blur: 0.0, _pad: 0.0,
    });
}

/// Parse an SVG `d` attribute into a stream of `(start, end)`
/// line segments in path-space coordinates. Supports the
/// command subset used by Lucide and similar icon packs:
/// `M`/`m` move, `L`/`l` line, `H`/`h` / `V`/`v` axis-aligned
/// lines, `C`/`c` cubic bezier (subdivided), `Q`/`q` quadratic
/// bezier (subdivided), `A`/`a` elliptical arc (subdivided),
/// `Z`/`z` close-subpath.
///
/// Arc-flag tokens (`large-arc-flag`, `sweep-flag`) are scanned
/// as single chars rather than greedy numbers — SVG allows
/// `a7 7 0 11-14 0` to mean "rx=7 ry=7 phi=0 flag=1 flag=1
/// dx=-14 dy=0", which a generic number tokenizer would
/// misread as `11` and `-14`.
fn path_segments(d: &str) -> Vec<((f32, f32), (f32, f32))> {
    let bytes = d.as_bytes();
    let mut scanner = Scanner { bytes, pos: 0 };
    let mut segments: Vec<((f32, f32), (f32, f32))> = Vec::new();
    let mut cursor = (0.0_f32, 0.0_f32);
    let mut subpath_start = (0.0_f32, 0.0_f32);
    let mut last_cmd: Option<u8> = None;

    loop {
        scanner.skip_separators();
        if scanner.peek().is_none() {
            break;
        }
        // Pick up either an explicit command letter or the
        // implicit continuation of the previous command. Per
        // SVG spec, additional coords after an `M`/`m` are
        // implicit `L`/`l`.
        let cmd = match scanner.peek() {
            Some(c) if c.is_ascii_alphabetic() => {
                scanner.pos += 1;
                last_cmd = Some(c);
                c
            }
            _ => match last_cmd {
                Some(b'M') => {
                    last_cmd = Some(b'L');
                    b'L'
                }
                Some(b'm') => {
                    last_cmd = Some(b'l');
                    b'l'
                }
                Some(other) => other,
                None => break,
            },
        };

        let ok = match cmd {
            b'M' => scanner.read_pair().map(|p| {
                cursor = p;
                subpath_start = cursor;
            }),
            b'm' => scanner.read_pair().map(|p| {
                cursor = (cursor.0 + p.0, cursor.1 + p.1);
                subpath_start = cursor;
            }),
            b'L' => scanner.read_pair().map(|p| {
                segments.push((cursor, p));
                cursor = p;
            }),
            b'l' => scanner.read_pair().map(|p| {
                let next = (cursor.0 + p.0, cursor.1 + p.1);
                segments.push((cursor, next));
                cursor = next;
            }),
            b'H' => scanner.next_number().map(|x| {
                let next = (x, cursor.1);
                segments.push((cursor, next));
                cursor = next;
            }),
            b'h' => scanner.next_number().map(|x| {
                let next = (cursor.0 + x, cursor.1);
                segments.push((cursor, next));
                cursor = next;
            }),
            b'V' => scanner.next_number().map(|y| {
                let next = (cursor.0, y);
                segments.push((cursor, next));
                cursor = next;
            }),
            b'v' => scanner.next_number().map(|y| {
                let next = (cursor.0, cursor.1 + y);
                segments.push((cursor, next));
                cursor = next;
            }),
            b'C' | b'c' => {
                let mut nums = [0.0_f32; 6];
                let mut got = true;
                for slot in &mut nums {
                    match scanner.next_number() {
                        Some(v) => *slot = v,
                        None => {
                            got = false;
                            break;
                        }
                    }
                }
                if !got {
                    None
                } else {
                    let (c1, c2, end) = if cmd == b'C' {
                        ((nums[0], nums[1]), (nums[2], nums[3]), (nums[4], nums[5]))
                    } else {
                        (
                            (cursor.0 + nums[0], cursor.1 + nums[1]),
                            (cursor.0 + nums[2], cursor.1 + nums[3]),
                            (cursor.0 + nums[4], cursor.1 + nums[5]),
                        )
                    };
                    sample_cubic(cursor, c1, c2, end, &mut segments);
                    cursor = end;
                    Some(())
                }
            }
            b'Q' | b'q' => {
                let mut nums = [0.0_f32; 4];
                let mut got = true;
                for slot in &mut nums {
                    match scanner.next_number() {
                        Some(v) => *slot = v,
                        None => {
                            got = false;
                            break;
                        }
                    }
                }
                if !got {
                    None
                } else {
                    let (c1, end) = if cmd == b'Q' {
                        ((nums[0], nums[1]), (nums[2], nums[3]))
                    } else {
                        (
                            (cursor.0 + nums[0], cursor.1 + nums[1]),
                            (cursor.0 + nums[2], cursor.1 + nums[3]),
                        )
                    };
                    sample_quadratic(cursor, c1, end, &mut segments);
                    cursor = end;
                    Some(())
                }
            }
            b'A' | b'a' => {
                let rx = scanner.next_number();
                let ry = scanner.next_number();
                let phi = scanner.next_number();
                let large = scanner.next_flag();
                let sweep = scanner.next_flag();
                let ex = scanner.next_number();
                let ey = scanner.next_number();
                match (rx, ry, phi, large, sweep, ex, ey) {
                    (
                        Some(rx),
                        Some(ry),
                        Some(phi),
                        Some(large),
                        Some(sweep),
                        Some(ex),
                        Some(ey),
                    ) => {
                        let end = if cmd == b'A' {
                            (ex, ey)
                        } else {
                            (cursor.0 + ex, cursor.1 + ey)
                        };
                        sample_arc(
                            cursor,
                            rx,
                            ry,
                            phi,
                            large,
                            sweep,
                            end,
                            &mut segments,
                        );
                        cursor = end;
                        Some(())
                    }
                    _ => None,
                }
            }
            b'Z' | b'z' => {
                if cursor != subpath_start {
                    segments.push((cursor, subpath_start));
                }
                cursor = subpath_start;
                Some(())
            }
            _ => None,
        };
        if ok.is_none() {
            // Unknown command or truncated args — stop here
            // rather than spin.
            break;
        }
    }
    segments
}

fn sample_cubic(
    p0: (f32, f32),
    p1: (f32, f32),
    p2: (f32, f32),
    p3: (f32, f32),
    out: &mut Vec<((f32, f32), (f32, f32))>,
) {
    let mut prev = p0;
    for i in 1..=ICON_BEZIER_STEPS {
        let t = i as f32 / ICON_BEZIER_STEPS as f32;
        let u = 1.0 - t;
        let b = (
            u * u * u * p0.0
                + 3.0 * u * u * t * p1.0
                + 3.0 * u * t * t * p2.0
                + t * t * t * p3.0,
            u * u * u * p0.1
                + 3.0 * u * u * t * p1.1
                + 3.0 * u * t * t * p2.1
                + t * t * t * p3.1,
        );
        out.push((prev, b));
        prev = b;
    }
}

fn sample_quadratic(
    p0: (f32, f32),
    p1: (f32, f32),
    p2: (f32, f32),
    out: &mut Vec<((f32, f32), (f32, f32))>,
) {
    let mut prev = p0;
    for i in 1..=ICON_BEZIER_STEPS {
        let t = i as f32 / ICON_BEZIER_STEPS as f32;
        let u = 1.0 - t;
        let b = (
            u * u * p0.0 + 2.0 * u * t * p1.0 + t * t * p2.0,
            u * u * p0.1 + 2.0 * u * t * p1.1 + t * t * p2.1,
        );
        out.push((prev, b));
        prev = b;
    }
}

/// Streaming reader over an SVG `d` attribute. Distinguishes
/// number reads (greedy, signed, with fraction/exponent) from
/// flag reads (single `0`/`1` char) so the arc command's
/// concatenated flag pair (`a7 7 0 11-14 0`) parses correctly.
struct Scanner<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Scanner<'a> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_separators(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace() || c == b',' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn next_number(&mut self) -> Option<f32> {
        self.skip_separators();
        let start = self.pos;
        if self.peek().map_or(false, |c| c == b'+' || c == b'-') {
            self.pos += 1;
        }
        let int_start = self.pos;
        while self.peek().map_or(false, |c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        let had_int = self.pos > int_start;
        if self.peek() == Some(b'.') {
            self.pos += 1;
            while self.peek().map_or(false, |c| c.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        if self.peek().map_or(false, |c| c == b'e' || c == b'E') {
            self.pos += 1;
            if self.peek().map_or(false, |c| c == b'+' || c == b'-') {
                self.pos += 1;
            }
            while self.peek().map_or(false, |c| c.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        if self.pos == start || (!had_int && self.bytes[start..self.pos].iter().all(|&c| !c.is_ascii_digit())) {
            self.pos = start;
            return None;
        }
        let s = std::str::from_utf8(&self.bytes[start..self.pos]).ok()?;
        s.parse::<f32>().ok()
    }

    fn next_flag(&mut self) -> Option<bool> {
        self.skip_separators();
        match self.peek()? {
            b'0' => {
                self.pos += 1;
                Some(false)
            }
            b'1' => {
                self.pos += 1;
                Some(true)
            }
            _ => None,
        }
    }

    fn read_pair(&mut self) -> Option<(f32, f32)> {
        Some((self.next_number()?, self.next_number()?))
    }
}

/// Sample an SVG elliptical arc into a polyline. Implements
/// the W3C SVG implementation note's endpoint-to-center
/// parameterization, then walks the sweep at
/// [`ICON_ARC_STEPS`] subdivisions.
#[allow(clippy::too_many_arguments)]
fn sample_arc(
    start: (f32, f32),
    rx: f32,
    ry: f32,
    x_axis_rot_deg: f32,
    large_arc: bool,
    sweep: bool,
    end: (f32, f32),
    out: &mut Vec<((f32, f32), (f32, f32))>,
) {
    if (start.0 - end.0).abs() < 1e-4 && (start.1 - end.1).abs() < 1e-4 {
        return;
    }
    let mut rx = rx.abs();
    let mut ry = ry.abs();
    if rx < 1e-4 || ry < 1e-4 {
        out.push((start, end));
        return;
    }

    let phi = x_axis_rot_deg.to_radians();
    let cos_phi = phi.cos();
    let sin_phi = phi.sin();

    // Step 1: midpoint translation + rotation.
    let dx = (start.0 - end.0) * 0.5;
    let dy = (start.1 - end.1) * 0.5;
    let x1p = cos_phi * dx + sin_phi * dy;
    let y1p = -sin_phi * dx + cos_phi * dy;

    // Step 2: scale radii up if necessary.
    let radii_check = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    if radii_check > 1.0 {
        let s = radii_check.sqrt();
        rx *= s;
        ry *= s;
    }

    // Step 3: compute (cx', cy').
    let sign = if large_arc == sweep { -1.0 } else { 1.0 };
    let denom = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let num = (rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p).max(0.0);
    let coef = sign * (num / denom.max(f32::EPSILON)).sqrt();
    let cxp = coef * (rx * y1p / ry);
    let cyp = coef * -(ry * x1p / rx);

    // Step 4: un-rotate + un-translate to find center.
    let cx = cos_phi * cxp - sin_phi * cyp + (start.0 + end.0) * 0.5;
    let cy = sin_phi * cxp + cos_phi * cyp + (start.1 + end.1) * 0.5;

    // Step 5: angles.
    fn vec_angle(ux: f32, uy: f32, vx: f32, vy: f32) -> f32 {
        let dot = ux * vx + uy * vy;
        let len = ((ux * ux + uy * uy) * (vx * vx + vy * vy)).sqrt().max(f32::EPSILON);
        let mut a = (dot / len).clamp(-1.0, 1.0).acos();
        if ux * vy - uy * vx < 0.0 {
            a = -a;
        }
        a
    }
    let theta1 = vec_angle(1.0, 0.0, (x1p - cxp) / rx, (y1p - cyp) / ry);
    let mut delta = vec_angle(
        (x1p - cxp) / rx,
        (y1p - cyp) / ry,
        (-x1p - cxp) / rx,
        (-y1p - cyp) / ry,
    );
    if !sweep && delta > 0.0 {
        delta -= std::f32::consts::TAU;
    } else if sweep && delta < 0.0 {
        delta += std::f32::consts::TAU;
    }

    let mut prev = start;
    for i in 1..=ICON_ARC_STEPS {
        let t = i as f32 / ICON_ARC_STEPS as f32;
        let theta = theta1 + t * delta;
        let ct = theta.cos();
        let st = theta.sin();
        let pt = (
            cos_phi * rx * ct - sin_phi * ry * st + cx,
            sin_phi * rx * ct + cos_phi * ry * st + cy,
        );
        out.push((prev, pt));
        prev = pt;
    }
}

// ---------------------------------------------------------------------------
// Video controls
// ---------------------------------------------------------------------------

/// Tunable bar geometry. `CONTROLS_BAR_H` is the backdrop strip;
/// the play button is square + centered vertically; the scrubber
/// line is thin and full-width minus padding.
const CONTROLS_BAR_H: f32 = 44.0;
const CONTROLS_BTN: f32 = 28.0;
const CONTROLS_PAD: f32 = 12.0;
const CONTROLS_SCRUB_H: f32 = 4.0;
/// Hover-fade window: controls stay visible for this long after
/// the last pointer move, then fade out. Paused videos override
/// this and stay visible.
const CONTROLS_VISIBLE_SECS: f32 = 2.0;
const CONTROLS_FADE_SECS: f32 = 0.25;

thread_local! {
    /// Rects staged by `paint_video_controls` during the tree
    /// walk. Drained by the renderer immediately after the image
    /// pass so the controls paint *on top of* the video texture
    /// instead of being overwritten by it. Avoids threading a new
    /// `&mut Vec<RectInstance>` parameter through every walk
    /// recursion site.
    static VIDEO_CONTROLS_RECTS: std::cell::RefCell<Vec<RectInstance>> =
        std::cell::RefCell::new(Vec::new());
}

/// Drain whatever the latest walk staged. Caller is the renderer's
/// main pass, right after `image.render` finishes — that ordering
/// is what makes controls land above the video texture.
pub(crate) fn take_video_controls_rects() -> Vec<RectInstance> {
    VIDEO_CONTROLS_RECTS.with(|c| std::mem::take(&mut *c.borrow_mut()))
}

/// Paint the video controls bar on top of `(x, y, w, h)` and
/// stash the play-button / scrubber rects back onto the node for
/// pointer hit-testing. `is_playing` switches the icon between
/// play and pause; `cur_micros / dur_micros` drive the scrubber's
/// elapsed fill.
fn paint_video_controls(
    rect: (f32, f32, f32, f32),
    is_playing: bool,
    cur_micros: u64,
    dur_micros: u64,
    // `muted`: Some(true) → muted; Some(false) → audible;
    // None → silent clip with no audio track (hide button).
    muted: Option<bool>,
    last_hover: Option<Instant>,
    now: Instant,
    play_btn_rect: &std::cell::Cell<(f32, f32, f32, f32)>,
    scrubber_rect: &std::cell::Cell<(f32, f32, f32, f32)>,
    mute_btn_rect: &std::cell::Cell<(f32, f32, f32, f32)>,
    _rects_unused: &mut Vec<RectInstance>,
) {
    VIDEO_CONTROLS_RECTS.with(|cell| {
        let mut overlay = cell.borrow_mut();
        paint_video_controls_into(
            rect,
            is_playing,
            cur_micros,
            dur_micros,
            muted,
            last_hover,
            now,
            play_btn_rect,
            scrubber_rect,
            mute_btn_rect,
            &mut overlay,
        );
    });
}

fn paint_video_controls_into(
    rect: (f32, f32, f32, f32),
    is_playing: bool,
    cur_micros: u64,
    dur_micros: u64,
    muted: Option<bool>,
    last_hover: Option<Instant>,
    now: Instant,
    play_btn_rect: &std::cell::Cell<(f32, f32, f32, f32)>,
    scrubber_rect: &std::cell::Cell<(f32, f32, f32, f32)>,
    mute_btn_rect: &std::cell::Cell<(f32, f32, f32, f32)>,
    rects: &mut Vec<RectInstance>,
) {
    let (x, y, w, h) = rect;
    if w < 80.0 || h < 50.0 {
        return;
    }
    // Visibility: paused → always 1.0; playing → fade out 2s after
    // the last pointer move. Fade is linear over CONTROLS_FADE_SECS.
    let alpha = if !is_playing {
        1.0
    } else if let Some(t) = last_hover {
        let since = now.saturating_duration_since(t).as_secs_f32();
        if since < CONTROLS_VISIBLE_SECS {
            1.0
        } else if since < CONTROLS_VISIBLE_SECS + CONTROLS_FADE_SECS {
            1.0 - (since - CONTROLS_VISIBLE_SECS) / CONTROLS_FADE_SECS
        } else {
            0.0
        }
    } else {
        0.0
    };
    if alpha <= 0.01 {
        // Hidden — stash zeroed hit-rects so stray clicks don't
        // resolve to a stale region.
        play_btn_rect.set((0.0, 0.0, 0.0, 0.0));
        scrubber_rect.set((0.0, 0.0, 0.0, 0.0));
        mute_btn_rect.set((0.0, 0.0, 0.0, 0.0));
        return;
    }

    // Bar position pinned to the bottom of the video frame.
    let bar_y = y + h - CONTROLS_BAR_H;
    let bar_h = CONTROLS_BAR_H;

    // 1) Backdrop strip — solid dark with alpha, no fancy
    //    gradient (we don't have multi-stop gradients in the
    //    rect shader). Looks fine on most clips.
    let bg = srgb_rgba_to_linear([0.0, 0.0, 0.0, 0.55 * alpha]);
    rects.push(RectInstance {
        rect: [x, bar_y, w, bar_h],
        bg,
        corner_radius: [0.0, 0.0, 0.0, 0.0],
        border_color: [0.0; 4],
        border_width: 0.0,
        rotation: 0.0,
        shadow_blur: 0.0,
        _pad: 0.0,
    });

    // 2) Play / pause button — a 28×28 hit zone left-aligned in
    //    the bar. We paint Lucide-style outlines so the icon
    //    matches the rest of the framework's iconography.
    let btn_x = x + CONTROLS_PAD;
    let btn_y = bar_y + (bar_h - CONTROLS_BTN) * 0.5;
    play_btn_rect.set((btn_x, btn_y, CONTROLS_BTN, CONTROLS_BTN));
    let tint = [1.0, 1.0, 1.0, alpha];
    // Lucide play (filled triangle) / pause (two bars). Stroke-
    // based — `paint_icon` is stroke-only, so we get the outline
    // look common in minimalist players.
    let play_path = ["M 5 3 L 19 12 L 5 21 Z"];
    let pause_path = ["M 6 4 H 10 V 20 H 6 Z", "M 14 4 H 18 V 20 H 14 Z"];
    if is_playing {
        paint_icon(btn_x, btn_y, CONTROLS_BTN, CONTROLS_BTN, &pause_path, (24, 24), tint, 1.0, rects);
    } else {
        paint_icon(btn_x, btn_y, CONTROLS_BTN, CONTROLS_BTN, &play_path, (24, 24), tint, 1.0, rects);
    }

    // 3) Mute button — right-aligned in the bar, mirror of play.
    //    Only painted when there's an audio track on the clip;
    //    silent videos hide the button entirely.
    let mute_btn_x = x + w - CONTROLS_PAD - CONTROLS_BTN;
    let mute_btn_y = btn_y;
    let has_audio = muted.is_some();
    if has_audio {
        mute_btn_rect.set((mute_btn_x, mute_btn_y, CONTROLS_BTN, CONTROLS_BTN));
        // Speaker body — same for both states. Lucide "volume"
        // body path, stroked.
        let speaker_body = ["M 11 5 L 6 9 H 2 V 15 H 6 L 11 19 Z"];
        paint_icon(
            mute_btn_x,
            mute_btn_y,
            CONTROLS_BTN,
            CONTROLS_BTN,
            &speaker_body,
            (24, 24),
            tint,
            1.0,
            rects,
        );
        // Decorator: "X" when muted, two wave indicators when
        // audible. Both use straight-line approximations so
        // `paint_icon`'s stroke renderer can draw them — the
        // existing Lucide arcs would require `A` (arc) support
        // we don't have. Visually close enough to read.
        let decorator: &[&str] = if muted == Some(true) {
            &["M 16 9 L 22 15", "M 22 9 L 16 15"]
        } else {
            &["M 14 9 V 15", "M 17 7 V 17", "M 20 5 V 19"]
        };
        paint_icon(
            mute_btn_x,
            mute_btn_y,
            CONTROLS_BTN,
            CONTROLS_BTN,
            decorator,
            (24, 24),
            tint,
            1.0,
            rects,
        );
    } else {
        mute_btn_rect.set((0.0, 0.0, 0.0, 0.0));
    }

    // 4) Scrubber line — fills the space between the play and
    //    mute buttons (or out to the right padding if there's no
    //    audio). Background track at low alpha; elapsed fill at
    //    full white. The visual line is 4 px tall but the *hit*
    //    rect spans the full bar height so users don't have to
    //    pixel-aim at the thin track.
    let scrub_x = btn_x + CONTROLS_BTN + CONTROLS_PAD;
    let scrub_right = if has_audio {
        mute_btn_x - CONTROLS_PAD
    } else {
        x + w - CONTROLS_PAD
    };
    let scrub_w_total = scrub_right - scrub_x;
    let scrub_y = bar_y + (bar_h - CONTROLS_SCRUB_H) * 0.5;
    if scrub_w_total > 8.0 {
        // Hit rect covers the full bar vertically; the painted
        // track stays slim.
        scrubber_rect.set((scrub_x, bar_y, scrub_w_total, bar_h));
        // Track.
        rects.push(RectInstance {
            rect: [scrub_x, scrub_y, scrub_w_total, CONTROLS_SCRUB_H],
            bg: srgb_rgba_to_linear([1.0, 1.0, 1.0, 0.30 * alpha]),
            corner_radius: [
                CONTROLS_SCRUB_H * 0.5,
                CONTROLS_SCRUB_H * 0.5,
                CONTROLS_SCRUB_H * 0.5,
                CONTROLS_SCRUB_H * 0.5,
            ],
            border_color: [0.0; 4],
            border_width: 0.0,
            rotation: 0.0,
            shadow_blur: 0.0,
            _pad: 0.0,
        });
        // Elapsed fill — proportional to current_time / duration.
        let progress = if dur_micros > 0 {
            (cur_micros as f64 / dur_micros as f64).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        let fill_w = scrub_w_total * progress;
        if fill_w > 0.0 {
            rects.push(RectInstance {
                rect: [scrub_x, scrub_y, fill_w, CONTROLS_SCRUB_H],
                bg: srgb_rgba_to_linear([1.0, 1.0, 1.0, 0.95 * alpha]),
                corner_radius: [
                    CONTROLS_SCRUB_H * 0.5,
                    CONTROLS_SCRUB_H * 0.5,
                    CONTROLS_SCRUB_H * 0.5,
                    CONTROLS_SCRUB_H * 0.5,
                ],
                border_color: [0.0; 4],
                border_width: 0.0,
                rotation: 0.0,
                shadow_blur: 0.0,
                _pad: 0.0,
            });
            // Playhead — a small circle at the end of the fill.
            let head_d = CONTROLS_SCRUB_H * 2.5;
            let head_x = scrub_x + fill_w - head_d * 0.5;
            let head_y = scrub_y + CONTROLS_SCRUB_H * 0.5 - head_d * 0.5;
            rects.push(RectInstance {
                rect: [head_x, head_y, head_d, head_d],
                bg: srgb_rgba_to_linear([1.0, 1.0, 1.0, alpha]),
                corner_radius: [head_d * 0.5, head_d * 0.5, head_d * 0.5, head_d * 0.5],
                border_color: [0.0; 4],
                border_width: 0.0,
                rotation: 0.0,
                shadow_blur: 0.0,
                _pad: 0.0,
            });
        }
    } else {
        scrubber_rect.set((0.0, 0.0, 0.0, 0.0));
    }
}

/// Tab-bar strip at the bottom of a TabNavigator. Renders
/// `tab_count` evenly-spaced "tab buttons" with the active one
/// highlighted. The button hit-region is handled by the host's
/// pointer pipeline when wired; for now this is visual only.
fn paint_tab_bar(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    active_tab: usize,
    tab_count: usize,
    bar_bg_override: Option<[f32; 4]>,
    rects: &mut Vec<RectInstance>,
) {
    use crate::node::TAB_BAR_HEIGHT;
    let bar_y = y + h - TAB_BAR_HEIGHT;
    // Bar background — author override beats the neutral gray
    // default. The override comes from the navigator's
    // `bar_style` (set via `.tab_bar_style(...)`).
    let bar_bg = bar_bg_override.unwrap_or([0.96, 0.96, 0.97, 1.0]);
    rects.push(RectInstance {
        rect: [x, bar_y, w, TAB_BAR_HEIGHT],
        bg: srgb_rgba_to_linear(bar_bg),
        corner_radius: [0.0; 4],
        border_color: srgb_rgba_to_linear([0.86, 0.86, 0.88, 1.0]),
        border_width: 1.0,
        rotation: 0.0,
        shadow_blur: 0.0, _pad: 0.0,
    });
    if tab_count == 0 {
        return;
    }
    let tab_w = w / tab_count as f32;
    let active_w = tab_w * 0.6;
    let active_h = 4.0;
    let active_x = x + active_tab as f32 * tab_w + (tab_w - active_w) * 0.5;
    let active_y = bar_y + 4.0;
    rects.push(RectInstance {
        rect: [active_x, active_y, active_w, active_h],
        bg: srgb_rgba_to_linear([0.0, 0x7a as f32 / 255.0, 1.0, 1.0]),
        corner_radius: [active_h * 0.5; 4],
        border_color: [0.0; 4],
        border_width: 0.0,
        rotation: 0.0,
        shadow_blur: 0.0, _pad: 0.0,
    });
}

/// "Not supported in this simulator" panel for WebView / Video /
/// Graphics. A striped warning-colored box with a horizontal
/// caption stripe across the middle; authors immediately see
/// *what* would have rendered.
fn paint_unsupported(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    _label: &'static str,
    rects: &mut Vec<RectInstance>,
) {
    rects.push(RectInstance {
        rect: [x, y, w, h],
        bg: srgb_rgba_to_linear([0.99, 0.95, 0.86, 1.0]),
        corner_radius: [6.0; 4],
        border_color: srgb_rgba_to_linear([0.88, 0.78, 0.45, 1.0]),
        border_width: 1.0,
        rotation: 0.0,
        shadow_blur: 0.0, _pad: 0.0,
    });
    // Horizontal accent stripe.
    let stripe_h = 3.0_f32;
    rects.push(RectInstance {
        rect: [x + 12.0, y + (h - stripe_h) * 0.5, (w - 24.0).max(0.0), stripe_h],
        bg: srgb_rgba_to_linear([0.78, 0.62, 0.20, 0.55]),
        corner_radius: [stripe_h * 0.5; 4],
        border_color: [0.0; 4],
        border_width: 0.0,
        rotation: 0.0,
        shadow_blur: 0.0, _pad: 0.0,
    });
}

thread_local! {
    /// First-frame `Instant`. Used as the epoch for caret blink
    /// phase math so the blink rhythm is stable across the
    /// session. Lazily initialized on first frame.
    static RENDER_EPOCH: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

/// `true` if a blinking caret should be visible at `now`.
/// On for the first half of [`CARET_BLINK_PERIOD_SEC`], off for
/// the second half. All focused inputs share the same phase.
fn caret_blink_visible(now: Instant) -> bool {
    let epoch = render_epoch(now);
    let phase = now.duration_since(epoch).as_secs_f32() % CARET_BLINK_PERIOD_SEC;
    phase < CARET_BLINK_PERIOD_SEC * 0.5
}

/// Spinner rotation phase in `[0.0, 1.0)`. Shared epoch with the
/// caret blink so the two stay in lockstep across the session.
fn spinner_phase(now: Instant) -> f32 {
    let epoch = render_epoch(now);
    let secs = now.duration_since(epoch).as_secs_f32();
    (secs / ACTIVITY_INDICATOR_SPIN_PERIOD_SEC).fract()
}

/// Lazily initialize and return the render epoch. The first frame
/// rendered seeds it; all later calls return the same instant so
/// time-based phase math stays stable.
fn render_epoch(now: Instant) -> Instant {
    RENDER_EPOCH.with(|e| match e.get() {
        Some(t) => t,
        None => {
            e.set(Some(now));
            now
        }
    })
}

// ---------------------------------------------------------------------------
// Image decode + upload
// ---------------------------------------------------------------------------

/// Resolve an image `src` to a decoded `image::RgbaImage`,
/// upload it to a GPU texture, and build the bind group the
/// `ImagePipeline` will sample from. Returns `None` on IO or
/// decode failure — the caller caches that and falls back to
/// the missing-image placeholder.
///
/// `src` is currently treated as a filesystem path (absolute
/// or relative to cwd). Future loaders (`http://`, embedded
/// asset URIs) can branch on the scheme here without touching
/// the cache or pipeline.
fn decode_and_upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &crate::image_pipeline::ImagePipeline,
    src: &str,
) -> Option<ImageEntry> {
    let resolved = resolve_asset_path(src).or_else(|| {
        log::warn!(
            "image::resolve({src:?}) — not found in cwd, exe dir, or workspace root"
        );
        None
    })?;
    let bytes = std::fs::read(&resolved)
        .map_err(|e| log::warn!("image::read({src:?} → {resolved:?}) failed: {e}"))
        .ok()?;
    let decoded = image::load_from_memory(&bytes)
        .map_err(|e| log::warn!("image::decode({src:?}) failed: {e}"))
        .ok()?
        .to_rgba8();
    let (w, h) = decoded.dimensions();

    // sRGB texture format — the surface is sRGB-encoded, so a
    // linear-space sampler write here would gamma-shift colors.
    // The image crate produces 8-bit sRGB RGBA; tagging the
    // texture as `Rgba8UnormSrgb` lets the GPU handle the
    // sRGB→linear conversion at sample time for free.
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("image-texture"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &decoded,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * w),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("image-texture-bg"),
        layout: &pipeline.texture_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&pipeline.sampler),
            },
        ],
    });
    Some(ImageEntry { texture, view, bind_group, size: (w, h) })
}

/// Resolve a user-supplied image path to an absolute path on
/// disk. Tries — in order — the literal path, the current
/// working directory, the executable's directory (for
/// distributed binaries that ship assets next to the
/// executable), and a walk up from cwd looking for a
/// `Cargo.lock` (the workspace root, useful in dev when
/// running from a sub-crate's directory). Returns `None` if
/// none of them exist; the loader caches that as a `Failed`
/// state so we don't re-walk the FS every frame.
fn resolve_asset_path(src: &str) -> Option<std::path::PathBuf> {
    use std::path::{Path, PathBuf};
    let raw = Path::new(src);
    if raw.is_absolute() {
        return if raw.exists() { Some(raw.to_path_buf()) } else { None };
    }
    // Candidate roots, in priority order.
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        roots.push(cwd.clone());
        // Walk up from cwd looking for the workspace root (the
        // first parent that owns a `Cargo.lock`).
        let mut p = cwd;
        while let Some(parent) = p.parent().map(|p| p.to_path_buf()) {
            if parent.join("Cargo.lock").exists() {
                roots.push(parent);
                break;
            }
            // Stop at the FS root.
            if parent.parent().is_none() {
                break;
            }
            p = parent;
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            // For `cargo run`, the exe lives in `target/debug`
            // (or `target/release`) — its grandparent is the
            // workspace root.
            roots.push(parent.to_path_buf());
            if let Some(target_root) = parent.parent().and_then(|p| p.parent()) {
                roots.push(target_root.to_path_buf());
            }
        }
    }
    for root in &roots {
        let candidate = root.join(src);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}
