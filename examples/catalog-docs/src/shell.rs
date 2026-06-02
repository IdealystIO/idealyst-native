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
    typography_kind, Card, Divider, Spacer, Stack, StackGap, Table, TableCell, TableRow,
    Typography,
};
use runtime_core::{
    component, switch, ui, Color, Element, IntoElement, SafeAreaSides, StyleApplication, Tokenized,
};

use crate::catalog::{CatalogModel, Entry, Kind};
use crate::routes::{EntryParams, ENTRY_ROUTE, OVERVIEW_ROUTE};
use crate::styles::{
    Callout as CalloutBox, CodePanel as CodePanelBox, CodeText, PageColumn, PagePad, ScreenScroll,
    SidebarBody, SidebarHeader, SidebarSection,
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

pub fn sidebar(slot: SlotProps, model: Rc<CatalogModel>) -> Element {
    let active_path = slot.active_path;

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
            // Overview link.
            sidebar_overview_link(active_path)
            for kind in model.populated_kinds() {
                text(style = SidebarSection()) { kind.title().to_string() }
                for entry in model.of_kind(kind) {
                    sidebar_entry_link(entry, active_path)
                }
            }
            Spacer()
        }
        .safe_area(SafeAreaSides::VERTICAL)
    }
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
// idea-ui-docs). Renders usage snippets through idea-codeblock.
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
        idea_codeblock::code_block(spans).with_style(code_style).into_element()
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
        let ty = f.ty.clone();
        // Fold the constraint hint into the description column so the
        // table stays three columns wide on narrow screens.
        let desc = if f.constraint.is_empty() {
            f.doc.clone()
        } else if f.doc.is_empty() {
            format!("constraint: {}", f.constraint)
        } else {
            format!("{} (constraint: {})", f.doc, f.constraint)
        };
        rows.push(ui! {
            TableRow {
                TableCell(text = Some(name))
                TableCell(text = Some(ty))
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
        Kind::Component => "Author-defined `#[component]`s — props, composition graph, methods, animations.",
        Kind::Primitive => "Framework leaf nodes of the `ui!` grammar — view, text, button, scroll_view, and friends.",
        Kind::Utility => "Free functions authors call from regular Rust (platform, color, time, theme, layout, math).",
        Kind::Type => "Structs and enums registered via `#[derive(IdealystSchema)]`.",
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
                markdown(&entry.docs)
            }
        });
    }

    let name = entry.name.clone();
    let module = entry.module_path.clone();
    let docs = entry.docs.clone();
    let kind_label = match entry.kind {
        Kind::Component => "component",
        Kind::Primitive => "primitive",
        Kind::Utility => "utility",
        Kind::Type => "type",
        Kind::Guide => "guide",
    }
    .to_string();

    layout(ui! {
        view(style = pad) {
            Stack(gap = StackGap::Xs) {
                Typography(content = name.clone(), kind = typography_kind::H1)
                Typography(content = format!("{} · {}", kind_label, module), muted = true)
            }
            // Docs paragraph(s).
            if !docs.is_empty() {
                markdown(&docs)
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
                    CodePanel(src = entry.return_type.clone())
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
        }
    })
}

fn fields_section(entry: &Entry) -> Element {
    let label = match entry.kind {
        Kind::Component | Kind::Primitive => "Props",
        Kind::Utility => "Parameters",
        Kind::Type => "Fields",
        Kind::Guide => "Fields",
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
                    Typography(content = label)
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
        .map(|p| format!("{}: {}", p.name, p.ty))
        .collect::<Vec<_>>()
        .join(", ");
    let ret = if m.return_type.is_empty() { String::new() } else { format!(" -> {}", m.return_type) };
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

// =============================================================================
// markdown — a tiny block-level renderer for docs/guide bodies. Not a
// full CommonMark engine; it handles the constructs the catalog's docs
// and bundled guides actually use: headings (`#`..`###`), fenced code
// blocks (```), and paragraphs. Inline emphasis is left as-is (legible
// either way). Anything fancier degrades to a plain paragraph rather
// than crashing.
// =============================================================================

fn markdown(src: &str) -> Element {
    let mut blocks: Vec<Element> = Vec::new();
    let mut lines = src.lines().peekable();
    let mut paragraph: Vec<String> = Vec::new();

    let flush_para = |paragraph: &mut Vec<String>, blocks: &mut Vec<Element>| {
        if paragraph.is_empty() {
            return;
        }
        let text = paragraph.join(" ");
        paragraph.clear();
        blocks.push(ui! { Typography(content = text) });
    };

    while let Some(line) = lines.next() {
        let trimmed = line.trim_end();
        // Fenced code block.
        if trimmed.trim_start().starts_with("```") {
            flush_para(&mut paragraph, &mut blocks);
            let mut code: Vec<String> = Vec::new();
            for code_line in lines.by_ref() {
                if code_line.trim_start().starts_with("```") {
                    break;
                }
                code.push(code_line.to_string());
            }
            let src = code.join("\n");
            blocks.push(ui! { CodePanel(src = src) });
            continue;
        }
        // Headings.
        let heading = trimmed.trim_start();
        if let Some(rest) = heading.strip_prefix("### ") {
            flush_para(&mut paragraph, &mut blocks);
            let t = rest.to_string();
            blocks.push(ui! { Typography(content = t, kind = typography_kind::H3) });
            continue;
        }
        if let Some(rest) = heading.strip_prefix("## ") {
            flush_para(&mut paragraph, &mut blocks);
            let t = rest.to_string();
            blocks.push(ui! { Typography(content = t, kind = typography_kind::H2) });
            continue;
        }
        if let Some(rest) = heading.strip_prefix("# ") {
            flush_para(&mut paragraph, &mut blocks);
            let t = rest.to_string();
            blocks.push(ui! { Typography(content = t, kind = typography_kind::H1) });
            continue;
        }
        // Blank line ends a paragraph.
        if trimmed.is_empty() {
            flush_para(&mut paragraph, &mut blocks);
            continue;
        }
        paragraph.push(trimmed.to_string());
    }
    flush_para(&mut paragraph, &mut blocks);

    ui! { Stack(gap = StackGap::Sm) { blocks } }
}
