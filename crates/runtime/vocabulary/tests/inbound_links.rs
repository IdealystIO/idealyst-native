//! Warm inbound links — a deep link / universal link that arrives while the
//! app is running moves the live navigators to the link's path
//! (`runtime_shared::inbound_link::deliver` → the router the navigator
//! handlers install → `handlers::navigator::inbound::open_path`).
//!
//! The cold-start link is pinned in `walker_ports.rs`
//! (`port_cold_start_*`): the host seeds the launch slot and the initial
//! mount resolves it. These tests pin the other half of the contract —
//! the SAME link opens the SAME screen when the app is already up —
//! including the nested case, where the navigator that owns the end of
//! the path does not exist until its parent's screen mounts.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use host_mock::Harness;
use runtime_shared::inbound_link::{deliver, intercept, route};
use runtime_shared::primitives::navigator::{peek_initial_path, Route};
use runtime_scene::Element;
use runtime_vocabulary::builders::{navigator_outlet, stack_navigator, swap_navigator, text, view};
use runtime_vocabulary::prims::{MountPolicy, StackNav, SwapNav};
use runtime_world::inject;

const HOME: Route<()> = Route::new("home", "/");
const SETTINGS: Route<()> = Route::new("settings", "/settings");
const DOCS: Route<()> = Route::new("docs", "/docs");
const INDEX: Route<()> = Route::new("index", "");
const DETAIL: Route<()> = Route::new("detail", "/detail");

fn shows(h: &Harness, root: host_mock::Node, label: &str) -> bool {
    h.tree(root).contains(&format!("text {label:?}"))
}

fn one_root(realized: &runtime_scene::Realized<host_mock::Node>) -> host_mock::Node {
    let nodes = realized.collect_nodes();
    assert_eq!(nodes.len(), 1, "fixture mounts one root");
    nodes[0]
}

