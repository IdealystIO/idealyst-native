//! `HoverCard` — a rich, non-modal card that opens while the pointer rests
//! on a trigger: a per-scope breakdown under a progress bar, a profile
//! preview under a name. Like [`Tooltip`](super::tooltip::Tooltip) it
//! *wraps* its trigger and owns its own visibility; unlike a Tooltip the
//! card holds any content and the pointer can move into it.
//!
//! ```ignore
//! ui! {
//!     HoverCard(content = move || ui! {
//!         Typography(content = "Drilling 64% · Bolting 40%".to_string())
//!     }) {
//!         Progress(value = 0.52)
//!     }
//! }
//! ```
//!
//! `content` is a closure, not a `{ }` block: the card is built afresh on
//! every open (the `{ }` block is the trigger, which is built once).
//!
//! ## How it opens and closes
//!
//! - **Pointer (desktop / web)** — `on_hover` on the trigger wrapper AND on
//!   the card. Resting on either for [`HoverCardProps::open_delay_ms`]
//!   opens it; leaving both for [`HoverCardProps::close_delay_ms`] closes
//!   it. The close delay is what bridges the `offset` gap between trigger
//!   and card: leaving the trigger starts the close timer, entering the
//!   card cancels it. A pass over the trigger shorter than the open delay
//!   never opens anything.
//! - **Touch (iOS / Android)** — no hover with a finger, so a long-press
//!   on the trigger opens the card at once. A tap anywhere off the card
//!   then closes it: while a touch-opened card is up, a fullscreen,
//!   transparent click-away catcher (the one `Popover` uses) sits behind
//!   it, so the page under the card does not scroll or take presses until
//!   the card is closed.
//! - **Keyboard** — keyboard focus on the trigger (Tab onto the button it
//!   wraps; `on_focus` on the wrapper, which hears its children) opens the
//!   card at once: no open delay, since a focus is deliberate where a
//!   pointer pass may not be. Focus leaving the trigger closes it after
//!   [`HoverCardProps::close_delay_ms`], unless focus moved into the card
//!   (or the pointer rests on either) — the same bridge the pointer uses.
//!   A click that focuses the trigger is not keyboard focus and does not
//!   pin the card open.
//! - **Escape** closes it (the anchored portal's `on_dismiss`). While focus
//!   stays on the trigger it stays closed; focusing the trigger again
//!   reopens it.
//! - **`disabled`** — while it is `true` nothing opens the card, and turning
//!   it on closes an open card at once. Bind it to app state the card would
//!   misdescribe, e.g. `disabled = dragging` while a row is dragged:
//!   the card would keep showing the row where it was. Turning it off opens
//!   nothing by itself; the next hover, focus or long-press does.
//!
//! ## No backdrop on hover
//!
//! A pointer-opened card has no backdrop of any kind. A backdrop — even
//! the transparent click-away catcher a `Popover` lays over the page —
//! covers the trigger, so the pointer "leaves" the trigger the moment the
//! card opens and the card closes again: driven by hover, a `Popover`
//! flickers shut. The card's anchored portal is sized to the card and
//! covers nothing else, so the page and the trigger stay live (hoverable,
//! clickable, scrollable) while it is open.
//!
//! ## Accessibility
//!
//! The card carries `Role::Tooltip` — it describes its trigger and does
//! not take focus itself. A keyboard user sees it by focusing the trigger,
//! which needs a focusable trigger: a card around a `Button` opens on Tab,
//! one around a bare `Progress` (nothing focusable) cannot be reached by
//! keyboard — keep what such a card says available elsewhere. What takes
//! keyboard focus is the platform's: a `Button` (a `pressable`) on web,
//! macOS (Tab with Full Keyboard Access, as for any AppKit button) and
//! Android (keyboard / D-pad); text inputs everywhere. A `pressable` is not
//! a focus target on Linux, Windows or iOS, so there only a text input
//! inside the trigger opens it from the keyboard (see `on_focus` in the
//! primitives docs). A touch-only phone has no keyboard focus; the
//! long-press opens the card there.

