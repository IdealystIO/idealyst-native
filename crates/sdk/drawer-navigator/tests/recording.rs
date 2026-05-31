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
