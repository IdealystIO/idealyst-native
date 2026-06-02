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
//! ## Width and height
//! [`ModalProps::width`] is the surface's desired width on a roomy viewport;
//! it is capped to the viewport width (minus a margin) reactively so the
//! surface never overflows a phone. The surface height is likewise capped to
//! the viewport height (minus the same margin) — a `max-height`, not a fixed
//! height, so a short modal stays content-sized. When the content is taller
//! than the cap, the card clips (`overflow: hidden`, keeping the rounded
//! corners) and an inner `scroll_view` scrolls the body. The frame keeps
//! `ModalStyle`'s visuals (bg, radius, border, shadow); its `padding`/`gap`
//! move to the inner body view so the spacing lives *inside* the scroller.
//! (v1 scrolls the whole body — a fixed, non-scrolling footer is not split
//! out yet.)

use std::rc::Rc;
use std::time::Duration;

use runtime_core::animation::{AnimProp, AnimatedValue, TweenTo};
use runtime_core::primitives::overlay::BackdropMode;
use runtime_core::primitives::portal::ViewportPlacement;
use runtime_core::{
    component, viewport_size, AlignItems, Color, Element, FlexDirection, IntoElement,
    JustifyContent, Length, Overflow, Position, Ref, StyleApplication, StyleRules, StyleSheet,
    Tokenized, VariantSet, ViewHandle,
};

use crate::stylesheets::Modal as ModalStyle;

/// Desired surface width when the viewport has room for it.
const DEFAULT_MODAL_WIDTH: f32 = 520.0;
/// Breathing room kept on each side between the surface and the viewport
/// edge when the surface would otherwise be wider than the screen.
const MODAL_EDGE_MARGIN: f32 = 16.0;
/// Don't shrink the surface below this even on a very narrow viewport.
const MODAL_MIN_FIT: f32 = 280.0;
/// Don't cap the surface height below this even on a very short viewport
/// (e.g. a landscape phone). Mirrors [`MODAL_MIN_FIT`] for the height axis
/// so a tiny viewport still leaves a usable, scrollable surface.
const MODAL_MIN_HEIGHT_FIT: f32 = 200.0;
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

/// The surface's height cap: the viewport height minus a margin on the top
/// and bottom, but never below [`MODAL_MIN_HEIGHT_FIT`] on a very short
/// viewport. This is a `max-height`, not a fixed height — a modal whose
/// content is shorter than the cap stays content-sized; only a taller one
/// is clamped here (and then scrolls internally). Pure so the cap is
/// unit-testable without a backend.
fn effective_modal_max_height(viewport_height: f32) -> f32 {
    (viewport_height - MODAL_EDGE_MARGIN * 2.0).max(MODAL_MIN_HEIGHT_FIT)
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
        // Start fully transparent so the FIRST painted frame is invisible and
        // the AnimatedValue fades it up from there. Without this, the view
        // keeps its default alpha 1 until the first animate *tick* fires —
        // `AnimatedValue::bind` applies its initial value via
        // `subscribe_and_apply`, but the bind runs before the ref is filled
        // (view not mounted yet) so that initial write is silently skipped and
        // never re-applied. The gap = one frame of full-opacity scrim = the
        // open flicker. A static `opacity` in the stylesheet is applied at
        // `apply_style` (pre-paint) and covers it.
        opacity: Some(Tokenized::Literal(0.0)),
        ..Default::default()
    }))
}

