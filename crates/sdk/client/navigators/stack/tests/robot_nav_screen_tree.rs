//! Regression: a stack navigator's CURRENT-SCREEN content must stay in the
//! robot tree (`Robot::snapshot()` / `find`) AFTER a runtime navigation
//! (push/reset), not just after the initial mount.
//!
//! `robot_screen_tree.rs` covers the INITIAL-mount case (the per-screen
//! `scopes` keepalive). This one covers the NAVIGATION-time case: a screen
//! mounted on `NavCommand::Push` runs through the same framework
//! `host.mount_screen` -> `walker::build` path the macOS handler uses, and
//! its content must remain reachable from a snapshot root. The screen body
//! registers with the `PARENT_STACK` top at dispatch time as its robot
//! parent; if that parent is dead by snapshot time the content is an orphan,
//! and `Robot::snapshot()` must still surface it (see the matching
//! `snapshot_surfaces_orphan_with_dead_parent` unit test in
//! `runtime-core::robot`).
//!
//! Note: SSR's default `make_navigator_handle` returns an inert (control-less)
//! handle, so a `Ref<StackHandle>` can't drive navigation here. This test
//! installs a handler that stashes its `NavigatorControl` and dispatches the
//! `Push` directly through it — the SAME `NavigatorControl::dispatch` ->
//! installed-closure -> `mount_screen` path a live macOS `nav.push()` reaches.
//!
//! Run: `cargo test -p stack-navigator --features robot --test robot_nav_screen_tree`.

use std::cell::RefCell;
use std::rc::Rc;

use backend_ssr::SsrBackend;
use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::navigator::{
    NavCommand, NavigatorControl, NavigatorHandler, NavigatorHost, NavigatorOps, RegisterNavigator,
};
use runtime_core::robot::{Query, Robot, TreeNode};
use runtime_core::{text, ui, view, Backend, IntoElement, Ref, Route, Screen};
use stack_navigator::{Navigator, StackBuilder, StackHandle, StackPresentation, StackScreenExt};

const HOME: Route<()> = Route::<()>::new("home", "/");
const DETAIL: Route<()> = Route::<()>::new("detail", "/detail");

fn subtree_has_test_id(node: &TreeNode, id: &str) -> bool {
    node.test_id == Some(id) || node.children.iter().any(|c| subtree_has_test_id(c, id))
}

thread_local! {
    static TEST_CONTROL: RefCell<Option<Rc<NavigatorControl>>> = const { RefCell::new(None) };
}

// A handler that installs a dispatcher (the SSR chrome handler doesn't),
// mirroring `macos.rs`: Push calls `mount_screen` and stashes the result.
struct NavTestHandler {
    stack: Rc<RefCell<Vec<u64>>>,
}

struct NoopOps;
impl NavigatorOps for NoopOps {}
static NOOP_OPS: NoopOps = NoopOps;

impl NavigatorHandler<SsrBackend> for NavTestHandler {
    fn init(
        &mut self,
        backend: &mut SsrBackend,
        host: NavigatorHost<<SsrBackend as Backend>::Node>,
        _presentation: Rc<dyn std::any::Any>,
    ) -> <SsrBackend as Backend>::Node {
        let outlet = backend.create_view(&AccessibilityProps::default());
        TEST_CONTROL.with(|c| *c.borrow_mut() = Some(host.control.clone()));

        let stack = self.stack.clone();
        let mount_screen = host.mount_screen.clone();
        let release_screen = host.release_screen.clone();
        host.control.install(Box::new(move |cmd| match cmd {
            NavCommand::Push { name, params, .. } => {
                let result = mount_screen(name, params, None);
                stack.borrow_mut().push(result.scope_id);
            }
            NavCommand::Reset { name, params, .. } => {
                let result = mount_screen(name, params, None);
                for sid in stack.borrow_mut().drain(..) {
                    release_screen(sid);
                }
                stack.borrow_mut().push(result.scope_id);
            }
            _ => {}
        }));
        outlet
    }

    fn attach_initial(
        &mut self,
        _backend: &mut SsrBackend,
        _screen: <SsrBackend as Backend>::Node,
        scope_id: u64,
        _options: Box<dyn std::any::Any>,
    ) {
        self.stack.borrow_mut().push(scope_id);
    }

    fn make_handle(&self) -> runtime_core::NavigatorHandle {
        runtime_core::NavigatorHandle::new(Rc::new(()), &NOOP_OPS)
    }
}

fn register_test_handler(backend: &mut SsrBackend) {
    backend.register_navigator::<StackPresentation, _>(|| {
        Box::new(NavTestHandler {
            stack: Rc::new(RefCell::new(Vec::new())),
        })
    });
}

#[test]
fn navigation_time_screen_content_is_in_robot_tree() {
    Robot::new().reset();
    TEST_CONTROL.with(|c| *c.borrow_mut() = None);

    let mut backend = SsrBackend::new();
    register_test_handler(&mut backend);
    let backend = Rc::new(RefCell::new(backend));

    let nav: Ref<StackHandle> = Ref::new();
    let owner = runtime_core::mount(backend.clone(), move || {
        let builder = Navigator::new(&HOME)
            .screen(HOME, move |_| {
                Screen::new(
                    view(vec![text("HOME").test_id("home-marker").into_element()]).into_element(),
                )
                .title("Home")
            })
            .screen(DETAIL, move |_| {
                Screen::new(
                    view(vec![text("DETAIL").test_id("detail-marker").into_element()])
                        .into_element(),
                )
                .title("Detail")
            });
        ui! { builder.bind(nav) }
    });

    let robot = Robot::new();
    assert!(
        robot.find(Query::test_id("home-marker")).is_some(),
        "home content must be present after initial mount"
    );

    // Navigate to DETAIL through the control the handler installed onto —
    // the same path a live macOS `nav.push()` reaches.
    let control = TEST_CONTROL
        .with(|c| c.borrow().clone())
        .expect("handler stashed its control");
    control.dispatch(NavCommand::Push {
        name: DETAIL.name(),
        url: "/detail".to_string(),
        params: Box::new(()),
        state: None,
    });

    // The detail content must be registered AND reachable from a snapshot
    // root (not orphaned by a dead parent, which was the inspector's bare
    // `Navigator` symptom).
    assert!(
        robot.find(Query::test_id("detail-marker")).is_some(),
        "detail screen content must be registered after navigation"
    );
    let tree = robot.snapshot();
    assert!(
        tree.iter().any(|root| subtree_has_test_id(root, "detail-marker")),
        "detail content must be reachable from a snapshot root after navigation; tree = {:#?}",
        tree.iter()
            .map(|n| (n.kind, n.test_id, n.children.len()))
            .collect::<Vec<_>>()
    );

    // Tear down deterministically: drop the owner (frees the tree) and clear
    // the control out of the thread-local so it isn't dropped during TLS
    // destruction at process exit (which would touch an already-destroyed
    // thread-local and abort).
    drop(owner);
    TEST_CONTROL.with(|c| *c.borrow_mut() = None);
    Robot::new().reset();
}
