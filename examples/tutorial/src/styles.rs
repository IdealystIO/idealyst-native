//! Stylesheets for the tutorial's chrome — sidebar, nav links, the
//! content column, code panels, callouts, and the prev/next step bar.
//!
//! These are deliberately written in the **current** token API
//! (`Tokenized::token(name, fallback)`, no theme-struct reads) so the
//! tutorial dogfoods exactly what the Stylesheets track teaches. Token
//! names match idea-ui's theme so the installed light/dark palette
//! drives this chrome; the fallbacks keep it legible if a name drifts.

use runtime_core::stylesheet;
use runtime_core::{
    AlignItems, Color, FlexDirection, JustifyContent, Length, Overflow, Position, TextAlign,
    Tokenized,
};

// ---- Layout root + content column -----------------------------------------

stylesheet! {
    pub LayoutRoot<()> {
        base(_t) {
            width: Length::pct(100.0),
            height: Length::pct(100.0),
            position: Position::Relative,
            flex_direction: FlexDirection::Column,
        }
    }
}

stylesheet! {
    pub ScreenScroll<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            width: Length::pct(100.0),
            background: Tokenized::token("color-background", Color("#f7f5ef".into())),
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
        }
        transitions {
            background: 250ms EaseInOut,
            color: 250ms EaseInOut,
        }
    }
}

// Centered, padded reading column for each step page.
stylesheet! {
    pub PagePad<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            max_width: 760.0,
            align_self: runtime_core::AlignSelf::Center,
            width: Length::pct(100.0),
            padding: 48.0,
            gap: 24.0,
        }
    }
}

// ---- Sidebar ---------------------------------------------------------------

stylesheet! {
    pub SidebarBody<()> {
        base(_t) {
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            border_right_width: 1.0,
            border_right_color: Tokenized::token("color-border", Color("#e7e2d3".into())),
            padding: Tokenized::token("spacing-lg", Length::Px(16.0)),
            gap: Tokenized::token("spacing-xs", Length::Px(4.0)),
            flex_direction: FlexDirection::Column,
            min_height: Length::pct(100.0),
        }
        transitions {
            background: 250ms EaseInOut,
            border_right_color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub SidebarHeader<()> {
        base(_t) {
            padding_bottom: Tokenized::token("spacing-md", Length::Px(12.0)),
            border_bottom_width: 1.0,
            border_bottom_color: Tokenized::token("color-border", Color("#e7e2d3".into())),
            margin_bottom: Tokenized::token("spacing-sm", Length::Px(8.0)),
            gap: Tokenized::token("spacing-xs", Length::Px(4.0)),
            flex_direction: FlexDirection::Column,
        }
    }
}

stylesheet! {
    pub SidebarSection<()> {
        base(_t) {
            padding_top: Tokenized::token("spacing-md", Length::Px(12.0)),
            padding_bottom: Tokenized::token("spacing-xs", Length::Px(4.0)),
            padding_horizontal: Tokenized::token("spacing-md", Length::Px(12.0)),
            color: Tokenized::token("color-text-muted", Color("#8a8270".into())),
            font_size: 11.0,
            font_weight: runtime_core::FontWeight::SemiBold,
            letter_spacing: 0.8,
            text_transform: runtime_core::TextTransform::Uppercase,
            text_align: TextAlign::Left,
        }
    }
}

stylesheet! {
    pub SidebarFooter<()> {
        base(_t) {
            padding_top: Tokenized::token("spacing-md", Length::Px(12.0)),
            border_top_width: 1.0,
            border_top_color: Tokenized::token("color-border", Color("#e7e2d3".into())),
            flex_direction: FlexDirection::Column,
            gap: Tokenized::token("spacing-sm", Length::Px(8.0)),
        }
    }
}

stylesheet! {
    pub NavLink<()> {
        base(_t) {
            padding_vertical: Tokenized::token("spacing-sm", Length::Px(8.0)),
            padding_horizontal: Tokenized::token("spacing-md", Length::Px(12.0)),
            border_radius: Tokenized::token("radius-md", Length::Px(8.0)),
            background: Color("transparent".into()),
            color: Tokenized::token("color-text-muted", Color("#6b7280".into())),
            font_size: 14.0,
            text_align: TextAlign::Left,
        }
        variant active {
            #[default]
            off(_t) {}
            on(_t) {
                background: Tokenized::token("intent-primary-soft-bg", Color("rgba(91, 108, 255, 0.12)".into())),
                color: Tokenized::token("intent-primary-fg", Color("#3947d6".into())),
            }
        }
        state hovered(_t) {
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
        }
        transitions {
            background: 180ms EaseOut,
            color: 180ms EaseOut,
        }
    }
}

// ---- Code panel ------------------------------------------------------------

stylesheet! {
    pub CodePanel<()> {
        base(_t) {
            background: Tokenized::token("color-surface-alt", Color("#f4eedb".into())),
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e7e2d3".into())),
            border_radius: Tokenized::token("radius-lg", Length::Px(12.0)),
            padding: 20.0,
            overflow: Overflow::Hidden,
            min_width: 0.0,
        }
    }
}

stylesheet! {
    pub CodeText<()> {
        base(_t) {
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace",
            font_size: 14.0,
            line_height: 22.0,
            color: Tokenized::token("color-text", Color("#1f2328".into())),
        }
        transitions {
            color: 250ms EaseInOut,
        }
    }
}

// ---- Callout (tips + "read more in the docs") ------------------------------

stylesheet! {
    pub Callout<()> {
        base(_t) {
            background: Tokenized::token("intent-primary-soft-bg", Color("rgba(91, 108, 255, 0.10)".into())),
            border_left_width: 3.0,
            border_left_color: Tokenized::token("intent-primary-fg", Color("#3947d6".into())),
            border_radius: Tokenized::token("radius-md", Length::Px(8.0)),
            padding: 16.0,
            gap: 6.0,
            flex_direction: FlexDirection::Column,
        }
    }
}

stylesheet! {
    pub DocsLink<()> {
        base(_t) {
            color: Tokenized::token("intent-primary-fg", Color("#3947d6".into())),
            font_size: 14.0,
            font_weight: runtime_core::FontWeight::SemiBold,
            text_align: TextAlign::Left,
        }
        state hovered(_t) {
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
        }
        transitions {
            color: 150ms EaseOut,
        }
    }
}

// ---- Prev / next step bar --------------------------------------------------

stylesheet! {
    pub StepNavRow<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::SpaceBetween,
            align_items: AlignItems::Center,
            gap: 16.0,
            margin_top: 24.0,
            padding_top: 20.0,
            border_top_width: 1.0,
            border_top_color: Tokenized::token("color-border", Color("#e7e2d3".into())),
        }
    }
}

stylesheet! {
    pub StepNavLink<()> {
        base(_t) {
            color: Tokenized::token("intent-primary-fg", Color("#3947d6".into())),
            font_size: 15.0,
            font_weight: runtime_core::FontWeight::SemiBold,
        }
        state hovered(_t) {
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
        }
        transitions {
            color: 150ms EaseOut,
        }
    }
}
