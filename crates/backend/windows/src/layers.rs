//! Painted content above graphics surfaces.
//!
//! Views are painted into one scene (the window's own surface under GDI+,
//! the bottom composition visual under Direct2D), and every `graphics`
//! node — a canvas, a 3D view — is a DirectComposition visual ABOVE that
//! scene. So anything painted over a graphics node in the author's tree
//! (a chart's tooltip, a modal over a canvas, canvas3d's HUD text) ended up
//! underneath it. On every other backend a graphics node is an ordinary
//! view and stacks in tree order.
//!
//! [`plan`] restores tree order. It walks the tree in paint order and gives
//! each node that draws something a layer: 0 is the scene; layer `k` is a
//! composition visual just above the `k`-th graphics node in paint order. A
//! node goes in the lowest layer that keeps it above everything earlier in
//! paint order that it overlaps — a graphics node `k` it overlaps needs
//! layer ≥ `k`, an earlier lifted node in layer `l` it overlaps needs ≥ `l`.
//! So only content that actually covers a surface leaves the scene: a
//! tooltip over a chart is lifted, the card background behind a sparkline
//! further down is not. The painters then draw layer 0 as before and each
//! lifted layer into its own visual (`crate::overlay`), sized to its
//! content.
//!
//! Approximation: overlap is tested on axis-aligned boxes with translations
//! (as the hit test is), so a rotated or scaled node is placed by its
//! untransformed box.

use std::collections::HashMap;

use crate::{scene, NodeKind, NodeMeta, WindowsBackend};

/// A device-space rectangle `(x0, y0, x1, y1)`.
pub(crate) type Rect = (f32, f32, f32, f32);

/// Which layer each lifted node paints in, and what each lifted layer
/// covers. Nodes not in `layer_of` paint in the scene (layer 0).
#[derive(Debug, Default, PartialEq)]
pub(crate) struct LayerPlan {
    pub layer_of: HashMap<u64, usize>,
    /// Layer `k` is `layers[k - 1]`.
    pub layers: Vec<Overlay>,
}

/// One lifted layer: the graphics node it sits directly above, and the
/// device-space bounds of its content (empty bounds = nothing lifted).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Overlay {
    pub above: u64,
    pub bounds: Option<Rect>,
}

impl LayerPlan {
    /// The layer `id` paints in.
    pub(crate) fn layer(&self, id: u64) -> usize {
        self.layer_of.get(&id).copied().unwrap_or(0)
    }

    /// The layers that have content: `(k, overlay)`.
    pub(crate) fn lifted(&self) -> impl Iterator<Item = (usize, &Overlay)> {
        self.layers.iter().enumerate().filter(|(_, o)| o.bounds.is_some()).map(|(i, o)| (i + 1, o))
    }
}

/// Plan the layers for the current frames. `is_graphics` names the
/// graphics nodes (visible ones only).
pub(crate) fn plan(b: &WindowsBackend, is_graphics: impl Fn(u64) -> bool) -> LayerPlan {
    let mut walk = Walk { b, is_graphics: &is_graphics, scale: b.dpi.factor(), plan: LayerPlan::default(), placed: Vec::new() };
    if let Some(root) = b.root_id {
        let screen = (f32::MIN, f32::MIN, f32::MAX, f32::MAX);
        walk.visit(root, 0.0, 0.0, screen);
    }
    walk.plan
}

struct Walk<'a, G: Fn(u64) -> bool> {
    b: &'a WindowsBackend,
    is_graphics: &'a G,
    scale: f32,
    plan: LayerPlan,
    /// Everything that can force a later node up: graphics nodes (with the
    /// layer just above them) and lifted nodes (with their layer).
    placed: Vec<(Rect, usize)>,
}

impl<G: Fn(u64) -> bool> Walk<'_, G> {
    fn visit(&mut self, id: u64, ox: f32, oy: f32, clip: Rect) {
        let b = self.b;
        let Some((nx, ny, w, h)) = scene::node_origin(b, id, ox, oy) else { return };
        let Some(meta) = b.nodes.get(&id) else { return };
        if meta.hidden || meta.effective_opacity() <= 0.003 {
            return;
        }
        let s = self.scale;
        let rect = intersect((nx * s, ny * s, (nx + w) * s, (ny + h) * s), clip);
        let visible = rect.0 < rect.2 && rect.1 < rect.3;
        if visible {
            if (self.is_graphics)(id) {
                self.plan.layers.push(Overlay { above: id, bounds: None });
                self.placed.push((rect, self.plan.layers.len()));
            } else if draws(meta) {
                let layer = self.placed.iter().filter(|(r, _)| overlaps(*r, rect)).map(|(_, l)| *l).max().unwrap_or(0);
                if layer > 0 {
                    self.plan.layer_of.insert(id, layer);
                    self.placed.push((rect, layer));
                    let o = &mut self.plan.layers[layer - 1];
                    o.bounds = Some(match o.bounds {
                        None => rect,
                        Some(r) => (r.0.min(rect.0), r.1.min(rect.1), r.2.max(rect.2), r.3.max(rect.3)),
                    });
                }
            }
        }
        if b.children.get(&id).is_none_or(|c| c.is_empty()) {
            return;
        }
        let child_clip = if meta.clips_children() { rect } else { clip };
        let (cx, cy) = scene::content_origin(meta, nx, ny);
        for c in scene::child_paint_order(b, id) {
            self.visit(c, cx, cy, child_clip);
        }
    }
}

