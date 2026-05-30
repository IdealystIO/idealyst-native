//! Shared page chrome — `PageHeader`, `PageSection`, `Section`,
//! `CodeBlock`, `CodePanel`, `DemoShowcase`, `PlaceholderBlock`. Each is
//! a `#[component]` so its tag and props struct wire into `ui!` dispatch
//! automatically (the macro emits `pub type Foo = FooProps` + the
//! `BuildElement` impl).

use runtime_core::{
    component, switch, ui, Color, Element, IntoElement, Ref, StyleApplication, Tokenized,
    ViewHandle,
};
use idea_ui::{typography_kind, Stack, StackGap, Typography};

use crate::styles::{
    CodePanel as CodePanelStyle,
    CodeText, PlaceholderBox, SectionWrap, ShowcaseCard, ShowcaseCode, ShowcaseDemo,
};

// =============================================================================
// PageHeader — H1 + lead paragraph at the top of every page.
// =============================================================================

#[derive(Default)]
pub struct PageHeaderProps {
    pub title: String,
    pub blurb: String,
}

/// Page title block. The wrapper is a flex-column `Stack` so the H1 and
/// the lead stack vertically; a bare `View` with no flex props stays
/// `display: block` and the Typography children would flow inline.
#[component]
pub fn PageHeader(props: &PageHeaderProps) -> Element {
    let title = props.title.clone();
    let blurb = props.blurb.clone();
    // `Md` not `Sm`: the H1 + lead-body pair is the page's most important
    // hierarchy moment, deserves a comfortable gap.
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = title, kind = typography_kind::H1)
            Typography(content = blurb, kind = typography_kind::BodyLg, muted = true)
        }
    }
}

// =============================================================================
// PageSection — TOC-anchored container that binds a `Ref<ViewHandle>`.
// =============================================================================

#[derive(Default)]
pub struct PageSectionProps {
    pub handle: Ref<ViewHandle>,
    pub children: Vec<Element>,
}

/// Wrap a section's children in a `View` bound to `handle`. The site's
/// table-of-contents column reads each section's
/// `ViewHandle::absolute_frame()` to drive the active-link highlight
/// and to compute the click-to-scroll target. Pairs with
/// `shell::layout_with_toc(...)` and the matching `TocEntry::handle`.
///
/// `Ref<H>` is `Copy`, so the same handle threads through both the
/// `TocEntry` list and this section's `handle` prop without ceremony.
#[component]
pub fn PageSection(props: PageSectionProps) -> Element {
    let style = SectionWrap();
    let handle = props.handle;
    let children = props.children;
    ui! { view(style = style) { children }.bind(handle) }
}

// =============================================================================
// Code panel — theme-aware syntax highlighting
// =============================================================================
//
// `idea-codeblock` stamps the per-span color into the External primitive's
// payload at construction time — the colors don't re-resolve on theme
// change. So we wrap the codeblock in a `runtime_core::switch` keyed on
// the active theme's background luminance: a theme swap re-runs
// `highlight(..)` with a different palette and rebuilds the codeblock.

#[derive(Copy, Clone)]
struct Palette {
    ink: &'static str,
    comment: &'static str,
    string: &'static str,
    accent: &'static str,
}

/// Light-theme syntax palette — dark ink, muted warm comments, teal
/// strings, deep violet keywords. Tuned for `color-surface-alt` in
/// light mode.
const LIGHT_PALETTE: Palette = Palette {
    ink: "#1f2328",
    comment: "#8a8270",
    string: "#1f6e5f",
    accent: "#5a4fcf",
};

/// Dark-theme syntax palette — light ink, brighter accents. Tuned for
/// `color-surface-alt` in dark mode.
const DARK_PALETTE: Palette = Palette {
    ink: "#e8eaf0",
    comment: "#9099a8",
    string: "#5eead4",
    accent: "#c4b5fd",
};

/// Read the current `color-background` token and decide whether we're in
/// a dark theme. Idea-ui's light themes start with near-white
/// backgrounds; dark themes start with near-black. The luminance check
/// is robust against minor palette tweaks on either side as long as the
/// backgrounds remain roughly in the standard light/dark zones.
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
    // BT.709 luma. Values 0..=255 — threshold at the midpoint.
    let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    luma < 128.0
}

/// Tiny three-tone Rust-ish tokenizer. Recognizes line comments, strings,
/// identifiers (with a `match` against the standard keyword list), and
/// lumps the rest as default ink. Not a real parser; just enough to make
/// a code snippet readable.
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

/// The theme-aware highlighted code block, without any surrounding chrome.
/// The syntax palette swaps reactively when the active theme changes (via
/// a `switch` keyed on the background's luminance). Callers supply the
/// surrounding surface (`CodePanel` for standalone code, `DemoShowcase`
/// for the showcase card's code region).
///
// =============================================================================
// CodeBlock — theme-aware highlighted code block, no surrounding chrome.
// Composed inside `CodePanel` (standalone snippet on a surface) and
// `DemoShowcase` (the code region of the live-demo card). Promoted to a
// `#[component]` rather than a snake_case helper because it has a prop
// and is called from more than one site (CLAUDE.md §9.5).
// =============================================================================

