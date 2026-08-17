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
//!   `pointerenter`/`pointerleave`, macOS `NSTrackingArea`). Enter shows
//!   the bubble, leave hides it.
//! - **Touch (iOS / Android)** — there is no hover with a finger, so a
//!   `long_press` recognizer shows the bubble; it auto-dismisses after
//!   [`TooltipProps::dismiss_ms`] (the recognizer reports the press start,
//!   not the release, so a timed dismissal is the touch idiom).
//!
//! The bubble itself is a styled `view` box holding the label text, rendered
//! through the framework's `anchored_overlay` and anchored to the wrapper —
//! non-interactive, no backdrop, no focus trap. The box (not the text node)
//! carries the background, border, padding and the max-width clamp, so a long
//! hint wraps *inside* one rounded surface instead of painting a ragged
//! background strip per line. For clickable content reach for `Popover`.

use std::rc::Rc;

use runtime_core::primitives::overlay::BackdropMode;
use runtime_core::primitives::portal::{AnchorTarget, ElementAlign, ElementSide};
use runtime_core::{
    after_ms_detached, component, long_press, signal, when, ChildList, Element, IdealystSchema,
    IntoElement, LongPressRecognizer, Position, Reactive, Ref, StyleRules, StyleSheet, ViewHandle,
};

use crate::stylesheets::{TooltipBubble, TooltipBubbleText};

/// Default time (ms) a touch-triggered (long-press) tooltip stays up
/// before auto-dismissing. Hover tooltips ignore this — they hide on
/// pointer-leave.
pub const TOOLTIP_DISMISS_MS: u32 = 1800;

// Reactive-by-default: `#[props]` wraps the scalar data props (`side`/`align`
// enums, `offset`, `dismiss_ms`) → `Reactive<…>`. `text` is already `Reactive`
// (routes to the `text()` sink, untouched); `children` is a `Vec<Element>`
// (auto-skipped).
#[runtime_core::props]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct TooltipProps {
    /// Bubble text. `Reactive<String>` — static or live.
    #[schema(constraint = "reactive: static String or Signal/rx!")]
    pub text: Reactive<String>,
    /// Which side of the trigger the bubble sits on. Default `Above`.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub side: ElementSide,
    /// Alignment along the anchor edge. Default `Center`.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub align: ElementAlign,
    /// Gap in px between the trigger and the bubble. Default 6.
    pub offset: f32,
    /// How long a touch (long-press) tooltip stays up before
    /// auto-dismissing, in ms. Ignored for hover. Default
    /// [`TOOLTIP_DISMISS_MS`].
    pub dismiss_ms: u32,
    /// The trigger the tooltip wraps and anchors to.
    #[cfg_attr(feature = "docs", doc_control(skip))]
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
            children: Vec::new(),
        }
    }
}

/// Hug sheet for the wrapper so it sizes to the trigger instead of
/// stretching across a flex parent's cross axis (see
/// [`crate::components::hug_self`]).
fn hug_sheet() -> Rc<StyleSheet> {
    // `r#static` auto-premints by content, and the wrapper builds with the
    // TRIGGER (crawl-visible), not with the lazily-opened bubble — so the
    // dump does see this construction and its CSS ships.
    Rc::new(StyleSheet::r#static(crate::components::hug_self()))
}

/// Layout-neutral (out-of-flow) wrapper for the `when` bubble's *closed*
/// branch, so a hidden tooltip never adds a flex slot that would shift the
/// trigger's siblings as it mounts/unmounts. Mirrors the `if`-without-else
/// macro lowering and `Popover`'s wrapper.
fn hidden_sheet() -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(StyleRules {
        position: Some(Position::Absolute),
        ..Default::default()
    }))
}

