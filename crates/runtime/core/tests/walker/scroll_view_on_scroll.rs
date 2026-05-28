//! `Element::ScrollView::on_scroll` plumbing — assert that the
//! callback attached via `Bound::<ScrollViewHandle>::on_scroll` survives
//! mount, gets registered through the backend's `create_scroll_view`
//! method, and fires with the offset args the backend dispatches.
//!
//! Author surface is uniform across backends \u{2014} per the framework
//! rule that backends diverge in mechanism but converge in observable
//! behaviour (CLAUDE.md #7). The regression that this test guards
//! against is the previous shape where `Element::ScrollView` had no
//! `on_scroll` field at all and author code had to reach into
//! `web_sys` / `UIScrollViewDelegate` / `setOnScrollChangeListener`
//! directly to observe scroll position. That \u{2014} a
//! `#[cfg(target_arch = "wasm32")]` block in author code \u{2014} is
//! the smell. This test asserts the framework primitive exists and
//! fires uniformly through the Backend trait.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::primitives::scroll_view::scroll_view;

use crate::common::{Event, NodeId, TestRuntime};

/// `scroll_view().on_scroll(...)` records `has_on_scroll: true` and
/// the registered closure fires with the args the backend dispatches.
#[test]
fn regression_scroll_view_on_scroll_registers_and_fires() {
    let rt = TestRuntime::new();
    let fired: Rc<RefCell<Vec<(f32, f32)>>> = Rc::new(RefCell::new(Vec::new()));
    let fired_clone = fired.clone();

    let _owner = rt.render(
        scroll_view(Vec::new())
            .on_scroll(move |x, y| {
                fired_clone.borrow_mut().push((x, y));
            })
            .into(),
    );

    // Mount-side: the create event records the callback presence.
    // Catches the regression where `on_scroll` is silently dropped
    // (passed to the builder but never reaching `Backend::
    // create_scroll_view`).
    rt.backend().assert_any(|e| {
        matches!(
            e,
            Event::CreateScrollView {
                has_on_scroll: true,
                ..
            }
        )
    });

    // Synthesize a scroll event via the mock helper. The handler
    // should observe the exact `(x, y)` we passed in. Two events to
    // catch a regression where only the first call is forwarded.
    let node = NodeId(0);
    assert!(rt.backend().fire_scroll_event(node, 12.0, 48.0));
    assert!(rt.backend().fire_scroll_event(node, 12.0, 96.0));
    let received = fired.borrow();
    assert_eq!(received.len(), 2);
    assert_eq!(received[0], (12.0, 48.0));
    assert_eq!(received[1], (12.0, 96.0));
}

/// A scroll view *without* `on_scroll` records `has_on_scroll: false`
/// and has no registered callback to fire \u{2014} `fire_scroll_event`
/// returns `false`. Guards the inverse regression: that the builder
/// doesn't accidentally always pass `Some` because of a default-value
/// or `unwrap_or_default` mistake.
#[test]
fn regression_scroll_view_without_on_scroll_records_absence() {
    let rt = TestRuntime::new();
    let _owner = rt.render(scroll_view(Vec::new()).into());

    rt.backend().assert_any(|e| {
        matches!(
            e,
            Event::CreateScrollView {
                has_on_scroll: false,
                ..
            }
        )
    });
    assert!(!rt.backend().fire_scroll_event(NodeId(0), 0.0, 100.0));
}
