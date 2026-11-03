//! A `flat_list` with `ItemSize::Measured(estimate)` spaces its rows by
//! each row's REAL rendered size, not by the estimate.
//!
//! ## The bug this pins
//!
//! The GTK virtualizer laid every row out from `item_size` and never
//! measured a mounted row, so a Measured list kept the author's estimate
//! forever: idea-ui `TableCards` estimates cards at 132pt, the cards render
//! at 200pt, and every card overlapped the next by 58pt. Rows whose content
//! is shorter than the estimate left gaps the same way, and the scroll
//! extent and the visible window were computed from the estimate too.
//!
//! ## Shape
//!
//! Each row is an auto-sized view holding one child of a fixed main-axis
//! size — the row's real size. The callbacks are hand-built (no
//! vocabulary) but store measured sizes exactly as the vocabulary does: by
//! item key, layered over the estimate in `item_size`. Three lists:
//!
//! 1. vertical, with a main-axis gap, rows taller AND shorter than the
//!    estimate, all in view: row offsets, row heights and the document
//!    extent follow the real sizes; each row reports once (converged).
//! 2. vertical, 20 rows taller than the estimate: the visible window
//!    shrinks to the rows that really fit, and the scroll extent counts
//!    measured rows at their real size and unmeasured ones at the estimate.
//! 3. horizontal: the same, along x.
//!
//! One GTK `#[test]` in this binary: GTK may only be driven from the
//! thread that ran `gtk::init`. Skips with no display.

#![cfg(target_os = "linux")]

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

use backend_linux::{gtk4, LinuxBackend, LinuxNode};
use gtk4::prelude::*;
use runtime_scene::Host;
use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::primitives::virtualizer::{Axis, VirtualLayout};
use runtime_shared::{Length, StyleRules, Tokenized, VirtualizerCallbacks};
use runtime_vocabulary::caps::{LifecycleOps, StyleOps, ViewOps, VirtualizerOps};

const ESTIMATE: f32 = 40.0;

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

/// What one Measured list's callbacks observed.
#[derive(Default)]
struct Probe {
    /// index → the row root currently mounted for it.
    mounted: HashMap<usize, LinuxNode>,
    /// Every `set_measured_size` call, as `(index, size)`.
    reports: Vec<(usize, f32)>,
}

/// A Measured virtualizer whose row `i` renders at `real(i)` along the
/// main axis while estimating `ESTIMATE`.
fn measured_list(
    backend: &Rc<RefCell<LinuxBackend>>,
    count: usize,
    horizontal: bool,
    real: fn(usize) -> f32,
    layout: VirtualLayout,
) -> (LinuxNode, Rc<RefCell<Probe>>) {
    let probe = Rc::new(RefCell::new(Probe::default()));
    // Keyed by item key (= index here), like the vocabulary's override store.
    let measured: Rc<RefCell<HashMap<usize, f32>>> = Rc::new(RefCell::new(HashMap::new()));
    let scope_to_idx: Rc<RefCell<HashMap<u64, usize>>> = Rc::new(RefCell::new(HashMap::new()));
    let next_scope = Rc::new(RefCell::new(0u64));

    let callbacks = VirtualizerCallbacks {
        item_count: Rc::new(move || count),
        item_key: Rc::new(|i| i as u64),
        item_size: {
            let measured = measured.clone();
            Rc::new(move |i| measured.borrow().get(&i).copied().unwrap_or(ESTIMATE))
        },
        measure_sizes: true,
        mount_item: {
            let backend = backend.clone();
            let probe = probe.clone();
            let scope_to_idx = scope_to_idx.clone();
            Rc::new(move |idx| {
                let a11y = AccessibilityProps::default();
                let mut b = backend.borrow_mut();
                // Auto-sized row root: its size comes from its content.
                let mut root = b.create_view(&a11y);
                let content = b.create_view(&a11y);
                let style = if horizontal {
                    StyleRules { width: px(real(idx)), ..StyleRules::default() }
                } else {
                    StyleRules { height: px(real(idx)), ..StyleRules::default() }
                };
                b.apply_style(&content, &Rc::new(style));
                b.insert(&mut root, content);
                let scope = {
                    let mut n = next_scope.borrow_mut();
                    *n += 1;
                    *n
                };
                scope_to_idx.borrow_mut().insert(scope, idx);
                probe.borrow_mut().mounted.insert(idx, root.clone());
                (root, scope)
            })
        },
        release_item: {
            let probe = probe.clone();
            let scope_to_idx = scope_to_idx.clone();
            Rc::new(move |scope| {
                if let Some(idx) = scope_to_idx.borrow_mut().remove(&scope) {
                    probe.borrow_mut().mounted.remove(&idx);
                }
            })
        },
        set_measured_size: {
            let probe = probe.clone();
            Rc::new(move |scope, size| {
                let idx = scope_to_idx.borrow()[&scope];
                measured.borrow_mut().insert(idx, size);
                probe.borrow_mut().reports.push((idx, size));
            })
        },
        on_scroll: None,
        item_changed: None,
    };
    let a11y = AccessibilityProps::default();
    let list = backend.borrow_mut().create_virtualizer(callbacks, 0.0, layout, &a11y);
    (list, probe)
}

