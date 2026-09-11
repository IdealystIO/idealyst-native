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
            on_end_reached: None,
            end_reached_threshold: 0.0,
            safe_area: None,
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
    /// a second pane synced to the same offset. (For "load more", reach
    /// for [`on_end_reached`](Self::on_end_reached) instead: an offset
    /// alone cannot say how much is left.) Without it those force the
    /// app to hand-roll
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

    /// Fetch-ahead hook: fires once when the reader comes within
    /// `threshold` px of the last item, and re-arms when they leave.
    ///
    /// Pair it with [`end_reached_threshold`](Self::end_reached_threshold).
    /// A backend that does not implement `observe_scroll_end` never
    /// fires it, so a list that grows ONLY this way stops growing —
    /// keep a manual control as the fallback.
    pub fn on_end_reached(mut self, handler: impl Fn() + 'static) -> Self {
        self.prim.on_end_reached = Some(Rc::new(handler));
        self
    }

    /// How close to the end counts as arriving, in logical px. Default
    /// `0.0` — the very end. A screenful is the usual choice, so the
    /// next page is there before the reader is.
    /// Inset the list's CONTENT by the safe area on `sides` while the
    /// scroller keeps drawing through it — see
    /// `VirtualizerPrim::safe_area`. This is how a list that reaches
    /// the bottom of the screen lets its rows scroll under the home
    /// indicator without the last one hiding behind it.
    pub fn safe_area(mut self, sides: runtime_shared::SafeAreaSides) -> Self {
        self.prim.safe_area = Some(sides);
        self
    }

    pub fn end_reached_threshold(mut self, px: f32) -> Self {
        self.prim.end_reached_threshold = px;
        self
    }

    pub fn build(self) -> Element {
        item(PrimCell::new(self.prim), Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_shared::primitives::virtualizer::ItemKey;
    use runtime_shared::SafeAreaSides;

    fn probe() -> VirtualizerBuilder {
        virtualizer(
            || 0usize,
            |i| i as ItemKey,
            ItemSize::Known(Rc::new(|_| 0.0)),
            |_| runtime_scene::Element::Fragment(Vec::new()),
        )
    }

    /// `safe_area` is the same three-state contract its `scroll_view`
    /// counterpart has: silence leaves the backend's default alone, and
    /// an EMPTY set is a stated opt-out rather than another way of
    /// saying nothing.
    ///
    /// A virtualized list had no way to say either, which is why one
    /// reaching the bottom of a phone screen could only stop short of
    /// the home indicator or hide its last row behind it — never do
    /// what a native list does and scroll under it with the content
    /// inset.
    #[test]
    fn safe_area_distinguishes_silence_from_a_stated_preference() {
        assert_eq!(probe().prim.safe_area, None);
        assert_eq!(
            probe().safe_area(SafeAreaSides::BOTTOM).prim.safe_area,
            Some(SafeAreaSides::BOTTOM)
        );
        assert_eq!(
            probe().safe_area(SafeAreaSides::NONE).prim.safe_area,
            Some(SafeAreaSides::NONE),
            "an empty set must survive as Some — it is the opt-out, and \
             collapsing it to None would hand the decision back to the backend"
        );
    }
}
