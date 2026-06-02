//! Persistent shell (drawer sidebar) + the per-page chrome primitives
//! every component page is built from: `CodePanel`, `PropsTable`,
//! `DemoSurface`, `Demo`, `Callout`, `ComponentPage`.
//!
//! Each of these is a real `#[component]` so it dispatches inside
//! `ui!` like any other tag (e.g. `Demo(preview = ..., controls = ...)`)
//! and pages compose them through the macro.

use std::rc::Rc;

use runtime_core::{
    component, derived, switch, ui, Color, Element, IntoElement, SafeAreaSides, StyleApplication,
    Tokenized,
};
use drawer_navigator::SlotProps;
use idea_ui::{
    dark_theme, light_theme, set_idea_theme, typography_kind, Card, Spacer, Stack, StackGap,
    Switch, Table, TableCell, TableRow, Typography,
};

use crate::routes::SECTIONS;
use crate::styles::{
    Callout as CalloutBox, CodePanel as CodePanelBox, CodeText, ControlsBox, DemoRow,
    DemoSurface as DemoSurfaceBox, NavLink, NavLinkActive, NavLinkText, NavLinkTextActive,
    PageColumn, PagePad, PreviewBox, PreviewSlot, ScreenScroll, SidebarBody, SidebarFooter,
    SidebarHeader, SidebarSection,
};

// =============================================================================
// Layout wrapper — every page calls `shell::layout(...)` to render
// inside the page-background scroll surface.
// =============================================================================

pub fn layout(content: Element) -> Element {
    // Each page's body lives inside a vertical scroll view. On web the
    // browser provides page scroll for free, but on native (iOS /
    // Android) an overflowing flex view just clips — there's no scroll
    // affordance unless we explicitly wrap the content in a scroll
    // primitive. `scroll_view` defaults to vertical, which is what
    // every page needs.
    //
    // `.safe_area(BOTTOM)` adds the device's bottom inset (Android
    // gesture bar / iOS home indicator) to the scroll content so the
    // last page section isn't sitting under the system chrome. Top is
    // handled by the navigator's toolbar, which has its own
    // status-bar inset; horizontal insets aren't an issue here
    // because the page never bleeds into them.
    let style = ScreenScroll();
    // The hamburger sits above the scroll surface so it stays pinned
    // while the page body scrolls. It renders itself reactively (only
    // when the drawer is collapsed), so on wide/pinned layouts this
    // column is just the scroll view. The outer column fills the
    // screen; the scroll view grows to take the remaining height under
    // the (conditionally-rendered) top bar.
    ui! {
        view(style = PageColumn()) {
            menu_button()
            scroll_view(style = style) { content }
                .safe_area(SafeAreaSides::BOTTOM)
        }
    }
}

// =============================================================================
// Menu button (hamburger) — consumes `ambient_drawer()` and opens the
// collapsed drawer. Renders ONLY when the viewport is narrower than the
// navigator's pin width (so the drawer is modal/off-canvas); at wide
// viewports the sidebar is pinned and no hamburger is needed.
//
// This is the documented `DrawerChrome` consumer pattern
// (`runtime_core::primitives::navigator::ambient_drawer`): screens are
// mounted as navigator *content* (not slot closures), so they reach the
// "open the drawer" action through this thread-local ambient. The
// reactive `viewport_size().get()` read inside the `ui!` `if` keeps the
// region live, so the button appears/disappears as the viewport crosses
// the pin breakpoint.
// =============================================================================

