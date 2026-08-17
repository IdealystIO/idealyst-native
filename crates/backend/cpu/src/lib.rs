//! CPU-rasterizer backend for `runtime_shared::Backend`.
//!
//! Renders the framework's primitive tree into a pixel framebuffer
//! using a pure-Rust software rasterizer. Output is decoupled from
//! storage by the [`Surface`] trait — the same `CpuBackend` drives
//! an in-memory buffer ([`MemSurface`]), a desktop preview window
//! (via a `Surface` impl that wraps `softbuffer`), or an ESP32 SPI
//! display ([`Surface`] impl that wraps `esp-idf-hal`'s SPI
//! driver). The backend itself doesn't know which.
//!
//! ## Layout
//!
//! Same Taffy-via-`runtime-layout` model the iOS / Android /
//! Terminal backends use. The framework's `StyleRules` translate to
//! Taffy's Flex semantics 1:1; we walk the laid-out tree at paint
//! time and emit rectangles + text glyphs in painter's-algorithm
//! order.
//!
//! ## Render lifecycle
//!
//! ```ignore
//! let mut backend = CpuBackend::new(320, 240);
//! // ... mount framework tree into backend via `runtime_shared::mount` ...
//! let mut surface = MemSurface::new(320, 240);
//! backend.render(&mut surface);
//! // `surface.pixels()` now contains the rendered frame.
//! ```
//!
//! ## New core (`new-core` feature)
//!
//! With the `new-core` feature, [`newcore::start`] mounts a
//! scene-element tree on a `runtime_world::World` through the
//! vocabulary's builtin handlers — `CpuBackend` implements
//! `runtime_scene::Host` plus all 30 capability traits by UFCS
//! delegation to this same `Backend` impl, so the rasterizer renders
//! identical pixels on both cores (pinned byte-for-byte by
//! `tests/newcore_parity.rs`). Additive: the old-core mount path is
//! untouched with or without the feature.
//!
//! ## Scope of this MVP
//!
//! Implemented: View, Text, Button, Pressable, ScrollView; solid +
//! rounded-rect backgrounds with alpha; per-side borders; built-in
//! 8x8 bitmap font; click hit-testing.
//!
//! **Unsupported primitives render a visible placeholder text node**
//! (`"<PrimitiveName> not supported on CPU backend"`), not a silent
//! no-op. See the README for the full matrix. The deliberate
//! placeholder posture comes from `feedback_cpu_unsupported_placeholders`:
//! we want missing support to be SEEN on the device, not masked.
//!
//! Deferred (still in scope, can be added without breaking the
//! MCU constraint): gradients are already in, image decode behind a
//! feature flag, icon path rasterization, per-frame animation,
//! raw-touch dispatch refinements.

mod font_8x8;
mod node;
mod raster;
mod surface;

// Post-dispatch hook slot — UNCONDITIONAL (the fire sites live in
// host crates, which can't see this crate's features); a no-op single
// Cell read unless `newcore::start` installed the flush driver. See
// the module docs for the embedder contract (the CPU backend owns no
// scheduler — the host decides cadence).
pub mod dispatch_hook;

/// `runtime_scene::Host` + the 30 capability traits on [`CpuBackend`],
/// plus the boot entry and flush driver.
pub mod newcore;

pub use node::{CpuNode, NodeKind};
pub use surface::{MemSurface, Surface};

use std::collections::HashMap;
use std::rc::Rc;

use runtime_shared::color::Rgba;
use runtime_shared::{GradientKind, Length, RadialExtent};
use runtime_layout::LayoutTree;

use node::{NodeData, ResolvedGradient};
use raster::{blend_over, fill_rounded_rect_blended, premultiply_alpha, stroke_border, Rect};

// ---------------------------------------------------------------------------
// ClickOutcome — same shape as the terminal backend
// ---------------------------------------------------------------------------

