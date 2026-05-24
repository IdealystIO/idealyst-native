//! `Modal` — a centered viewport overlay with a themed surface.
//!
//! Mostly sugar over [`runtime_core::overlay`] —
//! viewport-centered placement, dismiss-on-backdrop-click,
//! focus-trapped. Renders a themed `Card`-like surface around the
//! caller's children so a typical modal call site is just:
//!
//! ```ignore
//! let open = signal!(false);
//! ui! {
//!     Pressable(label = "Open".to_string(), on_click = move || open.set(true), intent = Primary.into_rc())
//!     if open.get() {
//!         Modal(on_dismiss = move || open.set(false)) {
//!             Heading(content = "Confirm".to_string(), kind = HeadingKind::H2)
//!             Body(content = "Are you sure?".to_string())
//!             Stack(axis = StackAxis::Row, gap = StackGap::Sm, justify = StackJustify::End) {
//!                 Pressable(
//!                     label = "Cancel".to_string(),
//!                     on_click = move || open.set(false),
//!                     intent = Neutral.into_rc()
//!                 )
//!                 Pressable(
//!                     label = "Delete".to_string(),
//!                     on_click = on_confirm,
//!                     intent = Danger.into_rc()
//!                 )
//!             }
//!         }
//!     }
//! }
//! ```
//!
//! For non-dismissable modals, pass `dismissable = false` — the
//! backdrop becomes `Opaque` (no click-dismiss, no Escape route).

use std::rc::Rc;

use runtime_core::primitives::overlay::BackdropMode;
use runtime_core::primitives::portal::ViewportPlacement;
use runtime_core::{ui, ChildList, Primitive};

use crate::stylesheets::Modal;

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct ModalProps {
    /// Fires when the user clicks outside the modal or presses
    /// Escape. The host is expected to flip its open-state signal
    /// in response — idea-ui's modal doesn't auto-unmount itself.
    pub on_dismiss: Option<Rc<dyn Fn()>>,
    /// `true` (default) lets users dismiss via backdrop or
    /// Escape. `false` switches to an opaque backdrop with no
    /// click-dismiss — useful for blocking flows that demand an
    /// explicit choice.
    pub dismissable: bool,
    pub children: Vec<Primitive>,
}

impl Default for ModalProps {
    fn default() -> Self {
        Self {
            on_dismiss: None,
            dismissable: true,
            children: Vec::new(),
        }
    }
}

pub fn modal(props: ModalProps) -> Primitive {
    let backdrop = if props.dismissable {
        BackdropMode::Dismiss
    } else {
        BackdropMode::Opaque
    };

    let on_dismiss = props.on_dismiss.clone();
    let surface_style = Modal();

    let mut content: Vec<Primitive> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut content);
    }

    let overlay_children = vec![ui! {
        View(style = surface_style) { content }
    }];

    let on_dismiss_handler: Option<Rc<dyn Fn()>> = on_dismiss;

    // Build the overlay manually since `on_dismiss` accepts
    // `Fn() + 'static` but we have an `Option<Rc<dyn Fn()>>`. The
    // `.on_dismiss(...)` builder takes a closure; we wrap the
    // optional Rc accordingly.
    let mut bound = runtime_core::overlay(overlay_children)
        .placement(ViewportPlacement::Center)
        .backdrop(backdrop);
    if let Some(d) = on_dismiss_handler {
        bound = bound.on_dismiss(move || (d)());
    }
    runtime_core::IntoPrimitive::into_primitive(bound)
}
