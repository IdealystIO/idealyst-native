//! Navigating to a docs page must not park keyboard focus in its first
//! code panel.
//!
//! ## The bug this pins
//!
//! `codeblock`'s GTK leaf used to build its `gtk::Label` with
//! `set_selectable(true)`. `gtk_label_set_selectable()` turns the
//! widget's `focusable` property on, and GTK gives keyboard focus to the
//! first focusable widget in a freshly mapped subtree — so every
//! navigation parked focus in that page's FIRST code panel. It rendered
//! with a focus caret (reported as "the first codeblock seems
//! highlighted") and swallowed the arrow keys that should have scrolled
//! the page.
//!
//! `codeblock`'s own unit test pins the leaf in isolation. This one pins
//! the OBSERVABLE symptom in the real docs tree, because that is where
//! the ingredients actually meet: a code-bearing page mounting into a
//! scroll shell, with focus previously held elsewhere.
#![cfg(target_os = "linux")]
use backend_linux::{gtk4, newcore, LinuxBackend};
use gtk4::prelude::*;

type Backend = std::rc::Rc<std::cell::RefCell<LinuxBackend>>;

/// Drive GTK for `ms` of real wall-clock, pumping the backend the way
/// `host_gtk`'s 60 Hz timeout does. Real time has to pass: layout,
/// idle-deferred viewport publishes and navigation all land on GLib
/// sources, and a tight non-blocking `iteration` loop would spin through
/// them without ever letting a timeout fire.
fn pump(backend: &Backend, ms: u64) {
    let ctx = gtk4::glib::MainContext::default();
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_millis(ms) {
        if let Ok(b) = backend.try_borrow() {
            b.pump();
        }
        ctx.iteration(false);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

fn walk(w: &gtk4::Widget, out: &mut Vec<gtk4::Widget>) {
    out.push(w.clone());
    let mut c = w.first_child();
    while let Some(ch) = c {
        walk(&ch, out);
        c = ch.next_sibling();
    }
}

fn click_gesture(w: &gtk4::Widget) -> Option<gtk4::GestureClick> {
    w.observe_controllers()
        .into_iter()
        .find_map(|c| c.ok().and_then(|c| c.downcast::<gtk4::GestureClick>().ok()))
}

/// Labels directly under `root`, paired with their text.
fn labels(root: &gtk4::Widget) -> Vec<gtk4::Label> {
    let mut all = Vec::new();
    walk(root, &mut all);
    all.iter()
        .filter_map(|w| w.downcast_ref::<gtk4::Label>())
        .cloned()
        .collect()
}

#[test]
fn regression_code_panel_does_not_steal_focus_on_navigation() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display");
        return;
    }

    // Install the scheduler the real host installs. Without it,
    // `after_ms` runs SYNCHRONOUSLY on native, so any self-rescheduling
    // animation (`Progress(mode = Simulated)`, `Skeleton`'s shimmer)
    // recurses into itself and overflows the stack — the test would be
    // exercising a runtime configuration production never uses.
    host_gtk::install_scheduler();
    let window = gtk4::Window::new();
    window.set_default_size(1280, 860);
    // Seed the viewport before realize, as `host_gtk::run_with` does, so
    // the shell resolves its DESKTOP breakpoint (pinned sidebar) — the
    // sidebar link this test clicks only exists in that layout.
    runtime_shared::set_viewport_size(runtime_shared::ViewportSize {
        width: 1280.0,
        height: 860.0,
    });
    let backend: Backend =
        std::rc::Rc::new(std::cell::RefCell::new(LinuxBackend::new(window.clone().upcast())));
    backend
        .borrow_mut()
        .set_self_ref(std::rc::Rc::downgrade(&backend));
    let app = newcore::start(
        backend.clone(),
        idea_ui_docs::register_scene_extensions,
        idea_ui_docs::app,
    );
    window.present();
    pump(&backend, 2000);
    if !window.is_mapped() {
        eprintln!("SKIP: window never mapped");
        return;
    }
    let root = window.child().expect("realized root");

    // Navigate to a page that HAS code panels. "Stack" is picked by its
    // sidebar row geometry (x = 16, width = 220) so the catalog card of
    // the same name on the Overview page can't match.
    let mut all = Vec::new();
    walk(&root, &mut all);
    let row = all
        .iter()
        .find(|w| {
            if click_gesture(w).is_none() {
                return false;
            }
            let Some(b) = w.compute_bounds(&root) else {
                return false;
            };
            if (b.width() - 220.0).abs() > 1.0 || (b.x() - 16.0).abs() > 1.0 {
                return false;
            }
            let texts: Vec<String> = labels(w).iter().map(|l| l.text().to_string()).collect();
            texts == vec!["Stack".to_string()]
        })
        .cloned()
        .expect("sidebar row for the Stack page");
    let gesture = click_gesture(&row).expect("sidebar row is a Pressable");
    let b = row.compute_bounds(&root).expect("row bounds");
    let (cx, cy) = (
        (b.x() + b.width() / 2.0) as f64,
        (b.y() + b.height() / 2.0) as f64,
    );
    gesture.emit_by_name::<()>("pressed", &[&1i32, &cx, &cy]);
    gesture.emit_by_name::<()>("released", &[&1i32, &cx, &cy]);
    pump(&backend, 2500);

    // The Stack page must actually have mounted — otherwise the
    // assertions below would pass vacuously on a page with no code.
    let page_labels = labels(&root);
    let code_panels: Vec<&gtk4::Label> = page_labels
        .iter()
        .filter(|l| l.text().contains("StackGap::"))
        .collect();
    assert!(
        !code_panels.is_empty(),
        "navigation did not reach the Stack page — no code panel text found, \
         so this test would not be exercising anything"
    );

    for l in &code_panels {
        assert!(
            !l.is_focusable(),
            "code panel is focusable: it becomes the newly mounted page's \
             initial focus target and renders with a focus caret"
        );
        assert!(!l.has_focus(), "code panel took keyboard focus on navigation");
        assert!(
            l.selection_bounds().is_none(),
            "code panel rendered with a text selection"
        );
    }
    let focused = gtk4::prelude::GtkWindowExt::focus(&window);
    assert!(
        focused
            .as_ref()
            .map(|w| w.downcast_ref::<gtk4::Label>().is_none())
            .unwrap_or(true),
        "keyboard focus landed on a GtkLabel after navigation \
         (was {:?}); a label is display text, not a focus stop",
        focused.map(|w| w.type_().name().to_string())
    );
    app.stop();
}
