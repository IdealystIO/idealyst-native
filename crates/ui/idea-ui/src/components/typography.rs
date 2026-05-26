//! `Typography` — the single component for every kind of text on a
//! page.
//!
//! Replaces the older `Heading` / `Body` / `Caption` split. One
//! component, three axes:
//!
//! - `kind` (the `TypographyKind` axis from the
//!   [`Typography`](crate::stylesheets::Typography) stylesheet) —
//!   picks the size + weight + spacing for the variant:
//!   `Display`, `H1`, `H2`, `H3`, `BodyXl`, `BodyLg`, `Body`
//!   (default), `BodySm`, `Caption`, `Overline`.
//! - `tone` — picks the color: `Default`, `Muted`, `Primary`,
//!   `Danger`, `Success`, `Warning`, `Info`, `Inverse`.
//! - `align` — `Start` (default), `Center`, `End`.
//!
//! Per-variant sizes are theme-tokenized (one `typography-{kind}-size`
//! token per variant) so apps can retune the type scale by overriding
//! the corresponding fields in [`Typography`](crate::theme::Typography).
//! Weight, line height, and letter spacing are encoded per-variant in
//! the stylesheet — apps that need to override those re-implement the
//! `Typography` stylesheet block on their own theme struct (or wait
//! for the per-property tokenization tier).
//!
//! Usage:
//!
//! ```ignore
//! ui! { Typography(content = "Welcome".into(), kind = TypographyKind::Display) }
//! ui! { Typography(content = paragraph_text(), tone = TypographyTone::Muted) }  // default kind = Body
//! ui! { Typography(content = "Section".into(), kind = TypographyKind::Overline) }
//! ```

use runtime_core::{ui, Primitive};

use crate::stylesheets::Typography;
pub use crate::stylesheets::{TypographyAlign, TypographyKind, TypographyTone};

#[derive(Default)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct TypographyProps {
    pub content: String,
    pub kind: TypographyKind,
    pub tone: TypographyTone,
    pub align: TypographyAlign,
}

pub fn typography(props: &TypographyProps) -> Primitive {
    let content = props.content.clone();
    let kind = props.kind;
    let tone = props.tone;
    let align = props.align;
    let style = Typography().kind(kind).tone(tone).align(align);
    ui! { Text(style = style) { content } }
}
