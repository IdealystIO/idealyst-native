//! What the robot reports a user can see, once clipping is involved, and
//! where a virtualized list's rows sit in the robot registry.
//!
//! host-mock stands in for the toolkit: `frames` lays elements out
//! (window coordinates), `clips` is each node's `overflow`
//! (`IntrospectionOps::content_clip`), `hidden` hides a node and what is
//! inserted under it, and `virtualizer(i)` hands back a list's callbacks
//! so a test mounts rows the way a platform's cell recycler does.
#![cfg(feature = "robot")]

use std::rc::Rc;

use host_mock::Harness;
use runtime_scene::realize;
use runtime_shared::__serde_json::{json, Value};
use runtime_shared::primitives::portal::ViewportRect;
use runtime_shared::primitives::virtualizer::ItemSize;
use runtime_vocabulary::builders::{text, view, virtualizer};
use runtime_vocabulary::caps::ContentClip;
use runtime_vocabulary::robot::bridge::invoke_command;
use runtime_vocabulary::robot::{ElementKind, Query, Robot, Visibility};

fn rect(x: f32, y: f32, width: f32, height: f32) -> ViewportRect {
    ViewportRect { x, y, width, height }
}

fn window(width: f32, height: f32) {
    runtime_shared::viewport::set_viewport_size(runtime_shared::viewport::ViewportSize { width, height });
}

fn find(robot: &Robot, id: &str) -> runtime_vocabulary::robot::Element {
    robot.find(Query::test_id(id)).unwrap_or_else(|| panic!("{id} registered"))
}

/// A drawer collapsed to a 0-pt-tall `overflow: hidden` box: its text keeps
/// a laid-out box of its own, but nothing of it can be seen. Before, the
/// robot asked only the toolkit (shown, in a window, opaque) and the text's
/// own box, so `assert_visible` passed on a blank screen.
#[test]
fn regression_text_in_a_zero_height_overflow_hidden_ancestor_is_not_visible() {
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let _r = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            view()
                .test_id("drawer")
                .child(view().test_id("body").child(text().content("Settings").test_id("item").build()).build())
                .build(),
        )
    });
    window(400.0, 800.0);
    {
        let mut frames = h.shared.frames.borrow_mut();
        frames.insert(0, rect(0.0, 100.0, 400.0, 0.0)); // drawer: collapsed
        frames.insert(1, rect(0.0, 100.0, 400.0, 200.0)); // its content keeps its height
        frames.insert(2, rect(16.0, 116.0, 120.0, 20.0));
    }
    h.shared.clips.borrow_mut().insert(0, ContentClip::Hidden);

    let item = find(&robot, "item");
    assert_eq!(robot.visibility(&item).unwrap(), Visibility { visible: false, in_viewport: Some(false) });
    let vf = robot.visible_frame(&item).unwrap().expect("frame known");
    assert_eq!((vf.width, vf.height), (0.0, 0.0), "nothing of it is left: {vf:?}");

    // The bridge says the same, with the clipped rect.
    let out: Value =
        runtime_shared::__serde_json::from_str(&invoke_command("find_element", &json!({ "test_id": "item" })).unwrap())
            .unwrap();
    assert_eq!(out["visible"], json!(false));
    assert_eq!(out["in_viewport"], json!(false));
    assert_eq!(out["visible_frame"]["height"].as_f64(), Some(0.0));

    // Opened, the same text is visible and whole.
    h.shared.frames.borrow_mut().insert(0, rect(0.0, 100.0, 400.0, 200.0));
    assert_eq!(robot.visibility(&item).unwrap(), Visibility { visible: true, in_viewport: Some(true) });
    assert_eq!(robot.visible_frame(&item).unwrap(), Some(rect(16.0, 116.0, 120.0, 20.0)));
    robot.reset();
}

/// A fully transparent (or hidden) ancestor hides its subtree: the toolkit
/// answers for the ancestors, and the robot believes it.
#[test]
fn an_element_under_a_hidden_ancestor_is_not_visible() {
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let _r = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            view().test_id("faded").child(text().content("Hi").test_id("hi").build()).build(),
        )
    });
    window(400.0, 800.0);
    h.shared.frames.borrow_mut().insert(0, rect(0.0, 0.0, 400.0, 100.0));
    h.shared.frames.borrow_mut().insert(1, rect(10.0, 10.0, 50.0, 20.0));
    h.shared.hidden.borrow_mut().insert(0);
    assert_eq!(robot.visibility(&find(&robot, "hi")).unwrap(), Visibility { visible: false, in_viewport: Some(false) });
    robot.reset();
}

