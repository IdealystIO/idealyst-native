//! Navigating between routes must not abort the process.
//!
//! ## The bug this pins
//!
//! `attach_states` installs GTK hover/press/focus controllers whose
//! setter writes a signal owned by the NODE'S reactive scope. A route
//! change drops that scope while the world stays alive — and GTK emits
//! `focus-leave` right then, because the framework is unparenting a
//! focused widget. The setter writes through a freed slot and panics
//! inside a GObject signal trampoline, which is `extern "C"` and cannot
//! unwind: the process ABORTS rather than reporting a panic.
//!
//! `on_node_unstyled` must therefore detach the controllers (and clear
//! their liveness guard) when the style scope dies. See
//! `backend_linux::states`.
//!
//! This drives the REAL docs tree rather than a synthetic one: the crash
//! needs a focused widget inside a scope that a navigation actually
//! tears down, which is exactly what a sidebar link does and what a
//! hand-built two-node tree does not reproduce by accident.
#![cfg(target_os = "linux")]
use backend_linux::{gtk4, newcore, LinuxBackend};
use gtk4::prelude::*;

fn is_click(w: &gtk4::Widget) -> bool {
    w.observe_controllers().into_iter().any(|c| {
        c.map(|c| c.downcast::<gtk4::GestureClick>().is_ok()).unwrap_or(false)
    })
}

#[test]
fn regression_route_change_does_not_abort_on_stale_state_setter() {
    if gtk4::init().is_err() { eprintln!("SKIP"); return; }
    let window = gtk4::Window::new();
    window.set_default_size(1280, 860);
    runtime_shared::set_viewport_size(runtime_shared::ViewportSize { width: 1280.0, height: 860.0 });
    let backend = std::rc::Rc::new(std::cell::RefCell::new(LinuxBackend::new(window.clone())));
    backend.borrow_mut().set_self_ref(std::rc::Rc::downgrade(&backend));
    let app = newcore::start(backend.clone(), idea_ui_docs::register_scene_extensions, || idea_ui_docs::app());
    window.present();
    let ctx = gtk4::glib::MainContext::default();
    for _ in 0..40_000 { if window.is_mapped() { break; } ctx.iteration(false); }
    if !window.is_mapped() { eprintln!("SKIP: unmapped"); return; }
    for _ in 0..5_000 { ctx.iteration(false); }

    let root = window.child().expect("root");
    // Click several distinct sidebar links in sequence: each unmounts the
    // previous screen's scope while the world stays alive, and moves focus.
    for round in 0..4 {
        let mut links: Vec<gtk4::Widget> = Vec::new();
        fn collect(w: &gtk4::Widget, root: &gtk4::Widget, out: &mut Vec<gtk4::Widget>) {
            if is_click(w) {
                if let Some(b) = w.compute_bounds(root) {
                    if (b.width() - 220.0).abs() < 1.0 && (b.height() - 34.0).abs() < 1.0 && b.y() > 90.0 {
                        out.push(w.clone());
                    }
                }
            }
            let mut c = w.first_child();
            while let Some(ch) = c { collect(&ch, root, out); c = ch.next_sibling(); }
        }
        collect(&root, &root, &mut links);
        if links.is_empty() { eprintln!("route-change: no links round {round}"); break; }
        let target = &links[(round * 3 + 2).min(links.len() - 1)];
        let b = target.compute_bounds(&root).unwrap();
        let (cx, cy) = ((b.x()+b.width()/2.0) as f64, (b.y()+b.height()/2.0) as f64);
        // Hover + focus it first, so the orphaned controllers are "live"
        // when the scope dies — the exact shape of the abort.
        for m in &target.observe_controllers() {
            let m = m.expect("c");
            if let Ok(mo) = m.clone().downcast::<gtk4::EventControllerMotion>() {
                mo.emit_by_name::<()>("enter", &[&cx, &cy]);
            }
            if let Ok(g) = m.downcast::<gtk4::GestureClick>() {
                g.emit_by_name::<()>("pressed", &[&1i32, &cx, &cy]);
                g.emit_by_name::<()>("released", &[&1i32, &cx, &cy]);
            }
        }
        for _ in 0..4_000 { if let Ok(bb)=backend.try_borrow(){bb.pump();} ctx.iteration(false); }
        eprintln!("route-change: round {round} survived");
    }
    eprintln!("route-change: all route changes survived");
    app.stop();
}
