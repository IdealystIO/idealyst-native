//! End-to-end OUTLET-MODEL navigator round-trips over the wire.
//!
//! The outlet model needs no kind-specific wire commands: the author
//! layout is recorded like any subtree, and the handler's screen swaps
//! go through ordinary node ops (`create_*` / `insert` /
//! `clear_children`) the dev-client already replays. These tests prove
//! that by realizing the REAL `swap_navigator` / `stack_navigator`
//! vocabulary handlers on the runtime-server recorder and replaying into
//! a `MockBackend`:
//!
//! - swap: author chrome + initial screen reconstruct; `Select` swaps
//!   the outlet's child on the far side;
//! - stack: push shows the detail on the far side, pop reveals the
//!   screen below.
//!
//! The thing under guard is the ABSENCE of a navigator-shaped protocol:
//! if a handler ever starts needing a `Command::Navigator*` op to make a
//! screen visible on a thin client, one of these assertions goes red.

use std::cell::RefCell;
use std::rc::Rc;

use mock_backend::WireHarness;
use runtime_shared::primitives::navigator::Route;
use runtime_vocabulary::builders::{navigator_outlet, stack_navigator, swap_navigator, text, view};
use runtime_vocabulary::prims::{NavHandle, SwapNav};
use runtime_world::inject;

const HOME: Route<()> = Route::<()>::new("home", "/");
const SETTINGS: Route<()> = Route::<()>::new("settings", "/settings");
const DETAIL: Route<()> = Route::<()>::new("detail", "/detail");

#[test]
fn swap_select_round_trips_through_wire_to_mock() {
    // The layout runs inside the realize pass, so the select callback is
    // captured out of the injected `SwapNav` context (same shape the
    // scene-parity nav scenarios use).
    let on_select: Rc<RefCell<Option<Rc<dyn Fn(&'static str)>>>> = Rc::new(RefCell::new(None));
    let slot = on_select.clone();

    let mut h = WireHarness::mount(move || {
        swap_navigator(&HOME)
            .screen(HOME, |_| view().child(text().content("HOME CONTENT")).build())
            .screen(SETTINGS, |_| {
                view().child(text().content("SETTINGS CONTENT")).build()
            })
            .layout(move || {
                let ctx = inject::<SwapNav>().expect("SwapNav provided during layout build");
                *slot.borrow_mut() = Some(ctx.on_select.clone());
                view()
                    .child(navigator_outlet())
                    .child(text().content("TAB BAR"))
                    .build()
            })
            .build()
    });

    {
        let scene = h.scene();
        assert!(
            scene.contains_text("TAB BAR"),
            "author chrome replayed:\n{}",
            scene.dump()
        );
        assert!(
            scene.contains_text("HOME CONTENT"),
            "initial screen replayed:\n{}",
            scene.dump()
        );
        assert!(
            !scene.contains_text("SETTINGS CONTENT"),
            "inactive sibling not mounted:\n{}",
            scene.dump()
        );
    }

    // Select on the HOST side — the outlet swap ships as plain node ops.
    let select = on_select.borrow().clone().expect("layout ran during mount");
    select("settings");
    h.tick_and_sync();

    let scene = h.scene();
    assert!(
        scene.contains_text("SETTINGS CONTENT"),
        "selected screen replayed:\n{}",
        scene.dump()
    );
    assert!(
        !scene.contains_text("HOME CONTENT"),
        "prior screen removed:\n{}",
        scene.dump()
    );
    assert!(
        scene.contains_text("TAB BAR"),
        "chrome persists:\n{}",
        scene.dump()
    );
}

#[test]
fn stack_push_pop_round_trips_through_wire_to_mock() {
    let handle_slot: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
    let slot = handle_slot.clone();

    let mut h = WireHarness::mount(move || {
        stack_navigator(&HOME)
            .screen(HOME, |_| view().child(text().content("HOME CONTENT")).build())
            .screen(DETAIL, |_| {
                view().child(text().content("DETAIL CONTENT")).build()
            })
            .layout(|| {
                view()
                    .child(text().content("HEADER CHROME"))
                    .child(navigator_outlet())
                    .build()
            })
            .on_handle(move |handle| *slot.borrow_mut() = Some(handle))
            .build()
    });

    {
        let scene = h.scene();
        assert!(
            scene.contains_text("HEADER CHROME"),
            "author chrome replayed:\n{}",
            scene.dump()
        );
        assert!(
            scene.contains_text("HOME CONTENT"),
            "initial screen replayed:\n{}",
            scene.dump()
        );
    }

    let handle = handle_slot.borrow().clone().expect("handle bound at mount");
    handle.push(&DETAIL, ());
    h.tick_and_sync();
    {
        let scene = h.scene();
        assert!(
            scene.contains_text("DETAIL CONTENT"),
            "pushed screen replayed:\n{}",
            scene.dump()
        );
        assert!(
            !scene.contains_text("HOME CONTENT"),
            "covered screen detached:\n{}",
            scene.dump()
        );
    }

    handle.pop();
    h.tick_and_sync();
    let scene = h.scene();
    assert!(
        scene.contains_text("HOME CONTENT"),
        "pop revealed home:\n{}",
        scene.dump()
    );
    assert!(
        !scene.contains_text("DETAIL CONTENT"),
        "popped screen removed:\n{}",
        scene.dump()
    );
}

/// The point of the outlet model, stated as an assertion: a navigator's
/// screens become visible on a thin client using ONLY the generic node
/// ops. No navigator-shaped `Command` variant appears in the recorded
/// stream at all.
#[test]
fn navigators_need_no_kind_specific_wire_commands() {
    dev_server::scheduler::install();
    let recorder = dev_server::WireRecordingBackend::new();
    let _session = dev_server::newcore::SceneSession::mount(&recorder, |_r| {}, || {
        swap_navigator(&HOME)
            .screen(HOME, |_| view().child(text().content("HOME CONTENT")).build())
            .screen(SETTINGS, |_| {
                view().child(text().content("SETTINGS CONTENT")).build()
            })
            .layout(|| view().child(navigator_outlet()).build())
            .build()
    });
    recorder.tick_animations(std::time::Duration::from_millis(16));

    let offenders: Vec<String> = recorder
        .drain_commands()
        .iter()
        .filter_map(|c| {
            let head = format!("{c:?}")
                .split(&[' ', '{'][..])
                .next()
                .unwrap_or("")
                .to_string();
            head.contains("Navigator").then_some(head)
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "the outlet model must ride generic node ops only; got navigator-shaped \
         wire commands: {offenders:?}"
    );
}
