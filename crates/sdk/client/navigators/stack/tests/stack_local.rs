//! Local-mount coverage for the outlet-model stack: retention semantics
//! and cold deep links, driven through the SDK's authored surface on the
//! `host-mock` recording substrate.
//!
//! `tests/navigator.rs` covers the happy path (mount, push/pop, chrome
//! republish, `link` → push). THIS file is the retention + deep-link
//! half:
//!
//! - `Rebuild` (the web default) disposes the covered screen on push and
//!   re-mounts it on pop;
//! - `Retain` (the native default) keeps it alive, so pop reveals the
//!   same instance with no rebuild;
//! - a cold deep link seats the configured initial BELOW the resolved
//!   screen so Back can return to it — and under `Rebuild` that
//!   synthesized parent is URL-only: it must NOT build (no effects, no
//!   fetches) until the user actually pops to it;
//! - popping the root is a no-op.
//!
//! Screen builds/disposals are counted inside the screen closure, which
//! is why this cannot be folded into an op-log assertion: the point is
//! that the closure never RAN, not that no nodes appeared. Disposal is
//! observed through an effect the screen body creates — the effect is
//! collected into the screen's scope, so its cleanup fires exactly when
//! that scope drops.

#![cfg(not(target_arch = "wasm32"))]

use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;

use host_mock::Harness;
use runtime_shared::primitives::navigator::set_initial_path;
use runtime_vocabulary::builders::{text, view};
use runtime_vocabulary::glue::{IntoElement, Ref};
use stack_navigator::{
    Route, Screen, StackBuilder, StackContext, StackHandle, StackNavigator, StackRetention,
};

const HOME: Route<()> = Route::<()>::new("home", "/");
const DETAIL: Route<()> = Route::<()>::new("detail", "/detail");

/// Per-screen lifecycle counters: how many times the screen body was
/// built, and how many times a built instance was disposed.
#[derive(Clone, Default)]
struct Counters {
    builds: Rc<Cell<u32>>,
    disposals: Rc<Cell<u32>>,
}

struct App {
    nav: Ref<StackHandle>,
    /// The context the author layout received, kept so the test can read
    /// `depth` / `can_go_back` / `active_route` the way author chrome
    /// would.
    ctx: Rc<RefCell<Option<StackContext>>>,
    _realized: runtime_scene::Realized<u32>,
}

/// Mount a HOME/DETAIL stack whose HOME screen reports into `c`.
fn mount_counted(h: &Harness, c: Counters, retention: StackRetention) -> App {
    let nav: Ref<StackHandle> = Ref::new();
    let ctx: Rc<RefCell<Option<StackContext>>> = Rc::new(RefCell::new(None));
    let element = {
        let ctx = ctx.clone();
        StackNavigator::new(&HOME)
            .screen(HOME, move |_| {
                c.builds.set(c.builds.get() + 1);
                // Disposal is observed through an effect the screen body
                // creates: the effect is collected into the screen's own
                // scope, so its cleanup fires exactly when that scope
                // drops. (`on_cleanup` at build time would panic — it
                // registers against the innermost RUNNING effect, and a
                // screen body is not one.)
                let disposals = c.disposals.clone();
                runtime_world::effect(move || {
                    let disposals = disposals.clone();
                    runtime_world::on_cleanup(move || disposals.set(disposals.get() + 1));
                });
                Screen::new(view().child(text().content("HOME SCREEN")).build())
            })
            .screen(DETAIL, |_| {
                Screen::new(view().child(text().content("DETAIL SCREEN")).build())
            })
            .layout(move |nav_ctx: StackContext| {
                let StackContext {
                    outlet,
                    active_route,
                    active_path,
                    depth,
                    can_go_back,
                    pop,
                    screen_chrome,
                } = nav_ctx;
                // Keep the signal half; splat the one-shot outlet.
                *ctx.borrow_mut() = Some(StackContext {
                    outlet: view().build(), // placeholder; never splatted
                    active_route,
                    active_path,
                    depth,
                    can_go_back,
                    pop,
                    screen_chrome,
                });
                view().child(outlet).build()
            })
            .retention(retention)
            .bind(nav)
            .into_element()
    };
    let realized = h.mount(element);
    h.world.flush();
    App {
        nav,
        ctx,
        _realized: realized,
    }
}

/// `(active_route, active_path, depth, can_go_back)` — the snapshot the
/// old-core `NavControl::nav_state_snapshot` served, read here off the
/// signals the author chrome reads.
fn nav_state(h: &Harness, app: &App) -> (&'static str, String, usize, bool) {
    let ctx = app.ctx.borrow();
    let ctx = ctx.as_ref().expect("author layout ran");
    h.world.enter(|| {
        (
            ctx.active_route.get(),
            ctx.active_path.get(),
            ctx.depth.get(),
            ctx.can_go_back.get(),
        )
    })
}

fn shows(h: &Harness, needle: &str) -> bool {
    h.ops().iter().any(|op| op.contains(needle))
}

