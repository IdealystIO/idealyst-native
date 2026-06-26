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

use runtime_core::primitives::overlay::{overlay, BackdropMode};
use runtime_core::primitives::portal::{AnchorTarget, ElementAlign, ElementSide, ViewportPlacement};
use runtime_core::{
    component, ui, ChildList, Color, Element, IdealystSchema, IntoElement, Length, Position,
    Reactive, StyleApplication, StyleRules, StyleSheet, Tokenized, VariantSet,
};

use crate::stylesheets::Popover as PopoverStyle;

/// A full-bleed, fully transparent backdrop sheet. It catches the
/// outside tap (so the popover can dismiss on outside-click) without
/// dimming or recoloring the page behind it — the popover stays
/// scrim-less to the eye. Absolutely positioned with zero insets so it
/// fills the portal behind the anchored surface. Mirrors `Modal`'s
/// backdrop, minus the dimming color.
/// A layout-neutral (out-of-flow) wrapper sheet for the
/// `view(vec![catcher, anchored])` the Popover returns. Both children are
/// portals that render at the viewport root, so the wrapper holds no
/// visible content — but as a plain in-flow `view` it would still be a
/// flex item, so a gapped/centered parent would *shift its siblings* when
/// the popover mounts/unmounts (the "the popover button moves on open/close"
/// report). `position: absolute` takes the wrapper out of flow entirely, so
/// the trigger stays put. (Same fix the `if`-without-else macro lowering
/// applies to its empty branch.)
fn out_of_flow_wrapper_sheet() -> Rc<StyleSheet> {
    Rc::new(StyleSheet::new(|_vs: &VariantSet| StyleRules {
        position: Some(Position::Absolute),
        ..Default::default()
    }))
}

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

