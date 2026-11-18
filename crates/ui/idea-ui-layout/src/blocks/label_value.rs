//! [`LabelValue`] — a read-only fact: its label and its value (rule
//! `label-value`).

use idea_ui::Typography;

use super::HelpHint;
use runtime_core::{
    component, rx, stylesheet, ui, AlignItems, AlignSelf, Element, FlexDirection, IdealystSchema, Length, Reactive,
    StyleRules,
};

/// The label column's width beside its value.
pub const LABEL_WIDTH: f32 = 96.0;
/// The label column's width for a list whose labels run past
/// [`LABEL_WIDTH`].
pub const LABEL_WIDTH_WIDE: f32 = 160.0;

/// What an absent value reads as: not stated is not zero.
const ABSENT: &str = "—";

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct LabelValueProps {
    /// What the fact is: "Hired".
    #[prop(static)]
    pub label: String,
    /// The fact. `None` (or blank) reads as "—".
    pub value: Reactive<Option<String>>,
    /// A value that is more than text (a tag, a link), in place of
    /// `value`.
    pub content: Option<Element>,
    /// What the label cannot say for itself: a `?` beside it whose
    /// tooltip is this. `None` or blank mounts nothing.
    #[prop(static)]
    pub hint: Option<String>,
    /// The wide label column, for a list whose labels run long
    /// ("Effective since"). Every row of one list takes the same, or
    /// its values stop lining up.
    #[prop(static)]
    pub wide_label: bool,
    /// The value has no line of text to share with the label (a
    /// signature, a strip of photos): beside it, the label sits at its
    /// top rather than on its baseline.
    #[prop(static)]
    pub top: bool,
}

impl Default for LabelValueProps {
    fn default() -> Self {
        Self {
            label: String::new(),
            value: Reactive::Static(None),
            content: None,
            hint: None,
            wide_label: false,
            top: false,
        }
    }
}

/// A label and its value: beside each other from `Sm` up, the label
/// muted in a fixed column; the label above the value on a phone. A
/// list of them is a `Stack` with the `sm` gap.
///
/// ```ignore
/// Stack(gap = StackGap::Sm) {
///     LabelValue(label = "Hired".to_string(), value = Some(hired_on))
///     LabelValue(label = "Division".to_string(), content = Some(ui! { Tag(…) }))
///     LabelValue(label = "Signature".to_string(), content = Some(signature), top = true)
/// }
/// ```
#[component]
pub fn LabelValue(props: LabelValueProps) -> Element {
    let label = props.label;
    let value = props.value;
    let label_style = match props.wide_label {
        false => LabelValueLabel().into_style_application(),
        true => LabelValueLabelWide().into_style_application(),
    };
    let label_style = match props.top {
        false => label_style,
        true => label_style
            .with_overrides(StyleRules { align_self: Some(AlignSelf::FlexStart), ..Default::default() }),
    };
    let shown = move || match value.get() {
        Some(v) if !v.trim().is_empty() => v,
        _ => ABSENT.to_string(),
    };
    let hint = props.hint.filter(|h| !h.trim().is_empty());
    let label = ui! {
        view(style = label_style) {
            view(style = LabelValueLabelLine()) {
                Typography(content = label, muted = true)
                if let Some(hint) = hint {
                    HelpHint(text = hint)
                }
            }
        }
    };
    match props.content {
        Some(content) => ui! {
            view(style = LabelValueRow()) {
                label
                view(style = LabelValueValue()) { content }
            }
        },
        None => ui! {
            view(style = LabelValueRow()) {
                label
                view(style = LabelValueValue()) { Typography(content = rx!(shown())) }
            }
        },
    }
}

stylesheet! {
    // The phone stacks the label over the value, `xs` apart (two parts
    // of one fact); from `sm` they share a line.
    LabelValueRow<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.xs(),
            min_width: Length::Px(0.0),
        }
        breakpoint sm(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Baseline,
            gap: t.spacing.md(),
        }
    }
}

stylesheet! {
    // The label and its hint on one line.
    LabelValueLabelLine<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.xs(),
        }
    }
}

stylesheet! {
    LabelValueLabel<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
        }
        breakpoint sm(_t) {
            width: Length::Px(LABEL_WIDTH),
            flex_shrink: 0.0,
        }
    }
}

stylesheet! {
    // Its own sheet rather than a variant: a variant arm cannot be
    // scoped to a breakpoint, and the phone label takes the width it
    // needs whatever the column is.
    LabelValueLabelWide<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
        }
        breakpoint sm(_t) {
            width: Length::Px(LABEL_WIDTH_WIDE),
            flex_shrink: 0.0,
        }
    }
}

stylesheet! {
    LabelValueValue<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            flex_shrink: 1.0,
            min_width: Length::Px(0.0),
        }
    }
}
