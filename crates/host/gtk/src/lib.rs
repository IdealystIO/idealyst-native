//! GTK4 desktop host for the native `backend-linux` GTK backend.
//!
//! [`run`] opens a real `gtk::ApplicationWindow`, builds a
//! [`LinuxBackend`](backend_linux::LinuxBackend) rooted at it, mounts
//! the app tree via [`runtime_core::mount`], and hands control to the
//! GLib main loop. The framework's scheduler is installed (on the same
//! main loop) *before* the mount so animations advance.
//!
//! ```no_run
//! use host_gtk::RunOptions;
//! use runtime_core::{view, Element};
//!
//! fn app() -> Element {
//!     view(vec![]).into()
//! }
//!
//! fn main() {
//!     host_gtk::run(RunOptions { title: "Demo".into(), width: 900, height: 700 }, app);
//! }
//! ```
//!
//! The app tree is host-triple-agnostic — the same `app()` that runs on
//! web/iOS/Android renders here as native GTK widgets.

#![cfg(target_os = "linux")]

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use runtime_core::{Element, Owner};

use backend_linux::LinuxBackend;

mod scheduler;

/// Window configuration for [`run`].
pub struct RunOptions {
    pub title: String,
    /// Initial window width in logical pixels.
    pub width: i32,
    /// Initial window height in logical pixels.
    pub height: i32,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            title: "Idealyst".to_string(),
            width: 900,
            height: 700,
        }
    }
}

/// Run an app on the GTK backend. Blocks until the window closes;
/// returns the process exit code (0 = clean).
pub fn run<F>(opts: RunOptions, build_ui: F) -> i32
where
    F: FnOnce() -> Element + 'static,
{
    run_with(opts, |_| {}, build_ui)
}

/// As [`run`], but invokes `register` on the freshly-built
/// [`LinuxBackend`] before the tree mounts — the hook for registering
/// `Element::External` / navigator SDK handlers (mirrors the winit /
/// AppKit hosts' `run_with`).
pub fn run_with<R, F>(opts: RunOptions, register: R, build_ui: F) -> i32
where
    R: FnOnce(&mut LinuxBackend) + 'static,
    F: FnOnce() -> Element + 'static,
{
    // Install the scheduler before the event loop runs (and therefore
    // before `activate` mounts the tree): author `effect!` blocks fire
    // `after_ms` / `raf_loop` during mount, and without a scheduler
    // those fall into the inert fallbacks and every animation freezes.
    scheduler::install();

    // `NON_UNIQUE`: don't fold a second launch into a running instance
    // (GtkApplication's default single-instance behavior). A framework
    // runner should open an independent window per launch, not silently
    // forward `activate` to an already-running process.
    let app = gtk4::Application::builder()
        .application_id("ai.truday.idealyst.host")
        .flags(gtk4::gio::ApplicationFlags::NON_UNIQUE)
        .build();

    // `register` + `build_ui` are `FnOnce` but `connect_activate` takes
    // an `Fn`; stash them (with the options) in a slot the first
    // activation consumes.
    type Init = (
        RunOptions,
        Box<dyn FnOnce(&mut LinuxBackend)>,
        Box<dyn FnOnce() -> Element>,
    );
    let init: Rc<RefCell<Option<Init>>> = Rc::new(RefCell::new(Some((
        opts,
        Box::new(register),
        Box::new(build_ui),
    ))));

    // Keep the mounted `Owner` (and the backend) alive for the window's
    // lifetime — dropping the `Owner` tears the whole UI down.
    let keep: Rc<RefCell<Option<(Owner, Rc<RefCell<LinuxBackend>>)>>> =
        Rc::new(RefCell::new(None));
    let keep_for_activate = keep.clone();

    app.connect_activate(move |app| {
        let Some((opts, register, build_ui)) = init.borrow_mut().take() else {
            return;
        };

        let window = gtk4::ApplicationWindow::new(app);
        window.set_title(Some(&opts.title));
        window.set_default_size(opts.width, opts.height);

        let mut backend = LinuxBackend::new(window.clone().upcast());
        register(&mut backend);
        let backend_rc = Rc::new(RefCell::new(backend));
        // Give the backend a weak handle to itself so node handles
        // (ViewHandle/TextHandle) can route per-frame animation writes
        // back in. Must happen before mount builds the tree + handles.
        backend_rc
            .borrow_mut()
            .set_self_ref(Rc::downgrade(&backend_rc));

        // Mount. `mount` runs `finish` once, but the window isn't
        // allocated yet (size 0), so the first real layout happens in
        // the tick callback below once the size is known.
        // Static layout is driven by the root widget's `size_allocate`
        // (wired in `LinuxBackend::finish`), which GTK invokes on map +
        // resize. Animation, though, needs a steady frame beat: GTK's
        // frame clock idles on Wayland once drawing settles, and the
        // per-frame `set_animated_*` writes don't reliably wake it. A
        // 60 Hz GLib timeout that re-allocates + redraws the root keeps
        // the scene advancing while animations are live.
        //
        // Trade-off: this pumps a relayout + repaint every frame
        // unconditionally, so a fully idle app still spends ~60 fps of
        // layout/paint. Acceptable for the welcome scene (which animates
        // for its whole lifetime); a general app would want to gate the
        // pump on "is any animation active" (e.g. off the animation
        // clock's registration count) — a follow-on optimization.
        let owner = runtime_core::mount(backend_rc.clone(), build_ui);
        let backend_for_pump = backend_rc.clone();
        gtk4::glib::source::timeout_add_local(std::time::Duration::from_millis(16), move || {
            if let Ok(b) = backend_for_pump.try_borrow() {
                b.pump();
            }
            gtk4::glib::ControlFlow::Continue
        });
        *keep_for_activate.borrow_mut() = Some((owner, backend_rc));

        window.present();
    });

    let exit = app.run();
    // Drop the mounted tree explicitly before returning.
    keep.borrow_mut().take();
    exit.into()
}
