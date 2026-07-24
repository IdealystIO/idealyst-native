//! Is an absolutely-positioned pressable actually clickable where it
//! paints?
//!
//! `IdealystView` positions children by handing a `GskTransform` to
//! `gtk_widget_allocate`, and GTK's picking has to follow that same
//! transform. If it doesn't, a control paints in the right place and
//! answers clicks nowhere — indistinguishable from a dead button, and
//! invisible to every layout dump (the frames all look correct).
//!
//! # Why an integration test with a real window
//!
//! `gtk_widget_pick` returns `None` for an unmapped widget, so the same
//! check inside the lib's unit test would "fail" on every tree
//! regardless of whether picking works — a false positive that looks
//! exactly like the real bug. Only a presented window gives mapped
//! widgets, and that needs its own process (GTK is single-threaded and
//! the lib's tests already own one `gtk_init`).
//!
//! Skips rather than fails with no display, so it stays runnable
//! headless.

#![cfg(target_os = "linux")]

use backend_linux::gtk4;
use gtk4::prelude::*;
use runtime_core::accessibility::AccessibilityProps;
use runtime_core::{Backend, Length, Position, StyleRules, Tokenized};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const BTN_X: f32 = 120.0;
const BTN_Y: f32 = 60.0;
const BTN_SIZE: f32 = 44.0;

#[test]
fn absolutely_positioned_pressable_is_hit_testable_where_it_paints() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display / GTK init failed");
        return;
    }

    let window = gtk4::Window::new();
    window.set_default_size(400, 300);
    // The backend MUST live behind the same `Rc<RefCell<_>>` it hands
    // out as its self-reference: `finish` installs the root layout
    // callback through that weak ref, and a dead weak means the Taffy
    // pass never runs inside `size_allocate` — every child stays 0x0.
    // That is how the real host mounts it (`runtime_core::render`).
    let backend = Rc::new(RefCell::new(backend_linux::LinuxBackend::new(window.clone())));
    backend.borrow_mut().set_self_ref(Rc::downgrade(&backend));
    let a11y = AccessibilityProps::default();

    let mut root = backend.borrow_mut().create_view(&a11y);
    let pressed = Rc::new(Cell::new(0u32));
    let cb = pressed.clone();
    let button = backend
        .borrow_mut()
        .create_pressable(Rc::new(move || cb.set(cb.get() + 1)), &a11y);
    backend.borrow_mut().insert(&mut root, button.clone());
    // Park it away from the origin so a transform-blind pick can't pass
    // by accident.
    backend.borrow_mut().apply_style(
        &button,
        &Rc::new(StyleRules {
            position: Some(Position::Absolute),
            left: Some(Tokenized::Literal(Length::Px(BTN_X))),
            top: Some(Tokenized::Literal(Length::Px(BTN_Y))),
            width: Some(Tokenized::Literal(Length::Px(BTN_SIZE))),
            height: Some(Tokenized::Literal(Length::Px(BTN_SIZE))),
            ..StyleRules::default()
        }),
    );
    backend.borrow_mut().finish(root.clone());

    window.set_child(Some(root.widget()));
    window.present();

    // Pump until the tree is mapped and allocated.
    let ctx = gtk4::glib::MainContext::default();
    for _ in 0..10_000 {
        if button.widget().is_mapped() && button.widget().allocated_width() > 0 {
            break;
        }
        ctx.iteration(false);
    }
    if !button.widget().is_mapped() {
        eprintln!("SKIP: window never mapped in this environment");
        return;
    }

    // Where GTK actually put it, in root coordinates — read from GTK
    // rather than from Taffy, so this asserts what the compositor sees.
    // A 0x0 allocation here means the root layout callback never ran —
    // check the self-reference wiring above before believing anything
    // this test says about picking.
    assert!(
        button.widget().allocated_width() > 0,
        "button was never allocated; the Taffy pass did not run",
    );
    let bounds = button
        .widget()
        .compute_bounds(root.widget())
        .expect("button must have bounds within the root");
    assert!(
        (bounds.x() - BTN_X).abs() < 1.0 && (bounds.y() - BTN_Y).abs() < 1.0,
        "absolute positioning must place the pressable at ({BTN_X}, {BTN_Y}); \
         GTK reports ({}, {})",
        bounds.x(),
        bounds.y(),
    );

    let (cx, cy) = (
        (bounds.x() + bounds.width() / 2.0) as f64,
        (bounds.y() + bounds.height() / 2.0) as f64,
    );
    let hit = root.widget().pick(cx, cy, gtk4::PickFlags::DEFAULT);
    let found = hit
        .as_ref()
        .map(|w| w == button.widget() || w.is_ancestor(button.widget()))
        .unwrap_or(false);
    assert!(
        found,
        "pick() at the pressable's own centre ({cx}, {cy}) returned {:?} — GTK \
         is not hit-testing through the child transform, so the control is \
         unclickable despite painting in the right place",
        hit.map(|w| w.type_().name().to_string()),
    );
}
