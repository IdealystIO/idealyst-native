//! Recording handler for the runtime-server recorder backend.
//!
//! Pins that mounting a `TabNavigator` on `WireRecordingBackend` emits
//! `CreateTabNavigator` (carrying the tab registrations) +
//! `NavigatorAttachInitial`, and that selecting a tab emits
//! `NavigatorSelect` with the tab's screen recorded as primitives — not
//! the Phase-1 fallback text node.

#![cfg(all(feature = "runtime-server", not(target_arch = "wasm32")))]

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use dev_server::WireRecordingBackend;
use runtime_core::primitives::navigator::Screen;
use runtime_core::{render, text, view, Ref, Route};
use tab_navigator::{TabNavigator, TabSpec, TabsBuilder, TabsHandle};
use wire::Command;

const HOME: Route<()> = Route::<()>::new("home", "/");
const SETTINGS: Route<()> = Route::<()>::new("settings", "/settings");

fn count(cmds: &[Command], pred: impl Fn(&Command) -> bool) -> usize {
    cmds.iter().filter(|c| pred(c)).count()
}
fn has_text(cmds: &[Command], needle: &str) -> bool {
    cmds.iter().any(|c| matches!(c, Command::CreateText { content, .. } if content == needle))
}

#[test]
fn recording_tab_emits_create_and_select() {
    dev_server::scheduler::install();
    let mut recorder = WireRecordingBackend::new();
    tab_navigator::recording::register(&mut recorder);

    let nav: Ref<TabsHandle> = Ref::new();
    let nav_for_app = nav.clone();
    let tree = TabNavigator::new(&HOME)
        .tab(HOME, TabSpec::new("HomeTab"), |_| {
            Screen::new(view(vec![text("HOME CONTENT").into()]))
        })
        .tab(SETTINGS, TabSpec::new("SettingsTab"), |_| {
            Screen::new(view(vec![text("SETTINGS CONTENT").into()]))
        })
        .bind(nav_for_app)
        .into();

    let backend_rc = Rc::new(RefCell::new(recorder.clone()));
    let _owner = render(backend_rc, tree);
    recorder.tick_animations(Duration::from_millis(16));

    let mount = recorder.drain_commands();
    assert_eq!(
        count(&mount, |c| matches!(c, Command::CreateTabNavigator { .. })),
        1,
        "one CreateTabNavigator, got: {mount:?}"
    );
    // The registrations carry both tab labels.
    assert!(
        mount.iter().any(|c| matches!(
            c,
            Command::CreateTabNavigator { tabs, .. }
                if tabs.iter().any(|t| t.label == "HomeTab")
                    && tabs.iter().any(|t| t.label == "SettingsTab")
        )),
        "CreateTabNavigator carries both tab registrations, got: {mount:?}"
    );
    assert_eq!(
        count(&mount, |c| matches!(c, Command::NavigatorAttachInitial { .. })),
        1,
        "one NavigatorAttachInitial, got: {mount:?}"
    );
    assert!(has_text(&mount, "HOME CONTENT"), "initial tab screen recorded, got: {mount:?}");
    assert!(!has_text(&mount, "SETTINGS CONTENT"), "inactive tab not mounted, got: {mount:?}");

    // Select settings → NavigatorSelect + the settings subtree.
    let handle = nav.get().expect("TabsHandle filled");
    handle.select(&SETTINGS, ());
    let after = recorder.drain_commands();
    assert_eq!(
        count(&after, |c| matches!(c, Command::NavigatorSelect { .. })),
        1,
        "one NavigatorSelect, got: {after:?}"
    );
    assert!(has_text(&after, "SETTINGS CONTENT"), "selected tab screen recorded, got: {after:?}");
}
