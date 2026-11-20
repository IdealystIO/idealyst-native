//! Mount coverage on `host_mock::Harness`: each layout draws the form
//! its width calls for.

use std::cell::Cell;
use std::rc::Rc;

use host_mock::Harness;
use idea_ui::{install_idea_theme, light_theme, Typography};
use idea_ui_layout::{
    Action, ActionMenu, Blank, ListPhase, ListStates, Page, PageFrame, PageHeader, PageVariant, PinnedPane, SplitMode,
    SplitView,
};
use runtime_core::{signal, ui, viewport_size, Element, IconData, Ref, ScrollViewHandle, Signal, ViewportSize};

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

mod app_frame {
    use super::*;
    use idea_ui_layout::{AppFrame, FrameSurface, SidebarCollapse, SidebarItem, SidebarState, SidebarToggle};

    /// A frame round one page, its sidebar holding one destination.
    fn framed(surface: FrameSurface, sidebar: SidebarState) -> Element {
        ui! {
            AppFrame(
                sidebar = vec![ui! {
                    SidebarItem(sidebar = sidebar, icon = Some(GLYPH), label = "Projects".to_string(), on_press = noop())
                }],
                state = sidebar,
                surface = surface,
            ) {
                Page(
                    header = ui! {
                        PageHeader(title = "Workers".to_string(), leading = Some(ui! { SidebarToggle(sidebar = sidebar) }))
                    },
                    body = ui! { Typography(content = "BODY".to_string()) },
                )
            }
        }
    }

    /// Build inside the world, where a `SidebarState`'s signals live.
    fn mount_framed(width: f32, surface: FrameSurface, collapse: SidebarCollapse) -> (Mounted, SidebarState) {
        let state = Rc::new(Cell::new(None));
        let out = state.clone();
        let m = mount_at(width, move || {
            let sidebar = SidebarState::new(collapse);
            out.set(Some(sidebar));
            framed(surface, sidebar)
        });
        (m, state.get().expect("built"))
    }

    /// The look is a restyle of one tree: both draw the sidebar once and
    /// the screen once, at every width.
    #[test]
    fn either_surface_draws_the_sidebar_and_the_screen_once() {
        for surface in [FrameSurface::Flat, FrameSurface::Raised] {
            for width in [PHONE, TABLET, DESKTOP] {
                let log = mount_framed(width, surface, SidebarCollapse::Off).0.log();
                for part in ["\"Projects\"", "\"Workers\"", "\"BODY\""] {
                    assert_eq!(log.matches(part).count(), 1, "{surface:?} at {width}: {part}\n{log}");
                }
            }
        }
    }

    /// Switching the look restyles the frame in place: the screen
    /// mounted in it is not rebuilt, so nothing typed in it is lost.
    #[test]
    fn a_live_surface_restyles_the_frame_and_remounts_nothing() {
        host_mock::pump::install_scheduler();
        let h = Harness::new();
        let (tree, surface) = h.world.enter(|| {
            install_idea_theme(light_theme());
            viewport_size().set(ViewportSize::new(DESKTOP, 800.0));
            let surface = signal(FrameSurface::Flat);
            let tree = ui! {
                AppFrame(
                    sidebar = vec![ui! { Typography(content = "DESTINATIONS".to_string()) }],
                    surface = runtime_core::rx!(surface.get()),
                ) {
                    Typography(content = "SCREEN".to_string())
                }
            };
            (tree, surface)
        });
        h.flush();
        let _realized = h.mount(tree);
        h.flush();
        h.take_log();
        h.world.enter(|| surface.set(FrameSurface::Raised));
        h.flush();
        let log = h.take_log().join("\n");
        assert!(!log.contains("create "), "nothing is remounted:\n{log}");
        assert!(log.contains("apply_style"), "the frame restyles:\n{log}");
    }

    /// A sidebar that does not collapse is always there on a desktop,
    /// so its toggle would do nothing and is not drawn.
    #[test]
    fn a_desktop_sidebar_that_does_not_collapse_draws_no_toggle() {
        let pressables = |log: &str| log.lines().filter(|l| l.starts_with("create ") && l.ends_with(" pressable")).count();
        let fixed = pressables(&mount_framed(DESKTOP, FrameSurface::Flat, SidebarCollapse::Off).0.log());
        let collapsing = pressables(&mount_framed(DESKTOP, FrameSurface::Flat, SidebarCollapse::Icons).0.log());
        assert_eq!(collapsing, fixed + 1, "the toggle is the one pressable collapsing adds");
        for width in [PHONE, TABLET] {
            let drawer = pressables(&mount_framed(width, FrameSurface::Flat, SidebarCollapse::Off).0.log());
            assert_eq!(drawer, fixed + 1, "{width}: a drawer always has its ☰");
        }
    }