use std::cell::Cell;
use std::rc::Rc;

use runtime_core::primitives::overlay::BackdropMode;
use runtime_core::primitives::portal::{AnchorTarget, ElementAlign, ElementSide};
use runtime_core::{
    after_ms_detached, component, component_styles, long_press, signal, ui, ChildList, Color,
    Element, FlexDirection, IdealystSchema, IntoElement, LongPressRecognizer, Reactive, Ref, Role,
    ScopeAlive, Signal, ViewHandle,
};

use crate::components::popover::dismiss_catcher;
use crate::theme::IdeaThemeRef;
// Named by `#[prop(example = …)]` values, which compile only in docs builds.
#[allow(unused_imports)]
use crate::{typography_kind, Stack, StackGap, Tag, Typography};

component_styles! {
    /// The styled sections of [`HoverCard`]: the card surface.
    pub HoverCard<IdeaThemeRef> {
        slot card {
            base(t) {
                background: t.color.surface(),
                padding: t.spacing.sm(),
                border_radius: t.radius.md(),
                border_width: 1.0,
                border_color: t.color.border(),
                gap: t.spacing.xs(),
                flex_direction: FlexDirection::Column,
                shadow: runtime_core::Shadow {
                    x: 0.0,
                    y: 6.0,
                    blur: 18.0,
                    color: Color("rgba(15, 17, 21, 0.18)".into()).into(),
                },
            }
            transitions {
                background: 250ms EaseInOut,
                border_color: 250ms EaseInOut,
            }
        }
    }
}

/// Default [`HoverCardProps::open_delay_ms`]: how long the pointer rests
/// on the trigger before the card opens. Long enough that sweeping the
/// pointer across a row of triggers opens nothing.
pub const HOVER_CARD_OPEN_DELAY_MS: u32 = 400;

/// Default [`HoverCardProps::close_delay_ms`]: how long after the pointer
/// leaves both the trigger and the card before the card closes. It has to
/// cover crossing the `offset` gap from the trigger into the card.
pub const HOVER_CARD_CLOSE_DELAY_MS: u32 = 200;

/// Default [`HoverCardProps::offset`]: the gap in px between the trigger
/// and the card.
pub const HOVER_CARD_DEFAULT_OFFSET: f32 = 6.0;

/// Builds the card body ([`HoverCardProps::content`]). `From<closure>` lets
/// a call site pass a bare `move || ui! { … }`. A closure because the card
/// is rebuilt on each open.
#[derive(Clone, runtime_core::Remote)]
pub struct HoverCardContent(pub Rc<dyn Fn() -> Element>);

impl<F: Fn() -> Element + 'static> From<F> for HoverCardContent {
    fn from(f: F) -> Self {
        HoverCardContent(Rc::new(f))
    }
}

impl Default for HoverCardContent {
    fn default() -> Self {
        HoverCardContent(Rc::new(|| runtime_core::view(Vec::new()).into_element()))
    }
}

