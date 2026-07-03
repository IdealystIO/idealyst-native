//! The resolved, render-ready markdown intermediate representation.
//!
//! Parsing (`parse.rs`) lowers a CommonMark source string into a flat
//! list of [`MdBlock`]s, each carrying a sequence of styled inline
//! [`MdRun`]s. Theme resolution is carried separately as an [`MdTheme`]:
//! concrete colors + sizes the per-backend handler maps onto native
//! attribute spans. Keeping the theme separate (rather than baking a
//! color into every run) keeps the serialized payload small and makes a
//! theme toggle a one-field diff.
//!
//! Everything here is `Clone + PartialEq + Serialize + Deserialize` so
//! that (a) the `Markdown` component can key a reactive `switch` on
//! `(source, theme)` and rebuild on change, and (b) the doc crosses the
//! runtime-server wire as JSON.

use serde::{Deserialize, Serialize};

/// One inline run of text with its (boolean) inline styles. The concrete
/// color/size/font is resolved at render time from the run's flags + the
/// owning block's kind + the [`MdTheme`] — not stored per run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MdRun {
    /// The literal text of this run (already entity-decoded by the parser).
    pub text: String,
    /// Rendered bold (`**x**` / `__x__`), or part of a heading.
    pub bold: bool,
    /// Rendered italic (`*x*` / `_x_`).
    pub italic: bool,
    /// Inline code span (`` `x` ``): monospace family + code colors.
    pub code: bool,
    /// Struck through (GFM `~~x~~`).
    pub strike: bool,
    /// Part of a link (`[x](url)`): link color + underline. The
    /// destination is carried for backends that can wire taps; v1
    /// renders it visually only (see README).
    pub link: Option<String>,
}

impl MdRun {
    /// A plain run inheriting only the ambient block style.
    pub fn plain(text: impl Into<String>) -> Self {
        MdRun {
            text: text.into(),
            bold: false,
            italic: false,
            code: false,
            strike: false,
            link: None,
        }
    }
}

/// A single document-level block. The renderer emits exactly one logical
/// "paragraph" of native text per block (separated by blank lines on the
/// single-node native backends; a semantic element on web).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MdBlock {
    /// `# ..` through `###### ..`.
    Heading {
        /// Heading level, 1–6.
        level: u8,
        /// Inline content.
        runs: Vec<MdRun>,
    },
    /// A normal paragraph.
    Paragraph {
        /// Inline content.
        runs: Vec<MdRun>,
    },
    /// A fenced or indented code block. Stored as raw text (no syntax
    /// highlighting in v1 — see README); rendered monospace with the
    /// code background.
    CodeBlock {
        /// The verbatim code body (fence info string is discarded).
        text: String,
    },
    /// A block quote, flattened to inline runs (one logical line; nested
    /// paragraphs are joined with newlines).
    Quote {
        /// Inline content of the quote.
        runs: Vec<MdRun>,
    },
    /// A list. `ordered` selects bullet vs. number; each item is a single
    /// flattened line of runs plus its nesting `depth` (0 = top level).
    List {
        /// `true` for `1.`-style ordered lists, `false` for bullets.
        ordered: bool,
        /// The flattened items (nested levels included, with `depth`).
        items: Vec<MdListItem>,
    },
    /// A thematic break (`---`).
    Rule,
}

/// One list item: a flattened line of runs, its nesting depth, and the
/// resolved marker glyph (`•` or `1.`). Nested lists are expanded into
/// sibling items with a larger `depth` at parse time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MdListItem {
    /// The item's flattened inline content.
    pub runs: Vec<MdRun>,
    /// Nesting depth (0 = top level); drives indentation.
    pub depth: u8,
    /// The resolved leading marker glyph (`•` or `1.`).
    pub marker: String,
}

/// A fully-parsed, render-ready document: blocks + the theme to paint
/// them with. This is the `Element::External` payload.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarkdownDoc {
    /// The document's blocks, in order.
    pub blocks: Vec<MdBlock>,
    /// The resolved theme to paint them with.
    pub theme: MdTheme,
}

/// Per-element-type resolved styling. This is the SDK's full styling
/// surface: every color/size the renderer needs, already resolved to
/// concrete values by the author (typically derived from the app theme
/// so a light/dark toggle flows straight through — see the demo).
///
/// `PartialEq` lets the `Markdown` component detect a theme change and
/// rebuild the one native node; `f32` fields compare structurally.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MdTheme {
    /// Body text color (CSS-style string: `#rrggbb`, `rgba(...)`, …).
    pub text: String,
    /// Secondary color: list markers, thematic rules.
    pub muted: String,
    /// Heading text color.
    pub heading: String,
    /// Link text color.
    pub link: String,
    /// Inline-code / code-block foreground.
    pub code_fg: String,
    /// Inline-code / code-block background tint.
    pub code_bg: String,
    /// Block-quote foreground.
    pub quote_fg: String,
    /// Base body font size, in points/px.
    pub base_size: f32,
    /// Per-level heading size multipliers (`h1 = base * scale[0]`, …).
    pub heading_scale: [f32; 6],
    /// Optional monospace family override for code. `None` → the
    /// platform default monospace face.
    pub mono_family: Option<String>,
}

impl MdTheme {
    /// A sensible light-mode default. Authors override per app theme.
    pub fn light() -> Self {
        MdTheme {
            text: "#1f2328".into(),
            muted: "#8a8f98".into(),
            heading: "#0b0c0e".into(),
            link: "#2563eb".into(),
            code_fg: "#9a3412".into(),
            code_bg: "#f3f4f6".into(),
            quote_fg: "#57606a".into(),
            base_size: 16.0,
            heading_scale: [2.0, 1.6, 1.3, 1.12, 1.0, 0.9],
            mono_family: None,
        }
    }

    /// A sensible dark-mode default.
    pub fn dark() -> Self {
        MdTheme {
            text: "#e6edf3".into(),
            muted: "#8b949e".into(),
            heading: "#f0f6fc".into(),
            link: "#58a6ff".into(),
            code_fg: "#ffa657".into(),
            code_bg: "#161b22".into(),
            quote_fg: "#9da7b3".into(),
            base_size: 16.0,
            heading_scale: [2.0, 1.6, 1.3, 1.12, 1.0, 0.9],
            mono_family: None,
        }
    }

    /// Resolved heading size for a 1-based level (clamped to 1..=6).
    pub fn heading_size(&self, level: u8) -> f32 {
        let idx = (level.clamp(1, 6) - 1) as usize;
        self.base_size * self.heading_scale[idx]
    }
}

impl Default for MdTheme {
    fn default() -> Self {
        MdTheme::light()
    }
}
