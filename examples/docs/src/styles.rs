//! Local stylesheets for the docs app shell.
//!
//! Idea-ui components handle their own surfaces; these sheets cover
//! framework-level chrome that idea-ui doesn't have vocabulary for
//! (full-height background, sidebar column, code blocks, etc.).
//! Every sheet closes over `IdeaThemeRef` so chrome follows the
//! installed theme.

use framework_core::stylesheet;
use framework_core::{AlignItems, Color, FlexDirection, Length};
use idea_ui::{IdeaTheme, IdeaThemeRef};

stylesheet! {
    pub PageRoot<IdeaThemeRef> {
        base(t) {
            background: t.colors().background.clone(),
            color: t.colors().text.clone(),
            // Exactly viewport-height so the sidebar can pin and the
            // content area can scroll independently. `overflow: Hidden`
            // stops the whole page from scrolling as one block; the
            // sidebar's own ScrollView and the content-area ScrollView
            // each handle their own overflow.
            height: Length::pct(100.0),
            overflow: framework_core::Overflow::Hidden,
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
        base(t) {
            background: t.colors().surface.clone(),
            border_right_width: 1.0,
            border_right_color: t.colors().border.clone(),
            padding: t.spacing().lg,
            gap: Length::Px(t.spacing().md),
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
        base(t) {
            padding_bottom: t.spacing().md,
            border_bottom_width: 1.0,
            border_bottom_color: t.colors().border.clone(),
            margin_bottom: t.spacing().sm,
            gap: Length::Px(t.spacing().xs),
            flex_direction: FlexDirection::Column,
        }
        transitions {
            border_bottom_color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub SidebarSection<IdeaThemeRef> {
        base(t) {
            gap: Length::Px(t.spacing().xs),
            margin_top: t.spacing().md,
            flex_direction: FlexDirection::Column,
        }
    }
}

stylesheet! {
    pub SidebarSectionLabel<IdeaThemeRef> {
        base(t) {
            color: t.colors().text_muted.clone(),
            font_size: t.typography().size_xs,
            font_weight: framework_core::FontWeight::Bold,
            text_transform: framework_core::TextTransform::Uppercase,
            padding_horizontal: t.spacing().md,
            padding_vertical: t.spacing().xs,
        }
    }
}

stylesheet! {
    pub Content<IdeaThemeRef> {
        base(t) {
            padding: t.spacing().xxl,
            gap: Length::Px(t.spacing().xl),
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

// Sidebar nav link. Active variant flips the styling so the
// current route stands out.
stylesheet! {
    pub NavLink<IdeaThemeRef> {
        base(t) {
            padding_vertical: t.spacing().sm,
            padding_horizontal: t.spacing().md,
            border_radius: t.radius().md,
            background: Color("transparent".into()),
            color: t.colors().text_muted.clone(),
            font_size: t.typography().size_md,
            text_align: framework_core::TextAlign::Left,
        }
        variant active {
            #[default]
            off(_t) {}
            on(t) {
                background: t.intents().primary.solid_bg.clone(),
                color: t.intents().primary.solid_text.clone(),
            }
        }
        state hovered(t) {
            color: t.colors().text.clone(),
        }
        transitions {
            background: 200ms EaseOut,
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
        base(t) {
            background: t.colors().surface_alt.clone(),
            border_width: 1.0,
            border_color: t.colors().border.clone(),
            border_radius: t.radius().md,
            padding: t.spacing().md,
            overflow: framework_core::Overflow::Hidden,
        }
        transitions {
            background: 250ms EaseInOut,
            border_color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub CodeBlockText<IdeaThemeRef> {
        base(t) {
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace".to_string(),
            font_size: t.typography().size_sm,
            color: t.colors().text.clone(),
        }
        transitions {
            color: 250ms EaseInOut,
        }
    }
}