    /// One press means two things: open the drawer where the sidebar is
    /// one, collapse it where it is pinned.
    #[test]
    fn the_toggle_opens_the_drawer_below_desktop_and_collapses_the_sidebar_on_it() {
        for (width, opens_drawer) in [(PHONE, true), (TABLET, true), (DESKTOP, false)] {
            let (m, sidebar) = mount_framed(width, FrameSurface::Flat, SidebarCollapse::Icons);
            m.h.world.enter(|| sidebar.toggle());
            m.h.flush();
            let (drawer, collapsed) = m.h.world.enter(|| (sidebar.drawer.get(), sidebar.collapsed.get()));
            assert_eq!(drawer, opens_drawer, "{width}");
            assert_eq!(collapsed, !opens_drawer, "{width}");
        }
    }

    /// The desktop control says which of the two a press does; the ☰ is
    /// "Menu". The CrewForge report (11-19): all three answered to "Menu".
    #[test]
    fn the_toggle_is_named_for_what_a_press_does() {
        let (m, sidebar) = mount_framed(DESKTOP, FrameSurface::Flat, SidebarCollapse::Icons);
        assert!(m.h.a11y_by_label("Collapse sidebar").is_some());
        assert!(m.h.a11y_by_label("Expand sidebar").is_none() && m.h.a11y_by_label("Menu").is_none());
        m.h.world.enter(|| sidebar.collapsed.set(true));
        m.h.flush();
        assert!(m.h.a11y_by_label("Expand sidebar").is_some());

        let (phone, _) = mount_framed(PHONE, FrameSurface::Flat, SidebarCollapse::Icons);
        assert!(phone.h.a11y_by_label("Menu").is_some());
        assert!(phone.h.a11y_by_label("Collapse sidebar").is_none());
    }

    /// The icon rail is the same rows, narrowed: collapsing restyles
    /// them and builds nothing, so the glyph cannot move or change size
    /// and the label has something to fade from. The CrewForge report
    /// (11-19): the rail swapped each row for an icon button, so the
    /// label vanished and the glyph jumped on the first frame.
    #[test]
    fn regression_collapsing_to_the_rail_restyles_the_rows_and_builds_nothing() {
        let state = Rc::new(Cell::new(None));
        let out = state.clone();
        let m = mount_at(DESKTOP, move || {
            let sidebar = SidebarState::new(SidebarCollapse::Icons);
            out.set(Some(sidebar));
            ui! { SidebarItem(sidebar = sidebar, icon = Some(GLYPH), label = "Projects".to_string(), on_press = noop()) }
        });
        let sidebar: SidebarState = state.get().expect("built");
        let open = m.log();
        assert!(open.contains("\"Projects\"") && open.contains(" icon"), "open: glyph and label:\n{open}");
        m.h.world.enter(|| sidebar.collapsed.set(true));
        m.h.flush();
        let rail = m.log();
        assert!(m.h.world.enter(|| sidebar.is_rail()));
        assert!(!rail.contains("create "), "nothing is rebuilt for the rail:\n{rail}");
        assert!(rail.contains("apply_style"), "the row is restyled:\n{rail}");
        m.h.world.enter(|| sidebar.collapsed.set(false));
        m.h.flush();
        let back = m.log();
        assert!(!back.contains("create "), "nor on the way back:\n{back}");
    }

