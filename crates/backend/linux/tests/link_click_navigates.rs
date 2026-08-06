//! A `link` click must COMMIT, not just fire.
//!
//! ## The bug this pins
//!
//! `caps::LinkOps::create_link` delegated to the inherent GTK body with a
//! RAW `on_activate`, while every other interactive capability on this
//! backend wraps its callback in `flushing0` / `flushing1` (pressable,
//! button, text_input, text_area, toggle, slider, portal `on_dismiss`).
//!
//! For an in-app link the framework wraps navigator push/replace dispatch
//! inside `on_activate`. Without the wrapper the click landed, the gesture
//! fired, the author callback ran and the route signal changed — and no
//! flush was ever scheduled, so nothing re-rendered. The whole app read as
//! "links/buttons don't do anything", which is how it was reported.
//!
//! Nothing catches this by construction: a raw delegation compiles, the
//! widget still reacts to input, and the reactive graph really does update.
//! Only the commit is missing. So the test asserts on RENDERED output after
//! a real gesture, not on the callback having run — a test that checked the
//! callback would have passed against the bug.

#![cfg(target_os = "linux")]

use std::cell::RefCell;
use std::rc::Rc;

use backend_linux::{gtk4, newcore, LinuxBackend};
use gtk4::prelude::*;
use runtime_vocabulary::builders::{link, text, view};
use runtime_world::signal;

/// Depth-first: the first widget carrying a `GestureClick` — what both
/// `create_link` and `create_pressable` build.
fn find_clickable(root: &gtk4::Widget) -> Option<gtk4::Widget> {
    let mut child = root.first_child();
    while let Some(c) = child {
        let has_click = c.observe_controllers().into_iter().any(|ctl| {
            ctl.map(|ctl| ctl.downcast::<gtk4::GestureClick>().is_ok()).unwrap_or(false)
        });
        if has_click {
            return Some(c);
        }
        if let Some(f) = find_clickable(&c) {
            return Some(f);
        }
        child = c.next_sibling();
    }
    None
}

fn find_label(root: &gtk4::Widget) -> Option<gtk4::Label> {
    if let Ok(l) = root.clone().downcast::<gtk4::Label>() {
        return Some(l);
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        if let Some(f) = find_label(&c) {
            return Some(f);
        }
        child = c.next_sibling();
    }
    None
}

#[test]
fn regression_link_activation_flushes_and_rerenders() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display / GTK init failed");
        return;
    }

    let window = gtk4::Window::new();
    window.set_default_size(400, 300);
    let backend = Rc::new(RefCell::new(LinuxBackend::new(window.clone())));
    backend.borrow_mut().set_self_ref(Rc::downgrade(&backend));

    // A link whose `on_activate` mutates a signal the tree renders — the
    // same shape as navigator dispatch, without needing a navigator.
    let app = newcore::start(backend.clone(), |_registry| {}, || {
        let route = signal(0i32);
        let for_link = route;
        let for_text = route;
        view()
            .children(vec![
                link()
                    .on_activate(move || for_link.set(for_link.get() + 1))
                    .build(),
                text().content(move || format!("route {}", for_text.get())).build(),
            ])
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
    for _ in 0..500 {
        ctx.iteration(false);
    }

    let root = window.child().expect("root attached by finish()");
    let label = find_label(&root).expect("the text node must build a GtkLabel");
    assert_eq!(label.text().as_str(), "route 0", "initial render");

    let target = find_clickable(&root).expect("the link must build a widget with a click gesture");
    let b = target.compute_bounds(&root).expect("link bounds");
    let (cx, cy) = ((b.x() + b.width() / 2.0) as f64, (b.y() + b.height() / 2.0) as f64);
    for controller in &target.observe_controllers() {
        let controller = controller.expect("controller");
        if let Ok(gesture) = controller.downcast::<gtk4::GestureClick>() {
            gesture.emit_by_name::<()>("pressed", &[&1i32, &cx, &cy]);
            gesture.emit_by_name::<()>("released", &[&1i32, &cx, &cy]);
        }
    }

    // Let the queued flush microtask run.
    for _ in 0..2_000 {
        ctx.iteration(false);
    }

    assert_eq!(
        label.text().as_str(),
        "route 1",
        "activating a link must queue a flush so the tree re-renders. Reading \
         \"route 0\" here means `on_activate` ran and mutated the signal but no \
         flush was scheduled — i.e. `caps::LinkOps::create_link` delegated the \
         RAW callback instead of wrapping it in `flushing0`. In a real app that \
         is every in-app link silently doing nothing.",
    );

    app.stop();
}
