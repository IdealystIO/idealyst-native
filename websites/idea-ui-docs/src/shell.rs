//! The docs chrome: the design's custom header bar, the grouped +
//! searchable sidebar with status dots (both built ONCE inside the swap
//! navigator's `.layout(...)` closure — see `lib.rs`), the central
//! `page_frame` (group overline, title + status badge, lead, body, Usage
//! panel), and the per-page helper components every body is built from
//! (`CodePanel`, `PropsTable`, `DemoSurface`, `Demo`, `Callout`,
//! `Section`, `P`, `H2`, `H3`).
//!
//! Each helper is a real `#[component]` so it dispatches inside `ui!`.

use std::rc::Rc;

use runtime_core::{
    component, derived, effect, fragment, pressable, signal, switch, ui, when, Breakpoint, Color,
    Element, IntoElement, SafeAreaSides, Signal, StyleApplication, Tokenized,
};
use idea_ui::{
    dark_theme, light_theme, set_idea_theme, typography_kind, Icon, Modal, Spacer, Stack, StackGap,
    Switch, Typography,
};
use idea_ui::{Table, TableCell, TableRow};
use icons_lucide::SEARCH;

use runtime_core::Route;

use crate::routes::{Entry, Status, CATALOG};
use crate::styles::{
    BrandName, Callout as CalloutBox, CodePanel as CodePanelBox, CodeText, ControlsBox, DemoRow,
    DemoSurface as DemoSurfaceBox, DemoSurfaceContent, DocHeader, GroupOverline, HeaderBrand, HeaderMono, HeaderSpacer,
    LogoBox, LogoGlyph, MenuButton, MenuGlyph, NavDot, NavDotReady, NavItem, NavItemActive, PagePad, PreviewBox,
    PreviewSlot, ScreenScroll, SearchDialogBody, SearchFieldRow, SearchInputBare, SearchResultsScroll,
    SearchTrigger, SearchTriggerText, SegBtn, SegBtnActive, SegBtnText, SegBtnTextActive,
    SegToggle, SidebarBody, SidebarScroll, SidebarSection, StatusBadge, StatusBadgeDetailed, StatusBadgeText,
    StatusBadgeTextDetailed, TitleRow, UsageLabel, VersionPill,
};

const VERSION: &str = "v0.1.0";

// =============================================================================
// Header — the custom top bar, built once by the `.layout(...)` closure
// and mounted above the outlet. Hamburger (narrow widths only) + logo +
// name + version, a "component reference" label, the active entry's
// token hint, and the Light/Dark segmented toggle.
// =============================================================================

pub fn header(
    active_route: Signal<&'static str>,
    is_dark: Signal<bool>,
    drawer_open: Signal<bool>,
) -> Element {
    // Token hint follows the active component. Rebuilt on navigation.
    let token_hint = switch(
        move || active_route.get(),
        move |route_name: &&'static str| {
            let token = crate::routes::entry_for(route_name).map(|e| e.token).unwrap_or("");
            ui! { text(style = HeaderMono()) { token.to_string() } }
        },
    );

    ui! {
        view(style = DocHeader()) {
            menu_button(drawer_open)
            view(style = HeaderBrand()) {
                view(style = LogoBox()) {
                    text(style = LogoGlyph()) { "i".to_string() }
                }
                text(style = BrandName()) { "idea-ui".to_string() }
                text(style = VersionPill()) { VERSION.to_string() }
            }
            text(style = HeaderMono()) { "component reference".to_string() }
            view(style = HeaderSpacer()) {}
            token_hint
            theme_toggle(is_dark)
        }
    }
}

