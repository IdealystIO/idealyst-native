//! Navigation — Breadcrumbs, Tabs, Pagination, Menu, List.
//!
//! Each `pub fn` returns the page **body only** — a column of demo
//! `Section`s. The central page frame renders the title, lead, group
//! overline, status badge, and the `Usage` panel, so these bodies never
//! render their own title/lead/scroll wrapper.

use std::rc::Rc;

use runtime_core::primitives::portal::AnchorTarget;
use runtime_core::{signal, ui, Element, IntoElement, PressableHandle, Ref};
use icons_lucide::{CHEVRON_RIGHT, COPY, PENCIL, SHARE_2, TRASH_2};
use idea_ui::{
    tone, typography_kind, variant, Avatar, AvatarColor, AvatarSize, Breadcrumbs, Button, Crumb, Icon, List,
    ListItem, Menu, MenuEntry, MenuItem, MenuLabel, MenuSeparator, Pagination, Stack, StackAxis,
    StackGap, SubMenu, Tab, TabIndicator, Tabs, Typography,
};

use crate::pages::body;
use crate::shell::{Callout, CodePanel, DemoSurface, Prop, PropsTable, Section, H3, P};

/// Icon size for menu/list leading glyphs.
const ROW_ICON_PX: f32 = 18.0;
/// Trailing chevron glyph size on list rows.
const CHEVRON_PX: f32 = 16.0;

// =============================================================================
// Breadcrumbs
// =============================================================================

pub fn breadcrumbs() -> Element {
    let noop: Rc<dyn Fn()> = Rc::new(|| {});

    body(vec![
        ui! {
            Section(title = "Default".to_string()) {
                P(content = "A root-first trail. Earlier crumbs built with `Crumb::linked` \
                    are clickable; the last crumb renders as the current page (emphasized, \
                    not clickable).".to_string())
                DemoSurface {
                    Breadcrumbs(items = vec![
                        Crumb::linked("Home", noop.clone()),
                        Crumb::linked("Library", noop.clone()),
                        Crumb::new("Settings"),
                    ])
                }
                CodePanel(src = r##"Breadcrumbs(items = vec![
    Crumb::linked("Home", go_home),
    Crumb::linked("Library", go_library),
    Crumb::new("Settings"), // current page — not linked
])"##.to_string())
            }
        },
        ui! {
            Section(title = "Overflow".to_string()) {
                P(content = "A deep trail wraps within its row; the separator glyph is \
                    configurable via the `separator` prop.".to_string())
                DemoSurface {
                    Breadcrumbs(
                        separator = "›".to_string(),
                        items = vec![
                            Crumb::linked("Workspace", noop.clone()),
                            Crumb::linked("Projects", noop.clone()),
                            Crumb::linked("Idealyst", noop.clone()),
                            Crumb::linked("crates", noop.clone()),
                            Crumb::linked("idea-ui", noop.clone()),
                            Crumb::new("breadcrumbs.rs"),
                        ],
                    )
                }
            }
        },
        ui! {
            Section(title = "Icon separator".to_string()) {
                P(content = "Pass `separator_icon` to draw an icon between crumbs instead of \
                    a text glyph — both forms are supported (the glyph is the fallback when \
                    no icon is set). Linked crumbs show a pointer cursor on hover.".to_string())
                DemoSurface {
                    Breadcrumbs(
                        separator_icon = Some(CHEVRON_RIGHT),
                        items = vec![
                            Crumb::linked("Home", noop.clone()),
                            Crumb::linked("Components", noop.clone()),
                            Crumb::new("Breadcrumbs"),
                        ],
                    )
                }
                CodePanel(src = r##"Breadcrumbs(
    separator_icon = Some(icons_lucide::CHEVRON_RIGHT),
    items = vec![
        Crumb::linked("Home", go_home),
        Crumb::linked("Components", go_components),
        Crumb::new("Breadcrumbs"),
    ],
)"##.to_string())
            }
        },
        ui! {
            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "items",          ty: "Vec<Crumb>",       desc: "Trail in order. Crumb::new(label) is plain; Crumb::linked(label, on_press) is clickable. The last crumb always renders as the current page." },
                    Prop { name: "separator",      ty: "String",           desc: "Glyph drawn between crumbs. Default: \"/\". Ignored when separator_icon is set." },
                    Prop { name: "separator_icon", ty: "Option<IconData>", desc: "Icon drawn between crumbs instead of the glyph. None falls back to separator." },
                ])
            }
        },
    ])
}

// =============================================================================
// Tabs
// =============================================================================

pub fn tabs() -> Element {
    body(vec![tabs_underline_section(), tabs_pill_section(), tabs_props_section()])
}

