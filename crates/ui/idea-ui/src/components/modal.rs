//! `Modal` — a viewport overlay with a themed surface.
//!
//! Sugar over [`runtime_core::overlay`]: one fullscreen portal holding a
//! dimming backdrop and a themed, `Card`-like surface. The modal is ALWAYS
//! mounted and driven by its `open` prop; a typical call site is:
//!
//! ```ignore
//! let open = signal(false);
//! ui! {
//!     Button(label = "Open", on_click = move || open.set(true))
//!     Modal(
//!         open = open,
//!         on_dismiss = Some(Rc::new(move || open.set(false)) as Rc<dyn Fn()>),
//!         content = move || ui! {
//!             Typography(content = "Confirm", kind = typography_kind::H2)
//!             Typography(content = "Are you sure?")
//!         },
//!     )
//! }
//! ```
//!
//! ## Presentations
//! [`ModalPresentation`] picks where the surface sits: `Centered` (the
//! default) centers a card in the safe rect; `Top` pins the card's TOP edge
//! a fixed distance below the top of the safe rect, so its top — and a
//! search field in it — stays put while the content height changes (a
//! command palette); `Sheet` pins a full-width surface to the bottom edge.
//! Only the placement container, the surface geometry and the enter/exit
//! motion differ — the portal, backdrop, focus trap, dismissal and
//! scrolling body are shared.
//!
//! ## One portal, two layered children
//! A single fullscreen portal whose content is `[backdrop, card]` as
//! siblings — the backdrop is an absolutely-positioned full-bleed layer
//! behind a flex-placed card. (Two separate portals layer fine on web/iOS
//! but break on Android, where each portal is its own touch-modal Dialog
//! window and the scrim window never gets the outside tap.) The card is
//! content-sized, so a tap outside it lands on the backdrop on every
//! backend. The card is `position: relative` so it paints above the
//! absolutely-positioned backdrop on web (where positioned elements stack
//! above in-flow siblings regardless of DOM order).
//!
//! ## Enter and exit animation
//! Driven by [`runtime_core::AnimatedValue`] bound to the layers'
//! `ViewHandle`s (this reliably animates portal content on every backend —
//! `presence` only animates its direct child, which for a portal is the
//! escaped placeholder). The **backdrop fades** (opacity only — no slide,
//! so there's no hard dark edge sweeping across), while the **card fades
//! and slides** a few DIPs: up from below for a centered card, down from
//! above for a top-anchored one (it settles toward the edge it hangs
//! from), and a sheet rises from and leaves by the bottom edge. Opacity/
//! translate are used rather than scale, which raced with layout and
//! stuttered. `presence` holds the portal mounted through the exit, then
//! unmounts it.
//!
//! ## Backdrop press is a handler, not a hardcoded close
//! [`ModalProps::on_backdrop_press`] fires when the backdrop is tapped.
//! Unset → falls back to [`ModalProps::on_dismiss`] *if* `dismissable`, and
//! to nothing when non-dismissable. Set it to intercept the tap without
//! assuming dismissal. Escape / back reach `on_dismiss` only when
//! `dismissable` too.
//!
//! ## Width and height
//! [`ModalProps::width`] is the surface's desired width on a roomy viewport;
//! it is capped to the viewport width (minus a margin) reactively so the
//! surface never overflows a phone. For a centered or top-anchored card it
//! REPLACES the theme sheet's `max_width` (560) — any width, including one
//! above 560, is honored up to the viewport cap. A sheet fills its
//! container up to the theme's `max_width`. The surface height is likewise
//! capped to the safe viewport height (a centered card keeps a margin top
//! and bottom; a top-anchored card keeps its offset above and a margin
//! below) — a `max-height`, not a fixed height, so a short modal stays
//! content-sized. When the content is taller than the cap, the card clips
//! (`overflow: hidden`, keeping the rounded corners) and an inner
//! `scroll_view` scrolls the body. The `surface` slot is the frame's look
//! (bg, radius, border, shadow); the padding lives on the `body` slot, the
//! view *inside* the scroller. (v1 scrolls the whole
//! body — a fixed, non-scrolling footer is not split out yet.)

use std::rc::Rc;
use std::time::Duration;

use runtime_core::animation::{AnimProp, AnimatedValue, TweenTo};
use runtime_core::primitives::overlay::BackdropMode;
use runtime_core::primitives::portal::ViewportPlacement;
use runtime_core::{
    component, component_styles, effect, presence, EdgeInsets, safe_area_insets, stylesheet, viewport_size, AlignItems, Color,
    Easing, Element, FlexDirection, IdealystSchema, IntoElement, JustifyContent, Length, Overflow,
    Position, PresenceAnim, PresenceState, Reactive, Ref, StyleApplication, StyleRules, StyleSheet,
    Tokenized, ViewHandle
};

use crate::theme::IdeaThemeRef;

component_styles! {
    /// The styled sections of [`Modal`]. The positioning layers (the
    /// fullscreen centering container, the animation wrapper, the scroller)
    /// are structure, not look, and stay private.
    pub Modal<IdeaThemeRef> {
        /// The card frame: background, border, radius, shadow. Width and
        /// height caps are viewport-derived and ride the inline layer; for a
        /// card the inline `max_width` is the author's `width`, so the
        /// `max_width` here caps only a sheet.
        slot surface {
            base(t) {
            background: t.color.surface(),
            border_radius: t.radius.lg(),
            border_width: 1.0,
            border_color: t.color.border(),
            flex_direction: FlexDirection::Column,
            min_width: 320.0,
            max_width: 560.0,
            shadow: runtime_core::Shadow {
                x: 0.0,
                y: 12.0,
                blur: 32.0,
                color: Color("rgba(15, 17, 21, 0.25)".into()).into(),
            },
            }
            transitions {
            background: 250ms EaseInOut,
            border_color: 250ms EaseInOut,
            }
        }

        /// The padded column inside the scroller that holds `content`. Set
        /// its padding to zero (per instance, through `styles`) to let an
        /// edge-to-edge header sit flush.
        slot body {
            base(t) {
                flex_direction: FlexDirection::Column,
                // Don't shrink inside the scroll viewport. The scroller is
                // capped at the viewport height (`max_height` on
                // `modal_scroll_sheet`); without `flex_shrink:0` the body
                // would shrink to fit the capped scroller and there'd be
                // nothing to scroll. With it, tall content keeps its full
                // height and overflows the viewport → it scrolls.
                flex_shrink: 0.0,
                gap: t.spacing.md(),
                padding: t.spacing.lg(),
            }
        }

        /// The dimming scrim: a full-bleed, near-black wash, absolutely
        /// positioned with zero insets so it fills the fullscreen portal
        /// behind the card. A restyle must keep it full-bleed.
        ///
        /// It starts fully transparent so the FIRST painted frame is
        /// invisible and the `AnimatedValue` fades it up from there: the
        /// animator's bind runs before the view mounts, so its initial write
        /// is skipped, and without a static `opacity: 0` the scrim paints
        /// one full-opacity frame (the open flicker).
        slot backdrop {
            base(_t) {
                position: Position::Absolute,
                top: Length::Px(0.0),
                left: Length::Px(0.0),
                right: Length::Px(0.0),
                bottom: Length::Px(0.0),
                background: Color("rgba(15, 17, 21, 0.45)".into()),
                opacity: 0.0,
            }
        }
    }
}

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
/// A SHEET's exit takes longer than a card's, because it travels further:
/// it leaves by the edge it came from rather than fading where it stands.
const SHEET_EXIT_MS: u64 = 240;
/// How far below its resting position the card starts before sliding up.
const CARD_SLIDE_PX: f32 = 14.0;
/// How far below its resting position a SHEET starts before rising. Larger
/// than [`CARD_SLIDE_PX`]: a sheet should read as arriving from the screen
/// edge, where a card reads as settling in place.
///
/// It is a rise-and-fade on the way IN. The surface height isn't known
/// until layout (`ViewHandle` exposes no frame), so no build-time value
/// puts it exactly one height below — and a value large enough for a
/// tall sheet would make a short one whip in from far off screen.
///
/// The way OUT is a full slide instead — see [`sheet_exit_travel`]. The
/// constraint that forces a small rise on entry does not apply to an
/// exit: entering, over-travel is a sheet arriving too fast from too far
/// away; leaving, it only means the sheet is off screen sooner, which is
/// where it is going anyway.
const SHEET_SLIDE_PX: f32 = 32.0;
/// How far a sheet travels to leave the screen: its own height cap, so a
/// full-height sheet lands exactly off the bottom edge and a shorter one
/// is already gone before the tween ends.
///
/// Not the card's exit (a 32px nudge plus a 150ms fade): a fade that
/// short reads as the sheet vanishing, not departing. A sheet is the one
/// surface whose whole identity is the edge it came from, so it has to
/// go back there.
fn sheet_exit_travel(avail_h: f32) -> f32 {
    presentation_max_height(ModalPresentation::Sheet, avail_h, None)
}