// The leading hamburger. The AppShell's sidebar pins in-flow at/above
// the `Lg` breakpoint (900 px — see `install_breakpoints` in `lib.rs`)
// and becomes an off-canvas drawer below it, so the hamburger is only
// rendered while the sidebar is a drawer. `sidebar_pinned` reads the
// reactive breakpoint, so the `when` flips in lockstep with the
// AppShell's own pin/drawer switch. Pressing it opens the app-owned
// `drawer_open` signal the AppShell panel + scrim subscribe to.
fn menu_button(drawer_open: Signal<bool>) -> Element {
    when(
        move || !idea_ui_nav::sidebar_pinned(Breakpoint::Lg),
        move || {
            let glyph = ui! { text(style = MenuGlyph()) { "\u{2630}".to_string() } };
            pressable(vec![glyph], move || drawer_open.set(true))
                .with_style(MenuButton())
                .into_element()
        },
        || fragment(vec![]),
    )
}

fn theme_toggle(is_dark: Signal<bool>) -> Element {
    let set_light: Rc<dyn Fn()> = Rc::new(move || {
        is_dark.set(false);
        set_idea_theme(light_theme());
    });
    let set_dark: Rc<dyn Fn()> = Rc::new(move || {
        is_dark.set(true);
        set_idea_theme(dark_theme());
    });

    let light_btn = seg_button("Light", set_light, is_dark, true);
    let dark_btn = seg_button("Dark", set_dark, is_dark, false);

    ui! {
        view(style = SegToggle()) {
            light_btn
            dark_btn
        }
    }
}

fn seg_button(
    label: &'static str,
    on_press: Rc<dyn Fn()>,
    is_dark: Signal<bool>,
    is_light: bool,
) -> Element {
    let btn_style = SegBtn().active(derived(move || {
        let active = if is_light { !is_dark.get() } else { is_dark.get() };
        if active { SegBtnActive::On } else { SegBtnActive::Off }
    }));
    let text_style = SegBtnText().active(derived(move || {
        let active = if is_light { !is_dark.get() } else { is_dark.get() };
        if active { SegBtnTextActive::On } else { SegBtnTextActive::Off }
    }));
    let label_el = ui! { text(style = text_style) { label.to_string() } };
    runtime_core::pressable(vec![label_el], move || (on_press)())
        .with_style(btn_style)
        .into()
}

// =============================================================================
// Sidebar — search box + grouped nav with status dots. Built ONCE and
// handed to the AppShell's `sidebar` prop (one panel serves both the
// pinned column and the off-canvas drawer). Filtered reactively by the
// shared `q` signal; `active_route` is the navigator's reactive mirror.
// =============================================================================

pub fn sidebar(active_route: Signal<&'static str>, q: Signal<String>) -> Element {
    // The search now lives in a dialog. This open-state drives the modal; the
    // sidebar shows a button that opens it.
    let open: Signal<bool> = signal(false);

    // Search TRIGGER — a button styled like a search field. Opening clears any
    // prior query so each search starts fresh.
    let open_q = q;
    let open_set = open;
    let search_button: Element = pressable(
        vec![ui! { text(style = SearchTriggerText()) { "Search components…".to_string() } }],
        move || {
            open_q.set(String::new());
            open_set.set(true);
        },
    )
    .with_style(StyleApplication::new(SearchTrigger::sheet()))
    .into();

    // The full nav list (search/filtering moved into the dialog).
    let nav = build_nav("", active_route);

    let close: Rc<dyn Fn()> = {
        let o = open;
        Rc::new(move || o.set(false))
    };

    // A scroll view (not a plain view): the AppShell gives the sidebar a
    // fixed full-height panel and leaves scrolling to the author, so
    // a nav list taller than the viewport must scroll here. The
    // `scroll_view` seed (`flex_grow:1 / flex_basis:0`) fills the panel's
    // height, bounding the scroller so its content overflows and scrolls.
    //
    // The Modal is always-mounted (it animates its own exit via `presence`)
    // but renders into a screen-covering overlay, so it sits inertly in the
    // tree here and portals out when open.
    ui! {
        scroll_view(style = SidebarScroll()) {
            view(style = SidebarBody()) {
                search_button
                nav
                Modal(
                    open = open,
                    on_dismiss = Some(close),
                    content = move || search_dialog(q, active_route, open),
                )
            }
        }
        .safe_area(SafeAreaSides::VERTICAL)
    }
}

// =============================================================================
// Layout helpers for the swap `.layout(...)` closure in `lib.rs`:
// header-over-outlet column + growing outlet wrapper.
// =============================================================================