#[derive(Default)]
pub struct CodeBlockProps {
    pub src: String,
}

/// On non-web targets, `idea-codeblock` falls back to a placeholder — the
/// surrounding chrome still renders.
#[component]
pub fn CodeBlock(props: CodeBlockProps) -> Element {
    let src_owned = props.src;
    switch(theme_is_dark, move |&is_dark| {
        let palette = if is_dark { DARK_PALETTE } else { LIGHT_PALETTE };
        let spans = highlight(&src_owned, palette);
        let code_style = move || StyleApplication::new(CodeText::sheet());
        idea_codeblock::code_block(spans)
            .with_style(code_style)
            .into_element()
    })
}

// =============================================================================
// CodePanel — read-only standalone code block on its own surface.
// =============================================================================

#[derive(Default)]
pub struct CodePanelProps {
    pub src: String,
}

/// Standalone code panel on a `color-surface-alt` surface — used for
/// freestanding snippets (quickstart, concepts, …). Demo sections use
/// [`DemoShowcase`] instead, which owns its code region's surface itself.
#[component]
pub fn CodePanel(props: CodePanelProps) -> Element {
    let panel_style = CodePanelStyle();
    let src = props.src;
    ui! {
        view(style = panel_style) {
            CodeBlock(src = src)
        }
    }
}

// =============================================================================
// Section — H2 + paragraph stack + optional code panel. The shape every
// detail-page section was rebuilding from scratch as a snake_case
// `section(title, paragraphs, code)` helper. Promoted to a shared
// `#[component]` so per-page copies all go away (CLAUDE.md §9.5).
// =============================================================================

#[derive(Default)]
pub struct SectionProps {
    pub title: String,
    pub paragraphs: Vec<String>,
    pub code: Option<String>,
}

/// Renders an H2, then each paragraph as a `Typography`, then optionally
/// a `CodePanel` underneath — wrapped in a `Stack` with `Lg` gaps. Used
/// inside a `PageSection` body to keep the prose + code anchoring
/// consistent across the marketing pages.
#[component]
pub fn Section(props: SectionProps) -> Element {
    let title = props.title;
    let paragraphs = props.paragraphs;
    let code = props.code;
    ui! {
        Stack(gap = StackGap::Lg) {
            Typography(content = title, kind = typography_kind::H2)
            for paragraph in paragraphs {
                Typography(content = paragraph)
            }
            if let Some(src) = code {
                CodePanel(src = src)
            }
        }
    }
}

// =============================================================================
// DemoShowcase — live preview + source, stacked in a single card.
// =============================================================================

#[derive(Default)]
pub struct DemoShowcaseProps {
    pub source: String,
    /// The interactive preview content (running widget + controls). Sits
    /// on the clean `color-surface` top half of the card.
    pub children: Vec<Element>,
}

/// A live demo and its source, stacked inside one card with a clear color
/// split — the reusable building block for every code-backed demo section.
/// Children fill the top demo region; `source` renders below in the tinted
/// code region.
///
/// The card is two color regions: a clean `color-surface` preview area on
/// top and a tinted `color-surface-alt` code area below, divided by a
/// hairline border (see `ShowcaseCard` / `ShowcaseDemo` / `ShowcaseCode`).
/// Demo-on-top / code-below stacks rather than going side by side because
/// the body column isn't wide enough for two readable panes.
#[component]
pub fn DemoShowcase(props: DemoShowcaseProps) -> Element {
    let card_style = ShowcaseCard();
    let demo_style = ShowcaseDemo();
    let code_style = ShowcaseCode();
    let preview = props.children;
    let source = props.source;
    ui! {
        view(style = card_style) {
            view(style = demo_style) { preview }
            view(style = code_style) {
                CodeBlock(src = source)
            }
        }
    }
}

// =============================================================================
// PlaceholderBlock — "coming soon" surface for stub pages.
// =============================================================================

#[derive(Default)]
pub struct PlaceholderBlockProps {
    pub text: String,
}

/// "Coming soon" surface used by every placeholder page. Keeps the nav
/// structure visible while signalling each route still needs its real
/// content authored.
#[component]
pub fn PlaceholderBlock(props: &PlaceholderBlockProps) -> Element {
    let style = PlaceholderBox();
    let label = props.text.clone();
    ui! {
        view(style = style) {
            Typography(content = label, muted = true)
        }
    }
}

// =============================================================================
// Backward-compat shims — thin wrappers around the components above so
// page files compile while their call sites migrate one component at a
// time. Each shim disappears once all of its callers have been
// converted.
// =============================================================================

