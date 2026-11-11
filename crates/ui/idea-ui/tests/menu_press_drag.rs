//! Press, drag, release on a real `Menu`: a press that opened the menu
//! (a `PressActivation::Down` trigger holds the pointer) is dragged onto a
//! row and released there, and the row is picked.
//!
//! The row never gets a press of its own in that gesture — every backend
//! delivers the rest of a press to the control that took the down — so
//! before `press_drag` nothing fired and the menu just stayed open.
//!
//! Driven through the app-level pointer sink the backend would feed
//! (`AppEnvOps::set_pointer_sink`), with rows and panel placed on screen by
//! `Harness::place`. The pointer half of each backend (that a held press's
//! moves and release do reach the app-level source) is the backends' own
//! tests; what is pinned here is everything from that stream to `on_select`.

use std::cell::Cell;
use std::rc::Rc;

use host_mock::{Harness, Node};
use idea_ui::{install_idea_theme, light_theme, Menu, MenuEntry, MenuItem, SubMenu};
use runtime_core::{
    ui, AnchorTarget, AppPointerEvent, Element, MouseButton, PointerPhase, PressableHandle, Ref,
    ViewportRect,
};

const ROW_H: f32 = 30.0;
const PANEL: ViewportRect = ViewportRect { x: 100.0, y: 100.0, width: 200.0, height: 300.0 };

fn mount(build: impl FnOnce() -> Element) -> (Harness, runtime_scene::Realized<Node>) {
    runtime_shared::pointer_input::reset_for_tests();
    runtime_shared::held_press::reset_for_tests();
    let h = Harness::new();
    runtime_vocabulary::backend::install_env_services(&h.backend);
    let tree = h.world.enter(|| {
        install_idea_theme(light_theme());
        build()
    });
    let realized = h.mount(tree);
    h.flush();
    (h, realized)
}

/// The pressable whose subtree shows `label`.
fn row(h: &Harness, label: &str) -> Node {
    let quoted = format!("text {label:?}");
    let text = h
        .shared
        .kinds
        .borrow()
        .iter()
        .find(|(_, kind)| **kind == quoted)
        .map(|(n, _)| *n)
        .unwrap_or_else(|| panic!("no text {label:?} mounted"));
    let mut node = text;
    loop {
        if h.kind_of(node).is_some_and(|k| k.starts_with("pressable")) {
            return node;
        }
        node = *h.shared.parent.borrow().get(&node).unwrap_or_else(|| panic!("{label:?} is not in a pressable"));
    }
}

/// Lay the named rows out top to bottom inside [`PANEL`], and put every
/// plain view (the panel among them) on the panel's rect.
fn place_rows(h: &Harness, labels: &[&str]) {
    let views: Vec<Node> =
        h.shared.kinds.borrow().iter().filter(|(_, k)| *k == "view").map(|(n, _)| *n).collect();
    for v in views {
        h.place(v, PANEL);
    }
    for (i, label) in labels.iter().enumerate() {
        h.place(
            row(h, label),
            ViewportRect { x: PANEL.x, y: PANEL.y + i as f32 * ROW_H, width: PANEL.width, height: ROW_H },
        );
    }
}

/// A point inside row `i`.
fn over_row(i: usize) -> (f32, f32) {
    (PANEL.x + 20.0, PANEL.y + i as f32 * ROW_H + ROW_H / 2.0)
}

fn send(h: &Harness, phase: PointerPhase, button: Option<MouseButton>, (x, y): (f32, f32)) {
    let sink = h.shared.pointer_sink.borrow().clone().expect("the menu rows keep a pointer source installed");
    sink.event(&AppPointerEvent { button, ..AppPointerEvent::new(phase, x, y) });
    h.flush();
}

fn counter() -> (Rc<Cell<u32>>, Rc<dyn Fn()>) {
    let n = Rc::new(Cell::new(0u32));
    let n2 = n.clone();
    (n, Rc::new(move || n2.set(n2.get() + 1)))
}

fn menu(rename: Rc<dyn Fn()>, delete: Rc<dyn Fn()>) -> Element {
    let trigger: Ref<PressableHandle> = Ref::new();
    ui! {
        Menu(target = Some(AnchorTarget::from(trigger))) {
            MenuItem(label = "Rename", on_select = rename)
            MenuItem(label = "Delete", on_select = delete)
        }
    }
}

