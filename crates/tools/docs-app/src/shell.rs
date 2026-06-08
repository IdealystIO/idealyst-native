//! Persistent shell (drawer sidebar) + the page chrome the catalog
//! renderer composes from: `CodePanel`, `FieldsTable`, `Callout`,
//! `Section`, and the two page entry points `overview_page` /
//! `entry_page`.
//!
//! Every page is generated from the [`crate::catalog::CatalogModel`] —
//! there are no hand-written component pages here. The chrome
//! primitives are real `#[component]`s so they dispatch inside `ui!`.

use std::rc::Rc;

use drawer_navigator::SlotProps;
use idea_ui::{
    tone, typography_kind, variant, Card, Divider, Field, Modal, Spacer, Stack, StackGap, Table,
    TableCell, Tag, TableRow, Typography,
};
use markdown::{Markdown, MdTheme};
use runtime_core::{
    component, effect, fixed_size, flat_list, rx, signal, switch, ui, Color, Element, IntoElement,
    SafeAreaSides, StyleApplication, Tokenized,
};

use crate::catalog::{CatalogModel, Entry, Kind};
use crate::routes::{EntryParams, ENTRY_ROUTE, OVERVIEW_ROUTE};
use crate::styles::{
    Callout as CalloutBox, Chip, ChipActive, ChipRow, ChipText, ChipTextActive,
    CodePanel as CodePanelBox, CodeText, LinkText, MemberRow, PageColumn, PagePad, PreviewBox, PreviewSlot,
    ResultHead, ResultNamespace, ResultsScroll, ScreenScroll, SearchBox, SearchBoxText,
    SearchResultsBody, SidebarBody, SidebarHeader, SidebarSection, ThemeToggleBox, ThemeToggleText,
};

// =============================================================================
// Page chrome: scroll surface + mobile hamburger (lifted from idea-ui-docs).
// =============================================================================

fn layout(content: Element) -> Element {
    let style = ScreenScroll();
    ui! {
        view(style = PageColumn()) {
            menu_button()
            scroll_view(style = style) { content }
                .safe_area(SafeAreaSides::BOTTOM)
        }
    }
}

fn menu_button() -> Element {
    use runtime_core::primitives::navigator::ambient_drawer;
    use runtime_core::viewport_size;

    let Some(chrome) = ambient_drawer() else {
        return ui! { view {} };
    };
    let open = chrome.open.clone();
    let below = chrome.collapse_below;
    ui! {
        if viewport_size().get().width < below {
            view(style = crate::styles::TopBar()) {
                hamburger(open.clone())
            }
        }
    }
}

fn hamburger(open: Rc<dyn Fn()>) -> Element {
    let glyph = runtime_core::icon(icons_lucide::MENU)
        .color(idea_ui::idea_color(|c| c.text.clone()))
        .into_element();
    runtime_core::pressable(vec![glyph], move || (open)())
        .with_style(move || StyleApplication::new(crate::styles::MenuButton::sheet()))
        .into_element()
}

// =============================================================================
// Sidebar — built once at navigator init; walks the catalog model,
// grouping entries by kind. Survives screen swaps.
// =============================================================================

/// A search trigger that *looks* like a text input (bordered, rounded,
/// muted placeholder) but is a `pressable` that opens the search modal.
#[derive(Default)]
pub struct SearchTriggerProps {
    pub on_press: Option<Rc<dyn Fn()>>,
}

#[component]
pub fn SearchTrigger(props: SearchTriggerProps) -> Element {
    let on_press = props.on_press.clone();
    let inner = ui! {
        view(style = SearchBox()) {
            text(style = SearchBoxText()) { "Search the catalog…".to_string() }
        }
    };
    runtime_core::pressable(vec![inner], move || {
        if let Some(cb) = &on_press {
            (cb)();
        }
    })
    .into_element()
}

/// Light/dark theme toggle. The label is a reactive `text` node bound to
/// the global dark-mode signal, so it updates in place when the mode
/// flips (the node's own effect survives the sidebar's scope, unlike a
/// free `Effect`). Pressing it swaps the live theme via `toggle_theme`.
#[component]
pub fn ThemeToggle() -> Element {
    let label = runtime_core::text(|| {
        if crate::theme::dark_mode().get() {
            "\u{2600} Light".to_string()
        } else {
            "\u{263e} Dark".to_string()
        }
    })
    .with_style(ThemeToggleText())
    .into_element();
    let inner = ui! {
        view(style = ThemeToggleBox()) { label }
    };
    runtime_core::pressable(vec![inner], || crate::theme::toggle_theme()).into_element()
}

