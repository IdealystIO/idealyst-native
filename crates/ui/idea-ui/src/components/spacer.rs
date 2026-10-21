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

use runtime_core::{component, component_styles, ui, Element, IdealystSchema};

use crate::theme::IdeaThemeRef;

component_styles! {
    /// The styled sections of [`Spacer`].
    pub Spacer<IdeaThemeRef> {
        slot root {
            base(_t) {
                flex_grow: 1.0,
            }
        }
    }
}

/// Props for [`Spacer`]: configuration-free beyond the per-instance
/// `styles` override. `#[props]` is also what lets remote code use the
/// app's `Spacer`.
#[runtime_core::props(styles = SpacerStyles)]
#[derive(Default, IdealystSchema)]
pub struct SpacerProps;

/// An empty `flex-grow: 1` item. Drop it between siblings in a row/
/// column `Stack` to push them to opposite ends without computing
/// margins.
#[component]
pub fn Spacer(props: &SpacerProps) -> Element {
    let style = SpacerStyles::root().overridden(props.styles.root.as_ref());
    ui! { view(style = style) {} }
}