const TRIGGER: (f32, f32) = (120.0, 80.0);

#[test]
fn regression_press_dragged_onto_a_menu_row_and_released_selects_it() {
    let (renames, rename) = counter();
    let (deletes, delete) = counter();
    let (h, realized) = mount(|| menu(rename, delete));
    place_rows(&h, &["Rename", "Delete"]);

    // The press is held on the trigger; web reports its `Down` after the
    // menu mounted. Then the drag, across the first row onto the second.
    send(&h, PointerPhase::Down, Some(MouseButton::Primary), TRIGGER);
    send(&h, PointerPhase::Move, None, over_row(0));
    send(&h, PointerPhase::Move, None, over_row(1));
    assert_eq!((renames.get(), deletes.get()), (0, 0), "nothing fires while the press is held");
    send(&h, PointerPhase::Up, Some(MouseButton::Primary), over_row(1));
    assert_eq!((renames.get(), deletes.get()), (0, 1), "the row under the release is picked, once");
    drop(realized);
}

#[test]
fn releasing_on_the_trigger_selects_nothing() {
    let (renames, rename) = counter();
    let (deletes, delete) = counter();
    let (h, realized) = mount(|| menu(rename, delete));
    place_rows(&h, &["Rename", "Delete"]);

    send(&h, PointerPhase::Move, None, over_row(0));
    send(&h, PointerPhase::Move, None, TRIGGER);
    send(&h, PointerPhase::Up, Some(MouseButton::Primary), TRIGGER);
    assert_eq!((renames.get(), deletes.get()), (0, 0));
    drop(realized);
}

/// An ordinary click on a row goes through the row's own pressable. The
/// drag path must stay out of it, or one click selects twice.
#[test]
fn regression_a_click_on_a_row_is_not_also_a_drag_pick() {
    let (renames, rename) = counter();
    let (_deletes, delete) = counter();
    let (h, realized) = mount(|| menu(rename, delete));
    place_rows(&h, &["Rename", "Delete"]);

    send(&h, PointerPhase::Move, None, over_row(0));
    send(&h, PointerPhase::Down, Some(MouseButton::Primary), over_row(0));
    send(&h, PointerPhase::Move, None, over_row(0));
    send(&h, PointerPhase::Up, Some(MouseButton::Primary), over_row(0));
    assert_eq!(renames.get(), 0, "the drag path leaves a row's own click to its pressable");
    drop(realized);
}

/// A row scrolled out of a height-capped panel still has a rect, below the
/// panel. Releasing there — over whatever else is on screen — must not pick
/// a row nobody can see.
#[test]
fn regression_a_row_outside_its_panel_is_not_under_the_pointer() {
    let (renames, rename) = counter();
    let (deletes, delete) = counter();
    let (h, realized) = mount(|| menu(rename, delete));
    place_rows(&h, &["Rename", "Delete"]);
    // Push "Delete" below the panel's bottom edge.
    let hidden = ViewportRect { x: PANEL.x, y: PANEL.y + PANEL.height + 10.0, width: PANEL.width, height: ROW_H };
    h.place(row(&h, "Delete"), hidden);
    let at = (hidden.x + 20.0, hidden.y + ROW_H / 2.0);

    send(&h, PointerPhase::Move, None, at);
    send(&h, PointerPhase::Up, Some(MouseButton::Primary), at);
    assert_eq!((renames.get(), deletes.get()), (0, 0));
    drop(realized);
}

