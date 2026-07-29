//! New-core ports of the old walker's behavioral coverage — the
//! CORE-LEVEL semantics pinned by `crates/dev/mock-backend/tests/`
//! (the old-core walker integration suite), re-expressed against the
//! new core (runtime-world + runtime-scene + runtime-vocabulary) on the
//! `host-mock` substrate.
//!
//! Each test names the mock-backend test it derives from:
//!
//! - `navigator_control_keepalive.rs::reactive_nav_state_survives_handler_dropping_host`
//!   → `port_nav_state_survives_without_external_retention`
//! - `nested_dual_layout.rs::nested_swap_in_swap_renders_both_sidebars_simultaneously`
//!   → `port_nested_swap_in_swap_renders_both_sidebars_simultaneously`
//! - `nested_dual_layout.rs::nested_stack_in_swap_renders_both_chromes_and_pushes_independently`
//!   → `port_nested_stack_in_swap_renders_both_chromes_and_pushes_independently`
//! - `nested_teardown_repro.rs::nested_stack_in_swap_teardown_without_url_sync`
//!   → `port_nested_navigator_teardown_releases_inner_screens`
//! - `swap_navigator_local.rs::regression_deep_link_initial_cached_under_resolved_route`
//!   → `port_cold_start_deep_link_mounts_url_screen_and_caches_under_resolved_route`
//! - `swap_navigator_local.rs::regression_deep_link_active_path_is_concrete_url_not_pattern`
//!   → `port_deep_link_active_path_is_concrete_url_not_pattern`
//! - `navigator_url_sync.rs::regression_cold_start_nested_deep_link_preserves_full_url`
//!   (the handler leg: nested resolution + synthesized back stack + the
//!   root clearing the launch-path slot)
//!   → `port_cold_start_nested_deep_link_resolves_through_both_navigators`
//! - `navigator_url_sync.rs::regression_swap_parameterized_route_selects_by_url_not_name`
//!   (the structural leg; history dedupe is service policy)
//!   → `port_swap_parameterized_route_selects_by_url_not_name`
//! - `swap_navigator_local.rs::regression_on_select_params_route_is_refused_not_panic`
//!   → `port_on_select_with_params_route_is_refused_not_panic`
//! - `swap_navigator_local.rs::{regression_navigator_root_fills_container_by_default,
//!   navigator_root_author_style_overrides_default_fill}`
//!   → `port_navigator_root_fills_container_by_default_and_author_style_wins`
//! - `swap_navigator_local.rs::{regression_outlet_fills_by_default,
//!   outlet_author_style_replaces_default_fill}`
//!   → `port_outlet_fills_by_default_and_author_style_replaces_it`
//! - `swap_navigator_local.rs::regression_author_layout_effect_has_reactive_scope`
//!   → `port_author_layout_effect_has_reactive_scope_and_dies_with_navigator`
//! - `swap_navigator_local.rs::regression_lazy_disposing_cleanup_navigation_does_not_double_borrow`
//!   → `port_lazy_disposing_cleanup_navigation_commits_without_panic`
//! - `theme_chrome_cohort.rs::regression_chrome_static_token_style_reapplies_on_theme_swap`
//!   → `port_chrome_static_token_style_reapplies_on_theme_swap`
//! - `theme_chrome_cohort.rs::regression_chrome_retints_after_lazy_disposing_navigation`
//!   → `port_chrome_retints_after_lazy_disposing_navigation`
//! - `theme_chrome_cohort.rs::regression_shared_resolution_key_effects_survive_repeated_theme_swaps`
//!   → `port_shared_sheet_nodes_all_reapply_on_repeated_theme_swaps`
//! - `navigator_url_sync.rs::stack_push_pushes_url_and_browser_back_pops` +
//!   `stack_scroll_snapshot_restores_on_browser_back` (the seam legs:
//!   registration data, dispatch/commit hooks, the outlet node handed
//!   to the service for scroll bookkeeping)
//!   → `port_stack_url_sync_registers_hooks_push_pop_and_hands_the_real_outlet`
//! - `navigator_url_sync.rs::regression_programmatic_pop_swallows_its_popstate_echo`
//!   (the seam mechanism: the suppress bit captured at dispatch skips
//!   `after_commit` for the service's own echo)
//!   → `port_url_sync_suppress_bit_skips_after_commit`
//! - `navigator_url_sync.rs::regression_nested_stack_popstate_does_not_remount_parent_swap_screen`
//!   (the seam legs: per-navigator base slices, prefix resolvers, and
//!   base-composed dispatch through the registration)
//!   → `port_nested_navigator_url_sync_registers_composed_base_and_resolver`
//!
//! Browser-history behavior itself (pushState/replaceState writes,
//! popstate reconciliation, scroll snapshot/restore) lives in the
//! host's `UrlSyncService` implementation (backend-web's
//! `newcore_url_sync`) — these ports pin the vocabulary's half of that
//! contract: registration data, hook ordering, the suppress bit, and
//! the base partition.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use host_mock::Harness;
use runtime_core::primitives::navigator::{
    peek_initial_path, set_initial_path, NavCommand, Route, RouteParams,
};
use runtime_core::{
    Color, Length, StyleApplication, StyleRules, StyleSheet, TokenEntry, TokenValue, Tokenized,
};
use runtime_scene::Element;
use runtime_vocabulary::builders::{navigator_outlet, stack_navigator, swap_navigator, text, view};
use runtime_vocabulary::handlers::nav_url_sync::{
    clear_url_sync_service, install_url_sync_service, CommittedKind, NavSyncKind,
    NavSyncRegistration, UrlSyncService,
};
use runtime_vocabulary::prims::{MountPolicy, NavHandle, StackNav, StackRetention, SwapNav};
use runtime_vocabulary::{on_teardown, theme};
use runtime_world::{effect, inject};

// ===========================================================================
// Shared fixtures
// ===========================================================================

const HOME: Route<()> = Route::new("home", "/");
const ABOUT: Route<()> = Route::new("about", "/about");
const SETTINGS: Route<()> = Route::new("settings", "/settings");
const PROFILE: Route<()> = Route::new("profile", "/profile");
const DOCS: Route<()> = Route::new("docs", "/docs");
const REPORTS: Route<()> = Route::new("reports", "/reports");
const DETAIL: Route<()> = Route::new("detail", "/detail");

/// True when a text leaf with `label` is REACHABLE from `root` in the
/// mock's live tree (detached cached screens keep their nodes but drop
/// out of the parent/child maps, so this is the "visible" predicate the
/// old `MockBackend::contains_text` provided).
fn shows(h: &Harness, root: host_mock::Node, label: &str) -> bool {
    h.tree(root).contains(&format!("text {label:?}"))
}

fn one_root(realized: &runtime_scene::Realized<host_mock::Node>) -> host_mock::Node {
    let nodes = realized.collect_nodes();
    assert_eq!(nodes.len(), 1, "fixture mounts one root");
    nodes[0]
}

/// A screen body that bumps `builds` when the route builder runs and
/// bumps `torn` when the screen's scope tears down.
fn probe_screen(
    label: &'static str,
    builds: Rc<Cell<u32>>,
    torn: Rc<Cell<u32>>,
) -> impl Fn(()) -> Element + 'static {
    move |_| {
        builds.set(builds.get() + 1);
        let torn = torn.clone();
        on_teardown(move || torn.set(torn.get() + 1));
        view().child(text().content(label)).build()
    }
}

