//! `nav-showcase` — a kitchen-sink of nested navigators on the 0.2 model.
//!
//! Structure (navigators nested in each other):
//!
//! ```text
//! Drawer (swap + idea-ui-nav Drawer)
//! ├─ "Home"     → Tabs (swap + TabBar)
//! │              ├─ "Feed"  → Stack (list → detail)   ← Drawer→Tab→Stack, 3 deep
//! │              ├─ "Alerts"
//! │              └─ "Profile"
//! ├─ "Wizard"   → Stepper (swap + Next/Back + progress) ← an obscure swap variant
//! └─ "Settings" → Stack (settings → about)
//! ```
//!
//! Every chrome type is author layout wrapping an outlet: the drawer panel, the
//! tab bar, the stack headers, and the wizard's step chrome are all just views.

// idea-lite core migration (P6 SDK retarget): under `new-core` this alias
// shadows the extern-prelude `runtime-core` for the WHOLE crate, so the
// same source compiles against `runtime_vocabulary::glue`'s mirrors of
// the old author surface. The default build has no alias and is
// byte-identical old-core (the idea-ui-nav pattern).
#[cfg(feature = "new-core")]
extern crate runtime_facade as runtime_core;

use idea_ui::{install_idea_theme, light_theme, Typography};
use idea_ui_nav::{Drawer, StackHeader, TabBar, TabItem};
// `SwapContext`/`HeaderButton` come from the SDK preludes — the
// same-source import home on both cores (the new-core SDKs define their
// own `SwapContext`; glue deliberately doesn't mirror it).
use runtime_core::{
    component, pressable, rx, signal, text, ui, AlignItems, Element, FlexDirection, Length, Ref,
    Route, Screen, Signal, StyleApplication, StyleRules, StyleSheet,
};
use stack_navigator::prelude::HeaderButton;
use stack_navigator::{header_state, StackBuilder, StackHandle, StackNavigator, StackScreenExt};
use std::rc::Rc;
use swap_navigator::prelude::SwapContext;
use swap_navigator::{SwapBuilder, SwapHandle, SwapNavigator};

/// Navigators self-register via `inventory` (force-linked, so it works in dev +
/// release). Hook kept for the CLI bootstrap.
#[cfg(not(feature = "new-core"))]
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

/// New-core builds boot via the backends' `newcore::start`, where
/// `register_builtins` covers the navigators — the hook keeps its shape
/// without the old-core `Backend` bound (which has no glue mirror).
#[cfg(feature = "new-core")]
pub fn register_extensions<B>(_backend: &mut B) {}

/// Runtime-server (sidecar) recorder registrations — the CLI-generated
/// sidecar wrapper calls this so the outlet-model navigators run
/// host-side in `idealyst dev` non-local mode (their screen swaps ship
/// as plain node ops; see the SDKs' `recording` modules).
#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(backend: &mut dev_server::WireRecordingBackend) {
    swap_navigator::recording::register(backend);
    stack_navigator::recording::register(backend);
}

// ---------------------------------------------------------------------------
// Root: a Drawer over three sections.
// ---------------------------------------------------------------------------

const D_HOME: Route<()> = Route::<()>::new("home", "/");
const D_WIZARD: Route<()> = Route::<()>::new("wizard", "/wizard");
const D_SETTINGS: Route<()> = Route::<()>::new("settings", "/settings");

#[component]
pub fn app() -> Element {
    install_idea_theme(light_theme());

    let drawer_open = signal(false);
    let nav: Ref<SwapHandle> = Ref::new();

    let builder = SwapNavigator::new(&D_HOME)
        .screen(D_HOME, |_| Screen::new(tabs_section()))
        .screen(D_WIZARD, |_| Screen::new(wizard_section()))
        .screen(D_SETTINGS, |_| Screen::new(settings_stack()))
        .layout(move |nav: SwapContext| {
            let sidebar = drawer_sidebar(nav.on_select.clone(), drawer_open, nav.active_route);
            let content: Element = ui! {
                view(style = fill_col) {
                    { top_bar("Nav Showcase", drawer_open) }
                    view(style = grow) { { nav.outlet } }
                }
            };
            ui! {
                Drawer(sidebar = sidebar, is_open = drawer_open, width = 260.0) {
                    { content }
                }
            }
        });

    ui! { builder.bind(nav) }
}

