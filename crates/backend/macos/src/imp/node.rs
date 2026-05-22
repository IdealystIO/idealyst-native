//! [`MacosNode`] — the backend's `Backend::Node` type. Wraps the
//! concrete AppKit class for each primitive the backend creates,
//! so call sites can dispatch on shape (Label vs ScrollView vs
//! generic View) without re-casting.
//!
//! Mirrors `IosNode` from `backend-ios-mobile`.

use objc2::rc::Retained;
use objc2_app_kit::{NSTextField, NSView};

#[derive(Clone)]
pub enum MacosNode {
    /// Generic container. Either a `FlippedView` (top-left origin)
    /// or any other NSView subclass we own.
    View(Retained<NSView>),
    /// NSTextField in label mode. Single-cell static text with
    /// wrap + measure_fn installed by [`Backend::create_text`].
    Label(Retained<NSTextField>),
}

impl MacosNode {
    pub(crate) fn as_view(&self) -> &NSView {
        match self {
            MacosNode::View(v) => v,
            MacosNode::Label(l) => l,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn view_key(&self) -> usize {
        self.as_view() as *const NSView as usize
    }
}
