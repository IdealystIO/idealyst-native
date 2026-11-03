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

/// The robot's `open_link` verb is the OS's warm link: observers see it,
/// the navigators move, and it reports how far they got — so an E2E test
/// drives deep links the same way on every platform.
#[test]
fn robot_open_link_routes_like_an_os_link() {
    use runtime_shared::__serde_json::json;
    use runtime_shared::inbound_link::observe;
    use runtime_vocabulary::robot::bridge::invoke_command;

    let h = Harness::new();
    let (enter_world, flush_world) = (h.world.clone(), h.world.clone());
    runtime_vocabulary::robot::install_driver_env(move |f| enter_world.enter(|| f()), move || flush_world.flush());
    let (_fx, element) = nested_app(MountPolicy::LazyPersistent);
    let realized = h.mount(element);
    let root = one_root(&realized);
    let seen: Rc<RefCell<Vec<String>>> = Rc::default();
    let _watch = {
        let seen = seen.clone();
        observe(move |url| seen.borrow_mut().push(url.to_string()))
    };

    let answer = |url: &str| -> runtime_shared::__serde_json::Value {
        runtime_shared::__serde_json::from_str(&invoke_command("open_link", &json!({ "url": url })).unwrap())
            .unwrap()
    };
    assert_eq!(
        answer("myapp://settings"),
        json!({ "reached": "/settings", "complete": true, "routed": true })
    );
    h.flush();
    assert!(shows(&h, root, "SETTINGS"), "{}", h.tree(root));
    assert_eq!(seen.borrow().as_slice(), ["myapp://settings"]);

    assert!(invoke_command("open_link", &json!({})).unwrap_err().contains("'url'"));
    // Teardown runs effect cleanups, which need the world entered.
    h.world.enter(|| drop(realized));
    assert_eq!(
        answer("myapp://settings"),
        json!({ "reached": "/", "complete": false, "routed": false }),
        "no navigator is left to take it"
    );
}

// ---------------------------------------------------------------------------
// Robot `open_link`: the whole path, across navigators that mount late
// ---------------------------------------------------------------------------

mod open_link_deep {
    use super::*;
    use runtime_shared::__serde_json::{self as serde_json, json, Value};
    use runtime_vocabulary::glue::primitives::lazy::{lazy_split, LazyBodyThunk, LazyFuture};
    use runtime_vocabulary::glue::IntoElement as _;
    use runtime_vocabulary::robot::bridge::invoke_command;
    use runtime_world::{effect, signal};
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    const PROJECTS: Route<()> = Route::new("projects", "/projects");
    const LIST: Route<()> = Route::new("list", "");
    const PROJECT: Route<()> = Route::new("project", "/:id");
    const OVERVIEW: Route<()> = Route::new("overview", "");
    const SCHEDULE: Route<()> = Route::new("schedule", "/schedule");

    /// A robot-driven harness: `settle` flushes the world, as every real
    /// transport's driver env does.
    fn harness() -> Harness {
        runtime_vocabulary::robot::Robot::new().reset();
        host_mock::pump::install_executor();
        let h = Harness::new();
        let (enter_world, flush_world) = (h.world.clone(), h.world.clone());
        runtime_vocabulary::robot::install_driver_env(
            move |f| enter_world.enter(|| f()),
            move || flush_world.flush(),
        );
        h
    }

    fn open_link(args: Value) -> Value {
        serde_json::from_str(&invoke_command("open_link", &args).unwrap()).unwrap()
    }