    /// The row's tooltip is its label, and only the rail needs it: with
    /// the label on show a hover draws no bubble. Timers are pumped, so a
    /// hover rests past the tooltip's show delay.
    #[test]
    fn a_sidebar_item_says_its_name_on_hover_only_in_the_rail() {
        host_mock::pump::install_scheduler();
        let state = Rc::new(Cell::new(None));
        let out = state.clone();
        let m = mount_at(DESKTOP, move || {
            let sidebar = SidebarState::new(SidebarCollapse::Icons);
            out.set(Some(sidebar));
            ui! { SidebarItem(sidebar = sidebar, icon = Some(GLYPH), label = "Projects".to_string(), on_press = noop()) }
        });
        let sidebar: SidebarState = state.get().expect("built");
        let hover = |entering: bool| {
            let handlers: Vec<_> = m.h.shared.hover_handlers.borrow().iter().map(|(_, h)| h.clone()).collect();
            for handler in handlers {
                handler(entering);
            }
            m.h.flush();
            host_mock::pump::pump_timers();
            m.h.flush();
        };
        m.log();
        hover(true);
        let open = m.log();
        assert_eq!(open.matches("\"Projects\"").count(), 0, "no bubble beside a label that is on show:\n{open}");
        hover(false);
        m.h.world.enter(|| sidebar.collapsed.set(true));
        m.h.flush();
        m.log();
        hover(true);
        let rail = m.log();
        assert_eq!(rail.matches("\"Projects\"").count(), 1, "the rail's row names itself:\n{rail}");
    }

    /// The rail is a desktop form only: below it the sidebar is a
    /// drawer, and an open drawer is the full sidebar with its labels,
    /// whatever `collapsed` was left at.
    #[test]
    fn the_icon_rail_does_not_apply_below_desktop() {
        for width in [PHONE, TABLET] {
            let (m, sidebar) = mount_framed(width, FrameSurface::Flat, SidebarCollapse::Icons);
            m.log();
            m.h.world.enter(|| sidebar.collapsed.set(true));
            m.h.flush();
            assert!(!m.h.world.enter(|| sidebar.is_rail()), "{width}");
            let after = m.log();
            assert!(!after.contains("create "), "{width}: the items are not rebuilt as glyphs:\n{after}");
        }
    }

    /// A counted destination: the number at the open row's end and a
    /// dot on the glyph for the rail, nothing at zero.
    #[test]
    fn a_sidebar_item_shows_its_count_as_a_number_or_a_rail_dot() {
        use idea_ui::tone;
        use idea_ui_layout::SidebarCount;
        let handles = Rc::new(Cell::new(None));
        let out = handles.clone();
        let m = mount_at(DESKTOP, move || {
            let sidebar = SidebarState::new(SidebarCollapse::Icons);
            let open: Signal<i64> = signal(3);
            out.set(Some((sidebar, open)));
            ui! {
                SidebarItem(
                    sidebar = sidebar,
                    icon = Some(GLYPH),
                    label = "Tasks".to_string(),
                    count = Some(SidebarCount::new(open, tone::Danger)),
                    on_press = noop(),
                )
            }
        });
        let (sidebar, open): (SidebarState, Signal<i64>) = handles.get().expect("built");
        let row = m.log();
        assert!(row.contains("\"Tasks\"") && row.contains("\"3\""), "the open row carries the number:\n{row}");

        m.h.world.enter(|| open.set(0));
        m.h.flush();
        m.h.world.enter(|| open.set(12));
        m.h.flush();
        let back = m.log();
        assert!(back.contains("\"12\""), "hidden at zero, back with the next count:\n{back}");

        // The dot is mounted with the number and shown by the rail's
        // restyle: collapsing builds nothing.
        m.h.world.enter(|| sidebar.collapsed.set(true));
        m.h.flush();
        let rail = m.log();
        assert!(!rail.contains("create "), "the rail restyles the count, it does not rebuild it:\n{rail}");
    }

    #[test]
    fn a_sidebar_item_closes_the_drawer_and_goes() {
        let went = Rc::new(Cell::new(false));
        let flag = went.clone();
        let state = Rc::new(Cell::new(None));
        let out = state.clone();
        let m = mount_at(PHONE, move || {
            let sidebar = SidebarState::new(SidebarCollapse::Icons);
            sidebar.drawer.set(true);
            out.set(Some(sidebar));
            let go: Rc<dyn Fn()> = Rc::new(move || flag.set(true));
            ui! { SidebarItem(sidebar = sidebar, label = "Projects".to_string(), on_press = go) }
        });
        let sidebar: SidebarState = state.get().expect("built");
        m.h.press_labelled("Projects");
        m.h.flush();
        assert!(went.get());
        assert!(!m.h.world.enter(|| sidebar.drawer.get()));
    }
}

mod header_nav {
    use super::*;
    use idea_ui::Crumb;
    use idea_ui_layout::{HeaderTrail, HeaderWayBack, WayBack};

    fn crumbs() -> Vec<Crumb> {
        vec![Crumb::linked("Projects", noop()), Crumb::linked("North Ramp", noop())]
    }