fn menu_button() -> Element {
    use runtime_core::primitives::navigator::ambient_drawer;
    use runtime_core::viewport_size;

    // No drawer chrome published (e.g. headless/non-navigator render) —
    // render nothing.
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

// The pressable glyph itself. Built with `runtime_core::pressable` (not
// a `ui!` tag — pressable has no macro tag) wrapping the Lucide `MENU`
// icon, mirroring idea-ui's `IconButton` construction.
fn hamburger(open: Rc<dyn Fn()>) -> Element {
    let glyph = runtime_core::icon(icons_lucide::MENU)
        .color(idea_ui::idea_color(|c| c.text.clone()))
        .into_element();
    runtime_core::pressable(vec![glyph], move || (open)())
        .with_style(move || runtime_core::StyleApplication::new(crate::styles::MenuButton::sheet()))
        .into_element()
}

// =============================================================================
// Sidebar — runs once at navigator init; survives screen swaps.
// =============================================================================

pub fn sidebar(slot: SlotProps, is_dark: runtime_core::Signal<bool>) -> Element {
    let body_style = SidebarBody();
    let header_style = SidebarHeader();
    let footer_style = SidebarFooter();

    let header_children: Vec<Element> = vec![
        ui! { Typography(content = "idea-ui".to_string(), kind = typography_kind::H3) },
        ui! {
            Typography(
                content = "Component reference, theming, and extension guide.".to_string(),
                muted = true,
            )
        },
    ];

    let active_route = slot.active_route;

    // The sidebar is a full-height panel that slides over (or pins
    // beside) the screen, bleeding under the status bar / Dynamic Island
    // at the top and the home indicator at the bottom. Navigators no
    // longer apply safe-area insets to slots, so the sidebar opts in
    // itself: `.safe_area(VERTICAL)` pads the top + bottom by the device
    // insets (the background still bleeds edge-to-edge — only the header
    // and footer content inset). The opt-in travels over the
    // runtime-server wire; the client resolves the real device inset.
    ui! {
        view(style = body_style) {
            view(style = header_style) { header_children }
            for section in SECTIONS {
                (!section.title.is_empty()).then(|| ui! {
                    text(style = SidebarSection()) { section.title.to_string() }
                })
                for entry in section.entries {
                    nav_link(entry.route, entry.label, active_route)
                }
            }
            Spacer()
            theme_toggle(footer_style, is_dark)
        }
        .safe_area(SafeAreaSides::VERTICAL)
    }
}

fn theme_toggle(footer_style: SidebarFooter, is_dark: runtime_core::Signal<bool>) -> Element {
    let on_change: Rc<dyn Fn(bool)> = Rc::new(move |dark| {
        is_dark.set(dark);
        if dark {
            set_idea_theme(dark_theme());
        } else {
            set_idea_theme(light_theme());
        }
    });

    let row_children: Vec<Element> = vec![ui! {
        Switch(
            label = Some("Dark mode".to_string()),
            value = is_dark,
            on_change = on_change,
        )
    }];

    ui! { view(style = footer_style) { row_children } }
}

fn nav_link(
    route: &'static runtime_core::Route<()>,
    label: &'static str,
    active_route: runtime_core::Signal<&'static str>,
) -> Element {
    let route_for_match: &'static str = route.name();
    // Container styles (padding, background, border-radius) on the
    // wrapping view; text styles (color, font) on the text. Splitting
    // is required because Android `apply_style` doesn't propagate
    // padding to `setPadding` — padding works only via Taffy shifting
    // child positions, so a text node (no children) gets zero
    // padding on native. See styles.rs for the full rationale.
    let container_style = NavLink().active(derived(move || {
        if active_route.get() == route_for_match {
            NavLinkActive::On
        } else {
            NavLinkActive::Off
        }
    }));
    let text_style = NavLinkText().active(derived(move || {
        if active_route.get() == route_for_match {
            NavLinkTextActive::On
        } else {
            NavLinkTextActive::Off
        }
    }));
    let label_text = label.to_string();

    ui! {
        link(route = route, params = ()) {
            view(style = container_style) {
                text(style = text_style) { label_text }
            }
        }
    }
}

// =============================================================================
// CodePanel — theme-aware syntax-highlighted code block. Lifted from
// the tutorial.
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
// DemoSurface — a single-purpose preview panel (no controls). Used
// when a static preview is enough.
// =============================================================================

#[derive(Default)]
pub struct DemoSurfaceProps {
    pub children: Vec<Element>,
}

#[component]
pub fn DemoSurface(props: DemoSurfaceProps) -> Element {
    let style = DemoSurfaceBox();
    let children = props.children;
    ui! {
        view(style = style) { children }
    }
}

// =============================================================================
// Demo — preview + controls in a side-by-side wrapping row. Used for
// interactive component pages where `DocControls::render_controls`
// emits the right panel.
// =============================================================================

#[derive(Default)]
pub struct DemoProps {
    pub preview: Option<Element>,
    pub controls: Option<Element>,
}

#[component]
pub fn Demo(props: DemoProps) -> Element {
    let preview = props
        .preview
        .unwrap_or_else(|| ui! { view {} });
    let controls = props
        .controls
        .unwrap_or_else(|| ui! { view {} });
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
// PropsTable — every component page documents its props with this.
// One header row + one row per prop, all rendered as flex rows so
// columns line up.
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

#[component]
pub fn PropsTable(props: PropsTableProps) -> Element {
    // Uses idea-ui's themed `Table` / `TableRow` / `TableCell`
    // components, which layer on the cross-platform `table` SDK
    // (real HTML `<table>` on web) and read header/body cell tokens
    // straight from the active theme. No local cell stylesheets
    // needed — the docs app is the consumer, not the designer.
    let mut rows: Vec<Element> = Vec::with_capacity(props.rows.len() + 1);
    rows.push(ui! {
        TableRow {
            TableCell(header = true, text = Some("Prop".to_string()))
            TableCell(header = true, text = Some("Type".to_string()))
            TableCell(header = true, text = Some("Description".to_string()))
        }
    });
    for p in props.rows {
        let name = p.name.to_string();
        let ty = p.ty.to_string();
        let desc = p.desc.to_string();
        rows.push(ui! {
            TableRow {
                TableCell(text = Some(name))
                TableCell(text = Some(ty))
                TableCell(text = Some(desc))
            }
        });
    }
    ui! { Table { rows } }
}

// =============================================================================
// Section — a labelled block inside a page. Useful for grouping
// "Examples", "Variants", "Props" under a sub-heading.
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
// ComponentPage — top-level frame for every page in the docs. Renders
// the title block and supplied body inside the padded reading column.
// =============================================================================

#[derive(Default)]
pub struct ComponentPageProps {
    pub title: String,
    pub lead: String,
    pub children: Vec<Element>,
}

#[component]
pub fn ComponentPage(props: ComponentPageProps) -> Element {
    let pad = PagePad();
    let title = props.title;
    let lead = props.lead;
    let children = props.children;
    ui! {
        view(style = pad) {
            Stack(gap = StackGap::Sm) {
                Typography(content = title, kind = typography_kind::H1)
                Typography(content = lead, kind = typography_kind::BodyLg, muted = true)
            }
            children
        }
    }
}

// =============================================================================
// Convenience: paragraph helper. Most pages render a lot of body text
// — a tiny wrapper saves the `kind = …` boilerplate.
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
pub struct H2Props {
    pub content: String,
}

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

// Silence the `Card`-import unused-warning if a page doesn't use it.
#[allow(dead_code)]
fn _force_card_import() -> Element {
    ui! { Card {} }
}