/// A persistent top bar with a hamburger that opens the drawer.
fn top_bar(title: &str, drawer_open: Signal<bool>) -> Element {
    let open = move || drawer_open.set(true);
    ui! {
        view(style = bar_style) {
            { pressable(vec![text("\u{2630}").into()], open).with_style(slot_style) } // ☰
            { text(title.to_string()) }
        }
    }
}

/// The drawer panel content: section links that select + close the drawer.
fn drawer_sidebar(
    on_select: Rc<dyn Fn(&'static str)>,
    drawer_open: Signal<bool>,
    active: Signal<&'static str>,
) -> Vec<Element> {
    let link = |label: &str, route: &'static str| -> Element {
        let on_select = on_select.clone();
        let go = move || {
            on_select(route);
            drawer_open.set(false);
        };
        let label = label.to_string();
        let style = move || {
            let on = active.get() == route;
            sidebar_item_style(on)
        };
        pressable(vec![text(label).into()], go).with_style(style).into()
    };
    vec![
        ui! { Typography(content = "Sections".to_string(), kind = idea_ui::typography_kind::H3) },
        link("Home (Tabs)", "home"),
        link("Wizard", "wizard"),
        link("Settings", "settings"),
    ]
}

// ---------------------------------------------------------------------------
// "Home" section: a Tab navigator; the Feed tab hosts a Stack.
// ---------------------------------------------------------------------------

const T_FEED: Route<()> = Route::<()>::new("feed", "/");
const T_ALERTS: Route<()> = Route::<()>::new("alerts", "/alerts");
const T_PROFILE: Route<()> = Route::<()>::new("profile", "/profile");

fn tabs_section() -> Element {
    let nav: Ref<SwapHandle> = Ref::new();
    let builder = SwapNavigator::new(&T_FEED)
        .screen(T_FEED, |_| Screen::new(feed_stack()))
        .screen(T_ALERTS, |_| Screen::new(simple_page("Alerts", "No new alerts.")))
        .screen(T_PROFILE, |_| Screen::new(simple_page("Profile", "Your profile tab.")))
        .layout(|nav: SwapContext| {
            ui! {
                view(style = fill_col) {
                    view(style = grow) { { nav.outlet } }
                    TabBar(
                        items = vec![
                            TabItem::new("feed", "Feed"),
                            TabItem::new("alerts", "Alerts"),
                            TabItem::new("profile", "Profile"),
                        ],
                        active_route = nav.active_route,
                        on_select = nav.on_select,
                    )
                }
            }
        });
    ui! { builder.bind(nav) }
}

// ---------------------------------------------------------------------------
// Feed: a Stack (list → detail), nested inside the Feed tab.
// ---------------------------------------------------------------------------

const F_LIST: Route<()> = Route::<()>::new("feedlist", "/");
const F_ITEM: Route<()> = Route::<()>::new("feeditem", "/item");

fn feed_stack() -> Element {
    let nav: Ref<StackHandle> = Ref::new();
    // Shared selection: the list sets it, the detail reads it (both live in this
    // stack's builder scope).
    let selected = signal(0usize);

    let builder = StackNavigator::new(&F_LIST)
        .screen(F_LIST, {
            let nav = nav.clone();
            move |_| Screen::new(feed_list(nav.clone(), selected)).title("Feed")
        })
        .screen(F_ITEM, move |_| {
            let s = selected;
            // Title captured at push time (`selected` was set just before push).
            Screen::new(feed_item(s)).title(format!("Item {}", s.get()))
        })
        .layout(|nav| stack_layout(nav));
    ui! { builder.bind(nav) }
}