pub fn sidebar(slot: SlotProps, model: Rc<CatalogModel>) -> Element {
    let active_path = slot.active_path;

    // Search modal state.
    let open = signal!(false);
    let query = signal!(String::new());
    let filter = signal!(Option::<Kind>::None);
    let on_query: Rc<dyn Fn(String)> = Rc::new(move |s| query.set(s));
    let open_search: Rc<dyn Fn()> = Rc::new(move || open.set(true));
    let close_search: Rc<dyn Fn()> = Rc::new(move || open.set(false));
    let model_for_results = model.clone();

    let header_children: Vec<Element> = vec![
        ui! { Typography(content = "Idealyst Catalog".to_string(), kind = typography_kind::H3) },
        ui! {
            Typography(
                content = format!("{} entries, generated from the live catalog.", model.total()),
                muted = true,
            )
        },
    ];

    ui! {
        view(style = SidebarBody()) {
            view(style = SidebarHeader()) { header_children }
            SearchTrigger(on_press = Some(open_search.clone()))
            ThemeToggle()
            // Overview link.
            sidebar_overview_link(active_path)
            for kind in model.populated_kinds() {
                if kind == Kind::Scope {
                    // Scopes ARE the primary headings (same look as a kind
                    // section header) — pressable, each navigating to its
                    // scope screen. No generic "Scopes" group header.
                    for entry in model.of_kind(kind) {
                        sidebar_scope_heading(entry, active_path)
                    }
                } else {
                    text(style = SidebarSection()) { kind.title().to_string() }
                    for entry in model.of_kind(kind) {
                        sidebar_entry_link(entry, active_path)
                    }
                }
            }
            Spacer()
            if open.get() {
                Modal(on_dismiss = Some(close_search.clone()), width = 600.0) {
                    Typography(content = "Search the catalog".to_string(), kind = typography_kind::H3)
                    Field(
                        value = query,
                        on_change = on_query.clone(),
                        placeholder = Some("Component, type, utility…".to_string()),
                    )
                    search_results(
                        model_for_results.clone(),
                        query,
                        filter,
                        close_search.clone(),
                    )
                }
            }
        }
        .safe_area(SafeAreaSides::VERTICAL)
    }
}

/// Reactive search results — rebuilds the result list as the query
/// changes (via `switch`, keyed on the query string). Each result links
/// to its entry page; the modal closes on navigation (see `sidebar`).
/// Reactive search body — a row of kind-filter chips plus result cards.
/// Rebuilds (via `switch`) whenever the query *or* the active kind filter
/// changes. Each card links to its entry and closes the modal on press.
fn search_results(
    model: Rc<CatalogModel>,
    query: runtime_core::Signal<String>,
    filter: runtime_core::Signal<Option<Kind>>,
    close: Rc<dyn Fn()>,
) -> Element {
    // Filter chips stay pinned above the scroll region; they rebuild only
    // when the active filter changes.
    let chips = switch(
        move || filter.get(),
        move |f: &Option<Kind>| filter_chips(filter, *f),
    );
    // The results list — the only part that scrolls.
    let results = switch(
        move || (query.get(), filter.get()),
        move |(q, f): &(String, Option<Kind>)| {
            let f = *f;
            if q.trim().chars().count() < 2 {
                ui! {
                    Typography(
                        content = "Type at least 2 characters to search…".to_string(),
                        muted = true,
                    )
                }
            } else {
                let hits = model.search(q, f);
                if hits.is_empty() {
                    ui! {
                        Typography(
                            content = format!("No matches for \u{201c}{}\u{201d}.", q.trim()),
                            muted = true,
                        )
                    }
                } else {
                    let cards: Vec<Element> =
                        hits.into_iter().map(|h| result_card(h, close.clone())).collect();
                    ui! { Stack(gap = StackGap::Sm) { cards } }
                }
            }
        },
    );
    ui! {
        view(style = SearchResultsBody()) {
            chips
            scroll_view(style = ResultsScroll()) { results }
        }
    }
}

/// The kind-filter chip row. `current` is the active filter (`None` = all).
fn filter_chips(filter: runtime_core::Signal<Option<Kind>>, current: Option<Kind>) -> Element {
    let opts: [(&str, Option<Kind>); 5] = [
        ("All", None),
        ("Components", Some(Kind::Component)),
        ("Primitives", Some(Kind::Primitive)),
        ("Types", Some(Kind::Type)),
        ("Utilities", Some(Kind::Utility)),
    ];
    let chips: Vec<Element> = opts
        .into_iter()
        .map(|(label, k)| filter_chip(label, current == k, filter, k))
        .collect();
    ui! { view(style = ChipRow()) { chips } }
}

