//! Local stylesheets for the docs app shell. Idea-ui components
//! handle their own visual surfaces; these styles cover the
//! framework-level chrome that idea-ui doesn't have a vocabulary
//! for yet — full-height page background, sidebar column, the
//! preview/control split.
//!
//! All sheets close over `IdeaThemeRef` so the docs follow the
//! installed theme (dark/light) without re-declaring tokens.

use framework_core::stylesheet;
use framework_core::{AlignItems, Color, FlexDirection, JustifyContent, Length, Tokenized};
use idea_ui::{IdeaTheme, IdeaThemeRef};

stylesheet! {
    pub PageRoot<IdeaThemeRef> {
        base(_t) {
            background: Tokenized::token("color-background", Color("#f7f8fb".into())),
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
            min_height: Length::pct(100.0),
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
            gap: Tokenized::token("spacing-xs", Length::Px(4.0)),
            flex_direction: FlexDirection::Column,
            // Lock width — in a flex row, items shrink by default
            // when the content gets wide. `flex_shrink: 0` keeps
            // the sidebar at its declared width regardless of how
            // big the page content gets.
            width: 240.0,
            min_width: 240.0,
            max_width: 240.0,
            flex_shrink: 0.0,
            min_height: Length::pct(100.0),
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
    pub Content<IdeaThemeRef> {
        base(_t) {
            padding: Tokenized::token("spacing-xxl", Length::Px(32.0)),
            gap: Tokenized::token("spacing-xl", Length::Px(24.0)),
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
        }
    }
}

/// One demo card on a page. Holds the preview + the controls
/// panel side by side. The preview can grow; the controls panel
/// is a fixed-ish width.
stylesheet! {
    pub DemoCard<IdeaThemeRef> {
        base(_t) {
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            border_radius: Tokenized::token("radius-lg", Length::Px(12.0)),
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            padding: Tokenized::token("spacing-xl", Length::Px(24.0)),
            gap: Tokenized::token("spacing-lg", Length::Px(16.0)),
            flex_direction: FlexDirection::Column,
        }
        transitions {
            background: 250ms EaseInOut,
            border_color: 250ms EaseInOut,
        }
    }
}

/// Side-by-side row containing [preview, controls]. Wraps to
/// stacking on narrow viewports — preview gets `flex_grow: 1` so
/// it fills the leftover space.
stylesheet! {
    pub DemoRow<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            gap: Tokenized::token("spacing-xl", Length::Px(24.0)),
            align_items: AlignItems::FlexStart,
            flex_wrap: framework_core::FlexWrap::Wrap,
        }
    }
}

stylesheet! {
    pub PreviewBox<IdeaThemeRef> {
        base(_t) {
            // No tray surface — the DemoCard's white background
            // shows through. Padding + gap keep the demo content
            // breathing without visually nesting "boxes inside
            // boxes."
            padding: Tokenized::token("spacing-md", Length::Px(12.0)),
            gap: Tokenized::token("spacing-md", Length::Px(12.0)),
            flex_direction: FlexDirection::Column,
            // `flex_basis: 0 + flex_grow: 2` so the preview claims
            // most of the row width without depending on
            // intrinsic content width. (See the divider-collapse
            // story in the layout-fix commit message for the
            // long version.)
            flex_basis: 0.0,
            flex_grow: 2.0,
            flex_shrink: 1.0,
            min_width: 240.0,
            min_height: 120.0,
            align_items: AlignItems::Stretch,
            justify_content: JustifyContent::Center,
        }
    }
}

stylesheet! {
    pub ControlsBox<IdeaThemeRef> {
        base(_t) {
            min_width: 280.0,
            max_width: 360.0,
            gap: Tokenized::token("spacing-sm", Length::Px(8.0)),
        }
    }
}

/// Inner wrapper that gives the preview content a known
/// `width: 100%` container, so full-width children (Divider,
/// Alert, etc.) have something concrete to expand into.
///
/// Why this isn't redundant with PreviewBox: the framework's
/// `switch` primitive (used by `DocControls::reactive_preview`)
/// inserts an unstyled `<div>` placeholder between PreviewBox and
/// the actual content. That placeholder is `display: block` with no
/// explicit width, so it gets shrink-fit to content. A full-width
/// child of the placeholder resolves `100%` against the placeholder's
/// collapsed width — landing at zero. Wrapping the preview in this
/// extra view (which the docs code controls and can make 100%-wide)
/// happens *inside* the placeholder, so the placeholder's content is
/// now genuinely 100% of PreviewBox's width, and shrink-fitting
/// produces the desired full-width box.
stylesheet! {
    pub PreviewSlot<IdeaThemeRef> {
        base(_t) {
            width: Length::pct(100.0),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            gap: Tokenized::token("spacing-md", Length::Px(12.0)),
        }
    }
}

/// Sidebar nav link. Active variant flips the styling so the
/// current route stands out.
stylesheet! {
    pub NavLink<IdeaThemeRef> {
        base(_t) {
            padding_vertical: Tokenized::token("spacing-sm", Length::Px(8.0)),
            padding_horizontal: Tokenized::token("spacing-md", Length::Px(12.0)),
            border_radius: Tokenized::token("radius-md", Length::Px(8.0)),
            background: Color("transparent".into()),
            color: Tokenized::token("color-text-muted", Color("#6b7280".into())),
            font_size: Tokenized::token("typography-size-md", Length::Px(14.0)),
            text_align: framework_core::TextAlign::Left,
        }
        variant active {
            #[default]
            off(_t) {}
            on(_t) {
                background: Tokenized::token("intent-primary-solid-bg", Color("#5b6cff".into())),
                color: Tokenized::token("intent-primary-solid-text", Color("#ffffff".into())),
            }
        }
        state hovered(_t) {
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
        }
        transitions {
            background: 200ms EaseOut,
            color: 200ms EaseOut,
        }
    }
}
