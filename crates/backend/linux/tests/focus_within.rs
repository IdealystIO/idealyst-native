//! `view` / `pressable` `on_focus` on GTK (`InputOps::install_focus_handler`
//! → `states::install_focus_within`): keyboard focus entering / leaving a
//! wrapper's subtree, gated on the window's `focus-visible` like the
//! `FOCUSED` style state.
//!
//! Reads the RAW backend reports (no vocabulary dedup in between), so a
//! report that repeats the current state shows up; the vocabulary drops
//! those.
//!
//! The focus targets are native `button`s (`GtkButton`) and a `text_input`:
//! a `pressable` is an `IdealystView`, which is not a focus target on this
//! backend.
//!
//! One GTK `#[test]` per binary — GTK initialises on one thread only.

#![cfg(target_os = "linux")]

use backend_linux::gtk4;
use gtk4::prelude::*;
use runtime_scene::Host;
use runtime_shared::accessibility::AccessibilityProps;
use runtime_vocabulary::caps::{ButtonOps, InputOps, LifecycleOps, TextInputOps, ViewOps};
use std::cell::RefCell;
use std::rc::Rc;

#[test]
fn on_focus_reports_keyboard_focus_within_the_wrapper() {
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
    let mut wrap = backend.borrow_mut().create_view(&a11y);
    let action = || runtime_shared::Action {
        method: "",
        inputs: Vec::new(),
        initial: Vec::new(),
        output: None,
        fire: Rc::new(|| {}),
    };
    let a = backend.borrow_mut().create_button("a", &action(), None, None, &a11y);
    let b = backend.borrow_mut().create_text_input("", None, Rc::new(|_| {}), None, None, false, &a11y);
    let outside = backend.borrow_mut().create_button("out", &action(), None, None, &a11y);
    backend.borrow_mut().insert(&mut wrap, a.clone());
    backend.borrow_mut().insert(&mut wrap, b.clone());
    backend.borrow_mut().insert(&mut root, wrap.clone());
    backend.borrow_mut().insert(&mut root, outside.clone());
    let log: Rc<RefCell<Vec<bool>>> = Rc::default();
    {
        let log = log.clone();
        backend.borrow_mut().install_focus_handler(&wrap, Rc::new(move |f| log.borrow_mut().push(f)));
    }
    backend.borrow_mut().finish(root.clone());
    window.set_child(Some(root.widget()));
    window.present();

    let ctx = gtk4::glib::MainContext::default();
    let pump = || {
        for _ in 0..200 {
            ctx.iteration(false);
        }
    };
    for _ in 0..10_000 {
        if a.widget().is_mapped() {
            break;
        }
        ctx.iteration(false);
    }
    if !a.widget().is_mapped() {
        eprintln!("SKIP: window never mapped in this environment");
        return;
    }
    let take = || std::mem::take(&mut *log.borrow_mut());

    let last = || log.borrow().last().copied();

    // Keyboard focus (focus-visible) lands on a child: the wrapper hears it.
    window.set_focus_visible(true);
    a.widget().grab_focus();
    pump();
    assert_eq!(last(), Some(true), "keyboard focus entering the wrapper: {:?}", log.borrow());

    // Moving between two children stays within.
    b.widget().grab_focus();
    pump();
    assert_eq!(last(), Some(true), "a → the field stays within the wrapper");

    // Leaving the subtree.
    outside.widget().grab_focus();
    pump();
    assert_eq!(last(), Some(false), "focus left the wrapper");
    take();

    // Pointer-style focus (no focus ring) on the button: not keyboard focus.
    window.set_focus_visible(false);
    a.widget().grab_focus();
    pump();
    assert!(!log.borrow().contains(&true), "click-style focus is not keyboard focus: {:?}", log.borrow());

    // A text field shows focus however it arrived.
    b.widget().grab_focus();
    pump();
    assert_eq!(last(), Some(true), "a text field is keyboard focus on any focus");

    // GTK turns focus-visible on with the first Tab; a button focused by
    // click then reports once the ring shows.
    a.widget().grab_focus();
    pump();
    assert_eq!(last(), Some(false));
    window.set_focus_visible(true);
    pump();
    assert_eq!(last(), Some(true), "keyboard use while focus is inside reports it");
}