/// One filter chip — a pressable pill that sets the active kind filter.
fn filter_chip(
    label: &str,
    active: bool,
    filter: runtime_core::Signal<Option<Kind>>,
    kind: Option<Kind>,
) -> Element {
    use runtime_core::derived;
    let container = Chip().active(derived(move || {
        if active { ChipActive::On } else { ChipActive::Off }
    }));
    let text_style = ChipText().active(derived(move || {
        if active { ChipTextActive::On } else { ChipTextActive::Off }
    }));
    let label = label.to_string();
    let inner = ui! { view(style = container) { text(style = text_style) { label } } };
    runtime_core::pressable(vec![inner], move || filter.set(kind)).into_element()
}

/// One search result, as a card linking to the entry: a blue title, the
/// kind, and a short doc summary. Wrapped in a `pressable` that closes the
/// modal on press (the inner `link` handles navigation).
fn result_card(h: crate::catalog::SearchHit, close: Rc<dyn Fn()>) -> Element {
    let params = EntryParams::new(h.kind, h.slug.clone());
    let name = h.name.clone();
    let kind_label = h.kind.noun().to_string();
    let module = h.module_path.clone();
    let summary = h.summary.clone();
    let card = ui! {
        Card {
            Stack(gap = StackGap::Xs) {
                // Crate / namespace — a small label above the result.
                if !module.is_empty() {
                    text(style = ResultNamespace()) { module }
                }
                // Name + kind tag, inline.
                view(style = ResultHead()) {
                    text(style = LinkText()) { name }
                    Tag(label = kind_label, tone = tone::Neutral, variant = variant::Soft)
                }
                if !summary.is_empty() {
                    Typography(content = summary, muted = true)
                }
            }
        }
    };
    let link_el = ui! { link(route = &ENTRY_ROUTE, params = params) { card } };
    runtime_core::pressable(vec![link_el], move || (close)()).into_element()
}

fn sidebar_overview_link(active_path: runtime_core::Signal<String>) -> Element {
    use crate::styles::{NavLink, NavLinkActive, NavLinkText, NavLinkTextActive};
    use runtime_core::derived;
    let container = NavLink().active(derived(move || {
        if active_path.get() == "/" { NavLinkActive::On } else { NavLinkActive::Off }
    }));
    let text_style = NavLinkText().active(derived(move || {
        if active_path.get() == "/" { NavLinkTextActive::On } else { NavLinkTextActive::Off }
    }));
    ui! {
        link(route = &OVERVIEW_ROUTE, params = ()) {
            view(style = container) {
                text(style = text_style) { "Overview".to_string() }
            }
        }
    }
}

/// A scope rendered as a pressable primary heading — the same uppercase
/// `SidebarSection` look as a kind header, but wrapped in a link so it
/// navigates to the scope's screen.
fn sidebar_scope_heading(entry: &Entry, _active_path: runtime_core::Signal<String>) -> Element {
    let params = EntryParams::new(entry.kind, entry.slug.clone());
    let label = entry.name.clone();
    ui! {
        link(route = &ENTRY_ROUTE, params = params) {
            text(style = SidebarSection()) { label }
        }
    }
}

fn sidebar_entry_link(entry: &Entry, active_path: runtime_core::Signal<String>) -> Element {
    use crate::styles::{NavLink, NavLinkActive, NavLinkText, NavLinkTextActive};
    use runtime_core::derived;

    let params = EntryParams::new(entry.kind, entry.slug.clone());
    let url = params.url();
    let url_for_container = url.clone();
    let url_for_text = url.clone();

    let container = NavLink().active(derived(move || {
        if active_path.get() == url_for_container { NavLinkActive::On } else { NavLinkActive::Off }
    }));
    let text_style = NavLinkText().active(derived(move || {
        if active_path.get() == url_for_text { NavLinkTextActive::On } else { NavLinkTextActive::Off }
    }));
    let label = entry.name.clone();

    ui! {
        link(route = &ENTRY_ROUTE, params = params) {
            view(style = container) {
                text(style = text_style) { label }
            }
        }
    }
}

// =============================================================================
// CodePanel — theme-aware syntax-highlighted code block (lifted from
// idea-ui-docs). Renders usage snippets through codeblock.
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

const LIGHT_PALETTE: Palette =
    Palette { ink: "#1f2328", comment: "#8a8270", string: "#1f6e5f", accent: "#5a4fcf" };
const DARK_PALETTE: Palette =
    Palette { ink: "#e8eaf0", comment: "#9099a8", string: "#5eead4", accent: "#c4b5fd" };

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
        let color = if keywords.contains(&buf.as_str()) { palette.accent } else { palette.ink };
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

#[component]
pub fn CodePanel(props: &CodePanelProps) -> Element {
    let panel_style = CodePanelBox();
    let src = props.src.clone();
    let dynamic = switch(theme_is_dark, move |&is_dark| {
        let palette = if is_dark { DARK_PALETTE } else { LIGHT_PALETTE };
        let spans = highlight(&src, palette);
        let code_style = move || StyleApplication::new(CodeText::sheet());
        codeblock::code_block(spans).with_style(code_style).into_element()
    });
    ui! { view(style = panel_style) { dynamic } }
}

