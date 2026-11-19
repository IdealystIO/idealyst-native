//! `Tooltip` — a compact, non-interactive hint bubble that appears while
//! the user **hovers** the wrapped trigger (desktop/web) or **long-presses**
//! it (touch). The Tooltip *wraps* its trigger and owns its own visibility —
//! no host open-state signal required:
//!
//! ```ignore
//! ui! {
//!     Tooltip(text = "Resets to defaults".into()) {
//!         IconButton(glyph = "?".into(), on_press = move || reset())
//!     }
//! }
//! ```
//!
//! ## How it shows
//!
//! - **Desktop / web** — the wrapper view carries an `on_hover` handler
//!   (the framework's pointer-enter/leave channel: web
//!   `pointerenter`/`pointerleave`, macOS `NSTrackingArea`). Resting on
//!   the trigger for [`TooltipProps::delay_ms`] shows the bubble; leaving
//!   hides it at once. A pass over the trigger shorter than the delay shows
//!   nothing.
//! - **Warm window** — once any tooltip has shown, the tooltips of the same
//!   world are *warm*: the next one shows the moment the pointer enters its
//!   trigger, with no delay, so moving along a toolbar reads each hint in
//!   turn. The window closes [`TooltipProps::skip_delay_ms`] after the
//!   pointer leaves a shown tooltip without entering another; the next
//!   tooltip after that waits its `delay_ms` again.
//! - **Touch (iOS / Android)** — there is no hover with a finger, so a
//!   `long_press` recognizer shows the bubble at once (the hold is the
//!   delay); it auto-dismisses after [`TooltipProps::dismiss_ms`] (the
//!   recognizer reports the press start, not the release, so a timed
//!   dismissal is the touch idiom). A long-press neither uses nor opens
//!   the warm window.
//!
//! The bubble itself is a styled `view` box holding the label text, rendered
//! through the framework's `anchored_overlay` and anchored to the wrapper —
//! non-interactive, no backdrop, no focus trap. The box (not the text node)
//! carries the background, border, padding and the max-width clamp, so a long
//! hint wraps *inside* one rounded surface instead of painting a ragged
//! background strip per line. For clickable content reach for `Popover`.

use std::cell::Cell;
use std::rc::Rc;

use runtime_core::primitives::overlay::BackdropMode;
use runtime_core::primitives::portal::{AnchorTarget, ElementAlign, ElementSide};
use runtime_core::{
    after_ms_detached, component, long_press, on_owned_drop, signal, ui, world_local, ChildList,
    Element, IdealystSchema, IntoElement, LongPressRecognizer, Reactive, Ref, ScopeAlive, Signal,
    ViewHandle, component_styles, Color, FlexDirection, TextAlign,
};

use crate::theme::IdeaThemeRef;
// Named by `#[prop(example = …)]` values, which compile only in docs builds.
#[allow(unused_imports)]
use crate::Button;

component_styles! {
    /// The styled sections of [`Tooltip`].
    ///
    /// The bubble's box and its label are separate slots, and the split is
    /// load-bearing: painting the box on the text node gives a per-LINE
    /// background on backends that lay text out inline (web: a wrapped
    /// `text` paints one ragged rect per line), so the box is a real
    /// container and every backend draws the same single rounded rect.
    pub Tooltip<IdeaThemeRef> {
        slot bubble {
            base(t) {
                background: t.color.surface(),
                border_width: 1.0,
                border_color: t.color.border(),
                padding_vertical: t.spacing.xs(),
                padding_horizontal: t.spacing.sm(),
                border_radius: t.radius.md(),
                flex_direction: FlexDirection::Column,
                // Clamps the bubble so a long hint wraps into a readable column
                // instead of running the width of the viewport.
                max_width: 260.0,
                shadow: runtime_core::Shadow {
                    x: 0.0, y: 4.0, blur: 12.0, color: Color("rgba(15, 17, 21, 0.22)".into()).into(),
                },
            }
            transitions {
                background: 250ms EaseInOut,
                border_color: 250ms EaseInOut,
            }
        }

        slot text {
            base(t) {
                color: t.color.text(),
                font_size: t.typography.body_sm_size(),
                line_height: 18.0,
                text_align: TextAlign::Left,
            }
        }
    }
}

