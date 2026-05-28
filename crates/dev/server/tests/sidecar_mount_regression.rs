//! Regression: the runtime-server sidecar's session thread MUST use
//! `runtime_core::mount(backend, app_fn)` rather than
//! `render(backend, app_fn())`.
//!
//! The bug this guards against: when the welcome example's
//! `coordinator::use_welcome()` (called from inside the user's `app()`)
//! schedules `after_ms_scoped(...)` for the Act-1 timeline, the
//! scope-anchored helper's `on_cleanup(move || drop(task))` needs an
//! active reactive scope to attach to. With `render(backend, app())`
//! the user's `app()` runs *before* the root scope exists, so the
//! cleanup gets silently dropped and the task is cancelled
//! immediately — every planet stayed at `opacity: 0` and the welcome
//! text never faded in.
//!
//! See also: [walker.rs:91-111][1] for the framework's own description
//! of the `render` vs `mount` distinction, and the
//! `project_mount_vs_render` memory for the macOS-host occurrence of
//! the same bug.
//!
//! [1]: ../../framework/core/src/walker.rs

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use dev_server::{scheduler, WireRecordingBackend};
use runtime_core::{
    after_ms_scoped, mount, render, Element, SafeAreaSides,
};

/// An "app" constructor that schedules a 0-ms scope-anchored timer
/// while building its tree. When invoked inside an active reactive
/// scope, the timer fires on the next `drive_pending`; outside a
/// scope, `on_cleanup` drops the cleanup closure (which owns the
/// `ScheduledTask`), cancelling the timer before it can fire.
fn make_app(fired: Rc<Cell<bool>>) -> impl FnOnce() -> Element + 'static {
    move || {
        after_ms_scoped(0, move || fired.set(true));
        Element::View {
            children: vec![],
            style: None,
            ref_fill: None,
            safe_area_sides: SafeAreaSides::NONE,
            on_touch: None,
            accessibility: Default::default(),
            test_id: None,
        }
    }
}

/// **The fix.** `mount(backend, app_fn)` runs the closure inside the
/// root reactive scope, so the `after_ms_scoped` inside it adopts
/// that scope. The timer survives `drive_pending` and fires.
#[test]
fn regression_mount_runs_after_ms_scoped_from_app_constructor() {
    scheduler::install();

    let recorder = WireRecordingBackend::new();
    let backend_rc = Rc::new(RefCell::new(recorder));
    let fired = Rc::new(Cell::new(false));
    let app = make_app(fired.clone());

    let _owner = mount(backend_rc, app);

    // Sleep is unnecessary — `after_ms_scoped(0, ...)` deadlines at
    // "now" and `drive_pending` checks `now >= deadline`. By the
    // time the next line runs, the deadline is in the past.
    scheduler::drive_pending();

    assert!(
        fired.get(),
        "after_ms_scoped scheduled from inside mount's closure must fire — \
         if this fails, the sidecar/host swallowed the timer (likely \
         reverted to `render(_, app())`)"
    );
}

/// **The bug.** `render(backend, app())` calls `app()` *before* the
/// root scope is established, so the `on_cleanup` inside
/// `after_ms_scoped` has no scope to attach to. The cleanup is
/// dropped immediately, which drops the `ScheduledTask`, which
/// cancels the registered deadline. `drive_pending` finds nothing
/// to fire.
#[test]
fn render_pre_built_tree_silently_cancels_scoped_timer() {
    scheduler::install();

    let recorder = WireRecordingBackend::new();
    let backend_rc = Rc::new(RefCell::new(recorder));
    let fired = Rc::new(Cell::new(false));
    let app = make_app(fired.clone());

    // `app()` runs *outside* any scope here — capture happens before
    // `render` builds the root scope.
    let _owner = render(backend_rc, app());

    scheduler::drive_pending();

    assert!(
        !fired.get(),
        "this assertion documents the bug: when `app()` is invoked \
         outside the root scope, `after_ms_scoped` cancels itself. \
         If this starts failing, `render(backend, app())` has been \
         made scope-aware — collapse this test into the mount-only \
         variant above."
    );
}