/// Browser semantics (`Rebuild`, the web default): pushing over a screen
/// DISPOSES it — nothing below the visible screen stays resident — and
/// pop re-mounts the revealed screen from its URL like a fresh
/// navigation.
#[test]
fn regression_rebuild_disposes_covered_screen_and_pop_remounts() {
    let h = Harness::new();
    let c = Counters::default();
    let app = mount_counted(&h, c.clone(), StackRetention::Rebuild);
    assert_eq!(c.builds.get(), 1, "index mounted once");

    // Push covers the index — its scope must be dropped, not parked.
    h.take_log();
    app.nav.get().expect("handle filled").push(&DETAIL, ());
    h.world.flush();
    assert_eq!(
        c.disposals.get(),
        1,
        "covered screen disposed on push (browser semantics)"
    );
    assert!(shows(&h, "DETAIL SCREEN"), "pushed screen is the top");

    // Pop re-mounts it fresh from its URL.
    h.take_log();
    app.nav.get().unwrap().pop();
    h.world.flush();
    assert_eq!(c.builds.get(), 2, "pop re-mounts the revealed screen fresh");
    assert!(shows(&h, "HOME SCREEN"), "revealed screen rebuilt");
}

/// Native semantics (`Retain`, the non-web default): covered screens stay
/// alive and pop reveals the SAME instance, no rebuild.
#[test]
fn retain_keeps_covered_screen_alive_across_push_pop() {
    let h = Harness::new();
    let c = Counters::default();
    let app = mount_counted(&h, c.clone(), StackRetention::Retain);

    app.nav.get().expect("handle filled").push(&DETAIL, ());
    h.world.flush();
    assert_eq!(
        c.disposals.get(),
        0,
        "covered screen stays alive (native semantics)"
    );

    app.nav.get().unwrap().pop();
    h.world.flush();
    assert_eq!(c.builds.get(), 1, "pop reveals the retained instance, no rebuild");
}

/// Cold-start deep link: the launch path resolves to its own screen, and
/// the configured initial is reconstructed BENEATH it so Back can return
/// to the index. The old handler seated the attached screen as
/// `route: initial_route / path: initial_path` at depth 1 — the entry was
/// mislabeled, `can_go_back` stayed false, and Back could never return.
#[test]
fn regression_cold_deep_link_reconstructs_back_stack() {
    let h = Harness::new();
    set_initial_path(Some("/detail".to_string()));
    let c = Counters::default();
    let app = mount_counted(&h, c.clone(), StackRetention::Retain);

    assert!(shows(&h, "DETAIL SCREEN"), "deep link resolved its own screen");
    // Under `Retain` the synthesized parent is materialized at seat time
    // (it is what pop reveals, with state intact) — the `Rebuild` test
    // below is the one that pins the never-mount-until-pop contract.
    assert_eq!(c.builds.get(), 1, "the index below was seated");
    let (route, path, depth, can_go_back) = nav_state(&h, &app);
    assert_eq!(route, "detail");
    assert_eq!(path, "/detail");
    assert_eq!(
        depth, 2,
        "configured initial reconstructed beneath the resolved screen"
    );
    assert!(can_go_back, "back is possible after a cold deep link");

    // Back returns to the index — the whole point of the reconstruction.
    h.take_log();
    app.nav.get().expect("handle filled").pop();
    h.world.flush();
    // The outlet swapped to the screen below: children cleared, the
    // RETAINED index node re-inserted, no rebuild. (Visibility can't be
    // read off `create` ops here — the retained node was created at
    // seat time, so a structural swap is the observable.)
    let ops = h.ops().join("\n");
    assert!(ops.contains("clear_children"), "outlet cleared: {ops}");
    assert!(ops.contains("insert"), "revealed screen re-inserted: {ops}");
    assert_eq!(c.builds.get(), 1, "the revealed index was retained, not rebuilt");
    let (route, path, depth, can_go_back) = nav_state(&h, &app);
    assert_eq!(route, "home", "active mirror updated to the revealed index");
    assert_eq!(path, "/");
    assert_eq!(depth, 1);
    assert!(!can_go_back);
}

/// `Rebuild` + cold deep link: the synthesized parent entry is URL-only —
/// a page loaded at /detail must NEVER load the index (no build, no
/// effects, no fetches) until the user actually pops to it.
#[test]
fn regression_rebuild_cold_deep_link_never_mounts_parent_until_pop() {
    let h = Harness::new();
    set_initial_path(Some("/detail".to_string()));
    let c = Counters::default();
    let app = mount_counted(&h, c.clone(), StackRetention::Rebuild);

    assert_eq!(c.builds.get(), 0, "unvisited deep-link parent must not mount");
    assert!(shows(&h, "DETAIL SCREEN"));
    let (route, _, depth, can_go_back) = nav_state(&h, &app);
    assert_eq!(route, "detail");
    assert_eq!(depth, 2, "cold parent still counts for depth/back");
    assert!(can_go_back);

    // Popping is the first time the parent actually loads.
    h.take_log();
    app.nav.get().expect("handle filled").pop();
    h.world.flush();
    assert_eq!(c.builds.get(), 1, "parent mounts on first reveal");
    assert!(shows(&h, "HOME SCREEN"));
    let (route, path, depth, _) = nav_state(&h, &app);
    assert_eq!((route, path.as_str(), depth), ("home", "/", 1));
}

/// Popping the root must not remove it.
#[test]
fn pop_at_root_is_a_noop() {
    let h = Harness::new();
    let c = Counters::default();
    let app = mount_counted(&h, c.clone(), StackRetention::Retain);
    assert!(shows(&h, "HOME SCREEN"));

    app.nav.get().expect("handle filled").pop();
    h.world.flush();
    assert_eq!(c.disposals.get(), 0, "root survives a pop");
    let (route, _, depth, can_go_back) = nav_state(&h, &app);
    assert_eq!((route, depth, can_go_back), ("home", 1, false));
}