// ===========================================================================
// Keepalive: nav-state signals live exactly as long as the navigator
// (from navigator_control_keepalive.rs). The old bug was the SDK handler
// dropping the only strong ref to the control scope that owned
// `active_route`; on the new core ownership is structural (the signals
// are created inside the mount handler and collected into the
// navigator's `Realized`) — this pins that nothing else needs to retain
// anything for nav state to stay readable and live.
// ===========================================================================

#[test]
fn port_nav_state_survives_without_external_retention() {
    let h = Harness::new();
    let ctx_slot: Rc<RefCell<Option<SwapNav>>> = Rc::new(RefCell::new(None));

    let element = {
        let ctx_slot = ctx_slot.clone();
        swap_navigator(&HOME)
            .screen(HOME, |_| view().child(text().content("HOME")).build())
            .screen(ABOUT, |_| view().child(text().content("ABOUT")).build())
            .layout(move || {
                *ctx_slot.borrow_mut() = inject::<SwapNav>();
                view().child(navigator_outlet()).build()
            })
            .build()
    };
    // Deliberately NO `.on_handle` and no fixture struct: after mount the
    // ONLY thing alive is the Realized. The captured SwapNav is a clone of
    // plain signal handles — it retains nothing.
    let realized = h.mount(element);
    let ctx = ctx_slot.borrow_mut().take().expect("layout captured SwapNav");

    assert_eq!(
        h.world.enter(|| ctx.active_route.get()),
        "home",
        "nav state readable after mount with no external retention"
    );

    // And it stays LIVE — a navigation through the chrome-facing select
    // still commits and updates the mirror.
    (ctx.on_select)("about");
    h.flush();
    assert_eq!(h.world.enter(|| ctx.active_route.get()), "about");
    assert_eq!(h.world.enter(|| ctx.active_path.get()), "/about");
    drop(realized);
}

// ===========================================================================
// Nested navigators with two simultaneous author layouts
// (from nested_dual_layout.rs)
// ===========================================================================

#[test]
fn port_nested_swap_in_swap_renders_both_sidebars_simultaneously() {
    const SUMMARY: Route<()> = Route::new("summary", "");
    const TRENDS: Route<()> = Route::new("trends", "/trends");

    let h = Harness::new();
    let outer_slot: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
    let inner_slot: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));

    let element = {
        let outer_slot = outer_slot.clone();
        let inner_slot = inner_slot.clone();
        swap_navigator(&HOME)
            .screen(HOME, |_| view().child(text().content("HOME_BODY")).build())
            .screen(REPORTS, move |_| {
                let inner_slot = inner_slot.clone();
                // The whole secondary navigator (with its own right-hand
                // sidebar) is this outer screen's body.
                swap_navigator(&SUMMARY)
                    .screen(SUMMARY, |_| {
                        view().child(text().content("SUMMARY_BODY")).build()
                    })
                    .screen(TRENDS, |_| {
                        view().child(text().content("TRENDS_BODY")).build()
                    })
                    // Secondary sidebar on the RIGHT: outlet first.
                    .layout(|| {
                        view()
                            .child(navigator_outlet())
                            .child(text().content("SECONDARY_SIDEBAR"))
                            .build()
                    })
                    .mount_policy(MountPolicy::LazyPersistent)
                    .on_handle(move |handle| *inner_slot.borrow_mut() = Some(handle))
                    .build()
            })
            // Primary sidebar on the LEFT: chrome first, outlet after.
            .layout(|| {
                view()
                    .child(text().content("PRIMARY_SIDEBAR"))
                    .child(navigator_outlet())
                    .build()
            })
            .mount_policy(MountPolicy::LazyPersistent)
            .on_handle(move |handle| *outer_slot.borrow_mut() = Some(handle))
            .build()
    };
    let realized = h.mount(element);
    let root = one_root(&realized);
    let outer = outer_slot.borrow().clone().expect("outer handle bound");

    // Start on HOME: only the primary sidebar + HOME body.
    assert!(shows(&h, root, "PRIMARY_SIDEBAR"), "{}", h.tree(root));
    assert!(shows(&h, root, "HOME_BODY"), "{}", h.tree(root));
    assert!(!shows(&h, root, "SECONDARY_SIDEBAR"), "{}", h.tree(root));
    assert!(!shows(&h, root, "SUMMARY_BODY"), "{}", h.tree(root));

    // Enter REPORTS: on a route nested in both, BOTH sidebars render at
    // once, wrapping the innermost (SUMMARY) screen.
    outer.select(&REPORTS, ());
    h.flush();
    assert!(
        shows(&h, root, "PRIMARY_SIDEBAR") && shows(&h, root, "SECONDARY_SIDEBAR"),
        "both sidebars must render simultaneously:\n{}",
        h.tree(root)
    );
    assert!(shows(&h, root, "SUMMARY_BODY"), "{}", h.tree(root));
    assert!(!shows(&h, root, "HOME_BODY"), "{}", h.tree(root));

    // Switch the INNER sidebar: only the inner outlet's child swaps.
    let inner = inner_slot.borrow().clone().expect("inner handle bound");
    inner.select(&TRENDS, ());
    h.flush();
    assert!(shows(&h, root, "PRIMARY_SIDEBAR"), "{}", h.tree(root));
    assert!(shows(&h, root, "SECONDARY_SIDEBAR"), "{}", h.tree(root));
    assert!(shows(&h, root, "TRENDS_BODY"), "{}", h.tree(root));
    assert!(!shows(&h, root, "SUMMARY_BODY"), "{}", h.tree(root));

    // Back to HOME on the OUTER navigator: primary sidebar stays, the
    // whole secondary navigator leaves the outer outlet.
    outer.select(&HOME, ());
    h.flush();
    assert!(shows(&h, root, "PRIMARY_SIDEBAR"), "{}", h.tree(root));
    assert!(shows(&h, root, "HOME_BODY"), "{}", h.tree(root));
    assert!(!shows(&h, root, "SECONDARY_SIDEBAR"), "{}", h.tree(root));

    // Return: under LazyPersistent the nested navigator instance was
    // retained (detached), so it comes back on its OWN retained
    // selection — TRENDS, not the SUMMARY index.
    outer.select(&REPORTS, ());
    h.flush();
    assert!(
        shows(&h, root, "PRIMARY_SIDEBAR") && shows(&h, root, "SECONDARY_SIDEBAR"),
        "{}",
        h.tree(root)
    );
    assert!(
        shows(&h, root, "TRENDS_BODY") && !shows(&h, root, "SUMMARY_BODY"),
        "nested navigator selection not retained across an outer swap-away:\n{}",
        h.tree(root)
    );
    drop(realized);
}

