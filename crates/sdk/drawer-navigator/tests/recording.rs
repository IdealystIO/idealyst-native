//! Recording handler for the runtime-server recorder backend.
//!
//! Regression coverage for the NavigatorExt "Phase 2" gap: before this
//! handler existed, mounting a `DrawerNavigator` on
//! `dev_server::WireRecordingBackend` hit the `create_navigator` trait
//! default (`unimplemented!()`) and killed the sidecar session, so
//! navigator apps (the website, idea-ui-docs) couldn't run under
//! `idealyst dev` (runtime-server) or be headless-screenshotted.
//!
//! These tests pin that the recorder now emits the navigator wire
//! commands the protocol carries — `CreateDrawerNavigator`,
//! `NavigatorAttachInitial`, `DrawerAttachSidebar`, `NavigatorSelect`,
//! `OpenDrawer` — with the sidebar + screen content recorded as ordinary
//! primitive subtrees.

#![cfg(all(feature = "runtime-server", not(target_arch = "wasm32")))]

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use dev_server::WireRecordingBackend;
use drawer_navigator::{DrawerBuilder, DrawerHandle, DrawerNavigator, DrawerScreenExt};
use runtime_core::primitives::navigator::Screen;
use runtime_core::{render, text, view, Ref, Route};
use wire::Command;

const HOME: Route<()> = Route::<()>::new("home", "/");
const ABOUT: Route<()> = Route::<()>::new("about", "/about");

/// Build + mount a drawer navigator on a fresh recorder, returning the
/// recorder, the render owner (must be held — dropping it tears down the
/// reactive tree), and the bound handle.
fn mount_drawer() -> (WireRecordingBackend, runtime_core::Owner, Ref<DrawerHandle>) {
    // Synchronous-microtask scheduler so the framework's deferred work
    // runs; the sidebar's `after_ms(0)` queues to the deadline list,
    // drained by `tick_animations` below.
    dev_server::scheduler::install();

    let mut recorder = WireRecordingBackend::new();
    drawer_navigator::recording::register(&mut recorder);

    let nav_ref: Ref<DrawerHandle> = Ref::new();
    let tree = DrawerNavigator::new(&HOME)
        .sidebar(view(vec![text("SIDEBAR NAV").into()]).into())
        .screen(HOME, |_| {
            Screen::new(view(vec![text("HOME BODY").into()])).title("Home")
        })
        .screen(ABOUT, |_| {
            Screen::new(view(vec![text("ABOUT BODY").into()])).title("About")
        })
        .drawer_width(280.0)
        .bind(nav_ref.clone())
        .into();

    let backend_rc = Rc::new(RefCell::new(recorder.clone()));
    let owner = render(backend_rc, tree);
    // Fire the deferred sidebar build (after_ms(0) → deadline list).
    recorder.tick_animations(Duration::from_millis(16));
    (recorder, owner, nav_ref)
}

fn count(cmds: &[Command], pred: impl Fn(&Command) -> bool) -> usize {
    cmds.iter().filter(|c| pred(c)).count()
}

fn has_text(cmds: &[Command], needle: &str) -> bool {
    cmds.iter().any(|c| matches!(c, Command::CreateText { content, .. } if content == needle))
}

