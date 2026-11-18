//! [`PageFrame`] — what content under a pinned header needs to know
//! about the page around it.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::{
    after_animation_frame, on_scope_drop, signal, LayoutSubscription, Ref, ScrollViewHandle, Signal, ViewHandle,
};

/// The page around a body: how tall its pinned header is, how tall the
/// window under it is, and a way to move the page's scroller.
///
/// A screen makes one and hands it to its [`Page`](super::Page)
/// (`frame`), which fills it in, and to whatever inside pins against
/// the header: a [`SplitView`](crate::SplitView) or a
/// [`PinnedPane`](super::PinnedPane). Passed down rather than looked
/// up, because a page's body is built before the page around it.
/// `Copy`: signals and a ref.
#[derive(Clone, Copy)]
pub struct PageFrame {
    /// The pinned header's height.
    pub header_height: Signal<f32>,
    /// The page's height, header included. A pinned pane is at most this
    /// less the header, so its foot stays on screen.
    pub port_height: Signal<f32>,
    pub(crate) scroller: Ref<ScrollViewHandle>,
}

impl PageFrame {
    /// A frame no page has filled in yet.
    pub fn new() -> Self {
        Self { header_height: signal(0.0), port_height: signal(0.0), scroller: Ref::new() }
    }

    /// Put the page at its top: moving to another page of a tab is
    /// arriving somewhere, and the one scroller would otherwise keep the
    /// last offset.
    pub fn scroll_to_top(self) {
        self.scroll_to(0.0);
    }

    /// Scroll the page to `y` from its top: a jump to one section of a
    /// long page.
    pub fn scroll_to(self, y: f32) {
        self.scroller.with(|h| h.scroll_to(0.0, y));
    }
}

impl Default for PageFrame {
    fn default() -> Self {
        Self::new()
    }
}

/// A height change smaller than this is not written, so sub-pixel
/// layout noise does not re-run what reads the frame.
const HEIGHT_EPSILON: f32 = 0.5;

/// Keep `into` at `view`'s height for as long as the calling scope
/// lives.
pub(crate) fn watch_height(view: Ref<ViewHandle>, into: Signal<f32>) {
    let held: Rc<RefCell<Option<LayoutSubscription>>> = Rc::new(RefCell::new(None));
    let holder = held.clone();
    // A frame later: the ref is filled at mount, after this runs.
    let task = after_animation_frame(move || {
        if let Some(height) = view.with(|h| h.frame().map(|r| r.height)).flatten() {
            into.set(height);
        }
        let sub = view.with(|h| {
            h.on_layout(move |_width, height| {
                if (into.peek() - height).abs() > HEIGHT_EPSILON {
                    into.set(height);
                }
            })
        });
        if let Some(sub) = sub {
            *holder.borrow_mut() = Some(sub);
        }
    });
    on_scope_drop(move || {
        drop(task);
        drop(held);
    });
}
