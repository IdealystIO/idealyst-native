//! An app rooted in several nodes, or in a reactive region.
//!
//! REGRESSION: the scene hands such an app's root over as an anchor, which
//! has no layout box, so `run_layout` computed nothing and the window stayed
//! empty. Several root nodes stack in the window; a lone branch fills it.
//!
//! GTK may be driven only from the thread that ran `gtk::init`, so this is
//! its own test binary with exactly one GTK `#[test]`. Skips rather than
//! fails with no display.

#![cfg(target_os = "linux")]

use std::cell::RefCell;
use std::rc::Rc;

use backend_linux::{gtk4, newcore, LinuxBackend};
use gtk4::prelude::*;
use runtime_scene::Element;
use runtime_shared::{Length, StyleRules, StyleSheet};
use runtime_vocabulary::builders::view;

const W: f32 = 400.0;
const H: f32 = 300.0;

fn tall(h: f32) -> Element {
    view()
        .style(Rc::new(StyleSheet::new(move |_| StyleRules {
            height: Some(Length::Px(h).into()),
            ..Default::default()
        })))
        .build()
}

/// Frames of the root widget's children, after a layout pass.
fn frames(build: impl FnOnce() -> Element) -> Vec<(f32, f32, f32, f32)> {
    let window = gtk4::Window::new();
    let backend = Rc::new(RefCell::new(LinuxBackend::new(window.clone())));
    backend.borrow_mut().set_self_ref(Rc::downgrade(&backend));
    let app = newcore::start(backend.clone(), |_r| {}, build);
    backend.borrow_mut().run_layout(W, H);
    let root = window.child().expect("the root is the window's child");
    let b = backend.borrow();
    let mut out = Vec::new();
    let mut c = root.first_child();
    while let Some(w) = c {
        if let Some(f) = b.node_id_of_widget(&w).and_then(|id| b.node_frame(id)) {
            out.push(f);
        }
        c = w.next_sibling();
    }
    drop(b);
    app.stop();
    window.destroy();
    out
}

#[test]
fn regression_an_app_rooted_in_several_nodes_or_a_branch_is_laid_out() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display / GTK init failed");
        return;
    }
    let several = frames(|| runtime_scene::fragment(vec![tall(40.0), tall(50.0)]));
    assert_eq!(several, [(0.0, 0.0, W, 40.0), (0.0, 40.0, W, 50.0)]);
    let branch = frames(|| runtime_scene::dyn_element(|| view().build()));
    assert_eq!(branch, [(0.0, 0.0, W, H)]);
}