fn probe(label: &'static str, builds: Rc<Cell<u32>>) -> impl Fn(()) -> Element + 'static {
    move |_| {
        builds.set(builds.get() + 1);
        view().child(text().content(label)).build()
    }
}

fn leaf(label: &'static str) -> impl Fn(()) -> Element + 'static {
    move |_| view().child(text().content(label)).build()
}

/// Root swap: HOME | SETTINGS | DOCS, where DOCS hosts a nested stack
/// (INDEX → DETAIL). The nested StackNav is captured on every build of
/// its layout so a test can read the CURRENT instance.
struct Fixture {
    swap: Rc<RefCell<Option<SwapNav>>>,
    stack: Rc<RefCell<Option<StackNav>>>,
    docs_builds: Rc<Cell<u32>>,
    settings_builds: Rc<Cell<u32>>,
}

fn nested_app(policy: MountPolicy) -> (Fixture, Element) {
    let fx = Fixture {
        swap: Rc::new(RefCell::new(None)),
        stack: Rc::new(RefCell::new(None)),
        docs_builds: Rc::new(Cell::new(0)),
        settings_builds: Rc::new(Cell::new(0)),
    };
    let element = {
        let swap = fx.swap.clone();
        let stack = fx.stack.clone();
        let docs_builds = fx.docs_builds.clone();
        swap_navigator(&HOME)
            .screen(HOME, leaf("HOME"))
            .screen(SETTINGS, probe("SETTINGS", fx.settings_builds.clone()))
            .screen(DOCS, move |_| {
                docs_builds.set(docs_builds.get() + 1);
                let stack = stack.clone();
                stack_navigator(&INDEX)
                    .screen(INDEX, leaf("DOCS_INDEX"))
                    .screen(DETAIL, leaf("DOCS_DETAIL"))
                    .layout(move || {
                        *stack.borrow_mut() = inject::<StackNav>();
                        view().child(navigator_outlet()).build()
                    })
                    .build()
            })
            .mount_policy(policy)
            .layout(move || {
                *swap.borrow_mut() = inject::<SwapNav>();
                view().child(navigator_outlet()).build()
            })
            .build()
    };
    (fx, element)
}

#[test]
fn regression_warm_link_selects_the_linked_screen_on_a_running_swap() {
    let h = Harness::new();
    let (fx, element) = nested_app(MountPolicy::LazyPersistent);
    let realized = h.mount(element);
    let root = one_root(&realized);
    assert!(shows(&h, root, "HOME"));

    // Before the fix nothing routed a warm link: no router existed, so
    // the app stayed on HOME.
    assert!(deliver("myapp://settings"), "a navigator takes the link");
    h.flush();
    assert!(shows(&h, root, "SETTINGS"), "{}", h.tree(root));
    assert!(!shows(&h, root, "HOME"));
    let swap = fx.swap.borrow().clone().unwrap();
    assert_eq!(h.world.enter(|| swap.active_path.get()), "/settings");

    // The same link again is a no-op, not a rebuild.
    deliver("myapp://settings");
    h.flush();
    assert_eq!(fx.settings_builds.get(), 1);
    drop(realized);
}

#[test]
fn regression_warm_nested_link_mounts_parent_and_resolves_child_from_the_link() {
    let h = Harness::new();
    let (fx, element) = nested_app(MountPolicy::LazyDisposing);
    let realized = h.mount(element);
    h.flush();
    let root = one_root(&realized);
    assert!(shows(&h, root, "HOME"));
    assert!(fx.stack.borrow().is_none(), "DOCS (and its stack) not mounted yet");

    // The nested stack does not exist when the link arrives; the parent's
    // commit carries the link so the stack resolves `/detail` as it mounts.
    deliver("https://example.com/docs/detail");
    h.flush();
    assert!(shows(&h, root, "DOCS_DETAIL"), "{}", h.tree(root));
    let stack = fx.stack.borrow().clone().expect("nested stack mounted");
    h.world.enter(|| {
        assert_eq!(stack.active_path.get(), "/docs/detail");
        // Same back stack a cold start builds: the index sits below.
        assert_eq!(stack.depth.get(), 2);
        assert!(stack.can_go_back.get());
    });

    // The link path was scoped to that commit: it does not poison the
    // launch slot or a later rebuild of the same subtree.
    assert!(peek_initial_path().is_none());
    let swap = fx.swap.borrow().clone().unwrap();
    let on_select = swap.on_select.clone();
    on_select("home");
    h.flush();
    on_select("docs");
    h.flush();
    assert_eq!(fx.docs_builds.get(), 2, "LazyDisposing rebuilt DOCS");
    assert!(
        shows(&h, root, "DOCS_INDEX"),
        "a rebuild opens on the configured initial, not the old link:\n{}",
        h.tree(root)
    );
    drop(realized);
}

#[test]
fn regression_warm_nested_link_on_a_shown_parent_moves_only_the_child() {
    let h = Harness::new();
    let (fx, element) = nested_app(MountPolicy::LazyPersistent);
    let realized = h.mount(element);
    let root = one_root(&realized);
    deliver("myapp://docs");
    h.flush();
    assert!(shows(&h, root, "DOCS_INDEX"), "{}", h.tree(root));
    assert_eq!(fx.docs_builds.get(), 1);

    // The parent's slice is unchanged: it must not re-select (that would
    // tear the nested stack down) — only the stack pushes.
    deliver("myapp://docs/detail");
    h.flush();
    assert!(shows(&h, root, "DOCS_DETAIL"), "{}", h.tree(root));
    assert_eq!(fx.docs_builds.get(), 1, "parent screen not rebuilt");
    let stack = fx.stack.borrow().clone().unwrap();
    h.world.enter(|| assert_eq!(stack.depth.get(), 2, "pushed onto the live stack"));

    // Back returns to where the user was before the link.
    let pop = stack.pop.clone();
    pop();
    h.flush();
    assert!(shows(&h, root, "DOCS_INDEX"), "{}", h.tree(root));
    drop(realized);
}

#[test]
fn regression_warm_link_pushes_onto_a_root_stack() {
    let h = Harness::new();
    let ctx: Rc<RefCell<Option<StackNav>>> = Rc::new(RefCell::new(None));
    let element = {
        let ctx = ctx.clone();
        stack_navigator(&HOME)
            .screen(HOME, leaf("HOME"))
            .screen(SETTINGS, leaf("SETTINGS"))
            .layout(move || {
                *ctx.borrow_mut() = inject::<StackNav>();
                view().child(navigator_outlet()).build()
            })
            .build()
    };
    let realized = h.mount(element);
    h.flush();
    let root = one_root(&realized);

    deliver("myapp://settings");
    h.flush();
    assert!(shows(&h, root, "SETTINGS"), "{}", h.tree(root));
    let ctx = ctx.borrow().clone().unwrap();
    h.world.enter(|| {
        assert_eq!(ctx.depth.get(), 2);
        assert!(ctx.can_go_back.get());
    });
    drop(realized);
}

#[test]
fn regression_warm_link_query_only_change_updates_the_leaf_state() {
    let h = Harness::new();
    let (fx, element) = nested_app(MountPolicy::LazyPersistent);
    let realized = h.mount(element);
    deliver("myapp://settings?tab=1");
    h.flush();
    deliver("myapp://settings?tab=2");
    h.flush();
    let swap = fx.swap.borrow().clone().unwrap();
    assert_eq!(
        h.world.enter(|| swap.query.get().get("tab").map(str::to_string)),
        Some("2".to_string()),
        "the screen's state follows the link's query"
    );
    assert_eq!(fx.settings_builds.get(), 1, "query change is not a remount");
    drop(realized);
}

#[test]
fn unmatched_link_reports_false_and_moves_nothing() {
    let h = Harness::new();
    let (_fx, element) = nested_app(MountPolicy::LazyPersistent);
    let realized = h.mount(element);
    let root = one_root(&realized);
    // Routes match by prefix (as at cold start): HOME (`/`) takes the
    // path and leaves the tail for a nested navigator HOME doesn't have.
    // The app is already there, so the link reports it did not land.
    assert!(!deliver("myapp://nowhere/at/all"));
    h.flush();
    assert!(shows(&h, root, "HOME"));
    // Already showing the linked screen counts as landing.
    assert!(deliver("myapp://"));
    // Teardown runs effect cleanups, which need the world entered.
    h.world.enter(|| drop(realized));
    // Navigators deregister at teardown: nothing is left to take a link.
    assert!(!deliver("myapp://settings"));
}

#[test]
fn intercepted_link_waits_until_the_app_routes_it() {
    let h = Harness::new();
    let (_fx, element) = nested_app(MountPolicy::LazyPersistent);
    let realized = h.mount(element);
    let root = one_root(&realized);

    let held: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let gate = {
        let held = held.clone();
        intercept(move |url| {
            *held.borrow_mut() = Some(url.to_string());
            true
        })
    };
    deliver("myapp://settings");
    h.flush();
    assert!(shows(&h, root, "HOME"), "the gate held the link");

    // "Signed in": the app routes what it held.
    drop(gate);
    let url = held.borrow_mut().take().unwrap();
    assert!(route(&url));
    h.flush();
    assert!(shows(&h, root, "SETTINGS"), "{}", h.tree(root));
    drop(realized);
}
