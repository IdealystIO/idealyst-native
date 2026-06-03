//! Lower a [`MarkdownDoc`] to a flat list of styled text [`Seg`]ments.
//!
//! Shared by the native single-node handlers (iOS `NSAttributedString`,
//! Android `SpannableStringBuilder`): both render the WHOLE document as
//! one styled-text widget, so both need the identical "flatten the block
//! tree into a linear run of (text, concrete style) pairs" step. Block
//! spacing is encoded as blank-line separator segments; list markers and
//! indents are literal text; thematic rules are a dash run. This keeps
//! the two platform handlers thin and guarantees they converge on the
//! same visual structure (CLAUDE.md rule 7).

use crate::ir::{MarkdownDoc, MdBlock, MdRun, MdTheme};

/// Resolved style for one contiguous segment of uniform formatting.
pub(crate) struct SegStyle {
    pub color: String,
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub mono: bool,
    pub bg: Option<String>,
    pub underline: bool,
    pub strike: bool,
}

/// One segment: literal text + the style to paint it with.
pub(crate) struct Seg {
    pub text: String,
    pub style: SegStyle,
}

/// Indent (spaces) per list nesting level.
const LIST_INDENT_SPACES: usize = 4;
/// Width of the thematic-break dash run.
const RULE_WIDTH: usize = 24;

/// Flatten `doc` into linear styled segments.
pub(crate) fn lower(doc: &MarkdownDoc) -> Vec<Seg> {
    let t = &doc.theme;
    let mut out: Vec<Seg> = Vec::new();
    let mut first = true;

    for block in &doc.blocks {
        if !first {
            // Blank line between blocks → vertical spacing in a single
            // text widget.
            out.push(plain_seg("\n\n", &t.text, t.base_size, t));
        }
        first = false;

        match block {
            MdBlock::Heading { level, runs } => {
                let size = t.heading_size(*level);
                for r in runs {
                    out.push(run_seg(r, t, &t.heading, size, true));
                }
            }
            MdBlock::Paragraph { runs } => {
                for r in runs {
                    out.push(run_seg(r, t, &t.text, t.base_size, false));
                }
            }
            MdBlock::Quote { runs } => {
                out.push(plain_seg("\u{2502}  ", &t.muted, t.base_size, t));
                for r in runs {
                    let mut s = run_seg(r, t, &t.quote_fg, t.base_size, false);
                    s.style.italic = true;
                    out.push(s);
                }
            }
            MdBlock::CodeBlock { text } => {
                out.push(Seg {
                    text: text.clone(),
                    style: SegStyle {
                        color: t.code_fg.clone(),
                        size: t.base_size * 0.9,
                        bold: false,
                        italic: false,
                        mono: true,
                        bg: Some(t.code_bg.clone()),
                        underline: false,
                        strike: false,
                    },
                });
            }
            MdBlock::List { ordered: _, items } => {
                let mut first_item = true;
                for item in items {
                    if !first_item {
                        out.push(plain_seg("\n", &t.text, t.base_size, t));
                    }
                    first_item = false;
                    let indent = " ".repeat(item.depth as usize * LIST_INDENT_SPACES);
                    out.push(plain_seg(
                        &format!("{}{}  ", indent, item.marker),
                        &t.muted,
                        t.base_size,
                        t,
                    ));
                    for r in &item.runs {
                        out.push(run_seg(r, t, &t.text, t.base_size, false));
                    }
                }
            }
            MdBlock::Rule => {
                out.push(plain_seg(
                    &"\u{2500}".repeat(RULE_WIDTH),
                    &t.muted,
                    t.base_size,
                    t,
                ));
            }
        }
    }
    out
}

/// A run with no inline formatting (separators, markers, rules).
fn plain_seg(text: &str, color: &str, size: f32, _t: &MdTheme) -> Seg {
    Seg {
        text: text.to_string(),
        style: SegStyle {
            color: color.to_string(),
            size,
            bold: false,
            italic: false,
            mono: false,
            bg: None,
            underline: false,
            strike: false,
        },
    }
}

/// Resolve an inline [`MdRun`] against the ambient block color/size.
fn run_seg(r: &MdRun, t: &MdTheme, base: &str, size: f32, force_bold: bool) -> Seg {
    let (color, bg, mono) = if r.link.is_some() {
        (t.link.clone(), None, r.code)
    } else if r.code {
        (t.code_fg.clone(), Some(t.code_bg.clone()), true)
    } else {
        (base.to_string(), None, false)
    };
    Seg {
        text: r.text.clone(),
        style: SegStyle {
            color,
            size,
            bold: r.bold || force_bold,
            italic: r.italic,
            mono,
            bg,
            underline: r.link.is_some(),
            strike: r.strike,
        },
    }
}