#[test]
fn port_nested_stack_in_swap_renders_both_chromes_and_pushes_independently() {
    const ROOT: Route<()> = Route::new("root", "");
    const NDETAIL: Route<()> = Route::new("ndetail", "/detail");

    let h = Harness::new();
    let outer_slot: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
    let inner_slot: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));

    let element = {
        let outer_slot = outer_slot.clone();
        let inner_slot = inner_slot.clone();
        swap_navigator(&HOME)
            .screen(HOME, |_| view().child(text().content("HOME_BODY")).build())
            .screen(REPORTS, move |_| {
                let inner_slot = inner_slot.clone();
                stack_navigator(&ROOT)
                    .screen(ROOT, |_| view().child(text().content("ROOT_BODY")).build())
                    .screen(NDETAIL, |_| {
                        view().child(text().content("DETAIL_BODY")).build()
                    })
                    // The inner stack's own persistent chrome (a header).
                    .layout(|| {
                        view()
                            .child(text().content("STACK_CHROME"))
                            .child(navigator_outlet())
                            .build()
                    })
                    .retention(StackRetention::Retain)
                    .on_handle(move |handle| *inner_slot.borrow_mut() = Some(handle))
                    .build()
            })
            .layout(|| {
                view()
                    .child(text().content("PRIMARY_SIDEBAR"))
                    .child(navigator_outlet())
                    .build()
            })
            .on_handle(move |handle| *outer_slot.borrow_mut() = Some(handle))
            .build()
    };
    let realized = h.mount(element);
    let root = one_root(&realized);

    // Enter the section hosting the nested stack: primary sidebar +
    // stack chrome + the stack root, all at once.
    let outer = outer_slot.borrow().clone().expect("outer bound");
    outer.select(&REPORTS, ());
    h.flush();
    assert!(
        shows(&h, root, "PRIMARY_SIDEBAR") && shows(&h, root, "STACK_CHROME"),
        "outer sidebar and inner stack chrome must render together:\n{}",
        h.tree(root)
    );
    assert!(shows(&h, root, "ROOT_BODY"), "{}", h.tree(root));

    // Push on the INNER stack: both chromes stay, only the inner
    // outlet's top screen changes.
    let inner = inner_slot.borrow().clone().expect("inner stack bound");
    inner.push(&NDETAIL, ());
    h.flush();
    assert!(shows(&h, root, "PRIMARY_SIDEBAR"), "{}", h.tree(root));
    assert!(shows(&h, root, "STACK_CHROME"), "{}", h.tree(root));
    assert!(shows(&h, root, "DETAIL_BODY"), "{}", h.tree(root));
    assert!(!shows(&h, root, "ROOT_BODY"), "{}", h.tree(root));

    // Pop back — chromes intact, root revealed, detail released.
    inner.pop();
    h.flush();
    assert!(
        shows(&h, root, "PRIMARY_SIDEBAR") && shows(&h, root, "STACK_CHROME"),
        "{}",
        h.tree(root)
    );
    assert!(shows(&h, root, "ROOT_BODY"), "{}", h.tree(root));
    assert!(!shows(&h, root, "DETAIL_BODY"), "{}", h.tree(root));
    drop(realized);
}

// ===========================================================================
// Nested teardown (from nested_teardown_repro.rs): dropping the outer
// navigator releases every nested screen's scope — cleanups fire, no
// panic/abort at teardown.
// ===========================================================================

#[test]
fn port_nested_navigator_teardown_releases_inner_screens() {
    const INDEX: Route<()> = Route::new("index", "");
    const NDETAIL: Route<()> = Route::new("ndetail", "/detail");

    let h = Harness::new();
    let outer_slot: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
    let inner_slot: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
    let torn_idx = Rc::new(Cell::new(0u32));
    let torn_det = Rc::new(Cell::new(0u32));

    let element = {
        let outer_slot = outer_slot.clone();
        let inner_slot = inner_slot.clone();
        let torn_idx = torn_idx.clone();
        let torn_det = torn_det.clone();
        swap_navigator(&HOME)
            .screen(HOME, |_| view().child(text().content("HOME")).build())
            .screen(DOCS, move |_| {
                let inner_slot = inner_slot.clone();
                stack_navigator(&INDEX)
                    .screen(
                        INDEX,
                        probe_screen("IDX", Rc::new(Cell::new(0)), torn_idx.clone()),
                    )
                    .screen(
                        NDETAIL,
                        probe_screen("DET", Rc::new(Cell::new(0)), torn_det.clone()),
                    )
                    .retention(StackRetention::Retain)
                    .on_handle(move |handle| *inner_slot.borrow_mut() = Some(handle))
                    .build()
            })
            .layout(|| view().child(navigator_outlet()).build())
            .mount_policy(MountPolicy::LazyPersistent)
            .on_handle(move |handle| *outer_slot.borrow_mut() = Some(handle))
            .build()
    };
    let realized = h.mount(element);
    let root = one_root(&realized);

    let outer = outer_slot.borrow().clone().expect("outer bound");
    outer.select(&DOCS, ());
    h.flush();
    assert!(shows(&h, root, "IDX"), "{}", h.tree(root));

    let inner = inner_slot.borrow().clone().expect("inner bound");
    inner.push(&NDETAIL, ());
    h.flush();
    assert!(shows(&h, root, "DET"), "{}", h.tree(root));
    assert_eq!(torn_idx.get(), 0);
    assert_eq!(torn_det.get(), 0);

    // Dropping the OUTER navigator's Realized is the entire teardown:
    // the nested navigator (cached in the outer swap) and every screen
    // on its back stack release, cleanups fire, and nothing panics.
    h.world.enter(|| drop(realized));
    assert_eq!(torn_idx.get(), 1, "nested retained index screen released");
    assert_eq!(torn_det.get(), 1, "nested top screen released");
}

// ===========================================================================
// Deep links / URL-keyed selection (from swap_navigator_local.rs +
// navigator_url_sync.rs)
// ===========================================================================

#[test]
fn port_cold_start_deep_link_mounts_url_screen_and_caches_under_resolved_route() {
    let h = Harness::new();
    let handle_slot: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
    let builds_home = Rc::new(Cell::new(0u32));
    let builds_settings = Rc::new(Cell::new(0u32));

    set_initial_path(Some("/settings".to_string()));

    let element = {
        let handle_slot = handle_slot.clone();
        let bh = builds_home.clone();
        let bs = builds_settings.clone();
        swap_navigator(&HOME)
            .screen(HOME, probe_screen("HOME", bh, Rc::new(Cell::new(0))))
            .screen(SETTINGS, probe_screen("SETTINGS", bs, Rc::new(Cell::new(0))))
            .layout(|| view().child(navigator_outlet()).build())
            .mount_policy(MountPolicy::LazyPersistent)
            .on_handle(move |handle| *handle_slot.borrow_mut() = Some(handle))
            .build()
    };
    let realized = h.mount(element);
    let root = one_root(&realized);

    // The deep link's screen mounted, not the configured initial.
    assert!(shows(&h, root, "SETTINGS"), "{}", h.tree(root));
    assert!(!shows(&h, root, "HOME"), "{}", h.tree(root));
    assert_eq!(builds_home.get(), 0);
    assert_eq!(builds_settings.get(), 1);

    // Selecting the configured initial must actually navigate — the old
    // bug marked "home" active at mount, making this a no-op.
    let handle = handle_slot.borrow().clone().expect("bound");
    handle.select(&HOME, ());
    h.flush();
    assert!(shows(&h, root, "HOME"), "{}", h.tree(root));
    assert!(!shows(&h, root, "SETTINGS"), "{}", h.tree(root));

    // And back: the deep-linked screen was cached under its RESOLVED
    // url, so this re-shows the cached instance (no duplicate mount).
    handle.select(&SETTINGS, ());
    h.flush();
    assert!(shows(&h, root, "SETTINGS"), "{}", h.tree(root));
    assert_eq!(
        builds_settings.get(),
        1,
        "deep-linked screen cached under its own key — not rebuilt"
    );
    drop(realized);
}

#[derive(Clone)]
struct UserParams {
    id: String,
}
impl RouteParams for UserParams {
    fn to_path(&self, pattern: &str) -> String {
        pattern.replace(":id", &self.id)
    }
    fn from_segments(segs: &HashMap<String, String>) -> Option<Self> {
        segs.get("id").map(|id| UserParams { id: id.clone() })
    }
}