    fn pushed(form: WayBack, went_back: Rc<Cell<u32>>) -> Element {
        let back: Rc<dyn Fn()> = Rc::new(move || went_back.set(went_back.get() + 1));
        ui! {
            PageHeader(
                title = "Settings".to_string(),
                nav = Some(ui! { HeaderWayBack(crumbs = crumbs(), on_back = Some(back), form = form) }),
            )
        }
    }

    /// The way back on the web and the desktop: every ancestor named
    /// and linked, and no arrow.
    #[test]
    fn a_trail_names_every_ancestor_and_draws_no_back_arrow() {
        let m = mount_at(DESKTOP, || pushed(WayBack::Trail, Rc::default()));
        let log = m.log();
        for part in ["\"Projects\"", "\"North Ramp\"", "\"Settings\""] {
            assert!(log.contains(part), "{part}:\n{log}");
        }
        // The two crumbs are the only pressables: no back arrow, no menus.
        assert_eq!(log.matches(" pressable").count(), 2, "{log}");
    }

    /// The kit's trail folds on a phone: the ancestors are one "…" and
    /// the row is the title's.
    #[test]
    fn a_phone_trail_folds_its_ancestors_into_one_menu() {
        let m = mount_at(PHONE, || pushed(WayBack::Trail, Rc::default()));
        let closed = m.log();
        assert!(closed.contains("\"Settings\""), "{closed}");
        assert!(!closed.contains("\"Projects\"") && !closed.contains("\"North Ramp\""), "{closed}");
        (m.h.press_handler(0))();
        m.h.flush();
        let open = m.log();
        assert!(open.contains("\"Projects\"") && open.contains("\"North Ramp\""), "{open}");
    }

    /// Folding is the trail's default, not the header's rule: an app
    /// that wants every crumb on a phone turns it off.
    #[test]
    fn a_trail_told_not_to_fold_draws_every_ancestor_on_a_phone() {
        let log = mount_at(PHONE, || ui! { HeaderTrail(crumbs = crumbs(), fold_on_phone = false) }).log();
        assert!(log.contains("\"Projects\"") && log.contains("\"North Ramp\""), "{log}");
    }

    /// The way back on iOS and Android: an arrow that pops, and no
    /// trail taking the title's room.
    #[test]
    fn the_arrow_form_draws_the_arrow_and_no_trail() {
        let went_back = Rc::new(Cell::new(0));
        let count = went_back.clone();
        let m = mount_at(PHONE, move || pushed(WayBack::Arrow, count));
        let log = m.log();
        assert!(log.contains("\"Settings\""), "{log}");
        assert!(!log.contains("\"Projects\""), "{log}");
        (m.h.press_handler(0))();
        m.h.flush();
        assert_eq!(went_back.get(), 1);
    }

    /// The host mock is no phone OS, so `Auto` is the trail.
    #[test]
    fn auto_follows_the_platform() {
        let log = mount_at(DESKTOP, || pushed(WayBack::Auto, Rc::default())).log();
        assert!(log.contains("\"Projects\""), "{log}");
    }

    /// The header is slots: what stands in `nav` and in the title's
    /// seat is whatever the app passes.
    #[test]
    fn the_nav_and_title_slots_take_any_element() {
        let log = mount_at(PHONE, || {
            ui! {
                PageHeader(
                    title = "IGNORED".to_string(),
                    nav = Some(ui! { Typography(content = "OWN NAV".to_string()) }),
                    title_content = Some(ui! { Typography(content = "OWN TITLE".to_string()) }),
                )
            }
        })
        .log();
        assert!(log.contains("\"OWN NAV\"") && log.contains("\"OWN TITLE\""), "{log}");
        assert!(!log.contains("\"IGNORED\""), "the slot replaces the title's text:\n{log}");
    }
}

mod toolbar {
    use super::*;
    use idea_ui::Tab;
    use idea_ui_layout::{Toolbar, ToolbarButton, ToolbarSearch, ToolbarTabs};

    #[test]
    fn a_toolbar_draws_its_start_end_and_below_slots() {
        let log = mount_at(DESKTOP, || {
            ui! {
                Toolbar(
                    start = Some(ui! { Typography(content = "START".to_string()) }),
                    end = Some(ui! { Typography(content = "END".to_string()) }),
                    below = Some(ui! { Typography(content = "BELOW".to_string()) }),
                )
            }
        })
        .log();
        for part in ["\"START\"", "\"END\"", "\"BELOW\""] {
            assert!(log.contains(part), "{part}:\n{log}");
        }
    }