fn feed_list(nav: Ref<StackHandle>, selected: Signal<usize>) -> Element {
    ui! {
        view(style = page) {
            Typography(content = "Feed".to_string(), kind = idea_ui::typography_kind::H1)
            for i in 1..=4usize {
                button(label = format!("Open item {}", i), on_click = {
                    let nav = nav.clone();
                    move || {
                        selected.set(i);
                        nav.get().map(|h| h.push(&F_ITEM, ())).unwrap_or_default();
                    }
                })
            }
        }
    }
}

fn feed_item(selected: Signal<usize>) -> Element {
    ui! {
        view(style = page) {
            { text(move || format!("Item {}", selected.get())) }
            Typography(content = "Pushed onto the Feed stack. Back (native/header) pops it.".to_string(), muted = true)
        }
    }
}

// ---------------------------------------------------------------------------
// "Wizard" section: a Stepper — a swap navigator with Back/Next + progress.
// ---------------------------------------------------------------------------

const W_1: Route<()> = Route::<()>::new("step1", "/");
const W_2: Route<()> = Route::<()>::new("step2", "/step2");
const W_3: Route<()> = Route::<()>::new("step3", "/step3");
const WIZARD_ORDER: [&str; 3] = ["step1", "step2", "step3"];

fn wizard_section() -> Element {
    let nav: Ref<SwapHandle> = Ref::new();
    let builder = SwapNavigator::new(&W_1)
        .screen(W_1, |_| Screen::new(simple_page("Step 1", "Account details.")))
        .screen(W_2, |_| Screen::new(simple_page("Step 2", "Preferences.")))
        .screen(W_3, |_| Screen::new(simple_page("Step 3", "Review & finish.")))
        .layout(|nav: SwapContext| {
            let active = nav.active_route;
            let on_select = nav.on_select.clone();

            // index of the active step
            let idx = move || WIZARD_ORDER.iter().position(|r| *r == active.get()).unwrap_or(0);

            let back = {
                let on_select = on_select.clone();
                move || {
                    let i = idx();
                    if i > 0 {
                        on_select(WIZARD_ORDER[i - 1]);
                    }
                }
            };
            let next = {
                let on_select = on_select.clone();
                move || {
                    let i = idx();
                    if i + 1 < WIZARD_ORDER.len() {
                        on_select(WIZARD_ORDER[i + 1]);
                    }
                }
            };
            ui! {
                view(style = fill_col) {
                    view(style = bar_style) {
                        { text(move || format!("Step {} of {}", idx() + 1, WIZARD_ORDER.len())) }
                    }
                    view(style = grow) { { nav.outlet } }
                    view(style = bar_style) {
                        button(label = "Back".to_string(), on_click = back)
                        button(label = "Next".to_string(), on_click = next)
                    }
                }
            }
        });
    ui! { builder.bind(nav) }
}

// ---------------------------------------------------------------------------
// "Settings" section: a standalone Stack.
// ---------------------------------------------------------------------------

const S_HOME: Route<()> = Route::<()>::new("settingshome", "/");
const S_ABOUT: Route<()> = Route::<()>::new("about", "/about");

fn settings_stack() -> Element {
    let nav: Ref<StackHandle> = Ref::new();
    let builder = StackNavigator::new(&S_HOME)
        .screen(S_HOME, {
            let nav = nav.clone();
            move |_| {
                let nav = nav.clone();
                let go = move || {
                    nav.get().map(|h| h.push(&S_ABOUT, ())).unwrap_or_default();
                };
                let body = ui! {
                    view(style = page) {
                        Typography(content = "Settings".to_string(), kind = idea_ui::typography_kind::H1)
                        button(label = "About".to_string(), on_click = go)
                    }
                };
                Screen::new(body).title("Settings")
            }
        })
        .screen(S_ABOUT, |_| {
            Screen::new(simple_page("About", "Nav Showcase — every navigator, nested."))
                .title("About")
                .header_right(HeaderButton::text("Docs").on_press(|| {}))
        })
        .layout(|nav| stack_layout(nav));
    ui! { builder.bind(nav) }
}

