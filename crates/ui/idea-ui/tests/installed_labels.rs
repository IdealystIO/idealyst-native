//! The words idea-ui's components draw themselves come from the labels
//! the app installs, so an app in another language translates them once
//! at boot instead of finding each one baked in.

use std::collections::BTreeSet;
use std::rc::Rc;

use idea_ui::components::table::{pager_count_text, TableCards, TableCardsProps};
use idea_ui::date::{format_date, CivilDate};
use idea_ui::{
    install_idea_theme, install_ui_labels, light_theme, Autocomplete, Calendar, DateLabels, Pagination, PagerCount,
    RowId, SelectOption, Table, TableCell, TableCellProps, TableColumn, TableProps, TableRow, TableRowProps,
    UiLabels,
};
use runtime_core::{signal, ui, Element, Signal};
use runtime_vocabulary::robot::{ElementKind, Query, Robot};

fn mount(build: impl FnOnce() -> Element) -> (host_mock::Harness, runtime_scene::Realized<u32>) {
    Robot::new().reset();
    let h = host_mock::Harness::new();
    let tree = h.world.enter(|| {
        install_idea_theme(light_theme());
        build()
    });
    let realized = h.mount(tree);
    h.flush();
    (h, realized)
}

fn named(name: &str) {
    let all = Robot::new().find_all(Query::label(name).and(Query::kind(ElementKind::Pressable)));
    assert_eq!(all.len(), 1, "exactly one pressable is named {name:?}, got {all:?}");
}

fn french() -> UiLabels {
    let mut date = (*DateLabels::english()).clone();
    date.months[8] = "septembre".to_string();
    date.previous_month = "Mois précédent".to_string();
    date.next_month = "Mois suivant".to_string();
    let mut fr = UiLabels::english();
    fr.date = Rc::new(date);
    fr.show_options = "Afficher les options".to_string();
    fr.previous_page = "Page précédente".to_string();
    fr.next_page = "Page suivante".to_string();
    fr.select_all_rows = "Tout sélectionner".to_string();
    fr.select_row = "Sélectionner la ligne".to_string();
    fr.selection = Rc::new(|n| if n == 0 { "Tout sélectionner".to_string() } else { format!("{n} sélectionnés") });
    fr.pager_count = Rc::new(|count| match count {
        PagerCount::None { noun } => (format!("Aucun {noun}"), String::new()),
        PagerCount::All { total, noun } => (format!("{total} {noun}"), String::new()),
        PagerCount::Window { window, total, noun } => (window.to_string(), format!("sur {total} {noun}")),
    });
    fr
}

/// A user of the framework could not translate the words idea-ui draws
/// itself: the chevrons' and steppers' names, the selection boxes', the
/// pager's count. Each now reads the installed labels.
#[test]
fn regression_components_draw_the_installed_labels() {
    install_ui_labels(french());

    let (_h, _r) = mount(|| {
        let picked: Signal<Option<CivilDate>> = signal(None);
        let page = signal(1usize);
        let text: Signal<String> = signal(String::new());
        ui! {
            view {
                Calendar(value = picked)
                Pagination(page = page, total = 20usize, on_change = Rc::new(move |n| page.set(n)) as Rc<dyn Fn(usize)>)
                Autocomplete(value = text, options = vec![SelectOption::new("a", "Apple")])
            }
        }
    });
    named("Mois précédent");
    named("Mois suivant");
    named("Page précédente");
    named("Page suivante");
    named("Afficher les options");

    let (_h, _r) = mount(|| {
        let cell = TableCell(TableCellProps {
            text: runtime_core::Reactive::Static(Some("Ice on the ramp".into())),
            ..Default::default()
        });
        let head = TableCell(TableCellProps {
            header: true,
            text: runtime_core::Reactive::Static(Some("Tip".into())),
            ..Default::default()
        });
        Table(TableProps {
            columns: vec![TableColumn::new("tip")],
            selection: Some(signal(BTreeSet::new())),
            children: vec![
                TableRow(TableRowProps { selectable: true, children: vec![head], ..Default::default() }),
                TableRow(TableRowProps {
                    id: Some(RowId::from("t1")),
                    selectable: true,
                    children: vec![cell],
                    ..Default::default()
                }),
            ],
            ..Default::default()
        })
    });
    let robot = Robot::new();
    let name = |id: &str| robot.find(Query::test_id(id)).and_then(|e| e.label);
    assert_eq!(name(idea_ui::SELECT_ALL_TEST_ID).as_deref(), Some("Tout sélectionner"));
    assert_eq!(name(idea_ui::SELECT_ROW_TEST_ID).as_deref(), Some("Sélectionner la ligne"));

    // The card list's select-all line reads `selection`.
    let (_h, _r) = mount(|| {
        TableCards(&TableCardsProps {
            columns: vec![TableColumn::new("tip")],
            row_count: runtime_core::Reactive::Static(2),
            row_id: Rc::new(|i| RowId::from(format!("t{i}"))),
            cell: Rc::new(|i, _| ui! { text { format!("tip {i}") } }),
            selection: Some(signal(BTreeSet::new())),
            ..Default::default()
        })
    });
    assert!(Robot::new().find(Query::label("Tout sélectionner")).is_some(), "the select-all line is in the installed words");

    assert_eq!(pager_count_text(25, 25, 312, "projets"), ("26\u{2013}50".to_string(), "sur 312 projets".to_string()));
    assert_eq!(pager_count_text(0, 25, 0, "projets").0, "Aucun projets");

    // Plain `format_date` names the month in the installed words.
    let day = CivilDate::new(2026, 9, 30).unwrap();
    assert_eq!(format_date(day, "D MMMM YYYY"), "30 septembre 2026");
}

/// An app that installs nothing gets English, so the default is
/// unchanged.
#[test]
fn without_an_install_the_labels_are_english() {
    let (_h, _r) = mount(|| {
        let page = signal(1usize);
        ui! { Pagination(page = page, total = 20usize, on_change = Rc::new(move |n| page.set(n)) as Rc<dyn Fn(usize)>) }
    });
    named("Previous page");
    named("Next page");
    assert_eq!(pager_count_text(0, 25, 6, "projects").0, "6 projects");
}