/// The header-over-outlet column filling the AppShell's content region.
pub fn shell_column_style() -> StyleApplication {
    static KEY: u8 = 0;
    let sheet = runtime_core::cached_stylesheet(&KEY as *const u8 as usize, || {
        Rc::new(runtime_core::StyleSheet::r#static(runtime_core::StyleRules {
            flex_direction: Some(runtime_core::FlexDirection::Column),
            width: Some(runtime_core::Length::Percent(100.0).into()),
            height: Some(runtime_core::Length::Percent(100.0).into()),
            min_height: Some(runtime_core::Length::Px(0.0).into()),
            ..Default::default()
        }))
    });
    StyleApplication::new(sheet)
}

/// The outlet's growing wrapper: absorbs the column's remaining height
/// so screens (whose own `scroll_view`s need a bounded box) fill below
/// the header.
pub fn outlet_grow_style() -> StyleApplication {
    static KEY: u8 = 0;
    let sheet = runtime_core::cached_stylesheet(&KEY as *const u8 as usize, || {
        Rc::new(runtime_core::StyleSheet::r#static(runtime_core::StyleRules {
            flex_direction: Some(runtime_core::FlexDirection::Column),
            flex_grow: Some(1.0f32.into()),
            min_height: Some(runtime_core::Length::Px(0.0).into()),
            ..Default::default()
        }))
    });
    StyleApplication::new(sheet)
}

/// The search dialog body: a live text input over the reactive result list.
/// Selecting a result navigates (the `link` in `nav_item`) AND closes the
/// dialog — we watch `active_route` and flip `open` off on the first change.
fn search_dialog(
    q: Signal<String>,
    active_route: Signal<&'static str>,
    open: Signal<bool>,
) -> Element {
    let on_q = q;
    let input = runtime_core::text_input(q, move |v: String| on_q.set(v))
        .placeholder("Search components…".to_string())
        .with_style(move || StyleApplication::new(SearchInputBare::sheet()))
        .into_element();

    let results = switch(
        move || q.get(),
        move |query: &String| build_search_results(query, active_route),
    );

    // Close-on-navigate. `content` is rebuilt per open, so this arms fresh each
    // time: the first run (mount) only arms; the next run — fired when a result
    // link changes `active_route` — closes the dialog.
    let armed = std::cell::Cell::new(false);
    effect!({
        let _ = active_route.get();
        if armed.get() {
            open.set(false);
        } else {
            armed.set(true);
        }
    });

    ui! {
        view(style = SearchDialogBody()) {
            view(style = SearchFieldRow()) {
                Icon(data = SEARCH, size = 16.0, color = Some(Color("#64748b".into())))
                input
            }
            scroll_view(style = SearchResultsScroll()) {
                results
            }
        }
    }
}

/// Flat, filtered result list for the search dialog — reuses `nav_item` so a
/// result is the same navigating link as the sidebar.
fn build_search_results(query: &str, active_route: Signal<&'static str>) -> Element {
    let q = query.trim().to_lowercase();
    let mut items: Vec<Element> = Vec::new();
    for group in CATALOG {
        for entry in group
            .entries
            .iter()
            .filter(|e| q.is_empty() || e.name.to_lowercase().contains(&q))
        {
            items.push(nav_item(entry, active_route));
        }
    }
    if items.is_empty() {
        items.push(ui! { text(style = SidebarSection()) { "No matches".to_string() } });
    }
    ui! { Stack(gap = StackGap::None) { items } }
}

fn build_nav(query: &str, active_route: Signal<&'static str>) -> Element {
    let q = query.trim().to_lowercase();
    let mut items: Vec<Element> = Vec::new();
    for group in CATALOG {
        let matches = group
            .entries
            .iter()
            .filter(|e| q.is_empty() || e.name.to_lowercase().contains(&q));
        let mut any = false;
        let mut group_items: Vec<Element> = Vec::new();
        for entry in matches {
            if !any {
                items.push(ui! { text(style = SidebarSection()) { group.label.to_string() } });
                any = true;
            }
            group_items.push(nav_item(entry, active_route));
        }
        items.extend(group_items);
    }
    ui! { Stack(gap = StackGap::None) { items } }
}

