//! # App shell: bottom tabs on phones, sidebar on wide screens
//!
//! summary: The adaptive shell for a mobile-first app with 2–5 daily sections — one SwapNavigator whose chrome is a safe-area bottom TabBar on phones and a pinned sidebar from Lg up.
//! tags: navigation, shell, tabs, sidebar, adaptive, responsive, swap, safe-area
//! render: / @phone => Home, Activity, Profile, Good morning, !Acme
//! render: / @desktop => Acme, Home, Activity, Profile, Good morning
//! render: /activity @phone => Nothing new since yesterday, !Good morning
//! render: /activity @desktop => Acme, Nothing new since yesterday, !Good morning
//!
//! Use this instead of `app_shell_sidebar` when the app lives on phones and
//! its sections are few and visited constantly (a consumer app, a field
//! tool). A drawer hides the sections behind a tap; a tab bar keeps them a
//! thumb away. On a wide screen the same sections become a sidebar, because
//! a tab bar stretched across a 1400px window is a toy.
//!
//! Decisions:
//! - **One section list** (`SECTIONS`) drives both the tab bar and the
//!   sidebar, so they can never disagree.
//! - **The outlet never moves.** It sits in the content column at every
//!   width; the sidebar (wide) and the tab bar (phone) appear *around* it,
//!   so crossing the breakpoint keeps every section's state.
//! - **Sections persist** (`MountPolicy::LazyPersistent`, the swap
//!   default): switching tabs returns to the same scroll position and input.
//! - **The tab bar pads for the home indicator** (`safe_area = BOTTOM`); the
//!   framework draws it on every platform and applies no inset itself.
//! - Labels always — an icon-only tab bar is unreadable to new users.

use idea_ui::{
    typography_kind, Icon, IdeaThemeRef, ListItem, Stack, StackGap, StackPadding, TabIndicator,
    Typography,
};
use idea_ui_nav::{TabBar, TabItem};
use runtime_core::{
    current_breakpoint, rx, stylesheet, ui, Breakpoint, Element, FlexDirection, IconData, Length,
    Ref, Route, SafeAreaSides, Screen, Signal,
};
use std::rc::Rc;
use swap_navigator::{SwapBuilder, SwapHandle, SwapNavigator};

const HOME: Route<()> = Route::<()>::new("home", "/");
const ACTIVITY: Route<()> = Route::<()>::new("activity", "/activity");
const PROFILE: Route<()> = Route::<()>::new("profile", "/profile");

/// The app's sections in priority order: route name, label, icon.
const SECTIONS: &[(&str, &str, IconData)] = &[
    ("home", "Home", icons_lucide::HOUSE),
    ("activity", "Activity", icons_lucide::BELL),
    ("profile", "Profile", icons_lucide::USER),
];

/// Where the tab bar becomes a sidebar.
const WIDE: Breakpoint = Breakpoint::Lg;

fn is_wide() -> bool {
    current_breakpoint().get().is_at_least(WIDE)
}

stylesheet! {
    /// Sidebar (wide only) beside the content column.
    Frame<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            width: Length::pct(100.0),
            height: Length::pct(100.0),
            background: t.color.background(),
        }
    }
}

stylesheet! {
    /// The wide-screen sidebar column.
    Sidebar<IdeaThemeRef> {
        base(t) {
            width: Length::Px(240.0),
            flex_shrink: 0.0,
            height: Length::pct(100.0),
            background: t.color.surface(),
            border_right_width: 1.0,
            border_right_color: t.color.border(),
        }
    }
}

stylesheet! {
    /// The outlet over the (phone-only) tab bar.
    Content<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            flex_basis: Length::Px(0.0),
            min_width: Length::Px(0.0),
            min_height: Length::Px(0.0),
        }
    }
}

stylesheet! {
    /// The bottom bar: a surface band with a top hairline.
    TabBarSlot<IdeaThemeRef> {
        base(t) {
            flex_shrink: 0.0,
            background: t.color.surface(),
            border_top_width: 1.0,
            border_top_color: t.color.border(),
        }
    }
}

pub fn app() -> Element {
    idea_ui::install_idea_theme(idea_ui::light_theme());
    let nav: Ref<SwapHandle> = Ref::new();
    let builder = SwapNavigator::new(&HOME)
        .screen(HOME, |_| Screen::new(ui! { SectionPage(title = "Good morning", subtitle = "Here's what's happening today.") }))
        .screen(ACTIVITY, |_| Screen::new(ui! { SectionPage(title = "Activity", subtitle = "Nothing new since yesterday.") }))
        .screen(PROFILE, |_| Screen::new(ui! { SectionPage(title = "Profile", subtitle = "Your account and preferences.") }))
        .layout(|nav| {
            let active = nav.active_route;
            // Each reactive branch below captures its own handle.
            let (side_select, tab_select) = (nav.on_select.clone(), nav.on_select.clone());
            let outlet = nav.outlet;
            let items: Vec<TabItem> = SECTIONS.iter().map(|(route, label, _)| TabItem::new(route, *label)).collect();
            ui! {
                view(style = Frame()) {
                    if is_wide() {
                        view(style = Sidebar(), safe_area = SafeAreaSides::TOP) {
                            SidebarNav(active = active, select = side_select.clone())
                        }
                    }
                    view(style = Content()) {
                        outlet
                        if !is_wide() {
                            view(style = TabBarSlot(), safe_area = SafeAreaSides::BOTTOM) {
                                TabBar(
                                    items = items.clone(),
                                    active_route = active,
                                    on_select = tab_select.clone(),
                                    indicator = TabIndicator::Underline,
                                    fill = true,
                                )
                            }
                        }
                    }
                }
            }
        });
    ui! { builder.bind(nav) }
}

/// The wide-screen form of the same section list.
#[runtime_core::component]
fn SidebarNav(
    #[prop(default = runtime_core::signal("home"))] active: Signal<&'static str>,
    #[prop(default = Rc::new(|_: &'static str| {}) as Rc<dyn Fn(&'static str)>)] select: Rc<dyn Fn(&'static str)>,
) -> Element {
    ui! {
        Stack(gap = StackGap::Lg, padding = StackPadding::Md) {
            Typography(content = "Acme", kind = typography_kind::H3)
            Stack(gap = StackGap::Xs) {
                for (route, label, glyph) in SECTIONS.iter().copied() {
                    ListItem(
                        label = label.to_string(),
                        leading = Some(ui! { Icon(data = glyph, size = 18.0) }),
                        active = rx!(active.get() == route),
                        on_press = Some({
                            let select = select.clone();
                            Rc::new(move || select(route)) as Rc<dyn Fn()>
                        }),
                    )
                }
            }
        }
    }
}

/// Stand-in section body. Real sections follow the `page_layout` recipe.
#[runtime_core::component]
fn SectionPage(title: String, subtitle: String) -> Element {
    ui! {
        scroll_view {
            Stack(gap = StackGap::Xs, padding = StackPadding::Lg) {
                Typography(content = title, kind = typography_kind::H1)
                Typography(content = subtitle, muted = true)
            }
        }
    }
}