/// While a press is held no view gets hover, so a SubMenu row has to open
/// its flyout from the followed pointer, keep it open as the drag moves
/// into it, and let the release pick a flyout row.
#[test]
fn press_dragged_through_a_submenu_picks_a_flyout_row() {
    let (picks, pick) = counter();
    let (h, realized) = mount(move || {
        let trigger: Ref<PressableHandle> = Ref::new();
        ui! {
            Menu(target = Some(AnchorTarget::from(trigger))) {
                MenuItem(label = "Rename")
                SubMenu(label = "Move to", items = vec![MenuEntry::new("Archive", pick.clone())])
            }
        }
    });
    place_rows(&h, &["Rename", "Move to"]);

    send(&h, PointerPhase::Move, None, over_row(1));
    // The flyout mounted: place it (and its panel) beside the parent.
    let flyout_row_rect = ViewportRect { x: PANEL.x + PANEL.width, y: PANEL.y + ROW_H, width: 200.0, height: ROW_H };
    let views: Vec<Node> =
        h.shared.kinds.borrow().iter().filter(|(_, k)| *k == "view").map(|(n, _)| *n).collect();
    for v in views {
        h.place(v, ViewportRect { x: PANEL.x, y: PANEL.y, width: PANEL.width + 200.0, height: PANEL.height });
    }
    h.place(row(&h, "Archive"), flyout_row_rect);
    let at = (flyout_row_rect.x + 20.0, flyout_row_rect.y + ROW_H / 2.0);

    send(&h, PointerPhase::Move, None, at);
    send(&h, PointerPhase::Up, Some(MouseButton::Primary), at);
    assert_eq!(picks.get(), 1, "the flyout row under the release is picked");
    drop(realized);
}

/// A finger. The app-level pointer source carries no touches, so the
/// press-down trigger reports where the held finger goes
/// (`runtime_shared::held_press`) and the rows follow that instead. Before
/// that channel existed, dragging a touch into the menu did nothing.
#[test]
fn regression_finger_dragged_onto_a_menu_row_and_lifted_selects_it() {
    use runtime_shared::held_press::{report, HeldPressPhase};
    let (renames, rename) = counter();
    let (deletes, delete) = counter();
    let (h, realized) = mount(|| menu(rename, delete));
    place_rows(&h, &["Rename", "Delete"]);

    let (x0, y0) = over_row(0);
    let (x1, y1) = over_row(1);
    report(HeldPressPhase::Moved, x0, y0);
    report(HeldPressPhase::Moved, x1, y1);
    h.flush();
    assert_eq!((renames.get(), deletes.get()), (0, 0), "nothing fires while the finger is down");
    report(HeldPressPhase::Released, x1, y1);
    h.flush();
    assert_eq!((renames.get(), deletes.get()), (0, 1), "the row under the lift is picked, once");
    drop(realized);
}

#[test]
fn a_cancelled_finger_press_picks_nothing() {
    use runtime_shared::held_press::{report, HeldPressPhase};
    let (renames, rename) = counter();
    let (deletes, delete) = counter();
    let (h, realized) = mount(|| menu(rename, delete));
    place_rows(&h, &["Rename", "Delete"]);

    let (x, y) = over_row(0);
    report(HeldPressPhase::Moved, x, y);
    report(HeldPressPhase::Cancelled, x, y);
    report(HeldPressPhase::Released, x, y);
    h.flush();
    assert_eq!((renames.get(), deletes.get()), (0, 0));
    drop(realized);
}

/// An iPad mouse is both a touch contact and a pointer: the same press
/// arrives on both channels and must still pick once.
#[test]
fn regression_a_press_on_both_channels_picks_once() {
    use runtime_shared::held_press::{report, HeldPressPhase};
    let (renames, rename) = counter();
    let (_deletes, delete) = counter();
    let (h, realized) = mount(|| menu(rename, delete));
    place_rows(&h, &["Rename", "Delete"]);

    let at = over_row(0);
    send(&h, PointerPhase::Move, None, at);
    report(HeldPressPhase::Moved, at.0, at.1);
    send(&h, PointerPhase::Up, Some(MouseButton::Primary), at);
    report(HeldPressPhase::Released, at.0, at.1);
    h.flush();
    assert_eq!(renames.get(), 1);
    drop(realized);
}

