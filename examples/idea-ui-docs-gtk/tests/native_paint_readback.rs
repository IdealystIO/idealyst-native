//! The GTK backend's `introspect_native` must report the paint GSK actually
//! drew, not the paint the framework asked for.
//!
//! `runtime_shared::introspect`'s cardinal rule is that every value comes from
//! the live platform object. This backend originally read background / border /
//! radius from `IdealystView::paint_model()` — the framework's own resolved
//! intent, one step short of an engine query. It now walks the widget's GSK
//! render node, the tree GTK rasterizes.
//!
//! The pill assertion below is what makes the difference observable: a pill's
//! author radius is a sentinel (`999`), which `IdealystView::snapshot` clamps to
//! half the shorter side when it paints. The paint model holds the sentinel; the
//! render node holds the clamp. Web's `getComputedStyle` reports the clamped
//! value too, so reading the model made every pill a spurious cross-platform
//! divergence.
#![cfg(target_os = "linux")]
use backend_linux::{gtk4, newcore, LinuxBackend};
use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

type Backend = Rc<RefCell<LinuxBackend>>;

fn pump(b: &Backend, ms: u64) {
    let ctx = gtk4::glib::MainContext::default();
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_millis(ms) {
        if let Ok(x) = b.try_borrow() {
            x.pump();
        }
        ctx.iteration(false);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

#[test]
fn native_paint_comes_from_the_render_tree_not_the_style() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display");
        return;
    }
    host_gtk::install_scheduler();
    let window = gtk4::Window::new();
    window.set_default_size(1280, 800);
    runtime_shared::set_viewport_size(runtime_shared::ViewportSize {
        width: 1280.0,
        height: 800.0,
    });
    let backend: Backend = Rc::new(RefCell::new(LinuxBackend::new(window.clone().upcast())));
    backend.borrow_mut().set_self_ref(Rc::downgrade(&backend));
    let app = newcore::start(
        backend.clone(),
        idea_ui_docs::register_scene_extensions,
        idea_ui_docs::app,
    );
    window.present();
    pump(&backend, 2500);
    if !window.is_mapped() {
        eprintln!("SKIP: window never mapped");
        return;
    }

    let mut with_bg = 0usize;
    let mut with_radius = 0usize;
    let mut pills: Vec<(f32, f32)> = Vec::new();
    {
        let b = backend.borrow();
        for id in 1..500u64 {
            let Some(n) = b.introspect_native_for_test(id) else {
                continue;
            };
            use runtime_shared::introspect::NativeValue as V;
            if matches!(n.props.get("background_color"), Some(V::Color(_))) {
                with_bg += 1;
            }
            let radius = match n.props.get("corner_radius") {
                Some(V::Length(r)) => {
                    with_radius += 1;
                    *r
                }
                _ => continue,
            };
            // A pill: the drawn radius is half the shorter side. Collect the
            // ones where the radius is at least half the height, which only
            // happens for a clamped sentinel.
            if n.frame.height > 8.0 && radius >= n.frame.height / 2.0 - 0.51 {
                pills.push((radius, n.frame.height));
            }
        }
    }

    assert!(
        with_bg > 20,
        "only {with_bg} elements reported a background colour — the render-tree \
         walk is not finding the fills it should (the docs app's cards, chips, \
         badges and buttons all paint one)"
    );
    assert!(
        with_radius > 20,
        "only {with_radius} elements reported a corner radius"
    );
    assert!(
        !pills.is_empty(),
        "no pill-shaped element reported a clamped radius — the docs app has \
         chips and dots whose author radius is a `999` sentinel, so at least one \
         should come back as half its height"
    );
    for (radius, height) in &pills {
        assert!(
            *radius <= height / 2.0 + 0.51,
            "a drawn corner radius ({radius}) exceeded half the element's height \
             ({height}) — that is the author's unclamped sentinel leaking through, \
             which means the read came from the style rather than the render tree",
        );
        assert!(
            *radius < 900.0,
            "corner radius {radius} is the raw `999` pill sentinel — \
             `introspect_native` must report what GSK drew, not what the \
             stylesheet asked for",
        );
    }
    eprintln!(
        "read {with_bg} backgrounds, {with_radius} radii, {} clamped pills",
        pills.len()
    );
    app.stop();
}
