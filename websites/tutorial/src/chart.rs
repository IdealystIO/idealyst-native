//! Architecture-diagram primitives — labeled cards (`ChartBox`) stacked
//! into layers with `ChartArrow` connectors and `ChartRow` splits. Built
//! entirely from native `view`/`text` primitives (no SVG, no absolute
//! positioning) so the diagram renders identically on every backend. The
//! Architecture track composes these into per-concept diagrams.

use runtime_core::{
    component, stylesheet, ui, AlignItems, Color, Element, FlexDirection, FontWeight, Length,
    StyleApplication, TextAlign, TextTransform, Tokenized,
};
use idea_ui::IdeaThemeRef;

// =============================================================================
// Styles — cards, connectors, the row split, and the text tiers.
// =============================================================================

stylesheet! {
    pub ChartCard<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            gap: 6.0,
            padding: 18.0,
            border_radius: 14.0,
            border_width: 1.0,
            border_color: t.color.border(),
            background: t.color.surface(),
            width: Length::pct(100.0),
            min_width: 0.0,
        }
        transitions {
            background: 250ms EaseInOut,
            border_color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub ChartCardAccent<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            gap: 6.0,
            padding: 18.0,
            border_radius: 14.0,
            border_width: 1.0,
            border_color: t.intent.primary.fg(),
            background: t.intent.primary.soft_bg(),
            width: Length::pct(100.0),
            min_width: 0.0,
        }
        transitions {
            background: 250ms EaseInOut,
            border_color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub ChartSplit<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Stretch,
            gap: 14.0,
            width: Length::pct(100.0),
        }
    }
}

stylesheet! {
    pub ChartCell<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            flex_basis: 0.0,
            min_width: 0.0,
        }
    }
}

stylesheet! {
    pub ChartEyebrow<IdeaThemeRef> {
        base(t) {
            font_size: 11.0,
            font_weight: FontWeight::SemiBold,
            letter_spacing: 0.8,
            text_transform: TextTransform::Uppercase,
            color: t.color.text_muted(),
            text_align: TextAlign::Center,
        }
        transitions { color: 250ms EaseInOut, }
    }
}

stylesheet! {
    pub ChartTitle<IdeaThemeRef> {
        base(t) {
            font_size: 17.0,
            font_weight: FontWeight::Bold,
            color: t.color.text(),
            text_align: TextAlign::Center,
        }
        transitions { color: 250ms EaseInOut, }
    }
}

stylesheet! {
    pub ChartBody<IdeaThemeRef> {
        base(t) {
            font_size: 14.0,
            line_height: 20.0,
            color: t.color.text_muted(),
            text_align: TextAlign::Center,
        }
        transitions { color: 250ms EaseInOut, }
    }
}

stylesheet! {
    pub ChartArrowText<IdeaThemeRef> {
        base(t) {
            font_size: 20.0,
            color: t.color.text_muted(),
            text_align: TextAlign::Center,
        }
        transitions { color: 250ms EaseInOut, }
    }
}

// =============================================================================
// Components.
// =============================================================================

#[derive(Default)]
pub struct ChartBoxProps {
    /// Small uppercase kicker above the title. Empty = omit.
    pub eyebrow: String,
    pub title: String,
    pub body: String,
    /// Tint with the primary intent — reserved for the Framework Core card.
    pub accent: bool,
}

/// A single labeled card: optional eyebrow, a bold title, a muted body
/// line, all centered.
#[component]
pub fn ChartBox(props: ChartBoxProps) -> Element {
    // Both arms coerce to one `fn` pointer so the `if`/`else` unifies on a
    // single type; the closure form also re-resolves on theme change.
    let card: fn() -> StyleApplication = if props.accent {
        || StyleApplication::new(ChartCardAccent::sheet())
    } else {
        || StyleApplication::new(ChartCard::sheet())
    };
    let eyebrow = props.eyebrow;
    let title = props.title;
    let body = props.body;
    let eyebrow_style = move || StyleApplication::new(ChartEyebrow::sheet());
    let title_style = move || StyleApplication::new(ChartTitle::sheet());
    let body_style = move || StyleApplication::new(ChartBody::sheet());
    // 0.4.0: `if !eyebrow.is_empty()` lowers reactively (it contains a
    // method call), which would move the `String` into a re-callable
    // condition/branch closure (E0507). These are static presence checks
    // over plain props, so hoist them to a `let bool` — a bare-path
    // condition dispatches to the static `StaticCond` path, whose branch
    // runs once and may move its captures. See the
    // migration-0-3-0-to-0-4-0 guide.
    let has_eyebrow = !eyebrow.is_empty();
    let has_body = !body.is_empty();
    ui! {
        view(style = card) {
            if has_eyebrow {
                text(style = eyebrow_style) { eyebrow }
            }
            text(style = title_style) { title }
            if has_body {
                text(style = body_style) { body }
            }
        }
    }
}

#[derive(Default)]
pub struct ChartArrowProps {}

/// A centered downward connector between two stacked layers.
#[component]
pub fn ChartArrow(_props: &ChartArrowProps) -> Element {
    let style = move || StyleApplication::new(ChartArrowText::sheet());
    ui! { text(style = style) { "\u{2193}".to_string() } }
}

#[derive(Default)]
pub struct ChartLabelProps {
    pub label: String,
}

/// A centered overline titling a band of the diagram, without a card.
#[component]
pub fn ChartLabel(props: ChartLabelProps) -> Element {
    let style = move || StyleApplication::new(ChartEyebrow::sheet());
    let label = props.label;
    ui! { text(style = style) { label } }
}

#[derive(Default)]
pub struct ChartRowProps {
    pub children: Vec<Element>,
}

/// Lay children out in a row, each in an equal-width, equal-height cell.
#[component]
pub fn ChartRow(props: ChartRowProps) -> Element {
    let row = ChartSplit();
    ui! {
        view(style = row) {
            for child in props.children {
                view(style = ChartCell()) { child }
            }
        }
    }
}