/// Whether a node paints anything of its own.
pub(crate) fn draws(meta: &NodeMeta) -> bool {
    match &meta.kind {
        NodeKind::View(v) => {
            v.effective_background().is_some_and(|c| c[3] > 0.0)
                || v.gradient.as_ref().is_some_and(|g| !g.stops.is_empty())
                || v.borders.iter().any(|b| b.width > 0.0)
        }
        NodeKind::Text(t) => !t.content.is_empty(),
        NodeKind::Code(_) | NodeKind::Icon(_) | NodeKind::Image(_) => true,
        NodeKind::Control { .. } | NodeKind::External { .. } => false,
    }
}

fn intersect(a: Rect, b: Rect) -> Rect {
    (a.0.max(b.0), a.1.max(b.1), a.2.min(b.2), a.3.min(b.3))
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3
}

/// `bounds` grown to whole device pixels: `(x, y, w, h)`.
pub(crate) fn pixel_box(bounds: Rect) -> (i32, i32, u32, u32) {
    let x0 = bounds.0.floor() as i32;
    let y0 = bounds.1.floor() as i32;
    let x1 = bounds.2.ceil() as i32;
    let y1 = bounds.3.ceil() as i32;
    (x0, y0, (x1 - x0).max(1) as u32, (y1 - y0).max(1) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ViewVisual;
    use windows::Win32::Foundation::HWND;

    fn put(b: &mut WindowsBackend, id: u64, parent: Option<u64>, frame: (f32, f32, f32, f32), kind: NodeKind) {
        let mut m = NodeMeta::new(kind);
        m.frame = frame;
        b.nodes.insert(id, m);
        match parent {
            Some(p) => b.children.entry(p).or_default().push(id),
            None => b.root_id = Some(id),
        }
    }

    fn filled() -> NodeKind {
        NodeKind::View(ViewVisual { background: Some([1.0, 0.0, 0.0, 1.0]), ..Default::default() })
    }

    fn graphics() -> NodeKind {
        NodeKind::External { hwnd: HWND(std::ptr::null_mut()) }
    }

    /// Root 800×600. A chart: canvas (2) at (0,0) 400×300, then its tooltip
    /// (3) over it. A card below (4) with a background, holding a sparkline
    /// canvas (5) — the card comes after the chart canvas in paint order
    /// but does not overlap it.
    fn fixture() -> WindowsBackend {
        let mut b = WindowsBackend::new(HWND(std::ptr::null_mut()));
        put(&mut b, 1, None, (0.0, 0.0, 800.0, 600.0), NodeKind::View(ViewVisual::default()));
        put(&mut b, 2, Some(1), (0.0, 0.0, 400.0, 300.0), graphics());
        put(&mut b, 3, Some(1), (50.0, 50.0, 100.0, 40.0), filled());
        put(&mut b, 4, Some(1), (0.0, 400.0, 400.0, 100.0), filled());
        put(&mut b, 5, Some(4), (0.0, 0.0, 400.0, 100.0), graphics());
        b
    }

    /// A chart's tooltip — painted after the chart's canvas and over it —
    /// was drawn under the canvas: painted content was all below every
    /// graphics surface. It goes in a layer just above that canvas.
    #[test]
    fn regression_content_over_a_graphics_node_is_lifted_above_it() {
        let b = fixture();
        let plan = plan(&b, |id| id == 2 || id == 5);
        assert_eq!(plan.layer(3), 1, "the tooltip sits above the chart canvas");
        assert_eq!(plan.layers[0], Overlay { above: 2, bounds: Some((50.0, 50.0, 150.0, 90.0)) });
        assert_eq!(plan.layer(4), 0, "the card stays in the scene, under its own sparkline");
        assert_eq!(plan.layers[1], Overlay { above: 5, bounds: None });
        assert_eq!(plan.lifted().count(), 1);
    }

    /// A node over a lifted node is lifted with it, or it would draw under
    /// it; a node over neither stays in the scene.
    #[test]
    fn content_over_lifted_content_follows_it_up() {
        let mut b = fixture();
        // Over the tooltip, reaching past the canvas's right edge.
        put(&mut b, 6, Some(1), (100.0, 80.0, 400.0, 30.0), filled());
        // Over nothing.
        put(&mut b, 7, Some(1), (600.0, 0.0, 50.0, 50.0), filled());
        let plan = plan(&b, |id| id == 2 || id == 5);
        assert_eq!(plan.layer(6), 1);
        assert_eq!(plan.layer(7), 0);
    }

    #[test]
    fn scale_puts_bounds_in_device_pixels() {
        let mut b = fixture();
        b.dpi = crate::dpi::DpiScale::from_dpi(192);
        let plan = plan(&b, |id| id == 2 || id == 5);
        assert_eq!(plan.layers[0].bounds, Some((100.0, 100.0, 300.0, 180.0)));
        assert_eq!(pixel_box((10.2, 5.0, 20.5, 6.1)), (10, 5, 11, 2));
    }
}
