//! `scroll_into_view`: the robot scrolls every scroll view an element sits
//! in, innermost first, by the least that shows it — along the axes each
//! one scrolls — and nothing at all when it is already in view. And
//! `get_platform`: the running app's platform, as a test driver names it.
//!
//! host-mock stands in for the toolkit: `frames` lays the elements out in
//! window coordinates, `scroll_offsets` is where each scroller is, and the
//! handle log records the offsets the robot asked for.
#![cfg(feature = "robot")]

use host_mock::Harness;
use runtime_scene::realize;
use runtime_shared::__serde_json::{from_str, json, Value};
use runtime_shared::primitives::portal::ViewportRect;
use runtime_shared::primitives::scroll_view::ScrollAxis;
use runtime_vocabulary::builders::{pressable, scroll_view, view};
use runtime_vocabulary::robot::bridge::invoke_command;
use runtime_vocabulary::caps::ScrollPort;
use runtime_vocabulary::robot::{Query, Robot, ScrollMove};

fn rect(x: f32, y: f32, width: f32, height: f32) -> ViewportRect {
    ViewportRect { x, y, width, height }
}

/// A 400×300 window holding a vertical page (n0) whose content (n1) has
/// a horizontal tab strip (n2) with a tab (n3) in it — a phone screen
/// with a scrolling tab bar below the fold.
fn page(h: &Harness, strip: ScrollAxis) -> runtime_scene::Realized<host_mock::Node> {
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            scroll_view()
                .test_id("page")
                .child(
                    view()
                        .child(
                            scroll_view()
                                .axis(strip)
                                .test_id("tabs")
                                .child(pressable(|| {}).test_id("tab").build())
                                .build(),
                        )
                        .build(),
                )
                .build(),
        )
    });
    assert_eq!(h.kind_of(0).as_deref(), Some("scroll_view"));
    assert_eq!(h.kind_of(2).as_deref(), Some("scroll_view"));
    runtime_shared::viewport::set_viewport_size(runtime_shared::viewport::ViewportSize { width: 400.0, height: 300.0 });
    let mut frames = h.shared.frames.borrow_mut();
    frames.insert(0, rect(0.0, 0.0, 400.0, 300.0));
    frames.insert(1, rect(0.0, 0.0, 400.0, 1000.0));
    frames.insert(2, rect(0.0, 500.0, 400.0, 50.0));
    drop(frames);
    realized
}

fn find(robot: &Robot, id: &str) -> runtime_vocabulary::robot::Element {
    robot.find(Query::test_id(id)).unwrap_or_else(|| panic!("{id} registered"))
}

fn scroll_calls() -> Vec<String> {
    host_mock::take_handle_log().into_iter().filter(|l| l.starts_with("scroll_to")).collect()
}

/// The CrewForge case: a tab off to the right of a horizontal strip that
/// is itself below the fold. The strip scrolls sideways only, the page
/// down only, each by the least that shows the whole tab.
#[test]
fn nested_scroll_views_each_scroll_the_least_along_their_own_axis() {
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let _r = page(&h, ScrollAxis::Horizontal);
    // The strip is already 100 px along; the tab sits at content x 1000.
    h.shared.scroll_offsets.borrow_mut().insert(2, (100.0, 0.0));
    h.shared.frames.borrow_mut().insert(3, rect(900.0, 510.0, 100.0, 30.0));
    let tabs = find(&robot, "tabs").id;
    let page_id = find(&robot, "page").id;
    host_mock::take_handle_log();

    let moved = robot.scroll_into_view(&find(&robot, "tab")).unwrap();

    assert_eq!(
        moved,
        vec![
            // Tab right edge 1000 → strip right edge 400: 600 more.
            ScrollMove { scroll_view: tabs, page: false, x: 700.0, y: 0.0 },
            // Tab bottom 540 → window bottom 300.
            ScrollMove { scroll_view: page_id, page: false, x: 0.0, y: 240.0 },
        ]
    );
    assert_eq!(scroll_calls(), ["scroll_to 700 0 n2", "scroll_to 0 240 n0"]);
    robot.reset();
}

/// Already fully in view: no scroll view moves.
#[test]
fn an_element_already_in_view_scrolls_nothing() {
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let _r = page(&h, ScrollAxis::Horizontal);
    h.shared.frames.borrow_mut().insert(2, rect(0.0, 100.0, 400.0, 50.0));
    h.shared.frames.borrow_mut().insert(3, rect(20.0, 110.0, 100.0, 30.0));
    host_mock::take_handle_log();

    assert_eq!(robot.scroll_into_view(&find(&robot, "tab")).unwrap(), vec![]);
    assert!(scroll_calls().is_empty());

    let out: Value =
        from_str(&invoke_command("scroll_into_view", &json!({ "element_id": find(&robot, "tab").id.0 })).unwrap())
            .unwrap();
    assert_eq!(out, json!({ "scrolled": [] }));
    robot.reset();
}