// =============================================================================
// Callout — tinted note block.
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
// Section — a labelled block on a detail page.
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
// FieldsTable — the auto-generated props/fields/params table. Built from
// the catalog's normalized `Field`s through idea-ui's themed `Table`.
// =============================================================================

#[derive(Default)]
pub struct FieldsTableProps {
    pub fields: Vec<crate::catalog::Field>,
    /// Whether any field carried docs/constraint. When false, the table
    /// still renders (name + type) but the caller pairs it with a
    /// "not yet documented" note.
    pub documented: bool,
}

#[component]
pub fn FieldsTable(props: FieldsTableProps) -> Element {
    let mut rows: Vec<Element> = Vec::with_capacity(props.fields.len() + 1);
    rows.push(ui! {
        TableRow {
            TableCell(header = true, text = Some("Name".to_string()))
            TableCell(header = true, text = Some("Type".to_string()))
            TableCell(header = true, text = Some("Description".to_string()))
        }
    });
    for f in props.fields {
        let name = f.name.clone();
        let ty = tidy_type(&f.ty);
        // Fold the constraint hint into the description column so the
        // table stays three columns wide on narrow screens.
        let desc = if f.constraint.is_empty() {
            f.doc.clone()
        } else if f.doc.is_empty() {
            format!("constraint: {}", f.constraint)
        } else {
            format!("{} (constraint: {})", f.doc, f.constraint)
        };
        // When the type resolves to a catalog Type (not a primitive), make
        // the Type cell a link to that type's page.
        let type_cell = match f.type_link {
            Some(link) => {
                let params = EntryParams::new(link.kind, link.slug);
                ui! {
                    TableCell {
                        link(route = &ENTRY_ROUTE, params = params) {
                            text(style = LinkText()) { ty }
                        }
                    }
                }
            }
            None => ui! { TableCell(text = Some(ty)) },
        };
        rows.push(ui! {
            TableRow {
                TableCell(text = Some(name))
                type_cell
                TableCell(text = Some(desc))
            }
        });
    }
    let _ = props.documented;
    ui! { Table { rows } }
}

// =============================================================================
// Page entry points.
// =============================================================================

/// Catalog landing page — one card per populated kind with its count and
/// a short description.
pub fn overview_page(model: &CatalogModel) -> Element {
    let total = model.total();
    let kinds = model.populated_kinds();
    // Pre-compute the per-kind card data eagerly (owned), so the `ui!`
    // `for` below iterates an owned `Vec` and never borrows `model`
    // across the macro's closure boundary.
    let cards: Vec<(String, usize, &'static str)> = kinds
        .iter()
        .map(|&k| (k.title().to_string(), model.of_kind(k).len(), kind_blurb(k)))
        .collect();
    let pad = PagePad();
    layout(ui! {
        view(style = pad) {
            Stack(gap = StackGap::Sm) {
                Typography(content = "Idealyst Catalog".to_string(), kind = typography_kind::H1)
                Typography(
                    content = format!(
                        "Auto-generated from the framework's live introspection catalog — \
                         {} entries across {} kinds. Pick a kind from the sidebar, or jump in below.",
                        total,
                        kinds.len()
                    ),
                    kind = typography_kind::BodyLg,
                    muted = true,
                )
            }
            Callout(label = "How this works".to_string()) {
                Typography(
                    content = "Every `#[component]`, primitive, utility, type, and bundled guide \
                               the build links registers itself into an in-process catalog. This \
                               page calls `mcp_catalog::ResolvedCatalog::build()` at runtime and \
                               renders what it finds — no hand-written pages.".to_string(),
                )
            }
            for card in cards {
                overview_kind_card(card.0, card.1, card.2)
            }
        }
    })
}

fn overview_kind_card(title: String, count: usize, blurb: &'static str) -> Element {
    let blurb = blurb.to_string();
    ui! {
        Card {
            Stack(gap = StackGap::Xs) {
                Typography(content = format!("{} ({})", title, count), kind = typography_kind::H3)
                Typography(content = blurb, muted = true)
            }
        }
    }
}

fn kind_blurb(kind: Kind) -> &'static str {
    match kind {
        Kind::Scope => "Feature-area groupings — each scope lists the components and utilities assigned to it by module proximity.",
        Kind::Component => "Author-defined `#[component]`s — props, composition graph, methods, animations.",
        Kind::Primitive => "Framework leaf nodes of the `ui!` grammar — view, text, button, scroll_view, and friends.",
        Kind::Utility => "Free functions authors call from regular Rust (platform, color, time, theme, layout, math).",
        Kind::Type => "Structs and enums registered via `#[derive(IdealystSchema)]`.",
        Kind::IconSet => "Icon packs — every glyph in each set, searchable, with its `icon(...)` import.",
        Kind::Guide => "Bundled framework authoring guides, rendered from their shipped markdown.",
    }
}

