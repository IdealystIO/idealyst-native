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

use runtime_core::{component, ui, Element, IdealystSchema};

use crate::stylesheets::Spacer as SpacerStyle;

/// Props for [`Spacer`]. None — the component is configuration-free.
#[derive(Default, IdealystSchema)]
pub struct SpacerProps;

/// An empty `flex-grow: 1` item. Drop it between siblings in a row/
/// column `Stack` to push them to opposite ends without computing
/// margins.
#[component]
pub fn Spacer(_props: &SpacerProps) -> Element {
    let style = SpacerStyle();
    ui! { view(style = style) {} }
}
