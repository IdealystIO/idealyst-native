//! Hover / press / focus style variants must actually light up.
//!
//! ## The bug this pins
//!
//! `StyleOps::handles_states_natively()` is `false` on this backend — the
//! trait default, and correct for GTK (no CSS pseudo-class layer to hand
//! states off to). That answer selects the EVENT-DRIVEN path: the
//! framework hands each stateful node a setter via `attach_states` and the
//! BACKEND must call it from real input.
//!
//! `attach_states` has a no-op default, so never overriding it compiles,
//! renders every node's BASE style correctly, and lights no variant ever.
//! Every hover highlight, press feedback and focus ring in idea-ui was
//! dead on Linux, and neither the compiler nor a green suite said a word.
//!
//! The test drives GTK's own controllers (the ones a compositor would
//! drive) and asserts on the RE-APPLIED style, not on the setter having
//! been called — a test that checked the callback would pass against a
//! backend that wired the events but never re-styled.

#![cfg(target_os = "linux")]

use std::cell::RefCell;
use std::rc::Rc;

use backend_linux::{gtk4, newcore, LinuxBackend};
use gtk4::prelude::*;
use runtime_shared::{Length, StyleRules, StyleSheet};
use runtime_vocabulary::builders::view;

const WIN_W: f32 = 600.0;
const WIN_H: f32 = 400.0;

/// Walk `w` and its descendants for a controller of type `T`.
fn controllers_of<T: IsA<gtk4::EventController> + IsA<gtk4::glib::Object>>(
    w: &gtk4::Widget,
) -> Vec<T> {
    let mut out = Vec::new();
    for c in &w.observe_controllers() {
        if let Ok(c) = c.expect("controller").downcast::<T>() {
            out.push(c);
        }
    }
    out
}

fn find_with_motion(root: &gtk4::Widget) -> Option<gtk4::Widget> {
    if !controllers_of::<gtk4::EventControllerMotion>(root).is_empty() {
        return Some(root.clone());
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        if let Some(f) = find_with_motion(&c) {
            return Some(f);
        }
        child = c.next_sibling();
    }
    None
}

/// A sheet whose `hover` variant widens the node from 100 to 300 — a
/// property this backend pushes straight into Taffy, so the effect is
/// observable in the node's frame without any paint inspection.
fn hoverable() -> Rc<StyleSheet> {
    Rc::new(
        StyleSheet::new(|_| StyleRules {
            width: Some(Length::Px(100.0).into()),
            height: Some(Length::Px(40.0).into()),
            ..Default::default()
        })
        .variant("__state_hovered", "on", |_| StyleRules {
            width: Some(Length::Px(300.0).into()),
            ..Default::default()
        })
        .variant("__state_pressed", "on", |_| StyleRules {
            width: Some(Length::Px(150.0).into()),
            ..Default::default()
        }),
    )
}


/// Drain the main loop AND pump the backend. `apply_style` writes the new
/// Taffy style, but the frame only reaches GTK when something
/// re-allocates the root — in a real app that is host-gtk's 16 ms
/// `pump()` timeout, which this stands in for.
fn settle(ctx: &gtk4::glib::MainContext, backend: &Rc<RefCell<LinuxBackend>>) {
    // Drain first so the state flip's flush microtask runs and
    // `apply_style` pushes the new rules into Taffy...
    for _ in 0..2_000 {
        ctx.iteration(false);
    }
    // ...then run the layout pass explicitly. In a real app host-gtk's
    // 16 ms `pump()` drives this off the frame clock; under `cargo test`
    // the window gets no frame callbacks, so `queue_allocate` alone
    // never produces a `size_allocate` and the Taffy frame would never
    // reach the widget. Calling `run_layout` directly is the same work
    // that callback does, without depending on a compositor.
    if let Ok(mut b) = backend.try_borrow_mut() {
        b.run_layout(WIN_W, WIN_H);
    }
    for _ in 0..500 {
        ctx.iteration(false);
    }
}

#[test]
fn regression_hover_variant_restyles_the_node() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display / GTK init failed");
        return;
    }

    let window = gtk4::Window::new();
    window.set_default_size(WIN_W as i32, WIN_H as i32);
    let backend = Rc::new(RefCell::new(LinuxBackend::new(window.clone())));
    backend.borrow_mut().set_self_ref(Rc::downgrade(&backend));

    let app = newcore::start(backend.clone(), |_r| {}, || {
        view()
            .children(vec![view().style(hoverable()).build()])
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
    settle(&ctx, &backend);

    let root = window.child().expect("root attached by finish()");
    let node = find_with_motion(&root).expect(
        "a node with a `hover` variant must get an EventControllerMotion from \
         attach_states — none present means the cap is still the no-op default",
    );

    // Read the TAFFY frame, which is what `apply_style` feeds and what the
    // layout pass then writes into GTK. Reading GTK's allocation instead
    // would make the assertion depend on a live frame clock.
    let width_now = |id: u64| backend.borrow().node_frame(id).map(|f| f.2).unwrap_or(-1.0);
    let node_id = backend.borrow().node_id_of_widget(&node).expect("styled node id");
    assert_eq!(width_now(node_id), 100.0, "base width before any hover");

    // Drive GTK's own motion controller, as the compositor would.
    for m in controllers_of::<gtk4::EventControllerMotion>(&node) {
        m.emit_by_name::<()>("enter", &[&10.0f64, &10.0f64]);
    }
    settle(&ctx, &backend);
    assert_eq!(
        width_now(node_id),
        300.0,
        "pointer-enter must flip StateBits::HOVERED, re-resolve the sheet and \
         re-apply it. Still 100 means `attach_states` never wired the event (or \
         wired it without scheduling the flush that commits the re-style).",
    );

    for m in controllers_of::<gtk4::EventControllerMotion>(&node) {
        m.emit_by_name::<()>("leave", &[]);
    }
    settle(&ctx, &backend);
    assert_eq!(
        width_now(node_id),
        100.0,
        "pointer-leave must clear HOVERED and fall back to the base rules; a node \
         stuck wide would mean leave was never wired",
    );

    // --- press: light on press, and CLEAR ON CANCEL ------------------
    //
    // Same test function on purpose: GTK4 must be driven from the thread
    // that ran `gtk::init`, and cargo gives every `#[test]` its own
    // thread — a second GTK test in this binary dies with "Attempted to
    // initialize GTK from two different threads". The crate's
    // `layout_tests` module keeps all its GTK checks in one function for
    // exactly this reason.
    //
    // `cancel` matters as much as `released`: a press that ends outside
    // the widget (dragged off, or GTK reassigning the sequence to a
    // scroll) fires `cancel`. Wiring only `released` leaves the node
    // stuck in its pressed style forever, which is worse than having no
    // press state at all.
    for g in controllers_of::<gtk4::GestureClick>(&node) {
        g.emit_by_name::<()>("pressed", &[&1i32, &5.0f64, &5.0f64]);
    }
    settle(&ctx, &backend);
    assert_eq!(
        width_now(node_id),
        150.0,
        "press must light the `__state_pressed` variant",
    );

    for g in controllers_of::<gtk4::GestureClick>(&node) {
        g.emit_by_name::<()>("cancel", &[&None::<gtk4::gdk::EventSequence>]);
    }
    settle(&ctx, &backend);
    assert_eq!(
        width_now(node_id),
        100.0,
        "a cancelled press must clear PRESSED. Still 150 means only `released` \
         was wired, so dragging off a button leaves it pressed forever",
    );

    app.stop();
}