// =============================================================================
// route_link — the in-app route-jump wrapper shared by the sidebar nav
// items and the Overview landing CTAs. The framework
// `link(route = …, params = ())` primitive: the swap navigator provides
// the `LinkActivator` world context around every screen AND
// author-layout build, so these are real `<a href>` sidebar entries.
// =============================================================================

pub fn route_link(route: &'static Route<()>, child: Element) -> Element {
    ui! {
        link(route = route, params = ()) {
            child
        }
    }
}

fn nav_item(entry: &'static Entry, active_route: Signal<&'static str>) -> Element {
    let route = entry.route;
    let route_name = route.name();
    let name = entry.name.to_string();
    let ready = entry.status == Status::Detailed;

    let container = NavItem().active(derived(move || {
        if active_route.get() == route_name { NavItemActive::On } else { NavItemActive::Off }
    }));
    let dot = NavDot().ready(if ready { NavDotReady::On } else { NavDotReady::Off });

    let row: Element = ui! {
        view(style = container) {
            text(style = crate::styles::NavLinkText()) { name }
            view(style = dot) {}
        }
    };
    route_link(route, row)
}

// =============================================================================
// page_frame — the central frame applied to every screen. Pulls the
// group, title, status and Usage code from the catalog `Entry`; calls
// the entry's `body` for the demo sections.
// =============================================================================

pub fn page_frame(entry: &'static Entry) -> Element {
    let group = crate::routes::group_for(entry.route.name()).unwrap_or("");
    let detailed = entry.status == Status::Detailed;
    let status_label = if detailed { "Detailed" } else { "Preview" };

    let badge_style = StatusBadge()
        .detailed(if detailed { StatusBadgeDetailed::On } else { StatusBadgeDetailed::Off });
    let badge_text_style = StatusBadgeText().detailed(if detailed {
        StatusBadgeTextDetailed::On
    } else {
        StatusBadgeTextDetailed::Off
    });

    let body = (entry.body)();
    let lead = entry.desc.to_string();
    let title = entry.name.to_string();

    let usage: Element = if entry.code.is_empty() {
        ui! { view {} }
    } else {
        let code = entry.code.to_string();
        ui! {
            view {
                text(style = UsageLabel()) { "Usage".to_string() }
                CodePanel(src = code)
            }
        }
    };

    ui! {
        scroll_view(style = ScreenScroll()) {
            view(style = PagePad()) {
                text(style = GroupOverline()) { group.to_string() }
                view(style = TitleRow()) {
                    Typography(content = title, kind = typography_kind::H1)
                    view(style = badge_style) {
                        text(style = badge_text_style) { status_label.to_string() }
                    }
                }
                Typography(content = lead, kind = typography_kind::BodyLg, muted = true)
                body
                usage
            }
        }
        .safe_area(SafeAreaSides::BOTTOM)
    }
}

// =============================================================================
// landing_frame — the full-bleed frame for the Overview landing screen.
// Unlike `page_frame`, it adds NO title block / status badge / Usage
// panel; the page body (`pages::overview`) owns its whole layout inside
// the wide `LandingPad` column. Just the scrolling surface + safe area.
// =============================================================================

pub fn landing_frame(entry: &'static Entry) -> Element {
    let body = (entry.body)();
    ui! {
        scroll_view(style = ScreenScroll()) {
            body
        }
        .safe_area(SafeAreaSides::BOTTOM)
    }
}

// =============================================================================
// CodePanel — theme-aware syntax-highlighted code block.
// =============================================================================

#[derive(Default)]
pub struct CodePanelProps {
    pub src: String,
}

#[derive(Copy, Clone)]
struct Palette {
    ink: &'static str,
    comment: &'static str,
    string: &'static str,
    accent: &'static str,
}

const LIGHT_PALETTE: Palette = Palette {
    ink: "#1f2328",
    comment: "#8a8270",
    string: "#1f6e5f",
    accent: "#5a4fcf",
};

