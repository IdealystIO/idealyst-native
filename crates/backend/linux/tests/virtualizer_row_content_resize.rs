//! A change inside a MOUNTED virtualizer row that resizes its content is
//! laid out: the row's inner frames follow in every list, and in a
//! `Measured` list the row's slot — and every following row's offset —
//! follows its new size.
//!
//! ## The bug this pins
//!
//! A row root is a detached Taffy root. The main layout pass (driven every
//! frame by the GTK host through `LinuxBackend::pump`) computes from the
//! framework root, so it never recomputed a row: a restyle inside a mounted
//! row dirtied Taffy and nothing laid it out again until the next
//! scroll/resize re-windowed the list. The row's inner frames stayed stale
//! (Known and Measured alike), and a Measured row kept the slot it was
//! measured into at mount, so a card that grew overlapped the next one.
//!
//! ## Shape
//!
//! Each row is an auto-sized view holding one content view; the test keeps
//! each row's content node and restyles its height after mount, then lets
//! frames run (calling `pump()` like the host's frame tick) — no scroll, no
//! resize, no data change.
//!
//! - Known list (40px slots, content 30px): content grows to 70, then
//!   shrinks to 10. The content's frame follows; every row stays in its
//!   40px slot and nothing is reported.
//! - Measured list (estimate 40, content 50): row 1 grows to 90, then
//!   shrinks to 20. Row 1's slot and rows 2–3's offsets follow, and the
//!   document extent with them.
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
use runtime_shared::{FlexDirection, Length, StyleRules, Tokenized, VirtualizerCallbacks};
use runtime_vocabulary::caps::{LifecycleOps, StyleOps, ViewOps, VirtualizerOps};

const SLOT: f32 = 40.0;

fn px(v: f32) -> Option<Tokenized<Length>> {
    Some(Tokenized::Literal(Length::Px(v)))
}

/// `flex_shrink: 0` so content taller than a Known row's 40px slot keeps
/// its declared height (overflowing the row) instead of shrinking to fit.
fn content_style(h: f32) -> Rc<StyleRules> {
    Rc::new(StyleRules {
        height: px(h),
        flex_shrink: Some(Tokenized::Literal(0.0)),
        ..StyleRules::default()
    })
}

/// What one list's callbacks observed.
#[derive(Default)]
struct Probe {
    /// index → (row root, its content view).
    mounted: HashMap<usize, (LinuxNode, LinuxNode)>,
    /// Every `set_measured_size` call, as `(index, size)`.
    reports: Vec<(usize, f32)>,
}

/// A 4-row list whose row `i` starts with `initial`px of content. Measured
/// lists store reported sizes by key over the `SLOT` estimate, like the
/// vocabulary; Known lists always answer `SLOT`.
fn list(backend: &Rc<RefCell<LinuxBackend>>, measured_mode: bool, initial: f32) -> (LinuxNode, Rc<RefCell<Probe>>) {
    let probe = Rc::new(RefCell::new(Probe::default()));
    let measured: Rc<RefCell<HashMap<usize, f32>>> = Rc::new(RefCell::new(HashMap::new()));
    let callbacks = VirtualizerCallbacks {
        item_count: Rc::new(|| 4),
        item_key: Rc::new(|i| i as u64),
        item_size: {
            let measured = measured.clone();
            Rc::new(move |i| measured.borrow().get(&i).copied().unwrap_or(SLOT))
        },
        measure_sizes: measured_mode,
        mount_item: {
            let backend = backend.clone();
            let probe = probe.clone();
            Rc::new(move |idx| {
                let a11y = AccessibilityProps::default();
                let mut b = backend.borrow_mut();
                let mut root = b.create_view(&a11y);
                let content = b.create_view(&a11y);
                b.apply_style(&content, &content_style(initial));
                b.insert(&mut root, content.clone());
                probe.borrow_mut().mounted.insert(idx, (root.clone(), content));
                // All 4 rows fit, so a row mounts once: scope id = index.
                (root, idx as u64)
            })
        },
        release_item: Rc::new(|_| {}),
        set_measured_size: {
            let probe = probe.clone();
            Rc::new(move |scope, size| {
                measured.borrow_mut().insert(scope as usize, size);
                probe.borrow_mut().reports.push((scope as usize, size));
            })
        },
        on_scroll: None,
        item_changed: None,
    };
    let a11y = AccessibilityProps::default();
    let node = backend.borrow_mut().create_virtualizer(callbacks, 0.0, VirtualLayout::default(), &a11y);
    backend.borrow_mut().apply_style(
        &node,
        &Rc::new(StyleRules { width: px(200.0), height: px(400.0), ..StyleRules::default() }),
    );
    (node, probe)
}

