//! ScrollView primitive.
//!
//! Backed by a `<div style="overflow: scroll">` on web, `UIScrollView`
//! on iOS, `ScrollView` / `HorizontalScrollView` on Android. Default
//! orientation is vertical; pass `.horizontal()` for left-right
//! scrolling. Two-axis scrolling is not supported in v1 — pick one
//! direction.

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


