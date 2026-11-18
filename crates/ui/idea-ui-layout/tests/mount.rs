//! Mount coverage on `host_mock::Harness`: each layout draws the form
//! its width calls for.

use std::cell::Cell;
use std::rc::Rc;

use host_mock::Harness;
use idea_ui::{install_idea_theme, light_theme, Typography};
use idea_ui_layout::{
    Action, ActionMenu, Blank, ListPhase, ListStates, Page, PageHeader, PageVariant, SplitMode, SplitView,
};
use runtime_core::{signal, ui, viewport_size, Element, IconData, Signal, ViewportSize};

const PHONE: f32 = 390.0;
const TABLET: f32 = 820.0;
const DESKTOP: f32 = 1440.0;

struct Mounted {
    h: Harness,
    _realized: runtime_scene::Realized<host_mock::Node>,
}

impl Mounted {
    fn log(&self) -> String {
        self.h.take_log().join("\n")
    }
}

/// Mount `build`'s tree in a window `width` wide.
fn mount_at(width: f32, build: impl FnOnce() -> Element) -> Mounted {
    host_mock::pump::install_scheduler();
    let h = Harness::new();
    let tree = h.world.enter(|| {
        install_idea_theme(light_theme());
        viewport_size().set(ViewportSize::new(width, 800.0));
        build()
    });
    h.flush();
    let realized = h.mount(tree);
    h.flush();
    Mounted { h, _realized: realized }
}

fn noop() -> Rc<dyn Fn()> {
    Rc::new(|| {})
}

const GLYPH: IconData = icons_lucide::PLUS;

fn header() -> Element {
    ui! {
        PageHeader(
            title = "Ramp 4".to_string(),
            actions = vec![Action::new("Add heading", noop()).icon(GLYPH).primary(), Action::new("Export", noop())],
            menu = vec![Action::new("Archive", noop())],
        )
    }
}

/// What the sheet form mounts and the anchored form does not: the
/// sheet is a `Modal`, which stays mounted while closed.
const SHEET_MARK: &str = "presence_placeholder";

#[test]
fn a_phone_header_collapses_its_primary_and_folds_the_rest_into_one_sheet() {
    let m = mount_at(PHONE, header);
    let closed = m.log();
    assert!(closed.contains("\"Ramp 4\""), "{closed}");
    assert!(!closed.contains("\"Add heading\""), "the primary is its glyph alone on a phone:\n{closed}");
    assert!(!closed.contains("\"Export\""), "a second verb has no button on a phone:\n{closed}");
    assert!(closed.contains(SHEET_MARK), "the phone's menu is a sheet:\n{closed}");

    // Pressable 0 is the primary's glyph, 1 the one ⋮.
    (m.h.press_handler(1))();
    m.h.flush();
    let open = m.log();
    assert!(open.contains("\"Export\""), "the folded verb is in the menu:\n{open}");
    assert!(open.contains("\"Archive\""), "so is the record's menu:\n{open}");
    assert!(!open.contains("\"Add heading\""), "the primary never folds into the menu:\n{open}");
}

#[test]
fn a_wide_header_labels_its_verbs_and_keeps_the_record_menu_anchored() {
    for width in [TABLET, DESKTOP] {
        let m = mount_at(width, header);
        let closed = m.log();
        assert!(closed.contains("\"Add heading\""), "{width}:\n{closed}");
        assert!(closed.contains("\"Export\""), "{width}:\n{closed}");
        assert!(!closed.contains(SHEET_MARK), "{width}: an anchored menu, not a sheet:\n{closed}");

        // Pressable 0 is the record's ⋮, right after the title.
        (m.h.press_handler(0))();
        m.h.flush();
        let open = m.log();
        assert!(open.contains("\"Archive\""), "{width}:\n{open}");
        assert!(!open.contains("\"Export\""), "{width}: a verb with a button is not also in the menu:\n{open}");
    }
}

#[test]
fn an_action_menu_entry_closes_the_surface_and_runs_once_at_either_form() {
    for width in [PHONE, DESKTOP] {
        let ran = Rc::new(Cell::new(0));
        let count = ran.clone();
        let m = mount_at(width, move || {
            let run: Rc<dyn Fn()> = Rc::new(move || count.set(count.get() + 1));
            ui! { ActionMenu(actions = vec![Action::new("Rename", run)]) }
        });
        m.log();
        (m.h.press_handler(0))();
        m.h.flush();
        let open = m.log();
        assert!(open.contains("\"Rename\""), "{width}:\n{open}");
        m.h.press_labelled("Rename");
        m.h.flush();
        assert_eq!(ran.get(), 1, "{width}");
    }
}