/// On a narrow screen a SubMenu's flyout flips and OVERLAPS the parent
/// menu, so a held finger travelling from the SubMenu row is over the row
/// and the flyout at once before it is over the flyout alone. "Entered the
/// flyout" fires while it is still on the row; then "left the row" starts
/// the hover-out grace timer, and no further enter comes to cancel it —
/// the flyout closed under the finger 120 ms later. The timer must not
/// close a flyout the pointer is still over.
#[test]
fn regression_flyout_overlapping_its_row_stays_open_under_the_finger() {
    use runtime_shared::held_press::{report, HeldPressPhase};
    host_mock::pump::install_scheduler();
    let (picks, pick) = counter();
    let (h, realized) = mount(move || {
        let trigger: Ref<PressableHandle> = Ref::new();
        ui! {
            Menu(target = Some(AnchorTarget::from(trigger))) {
                MenuItem(label = "Rename")
                SubMenu(label = "Move to", items = vec![MenuEntry::new("Archive", pick.clone())])
            }
        }
    });
    place_rows(&h, &["Rename", "Move to"]);
    let (rx, ry) = over_row(1);
    report(HeldPressPhase::Moved, rx, ry);
    h.flush();

    // The flyout mounted. It overlaps the parent panel's right 40 px; its
    // one row sits level with the SubMenu row.
    const OVERLAP: f32 = 40.0;
    let flyout = ViewportRect { x: PANEL.x + PANEL.width - OVERLAP, y: PANEL.y, width: 200.0, height: PANEL.height };
    let archive = ViewportRect { x: flyout.x, y: PANEL.y + ROW_H, width: flyout.width, height: ROW_H };
    let views: Vec<Node> =
        h.shared.kinds.borrow().iter().filter(|(_, k)| *k == "view").map(|(n, _)| *n).collect();
    for v in views {
        h.place(v, ViewportRect { x: PANEL.x, y: PANEL.y, width: flyout.x + flyout.width - PANEL.x, height: PANEL.height });
    }
    h.place(row(&h, "Archive"), archive);
    let y = PANEL.y + ROW_H + ROW_H / 2.0;
    let on_both = (PANEL.x + PANEL.width - OVERLAP / 2.0, y);
    let flyout_only = (PANEL.x + PANEL.width + 50.0, y);

    report(HeldPressPhase::Moved, on_both.0, on_both.1);
    h.flush();
    report(HeldPressPhase::Moved, flyout_only.0, flyout_only.1);
    h.flush();
    // The hover-out grace runs out with the finger resting in the flyout.
    host_mock::pump::pump_timers();
    h.flush();

    report(HeldPressPhase::Released, flyout_only.0, flyout_only.1);
    h.flush();
    assert_eq!(picks.get(), 1, "the flyout was still there to pick from");
    drop(realized);
}

/// Where a flipped flyout overlaps the parent menu, the parent's rows are
/// covered. A release there picks the flyout row on top — and must not
/// also pick the parent row hidden beneath it (releasing on "Archive"
/// also ran "Delete").
#[test]
fn regression_a_parent_row_covered_by_the_flyout_is_not_picked() {
    use runtime_shared::held_press::{report, HeldPressPhase};
    let (archives, archive_pick) = counter();
    let (deletes, delete) = counter();
    let (h, realized) = mount(move || {
        let trigger: Ref<PressableHandle> = Ref::new();
        ui! {
            Menu(target = Some(AnchorTarget::from(trigger))) {
                SubMenu(label = "Move to", items = vec![MenuEntry::new("Archive", archive_pick.clone())])
                MenuItem(label = "Delete", on_select = delete)
            }
        }
    });
    place_rows(&h, &["Move to", "Delete"]);
    let parent_views: Vec<Node> =
        h.shared.kinds.borrow().iter().filter(|(_, k)| *k == "view").map(|(n, _)| *n).collect();
    let (rx, ry) = over_row(0);
    report(HeldPressPhase::Moved, rx, ry);
    h.flush();

    // The flyout overlaps the parent's right 40 px, and its row sits level
    // with the parent's "Delete" row.
    const OVERLAP: f32 = 40.0;
    let flyout = ViewportRect { x: PANEL.x + PANEL.width - OVERLAP, y: PANEL.y, width: 200.0, height: PANEL.height };
    let new_views: Vec<Node> = h
        .shared
        .kinds
        .borrow()
        .iter()
        .filter(|(n, k)| *k == "view" && !parent_views.contains(n))
        .map(|(n, _)| *n)
        .collect();
    for v in new_views {
        h.place(v, flyout);
    }
    h.place(row(&h, "Archive"), ViewportRect { x: flyout.x, y: PANEL.y + ROW_H, width: flyout.width, height: ROW_H });
    let in_overlap = (PANEL.x + PANEL.width - OVERLAP / 2.0, PANEL.y + ROW_H + ROW_H / 2.0);

    report(HeldPressPhase::Moved, in_overlap.0, in_overlap.1);
    report(HeldPressPhase::Released, in_overlap.0, in_overlap.1);
    h.flush();
    assert_eq!((archives.get(), deletes.get()), (1, 0), "only the row on top is picked");
    drop(realized);
}