/// Outcome of [`CpuBackend::dispatch_click`]. The host pattern-
/// matches this and fires `HandlerFired`'s closure **after** it
/// releases its `&mut self` borrow on the backend — the closure
/// will typically call `Signal::set`, which re-enters the framework
/// and would panic with "RefCell already borrowed" if invoked
/// inline. Same posture as the terminal backend.
pub enum ClickOutcome {
    /// Click landed on a clickable node. Host fires the handler
    /// once the backend borrow is released.
    HandlerFired(Rc<dyn Fn()>),
    /// Click landed somewhere with no handler.
    Unhandled,
}

impl std::fmt::Debug for ClickOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClickOutcome::HandlerFired(_) => f.write_str("HandlerFired"),
            ClickOutcome::Unhandled => f.write_str("Unhandled"),
        }
    }
}

// ---------------------------------------------------------------------------
// CpuBackend
// ---------------------------------------------------------------------------

/// CPU-rasterizer backend. One per render root. Owns the parallel
/// Taffy layout tree and every node's data.
pub struct CpuBackend {
    pub(crate) layout: LayoutTree,
    pub(crate) nodes: HashMap<u32, NodeData>,
    pub(crate) next_id: u32,
    /// Viewport dimensions in pixels. The host updates this on
    /// resize via [`CpuBackend::set_viewport`].
    pub(crate) viewport: (u32, u32),
    /// Color the framebuffer is cleared to at the start of each
    /// render. Default opaque black; the host can override via
    /// [`CpuBackend::set_clear_color`] for backends that want a
    /// known background under translucent root views.
    pub(crate) clear_color: [u8; 4],
}

impl Default for CpuBackend {
    fn default() -> Self {
        Self::new(320, 240)
    }
}

