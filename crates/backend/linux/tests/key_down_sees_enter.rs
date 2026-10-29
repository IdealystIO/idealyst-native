//! `on_key_down` on a `text_input` / `text_area` must see Enter.
//!
//! ## The bug this pins
//!
//! The backend added the author's key controller in GTK's default BUBBLE
//! phase. A key event reaches the focused widget's own handlers first:
//! for a `GtkEntry` that is its inner `GtkText`, which consumes Enter as its
//! `activate` action; for a `GtkTextView`, its own key handling, installed
//! before ours, which inserts a newline. Either way the bubble-phase
//! controller never ran, so an `input_with_submit` (Enter submits) did
//! nothing on Linux and `PreventDefault` could not stop the newline. The
//! controller must run in the CAPTURE phase, which GTK runs from the
//! toplevel down to the target before any target or bubble handler.
//!
//! ## Why the phase is asserted rather than a real key routed
//!
//! GTK4 offers no public way to synthesize a `GdkKeyEvent`, and real input
//! needs XTest on a real X server (`user_activity.rs`), which an XWayland
//! desktop session does not route reliably. So this pins the cause — the
//! author controller's propagation phase — and emits that controller's own
//! `key-pressed` for Enter to check the handler and outcome; GDK's routing
//! of a capture-phase controller ahead of `GtkText` is GTK's documented
//! propagation order. A test that only emitted on "the" key controller
//! would pass against the bug (the existing `text_area_on_change` test did).
//!
//! One `#[test]`: GTK must be driven from the thread that ran `gtk::init`.

#![cfg(target_os = "linux")]

use std::cell::RefCell;
use std::rc::Rc;

use backend_linux::{gtk4, newcore, LinuxBackend};
use gtk4::glib::translate::IntoGlib;
use gtk4::prelude::*;
use runtime_shared::primitives::key::{KeyEvent, KeyOutcome};
use runtime_vocabulary::builders::{text_area, text_input, view};

fn find<T: IsA<gtk4::Widget>>(root: &gtk4::Widget) -> Option<T> {
    if let Ok(w) = root.clone().downcast::<T>() {
        return Some(w);
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        if let Some(f) = find::<T>(&c) {
            return Some(f);
        }
        child = c.next_sibling();
    }
    None
}

/// The capture-phase key controller on `w` — the one the author's
/// `on_key_down` must be.
fn capture_key_controller(w: &impl IsA<gtk4::Widget>) -> Option<gtk4::EventControllerKey> {
    w.observe_controllers()
        .into_iter()
        .filter_map(|c| c.ok()?.downcast::<gtk4::EventControllerKey>().ok())
        .find(|k| k.propagation_phase() == gtk4::PropagationPhase::Capture)
}

fn press_enter(c: &gtk4::EventControllerKey) -> bool {
    c.emit_by_name("key-pressed", &[&gtk4::gdk::Key::Return.into_glib(), &0u32, &gtk4::gdk::ModifierType::empty()])
}

#[test]
fn regression_enter_reaches_text_field_on_key_down() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display / GTK init failed");
        return;
    }
    let window = gtk4::Window::new();
    let backend = Rc::new(RefCell::new(LinuxBackend::new(window.clone())));
    backend.borrow_mut().set_self_ref(Rc::downgrade(&backend));

    let input_keys: Rc<RefCell<Vec<String>>> = Rc::default();
    let area_keys: Rc<RefCell<Vec<String>>> = Rc::default();
    let (ik, ak) = (input_keys.clone(), area_keys.clone());
    let app = newcore::start(backend.clone(), |_r| {}, move || {
        let (ik, ak) = (ik.clone(), ak.clone());
        view()
            .child(
                text_input()
                    .on_key_down(move |e: &KeyEvent| {
                        ik.borrow_mut().push(e.key.clone());
                        // Submit-on-Enter claims the key.
                        if e.key == "Enter" { KeyOutcome::PreventDefault } else { KeyOutcome::Default }
                    })
                    .build(),
            )
            .child(
                text_area()
                    .on_key_down(move |e: &KeyEvent| {
                        ak.borrow_mut().push(e.key.clone());
                        if e.key == "Enter" { KeyOutcome::PreventDefault } else { KeyOutcome::Default }
                    })
                    .build(),
            )
            .build()
    });
    let ctx = gtk4::glib::MainContext::default();
    for _ in 0..2_000 {
        ctx.iteration(false);
    }

    let root = window.child().expect("root attached");

    let entry: gtk4::Entry = find(&root).expect("text_input is a GtkEntry");
    let keys = capture_key_controller(&entry).expect(
        "the text_input's on_key_down controller must be CAPTURE-phase: in the default \
         bubble phase the entry's inner GtkText consumes Enter as `activate` first",
    );
    assert!(press_enter(&keys), "PreventDefault on Enter stops the key before GtkText activates");
    assert_eq!(*input_keys.borrow(), vec!["Enter".to_string()]);

    let text_view: gtk4::TextView = find(&root).expect("text_area wraps a GtkTextView");
    let keys = capture_key_controller(&text_view).expect(
        "the text_area's on_key_down controller must be CAPTURE-phase: in the bubble phase \
         the text view's own key handling inserts the newline first",
    );
    assert!(press_enter(&keys), "PreventDefault on Enter stops the newline");
    assert_eq!(*area_keys.borrow(), vec!["Enter".to_string()]);

    app.stop();
}