/// Default time (ms) a touch-triggered (long-press) tooltip stays up
/// before auto-dismissing. Hover tooltips ignore this — they hide on
/// pointer-leave.
pub const TOOLTIP_DISMISS_MS: u32 = 1800;

/// Default [`TooltipProps::delay_ms`]: how long the pointer rests on a
/// trigger before the first tooltip shows. Long enough that sweeping the
/// pointer across a toolbar shows nothing.
pub const TOOLTIP_DELAY_MS: u32 = 500;

/// Default [`TooltipProps::skip_delay_ms`]: how long the warm window stays
/// open after the pointer leaves a shown tooltip. It has to cover crossing
/// the gap to a neighbouring trigger.
pub const TOOLTIP_SKIP_DELAY_MS: u32 = 300;

// Reactive-by-default: `#[props]` wraps the scalar data props (`side`/`align`
// enums, `offset`, the delays) → `Reactive<…>`. `text` is already `Reactive`
// (routes to the `text()` sink, untouched); `children` is a `Vec<Element>`
// (auto-skipped).
#[runtime_core::props(styles = TooltipStyles)]
#[derive(IdealystSchema)]
pub struct TooltipProps {
    /// Bubble text. `Reactive<String>` — static or live.
    #[schema(constraint = "reactive: static String or Signal/rx!")]
    #[prop(example = "Saves your changes")]
    pub text: Reactive<String>,
    /// Which side of the trigger the bubble sits on. Default `Above`.
    pub side: ElementSide,
    /// Alignment along the anchor edge. Default `Center`.
    pub align: ElementAlign,
    /// Gap in px between the trigger and the bubble. Default 6.
    pub offset: f32,
    /// How long a touch (long-press) tooltip stays up before
    /// auto-dismissing, in ms. Ignored for hover. Default
    /// [`TOOLTIP_DISMISS_MS`].
    pub dismiss_ms: u32,
    /// How long the pointer rests on the trigger before the bubble shows,
    /// in ms, when no tooltip is warm (see the module docs). Ignored for
    /// touch. Default [`TOOLTIP_DELAY_MS`].
    pub delay_ms: u32,
    /// How long after the pointer leaves this tooltip, shown, the next
    /// tooltip still shows with no delay, in ms. `0` makes every tooltip
    /// wait its `delay_ms`. Ignored for touch. Default
    /// [`TOOLTIP_SKIP_DELAY_MS`].
    pub skip_delay_ms: u32,
    /// No bubble while `true`: for a hint that only applies in one state
    /// of its trigger (a row whose label is hidden), without unmounting
    /// the trigger to take the tooltip off. Default `false`.
    pub disabled: bool,
    /// The trigger the tooltip wraps and anchors to.
    #[prop(example = ui! { Button(label = "Hover me") })]
    pub children: Vec<Element>,
}

impl Default for TooltipProps {
    fn default() -> Self {
        Self {
            text: Reactive::Static(String::new()),
            side: Reactive::Static(ElementSide::Above),
            align: Reactive::Static(ElementAlign::Center),
            offset: Reactive::Static(6.0),
            dismiss_ms: Reactive::Static(TOOLTIP_DISMISS_MS),
            delay_ms: Reactive::Static(TOOLTIP_DELAY_MS),
            skip_delay_ms: Reactive::Static(TOOLTIP_SKIP_DELAY_MS),
            disabled: Reactive::Static(false),
            children: Vec::new(),
            styles: Default::default(),
        }
    }
}

/// The warm window every Tooltip of one world shares (see the module docs).
///
/// Per world, not per thread: a server render runs one world per request on
/// a shared thread, and one session's hover must not warm another's
/// tooltips. Reached through [`world_local`] at build time and held by each
/// tooltip, because hover handlers can run outside the world's ambient
/// scope.
#[derive(Clone)]
struct WarmWindow(Rc<WarmState>);

struct WarmState {
    warm: Cell<bool>,
    /// Stamps the pending cool-down; a cool-down whose stamp is no longer
    /// current does nothing, so a tooltip showing inside the window keeps
    /// it open.
    generation: Cell<u64>,
}

impl WarmWindow {
    /// The current world's window, created cold. Outside every world (a
    /// backend-less unit test) a private window, which behaves the same for
    /// the one tooltip that holds it.
    fn current() -> WarmWindow {
        let fresh = || WarmWindow(Rc::new(WarmState { warm: Cell::new(false), generation: Cell::new(0) }));
        world_local(fresh).unwrap_or_else(fresh)
    }

