//! Small helpers reused by every page: the page header block, the
//! syntax-highlighted code panel, the placeholder block used by
//! stubbed-out screens.

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::{switch, ui, Color, IntoPrimitive, Primitive, StyleApplication, Tokenized};
use idea_ui::{stack, typography, StackGap, TypographyKind, TypographyTone};

use crate::styles::{CodePanel, CodeText, PlaceholderBox, SectionWrap};

/// Page title block — every page calls this at the top. The wrapper
/// must be a flex-column container (here, `Stack`) so the H1 and the
/// blurb stack vertically instead of flowing as sibling inline spans.
/// A bare `View` with no flex props stays `display: block` and the
/// Typography children inherit HTML's default inline span behavior \u{2014}
/// the title and blurb end up on the same line.
pub fn page_header(title: &str, blurb: &str) -> Primitive {
    let title_text = title.to_string();
    let blurb_text = blurb.to_string();
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = title_text, kind = TypographyKind::H1) },
        ui! { Typography(content = blurb_text, kind = TypographyKind::BodyLg, tone = TypographyTone::Muted) },
    ];
    // `Md` not `Sm`: the H1 + lead-body pair is the page's most
    // important hierarchy moment, deserves a comfortable gap.
    ui! { Stack(gap = StackGap::Md) { children } }
}

/// Wrap a section's children in a `View` whose
/// `AccessibilityProps::identifier` is set to `id` (web emits this
/// as the DOM `id` attribute). The site's table-of-contents column
/// uses these ids for click-to-scroll AND for the
/// `IntersectionObserver`-driven active-link highlight.
///
/// Pairs with `shell::layout_with_toc(...)` \u{2014} the same id
/// strings the page hands to `layout_with_toc` as `TocEntry`s must
/// be the ones wrapping each section here.
pub fn page_section(id: &'static str, children: Vec<Primitive>) -> Primitive {
    let wrap_style = SectionWrap();
    let mut primitive = ui! { View(style = wrap_style) { children } };
    // Stamp the DOM `id` (web) / `accessibilityIdentifier` (iOS) /
    // resource name (Android) for the section so the TOC's
    // click-to-scroll + scroll-spy can target it. `Bound<H>`
    // doesn't expose an `accessibility(...)` chain method, so we
    // reach into the Primitive variant directly \u{2014} the enum
    // fields are public, and this is the documented escape hatch
    // for cases the builder chain doesn't cover.
    if let Primitive::View { accessibility, .. } = &mut primitive {
        accessibility.identifier = Some(id.to_string());
    }
    primitive
}

// =============================================================================
// Code panel — theme-aware syntax highlighting
// =============================================================================
//
// `idea-codeblock` stamps the per-span color into the External
// primitive's payload at construction time \u{2014} the colors don't
// re-resolve on theme change. To keep code readable in both light
// and dark modes, we wrap the codeblock in a `runtime_core::switch`
// keyed on the active theme's background luminance. When the theme
// swaps, the switch re-runs `highlight(...)` with a different
// palette and rebuilds the codeblock subtree.

#[derive(Copy, Clone)]
struct Palette {
    ink: &'static str,
    comment: &'static str,
    string: &'static str,
    accent: &'static str,
}

/// Light-theme syntax palette \u{2014} dark ink, muted warm comments,
/// teal strings, deep violet keywords. Tuned for `color-surface-alt`
/// in light mode.
const LIGHT_PALETTE: Palette = Palette {
    ink: "#1f2328",
    comment: "#8a8270",
    string: "#1f6e5f",
    accent: "#5a4fcf",
};

/// Dark-theme syntax palette \u{2014} light ink, brighter accents.
/// Tuned for `color-surface-alt` in dark mode.
const DARK_PALETTE: Palette = Palette {
    ink: "#e8eaf0",
    comment: "#9099a8",
    string: "#5eead4",
    accent: "#c4b5fd",
};

/// Heuristic: read the current `color-background` token and decide
/// whether we're in a dark theme. Idea-ui's light themes start with
/// near-white backgrounds; dark themes start with near-black. The
/// luminance check is robust against minor palette tweaks on either
/// side as long as the backgrounds remain roughly in the standard
/// light/dark zones.
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

/// Tiny three-tone Rust-ish tokenizer. Recognizes line comments,
/// strings, identifiers (with a `match` against the standard keyword
/// list), and lumps the rest as default ink. Not a real parser; just
/// enough to make a code snippet readable.
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

        // Line comment
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
        // String literal
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
        // Identifier
        if b.is_ascii_alphabetic() || b == b'_' {
            buf.push(b as char);
            i += 1;
            continue;
        }
        flush_ident(&mut buf, &mut out, &palette);
        // Lump everything non-ident as INK so layout stays exact.
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

/// Read-only code panel with light syntax tinting. The panel
/// background + border come from theme tokens; the syntax palette
/// swaps reactively when the active theme changes (via a `switch`
/// keyed on the background's luminance).
///
/// On non-web targets, `idea-codeblock` falls back to a "External
/// CodeBlockProps not supported" placeholder \u{2014} the surrounding
/// card chrome still renders.
pub fn code_panel(src: &str) -> Primitive {
    let panel_style = CodePanel();
    let src_owned = src.to_string();
    let dynamic = switch(
        theme_is_dark,
        move |&is_dark| {
            let palette = if is_dark { DARK_PALETTE } else { LIGHT_PALETTE };
            let spans = highlight(&src_owned, palette);
            let code_style = move || StyleApplication::new(CodeText::sheet());
            idea_codeblock::code_block(spans)
                .with_style(code_style)
                .into_primitive()
        },
    );
    ui! { View(style = panel_style) { dynamic } }
}

/// "Coming soon" surface used by every placeholder page. Keeps the
/// nav structure visible while signalling each route still needs
/// its real content authored.
pub fn placeholder_block(text: &str) -> Primitive {
    let style = PlaceholderBox();
    let label = text.to_string();
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = label, tone = TypographyTone::Muted) },
    ];
    ui! { View(style = style) { children } }
}
