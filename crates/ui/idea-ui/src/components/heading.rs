//! `Heading` — styled text for titles.
//!
//! Variants: `display`, `h1` (default), `h2`, `h3`. Wraps the framework's
//! `Text` primitive with the [`Heading`](crate::stylesheets::Heading)
//! stylesheet pre-applied.
//!
//! Usage:
//! ```ignore
//! ui! { Heading(content = "Welcome", kind = HeadingKind::H1) }
//! ui! { Heading(content = format!("Score: {}", score.get()), kind = HeadingKind::H2) }
//! ```
//!
//! `content` is a `String`; signals read inside the expression (the
//! `.get()` heuristic) trigger reactive updates on the underlying
//! `Text` primitive automatically.

use framework_core::{ui, Primitive};

use crate::stylesheets::Heading;
pub use crate::stylesheets::{HeadingAlign, HeadingKind};

#[derive(Default)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct HeadingProps {
    pub content: String,
    pub kind: HeadingKind,
    pub align: HeadingAlign,
}

pub fn heading(props: &HeadingProps) -> Primitive {
    let content = props.content.clone();
    let kind = props.kind;
    let align = props.align;
    let style = Heading().kind(kind).align(align);
    ui! { Text(style = style) { content } }
}