/// `(main offset, main size)` of row `i`'s root frame.
fn row_main(backend: &Rc<RefCell<LinuxBackend>>, probe: &Rc<RefCell<Probe>>, i: usize, horizontal: bool) -> Option<(f32, f32)> {
    let id = probe.borrow().mounted.get(&i)?.id();
    let (x, y, w, h) = backend.borrow().node_frame(id)?;
    Some(if horizontal { (x, w) } else { (y, h) })
}

/// The virtualizer's document (`GtkFixed`) — a mounted row's parent.
fn document(probe: &Rc<RefCell<Probe>>) -> gtk4::Widget {
    let p = probe.borrow();
    let row = p.mounted.values().next().expect("a mounted row");
    row.widget().parent().expect("rows are parented into the document")
}

fn mounted_set(probe: &Rc<RefCell<Probe>>) -> BTreeSet<usize> {
    probe.borrow().mounted.keys().copied().collect()
}

#[test]
fn regression_measured_rows_are_spaced_by_their_rendered_size() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display / GTK init failed");
        return;
    }

    let window = gtk4::Window::new();
    window.set_default_size(900, 500);
    let backend = Rc::new(RefCell::new(LinuxBackend::new(window.clone())));
    backend.borrow_mut().set_self_ref(Rc::downgrade(&backend));
    let a11y = AccessibilityProps::default();

    let sized = |w: f32, h: f32| Rc::new(StyleRules { width: px(w), height: px(h), ..StyleRules::default() });

    // 1. Vertical, gap 10, rows 0/2/3/4 taller than the estimate and row 1
    //    shorter; 5 rows fit in a 400px viewport either way.
    const GAP: f32 = 10.0;
    fn real_a(i: usize) -> f32 {
        if i == 1 { 25.0 } else { 70.0 }
    }
    let (list_a, probe_a) = measured_list(
        &backend,
        5,
        false,
        real_a,
        VirtualLayout { main_spacing: GAP, ..VirtualLayout::default() },
    );
    backend.borrow_mut().apply_style(&list_a, &sized(200.0, 400.0));

    // 2. Vertical, 20 rows of 100 under a 40 estimate, 250px viewport.
    fn real_b(_: usize) -> f32 {
        100.0
    }
    let (list_b, probe_b) = measured_list(&backend, 20, false, real_b, VirtualLayout::default());
    backend.borrow_mut().apply_style(&list_b, &sized(200.0, 250.0));

    // 3. Horizontal, 3 columns.
    fn real_c(i: usize) -> f32 {
        [90.0, 20.0, 60.0][i]
    }
    let (list_c, probe_c) = measured_list(
        &backend,
        3,
        true,
        real_c,
        VirtualLayout { axis: Axis::Horizontal, ..VirtualLayout::default() },
    );
    backend.borrow_mut().apply_style(&list_c, &sized(300.0, 80.0));

    let mut root = backend.borrow_mut().create_view(&a11y);
    backend.borrow_mut().apply_style(
        &root,
        &Rc::new(StyleRules {
            flex_direction: Some(runtime_shared::FlexDirection::Row),
            ..StyleRules::default()
        }),
    );
    for list in [&list_a, &list_b, &list_c] {
        backend.borrow_mut().insert(&mut root, list.clone());
    }
    backend.borrow_mut().finish(root.clone());
    window.set_child(Some(root.widget()));
    window.present();

    let expected_a: [(f32, f32); 5] = [(0.0, 70.0), (80.0, 25.0), (115.0, 70.0), (195.0, 70.0), (275.0, 70.0)];
    let settled_a = || (0..5).all(|i| row_main(&backend, &probe_a, i, false) == Some(expected_a[i]));
    let settled_b = || mounted_set(&probe_b) == BTreeSet::from([0, 1, 2]);
    let expected_c: [(f32, f32); 3] = [(0.0, 90.0), (90.0, 20.0), (110.0, 60.0)];
    let settled_c = || (0..3).all(|i| row_main(&backend, &probe_c, i, true) == Some(expected_c[i]));
    pump(|| settled_a() && settled_b() && settled_c());
    if !root.widget().is_mapped() {
        eprintln!("SKIP: window never mapped in this environment");
        return;
    }

    // ---- 1. offsets + sizes + extent follow the real sizes ----
    for (i, want) in expected_a.iter().enumerate() {
        assert_eq!(
            row_main(&backend, &probe_a, i, false),
            Some(*want),
            "list 1 row {i}: (y, height) follows the rendered sizes, not the {ESTIMATE}px estimate"
        );
    }
    // 70 + 25 + 70 + 70 + 70 + 4 gaps of 10.
    assert_eq!(document(&probe_a).size_request().1, 345, "list 1 document extent is the real total");
    let mut reports = probe_a.borrow().reports.clone();
    reports.sort_by_key(|r| r.0);
    assert_eq!(
        reports,
        (0..5).map(|i| (i, real_a(i))).collect::<Vec<_>>(),
        "each row reports its real size exactly once (re-measuring at that size reports nothing)"
    );

    // ---- 2. windowing uses measured sizes ----
    // With the estimate, [0, 250) held rows 0..7; at 100px each it holds 0..3.
    assert_eq!(mounted_set(&probe_b), BTreeSet::from([0, 1, 2]), "list 2 windows by the measured sizes");
    for i in 0..3 {
        assert_eq!(row_main(&backend, &probe_b, i, false), Some((i as f32 * 100.0, 100.0)), "list 2 row {i}");
    }
    // Rows 0..7 were measured (100 each, kept by key after rows 3..7
    // unmounted); rows 7..20 are still at the estimate.
    let reported_b: BTreeSet<usize> = probe_b.borrow().reports.iter().map(|r| r.0).collect();
    assert_eq!(reported_b, (0..7).collect::<BTreeSet<_>>(), "list 2 measured exactly the rows it mounted");
    assert_eq!(probe_b.borrow().reports.len(), 7, "list 2: no row reported twice");
    let total_b = 7.0 * 100.0 + 13.0 * ESTIMATE;
    assert_eq!(document(&probe_b).size_request().1, total_b as i32, "list 2 document extent");
    let scrolled_b = list_b.widget().downcast_ref::<gtk4::ScrolledWindow>().expect("a ScrolledWindow").clone();
    pump(|| scrolled_b.vadjustment().upper() as f32 == total_b);
    assert_eq!(scrolled_b.vadjustment().upper() as f32, total_b, "list 2 scroll range is the measured extent");

    // Scrolling into rows that were never mounted measures them on mount.
    scrolled_b.vadjustment().set_value(700.0);
    pump(|| probe_b.borrow().reports.iter().any(|r| r.0 == 7));
    for i in mounted_set(&probe_b) {
        let (_, h) = row_main(&backend, &probe_b, i, false).expect("mounted row framed");
        assert_eq!(h, 100.0, "list 2 row {i} mounted after scrolling is laid out at its real size");
    }

    // ---- 3. horizontal ----
    for (i, want) in expected_c.iter().enumerate() {
        assert_eq!(row_main(&backend, &probe_c, i, true), Some(*want), "list 3 column {i}: (x, width)");
    }
    assert_eq!(document(&probe_c).size_request().0, 170, "list 3 document extent is the real total");

    window.close();
}
