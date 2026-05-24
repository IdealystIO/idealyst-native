//! `Card` — surface container with padding, radius, and optional
//! elevation. The most common building block for grouping related
//! content.
//!
//! ```ignore
//! ui! {
//!     Card(tone = CardTone::Elevated) {
//!         Heading(content = "Stats", kind = HeadingKind::H2)
//!         Body(content = "Today's activity")
//!     }
//! }
//! ```

use runtime_core::{ui, ChildList, Primitive};

use crate::stylesheets::Card;
pub use crate::stylesheets::{CardPadding, CardTone};

#[derive(Default)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct CardProps {
    pub tone: CardTone,
    pub padding: CardPadding,
    pub children: Vec<Primitive>,
}

pub fn card(props: CardProps) -> Primitive {
    let tone = props.tone;
    let padding = props.padding;
    let style = Card().tone(tone).padding(padding);
    let mut children: Vec<Primitive> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut children);
    }
    ui! { View(style = style) { children } }
}