const USER: Route<UserParams> = Route::new("user", "/user/:id");

#[test]
fn port_deep_link_active_path_is_concrete_url_not_pattern() {
    let h = Harness::new();
    let ctx_slot: Rc<RefCell<Option<SwapNav>>> = Rc::new(RefCell::new(None));

    set_initial_path(Some("/user/123".to_string()));

    let element = {
        let ctx_slot = ctx_slot.clone();
        swap_navigator(&HOME)
            .screen(HOME, |_| view().child(text().content("HOME")).build())
            .screen(USER, |p: UserParams| {
                view()
                    .child(text().content(format!("USER {}", p.id)))
                    .build()
            })
            .layout(move || {
                *ctx_slot.borrow_mut() = inject::<SwapNav>();
                view().child(navigator_outlet()).build()
            })
            .build()
    };
    let realized = h.mount(element);
    let root = one_root(&realized);

    assert!(shows(&h, root, "USER 123"), "{}", h.tree(root));
    let ctx = ctx_slot.borrow_mut().take().expect("SwapNav captured");
    assert_eq!(h.world.enter(|| ctx.active_route.get()), "user");
    assert_eq!(
        h.world.enter(|| ctx.active_path.get()),
        "/user/123",
        "the mirror must hold the concrete launch URL, not the registered pattern"
    );
    drop(realized);
}

#[test]
fn port_cold_start_nested_deep_link_resolves_through_both_navigators() {
    const INDEX: Route<()> = Route::new("index", "");
    const NDETAIL: Route<()> = Route::new("ndetail", "/detail");

    let h = Harness::new();
    let stack_ctx: Rc<RefCell<Option<StackNav>>> = Rc::new(RefCell::new(None));

    set_initial_path(Some("/docs/detail".to_string()));

    let element = {
        let stack_ctx = stack_ctx.clone();
        swap_navigator(&HOME)
            .screen(HOME, |_| view().child(text().content("HOME")).build())
            .screen(DOCS, move |_| {
                let stack_ctx = stack_ctx.clone();
                stack_navigator(&INDEX)
                    .screen(INDEX, |_| view().child(text().content("DOCS_INDEX")).build())
                    .screen(NDETAIL, |_| {
                        view().child(text().content("NESTED_DETAIL")).build()
                    })
                    .retention(StackRetention::Retain)
                    .layout(move || {
                        *stack_ctx.borrow_mut() = inject::<StackNav>();
                        view().child(navigator_outlet()).build()
                    })
                    .build()
            })
            .layout(|| view().child(navigator_outlet()).build())
            .build()
    };
    let realized = h.mount(element);
    // publish_depth writes are staged; the boot flush commits them (same
    // contract the stack SDK newcore suite pins) — flush before reading ctx.
    h.flush();
    let root = one_root(&realized);

    // The nested navigator's screen mounted from the URL tail.
    assert!(
        shows(&h, root, "NESTED_DETAIL"),
        "cold deep link mounted the nested screen:\n{}",
        h.tree(root)
    );
    assert!(!shows(&h, root, "HOME"), "{}", h.tree(root));

    // The nested stack synthesized the configured index BELOW the
    // deep-linked screen so Back returns to it.
    let ctx = stack_ctx.borrow_mut().take().expect("nested StackNav captured");
    h.world.enter(|| {
        assert_eq!(ctx.depth.get(), 2, "index entry synthesized below the deep link");
        assert!(ctx.can_go_back.get());
        assert_eq!(ctx.active_route.get(), "ndetail");
        assert_eq!(ctx.active_path.get(), "/docs/detail");
    });

    // The ROOT navigator cleared the launch-path slot after its subtree
    // mounted (so later rebuilds aren't poisoned by a stale deep link).
    assert!(
        peek_initial_path().is_none(),
        "root navigator must clear the initial-path slot after mount"
    );

    // Popping the nested stack reveals the synthesized index.
    let pop = h.world.enter(|| ctx.pop.clone());
    pop();
    h.flush();
    assert!(shows(&h, root, "DOCS_INDEX"), "{}", h.tree(root));
    assert!(!shows(&h, root, "NESTED_DETAIL"), "{}", h.tree(root));
    drop(realized);
}

#[derive(Clone)]
struct EntryParams {
    name: String,
}
impl RouteParams for EntryParams {
    fn to_path(&self, pattern: &str) -> String {
        pattern.replace(":name", &self.name)
    }
    fn from_segments(segs: &HashMap<String, String>) -> Option<Self> {
        segs.get("name").map(|n| EntryParams { name: n.clone() })
    }
}

const ENTRY: Route<EntryParams> = Route::new("entry", "/entry/:name");

/// The docs-app catalog bug: the swap's cache and no-op guard are keyed
/// by URL, not route name — a different URL of the SAME parameterized
/// route must swap; re-selecting the active URL is structurally free.
#[test]
fn port_swap_parameterized_route_selects_by_url_not_name() {
    let h = Harness::new();
    let handle_slot: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
    let entry_builds = Rc::new(Cell::new(0u32));

    let element = {
        let handle_slot = handle_slot.clone();
        let entry_builds = entry_builds.clone();
        swap_navigator(&HOME)
            .screen(HOME, |_| view().child(text().content("OVERVIEW")).build())
            .screen(ENTRY, move |p: EntryParams| {
                entry_builds.set(entry_builds.get() + 1);
                view()
                    .child(text().content(format!("ENTRY {}", p.name)))
                    .build()
            })
            .layout(|| view().child(navigator_outlet()).build())
            .mount_policy(MountPolicy::LazyPersistent)
            .on_handle(move |handle| *handle_slot.borrow_mut() = Some(handle))
            .build()
    };
    let realized = h.mount(element);
    let root = one_root(&realized);
    let handle = handle_slot.borrow().clone().expect("bound");

    handle.select(&ENTRY, EntryParams { name: "button".into() });
    h.flush();
    assert!(shows(&h, root, "ENTRY button"), "{}", h.tree(root));
    assert_eq!(entry_builds.get(), 1);

    // Same route NAME, different URL — must swap (a name-keyed cache
    // served the stale screen; a name-keyed guard skipped entirely).
    handle.select(&ENTRY, EntryParams { name: "card".into() });
    h.flush();
    assert!(shows(&h, root, "ENTRY card"), "{}", h.tree(root));
    assert!(!shows(&h, root, "ENTRY button"), "{}", h.tree(root));
    assert_eq!(entry_builds.get(), 2, "different URL mounts a distinct screen");

    // Re-selecting the ACTIVE URL: a structural no-op (no creates, no
    // outlet churn, no rebuild).
    h.take_log();
    handle.select(&ENTRY, EntryParams { name: "card".into() });
    h.flush();
    let log = h.take_log();
    assert!(
        log.iter()
            .all(|l| !l.starts_with("create") && !l.starts_with("clear_children")),
        "re-selecting the active URL must be structurally inert: {log:?}"
    );
    assert_eq!(entry_builds.get(), 2);

    // And returning to a previously visited URL re-shows ITS cached
    // screen (URL-keyed cache, both entries retained).
    handle.select(&ENTRY, EntryParams { name: "button".into() });
    h.flush();
    assert!(shows(&h, root, "ENTRY button"), "{}", h.tree(root));
    assert_eq!(entry_builds.get(), 2, "cached URL entry not rebuilt");
    drop(realized);
}

