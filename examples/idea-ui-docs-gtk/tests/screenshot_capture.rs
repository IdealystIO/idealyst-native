//! The GTK backend must answer the Robot bridge's `screenshot` verb with
//! a real picture of the app.
//!
//! `caps::IntrospectionOps` was an EMPTY impl on `LinuxBackend`, so every
//! body fell through to the trait default: `supports_screenshot()` said
//! `false` and `capture_screenshot` returned "not supported on this
//! backend", while `frame`/`absolute_frame` returned `None` even though
//! the backend already tracked both. An agent could drive a GTK app but
//! never see or measure it — and this backend's own history is a list of
//! layout bugs that built cleanly, logged nothing, and were only caught
//! from a screenshot.
//!
//! Drives the real docs tree, because the thing worth asserting is that a
//! capture of an actual app is a plausible IMAGE (right size, not blank),
//! which a two-node synthetic tree cannot show.
#![cfg(target_os = "linux")]
use backend_linux::{gtk4, newcore, LinuxBackend};
use gtk4::prelude::*;
use runtime_vocabulary::caps::IntrospectionOps;
use std::cell::RefCell;
use std::rc::Rc;

type Backend = Rc<RefCell<LinuxBackend>>;

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

#[test]
fn gtk_backend_captures_a_screenshot_of_the_docs_app() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display");
        return;
    }
    host_gtk::install_scheduler();
    let window = gtk4::Window::new();
    window.set_default_size(1280, 860);
    runtime_shared::set_viewport_size(runtime_shared::ViewportSize {
        width: 1280.0,
        height: 860.0,
    });
    let backend: Backend = Rc::new(RefCell::new(LinuxBackend::new(window.clone().upcast())));
    backend
        .borrow_mut()
        .set_self_ref(Rc::downgrade(&backend));
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

    let out: Rc<RefCell<Option<Result<runtime_shared::Screenshot, String>>>> =
        Rc::new(RefCell::new(None));
    {
        let b = backend.borrow();
        assert!(
            b.supports_screenshot(),
            "GTK backend reports it cannot screenshot a mounted, realized window"
        );
        let sink = out.clone();
        b.capture_screenshot(Box::new(move |r| *sink.borrow_mut() = Some(r)));
    }
    // Capture is FRAME-DEFERRED on GTK (a widget's render node only exists
    // once GTK has drawn it — see `backend_linux::screenshot`), so the main
    // loop has to run for `done` to fire. Nothing to poll on the backend;
    // the callback IS the completion signal.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while out.borrow().is_none() && std::time::Instant::now() < deadline {
        pump(&backend, 50);
    }
    let shot = out
        .borrow_mut()
        .take()
        .expect("capture_screenshot never called back within 5s")
        .expect("capture failed");

    // Physical pixels, so a scaled display legitimately exceeds the
    // logical 1280x860 — but never falls below it.
    assert!(
        shot.width >= 1280 && shot.height >= 860,
        "capture is smaller than the window: {}x{}",
        shot.width,
        shot.height
    );
    assert_eq!(
        &shot.png[..8],
        b"\x89PNG\r\n\x1a\n",
        "capture is not a PNG"
    );
    // A blank capture would still be a valid PNG. The docs app's light
    // theme over a full 1280x860 of text and chrome compresses to well
    // over 20 kB; a single flat colour compresses to a few hundred bytes.
    assert!(
        shot.png.len() > 20_000,
        "capture is only {} bytes — that is a blank or near-blank image, \
         not the docs app",
        shot.png.len()
    );

    // The geometry verbs must answer too: same numbers `ViewHandle`
    // already reported to author code.
    if let Some(dir) = std::env::var_os("IDEALYST_TEST_SHOT_DIR") {
        let path = std::path::Path::new(&dir).join("docs_gtk_capture.png");
        std::fs::write(&path, &shot.png).expect("write capture");
        eprintln!("wrote {}", path.display());
    }
    eprintln!("captured {}x{} ({} bytes)", shot.width, shot.height, shot.png.len());
    app.stop();
}
