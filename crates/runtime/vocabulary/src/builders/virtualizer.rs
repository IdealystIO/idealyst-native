//! Virtualizer builder: `virtualizer()` (`flat_list`'s type-erased core).

use std::rc::Rc;

use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::primitives::virtualizer::{
    Axis, ItemKey, ItemSize, Lanes, VirtualLayout, VirtualizerHandle,
};
use runtime_scene::{item, Element};

use crate::prims::{PrimCell, VirtualizerPrim};
use crate::style_attach::IntoStyleProp;

/// Start a `virtualizer` — port of the old
/// `primitives::virtualizer::virtualizer(...)` constructor (the four
/// required callbacks are positional, exactly like the old entry point;
/// the typed `flat_list<T>` wrapper re-lands on this builder when the
/// SDK layer retargets at P6). Defaults mirror the old `Bound`:
/// `overscan = 1.0`, `VirtualLayout::default()` (vertical single-lane
/// list, no gaps).
pub fn virtualizer(
    item_count: impl Fn() -> usize + 'static,
    item_key: impl Fn(usize) -> ItemKey + 'static,
    item_size: ItemSize,
    render_item: impl Fn(usize) -> Element + 'static,
) -> VirtualizerBuilder {
    VirtualizerBuilder {
        prim: VirtualizerPrim {
            item_count: Box::new(item_count),
            item_key: Box::new(item_key),
            item_size,
            render_item: Rc::new(render_item),
            overscan: 1.0,
            layout: VirtualLayout::default(),
            style: None,
            a11y: AccessibilityProps::default(),
            ref_fill: None,
            on_scroll: None,
        },
    }
}

pub struct VirtualizerBuilder {
    prim: VirtualizerPrim,
}

impl VirtualizerBuilder {
    /// Buffer factor outside the visible window. Default `1.0` (one
    /// viewport extent above and below).
    pub fn overscan(mut self, factor: f32) -> Self {
        self.prim.overscan = factor;
        self
    }

    /// Scroll axis. Default `Axis::Vertical`.
    pub fn axis(mut self, axis: Axis) -> Self {
        self.prim.layout.axis = axis;
        self
    }

    /// Cross-axis lane subdivision. `Lanes::Fixed(1)` (default) is a
    /// list; `Fixed(N)` an N-lane grid; `AutoFit` responsive.
    pub fn lanes(mut self, lanes: Lanes) -> Self {
        self.prim.layout.lanes = lanes;
        self
    }

    /// Gaps: `main` between grid-rows along the scroll axis, `cross`
    /// between lanes.
    pub fn spacing(mut self, main: f32, cross: f32) -> Self {
        self.prim.layout.main_spacing = main;
        self.prim.layout.cross_spacing = cross;
        self
    }

    /// Convenience: equal gap on both axes.
    pub fn gap(self, gap: f32) -> Self {
        self.spacing(gap, gap)
    }

    pub fn style(mut self, style: impl IntoStyleProp) -> Self {
        self.prim.style = Some(style.into_style_prop());
        self
    }

    pub fn a11y(mut self, a11y: AccessibilityProps) -> Self {
        self.prim.a11y = a11y;
        self
    }

    pub fn on_handle(mut self, fill: impl FnOnce(VirtualizerHandle) + 'static) -> Self {
        self.prim.ref_fill = Some(Box::new(fill));
        self
    }

    /// Observe the virtualizer's own scroll offset. Fires per scroll
    /// event with `(x, y)` in CSS px / native points — identical
    /// contract to `scroll_view`'s `.on_scroll(..)`, including which
    /// component is meaningful: a `Axis::Vertical` virtualizer reports
    /// `y` and a constant `0.0` for `x`, and vice versa.
    ///
    /// A virtualizer owns its scroller, so this is the only way a
    /// sibling can align to it — a sticky header that tracks the list,
    /// an edge-triggered "load more", a second pane synced to the same
    /// offset. Without it those all force the app to hand-roll
    /// virtualization over a `scroll_view` purely to get the offset
    /// back.
    ///
    /// Runs like any author callback (outside `World::enter`); writes
    /// staged here flush with the surrounding dispatch. Keep it cheap
    /// — it fires at scroll frequency.
    pub fn on_scroll(mut self, handler: impl Fn(f32, f32) + 'static) -> Self {
        self.prim.on_scroll = Some(Rc::new(handler));
        self
    }

    pub fn build(self) -> Element {
        item(PrimCell::new(self.prim), Vec::new())
    }
}
