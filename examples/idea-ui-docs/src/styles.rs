//! Local stylesheets for the docs app shell. Idea-ui components
//! handle their own visual surfaces; these styles cover the
//! framework-level chrome that idea-ui doesn't have a vocabulary
//! for yet — full-height page background, sidebar column, the
//! preview/control split.
//!
//! All sheets close over `IdeaThemeRef` so the docs follow the
//! installed theme (dark/light) without re-declaring tokens.

use framework_core::stylesheet;
use framework_core::{AlignItems, Color, FlexDirection, JustifyContent, Length};
use idea_ui::{IdeaTheme, IdeaThemeRef};

stylesheet! {
    pub PageRoot<IdeaThemeRef> {
        base(t) {
            background: t.colors().background.clone(),
            color: t.colors().text.clone(),
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
        base(t) {
            background: t.colors().surface.clone(),
            border_right_width: 1.0,
            border_right_color: t.colors().border.clone(),
            padding: t.spacing().lg,
            gap: Length::Px(t.spacing().xs),
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
    pub Content<IdeaThemeRef> {
        base(t) {
            padding: t.spacing().xxl,
            gap: Length::Px(t.spacing().xl),
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
        base(t) {
            background: t.colors().surface.clone(),
            border_radius: t.radius().lg,
            border_width: 1.0,
            border_color: t.colors().border.clone(),
            padding: t.spacing().xl,
            gap: Length::Px(t.spacing().lg),
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
        base(t) {
            flex_direction: FlexDirection::Row,
            gap: Length::Px(t.spacing().xl),
            align_items: AlignItems::FlexStart,
            flex_wrap: framework_core::FlexWrap::Wrap,
        }
    }
}

stylesheet! {
    pub PreviewBox<IdeaThemeRef> {
        base(t) {
            // No tray surface — the DemoCard's white background
            // shows through. Padding + gap keep the demo content
            // breathing without visually nesting "boxes inside
            // boxes."
            padding: t.spacing().md,
            gap: Length::Px(t.spacing().md),
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
        base(t) {
            min_width: 280.0,
            max_width: 360.0,
            gap: Length::Px(t.spacing().sm),
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
        base(t) {
            width: Length::pct(100.0),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            gap: Length::Px(t.spacing().md),
        }
    }
}

/// Sidebar nav link. Active variant flips the styling so the
/// current route stands out.
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
                background: t.colors().primary.clone(),
                color: t.colors().primary_text.clone(),
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