    fn is_warm(&self) -> bool {
        self.0.warm.get()
    }

    fn bump(&self) -> u64 {
        let g = self.0.generation.get().wrapping_add(1);
        self.0.generation.set(g);
        g
    }

    /// A tooltip showed: warm, and any pending cool-down goes stale.
    fn warm_up(&self) {
        self.bump();
        self.0.warm.set(true);
    }

    /// A shown tooltip went away: close the window after `skip_ms`, unless
    /// another tooltip shows first. The timer touches no signal, so it is
    /// safe to outlive the tooltip that armed it.
    fn cool_after(&self, skip_ms: u32) {
        let generation = self.bump();
        if skip_ms == 0 {
            self.0.warm.set(false);
            return;
        }
        let state = self.0.clone();
        after_ms_detached(skip_ms.min(i32::MAX as u32) as i32, move || {
            if state.generation.get() == generation {
                state.warm.set(false);
            }
        });
    }
}

/// One Tooltip's show/hide state machine; the only writer of `open`.
///
/// Every hover change or long-press stamps a fresh `generation`, and a timer
/// whose stamp is no longer current does nothing: that is how a leave
/// cancels a pending show, and a second long-press restarts the dismiss.
/// `after_ms_detached` rather than `after_ms_scoped` because the timers are
/// armed from event handlers, which run outside any component scope;
/// `alive` stops a timer that outlives the component from writing `open`.
///
/// The delays are read when they are used, so a live `delay_ms` /
/// `skip_delay_ms` / `dismiss_ms` takes effect on the next hover or press.
struct TooltipIntent {
    open: Signal<bool>,
    /// Mirrors `open` without a tracked read.
    shown: Cell<bool>,
    /// The bubble was shown by hover, so hiding it starts the warm
    /// window's cool-down. A long-press shows without warming.
    by_hover: Cell<bool>,
    generation: Cell<u64>,
    /// The `skip_delay_ms` read when the bubble last showed by hover: the
    /// cool-down an unmount starts can't read the prop, whose signal may
    /// already be freed by then.
    skip_ms: Cell<u32>,
    window: WarmWindow,
    delay_ms: Reactive<u32>,
    skip_delay_ms: Reactive<u32>,
    dismiss_ms: Reactive<u32>,
    disabled: Reactive<bool>,
    alive: ScopeAlive,
}

impl TooltipIntent {
    /// A hidden tooltip's state machine. Must run in the component body:
    /// `open` and the `alive` token belong to its scope, and so does the
    /// unmount hook that closes the warm window behind a bubble removed
    /// while it was showing (no pointer leave reports that).
    fn new(
        delay_ms: Reactive<u32>,
        skip_delay_ms: Reactive<u32>,
        dismiss_ms: Reactive<u32>,
        disabled: Reactive<bool>,
    ) -> Rc<Self> {
        let me = Rc::new(TooltipIntent {
            open: signal(false),
            shown: Cell::new(false),
            by_hover: Cell::new(false),
            generation: Cell::new(0),
            skip_ms: Cell::new(0),
            window: WarmWindow::current(),
            delay_ms,
            skip_delay_ms,
            dismiss_ms,
            disabled,
            alive: ScopeAlive::current(),
        });
        let on_drop = me.clone();
        on_owned_drop(move || {
            if on_drop.shown.get() && on_drop.by_hover.get() {
                on_drop.window.cool_after(on_drop.skip_ms.get());
            }
        });
        me
    }

    fn bump(&self) -> u64 {
        let g = self.generation.get().wrapping_add(1);
        self.generation.set(g);
        g
    }

    fn is_disabled(&self) -> bool {
        self.disabled.get_untracked()
    }

    /// Pointer enter / leave on the trigger.
    fn hover(self: &Rc<Self>, entering: bool) {
        let generation = self.bump();
        if !entering {
            if self.shown.get() {
                self.hide();
            }
            return;
        }
        if self.is_disabled() {
            return;
        }
        let delay = self.delay_ms.get_untracked();
        if self.window.is_warm() || delay == 0 {
            self.show_by_hover();
            return;
        }
        let me = self.clone();
        after_ms_detached(delay.min(i32::MAX as u32) as i32, move || {
            // A leave bumps the generation; the `disabled` re-check covers
            // a source that turned on while the timer was pending.
            if me.alive.get() && me.generation.get() == generation && !me.is_disabled() {
                me.show_by_hover();
            }
        });
    }

