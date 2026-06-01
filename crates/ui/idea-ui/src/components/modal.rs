//! `Modal` — a centered viewport overlay with a themed surface.
//!
//! Sugar over [`runtime_core::overlay`]: one fullscreen portal holding a
//! dimming backdrop and a centered, themed `Card`-like surface. A typical
//! call site is:
//!
//! ```ignore
//! let open = signal!(false);
//! ui! {
//!     Pressable(label = "Open".to_string(), on_click = move || open.set(true), intent = Primary.into_rc())
//!     if open.get() {
//!         Modal(on_dismiss = move || open.set(false)) {
//!             Typography(content = "Confirm".to_string(), kind = TypographyKind::H2)
//!             Typography(content = "Are you sure?".to_string())
//!         }
//!     }
//! }
//! ```
//!
//! ## One portal, two layered children
//! A single fullscreen portal whose content is `[backdrop, card]` as
//! siblings — the backdrop is an absolutely-positioned full-bleed layer
//! behind a flex-centered card. (Two separate portals layer fine on web/iOS
//! but break on Android, where each portal is its own touch-modal Dialog
//! window and the scrim window never gets the outside tap.) The card is
//! content-sized, so a tap outside it lands on the backdrop on every
//! backend. The card is `position: relative` so it paints above the
//! absolutely-positioned backdrop on web (where positioned elements stack
//! above in-flow siblings regardless of DOM order).
//!
//! ## Enter animation
//! Driven by [`runtime_core::AnimatedValue`] bound to the layers'
//! `ViewHandle`s (this reliably animates portal content on every backend —
//! `presence` only animates its direct child, which for a portal is the
//! escaped placeholder, so it no-ops on iOS). The **backdrop fades** in
//! (opacity only — no slide, so there's no hard dark edge sweeping across),
//! while the **card fades and slides** up a few DIPs. Opacity/translate are
//! used rather than scale, which raced with layout and stuttered.
//!
//! Exit is not yet animated here — a bare `if open { Modal }` unmounts
//! instantly. A delayed-unmount exit is tracked separately.
//!
//! ## Backdrop press is a handler, not a hardcoded close
//! [`ModalProps::on_backdrop_press`] fires when the backdrop is tapped.
//! Unset → falls back to [`ModalProps::on_dismiss`] *if* `dismissable`, and
//! to nothing when non-dismissable. Set it to intercept the tap without
//! assuming dismissal.
//!
//! ## Width
//! [`ModalProps::width`] is the surface's desired width on a roomy viewport;
//! it is capped to the viewport width (minus a margin) reactively so the
//! surface never overflows a phone.

use std::rc::Rc;
use std::time::Duration;

use runtime_core::animation::{AnimProp, AnimatedValue, TweenTo};
use runtime_core::primitives::overlay::BackdropMode;
use runtime_core::primitives::portal::ViewportPlacement;
use runtime_core::{
    component, viewport_size, AlignItems, Color, Element, FlexDirection, IntoElement,
    JustifyContent, Length, Position, Ref, StyleApplication, StyleRules, StyleSheet, Tokenized,
    VariantSet, ViewHandle,
};

use crate::stylesheets::Modal as ModalStyle;

/// Desired surface width when the viewport has room for it.
const DEFAULT_MODAL_WIDTH: f32 = 520.0;
/// Breathing room kept on each side between the surface and the viewport
/// edge when the surface would otherwise be wider than the screen.
const MODAL_EDGE_MARGIN: f32 = 16.0;
/// Don't shrink the surface below this even on a very narrow viewport.
const MODAL_MIN_FIT: f32 = 280.0;
/// Enter animation duration.
const ENTER_MS: u64 = 180;
/// How far below its resting position the card starts before sliding up.
const CARD_SLIDE_PX: f32 = 14.0;

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct ModalProps {
    /// Fires when the user dismisses (backdrop tap — unless
    /// `on_backdrop_press` overrides it — or Escape / back). The host is
    /// expected to flip its open-state signal in response; idea-ui's modal
    /// doesn't auto-unmount itself.
    pub on_dismiss: Option<Rc<dyn Fn()>>,
    /// Fires when the backdrop is pressed. Unset → falls back to
    /// `on_dismiss` when `dismissable`, else does nothing. Set it to take
    /// over the tap (e.g. confirm before closing) instead of assuming
    /// dismissal.
    pub on_backdrop_press: Option<Rc<dyn Fn()>>,
    /// `true` (default) lets the backdrop tap dismiss (via `on_dismiss`)
    /// and routes Escape/back to `on_dismiss`. `false` makes the backdrop
    /// inert (no dismissal) unless `on_backdrop_press` is set.
    pub dismissable: bool,
    /// Desired surface width on a roomy viewport, in DIPs. Capped to the
    /// viewport width reactively so it never overflows a phone.
    pub width: f32,
    /// Custom backdrop scrim style. `None` uses the default dimming scrim.
    /// Must keep the backdrop full-bleed (`position: absolute` + zero
    /// insets).
    pub backdrop_style: Option<Rc<StyleSheet>>,
    pub children: Vec<Element>,
}

