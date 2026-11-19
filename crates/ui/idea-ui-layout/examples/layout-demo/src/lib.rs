//! `layout-demo` — every layout in `idea-ui-layout`, one per screen, to
//! look at across phone (390), tablet (820) and desktop (1440) widths.
//!
//! | Screen | Shows |
//! |---|---|
//! | Content page | `Page(Content)`, `PageHeader` with a trail, `Section`, `StatGrid`, `LabelValue`, `FieldGrid`, `Dialog` |
//! | Table page | `Page(Table)`, a `Toolbar` with an inline `ToolbarSearch` and a `ToolbarButton`, `ListStates` in each of its states |
//! | Blank page | `Page(Blank)`: a body that lays itself out edge to edge |
//! | Split · Nav | `SplitView(Nav)` in a scrolling page, its list pinned |
//! | Split · Detail | `SplitView(Detail)`: a record and its companion panel |
//! | Workbench | `Page(Blank)` holding `SplitView(Inspector)` |
//! | Flow | `Flow`, `ActionBar(pinned)` |
//! | Tabbed record | a project whose tabs are each a screen with its own `Page`, sharing one header and one `ToolbarTabs` strip that overflows into "N more" |
//! | Overlays | `ActionMenu`, `ActionMenuSurface`, `AdaptivePopover`, `DrillSheet`, `Dialog`, `Sheet` |
//!
//! The picker in every screen's header switches the frame between its
//! two looks (`FrameSurface::Flat` and `Raised`), live.
//!
//! No screen here reads the breakpoint or writes a breakpoint overlay:
//! what changes between widths is the kit's doing (rule `layout`).

use std::rc::Rc;

use icons_lucide::{COLUMNS_2, FILE_TEXT, FOLDER, LAYERS, LIST, LIST_ORDERED, SQUARE, TABLE, WRENCH};
use idea_ui::{install_idea_theme, light_theme, tone, typography_kind, Crumb, Select, SelectOption, Stack, StackGap, StackPadding, Typography};
use idea_ui_layout::{
    AppFrame, FrameSurface, HeaderWayBack, SidebarCollapse, SidebarCount, SidebarItem, SidebarState, SidebarToggle,
};
use runtime_core::{component, rx, signal, stylesheet, ui, Element, FlexDirection, IconData, Length, Signal};

mod screens;

use screens::{
    BlankScreen, ContentScreen, FlowScreen, OverlaysScreen, ShellTabsScreen, SplitDetailScreen, SplitNavScreen,
    TableScreen, WorkbenchScreen,
};

#[cfg(feature = "sidecar")]
pub fn register_scene_extensions_recorder(_registry: &mut dev_server::newcore::SceneRegistry) {}

/// The SDK-handler seam the generated wrappers call. This app adds none.
pub fn register_scene_extensions<H: runtime_scene::Host>(_registry: &mut runtime_scene::Registry<H>) {}

/// The Android wrapper's conventional entry name.
pub fn scene_app() -> Element {
    app()
}

/// The looks the header's picker offers, by the id the picker stores.
const SURFACES: [(&str, &str, FrameSurface); 2] =
    [("flat", "Flat: sidebar ruled off", FrameSurface::Flat), ("raised", "Raised: content on a card", FrameSurface::Raised)];

/// The layout on show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Demo {
    #[default]
    Content,
    Table,
    Blank,
    SplitNav,
    SplitDetail,
    Workbench,
    Flow,
    ShellTabs,
    Overlays,
}

impl Demo {
    /// What waits at a destination: a quiet count and an alarm, to show
    /// the two tones a sidebar count takes.
    fn count(self) -> Option<SidebarCount> {
        match self {
            Demo::Table => Some(SidebarCount::new(12, tone::Neutral)),
            Demo::Flow => Some(SidebarCount::new(2, tone::Danger)),
            _ => None,
        }
    }

    const ALL: [Demo; 9] = [
        Demo::Content,
        Demo::Table,
        Demo::Blank,
        Demo::SplitNav,
        Demo::SplitDetail,
        Demo::Workbench,
        Demo::Flow,
        Demo::ShellTabs,
        Demo::Overlays,
    ];

    fn label(self) -> &'static str {
        match self {
            Demo::Content => "Content page",
            Demo::Table => "Table page",
            Demo::Blank => "Blank page",
            Demo::SplitNav => "Split · Nav",
            Demo::SplitDetail => "Split · Detail",
            Demo::Workbench => "Workbench",
            Demo::Flow => "Flow",
            Demo::ShellTabs => "Tabbed record",
            Demo::Overlays => "Overlays",
        }
    }

    fn icon(self) -> IconData {
        match self {
            Demo::Content => FILE_TEXT,
            Demo::Table => TABLE,
            Demo::Blank => SQUARE,
            Demo::SplitNav => LIST,
            Demo::SplitDetail => COLUMNS_2,
            Demo::Workbench => WRENCH,
            Demo::Flow => LIST_ORDERED,
            Demo::ShellTabs => FOLDER,
            Demo::Overlays => LAYERS,
        }
    }
}

