//! `Spacer` — an empty flex item that grows to fill available space.
//!
//! Use it inside a row `Stack` to push siblings to opposite ends
//! without computing margins:
//!
//! ```ignore
//! ui! {
//!     Stack(axis = StackAxis::Row) {
//!         Heading(content = "Title".to_string())
//!         Spacer()
//!         Pressable(label = "Save".to_string(), on_click = on_save)
//!     }
//! }
//! ```

use framework_core::{ui, Primitive};

use crate::stylesheets::Spacer;

#[derive(Default)]
pub struct SpacerProps;

pub fn spacer(_props: &SpacerProps) -> Primitive {
    let style = Spacer();
    ui! { View(style = style) {} }
}