/// A partly clipped element is visible, and `visible_frame` is the part
/// left; content a SCROLLER cuts off is still visible (scrolling reveals
/// it) but not in the viewport.
#[test]
fn clipping_cuts_the_visible_frame_and_only_overflow_hidden_hides() {
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let _r = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            view()
                .test_id("card")
                .child(text().content("half").test_id("half").build())
                .child(view().test_id("scroller").child(text().content("below").test_id("below").build()).build())
                .build(),
        )
    });
    window(400.0, 800.0);
    {
        let mut frames = h.shared.frames.borrow_mut();
        frames.insert(0, rect(0.0, 0.0, 300.0, 300.0)); // card, overflow hidden
        frames.insert(1, rect(250.0, 10.0, 100.0, 20.0)); // half out of the card
        frames.insert(2, rect(0.0, 100.0, 300.0, 100.0)); // scroll viewport
        frames.insert(3, rect(0.0, 250.0, 300.0, 20.0)); // scrolled below it
    }
    h.shared.clips.borrow_mut().insert(0, ContentClip::Hidden);
    h.shared.clips.borrow_mut().insert(2, ContentClip::Scroll);

    let half = find(&robot, "half");
    assert_eq!(robot.visibility(&half).unwrap(), Visibility { visible: true, in_viewport: Some(true) });
    assert_eq!(robot.visible_frame(&half).unwrap(), Some(rect(250.0, 10.0, 50.0, 20.0)));

    let below = find(&robot, "below");
    assert_eq!(
        robot.visibility(&below).unwrap(),
        Visibility { visible: true, in_viewport: Some(false) },
        "inside the card, scrolled out of its scroller: reachable, not on screen"
    );
    let vf = robot.visible_frame(&below).unwrap().unwrap();
    assert!(vf.width == 0.0 || vf.height == 0.0, "{vf:?}");

    // A `Visible` (or unknown) ancestor cuts nothing.
    h.shared.clips.borrow_mut().insert(0, ContentClip::Visible);
    assert_eq!(robot.visible_frame(&half).unwrap(), Some(rect(250.0, 10.0, 100.0, 20.0)));
    // The window still does.
    h.shared.frames.borrow_mut().insert(1, rect(350.0, 10.0, 100.0, 20.0));
    assert_eq!(robot.visible_frame(&half).unwrap(), Some(rect(350.0, 10.0, 50.0, 20.0)));
    robot.reset();
}

/// A list built from a `test_id`-anchored container, as an app writes one,
/// with one row mounted the way a platform's cell recycler mounts it.
fn list_fixture(h: &Harness) -> (runtime_scene::Realized<host_mock::Node>, host_mock::Node) {
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            view()
                .test_id("screen")
                .child(
                    virtualizer(
                        || 3,
                        |i| i as u64,
                        ItemSize::Known(Rc::new(|_| 40.0)),
                        |i| view().test_id("card").child(text().content(format!("Row {i}")).build()).build(),
                    )
                    .test_id("list")
                    .build(),
                )
                .build(),
        )
    });
    let cbs = h.virtualizer(0);
    let (row, _scope) = h.world.enter(|| (cbs.mount_item)(0));
    (realized, row)
}

/// Each virtualized row realizes detached — on every backend it sits in its
/// own native cell — so its registry entry had no parent: a row was a ROOT.
/// `get_parent` stopped at the row, `within(list)` never matched one, and a
/// refused `tap_native` named the list's container as the element on top
/// of its own row (iOS, CrewForge: "View #297 would receive the tap").
/// The list is an element now, and its rows register under it.
#[test]
fn regression_virtualized_rows_register_under_their_list() {
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let (_r, _row) = list_fixture(&h);

    let list = find(&robot, "list");
    assert_eq!(list.kind, ElementKind::Virtualizer);
    assert_eq!(robot.parent_of(&list).and_then(|p| p.test_id), Some("screen"));

    let card = find(&robot, "card");
    assert_eq!(robot.parent_of(&card).map(|p| p.id), Some(list.id), "the row's parent is the list");
    assert_eq!(robot.children_of(&list).iter().map(|c| c.id).collect::<Vec<_>>(), vec![card.id]);

    let row_text = robot.find(Query::label("Row 0").within(Query::test_id("list"))).expect("within(list) reaches rows");
    assert_eq!(robot.parent_of(&row_text).map(|p| p.id), Some(card.id));
    assert!(robot.find(Query::label("Row 0").within(Query::test_id("screen"))).is_some());

    // The snapshot nests it too: one root, the screen.
    assert_eq!(robot.snapshot().len(), 1, "no detached row roots");
    robot.reset();
}