/// A bare-name chrome selection of a route that REQUIRES path params
/// must be refused, not panic — the old `on_select` boxed `()` for every
/// route and aborted in the params downcast.
#[test]
fn port_on_select_with_params_route_is_refused_not_panic() {
    let h = Harness::new();
    let ctx_slot: Rc<RefCell<Option<SwapNav>>> = Rc::new(RefCell::new(None));

    let element = {
        let ctx_slot = ctx_slot.clone();
        swap_navigator(&HOME)
            .screen(HOME, |_| view().child(text().content("HOME")).build())
            .screen(USER, |p: UserParams| {
                view()
                    .child(text().content(format!("USER {}", p.id)))
                    .build()
            })
            .layout(move || {
                *ctx_slot.borrow_mut() = inject::<SwapNav>();
                view().child(navigator_outlet()).build()
            })
            .build()
    };
    let realized = h.mount(element);
    let root = one_root(&realized);
    let ctx = ctx_slot.borrow_mut().take().expect("SwapNav captured");
    h.take_log();

    // Must not panic, must not corrupt the mirrors, must not mount.
    (ctx.on_select)("user");
    h.flush();
    assert_eq!(
        h.world.enter(|| ctx.active_route.get()),
        "home",
        "refused selection leaves the active route untouched"
    );
    assert!(shows(&h, root, "HOME"), "{}", h.tree(root));
    let log = h.take_log();
    assert!(
        log.iter().all(|l| !l.starts_with("create")),
        "refused selection mounts nothing: {log:?}"
    );
    drop(realized);
}

// ===========================================================================
// Fill defaults (from swap_navigator_local.rs): the navigator root and a
// style-less outlet default to fill sizing; author styles win.
// ===========================================================================

type AppliedLog = Rc<RefCell<Vec<(host_mock::Node, StyleRules)>>>;

fn capture_styles(h: &Harness) -> AppliedLog {
    let applied: AppliedLog = Rc::new(RefCell::new(Vec::new()));
    let a = applied.clone();
    h.set_style_line(move |n, r| {
        a.borrow_mut().push((n, r.clone()));
        format!("apply_style n{n}")
    });
    applied
}

fn is_nav_fill(r: &StyleRules) -> bool {
    r.width == Some(Length::Percent(100.0).into())
        && r.height == Some(Length::Percent(100.0).into())
        && r.flex_grow == Some(1.0.into())
}

fn is_outlet_fill(r: &StyleRules) -> bool {
    r.flex_grow == Some(1.0.into())
        && r.flex_basis == Some(Length::Px(0.0).into())
        && r.min_height == Some(Length::Px(0.0).into())
}

#[test]
fn port_navigator_root_fills_container_by_default_and_author_style_wins() {
    // Default: the root gets fill-the-container sizing (a bare root hugs
    // content and collapses a viewport-height app).
    {
        let h = Harness::new();
        let applied = capture_styles(&h);
        let realized = h.mount(
            swap_navigator(&HOME)
                .screen(HOME, |_| view().child(text().content("HOME")).build())
                .layout(|| view().child(navigator_outlet()).build())
                .build(),
        );
        let root = one_root(&realized);
        assert!(
            applied
                .borrow()
                .iter()
                .any(|(n, r)| *n == root && is_nav_fill(r)),
            "the navigator root defaults to fill-the-container sizing"
        );
        drop(realized);
    }

    // Author `.style(...)` on the navigator applies AFTER the default,
    // onto the same root node — the author's sizing wins.
    {
        let h = Harness::new();
        let applied = capture_styles(&h);
        let realized = h.mount(
            swap_navigator(&HOME)
                .screen(HOME, |_| view().child(text().content("HOME")).build())
                .layout(|| view().child(navigator_outlet()).build())
                .style(StyleRules {
                    width: Some(Tokenized::Literal(Length::Px(321.0))),
                    height: Some(Tokenized::Literal(Length::Px(123.0))),
                    ..Default::default()
                })
                .build(),
        );
        let root = one_root(&realized);
        let last_root_style = applied
            .borrow()
            .iter()
            .filter(|(n, _)| *n == root)
            .map(|(_, r)| r.clone())
            .last()
            .expect("root styled");
        assert_eq!(
            last_root_style.width,
            Some(Tokenized::Literal(Length::Px(321.0))),
            "author style lands last on the navigator root"
        );
        assert_eq!(
            last_root_style.height,
            Some(Tokenized::Literal(Length::Px(123.0)))
        );
        drop(realized);
    }
}

#[test]
fn port_outlet_fills_by_default_and_author_style_replaces_it() {
    // A style-less `navigator_outlet()` defaults to a bounded, fillable
    // flex region (screens assume `flex: 1` + a bounded height).
    {
        let h = Harness::new();
        let applied = capture_styles(&h);
        let realized = h.mount(
            swap_navigator(&HOME)
                .screen(HOME, |_| view().child(text().content("HOME")).build())
                .layout(|| {
                    view()
                        .child(navigator_outlet())
                        .child(text().content("BAR"))
                        .build()
                })
                .build(),
        );
        assert!(
            applied.borrow().iter().any(|(_, r)| is_outlet_fill(r)),
            "a style-less outlet defaults to a bounded, fillable flex region"
        );
        drop(realized);
    }

    // An author outlet style REPLACES the fill default (opting out is a
    // real control, not a merge).
    {
        let h = Harness::new();
        let applied = capture_styles(&h);
        let realized = h.mount(
            swap_navigator(&HOME)
                .screen(HOME, |_| view().child(text().content("HOME")).build())
                .layout(|| {
                    view()
                        .child(navigator_outlet().style(StyleRules {
                            height: Some(Tokenized::Literal(Length::Px(222.0))),
                            ..Default::default()
                        }))
                        .child(text().content("BAR"))
                        .build()
                })
                .build(),
        );
        let a = applied.borrow();
        assert!(
            a.iter()
                .any(|(_, r)| r.height == Some(Tokenized::Literal(Length::Px(222.0)))),
            "author outlet style landed"
        );
        assert!(
            a.iter().all(|(_, r)| !is_outlet_fill(r)),
            "author outlet style replaced the fill default"
        );
        drop(a);
        drop(realized);
    }
}

// ===========================================================================
// Author-layout reactive scope (from swap_navigator_local.rs): an effect
// created directly in the layout closure is legal, owned by the
// navigator, re-fires on navigation, and dies silently at teardown.
// ===========================================================================

#[test]
fn port_author_layout_effect_has_reactive_scope_and_dies_with_navigator() {
    let h = Harness::new();
    let handle_slot: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
    let seen: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));

    let element = {
        let handle_slot = handle_slot.clone();
        let seen = seen.clone();
        swap_navigator(&HOME)
            .screen(HOME, |_| view().child(text().content("HOME")).build())
            .screen(SETTINGS, |_| view().child(text().content("SETTINGS")).build())
            .layout(move || {
                // The website's drawer auto-close shape: subscribe to
                // every navigation from author chrome. This panicked at
                // mount when the layout built outside a reactive scope.
                let ctx = inject::<SwapNav>().expect("SwapNav ambient in layout");
                let active_route = ctx.active_route;
                let seen = seen.clone();
                effect(move || {
                    seen.borrow_mut().push(active_route.get());
                });
                view()
                    .child(navigator_outlet())
                    .child(text().content("BAR"))
                    .build()
            })
            .on_handle(move |handle| *handle_slot.borrow_mut() = Some(handle))
            .build()
    };
    let realized = h.mount(element);
    h.flush();
    assert_eq!(
        seen.borrow().as_slice(),
        &["home"],
        "chrome effect ran at mount with the initial route"
    );

    let handle = handle_slot.borrow().clone().expect("bound");
    handle.select(&SETTINGS, ());
    h.flush();
    assert_eq!(
        seen.borrow().as_slice(),
        &["home", "settings"],
        "chrome effect re-ran on Select"
    );

    // Teardown: the effect is owned by the navigator — unmounting frees
    // it cleanly without a final fire.
    h.world.enter(|| drop(realized));
    h.flush();
    assert_eq!(seen.borrow().len(), 2, "effect did not fire during teardown");
}