/// Run frames the way the GTK host does: `pump()` re-allocates the root
/// (→ `run_layout`) each tick.
fn frames(backend: &Rc<RefCell<LinuxBackend>>, until: impl Fn() -> bool) {
    let ctx = gtk4::glib::MainContext::default();
    for _ in 0..300 {
        backend.borrow().pump();
        for _ in 0..200 {
            ctx.iteration(false);
        }
        if until() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    for _ in 0..5 {
        backend.borrow().pump();
        for _ in 0..200 {
            ctx.iteration(false);
        }
    }
}

/// `(y, height)` of row `i`'s root and of its content view.
fn row_and_content(backend: &Rc<RefCell<LinuxBackend>>, probe: &Rc<RefCell<Probe>>, i: usize) -> Option<((f32, f32), (f32, f32))> {
    let (root, content) = probe.borrow().mounted.get(&i).cloned()?;
    let b = backend.borrow();
    let (_, ry, _, rh) = b.node_frame(root.id())?;
    let (_, cy, _, ch) = b.node_frame(content.id())?;
    Some(((ry, rh), (cy, ch)))
}

fn row_ys(backend: &Rc<RefCell<LinuxBackend>>, probe: &Rc<RefCell<Probe>>) -> Vec<(f32, f32)> {
    (0..4).filter_map(|i| row_and_content(backend, probe, i).map(|r| r.0)).collect()
}

fn restyle(backend: &Rc<RefCell<LinuxBackend>>, probe: &Rc<RefCell<Probe>>, i: usize, h: f32) {
    let content = probe.borrow().mounted[&i].1.clone();
    backend.borrow_mut().apply_style(&content, &content_style(h));
}

fn document_height(probe: &Rc<RefCell<Probe>>) -> i32 {
    let root = probe.borrow().mounted[&0].0.clone();
    root.widget().parent().expect("rows are parented into the document").size_request().1
}

#[test]
fn regression_mounted_row_content_resize_relayouts_the_row() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display / GTK init failed");
        return;
    }

    let window = gtk4::Window::new();
    window.set_default_size(500, 500);
    let backend = Rc::new(RefCell::new(LinuxBackend::new(window.clone())));
    backend.borrow_mut().set_self_ref(Rc::downgrade(&backend));
    let a11y = AccessibilityProps::default();

    let (known_list, known) = list(&backend, false, 30.0);
    let (measured_list, measured) = list(&backend, true, 50.0);
    let mut root = backend.borrow_mut().create_view(&a11y);
    backend.borrow_mut().apply_style(
        &root,
        &Rc::new(StyleRules { flex_direction: Some(FlexDirection::Row), ..StyleRules::default() }),
    );
    backend.borrow_mut().insert(&mut root, known_list);
    backend.borrow_mut().insert(&mut root, measured_list);
    backend.borrow_mut().finish(root.clone());
    window.set_child(Some(root.widget()));
    window.present();

    let known_slots: Vec<(f32, f32)> = (0..4).map(|i| (i as f32 * SLOT, SLOT)).collect();
    let measured_at = |hs: [f32; 4]| -> Vec<(f32, f32)> {
        let mut y = 0.0;
        hs.iter()
            .map(|h| {
                let r = (y, *h);
                y += h;
                r
            })
            .collect()
    };
    frames(&backend, || {
        row_ys(&backend, &known) == known_slots && row_ys(&backend, &measured) == measured_at([50.0; 4])
    });
    if !root.widget().is_mapped() {
        eprintln!("SKIP: window never mapped in this environment");
        return;
    }
    // Baseline: mount-time measurement and layout.
    assert_eq!(row_ys(&backend, &known), known_slots, "known rows in their slots at mount");
    assert_eq!(row_ys(&backend, &measured), measured_at([50.0; 4]), "measured rows at 50 at mount");
    assert_eq!(row_and_content(&backend, &known, 1).map(|r| r.1), Some((0.0, 30.0)));

    // The two lists are checked independently and both verdicts reported,
    // so a failure in one doesn't hide the other's.
    let known_result = (|| -> Result<(), String> {
        // ---- Known: the content's frame follows; the slot does not ----
        for h in [70.0, 10.0] {
            restyle(&backend, &known, 1, h);
            frames(&backend, || row_and_content(&backend, &known, 1).map(|r| r.1) == Some((0.0, h)));
            check(
                row_and_content(&backend, &known, 1).map(|r| r.1),
                Some((0.0, h)),
                format!("known row 1's content was laid out again at {h}px after a restyle inside the mounted row"),
            )?;
            check(row_ys(&backend, &known), known_slots.clone(), format!("known rows keep their slots (content {h}px)"))?;
            check(document_height(&known), (4.0 * SLOT) as i32, "known document extent".into())?;
        }
        check(known.borrow().reports.clone(), vec![], "a Known list reports nothing".into())
    })();

    let measured_result = (|| -> Result<(), String> {
        // ---- Measured: row 1's slot and the offsets after it follow ----
        for h in [90.0, 20.0] {
            restyle(&backend, &measured, 1, h);
            let want = measured_at([50.0, h, 50.0, 50.0]);
            frames(&backend, || row_ys(&backend, &measured) == want);
            check(
                row_ys(&backend, &measured),
                want,
                format!("measured row 1 resized to {h}px: its slot and rows 2-3's offsets follow"),
            )?;
            check(
                row_and_content(&backend, &measured, 1).map(|r| r.1),
                Some((0.0, h)),
                format!("measured row 1's content at {h}px"),
            )?;
            check(document_height(&measured), (150.0 + h) as i32, format!("measured document extent (row 1 at {h}px)"))?;
        }
        let row1_reports: Vec<f32> =
            measured.borrow().reports.iter().filter(|r| r.0 == 1).map(|r| r.1).collect();
        check(row1_reports, vec![50.0, 90.0, 20.0], "row 1 reported once per size (converged)".into())
    })();

    window.close();
    let failures: Vec<String> = [known_result, measured_result].into_iter().filter_map(Result::err).collect();
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// `assert_eq!` that returns the failure instead of panicking.
fn check<T: PartialEq + std::fmt::Debug>(got: T, want: T, what: String) -> Result<(), String> {
    if got == want {
        Ok(())
    } else {
        Err(format!("{what}: got {got:?}, want {want:?}"))
    }
}