/// Content scrolled past (above / to the left) scrolls back, by the least
/// that shows it; an element bigger than its scroll view aligns to the
/// start; a two-axis scroller moves on both.
#[test]
fn scrolls_back_and_aligns_an_oversized_element_to_the_start() {
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let _r = page(&h, ScrollAxis::Both);
    h.shared.frames.borrow_mut().insert(2, rect(0.0, 0.0, 400.0, 300.0));
    h.shared.scroll_offsets.borrow_mut().insert(2, (500.0, 400.0));
    // 50 px above and 600 px wider than the strip, 200 px to its left.
    h.shared.frames.borrow_mut().insert(3, rect(-200.0, -50.0, 1000.0, 40.0));
    let tabs = find(&robot, "tabs").id;
    host_mock::take_handle_log();

    let moved = robot.scroll_into_view(&find(&robot, "tab")).unwrap();

    assert_eq!(moved, vec![ScrollMove { scroll_view: tabs, page: false, x: 300.0, y: 350.0 }]);
    assert_eq!(scroll_calls(), ["scroll_to 300 350 n2"]);
    robot.reset();
}

/// A horizontal strip never scrolls vertically, even when its own content
/// is below the window and no vertical scroller is outside it: UIKit takes
/// any offset it is given and would knock the strip off its rail.
#[test]
fn a_horizontal_scroller_is_never_asked_for_a_vertical_offset() {
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            scroll_view()
                .horizontal(true)
                .test_id("strip")
                .child(pressable(|| {}).test_id("chip").build())
                .build(),
        )
    });
    runtime_shared::viewport::set_viewport_size(runtime_shared::viewport::ViewportSize { width: 400.0, height: 300.0 });
    h.shared.frames.borrow_mut().insert(0, rect(0.0, 250.0, 400.0, 100.0));
    h.shared.frames.borrow_mut().insert(1, rect(450.0, 320.0, 50.0, 20.0));
    host_mock::take_handle_log();

    let moved = robot.scroll_into_view(&find(&robot, "chip")).unwrap();

    assert_eq!(moved, vec![ScrollMove { scroll_view: find(&robot, "strip").id, page: false, x: 100.0, y: 0.0 }]);
    assert_eq!(scroll_calls(), ["scroll_to 100 0 n0"]);
    drop(realized);
    robot.reset();
}

/// The verb answers with each scroll view it moved and its new offset.
#[test]
fn the_verb_reports_what_it_scrolled() {
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let _r = page(&h, ScrollAxis::Horizontal);
    h.shared.frames.borrow_mut().insert(3, rect(20.0, 510.0, 100.0, 30.0));

    let out: Value =
        from_str(&invoke_command("scroll_into_view", &json!({ "element_id": find(&robot, "tab").id.0 })).unwrap())
            .unwrap();
    assert_eq!(out, json!({ "scrolled": [{ "element_id": find(&robot, "page").id.0, "x": 0.0, "y": 240.0 }] }));
    robot.reset();
}

fn port(x: f32, y: f32, max_x: f32, max_y: f32, at: Option<ViewportRect>) -> ScrollPort {
    ScrollPort { x, y, max_x, max_y, port: at }
}

/// Regression (CrewForge 2026-11-03): a button below the fold of a page no
/// `scroll_view` holds — on the web the document scrolls when the app's
/// root outgrows the window — was never scrolled to, so `click()` failed
/// with "outside the window". The page's own scroller (the host scroller
/// above the app's tree) moves, by the least that shows the button.
#[test]
fn regression_click_scrolls_the_page_when_no_scroll_view_holds_the_element() {
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let _r = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            view().child(view().child(pressable(|| {}).test_id("add").build()).build()).build(),
        )
    });
    runtime_shared::viewport::set_viewport_size(runtime_shared::viewport::ViewportSize { width: 400.0, height: 300.0 });
    h.shared.frames.borrow_mut().insert(0, rect(0.0, 0.0, 400.0, 1000.0));
    h.shared.frames.borrow_mut().insert(1, rect(0.0, 0.0, 400.0, 1000.0));
    h.shared.frames.borrow_mut().insert(2, rect(10.0, 600.0, 100.0, 40.0));
    *h.shared.host_scroll.borrow_mut() = Some(port(0.0, 0.0, 0.0, 700.0, Some(rect(0.0, 0.0, 400.0, 300.0))));
    host_mock::take_handle_log();

    let moved = robot.scroll_into_view(&find(&robot, "add")).unwrap();

    let root = robot.find_all(Query::kind(runtime_vocabulary::robot::ElementKind::View))[0].id;
    // Bottom 640 → window bottom 300.
    assert_eq!(moved, vec![ScrollMove { scroll_view: root, page: true, x: 0.0, y: 340.0 }]);
    assert_eq!(host_mock::take_handle_log(), ["set_scroll_offset 0 340 host"]);
    let out: Value =
        from_str(&invoke_command("scroll_into_view", &json!({ "element_id": find(&robot, "add").id.0 })).unwrap())
            .unwrap();
    // The page is at 340 now, and the frames are the mock's to move: the
    // robot reads them again and scrolls again from the new offset.
    assert_eq!(out["scrolled"][0]["page"], json!(true), "{out}");
    robot.reset();
}

