//! The words this crate's components draw themselves come from the
//! labels the app installs, so an app in another language translates
//! them once at boot instead of finding each one baked in.

use std::rc::Rc;

use host_mock::Harness;
use idea_ui::{install_idea_theme, light_theme};
use idea_ui_layout::{
    install_layout_labels, ActionMenuProps, Blank, Dialog, FlowLabels, LayoutLabels, ListPhase, ListStates, ListStatesProps,
    SidebarToggleProps, ToolbarSearch, ToolbarSearchProps,
};
use runtime_core::{signal, ui, viewport_size, Element, ViewportSize};
use runtime_vocabulary::robot::{ElementKind, Query, Robot};

fn mount(build: impl FnOnce() -> Element) -> (Harness, runtime_scene::Realized<host_mock::Node>) {
    host_mock::pump::install_scheduler();
    Robot::new().reset();
    let h = Harness::new();
    let tree = h.world.enter(|| {
        install_idea_theme(light_theme());
        viewport_size().set(ViewportSize::new(1440.0, 800.0));
        build()
    });
    h.flush();
    let realized = h.mount(tree);
    h.flush();
    (h, realized)
}

/// `f` inside a world: some props' defaults create a signal.
fn in_world<R>(f: impl FnOnce() -> R) -> R {
    Harness::new().world.enter(f)
}

fn named(name: &str) {
    let all = Robot::new().find_all(Query::label(name).and(Query::kind(ElementKind::Pressable)));
    assert_eq!(all.len(), 1, "exactly one pressable is named {name:?}, got {all:?}");
}

fn french() -> LayoutLabels {
    let mut fr = LayoutLabels::english();
    fr.close = "Fermer".to_string();
    fr.back = "Retour".to_string();
    fr.search = "Rechercher".to_string();
    fr.menu = "Menu principal".to_string();
    fr.more_actions = "Plus d'actions".to_string();
    fr.nothing_here_yet = "Rien pour l'instant".to_string();
    fr.no_rows_match = "Aucune ligne ne correspond.".to_string();
    fr.flow.steps = "Étapes".to_string();
    fr
}

/// A user of the framework reported that a `Dialog`'s ✕ was always
/// named "Close" and a `ToolbarSearch` always said "Search", with no way
/// to translate either from the app. Both, and the crate's other words,
/// now read the installed labels.
#[test]
fn regression_dialog_close_and_toolbar_search_draw_the_installed_labels() {
    install_layout_labels(french());

    let (_h, _r) = mount(|| {
        let open = signal(true);
        ui! {
            Dialog(
                open = open,
                title = "Modifier".to_string(),
                on_back = Some(Rc::new(|| {}) as Rc<dyn Fn()>),
                show_back = true,
                on_close = Rc::new(move || open.set(false)) as Rc<dyn Fn()>,
                body = || ui! { view {} },
            )
        }
    });
    named("Fermer");
    named("Retour");

    let (_h, _r) = mount(|| ui! { ToolbarSearch() });
    named("Rechercher");
    assert_eq!(in_world(|| ToolbarSearchProps::default().placeholder), "Rechercher");

    assert_eq!(in_world(|| SidebarToggleProps::default().label), "Menu principal");
    assert_eq!(in_world(|| ActionMenuProps::default().label), "Plus d'actions");
    let (_h, _r) = mount(|| {
        ui! { ListStates(phase = ListPhase::Ready, is_empty = true, blank = Blank::default(), rows = || ui! { view {} }) }
    });
    assert!(Robot::new().find(Query::label("Rien pour l'instant")).is_some(), "the blank state's default title");
    assert_eq!(in_world(|| ListStatesProps::default().no_match), "Aucune ligne ne correspond.");
    assert_eq!(FlowLabels::default().steps, "Étapes");
}

/// An app that installs nothing gets English.
#[test]
fn without_an_install_the_labels_are_english() {
    let (_h, _r) = mount(|| {
        let open = signal(true);
        ui! { Dialog(open = open, title = "Edit".to_string(), body = || ui! { view {} }) }
    });
    named("Close");
    assert_eq!(in_world(|| ToolbarSearchProps::default().placeholder), "Search");
    assert_eq!((FlowLabels::default().step_of)(2, 5), "Step 2 of 5");
}
