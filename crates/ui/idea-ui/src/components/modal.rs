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
    component, effect, presence, safe_area_insets, viewport_size, AlignItems, Color, Easing,
    Element, FlexDirection, IdealystSchema, IntoElement, JustifyContent, Length, Overflow,
    Position, PresenceAnim, PresenceState, Reactive, Ref, StyleApplication, StyleRules, StyleSheet,
    Tokenized, VariantSet, ViewHandle,
};

use crate::slot_override::apply_override;
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
/// Exit animation duration. The modal stays mounted this long after `open`
/// flips false so the fade/slide-out can play before `presence` unmounts it.
const EXIT_MS: u64 = 150;
/// How far below its resting position the card starts before sliding up.
const CARD_SLIDE_PX: f32 = 14.0;

// Reactive-by-default: `#[props]` wraps the scalar data props (`dismissable`,
// `width`) → `Reactive<…>`. `open` is already `Reactive<bool>` (structural —
// drives presence mount/unmount, untouched); `content` is a custom
// element-builder newtype (`#[prop(static)]`); the `Rc<dyn Fn>` handlers and
// the `Option<Rc<StyleSheet>>` backdrop are auto-skipped (Rc).
#[runtime_core::props]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct ModalProps {
    /// Open state. `Reactive<bool>` — pass your open-state `Signal<bool>`
    /// directly (it coerces). The modal is **always mounted**; flipping this
    /// false plays the exit animation and then unmounts via `presence`. Do
    /// NOT gate the modal with `if open.get() { Modal(...) }` — that unmounts
    /// instantly and skips the exit animation (the bug this replaced).
    #[schema(constraint = "reactive: static bool or Signal/rx!")]
    pub open: Reactive<bool>,
    /// Builds the modal body, laid out in a column inside the scrollable
    /// surface. A closure (not a `{ }` child block) because `presence`
    /// rebuilds the content on each open — so it must be reconstructable.
    /// Author it as `content = move || ui! { … }` (a `move` closure; multiple
    /// nodes → a `Vec<Element>`). State inside is rebuilt fresh on each open.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    #[prop(static)]
    pub content: ModalContent,
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
    /// Style override for the card **surface** (the frame — background, border,
    /// radius, shadow). Layered on the theme `ModalStyle`. See
    /// [`crate::slot_override`].
    #[prop(static)]
    pub surface_style: Option<Rc<StyleSheet>>,
    /// Style override for the scrollable **body** that wraps `content`. Its
    /// most common use is padding: the body hard-codes `spacing-lg` on all four
    /// sides, so set `padding: 0` here (a "bleed") to let an edge-to-edge
    /// illustration header sit flush. Layered on the body style. See
    /// [`crate::slot_override`].
    #[prop(static)]
    pub content_style: Option<Rc<StyleSheet>>,
}

/// The modal body builder. A newtype over `Rc<dyn Fn() -> Element>` whose
/// `From<closure>` lets a call site pass a bare `move || ui! { … }` (the `ui!`
/// macro feeds prop values through `.into()`, which can't unsize a
/// `Rc<{closure}>` to a `Rc<dyn Fn …>` — this newtype bridges that). The
/// closure returns one `Element` — exactly what a `ui! { … }` block yields
/// (multi-node blocks are view-wrapped by the macro). Cloned cheaply (`Rc`).
#[derive(Clone)]
pub struct ModalContent(pub Rc<dyn Fn() -> Element>);

impl<F: Fn() -> Element + 'static> From<F> for ModalContent {
    fn from(f: F) -> Self {
        ModalContent(Rc::new(f))
    }
}

impl Default for ModalContent {
    fn default() -> Self {
        ModalContent(Rc::new(|| runtime_core::view(Vec::new()).into_element()))
    }
}