/// Static sheet for the card's animation wrapper: starts at `opacity: 0` so
/// the card's first painted frame is invisible (the AnimatedValue fades it up
/// + slides it). Same first-frame-flicker reason as [`default_backdrop_sheet`].
/// MUST be static (not in the surface's reactive width closure, which re-runs
/// on viewport change and would re-zero the alpha mid-animation). The AV binds
/// this view's `ViewHandle` (a pressable can't carry one), so it's a distinct
/// layer between the touch-consuming card pressable and the visible surface.
fn card_anim_sheet() -> Rc<StyleSheet> {
    Rc::new(StyleSheet::new(|_vs: &VariantSet| StyleRules {
        flex_direction: Some(FlexDirection::Column),
        opacity: Some(Tokenized::Literal(0.0)),
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

/// The scrolling body's inner layout view. Carries the LAYOUT half of
/// `ModalStyle` that the frame gives up (flex column + the inter-child
/// `gap` + the surface `padding`) so the card's look and spacing are
/// unchanged, while the frame clips and the `scroll_view` between them
/// handles overflow. Static (token-resolved at apply time) so `padding`
/// and `gap` emit reliably — the dynamic `with_computed` layer on the
/// frame is reserved for the viewport-derived width/height caps.
fn modal_body_sheet() -> Rc<StyleSheet> {
    Rc::new(StyleSheet::new(|_vs: &VariantSet| StyleRules {
        flex_direction: Some(FlexDirection::Column),
        // Mirrors `ModalStyle`'s base `gap`/`padding` (spacing-md / spacing-lg).
        gap: Some(Tokenized::token("spacing-md", Length::Px(12.0))),
        padding_top: Some(Tokenized::token("spacing-lg", Length::Px(16.0))),
        padding_right: Some(Tokenized::token("spacing-lg", Length::Px(16.0))),
        padding_bottom: Some(Tokenized::token("spacing-lg", Length::Px(16.0))),
        padding_left: Some(Tokenized::token("spacing-lg", Length::Px(16.0))),
        ..Default::default()
    }))
}

/// The scroll_view that sits between the clipping frame and the body. It
/// fills the frame (`flex_grow: 1`) so it takes the capped height and
/// scrolls its single body child when that child is taller. Static so
/// `flex_grow` emits reliably.
fn modal_scroll_sheet() -> Rc<StyleSheet> {
    Rc::new(StyleSheet::new(|_vs: &VariantSet| StyleRules {
        flex_grow: Some(Tokenized::Literal(1.0)),
        // The scroller is itself a column so its body child lays out
        // top-to-bottom and can grow past the visible region.
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

    // Card: themed sheet + viewport-capped width AND height. The frame keeps
    // `ModalStyle`'s VISUALS (bg, radius, border, shadow) but gives up its
    // `padding`/`gap` (overridden to 0 in the computed layer) — those move to
    // the inner body view so they live *inside* the scroller. `overflow:
    // hidden` clips the scroll content to the rounded corners. Inside the
    // frame, a `scroll_view` (flex_grow: 1) takes the capped height and
    // scrolls its single body child when the content is taller than the cap.
    //
    // A content-shorter-than-the-cap modal stays content-sized: `max_height`
    // is a max, not a fixed height, and the scroll_view only scrolls when its
    // child overflows. The visible surface itself carries NO animator — the
    // fade/slide live on a dedicated `anim_view` wrapper below whose static
    // `opacity: 0` sheet makes the first painted frame invisible (see
    // `card_anim_sheet`).
    let viewport = viewport_size();
    let desired = props.width;

    let body = runtime_core::view(content)
        .with_style(StyleApplication::new(modal_body_sheet()))
        .into_element();
    let scroller = runtime_core::primitives::scroll_view::scroll_view(vec![body])
        .with_style(StyleApplication::new(modal_scroll_sheet()))
        .into_element();

    let surface = runtime_core::view(vec![scroller])
        .with_style(move || {
            let vp = viewport.get();
            let effective = effective_modal_width(desired, vp.width);
            let max_h = effective_modal_max_height(vp.height);
            StyleApplication::new(ModalStyle::sheet()).with_computed(
                format!(
                    "modal-wh-{}-{}",
                    effective.round() as i32,
                    max_h.round() as i32
                ),
                move || StyleRules {
                    width: Some(Tokenized::Literal(Length::Px(effective))),
                    max_height: Some(Tokenized::Literal(Length::Px(max_h))),
                    // Clip the scroll content to the frame's rounded corners.
                    overflow: Some(Overflow::Hidden),
                    // The padding/gap baked into `ModalStyle::sheet()` move to
                    // the inner body view (inside the scroller); zero them on
                    // the frame so the scroller fills it edge-to-edge.
                    padding_top: Some(Tokenized::Literal(Length::Px(0.0))),
                    padding_right: Some(Tokenized::Literal(Length::Px(0.0))),
                    padding_bottom: Some(Tokenized::Literal(Length::Px(0.0))),
                    padding_left: Some(Tokenized::Literal(Length::Px(0.0))),
                    gap: Some(Tokenized::Literal(Length::Px(0.0))),
                    ..Default::default()
                },
            )
        })
        .into_element();

    // Animation wrapper: opacity 0→1 fade + translateY slide-up. Bound here
    // (a real `view` with a `ViewHandle`) rather than on the surface so the
    // static `opacity: 0` base (pre-paint, flicker-free) is separate from the
    // surface's reactive width style, and so the card pressable below (which
    // can't carry a `ViewHandle`) stays purely a touch layer.
    let surface_ref: Ref<ViewHandle> = Ref::new();
    let card_opacity = AnimatedValue::new(0.0_f32);
    card_opacity.bind(surface_ref, AnimProp::Opacity);
    card_opacity.animate(TweenTo::new(1.0_f32, Duration::from_millis(ENTER_MS)).ease_out());
    let card_slide = AnimatedValue::new(CARD_SLIDE_PX);
    card_slide.bind(surface_ref, AnimProp::TranslateY);
    card_slide.animate(TweenTo::new(0.0_f32, Duration::from_millis(ENTER_MS)).ease_out());
    let anim_view = runtime_core::view(vec![surface])
        .with_style(StyleApplication::new(card_anim_sheet()))
        .bind(surface_ref)
        .into_element();

    // Positioned wrapper so the card stacks above the absolute backdrop on
    // web (see `card_layer_sheet`).
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
    let card = runtime_core::pressable(vec![anim_view], || {})
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

    #[test]
    fn max_height_caps_to_viewport_minus_margin() {
        // A roomy phone in portrait: the surface may grow to the viewport
        // height minus the top+bottom margin, then scroll past that.
        let h = effective_modal_max_height(852.0);
        assert_eq!(h, 852.0 - MODAL_EDGE_MARGIN * 2.0);
        assert!(h < 852.0, "the surface must fit within the viewport height");
    }

    #[test]
    fn regression_tall_modal_caps_to_viewport_height() {
        // The bug: `ModalStyle` capped WIDTH (`max_width: 560`) but never
        // height, so a modal whose content was taller than the screen ran
        // off the top and bottom edges with no way to reach the clipped
        // parts. The height cap brings it to viewport - 2*margin so it fits;
        // the internal scroll_view then reaches the overflow.
        let short_viewport = 500.0;
        let h = effective_modal_max_height(short_viewport);
        assert_eq!(h, short_viewport - MODAL_EDGE_MARGIN * 2.0);
        assert!(h < short_viewport);
    }

    #[test]
    fn max_height_never_shrinks_below_min_on_tiny_viewports() {
        // A very short viewport (e.g. a landscape phone) still leaves a
        // usable, scrollable surface rather than collapsing to a sliver.
        let h = effective_modal_max_height(150.0);
        assert_eq!(h, MODAL_MIN_HEIGHT_FIT);
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
