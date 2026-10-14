//! Regression: changing a virtualizer's data while it has MOUNTED rows must
//! not panic with "already borrowed", and the rows must then show the new
//! data.
//!
//! ## The bug this pins
//!
//! The vocabulary's data effect calls `backend.borrow_mut()
//! .virtualizer_data_changed(&node)` — the backend is mutably borrowed for
//! the whole call. The GTK body used to recycle every mounted cell and
//! re-window *synchronously* inside that call: `release_item` drops each
//! row's scope (a styled row's teardown re-borrows the backend to unstyle
//! its node) and `mount_item` realizes the new rows (realize borrows the
//! backend). Either re-borrow panics with `BorrowMutError`. It never showed
//! at boot because the effect's first run finds no mounted rows and an
//! unallocated scroller; it fires on the first data change after rows
//! mounted. Every other backend hops the reload off this call (web/iOS: a
//! microtask; Android/macOS: the toolkit's own deferred re-layout), which
//! is the contract the GTK backend now follows too.
//!
//! ## Shape
//!
//! A real app (`newcore::start`) renders a `virtualizer` whose count, keys
//! and rows read one `Signal<Vec<String>>`. Once rows have mounted, the data
//! is appended to, shrunk, and wholly replaced; after each change the
//! visible row labels must equal the head of the new data.
//!
//! One GTK `#[test]` in this binary: GTK may only be driven from the thread
//! that ran `gtk::init`, and cargo gives each test its own thread.

#![cfg(target_os = "linux")]

use std::cell::RefCell;
use std::rc::Rc;

use backend_linux::{gtk4, newcore, LinuxBackend};
use gtk4::prelude::*;
use runtime_shared::primitives::virtualizer::ItemSize;
use runtime_shared::{Length, StyleRules};
use runtime_vocabulary::builders::{text, view, virtualizer};

const WIN_W: f32 = 640.0;
const WIN_H: f32 = 480.0;
/// Row height and list height: the list shows 5 rows (plus overscan).
const ROW_H: f32 = 20.0;
const LIST_H: f32 = 100.0;

/// Drain the main loop and run a layout pass, as host-gtk's 16 ms pump
/// would (under `cargo test` the window gets no frame callbacks).
fn settle(ctx: &gtk4::glib::MainContext, backend: &Rc<RefCell<LinuxBackend>>) {
    for _ in 0..2_000 {
        ctx.iteration(false);
    }
    if let Ok(mut b) = backend.try_borrow_mut() {
        b.run_layout(WIN_W, WIN_H);
    }
    for _ in 0..2_000 {
        ctx.iteration(false);
    }
}

/// Texts of every row label (`item:…`) in the tree, in document order of
/// the rows' main-axis position.
fn row_labels(window: &gtk4::Window) -> Vec<String> {
    fn walk(w: &gtk4::Widget, out: &mut Vec<(f64, String)>, root: &gtk4::Widget) {
        if let Some(l) = w.downcast_ref::<gtk4::Label>() {
            let t = l.text().to_string();
            if t.starts_with("item:") {
                let y = w
                    .compute_point(root, &gtk4::graphene::Point::new(0.0, 0.0))
                    .map(|p| p.y() as f64)
                    .unwrap_or(0.0);
                out.push((y, t));
            }
        }
        let mut c = w.first_child();
        while let Some(x) = c {
            walk(&x, out, root);
            c = x.next_sibling();
        }
    }
    let root: &gtk4::Widget = window.upcast_ref();
    let mut out = Vec::new();
    walk(root, &mut out, root);
    out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    out.into_iter().map(|(_, t)| t).collect()
}

