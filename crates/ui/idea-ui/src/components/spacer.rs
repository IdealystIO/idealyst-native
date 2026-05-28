//! `Spacer` — an empty flex item that grows to fill available space.
//!
//! Use it inside a row `Stack` to push siblings to opposite ends
//! without computing margins:
//!
//! ```ignore
//! ui! {
//!     Stack(axis = StackAxis::Row) {
//!         Typography(content = "Title".to_string(), kind = TypographyKind::H1)
//!         Spacer()
//!         Pressable(label = "Save".to_string(), on_click = on_save)
//!     }
//! }
//! ```

use runtime_core::{ui, Element};

use crate::stylesheets::Spacer;

#[derive(Default)]
pub struct SpacerProps;

pub fn spacer(_props: &SpacerProps) -> Element {
    let style = Spacer();
    ui! { View(style = style) {} }
}
