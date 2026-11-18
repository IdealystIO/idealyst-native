//! [`HelpHint`] — the `?` beside a title or a label whose tooltip
//! explains it (rule `help-hint`).

use icons_lucide::CIRCLE_HELP;
use idea_ui::Tooltip;
use runtime_core::{component, icon, stylesheet, ui, AlignItems, Element, FlexDirection, IdealystSchema, IntoElement};

/// The `?` glyph, in px.
const HELP_HINT_PX: f32 = 15.0;

#[runtime_core::props]
#[derive(IdealystSchema, Default)]
pub struct HelpHintProps {
    /// The explanation, a sentence or two.
    #[prop(static)]
    pub text: String,
}

/// A muted `?` whose tooltip explains the thing it stands beside: on
/// hover with a mouse, on a long press by touch. For what a screen's
/// structure cannot say; `SectionHeading` and `LabelValue` draw it from
/// their `hint`.
///
/// ```ignore
/// HelpHint(text = "Added to every quantity before it is priced.".to_string())
/// ```
#[component]
pub fn HelpHint(props: &HelpHintProps) -> Element {
    let text = props.text.clone();
    // The `icon` builder rather than `Icon`: the colour is a token
    // closure, which is the primitive's own spelling.
    let glyph =
        icon(CIRCLE_HELP).size(HELP_HINT_PX).color(|| idea_ui::tokens().color.text_muted().resolve()).into_element();
    ui! {
        Tooltip(text = text) {
            view(style = HelpHintSeat()) { glyph }
        }
    }
}

stylesheet! {
    // The glyph's own box: what the tooltip anchors to and a pointer
    // rests on.
    HelpHintSeat<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            flex_shrink: 0.0,
        }
    }
}