fn tabs_underline_section() -> Element {
    let active = signal!("overview".to_string());
    let on_change: Rc<dyn Fn(String)> = Rc::new(move |id| active.set(id));

    // The strip owns the active id; the caller wires the panel swap via
    // `runtime_core::switch` keyed on that same id.
    let panel = runtime_core::switch(
        move || active.get(),
        |id: &String| match id.as_str() {
            "overview" => ui! {
                Stack(gap = StackGap::Sm) {
                    H3(content = "Overview".to_string())
                    P(content = "The Overview panel is mounted whenever its id is active; \
                        switching tabs disposes this subtree and mounts a fresh one for the \
                        newly-active panel.".to_string())
                }
            },
            "activity" => ui! {
                Stack(gap = StackGap::Sm) {
                    H3(content = "Activity".to_string())
                    P(content = "Because the panel is rebuilt on every tab change, signal \
                        subscriptions inside it release when the user switches away — no \
                        stale effects accumulate.".to_string())
                }
            },
            _ => ui! {
                Stack(gap = StackGap::Sm) {
                    H3(content = "Settings".to_string())
                    P(content = "The strip doesn't dictate panel layout — each branch returns \
                        whatever primitive tree makes sense for that view.".to_string())
                }
            },
        },
    );

    ui! {
        Section(title = "Underline".to_string()) {
            P(content = "The default indicator — a 2px accent underline beneath the active \
                tab. Bind `active` to a Signal and wire `on_change` to set it.".to_string())
            DemoSurface {
                Tabs(
                    active = active,
                    on_change = on_change,
                    tabs = signal!(vec![
                        Tab::new("overview", "Overview"),
                        Tab::new("activity", "Activity"),
                        Tab::new("settings", "Settings"),
                    ]),
                )
                panel
            }
            CodePanel(src = r##"let active = signal!("overview".to_string());
let on_change: Rc<dyn Fn(String)> = Rc::new(move |id| active.set(id));

let panel = runtime_core::switch(
    move || active.get(),
    |id: &String| match id.as_str() {
        "overview" => ui! { /* Overview content */ },
        "activity" => ui! { /* Activity content */ },
        _ => ui! { /* Settings content */ },
    },
);

ui! {
    Tabs(
        active = active,                  // active tab's id (Reactive<String>)
        on_change = on_change,            // Fn(String) — the tapped id
        tabs = signal!(vec![              // reactive, id-keyed list
            Tab::new("overview", "Overview"),
            Tab::new("activity", "Activity"),
            Tab::new("settings", "Settings"),
        ]),
    )
    panel
}"##.to_string())
        }
    }
}

fn tabs_pill_section() -> Element {
    let active = signal!("list".to_string());
    let on_change: Rc<dyn Fn(String)> = Rc::new(move |id| active.set(id));

    ui! {
        Section(title = "Pill".to_string()) {
            P(content = "`TabIndicator::Dot` gives the compact switcher look — a leading \
                colored dot plus a chip background on the active tab.".to_string())
            DemoSurface {
                Tabs(
                    indicator = TabIndicator::Dot,
                    active = active,
                    on_change = on_change,
                    tabs = signal!(vec![
                        Tab::new("list", "List"),
                        Tab::new("grid", "Grid"),
                        Tab::new("cards", "Cards"),
                    ]),
                )
            }
        }
    }
}

fn tabs_props_section() -> Element {
    ui! {
        Section(title = "Props".to_string()) {
            PropsTable(rows = vec![
                Prop { name: "tabs",      ty: "Signal<Vec<Tab>>",   desc: "Reactive, id-keyed list. Tab::new(id, label); tabs reconcile by id so a surviving tab keeps its state. Wrap a fixed set in signal!." },
                Prop { name: "active",    ty: "Reactive<String>",   desc: "The active tab's id. Drives the strip highlight and is the caller's source of truth for panel swap." },
                Prop { name: "on_change", ty: "Rc<dyn Fn(String)>", desc: "Fires when the user taps a tab; receives the tapped tab's id." },
                Prop { name: "indicator", ty: "TabIndicator",       desc: "Underline (default) or Dot (compact dot + chip pill)." },
            ])
            Callout(label = "Tabs vs Segmented Control".to_string()) {
                P(content = "Use Tabs when each option opens a distinct content surface. For \
                    flipping a single value across three or four options, reach for a \
                    Segmented Control instead.".to_string())
            }
        }
    }
}

// =============================================================================
// Pagination
// =============================================================================

pub fn pagination() -> Element {
    let page = signal!(3usize);
    let on_page: Rc<dyn Fn(usize)> = Rc::new(move |p| page.set(p));

    body(vec![
        ui! {
            Section(title = "Windowed".to_string()) {
                P(content = "Controlled — `page` (1-based) is the source of truth; the chevrons \
                    and page buttons fire `on_change` with the requested page. For a large \
                    `total` the middle collapses to ellipses around the current page, with the \
                    first and last always shown.".to_string())
                DemoSurface {
                    Pagination(page = page, total = 20usize, on_change = on_page)
                }
                CodePanel(src = r##"let page = signal!(1usize);
ui! {
    Pagination(
        page = page,                       // Signal<usize>, 1-based
        total = 20usize,
        on_change = move |p: usize| page.set(p),
    )
}"##.to_string())
            }
        },
        ui! {
            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "page",      ty: "Signal<usize>",     desc: "Current page, 1-based. The host owns the signal." },
                    Prop { name: "total",     ty: "usize",             desc: "Total number of pages (>= 1)." },
                    Prop { name: "on_change", ty: "Rc<dyn Fn(usize)>", desc: "Fires with the requested page when the user navigates." },
                ])
            }
        },
    ])
}

