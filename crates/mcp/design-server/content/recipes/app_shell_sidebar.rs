//! # App shell: pinned sidebar on desktop, drawer on mobile
//!
//! summary: The default shell for a multi-section app — SwapNavigator + idea-ui-nav AppShell, themed sidebar nav, mobile top bar.
//! tags: navigation, shell, sidebar, responsive, swap
//! render: / @desktop => Acme, Overview, Projects, What needs your attention, !Everything your team
//! render: / @phone => Acme, What needs your attention, !Everything your team
//! render: /settings @desktop => Notifications, members and billing, !What needs your attention
//!
//! Use this when the app has 3–7 top-level, co-equal sections (a dashboard,
//! an admin, a workspace). The sections are a `SwapNavigator` (one visible
//! screen, no back stack); the chrome is `AppShell`, which pins the sidebar
//! at `pin_at` and turns it into an off-canvas drawer below it, building the
//! sidebar exactly once.
//!
//! Decisions this recipe makes for you:
//! - Navigation rows are `ListItem(active = …, on_press = …)`: the active
//!   section is shown by the component's own `active` state, so a restyle
//!   of `ListItemStyles` rebrands every nav row at once.
//! - The mobile top bar exists only while the sidebar is a drawer
//!   (`if !sidebar_pinned(..)`), and a nav press closes the drawer only
//!   when it is one.
//! - The outlet is splatted bare inside a column that fills the shell: it
//!   carries its own `flex: 1 1 0; min-height: 0` fill rules.
//! - Every color, gap and radius is a theme token, so the brand module
//!   (see the `brand_module` recipe) restyles this shell with no edits.

use idea_ui::{
    tone, typography_kind, variant, Icon, IconButton, IconButtonSize, IdeaThemeRef, ListItem,
    Spacer, Stack, StackAxis, StackAlign, StackGap, StackPadding, Typography,
};
use idea_ui_nav::{sidebar_pinned, AppShell};
use runtime_core::{
    rx, signal, stylesheet, ui, Breakpoint, Element, FlexDirection, IconData, Length, Ref, Route,
    SafeAreaSides, Screen, Signal,
};
use std::rc::Rc;
use swap_navigator::{SwapBuilder, SwapHandle, SwapNavigator};

const HOME: Route<()> = Route::<()>::new("home", "/");
const PROJECTS: Route<()> = Route::<()>::new("projects", "/projects");
const SETTINGS: Route<()> = Route::<()>::new("settings", "/settings");

/// One sidebar entry: route name, label, icon. The list IS the app's
/// information architecture — keep it short and in priority order.
const SECTIONS: &[(&str, &str, IconData)] = &[
    ("home", "Overview", icons_lucide::HOUSE),
    ("projects", "Projects", icons_lucide::FOLDER),
    ("settings", "Settings", icons_lucide::SETTINGS),
];

/// The breakpoint the sidebar pins at. One constant, read by both the
/// shell and the mobile top bar, so they can never disagree.
const PIN_AT: Breakpoint = Breakpoint::Lg;

stylesheet! {
    /// Fills the shell's content area: the top bar (when present) over the
    /// outlet.
    ContentColumn<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            width: Length::pct(100.0),
            height: Length::pct(100.0),
            min_height: Length::Px(0.0),
            background: t.color.background(),
        }
    }
}

stylesheet! {
    /// The mobile-only bar that hosts the menu button and the app name.
    TopBar<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: runtime_core::AlignItems::Center,
            gap: t.spacing.sm(),
            padding_vertical: t.spacing.sm(),
            padding_horizontal: t.spacing.md(),
            background: t.color.surface(),
            border_bottom_width: 1.0,
            border_bottom_color: t.color.border(),
        }
    }
}

stylesheet! {
    /// The sidebar panel's own scroll surface: AppShell's panel is a fixed
    /// height box, so a long nav must scroll inside it.
    SidebarScroll<IdeaThemeRef> {
        base(t) {
            height: Length::pct(100.0),
            background: t.color.surface(),
            border_right_width: 1.0,
            border_right_color: t.color.border(),
        }
    }
}