/// A sheet's height cap, as a fraction of the SAFE viewport height. A sheet
/// deliberately never covers the whole screen: the strip of page left
/// visible above it is what says "layered over what you were reading"
/// rather than "a new screen".
const SHEET_MAX_HEIGHT_FRACTION: f32 = 0.78;
/// A top-anchored card's default distance below the top of the SAFE rect,
/// as a fraction of the safe height. A fraction rather than a fixed DIP
/// value because the command palettes this presentation exists for float
/// in the upper part of the window at a viewport-relative height — about
/// a sixth of the way down — rather than hugging the edge: on a 900pt
/// desktop window it is 135pt, on an 800pt phone 120pt. Override per
/// call site with [`ModalProps::top_offset`].
const TOP_OFFSET_FRACTION: f32 = 0.15;
/// How a [`Modal`] presents itself.
///
/// It is the SAME component either way — one portal, one backdrop, the same
/// focus trap, the same dismissal contract, the same scrolling body. Only
/// the geometry and the enter animation differ, which is why this is a prop
/// and not a second component: a sheet that drifted apart from the modal
/// would be a second place to fix the next Android hit-testing or safe-area
/// bug.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, IdealystSchema, runtime_core::Remote)]
pub enum ModalPresentation {
    /// A centered, width-capped card that fades and settles into place.
    /// The default, and what every existing call site gets.
    #[default]
    Centered,
    /// A width-capped card whose TOP edge is pinned a fixed distance below
    /// the top of the safe rect ([`ModalProps::top_offset`], default
    /// [`TOP_OFFSET_FRACTION`] of the safe height), growing DOWNWARD as its
    /// content grows.
    ///
    /// Reach for it when the content height changes while the user is
    /// typing into it — a command palette or a search-as-you-type picker.
    /// A centered card re-centers on every height change, so a field at
    /// its top moves under the caret; a top-anchored card keeps the field
    /// still at any height. It is capped to the safe height below the
    /// offset (minus a margin), then scrolls, like a centered card. It
    /// drops into place from a few DIPs above and fades out the same way.
    Top,
    /// A full-width surface pinned to the BOTTOM edge — rounded top corners,
    /// capped at [`SHEET_MAX_HEIGHT_FRACTION`] of the safe height — that
    /// rises from below.
    ///
    /// It carries no grabber and no drag gesture of its own; an app that
    /// wants either builds them in `content` and drives the surface through
    /// [`ModalProps::sheet_slide`].
    ///
    /// The thumb-reachable form. Reach for it when a phone user drives the
    /// surface from the bottom of the screen: filter and sort panels, row
    /// action menus, record detail. An anchored popover next to a 44dp row
    /// is not reachable with a thumb; this is what it becomes.
    Sheet,
}

// Reactive-by-default: `#[props]` wraps the scalar data props (`dismissable`,
// `width`, `presentation`) → `Reactive<…>`. `open` is already `Reactive<bool>` (structural —
// drives presence mount/unmount, untouched); `content` is a custom
// element-builder newtype (`#[prop(static)]`); the `Rc<dyn Fn>` handlers and
// the per-slot `styles` overrides are auto-skipped.
#[runtime_core::props(styles = ModalStyles)]
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
    /// Centered card (default), top-anchored card, or bottom sheet — see
    /// [`ModalPresentation`]. Everything else about the modal is unchanged
    /// by this choice.
    pub presentation: ModalPresentation,
    /// Distance in DIPs from the top of the safe rect to the card's top
    /// edge, for [`ModalPresentation::Top`]. `None` (the default) uses
    /// [`TOP_OFFSET_FRACTION`] of the safe height. Read live, so a signal
    /// moves the card. On a viewport too short to fit it, the offset gives
    /// way so the card keeps its minimum usable height. Ignored by the
    /// other presentations.
    pub top_offset: Option<f32>,
    /// Fires when the user dismisses (backdrop tap — unless
    /// `on_backdrop_press` overrides it — or Escape / back), only while
    /// `dismissable`. The host is
    /// expected to flip its open-state signal in response; idea-ui's modal
    /// doesn't auto-unmount itself.
    pub on_dismiss: Option<Rc<dyn Fn()>>,
    /// Fires when the backdrop is pressed. Unset → falls back to
    /// `on_dismiss` when `dismissable`, else does nothing. Set it to take
    /// over the tap (e.g. confirm before closing) instead of assuming
    /// dismissal.
    pub on_backdrop_press: Option<Rc<dyn Fn()>>,
    /// `true` (default) lets the backdrop tap dismiss (via `on_dismiss`)
    /// and routes Escape/back to `on_dismiss`. `false` makes both inert:
    /// Escape/back are swallowed without reaching `on_dismiss` (Android's
    /// back key does not fall through to the screen under the modal), and
    /// the backdrop does nothing unless `on_backdrop_press` is set. Close a non-dismissable modal
    /// from its own content (flip `open`).
    pub dismissable: bool,
    /// Desired surface width on a roomy viewport, in DIPs (default 520).
    /// Capped to the viewport width reactively so it never overflows a
    /// phone; otherwise honored as given — it overrides the theme sheet's
    /// 560 `max_width`. Applies to `Centered` and `Top`; ignored by
    /// `ModalPresentation::Sheet`, which spans its container.
    pub width: f32,
    /// The sheet's translate-Y animator, handed in by the app so it can drive
    /// the surface itself — drag-to-dismiss, a snap point, a peek state.
    ///
    /// `None` (the default) makes the modal create its own, which is what
    /// every call site that only wants enter/exit should do.
    ///
    /// When set, the modal still owns the enter and exit tweens and still
    /// resets the value to the entry offset at the start of each open cycle;
    /// between those, whatever the app writes lands on the surface. Units are
    /// DIPs below the resting position, so positive is DOWN, toward the edge
    /// a sheet leaves by. The exit tween starts from wherever the value is,
    /// so releasing a drag past your own threshold and flipping `open` false
    /// continues the travel instead of snapping back first.
    ///
    /// Meaningful only with [`ModalPresentation::Sheet`]; a centered card's
    /// slide is a 14px settle that nothing should be driving by hand. Pair it
    /// with your own `on_dismiss` — the modal has no opinion about what your
    /// gesture means.
    ///
    /// ```ignore
    /// let slide = AnimatedValue::new(0.0_f32);
    /// // …in `content`, a handle whose pan does `slide.set(dy)` and, past
    /// // your threshold on release, `open.set(false)`.
    /// Modal(presentation = ModalPresentation::Sheet, sheet_slide = Some(slide))
    /// ```
    #[cfg_attr(feature = "docs", doc_control(skip))]
    #[prop(static)]
    pub sheet_slide: Option<AnimatedValue<f32>>,
}

/// The modal body builder. A newtype over `Rc<dyn Fn() -> Element>` whose
/// `From<closure>` lets a call site pass a bare `move || ui! { … }` (the `ui!`
/// macro feeds prop values through `.into()`, which can't unsize a
/// `Rc<{closure}>` to a `Rc<dyn Fn …>` — this newtype bridges that). The
/// closure returns one `Element` — exactly what a `ui! { … }` block yields
/// (multi-node blocks are view-wrapped by the macro). Cloned cheaply (`Rc`).
#[derive(Clone, runtime_core::Remote)]
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
            presentation: Reactive::Static(ModalPresentation::Centered),
            top_offset: Reactive::Static(None),
            on_dismiss: None,
            on_backdrop_press: None,
            dismissable: Reactive::Static(true),
            width: Reactive::Static(DEFAULT_MODAL_WIDTH),
            sheet_slide: None,
            styles: Default::default(),
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

