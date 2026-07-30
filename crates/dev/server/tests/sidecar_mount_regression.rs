//! Regression: a scope-anchored timer scheduled from INSIDE the app
//! constructor must survive to fire in a dev session.
//!
//! # The bug this guards against
//!
//! The welcome example's `coordinator::use_welcome()` — called from
//! inside the user's `app()` — schedules `after_ms_scoped(...)` for its
//! Act-1 timeline. That helper anchors its task to the ambient reactive
//! lifetime and is documented-inert when there is none: the captured
//! task drops immediately and the timer is cancelled before it can fire.
//! Symptom when it regresses: every planet stays at `opacity: 0` and the
//! welcome text never fades in.
//!
//! On the old core this was the `mount(backend, app_fn)` vs
//! `render(backend, app())` distinction — `render` invoked `app()`
//! BEFORE the root scope existed, so the anchor was missing and the
//! timer died. The sidecar had shipped the broken spelling.
//!
//! # Why this file changed shape
//!
//! The surviving core has exactly ONE boot entry
//! ([`dev_server::newcore::SceneSession::mount`]), so the two-spelling
//! trap it guarded is structurally gone — the deletion baseline files
//! the old pair under DIES-legit for that reason (§4.3). What is NOT
//! gone is the underlying requirement: `SceneSession::mount` must invoke
//! `app()` somewhere that `after_ms_scoped` can anchor to, or the
//! welcome bug comes straight back in a new spelling.
//!
//! `SceneSession::mount` runs `app()` inside `World::enter` but OUTSIDE
//! the `collect_owned` that `realize` opens. That still resolves an
//! anchor — `scoped_scheduling::current_anchor`'s `is_entered()` branch
//! mints one owned by the world root — but the distinction is one line
//! of ordering away from being wrong again, which is precisely why it
//! needs a test rather than a comment. The second test below pins the
//! inert half so the first one cannot pass vacuously.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use dev_server::newcore::SceneSession;
use dev_server::{scheduler, WireRecordingBackend};
use runtime_vocabulary::builders::view;
use runtime_vocabulary::scoped_scheduling::after_ms_scoped;

/// An "app" constructor that schedules a 0-ms scope-anchored timer while
/// building its tree. Invoked where an anchor is resolvable, the timer
/// fires on the next `drive_pending`; invoked with no ambient reactive
/// lifetime, `after_ms_scoped` is inert by contract and it never fires.
fn make_app(fired: Rc<Cell<bool>>) -> impl FnOnce() -> runtime_scene::Element + 'static {
    move || {
        after_ms_scoped(0, move || fired.set(true));
        view().build()
    }
}

/// **The contract.** `SceneSession::mount` invokes the app constructor
/// with a resolvable anchor, so a timer scheduled during the build
/// survives and fires.
#[test]
fn regression_scene_session_mount_runs_after_ms_scoped_from_app_constructor() {
    scheduler::install();

    let recorder = WireRecordingBackend::new();
    let fired = Rc::new(Cell::new(false));
    let app = make_app(fired.clone());

    let _session = SceneSession::mount(&recorder, |_r| {}, app);

    // No sleep needed — `after_ms_scoped(0, ...)` deadlines at "now" and
    // `drive_pending` fires anything whose deadline has passed.
    scheduler::drive_pending();

    assert!(
        fired.get(),
        "after_ms_scoped scheduled from inside the app constructor must fire — \
         if this fails, SceneSession::mount stopped invoking `app()` under a \
         resolvable reactive anchor and the welcome-timeline bug is back"
    );
}

/// **The inert half**, so the test above cannot pass vacuously.
///
/// Called with NO ambient world, `after_ms_scoped` drops its task
/// immediately — the documented old-core posture, carried over
/// unchanged. This is the shape the old `render(backend, app())` spelling
/// produced, reproduced here directly now that the spelling itself is
/// gone.
#[test]
fn after_ms_scoped_outside_any_reactive_lifetime_is_inert() {
    scheduler::install();

    let fired = Rc::new(Cell::new(false));
    let app = make_app(fired.clone());

    // Run the constructor with no world entered and no collector open.
    let element = app();
    drop(element);

    scheduler::drive_pending();

    assert!(
        !fired.get(),
        "this assertion documents the contract: with no ambient reactive \
         lifetime, `after_ms_scoped` cancels itself. If it starts failing, \
         the helper became world-independent — fold this into the test above."
    );
}

/// The mounted session records the app's tree as wire commands and ends
/// the initial batch with `Finish`, so a replay client knows the mount is
/// complete. (The old file asserted this implicitly by constructing a
/// `WireRecordingBackend`; making it explicit keeps the target honest
/// about what it drives.)
#[test]
fn scene_session_mount_emits_a_finished_command_stream() {
    scheduler::install();

    let recorder = WireRecordingBackend::new();
    let fired = Rc::new(Cell::new(false));
    let session = SceneSession::mount(&recorder, |_r| {}, make_app(fired));

    assert_eq!(session.root_count(), 1, "single-root mount contract");

    let cmds = recorder.drain_commands();
    assert!(
        matches!(cmds.last(), Some(wire::Command::Finish { .. })),
        "the mount batch must end in Finish, got {:?}",
        cmds.last()
    );

    // Keep the RefCell import honest about the session's teardown order
    // (realized before world) by dropping explicitly.
    let boxed = RefCell::new(Some(session));
    drop(boxed.borrow_mut().take());
}