// ===========================================================================
// LazyDisposing cleanup navigation (from swap_navigator_local.rs): an
// author `on_cleanup`/`on_teardown` in a disposed screen that NAVIGATES
// must not abort. NOTE the committed order differs from the old core by
// design: the old walker ran the re-entrant redirect synchronously
// inside the outer selection (outer landed last); the new core stages
// commands in dispatch order, so the redirect staged DURING the outer
// selection's disposal commits after it — the redirect wins. The pinned
// invariant is no-abort + a coherent final state (visible screen ==
// route mirror).
// ===========================================================================

#[test]
fn port_lazy_disposing_cleanup_navigation_commits_without_panic() {
    let h = Harness::new();
    let handle_slot: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
    let ctx_slot: Rc<RefCell<Option<SwapNav>>> = Rc::new(RefCell::new(None));

    let element = {
        let handle_slot = handle_slot.clone();
        let ctx_slot = ctx_slot.clone();
        let handle_for_cleanup = handle_slot.clone();
        swap_navigator(&HOME)
            .screen(HOME, move |_| {
                // Route-guard shape: navigating away from Home redirects
                // to Profile from the screen's teardown.
                let handle_for_cleanup = handle_for_cleanup.clone();
                on_teardown(move || {
                    if let Some(handle) = handle_for_cleanup.borrow().clone() {
                        handle.select(&PROFILE, ());
                    }
                });
                view().child(text().content("HOME")).build()
            })
            .screen(SETTINGS, |_| view().child(text().content("SETTINGS")).build())
            .screen(PROFILE, |_| view().child(text().content("PROFILE")).build())
            .layout(move || {
                *ctx_slot.borrow_mut() = inject::<SwapNav>();
                view().child(navigator_outlet()).build()
            })
            .mount_policy(MountPolicy::LazyDisposing)
            .on_handle(move |handle| *handle_slot.borrow_mut() = Some(handle))
            .build()
    };
    let realized = h.mount(element);
    let root = one_root(&realized);
    assert!(shows(&h, root, "HOME"), "{}", h.tree(root));

    // Switching away disposes Home; its cleanup re-enters the navigator.
    // Must not abort ("RefCell already borrowed" was the old failure),
    // and must end coherent: the visible screen matches the mirror.
    let handle = handle_slot.borrow().clone().expect("bound");
    handle.select(&SETTINGS, ());
    h.flush();

    let ctx = ctx_slot.borrow_mut().take().expect("SwapNav captured");
    let active = h.world.enter(|| ctx.active_route.get());
    assert_eq!(
        active, "profile",
        "the cleanup redirect (staged during disposal) commits after the \
         outer selection on the new core's dispatch-ordered queue"
    );
    assert!(shows(&h, root, "PROFILE"), "{}", h.tree(root));
    assert!(!shows(&h, root, "HOME"), "{}", h.tree(root));
    assert!(!shows(&h, root, "SETTINGS"), "{}", h.tree(root));
    drop(realized);
}

// ===========================================================================
// Theme cohort × navigator chrome (from theme_chrome_cohort.rs)
// ===========================================================================

fn chrome_sheet() -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(StyleRules {
        width: Some(Tokenized::Literal(Length::Px(111.0))),
        background: Some(Tokenized::token("tok-chrome-bg", Color("#cccccc".into()))),
        ..Default::default()
    }))
}

fn screen_sheet() -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(StyleRules {
        width: Some(Tokenized::Literal(Length::Px(222.0))),
        background: Some(Tokenized::token("tok-screen-bg", Color("#dddddd".into()))),
        ..Default::default()
    }))
}

fn tokens(chrome: &str, screen: &str) -> [TokenEntry; 2] {
    [
        TokenEntry {
            name: "tok-chrome-bg",
            value: TokenValue::Color(Color(chrome.into())),
        },
        TokenEntry {
            name: "tok-screen-bg",
            value: TokenValue::Color(Color(screen.into())),
        },
    ]
}

/// "The sidebar doesn't re-theme": a static token-driven sheet on a node
/// built inside the navigator's author layout (the retained chrome
/// scope) must re-apply on a theme swap exactly like a screen node.
#[test]
fn port_chrome_static_token_style_reapplies_on_theme_swap() {
    let h = Harness::new();
    let chrome = chrome_sheet();
    let screen = screen_sheet();

    h.world.enter(|| theme::install_tokens(&tokens("#ffffff", "#fefefe")));
    let realized = h.mount(
        swap_navigator(&HOME)
            .screen(HOME, move |_| {
                view()
                    .style(StyleApplication::new(screen.clone()))
                    .child(text().content("CONTENT"))
                    .build()
            })
            .layout(move || {
                view()
                    .child(
                        view()
                            .style(StyleApplication::new(chrome.clone()))
                            .child(text().content("SIDEBAR"))
                            .build(),
                    )
                    .child(navigator_outlet())
                    .build()
            })
            .build(),
    );
    h.take_log();

    // Theme swap: BOTH the screen node and the chrome node re-apply.
    h.world.enter(|| theme::update_tokens(&tokens("#111111", "#222222")));
    h.flush();
    let log = h.take_log().join("\n");
    assert!(
        log.contains("width=Literal(Px(222.0))"),
        "screen node re-applied on theme swap (control):\n{log}"
    );
    assert!(
        log.contains("width=Literal(Px(111.0))"),
        "CHROME node re-applied on theme swap — the sidebar-stays-light bug:\n{log}"
    );
    drop(realized);
}

/// The real repro shape: the cohort must survive screen DISPOSAL. Under
/// `LazyDisposing`, navigating drops the first screen's scope; a theme
/// swap afterwards must still re-apply the (still-mounted) chrome.
#[test]
fn port_chrome_retints_after_lazy_disposing_navigation() {
    const OTHER: Route<()> = Route::new("other", "/other");

    let h = Harness::new();
    let handle_slot: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
    let chrome = chrome_sheet();
    let screen_a = screen_sheet();
    let screen_b = screen_sheet();

    h.world.enter(|| theme::install_tokens(&tokens("#ffffff", "#fefefe")));
    let realized = h.mount({
        let handle_slot = handle_slot.clone();
        swap_navigator(&HOME)
            .screen(HOME, move |_| {
                view()
                    .style(StyleApplication::new(screen_a.clone()))
                    .child(text().content("HOME"))
                    .build()
            })
            .screen(OTHER, move |_| {
                view()
                    .style(StyleApplication::new(screen_b.clone()))
                    .child(text().content("OTHER"))
                    .build()
            })
            .mount_policy(MountPolicy::LazyDisposing)
            .layout(move || {
                view()
                    .child(
                        view()
                            .style(StyleApplication::new(chrome.clone()))
                            .child(text().content("SIDEBAR"))
                            .build(),
                    )
                    .child(navigator_outlet())
                    .build()
            })
            .on_handle(move |handle| *handle_slot.borrow_mut() = Some(handle))
            .build()
    });

    // Navigate: disposes HOME's scope (and its cohort membership). The
    // old bug: the cohort driver was owned by the first screen's scope,
    // so this navigation wiped the chrome's cohort entry too.
    let handle = handle_slot.borrow().clone().expect("bound");
    handle.select(&OTHER, ());
    h.flush();
    h.take_log();

    // Theme swap AFTER the navigation: the chrome must still re-apply.
    h.world.enter(|| theme::update_tokens(&tokens("#111111", "#222222")));
    h.flush();
    let log = h.take_log().join("\n");
    assert!(
        log.contains("width=Literal(Px(111.0))"),
        "chrome node must RE-APPLY on a theme swap after a LazyDisposing \
         navigation — the sidebar-stays-on-old-theme bug:\n{log}"
    );
    assert!(
        log.contains("width=Literal(Px(222.0))"),
        "the live screen re-applied too (control):\n{log}"
    );
    drop(realized);
}