/// Detail page for a single catalog entry. Dispatches on kind for the
/// kind-specific sections (composes/methods/animations for components,
/// variants for enum types, return type for utilities, markdown body for
/// guides) while sharing the header + fields table.
pub fn entry_page(model: &CatalogModel, kind: Kind, slug: &str) -> Element {
    let pad = PagePad();
    let Some(entry) = model.find(kind, slug) else {
        return layout(ui! {
            view(style = pad) {
                Typography(content = "Not found".to_string(), kind = typography_kind::H1)
                Typography(
                    content = format!("No catalog entry for {}/{}.", kind.path_segment(), slug),
                    muted = true,
                )
            }
        });
    };

    // Guides are pure markdown — render the body, nothing else.
    if entry.kind == Kind::Guide {
        return layout(ui! {
            view(style = pad) {
                Typography(content = entry.name.clone(), kind = typography_kind::H1)
                render_markdown(&entry.docs)
            }
        });
    }

    // Icon packs get the searchable, virtualized glyph gallery — a
    // viewport-filling page (not the shared scroll column) so the
    // `flat_list` can bound and virtualize ~1600 icons.
    if entry.kind == Kind::IconSet {
        return icon_set_page(entry);
    }

    // Scopes list the entities assigned to them by module proximity,
    // grouped by kind so types are visually separate from components.
    if entry.kind == Kind::Scope {
        let members = entry.members.clone();
        let count = members.len();
        let docs = entry.docs.clone();
        let groups: Vec<(Kind, Vec<crate::catalog::EntryLink>)> = [
            Kind::Component,
            Kind::Primitive,
            Kind::Utility,
            Kind::Type,
            Kind::Guide,
        ]
        .into_iter()
        .filter_map(|k| {
            let items: Vec<_> = members.iter().filter(|m| m.kind == k).cloned().collect();
            if items.is_empty() {
                None
            } else {
                Some((k, items))
            }
        })
        .collect();
        return layout(ui! {
            view(style = pad) {
                Stack(gap = StackGap::Xs) {
                    Typography(content = entry.name.clone(), kind = typography_kind::H1)
                    Typography(content = format!("scope · {} members", count), muted = true)
                }
                if !docs.is_empty() {
                    render_markdown(&docs)
                }
                Divider()
                if count == 0 {
                    Typography(
                        content = "No entries resolve into this scope yet.".to_string(),
                        muted = true,
                    )
                }
                for (k, items) in groups {
                    scope_member_group(k, items)
                }
            }
        });
    }

    let name = entry.name.clone();
    let module = entry.module_path.clone();
    let docs = entry.docs.clone();
    let scope = entry.scope.clone();
    let kind_label = entry.kind.noun().to_string();

    layout(ui! {
        view(style = pad) {
            Stack(gap = StackGap::Xs) {
                Typography(content = name.clone(), kind = typography_kind::H1)
                Typography(content = format!("{} · {}", kind_label, module), muted = true)
            }
            // Scope badge — a link to the owning scope's page.
            if let Some(s) = scope {
                scope_member_link(s)
            }
            // Docs paragraph(s).
            if !docs.is_empty() {
                render_markdown(&docs)
            }
            Divider()
            // Fields / props / params table.
            fields_section(entry)
            // Enum variants.
            if !entry.variants.is_empty() {
                variants_section(entry)
            }
            // Return type (utilities).
            if !entry.return_type.is_empty() {
                Section(title = "Returns".to_string()) {
                    CodePanel(src = tidy_type(&entry.return_type))
                }
            }
            // Composition graph (components).
            if !entry.composes.is_empty() {
                composes_section(entry)
            }
            // Methods (components).
            if !entry.methods.is_empty() {
                methods_section(entry)
            }
            // Animations (components).
            if !entry.animations.is_empty() {
                animations_section(entry)
            }
            // Usage recipes (components).
            if !entry.recipes.is_empty() {
                recipes_section(entry)
            }
        }
    })
}