/// A registered element whose native object scrolls without being a
/// `scroll_view` (a list's own scroller, a web element whose CSS `overflow`
/// scrolls) is scrolled through `set_scroll_offset`, by the least that
/// shows the element and never past its range.
#[test]
fn a_scrolling_element_that_is_not_a_scroll_view_is_scrolled_too() {
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let _r = h.world.enter(|| {
        realize(&h.backend, &h.registry, view().test_id("list").child(pressable(|| {}).test_id("row").build()).build())
    });
    runtime_shared::viewport::set_viewport_size(runtime_shared::viewport::ViewportSize { width: 400.0, height: 300.0 });
    h.shared.frames.borrow_mut().insert(0, rect(0.0, 0.0, 400.0, 200.0));
    // The row's bottom is 260 px below the list's: 60 more than it holds.
    h.shared.frames.borrow_mut().insert(1, rect(0.0, 420.0, 400.0, 40.0));
    h.shared.scroll_ports.borrow_mut().insert(0, port(0.0, 0.0, 0.0, 200.0, None));
    host_mock::take_handle_log();

    let moved = robot.scroll_into_view(&find(&robot, "row")).unwrap();

    // 460 - 200 = 260, clamped to the list's 200.
    assert_eq!(moved, vec![ScrollMove { scroll_view: find(&robot, "list").id, page: false, x: 0.0, y: 200.0 }]);
    assert_eq!(host_mock::take_handle_log(), ["set_scroll_offset 0 200 n0"]);
    robot.reset();
}

/// A `scroll_view` that grew to its content (its range is 0) has nothing
/// to scroll: it is passed over, and the scroller outside it moves. Asking
/// it would have "moved" a scroller that cannot move, every retry.
#[test]
fn a_scroll_view_with_no_range_is_passed_over_for_the_one_outside_it() {
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let _r = page(&h, ScrollAxis::Vertical);
    // The inner scroller grew to 1000 px and reaches past the window.
    h.shared.frames.borrow_mut().insert(2, rect(0.0, 0.0, 400.0, 1000.0));
    h.shared.frames.borrow_mut().insert(3, rect(0.0, 700.0, 100.0, 30.0));
    h.shared.scroll_ports.borrow_mut().insert(2, port(0.0, 0.0, 0.0, 0.0, None));
    h.shared.scroll_ports.borrow_mut().insert(0, port(0.0, 0.0, 0.0, 700.0, None));
    host_mock::take_handle_log();

    let moved = robot.scroll_into_view(&find(&robot, "tab")).unwrap();

    assert_eq!(moved, vec![ScrollMove { scroll_view: find(&robot, "page").id, page: false, x: 0.0, y: 430.0 }]);
    // A scroll_view still moves through its own handle.
    assert_eq!(scroll_calls(), ["scroll_to 0 430 n0"]);
    robot.reset();
}

/// `get_platform` names the platform the boot declared, in the spelling
/// of `idealyst test --<platform>`.
#[test]
fn get_platform_names_the_running_platform() {
    use runtime_shared::Platform;
    for (platform, id) in [
        (Platform::Custom(""), "unknown"),
        (Platform::Ios, "ios"),
        (Platform::MacOs, "macos"),
        (Platform::Web, "web"),
        (Platform::Android, "android"),
        (Platform::Custom("windows"), "windows"),
        (Platform::Custom("linux"), "linux"),
    ] {
        runtime_shared::install_current_platform(platform);
        let out: Value = from_str(&invoke_command("get_platform", &json!({})).unwrap()).unwrap();
        assert_eq!(out, json!({ "platform": id }), "{platform:?}");
    }
}