    fn filter(count: usize) -> Element {
        ui! { ToolbarButton(label = "Filter".to_string(), icon = Some(GLYPH), count = count) }
    }

    /// A toolbar control collapses to its glyph on a phone; it never
    /// takes a shorter label.
    #[test]
    fn a_toolbar_button_is_labelled_when_wide_and_a_glyph_on_a_phone() {
        let wide = mount_at(DESKTOP, || filter(0)).log();
        assert!(wide.contains("\"Filter\""), "{wide}");
        let counted = mount_at(TABLET, || filter(2)).log();
        assert!(counted.contains("\"Filter · 2\""), "{counted}");
        let phone = mount_at(PHONE, || filter(2)).log();
        assert!(!phone.contains("Filter"), "{phone}");
        assert!(phone.contains(" icon"), "{phone}");
    }

    #[test]
    fn a_toolbar_button_with_no_glyph_keeps_its_label_on_a_phone() {
        let log = mount_at(PHONE, || ui! { ToolbarButton(label = "Columns".to_string()) }).log();
        assert!(log.contains("\"Columns\""), "{log}");
    }

    #[test]
    fn inline_search_is_the_field_and_the_glyph_opens_one() {
        let inline = mount_at(PHONE, || ui! { ToolbarSearch(inline = true) }).log();
        assert!(inline.contains("text_input"), "{inline}");

        let m = mount_at(PHONE, || ui! { ToolbarSearch() });
        let closed = m.log();
        assert!(!closed.contains("text_input"), "{closed}");
        (m.h.press_handler(0))();
        m.h.flush();
        let open = m.log();
        assert!(open.contains("text_input"), "{open}");
    }

    /// Ctrl-F (⌘F on a Mac) opens the glyph's field and takes the key
    /// from the browser's find; an app holding `open` works it too; a
    /// search told to leave the chord alone does.
    #[test]
    fn the_find_chord_and_an_outside_signal_open_the_search() {
        use runtime_core::{AppKeyEvent, KeyOutcome};
        use runtime_shared::key_input::deliver;
        let find = |ctrl: bool, meta: bool| {
            let mut down = AppKeyEvent::down("f", "KeyF");
            (down.ctrl, down.meta) = (ctrl, meta);
            let outcome = deliver(&down);
            let mut up = down.clone();
            up.phase = runtime_core::KeyPhase::Up;
            deliver(&up);
            outcome
        };

        let handle = Rc::new(Cell::new(None));
        let out = handle.clone();
        let m = mount_at(DESKTOP, move || {
            let open = signal(false);
            out.set(Some(open));
            ui! { ToolbarSearch(open = Some(open)) }
        });
        let open: Signal<bool> = handle.get().expect("built");
        assert!(!m.log().contains("text_input"));
        assert_eq!(find(false, false), KeyOutcome::Default, "a bare f is typing");
        assert_eq!(find(true, false), KeyOutcome::PreventDefault, "the browser's find does not open");
        m.h.flush();
        assert!(m.h.world.enter(|| open.get()));
        assert!(m.log().contains("text_input"), "Ctrl-F opened the field");

        m.h.world.enter(|| open.set(false));
        m.h.flush();
        m.log();
        assert_eq!(find(false, true), KeyOutcome::PreventDefault, "⌘F is the same chord");
        m.h.flush();
        assert!(m.log().contains("text_input"));

        m.h.world.enter(|| open.set(false));
        m.h.flush();
        m.log();
        m.h.world.enter(|| open.set(true));
        m.h.flush();
        assert!(m.log().contains("text_input"), "the app's own signal opens it");
        drop(m);

        let quiet = mount_at(DESKTOP, || ui! { ToolbarSearch(find_shortcut = false) });
        quiet.log();
        assert_eq!(find(true, false), KeyOutcome::Default, "left to the browser");
        quiet.h.flush();
        assert!(!quiet.log().contains("text_input"));
    }