#[test]
fn a_menu_entry_does_nothing_while_its_verb_is_disabled() {
    let ran = Rc::new(Cell::new(0));
    let count = ran.clone();
    let m = mount_at(PHONE, move || {
        let run: Rc<dyn Fn()> = Rc::new(move || count.set(count.get() + 1));
        ui! { ActionMenu(actions = vec![Action::new("Delete", run).disabled(true)]) }
    });
    (m.h.press_handler(0))();
    m.h.flush();
    m.h.press_labelled("Delete");
    m.h.flush();
    assert_eq!(ran.get(), 0);
}

#[test]
fn an_action_menu_with_no_entries_draws_no_trigger() {
    let m = mount_at(DESKTOP, || ui! { ActionMenu(actions = Vec::<Action>::new()) });
    let log = m.log();
    assert!(!log.contains("pressable"), "{log}");
}

fn page(variant: PageVariant) -> Element {
    ui! {
        Page(
            variant = variant,
            header = ui! { PageHeader(title = "Materials".to_string()) },
            body = ui! { Typography(content = "BODY".to_string()) },
        )
    }
}

/// Only a `Content` page has a page scroller. A scroller round a
/// bounded body rubber-bands the header and competes with the inner
/// scroller's pan, so `Table` and `Blank` mount none.
#[test]
fn only_a_content_page_mounts_a_page_scroller() {
    for width in [PHONE, DESKTOP] {
        assert_eq!(mount_at(width, || page(PageVariant::Content)).h.scroll_view_count(), 1, "{width}");
        assert_eq!(mount_at(width, || page(PageVariant::Table)).h.scroll_view_count(), 0, "{width}");
        assert_eq!(mount_at(width, || page(PageVariant::Blank)).h.scroll_view_count(), 0, "{width}");
    }
}

#[test]
fn a_page_draws_its_header_toolbar_body_and_overlays() {
    let m = mount_at(DESKTOP, || {
        ui! {
            Page(
                header = ui! { PageHeader(title = "Materials".to_string()) },
                toolbar = Some(ui! { Typography(content = "TOOLBAR".to_string()) }),
                body = ui! { Typography(content = "BODY".to_string()) },
            ) {
                Typography(content = "OVERLAY".to_string())
            }
        }
    });
    let log = m.log();
    for part in ["\"Materials\"", "\"TOOLBAR\"", "\"BODY\"", "\"OVERLAY\""] {
        assert!(log.contains(part), "{part}:\n{log}");
    }
}

/// A shell's variant follows its tab without rebuilding the outlet: a
/// change of variant restyles the frame and creates nothing.
#[test]
fn a_live_variant_restyles_the_frame_and_keeps_the_body_mounted() {
    host_mock::pump::install_scheduler();
    let h = Harness::new();
    let (tree, variant) = h.world.enter(|| {
        install_idea_theme(light_theme());
        viewport_size().set(ViewportSize::new(DESKTOP, 800.0));
        let variant: Signal<PageVariant> = signal(PageVariant::Content);
        let tree = ui! {
            Page(
                live_variant = Some(runtime_core::rx!(variant.get())),
                header = ui! { PageHeader(title = "Project".to_string()) },
                body = ui! { Typography(content = "TAB".to_string()) },
            )
        };
        (tree, variant)
    });
    let _realized = h.mount(tree);
    h.flush();
    h.take_log();
    h.world.enter(|| variant.set(PageVariant::Table));
    h.flush();
    let log = h.take_log().join("\n");
    assert!(!log.contains("create "), "nothing is rebuilt:\n{log}");
    assert!(log.contains("apply_style"), "the frame restyles:\n{log}");
}

fn states(phase: ListPhase, is_empty: bool, narrowed: bool) -> Element {
    ui! {
        ListStates(
            phase = phase,
            is_empty = is_empty,
            narrowed = narrowed,
            blank = Blank::new("No materials yet").action(Action::new("Add a material", noop())),
            no_match = "No materials match.".to_string(),
            failed_title = "Couldn't load the materials".to_string(),
            on_retry = Some(noop()),
            rows = || ui! { Typography(content = "ROWS".to_string()) },
        )
    }
}

