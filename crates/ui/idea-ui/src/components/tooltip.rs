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
//! The bubble itself is a single styled text node rendered through the
//! framework's `anchored_overlay`, anchored to the wrapper — non-interactive,
//! no backdrop, no focus trap. For clickable content reach for `Popover`.

use std::rc::Rc;

use runtime_core::primitives::overlay::BackdropMode;
use runtime_core::primitives::portal::{AnchorTarget, ElementAlign, ElementSide};
use runtime_core::{
    after_ms_detached, component, long_press, signal, when, ChildList, Element, IdealystSchema,
    IntoElement, LongPressRecognizer, Position, Reactive, Ref, StyleRules, StyleSheet, ViewHandle,
};

use crate::stylesheets::TooltipBubble;

/// Default time (ms) a touch-triggered (long-press) tooltip stays up
/// before auto-dismissing. Hover tooltips ignore this — they hide on
/// pointer-leave.
pub const TOOLTIP_DISMISS_MS: u32 = 1800;

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
            side: ElementSide::Above,
            align: ElementAlign::Center,
            offset: 6.0,
            dismiss_ms: TOOLTIP_DISMISS_MS,
            children: Vec::new(),
        }
    }
}

/// Hug sheet for the wrapper so it sizes to the trigger instead of
/// stretching across a flex parent's cross axis (see
/// [`crate::components::hug_self`]).
fn hug_sheet() -> Rc<StyleSheet> {
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

/// Renders the trigger wrapped in a hover/long-press anchor; shows a hint
/// bubble (anchored to the trigger) while hovered (desktop) or briefly on
/// long-press (touch). See the module docs.
#[component(children)]
pub fn Tooltip(props: TooltipProps) -> Element {
    let open = signal!(false);
    let anchor_ref: Ref<ViewHandle> = Ref::new();
    let text = props.text;
    let side = props.side;
    let align = props.align;
    let offset = props.offset;
    let dismiss_ms = props.dismiss_ms as i32;

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
            let bubble_text =
                runtime_core::text(text.clone()).with_style(TooltipBubble()).into_element();
            runtime_core::anchored_overlay(AnchorTarget::from(anchor_ref), vec![bubble_text])
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

    /// The Tooltip wraps its trigger in an anchor view that carries BOTH the
    /// hover handler (desktop show/hide) and a touch handler (mobile
    /// long-press), and emits the bubble as a reactive `when` sibling — all
    /// spliced via a `fragment` so the wrapper adds no extra layout box around
    /// the trigger. Guards the whole wiring: if a refactor drops `on_hover`
    /// (no desktop hover) or the `when` bubble, this fails.
    #[test]
    fn tooltip_wraps_trigger_with_hover_touch_and_reactive_bubble() {
        let el = Tooltip(TooltipProps {
            text: Reactive::Static("hi".into()),
            children: vec![runtime_core::text("trigger").into_element()],
            ..Default::default()
        });
        let kids = match el {
            Element::Fragment { children } => children,
            _ => panic!("Tooltip must build a Fragment [anchor, bubble]"),
        };
        assert_eq!(kids.len(), 2, "fragment = anchor view + reactive bubble");
        match &kids[0] {
            Element::View { on_hover, on_touch, .. } => {
                assert!(on_hover.is_some(), "anchor must carry on_hover (desktop show/hide)");
                assert!(on_touch.is_some(), "anchor must carry on_touch (mobile long-press)");
            }
            _ => panic!("first fragment child must be the anchor View"),
        }
        assert!(
            matches!(kids[1], Element::When { .. }),
            "second child must be the reactive bubble (a `when` gated on hover/press)",
        );
    }
}
