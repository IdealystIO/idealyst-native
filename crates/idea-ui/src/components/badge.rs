//! `Badge` — small pill-shaped status indicator.
//!
//! ```ignore
//! ui! { Badge(label = "New", tone = BadgeTone::Primary) }
//! ```

use framework_core::{ui, Primitive};

use crate::stylesheets::Badge;
pub use crate::stylesheets::BadgeTone;

#[derive(Default)]
pub struct BadgeProps {
    pub label: String,
    pub tone: BadgeTone,
}

pub fn badge(props: &BadgeProps) -> Primitive {
    let label = props.label.clone();
    let tone = props.tone;
    let style = Badge().tone(tone);
    ui! { Text(style = style) { label } }
}
