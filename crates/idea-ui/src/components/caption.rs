//! `Caption` — small muted helper text.

use framework_core::{ui, Primitive};

use crate::stylesheets::Caption;
pub use crate::stylesheets::{CaptionAlign, CaptionTone};

#[derive(Default)]
pub struct CaptionProps {
    pub content: String,
    pub tone: CaptionTone,
    pub align: CaptionAlign,
}

pub fn caption(props: &CaptionProps) -> Primitive {
    let content = props.content.clone();
    let tone = props.tone;
    let align = props.align;
    let style = Caption().tone(tone).align(align);
    ui! { Text(style = style) { content } }
}
