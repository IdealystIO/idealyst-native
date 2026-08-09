//! Screen state ⇄ query params.
//!
//! The contract these pin down: a screen's initial state can arrive either
//! from an in-app navigation (`push_with_state` / `select_with_state`) or
//! from a cold load of a URL carrying the same query — and BOTH deliver the
//! identical value to `screen_state::<S>()` inside the screen builder. That
//! equivalence is the whole point of encoding state as query params rather
//! than as an opaque payload, so it is what the tests assert directly.
//!
//! The second contract: query params are NOT part of route identity.
//! Navigating to the same path with a different query updates the reactive
//! `query` signal without remounting the screen — a filter change must not
//! tear down and rebuild the list it filters.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use host_mock::Harness;
use runtime_shared::primitives::navigator::{
    screen_query, screen_state, set_initial_path, QueryParams, Route, RouteParams, ScreenState,
};
use runtime_scene::{realize, Realized};
use runtime_vocabulary::builders::{navigator_outlet, stack_navigator, swap_navigator, text, view};
use runtime_vocabulary::prims::{NavHandle, StackNav, StackRetention, SwapNav};
use runtime_world::inject;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const INBOX: Route = Route::new("inbox", "/inbox");
const DETAIL: Route<ItemId> = Route::new("detail", "/items/:id");

#[derive(Clone, PartialEq, Debug)]
struct ItemId(u32);

impl RouteParams for ItemId {
    fn to_path(&self, pattern: &str) -> String {
        pattern.replace(":id", &self.0.to_string())
    }
    fn from_segments(segs: &std::collections::HashMap<String, String>) -> Option<Self> {
        segs.get("id").and_then(|s| s.parse().ok()).map(ItemId)
    }
}

/// The canonical `ScreenState` shape: every field has a default, so a
/// partial or absent query still decodes.
#[derive(Clone, Debug, Default, PartialEq)]
struct Filters {
    tab: String,
    page: u32,
}

impl ScreenState for Filters {
    fn to_query(&self) -> QueryParams {
        QueryParams::new()
            .with("tab", self.tab.clone())
            .with("page", self.page.to_string())
    }
    fn from_query(q: &QueryParams) -> Option<Self> {
        Some(Filters {
            tab: q.get("tab").unwrap_or("all").to_string(),
            page: q.get_as("page").unwrap_or(1),
        })
    }
}

fn harness() -> Harness {
    Harness::new()
}

/// Records what each build of a screen saw as its state, and how many
/// times the builder ran.
#[derive(Clone, Default)]
struct Seen {
    states: Rc<RefCell<Vec<Filters>>>,
    raw: Rc<RefCell<Vec<QueryParams>>>,
    builds: Rc<Cell<u32>>,
}

impl Seen {
    fn record(&self) {
        self.builds.set(self.builds.get() + 1);
        self.states
            .borrow_mut()
            .push(screen_state::<Filters>().expect("Filters decodes any query"));
        self.raw.borrow_mut().push(screen_query());
    }
    fn last(&self) -> Filters {
        self.states.borrow().last().cloned().expect("a build happened")
    }
    fn builds(&self) -> u32 {
        self.builds.get()
    }
}

struct SwapFixture {
    _realized: Realized<u32>,
    handle: NavHandle,
    ctx: SwapNav,
    inbox: Seen,
}

fn mount_swap(h: &Harness) -> SwapFixture {
    let inbox = Seen::default();
    let handle_slot: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
    let ctx_slot: Rc<RefCell<Option<SwapNav>>> = Rc::new(RefCell::new(None));

    let element = {
        let seen = inbox.clone();
        let handle_slot = handle_slot.clone();
        let ctx_slot = ctx_slot.clone();
        swap_navigator(&INBOX)
            .screen(INBOX, move |_| {
                seen.record();
                view().child(text().content("inbox")).build()
            })
            .layout(move || {
                *ctx_slot.borrow_mut() = inject::<SwapNav>();
                view().child(navigator_outlet()).build()
            })
            .on_handle(move |handle| *handle_slot.borrow_mut() = Some(handle))
            .build()
    };
    let realized = realize(&h.backend, &h.registry, element);
    let handle = handle_slot.borrow_mut().take().expect("handle at mount");
    let ctx = ctx_slot.borrow_mut().take().expect("SwapNav provided");
    SwapFixture { _realized: realized, handle, ctx, inbox }
}