impl Default for ModalProps {
    fn default() -> Self {
        Self {
            open: Reactive::Static(false),
            content: ModalContent::default(),
            on_dismiss: None,
            on_backdrop_press: None,
            dismissable: Reactive::Static(true),
            width: Reactive::Static(DEFAULT_MODAL_WIDTH),
            backdrop_style: None,
            surface_style: None,
            content_style: None,
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
        // Don't shrink inside the scroll viewport. The scroller is capped at
        // the viewport height (`max_height` on `modal_scroll_sheet`); without
        // `flex_shrink:0` the body would shrink to fit the capped scroller and
        // there'd be nothing to scroll. With it, tall content keeps its full
        // height and overflows the viewport → the scroll view scrolls.
        flex_shrink: Some(Tokenized::Literal(0.0)),
        // Mirrors `ModalStyle`'s base `gap`/`padding` (spacing-md / spacing-lg).
        gap: Some(Tokenized::token("spacing-md", Length::Px(12.0))),
        padding_top: Some(Tokenized::token("spacing-lg", Length::Px(16.0))),
        padding_right: Some(Tokenized::token("spacing-lg", Length::Px(16.0))),
        padding_bottom: Some(Tokenized::token("spacing-lg", Length::Px(16.0))),
        padding_left: Some(Tokenized::token("spacing-lg", Length::Px(16.0))),
        ..Default::default()
    }))
}