    /// Long-press: show now, then hide after `dismiss_ms`.
    fn long_press(self: &Rc<Self>) {
        if self.is_disabled() {
            return;
        }
        let generation = self.bump();
        if !self.shown.get() {
            self.by_hover.set(false);
            self.set_shown(true);
        }
        let me = self.clone();
        let dismiss = self.dismiss_ms.get_untracked().min(i32::MAX as u32) as i32;
        after_ms_detached(dismiss, move || {
            if me.alive.get() && me.generation.get() == generation {
                me.hide();
            }
        });
    }

    fn show_by_hover(&self) {
        if self.shown.get() {
            return;
        }
        self.skip_ms.set(self.skip_delay_ms.get_untracked());
        self.by_hover.set(true);
        self.window.warm_up();
        self.set_shown(true);
    }

    fn hide(&self) {
        if self.by_hover.get() {
            self.window.cool_after(self.skip_ms.get());
        }
        self.set_shown(false);
    }

    fn set_shown(&self, shown: bool) {
        self.shown.set(shown);
        self.open.set(shown);
    }
}

/// The bubble surface: a `view` BOX carrying the background, border, padding,
/// radius and the max-width clamp, with the label `text` inside it.
///
/// The box has to be a real container. Painting the bubble on the text node
/// itself yields one background rect per *line* on
/// backends that lay text out inline — a wrapped hint rendered as ragged bars
/// behind each line rather than a single rounded bubble — and leaves the
/// padding hanging off the text box. Splitting box from label makes every
/// backend draw the same surface, and puts the width clamp on the thing that
/// actually wraps its content.
fn bubble_box(text: Reactive<String>, styles: &TooltipSlotStyles) -> Element {
    let label = runtime_core::text(text)
        .with_style(TooltipStyles::text().overridden(styles.text.as_ref()))
        .into_element();
    runtime_core::view(vec![label])
        .with_style(TooltipStyles::bubble().overridden(styles.bubble.as_ref()))
        .into_element()
}