struct StackFixture {
    _realized: Realized<u32>,
    handle: NavHandle,
    ctx: StackNav,
    inbox: Seen,
    detail: Seen,
}

fn mount_stack(h: &Harness) -> StackFixture {
    let inbox = Seen::default();
    let detail = Seen::default();
    let handle_slot: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
    let ctx_slot: Rc<RefCell<Option<StackNav>>> = Rc::new(RefCell::new(None));

    let element = {
        let seen_inbox = inbox.clone();
        let seen_detail = detail.clone();
        let handle_slot = handle_slot.clone();
        let ctx_slot = ctx_slot.clone();
        stack_navigator(&INBOX)
            .screen(INBOX, move |_| {
                seen_inbox.record();
                view().child(text().content("inbox")).build()
            })
            .screen(DETAIL, move |_id: ItemId| {
                seen_detail.record();
                view().child(text().content("detail")).build()
            })
            .retention(StackRetention::Retain)
            .layout(move || {
                *ctx_slot.borrow_mut() = inject::<StackNav>();
                view().child(navigator_outlet()).build()
            })
            .on_handle(move |handle| *handle_slot.borrow_mut() = Some(handle))
            .build()
    };
    let realized = realize(&h.backend, &h.registry, element);
    let handle = handle_slot.borrow_mut().take().expect("handle at mount");
    let ctx = ctx_slot.borrow_mut().take().expect("StackNav provided");
    StackFixture { _realized: realized, handle, ctx, inbox, detail }
}

// ---------------------------------------------------------------------------
// The two sources agree
// ---------------------------------------------------------------------------

#[test]
fn push_with_state_reaches_the_screen_builder() {
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        let fx = mount_stack(&h);
        fx.handle.push_with_state(
            &DETAIL,
            ItemId(5),
            Filters { tab: "notes".into(), page: 3 },
        );
        world.flush();
        assert_eq!(
            fx.detail.last(),
            Filters { tab: "notes".into(), page: 3 },
            "the pushed state is what the screen builder read"
        );
    });
}

#[test]
fn cold_load_of_a_query_url_seeds_the_same_state_a_push_would() {
    let h = harness();
    let world = h.world.clone();

    // What an in-app navigation produces.
    let pushed = world.enter(|| {
        let fx = mount_stack(&h);
        fx.handle.push_with_state(
            &DETAIL,
            ItemId(5),
            Filters { tab: "notes".into(), page: 3 },
        );
        world.flush();
        fx.detail.last()
    });

    // What a cold load of the URL that navigation wrote produces. This is
    // the equivalence the whole design rests on: without it, state passing
    // would only work for screens reached by navigating, and every screen
    // would need a second code path for "arrived by link".
    let h2 = harness();
    let world2 = h2.world.clone();
    let cold = world2.enter(|| {
        set_initial_path(Some("/items/5?tab=notes&page=3".to_string()));
        let fx = mount_stack(&h2);
        fx.detail.last()
    });

    assert_eq!(pushed, cold, "navigation and cold load must agree");
    assert_eq!(cold, Filters { tab: "notes".into(), page: 3 });
}

#[test]
fn cold_load_without_a_query_falls_back_to_defaults() {
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        set_initial_path(None);
        let fx = mount_stack(&h);
        assert_eq!(
            fx.inbox.last(),
            Filters { tab: "all".into(), page: 1 },
            "no query ⇒ ScreenState::from_query fills its own defaults"
        );
    });
}

#[test]
fn a_partial_query_fills_the_missing_fields_with_defaults() {
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        // A hand-truncated / hand-edited URL must degrade, not fail.
        set_initial_path(Some("/items/5?tab=labs".to_string()));
        let fx = mount_stack(&h);
        assert_eq!(fx.detail.last(), Filters { tab: "labs".into(), page: 1 });
    });
}

// ---------------------------------------------------------------------------
// Query is state, not identity
// ---------------------------------------------------------------------------