/// A top-anchored card's resolved distance below the top of the safe rect.
///
/// `requested` is the author's [`ModalProps::top_offset`] (honored as given,
/// floored at 0); `None` is [`TOP_OFFSET_FRACTION`] of the safe height.
/// Either way it is then capped so the
/// card still gets [`MODAL_MIN_HEIGHT_FIT`] above a bottom margin — on a
/// landscape phone the offset gives way before the card becomes a sliver.
/// Depends only on the safe height, never on the content, which is the
/// whole point: the card's top does not move as its content grows.
fn resolve_top_offset(requested: Option<f32>, safe_height: f32) -> f32 {
    let wanted = match requested {
        Some(v) => v.max(0.0),
        None => safe_height * TOP_OFFSET_FRACTION,
    };
    let room = (safe_height - MODAL_EDGE_MARGIN - MODAL_MIN_HEIGHT_FIT).max(0.0);
    wanted.min(room)
}

/// The surface's height cap for one presentation, given the SAFE viewport
/// height (insets already subtracted). A centered card keeps a margin top
/// and bottom; a top-anchored card keeps its offset above and a margin
/// below; a sheet takes a fixed fraction, so a strip of the page it is
/// layered over stays visible above it. `top_offset` is the author's
/// request, read only by the `Top` arm. Pure, so every arm is testable
/// without a backend.
fn presentation_max_height(
    presentation: ModalPresentation,
    safe_height: f32,
    top_offset: Option<f32>,
) -> f32 {
    match presentation {
        ModalPresentation::Centered => effective_modal_max_height(safe_height),
        ModalPresentation::Top => {
            let offset = resolve_top_offset(top_offset, safe_height);
            (safe_height - offset - MODAL_EDGE_MARGIN).max(MODAL_MIN_HEIGHT_FIT)
        }
        ModalPresentation::Sheet => {
            (safe_height * SHEET_MAX_HEIGHT_FRACTION).max(MODAL_MIN_HEIGHT_FIT)
        }
    }
}

/// How far the card starts from its resting position on entry (DIPs,
/// positive = below), and returns to on a card's exit. A centered card
/// rises a little into place; a top-anchored card DROPS a little into
/// place, toward the edge it hangs from (rising into a top anchor reads as
/// overshooting it); a sheet rises further — see [`SHEET_SLIDE_PX`].
fn enter_slide_px(presentation: ModalPresentation) -> f32 {
    match presentation {
        ModalPresentation::Centered => CARD_SLIDE_PX,
        ModalPresentation::Top => -CARD_SLIDE_PX,
        ModalPresentation::Sheet => SHEET_SLIDE_PX,
    }
}

/// The placement container's style: which sheet lays the card out, plus
/// the safe-area padding (and, for `Top`, the offset) on the INLINE layer.
///
/// - `Centered`: centered in the safe rect — padded by every inset.
/// - `Top`: `justify_content: FlexStart`, padded by every inset, with the
///   resolved top offset added to the top padding. The card's top edge is
///   therefore `insets.top + offset` regardless of its height.
/// - `Sheet`: pinned to the bottom edge and full-bleed, so only the TOP
///   inset constrains it (it must not grow under the notch); the surface
///   itself carries the bottom inset as frame padding so its background
///   reaches the screen edge.
///
/// Insets/offset are viewport-derived continuous values — the INLINE layer,
/// not a `with_computed` keyed on them: inline stays premintable (the
/// computed layer forced this one node onto the live engine, a
/// `--premint-only` panic) and applies out of band. Pure so each arm is
/// testable without a backend.
fn placement_container_style(
    presentation: ModalPresentation,
    insets: EdgeInsets,
    safe_height: f32,
    top_offset: Option<f32>,
) -> StyleApplication {
    let (sheet, pad_t, pad_r, pad_b, pad_l) = match presentation {
        ModalPresentation::Centered => (
            center_container_sheet(),
            insets.top,
            insets.right,
            insets.bottom,
            insets.left,
        ),
        ModalPresentation::Top => (
            top_container_sheet(),
            insets.top + resolve_top_offset(top_offset, safe_height),
            insets.right,
            insets.bottom,
            insets.left,
        ),
        ModalPresentation::Sheet => (sheet_container_sheet(), insets.top, 0.0, 0.0, 0.0),
    };
    let px = |v: f32| Some(Tokenized::Literal(Length::Px(v)));
    StyleApplication::new(sheet).with_inline(StyleRules {
        padding_top: px(pad_t),
        padding_right: px(pad_r),
        padding_bottom: px(pad_b),
        padding_left: px(pad_l),
        ..Default::default()
    })
}

/// Fullscreen flex container that pins the card's TOP edge (the offset
/// rides the inline padding — see [`placement_container_style`]) and
/// centers it horizontally. The `Top` counterpart of
/// [`center_container_sheet`]: identical but for `justify_content`, so a
/// change in the card's height grows it downward instead of re-centering.
fn top_container_sheet() -> Rc<StyleSheet> {
    ModalTopContainerSheet::sheet()
}

stylesheet! {
    ModalTopContainerSheet<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::FlexStart,
            width: Length::pct(100.0),
            height: Length::pct(100.0),
        }
    }
}

/// Fullscreen flex container that pins the sheet to the BOTTOM edge and
/// stretches it to the full width. The sheet counterpart of
/// [`center_container_sheet`].
fn sheet_container_sheet() -> Rc<StyleSheet> {
    ModalSheetContainerSheet::sheet()
}

stylesheet! {
    ModalSheetContainerSheet<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            // Stretch, not Center: a sheet is full-bleed horizontally.
            align_items: AlignItems::Stretch,
            justify_content: JustifyContent::FlexEnd,
            width: Length::pct(100.0),
            height: Length::pct(100.0),
        }
    }
}

/// Static sheet for the card's animation wrapper: starts at `opacity: 0` so
/// the card's first painted frame is invisible (the AnimatedValue fades it up
/// + slides it). Same first-frame-flicker reason as the `backdrop` slot's.
/// MUST be static (not in the surface's reactive width closure, which re-runs
/// on viewport change and would re-zero the alpha mid-animation). The AV binds
/// this view's `ViewHandle` (a pressable can't carry one), so it's a distinct
/// layer between the touch-consuming card pressable and the visible surface.
fn card_anim_sheet() -> Rc<StyleSheet> {
    ModalCardAnimSheet::sheet()
}

// The private Modal sheets are `stylesheet!` declarations, NOT ad-hoc
// closure sheets, and that choice is load-bearing for `--premint`: a
// Modal's styles first construct when it OPENS, and the premint dump's
// crawl mounts routes without opening anything — so a runtime-registered
// sheet here would get no build-time CSS and `--premint-only` would panic
// on the first open. The macro registers at LINK time, construction-
// independent (the `component_styles!` slots above register the same way).
stylesheet! {
    ModalCardAnimSheet<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            opacity: 0.0,
        }
    }
}

/// Transparent full-fill pressable that sits inside the backdrop layer and
/// catches taps. (The scrim color + fade live on the parent layer view so a
/// single opacity animation fades the whole backdrop.)
fn backdrop_hit_sheet() -> Rc<StyleSheet> {
    ModalBackdropHitSheet::sheet()
}

stylesheet! {
    ModalBackdropHitSheet<()> {
        base(_t) {
            width: Length::pct(100.0),
            height: Length::pct(100.0),
        }
    }
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
    ModalCardLayerSheet::sheet()
}

stylesheet! {
    ModalCardLayerSheet<()> {
        base(_t) {
            position: Position::Relative,
            flex_direction: FlexDirection::Column,
        }
    }
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
    ModalScrollSheet::sheet()
}

stylesheet! {
    ModalScrollSheet<()> {
        base(_t) {
            flex_grow: 0.0,
            flex_basis: Length::Auto,
            min_height: Length::Px(0.0),
            // The scroller is itself a column so its body child lays out
            // top-to-bottom and can grow past the visible region.
            flex_direction: FlexDirection::Column,
        }
    }
}

/// Fullscreen flex container that centers the card over the backdrop.
fn center_container_sheet() -> Rc<StyleSheet> {
    ModalCenterSheet::sheet()
}