/// The app's root. In a real app `app()` installs YOUR brand (see the
/// `brand_module` recipe) instead of the stock light theme.
pub fn app() -> Element {
    idea_ui::install_idea_theme(idea_ui::light_theme());

    let nav: Ref<SwapHandle> = Ref::new();
    let drawer_open: Signal<bool> = signal(false);

    let builder = SwapNavigator::new(&HOME)
        .screen(HOME, |_| Screen::new(ui! { PlaceholderPage(title = "Overview", subtitle = "What needs your attention today.") }))
        .screen(PROJECTS, |_| Screen::new(ui! { PlaceholderPage(title = "Projects", subtitle = "Everything your team is working on.") }))
        .screen(SETTINGS, |_| Screen::new(ui! { PlaceholderPage(title = "Settings", subtitle = "Notifications, members and billing.") }))
        .layout(move |nav| {
            let active = nav.active_route;
            let select = nav.on_select.clone();
            // A nav press switches section, then closes the drawer — but
            // only when the sidebar IS a drawer.
            let go: Rc<dyn Fn(&'static str)> = Rc::new(move |route| {
                select(route);
                if !sidebar_pinned(PIN_AT) {
                    drawer_open.set(false);
                }
            });
            let sidebar = ui! { SidebarNav(active = active, go = go) };
            let open_drawer: Rc<dyn Fn()> = Rc::new(move || drawer_open.set(true));
            let outlet = nav.outlet;
            ui! {
                AppShell(sidebar = vec![sidebar], is_open = drawer_open, pin_at = PIN_AT, width = 264.0) {
                    view(style = ContentColumn()) {
                        if !sidebar_pinned(PIN_AT) {
                            // The bar sits under a phone's notch: it pads itself.
                            view(style = TopBar(), safe_area = SafeAreaSides::TOP) {
                                IconButton(
                                    icon = Some(icons_lucide::MENU),
                                    on_click = open_drawer.clone(),
                                    tone = tone::Neutral,
                                    variant = variant::Ghost,
                                    size = IconButtonSize::Md,
                                )
                                Typography(content = "Acme", kind = typography_kind::H4)
                            }
                        }
                        outlet
                    }
                }
            }
        });

    ui! { builder.bind(nav) }
}

/// The sidebar: product name on top, sections below. Built once by
/// AppShell; `active` keeps it current.
#[runtime_core::component]
fn SidebarNav(
    #[prop(default = signal("home"))] active: Signal<&'static str>,
    #[prop(default = Rc::new(|_: &'static str| {}) as Rc<dyn Fn(&'static str)>)] go: Rc<dyn Fn(&'static str)>,
) -> Element {
    ui! {
        scroll_view(style = SidebarScroll()) {
            Stack(gap = StackGap::Lg, padding = StackPadding::Md) {
                Stack(axis = StackAxis::Row, align = StackAlign::Center, gap = StackGap::Sm) {
                    Typography(content = "Acme", kind = typography_kind::H3)
                }
                Stack(gap = StackGap::Xs) {
                    for (route, label, glyph) in SECTIONS.iter().copied() {
                        ListItem(
                            label = label.to_string(),
                            leading = Some(ui! { Icon(data = glyph, size = 18.0) }),
                            active = rx!(active.get() == route),
                            on_press = Some({
                                let go = go.clone();
                                Rc::new(move || go(route)) as Rc<dyn Fn()>
                            }),
                        )
                    }
                }
                Spacer()
            }
        }
    }
}

/// Stand-in screen body. Real screens follow the `page_layout` recipe.
#[runtime_core::component]
fn PlaceholderPage(title: String, subtitle: String) -> Element {
    ui! {
        Stack(gap = StackGap::Xs, padding = StackPadding::Lg) {
            Typography(content = title, kind = typography_kind::H1)
            Typography(content = subtitle, muted = true)
        }
    }
}