#[test]
fn regression_query_does_not_leak_into_route_params() {
    // `match_prefix` splits on '/' — before the query split, a deep link to
    // `/items/5?tab=notes` bound `id` to the literal string "5?tab=notes",
    // so the screen fetched a nonexistent record. The param must be clean.
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        set_initial_path(Some("/items/5?tab=notes".to_string()));
        let seen_id = Rc::new(RefCell::new(None));
        let element = {
            let seen_id = seen_id.clone();
            stack_navigator(&INBOX)
                .screen(INBOX, |_| view().build())
                .screen(DETAIL, move |id: ItemId| {
                    *seen_id.borrow_mut() = Some(id);
                    view().build()
                })
                .build()
        };
        let _realized = realize(&h.backend, &h.registry, element);
        assert_eq!(
            *seen_id.borrow(),
            Some(ItemId(5)),
            "the query must not reach the path params"
        );
    });
}

#[test]
fn regression_active_path_excludes_the_query() {
    // The path mirror feeds nested navigators' base prefixes; a `?` in it
    // would compose into `/items/5?tab=a/notes` for a nested navigator.
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        let fx = mount_stack(&h);
        fx.handle
            .push_with_state(&DETAIL, ItemId(5), Filters { tab: "notes".into(), page: 1 });
        world.flush();
        assert_eq!(fx.ctx.active_path.get(), "/items/5", "path mirror is path-only");
        assert_eq!(fx.ctx.query.get().get("tab"), Some("notes"));
    });
}

#[test]
fn changing_only_the_query_updates_state_without_remounting() {
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        let fx = mount_swap(&h);
        assert_eq!(fx.inbox.builds(), 1);

        // A filter change: same route, same path, different state. The
        // screen must NOT be rebuilt (that would reset its scroll and
        // focus on every keystroke) but the reactive query must update.
        fx.handle
            .select_with_state(&INBOX, (), Filters { tab: "starred".into(), page: 2 });
        world.flush();

        assert_eq!(
            fx.inbox.builds(),
            1,
            "a query-only change must not remount the screen"
        );
        assert_eq!(fx.ctx.query.get().get("tab"), Some("starred"));
        assert_eq!(fx.ctx.query.get().get_as::<u32>("page"), Some(2));
        assert_eq!(
            Filters::from_query(&fx.ctx.query.get()),
            Some(Filters { tab: "starred".into(), page: 2 }),
            "the reactive signal decodes to the same struct the builder would"
        );
    });
}

#[test]
fn stack_replace_with_state_on_the_same_path_does_not_remount() {
    // `replace_with_state` is the documented verb for a filter change, so
    // it must behave like one: the URL and the reactive state move, the
    // screen does not. A remount here would reset the list's scroll and
    // focus on every keystroke of a search box.
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        set_initial_path(None);
        let fx = mount_stack(&h);
        assert_eq!(fx.inbox.builds(), 1);

        fx.handle
            .replace_with_state(&INBOX, (), Filters { tab: "starred".into(), page: 2 });
        world.flush();

        assert_eq!(fx.inbox.builds(), 1, "same-path replace must not remount");
        assert_eq!(fx.ctx.depth.get(), 1, "and must not change depth");
        assert_eq!(
            Filters::from_query(&fx.ctx.query.get()),
            Some(Filters { tab: "starred".into(), page: 2 }),
            "but the state does change"
        );
    });
}

#[test]
fn stack_replace_onto_a_different_path_still_swaps_the_screen() {
    // The in-place fast path above must not swallow a real replace.
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        set_initial_path(None);
        let fx = mount_stack(&h);
        fx.handle
            .replace_with_state(&DETAIL, ItemId(9), Filters { tab: "notes".into(), page: 1 });
        world.flush();

        assert_eq!(fx.detail.builds(), 1, "a different route still mounts");
        assert_eq!(fx.ctx.active_route.get(), "detail");
        assert_eq!(fx.ctx.active_path.get(), "/items/9");
        assert_eq!(fx.ctx.depth.get(), 1, "replace does not grow the stack");
    });
}