#[test]
fn a_list_shows_its_rows_or_the_one_state_that_explains_their_absence() {
    let blank = mount_at(PHONE, || states(ListPhase::Ready, true, false)).log();
    assert!(blank.contains("\"No materials yet\"") && blank.contains("\"Add a material\""), "{blank}");

    // The filters removed every record: the line, and no action. The
    // toolbar above is the way back.
    let empty = mount_at(PHONE, || states(ListPhase::Ready, true, true)).log();
    assert!(empty.contains("\"No materials match.\""), "{empty}");
    assert!(!empty.contains("\"Add a material\""), "{empty}");

    let failed = mount_at(PHONE, || states(ListPhase::Failed("You are offline.".into()), true, false)).log();
    for part in ["\"Couldn't load the materials\"", "\"You are offline.\"", "\"Retry\""] {
        assert!(failed.contains(part), "{part}:\n{failed}");
    }

    let rows = mount_at(PHONE, || states(ListPhase::Ready, false, false)).log();
    assert!(rows.contains("\"ROWS\"") && !rows.contains("\"No materials yet\""), "{rows}");

    let loading = mount_at(PHONE, || states(ListPhase::Loading, true, false)).log();
    assert!(!loading.contains("\"ROWS\"") && !loading.contains("\"No materials yet\""), "{loading}");
}

/// One row changing must not rebuild the list: the rows are built when
/// the list goes from having none to having some, not on each write to
/// what `is_empty` reads.
#[test]
fn a_write_that_leaves_the_state_alone_does_not_rebuild_the_rows() {
    host_mock::pump::install_scheduler();
    let h = Harness::new();
    let built = Rc::new(Cell::new(0));
    let count = built.clone();
    let (tree, rows) = h.world.enter(|| {
        install_idea_theme(light_theme());
        let rows: Signal<Vec<u32>> = signal(vec![1]);
        let tree = ui! {
            ListStates(
                phase = ListPhase::Ready,
                is_empty = runtime_core::rx!(rows.get().is_empty()),
                rows = move || {
                    count.set(count.get() + 1);
                    ui! { Typography(content = "ROWS".to_string()) }
                },
            )
        };
        (tree, rows)
    });
    let _realized = h.mount(tree);
    h.flush();
    assert_eq!(built.get(), 1);
    h.world.enter(|| rows.set(vec![1, 2, 3]));
    h.flush();
    assert_eq!(built.get(), 1, "a longer list is still the rows state");
    h.world.enter(|| rows.set(Vec::new()));
    h.flush();
    let log = h.take_log().join("\n");
    assert!(log.contains("\"Nothing here yet\""), "an emptied list is blank:\n{log}");
}

/// Crossing a split's width restyles its regions and remounts neither,
/// so a half-typed field survives a rotation.
#[test]
fn a_split_view_restyles_across_its_width_and_remounts_nothing() {
    host_mock::pump::install_scheduler();
    let h = Harness::new();
    let tree = h.world.enter(|| {
        install_idea_theme(light_theme());
        viewport_size().set(ViewportSize::new(DESKTOP, 800.0));
        ui! {
            SplitView(mode = SplitMode::Nav, first = ui! { Typography(content = "LIST".to_string()) }, second = ui! { Typography(content = "DESTINATION".to_string()) })
        }
    });
    h.flush();
    let _realized = h.mount(tree);
    h.flush();
    let mounted = h.take_log().join("\n");
    assert!(mounted.contains("\"LIST\"") && mounted.contains("\"DESTINATION\""), "{mounted}");
    h.world.enter(|| viewport_size().set(ViewportSize::new(PHONE, 800.0)));
    h.flush();
    let crossed = h.take_log().join("\n");
    assert!(!crossed.contains("create "), "nothing is remounted:\n{crossed}");
    assert!(crossed.contains("apply_style"), "the regions restyle:\n{crossed}");
}

mod flow {
    use super::*;
    use idea_ui_layout::{ActionBar, Flow, FlowStep};

    fn wizard() -> Element {
        ui! {
            Flow(
                header = ui! { PageHeader(title = "New project".to_string()) },
                steps = vec![FlowStep::new("Basics"), FlowStep::new("Shifts").subtitle("When the crews work")],
                current = 1_usize,
                body = ui! { Typography(content = "STEP BODY".to_string()) },
                actions = ui! {
                    ActionBar(
                        primary = Some(Action::new("Create project", noop())),
                        secondary = vec![Action::new("Back", noop())],
                        pinned = true,
                    )
                },
            )
        }
    }

    /// The step, its heading and its actions are the same elements at
    /// every width; only the step list's form differs, by stylesheet.
    #[test]
    fn a_flow_draws_its_step_heading_and_pinned_actions_at_every_width() {
        for width in [PHONE, TABLET, DESKTOP] {
            let log = mount_at(width, wizard).log();
            for part in [
                "\"New project\"",
                "\"Shifts\"",
                "\"When the crews work\"",
                "\"Step 2 of 2\"",
                "\"Step 2 of 2 · Shifts\"",
                "\"STEP BODY\"",
                "\"Create project\"",
                "\"Back\"",
            ] {
                assert!(log.contains(part), "{width}: {part}\n{log}");
            }
        }
    }
}