// Reactive-by-default: `#[props]` wraps the scalar data props (`side`/`align`
// enums, `offset`) → `Reactive<…>`. `target` is an anchor handle
// (`#[prop(static)]`, ref-like — like `Ref`), `on_dismiss` is an `Rc` handler
// (auto-skipped), and `children` is a `Vec<Element>` (auto-skipped).
#[runtime_core::props]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct PopoverProps {
    /// The element to anchor against. Construct via
    /// `AnchorTarget::from(some_ref)` where `some_ref` is a `Ref<H>`
    /// to any primitive whose handle implements `AnchorableHandle`.
    #[prop(static)]
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
            side: Reactive::Static(ElementSide::Below),
            align: Reactive::Static(ElementAlign::Start),
            offset: Reactive::Static(4.0),
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
    let surface = ui! {
        view(style = surface_style) { content }
    };

    // Outside-click dismissal: a FULLSCREEN, transparent dismiss catcher
    // rendered *behind* the anchored surface. An anchored overlay's own
    // `Dismiss` backdrop only fills its small anchored box (the portal is
    // positioned at the trigger; there's no `position: fixed` to escape
    // it), so a click truly *away* from the surface misses it and the
    // popover never closes. A fullscreen `overlay()` portal IS
    // viewport-sized, so its transparent Dismiss backdrop catches every
    // outside click and fires `on_dismiss`. A tap *on* the surface lands on
    // the surface (rendered after the catcher, so above it) and never
    // reaches the catcher, so it doesn't dismiss. (Same fix as `Select`.)
    let dismiss = props.on_dismiss;
    let catcher = {
        let mut c = overlay(Vec::new())
            // FullScreen so the portal (and its inset-0 backdrop) is
            // viewport-sized — the default Center placement would size the
            // portal to its (empty) content, collapsing the catcher to 0×0.
            .placement(ViewportPlacement::FullScreen)
            .backdrop(BackdropMode::Dismiss)
            .backdrop_style(StyleApplication::new(transparent_backdrop_sheet()));
        if let Some(d) = dismiss.clone() {
            c = c.on_dismiss(move || (d)());
        }
        c.into_element()
    };

    // The anchored surface sits ABOVE the catcher (same layer, rendered
    // after). Its own backdrop is None — the catcher owns outside-click
    // dismissal; Escape still closes via this `on_dismiss`.
    // TODO(reactive-sweep): route `side`/`align`/`offset` reactively into the
    // `anchored_overlay` placement. They're consumed by value as builder args
    // (`.side()/.align()/.offset()`) that drive STRUCTURE (portal anchoring),
    // not a style closure — a live signal would need the anchored overlay
    // rebuilt on change. The host gates the popover behind an open-signal and
    // rebuilds it on toggle, so a value change between opens is picked up.
    let anchored = {
        let mut a = runtime_core::anchored_overlay(target, vec![surface])
            .side(props.side.get())
            .align(props.align.get())
            .offset(props.offset.get())
            .backdrop(BackdropMode::None)
            .trap_focus(false);
        if let Some(d) = dismiss {
            a = a.on_dismiss(move || (d)());
        }
        a.into_element()
    };

    // Out-of-flow wrapper: both children are viewport portals, so the
    // wrapper must not occupy a flex slot or it shifts the trigger's
    // siblings on open/close (see `out_of_flow_wrapper_sheet`).
    runtime_core::view(vec![catcher, anchored])
        .with_style(out_of_flow_wrapper_sheet())
        .into_element()
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
        // Degenerate output: a plain (childless) view — there's nothing to
        // anchor so nothing is rendered. A *targeted* popover is a View too
        // (catcher + anchored), so distinguish on child count: None = empty.
        match el {
            Element::View { children, .. } => assert!(
                children.is_empty(),
                "a None-target Popover must render an EMPTY View (nothing to anchor)"
            ),
            _ => panic!("a None-target Popover must render an empty View, not panic / build a Portal"),
        }
    }

    /// Regression: an outside click dismisses. Like `Select`, the popover
    /// composes a FULLSCREEN dismiss catcher (a viewport-sized `overlay()`
    /// portal whose transparent backdrop is a `Pressable` wired to
    /// `on_dismiss`) *behind* the anchored surface. An anchored overlay's
    /// own backdrop only fills its small anchored box, so a click away from
    /// the surface would miss it — the fullscreen catcher is what makes
    /// click-away actually close. If a refactor drops the catcher (or
    /// shrinks it from FullScreen), outside-click dismissal silently
    /// disappears and this test fails.
    #[test]
    fn outside_click_uses_fullscreen_catcher_behind_surface() {
        use runtime_core::primitives::portal::{AnchorTarget, PortalTarget};
        use runtime_core::{PressableHandle, Ref};

        let trigger: Ref<PressableHandle> = Ref::new();
        let el = Popover(PopoverProps {
            target: Some(AnchorTarget::from(trigger)),
            children: vec![runtime_core::text("body").into_element()],
            ..Default::default()
        });

        // Top level: a View wrapping [catcher, anchored].
        let kids = match &el {
            Element::View { children, .. } => children,
            _ => panic!("a targeted Popover should wrap [catcher, anchored] in a View"),
        };
        assert_eq!(kids.len(), 2, "Popover = fullscreen catcher + anchored surface");

        // child[0]: the fullscreen catcher portal. Its backdrop pressable is
        // the first portal child, and its target is the FullScreen viewport.
        match &kids[0] {
            Element::Portal { children, target, .. } => {
                assert!(
                    matches!(target, PortalTarget::Viewport(ViewportPlacement::FullScreen)),
                    "the catcher must be a FULLSCREEN viewport portal so its backdrop covers the page"
                );
                assert!(
                    matches!(children.first(), Some(Element::Pressable { .. })),
                    "the catcher's first child must be the tap-catching backdrop Pressable"
                );
            }
            _ => panic!("Popover's first child must be the fullscreen catcher Portal"),
        }

        // child[1]: the anchored surface portal (backdrop None → no catcher
        // pressable of its own; the surface view is its content).
        assert!(
            matches!(&kids[1], Element::Portal { .. }),
            "Popover's second child must be the anchored surface Portal"
        );
    }
}
