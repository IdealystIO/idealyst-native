//! Static `z_index` restacks siblings for paint AND for picking.
//!
//! `IdealystView` paints its children from its own Vec, while GTK picks
//! them by walking the widget sibling list last-to-first. A restack that
//! reorders only the Vec draws a raised sibling on top while the later
//! sibling under it still takes the click, so both orders are asserted
//! here, plus the `GtkFixed` document a scroll view's children live in.
//!
//! # Why an integration test with a real window
//!
//! `gtk_widget_pick` returns `None` for an unmapped widget, so a pick
//! assertion needs a presented window, and GTK may be driven only from
//! the one thread that ran `gtk::init` — hence its own test binary with
//! exactly one GTK `#[test]`. Skips rather than fails with no display.

#![cfg(target_os = "linux")]

use backend_linux::gtk4;
use backend_linux::IdealystView;
use gtk4::prelude::*;
use runtime_scene::Host;
use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::{Length, Position, StyleRules, Tokenized};
use runtime_vocabulary::caps::{LifecycleOps, PressableOps, ScrollOps, StyleOps, ViewOps};
use std::cell::RefCell;
use std::rc::Rc;

fn px(v: f32) -> Option<Tokenized<Length>> {
    Some(Tokenized::Literal(Length::Px(v)))
}

/// An absolutely-placed 120×80 box at `(left, 40)`.
fn placed(left: f32, z_index: Option<i32>) -> Rc<StyleRules> {
    Rc::new(StyleRules {
        position: Some(Position::Absolute),
        left: px(left),
        top: px(40.0),
        width: px(120.0),
        height: px(80.0),
        z_index,
        ..StyleRules::default()
    })
}

fn pump(until: impl Fn() -> bool) {
    let ctx = gtk4::glib::MainContext::default();
    for _ in 0..10_000 {
        if until() {
            break;
        }
        ctx.iteration(false);
    }
    // Let a queued allocate/draw settle.
    for _ in 0..50 {
        ctx.iteration(false);
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
fn regression_static_z_index_raises_an_earlier_sibling_for_paint_and_pick() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display / GTK init failed");
        return;
    }

    let window = gtk4::Window::new();
    window.set_default_size(400, 300);
    // Same self-ref wiring the real host uses; without it the Taffy pass
    // never runs inside `size_allocate` (see `pressable_hit_test.rs`).
    let backend = Rc::new(RefCell::new(backend_linux::LinuxBackend::new(window.clone())));
    backend.borrow_mut().set_self_ref(Rc::downgrade(&backend));
    let a11y = AccessibilityProps::default();

    let mut root = backend.borrow_mut().create_view(&a11y);
    let first = backend.borrow_mut().create_pressable(Rc::new(|| {}), &a11y);
    let second = backend.borrow_mut().create_pressable(Rc::new(|| {}), &a11y);
    // `apply_style` runs before `insert` in realize — the raised child's
    // z is set before it has a parent, so `insert` must place it.
    backend.borrow_mut().apply_style(&first, &placed(40.0, Some(2)));
    backend.borrow_mut().apply_style(&second, &placed(100.0, None));
    backend.borrow_mut().insert(&mut root, first.clone());
    backend.borrow_mut().insert(&mut root, second.clone());
    backend.borrow_mut().finish(root.clone());

    window.set_child(Some(root.widget()));
    window.present();
    pump(|| second.widget().is_mapped() && second.widget().width() > 0);
    if !second.widget().is_mapped() {
        eprintln!("SKIP: window never mapped in this environment");
        return;
    }

    let root_view = root.widget().downcast_ref::<IdealystView>().expect("a view is an IdealystView").clone();
    let (fw, sw) = (first.widget().clone(), second.widget().clone());
    // In the overlap (x 100..160): whichever pressable GTK picks there.
    let picked = || {
        let hit = root.widget().pick(130.0, 80.0, gtk4::PickFlags::DEFAULT);
        let is = |w: &gtk4::Widget| hit.as_ref().is_some_and(|h| h == w || h.is_ancestor(w));
        if is(&fw) {
            "first"
        } else if is(&sw) {
            "second"
        } else {
            "neither"
        }
    };

    // 1. Raised at insert time.
    assert_eq!(root_view.paint_order(), [sw.clone(), fw.clone()], "z 2 paints the first sibling last");
    assert_eq!(siblings(root.widget()), [sw.clone(), fw.clone()], "the GTK sibling list follows paint order");
    assert_eq!(picked(), "first", "the raised sibling takes the click where they overlap");

    // 2. A restyle without `z_index` restores tree order — paint and pick.
    backend.borrow_mut().apply_style(&first, &placed(40.0, None));
    pump(|| false);
    assert_eq!(root_view.paint_order(), [fw.clone(), sw.clone()]);
    assert_eq!(siblings(root.widget()), [fw.clone(), sw.clone()]);
    assert_eq!(picked(), "second", "removing z_index gives the later sibling the click back");

    // 3. Raised by a restyle of a node already in the tree.
    backend.borrow_mut().apply_style(&first, &placed(40.0, Some(2)));
    pump(|| false);
    assert_eq!(root_view.paint_order(), [sw.clone(), fw.clone()]);
    assert_eq!(picked(), "first");

    // 4. A sibling appended at a LOWER z than an existing raised one goes
    //    beneath it, not on top.
    let third = backend.borrow_mut().create_view(&a11y);
    backend.borrow_mut().apply_style(&third, &placed(160.0, Some(1)));
    backend.borrow_mut().insert(&mut root, third.clone());
    assert_eq!(
        root_view.paint_order(),
        [sw.clone(), third.widget().clone(), fw.clone()],
        "z 1 appended after a z 2 sibling stacks beneath it"
    );

    // 5. A scroll view's children live in its `GtkFixed` document, which
    //    paints and picks in sibling order — the restack must reach it.
    let mut scroll = backend.borrow_mut().create_scroll_view(Default::default(), None, &a11y);
    let low = backend.borrow_mut().create_view(&a11y);
    let high = backend.borrow_mut().create_view(&a11y);
    backend.borrow_mut().apply_style(&high, &placed(0.0, Some(3)));
    backend.borrow_mut().insert(&mut scroll, high.clone());
    backend.borrow_mut().insert(&mut scroll, low.clone());
    let document = high.widget().parent().expect("a scroll child is parented to its document");
    assert_eq!(siblings(&document), [low.widget().clone(), high.widget().clone()]);
    backend.borrow_mut().apply_style(&high, &placed(0.0, None));
    assert_eq!(siblings(&document), [high.widget().clone(), low.widget().clone()]);

    window.close();
}
