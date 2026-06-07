//! Regression: a stack navigator's CURRENT-SCREEN content must appear in
//! the robot registry tree (`Robot::snapshot()` / `find`), nested under the
//! `Navigator` node.
//!
//! Before the fix, each mounted screen's reactive scope — which owns the
//! screen content's robot-registry entries — was kept alive ONLY by `Rc`
//! clones inside the `NavigatorHost` handed to the SDK handler. A handler
//! that drops the host (the SSR primitive-chrome handler does) let the
//! per-screen `scopes` map's refcount hit zero when `navigator::build`
//! returned, dropping the screen scope and firing every screen element's
//! `on_cleanup(deregister)`. Result: `Robot::snapshot()` showed a bare
//! `Navigator` with no children and `find(test_id)` returned `None` — which
//! is exactly what the Idealyst Inspector's empty TREE panel surfaced. The
//! fix has the framework retain the `scopes` map for the navigator's
//! lifetime (`crates/runtime/core/src/walker/navigator.rs`), so screen
//! scopes survive regardless of which backend/handler is in play.
//!
//! Run: `cargo test -p stack-navigator --features robot --test robot_screen_tree`.

use std::cell::RefCell;
use std::rc::Rc;

use backend_ssr::SsrBackend;
use runtime_core::robot::{ElementKind, Query, Robot, TreeNode};
use runtime_core::{text, ui, view, IntoElement, Ref, Route, Screen};
use stack_navigator::{Navigator, StackBuilder, StackHandle, StackScreenExt};

const HOME: Route<()> = Route::<()>::new("home", "/");

fn subtree_has_test_id(node: &TreeNode, id: &str) -> bool {
    node.test_id == Some(id) || node.children.iter().any(|c| subtree_has_test_id(c, id))
}

#[test]
fn navigator_screen_content_is_in_robot_tree() {
    // Fresh registry for an isolated assertion.
    Robot::new().reset();

    let mut backend = SsrBackend::new();
    stack_navigator::chrome::register(&mut backend);
    let backend = Rc::new(RefCell::new(backend));

    // Hold the owner: dropping it tears the tree down and empties the
    // registry, so the snapshot must be taken while it's alive (this is the
    // running-app / Inspector condition).
    let _owner = runtime_core::mount(backend.clone(), move || {
        let nav: Ref<StackHandle> = Ref::new();
        let builder = Navigator::new(&HOME).screen(HOME, move |_| {
            Screen::new(
                view(vec![text("SCREEN BODY")
                    .test_id("screen-marker")
                    .into_element()])
                .into_element(),
            )
            .title("Home")
        });
        ui! { builder.bind(nav) }
    });

    let robot = Robot::new();

    // The screen's element must be findable at all (it was `None` before).
    assert!(
        robot.find(Query::test_id("screen-marker")).is_some(),
        "navigator screen content must be registered in the robot registry; \
         got None — the per-screen scope was dropped at navigator-build return"
    );

    // …and it must be nested UNDER the Navigator node, not orphaned.
    let tree = robot.snapshot();
    let under_navigator = tree.iter().any(|root| {
        root.kind == ElementKind::Navigator
            && root.children.iter().any(|c| subtree_has_test_id(c, "screen-marker"))
    });
    assert!(
        under_navigator,
        "screen content must appear under the Navigator node in snapshot(); tree = {:#?}",
        tree.iter().map(|n| (n.kind, n.test_id, n.children.len())).collect::<Vec<_>>(),
    );
}