/// The scroll_view that sits between the clipping frame and the body, sized
/// to its content up to the viewport cap (the cap is applied reactively as a
/// `max_height` at the call site).
///
/// `scroll_view` creation seeds the scroll node with `flex_grow:1 /
/// flex_basis:0` — the "fill a bounded parent" shape used when a scroll view
/// is a flex child of a fixed-size region (e.g. a screen). That shape is
/// WRONG for a modal: the surface is content-sized (auto height), so a
/// fill-parent scroller contributes 0 to the surface's intrinsic height and
/// the entire card collapses to 0×0 — the modal renders nothing but the
/// backdrop. Override to the "content-sized up to a cap, then scroll" shape:
///
/// - `flex_grow:0` + `flex_basis:auto` — the scroller sizes to its body's
///   content, so the surface (and card) size to it. A short modal hugs its
///   content instead of collapsing.
/// - `min_height:0` — paired with the scroll node's `overflow:scroll` (set by
///   `scroll_view`), this lets the reactive `max_height` cap clamp the
///   scroller BELOW its content height, so a modal taller than the viewport
///   scrolls internally instead of overflowing the screen.
///
/// Verified in runtime-layout (`regression_modal_scroller_content_sized_then_capped`).
/// MUST emit `flex_grow`/`flex_basis`/`min_height` to override the
/// `scroll_view` seed (`set_style` only writes the fields the sheet sets).
fn modal_scroll_sheet() -> Rc<StyleSheet> {
    Rc::new(StyleSheet::new(|_vs: &VariantSet| StyleRules {
        flex_grow: Some(Tokenized::Literal(0.0)),
        flex_basis: Some(Tokenized::Literal(Length::Auto)),
        min_height: Some(Tokenized::Literal(Length::Px(0.0))),
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

/// Renders a centered viewport overlay: one fullscreen portal holding a
/// fading dimming backdrop and a themed, viewport-capped, scrollable
/// surface that fades and slides in. Dismissal is delegated to the host
/// via `on_dismiss`/`on_backdrop_press`.
#[component]
pub fn Modal(props: ModalProps) -> Element {
    let open = props.open;
    let content = props.content;
    let on_dismiss = props.on_dismiss;
    let on_backdrop_press = props.on_backdrop_press;
    // TODO(reactive-sweep): route `dismissable`/`width` reactively into the
    // overlay structure. Both thread through `build_overlay` →
    // `assemble_overlay` as plain `bool`/`f32` and drive STRUCTURE: `width`
    // feeds the viewport-cap closure that sizes the surface (already reactive
    // on viewport, but the *desired* width is captured by value), and
    // `dismissable` selects the backdrop handler. A live signal would need the
    // surface width closure and the backdrop-handler wiring rebuilt on change.
    // Snapshot here (presence rebuilds `build` per open, so a value change
    // between opens is still picked up).
    let dismissable = props.dismissable.get();
    let desired = props.width.get();
    let backdrop_style = props.backdrop_style;
    let surface_style = props.surface_style;
    let content_style = props.content_style;

    // `presence` keeps the portal mounted through the EXIT window, then truly
    // unmounts it — so a closed modal leaves the tree on every backend (no
    // blocking, no focus trap), and `trap_focus(true)` is safe again. `build`
    // is re-run on each open, so the body content + its state are rebuilt
    // fresh (`content` is a closure, not an owned `Vec`, precisely so it can
    // be reconstructed). The visual fade/slide stays on `AnimatedValue`s bound
    // to the inner real views — presence's own opacity/translate no-op on a
    // *portal* child (see the module docs), so we only use presence for the
    // mount/unmount TIMING and drive the animation ourselves, now
    // bidirectionally via an effect that reads `open`.
    let build = {
        let open = open.clone();
        move || build_overlay(
            (content.0)(),
            open.clone(),
            on_dismiss.clone(),
            on_backdrop_press.clone(),
            dismissable,
            desired,
            backdrop_style.clone(),
            surface_style.clone(),
            content_style.clone(),
        )
    };

    presence(build)
        .present(move || open.get())
        // Timing-only exit: hold the portal mounted EXIT_MS so the
        // animate-out (driven inside `build`) can play, then unmount. The
        // PresenceState is a no-op on the portal child; the visual lives on
        // the inner views.
        .exit(PresenceAnim::new(PresenceState::default(), EXIT_MS as u32, Easing::EaseIn))
        .into_element()
}

/// Build the full modal overlay for one open cycle. Called fresh by
/// `presence` on each open, so all animators/effect/content are recreated.
#[allow(clippy::too_many_arguments)]
fn build_overlay(
    content: Element,
    open: Reactive<bool>,
    on_dismiss: Option<Rc<dyn Fn()>>,
    on_backdrop_press: Option<Rc<dyn Fn()>>,
    dismissable: bool,
    desired: f32,
    backdrop_style: Option<Rc<StyleSheet>>,
    surface_style: Option<Rc<StyleSheet>>,
    content_style: Option<Rc<StyleSheet>>,
) -> Element {
    // Resolve the backdrop press handler: explicit override wins; otherwise
    // dismiss when dismissable; otherwise the backdrop is inert.
    let backdrop_handler: Option<Rc<dyn Fn()>> = on_backdrop_press.or_else(|| {
        if dismissable {
            on_dismiss.clone()
        } else {
            None
        }
    });

    // Bidirectional animators: bound to the inner views, then driven by an
    // effect reading `open` — animate IN on open, OUT on close. The backdrop
    // fades only (no slide → no hard dark edge sweep); the card fades + slides.
    let backdrop_ref: Ref<ViewHandle> = Ref::new();
    let bd_opacity = AnimatedValue::new(0.0_f32);
    bd_opacity.bind(backdrop_ref, AnimProp::Opacity);
    let surface_ref: Ref<ViewHandle> = Ref::new();
    let card_opacity = AnimatedValue::new(0.0_f32);
    card_opacity.bind(surface_ref, AnimProp::Opacity);
    let card_slide = AnimatedValue::new(CARD_SLIDE_PX);
    card_slide.bind(surface_ref, AnimProp::TranslateY);
    // Scope-adopted by the presence-mounted subtree: freed when presence
    // unmounts after exit. No `mem::forget`.
    effect!({
        let is_open = open.get();
        let (op, slide, ms) = if is_open {
            (1.0_f32, 0.0_f32, ENTER_MS)
        } else {
            (0.0_f32, CARD_SLIDE_PX, EXIT_MS)
        };
        bd_opacity.animate(TweenTo::new(op, Duration::from_millis(ms)).ease_out());
        card_opacity.animate(TweenTo::new(op, Duration::from_millis(ms)).ease_out());
        card_slide.animate(TweenTo::new(slide, Duration::from_millis(ms)).ease_out());
    });

    assemble_overlay(
        content,
        backdrop_ref,
        surface_ref,
        backdrop_handler,
        backdrop_style,
        desired,
        on_dismiss,
        surface_style,
        content_style,
    )
}

/// Pure structural assembly of the modal portal (backdrop + scrollable card +
/// safe-area centering container) — no animators/effect, so it can be built
/// WITHOUT a reactive scope (the `regression_modal_card_layer_consumes_touches`
/// test exercises it directly). The animator refs are passed in, already bound
/// by [`build_overlay`].
#[allow(clippy::too_many_arguments)]
fn assemble_overlay(
    content: Element,
    backdrop_ref: Ref<ViewHandle>,
    surface_ref: Ref<ViewHandle>,
    backdrop_handler: Option<Rc<dyn Fn()>>,
    backdrop_style: Option<Rc<StyleSheet>>,
    desired: f32,
    on_dismiss: Option<Rc<dyn Fn()>>,
    surface_style: Option<Rc<StyleSheet>>,
    content_style: Option<Rc<StyleSheet>>,
) -> Element {
    // Backdrop layer: a full-bleed view (scrim color) that fades, with a
    // transparent pressable inside catching taps.
    let backdrop_sheet = backdrop_style.unwrap_or_else(default_backdrop_sheet);
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

    // The content closure yields one Element (a `ui! { … }` block — multi-node
    // blocks are view-wrapped by the macro). Wrap it in the padded, column body
    // view. Inter-item spacing is the content's own concern (wrap it in a
    // `Stack` for a gap); the body supplies padding + scroll behavior.
    let body = runtime_core::view(vec![content])
        .with_style(apply_override(
            StyleApplication::new(modal_body_sheet()),
            &content_style,
        ))
        .into_element();
    // The scroller carries the viewport-derived height cap. It sizes to its
    // body's content (so a short modal hugs its content) until it hits
    // `max_height`, then `overflow:scroll` takes over. Reactive so the cap
    // tracks orientation / split-view resizes, mirroring the surface width.
    let viewport_for_scroll = viewport_size();
    let scroller = runtime_core::primitives::scroll_view::scroll_view(vec![body])
        .with_style(move || {
            // Cap to the SAFE height: subtract the top + bottom safe-area
            // insets so a maximally-tall modal can't grow under the notch /
            // Dynamic Island / home indicator (the centering container also
            // pads by the insets — both are needed; see the overlay below).
            let insets = safe_area_insets().get();
            let avail_h = viewport_for_scroll.get().height - insets.top - insets.bottom;
            let max_h = effective_modal_max_height(avail_h);
            StyleApplication::new(modal_scroll_sheet()).with_computed(
                format!("modal-scroll-maxh-{}", max_h.round() as i32),
                move || StyleRules {
                    max_height: Some(Tokenized::Literal(Length::Px(max_h))),
                    ..Default::default()
                },
            )
        })
        .into_element();

    let surface = runtime_core::view(vec![scroller])
        .with_style(move || {
            let vp = viewport.get();
            // Cap width/height to the SAFE rect (subtract the horizontal /
            // vertical safe-area insets) so the card fits inside the notch /
            // home-indicator-free region. Mirrors the scroller's height cap.
            let insets = safe_area_insets().get();
            let effective = effective_modal_width(desired, vp.width - insets.left - insets.right);
            let max_h = effective_modal_max_height(vp.height - insets.top - insets.bottom);
            let app = StyleApplication::new(ModalStyle::sheet()).with_computed(
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
            );
            // Author surface override (frame bg/border/radius/shadow) resolves
            // last, on top of the computed width/height/clip layer.
            apply_override(app, &surface_style)
        })
        .into_element();

    // Animation wrapper: the fade + translateY slide live on this real
    // `view` (`ViewHandle`), driven by `card_opacity`/`card_slide` (bound +
    // animated by the effect above). Its static `opacity: 0` base
    // (`card_anim_sheet`) keeps the first painted frame invisible, flicker-free.
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
        // Center the card within the SAFE rect, not the full window: pad the
        // fullscreen centering container by the platform safe-area insets.
        // This is what guarantees the card clears the notch / Dynamic Island /
        // home indicator even when the insets are asymmetric (centering in the
        // full window would leave the card under the larger inset). The
        // dimming backdrop is `position: absolute; inset: 0`, which resolves
        // against the container's PADDING box — so it still fills the whole
        // window (verified in runtime-layout); only the card is constrained.
        // Reactive: orientation flips / sheet adaptations re-fire
        // `safe_area_insets()`. Insets are ZERO where no observer is wired
        // (web today), degrading to the previous full-window centering.
        .with_style(|| {
            let insets = safe_area_insets().get();
            StyleApplication::new(center_container_sheet()).with_computed(
                format!(
                    "modal-safe-{}-{}-{}-{}",
                    insets.top.round() as i32,
                    insets.right.round() as i32,
                    insets.bottom.round() as i32,
                    insets.left.round() as i32,
                ),
                move || StyleRules {
                    padding_top: Some(Tokenized::Literal(Length::Px(insets.top))),
                    padding_right: Some(Tokenized::Literal(Length::Px(insets.right))),
                    padding_bottom: Some(Tokenized::Literal(Length::Px(insets.bottom))),
                    padding_left: Some(Tokenized::Literal(Length::Px(insets.left))),
                    ..Default::default()
                },
            )
        })
        .trap_focus(true);
    if let Some(d) = on_dismiss {
        overlay = overlay.on_dismiss(move || (d)());
    }
    runtime_core::IntoElement::into_element(overlay)
}

#[cfg(test)]
mod tests {
    use super::*;
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::resolve_style;

    #[test]
    fn props_default_is_dismissable_with_default_width() {
        let p = ModalProps::default();
        assert!(p.dismissable.get());
        assert!(p.backdrop_style.is_none());
        assert!(p.on_backdrop_press.is_none());
        assert_eq!(p.width.get(), DEFAULT_MODAL_WIDTH);
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

        // Assemble the portal structure directly. We call `assemble_overlay`
        // (the pure structural half) rather than the full Modal because
        // `build_overlay` creates an `effect!`, which requires a live reactive
        // scope (provided by the walker in real use, absent in a unit test).
        // `AnimatedValue::bind` no-ops without a backend and `viewport_size`
        // just returns a signal, so the structure builds fine here.
        let portal = assemble_overlay(
            runtime_core::text("hi").into_element(),
            Ref::new(),
            Ref::new(),
            None,
            None,
            DEFAULT_MODAL_WIDTH,
            None,
            None,
            None,
        );

        // The portal wraps the Modal's content in a single flex-center
        // content view, so it has ONE child (the center container) whose
        // children are [backdrop, card].
        let portal_children = match &portal {
            Element::Portal { children, .. } => children,
            _ => panic!("assemble_overlay should build a Portal"),
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

    /// Find the body view — the first `View` with a direct `Text` child (the
    /// modal wraps `content` in the padded body view) — and return its resolved
    /// top padding in px.
    fn body_padding_top(portal: &Element) -> f32 {
        fn find_body(el: &Element) -> Option<&Element> {
            match el {
                Element::View { children, .. }
                    if children.iter().any(|c| matches!(c, Element::Text { .. })) =>
                {
                    Some(el)
                }
                Element::View { children, .. }
                | Element::Pressable { children, .. }
                | Element::ScrollView { children, .. }
                | Element::Portal { children, .. } => children.iter().find_map(find_body),
                _ => None,
            }
        }
        let body = find_body(portal).expect("modal has a body view wrapping the content");
        let style = match body {
            Element::View { style, .. } => style.as_ref().expect("body carries a style"),
            _ => unreachable!(),
        };
        let app = match style {
            runtime_core::StyleSource::Static(a) => a.clone(),
            _ => panic!("body uses a static style"),
        };
        match resolve_style(&app).padding_top.as_ref().map(|t| t.resolve()) {
            Some(runtime_core::Length::Px(v)) => v,
            other => panic!("expected a px top padding, got {other:?}"),
        }
    }

    // Regression (edge-to-edge header couldn't sit flush): the body hard-codes
    // `spacing-lg` padding. A `content_style` override with zero padding must
    // win, letting an illustration header bleed to the surface edge. This is the
    // slot-override system applied to Modal's body slot.
    #[test]
    fn content_style_override_makes_body_flush() {
        install_idea_theme(light_theme());

        let default_portal = assemble_overlay(
            runtime_core::text("hi").into_element(),
            Ref::new(),
            Ref::new(),
            None,
            None,
            DEFAULT_MODAL_WIDTH,
            None,
            None,
            None,
        );
        assert!(
            body_padding_top(&default_portal) > 0.0,
            "the default body has non-zero padding (spacing-lg)"
        );

        let flush = Rc::new(StyleSheet::r#static(StyleRules {
            padding_top: Some(Tokenized::Literal(Length::Px(0.0))),
            padding_right: Some(Tokenized::Literal(Length::Px(0.0))),
            padding_bottom: Some(Tokenized::Literal(Length::Px(0.0))),
            padding_left: Some(Tokenized::Literal(Length::Px(0.0))),
            ..Default::default()
        }));
        let flush_portal = assemble_overlay(
            runtime_core::text("hi").into_element(),
            Ref::new(),
            Ref::new(),
            None,
            None,
            DEFAULT_MODAL_WIDTH,
            None,
            None,
            Some(flush),
        );
        assert_eq!(
            body_padding_top(&flush_portal),
            0.0,
            "content_style padding:0 override makes the body flush"
        );
    }
}
