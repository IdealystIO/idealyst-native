//! [`ActionBar`] — a screen's closing actions (rule `action-bar`).

use idea_ui::{tone, typography_kind, variant, Button, Typography};
use runtime_core::Tokens as _;
use runtime_core::{
    component, rx, stylesheet, ui, AlignItems, Element, FlexDirection, FlexWrap, IdealystSchema, JustifyContent,
    Length, Overflow, PointerEvents, Position, SafeAreaSides,
};

use crate::width::is_phone;
use crate::Action;
use crate::LayoutTheme;

/// The most verbs an [`ActionBar`] holds beside its primary; one more
/// belongs in the header's ⋮.
pub const ACTION_BAR_MAX_SECONDARY: usize = 2;

/// The narrowest the note sits beside the verbs: about four words a
/// line. Below it the note wraps onto a line of its own above them.
pub const ACTION_BAR_NOTE_MIN: f32 = 240.0;

/// How an [`ActionBar`]'s verbs sit on a phone. From `Sm` up they are
/// one right-aligned row either way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, IdealystSchema)]
pub enum PhoneActions {
    /// Full width and stacked, the primary first, where the thumb starts.
    #[default]
    Stacked,
    /// One row across the width, the primary last and twice the width of
    /// each other verb: a walk's Back / Skip / Next, read as a line.
    Row,
}

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
    /// screen: a rule above it, on the page's surface. A pinned bar stays
    /// at the screen's foot while the keyboard is up, under it.
    #[prop(static)]
    pub pinned: bool,
    /// A sentence at the bar's start saying what the primary records. A
    /// phone has no room for it and draws the verbs alone.
    #[prop(static)]
    pub note: Option<String>,
    /// How the verbs sit on a phone. Default [`PhoneActions::Stacked`].
    #[prop(static)]
    pub phone: PhoneActions,
}

/// A screen's closing actions: right-aligned with the primary last from
/// `Sm` up, after the `note` if there is one. On a phone, full width and
/// stacked with the primary first, where the thumb starts — or, with
/// `phone = PhoneActions::Row`, one row with the primary last and widest.
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
    let in_row = props.phone == PhoneActions::Row;
    let bar_style = move || {
        ActionBarRow()
            .phone(if in_row { ActionBarRowPhone::Row } else { ActionBarRowPhone::Stacked })
            .into_style_application()
    };
    // Each verb's share of a phone's row; nothing from `Sm`, where the
    // verbs take their own width. Read live, so crossing `Sm` restyles.
    let share = move |weight: ActionBarSlotShare| {
        move || {
            ActionBarSlot()
                .share(if in_row && is_phone() { weight } else { ActionBarSlotShare::None })
                .into_style_application()
        }
    };
    let row = ui! {
        view(style = bar_style) {
            if let Some(a) = primary {
                view(style = share(ActionBarSlotShare::Primary)) {
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
            }
            for a in secondary.iter(), key = a.label.clone() {
                view(style = share(ActionBarSlotShare::Other)) {
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
        // The bar's surface runs to the screen's bottom edge and its
        // verbs stand clear of the home indicator. While the keyboard is
        // up the bar stays at that edge, under the keyboard, and gives its
        // space to the content: riding on the keyboard, it reads as the
        // next thing to type into.
        let keyboard = runtime_core::keyboard_inset();
        let pinned_style = move || {
            ActionBarPinned()
                .keyboard(if keyboard.get().is_visible() {
                    ActionBarPinnedKeyboard::Up
                } else {
                    ActionBarPinnedKeyboard::Down
                })
                .into_style_application()
        };
        ui! { view(style = pinned_style, safe_area = SafeAreaSides::BOTTOM) { row } }
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
        // A phone's arrangement. `row` is the `sm` row's own values, so
        // it reads the same whichever of the two the cascade puts last.
        variant phone {
            #[default]
            stacked(_t) {}
            row(_t) {
                flex_direction: FlexDirection::RowReverse,
                flex_wrap: FlexWrap::WrapReverse,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::FlexStart,
            }
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
    // A verb's seat. In a phone's row the primary takes two shares of the
    // width and each other verb one; elsewhere the seat is its verb.
    ActionBarSlot<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
        }
        variant share {
            #[default]
            none(_t) {}
            primary(_t) {
                flex_grow: 2.0,
                flex_basis: Length::Px(0.0),
                min_width: Length::Px(0.0),
            }
            other(_t) {
                flex_grow: 1.0,
                flex_basis: Length::Px(0.0),
                min_width: Length::Px(0.0),
            }
        }
    }
}

stylesheet! {
    ActionBarPinned<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            flex_shrink: 0.0,
            padding_top: t.spacing.md(),
            padding_left: LayoutTheme::tokens().gutter.phone(),
            padding_right: LayoutTheme::tokens().gutter.phone(),
            padding_bottom: t.spacing.md(),
            border_top_width: 1.0,
            border_top_color: t.color.border(),
            background: t.color.surface(),
        }
        variant keyboard {
            #[default]
            down(_t) {}
            up(_t) {
                max_height: Length::Px(0.0),
                overflow: Overflow::Hidden,
                padding_top: Length::Px(0.0),
                padding_bottom: Length::Px(0.0),
                border_top_width: 0.0,
            }
        }
        breakpoint sm(t) {
            padding_left: LayoutTheme::tokens().gutter.tablet(),
            padding_right: LayoutTheme::tokens().gutter.tablet(),
        }
        breakpoint lg(t) {
            padding_left: LayoutTheme::tokens().gutter.desktop(),
            padding_right: LayoutTheme::tokens().gutter.desktop(),
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