/// Settle until the visible rows are exactly the head of `data` (rows mount
/// from the list's adjustment signals, i.e. once GTK has allocated it — a
/// frame-clock tick, which needs wall time, not just main-loop spins).
fn wait_for_rows(
    ctx: &gtk4::glib::MainContext,
    backend: &Rc<RefCell<LinuxBackend>>,
    window: &gtk4::Window,
    data: &[String],
    step: &str,
) {
    let expect = |labels: &[String]| {
        !labels.is_empty()
            && labels.len() <= data.len()
            && labels
                .iter()
                .zip(data.iter())
                .all(|(l, d)| *l == format!("item:{d}"))
    };
    for _ in 0..400 {
        settle(ctx, backend);
        if expect(&row_labels(window)) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let labels = row_labels(window);
    panic!("{step}: visible rows {labels:?} are not the head of the data {data:?}");
}

fn strings(prefix: &str, n: usize) -> Vec<String> {
    (0..n).map(|i| format!("{prefix}{i}")).collect()
}

#[test]
fn regression_linux_virtualizer_data_change_with_mounted_rows_does_not_panic() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display / GTK init failed");
        return;
    }

    let window = gtk4::Window::new();
    window.set_default_size(WIN_W as i32, WIN_H as i32);
    let backend = Rc::new(RefCell::new(LinuxBackend::new(window.clone())));
    backend.borrow_mut().set_self_ref(Rc::downgrade(&backend));

    let slot: Rc<RefCell<Option<runtime_world::Signal<Vec<String>>>>> =
        Rc::new(RefCell::new(None));
    let slot_for_build = slot.clone();
    let app = newcore::start(backend.clone(), |_r| {}, move || {
        let data = runtime_world::signal(strings("a", 3));
        *slot_for_build.borrow_mut() = Some(data);
        view()
            .child(
                virtualizer(
                    move || data.with(|v| v.len()),
                    // Keys follow the CONTENT, so a replace changes them.
                    move |i| {
                        data.with(|v| {
                            v.get(i).map_or(u64::MAX, |s| {
                                s.bytes().fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(b as u64))
                            })
                        })
                    },
                    ItemSize::Known(Rc::new(|_| ROW_H)),
                    move |i| {
                        let label = data.with(|v| v.get(i).cloned().unwrap_or_default());
                        // A STYLED row root: its scope's teardown unstyles the
                        // node, which re-borrows the backend — the release half
                        // of the bug, alongside realize's borrow on mount.
                        view()
                            .style(StyleRules {
                                height: Some(Length::Px(ROW_H).into()),
                                ..Default::default()
                            })
                            .child(text().content(format!("item:{label}")).build())
                            .build()
                    },
                )
                .style(StyleRules {
                    height: Some(Length::Px(LIST_H).into()),
                    min_height: Some(Length::Px(LIST_H).into()),
                    width: Some(Length::Px(200.0).into()),
                    ..Default::default()
                })
                .build(),
            )
            .build()
    });

    window.present();
    let ctx = gtk4::glib::MainContext::default();
    for _ in 0..20_000 {
        if window.is_mapped() {
            break;
        }
        ctx.iteration(false);
    }
    if !window.is_mapped() {
        eprintln!("SKIP: window never mapped in this environment");
        return;
    }

    let data = slot.borrow().expect("signal created in build");
    wait_for_rows(&ctx, &backend, &window, &strings("a", 3), "initial mount");

    // Each step changes the data while rows are mounted.
    let steps: [(&str, Vec<String>); 5] = [
        ("append", strings("a", 8)),
        ("remove", strings("a", 2)),
        ("replace", strings("b", 4)),
        ("replace same length", strings("c", 4)),
        ("append after replace", strings("c", 12)),
    ];
    let mut after_step = Vec::new();
    for (step, next) in steps {
        data.set(next.clone());
        newcore::flush_sync();
        wait_for_rows(&ctx, &backend, &window, &next, step);
        after_step.push(backend.borrow().per_node_state_counts_for_test());
    }

    // The deferred reload must still FREE the cells it recycles (the
    // backend can't do it in-call any more): two reloads that each leave
    // the same 4 rows mounted ("replace" → "replace same length") must
    // leave identical per-node tables, not the second on top of the first.
    assert_eq!(
        after_step[2], after_step[3],
        "a reload that swaps 4 rows for 4 rows grew the per-node state — \
         recycled cells are leaking",
    );

    app.stop();
}
