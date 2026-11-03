//! An `ItemSize::Known` list is never measured: its sizes are
//! authoritative, so the GTK virtualizer must not call `set_measured_size`
//! and must space its rows by `item_size` even when a row's content renders
//! at a different size.
//!
//! The rows here hold 70px of content under a known size of 40px. Were the
//! measure path gated on anything but `callbacks.measure_sizes`, the rows
//! would report 70 and be re-spaced; instead they sit at their 40px slots
//! (content overflowing, as the author asked) and the document extent is
//! `count × 40`. A scroll that mounts new rows is checked too.
//!
//! One GTK `#[test]` in this binary: GTK may only be driven from the
//! thread that ran `gtk::init`. Skips with no display.

#![cfg(target_os = "linux")]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use backend_linux::{gtk4, LinuxBackend, LinuxNode};
use gtk4::prelude::*;
use runtime_scene::Host;
use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::primitives::virtualizer::VirtualLayout;
use runtime_shared::{Length, StyleRules, Tokenized, VirtualizerCallbacks};
use runtime_vocabulary::caps::{LifecycleOps, StyleOps, ViewOps, VirtualizerOps};

const KNOWN: f32 = 40.0;
const CONTENT: f32 = 70.0;
const COUNT: usize = 30;

fn px(v: f32) -> Option<Tokenized<Length>> {
    Some(Tokenized::Literal(Length::Px(v)))
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
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    for _ in 0..200 {
        ctx.iteration(false);
    }
}

#[test]
fn known_size_list_never_calls_set_measured_size() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display / GTK init failed");
        return;
    }

    let window = gtk4::Window::new();
    window.set_default_size(400, 400);
    let backend = Rc::new(RefCell::new(LinuxBackend::new(window.clone())));
    backend.borrow_mut().set_self_ref(Rc::downgrade(&backend));
    let a11y = AccessibilityProps::default();

    let rows: Rc<RefCell<HashMap<usize, LinuxNode>>> = Rc::new(RefCell::new(HashMap::new()));
    let reports: Rc<RefCell<Vec<(u64, f32)>>> = Rc::new(RefCell::new(Vec::new()));

    let callbacks = VirtualizerCallbacks {
        item_count: Rc::new(|| COUNT),
        item_key: Rc::new(|i| i as u64),
        item_size: Rc::new(|_| KNOWN),
        measure_sizes: false,
        mount_item: {
            let backend = backend.clone();
            let rows = rows.clone();
            Rc::new(move |idx| {
                let a11y = AccessibilityProps::default();
                let mut b = backend.borrow_mut();
                let mut root = b.create_view(&a11y);
                let content = b.create_view(&a11y);
                b.apply_style(&content, &Rc::new(StyleRules { height: px(CONTENT), ..StyleRules::default() }));
                b.insert(&mut root, content);
                rows.borrow_mut().insert(idx, root.clone());
                (root, idx as u64)
            })
        },
        release_item: Rc::new(|_| {}),
        set_measured_size: {
            let reports = reports.clone();
            Rc::new(move |scope, size| reports.borrow_mut().push((scope, size)))
        },
        on_scroll: None,
        item_changed: None,
    };

    let mut root = backend.borrow_mut().create_view(&a11y);
    let list = backend.borrow_mut().create_virtualizer(callbacks, 0.0, VirtualLayout::default(), &a11y);
    backend.borrow_mut().apply_style(
        &list,
        &Rc::new(StyleRules { width: px(200.0), height: px(200.0), ..StyleRules::default() }),
    );
    backend.borrow_mut().insert(&mut root, list.clone());
    backend.borrow_mut().finish(root.clone());
    window.set_child(Some(root.widget()));
    window.present();

    // [0, 200) at 40px: rows 0..5.
    pump(|| rows.borrow().len() >= 5 && rows.borrow()[&0].widget().is_mapped());
    if !root.widget().is_mapped() {
        eprintln!("SKIP: window never mapped in this environment");
        return;
    }

    let frame = |i: usize| backend.borrow().node_frame(rows.borrow()[&i].id());
    for i in 0..5 {
        let (_, y, _, h) = frame(i).expect("row framed");
        assert_eq!((y, h), (i as f32 * KNOWN, KNOWN), "row {i} sits in its known 40px slot");
    }
    let document = rows.borrow()[&0].widget().parent().expect("rows are parented into the document");
    assert_eq!(document.size_request().1, (COUNT as f32 * KNOWN) as i32, "extent is count × known size");

    // Mount more rows by scrolling; still nothing measured.
    let scrolled = list.widget().downcast_ref::<gtk4::ScrolledWindow>().expect("a ScrolledWindow").clone();
    let adj = scrolled.vadjustment();
    // The scroll range follows the document's size request one allocation
    // later; a value set before that is clamped to the old range.
    pump(|| adj.upper() as f32 == COUNT as f32 * KNOWN);
    adj.set_value(400.0);
    pump(|| rows.borrow().contains_key(&10));
    assert!(
        rows.borrow().contains_key(&10),
        "scrolling mounted row 10 (adjustment value {} upper {} page {}; mounted {:?})",
        adj.value(),
        adj.upper(),
        adj.page_size(),
        rows.borrow().keys().collect::<Vec<_>>()
    );
    let (_, y, _, h) = frame(10).expect("row 10 framed");
    assert_eq!((y, h), (10.0 * KNOWN, KNOWN), "row 10 sits in its known slot");

    assert!(reports.borrow().is_empty(), "a Known list called set_measured_size: {:?}", reports.borrow());
    window.close();
}