// Reactive-by-default: `#[props]` wraps the scalar data props (`side` /
// `align` enums, `offset`, the two delays) → `Reactive<…>`. `content` is an
// element-builder newtype (`#[prop(static)]`), the test ids are
// `&'static str` handles (`#[prop(static)]`), and `children` is a
// `Vec<Element>` (auto-skipped).
#[runtime_core::props(styles = HoverCardStyles)]
#[derive(IdealystSchema)]
pub struct HoverCardProps {
    /// Builds the card body, laid out in a column inside the card surface.
    /// Author it as `content = move || ui! { … }`; it is rebuilt on each
    /// open, so state inside it starts fresh every time.
    #[prop(static, example = || ui! {
        Stack(gap = StackGap::Xs) {
            Typography(content = "Ada Lovelace", kind = typography_kind::H3)
            Typography(content = "Wrote the first published program, 1843.", muted = true)
        }
    })]
    pub content: HoverCardContent,
    /// Which side of the trigger the card sits on. Default `Below`.
    pub side: ElementSide,
    /// Alignment along the anchor edge. Default `Start`.
    pub align: ElementAlign,
    /// Gap in px between the trigger and the card. Default
    /// [`HOVER_CARD_DEFAULT_OFFSET`].
    #[schema(constraint = "pixels, >= 0")]
    pub offset: f32,
    /// How long the pointer rests on the trigger before the card opens, in
    /// ms. Default [`HOVER_CARD_OPEN_DELAY_MS`]. Touch (long-press) opens
    /// at once.
    pub open_delay_ms: u32,
    /// How long after the pointer leaves both the trigger and the card
    /// before it closes, in ms. Default [`HOVER_CARD_CLOSE_DELAY_MS`].
    pub close_delay_ms: u32,
    /// When `true`, the card does not open (hover, focus or long-press), and
    /// a card already open closes. Default `false`. Reactive: pass a
    /// `Signal<bool>` (`disabled = dragging`) to suppress the card
    /// while app state would make it stale, such as during a drag.
    #[schema(constraint = "reactive: static bool or Signal/rx!")]
    pub disabled: bool,
    /// Optional robot/E2E test id on the trigger wrapper — the view that
    /// takes the hover and the long-press. Inert outside a robot build.
    #[prop(static)]
    pub test_id: Option<&'static str>,
    /// Optional robot/E2E test id on the card surface, present while the
    /// card is open. Inert outside a robot build.
    #[prop(static)]
    pub card_test_id: Option<&'static str>,
    /// The trigger the card wraps and anchors to.
    #[prop(example = ui! { Tag(label = "@ada") })]
    pub children: Vec<Element>,
}

impl Default for HoverCardProps {
    fn default() -> Self {
        Self {
            content: HoverCardContent::default(),
            side: Reactive::Static(ElementSide::Below),
            align: Reactive::Static(ElementAlign::Start),
            offset: Reactive::Static(HOVER_CARD_DEFAULT_OFFSET),
            open_delay_ms: Reactive::Static(HOVER_CARD_OPEN_DELAY_MS),
            close_delay_ms: Reactive::Static(HOVER_CARD_CLOSE_DELAY_MS),
            disabled: Reactive::Static(false),
            test_id: None,
            card_test_id: None,
            children: Vec::new(),
            styles: Default::default(),
        }
    }
}

/// The open/close state machine shared by the trigger's and the card's
/// handlers. It is the only writer of `open` / `touch_open`.
///
/// Every hover change re-decides from where the pointer is now (`over_*`)
/// and arms one timer stamped with a fresh `generation`; a timer whose
/// stamp is no longer current is stale and does nothing. That is the
/// cancellation: a leave followed by an enter before the close delay
/// elapses leaves the close timer stale. `after_ms_detached` rather than
/// `after_ms_scoped` because the timers are armed from event handlers,
/// which run outside any component scope; `alive` stops a timer that
/// outlives the component from writing its signals.
struct HoverIntent {
    open: Signal<bool>,
    /// The card was opened by a long-press, so it renders the click-away
    /// catcher. Never set by hover (see the module docs).
    touch_open: Signal<bool>,
    shown: Cell<bool>,
    over_trigger: Cell<bool>,
    over_card: Cell<bool>,
    /// Keyboard focus is on the trigger / inside the card (`on_focus`).
    focus_trigger: Cell<bool>,
    focus_card: Cell<bool>,
    generation: Cell<u64>,
    open_delay_ms: Reactive<u32>,
    close_delay_ms: Reactive<u32>,
    /// [`HoverCardProps::disabled`]: read untracked by the open paths; its
    /// changes reach [`HoverIntent::set_disabled`] through a scope-owned
    /// binding ([`HoverIntent::bind_disabled`]).
    disabled: Reactive<bool>,
    alive: ScopeAlive,
}

