//! [`ActionBar`] — a screen's closing actions (rule `action-bar`).

use idea_ui::{tone, typography_kind, variant, Button, Typography};
use runtime_core::{
    component, rx, stylesheet, ui, AlignItems, Element, FlexDirection, FlexWrap, IdealystSchema, JustifyContent,
    Length, PointerEvents, Position,
};

use crate::width::is_phone;
use crate::Action;

/// The most verbs an [`ActionBar`] holds beside its primary; one more
/// belongs in the header's ⋮.
pub const ACTION_BAR_MAX_SECONDARY: usize = 2;

/// The narrowest the note sits beside the verbs: about four words a
/// line. Below it the note wraps onto a line of its own above them.
pub const ACTION_BAR_NOTE_MIN: f32 = 240.0;

#[runtime_core::props]
#[derive(IdealystSchema, Default)]
pub struct ActionBarProps {
    /// The verb the screen is for. At most one.
    #[prop(static)]
    pub primary: Option<Action>,
    /// At most [`ACTION_BAR_MAX_SECONDARY`] others.
    #[prop(static)]
    pub secondary: Vec<Action>,
    /// Pinned to the foot of a [`Flow`](crate::Flow) or a decision
    /// screen: a rule above it, on the page's surface.
    #[prop(static)]
    pub pinned: bool,
    /// A sentence at the bar's start saying what the primary records. A
    /// phone has no room for it and draws the verbs alone.
    #[prop(static)]
    pub note: Option<String>,
}

/// A screen's closing actions: right-aligned with the primary last from
/// `Sm` up, after the `note` if there is one; full width and stacked on
/// a phone with the primary first, where the thumb starts.
///
/// ```ignore
/// ActionBar(
///     primary = Some(Action::new("Approve", approve).busy(busy)),
///     secondary = vec![Action::new("Send back", send_back)],
///     pinned = true,
/// )
/// ```
#[component]
pub fn ActionBar(props: &ActionBarProps) -> Element {
    debug_assert!(
        props.secondary.len() <= ACTION_BAR_MAX_SECONDARY,
        "rule `action-bar`: a fourth action goes in the header's menu",
    );
    let primary = props.primary.clone();
    let secondary = props.secondary.clone();
    let note = props.note.clone();
    let row = ui! {
        view(style = ActionBarRow()) {
            if let Some(a) = primary {
                Button(
                    label = a.label.clone(),
                    on_click = a.on_press.clone(),
                    tone = a.tone.clone().unwrap_or_else(|| tone::Primary.into()),
                    loading = a.busy_now(),
                    disabled = a.disabled_now(),
                    block = rx!(is_phone()),
                    leading_icon = a.icon,
                    test_id = a.test_id,
                )
            }
            for a in secondary.iter(), key = a.label.clone() {
                Button(
                    label = a.label.clone(),
                    on_click = a.on_press.clone(),
                    tone = a.tone.clone().unwrap_or_else(|| tone::Neutral.into()),
                    variant = variant::Outlined,
                    loading = a.busy_now(),
                    disabled = a.disabled_now(),
                    block = rx!(is_phone()),
                    leading_icon = a.icon,
                    test_id = a.test_id,
                )
            }
            // Last in the source, so the reversed row puts it first.
            if let Some(note) = note {
                view(style = ActionBarNote()) {
                    Typography(content = note, kind = typography_kind::BodySm, muted = true)
                }
            }
        }
    };
    if props.pinned {
        ui! { view(style = ActionBarPinned()) { row } }
    } else {
        row
    }
}

stylesheet! {
    // A stretched column on a phone, the primary first. From `sm` a row
    // laid out from the trailing edge, so the primary (first in the
    // source) sits rightmost and the others step left of it. `sm`
    // apart: two forms of one decision.
    ActionBarRow<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Stretch,
            gap: t.spacing.sm(),
        }
        // A bar too narrow for the note beside the verbs wraps it onto
        // a line of its own ABOVE them (`WrapReverse` stacks later lines
        // upward) rather than squeezing it into a sliver.
        breakpoint sm(_t) {
            flex_direction: FlexDirection::RowReverse,
            flex_wrap: FlexWrap::WrapReverse,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::FlexStart,
        }
    }
}

stylesheet! {
    // The page owns the safe areas (rule `safe-areas`), so the pinned
    // bar pads only itself.
    ActionBarPinned<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            flex_shrink: 0.0,
            padding_top: t.spacing.md(),
            padding_left: t.spacing.lg(),
            padding_right: t.spacing.lg(),
            padding_bottom: t.spacing.md(),
            border_top_width: 1.0,
            border_top_color: t.color.border(),
            background: t.color.surface(),
        }
        breakpoint sm(t) {
            padding_left: t.spacing.xl(),
            padding_right: t.spacing.xl(),
        }
        breakpoint lg(t) {
            padding_left: t.spacing.xxl(),
            padding_right: t.spacing.xxl(),
        }
    }
}

stylesheet! {
    // Gone on a phone: out of flow, invisible and untouchable, since the
    // style vocabulary has no `display: none`. From `sm` it takes what
    // the verbs leave, down to the note's minimum.
    ActionBarNote<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            position: Position::Absolute,
            opacity: 0.0,
            pointer_events: PointerEvents::None,
        }
        breakpoint sm(_t) {
            position: Position::Relative,
            opacity: 1.0,
            pointer_events: PointerEvents::Auto,
            flex_basis: Length::Px(ACTION_BAR_NOTE_MIN),
            flex_grow: 1.0,
            flex_shrink: 1.0,
            min_width: Length::Px(ACTION_BAR_NOTE_MIN),
        }
    }
}
