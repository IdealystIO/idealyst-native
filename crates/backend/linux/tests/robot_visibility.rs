//! The robot's `visible` and scroll state on GTK, against a presented
//! window: `is_shown` follows mapping (an ancestor's `set_visible(false)`
//! unmaps the subtree) and opacity up the parent chain, and `node_scroll`
//! reads the `ScrolledWindow`'s adjustments. Before, Linux had neither
//! read: "visible" meant "registered", and a scroll view reported nothing
//! (the navigator's scroll snapshot read `(0, 0)` too).
//!
//! One GTK `#[test]` per binary — GTK initialises on one thread only.

#![cfg(target_os = "linux")]

use backend_linux::gtk4;
use gtk4::prelude::*;
use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::{Length, StyleRules, Tokenized};
use runtime_scene::Host;
use runtime_vocabulary::caps::{IntrospectionOps, LifecycleOps, ScrollOps, StyleOps, ViewOps};
use std::cell::RefCell;
use std::rc::Rc;

fn sized(w: f32, h: f32) -> Rc<StyleRules> {
    Rc::new(StyleRules {
        width: Some(Tokenized::Literal(Length::Px(w))),
        height: Some(Tokenized::Literal(Length::Px(h))),
        // Fixed sizes: a column parent would otherwise shrink the tall
        // content to fit, leaving nothing to scroll.
        flex_shrink: Some(Tokenized::Literal(0.0)),
        ..StyleRules::default()
    })
}

#[test]
fn is_shown_and_scroll_follow_gtk() {
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
    let mut parent = backend.borrow_mut().create_view(&a11y);
    let child = backend.borrow_mut().create_view(&a11y);
    backend.borrow_mut().apply_style(&child, &sized(40.0, 40.0));
    backend.borrow_mut().insert(&mut parent, child.clone());
    backend.borrow_mut().insert(&mut root, parent.clone());
    let mut scroller = backend.borrow_mut().create_scroll_view(false, None, &a11y);
    backend.borrow_mut().apply_style(&scroller, &sized(200.0, 100.0));
    let tall = backend.borrow_mut().create_view(&a11y);
    backend.borrow_mut().apply_style(&tall, &sized(200.0, 1000.0));
    backend.borrow_mut().insert(&mut scroller, tall);
    backend.borrow_mut().insert(&mut root, scroller.clone());
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
    if !pump(&|| child.widget().is_mapped()) {
        eprintln!("SKIP: window never mapped in this environment");
        return;
    }
    let shown = || backend.borrow().is_shown(&child);
    assert_eq!(shown(), Some(true), "mapped, nothing hiding it");

    parent.widget().set_visible(false);
    pump(&|| !child.widget().is_mapped());
    assert_eq!(shown(), Some(false), "an ancestor is hidden");
    parent.widget().set_visible(true);
    pump(&|| child.widget().is_mapped());
    assert_eq!(shown(), Some(true));

    parent.widget().set_opacity(0.0);
    assert_eq!(shown(), Some(false), "an ancestor is fully transparent");
    parent.widget().set_opacity(1.0);

    // Scroll state: the adjustment's value, once the content is laid out.
    let vadj = scroller.widget().downcast_ref::<gtk4::ScrolledWindow>().unwrap().vadjustment();
    assert!(pump(&|| vadj.upper() > 500.0), "the content laid out taller than the viewport (upper {})", vadj.upper());
    assert_eq!(backend.borrow().node_scroll(&scroller), (0.0, 0.0));
    backend.borrow_mut().set_node_scroll(&scroller, 0.0, 240.0);
    assert_eq!(backend.borrow().node_scroll(&scroller), (0.0, 240.0));
    assert_eq!(backend.borrow().node_scroll(&child), (0.0, 0.0), "not a scroller");
}
