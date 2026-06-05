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
//! The popover has no *visible* scrim — the page behind it stays
//! visually unchanged. It dismisses two ways:
//!
//! - **Escape** (via the underlying portal's `on_dismiss`).
//! - **Outside click**: an invisible, full-bleed backdrop sits behind
//!   the surface and routes a tap *outside* the popover to `on_dismiss`
//!   (mirroring [`Modal`](super::modal::Modal)'s backdrop-press
//!   behaviour). The backdrop is fully transparent, so the page looks
//!   unscrimmed — but a tap anywhere off the surface closes the popover,
//!   the near-universal dropdown/menu UX. A tap *on* the surface lands on
//!   the surface (painted above the backdrop) and is not treated as
//!   "outside", so it doesn't dismiss.
//!
//! Note this means the page behind a popover is no longer click-through
//! while it's open — the first outside tap is consumed to dismiss it.
//! That's the standard menu behaviour and matches `Modal`. `on_dismiss`
//! is only wired to the backdrop when the host supplied it; with no
//! `on_dismiss` the backdrop is inert (an outside tap is swallowed but
//! does nothing) so an unwired popover can't silently no-op-close.

use std::rc::Rc;

use runtime_core::primitives::overlay::BackdropMode;
use runtime_core::primitives::portal::{AnchorTarget, ElementAlign, ElementSide};
use runtime_core::{
    component, ui, ChildList, Color, Element, IdealystSchema, IntoElement, Length, Position,
    StyleApplication, StyleRules, StyleSheet, Tokenized, VariantSet,
};

use crate::stylesheets::Popover as PopoverStyle;

/// A full-bleed, fully transparent backdrop sheet. It catches the
/// outside tap (so the popover can dismiss on outside-click) without
/// dimming or recoloring the page behind it — the popover stays
/// scrim-less to the eye. Absolutely positioned with zero insets so it
/// fills the portal behind the anchored surface. Mirrors `Modal`'s
/// backdrop, minus the dimming color.
fn transparent_backdrop_sheet() -> Rc<StyleSheet> {
    Rc::new(StyleSheet::new(|_vs: &VariantSet| StyleRules {
        position: Some(Position::Absolute),
        top: Some(Tokenized::Literal(Length::Px(0.0))),
        left: Some(Tokenized::Literal(Length::Px(0.0))),
        right: Some(Tokenized::Literal(Length::Px(0.0))),
        bottom: Some(Tokenized::Literal(Length::Px(0.0))),
        background: Some(Tokenized::Literal(Color("transparent".into()))),
        ..Default::default()
    }))
}

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
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
    /// Gap in pixels between the anchor and the popover. Default 4.
    #[schema(constraint = "pixels, >= 0")]
    pub offset: f32,
    /// Fires on Escape and on an outside click (a tap anywhere off the
    /// popover surface). Flip your open-state signal here. When unset,
    /// the popover can't dismiss itself — Escape and the outside tap are
    /// swallowed but do nothing.
    pub on_dismiss: Option<Rc<dyn Fn()>>,
    /// Popover surface contents.
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

/// Renders a visually scrim-less surface anchored to `target`, holding
/// the composed children. A tap outside the surface (or Escape)
/// dismisses via `on_dismiss`.
///
/// `target` is `Option` because the host gates the popover behind an
/// open-state signal and only fills the anchor `Ref` once the trigger has
/// mounted — so a popover can legitimately be built for one frame with no
/// target yet. When `target` is `None` there's nothing to anchor to, so
/// this renders an empty (no-op) element rather than panicking. The host
/// supplies a real `AnchorTarget` once the trigger is bound.
#[component(children)]
pub fn Popover(props: PopoverProps) -> Element {
    // No anchor → nothing to position against. Degrade to an empty,
    // layout-free element instead of panicking. (Regression: a `None`
    // target used to `.expect()` and crash — see the `none_target_*`
    // tests below.)
    let Some(target) = props.target else {
        return runtime_core::view(Vec::new()).into_element();
    };

    let surface_style = PopoverStyle();

    let mut content: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut content);
    }
    let overlay_children = vec![ui! {
        view(style = surface_style) { content }
    }];

    // Outside-click dismissal: `BackdropMode::Dismiss` renders a
    // full-bleed pressable *behind* the surface that routes its tap to
    // `on_dismiss`. We override the default dimming scrim with a fully
    // transparent sheet so the page behind looks unscrimmed (the
    // popover's hallmark) while the backdrop still catches the outside
    // tap. A tap on the surface itself lands on the surface (painted
    // above the backdrop) and never reaches the backdrop, so it doesn't
    // dismiss. This mirrors `Modal`'s backdrop-press path.
    let mut bound = runtime_core::anchored_overlay(target, overlay_children)
        .side(props.side)
        .align(props.align)
        .offset(props.offset)
        .backdrop(BackdropMode::Dismiss)
        .backdrop_style(StyleApplication::new(transparent_backdrop_sheet()))
        .trap_focus(false);
    if let Some(d) = props.on_dismiss {
        bound = bound.on_dismiss(move || (d)());
    }
    runtime_core::IntoElement::into_element(bound)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: building a `Popover` with `target: None` used to
    /// `.expect()` and panic. A host gates the popover behind an
    /// open-state signal and fills the anchor `Ref` only after the
    /// trigger mounts, so a `None` target is a legitimate transient
    /// state. It must degrade to a harmless empty element, not crash.
    #[test]
    fn none_target_does_not_panic() {
        // Would have panicked on the old `.expect(...)`.
        let el = Popover(PopoverProps {
            target: None,
            children: vec![runtime_core::text("hi").into_element()],
            ..Default::default()
        });
        // Degenerate output: a plain (childless) view, never a Portal —
        // there's nothing to anchor so nothing is rendered.
        assert!(
            matches!(el, Element::View { .. }),
            "a None-target Popover must render an empty View, not panic or build a Portal"
        );
    }

    /// Regression: an outside click dismisses. The composition relies on
    /// a backdrop layer behind the surface (a `Pressable` wired to
    /// `on_dismiss`) — the same shape `Modal` uses. If a refactor drops
    /// the backdrop (e.g. reverts to `BackdropMode::None`), outside-click
    /// dismissal silently disappears and this test fails.
    #[test]
    fn outside_click_backdrop_is_present_behind_surface() {
        use runtime_core::primitives::portal::AnchorTarget;
        use runtime_core::{PressableHandle, Ref};

        let trigger: Ref<PressableHandle> = Ref::new();
        let el = Popover(PopoverProps {
            target: Some(AnchorTarget::from(trigger)),
            children: vec![runtime_core::text("body").into_element()],
            ..Default::default()
        });

        let portal_children = match &el {
            Element::Portal { children, .. } => children,
            _ => panic!("a targeted Popover should build a Portal"),
        };
        // anchored_overlay lowers to [backdrop, content_view] when the
        // backdrop is not `None`. The first child must be the
        // tap-catching backdrop pressable.
        assert_eq!(
            portal_children.len(),
            2,
            "Popover must have a backdrop layer behind its surface for outside-click dismissal"
        );
        assert!(
            matches!(portal_children[0], Element::Pressable { .. }),
            "the first portal child must be the backdrop Pressable that catches the outside tap"
        );
    }
}
