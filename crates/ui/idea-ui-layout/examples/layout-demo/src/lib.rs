//! `layout-demo` — every layout in `idea-ui-layout`, one per screen, to
//! look at across phone (390), tablet (820) and desktop (1440) widths.
//!
//! | Screen | Shows |
//! |---|---|
//! | Content page | `Page(Content)`, `PageHeader`, `Section`, `StatGrid`, `LabelValue`, `FieldGrid`, `Dialog` |
//! | Table page | `Page(Table)`, a pinned toolbar, `ListStates` in each of its states |
//! | Blank page | `Page(Blank)`: a body that lays itself out edge to edge |
//! | Split · Nav | `SplitView(Nav)` in a scrolling page, its list pinned |
//! | Split · Detail | `SplitView(Detail)`: a record and its companion panel |
//! | Workbench | `Page(Blank)` holding `SplitView(Inspector)` |
//! | Flow | `Flow`, `ActionBar(pinned)` |
//! | Shell with tabs | one `Page` whose `live_variant` follows the tab |
//! | Overlays | `ActionMenu`, `ActionMenuSurface`, `AdaptivePopover`, `DrillSheet`, `Dialog`, `Sheet` |
//!
//! No screen here reads the breakpoint or writes a breakpoint overlay:
//! what changes between widths is the kit's doing (rule `layout`). The
//! one width read is the shell's ☰, which `idea_ui_nav` answers.

use std::rc::Rc;

use icons_lucide::MENU;
use idea_ui::{install_idea_theme, light_theme, typography_kind, variant, IconButton, ListItem, Stack, StackGap, StackPadding, Typography};
use idea_ui_nav::{sidebar_pinned, AppShell};
use runtime_core::{component, rx, signal, stylesheet, ui, Breakpoint, Element, FlexDirection, Length, Signal};

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

/// The width the sidebar pins from: the ladder's desktop.
const PIN_AT: Breakpoint = Breakpoint::Lg;

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
            Demo::ShellTabs => "Shell with tabs",
            Demo::Overlays => "Overlays",
        }
    }
}

/// What every screen needs from the shell: which screen is on show, and
/// the drawer its header's ☰ opens.
#[derive(Clone, Copy)]
pub(crate) struct Shell {
    pub screen: Signal<Demo>,
    pub drawer: Signal<bool>,
}

impl Default for Shell {
    fn default() -> Self {
        Self { screen: signal(Demo::default()), drawer: signal(false) }
    }
}

impl Shell {
    /// What stands before a screen's title: the drawer's ☰, drawn only
    /// while the sidebar is a drawer.
    pub fn leading(self) -> Element {
        let open: Rc<dyn Fn()> = Rc::new(move || self.drawer.set(true));
        ui! {
            if !sidebar_pinned(PIN_AT) {
                IconButton(icon = Some(MENU), variant = variant::Ghost, label = Some("Menu".to_string()), on_click = open.clone())
            }
        }
    }
}

/// The app: the sidebar's three seats (pinned on a desktop, a drawer on
/// a tablet and a phone) round the screen on show.
pub fn app() -> Element {
    install_idea_theme(light_theme());
    let shell = Shell::default();
    let screen = shell.screen;
    ui! {
        view(style = Root()) {
            AppShell(sidebar = vec![ui! { Sidebar(shell = shell) }], is_open = shell.drawer, pin_at = PIN_AT) {
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
        Stack(gap = StackGap::Xs, padding = StackPadding::Md) {
            Typography(content = "idea-ui-layout".to_string(), kind = typography_kind::H3)
            for demo in Demo::ALL.iter(), key = demo.label() {
                ListItem(
                    label = demo.label().to_string(),
                    active = { let demo = *demo; rx!(shell.screen.get() == demo) },
                    on_press = {
                        let demo = *demo;
                        let go: Rc<dyn Fn()> = Rc::new(move || {
                            shell.screen.set(demo);
                            shell.drawer.set(false);
                        });
                        Some(go)
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