impl CpuBackend {
    /// Fresh backend sized to a `width x height` viewport in pixels.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            layout: LayoutTree::new(),
            nodes: HashMap::new(),
            next_id: 1,
            viewport: (width, height),
            clear_color: [0, 0, 0, 255],
        }
    }

    /// Update the viewport size — call from the host when the
    /// underlying surface resizes (window resize, orientation flip,
    /// SPI display swap). Next render uses the new dimensions for
    /// Taffy compute.
    pub fn set_viewport(&mut self, width: u32, height: u32) {
        self.viewport = (width, height);
        // Mirror into the framework's reactive viewport signal.
        // Reactive subscribers (breakpoint hooks, responsive
        // containers) re-fire here even though the CPU backend has
        // no native resize event to drive this — the host is in
        // charge of calling `set_viewport` and so the host is in
        // charge of when the signal updates.
        runtime_shared::set_viewport_size(runtime_shared::ViewportSize {
            width: width as f32,
            height: height as f32,
        });
        // Forward the same pixel counts into the mounted world's viewport
        // ctx (captured signal + deduped flush — the backend-web
        // resize-listener discipline). Kept right beside the TLS write
        // above so the two sinks can never diverge. No-op unless an app
        // is booted.
        newcore::forward_viewport(runtime_shared::ViewportSize {
            width: width as f32,
            height: height as f32,
        });
    }

    /// Current viewport dimensions in pixels — `(width, height)`.
    /// Hosts read this back when translating input-event coordinates
    /// from physical window pixels into framebuffer pixels.
    pub fn viewport(&self) -> (u32, u32) {
        self.viewport
    }

    /// Set the color the framebuffer is cleared to before each
    /// paint. Useful for hosts that want translucent root views to
    /// composite onto a known background.
    pub fn set_clear_color(&mut self, rgba: [u8; 4]) {
        self.clear_color = rgba;
    }

    /// Allocate a new backend-side node and the matching Taffy slot.
    /// Used by every `create_*` impl.
    fn alloc_node(&mut self, kind: NodeKind, content: String) -> CpuNode {
        let id = self.next_id;
        self.next_id += 1;
        let layout = self.layout.new_node();
        self.nodes.insert(id, NodeData::new(kind, content, layout));
        CpuNode { id }
    }

    /// Walk the backend's nodes and find the application root. Same
    /// "lowest-id parentless node" rule the terminal backend uses —
    /// the first node created (id 1) is always the real root; any
    /// other parentless node is a transient orphan from a mid-flight
    /// reactive swap.
    fn find_root(&self) -> Option<u32> {
        let mut best: Option<u32> = None;
        for (id, data) in &self.nodes {
            if self.layout.is_root(data.layout) {
                best = Some(match best {
                    Some(b) => b.min(*id),
                    None => *id,
                });
            }
        }
        best
    }

    /// Render the current tree into `surface`. Recomputes layout
    /// against the viewport size, then walks the tree painter's-
    /// algorithm order (root first, children on top). Surface is
    /// cleared to [`Self::clear_color`] before painting; surfaces
    /// that maintain their own back-buffer can override `clear_color`
    /// to skip the redundant clear.
    pub fn render<S: Surface>(&mut self, surface: &mut S) {
        // Always clear + flush + present, even when the tree is
        // empty. The host's contract is "render produces a complete
        // frame on `surface`" — a no-root short-circuit would leave
        // the framebuffer in whatever state the previous frame put
        // it in (or, on first paint, uninitialized).
        let clip = Rect::surface(surface.width(), surface.height());
        surface.fill_rect(0, 0, surface.width(), surface.height(), self.clear_color);

        if let Some(root_id) = self.find_root() {
            if let Some(root_layout) = self.nodes.get(&root_id).map(|d| d.layout) {
                let (vw, vh) = self.viewport;
                self.layout.compute(root_layout, vw as f32, vh as f32);
                self.paint_node(surface, root_id, 0.0, 0.0, 1.0, clip);
            }
        }
        surface.flush_rect(0, 0, surface.width(), surface.height());
        surface.present();
    }

    /// Recursive paint walker. `(parent_x, parent_y)` is the parent's
    /// content-box origin in surface coordinates; `parent_opacity` is
    /// the cumulative product of ancestor opacities; `clip` is the
    /// active clip rectangle.
    fn paint_node<S: Surface>(
        &self,
        surface: &mut S,
        id: u32,
        parent_x: f32,
        parent_y: f32,
        parent_opacity: f32,
        clip: Rect,
    ) {
        let Some(data) = self.nodes.get(&id) else { return };
        let frame = self.layout.frame_of(data.layout);
        // Animated opacity, when set, replaces the static value
        // (NOT multiplied with it) — the AnimatedValue carries the
        // intended end-state opacity, not a delta. Matches every
        // other backend in the repo.
        let effective_self_opacity = data.animated_opacity.unwrap_or(data.opacity);
        let opacity = parent_opacity * effective_self_opacity;
        if opacity <= 0.001 {
            return;
        }

        // Resolve static translate (px / pct of own size) plus the
        // animation-driven translate (px, additive).
        let tx = data
            .static_translate_x
            .as_ref()
            .map(|l| resolve_length(l, frame.width))
            .unwrap_or(0.0)
            + data.animated_translate_x;
        let ty = data
            .static_translate_y
            .as_ref()
            .map(|l| resolve_length(l, frame.height))
            .unwrap_or(0.0)
            + data.animated_translate_y;

        let x = parent_x + frame.x + tx;
        let y = parent_y + frame.y + ty;
        let w = frame.width.max(0.0);
        let h = frame.height.max(0.0);
        let rect = Rect::new(x.round() as i32, y.round() as i32, w.round() as u32, h.round() as u32);

        // -----------------------------------------------------------------
        // Paint background + border for this node.
        // -----------------------------------------------------------------
        // Sampler the rasterizer uses to read framebuffer state for
        // alpha blending. For surfaces that don't support readback
        // (write-only SPI), we degrade to opaque blend against the
        // clear color — alpha-on-alpha won't accumulate, but solid
        // colors and the most common "opaque background over opaque
        // background" case still works. This is a known limitation.
        let clear = self.clear_color;
        let dst_sampler = move |_s: &S, _x: u32, _y: u32| -> [u8; 4] { clear };

        // Background paint. Gradient takes precedence over solid
        // color — same posture as every other backend.
        if let Some(gradient) = data.gradient.as_ref() {
            paint_gradient(
                surface,
                rect,
                data.corner_radii,
                gradient,
                opacity,
                clip,
                clear,
            );
        } else {
            // `animated_bg` wins over the static `bg`. Splitting
            // these slots keeps an `apply_style` re-fire (hot patch,
            // theme refresh, state overlay) from clobbering an
            // in-flight animation. Matches the terminal backend.
            let chosen = data.animated_bg.or(data.bg);
            if let Some(bg) = chosen {
                let color = premultiply_alpha([bg.r, bg.g, bg.b, bg.a], opacity);
                if color[3] > 0 {
                    fill_rounded_rect_blended(
                        surface,
                        rect,
                        data.corner_radii,
                        color,
                        clip,
                        dst_sampler,
                    );
                }
            }
        }

        // -----------------------------------------------------------------
        // Text content (Text + Button labels)
        // -----------------------------------------------------------------
        if matches!(data.kind, NodeKind::Text | NodeKind::Button) && !data.content.is_empty() {
            // `animated_fg` wins over the static `fg`, same as background.
            let fg = data.animated_fg.or(data.fg).unwrap_or(Rgba::BLACK);
            let fg_color = premultiply_alpha([fg.r, fg.g, fg.b, fg.a], opacity);
            let scale = data
                .font_size_px
                .map(|px| (px / font_8x8::GLYPH_H as f32).max(1.0).round() as u32)
                .unwrap_or(1);
            // Default placement: top-left of the content box. The
            // framework's text alignment + line height map to padding
            // in Taffy, so we don't have to redo alignment here.
            let text_x = x.round() as i32;
            let text_y = y.round() as i32;
            draw_text(surface, &data.content, text_x, text_y, scale, fg_color, clip);
        }

        // -----------------------------------------------------------------
        // Border on top of the content.
        // -----------------------------------------------------------------
        // Borders draw last so they sit above the background and the
        // text. Matches CSS painting order: backgrounds → contents → borders.
        let border_colors = [
            data.border_colors[0].map(|c| premultiply_alpha([c.r, c.g, c.b, c.a], opacity)),
            data.border_colors[1].map(|c| premultiply_alpha([c.r, c.g, c.b, c.a], opacity)),
            data.border_colors[2].map(|c| premultiply_alpha([c.r, c.g, c.b, c.a], opacity)),
            data.border_colors[3].map(|c| premultiply_alpha([c.r, c.g, c.b, c.a], opacity)),
        ];
        if data.border_widths.iter().any(|w| *w > 0.0) {
            stroke_border(
                surface,
                rect,
                data.border_widths,
                border_colors,
                clip,
                dst_sampler,
            );
        }

        // -----------------------------------------------------------------
        // Recurse into children. ScrollView clips its children to its
        // own frame; non-scroll containers don't clip (a child with
        // `position: absolute` can legitimately spill outside its
        // parent's box).
        // -----------------------------------------------------------------
        let (child_clip, child_off_x, child_off_y) = if matches!(data.kind, NodeKind::ScrollView) {
            let inner_clip = rect.intersect(clip).unwrap_or(clip);
            (inner_clip, data.scroll_x, data.scroll_y)
        } else {
            (clip, 0.0, 0.0)
        };
        // Sort children by z-index — higher z paints later (in
        // front). Stable sort preserves insertion order between
        // ties, matching the natural "later sibling on top" default.
        let ordered = self.children_in_z_order(&data.children);
        for child_id in ordered {
            self.paint_node(
                surface,
                child_id,
                x - child_off_x,
                y - child_off_y,
                opacity,
                child_clip,
            );
        }
    }

    /// Sort `children` by their `z_index`, stably (insertion order
    /// breaks ties). Higher z paints last (on top). Returns a fresh
    /// `Vec` rather than mutating the caller's children list so the
    /// per-frame sort doesn't reorder the framework-visible tree.
    fn children_in_z_order(&self, children: &[u32]) -> Vec<u32> {
        let mut indexed: Vec<(u32, f32, usize)> = children
            .iter()
            .enumerate()
            .map(|(idx, id)| {
                let z = self.nodes.get(id).map(|d| d.z_index).unwrap_or(0.0);
                (*id, z, idx)
            })
            .collect();
        // Sort primarily by z (ascending), secondarily by original
        // index — gives a stable "later sibling on top, with z as
        // the override" ordering.
        indexed.sort_by(|a, b| {
            a.1.partial_cmp(&b.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.2.cmp(&b.2))
        });
        indexed.into_iter().map(|(id, _, _)| id).collect()
    }

    /// Dispatch a click at surface pixel coordinates `(x, y)`. Walks
    /// the tree deepest-first and returns the first handler whose
    /// hit-rect contains the point — or `Unhandled` if no clickable
    /// node was hit. Must be called after at least one `render` so
    /// the frame cache is populated.
    pub fn dispatch_click(&mut self, x: u32, y: u32) -> ClickOutcome {
        let Some(root) = self.find_root() else { return ClickOutcome::Unhandled };
        let mut hit: Option<Rc<dyn Fn()>> = None;
        self.hit_test(root, 0.0, 0.0, x as f32, y as f32, &mut hit);
        match hit {
            Some(h) => ClickOutcome::HandlerFired(h),
            None => ClickOutcome::Unhandled,
        }
    }

    fn hit_test(
        &self,
        id: u32,
        parent_x: f32,
        parent_y: f32,
        px: f32,
        py: f32,
        out: &mut Option<Rc<dyn Fn()>>,
    ) {
        let Some(data) = self.nodes.get(&id) else { return };
        let frame = self.layout.frame_of(data.layout);
        let tx = data
            .static_translate_x
            .as_ref()
            .map(|l| resolve_length(l, frame.width))
            .unwrap_or(0.0);
        let ty = data
            .static_translate_y
            .as_ref()
            .map(|l| resolve_length(l, frame.height))
            .unwrap_or(0.0);
        let x = parent_x + frame.x + tx;
        let y = parent_y + frame.y + ty;
        let w = frame.width.max(0.0);
        let h = frame.height.max(0.0);
        let inside = px >= x && px < x + w && py >= y && py < y + h;
        if !inside {
            return;
        }
        // Visually-topmost child wins — walk children in REVERSE
        // z-order so a higher-z sibling (painted last, on top) gets
        // the hit first.
        let (child_off_x, child_off_y) = if matches!(data.kind, NodeKind::ScrollView) {
            (data.scroll_x, data.scroll_y)
        } else {
            (0.0, 0.0)
        };
        let mut ordered = self.children_in_z_order(&data.children);
        ordered.reverse();
        for child_id in ordered {
            self.hit_test(child_id, x - child_off_x, y - child_off_y, px, py, out);
            if out.is_some() {
                return;
            }
        }
        if let Some(handler) = &data.on_click {
            *out = Some(handler.clone());
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve a `Length` against an axis basis (the node's own width or
/// height, depending on which axis is being read). `Auto` → 0; the
/// caller is responsible for treating `Auto` specially when a real
/// value is required.
fn resolve_length(length: &Length, basis: f32) -> f32 {
    match length {
        Length::Px(v) => *v,
        Length::Percent(p) => basis * (p / 100.0),
        // `Full` is a corner-radius shape, not a linear length.
        Length::Auto | Length::Full => 0.0,
    }
}

/// Paint a gradient fill into `rect`, sampling per pixel and
/// honoring `radii` for rounded corners. The per-pixel sampler
/// composes gradient color with `opacity` via straight alpha; the
/// dst color is the clear color (cf. the `dst_sampler` limitation
/// described in `raster::fill_rect_blended` — write-only surfaces
/// can't be sampled back).
fn paint_gradient<S: Surface>(
    surface: &mut S,
    rect: Rect,
    radii: [f32; 4],
    gradient: &ResolvedGradient,
    opacity: f32,
    clip: Rect,
    clear: [u8; 4],
) {
    let drawn = match rect.intersect(clip) {
        Some(r) => r,
        None => return,
    };
    let sw = surface.width() as i32;
    let sh = surface.height() as i32;
    let x0 = drawn.x.max(0);
    let y0 = drawn.y.max(0);
    let x1 = (drawn.x + drawn.w as i32).min(sw);
    let y1 = (drawn.y + drawn.h as i32).min(sh);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let rect_x0 = rect.x as f32;
    let rect_y0 = rect.y as f32;
    let rect_w = rect.w as f32;
    let rect_h = rect.h as f32;
    let cx = rect_x0 + rect_w * 0.5;
    let cy = rect_y0 + rect_h * 0.5;

    // Pre-compute the per-kind sampler in a closure so we don't
    // re-branch in the inner loop.
    let sample_t: Box<dyn Fn(f32, f32) -> f32> = match &gradient.kind {
        GradientKind::Linear { angle_deg } => {
            let theta = (*angle_deg).to_radians();
            let dir_x = theta.sin();
            let dir_y = -theta.cos();
            // CSS gradient line length: project the rect's
            // half-extent onto the gradient axis. Total covered
            // length is twice that.
            let half_len = (rect_w * 0.5) * dir_x.abs() + (rect_h * 0.5) * dir_y.abs();
            let full_len = (2.0 * half_len).max(1e-3);
            Box::new(move |px: f32, py: f32| -> f32 {
                let dx = px - cx;
                let dy = py - cy;
                let proj = dx * dir_x + dy * dir_y;
                proj / full_len + 0.5
            })
        }
        GradientKind::Radial { center, radius, extent } => {
            let cx = rect_x0 + rect_w * center.0;
            let cy = rect_y0 + rect_h * center.1;
            let ref_dist = match extent {
                RadialExtent::ClosestSide => {
                    let l = cx - rect_x0;
                    let r = rect_x0 + rect_w - cx;
                    let t = cy - rect_y0;
                    let b = rect_y0 + rect_h - cy;
                    l.min(r).min(t).min(b).max(1e-3)
                }
                RadialExtent::FarthestCorner => {
                    let dx = (cx - rect_x0).abs().max((rect_x0 + rect_w - cx).abs());
                    let dy = (cy - rect_y0).abs().max((rect_y0 + rect_h - cy).abs());
                    (dx * dx + dy * dy).sqrt().max(1e-3)
                }
            };
            let r = (ref_dist * radius).max(1e-3);
            Box::new(move |px: f32, py: f32| -> f32 {
                let dx = px - cx;
                let dy = py - cy;
                (dx * dx + dy * dy).sqrt() / r
            })
        }
    };

    // Effective stops with per-stop animation overrides applied.
    // Build once, share across the inner loop. Vec allocation is
    // small (rarely > 4 stops) and amortized.
    let stops: Vec<(f32, Rgba)> = gradient
        .stops
        .iter()
        .enumerate()
        .map(|(i, (o, c))| (*o, gradient.animated_stops[i].unwrap_or(*c)))
        .collect();

    let radii_clamped = raster::clamp_radii_pub(radii, rect_w, rect_h);
    let any_round = radii_clamped.iter().any(|r| *r > 0.5);

    for py_i in y0..y1 {
        let cy = py_i as f32 + 0.5;
        for px_i in x0..x1 {
            let cx = px_i as f32 + 0.5;
            if any_round
                && !raster::rounded_rect_contains_pub(
                    cx,
                    cy,
                    rect_x0,
                    rect_y0,
                    rect_x0 + rect_w,
                    rect_y0 + rect_h,
                    radii_clamped[0],
                    radii_clamped[1],
                    radii_clamped[2],
                    radii_clamped[3],
                )
            {
                continue;
            }
            let t = sample_t(cx, cy);
            let sampled = sample_stops(&stops, t);
            let color = premultiply_alpha(
                [sampled.r, sampled.g, sampled.b, sampled.a],
                opacity,
            );
            if color[3] == 0 {
                continue;
            }
            let out = if color[3] == 255 {
                [color[0], color[1], color[2], 255]
            } else {
                blend_over(color, clear)
            };
            surface.put_pixel(px_i as u32, py_i as u32, out);
        }
    }
}

/// Interpolate `stops` (offset-sorted) at `t`. Clamps to the
/// first/last stop outside `[stop_min, stop_max]`.
fn sample_stops(stops: &[(f32, Rgba)], t: f32) -> Rgba {
    if stops.is_empty() {
        return Rgba::TRANSPARENT;
    }
    if stops.len() == 1 || t <= stops[0].0 {
        return stops[0].1;
    }
    let last = *stops.last().unwrap();
    if t >= last.0 {
        return last.1;
    }
    for i in 1..stops.len() {
        let (o1, c1) = stops[i];
        if t <= o1 {
            let (o0, c0) = stops[i - 1];
            let span = (o1 - o0).max(1e-6);
            let local = ((t - o0) / span).clamp(0.0, 1.0);
            return lerp_rgba(c0, c1, local);
        }
    }
    last.1
}

fn lerp_rgba(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let mix = |u: u8, v: u8| -> u8 {
        let f = u as f32 + (v as f32 - u as f32) * t;
        f.round().clamp(0.0, 255.0) as u8
    };
    Rgba::new(mix(a.r, b.r), mix(a.g, b.g), mix(a.b, b.b), mix(a.a, b.a))
}

/// Paint a string at `(x, y)` (top-left of first glyph) in `color`,
/// at integer pixel `scale`. Used by Text + Button.
fn draw_text<S: Surface>(
    surface: &mut S,
    text: &str,
    x: i32,
    y: i32,
    scale: u32,
    color: [u8; 4],
    clip: Rect,
) {
    if color[3] == 0 || scale == 0 {
        return;
    }
    let glyph_w = font_8x8::GLYPH_W * scale;
    let glyph_h = font_8x8::GLYPH_H * scale;
    let mut pen_x = x;
    let pen_y = y;
    for ch in text.chars() {
        if ch == '\n' {
            // Line wrapping isn't a font concern — Taffy doesn't
            // hand us multi-line text in a single Text node. Treat
            // bare newlines as line-feeds for hosts that want
            // ad-hoc multi-line output.
            pen_x = x;
            // pen_y shift handled below; bail per-glyph instead so
            // we don't have to track a separate pen state.
            continue;
        }
        let glyph = font_8x8::glyph_for(ch);
        for row in 0..font_8x8::GLYPH_H {
            let bits = glyph[row as usize];
            for col in 0..font_8x8::GLYPH_W {
                // Bit ordering: source data uses LSB-of-byte = leftmost
                // pixel (matches Marcel Sondaar's PD font dump). A
                // straightforward `(bits >> col) & 1` reads the
                // glyph correctly.
                let set = ((bits >> col) & 1) == 1;
                if !set {
                    continue;
                }
                // Scale up by drawing a `scale × scale` block per
                // bit. For `scale = 1` this is one `put_pixel` per
                // set bit; for `scale = 2` it's four; etc.
                let px = pen_x + (col as i32) * (scale as i32);
                let py = pen_y + (row as i32) * (scale as i32);
                let target = Rect::new(px, py, scale, scale);
                if let Some(clipped) = target.intersect(clip) {
                    surface.fill_rect(
                        clipped.x,
                        clipped.y,
                        clipped.w,
                        clipped.h,
                        color,
                    );
                }
            }
        }
        pen_x += glyph_w as i32;
        // For ASCII characters this stays on one line. Multi-line
        // text is the layout layer's job (each line gets its own
        // Text node), so we don't advance pen_y here.
        let _ = glyph_h;
    }
}

// ---------------------------------------------------------------------------
// Backend impl
// ---------------------------------------------------------------------------


impl CpuBackend {
    /// Recursively free a node and all its descendants. Removes the
    /// Taffy node + the per-id `NodeData` entry. Internal helper for
    /// `clear_children`.
    fn remove_subtree(&mut self, id: u32) {
        let Some(child_ids) = self.nodes.get(&id).map(|d| d.children.clone()) else { return };
        for cid in &child_ids {
            self.remove_subtree(*cid);
        }
        if let Some(data) = self.nodes.remove(&id) {
            self.layout.remove_node(data.layout);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