    /// A compact control is its glyph at every width, the count a number
    /// on its corner and nothing at zero.
    #[test]
    fn a_compact_toolbar_button_is_a_glyph_with_its_count_at_every_width() {
        for width in [PHONE, DESKTOP] {
            let handle = Rc::new(Cell::new(None));
            let out = handle.clone();
            let m = mount_at(width, move || {
                let on: Signal<usize> = signal(0);
                out.set(Some(on));
                ui! { ToolbarButton(label = "Filter".to_string(), icon = Some(GLYPH), count = on, compact = true) }
            });
            let on: Signal<usize> = handle.get().expect("built");
            let none = m.log();
            assert!(!none.contains("\"Filter") && none.contains(" icon"), "{width}: a glyph, no label:\n{none}");
            assert!(!none.contains("\"0\""), "{width}: nothing at zero:\n{none}");
            m.h.world.enter(|| on.set(3));
            m.h.flush();
            let three = m.log();
            assert!(three.contains("\"3\""), "{width}: the count is a number:\n{three}");
        }
    }

    #[test]
    fn search_writes_the_screens_term_and_reports_the_edit() {
        host_mock::pump::install_scheduler();
        let h = Harness::new();
        let seen = Rc::new(Cell::new(0));
        let count = seen.clone();
        let (tree, value) = h.world.enter(|| {
            install_idea_theme(light_theme());
            let value = signal(String::new());
            let after: Rc<dyn Fn(String)> = Rc::new(move |_| count.set(count.get() + 1));
            (ui! { ToolbarSearch(value = value, inline = true, on_change = Some(after)) }, value)
        });
        let _realized = h.mount(tree);
        h.flush();
        (h.text_input_change(0))("ramp".to_string());
        h.flush();
        assert_eq!(h.world.enter(|| value.get()), "ramp");
        assert_eq!(seen.get(), 1);
    }

    #[test]
    fn toolbar_tabs_draw_every_tab_and_report_a_pick() {
        let picked = Rc::new(Cell::new(false));
        let flag = picked.clone();
        let m = mount_at(DESKTOP, move || {
            let tabs = signal(vec![Tab::new("work", "Work"), Tab::new("crew", "Crew"), Tab::new("log", "Log")]);
            let on_change: Rc<dyn Fn(String)> = Rc::new(move |id| flag.set(id == "crew"));
            ui! { ToolbarTabs(tabs = tabs, active = "work".to_string(), on_change = on_change) }
        });
        let log = m.log();
        for part in ["\"Work\"", "\"Crew\"", "\"Log\""] {
            assert!(log.contains(part), "{part}:\n{log}");
        }
        m.h.press_labelled("Crew");
        m.h.flush();
        assert!(picked.get());
    }

    /// REGRESSION: the pill row's clip was a fixed 28 px line (and 2 px
    /// of slack), not tied to the pills, so an app whose controls are
    /// taller (CrewForge's 34 px pills) got every pill's bottom cut off
    /// and could not restyle the private sheet. The line is the tallest
    /// pill's measured height.
    #[test]
    fn regression_taller_pills_are_clipped_at_a_fixed_line() {
        use std::cell::RefCell;
        use std::collections::HashMap;

        use runtime_core::{Length, ViewportRect};

        const PILL_H: f32 = 34.0;
        host_mock::pump::install_scheduler();
        let h = Harness::new();
        // Every node's last applied pixel height.
        let heights: Rc<RefCell<HashMap<host_mock::Node, f32>>> = Rc::new(RefCell::new(HashMap::new()));
        {
            let heights = heights.clone();
            h.set_style_line(move |node, rules| {
                if let Some(Length::Px(px)) = rules.height.as_ref().map(|t| *t.value()) {
                    heights.borrow_mut().insert(node, px);
                }
                format!("apply_style n{node}")
            });
        }
        let tree = h.world.enter(|| {
            install_idea_theme(light_theme());
            viewport_size().set(ViewportSize::new(DESKTOP, 800.0));
            let tabs = signal(vec![Tab::new("work", "Work"), Tab::new("crew", "Crew")]);
            let on_change: Rc<dyn Fn(String)> = Rc::new(|_| {});
            ui! { ToolbarTabs(tabs = tabs, active = "work".to_string(), on_change = on_change) }
        });
        h.flush();
        let _realized = h.mount(tree);
        h.flush();
        let clipped = |at: f32| heights.borrow().values().any(|px| (*px - at).abs() < 0.01);
        assert!(clipped(30.0), "the unmeasured line is idea-ui's pill: {:?}", heights.borrow());
        // Lay every node out as a pill would be: 34 tall, with room for
        // all of them.
        let nodes: Vec<host_mock::Node> = h.shared.kinds.borrow().keys().copied().collect();
        for n in nodes {
            h.place(n, ViewportRect { x: 0.0, y: 0.0, width: 200.0, height: PILL_H });
        }
        host_mock::pump::pump_frames();
        h.flush();
        host_mock::pump::pump_timers();
        h.flush();
        assert!(clipped(PILL_H + 2.0), "the line follows the pills: {:?}", heights.borrow());
    }
}