/// Mounting a drawer navigator emits the navigator wire commands (not
/// the Phase-1 fallback text placeholder), with the initial screen and
/// the sidebar both recorded as primitive subtrees.
#[test]
fn recording_drawer_emits_navigator_commands() {
    let (recorder, _owner, _nav) = mount_drawer();
    let cmds = recorder.drain_commands();

    // The navigator node + its structural metadata.
    let create = count(&cmds, |c| matches!(c, Command::CreateDrawerNavigator { .. }));
    assert_eq!(create, 1, "exactly one CreateDrawerNavigator, got: {cmds:?}");
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::CreateDrawerNavigator { initial_route, drawer_width, .. }
                if initial_route == "home" && (*drawer_width - 280.0).abs() < f32::EPSILON
        )),
        "CreateDrawerNavigator should carry initial_route + width, got: {cmds:?}"
    );

    // Initial screen attaches; the home body is recorded, the inactive
    // (lazy) about screen is not.
    assert_eq!(
        count(&cmds, |c| matches!(c, Command::NavigatorAttachInitial { .. })),
        1,
        "exactly one NavigatorAttachInitial, got: {cmds:?}"
    );
    assert!(has_text(&cmds, "HOME BODY"), "initial screen body recorded, got: {cmds:?}");
    assert!(
        !has_text(&cmds, "ABOUT BODY"),
        "inactive screen must not mount eagerly, got: {cmds:?}"
    );

    // Sidebar built to primitives + referenced by DrawerAttachSidebar.
    assert_eq!(
        count(&cmds, |c| matches!(c, Command::DrawerAttachSidebar { .. })),
        1,
        "exactly one DrawerAttachSidebar, got: {cmds:?}"
    );
    assert!(has_text(&cmds, "SIDEBAR NAV"), "sidebar content recorded, got: {cmds:?}");

    // And crucially NOT the Phase-1 graceful-fallback placeholder.
    assert!(
        !cmds.iter().any(|c| matches!(
            c,
            Command::CreateText { content, .. } if content.contains("not registered")
        )),
        "must dispatch to the recording handler, not the fallback text node"
    );
}

/// The installed control dispatcher turns handle calls into wire
/// commands: `open()` → `OpenDrawer`, `select(route)` → `NavigatorSelect`
/// plus the newly-mounted screen's primitives.
#[test]
fn recording_drawer_dispatcher_emits_open_and_select() {
    let (recorder, _owner, nav) = mount_drawer();
    let _ = recorder.drain_commands(); // discard the mount stream

    // Clone the handle out of the Ref before dispatching — calling a
    // dispatch method (which flips signals + re-enters the reactive
    // system) while still inside `Ref::with`'s borrow double-borrows.
    // Real call sites invoke the handle from an event handler, not from
    // inside `Ref::with`, so this mirrors production use.
    let handle = nav.get().expect("DrawerHandle filled after render");

    // open() dispatches Custom(DrawerCmd::Open) through the control.
    handle.open();
    let after_open = recorder.drain_commands();
    assert_eq!(
        count(&after_open, |c| matches!(c, Command::OpenDrawer { .. })),
        1,
        "open() should emit exactly one OpenDrawer, got: {after_open:?}"
    );

    // select(ABOUT) mounts the about screen (recording its primitives)
    // and emits NavigatorSelect referencing the new screen.
    handle.select(&ABOUT, ());
    let after_select = recorder.drain_commands();
    assert_eq!(
        count(&after_select, |c| matches!(c, Command::NavigatorSelect { .. })),
        1,
        "select() should emit exactly one NavigatorSelect, got: {after_select:?}"
    );
    assert!(
        has_text(&after_select, "ABOUT BODY"),
        "selected screen body recorded, got: {after_select:?}"
    );
}

/// A *second* select must not panic. The first `select` mounts the new
/// outlet and releases the previously-active screen's reactive scope; a
/// second select releases the first selection's scope in turn. This is
/// the path that aborts the runtime-server sidecar (its non-unwinding
/// drop turns the scope-release panic into a process abort), so navigating
/// twice in a row killed any drawer app under `idealyst dev`. Reproduces
/// against `WireRecordingBackend` — the exact backend the sidecar runs.
#[test]
fn recording_drawer_second_select_releases_previous_outlet_without_panic() {
    let (recorder, _owner, nav) = mount_drawer();
    let _ = recorder.drain_commands(); // discard mount

    let handle = nav.get().expect("DrawerHandle filled after render");

    // 1st nav: HOME → ABOUT.
    handle.select(&ABOUT, ());
    let after_first = recorder.drain_commands();
    assert!(
        has_text(&after_first, "ABOUT BODY"),
        "first select records the about body, got: {after_first:?}"
    );

    // 2nd nav: ABOUT → HOME. Releasing the ABOUT outlet's scope here is
    // what panicked. Must complete and re-record the home body.
    handle.select(&HOME, ());
    let after_second = recorder.drain_commands();
    assert_eq!(
        count(&after_second, |c| matches!(c, Command::NavigatorSelect { .. })),
        1,
        "second select emits one NavigatorSelect, got: {after_second:?}"
    );
    assert!(
        has_text(&after_second, "HOME BODY"),
        "second select re-records the home body, got: {after_second:?}"
    );
}

