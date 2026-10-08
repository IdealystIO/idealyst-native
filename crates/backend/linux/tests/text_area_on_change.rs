//! `text_area` must report edits, follow its controlled `value`, and deliver
//! `on_key_down` on GTK.
//!
//! ## The bug this pins
//!
//! `create_text_area` built a `GtkTextView` in a `GtkScrolledWindow`, set its
//! text, and connected nothing. The buffer's `changed` signal never reached
//! the author, so `on_change` never fired — a controlled `value` never saw a
//! keystroke, and the app's state stayed at its initial text however much
//! the user typed. `update_text_area_value` was the trait's no-op default,
//! so a programmatic `value.set(…)` never reached the widget either, and
//! `on_key_down` was dropped (an Enter-to-submit text area could not
//! submit).
//!
//! The edit is made through the `GtkTextBuffer` (`insert_at_cursor`, the
//! same path GTK's own key handling takes), and the key through the view's
//! key controller (`key-pressed`) — GTK4 cannot synthesize a real keyboard
//! event without a seat, so the controller's signal is the closest a test
//! can get to the user's key.
//!
//! One `#[test]` on purpose: GTK must be driven from the thread that ran
//! `gtk::init`, and cargo gives every test its own thread.

#![cfg(target_os = "linux")]

use std::cell::RefCell;
use std::rc::Rc;

use backend_linux::{gtk4, newcore, LinuxBackend};
use gtk4::glib::translate::IntoGlib;
use gtk4::prelude::*;
use runtime_shared::primitives::key::{KeyEvent, KeyOutcome};
use runtime_vocabulary::builders::{text_area, view};

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

/// Commit staged signal writes, then drain GTK's main loop.
fn pump(ctx: &gtk4::glib::MainContext) {
    newcore::flush_sync();
    for _ in 0..2_000 {
        ctx.iteration(false);
    }
}

fn buffer_text(view: &gtk4::TextView) -> String {
    let b = view.buffer();
    let (s, e) = b.bounds();
    b.text(&s, &e, true).to_string()
}

#[test]
fn regression_linux_text_area_on_change_never_fires() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display / GTK init failed");
        return;
    }

    let window = gtk4::Window::new();
    window.set_default_size(600, 400);
    let backend = Rc::new(RefCell::new(LinuxBackend::new(window.clone())));
    backend.borrow_mut().set_self_ref(Rc::downgrade(&backend));

    let slot: Rc<RefCell<Option<runtime_world::Signal<String>>>> = Rc::new(RefCell::new(None));
    let slot_for_build = slot.clone();
    let edits: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let keys: Rc<RefCell<Vec<(String, usize, usize)>>> = Rc::new(RefCell::new(Vec::new()));
    let e = edits.clone();
    let k = keys.clone();
    let app = newcore::start(backend.clone(), |_r| {}, move || {
        let notes = runtime_world::signal(String::from("notes"));
        *slot_for_build.borrow_mut() = Some(notes);
        view()
            .child(
                text_area()
                    .value(notes)
                    // Controlled: the author writes every edit back.
                    .on_change(move |v: String| {
                        e.borrow_mut().push(v.clone());
                        notes.set(v);
                    })
                    .on_key_down(move |ev: &KeyEvent| {
                        k.borrow_mut().push((ev.key.clone(), ev.selection_start, ev.selection_end));
                        // Enter submits: no newline goes in.
                        if ev.key == "Enter" {
                            KeyOutcome::PreventDefault
                        } else {
                            KeyOutcome::Default
                        }
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
    pump(&ctx);

    let root = window.child().expect("root attached");
    let text_view: gtk4::TextView = find(&root).expect("text_area wraps a GtkTextView");
    let notes = slot.borrow().expect("build filled the slot");
    assert_eq!(buffer_text(&text_view), "notes", "mounts with its initial value");
    assert!(edits.borrow().is_empty(), "mounting reports no edit");

    // 1. A user edit reaches the author (pre-fix: never).
    let buffer = text_view.buffer();
    buffer.place_cursor(&buffer.end_iter());
    buffer.insert_at_cursor(" + more");
    pump(&ctx);
    assert_eq!(
        edits.borrow().last().map(String::as_str),
        Some("notes + more"),
        "typing into a text_area must fire on_change: {:?}",
        edits.borrow()
    );
    assert_eq!(notes.peek(), "notes + more", "the controlled value saw the edit");

    // 2. A programmatic write reaches the widget, and is NOT echoed back as
    //    an edit (pre-fix: the widget never followed its signal).
    let before = edits.borrow().len();
    notes.set("reset".to_string());
    pump(&ctx);
    assert_eq!(buffer_text(&text_view), "reset", "the text area follows its signal");
    assert_eq!(edits.borrow().len(), before, "a programmatic write is not reported as an edit");

    // 3. on_key_down reaches the author with the caret in characters, and
    //    PreventDefault stops the key.
    buffer.place_cursor(&buffer.iter_at_offset(2));
    let controller = text_view
        .observe_controllers()
        .into_iter()
        .filter_map(|c| c.ok()?.downcast::<gtk4::EventControllerKey>().ok())
        .next()
        .expect("text_area installs a key controller when on_key_down is set");
    let stopped: bool = controller.emit_by_name(
        "key-pressed",
        &[&gtk4::gdk::Key::Return.into_glib(), &0u32, &gtk4::gdk::ModifierType::empty()],
    );
    pump(&ctx);
    assert_eq!(*keys.borrow(), vec![("Enter".to_string(), 2, 2)], "on_key_down fired with the caret");
    assert!(stopped, "PreventDefault stops the key before the text view inserts a newline");

    app.stop();
}