mod sheet_rows {
    use super::*;
    use idea_ui_layout::SheetPickRow;

    /// One of a set: the chosen row carries the check, the others keep
    /// its slot empty, and a press reports the pick.
    #[test]
    fn a_pick_row_marks_the_one_chosen_and_reports_a_press() {
        let picked = Rc::new(Cell::new(false));
        let flag = picked.clone();
        let handle = Rc::new(Cell::new(None));
        let out = handle.clone();
        let m = mount_at(PHONE, move || {
            let dark = signal(false);
            out.set(Some(dark));
            let pick: Rc<dyn Fn()> = Rc::new(move || flag.set(true));
            ui! { SheetPickRow(label = "Dark".to_string(), selected = dark, on_press = pick) }
        });
        let dark: Signal<bool> = handle.get().expect("built");
        let unpicked = m.log();
        assert!(unpicked.contains("\"Dark\""), "{unpicked}");
        assert!(!unpicked.contains(" icon"), "no check until it is the one chosen:\n{unpicked}");
        m.h.world.enter(|| dark.set(true));
        m.h.flush();
        let chosen = m.log();
        assert!(chosen.contains(" icon"), "the check:\n{chosen}");
        (m.h.press_handler(0))();
        assert!(picked.get());
    }
}

mod blocks {
    use super::*;
    use idea_ui_layout::{LabelValue, Section, SplitMode, SplitView};

    /// A hint is a `?` beside the title and an action stands in the
    /// heading; a blank hint draws nothing.
    #[test]
    fn a_section_heading_carries_its_hint_and_its_action() {
        let plain = mount_at(DESKTOP, || ui! { Section(title = "Rotations".to_string(), hint = Some("  ".to_string())) }).log();
        assert!(plain.contains("\"Rotations\"") && !plain.contains(" icon"), "{plain}");
        let full = mount_at(DESKTOP, || {
            ui! {
                Section(
                    title = "Rotations".to_string(),
                    hint = Some("A rotation repeats until its end date.".to_string()),
                    action = Some(ui! { Typography(content = "ADD".to_string()) }),
                ) {
                    Typography(content = "BODY".to_string())
                }
            }
        })
        .log();
        for part in ["\"Rotations\"", " icon", "\"ADD\"", "\"BODY\""] {
            assert_eq!(full.matches(part).count(), 1, "{part}:\n{full}");
        }
    }

    #[test]
    fn a_label_value_carries_its_hint() {
        let plain = mount_at(DESKTOP, || ui! { LabelValue(label = "Waste".to_string(), value = Some("5%".to_string())) }).log();
        assert!(!plain.contains(" icon"), "{plain}");
        let hinted = mount_at(DESKTOP, || {
            ui! { LabelValue(label = "Waste".to_string(), value = Some("5%".to_string()), hint = Some("Added before pricing.".to_string())) }
        })
        .log();
        assert!(hinted.contains("\"Waste\"") && hinted.contains("\"5%\"") && hinted.contains(" icon"), "{hinted}");
    }

    /// The card form is a restyle of the same pane: both regions mount
    /// once at every width.
    #[test]
    fn a_carded_inspector_mounts_both_regions_once() {
        for width in [PHONE, TABLET, DESKTOP] {
            let log = mount_at(width, || {
                ui! {
                    SplitView(
                        mode = SplitMode::Inspector,
                        card = true,
                        first = ui! { Typography(content = "CANVAS".to_string()) },
                        second = ui! { Typography(content = "PANE".to_string()) },
                    )
                }
            })
            .log();
            for part in ["\"CANVAS\"", "\"PANE\""] {
                assert_eq!(log.matches(part).count(), 1, "{width}: {part}\n{log}");
            }
        }
    }
}

/// Records each `(x, y)` a scroller reports.
fn scroll_log() -> (Rc<std::cell::RefCell<Vec<(f32, f32)>>>, Rc<dyn Fn(f32, f32)>) {
    let seen = Rc::new(std::cell::RefCell::new(Vec::new()));
    let push = seen.clone();
    (seen, Rc::new(move |x, y| push.borrow_mut().push((x, y))))
}