stylesheet! {
    ModalCenterSheet<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            width: Length::pct(100.0),
            height: Length::pct(100.0),
        }
    }
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
    // Snapshotted for the same reason as `dismissable`/`width`: it drives
    // STRUCTURE (container, surface geometry, animation distance),
    // so a live change would need the whole overlay rebuilt. `presence`
    // rebuilds `build` per open, so a change between opens is picked up.
    let presentation = props.presentation.get();
    // Read LIVE (inside the container/surface style closures), unlike the
    // snapshots above: it moves the card but changes no structure.
    let top_offset = props.top_offset;
    let sheet_slide = props.sheet_slide;
    let styles = props.styles;

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
            presentation,
            top_offset.clone(),
            styles.clone(),
            sheet_slide.clone(),
        )
    };

    presence(build)
        .present(move || open.get())
        // Timing-only exit: hold the portal mounted EXIT_MS so the
        // animate-out (driven inside `build`) can play, then unmount. The
        // PresenceState is a no-op on the portal child; the visual lives on
        // the inner views.
        // Per presentation: a sheet's slide takes `SHEET_EXIT_MS`, and
        // unmounting at the card's 150ms would cut it off halfway down —
        // which looks exactly like the vanishing this replaced.
        .exit(PresenceAnim::new(
            PresenceState::default(),
            match presentation {
                ModalPresentation::Sheet => SHEET_EXIT_MS as u32,
                ModalPresentation::Centered | ModalPresentation::Top => EXIT_MS as u32,
            },
            Easing::EaseIn,
        ))
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
    presentation: ModalPresentation,
    top_offset: Reactive<Option<f32>>,
    styles: ModalSlotStyles,
    // App-owned slide animator, or `None` to make our own — see
    // [`ModalProps::sheet_slide`].
    sheet_slide: Option<AnimatedValue<f32>>,
) -> Element {
    let DismissRouting { backdrop: backdrop_handler, platform: platform_dismiss } =
        DismissRouting::resolve(on_dismiss, on_backdrop_press, dismissable);

    // Bidirectional animators: bound to the inner views, then driven by an
    // effect reading `open` — animate IN on open, OUT on close. The backdrop
    // fades only (no slide → no hard dark edge sweep); the card fades + slides.
    let backdrop_ref: Ref<ViewHandle> = Ref::new();
    let bd_opacity = AnimatedValue::new(0.0_f32);
    bd_opacity.bind(backdrop_ref, AnimProp::Opacity);
    let surface_ref: Ref<ViewHandle> = Ref::new();
    let card_opacity = AnimatedValue::new(0.0_f32);
    card_opacity.bind(surface_ref, AnimProp::Opacity);
    // Per presentation — see `enter_slide_px`.
    let slide_from = enter_slide_px(presentation);
    // An app-supplied animator outlives the open cycle (the app owns it), so
    // it still holds wherever the last exit left it. Reset BEFORE the bind:
    // `bind` re-applies the current value once the mount fills the ref, so
    // setting first is what stops a second open from starting a full
    // sheet-height below the screen.
    let card_slide = match sheet_slide {
        Some(av) => {
            av.set(slide_from);
            av
        }
        None => AnimatedValue::new(slide_from),
    };
    card_slide.bind(surface_ref, AnimProp::TranslateY);
    // Scope-adopted by the presence-mounted subtree: freed when presence
    // unmounts after exit. No `mem::forget`.
    let is_sheet = matches!(presentation, ModalPresentation::Sheet);
    let viewport_for_exit = viewport_size();
    effect!({
        let is_open = open.get();
        if is_open {
            bd_opacity.animate(TweenTo::new(1.0, Duration::from_millis(ENTER_MS)).ease_out());
            card_opacity.animate(TweenTo::new(1.0, Duration::from_millis(ENTER_MS)).ease_out());
            card_slide.animate(TweenTo::new(0.0, Duration::from_millis(ENTER_MS)).ease_out());
        } else if is_sheet {
            // A sheet LEAVES BY ITS EDGE. It keeps full opacity the whole
            // way down — fading it as it goes is what made this read as
            // "it just disappeared" rather than "it went back where it
            // came from". Only the backdrop fades.
            //
            // `ease_out`: a dismissal is a request to be rid of it, so
            // the surface has to COMMIT immediately and coast out.
            //
            // `ease_in` was measured and is wrong here. Slowed to 2s to
            // watch it, the sheet's top edge went 655 -> 666 -> 684 ->
            // 709 -> 745 -> 793: three quarters of the way through the
            // animation it had travelled an eighth of the distance. At
            // the real 240ms that is a few pixels of creep and then an
            // unmount with the sheet still on screen — which is exactly
            // the "it just disappears" this was meant to fix.
            let insets = safe_area_insets().get();
            let avail_h = viewport_for_exit.get().height - insets.top - insets.bottom;
            let ms = SHEET_EXIT_MS;
            bd_opacity.animate(TweenTo::new(0.0, Duration::from_millis(ms)).ease_out());
            card_slide.animate(
                TweenTo::new(sheet_exit_travel(avail_h), Duration::from_millis(ms)).ease_out(),
            );
        } else {
            bd_opacity.animate(TweenTo::new(0.0, Duration::from_millis(EXIT_MS)).ease_out());
            card_opacity.animate(TweenTo::new(0.0, Duration::from_millis(EXIT_MS)).ease_out());
            card_slide.animate(
                TweenTo::new(slide_from, Duration::from_millis(EXIT_MS)).ease_out(),
            );
        }
    });

    assemble_overlay(
        content,
        backdrop_ref,
        surface_ref,
        backdrop_handler,
        desired,
        presentation,
        top_offset,
        platform_dismiss,
        &styles,
    )
}

/// Where the modal's two dismissal gestures go.
struct DismissRouting {
    /// The backdrop tap: `on_backdrop_press` if set, else `on_dismiss`
    /// when dismissable, else inert.
    backdrop: Option<Rc<dyn Fn()>>,
    /// The platform gesture — Escape on web/desktop, the back key on
    /// Android: `on_dismiss` when dismissable. Gating only the backdrop on
    /// the flag would let a non-dismissable modal close on Escape.
    ///
    /// A non-dismissable modal hands the portal a handler that does
    /// nothing rather than none: the Android overlay attaches its back-key
    /// listener (which consumes the key) only when the portal has a
    /// handler, and without one the back key falls through to the
    /// Activity and pops the navigator screen UNDER the modal. The
    /// gesture is swallowed, not routed.
    platform: Option<Rc<dyn Fn()>>,
}

