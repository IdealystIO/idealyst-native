//! A `z_index` on a virtualized row's root orders that ROW against the
//! neighbouring rows, for paint AND for picking — as on web, where a
//! virtualized list's rows are plain siblings in one container.
//!
//! ## The bug this pins
//!
//! The GTK virtualizer parents each row root straight into its `GtkFixed`
//! document, which paints in sibling order and picks last-sibling-first.
//! A row root is a detached Taffy root with no `parent_of` entry, so the
//! backend's sibling restack never ran for it: the document stayed in
//! MOUNT order whatever the rows' `z_index`, a restyle or an animated
//! `ZIndex` changed nothing, and scrolling back up put the re-mounted
//! earlier row above its later neighbours.
//!
//! Pinned alongside: every row sits in its own slot, and stays there across
//! later layout passes. The detached layout pass — and every main layout
//! pass, which re-frames detached roots too — framed each row root at
//! `(0, 0)` and wrote that as its `GtkFixed` child transform, replacing the
//! slot `Fixed::put` gave it, so every row painted (and picked) over row 0.
//!
//! ## Shape
//!
//! Each row is a 40px view whose pressable child spills 20px into the next
//! row (an open dropdown in a row is the real-world case). In the spill,
//! whichever row stacks higher takes the pick. Checked: z set at mount,
//! raised and lowered by a restyle, raised by an animated `ZIndex`, and a
//! scroll that unmounts and re-mounts row 0 — the fresh row comes back at
//! its style's z, in row order.
//!
//! The callbacks are hand-built (no vocabulary) so the test holds each
//! row's node and drives `apply_style` / `set_animated_f32` on it directly,
//! like `z_index_stacking.rs`. One GTK `#[test]` in this binary: GTK may
//! only be driven from the thread that ran `gtk::init`, and `pick` needs a
//! mapped widget, hence a presented window. Skips with no display.

#![cfg(target_os = "linux")]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use backend_linux::{gtk4, LinuxBackend, LinuxNode};
use gtk4::prelude::*;
use runtime_scene::Host;
use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::animation::AnimProp;
use runtime_shared::primitives::virtualizer::VirtualLayout;
use runtime_shared::{Length, Position, StyleRules, Tokenized, VirtualizerCallbacks};
use runtime_vocabulary::caps::{
    AnimationOps, LifecycleOps, PressableOps, StyleOps, ViewOps, VirtualizerOps,
};

const ROW_H: f32 = 40.0;
const SPILL: f32 = 20.0;
const LIST_H: f32 = 240.0;
const COUNT: usize = 30;

fn px(v: f32) -> Option<Tokenized<Length>> {
    Some(Tokenized::Literal(Length::Px(v)))
}

fn row_style(z_index: Option<i32>) -> Rc<StyleRules> {
    Rc::new(StyleRules { height: px(ROW_H), z_index, ..StyleRules::default() })
}

/// The pressable that hangs `SPILL` px below its row.
fn spill_style() -> Rc<StyleRules> {
    Rc::new(StyleRules {
        position: Some(Position::Absolute),
        left: px(0.0),
        top: px(ROW_H - SPILL),
        width: px(100.0),
        height: px(ROW_H),
        ..StyleRules::default()
    })
}

