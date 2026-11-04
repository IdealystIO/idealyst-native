//! A live `activity_indicator` size on a real `GtkSpinner`.
//!
//! REGRESSION: `update_activity_indicator_size` was the trait default, so
//! `size_dyn` was ignored after creation: the spinner kept its first size request
//! and Taffy its cached measure.
//!
//! One GTK `#[test]` per binary — GTK initialises on one thread only.

// `frame` is `robot`-only.
#![cfg(all(target_os = "linux", feature = "robot"))]

use backend_linux::gtk4;
use gtk4::prelude::*;
use runtime_scene::Host;
use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::primitives::activity_indicator::ActivityIndicatorSize;
use runtime_vocabulary::caps::{ActivityIndicatorOps, IntrospectionOps, LifecycleOps, ViewOps};
use std::cell::RefCell;
use std::rc::Rc;

#[test]
fn regression_linux_activity_indicator_ignored_a_live_size() {
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
    let spinner = backend.borrow_mut().create_activity_indicator(ActivityIndicatorSize::Small, None, &a11y);
    backend.borrow_mut().insert(&mut root, spinner.clone());
    backend.borrow_mut().finish(root.clone());
    window.set_child(Some(root.widget()));
    window.present();

    let ctx = gtk4::glib::MainContext::default();
    let pump = |until: &dyn Fn() -> bool| {
        for _ in 0..20_000 {
            if until() {
                return true;
            }
            ctx.iteration(false);
        }
        until()
    };
    let height = || backend.borrow().frame(&spinner).map_or(0.0, |f| f.height);
    if !pump(&|| spinner.widget().is_mapped()) {
        eprintln!("SKIP: window never mapped in this environment");
        return;
    }
    assert!(pump(&|| height() == 16.0), "Small is 16 px: {}", height());

    backend.borrow_mut().update_activity_indicator_size(&spinner, ActivityIndicatorSize::Large);
    assert!(pump(&|| height() == 32.0), "the layout follows the new size: {}", height());

    backend.borrow_mut().update_activity_indicator_size(&spinner, ActivityIndicatorSize::Small);
    assert!(pump(&|| height() == 16.0), "and back: {}", height());
}