    struct Gate(Rc<Cell<bool>>);
    impl Future for Gate {
        type Output = ();
        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
            if self.0.get() { Poll::Ready(()) } else { Poll::Pending }
        }
    }

    /// A lazy chunk whose load lands when `ready` is set and the executor
    /// is pumped — the web shape of `#[component(lazy)]`.
    fn lazy_chunk(ready: Rc<Cell<bool>>, body: impl Fn() -> Element + Clone + 'static) -> Element {
        lazy_split(move || -> LazyFuture {
            let ready = ready.clone();
            let body = body.clone();
            Box::pin(async move {
                Gate(ready).await;
                Ok(Box::new(move || body()) as LazyBodyThunk)
            })
        })
        .placeholder(|| text().content("LOADING").build())
        .into_element()
    }

    /// Root swap HOME | PROJECTS; PROJECTS is a lazy chunk holding a
    /// stack LIST | PROJECT(`/:id`); PROJECT is a second lazy chunk
    /// holding a swap OVERVIEW | SCHEDULE — the CrewForge shape that
    /// stopped one level per call.
    fn lazy_app(chunk1: Rc<Cell<bool>>, chunk2: Rc<Cell<bool>>) -> Element {
        let project = move |_: ()| {
            lazy_chunk(chunk2.clone(), || {
                swap_navigator(&OVERVIEW)
                    .screen(OVERVIEW, leaf("OVERVIEW"))
                    .screen(SCHEDULE, leaf("SCHEDULE"))
                    .build()
            })
        };
        swap_navigator(&HOME)
            .screen(HOME, leaf("HOME"))
            .screen(PROJECTS, move |_| {
                let project = project.clone();
                lazy_chunk(chunk1.clone(), move || {
                    stack_navigator(&LIST)
                        .screen(LIST, leaf("LIST"))
                        .screen(PROJECT, project.clone())
                        .build()
                })
            })
            .build()
    }

    fn land(h: &Harness) {
        h.world.flush();
        host_mock::pump::pump_tasks();
        h.world.flush();
    }

    /// Before: `open_link` answered `true` and stopped at `/projects` —
    /// the stack inside the lazy chunk mounted after the link's commit and
    /// opened its initial. Now the answer says how far the link got, and a
    /// `resume` carries it on through each level as it mounts.
    #[test]
    fn regression_open_link_resolves_the_whole_path_through_lazy_navigators() {
        let h = harness();
        let (c1, c2) = (Rc::new(Cell::new(false)), Rc::new(Cell::new(false)));
        let realized = h.mount(lazy_app(c1.clone(), c2.clone()));
        h.flush();
        let root = one_root(&realized);
        assert!(shows(&h, root, "HOME"));

        let url = "myapp://projects/7/schedule";
        assert_eq!(
            open_link(json!({ "url": url })),
            json!({ "reached": "/projects", "complete": false, "routed": true })
        );

        c1.set(true);
        land(&h);
        assert!(shows(&h, root, "LIST"), "{}", h.tree(root));
        assert_eq!(
            open_link(json!({ "url": url, "resume": true })),
            json!({ "reached": "/projects/7", "complete": false, "routed": true })
        );

        c2.set(true);
        land(&h);
        assert_eq!(
            open_link(json!({ "url": url, "resume": true })),
            json!({ "reached": "/projects/7/schedule", "complete": true, "routed": true })
        );
        assert!(shows(&h, root, "SCHEDULE"), "{}", h.tree(root));

        // The stack holds the back stack a cold load builds: LIST below.
        let navs: Value =
            serde_json::from_str(&invoke_command("list_navigators", &json!({})).unwrap()).unwrap();
        let stack = navs
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["type_name"] == "stack_navigator")
            .expect("the stack is registered");
        assert_eq!(stack["depth"], 2, "{stack}");
        assert_eq!(stack["active_path"], "/projects/7");
        h.world.enter(|| drop(realized));
    }

    /// The stack's state as `list_navigators` reports it.
    fn stack_state() -> Value {
        let navs: Value =
            serde_json::from_str(&invoke_command("list_navigators", &json!({})).unwrap()).unwrap();
        navs.as_array()
            .unwrap()
            .iter()
            .find(|n| n["type_name"] == "stack_navigator")
            .cloned()
            .expect("the stack is registered")
    }

    /// Android `load_url` (an `am start` relaunch with a deep link) booted
    /// at `/projects` and stopped: the root cleared the launch slot once
    /// its synchronous mount was up, and the stack inside the PROJECTS
    /// chunk mounted later, on a host with no address bar to read the
    /// path back from, so it opened LIST. Web reached the end through the
    /// live URL. Now the root holds the unreached launch path for the
    /// navigators that mount later, until one reaches its end.
    #[test]
    fn regression_cold_launch_link_reaches_navigators_that_mount_late() {
        let h = harness();
        runtime_shared::inbound_link::launch("https://localhost/projects/7/schedule");
        let (c1, c2) = (Rc::new(Cell::new(false)), Rc::new(Cell::new(false)));
        let realized = h.mount(lazy_app(c1.clone(), c2.clone()));
        h.flush();
        let root = one_root(&realized);
        assert!(shows(&h, root, "LOADING"), "{}", h.tree(root));
        assert_eq!(peek_initial_path(), None, "the one-shot slot is cleared by the root");

        c1.set(true);
        land(&h);
        assert!(!shows(&h, root, "LIST"), "the stack opens the linked project: {}", h.tree(root));
        let stack = stack_state();
        assert_eq!(stack["active_path"], "/projects/7", "{stack}");
        assert_eq!(stack["depth"], 2, "LIST below, as a cold load builds: {stack}");

        c2.set(true);
        land(&h);
        assert!(shows(&h, root, "SCHEDULE"), "{}", h.tree(root));
        h.world.enter(|| drop(realized));
    }

    /// The held launch path serves only the navigators that mount before
    /// the app navigates: once it moves (here, a link home and back), a
    /// navigator mounting later opens its own initial, as on web, where the
    /// address bar no longer names the launch path.
    #[test]
    fn a_navigation_supersedes_the_held_launch_link() {
        let h = harness();
        runtime_shared::inbound_link::launch("myapp://projects/7/schedule");
        let (c1, c2) = (Rc::new(Cell::new(false)), Rc::new(Cell::new(false)));
        let realized = h.mount(lazy_app(c1.clone(), c2.clone()));
        h.flush();
        let root = one_root(&realized);

        open_link(json!({ "url": "myapp://" }));
        assert!(shows(&h, root, "HOME"), "{}", h.tree(root));
        open_link(json!({ "url": "myapp://projects" }));
        c1.set(true);
        land(&h);
        assert!(shows(&h, root, "LIST"), "{}", h.tree(root));
        assert_eq!(stack_state()["active_path"], "/projects");
        h.world.enter(|| drop(realized));
    }

    /// A launch the initial mount resolves to its end holds nothing: a
    /// navigator mounted later under the same screen opens its initial.
    #[test]
    fn a_launch_link_the_initial_mount_reaches_is_not_held() {
        let h = harness();
        runtime_shared::inbound_link::launch("myapp://projects");
        let (c1, c2) = (Rc::new(Cell::new(false)), Rc::new(Cell::new(false)));
        let realized = h.mount(lazy_app(c1.clone(), c2));
        h.flush();
        let root = one_root(&realized);
        c1.set(true);
        land(&h);
        assert!(shows(&h, root, "LIST"), "{}", h.tree(root));
        assert_eq!(stack_state()["depth"], 1);
        h.world.enter(|| drop(realized));
    }

    /// A screen that builds its nested navigator in a later reactive pass
    /// (a region waiting on state an effect fills) mounts it after the
    /// link's commit, but inside the same flush: one `open_link` call
    /// re-routes it and reports the whole path.
    #[test]
    fn regression_open_link_reaches_a_navigator_mounted_after_the_commit_in_one_call() {
        let h = harness();
        let realized = h.mount(
            swap_navigator(&HOME)
                .screen(HOME, leaf("HOME"))
                .screen(DOCS, |_| {
                    let loaded = signal(false);
                    let _fill = effect(move || loaded.set(true));
                    runtime_scene::dyn_element(move || {
                        if loaded.get() {
                            stack_navigator(&INDEX)
                                .screen(INDEX, leaf("DOCS_INDEX"))
                                .screen(DETAIL, leaf("DOCS_DETAIL"))
                                .build()
                        } else {
                            text().content("LOADING").build()
                        }
                    })
                })
                .build(),
        );
        h.flush();
        let root = one_root(&realized);

        assert_eq!(
            open_link(json!({ "url": "myapp://docs/detail" })),
            json!({ "reached": "/docs/detail", "complete": true, "routed": true })
        );
        assert!(shows(&h, root, "DOCS_DETAIL"), "{}", h.tree(root));

        // A segment no screen matches: the chain stops where the app
        // resolves it (the stack's index), and the answer names it.
        assert_eq!(
            open_link(json!({ "url": "myapp://docs/nowhere" })),
            json!({ "reached": "/docs", "complete": false, "routed": true })
        );
        h.world.enter(|| drop(realized));
    }

    /// A navigator inside a reactive region whose branch appears after the
    /// screen mounted (`if loaded { … }`) must still sit under the
    /// screen's base. The region realized it from its driver effect, with
    /// the screen's navigator guards long popped: it read an empty base,
    /// reported `/` as its path, and no link could reach it.
    #[test]
    fn regression_navigator_in_a_late_region_branch_keeps_the_screen_base() {
        let h = harness();
        let loaded = Rc::new(RefCell::new(None));
        let realized = {
            let loaded = loaded.clone();
            h.mount(
                swap_navigator(&DOCS)
                    .screen(HOME, leaf("HOME"))
                    .screen(DOCS, move |_| {
                        let ready = signal(false);
                        *loaded.borrow_mut() = Some(ready);
                        runtime_scene::dyn_element(move || {
                            if ready.get() {
                                stack_navigator(&INDEX)
                                    .screen(INDEX, leaf("DOCS_INDEX"))
                                    .screen(DETAIL, leaf("DOCS_DETAIL"))
                                    .build()
                            } else {
                                text().content("LOADING").build()
                            }
                        })
                    })
                    .build(),
            )
        };
        h.flush();
        let ready = loaded.borrow().expect("DOCS built");
        h.world.enter(|| ready.set(true));
        h.flush();
        let navs: Value =
            serde_json::from_str(&invoke_command("list_navigators", &json!({})).unwrap()).unwrap();
        let stack = navs
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["type_name"] == "stack_navigator")
            .expect("the late stack registered");
        assert_eq!(stack["base"], "/docs", "{stack}");
        assert_eq!(stack["active_path"], "/docs", "{stack}");
        h.world.enter(|| drop(realized));
    }

    /// A link the app's interceptor claims is not routed — the client
    /// must not resume it (that would bypass the gate).
    #[test]
    fn open_link_reports_an_intercepted_link_as_not_routed() {
        let h = harness();
        let (_fx, element) = nested_app(MountPolicy::LazyPersistent);
        let realized = h.mount(element);
        h.flush();
        let _gate = intercept(|_| true);
        assert_eq!(
            open_link(json!({ "url": "myapp://settings" })),
            json!({ "reached": "/", "complete": false, "routed": false })
        );
        h.world.enter(|| drop(realized));
    }

    /// The active screen's query is reported beside the path-only
    /// `active_path` — decoded in `query`, encoded in `href`.
    #[test]
    fn navigator_state_reports_the_active_query_and_href() {
        let h = harness();
        let (_fx, element) = nested_app(MountPolicy::LazyPersistent);
        let realized = h.mount(element);
        h.flush();
        assert_eq!(
            open_link(json!({ "url": "myapp://settings?asset_tag=~7781&tab=2" })),
            json!({ "reached": "/settings", "complete": true, "routed": true })
        );
        let navs: Value =
            serde_json::from_str(&invoke_command("list_navigators", &json!({})).unwrap()).unwrap();
        let swap = &navs.as_array().unwrap()[0];
        assert_eq!(swap["active_path"], "/settings");
        assert_eq!(swap["query"], json!({ "asset_tag": "~7781", "tab": "2" }));
        assert_eq!(swap["href"], "/settings?asset_tag=~7781&tab=2");

        // The query is part of what `complete` checks: a different one on
        // the same path is not yet reached, then is once the leaf takes it.
        assert_eq!(
            open_link(json!({ "url": "myapp://settings?tab=3" })),
            json!({ "reached": "/settings", "complete": true, "routed": true })
        );
        let navs: Value =
            serde_json::from_str(&invoke_command("list_navigators", &json!({})).unwrap()).unwrap();
        assert_eq!(navs[0]["query"], json!({ "tab": "3" }));
        assert_eq!(navs[0]["href"], "/settings?tab=3");
        h.world.enter(|| drop(realized));
    }
}
