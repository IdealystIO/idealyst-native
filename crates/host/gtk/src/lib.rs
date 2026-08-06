//! GTK4 desktop host for the native `backend-linux` GTK backend.
//!
//! [`run`] opens a real `gtk::ApplicationWindow`, builds a
//! [`LinuxBackend`](backend_linux::LinuxBackend) rooted at it, mounts
//! the app tree via `backend_linux::newcore::start`, and hands control to the
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
use runtime_core::Element;

use backend_linux::newcore::{self, NewCoreApp};
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

/// As [`run`], but invokes `register` on the scene [`Registry`] before the
/// tree realizes — the seam where an app installs its SDK payload handlers
/// (codeblock, table, svg, …). Runs after `register_builtins`, mirroring
/// `host_appkit::run_with`.
///
/// The hook takes the REGISTRY, not the backend: `Element::External` and
/// the per-backend External table are gone in v2, so a handler that isn't
/// registered here has no entry at all and realizing that payload panics
/// by design.
///
/// [`Registry`]: runtime_scene::Registry
pub fn run_with<R, F>(opts: RunOptions, register: R, build_ui: F) -> i32
where
    R: FnOnce(&mut runtime_scene::Registry<LinuxBackend>) + 'static,
    F: FnOnce() -> Element + 'static,
{
    // Install the scheduler before the event loop runs (and therefore
    // before `activate` mounts the tree): author `effect!` blocks fire
    // `after_ms` / `raf_loop` during mount, and without a scheduler
    // those fall into the inert fallbacks and every animation freezes.
    scheduler::install();
    scheduler::install_render_loop();

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
        Box<dyn FnOnce(&mut runtime_scene::Registry<LinuxBackend>)>,
        Box<dyn FnOnce() -> Element>,
    );
    let init: Rc<RefCell<Option<Init>>> = Rc::new(RefCell::new(Some((
        opts,
        Box::new(register),
        Box::new(build_ui),
    ))));

    // Keep the mounted `NewCoreApp` (and the backend) alive for the
    // window's lifetime. v2 replaced the reactive `Owner` with this: it
    // owns the realized tree, the registry and the world, and drops them
    // in field order on `stop()`.
    let keep: Rc<RefCell<Option<(NewCoreApp, Rc<RefCell<LinuxBackend>>)>>> =
        Rc::new(RefCell::new(None));
    let keep_for_activate = keep.clone();

    app.connect_activate(move |app| {
        let Some((opts, register, build_ui)) = init.borrow_mut().take() else {
            return;
        };

        let window = gtk4::ApplicationWindow::new(app);
        window.set_title(Some(&opts.title));
        window.set_default_size(opts.width, opts.height);

        let backend = LinuxBackend::new(window.clone().upcast());
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
        // v2 boot: `newcore::start` installs the time source, builds the
        // scene `Registry` (builtins, then the app's own handlers), enters
        // the world and realizes the tree. It replaces `runtime_core::mount`,
        // which went away with the old walker.
        //
        // `register` runs on the REGISTRY, not the backend: SDK payloads
        // (codeblock, table, svg, …) install typed handlers there now that
        // `Element::External` and the per-backend External table are gone.
        // Passing a no-op here — as this host did briefly — silently left
        // every SDK unregistered, so any payload realize would panic.
        let app_handle = newcore::start(backend_rc.clone(), register, build_ui);
        let backend_for_pump = backend_rc.clone();
        gtk4::glib::source::timeout_add_local(std::time::Duration::from_millis(16), move || {
            if let Ok(b) = backend_for_pump.try_borrow() {
                b.pump();
            }
            gtk4::glib::ControlFlow::Continue
        });
        *keep_for_activate.borrow_mut() = Some((app_handle, backend_rc));

        window.present();
    });

    let exit = app.run();
    // Unmount explicitly before returning, on the GTK main thread while
    // its TLS is still intact — `stop()` runs the reactive scope cleanups
    // and uninstalls the flush driver / dispatch hook, none of which a
    // plain drop of the tuple would do.
    if let Some((app_handle, _backend)) = keep.borrow_mut().take() {
        app_handle.stop();
    }
    exit.into()
}
