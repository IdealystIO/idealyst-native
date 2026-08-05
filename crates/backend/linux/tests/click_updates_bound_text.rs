//! End-to-end interaction test: a click must re-render.
//!
//! ## What this pins
//!
//! In runtime-v2 the flush is NOT implicit. `newcore` wraps every author
//! callback in `flushing0` / `flushing1` / `flushing_key` so exactly one
//! deduped flush microtask is queued after the handler returns. Drop that
//! wrapping and the handler still runs, the signal still updates, and the
//! UI still never changes.
//!
//! That regression is invisible to the rest of the suite: the unit tests
//! never boot a world, and the welcome app renders correctly without it
//! because its animation is driven by the host's raf pump, which flushes on
//! its own beat. The only signal was a dead-code warning on `flushing0`.
//!
//! So this test drives the whole chain the way an app does — boot the world
//! through `newcore::start`, click a real GTK widget, let the main loop
//! drain — and asserts the *rendered label text* changed. It fails if the
//! flush wrapper is removed, which is the point.

#![cfg(target_os = "linux")]

use std::cell::RefCell;
use std::rc::Rc;

use backend_linux::{gtk4, newcore, LinuxBackend};
use gtk4::prelude::*;
use runtime_vocabulary::builders::{pressable, text, view};
use runtime_world::signal;

/// Depth-first search for the first `GtkLabel` under `root`.
fn find_label(root: &gtk4::Widget) -> Option<gtk4::Label> {
    if let Some(l) = root.clone().downcast::<gtk4::Label>().ok() {
        return Some(l);
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        if let Some(found) = find_label(&c) {
            return Some(found);
        }
        child = c.next_sibling();
    }
    None
}

/// Depth-first search for the first pressable — our pressables are
/// `IdealystView`s carrying a click gesture, so match on the concrete type
/// the backend builds for `pressable()`.
fn find_pressable(root: &gtk4::Widget) -> Option<gtk4::Widget> {
    let mut child = root.first_child();
    while let Some(c) = child {
        // A pressable has a gesture controller attached; a plain view does not.
        if c.observe_controllers().n_items() > 0 {
            return Some(c);
        }
        if let Some(found) = find_pressable(&c) {
            return Some(found);
        }
        child = c.next_sibling();
    }
    None
}

fn pump(ctx: &gtk4::glib::MainContext, n: usize) {
    for _ in 0..n {
        ctx.iteration(false);
    }
}

#[test]
fn regression_click_flushes_and_updates_bound_text() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display / GTK init failed");
        return;
    }

    let window = gtk4::Window::new();
    window.set_default_size(400, 300);

    // Same wiring the real host uses: the backend must live behind the very
    // `Rc<RefCell<_>>` it hands out as its self-reference, or `finish`'s
    // layout callback holds a dead weak and nothing is ever allocated.
    let backend = Rc::new(RefCell::new(LinuxBackend::new(window.clone())));
    backend.borrow_mut().set_self_ref(Rc::downgrade(&backend));

    let app = newcore::start(backend.clone(), |_registry| {}, || {
        let count = signal(0i32);
        let for_press = count;
        let for_text = count;
        view()
            .children(vec![
                pressable(move || for_press.set(for_press.get() + 1)).build(),
                text().content(move || for_text.get().to_string()).build(),
            ])
            .build()
    });

    window.present();

    let ctx = gtk4::glib::MainContext::default();
    // Pump until the tree is mapped and allocated.
    for _ in 0..10_000 {
        if window.is_mapped() {
            break;
        }
        ctx.iteration(false);
    }
    if !window.is_mapped() {
        eprintln!("SKIP: window never mapped in this environment");
        return;
    }
    pump(&ctx, 500);

    let root = window.child().expect("root attached by finish()");
    let label = find_label(&root).expect("the text node must build a GtkLabel");
    assert_eq!(
        label.text().as_str(),
        "0",
        "initial render should show the signal's starting value"
    );

    let target = find_pressable(&root).expect("the pressable must build a widget with a gesture");

    // Drive the click through the widget's own gesture, which is what the
    // compositor would do — not by calling the author closure directly,
    // which would bypass exactly the wrapper under test.
    let bounds = target
        .compute_bounds(&root)
        .expect("pressable must have bounds");
    let (cx, cy) = (
        (bounds.x() + bounds.width() / 2.0) as f64,
        (bounds.y() + bounds.height() / 2.0) as f64,
    );
    for controller in &target.observe_controllers() {
        let controller = controller.expect("controller");
        if let Ok(gesture) = controller.downcast::<gtk4::GestureClick>() {
            gesture.emit_by_name::<()>("pressed", &[&1i32, &cx, &cy]);
            gesture.emit_by_name::<()>("released", &[&1i32, &cx, &cy]);
        }
    }

    // Let the queued flush microtask run.
    pump(&ctx, 2_000);

    assert_eq!(
        label.text().as_str(),
        "1",
        "clicking must queue a flush so the bound text re-renders. Reading \
         \"0\" here means the author callback ran and mutated the signal but \
         no flush was scheduled — i.e. the `flushing0` wrapper around the \
         pressable's on_press was dropped."
    );

    app.stop();
}