const DARK_PALETTE: Palette = Palette {
    ink: "#e8eaf0",
    comment: "#9099a8",
    string: "#5eead4",
    accent: "#c4b5fd",
};

fn theme_is_dark() -> bool {
    let bg: Color =
        Tokenized::<Color>::token("color-background", Color("#ffffff".into())).resolve();
    is_dark_color(&bg.0)
}

fn is_dark_color(s: &str) -> bool {
    let hex = s.trim_start_matches('#');
    if hex.len() < 6 {
        return false;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(255) as f32;
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(255) as f32;
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(255) as f32;
    let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    luma < 128.0
}

fn highlight(src: &str, palette: Palette) -> Vec<(String, Color)> {
    let keywords = [
        "fn", "let", "pub", "use", "mod", "struct", "enum", "impl", "trait", "for", "in", "if",
        "else", "match", "return", "move", "self", "Self", "async", "await", "true", "false",
        "as", "where", "type", "const", "static", "dyn",
    ];

    let mut out: Vec<(String, Color)> = Vec::new();
    let mut buf = String::new();
    let bytes = src.as_bytes();
    let mut i = 0;

    let flush_ident = |buf: &mut String, out: &mut Vec<(String, Color)>, palette: &Palette| {
        if buf.is_empty() {
            return;
        }
        let color = if keywords.contains(&buf.as_str()) {
            palette.accent
        } else {
            palette.ink
        };
        out.push((std::mem::take(buf), Color(color.into())));
    };

    while i < bytes.len() {
        let b = bytes[i];
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            flush_ident(&mut buf, &mut out, &palette);
            let mut j = i;
            while j < bytes.len() && bytes[j] != b'\n' {
                j += 1;
            }
            out.push((src[i..j].to_string(), Color(palette.comment.into())));
            i = j;
            continue;
        }
        if b == b'"' {
            flush_ident(&mut buf, &mut out, &palette);
            let mut j = i + 1;
            while j < bytes.len() {
                if bytes[j] == b'\\' && j + 1 < bytes.len() {
                    j += 2;
                    continue;
                }
                if bytes[j] == b'"' {
                    j += 1;
                    break;
                }
                j += 1;
            }
            out.push((src[i..j].to_string(), Color(palette.string.into())));
            i = j;
            continue;
        }
        if b.is_ascii_alphabetic() || b == b'_' {
            buf.push(b as char);
            i += 1;
            continue;
        }
        flush_ident(&mut buf, &mut out, &palette);
        let mut j = i;
        while j < bytes.len() {
            let c = bytes[j];
            if c == b'/' && j + 1 < bytes.len() && bytes[j + 1] == b'/' {
                break;
            }
            if c == b'"' || c.is_ascii_alphabetic() || c == b'_' {
                break;
            }
            j += 1;
        }
        out.push((src[i..j].to_string(), Color(palette.ink.into())));
        i = j;
    }
    flush_ident(&mut buf, &mut out, &palette);
    out
}

// The `codeblock` SDK registers its `<pre>`/span handler on the scene
// registry (`codeblock::register`, wired by
// `crate::register_scene_extensions` at boot), so this one CodePanel
// body carries the per-span highlight on every host.
#[component]
pub fn CodePanel(props: &CodePanelProps) -> Element {
    let panel_style = CodePanelBox();
    let src = props.src.clone();
    let dynamic = switch(theme_is_dark, move |&is_dark| {
        let palette = if is_dark { DARK_PALETTE } else { LIGHT_PALETTE };
        let spans = highlight(&src, palette);
        let code_style = move || StyleApplication::new(CodeText::sheet());
        codeblock::code_block(spans)
            .with_style(code_style)
            .into_element()
    });
    ui! { view(style = panel_style) { dynamic } }
}

// =============================================================================
// Callout — tinted block: short label + body children.
// =============================================================================

#[derive(Default)]
pub struct CalloutProps {
    pub label: String,
    pub children: Vec<Element>,
}