/// The icon-pack gallery page: a viewport-filling column (header +
/// virtualized `flat_list` grid) with a live name filter. Bypasses the
/// shared `layout()` scroll column because the grid must be height-bounded
/// to virtualize (see `crate::icons::list_style`). When the pack's crate
/// isn't linked into this app, only its metadata is shown.
///
/// The gallery body is a `#[component]` ([`IconGallery`]), not inline code,
/// for a load-bearing reason: the `flat_list` virtualizer installs its
/// reactive re-diff as an `Effect` that only survives if an *owning
/// reactive scope* is active when it's built (see
/// `runtime_core::walker::virtualizer`). A plain page fn provides no such
/// scope, so the grid would never re-filter on search; a component body
/// does — the same reason `examples/icon-gallery` is a `#[component]`.
fn icon_set_page(entry: &Entry) -> Element {
    use crate::icons;
    use crate::styles::{PageColumn, PagePad};

    let meta = entry.icon_set.clone().unwrap_or_default();
    let title = entry.name.clone();
    let docs = entry.docs.clone();
    let count = meta.count;

    // Pack registered in the catalog but its crate isn't linked here — show
    // metadata, no grid. (Renders inside the normal scroll column.)
    if icons::registry(&meta.crate_name).is_none() {
        let pad = PagePad();
        return layout(ui! {
            view(style = pad) {
                Stack(gap = StackGap::Xs) {
                    Typography(content = title, kind = typography_kind::H1)
                    Typography(content = format!("icon set · {} icons", count), muted = true)
                }
                if !docs.is_empty() {
                    render_markdown(&docs)
                }
                Callout(label = "Preview unavailable".to_string()) {
                    Typography(
                        content = "This pack's crate isn't linked into the docs app, so the \
                                   glyph grid can't render here. Its metadata is shown above.".to_string(),
                    )
                }
            }
        });
    }

    ui! {
        view(style = PageColumn()) {
            menu_button()
            IconGallery(
                crate_name = meta.crate_name,
                title = title,
                import_path = meta.import_path,
                license = meta.license,
                homepage = meta.homepage,
                count = count,
            )
        }
        .safe_area(SafeAreaSides::VERTICAL)
    }
}

/// Searchable, virtualized glyph grid for one icon pack. A `#[component]`
/// so its body runs in an owning reactive scope — required for the
/// `flat_list` virtualizer's data-effect to survive (see [`icon_set_page`]).
#[derive(Default)]
pub struct IconGalleryProps {
    /// Bridge key into `crate::icons::registry` for the glyph geometry.
    pub crate_name: String,
    /// Pack display title.
    pub title: String,
    /// `use` path root (`icons_lucide`), for the usage hint.
    pub import_path: String,
    pub license: String,
    pub homepage: String,
    /// Total icon count (for the live "N of M" tally).
    pub count: usize,
}

#[component]
pub fn IconGallery(props: IconGalleryProps) -> Element {
    use crate::icons;
    use crate::styles::PagePad;

    // Guaranteed Some — `icon_set_page` only renders this when linked.
    let set = icons::registry(&props.crate_name).unwrap_or(&[]);
    let count = props.count;

    let query: runtime_core::Signal<String> = signal!(String::new());
    let rows: runtime_core::Signal<Vec<icons::RowData>> = signal!(icons::build_rows(set, ""));
    // Anchored in this component's scope, so it re-runs on every edit —
    // which is also what keeps the virtualizer's own data-effect alive.
    effect!({
        let q = query.get();
        rows.set(icons::build_rows(set, &q));
    });
    let on_query: Rc<dyn Fn(String)> = Rc::new(move |s| query.set(s));

    // The import rule, spelled out so an author (or an LLM reading the page)
    // knows exactly how to use any glyph below.
    let usage = format!(
        "Import an icon by its SCREAMING_SNAKE_CASE constant — e.g. \
         `arrow-right` → `{ip}::ARROW_RIGHT`, then `icon({ip}::ARROW_RIGHT)`.",
        ip = props.import_path,
    );
    let attribution = if props.homepage.is_empty() {
        format!("Licensed under {}.", props.license)
    } else {
        format!("Licensed under {} · {}", props.license, props.homepage)
    };
    let title = props.title;

    let list = flat_list::<icons::RowData, _, (), _>(
        rows,
        |_idx, r: &icons::RowData| r.key,
        fixed_size(icons::ROW_H),
        |_idx, r: &icons::RowData| icons::render_row(r),
    )
    .into_element()
    .with_style(icons::list_style());

    let header_pad = PagePad();
    ui! {
        view(style = icons::gallery_col()) {
            view(style = header_pad) {
                Stack(gap = StackGap::Xs) {
                    Typography(content = title, kind = typography_kind::H1)
                    Typography(
                        content = rx!(format!(
                            "{} of {} icons",
                            icons::match_count(set, &query.get()),
                            count
                        )),
                        muted = true,
                    )
                }
                Callout(label = "Usage".to_string()) {
                    Typography(content = usage)
                    Typography(content = attribution, muted = true)
                }
                Field(
                    value = query,
                    on_change = on_query,
                    placeholder = Some("Search icons by name…".to_string()),
                )
            }
            list
        }
    }
}

