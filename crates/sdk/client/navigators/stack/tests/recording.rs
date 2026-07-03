//! Recording handler for the runtime-server recorder backend.
//!
//! Pins that mounting a stack `Navigator` on `WireRecordingBackend`
//! emits the stack navigator wire commands (`CreateNavigator`,
//! `NavigatorAttachInitial`, `NavigatorPush`, `NavigatorPop`) — not the
//! Phase-1 fallback text node — with the screen subtrees recorded as
//! primitives.

#![cfg(all(feature = "runtime-server", not(target_arch = "wasm32")))]

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use dev_server::WireRecordingBackend;
use runtime_core::primitives::navigator::Screen;
use runtime_core::{render, text, view, Ref, Route};
use stack_navigator::{Navigator, StackBuilder, StackHandle, StackScreenExt};
use wire::Command;

const HOME: Route<()> = Route::<()>::new("home", "/");
const DETAIL: Route<()> = Route::<()>::new("detail", "/detail");

fn count(cmds: &[Command], pred: impl Fn(&Command) -> bool) -> usize {
    cmds.iter().filter(|c| pred(c)).count()
}
fn has_text(cmds: &[Command], needle: &str) -> bool {
    cmds.iter().any(|c| matches!(c, Command::CreateText { content, .. } if content == needle))
}

#[test]
fn recording_stack_emits_navigator_commands_for_push_pop() {
    dev_server::scheduler::install();
    let mut recorder = WireRecordingBackend::new();
    stack_navigator::recording::register(&mut recorder);

    let nav: Ref<StackHandle> = Ref::new();
    let nav_for_app = nav.clone();
    let tree = Navigator::new(&HOME)
        .screen(HOME, |_| {
            Screen::new(view(vec![text("HOME SCREEN").into()])).title("Home")
        })
        .screen(DETAIL, |_| {
            Screen::new(view(vec![text("DETAIL SCREEN").into()])).title("Detail")
        })
        .bind(nav_for_app)
        .into();

    let backend_rc = Rc::new(RefCell::new(recorder.clone()));
    let _owner = render(backend_rc, tree);
    recorder.tick_animations(Duration::from_millis(16));

    let mount = recorder.drain_commands();
    assert_eq!(
        count(&mount, |c| matches!(c, Command::CreateNavigator { .. })),
        1,
        "one CreateNavigator (stack), got: {mount:?}"
    );
    assert_eq!(
        count(&mount, |c| matches!(c, Command::NavigatorAttachInitial { .. })),
        1,
        "one NavigatorAttachInitial, got: {mount:?}"
    );
    assert!(has_text(&mount, "HOME SCREEN"), "initial screen recorded, got: {mount:?}");
    assert!(!has_text(&mount, "DETAIL SCREEN"), "detail not mounted yet, got: {mount:?}");
    assert!(
        !mount.iter().any(|c| matches!(c, Command::CreateText { content, .. } if content.contains("not registered"))),
        "must dispatch to the recording handler, not the fallback"
    );

    // Push detail → NavigatorPush + the detail subtree.
    let handle = nav.get().expect("StackHandle filled");
    handle.push(&DETAIL, ());
    let after_push = recorder.drain_commands();
    assert_eq!(
        count(&after_push, |c| matches!(c, Command::NavigatorPush { .. })),
        1,
        "one NavigatorPush, got: {after_push:?}"
    );
    assert!(has_text(&after_push, "DETAIL SCREEN"), "pushed screen recorded, got: {after_push:?}");

    // Pop → NavigatorPop { count: 1 }.
    handle.pop();
    let after_pop = recorder.drain_commands();
    assert_eq!(
        count(&after_pop, |c| matches!(c, Command::NavigatorPop { count: 1, .. })),
        1,
        "one NavigatorPop(1), got: {after_pop:?}"
    );
}

/// Regression: a cold-start deep link to a non-initial route must
/// reconstruct the back stack as [initial, detail] — NOT mount the
/// detail alone — so Back returns to the index. The walker resolves the
/// launch URL and hands the RESOLVED (detail) screen to `attach_initial`;
/// the stack handler then notices `active_route != initial_route`, seats
/// the configured `initial` as the stack base, and pushes the detail on
/// top. Before the deep-link fix the non-deferred path full-matched and
/// mounted `detail` as the sole screen, leaving Back nowhere to go.
#[test]
fn recording_stack_deep_link_reconstructs_back_stack() {
    dev_server::scheduler::install();
    let mut recorder = WireRecordingBackend::new();
    stack_navigator::recording::register(&mut recorder);

    // Cold-start launch URL points at the detail route (not the initial).
    runtime_core::set_initial_path(Some("/detail".to_string()));

    let tree = Navigator::new(&HOME)
        .screen(HOME, |_| {
            Screen::new(view(vec![text("HOME SCREEN").into()])).title("Home")
        })
        .screen(DETAIL, |_| {
            Screen::new(view(vec![text("DETAIL SCREEN").into()])).title("Detail")
        })
        .into();

    let backend_rc = Rc::new(RefCell::new(recorder.clone()));
    let _owner = render(backend_rc, tree);
    recorder.tick_animations(Duration::from_millis(16));

    let mount = recorder.drain_commands();

    // The configured initial (HOME) seats the bottom of the stack...
    assert_eq!(
        count(&mount, |c| matches!(c, Command::NavigatorAttachInitial { .. })),
        1,
        "initial route attached as stack base, got: {mount:?}"
    );
    assert!(has_text(&mount, "HOME SCREEN"), "initial HOME mounted, got: {mount:?}");
    // ...and the deep-linked DETAIL is pushed on top.
    assert_eq!(
        count(&mount, |c| matches!(c, Command::NavigatorPush { .. })),
        1,
        "deep-link route pushed for stack reconstruction, got: {mount:?}"
    );
    assert!(has_text(&mount, "DETAIL SCREEN"), "deep-linked DETAIL pushed, got: {mount:?}");

    // The walker (root navigator) cleared the initial-path slot after its
    // subtree mounted, so a second navigator mount wouldn't re-deep-link.
    assert!(
        runtime_core::take_initial_path().is_none(),
        "root navigator must clear the launch-path slot after mounting"
    );
}