#[component]
pub fn Callout(props: CalloutProps) -> Element {
    let style = CalloutBox();
    let label = props.label;
    let children = props.children;
    ui! {
        view(style = style) {
            Typography(content = label, kind = typography_kind::Overline)
            children
        }
    }
}

// =============================================================================
// DemoSurface — a single-purpose preview panel (no controls).
// =============================================================================

#[derive(Default)]
pub struct DemoSurfaceProps {
    pub children: Vec<Element>,
}

#[component]
pub fn DemoSurface(props: DemoSurfaceProps) -> Element {
    let style = DemoSurfaceBox();
    let content = DemoSurfaceContent();
    let children = props.children;
    // Card spans the page; an inner max-width column caps + centers the content
    // so a full-width component (Field) renders at a sensible width.
    ui! {
        view(style = style) {
            view(style = content) { children }
        }
    }
}

// =============================================================================
// Demo — preview + controls in a side-by-side wrapping row.
// =============================================================================

#[derive(Default)]
pub struct DemoProps {
    pub preview: Option<Element>,
    pub controls: Option<Element>,
}

#[component]
pub fn Demo(props: DemoProps) -> Element {
    let preview = props.preview.unwrap_or_else(|| ui! { view {} });
    let controls = props.controls.unwrap_or_else(|| ui! { view {} });
    let row_style = DemoRow();
    let preview_style = PreviewBox();
    let controls_style = ControlsBox();
    let preview_slot = PreviewSlot();
    ui! {
        view(style = row_style) {
            view(style = preview_style) {
                view(style = preview_slot) { preview }
            }
            view(style = controls_style) { controls }
        }
    }
}

// =============================================================================
// PropsTable — documents a component's props.
// =============================================================================

pub struct Prop {
    pub name: &'static str,
    pub ty: &'static str,
    pub desc: &'static str,
}

#[derive(Default)]
pub struct PropsTableProps {
    pub rows: Vec<Prop>,
}

// Same-source on both cores: the table SDK is dual-core (real
// `<table>` on web via the External registry / the scene Registry, a
// shared-track CSS-grid on native).
#[component]
pub fn PropsTable(props: PropsTableProps) -> Element {
    ui! {
        Table {
            TableRow {
                TableCell(header = true, text = Some("Prop".to_string()))
                TableCell(header = true, text = Some("Type".to_string()))
                TableCell(header = true, text = Some("Description".to_string()))
            }
            for p in props.rows {
                TableRow {
                    TableCell(text = Some(p.name.to_string()))
                    TableCell(text = Some(p.ty.to_string()))
                    TableCell(text = Some(p.desc.to_string()))
                }
            }
        }
    }
}

// =============================================================================
// Section — a labelled block inside a page.
// =============================================================================

#[derive(Default)]
pub struct SectionProps {
    pub title: String,
    pub children: Vec<Element>,
}

#[component]
pub fn Section(props: SectionProps) -> Element {
    let title = props.title;
    let children = props.children;
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = title, kind = typography_kind::H2)
            children
        }
    }
}

// =============================================================================
// Paragraph + heading helpers.
// =============================================================================

#[derive(Default)]
pub struct PProps {
    pub content: String,
}

#[component]
pub fn P(props: PProps) -> Element {
    let content = props.content;
    ui! { Typography(content = content) }
}

#[derive(Default)]
#[allow(dead_code)]
pub struct H2Props {
    pub content: String,
}

#[allow(dead_code)]
#[component]
pub fn H2(props: H2Props) -> Element {
    let content = props.content;
    ui! { Typography(content = content, kind = typography_kind::H2) }
}

#[derive(Default)]
pub struct H3Props {
    pub content: String,
}

#[component]
pub fn H3(props: H3Props) -> Element {
    let content = props.content;
    ui! { Typography(content = content, kind = typography_kind::H3) }
}

// Keep `Spacer` + `Switch` imports live for pages that re-export shell.
#[allow(dead_code)]
fn _force_imports() -> Element {
    ui! {
        Stack {
            Spacer()
            Switch(value = runtime_core::signal(false))
        }
    }
}
