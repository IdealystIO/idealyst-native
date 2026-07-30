//! Tutorial chrome components — `CodePanel`, `Callout`, `DocsLink`,
//! `StepNav`, `LessonPage`. Each is a `#[component]` so its tag and props
//! struct are wired into `ui!` dispatch automatically (the macro emits
//! `pub type Foo = FooProps` + the matching `BuildElement` impl).
//!
//! Author paragraphs and sub-headings as `Typography(content = …, kind
//! = …)` directly — there's no point hiding it behind a wrapper.

use runtime_core::{
    component, switch, ui, Color, Element, IntoElement, Route, StyleApplication, Tokenized,
};
// `IdeaTheme as _` brings the theme trait into scope so `theme_is_dark`
// below can call `colors()` on the downcast `IdeaThemeRef` carrier.
use idea_ui::{IdeaTheme as _, Stack, StackGap, Typography};

use crate::routes;
use crate::styles::{
    Callout as CalloutBox,
    CodePanel as CodePanelBox,
    CodeText,
    DocsLink as DocsLinkBox,
    PagePad,
    StepNavLink,
    StepNavRow,
};

/// GitHub blob base for the deep-dive reference docs. The tutorial
/// teaches the concept; these links point at the verbose reference.
const DOCS_BASE: &str = "https://github.com/IdealystIO/idealyst-native/blob/master/docs/";

// =============================================================================
// CodePanel — theme-aware, syntax-tinted code block.
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

/// Read the active theme's background and decide whether we're in a dark
/// theme. idea-ui's light themes start with near-white backgrounds; dark
/// themes with near-black, so a luminance check survives palette tweaks on
/// either side.
///
/// Reads via [`idea_ui::active_theme`] (a tracked signal read) rather than
/// `Tokenized::resolve()`: the Tokenized registry read neither updates on
/// `set_idea_theme` nor subscribes the `switch` scrutinee below (Tokenized
/// freshness is a documented runtime-v2 deferral), which would leave dark
/// code panels wearing the light palette's ink.
fn theme_is_dark() -> bool {
    if idea_ui::theme_installed() {
        let theme = idea_ui::active_theme();
        if let Some(t) = theme.downcast_ref::<idea_ui::IdeaThemeRef>() {
            return is_dark_color(&t.colors().background.value().0);
        }
    }
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
    // `children` is a bare identifier (no braces). The `ui!` parser
    // treats it as a Rust expression in child position and feeds it
    // through `ChildList::append_to`, so the caller's body splats in
    // beside the label. Wrapping it as `{ children }` would instead be
    // consumed as the preceding component's child block.
    ui! {
        view(style = style) {
            Typography(content = label, kind = idea_ui::typography_kind::Overline)
            children
        }
    }
}

// =============================================================================
// DocsLink — "read more in the docs" cross-link card.
// =============================================================================

#[derive(Default)]
pub struct DocsLinkProps {
    pub summary: String,
    pub link_label: String,
    pub doc_file: String,
}

#[component]
pub fn DocsLink(props: &DocsLinkProps) -> Element {
    let style = CalloutBox();
    let url = format!("{DOCS_BASE}{}", props.doc_file);
    let link_style = move || StyleApplication::new(DocsLinkBox::sheet());
    let link_text = format!("{} \u{2197}", props.link_label);
    let summary = props.summary.clone();
    ui! {
        view(style = style) {
            Typography(content = summary, muted = true)
            link(external = url) { text(style = link_style) { link_text } }
        }
    }
}

// =============================================================================
// StepNav — prev / next bar derived from the linear step order in `routes`.
// =============================================================================

#[derive(Default)]
pub struct StepNavProps {
    pub current: &'static str,
}

fn step_link(route: &'static Route<()>, label: String) -> Element {
    let style = move || StyleApplication::new(StepNavLink::sheet());
    ui! {
        link(route = route, params = ()) {
            text(style = style) { label }
        }
    }
}

#[component]
pub fn StepNav(props: &StepNavProps) -> Element {
    let (prev, next) = routes::neighbors(props.current);
    let row = StepNavRow();
    // Each side resolves to a link OR an empty view — deliberately not an
    // in-macro `if`, whose empty branch is laid out `position: absolute`.
    // The row is `justify-content: space-between`, so an absolute (i.e.
    // out-of-flow) placeholder would let a lone "next →" drift to the left
    // edge on the first and last steps. The empty view keeps the slot.
    let prev_el = match prev {
        Some((r, l)) => step_link(r, format!("\u{2190} {l}")),
        None => runtime_core::view(Vec::<Element>::new()).into_element(),
    };
    let next_el = match next {
        Some((r, l)) => step_link(r, format!("{l} \u{2192}")),
        None => runtime_core::view(Vec::<Element>::new()).into_element(),
    };
    ui! {
        view(style = row) {
            prev_el
            next_el
        }
    }
}

// =============================================================================
// LessonPage — top-level frame for a step. Renders the title block, the
// supplied body as children, and the prev/next bar, all inside the padded
// reading column. By-value so it can move children out.
// =============================================================================

#[derive(Default)]
pub struct LessonPageProps {
    pub current: &'static str,
    pub title: String,
    pub lead: String,
    pub children: Vec<Element>,
}

#[component]
pub fn LessonPage(props: LessonPageProps) -> Element {
    let pad = PagePad();
    let title = props.title;
    let lead = props.lead;
    let current = props.current;
    let children = props.children;
    ui! {
        view(style = pad) {
            Stack(gap = StackGap::Sm) {
                Typography(content = title, kind = idea_ui::typography_kind::H1)
                Typography(content = lead, kind = idea_ui::typography_kind::BodyLg, muted = true)
            }
            children
            StepNav(current = current)
        }
    }
}

