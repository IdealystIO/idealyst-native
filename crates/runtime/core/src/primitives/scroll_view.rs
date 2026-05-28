//! ScrollView primitive.
//!
//! Backed by a `<div style="overflow: scroll">` on web, `UIScrollView`
//! on iOS, `ScrollView` / `HorizontalScrollView` on Android. Default
//! orientation is vertical; pass `.horizontal()` for left-right
//! scrolling. Two-axis scrolling is not supported in v1 — pick one
//! direction.

use crate::{Bound, Element, Ref, RefFill};
use std::any::Any;
use std::rc::Rc;

#[derive(Clone)]
pub struct ScrollViewHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn ScrollViewOps,
}

impl ScrollViewHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn ScrollViewOps) -> Self {
        Self { node, ops }
    }

    /// Scroll to absolute coordinates within the view's content box.
    /// `x` is meaningful for horizontal scrollers; vertical scrollers
    /// ignore it.
    pub fn scroll_to(&self, x: f32, y: f32) {
        self.ops.scroll_to(&*self.node, x, y);
    }

    /// Convenience: scroll to (0, 0). Common enough to warrant its
    /// own method.
    pub fn scroll_to_top(&self) {
        self.ops.scroll_to(&*self.node, 0.0, 0.0);
    }
}

pub trait ScrollViewOps {
    fn scroll_to(&self, node: &dyn Any, x: f32, y: f32);
}

pub fn scroll_view(children: Vec<Element>) -> Bound<ScrollViewHandle> {
    Bound::new(Element::ScrollView {
        children,
        horizontal: false,
        style: None,
        ref_fill: None,
        safe_area_sides: crate::SafeAreaSides::NONE,
        on_scroll: None,
        accessibility: crate::accessibility::AccessibilityProps::default(),
    })
}

impl Bound<ScrollViewHandle> {
    /// Configure the scroll axis. `horizontal(true)` makes the
    /// container scroll left-right instead of up-down.
    pub fn horizontal(mut self, h: bool) -> Self {
        if let Element::ScrollView { horizontal, .. } = &mut self.primitive {
            *horizontal = h;
        }
        self
    }

    pub fn bind(mut self, r: Ref<ScrollViewHandle>) -> Self {
        if let Element::ScrollView { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::ScrollView(Box::new(move |h| r.fill(h))));
        }
        self
    }

    /// Opt this scroll view into safe-area-aware padding. See
    /// [`Bound::<crate::ViewHandle>::safe_area`] for full semantics.
    /// Common use: a vertical scroll view at the screen root with
    /// `SafeAreaSides::VERTICAL` so its content respects status bar
    /// + home indicator while the background bleeds under both.
    pub fn safe_area(mut self, sides: crate::SafeAreaSides) -> Self {
        if let Element::ScrollView { safe_area_sides, .. } = &mut self.primitive {
            *safe_area_sides |= sides;
        }
        self
    }

    /// Register a callback that fires on every scroll offset change.
    /// Receives `(scroll_left_px, scroll_top_px)` in CSS pixels /
    /// native points. Uniform across backends \u{2014} the backend
    /// binds this to its native scroll observer (web `scroll` event,
    /// iOS `UIScrollViewDelegate::scrollViewDidScroll`, Android
    /// `OnScrollChangeListener`, etc.).
    ///
    /// Use this to drive a scroll-position signal in author code
    /// (`on_scroll = move |x, y| offset.set((x, y))`) and compose
    /// against `ViewHandle::absolute_frame()` for cross-platform
    /// scroll-spy / sticky-header / parallax patterns.
    pub fn on_scroll<F: Fn(f32, f32) + 'static>(mut self, cb: F) -> Self {
        if let Element::ScrollView { on_scroll, .. } = &mut self.primitive {
            *on_scroll = Some(Rc::new(cb));
        }
        self
    }
}