/// N reactive-styled nodes SHARING one sheet (one resolution key) must
/// ALL keep re-applying across REPEATED theme swaps — the old bug had
/// only the first (cache-miss) resolver subscribed to token signals, so
/// each successive toggle repainted fewer nodes.
#[test]
fn port_shared_sheet_nodes_all_reapply_on_repeated_theme_swaps() {
    let h = Harness::new();
    let shared: Rc<StyleSheet> = Rc::new(StyleSheet::r#static(StyleRules {
        width: Some(Tokenized::Literal(Length::Px(100.0))),
        background: Some(Tokenized::token("tok-shared-bg", Color("#000000".into()))),
        ..Default::default()
    }));

    h.world.enter(|| {
        theme::install_tokens(&[TokenEntry {
            name: "tok-shared-bg",
            value: TokenValue::Color(Color("#ffffff".into())),
        }])
    });

    let (sheet_a, sheet_b) = (shared.clone(), shared.clone());
    let realized = h.mount(
        view()
            // REACTIVE style closures (the ui! shape) so each node owns
            // its own resolution effect over the SAME sheet.
            .child(
                view()
                    .style(move || StyleApplication::new(sheet_a.clone()))
                    .child(text().content("A"))
                    .build(),
            )
            .child(
                view()
                    .style(move || StyleApplication::new(sheet_b.clone()))
                    .child(text().content("B"))
                    .build(),
            )
            .build(),
    );

    // Identify the two styled nodes from the mount log.
    let mount_log = h.take_log();
    let styled_nodes: Vec<String> = mount_log
        .iter()
        .filter(|l| l.starts_with("apply_style") && l.contains("width=Literal(Px(100.0))"))
        .map(|l| l.split_whitespace().nth(1).unwrap().to_string())
        .collect();
    assert_eq!(styled_nodes.len(), 2, "two shared-sheet nodes styled at mount: {mount_log:?}");

    // Two consecutive swaps; BOTH nodes must re-apply on EACH.
    for (round, color) in [("first", "#111111"), ("second", "#eeeeee")] {
        h.world.enter(|| {
            theme::update_tokens(&[TokenEntry {
                name: "tok-shared-bg",
                value: TokenValue::Color(Color(color.to_string())),
            }])
        });
        h.flush();
        let log = h.take_log();
        for node in &styled_nodes {
            assert!(
                log.iter()
                    .any(|l| l.starts_with(&format!("apply_style {node} ")))
                    || log.iter().any(|l| l.starts_with(&format!("apply_style {node}"))),
                "node {node} must re-apply on the {round} swap — the cache-hit \
                 subscriber-collapse bug: {log:?}"
            );
        }
    }
    drop(realized);
}

// ===========================================================================
// URL-sync seam (from navigator_url_sync.rs): the vocabulary's half of
// the platform-URL contract. The swap registration leg is pinned in the
// SDK suite (swap/tests/newcore.rs); these cover the stack, the
// suppress bit, and the nested base partition.
// ===========================================================================