/// CrewForge: a list that reorders by drag in a `PinnedPane` could not
/// reach the pane's scroller, so a drag could neither scroll it near an
/// edge nor tell its drag context it scrolled; and the page's scroller,
/// which moves the pane until it pins, reported to no one. Both
/// scrollers are now in reach: the page's and the pane's `on_scroll`
/// each hear their own scroller, and the pane's handle moves the pane's.
#[test]
fn a_pinned_pane_and_its_page_hand_out_their_scrollers() {
    let (page_seen, on_page) = scroll_log();
    let (pane_seen, on_pane) = scroll_log();
    let pane: Ref<ScrollViewHandle> = Ref::new();
    let frame = Cell::new(None);
    let m = mount_at(DESKTOP, || {
        let made = PageFrame::new();
        frame.set(Some(made));
        let frame = made;
        ui! {
            Page(
                header = ui! { PageHeader(title = "Bid".to_string()) },
                frame = Some(frame),
                on_scroll = Some(on_page.clone()),
                body = ui! {
                    PinnedPane(frame = frame, scroller = Some(pane), on_scroll = Some(on_pane.clone())) {
                        Typography(content = "SCOPES".to_string())
                    }
                },
            )
        }
    });
    assert_eq!(m.h.scroll_view_count(), 2, "the page's scroller and the pane's");
    // Created outside in: the page's first.
    (m.h.scroll_handler(0).expect("the page's on_scroll reaches its scroller"))(0.0, 120.0);
    (m.h.scroll_handler(1).expect("the pane's on_scroll reaches its scroller"))(0.0, 40.0);
    assert_eq!(*page_seen.borrow(), [(0.0, 120.0)]);
    assert_eq!(*pane_seen.borrow(), [(0.0, 40.0)]);

    host_mock::take_handle_log();
    pane.with(|h| h.scroll_to(0.0, 64.0)).expect("the pane's handle is bound");
    frame.get().expect("built").scroll_to(30.0);
    let moved = host_mock::take_handle_log();
    let node = |line: &str| line.rsplit(' ').next().map(str::to_string);
    assert!(moved.len() == 2 && moved[0].starts_with("scroll_to 0 64 "), "{moved:?}");
    assert_ne!(node(&moved[0]), node(&moved[1]), "the pane's handle moves the pane, not the page: {moved:?}");
}

/// No `on_scroll`, no handler: a scroller without one stays silent
/// rather than carrying a no-op.
#[test]
fn a_page_or_pane_without_on_scroll_installs_no_handler() {
    let m = mount_at(DESKTOP, || {
        let frame = PageFrame::new();
        ui! {
            Page(
                header = ui! { PageHeader(title = "Bid".to_string()) },
                frame = Some(frame),
                body = ui! { PinnedPane(frame = frame) { Typography(content = "SCOPES".to_string()) } },
            )
        }
    });
    assert_eq!(m.h.scroll_view_count(), 2);
    assert!(m.h.scroll_handler(0).is_none() && m.h.scroll_handler(1).is_none());
}

/// The same reach for a split's self-scrolling region, in every mode:
/// Nav's list, Detail's panel, Inspector's pane.
#[test]
fn a_split_view_hands_out_its_side_scroller() {
    for mode in [SplitMode::Nav, SplitMode::Detail, SplitMode::Inspector] {
        let (seen, on_scroll) = scroll_log();
        let side: Ref<ScrollViewHandle> = Ref::new();
        let m = mount_at(DESKTOP, || {
            ui! {
                SplitView(
                    mode = mode,
                    first = ui! { Typography(content = "FIRST".to_string()) },
                    second = ui! { Typography(content = "SECOND".to_string()) },
                    open = true,
                    scroller = Some(side),
                    on_scroll = Some(on_scroll.clone()),
                )
            }
        });
        assert_eq!(m.h.scroll_view_count(), 1, "{mode:?}");
        (m.h.scroll_handler(0).expect("on_scroll reaches the side scroller"))(0.0, 30.0);
        assert_eq!(*seen.borrow(), [(0.0, 30.0)], "{mode:?}");
        host_mock::take_handle_log();
        side.with(|h| h.scroll_to(0.0, 10.0)).expect("the side handle is bound");
        assert_eq!(host_mock::take_handle_log().len(), 1, "{mode:?}");
    }
}