fn pump(until: impl Fn() -> bool) {
    let ctx = gtk4::glib::MainContext::default();
    for _ in 0..400 {
        for _ in 0..200 {
            ctx.iteration(false);
        }
        if until() {
            break;
        }
        // Rows mount from the adjustments, i.e. after a frame-clock tick,
        // which needs wall time, not just main-loop spins.
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    for _ in 0..200 {
        ctx.iteration(false);
    }
}

/// Let a restack / restyle settle: a queued allocate + draw.
fn settle() {
    let ctx = gtk4::glib::MainContext::default();
    for _ in 0..10 {
        for _ in 0..200 {
            ctx.iteration(false);
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn siblings(parent: &gtk4::Widget) -> Vec<gtk4::Widget> {
    let mut out = Vec::new();
    let mut c = parent.first_child();
    while let Some(w) = c {
        c = w.next_sibling();
        out.push(w);
    }
    out
}

#[test]
fn regression_virtualizer_row_z_index_orders_rows_for_paint_and_pick() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display / GTK init failed");
        return;
    }

    let window = gtk4::Window::new();
    window.set_default_size(400, 400);
    let backend = Rc::new(RefCell::new(LinuxBackend::new(window.clone())));
    backend.borrow_mut().set_self_ref(Rc::downgrade(&backend));
    let a11y = AccessibilityProps::default();

    // The z each row root is styled with WHEN MOUNTED; row 3 starts raised.
    let mount_z: Rc<RefCell<HashMap<usize, i32>>> = Rc::new(RefCell::new(HashMap::from([(3, 1)])));
    // index → the row root currently mounted for it (re-mounts replace it).
    let rows: Rc<RefCell<HashMap<usize, LinuxNode>>> = Rc::new(RefCell::new(HashMap::new()));

    let callbacks = VirtualizerCallbacks {
        item_count: Rc::new(|| COUNT),
        item_key: Rc::new(|i| i as u64),
        item_size: Rc::new(|_| ROW_H),
        measure_sizes: false,
        mount_item: {
            let backend = backend.clone();
            let rows = rows.clone();
            let mount_z = mount_z.clone();
            Rc::new(move |idx| {
                let a11y = AccessibilityProps::default();
                let mut b = backend.borrow_mut();
                let mut root = b.create_view(&a11y);
                b.apply_style(&root, &row_style(mount_z.borrow().get(&idx).copied()));
                let spill = b.create_pressable(Rc::new(|| {}), &a11y);
                b.apply_style(&spill, &spill_style());
                b.insert(&mut root, spill);
                rows.borrow_mut().insert(idx, root.clone());
                (root, idx as u64)
            })
        },
        release_item: Rc::new(|_| {}),
        set_measured_size: Rc::new(|_, _| {}),
        on_scroll: None,
        item_changed: None,
    };

    let mut root = backend.borrow_mut().create_view(&a11y);
    let list =
        backend.borrow_mut().create_virtualizer(callbacks, 0.0, VirtualLayout::default(), &a11y);
    backend.borrow_mut().apply_style(
        &list,
        &Rc::new(StyleRules { width: px(200.0), height: px(LIST_H), ..StyleRules::default() }),
    );
    backend.borrow_mut().insert(&mut root, list.clone());
    backend.borrow_mut().finish(root.clone());

    window.set_child(Some(root.widget()));
    window.present();
    // Window [0, 240) with no overscan: rows 0..6.
    pump(|| rows.borrow().len() >= 6 && rows.borrow().get(&0).is_some_and(|n| n.widget().is_mapped()));
    if !root.widget().is_mapped() {
        eprintln!("SKIP: window never mapped in this environment");
        return;
    }
    assert!(rows.borrow().len() >= 6, "the virtualizer mounted {} rows", rows.borrow().len());

    let scrolled = list.widget().downcast_ref::<gtk4::ScrolledWindow>().expect("a ScrolledWindow").clone();
    let document = rows.borrow()[&0].widget().parent().expect("a row is parented into the document");
    let row = |i: usize| rows.borrow()[&i].widget().clone();
    // The rows in the document's sibling order (= paint order; pick runs
    // in reverse), named by index.
    let order = || -> Vec<usize> {
        let by_widget = rows.borrow();
        siblings(&document)
            .iter()
            .filter_map(|w| by_widget.iter().find(|(_, n)| n.widget() == w).map(|(i, _)| *i))
            .collect()
    };
    // Which row takes the pick in the band where row `upper`'s spill lies
    // over row `upper + 1` (document coordinates).
    let picked_in_spill_of = |upper: usize| -> Option<usize> {
        let y = (upper + 1) as f64 * ROW_H as f64 + SPILL as f64 / 2.0;
        let hit = document.pick(10.0, y, gtk4::PickFlags::DEFAULT)?;
        let by_widget = rows.borrow();
        by_widget.iter().find(|(_, n)| hit == *n.widget() || hit.is_ancestor(n.widget())).map(|(i, _)| *i)
    };

    // 0. Every row sits in its own slot, also after a main layout pass. A
    //    layout pass writes the row root's frame as its `GtkFixed` child
    //    transform, which replaces the slot `Fixed::put` set unless the slot
    //    is part of the frame — then every row lands at y 0.
    let row_y = |i: usize| {
        row(i).compute_point(&document, &gtk4::graphene::Point::new(0.0, 0.0)).map(|p| p.y())
    };
    let assert_slots = |when: &str| {
        for i in 0..6 {
            assert_eq!(row_y(i), Some(i as f32 * ROW_H), "{when}: row {i} sits at its slot");
        }
    };
    assert_slots("after mount");
    // A main layout pass walks every node, detached rows included; read the
    // frames it wrote straight away, before any scroll/resize re-windows.
    backend.borrow_mut().run_layout(400.0, 400.0);
    for i in 0..6 {
        let id = rows.borrow()[&i].id();
        let y = backend.borrow().node_frame(id).map(|f| f.1);
        assert_eq!(y, Some(i as f32 * ROW_H), "a main layout pass kept row {i} at its slot");
    }
    settle();
    assert_slots("after a main layout pass");

    // 1. z set at mount: row 3 stacks above every row, the rest in row order.
    assert_eq!(order(), [0, 1, 2, 4, 5, 3], "row 3 mounted at z 1 paints last");
    assert_eq!(picked_in_spill_of(3), Some(3), "raised row 3's spill takes the pick over row 4");
    assert_eq!(picked_in_spill_of(1), Some(2), "unraised: the later row takes the pick");

    // 2. A restyle raises row 1 above its neighbours…
    backend.borrow_mut().apply_style(&rows.borrow()[&1].clone(), &row_style(Some(2)));
    settle();
    assert_eq!(order(), [0, 2, 4, 5, 3, 1]);
    assert_eq!(picked_in_spill_of(1), Some(1), "restyled to z 2, row 1's spill takes the pick");
    //    …and a restyle without `z_index` puts it back in row order.
    backend.borrow_mut().apply_style(&rows.borrow()[&1].clone(), &row_style(None));
    settle();
    assert_eq!(order(), [0, 1, 2, 4, 5, 3]);
    assert_eq!(picked_in_spill_of(1), Some(2));

    // 3. Animated `ZIndex` raises row 0.
    let row0 = rows.borrow()[&0].clone();
    backend.borrow_mut().set_animated_f32(&row0, AnimProp::ZIndex, 5.0);
    settle();
    assert_eq!(order(), [1, 2, 4, 5, 3, 0]);
    assert_eq!(picked_in_spill_of(0), Some(0), "animated to z 5, row 0's spill takes the pick");

    // 4. Scroll row 0 out ([40, 280): rows 1..7), then back: the re-mounted
    //    row 0 is a fresh node at its style's z (none), and it stacks by ROW
    //    order even though it mounted after every other row.
    let adj = scrolled.vadjustment();
    adj.set_value(ROW_H as f64);
    pump(|| !row0.widget().is_ancestor(&document) && rows.borrow().contains_key(&6));
    assert!(!row0.widget().is_ancestor(&document), "row 0 left the document when scrolled out");
    adj.set_value(0.0);
    pump(|| rows.borrow()[&0].widget().parent().as_ref() == Some(&document));
    assert_ne!(row(0), *row0.widget(), "row 0 was re-mounted as a fresh node");
    assert_eq!(
        siblings(&document).first(),
        Some(&row(0)),
        "the re-mounted row 0 stacks below its later neighbours (row order, not mount order)"
    );
    assert_eq!(siblings(&document).last(), Some(&row(3)), "row 3 still stacks on top at z 1");
    assert_eq!(picked_in_spill_of(0), Some(1), "the re-mounted row 0 did not keep its old z");

    window.close();
}