impl HoverIntent {
    /// A closed card's state machine. Must run in the component body: the
    /// signals and the `alive` token belong to its scope.
    fn new(open_delay_ms: Reactive<u32>, close_delay_ms: Reactive<u32>, disabled: Reactive<bool>) -> Rc<Self> {
        Rc::new(HoverIntent {
            open: signal(false),
            touch_open: signal(false),
            shown: Cell::new(false),
            over_trigger: Cell::new(false),
            over_card: Cell::new(false),
            focus_trigger: Cell::new(false),
            focus_card: Cell::new(false),
            generation: Cell::new(0),
            open_delay_ms,
            close_delay_ms,
            disabled,
            alive: ScopeAlive::current(),
        })
    }

    /// Follow `disabled` for the component's life: a static `false` costs
    /// nothing, a live one closes the card each time it turns `true`.
    fn bind_disabled(self: &Rc<Self>) {
        let me = self.clone();
        self.disabled.clone().bind(move |disabled| me.set_disabled(disabled));
    }

    fn is_disabled(&self) -> bool {
        self.disabled.get_untracked()
    }

    /// `disabled` turned on: close an open card and stale any pending open
    /// timer. Turning it off opens nothing (module docs).
    fn set_disabled(&self, disabled: bool) {
        if disabled {
            self.bump();
            if self.shown.get() {
                self.set_shown(false, false);
            }
        }
    }

    fn hover_trigger(self: &Rc<Self>, entering: bool) {
        self.over_trigger.set(entering);
        self.settle();
    }

    fn hover_card(self: &Rc<Self>, entering: bool) {
        self.over_card.set(entering);
        self.settle();
    }

    /// Keyboard focus reached the trigger: open now (module docs). Losing
    /// it re-decides like a pointer leave, so focus moving into the card
    /// inside the close delay keeps it open.
    fn focus_trigger(self: &Rc<Self>, focused: bool) {
        self.focus_trigger.set(focused);
        if focused && self.is_disabled() {
            return;
        }
        if focused {
            self.bump();
            if !self.shown.get() {
                self.set_shown(true, false);
            }
        } else {
            self.settle();
        }
    }

    fn focus_card(self: &Rc<Self>, focused: bool) {
        self.focus_card.set(focused);
        self.settle();
    }

    fn bump(&self) -> u64 {
        let g = self.generation.get().wrapping_add(1);
        self.generation.set(g);
        g
    }

    /// Re-decide after a hover change: open after the open delay when the
    /// pointer is over either box, close after the close delay when it is
    /// over neither. Any pending timer goes stale.
    fn settle(self: &Rc<Self>) {
        let want = !self.is_disabled()
            && (self.over_trigger.get()
                || self.over_card.get()
                || self.focus_trigger.get()
                || self.focus_card.get());
        let generation = self.bump();
        if want == self.shown.get() {
            return;
        }
        let delay = if want {
            self.open_delay_ms.get()
        } else {
            self.close_delay_ms.get()
        };
        let me = self.clone();
        after_ms_detached(delay.min(i32::MAX as u32) as i32, move || {
            // `disabled` turning on bumps the generation; the re-check
            // covers a `Dynamic` source whose signal has not yet flushed.
            if me.alive.get() && me.generation.get() == generation && !(want && me.is_disabled()) {
                me.set_shown(want, false);
            }
        });
    }

    /// Long-press: open now, with the click-away catcher.
    fn open_by_touch(&self) {
        if self.is_disabled() {
            return;
        }
        self.bump();
        self.set_shown(true, true);
    }

    /// Escape / a tap outside: close now. Focus that is still on the
    /// trigger stops asking for the card — otherwise the next pointer
    /// leave would re-decide "focused, so open" and undo the Escape. It
    /// asks again on the next focus (`on_focus` reports transitions, so
    /// that is a fresh `true` after the trigger loses focus).
    fn close_now(&self) {
        self.bump();
        self.focus_trigger.set(false);
        self.set_shown(false, false);
    }