fn fields_section(entry: &Entry) -> Element {
    let label = match entry.kind {
        Kind::Component | Kind::Primitive => "Props",
        Kind::Utility => "Parameters",
        Kind::Type => "Fields",
        Kind::Guide | Kind::Scope | Kind::IconSet => "Fields",
    }
    .to_string();
    if entry.fields.is_empty() {
        return ui! {
            Section(title = label) {
                Typography(content = "No props.".to_string(), muted = true)
            }
        };
    }
    let fields = entry.fields.clone();
    let documented = entry.fields_documented;
    ui! {
        Section(title = label) {
            // Graceful handling of the common idea-ui case: props struct
            // hasn't derived IdealystSchema yet, so only the param's
            // type is known. Show the note, then the (type-only) table.
            if !documented {
                Callout(label = "Heads up".to_string()) {
                    Typography(
                        content = "Per-prop docs aren't captured for this entry yet (its props \
                                   struct hasn't derived `IdealystSchema`). The types below are \
                                   accurate; descriptions land when the schema derive is added.".to_string(),
                    )
                }
            }
            FieldsTable(fields = fields, documented = documented)
        }
    }
}

fn variants_section(entry: &Entry) -> Element {
    let variants = entry.variants.clone();
    let mut rows: Vec<Element> = Vec::with_capacity(variants.len() + 1);
    rows.push(ui! {
        TableRow {
            TableCell(header = true, text = Some("Variant".to_string()))
            TableCell(header = true, text = Some("Description".to_string()))
        }
    });
    for (vn, vd) in variants {
        rows.push(ui! {
            TableRow {
                TableCell(text = Some(vn))
                TableCell(text = Some(vd))
            }
        });
    }
    ui! {
        Section(title = "Variants".to_string()) {
            Table { rows }
        }
    }
}

fn composes_section(entry: &Entry) -> Element {
    let composes = entry.composes.clone();
    ui! {
        Section(title = "Composes".to_string()) {
            view {
                for c in composes {
                    compose_link(c)
                }
            }
        }
    }
}

/// The scope badge on an entry's detail page — links up to the owning
/// scope, labelled `"Scope: <name>"`.
fn scope_member_link(m: crate::catalog::EntryLink) -> Element {
    let label = format!("Scope: {}", m.name);
    let params = EntryParams::new(m.kind, m.slug.clone());
    ui! {
        link(route = &ENTRY_ROUTE, params = params) {
            text(style = LinkText()) { label }
        }
    }
}

/// One kind-grouped block of a scope's members — a subheading (the
/// kind's plural title + count) over its member links. This is what
/// keeps types visually distinct from components within a scope.
fn scope_member_group(kind: Kind, members: Vec<crate::catalog::EntryLink>) -> Element {
    ui! {
        Section(title = format!("{} ({})", kind.title(), members.len())) {
            view(style = MemberRow()) {
                for m in members {
                    member_link(m)
                }
            }
        }
    }
}

/// A plain link to an entry by name — the kind is implied by the
/// surrounding [`scope_member_group`] heading.
fn member_link(m: crate::catalog::EntryLink) -> Element {
    let params = EntryParams::new(m.kind, m.slug.clone());
    let label = m.name.clone();
    ui! {
        link(route = &ENTRY_ROUTE, params = params) {
            text(style = LinkText()) { label }
        }
    }
}

/// Collapse the spaces `quote!{ #ty }.to_string()` inserts around generic
/// brackets in catalog type strings: `Vec < X >` → `Vec<X>`,
/// `Signal < Vec < bool > >` → `Signal<Vec<bool>>`. Applied at every type
/// display site (props table, method signatures, return types).
fn tidy_type(s: &str) -> String {
    s.replace(" <", "<")
        .replace("< ", "<")
        .replace(" >", ">")
        .replace("> ", ">")
        .replace(" :: ", "::")
}

/// Drop the recipe fn's leading `///` doc lines from its source. The same
/// prose is already rendered above the code block (as markdown from the
/// recipe's `docs`), so leaving it in the source shows it twice.
fn strip_leading_doc(src: &str) -> String {
    let start = src
        .lines()
        .position(|l| {
            let t = l.trim_start();
            !(t.starts_with("///") || t.starts_with("//!"))
        })
        .unwrap_or(0);
    src.lines().skip(start).collect::<Vec<_>>().join("\n")
}

fn compose_link(c: crate::catalog::Compose) -> Element {
    // Resolved edges link to the target component's detail page;
    // unresolved/ambiguous edges render as plain muted text (the target
    // isn't a known catalog component — e.g. a primitive or external).
    match c.target_slug {
        Some(slug) => {
            let params = EntryParams::new(Kind::Component, slug);
            let label = c.name.clone();
            ui! {
                link(route = &ENTRY_ROUTE, params = params) {
                    text(style = LinkText()) { label }
                }
            }
        }
        None => {
            let label = format!("{} (primitive or unresolved)", c.name);
            ui! { Typography(content = label, muted = true) }
        }
    }
}