/// The bubble surface: a `view` BOX carrying the background, border, padding,
/// radius and the max-width clamp, with the label `text` inside it.
///
/// The box has to be a real container. Painting the bubble on the text node
/// itself (what this used to do) yields one background rect per *line* on
/// backends that lay text out inline — a wrapped hint rendered as ragged bars
/// behind each line rather than a single rounded bubble — and leaves the
/// padding hanging off the text box. Splitting box from label makes every
/// backend draw the same surface, and puts the width clamp on the thing that
/// actually wraps its content.
fn bubble_box(text: Reactive<String>) -> Element {
    let label = runtime_core::text(text).with_style(TooltipBubbleText()).into_element();
    runtime_core::view(vec![label]).with_style(TooltipBubble()).into_element()
}

/// Renders the trigger wrapped in a hover/long-press anchor; shows a hint
/// bubble (anchored to the trigger) while hovered (desktop) or briefly on
/// long-press (touch). See the module docs.
#[component(children)]
pub fn Tooltip(props: TooltipProps) -> Element {
    let open = signal(false);
    let anchor_ref: Ref<ViewHandle> = Ref::new();
    let text = props.text;
    // TODO(reactive-sweep): route `side`/`align`/`offset`/`dismiss_ms`
    // reactively into the bubble's `anchored_overlay` placement + the
    // long-press timer. They're consumed by value as builder args (inside the
    // `when` bubble closure) and the touch-dismiss delay — STRUCTURE, not a
    // style closure — so a live signal would need the bubble rebuilt on change.
    // The `when` already rebuilds the bubble on each open, so a value change
    // between shows is picked up. `text` stays reactive (routes to `text()`).
    let side = props.side.get();
    let align = props.align.get();
    let offset = props.offset.get();
    let dismiss_ms = props.dismiss_ms.get() as i32;

    // Touch path: long-press shows the bubble, then auto-dismisses. The
    // `long_press` recognizer reports recognition (press start) only — no
    // release — so a timed hide is the right touch idiom. No-op on desktop
    // (a mouse rarely long-presses; hover drives it there).
    let lp_handler = long_press(LongPressRecognizer::default(), move || {
        open.set(true);
        after_ms_detached(dismiss_ms, move || open.set(false));
    });

    // The trigger, wrapped so we can attach the anchor ref + hover/touch.
    let mut kids: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut kids);
    }
    let anchor = runtime_core::view(kids)
        .bind(anchor_ref)
        // Desktop/web: pointer enter → show, leave → hide. No-op on touch.
        .on_hover(move |entering| open.set(entering))
        // Touch: long-press → show (auto-dismiss). Returns the recognizer's
        // response so the press still bubbles for the trigger's own handler.
        .on_touch(move |ev| lp_handler(ev))
        // Hug the trigger so the wrapper doesn't stretch in a flex parent.
        .with_style(hug_sheet())
        .into_element();

    // The bubble — anchored to the wrapper, gated on `open`. Closed branch is
    // out-of-flow so toggling visibility never shifts layout.
    let bubble = when(
        move || open.get(),
        move || {
            runtime_core::anchored_overlay(
                AnchorTarget::from(anchor_ref),
                vec![bubble_box(text.clone())],
            )
                .side(side)
                .align(align)
                .offset(offset)
                .backdrop(BackdropMode::None)
                .trap_focus(false)
                .into_element()
        },
        || runtime_core::view(Vec::new()).with_style(hidden_sheet()).into_element(),
    );

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

    /// Regression: the bubble used to be a lone `text` node carrying the
    /// background/padding/max-width, which paints one ragged rect per line
    /// once the hint wraps (inline text layout) instead of a single rounded
    /// box. The surface must be a `view` BOX holding the label, with the box
    /// owning background + padding + the width clamp, and the label owning
    /// only ink (color/size) — no background of its own.
    #[test]
    fn regression_tooltip_bubble_is_a_box_not_a_styled_text_node() {
        with_test_world(|| {
            let (box_style, label) = match classify(bubble_box(Reactive::Static("hi".into()))) {
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