    fn set_shown(&self, shown: bool, by_touch: bool) {
        self.shown.set(shown);
        if !shown {
            // The card unmounts without reporting a pointer leave or a
            // focus loss.
            self.over_card.set(false);
            self.focus_card.set(false);
        }
        self.touch_open.set(by_touch);
        self.open.set(shown);
    }
}

/// The card surface: a column `view` holding the author's content, with
/// the tooltip role and the card test id. Returned as a builder: the caller
/// chains the card's `.on_hover` / `.on_focus` onto it.
fn card_surface(
    content: &HoverCardContent,
    styles: &HoverCardSlotStyles,
    card_test_id: Option<&'static str>,
) -> runtime_core::GlueView {
    let mut body: Vec<Element> = Vec::new();
    ChildList::append_to((content.0)(), &mut body);
    let surface = runtime_core::view(body)
        .with_style(HoverCardStyles::card().overridden(styles.card.as_ref()))
        .a11y_role(Role::Tooltip);
    with_test_id!(surface, card_test_id)
}

/// The click-away catcher, mounted only while the card was opened by a
/// long-press (see the module docs for why a hover-opened card has none).
fn touch_catcher(touch_open: Signal<bool>, dismiss: Rc<dyn Fn()>) -> Element {
    ui! {
        if touch_open.get() {
            { dismiss_catcher(Some(dismiss.clone())) }
        }
    }
}

