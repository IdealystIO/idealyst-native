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

use runtime_core::{component, ui, Element};

use crate::stylesheets::Spacer as SpacerStyle;

#[derive(Default)]
pub struct SpacerProps;

#[component]
pub fn Spacer(_props: &SpacerProps) -> Element {
    let style = SpacerStyle();
    ui! { View(style = style) {} }
}
