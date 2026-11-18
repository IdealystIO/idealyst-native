//! [`StateMessage`] — nothing to draw: a glyph over a message.

use idea_ui::{tokens, typography_kind, Icon, Typography};
use runtime_core::{
    component, stylesheet, ui, AlignItems, Element, FlexDirection, IconData, IdealystSchema, JustifyContent, Length,
    TextAlign,
};

/// The glyph's size, in px.
const GLYPH_PX: f32 = 32.0;
/// The least height the message stands in, so it keeps clear of a
/// toolbar where its room is only as tall as its content.
const SEAT_MIN_HEIGHT: f32 = 240.0;
/// The message's widest measure: a sentence wraps as a block under the
/// glyph rather than running the width of a desktop table.
const WORDS_MAX_WIDTH: f32 = 360.0;

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct StateMessageProps {
    /// The glyph over the message.
    #[prop(static)]
    pub icon: Option<IconData>,
    /// The message: what is not here.
    #[prop(static)]
    pub title: String,
    /// A second, quieter line.
    #[prop(static)]
    pub body: Option<String>,
    /// The one action, under the message: a filled `Button`.
    pub action: Option<Element>,
    /// The seat's robot/E2E id.
    #[prop(static)]
    pub test_id: &'static str,
}

impl Default for StateMessageProps {
    fn default() -> Self {
        Self { icon: None, title: String::new(), body: None, action: None, test_id: "state-message" }
    }
}

/// A glyph over a message, centred in the room it is given, with the
/// one action that fixes it under it. The shape of a list's blank,
/// empty and failed states (rules `blank-state`, `empty-state`); a
/// surface that is not a list (a board of cards) draws its blank state
/// with it too.
///
/// ```ignore
/// StateMessage(
///     icon = Some(LAYOUT_DASHBOARD),
///     title = "No widgets yet".to_string(),
///     action = Some(ui! { Button(label = "Add widget", on_click = on_add.clone()) }),
/// )
/// ```
#[component]
pub fn StateMessage(props: StateMessageProps) -> Element {
    let icon = props.icon;
    let body = props.body;
    let action = props.action;
    ui! {
        view(style = StateMessageSeat(), test_id = props.test_id) {
            if let Some(data) = icon {
                Icon(data = data, size = GLYPH_PX, color = Some(tokens().color.text_muted().resolve()))
            }
            view(style = StateMessageWords()) {
                Typography(content = props.title.clone(), muted = true, align = TextAlign::Center)
                if let Some(line) = body {
                    Typography(content = line, kind = typography_kind::BodySm, muted = true, align = TextAlign::Center)
                }
            }
            // A step further off than the lines are from each other: it
            // is what to do about the message, not part of it.
            if let Some(button) = action {
                view(style = StateMessageAction()) { button }
            }
        }
    }
}

stylesheet! {
    // The side padding keeps a long message on a phone off the screen's
    // edge where the seat bleeds.
    StateMessageSeat<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            flex_grow: 1.0,
            min_height: Length::Px(SEAT_MIN_HEIGHT),
            gap: t.spacing.sm(),
            padding_left: t.spacing.lg(),
            padding_right: t.spacing.lg(),
            padding_top: t.spacing.xl(),
            padding_bottom: t.spacing.xl(),
        }
    }
}

stylesheet! {
    StateMessageWords<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            gap: t.spacing.xs(),
            max_width: Length::Px(WORDS_MAX_WIDTH),
        }
    }
}

stylesheet! {
    StateMessageAction<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            margin_top: t.spacing.xs(),
        }
    }
}