// ---------------------------------------------------------------------------
// Shared bits.
// ---------------------------------------------------------------------------

/// The standard stack layout: a StackHeader over the outlet.
fn stack_layout(nav: stack_navigator::StackContext) -> Element {
    let screen_chrome = nav.screen_chrome;
    let state = rx!(header_state(&screen_chrome));
    ui! {
        view(style = fill_col) {
            StackHeader(
                state = state,
                show_back = nav.can_go_back,
                on_back = Some(nav.pop.clone()),
            )
            view(style = grow) { { nav.outlet } }
        }
    }
}

fn simple_page(title: &str, body: &str) -> Element {
    ui! {
        view(style = page) {
            Typography(content = title.to_string(), kind = idea_ui::typography_kind::H1)
            Typography(content = body.to_string(), muted = true)
        }
    }
}

// ---------------------------------------------------------------------------
// Inline styles (demo-local).
// ---------------------------------------------------------------------------

// Shared empty base sheet via `cached_stylesheet` — a per-file `thread_local!`
// burns an Android/bionic pthread TLS key per sheet (capped ~128).
fn base() -> Rc<StyleSheet> {
    static KEY: u8 = 0;
    runtime_core::cached_stylesheet(&KEY as *const u8 as usize, || {
        Rc::new(StyleSheet::r#static(StyleRules::default()))
    })
}
fn sheet(key: &'static str, rules: impl Fn() -> StyleRules + 'static) -> StyleApplication {
    StyleApplication::new(base()).with_computed(key, rules)
}
fn fill_col() -> StyleApplication {
    sheet("ns_fill_col", || StyleRules {
        flex_direction: Some(FlexDirection::Column),
        width: Some(Length::Percent(100.0).into()),
        height: Some(Length::Percent(100.0).into()),
        ..Default::default()
    })
}
fn grow() -> StyleApplication {
    sheet("ns_grow", || StyleRules { flex_grow: Some(1.0.into()), ..Default::default() })
}
fn page() -> StyleApplication {
    sheet("ns_page", || StyleRules {
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::FlexStart),
        gap: Some(Length::Px(10.0).into()),
        padding_top: Some(Length::Px(20.0).into()),
        padding_left: Some(Length::Px(20.0).into()),
        padding_right: Some(Length::Px(20.0).into()),
        ..Default::default()
    })
}
fn bar_style() -> StyleApplication {
    sheet("ns_bar", || StyleRules {
        flex_direction: Some(FlexDirection::Row),
        align_items: Some(AlignItems::Center),
        gap: Some(Length::Px(10.0).into()),
        padding_top: Some(Length::Px(10.0).into()),
        padding_bottom: Some(Length::Px(10.0).into()),
        padding_left: Some(Length::Px(14.0).into()),
        padding_right: Some(Length::Px(14.0).into()),
        ..Default::default()
    })
}
fn slot_style() -> StyleApplication {
    sheet("ns_slot", || StyleRules {
        padding_left: Some(Length::Px(4.0).into()),
        padding_right: Some(Length::Px(8.0).into()),
        ..Default::default()
    })
}
fn sidebar_item_style(active: bool) -> StyleApplication {
    let key = if active { "ns_side_on" } else { "ns_side_off" };
    sheet(key, move || StyleRules {
        padding_top: Some(Length::Px(10.0).into()),
        padding_bottom: Some(Length::Px(10.0).into()),
        padding_left: Some(Length::Px(6.0).into()),
        ..Default::default()
    })
}