fn methods_section(entry: &Entry) -> Element {
    let methods = entry.methods.clone();
    ui! {
        Section(title = "Methods".to_string()) {
            for m in methods {
                method_row(m)
            }
        }
    }
}

fn method_row(m: crate::catalog::Method) -> Element {
    let params = m
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name, tidy_type(&p.ty)))
        .collect::<Vec<_>>()
        .join(", ");
    let ret = if m.return_type.is_empty() {
        String::new()
    } else {
        format!(" -> {}", tidy_type(&m.return_type))
    };
    let sig = format!("fn {}({}){}", m.name, params, ret);
    let doc = m.doc.clone();
    ui! {
        Card {
            Stack(gap = StackGap::Xs) {
                CodePanel(src = sig)
                if !doc.is_empty() {
                    Typography(content = doc, muted = true)
                }
            }
        }
    }
}

fn animations_section(entry: &Entry) -> Element {
    let animations = entry.animations.clone();
    let mut rows: Vec<Element> = Vec::with_capacity(animations.len() + 1);
    rows.push(ui! {
        TableRow {
            TableCell(header = true, text = Some("Binding".to_string()))
            TableCell(header = true, text = Some("Initial value".to_string()))
        }
    });
    for a in animations {
        let binding = if a.binding.is_empty() { "(inline)".to_string() } else { a.binding.clone() };
        rows.push(ui! {
            TableRow {
                TableCell(text = Some(binding))
                TableCell(text = Some(a.initial.clone()))
            }
        });
    }
    ui! {
        Section(title = "Animations".to_string()) {
            Table { rows }
        }
    }
}

fn recipes_section(entry: &Entry) -> Element {
    let recipes = entry.recipes.clone();
    ui! {
        Section(title = "Recipes".to_string()) {
            for recipe in recipes {
                recipe_card(recipe)
            }
        }
    }
}

// The build-time `recipe_renderer(module_path, name) -> Option<fn() ->
// Element>` map. Generated by `build.rs` from the catalog's recipes:
// one arm per zero-arg `idea_ui` recipe (the renderable ones), keyed by
// the recipe's `module_path!()` + fn name. Lets us turn a recipe into a
// LIVE component preview instead of only showing its source. This is the
// only DCE-proof path on wasm — the runtime inventory thunks are pruned,
// but a statically-linked fn reference survives.
include!(concat!(env!("OUT_DIR"), "/recipe_renderers.rs"));

fn recipe_card(recipe: crate::catalog::Recipe) -> Element {
    // Pretty-print the recipe fn name (`button_basic` → "Button basic")
    // for the card heading; primary recipes get a tag so authors can
    // tell the canonical example apart from incidental `uses` mentions.
    let title = recipe.name.replace('_', " ");
    let heading = if recipe.primary { format!("{} · primary", title) } else { title };
    let docs = recipe.docs.clone();
    let source = strip_leading_doc(&recipe.source);
    // A live preview, when this recipe is a zero-arg renderable one the
    // build-time map could address. Props-defining recipes (those whose
    // wrapper fn takes args) aren't in the map → no preview, source only.
    let preview = recipe_renderer(&recipe.module_path, &recipe.name).map(|render| render());
    ui! {
        Card {
            Stack(gap = StackGap::Sm) {
                Typography(content = heading, kind = typography_kind::H3)
                if !docs.is_empty() {
                    render_markdown(&docs)
                }
                if let Some(preview) = preview {
                    view(style = PreviewBox()) {
                        view(style = PreviewSlot()) { preview }
                    }
                }
                CodePanel(src = source)
            }
        }
    }
}

// =============================================================================
// render_markdown — prose bodies (guide pages + component/type/scope/recipe
// docs) go through the `markdown` SDK: one native styled-text node per
// backend with full CommonMark/GFM (headings, lists, emphasis, links,
// quotes, inline + fenced code), replacing the old hand-rolled block
// renderer that dropped lists/emphasis/links. The theme follows the docs'
// live light/dark toggle, so flipping it re-paints the prose.
//
// Recipe code *samples* are NOT routed here — they keep their
// syntax-highlighted `codeblock` panels (`CodePanel`, used in the recipe
// section of `entry_page`). The SDK renders fenced code as plain monospace,
// which is fine for the incidental snippets embedded inside prose but not
// for the showcased recipe sources.
// =============================================================================

fn render_markdown(src: &str) -> Element {
    // `MdTheme` styles text only; the surrounding page background is owned
    // by the docs chrome. `rx!` keeps the theme live: when the sidebar
    // toggle flips `dark_mode`, the SDK rebuilds the single native node
    // with the matching colors.
    let dark = crate::theme::dark_mode();
    let theme = rx!(if dark.get() {
        MdTheme::dark()
    } else {
        MdTheme::light()
    });
    ui! { Markdown(source = src.to_string(), theme = theme) }
}