/// What every screen needs from the shell: which screen is on show, and
/// the sidebar its header's toggle works.
#[derive(Clone, Copy)]
pub(crate) struct Shell {
    pub screen: Signal<Demo>,
    /// The sidebar: open or collapsed, and to icons where it is pinned.
    pub sidebar: SidebarState,
    /// The frame's look, by its id in [`SURFACES`]. One signal restyles
    /// the whole app; no screen reads it.
    pub surface: Signal<String>,
}

impl Default for Shell {
    fn default() -> Self {
        Self {
            screen: signal(Demo::default()),
            sidebar: SidebarState::new(SidebarCollapse::Icons),
            surface: signal(SURFACES[0].0.to_string()),
        }
    }
}

impl Shell {
    /// What stands before a screen's title: the sidebar's toggle. ☰
    /// where the sidebar is a drawer, a collapse control where it is
    /// pinned.
    pub fn leading(self) -> Element {
        ui! { SidebarToggle(sidebar = self.sidebar) }
    }

    /// The picker every screen's header carries: how the screens sit
    /// against the sidebar. Its effect shows where the sidebar is
    /// pinned, from 1024 wide.
    pub fn style_picker(self) -> Element {
        let options = SURFACES.iter().map(|(id, label, _)| SelectOption::new(*id, *label)).collect::<Vec<_>>();
        let on_change: Rc<dyn Fn(String)> = Rc::new(move |id| self.surface.set(id));
        ui! { Select(value = self.surface, on_change = on_change, options = options) }
    }

    /// An ancestor crumb that goes to another of the demo's screens.
    pub fn crumb(self, label: &'static str, to: Demo) -> Crumb {
        Crumb::linked(label, Rc::new(move || self.screen.set(to)))
    }

    /// The way back a pushed screen puts in its header's `nav` slot:
    /// the kit's own, which is this trail in a browser and an arrow to
    /// `back` on iOS and Android.
    pub fn way_back(self, crumbs: Vec<Crumb>, back: Demo) -> Element {
        let pop: Rc<dyn Fn()> = Rc::new(move || self.screen.set(back));
        ui! { HeaderWayBack(crumbs = crumbs, on_back = Some(pop)) }
    }

    fn frame_surface(self) -> FrameSurface {
        let id = self.surface.get();
        SURFACES.iter().find(|s| s.0 == id).map_or(FrameSurface::Flat, |s| s.2)
    }
}

/// The app: an `AppFrame` (the sidebar pinned on a desktop, where it
/// collapses to icons, and a drawer on a tablet and a phone) round the
/// screen on show.
pub fn app() -> Element {
    install_idea_theme(light_theme());
    let shell = Shell::default();
    let screen = shell.screen;
    ui! {
        view(style = Root()) {
            AppFrame(
                sidebar = vec![ui! { Sidebar(shell = shell) }],
                state = shell.sidebar,
                surface = rx!(shell.frame_surface()),
            ) {
                match screen.get() {
                    Demo::Content => { ContentScreen(shell = shell) }
                    Demo::Table => { TableScreen(shell = shell) }
                    Demo::Blank => { BlankScreen(shell = shell) }
                    Demo::SplitNav => { SplitNavScreen(shell = shell) }
                    Demo::SplitDetail => { SplitDetailScreen(shell = shell) }
                    Demo::Workbench => { WorkbenchScreen(shell = shell) }
                    Demo::Flow => { FlowScreen(shell = shell) }
                    Demo::ShellTabs => { ShellTabsScreen(shell = shell) }
                    Demo::Overlays => { OverlaysScreen(shell = shell) }
                }
            }
        }
    }
}

/// The destinations, the same list at every width.
#[component]
fn Sidebar(#[prop(static)] shell: Shell) -> Element {
    ui! {
        Stack(gap = StackGap::Xs, padding = StackPadding::Sm) {
            // The sidebar's own content keeps its line and lets the
            // narrowing sidebar clip it, so the items under it hold still.
            Typography(content = "idea-ui-layout".to_string(), kind = typography_kind::H3, lines = Some(1))
            for demo in Demo::ALL.iter(), key = demo.label() {
                SidebarItem(
                    sidebar = shell.sidebar,
                    icon = Some(demo.icon()),
                    label = demo.label().to_string(),
                    active = { let demo = *demo; rx!(shell.screen.get() == demo) },
                    count = demo.count(),
                    on_press = {
                        let demo = *demo;
                        let go: Rc<dyn Fn()> = Rc::new(move || shell.screen.set(demo));
                        go
                    },
                )
            }
        }
    }
}

stylesheet! {
    // The app's root fills the window; each screen's `Page` grows in it.
    Root<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            width: Length::pct(100.0),
            height: Length::pct(100.0),
        }
    }
}
