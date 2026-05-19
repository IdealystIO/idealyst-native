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

use std::time::Instant;

use glyphon::TextBounds;
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
use crate::text::{render_text, StagedText, TextCtx, TextStore};

/// GPU-side rendering bundle. Holds the rect pipeline + the
/// glyphon text context. One per surface.
pub struct Renderer {
    pub rect: RectPipeline,
    pub text: TextCtx,
}

impl Renderer {
    /// Create the renderer's GPU resources. `format` must match
    /// the surface's color format; the rect pipeline is created
    /// against it.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Self {
        Self {
            rect: RectPipeline::new(device, format),
            text: TextCtx::new(device, queue, format),
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

        // Run Taffy layout against the logical viewport.
        let root = host.backend().borrow().root();
        if let Some(root) = root.as_ref() {
            let mut backend = host.backend().borrow_mut();
            let root_layout = root.borrow().layout;
            backend.layout.compute(root_layout, viewport[0], viewport[1]);
        }

        // Hold the immutable borrows for the rest of the frame.
        // The text store is its own `Rc<RefCell<>>` so glyphon
        // buffer refs stay valid across the encode.
        let backend = host.backend().borrow();
        let text_store = host.text_store().borrow();
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
            );
        }

        // On-screen keyboard overlay. Painted *after* the tree
        // walk so it sits on top of everything; the host's
        // `pointer_down` route ensures taps on keys never reach
        // the content beneath.
        if host.keyboard_visible() {
            keyboard::paint(
                skin.as_ref(),
                (viewport[0], viewport[1]),
                keyboard_slide,
                &host.keyboard_glyphs,
                &mut rects,
                &mut texts,
            );
        }

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("idealyst-frame"),
        });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("idealyst-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // Window aspect is locked by the host, so
                        // content fills the surface — clear color
                        // is only visible for one frame at most
                        // (before the app's bg paints over it).
                        // Plain white keeps the first frame from
                        // flashing.
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
            });
            // Constrain rasterization to the letterboxed content
            // rect. Both the rect pipeline and glyphon's text
            // renderer output NDC against `logical_viewport`, and
            // NDC then maps to whatever `set_viewport` defined —
            // so content scales to fill the rect while keeping
            // its aspect.
            let (vx, vy, vw, vh) = surface_viewport;
            pass.set_viewport(vx, vy, vw.max(1.0), vh.max(1.0), 0.0, 1.0);
            self.rect.render(device, queue, &mut pass, viewport, &rects);
            let mut fs = host.font_system().borrow_mut();
            let _ = render_text(
                &mut self.text,
                &mut fs,
                device,
                queue,
                &mut pass,
                [viewport[0] as u32, viewport[1] as u32],
                &texts,
            );
        }

        queue.submit(std::iter::once(encoder.finish()));
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
        if has_bg || any_border {
            let bg_rest = r.background.unwrap_or([0.0; 4]);
            let bg = backend.animator.sample_color(
                TweenKey::new(data.layout, AnimProperty::BackgroundColor),
                bg_rest,
                now,
            );
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
                _pad: [0.0; 2],
            });
        }
    }

    if in_clip {
        match &data.kind {
            NodeKind::Text { .. } | NodeKind::Button { .. } => {
                if let Some(entry) = text_store.buffers.get(&data.layout) {
                    let color = backend.animator.sample_color(
                        TweenKey::new(data.layout, AnimProperty::TextColor),
                        r.color,
                        now,
                    );
                    let tb = intersect_rect((x, y, w, h), clip);
                    texts.push(StagedText {
                        buffer: &entry.buffer,
                        x,
                        y,
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
            _ => {}
        }
    }

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
    drop(data);
    for child in &children {
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
            child_origin_x,
            child_origin_y,
            rects,
            texts,
        );
    }

    // Scrollbar overlay. Drawn after children so it sits on top
    // of the content. iOS-style overlay scrollbar: thin gray
    // translucent thumb pinned to the trailing edge of the
    // scrollview. Only painted when the content overflows.
    if let Some((horizontal, off_x, off_y)) = scrollbar_state {
        paint_scrollbar(backend, node, x, y, w, h, horizontal, off_x, off_y, rects);
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
            _pad: [0.0; 2],
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
            _pad: [0.0; 2],
        });
    }
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
