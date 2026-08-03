//! `StackHeader` — a themed top bar for a stack navigator's screens (web/desktop).
//!
//! Reads the active screen's [`StackHeaderState`] (title + leading/trailing
//! [`HeaderButton`] slots + hidden flag) — the same per-screen slot data the
//! NATIVE bar renders on mobile — and draws a themed header from it. It
//! **self-suppresses** when `state.native` is true (a native bar is already
//! rendering it) or `state.hidden` is set, so an author places it
//! unconditionally: native bar on iOS/Android, drawn header on web/desktop,
//! nothing doubled.
//!
//! ```ignore
//! // in a stack navigator's `.layout(|nav| …)`:
//! StackHeader(
//!     state = rx!(stack_navigator::header_state(&nav.screen_chrome)),
//!     show_back = nav.can_go_back,
//!     on_back = Some(nav.pop.clone()),
//! )
//! ```

use idea_ui::{Surface, SurfaceColor};
use runtime_core::primitives::navigator::{HeaderButton, StackHeaderState};
use runtime_core::{
    component, dynamic, fragment, pressable, stylesheet, text, ui, Element, IdealystSchema,
    Length, Reactive, StyleApplication,
};
use std::rc::Rc;

// `stylesheet!` declarations, NOT `with_computed` layers over an empty
// base: a computed layer can never premint, so a StackHeader in a
// `--premint-only` app would panic. And the bar is rebuilt per NAVIGATION
// (inside `dynamic`), which is exactly the open-on-demand shape the
// premint dump's crawl never reaches — link-time registration is the only
// construction-independent option (see the Modal sheets for the pattern).
stylesheet! {
    StackHeaderRowSheet<()> {
        base(_t) {
            flex_direction: runtime_core::FlexDirection::Row,
            align_items: runtime_core::AlignItems::Center,
            padding_left: Length::Px(12.0),
            padding_right: Length::Px(12.0),
            padding_top: Length::Px(10.0),
            padding_bottom: Length::Px(10.0),
            column_gap: Length::Px(8.0),
        }
    }
}

stylesheet! {
    StackHeaderSlotSheet<()> {
        base(_t) {
            padding_left: Length::Px(4.0),
            padding_right: Length::Px(4.0),
        }
    }
}

/// Props for [`StackHeader`].
#[runtime_core::props]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct StackHeaderProps {
    /// The active screen's header slots. Feed
    /// `rx!(stack_navigator::header_state(&nav.screen_chrome))`. `None` ⇒
    /// nothing renders; `state.native` / `state.hidden` also suppress it.
    #[schema(constraint = "reactive: Option<StackHeaderState>")]
    pub state: Reactive<Option<StackHeaderState>>,
    /// Whether the back affordance shows (a stack wires `nav.can_go_back`).
    pub show_back: bool,
    /// Back handler (a stack wires `nav.pop`).
    pub on_back: Option<Rc<dyn Fn()>>,
}

impl Default for StackHeaderProps {
    fn default() -> Self {
        Self {
            state: Reactive::Static(None),
            on_back: None,
            show_back: Reactive::Static(false),
        }
    }
}

/// Renders a themed header from the active screen's [`StackHeaderState`], or
/// nothing when there's no state / it's `native` / it's `hidden`.
#[component]
pub fn StackHeader(props: StackHeaderProps) -> Element {
    let state = props.state;
    let show_back = props.show_back;
    let on_back = props.on_back;

    // `dynamic` rebuilds the whole bar whenever the read state (or show_back)
    // changes — i.e. on every navigation — so title, slots, and the back arrow
    // are all fresh for the current screen.
    dynamic(move || {
        let st = state.get();
        let back = show_back.get();
        match st {
            // Native bar owns the header, or it's hidden, or no screen yet.
            None => fragment(Vec::new()),
            Some(s) if s.native || s.hidden => fragment(Vec::new()),
            Some(s) => build_bar(&s, back, on_back.clone()),
        }
    })
}

/// A single header-slot button: its label (or icon name as a fallback) + tap.
fn slot_button(btn: &HeaderButton) -> Element {
    let text_content = btn
        .label
        .clone()
        .or_else(|| btn.icon.clone())
        .unwrap_or_default();
    let on_press = btn.on_press.clone();
    let press = move || {
        if let Some(cb) = &on_press {
            cb();
        }
    };
    pressable(vec![text(text_content).into()], press)
        .with_style(|| slot_style())
        .into()
}

fn build_bar(s: &StackHeaderState, show_back: bool, on_back: Option<Rc<dyn Fn()>>) -> Element {
    let mut children: Vec<Element> = Vec::with_capacity(4);

    // Back affordance.
    if show_back {
        if let Some(cb) = on_back {
            let press = move || (cb)();
            children.push(
                pressable(vec![text("‹").into()], press)
                    .with_style(|| slot_style())
                    .into(),
            );
        }
    }
    // Leading slot.
    if let Some(left) = &s.left {
        children.push(slot_button(left));
    }
    // Title.
    children.push(text(s.title.clone()).into());
    // Trailing slot.
    if let Some(right) = &s.right {
        children.push(slot_button(right));
    }

    let row: Element = ui! {
        view(style = row_style) {
            children
        }
    };
    ui! {
        Surface(background = SurfaceColor::Surface, grow = 1.0) {
            { row }
        }
    }
}

fn row_style() -> StyleApplication {
    StyleApplication::new(StackHeaderRowSheet::sheet())
}

fn slot_style() -> StyleApplication {
    StyleApplication::new(StackHeaderSlotSheet::sheet())
}

#[cfg(test)]
mod tests {
    use super::*;
    use idea_theme::testing::with_test_world;

    // Both header styles must PREMINT. The old spelling was `with_computed`
    // layers over an empty base — a premint disqualifier, so a StackHeader
    // in a `--premint-only` app panicked. Worse, the bar is rebuilt per
    // navigation inside `dynamic`, so even a construction-registered sheet
    // could first construct after the premint dump's crawl; `stylesheet!`
    // registers at LINK time, construction-independent. Fails against the
    // computed spelling (`preminted_class_list()` is `None` for a
    // computed-carrying application).
    #[test]
    fn regression_header_styles_premint() {
        with_test_world(|| {
            assert!(
                row_style().preminted_class_list().is_some(),
                "header row style must premint"
            );
            assert!(
                slot_style().preminted_class_list().is_some(),
                "header slot style must premint"
            );
        });
    }
}