/// Renders the trigger wrapped in a hover/long-press anchor; shows a hint
/// bubble (anchored to the trigger) while hovered (desktop) or briefly on
/// long-press (touch). See the module docs.
#[component(children)]
pub fn Tooltip(props: TooltipProps) -> Element {
    let anchor_ref: Ref<ViewHandle> = Ref::new();
    let text = props.text;
    let styles = props.styles.clone();
    // `side`/`align`/`offset` are consumed by value as builder args of the
    // bubble's `anchored_overlay` — structure, not a style closure — so they
    // are snapshotted here at build. `text` stays reactive (it routes to
    // `text()`); the delays are read live by the intent.
    let side = props.side.get();
    let align = props.align.get();
    let offset = props.offset.get();
    let disabled = props.disabled.clone();
    let intent = TooltipIntent::new(props.delay_ms, props.skip_delay_ms, props.dismiss_ms, props.disabled);
    let open = intent.open;

    // Touch path: long-press shows the bubble, then auto-dismisses. The
    // `long_press` recognizer reports recognition (press start) only — no
    // release — so a timed hide is the right touch idiom. No-op on desktop
    // (a mouse rarely long-presses; hover drives it there).
    let lp_handler = {
        let intent = intent.clone();
        long_press(LongPressRecognizer::default(), move || intent.long_press())
    };

    // The trigger, wrapped so we can attach the anchor ref + hover/touch. Unstyled,
    // so layout-transparent: the wrapper follows the parent's `align_items` as the
    // bare trigger would, and a hugging trigger keeps to its own size inside it.
    // Any `align_self` or `justify_content` here would move the trigger.
    //
    // In a stretching parent the wrapper is the line's width, and the bubble
    // anchors to it: a hugging trigger at the start of a default column has its
    // bubble centred on the line. `align = ElementAlign::Start` on the Tooltip, or
    // a parent with `Stack(align = Start)`, puts the bubble over the trigger.
    // Not `width: FitContent` here: it collapses a trigger that fills, a `Field`
    // or a block Button, to its content.
    let mut kids: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut kids);
    }
    let anchor = runtime_core::view(kids)
        .bind(anchor_ref)
        // Desktop/web: pointer enter → show (after the delay unless warm),
        // leave → hide. No-op on touch.
        .on_hover(move |entering| intent.hover(entering))
        // Touch: long-press → show (auto-dismiss). Returns the recognizer's
        // response so the press still bubbles for the trigger's own handler.
        .on_touch(move |ev| lp_handler(ev))
        .into_element();

    // The bubble — anchored to the wrapper, present only while `open`. The
    // macro's closed-branch placeholder is out of flow, so toggling
    // visibility never shifts layout.
    let bubble = ui! {
        if open.get() && !disabled.get() {
            {
                runtime_core::anchored_overlay(
                    AnchorTarget::from(anchor_ref),
                    vec![bubble_box(text.clone(), &styles)],
                )
                .side(side)
                .align(align)
                .offset(offset)
                .backdrop(BackdropMode::None)
                .trap_focus(false)
            }
        }
    };

    runtime_core::fragment(vec![anchor, bubble])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{classify, P};
    use idea_theme::testing::with_test_world;

    /// The Tooltip wraps its trigger in an anchor view that carries BOTH the
    /// hover handler (desktop show/hide) and a touch handler (mobile
    /// long-press), and emits the bubble as a reactive `when` sibling — all
    /// spliced via a `fragment` so the wrapper adds no extra layout box around
    /// the trigger. Guards the whole wiring: if a refactor drops `on_hover`
    /// (no desktop hover) or the `when` bubble, this fails.
    #[test]
    fn tooltip_wraps_trigger_with_hover_touch_and_reactive_bubble() {
        with_test_world(|| {
            let el = Tooltip(TooltipProps {
                text: Reactive::Static("hi".into()),
                children: vec![runtime_core::text("trigger").into_element()],
                ..Default::default()
            });
            let mut kids = match classify(el) {
                P::Fragment { children } => children,
                _ => panic!("Tooltip must build a Fragment [anchor, bubble]"),
            };
            assert_eq!(kids.len(), 2, "fragment = anchor view + reactive bubble");
            match classify(kids.remove(0)) {
                P::View { on_hover, on_touch, .. } => {
                    assert!(on_hover, "anchor must carry on_hover (desktop show/hide)");
                    assert!(on_touch, "anchor must carry on_touch (mobile long-press)");
                }
                _ => panic!("first fragment child must be the anchor View"),
            }
            // The bubble is a reactive hole (an opaque `Dyn`) — the mirror
            // reports it as `P::Other`.
            assert!(
                matches!(classify(kids.remove(0)), P::Other(_)),
                "second child must be the reactive bubble (a `when` gated on hover/press)",
            );
        });
    }

    /// Regression: a lone `text` node carrying the bubble's
    /// background/padding/max-width paints one ragged rect per line
    /// once the hint wraps (inline text layout) instead of a single rounded
    /// box. The surface must be a `view` BOX holding the label, with the box
    /// owning background + padding + the width clamp, and the label owning
    /// only ink (color/size) — no background of its own.
    #[test]
    fn regression_tooltip_bubble_is_a_box_not_a_styled_text_node() {
        with_test_world(|| {
            let (box_style, label) = match classify(bubble_box(Reactive::Static("hi".into()), &TooltipSlotStyles::default())) {
                P::View { mut children, style, .. } => {
                    assert_eq!(children.len(), 1, "bubble box holds exactly the label");
                    (style.expect("bubble box must be styled"), children.remove(0))
                }
                _ => panic!("bubble must be a View box, not a bare styled text node"),
            };

            let rules = box_style.resolve();
            assert!(rules.background.is_some(), "the BOX paints the bubble background");
            assert!(rules.max_width.is_some(), "the BOX clamps the bubble width");
            // `padding_vertical` / `padding_horizontal` are shorthands — they
            // resolve into the per-edge fields.
            assert!(rules.padding_top.is_some(), "the BOX owns the bubble padding");
            assert!(rules.padding_left.is_some(), "the BOX owns the bubble padding");

            match classify(label) {
                P::Text { style, .. } => {
                    let text_rules = style.expect("label must be styled").resolve();
                    assert!(text_rules.color.is_some(), "label carries the ink color");
                    assert!(
                        text_rules.background.is_none(),
                        "label must NOT paint a background — that's the per-line-bars bug",
                    );
                }
                _ => panic!("bubble box's only child must be the label Text"),
            }
        });
    }
}