// =============================================================================
// Menu
// =============================================================================

pub fn menu() -> Element {
    let open = signal!(false);
    let trigger: Ref<PressableHandle> = Ref::new();
    let open_menu: Rc<dyn Fn()> = Rc::new(move || open.set(true));
    let close: Rc<dyn Fn()> = Rc::new(move || open.set(false));
    let dismiss: Rc<dyn Fn()> = close.clone();

    let folders = vec![
        MenuEntry::new("Inbox", close.clone()),
        MenuEntry::new("Archive", close.clone()),
        MenuEntry::new("Spam", close.clone()),
    ];

    body(vec![
        ui! {
            Section(title = "Anchored menu".to_string()) {
                P(content = "An anchored command surface composed from MenuItem / MenuLabel / \
                    MenuSeparator / SubMenu. The host owns an open-state signal and gates \
                    mounting — the same shape as Popover. Items carry leading icons, trailing \
                    keyboard shortcuts, and a destructive row.".to_string())
                DemoSurface {
                    Button(
                        label = "Actions".to_string(),
                        on_click = open_menu,
                        tone = tone::Neutral,
                        variant = variant::Soft,
                        bind_to = Some(trigger),
                    )
                    if open.get() {
                        Menu(target = Some(AnchorTarget::from(trigger)), on_dismiss = Some(dismiss.clone())) {
                            MenuLabel(text = "Edit")
                            MenuItem(
                                label = "Rename",
                                on_select = close.clone(),
                                leading = Some(menu_icon(PENCIL)),
                                trailing = Some(shortcut("⌘R")),
                            )
                            MenuItem(
                                label = "Duplicate",
                                on_select = close.clone(),
                                leading = Some(menu_icon(COPY)),
                                trailing = Some(shortcut("⌘D")),
                            )
                            MenuSeparator()
                            MenuLabel(text = "Organize")
                            SubMenu(label = "Move to…", items = folders.clone())
                            MenuItem(
                                label = "Share",
                                on_select = close.clone(),
                                leading = Some(menu_icon(SHARE_2)),
                            )
                            MenuSeparator()
                            MenuItem(
                                label = "Delete",
                                on_select = close.clone(),
                                leading = Some(menu_icon_tone(TRASH_2, tone::Danger.into())),
                                trailing = Some(shortcut("⌫")),
                            )
                        }
                    }
                }
                CodePanel(src = r##"let open = signal!(false);
let trigger: Ref<PressableHandle> = Ref::new();
let close: Rc<dyn Fn()> = Rc::new(move || open.set(false));

ui! {
    Button(label = "Actions".into(), on_click = move || open.set(true), bind_to = Some(trigger))
    if open.get() {
        Menu(target = Some(AnchorTarget::from(trigger)), on_dismiss = Some(close.clone())) {
            MenuLabel(text = "Edit")
            MenuItem(label = "Rename", on_select = close.clone(), trailing = Some(shortcut))
            MenuSeparator()
            SubMenu(label = "Move to…", items = vec![
                MenuEntry::new("Inbox", close.clone()),
                MenuEntry::new("Archive", close.clone()),
            ])
            MenuItem(label = "Delete", on_select = close.clone()) // destructive
        }
    }
}"##.to_string())
            }
        },
        ui! {
            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "target",     ty: "Option<AnchorTarget>", desc: "Menu: element to anchor against — AnchorTarget::from(some_ref). Required." },
                    Prop { name: "on_dismiss", ty: "Option<Rc<dyn Fn()>>", desc: "Menu: fires on click-outside / Escape; flip your open-state signal here." },
                    Prop { name: "children",   ty: "Vec<Element>",         desc: "Menu: rows — MenuItem / MenuLabel / MenuSeparator / SubMenu." },
                    Prop { name: "label",      ty: "Reactive<String>",     desc: "MenuItem: row label. Static or live." },
                    Prop { name: "on_select",  ty: "Rc<dyn Fn()>",         desc: "MenuItem: fires when the row is chosen. Typically also closes the menu." },
                    Prop { name: "leading",    ty: "Option<Element>",      desc: "MenuItem: optional leading element (icon, avatar)." },
                    Prop { name: "trailing",   ty: "Option<Element>",      desc: "MenuItem: optional trailing element (shortcut hint, badge), pushed right." },
                    Prop { name: "items",      ty: "Vec<MenuEntry>",       desc: "SubMenu: flyout contents as reconstructable data. MenuEntry::new(label, on_select)." },
                ])
            }
        },
    ])
}

