//! `Popover` — an element-anchored overlay with no backdrop scrim.
//!
//! Typical use is for menu / dropdown / contextual UI that follows
//! a trigger element. The host owns:
//!
//! 1. A `Signal<bool>` for open/closed state.
//! 2. A `Ref<H>` on the trigger element so the popover can be
//!    anchored to it.
//!
//! ```ignore
//! let trigger: Ref<ButtonHandle> = Ref::new();
//! let open = signal!(false);
//! ui! {
//!     Pressable(
//!         label = "Options".to_string(),
//!         on_click = move || open.set(true),
//!         intent = Neutral.into_rc()
//!     ).bind(trigger)
//!     if open.get() {
//!         Popover(
//!             target = AnchorTarget::from(trigger),
//!             side = ElementSide::Below,
//!             on_dismiss = move || open.set(false)
//!         ) {
//!             Stack {
//!                 Pressable(label = "Edit".to_string(), on_click = on_edit, intent = Ghost.into_rc())
//!                 Pressable(label = "Delete".to_string(), on_click = on_delete, intent = Danger.into_rc())
//!             }
//!         }
//!     }
//! }
//! ```
//!
//! The popover has no scrim — the page behind it stays interactive.
//! Clicking outside doesn't dismiss by default; pair with a
//! click-outside listener on the host if you want that behavior.
//! Escape always dismisses (via the underlying primitive).

use std::rc::Rc;

use runtime_core::primitives::overlay::BackdropMode;
use runtime_core::primitives::portal::{AnchorTarget, ElementAlign, ElementSide};
use runtime_core::{component, ui, ChildList, Element};

use crate::stylesheets::Popover as PopoverStyle;

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct PopoverProps {
    /// The element to anchor against. Construct via
    /// `AnchorTarget::from(some_ref)` where `some_ref` is a `Ref<H>`
    /// to any primitive whose handle implements `AnchorableHandle`.
    pub target: Option<AnchorTarget>,
    /// Which side of the target the popover sits on. Default:
    /// `ElementSide::Below`.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub side: ElementSide,
    /// Alignment along the anchor's edge. Default: `ElementAlign::Start`.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub align: ElementAlign,
    /// Gap in pixels between the anchor and the popover.
    pub offset: f32,
    pub on_dismiss: Option<Rc<dyn Fn()>>,
    pub children: Vec<Element>,
}

impl Default for PopoverProps {
    fn default() -> Self {
        Self {
            target: None,
            side: ElementSide::Below,
            align: ElementAlign::Start,
            offset: 4.0,
            on_dismiss: None,
            children: Vec::new(),
        }
    }
}

#[component(children)]
pub fn Popover(props: PopoverProps) -> Element {
    let target = props
        .target
        .expect("Popover: required `target` prop missing — set it to an AnchorTarget built from a Ref");

    let surface_style = PopoverStyle();

    let mut content: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut content);
    }
    let overlay_children = vec![ui! {
        View(style = surface_style) { content }
    }];

    let mut bound = runtime_core::anchored_overlay(target, overlay_children)
        .side(props.side)
        .align(props.align)
        .offset(props.offset)
        .backdrop(BackdropMode::None)
        .trap_focus(false);
    if let Some(d) = props.on_dismiss {
        bound = bound.on_dismiss(move || (d)());
    }
    runtime_core::IntoElement::into_element(bound)
}
