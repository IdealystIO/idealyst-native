//! Local stylesheets for the docs app shell.
//!
//! Idea-ui components handle their own surfaces; these sheets cover
//! framework-level chrome that idea-ui doesn't have vocabulary for
//! (full-height background, sidebar column, code blocks, etc.).
//! Every sheet closes over `IdeaThemeRef` so chrome follows the
//! installed theme.

use runtime_core::stylesheet;
use runtime_core::{AlignItems, Color, FlexDirection, Length, Tokenized};
use idea_ui::{IdeaTheme, IdeaThemeRef};

stylesheet! {
    pub PageRoot<IdeaThemeRef> {
        base(_t) {
            background: Tokenized::token("color-background", Color("#f7f8fb".into())),
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
            // Exactly viewport-height so the sidebar can pin and the
            // content area can scroll independently. `overflow: Hidden`
            // stops the whole page from scrolling as one block; the
            // sidebar's own ScrollView and the content-area ScrollView
            // each handle their own overflow.
            height: Length::pct(100.0),
            overflow: runtime_core::Overflow::Hidden,
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Stretch,
        }
        transitions {
            background: 250ms EaseInOut,
            color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub Sidebar<IdeaThemeRef> {
        base(_t) {
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            border_right_width: 1.0,
            border_right_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            padding: Tokenized::token("spacing-lg", Length::Px(16.0)),
            gap: Tokenized::token("spacing-md", Length::Px(12.0)),
            flex_direction: FlexDirection::Column,
            width: 260.0,
            min_width: 260.0,
            max_width: 260.0,
            flex_shrink: 0.0,
            // Exactly 100% of PageRoot (which is 100% of viewport).
            // This is what activates the surrounding ScrollView's
            // own overflow-y: auto — without a constrained height
            // the sidebar would just grow and PageRoot's
            // `overflow: Hidden` would clip the extra.
            height: Length::pct(100.0),
        }
        transitions {
            background: 250ms EaseInOut,
            border_right_color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub SidebarHeader<IdeaThemeRef> {
        base(_t) {
            padding_bottom: Tokenized::token("spacing-md", Length::Px(12.0)),
            border_bottom_width: 1.0,
            border_bottom_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            margin_bottom: Tokenized::token("spacing-sm", Length::Px(8.0)),
            gap: Tokenized::token("spacing-xs", Length::Px(4.0)),
            flex_direction: FlexDirection::Column,
        }
        transitions {
            border_bottom_color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub SidebarSection<IdeaThemeRef> {
        base(_t) {
            gap: Tokenized::token("spacing-xs", Length::Px(4.0)),
            margin_top: Tokenized::token("spacing-md", Length::Px(12.0)),
            flex_direction: FlexDirection::Column,
        }
    }
}

stylesheet! {
    pub SidebarSectionLabel<IdeaThemeRef> {
        base(_t) {
            color: Tokenized::token("color-text-muted", Color("#6b7280".into())),
            font_size: Tokenized::token("typography-size-xs", Length::Px(11.0)),
            font_weight: runtime_core::FontWeight::Bold,
            text_transform: runtime_core::TextTransform::Uppercase,
            padding_horizontal: Tokenized::token("spacing-md", Length::Px(12.0)),
            padding_vertical: Tokenized::token("spacing-xs", Length::Px(4.0)),
        }
    }
}

stylesheet! {
    pub Content<IdeaThemeRef> {
        base(_t) {
            padding: Tokenized::token("spacing-xxl", Length::Px(32.0)),
            gap: Tokenized::token("spacing-xl", Length::Px(24.0)),
            flex_direction: FlexDirection::Column,
            // Fill the rest of the row beside the sidebar; the
            // explicit `height: 100%` ensures the surrounding
            // ScrollView (in `web_layout`) has a constrained
            // height for its overflow-y: auto to engage. The old
            // `max_width: 920` made Content only 920px wide and
            // left the rest of the viewport blank; if you want
            // readable line lengths, put a `max_width` on an
            // inner container (page-level), not the scroll surface.
            flex_grow: 1.0,
            height: Length::pct(100.0),
        }
    }
}

// Sidebar nav link — two stylesheets because padding on a `Text`
// node is a framework no-op (Text is glyph rendering, not a
// container). Box concerns (padding, background, border-radius)
// belong to a wrapping View; glyph concerns (font, color) belong to
// the inner Text. The author tree is:
//   Link { View(style = NavLinkBox()) { Text(style = NavLinkText()) { label } } }
stylesheet! {
    pub NavLinkBox<IdeaThemeRef> {
        base(_t) {
            padding_vertical: Tokenized::token("spacing-sm", Length::Px(8.0)),
            padding_horizontal: Tokenized::token("spacing-md", Length::Px(12.0)),
            border_radius: Tokenized::token("radius-md", Length::Px(8.0)),
            background: Color("transparent".into()),
        }
        variant active {
            #[default]
            off(_t) {}
            on(_t) {
                background: Tokenized::token("intent-primary-solid-bg", Color("#5b6cff".into())),
            }
        }
        transitions {
            background: 200ms EaseOut,
        }
    }
}

stylesheet! {
    pub NavLinkText<IdeaThemeRef> {
        base(_t) {
            color: Tokenized::token("color-text-muted", Color("#6b7280".into())),
            font_size: Tokenized::token("typography-size-md", Length::Px(14.0)),
            text_align: runtime_core::TextAlign::Left,
        }
        variant active {
            #[default]
            off(_t) {}
            on(_t) {
                color: Tokenized::token("intent-primary-solid-text", Color("#ffffff".into())),
            }
        }
        state hovered(_t) {
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
        }
        transitions {
            color: 200ms EaseOut,
        }
    }
}

// Monospace block for inline code samples on pages. Two stylesheets
// — surface (the wrapping View) and text (the inner Text) — because
// the iOS backend's `apply_style_to_view` only honors View-relevant
// fields, and text properties (color, font_size, font_family) only
// take effect when applied directly to the Text node. Without
// splitting, `color` on the View was silently dropped and the code
// stayed UIKit-default black regardless of theme.
stylesheet! {
    pub CodeBlockSheet<IdeaThemeRef> {
        base(_t) {
            background: Tokenized::token("color-surface-alt", Color("#eef0f7".into())),
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            border_radius: Tokenized::token("radius-md", Length::Px(8.0)),
            padding: Tokenized::token("spacing-md", Length::Px(12.0)),
            overflow: runtime_core::Overflow::Hidden,
        }
        transitions {
            background: 250ms EaseInOut,
            border_color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub CodeBlockText<IdeaThemeRef> {
        base(_t) {
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace".to_string(),
            font_size: Tokenized::token("typography-size-sm", Length::Px(12.0)),
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
        }
        transitions {
            color: 250ms EaseInOut,
        }
    }
}