/// A muted menu/list leading icon.
fn menu_icon(data: runtime_core::IconData) -> Element {
    ui! { Icon(data = data, size = ROW_ICON_PX) }
}

/// A toned menu/list leading icon (e.g. the destructive Delete row).
fn menu_icon_tone(data: runtime_core::IconData, t: idea_ui::ToneRef) -> Element {
    ui! { Icon(data = data, size = ROW_ICON_PX, tone = Some(t)) }
}

/// A muted right-pushed keyboard-shortcut hint.
fn shortcut(text: &str) -> Element {
    let text = text.to_string();
    ui! { Typography(content = text, muted = true) }
}

// =============================================================================
// List
// =============================================================================

pub fn list() -> Element {
    let noop: Rc<dyn Fn()> = Rc::new(|| {});

    body(vec![
        ui! {
            Section(title = "Rows".to_string()) {
                P(content = "`List` draws the bordered surface and a hairline divider between \
                    consecutive rows. Each `ListItem` is a row with optional leading and \
                    trailing content; passing `on_press` makes the whole row clickable with a \
                    hover highlight.".to_string())
                DemoSurface {
                    List {
                        person_row("Ada Lovelace", "ada@idealyst.dev", "AL", AvatarColor::Primary, noop.clone())
                        person_row("Grace Hopper", "grace@idealyst.dev", "GH", AvatarColor::Success, noop.clone())
                        person_row("Alan Turing", "alan@idealyst.dev", "AT", AvatarColor::Warning, noop.clone())
                    }
                }
                CodePanel(src = r##"List {
    ListItem(
        label = "Ada Lovelace",
        leading = Some(ui! { Avatar(initials = "AL", color = AvatarColor::Primary) }),
        trailing = Some(ui! { Icon(data = icons_lucide::CHEVRON_RIGHT, size = 16.0) }),
        on_press = Some(on_open),
    )
    // …more rows; List inserts the dividers
}"##.to_string())
            }
        },
        ui! {
            Section(title = "Props".to_string()) {
                P(content = "`List` is the container; each row is a `ListItem`, clickable only \
                    when `on_press` is Some.".to_string())
                PropsTable(rows = vec![
                    Prop { name: "label",    ty: "Reactive<String>",     desc: "ListItem: row label. Static or live." },
                    Prop { name: "on_press", ty: "Option<Rc<dyn Fn()>>", desc: "ListItem: when Some, the row is clickable (hover highlight + handler)." },
                    Prop { name: "leading",  ty: "Option<Element>",      desc: "ListItem: optional leading element (icon, avatar)." },
                    Prop { name: "trailing", ty: "Option<Element>",      desc: "ListItem: optional trailing element (badge, chevron), pushed right." },
                    Prop { name: "active",   ty: "bool",                 desc: "ListItem: render the row in its highlighted/selected state." },
                ])
            }
        },
    ])
}

/// One contact row: leading avatar (initials) + title/subtitle stack +
/// trailing chevron, the whole row tappable. The `ListItem` label slot
/// holds a two-line title/subtitle stack so the row reads as a contact.
fn person_row(
    name: &str,
    email: &str,
    initials: &str,
    color: AvatarColor,
    on_press: Rc<dyn Fn()>,
) -> Element {
    let avatar = ui! {
        Avatar(initials = initials.to_string(), color = color, size = AvatarSize::Md)
    };
    let title = name.to_string();
    let subtitle = email.to_string();
    let leading = ui! {
        Stack(axis = StackAxis::Row, gap = StackGap::Md) {
            avatar
            Stack(gap = StackGap::Xs) {
                Typography(content = title, kind = typography_kind::Body)
                Typography(content = subtitle, muted = true)
            }
        }
    };
    let chevron = ui! { Icon(data = icons_lucide::CHEVRON_RIGHT, size = CHEVRON_PX) };
    ui! {
        ListItem(
            label = "".to_string(),
            leading = Some(leading.into_element()),
            trailing = Some(chevron),
            on_press = Some(on_press),
        )
    }
}