/// The list clips its rows: a row scrolled past the list's viewport is
/// visible (scrolling reveals it) but not in the viewport, and its
/// `visible_frame` is cut at the list — the cross-root case, since the row
/// is not under the list in any tree but the registry's.
#[test]
fn a_virtualized_row_is_clipped_by_its_list() {
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let (_r, row) = list_fixture(&h);
    let list_node = 1; // n0 screen, n1 list
    window(400.0, 800.0);
    {
        let mut frames = h.shared.frames.borrow_mut();
        frames.insert(0, rect(0.0, 0.0, 400.0, 800.0));
        frames.insert(list_node, rect(16.0, 198.0, 338.0, 100.0));
        frames.insert(row, rect(16.0, 278.0, 338.0, 40.0)); // half below the list's bottom
    }
    h.shared.clips.borrow_mut().insert(list_node, ContentClip::Scroll);

    let card = find(&robot, "card");
    assert_eq!(robot.visibility(&card).unwrap(), Visibility { visible: true, in_viewport: Some(true) });
    assert_eq!(robot.visible_frame(&card).unwrap(), Some(rect(16.0, 278.0, 338.0, 20.0)));

    h.shared.frames.borrow_mut().insert(row, rect(16.0, 318.0, 338.0, 40.0)); // wholly below
    assert_eq!(robot.visibility(&card).unwrap(), Visibility { visible: true, in_viewport: Some(false) });
    robot.reset();
}

/// `Visibility::clipped` on its own: unknown parts never hide.
#[test]
fn clipped_with_unknown_parts() {
    let (v, f) = Visibility::clipped(None, None, &[(ContentClip::Hidden, rect(0.0, 0.0, 0.0, 0.0))], None);
    assert_eq!((v, f), (Visibility { visible: true, in_viewport: None }, None));
    let (v, f) = Visibility::clipped(Some(true), Some(rect(0.0, 0.0, 10.0, 10.0)), &[], None);
    assert_eq!((v, f), (Visibility { visible: true, in_viewport: None }, Some(rect(0.0, 0.0, 10.0, 10.0))));
}

/// A horizontal tab strip inside a screen-wide column that clips (on
/// Android every `ViewGroup` does): the last tab sits far right of the
/// column, inside the strip's scrolled-away content. Cutting the tab by the
/// column called it not visible, so `click()` waited for it to appear
/// instead of scrolling the strip to it. The column cuts the STRIP's box;
/// the tab is visible, out of the viewport, until the strip scrolls.
#[test]
fn regression_tab_scrolled_out_of_a_strip_in_a_clipping_column_is_visible() {
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let _r = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            view()
                .test_id("column")
                .child(
                    view()
                        .test_id("strip")
                        .child(view().test_id("row").child(text().content("Last tab").test_id("last").build()).build())
                        .build(),
                )
                .build(),
        )
    });
    window(411.0, 800.0);
    {
        let mut frames = h.shared.frames.borrow_mut();
        frames.insert(0, rect(0.0, 0.0, 411.0, 691.0)); // column: clips
        frames.insert(1, rect(16.0, 431.0, 379.0, 50.0)); // strip: scrolls
        frames.insert(2, rect(16.0, 431.0, 4088.0, 50.0)); // row: as wide as its tabs
        frames.insert(3, rect(4016.0, 431.0, 88.0, 50.0)); // last tab, scrolled away
    }
    h.shared.clips.borrow_mut().insert(0, ContentClip::Hidden);
    h.shared.clips.borrow_mut().insert(1, ContentClip::Scroll);
    h.shared.clips.borrow_mut().insert(2, ContentClip::Hidden);

    let last = find(&robot, "last");
    assert_eq!(
        robot.visibility(&last).unwrap(),
        Visibility { visible: true, in_viewport: Some(false) },
        "scrolling the strip reveals it"
    );
    let vf = robot.visible_frame(&last).unwrap().unwrap();
    assert!(vf.width == 0.0 || vf.height == 0.0, "nothing of it on screen yet: {vf:?}");

    // Scrolled into the strip, it is on screen and whole.
    h.shared.frames.borrow_mut().insert(3, rect(300.0, 431.0, 88.0, 50.0));
    assert_eq!(robot.visibility(&last).unwrap(), Visibility { visible: true, in_viewport: Some(true) });
    assert_eq!(robot.visible_frame(&last).unwrap(), Some(rect(300.0, 431.0, 88.0, 50.0)));

    // A clipping box INSIDE the strip still hides what it cuts away.
    h.shared.frames.borrow_mut().insert(3, rect(4200.0, 431.0, 88.0, 50.0)); // past the row's end
    assert_eq!(robot.visibility(&last).unwrap().visible, false, "cut by the row, which scrolling doesn't change");

    // And the column still hides the strip itself when it cuts it away.
    h.shared.frames.borrow_mut().insert(3, rect(4016.0, 431.0, 88.0, 50.0));
    h.shared.frames.borrow_mut().insert(0, rect(0.0, 0.0, 411.0, 0.0)); // column collapsed
    assert_eq!(robot.visibility(&last).unwrap().visible, false);
    robot.reset();
}