/// Selecting a route auto-closes the drawer on the dev side
/// (`is_open → false`) so server-built reactive sidebars stay coherent —
/// but it must NOT emit a wire `CloseDrawer`. The CLIENT closes its own
/// drawer when it replays the `NavigatorSelect` through its native
/// handler's dispatcher (whose `Select` arm auto-closes), exactly as
/// local non-wire mode does. Emitting a server-side `CloseDrawer` on top
/// would animate the drawer shut twice. See the matching client-side
/// regression in `dev-client` (`native_select_swaps_screen_and_closes_drawer`).
#[test]
fn recording_drawer_select_auto_closes_signal_only_no_wire_close() {
    let (recorder, _owner, nav) = mount_drawer();
    let _ = recorder.drain_commands();

    let handle = nav.get().expect("DrawerHandle filled after render");
    handle.open();
    assert!(handle.is_open(), "drawer open after open()");
    let _ = recorder.drain_commands();

    // Select while open: the dev-side signal flips so reactive sidebars
    // recompute, but no wire CloseDrawer is shipped (the client self-closes).
    handle.select(&ABOUT, ());
    assert!(
        !handle.is_open(),
        "selecting a route must flip the dev-side is_open signal to false"
    );
    let after_open_select = recorder.drain_commands();
    assert_eq!(
        count(&after_open_select, |c| matches!(c, Command::CloseDrawer { .. })),
        0,
        "select must NOT emit a wire CloseDrawer — the client closes itself \
         via the NavigatorSelect dispatch, got: {after_open_select:?}"
    );
}

/// Reverse channel: a client-side drawer gesture
/// (`DrawerStateChanged`) syncs the dev-side `is_open` signal directly,
/// WITHOUT echoing an `OpenDrawer`/`CloseDrawer` command back to the
/// client (which already moved).
#[test]
fn recording_drawer_reverse_state_change_syncs_signal_without_echo() {
    let (recorder, _owner, nav) = mount_drawer();
    let cmds = recorder.drain_commands();

    // Pull the navigator's NodeId out of the recorded stream.
    let nav_id = cmds
        .iter()
        .find_map(|c| match c {
            Command::CreateDrawerNavigator { id, .. } => Some(*id),
            _ => None,
        })
        .expect("CreateDrawerNavigator in stream");

    let handle = nav.get().expect("DrawerHandle filled after render");
    assert!(!handle.is_open(), "drawer starts closed");

    // Client opened the drawer via gesture.
    recorder.handle_drawer_state_changed(nav_id, true);
    assert!(handle.is_open(), "is_open signal synced from client gesture");

    // Crucially: no OpenDrawer echoed back — the client already moved.
    let echoed = recorder.drain_commands();
    assert!(
        !echoed.iter().any(|c| matches!(
            c,
            Command::OpenDrawer { .. } | Command::CloseDrawer { .. } | Command::ToggleDrawer { .. }
        )),
        "reverse sync must not echo a drawer command, got: {echoed:?}"
    );

    // Idempotent: re-reporting the same state is a no-op.
    recorder.handle_drawer_state_changed(nav_id, true);
    assert!(handle.is_open());
    // And it syncs back to closed.
    recorder.handle_drawer_state_changed(nav_id, false);
    assert!(!handle.is_open(), "is_open synced back to closed");
}