impl Default for ModalProps {
    fn default() -> Self {
        Self {
            on_dismiss: None,
            on_backdrop_press: None,
            dismissable: true,
            width: DEFAULT_MODAL_WIDTH,
            backdrop_style: None,
            children: Vec::new(),
        }
    }
}

/// The surface's resolved width: the desired width, capped so it (plus a
/// margin on each side) fits the viewport, but never shrunk below
/// [`MODAL_MIN_FIT`]. Pure so the cap is unit-testable without a backend.
fn effective_modal_width(desired: f32, viewport_width: f32) -> f32 {
    desired.min((viewport_width - MODAL_EDGE_MARGIN * 2.0).max(MODAL_MIN_FIT))
}

/// The default dimming scrim: a full-bleed, near-black wash matching the
/// surface shadow's color family. Absolutely positioned with zero insets so
/// it fills the fullscreen portal behind the centered card.
fn default_backdrop_sheet() -> Rc<StyleSheet> {
    Rc::new(StyleSheet::new(|_vs: &VariantSet| StyleRules {
        position: Some(Position::Absolute),
        top: Some(Tokenized::Literal(Length::Px(0.0))),
        left: Some(Tokenized::Literal(Length::Px(0.0))),
        right: Some(Tokenized::Literal(Length::Px(0.0))),
        bottom: Some(Tokenized::Literal(Length::Px(0.0))),
        background: Some(Tokenized::Literal(Color("rgba(15, 17, 21, 0.45)".into()))),
        ..Default::default()
    }))
}

/// Transparent full-fill pressable that sits inside the backdrop layer and
/// catches taps. (The scrim color + fade live on the parent layer view so a
/// single opacity animation fades the whole backdrop.)
fn backdrop_hit_sheet() -> Rc<StyleSheet> {
    Rc::new(StyleSheet::new(|_vs: &VariantSet| StyleRules {
        width: Some(Tokenized::Literal(Length::pct(100.0))),
        height: Some(Tokenized::Literal(Length::pct(100.0))),
        ..Default::default()
    }))
}

/// Positioned wrapper around the card. On web, the absolutely-positioned
/// backdrop paints (and hit-tests) above any *static* sibling regardless of
/// DOM order, so the card needs to be positioned to sit on top. `position:
/// relative` via a plain base sheet emits reliably (the dynamic
/// `with_computed` layer drops `position`), and a positioned sibling that
/// comes later in DOM stacks above the earlier absolute backdrop. No-op on
/// native (z-order there follows insertion order, which already puts the
/// card on top).
fn card_layer_sheet() -> Rc<StyleSheet> {
    Rc::new(StyleSheet::new(|_vs: &VariantSet| StyleRules {
        position: Some(Position::Relative),
        flex_direction: Some(FlexDirection::Column),
        ..Default::default()
    }))
}

/// Fullscreen flex container that centers the card over the backdrop.
fn center_container_sheet() -> Rc<StyleSheet> {
    Rc::new(StyleSheet::new(|_vs: &VariantSet| StyleRules {
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        width: Some(Tokenized::Literal(Length::pct(100.0))),
        height: Some(Tokenized::Literal(Length::pct(100.0))),
        ..Default::default()
    }))
}