/// Renders the trigger wrapped in a hover / long-press anchor and, while
/// open, a card anchored to it. See the module docs.
#[component(children)]
pub fn HoverCard(props: HoverCardProps) -> Element {
    let anchor_ref: Ref<ViewHandle> = Ref::new();
    let intent = HoverIntent::new(props.open_delay_ms, props.close_delay_ms, props.disabled);
    intent.bind_disabled();
    let (open, touch_open) = (intent.open, intent.touch_open);

    let lp_handler = {
        let intent = intent.clone();
        long_press(LongPressRecognizer::default(), move || {
            intent.open_by_touch()
        })
    };

    let mut kids: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut kids);
    }
    // Built with the builder chain, not `ui!`: the wrapper needs `.bind`
    // and the handler setters, which the macro has no attribute for.
    let anchor = {
        let intent = intent.clone();
        let focus_intent = intent.clone();
        let wrapper = runtime_core::view(kids)
            .bind(anchor_ref)
            .on_hover(move |entering| intent.hover_trigger(entering))
            // Focus-within: the wrapper is not a tab stop; it hears the
            // focusable trigger it wraps.
            .on_focus(move |focused| focus_intent.focus_trigger(focused))
            // Returns the recognizer's response so the press still reaches
            // the trigger's own handler.
            .on_touch(move |ev| lp_handler(ev));
        with_test_id!(wrapper, props.test_id).into_element()
    };

    let content = props.content;
    let styles = props.styles;
    let card_test_id = props.card_test_id;
    let (side, align, offset) = (props.side, props.align, props.offset);
    let on_card_hover = {
        let intent = intent.clone();
        move |entering: bool| intent.hover_card(entering)
    };
    let on_card_focus = {
        let intent = intent.clone();
        move |focused: bool| intent.focus_card(focused)
    };
    let dismiss: Rc<dyn Fn()> = {
        let intent = intent.clone();
        Rc::new(move || intent.close_now())
    };

    // The card, present only while `open`; the placement props are read on
    // each open. The catcher renders BEFORE the anchored card so the card
    // paints above it and a tap on the card is not "outside".
    let card = ui! {
        if open.get() {
            { touch_catcher(touch_open, dismiss.clone()) }
            {
                let dismiss = dismiss.clone();
                runtime_core::anchored_overlay(
                    AnchorTarget::from(anchor_ref),
                    vec![card_surface(&content, &styles, card_test_id)
                        .on_hover(on_card_hover.clone())
                        .on_focus(on_card_focus.clone())
                        .into_element()],
                )
                .side(side.get())
                .align(align.get())
                .offset(offset.get())
                .backdrop(BackdropMode::None)
                .trap_focus(false)
                .on_dismiss(move || dismiss())
            }
        }
    };

    runtime_core::fragment(vec![anchor, card])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{classify, P};
    use idea_theme::testing::{commit, with_test_world};

    /// want_def82398: a card open over a row that starts dragging kept
    /// showing the row at its old place — its `open` state was private, so
    /// nothing outside could close or suppress it. A live `disabled` closes
    /// an open card, and while it is on no path (long-press, keyboard focus,
    /// hover) opens one.
    #[test]
    fn regression_disabled_closes_an_open_hover_card_and_keeps_it_closed() {
        with_test_world(|| {
            let dragging = signal(false);
            let intent = HoverIntent::new(Reactive::Static(0), Reactive::Static(0), dragging.into());
            intent.bind_disabled();

            intent.open_by_touch();
            commit();
            assert!(intent.open.get(), "enabled: a long-press opens the card");

            dragging.set(true);
            commit();
            assert!(!intent.open.get(), "turning `disabled` on closes the open card");

            intent.open_by_touch();
            intent.focus_trigger(true);
            intent.hover_trigger(true);
            commit();
            assert!(!intent.open.get(), "while disabled nothing reopens it");
            assert!(!intent.shown.get(), "and no open is pending");

            dragging.set(false);
            commit();
            assert!(!intent.open.get(), "re-enabling opens nothing by itself");
            intent.open_by_touch();
            commit();
            assert!(intent.open.get(), "the next long-press opens it again");
        });
    }

    /// A static `disabled = true` card never opens, and `false` is the
    /// default.
    #[test]
    fn a_statically_disabled_hover_card_never_opens() {
        with_test_world(|| {
            assert!(!HoverCardProps::default().disabled.get_untracked(), "enabled by default");
            let intent = HoverIntent::new(Reactive::Static(0), Reactive::Static(0), Reactive::Static(true));
            intent.bind_disabled();
            intent.open_by_touch();
            intent.focus_trigger(true);
            commit();
            assert!(!intent.open.get());
        });
    }

    /// The trigger wrapper carries the hover handler (pointer open/close),
    /// the focus handler (keyboard open/close) and the touch handler
    /// (long-press), and the card is a reactive
    /// sibling spliced in by a `fragment`, so the wrapper adds no layout box
    /// beyond the trigger's own.
    #[test]
    fn hover_card_wraps_trigger_with_hover_touch_and_reactive_card() {
        with_test_world(|| {
            let el = HoverCard(HoverCardProps {
                content: HoverCardContent::from(|| runtime_core::text("body").into_element()),
                children: vec![runtime_core::text("trigger").into_element()],
                ..Default::default()
            });
            let mut kids = match classify(el) {
                P::Fragment { children } => children,
                _ => panic!("HoverCard must build a Fragment [anchor, card]"),
            };
            assert_eq!(kids.len(), 2, "fragment = anchor view + reactive card");
            match classify(kids.remove(0)) {
                P::View {
                    on_hover,
                    on_touch,
                    on_focus,
                    ..
                } => {
                    assert!(
                        on_hover,
                        "the anchor must carry on_hover (pointer open/close)"
                    );
                    assert!(on_focus, "the anchor must carry on_focus (keyboard open/close)");
                    assert!(on_touch, "the anchor must carry on_touch (long-press)");
                }
                _ => panic!("the first fragment child must be the anchor View"),
            }
            assert!(
                matches!(classify(kids.remove(0)), P::Other(_)),
                "the second child must be the reactive card (gated on `open`)",
            );
        });
    }
}