#[test]
fn pop_restores_the_revealed_screens_state() {
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        set_initial_path(None);
        let fx = mount_stack(&h);
        // Give the root a state of its own, then cover it.
        fx.handle
            .replace_with_state(&INBOX, (), Filters { tab: "starred".into(), page: 7 });
        world.flush();
        fx.handle
            .push_with_state(&DETAIL, ItemId(5), Filters { tab: "notes".into(), page: 1 });
        world.flush();
        assert_eq!(fx.ctx.query.get().get("tab"), Some("notes"));

        // Popping must republish the REVEALED entry's state, not leave the
        // popped screen's state showing.
        (fx.ctx.pop)();
        world.flush();
        assert_eq!(
            Filters::from_query(&fx.ctx.query.get()),
            Some(Filters { tab: "starred".into(), page: 7 }),
            "pop restores the revealed screen's state"
        );
    });
}

#[test]
fn state_survives_a_cold_remount_of_a_covered_screen() {
    // Under `Rebuild` retention the covered screen is disposed on push and
    // re-mounted from its stack entry on pop. The entry must carry the
    // query, or the screen comes back with its state silently reset.
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        set_initial_path(None);
        let inbox = Seen::default();
        let handle_slot: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
        let element = {
            let seen = inbox.clone();
            let handle_slot = handle_slot.clone();
            stack_navigator(&INBOX)
                .screen(INBOX, move |_| {
                    seen.record();
                    view().child(text().content("inbox")).build()
                })
                .screen(DETAIL, |_id: ItemId| view().build())
                .retention(StackRetention::Rebuild)
                .on_handle(move |h| *handle_slot.borrow_mut() = Some(h))
                .build()
        };
        let _realized = realize(&h.backend, &h.registry, element);
        let handle = handle_slot.borrow_mut().take().expect("handle");

        handle.replace_with_state(&INBOX, (), Filters { tab: "starred".into(), page: 7 });
        world.flush();
        handle.push(&DETAIL, ItemId(5));
        world.flush();
        handle.pop();
        world.flush();

        assert!(inbox.builds() >= 2, "Rebuild retention re-mounts on pop");
        assert_eq!(
            inbox.last(),
            Filters { tab: "starred".into(), page: 7 },
            "the re-mounted screen sees the state it was left with"
        );
    });
}

// ---------------------------------------------------------------------------
// Route-pattern defaults
// ---------------------------------------------------------------------------

#[test]
fn a_route_pattern_may_carry_default_query_params() {
    const FILTERED: Route = Route::new("inbox", "/inbox?tab=unread");
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        set_initial_path(None);
        let seen = Seen::default();
        let element = {
            let seen = seen.clone();
            swap_navigator(&FILTERED)
                .screen(FILTERED, move |_| {
                    seen.record();
                    view().build()
                })
                .build()
        };
        let _realized = realize(&h.backend, &h.registry, element);
        assert_eq!(
            seen.last().tab,
            "unread",
            "the initial path's own query seeds the screen"
        );
        assert_eq!(
            seen.raw.borrow().last().unwrap().get("tab"),
            Some("unread")
        );
    });
}

#[test]
fn explicit_state_overrides_a_pattern_default_key_by_key() {
    const FILTERED: Route = Route::new("inbox", "/inbox?tab=unread&page=9");
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        set_initial_path(None);
        let seen = Seen::default();
        let handle_slot: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
        let element = {
            let seen = seen.clone();
            let handle_slot = handle_slot.clone();
            stack_navigator(&FILTERED)
                .screen(FILTERED, move |_| {
                    seen.record();
                    view().build()
                })
                .on_handle(move |h| *handle_slot.borrow_mut() = Some(h))
                .build()
        };
        let _realized = realize(&h.backend, &h.registry, element);
        let handle = handle_slot.borrow_mut().take().expect("handle");

        // `Filters` writes both keys, so both are overridden; a state that
        // wrote only `tab` would leave the pattern's `page=9` standing.
        handle.push_with_state(&FILTERED, (), Filters { tab: "all".into(), page: 2 });
        world.flush();
        assert_eq!(seen.last(), Filters { tab: "all".into(), page: 2 });
    });
}
