//! `Body` — paragraph / body text.
//!
//! ```ignore
//! ui! { Body(content = "Hello", tone = BodyTone::Muted) }
//! ```

use framework_core::{ui, Primitive};

use crate::stylesheets::Body;
pub use crate::stylesheets::{BodyAlign, BodyTone};

#[derive(Default)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct BodyProps {
    pub content: String,
    pub tone: BodyTone,
    pub align: BodyAlign,
}

pub fn body(props: &BodyProps) -> Primitive {
    let content = props.content.clone();
    let tone = props.tone;
    let align = props.align;
    let style = Body().tone(tone).align(align);
    ui! { Text(style = style) { content } }
}
