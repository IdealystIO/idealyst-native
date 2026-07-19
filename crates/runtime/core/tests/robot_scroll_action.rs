//! Regression: `Robot::set_scroll` (the `set_scroll` bridge verb) must reach
//! the backend's scroll mechanism with the scroll view's own backend node —
//! via the backend's `ScrollViewHandle`, NOT `Backend::set_node_scroll`
//! under a held backend borrow (the native scroll write fires scroll
//! notifications synchronously; their reactive restyles re-borrow the
//! backend, and a held `borrow_mut` aborts with "RefCell already
//! borrowed" — reproduced live on the macOS website drive).
//!
//! Added with the codeblock padding rework: the iOS simulator has no
//! scriptable touch injection, so e2e drives (and the verification run for
//! the padding change itself) rely on this action to bring off-screen
//! content into view. Before the walker wired it, a `ScrollView` robot
//! entry had no scroll action at all and the bridge answered
//! "action 'set_scroll' not available".

#![cfg(feature = "robot")]

#[path = "common/mock_backend.rs"]
mod mock_backend;
#[path = "common/runtime.rs"]
mod runtime;

use runtime::TestRuntime;
use runtime_core::primitives::scroll_view::scroll_view;
use runtime_core::robot::{ElementKind, Query, Robot};
use runtime_core::{view, IntoElement};

#[test]
fn robot_set_scroll_reaches_backend_scroll_handle() {
    let rt = TestRuntime::new();
    let _owner = rt.render(
        scroll_view(vec![view(Vec::new()).into_element()]).into_element(),
    );

    let robot = Robot::new();
    let el = robot
        .find(Query::Kind(ElementKind::ScrollView))
        .expect("the mounted scroll_view must register as a robot element");

    mock_backend::take_scroll_sets(); // clear any prior recordings
    robot
        .set_scroll(&el, 120.0, 340.0)
        .expect("scroll views must carry the set_scroll action");

    let sets = mock_backend::take_scroll_sets();
    assert_eq!(sets.len(), 1, "exactly one backend scroll write");
    assert_eq!((sets[0].1, sets[0].2), (120.0, 340.0));
}
