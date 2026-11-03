//! The robot's `focused` for a text field on GTK is
//! `IntrospectionOps::is_focused`, read from the window's focus widget at
//! query time. Before, Linux answered `None` and the robot replayed the
//! field's `EventControllerFocus` notifications, which GTK only sends while
//! the window is the active one — a field focused in a background window
//! (a test run, another app in front) read unfocused.
//!
//! One GTK `#[test]` per binary — GTK initialises on one thread only.

// `is_focused` is `robot`-only; without the feature the trait default
// answers `None` and every assertion would fail.
#![cfg(all(target_os = "linux", feature = "robot"))]

use backend_linux::gtk4;
use gtk4::prelude::*;
use runtime_scene::Host;
use runtime_shared::accessibility::AccessibilityProps;
use runtime_vocabulary::caps::{IntrospectionOps, LifecycleOps, TextInputOps, ViewOps};
use std::cell::RefCell;
use std::rc::Rc;

#[test]
fn is_focused_reads_the_window_focus_widget() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display / GTK init failed");
        return;
    }
    let window = gtk4::Window::new();
    window.set_default_size(400, 300);
    let backend = Rc::new(RefCell::new(backend_linux::LinuxBackend::new(window.clone())));
    backend.borrow_mut().set_self_ref(Rc::downgrade(&backend));
    let a11y = AccessibilityProps::default();
    let mut root = backend.borrow_mut().create_view(&a11y);
    let input = backend.borrow_mut().create_text_input("", None, Rc::new(|_| {}), None, None, false, &a11y);
    let area = backend.borrow_mut().create_text_area("", None, true, None, None, Rc::new(|_| {}), None, &a11y);
    backend.borrow_mut().insert(&mut root, input.clone());
    backend.borrow_mut().insert(&mut root, area.clone());
    backend.borrow_mut().finish(root.clone());
    window.set_child(Some(root.widget()));
    window.present();

    let ctx = gtk4::glib::MainContext::default();
    let pump = |until: &dyn Fn() -> bool| {
        for _ in 0..10_000 {
            if until() {
                return true;
            }
            ctx.iteration(false);
        }
        until()
    };
    if !pump(&|| input.widget().is_mapped()) {
        eprintln!("SKIP: window never mapped in this environment");
        return;
    }
    let focused = |node| backend.borrow().is_focused(node);
    GtkWindowExt::set_focus(&window, None::<&gtk4::Widget>);
    assert_eq!(focused(&input), Some(false), "nothing focused");

    backend.borrow().make_text_input_handle(&input).focus();
    assert!(pump(&|| focused(&input) == Some(true)), "the entry (its inner GtkText) holds focus");
    assert_eq!(focused(&area), Some(false));

    backend.borrow().make_text_area_handle(&area).focus();
    assert!(pump(&|| focused(&area) == Some(true)), "the text area's TextView holds focus");
    assert_eq!(focused(&input), Some(false), "focus moved on");
}