#[derive(Default)]
struct Probe {
    /// (id, kind, base, active_path, depth) per registration.
    regs: RefCell<Vec<(u64, NavSyncKind, String, String, usize)>>,
    resolvers: RefCell<HashMap<u64, Rc<dyn Fn(&str) -> Option<(&'static str, Box<dyn Any>, String)>>>>,
    dispatches: RefCell<HashMap<u64, Rc<dyn Fn(NavCommand)>>>,
    outlets: RefCell<HashMap<u64, Rc<dyn Any>>>,
    before: RefCell<Vec<(u64, String)>>,
    after: RefCell<Vec<(u64, CommittedKind)>>,
    dereg: RefCell<Vec<u64>>,
    next: Cell<u64>,
    /// When set, `before_command` claims every dispatch as the service's
    /// own echo (the popstate-reconciliation suppress bit).
    suppress_all: Cell<bool>,
}

fn cmd_digest(cmd: &NavCommand) -> String {
    match cmd {
        NavCommand::Push { url, .. } => format!("push:{url}"),
        NavCommand::Select { url, .. } => format!("select:{url}"),
        NavCommand::Replace { url, .. } => format!("replace:{url}"),
        NavCommand::Reset { url, .. } => format!("reset:{url}"),
        NavCommand::Pop => "pop".to_string(),
        NavCommand::Custom(_) => "custom".to_string(),
    }
}

struct ProbeService(Rc<Probe>);

impl UrlSyncService for ProbeService {
    fn register(&self, reg: NavSyncRegistration) -> Option<u64> {
        let id = self.0.next.get() + 1;
        self.0.next.set(id);
        self.0
            .regs
            .borrow_mut()
            .push((id, reg.kind, reg.base.clone(), reg.active_path.clone(), reg.depth));
        self.0.resolvers.borrow_mut().insert(id, reg.resolve_entry);
        self.0.dispatches.borrow_mut().insert(id, reg.dispatch);
        self.0.outlets.borrow_mut().insert(id, reg.outlet);
        Some(id)
    }
    fn before_command(&self, id: u64, cmd: &NavCommand) -> bool {
        self.0.before.borrow_mut().push((id, cmd_digest(cmd)));
        self.0.suppress_all.get()
    }
    fn after_commit(&self, id: u64, kind: CommittedKind) {
        self.0.after.borrow_mut().push((id, kind));
    }
    fn deregister(&self, id: u64) {
        self.0.dereg.borrow_mut().push(id);
    }
}

#[test]
fn port_stack_url_sync_registers_hooks_push_pop_and_hands_the_real_outlet() {
    let h = Harness::new();
    let probe: Rc<Probe> = Rc::default();
    install_url_sync_service(Rc::new(ProbeService(probe.clone())));

    let handle_slot: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
    let realized = h.mount({
        let handle_slot = handle_slot.clone();
        stack_navigator(&HOME)
            .screen(HOME, |_| view().child(text().content("HOME")).build())
            .screen(DETAIL, |_| view().child(text().content("DETAIL")).build())
            .retention(StackRetention::Retain)
            .layout(|| view().child(navigator_outlet()).build())
            .on_handle(move |handle| *handle_slot.borrow_mut() = Some(handle))
            .build()
    });

    // Registration carries the stack's committed state.
    let (id, kind, base, active, depth) = probe.regs.borrow()[0].clone();
    assert_eq!(kind, NavSyncKind::Stack);
    assert_eq!(base, "");
    assert_eq!(active, "/");
    assert_eq!(depth, 1);

    // The registered outlet is the REAL swapped node — the handle the
    // service uses for scroll snapshot/restore (the old
    // `stack_scroll_snapshot_restores_on_browser_back` seam).
    let outlet = *probe.outlets.borrow()[&id]
        .downcast_ref::<host_mock::Node>()
        .expect("outlet downcasts to the backend node");
    h.take_log();

    // Push: dispatch-time hook sees the command BEFORE commit; the
    // commit swaps the registered outlet's child; the driver then fires
    // the Forward after-hook.
    let handle = handle_slot.borrow().clone().expect("bound");
    handle.push(&DETAIL, ());
    h.flush();
    assert_eq!(probe.before.borrow().as_slice(), [(id, "push:/detail".to_string())]);
    assert_eq!(probe.after.borrow().as_slice(), [(id, CommittedKind::Forward)]);
    let log = h.take_log();
    assert!(
        log.iter().any(|l| l == &format!("clear_children n{outlet}")),
        "the screen swap targets the registered outlet node: {log:?}"
    );

    // Pop: classified Pop at commit.
    handle.pop();
    h.flush();
    assert_eq!(probe.before.borrow().last().unwrap(), &(id, "pop".to_string()));
    assert_eq!(probe.after.borrow().last().unwrap(), &(id, CommittedKind::Pop));

    // Teardown deregisters.
    h.world.enter(|| drop(realized));
    assert_eq!(probe.dereg.borrow().as_slice(), [id]);
    clear_url_sync_service();
}

/// The echo-swallow mechanism (old `regression_programmatic_pop_swallows
/// _its_popstate_echo`): when `before_command` claims a dispatch as the
/// service's own popstate echo (returns `true`), the command still
/// COMMITS structurally but `after_commit` is skipped — so the service
/// never double-writes history for its own reconciliation.
#[test]
fn port_url_sync_suppress_bit_skips_after_commit() {
    let h = Harness::new();
    let probe: Rc<Probe> = Rc::default();
    probe.suppress_all.set(true);
    install_url_sync_service(Rc::new(ProbeService(probe.clone())));

    let handle_slot: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
    let realized = h.mount({
        let handle_slot = handle_slot.clone();
        swap_navigator(&HOME)
            .screen(HOME, |_| view().child(text().content("HOME")).build())
            .screen(SETTINGS, |_| view().child(text().content("SETTINGS")).build())
            .layout(|| view().child(navigator_outlet()).build())
            .on_handle(move |handle| *handle_slot.borrow_mut() = Some(handle))
            .build()
    });
    let root = one_root(&realized);

    let handle = handle_slot.borrow().clone().expect("bound");
    handle.select(&SETTINGS, ());
    h.flush();

    // The command committed (the screen swapped)…
    assert!(shows(&h, root, "SETTINGS"), "{}", h.tree(root));
    assert!(!shows(&h, root, "HOME"), "{}", h.tree(root));
    assert_eq!(probe.before.borrow().len(), 1, "dispatch hook fired");
    // …but the after-hook was suppressed: this reconciliation is the
    // service's own echo and must not produce another history write.
    assert!(
        probe.after.borrow().is_empty(),
        "after_commit must be skipped for a suppressed (echo) dispatch: {:?}",
        probe.after.borrow()
    );
    drop(realized);
    clear_url_sync_service();
}

/// The owned-slice partition (old `regression_nested_stack_popstate_
/// does_not_remount_parent_swap_screen` seam legs): each navigator
/// registers with its OWN base slice; the parent's resolver yields the
/// unconsumed remainder for the nested navigator; the nested
/// registration's dispatch composes its base onto relative command URLs.
#[test]
fn port_nested_navigator_url_sync_registers_composed_base_and_resolver() {
    const INDEX: Route<()> = Route::new("index", "");
    const NDETAIL: Route<()> = Route::new("ndetail", "/detail");

    let h = Harness::new();
    let probe: Rc<Probe> = Rc::default();
    install_url_sync_service(Rc::new(ProbeService(probe.clone())));

    let handle_slot: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
    let realized = h.mount({
        let handle_slot = handle_slot.clone();
        swap_navigator(&HOME)
            .screen(HOME, |_| view().child(text().content("HOME")).build())
            .screen(DOCS, move |_| {
                stack_navigator(&INDEX)
                    .screen(INDEX, |_| view().child(text().content("DOCS_INDEX")).build())
                    .screen(NDETAIL, |_| {
                        view().child(text().content("NESTED_DETAIL")).build()
                    })
                    .retention(StackRetention::Retain)
                    .build()
            })
            .layout(|| view().child(navigator_outlet()).build())
            .on_handle(move |handle| *handle_slot.borrow_mut() = Some(handle))
            .build()
    });
    let root = one_root(&realized);

    // Only the ROOT navigator registered at mount.
    assert_eq!(probe.regs.borrow().len(), 1);
    let (outer_id, outer_kind, outer_base, _, _) = probe.regs.borrow()[0].clone();
    assert_eq!(outer_kind, NavSyncKind::Swap);
    assert_eq!(outer_base, "");

    // Entering /docs mounts the nested stack, which registers with its
    // COMPOSED base — the slice partition the popstate router keys on.
    let handle = handle_slot.borrow().clone().expect("bound");
    handle.select(&DOCS, ());
    h.flush();
    assert_eq!(probe.regs.borrow().len(), 2, "nested navigator registered on mount");
    let (inner_id, inner_kind, inner_base, inner_active, inner_depth) =
        probe.regs.borrow()[1].clone();
    assert_eq!(inner_kind, NavSyncKind::Stack);
    assert_eq!(inner_base, "/docs", "nested base composes the parent's slice");
    assert_eq!(inner_active, "/docs");
    assert_eq!(inner_depth, 1);

    // The parent's resolver consumes its slice and yields the remainder
    // for the nested navigator; the nested resolver consumes the tail.
    let outer_resolve = probe.resolvers.borrow()[&outer_id].clone();
    let (name, _, rem) = outer_resolve("/docs/detail").expect("outer resolves its slice");
    assert_eq!(name, "docs");
    assert_eq!(rem, "/detail", "unconsumed remainder belongs to the nested slice");
    let inner_resolve = probe.resolvers.borrow()[&inner_id].clone();
    let (name, _, rem) = inner_resolve("/docs/detail").expect("nested resolves the tail");
    assert_eq!(name, "ndetail");
    assert_eq!(rem, "");

    // Dispatching through the NESTED registration composes its base onto
    // the navigator-relative URL — the service's history writes always
    // see full paths.
    let inner_dispatch = probe.dispatches.borrow()[&inner_id].clone();
    inner_dispatch(NavCommand::Push {
        name: "ndetail",
        url: "/detail".to_string(),
        params: Box::new(()),
        state: None,
    });
    h.flush();
    assert!(
        probe
            .before
            .borrow()
            .iter()
            .any(|(id, d)| *id == inner_id && d == "push:/docs/detail"),
        "nested dispatch must base-compose its command URL: {:?}",
        probe.before.borrow()
    );
    assert!(shows(&h, root, "NESTED_DETAIL"), "{}", h.tree(root));

    // Teardown deregisters BOTH navigators.
    h.world.enter(|| drop(realized));
    let mut dereg = probe.dereg.borrow().clone();
    dereg.sort_unstable();
    assert_eq!(dereg, vec![outer_id, inner_id]);
    clear_url_sync_service();
}
