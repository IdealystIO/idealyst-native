//! The robot's Tab on GTK (`IntrospectionOps::move_focus`): the window's
//! own focus traversal moves focus to the next text field, Shift+Tab to the
//! previous one, as a user's Tab does.
//!
//! REGRESSION (robot E2E `tab_moves_focus_between_fields` on Linux): the
//! robot's key dispatch had no traversal, so `press_key("Tab")` left focus
//! in the first field.
//!
//! One GTK `#[test]` per binary — GTK initialises on one thread only.

// `move_focus` is `robot`-only; without the feature the trait default
// answers `None`.
#![cfg(all(target_os = "linux", feature = "robot"))]

use backend_linux::gtk4;
use gtk4::prelude::*;
use runtime_scene::Host;
use runtime_shared::accessibility::AccessibilityProps;
use runtime_vocabulary::caps::{IntrospectionOps, LifecycleOps, TextInputOps, ViewOps};
use std::cell::RefCell;
use std::rc::Rc;

#[test]
fn regression_tab_moves_focus_through_the_window_focus_chain() {
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
    let first = backend.borrow_mut().create_text_input("", None, Rc::new(|_| {}), None, None, false, &a11y);
    let second = backend.borrow_mut().create_text_input("", None, Rc::new(|_| {}), None, None, false, &a11y);
    backend.borrow_mut().insert(&mut root, first.clone());
    backend.borrow_mut().insert(&mut root, second.clone());
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
    if !pump(&|| second.widget().is_mapped()) {
        eprintln!("SKIP: window never mapped in this environment");
        return;
    }
    let focused = |node| backend.borrow().is_focused(node) == Some(true);
    let tab = |through, forward| {
        let inject = backend.borrow().move_focus(through, forward).expect("GTK moves focus");
        inject().expect("the window takes it");
    };

    backend.borrow().make_text_input_handle(&first).focus();
    assert!(pump(&|| focused(&first)), "the first field holds focus");

    tab(&first, true);
    assert!(pump(&|| focused(&second)), "Tab moves to the next field");
    assert!(!focused(&first));

    tab(&second, false);
    assert!(pump(&|| focused(&first)), "Shift+Tab moves back");

    // Past the last field GTK's traversal finds nothing further and leaves
    // focus where it is (GTK4's window does not wrap Tab); with nothing
    // focused, a Tab through the window starts at the first field.
    tab(&second, true);
    pump(&|| false);
    assert!(focused(&second) && !focused(&first), "Tab past the last field keeps it focused");
    GtkWindowExt::set_focus(&window, None::<&gtk4::Widget>);
    assert!(pump(&|| !focused(&first) && !focused(&second)), "nothing focused");
    tab(&root, true);
    assert!(pump(&|| focused(&first)), "a Tab with nothing focused starts at the first field");
    println!("robot_tab_traversal: ok");
}