#[component(children)]
pub fn Modal(props: ModalProps) -> Element {
    let on_dismiss = props.on_dismiss.clone();

    // Resolve the backdrop press handler: explicit override wins; otherwise
    // dismiss when dismissable; otherwise the backdrop is inert.
    let backdrop_handler: Option<Rc<dyn Fn()>> = props.on_backdrop_press.clone().or_else(|| {
        if props.dismissable {
            on_dismiss.clone()
        } else {
            None
        }
    });

    let mut content: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        runtime_core::ChildList::append_to(c, &mut content);
    }

    // Backdrop layer: a full-bleed view (scrim color) that fades in, with a
    // transparent pressable inside catching taps. Opacity-only — no slide —
    // so no hard dark edge sweeps across.
    let backdrop_ref: Ref<ViewHandle> = Ref::new();
    let bd_opacity = AnimatedValue::new(0.0_f32);
    bd_opacity.bind(backdrop_ref, AnimProp::Opacity);
    bd_opacity.animate(TweenTo::new(1.0_f32, Duration::from_millis(ENTER_MS)).ease_out());

    let backdrop_sheet = props
        .backdrop_style
        .unwrap_or_else(default_backdrop_sheet);
    let backdrop = {
        let h = backdrop_handler.clone();
        let hit = runtime_core::pressable(Vec::new(), move || {
            if let Some(h) = &h {
                h();
            }
        })
        .with_style(StyleApplication::new(backdrop_hit_sheet()))
        .into_element();
        runtime_core::view(vec![hit])
            .with_style(StyleApplication::new(backdrop_sheet))
            .bind(backdrop_ref)
            .into_element()
    };

    // Card: themed sheet + viewport-capped width, `position: relative` so it
    // paints above the absolute backdrop on web. Fades in and slides up.
    let surface_ref: Ref<ViewHandle> = Ref::new();
    let card_opacity = AnimatedValue::new(0.0_f32);
    card_opacity.bind(surface_ref, AnimProp::Opacity);
    card_opacity.animate(TweenTo::new(1.0_f32, Duration::from_millis(ENTER_MS)).ease_out());
    let card_slide = AnimatedValue::new(CARD_SLIDE_PX);
    card_slide.bind(surface_ref, AnimProp::TranslateY);
    card_slide.animate(TweenTo::new(0.0_f32, Duration::from_millis(ENTER_MS)).ease_out());

    let viewport = viewport_size();
    let desired = props.width;
    let surface = runtime_core::view(content)
        .with_style(move || {
            let vw = viewport.get().width;
            let effective = effective_modal_width(desired, vw);
            StyleApplication::new(ModalStyle::sheet()).with_computed(
                format!("modal-w-{}", effective.round() as i32),
                move || StyleRules {
                    width: Some(Tokenized::Literal(Length::Px(effective))),
                    ..Default::default()
                },
            )
        })
        .bind(surface_ref)
        .into_element();
    // Positioned wrapper so the card stacks above the absolute backdrop on
    // web (see `card_layer_sheet`). The fade/slide animators stay on the
    // inner surface; this wrapper only fixes stacking.
    //
    // It's a `pressable` (no-op handler), NOT a plain `view`, so a tap that
    // lands on the card itself is CONSUMED here and does not fall through to
    // the backdrop sibling beneath. On iOS/web the topmost view at a point
    // already wins hit-testing, so the card never leaked taps to the
    // backdrop — but on Android a non-clickable `ViewGroup` returns `false`
    // from `onTouchEvent`, and `FrameLayout` then keeps dispatching to the
    // earlier (lower) backdrop child, whose pressable consumes the tap and
    // dismisses the modal. Marking the card-layer pressable makes it
    // touch-consuming on Android (matching iOS/web), so tapping the card no
    // longer dismisses. Children-first dispatch still delivers taps on the
    // card's own interactive descendants (buttons, inputs) to them; only the
    // empty regions of the card are swallowed here — exactly what a modal
    // surface should do.
    let card = runtime_core::pressable(vec![surface], || {})
        .with_style(StyleApplication::new(card_layer_sheet()))
        .into_element();

    // One fullscreen portal: backdrop (behind) + card (centered) as siblings
    // in a flex-centering content wrapper. `backdrop(None)` because we supply
    // our own backdrop child; Escape/back still routes to `on_dismiss`.
    let mut overlay = runtime_core::overlay(vec![backdrop, card])
        .placement(ViewportPlacement::FullScreen)
        .backdrop(BackdropMode::None)
        .with_style(StyleApplication::new(center_container_sheet()))
        .trap_focus(true);
    if let Some(d) = on_dismiss {
        overlay = overlay.on_dismiss(move || (d)());
    }
    runtime_core::IntoElement::into_element(overlay)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn props_default_is_dismissable_with_default_width() {
        let p = ModalProps::default();
        assert!(p.dismissable);
        assert!(p.backdrop_style.is_none());
        assert!(p.on_backdrop_press.is_none());
        assert_eq!(p.width, DEFAULT_MODAL_WIDTH);
    }

    #[test]
    fn width_unclamped_when_viewport_has_room() {
        assert_eq!(effective_modal_width(520.0, 1280.0), 520.0);
        assert_eq!(effective_modal_width(440.0, 1280.0), 440.0);
    }

    #[test]
    fn regression_width_caps_to_viewport_on_a_phone() {
        // The bug: a 520-wide surface centered on a 393pt phone overflowed
        // ~63pt off each edge because `max_width: 100%` didn't clamp against
        // the auto-sized portal wrapper on the native backends. The cap must
        // bring it to viewport - 2*margin so it fits with breathing room.
        let eff = effective_modal_width(520.0, 393.0);
        assert_eq!(eff, 393.0 - MODAL_EDGE_MARGIN * 2.0);
        assert!(eff < 393.0, "surface must fit within the viewport");
    }

    #[test]
    fn width_never_shrinks_below_min_fit_on_tiny_viewports() {
        let eff = effective_modal_width(520.0, 300.0);
        assert_eq!(eff, MODAL_MIN_FIT);
    }

    /// Regression: the Modal's card layer must be a touch-CONSUMING
    /// `Element::Pressable`, not a plain `Element::View`.
    ///
    /// The bug ([[project_android_portal_is_dialog_smell]] follow-up):
    /// after the Android portal became a single-window view overlay, the
    /// overlay's children are `[backdrop, card]` siblings in one
    /// `FrameLayout`. A tap on the card landed on a non-clickable
    /// `ViewGroup`, which returns `false` from `onTouchEvent`, so
    /// `FrameLayout` fell through to the lower backdrop child whose
    /// pressable consumed the tap and dismissed the modal — i.e. tapping
    /// the card *closed* it. iOS/web don't fall through (topmost view at a
    /// point wins hit-testing), so this was Android-only. Making the card
    /// layer a `pressable` (no-op handler) makes it consume taps on every
    /// backend, so the card no longer leaks to the backdrop.
    ///
    /// We assert the structure the fix relies on: the portal's second
    /// child (the card, painted above the first-child backdrop) is a
    /// `Pressable`. If a refactor reverts it to a plain `view`, the
    /// Android tap-through regression returns and this test fails.
    #[test]
    fn regression_modal_card_layer_consumes_touches() {
        use runtime_core::Element;

        // Build the Modal element directly (no backend install needed —
        // `AnimatedValue::bind` no-ops without one and `viewport_size`
        // just returns a signal).
        let modal = Modal(ModalProps {
            children: vec![runtime_core::text("hi").into_element()],
            ..Default::default()
        });

        // Modal → Element::Portal { children: [content_view], .. }.
        // `build_overlay_portal` wraps the Modal's own children in a
        // single flex-center content view, so the portal has ONE child
        // (the center container) whose children are [backdrop, card].
        let portal_children = match &modal {
            Element::Portal { children, .. } => children,
            _ => panic!("Modal should build a Portal"),
        };
        assert_eq!(
            portal_children.len(),
            1,
            "overlay wraps the Modal's children in one center-container view"
        );
        let layers = match &portal_children[0] {
            Element::View { children, .. } => children,
            _ => panic!("portal child should be the center-container view"),
        };
        assert_eq!(
            layers.len(),
            2,
            "center container should hold [backdrop, card] as siblings"
        );
        // layer 0 = backdrop (a view wrapping the hit pressable),
        // layer 1 = card layer (must be a Pressable so card taps don't
        // fall through to the backdrop).
        assert!(
            matches!(layers[1], Element::Pressable { .. }),
            "card layer must be a touch-consuming Pressable so a tap on \
             the card doesn't fall through to the backdrop and dismiss \
             the modal (Android FrameLayout fall-through)"
        );
    }
}
