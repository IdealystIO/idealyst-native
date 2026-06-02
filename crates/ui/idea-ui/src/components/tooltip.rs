//! `Tooltip` — a compact, non-interactive bubble anchored to a trigger.
//!
//! Like [`Popover`](crate::components::popover::Popover), the host owns
//! visibility and gates mounting (the framework has no cross-backend
//! hover event — hover-to-show is a web-only affordance an app can add
//! on top, but tap/focus-driven display works everywhere):
//!
//! ```ignore
//! let trigger: Ref<PressableHandle> = Ref::new();
//! let open = signal!(false);
//! ui! {
//!     IconButton(glyph = "?".into(), on_press = move || open.set(!open.get()), bind_to = Some(trigger))
//!     if open.get() {
//!         Tooltip(target = AnchorTarget::from(trigger), text = "Resets to defaults".into())
//!     }
//! }
//! ```

use runtime_core::primitives::overlay::BackdropMode;
use runtime_core::primitives::portal::{AnchorTarget, ElementAlign, ElementSide};
use runtime_core::{component, IdealystSchema, IntoElement, Element, Reactive};

use crate::stylesheets::TooltipBubble;

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct TooltipProps {
    /// Element to anchor against — `AnchorTarget::from(some_ref)`.
    pub target: Option<AnchorTarget>,
    /// Bubble text. `Reactive<String>` — static or live.
    #[schema(constraint = "reactive: static String or Signal/rx!")]
    pub text: Reactive<String>,
    /// Which side of the target the bubble sits on. Default `Above`.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub side: ElementSide,
    /// Alignment along the anchor edge. Default `Center`.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub align: ElementAlign,
    /// Gap in px between the anchor and the bubble.
    pub offset: f32,
}

impl Default for TooltipProps {
    fn default() -> Self {
        Self {
            target: None,
            text: Reactive::Static(String::new()),
            side: ElementSide::Above,
            align: ElementAlign::Center,
            offset: 6.0,
        }
    }
}

/// Renders a compact, non-interactive bubble anchored to a trigger element
/// via the framework's `anchored_overlay`. The host owns visibility and
/// gates mounting; positioning follows `target`/`side`/`align`/`offset`.
#[component]
pub fn Tooltip(props: TooltipProps) -> Element {
    let target = props
        .target
        .expect("Tooltip: required `target` prop missing — set it to an AnchorTarget from a Ref");

    // The bubble is a single styled text node — styling the text
    // directly (not a wrapper view) keeps the foreground color on the
    // glyphs regardless of cross-view color inheritance.
    let bubble = runtime_core::text(props.text)
        .with_style(TooltipBubble())
        .into_element();

    runtime_core::anchored_overlay(target, vec![bubble])
        .side(props.side)
        .align(props.align)
        .offset(props.offset)
        .backdrop(BackdropMode::None)
        .trap_focus(false)
        .into_element()
}
