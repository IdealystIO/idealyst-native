//! `Element::ScrollView` — a `<div>` with `overflow: auto` on the
//! requested axis.

use crate::WebBackend;
use runtime_shared::primitives::scroll_view::{EndReach, ScrollViewHandle, ScrollViewOps};
use std::any::Any;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::Node;

pub(crate) fn create(
    b: &mut WebBackend,
    horizontal: bool,
    on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
) -> Node {
    // HYDRATION: adopt the SSR scroll `<div>` (tag match) so its children
    // (the scrolled content) adopt in place. Without this the scroll_view
    // built a FRESH div and never advanced the cursor, so the SSR scroll node
    // was left for the next primitive (the content panel) to mis-adopt —
    // diverging the whole scrolled subtree and cascading to later siblings.
    // Mirrors `view::create`. Off hydration both arms create a fresh div.
    let div: web_sys::Element = match b.hydrate_next("div") {
        Some(el) => el,
        None => {
            let el = b
                .doc
                .create_element("div")
                .expect("create_element div failed");
            b.hydrate_note_fresh(&el.clone().unchecked_into::<Node>());
            el
        }
    };
    // No `.ui-default` class — see view.rs for the rationale.
    // ScrollView's only fixed layout is the overflow we set inline
    // below; children stack via normal block flow unless the user's
    // style on the ScrollView itself opts into flex.
    // Apply the overflow style inline (not via the framework's style
    // system) so it's always present regardless of user-supplied
    // styling. The inline rules win over class rules for the overflow
    // properties; the class still governs flex direction etc.
    let overflow = if horizontal {
        "overflow-x: auto; overflow-y: hidden"
    } else {
        "overflow-y: auto; overflow-x: hidden"
    };
    let _ = div.set_attribute("style", overflow);
    // NOTE for `apply_scroll_view_bounces`: this writes the whole style
    // ATTRIBUTE, so anything that lands here later must go through
    // `style().set_property(...)` rather than a second `set_attribute`,
    // which would drop the overflow above.

    // Wire `on_scroll`. The callback receives CSS-pixel offsets
    // (`scrollLeft`/`scrollTop`) directly \u{2014} same units the
    // framework already uses for layout, so author code doesn't
    // need to translate.
    //
    // `Closure<dyn Fn>` rather than `FnMut` so wasm-bindgen doesn't
    // emit the `FnMut` runtime recursion guard. The author's callback
    // may write a signal whose subscribers can mutate layout in ways
    // that synchronously re-fire `scroll`; the guard would reject
    // that as recursive even though the second call is benign.
    //
    // `.forget()` leaks the Closure so JS can keep invoking it for
    // the lifetime of the element. We trade per-ScrollView leakage
    // (one Closure object) for never holding a dangling function
    // ref on the DOM listener side \u{2014} which would crash the
    // page with a "closure invoked after being dropped" throw.
    if let Some(cb) = on_scroll {
        let element_for_handler = div.clone();
        let scroll_handler: Closure<dyn Fn(web_sys::Event)> =
            Closure::wrap(Box::new(move |_evt: web_sys::Event| {
                if let Some(html) = element_for_handler.dyn_ref::<web_sys::HtmlElement>() {
                    let x = html.scroll_left() as f32;
                    let y = html.scroll_top() as f32;
                    cb(x, y);
                }
            }));
        let _ = div.add_event_listener_with_callback(
            "scroll",
            scroll_handler.as_ref().unchecked_ref(),
        );
        scroll_handler.forget();
    }

    div.unchecked_into::<Node>()
}

/// Watch for the reader arriving at the end of the scroll axis.
///
/// Its own listener rather than a branch inside `on_scroll`'s: the DOM
/// takes as many `scroll` listeners as you give it, so the two features
/// stay independent and either can be present without the other. (The
/// iOS backend cannot do this — a UIScrollView has one delegate — which
/// is why that side has to share.)
///
/// `scrollHeight`/`clientHeight` are the two numbers the app cannot
/// reach for itself: an author has the offset from `on_scroll` and no
/// way to ask how much content is under it.
pub(crate) fn observe_end(
    node: &Node,
    horizontal: bool,
    threshold: f32,
    on_end: Rc<dyn Fn()>,
) {
    let el: web_sys::Element = node.clone().unchecked_into();
    let el_for_handler = el.clone();
    let reach = std::cell::RefCell::new(EndReach::new(threshold));
    // Same `Fn` + `.forget()` trade as the `on_scroll` closure above,
    // and for the same two reasons — see the note there.
    let handler: Closure<dyn Fn(web_sys::Event)> =
        Closure::wrap(Box::new(move |_evt: web_sys::Event| {
            if let Some(html) = el_for_handler.dyn_ref::<web_sys::HtmlElement>() {
                let (offset, viewport, content) = if horizontal {
                    (
                        html.scroll_left() as f32,
                        html.client_width() as f32,
                        html.scroll_width() as f32,
                    )
                } else {
                    (
                        html.scroll_top() as f32,
                        html.client_height() as f32,
                        html.scroll_height() as f32,
                    )
                };
                if reach.borrow_mut().update(offset, viewport, content) {
                    on_end();
                }
            }
        }));
    let _ = el.add_event_listener_with_callback("scroll", handler.as_ref().unchecked_ref());
    handler.forget();
}

pub(crate) fn make_handle(node: &Node) -> ScrollViewHandle {
    let el: web_sys::HtmlElement = node
        .clone()
        .dyn_into()
        .expect("scroll_view node is not an HtmlElement");
    ScrollViewHandle::new(Rc::new(el), &WebScrollViewOps)
}

struct WebScrollViewOps;
impl ScrollViewOps for WebScrollViewOps {
    fn scroll_to(&self, node: &dyn Any, x: f32, y: f32) {
        if let Some(html) = node.downcast_ref::<web_sys::HtmlElement>() {
            html.set_scroll_left(x as i32);
            html.set_scroll_top(y as i32);
        }
    }
}

/// `overscroll-behavior` — the web's spelling of "may this scroller
/// travel past its content".
///
/// Not a perfect analogue of iOS `bounces` and it does not pretend to
/// be. `none` does two things: it stops the rubber-band on the mobile
/// engines that have one (iOS Safari), and it stops SCROLL CHAINING —
/// a gesture that reaches this scroller's end no longer continues into
/// the page behind it. For a bounded pane inside a page, which is the
/// case this exists for, both are wanted.
///
/// Written with `set_property` rather than `set_attribute`: the mount
/// above writes the style ATTRIBUTE wholesale for the overflow, and a
/// second attribute write would drop it.
pub(crate) fn apply_bounces(el: &web_sys::Element, bounces: bool) {
    let Some(html) = el.dyn_ref::<web_sys::HtmlElement>() else {
        return;
    };
    let value = if bounces { "auto" } else { "none" };
    let _ = html.style().set_property("overscroll-behavior", value);
}