impl DismissRouting {
    fn resolve(
        on_dismiss: Option<Rc<dyn Fn()>>,
        on_backdrop_press: Option<Rc<dyn Fn()>>,
        dismissable: bool,
    ) -> Self {
        if dismissable {
            Self { backdrop: on_backdrop_press.or_else(|| on_dismiss.clone()), platform: on_dismiss }
        } else {
            Self { backdrop: on_backdrop_press, platform: Some(Rc::new(|| {})) }
        }
    }
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
    desired: f32,
    presentation: ModalPresentation,
    top_offset: Reactive<Option<f32>>,
    on_dismiss: Option<Rc<dyn Fn()>>,
    styles: &ModalSlotStyles,
) -> Element {
    // Backdrop layer: a full-bleed view (scrim color) that fades, with a
    // transparent pressable inside catching taps.
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
            .with_style(ModalStyles::backdrop().overridden(styles.backdrop.as_ref()))
            .bind(backdrop_ref)
            .into_element()
    };

    // Card: the `surface` slot + viewport-capped width AND height. The
    // frame carries the look (bg, radius, border, shadow); the padding is
    // the inner `body` slot's, so it lives *inside* the scroller. `overflow:
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
        .with_style(ModalStyles::body().overridden(styles.body.as_ref()))
        .into_element();
    // The scroller carries the viewport-derived height cap. It sizes to its
    // body's content (so a short modal hugs its content) until it hits
    // `max_height`, then `overflow:scroll` takes over. Reactive so the cap
    // tracks orientation / split-view resizes, mirroring the surface width.
    let viewport_for_scroll = viewport_size();
    let top_offset_for_scroll = top_offset.clone();
    let scroller = runtime_core::primitives::scroll_view::scroll_view(vec![body])
        // No spring when there is nothing to scroll.
        //
        // The framework turns `alwaysBounce*` on for every scroller it
        // makes, so that a short PAGE still feels live. A modal is the
        // other case: it is sized to its content and capped, so the
        // usual state is content that fits — and a surface that
        // rubber-bands under the finger says "this scrolls" when it
        // does not. On a SHEET it says something worse, because a
        // downward drag on a sheet is the gesture a reader expects to
        // dismiss it: the bounce reads as that gesture failing.
        //
        // `bounces` itself is untouched, so a modal whose content is
        // genuinely taller than the cap still springs at its ends the
        // way everything else on the platform does.
        .always_bounce(false)
        .with_style(move || {
            // Cap to the SAFE height: subtract the top + bottom safe-area
            // insets so a maximally-tall modal can't grow under the notch /
            // Dynamic Island / home indicator (the centering container also
            // pads by the insets — both are needed; see the overlay below).
            let insets = safe_area_insets().get();
            let avail_h = viewport_for_scroll.get().height - insets.top - insets.bottom;
            // Same cap the surface uses, or the scroller would want to be
            // taller than the frame that clips it and the overflow would be
            // clipped instead of scrolled.
            let max_h =
                presentation_max_height(presentation, avail_h, top_offset_for_scroll.get());
            // Measured from the live viewport — continuous, so inline.
            StyleApplication::new(modal_scroll_sheet()).with_inline(StyleRules {
                max_height: Some(Tokenized::Literal(Length::Px(max_h))),
                ..Default::default()
            })
        })
        .into_element();

    // Both presentations hold the scroller alone. A sheet grabber is the
    // app's to build — see [`ModalProps::sheet_slide`].
    let top_offset_for_surface = top_offset.clone();
    let surface_style = ModalStyles::surface()
        .inline_with(move || {
            let vp = viewport.get();
            // Cap width/height to the SAFE rect (subtract the horizontal /
            // vertical safe-area insets) so the card fits inside the notch /
            // home-indicator-free region. Mirrors the scroller's height cap.
            let insets = safe_area_insets().get();
            let max_h = presentation_max_height(
                presentation,
                vp.height - insets.top - insets.bottom,
                top_offset_for_surface.get(),
            );
            // A card is width-capped and centered; a sheet is full-bleed and
            // takes the whole width its stretching container gives it.
            let width = match presentation {
                ModalPresentation::Centered | ModalPresentation::Top => Length::Px(effective_modal_width(
                    desired,
                    vp.width - insets.left - insets.right,
                )),
                ModalPresentation::Sheet => Length::pct(100.0),
            };
            // A sheet meets the bottom edge, so its lower corners are square
            // and its background runs UNDER the home indicator while its
            // content stops above it (the bottom inset becomes frame padding
            // rather than being subtracted from the surface).
            let (bottom_radius, pad_bottom) = match presentation {
                ModalPresentation::Centered | ModalPresentation::Top => (None, 0.0),
                ModalPresentation::Sheet => {
                    (Some(Tokenized::Literal(Length::Px(0.0))), insets.bottom)
                }
            };
            // The `surface` slot carries `max_width: 560` as the THEME's
            // default cap, and a max-width beats `width` in every layout engine — so
            // an inline `width` alone could never exceed 560 and
            // `ModalProps::width = 1200` was silently a no-op (#109). For a
            // card the author's width, already viewport-capped by
            // `effective_modal_width`, IS the cap: pin `max_width` to it.
            // A sheet keeps the sheet's cap (it fills its container up to
            // it, which is what keeps a sheet phone-shaped on a tablet).
            let max_width = match presentation {
                ModalPresentation::Centered | ModalPresentation::Top => Some(Tokenized::Literal(width.clone())),
                ModalPresentation::Sheet => None,
            };
            StyleRules {
                width: Some(Tokenized::Literal(width)),
                max_width,
                max_height: Some(Tokenized::Literal(Length::Px(max_h))),
                // Clip the scroll content to the frame's rounded corners.
                overflow: Some(Overflow::Hidden),
                padding_bottom: (pad_bottom > 0.0).then(|| Tokenized::Literal(Length::Px(pad_bottom))),
                border_bottom_left_radius: bottom_radius.clone(),
                border_bottom_right_radius: bottom_radius,
                ..Default::default()
            }
        })
        // The author's per-instance surface override resolves last, on top
        // of the viewport-derived caps.
        .overridden(styles.surface.as_ref());
    let surface = runtime_core::view(vec![scroller]).with_style(surface_style).into_element();

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
    // Keep the card clear of the soft keyboard (a form in a modal). A portal
    // mounts outside the app root, so an app-level `keyboard_avoiding_view`
    // can't reach it — the modal brings its own. `Translate` around the CARD
    // only: it lifts by just what the card itself overlaps (a centered card a
    // little, a bottom sheet by the keyboard's height), nothing re-lays out,
    // and the dimming backdrop stays put.
    let card = runtime_core::keyboard_avoiding_view(vec![card])
        .behavior(runtime_core::KeyboardAvoidBehavior::Translate)
        .into_element();

    // One fullscreen portal: backdrop (behind) + card as siblings in a
    // placement container (centered, top-anchored or bottom-pinned).
    // `backdrop(None)` because we supply our own backdrop child; Escape/back
    // routes to `on_dismiss` (`None` for a non-dismissable modal, see
    // `DismissRouting`).
    let viewport_for_container = viewport_size();
    let mut overlay = runtime_core::overlay(vec![backdrop, card])
        .placement(ViewportPlacement::FullScreen)
        .backdrop(BackdropMode::None)
        // Place the card within the SAFE rect, not the full window: pad the
        // fullscreen placement container by the platform safe-area insets
        // (see `placement_container_style` for the per-presentation arms).
        // This is what guarantees the card clears the notch / Dynamic Island /
        // home indicator even when the insets are asymmetric (centering in the
        // full window would leave the card under the larger inset). The
        // dimming backdrop is `position: absolute; inset: 0`, which resolves
        // against the container's PADDING box — so it still fills the whole
        // window (verified in runtime-layout); only the card is constrained.
        // Reactive: orientation flips / sheet adaptations re-fire
        // `safe_area_insets()`. Insets are ZERO where no observer is wired
        // (web today), degrading to the previous full-window centering.
        .with_style(move || {
            let insets = safe_area_insets().get();
            // Only `Top` needs the safe height (its offset is a fraction of
            // it) and its offset; the other arms don't subscribe to them.
            let (safe_h, requested) = match presentation {
                ModalPresentation::Top => (
                    viewport_for_container.get().height - insets.top - insets.bottom,
                    top_offset.get(),
                ),
                _ => (0.0, None),
            };
            placement_container_style(presentation, insets, safe_h, requested)
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
    use crate::test_support::{classify, P, TStyle};
    use idea_theme::testing::with_test_world;
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::resolve_style;

    #[test]
    fn props_default_is_dismissable_with_default_width() {
        with_test_world(|| {
            let p = ModalProps::default();
            assert!(p.dismissable.get());
            assert!(p.styles.backdrop.is_none());
            assert!(p.on_backdrop_press.is_none());
            assert_eq!(p.width.get(), DEFAULT_MODAL_WIDTH);
    });
    }

    #[test]
    fn width_unclamped_when_viewport_has_room() {
        with_test_world(|| {
            assert_eq!(effective_modal_width(520.0, 1280.0), 520.0);
            assert_eq!(effective_modal_width(440.0, 1280.0), 440.0);
    });
    }

    #[test]
    fn regression_width_caps_to_viewport_on_a_phone() {
        with_test_world(|| {
            // The bug: a 520-wide surface centered on a 393pt phone overflowed
            // ~63pt off each edge because `max_width: 100%` didn't clamp against
            // the auto-sized portal wrapper on the native backends. The cap must
            // bring it to viewport - 2*margin so it fits with breathing room.
            let eff = effective_modal_width(520.0, 393.0);
            assert_eq!(eff, 393.0 - MODAL_EDGE_MARGIN * 2.0);
            assert!(eff < 393.0, "surface must fit within the viewport");
    });
    }

    #[test]
    fn width_never_shrinks_below_min_fit_on_tiny_viewports() {
        with_test_world(|| {
            let eff = effective_modal_width(520.0, 300.0);
            assert_eq!(eff, MODAL_MIN_FIT);
    });
    }

    #[test]
    fn max_height_caps_to_viewport_minus_margin() {
        with_test_world(|| {
            // A roomy phone in portrait: the surface may grow to the viewport
            // height minus the top+bottom margin, then scroll past that.
            let h = effective_modal_max_height(852.0);
            assert_eq!(h, 852.0 - MODAL_EDGE_MARGIN * 2.0);
            assert!(h < 852.0, "the surface must fit within the viewport height");
    });
    }

    #[test]
    fn regression_tall_modal_caps_to_viewport_height() {
        with_test_world(|| {
            // The bug: `ModalStyle` capped WIDTH (`max_width: 560`) but never
            // height, so a modal whose content was taller than the screen ran
            // off the top and bottom edges with no way to reach the clipped
            // parts. The height cap brings it to viewport - 2*margin so it fits;
            // the internal scroll_view then reaches the overflow.
            let short_viewport = 500.0;
            let h = effective_modal_max_height(short_viewport);
            assert_eq!(h, short_viewport - MODAL_EDGE_MARGIN * 2.0);
            assert!(h < short_viewport);
    });
    }

    #[test]
    fn max_height_never_shrinks_below_min_on_tiny_viewports() {
        with_test_world(|| {
            // A very short viewport (e.g. a landscape phone) still leaves a
            // usable, scrollable surface rather than collapsing to a sliver.
            let h = effective_modal_max_height(150.0);
            assert_eq!(h, MODAL_MIN_HEIGHT_FIT);
    });
    }

    /// Regression (CrewForge): `dismissable = false` with an
    /// `on_dismiss` still closed on Escape. The flag only gated the
    /// backdrop; `on_dismiss` went to the portal unconditionally, and the
    /// web portal installs its Escape listener whenever it has one.
    #[test]
    fn regression_non_dismissable_modal_ignores_escape() {
        let hits = Rc::new(std::cell::Cell::new(0));
        let h = hits.clone();
        let dismiss: Rc<dyn Fn()> = Rc::new(move || h.set(h.get() + 1));

        let r = DismissRouting::resolve(Some(dismiss.clone()), None, false);
        // The portal still gets a handler, so Android consumes the back
        // key instead of popping the screen under the modal...
        (r.platform.expect("the gesture is swallowed, not passed through"))();
        // ...but it never reaches `on_dismiss`.
        assert_eq!(hits.get(), 0, "Escape/back must not reach on_dismiss");
        assert!(r.backdrop.is_none(), "the backdrop is inert");

        // An explicit backdrop handler still fires on a non-dismissable one.
        let press: Rc<dyn Fn()> = Rc::new(|| {});
        let r = DismissRouting::resolve(Some(dismiss.clone()), Some(press), false);
        assert!(r.backdrop.is_some());

        // Dismissable: both gestures dismiss.
        let r = DismissRouting::resolve(Some(dismiss), None, true);
        (r.platform.expect("Escape/back dismiss"))();
        (r.backdrop.expect("the backdrop dismisses"))();
        assert_eq!(hits.get(), 2);
    }

    /// Regression: the Modal's card layer must be a touch-CONSUMING
    /// `pressable`, not a plain `view`.
    ///
    /// The Android portal is a single-window view overlay whose children
    /// are `[backdrop, card]` siblings in one `FrameLayout`. A tap on a
    /// non-clickable card `ViewGroup` returns `false` from `onTouchEvent`,
    /// so `FrameLayout` falls through to the lower backdrop child, whose
    /// pressable consumes the tap and dismisses the modal — tapping the
    /// card *closed* it. iOS/web don't fall through (topmost view at a
    /// point wins hit-testing). A `pressable` card layer (no-op handler)
    /// consumes taps on every backend.
    ///
    /// We assert the structure the fix relies on: the portal's second
    /// child (the card, painted above the first-child backdrop) is a
    /// `Pressable`. If a refactor reverts it to a plain `view`, the
    /// Android tap-through regression returns and this test fails.
    #[test]
    fn regression_modal_card_layer_consumes_touches() {
        with_test_world(|| {
            // Assemble the portal structure directly. We call `assemble_overlay`
            // (the pure structural half) rather than the full Modal because
            // `build_overlay` creates an `effect!`, which requires a live reactive
            // scope (provided by realize in real use, absent in a unit test).
            // `AnimatedValue::bind` no-ops without a backend and `viewport_size`
            // just returns a signal, so the structure builds fine here.
            let portal = assemble_overlay(
                runtime_core::text("hi").into_element(),
                Ref::new(),
                Ref::new(),
                None,
                DEFAULT_MODAL_WIDTH,
                ModalPresentation::Centered,
                Reactive::Static(None),
                None,
                &ModalSlotStyles::default(),
            );

            // The portal wraps the Modal's content in a single flex-center
            // content view, so it has ONE child (the center container) whose
            // children are [backdrop, card].
            let mut portal_children = match classify(portal) {
                P::Portal { children, .. } => children,
                _ => panic!("assemble_overlay should build a Portal"),
            };
            assert_eq!(
                portal_children.len(),
                1,
                "overlay wraps the Modal's children in one center-container view"
            );
            let mut layers = match classify(portal_children.remove(0)) {
                P::View { children, .. } => children,
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
                matches!(card_layer(layers.remove(1)), P::Pressable { .. }),
                "card layer must be a touch-consuming Pressable so a tap on \
                 the card doesn't fall through to the backdrop and dismiss \
                 the modal (Android FrameLayout fall-through)"
            );
    });
    }

    /// The card layer: a `Translate` keyboard avoider (a portal's content is
    /// outside any app-level avoider) wrapping the touch-consuming card
    /// `Pressable`. Returns the classified inner layer.
    fn card_layer(layer: Element) -> P {
        match classify(layer) {
            P::View { keyboard_avoid: Some(avoid), mut children, .. } => {
                assert_eq!(
                    avoid.behavior,
                    runtime_core::KeyboardAvoidBehavior::Translate,
                    "the card lifts clear of the keyboard without re-laying out"
                );
                assert_eq!(children.len(), 1, "the avoider wraps exactly the card");
                classify(children.remove(0))
            }
            _ => panic!(
                "card layer must be a keyboard_avoiding_view around the card, \
                 or a modal form sits under the soft keyboard"
            ),
        }
    }

    /// Find the body view — the first `View` with a direct `Text` child (the
    /// modal wraps `content` in the padded body view) — and return its resolved
    /// top padding in px.
    fn body_padding_top(portal: Element) -> f32 {
        // `classify` consumes, so the walk classifies each node once and
        // recurses over the normalized kinds.
        fn find_body(node: P) -> Option<TStyle> {
            let (children, style, is_view) = match node {
                P::View { children, style, .. } => (children, style, true),
                P::Pressable { children, .. }
                | P::ScrollView { children, .. }
                | P::Portal { children, .. } => (children, None, false),
                _ => return None,
            };
            let kids: Vec<P> = children.into_iter().map(classify).collect();
            if is_view && kids.iter().any(|k| matches!(k, P::Text { .. })) {
                return Some(style.expect("body carries a style"));
            }
            kids.into_iter().find_map(find_body)
        }
        let style =
            find_body(classify(portal)).expect("modal has a body view wrapping the content");
        let app = match style {
            TStyle::App(a) => a,
            _ => panic!("body uses a static style"),
        };
        match resolve_style(&app).padding_top.as_ref().map(|t| t.resolve()) {
            Some(runtime_core::Length::Px(v)) => v,
            other => panic!("expected a px top padding, got {other:?}"),
        }
    }

    /// User-reported `--premint-only` panic: opening ANY Modal (the docs
    /// search dialog was the repro) hit the stripped-engine violation.
    /// Modal's sheets construct on OPEN, and the premint dump's crawl
    /// mounts routes without opening anything — so runtime-registered
    /// sheets got no build-time CSS. Every Modal sheet must therefore be
    /// a `stylesheet!` (LINK-time registration, construction-independent)
    /// and every default-modal application must premint; the safe-area
    /// padding rides the inline layer (a `with_computed` layer would
    /// disqualify the centering overlay). Fails against the
    /// closure-sheet form: those have no premint class at all.
    #[test]
    fn regression_default_modal_styles_all_premint() {
        with_test_world(|| {
            for (name, sheet) in [
                ("backdrop_hit", backdrop_hit_sheet()),
                ("card_anim", card_anim_sheet()),
                ("card_layer", card_layer_sheet()),
                ("scroll", modal_scroll_sheet()),
                ("center", center_container_sheet()),
                ("top", top_container_sheet()),
                ("sheet_container", sheet_container_sheet()),
            ] {
                assert!(
                    StyleApplication::new(sheet).preminted_class_list().is_some(),
                    "the {name} sheet must premint — Modal styles first construct \
                     on open, which the dump crawl never does, so only link-time \
                     registration gets them build-time CSS"
                );
            }
            for (name, app) in [
                ("surface", ModalStyles::surface().into_style_application()),
                ("body", ModalStyles::body().into_style_application()),
                ("backdrop", ModalStyles::backdrop().into_style_application()),
            ] {
                assert!(
                    app.preminted_class_list().is_some(),
                    "the {name} slot must premint without the Modal ever opening"
                );
            }
            // The safe-area centering layer must ride INLINE, keeping the
            // overlay's application premintable.
            let app = StyleApplication::new(center_container_sheet()).with_inline(StyleRules {
                padding_top: Some(Tokenized::Literal(Length::Px(47.0))),
                ..Default::default()
            });
            assert!(
                app.preminted_class_list().is_some(),
                "the safe-area inset layer must not drag the overlay onto the engine"
            );
        });
    }

    // Regression (edge-to-edge header couldn't sit flush): the body slot
    // carries `spacing-lg` padding. A per-instance `styles.body` override with
    // zero padding must win, letting an illustration header bleed to the
    // surface edge.
    #[test]
    fn body_slot_override_makes_body_flush() {
        with_test_world(|| {
            install_idea_theme(light_theme());

            let default_portal = assemble_overlay(
                runtime_core::text("hi").into_element(),
                Ref::new(),
                Ref::new(),
                None,
                DEFAULT_MODAL_WIDTH,
                ModalPresentation::Centered,
                Reactive::Static(None),
                None,
                &ModalSlotStyles::default(),
            );
            // Under the premint cfg the DEFAULT body application premints
            // to an opaque class stamp — its rules aren't resolvable from
            // the test mirror, and the premintability itself is pinned by
            // `regression_default_modal_styles_all_premint`. The override
            // half below stays on a resolvable application either way,
            // and it is the regression subject here.
            #[cfg(not(idealyst_premint))]
            assert!(
                body_padding_top(default_portal) > 0.0,
                "the default body has non-zero padding (spacing-lg)"
            );
            #[cfg(idealyst_premint)]
            drop(default_portal);

            let flush = StyleRules {
                padding_top: Some(Tokenized::Literal(Length::Px(0.0))),
                padding_right: Some(Tokenized::Literal(Length::Px(0.0))),
                padding_bottom: Some(Tokenized::Literal(Length::Px(0.0))),
                padding_left: Some(Tokenized::Literal(Length::Px(0.0))),
                ..Default::default()
            };
            let flush_portal = assemble_overlay(
                runtime_core::text("hi").into_element(),
                Ref::new(),
                Ref::new(),
                None,
                DEFAULT_MODAL_WIDTH,
                ModalPresentation::Centered,
                Reactive::Static(None),
                None,
                &ModalSlotStyles::default().body(flush),
            );
            assert_eq!(
                body_padding_top(flush_portal),
                0.0,
                "a body padding:0 override makes the body flush"
            );
    });
    }

    #[test]
    fn props_default_presentation_is_centered() {
        with_test_world(|| {
            // Every call site that existed before `presentation` keeps the
            // centered card it was written against.
            assert_eq!(
                ModalProps::default().presentation.get(),
                ModalPresentation::Centered
            );
    });
    }

    #[test]
    fn sheet_cap_leaves_a_strip_of_the_page_visible() {
        with_test_world(|| {
            // A sheet is layered OVER what you were reading; if it could grow
            // to the full height it would read as a new screen instead.
            let safe = 852.0;
            let h = presentation_max_height(ModalPresentation::Sheet, safe, None);
            assert_eq!(h, safe * SHEET_MAX_HEIGHT_FRACTION);
            assert!(h < safe, "a sheet never covers the whole viewport");
    });
    }

    #[test]
    fn sheet_and_card_caps_differ_at_the_same_height() {
        with_test_world(|| {
            // The two arms are genuinely different geometry, not the same
            // number reached twice: a card keeps a fixed margin top and
            // bottom, a sheet takes a fraction.
            let safe = 852.0;
            let card = presentation_max_height(ModalPresentation::Centered, safe, None);
            let sheet = presentation_max_height(ModalPresentation::Sheet, safe, None);
            assert_eq!(card, safe - MODAL_EDGE_MARGIN * 2.0);
            assert!(
                sheet < card,
                "a sheet stops well short of a card's cap ({sheet} vs {card})"
            );
    });
    }

    #[test]
    fn sheet_cap_never_shrinks_below_min_on_tiny_viewports() {
        with_test_world(|| {
            // Mirrors `max_height_never_shrinks_below_min_on_tiny_viewports`
            // for the sheet arm: a landscape phone still gets a usable,
            // scrollable surface rather than a sliver.
            assert_eq!(
                presentation_max_height(ModalPresentation::Sheet, 150.0, None),
                MODAL_MIN_HEIGHT_FIT
            );
    });
    }

    /// Walk `[center] → [backdrop, card] → anim view → surface` and return
    /// the surface's resolved rules.
    fn surface_rules(portal: Element) -> Rc<StyleRules> {
        let mut portal_children = match classify(portal) {
            P::Portal { children, .. } => children,
            _ => panic!("assemble_overlay should build a Portal"),
        };
        let mut layers = match classify(portal_children.remove(0)) {
            P::View { children, .. } => children,
            _ => panic!("portal child should be the placement container"),
        };
        let mut card = match card_layer(layers.remove(1)) {
            P::Pressable { children, .. } => children,
            _ => panic!("card layer should be a Pressable"),
        };
        let mut anim = match classify(card.remove(0)) {
            P::View { children, .. } => children,
            _ => panic!("card layer wraps the animation view"),
        };
        match classify(anim.remove(0)) {
            P::View { style: Some(style), .. } => style.resolve(),
            _ => panic!("animation view wraps a styled surface"),
        }
    }

    /// The width the surface actually lays out at: `width`, clamped by
    /// `max_width` — the clamp every backend's layout engine applies.
    fn laid_out_width(rules: &StyleRules) -> f32 {
        let px = |l: Option<&Tokenized<Length>>| match l.map(|t| t.resolve()) {
            Some(Length::Px(v)) => Some(v),
            None => None,
            other => panic!("expected a px length, got {other:?}"),
        };
        let width = px(rules.width.as_ref()).expect("the surface carries a width");
        match px(rules.max_width.as_ref()) {
            Some(max) => width.min(max),
            None => width,
        }
    }

    /// Regression #109: `ModalProps::width` had no effect above 560.
    /// `ModalStyle` caps `max_width: 560` and the surface only set an
    /// inline `width`, so a max-width that beats width in every layout
    /// engine silently clamped `width = 1200` back to 560.
    #[test]
    fn regression_modal_width_above_560_is_honored() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            runtime_core::viewport_size().set(runtime_core::ViewportSize {
                width: 1600.0,
                height: 1000.0,
            });
            idea_theme::testing::commit();
            let portal = assemble_overlay(
                runtime_core::text("hi").into_element(),
                Ref::new(),
                Ref::new(),
                None,
                1200.0,
                ModalPresentation::Centered,
                Reactive::Static(None),
                None,
                &ModalSlotStyles::default(),
            );
            let w = laid_out_width(&surface_rules(portal));
            assert_eq!(w, 1200.0, "an author width of 1200 lays out at 1200, not the sheet's 560 cap");
        });
    }

    /// The author width stays viewport-capped, like the default: a
    /// 1200-wide card on an 800-wide window fits with a margin each side.
    #[test]
    fn wide_modal_width_still_caps_to_the_viewport() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            runtime_core::viewport_size().set(runtime_core::ViewportSize {
                width: 800.0,
                height: 1000.0,
            });
            idea_theme::testing::commit();
            let portal = assemble_overlay(
                runtime_core::text("hi").into_element(),
                Ref::new(),
                Ref::new(),
                None,
                1200.0,
                ModalPresentation::Centered,
                Reactive::Static(None),
                None,
                &ModalSlotStyles::default(),
            );
            assert_eq!(
                laid_out_width(&surface_rules(portal)),
                800.0 - MODAL_EDGE_MARGIN * 2.0
            );
        });
    }

    /// A sheet's surface holds the scroller ALONE, exactly like a centered
    /// card's — the two presentations differ in geometry and animation, not
    /// in structure.
    ///
    /// A built-in grabber + drag-to-dismiss gesture belongs to the app, not
    /// the modal: the bar is a lie unless something drags it, the gesture
    /// stuttered on iOS (two touch origins 8px apart reaching one
    /// recognizer), and what a downward drag MEANS is a screen's decision.
    /// An app that wants one builds it in `content` and drives the surface
    /// through `ModalProps::sheet_slide`. If a child reappears in this arm,
    /// that decision is being taken back.
    ///
    /// This also pins the Android fall-through fix for the sheet arm — the
    /// card layer must stay a `Pressable` in both presentations, or a tap on
    /// a sheet dismisses it on Android (see
    /// `regression_modal_card_layer_consumes_touches`).
    #[test]
    fn a_sheet_surface_holds_the_scroller_alone() {
        with_test_world(|| {
            let surface_children = |presentation| {
                let portal = assemble_overlay(
                    runtime_core::text("hi").into_element(),
                    Ref::new(),
                    Ref::new(),
                    None,
                    DEFAULT_MODAL_WIDTH,
                    presentation,
                    Reactive::Static(None),
                    None,
                    &ModalSlotStyles::default(),
                );
                let mut portal_children = match classify(portal) {
                    P::Portal { children, .. } => children,
                    _ => panic!("assemble_overlay should build a Portal"),
                };
                let mut layers = match classify(portal_children.remove(0)) {
                    P::View { children, .. } => children,
                    _ => panic!("portal child should be the placement container"),
                };
                assert_eq!(layers.len(), 2, "container holds [backdrop, card]");
                let mut card = match card_layer(layers.remove(1)) {
                    P::Pressable { children, .. } => children,
                    _ => panic!(
                        "the card layer must stay a touch-consuming Pressable in \
                         BOTH presentations, or a tap on the surface falls \
                         through to the backdrop on Android"
                    ),
                };
                let mut anim = match classify(card.remove(0)) {
                    P::View { children, .. } => children,
                    _ => panic!("card layer wraps the animation view"),
                };
                match classify(anim.remove(0)) {
                    P::View { children, .. } => children.len(),
                    _ => panic!("animation view wraps the surface"),
                }
            };

            assert_eq!(
                surface_children(ModalPresentation::Centered),
                1,
                "a centered card holds the scroller alone"
            );
            assert_eq!(
                surface_children(ModalPresentation::Top),
                1,
                "a top-anchored card holds the scroller alone"
            );
            assert_eq!(
                surface_children(ModalPresentation::Sheet),
                1,
                "a sheet holds the scroller alone too — no grabber, no \
                 gesture. Drag-to-dismiss is the app's to build, over \
                 `ModalProps::sheet_slide`."
            );
    });
    }

    // ---- ModalPresentation::Top (CrewForge report: a command palette's
    // search field moved under the caret as results filtered, because the
    // only placement on offer re-centered the card on every height change).

    /// The placement container for `Top` must anchor the card's top edge:
    /// `justify_content: FlexStart` and the offset added to the top inset.
    /// Under the centered container (`ModalCenterSheet`, `justify_content:
    /// Center`) a card that grew 35px moved its top up ~17px.
    #[test]
    fn regression_top_modal_anchors_card_top_edge() {
        with_test_world(|| {
            let insets = EdgeInsets { top: 47.0, right: 0.0, bottom: 34.0, left: 0.0 };
            let safe_h = 900.0;
            let rules = resolve_style(&placement_container_style(
                ModalPresentation::Top,
                insets,
                safe_h,
                None,
            ));
            assert_eq!(rules.justify_content, Some(JustifyContent::FlexStart));
            assert_eq!(rules.align_items, Some(AlignItems::Center));
            let pad_top = match rules.padding_top.as_ref().map(|t| t.resolve()) {
                Some(Length::Px(v)) => v,
                other => panic!("expected a px top padding, got {other:?}"),
            };
            assert_eq!(pad_top, 47.0 + safe_h * TOP_OFFSET_FRACTION);
            // The bottom inset is still honored (the card can't grow under
            // the home indicator).
            assert_eq!(
                rules.padding_bottom.as_ref().map(|t| t.resolve()),
                Some(Length::Px(34.0))
            );
        });
    }

    /// `Centered` keeps the exact container it had before `Top` existed:
    /// centered on both axes, padded by every inset, no offset.
    #[test]
    fn centered_container_is_unchanged_by_top() {
        with_test_world(|| {
            let insets = EdgeInsets { top: 47.0, right: 5.0, bottom: 34.0, left: 6.0 };
            let rules = resolve_style(&placement_container_style(
                ModalPresentation::Centered,
                insets,
                900.0,
                // Ignored by every arm but `Top`.
                Some(200.0),
            ));
            assert_eq!(rules.justify_content, Some(JustifyContent::Center));
            assert_eq!(rules.align_items, Some(AlignItems::Center));
            let px = |l: &Option<Tokenized<Length>>| l.as_ref().map(|t| t.resolve());
            assert_eq!(px(&rules.padding_top), Some(Length::Px(47.0)));
            assert_eq!(px(&rules.padding_right), Some(Length::Px(5.0)));
            assert_eq!(px(&rules.padding_bottom), Some(Length::Px(34.0)));
            assert_eq!(px(&rules.padding_left), Some(Length::Px(6.0)));
        });
    }

    #[test]
    fn top_offset_explicit_value_is_honored() {
        with_test_world(|| {
            assert_eq!(resolve_top_offset(Some(80.0), 900.0), 80.0);
            // Flush to the safe top is a legitimate request.
            assert_eq!(resolve_top_offset(Some(0.0), 900.0), 0.0);
            assert_eq!(resolve_top_offset(Some(-10.0), 900.0), 0.0);
            let rules = resolve_style(&placement_container_style(
                ModalPresentation::Top,
                EdgeInsets::ZERO,
                900.0,
                Some(80.0),
            ));
            assert_eq!(
                rules.padding_top.as_ref().map(|t| t.resolve()),
                Some(Length::Px(80.0))
            );
        });
    }

    #[test]
    fn top_offset_default_is_a_fraction_of_the_safe_height() {
        with_test_world(|| {
            assert_eq!(resolve_top_offset(None, 1000.0), 1000.0 * TOP_OFFSET_FRACTION);
            assert_eq!(resolve_top_offset(None, 800.0), 800.0 * TOP_OFFSET_FRACTION);
        });
    }

    /// On a viewport too short for the offset AND a usable card, the offset
    /// gives way: the card keeps `MODAL_MIN_HEIGHT_FIT` above a margin and
    /// never runs off the bottom.
    #[test]
    fn top_offset_gives_way_on_a_short_viewport() {
        with_test_world(|| {
            let safe = 260.0;
            let offset = resolve_top_offset(Some(120.0), safe);
            assert_eq!(offset, safe - MODAL_EDGE_MARGIN - MODAL_MIN_HEIGHT_FIT);
            let cap = presentation_max_height(ModalPresentation::Top, safe, Some(120.0));
            assert_eq!(cap, MODAL_MIN_HEIGHT_FIT);
            assert!(offset + cap + MODAL_EDGE_MARGIN <= safe, "card fits the safe rect");
        });
    }

    /// A top-anchored card is capped to the room BELOW its offset (minus
    /// the bottom margin), so tall content scrolls inside the card instead
    /// of pushing it off the bottom of the screen.
    #[test]
    fn top_cap_is_the_room_below_the_offset() {
        with_test_world(|| {
            let safe = 900.0;
            let offset = resolve_top_offset(None, safe);
            assert_eq!(
                presentation_max_height(ModalPresentation::Top, safe, None),
                safe - offset - MODAL_EDGE_MARGIN
            );
            assert_eq!(
                presentation_max_height(ModalPresentation::Top, safe, Some(100.0)),
                safe - 100.0 - MODAL_EDGE_MARGIN
            );
            // The `Top` request never leaks into the other arms.
            assert_eq!(
                presentation_max_height(ModalPresentation::Centered, safe, Some(100.0)),
                effective_modal_max_height(safe)
            );
        });
    }

    /// A top-anchored card drops toward the edge it hangs from; a centered
    /// card still rises the same 14px it always did.
    #[test]
    fn top_card_enters_from_above_centered_unchanged() {
        assert_eq!(enter_slide_px(ModalPresentation::Centered), CARD_SLIDE_PX);
        assert_eq!(enter_slide_px(ModalPresentation::Top), -CARD_SLIDE_PX);
        assert_eq!(enter_slide_px(ModalPresentation::Sheet), SHEET_SLIDE_PX);
    }

    /// The `Top` surface is a width-capped card, like `Centered`: the
    /// author width is honored and viewport-capped, not stretched like a
    /// sheet's.
    #[test]
    fn top_surface_is_width_capped_like_a_card() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            runtime_core::viewport_size().set(runtime_core::ViewportSize {
                width: 800.0,
                height: 1000.0,
            });
            idea_theme::testing::commit();
            let portal = assemble_overlay(
                runtime_core::text("hi").into_element(),
                Ref::new(),
                Ref::new(),
                None,
                1200.0,
                ModalPresentation::Top,
                Reactive::Static(None),
                None,
                &ModalSlotStyles::default(),
            );
            let rules = surface_rules(portal);
            assert_eq!(laid_out_width(&rules), 800.0 - MODAL_EDGE_MARGIN * 2.0);
            let offset = resolve_top_offset(None, 1000.0);
            assert_eq!(
                rules.max_height.as_ref().map(|t| t.resolve()),
                Some(Length::Px(1000.0 - offset - MODAL_EDGE_MARGIN))
            );
        });
    }
}
