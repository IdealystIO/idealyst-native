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
use idea_ui::IdeaThemeRef;

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

// The sidebar's scroll surface. Carries the sidebar background + right divider
// so they span the whole panel/viewport — the scrolled CONTENT (SidebarBody) is
// transparent and can be shorter than the panel, so the background must live on
// the scroll view (its clip fills the viewport) rather than on the content,
// which otherwise ends partway down and leaves the overflow on a bare window.
stylesheet! {
    pub SidebarScroll<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            flex_basis: 0.0,
            width: Length::pct(100.0),
            flex_direction: FlexDirection::Column,
            background: t.color.surface(),
            border_right_width: 1.0,
            border_right_color: t.color.border(),
        }
        transitions {
            background: 250ms EaseInOut,
            border_right_color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub ScreenScroll<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            width: Length::pct(100.0),
            // Fill the navigator body so the page scrolls inside its own
            // region — the drawer no longer owns scroll, the screen does.
            flex_grow: 1.0,
            flex_shrink: 1.0,
            flex_basis: 0.0,
            background: t.color.background(),
            color: t.color.text(),
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
    pub SidebarBody<IdeaThemeRef> {
        base(t) {
            // Background + right divider now live on `SidebarScroll` (the scroll
            // surface) so they span the full viewport; this content layer is
            // transparent and only owns padding/spacing.
            //
            // NO `min_height: 100%`: as a flex child of the scroll container it
            // would set the shrink floor to the viewport (not the content), so
            // flex-shrink clamped this container to the window height and its
            // overflowing children fell OUTSIDE its frame — invisible to AppKit
            // hit-testing (clicks below the window height did nothing even when
            // scrolled into view). Sizing to content keeps the frame around all
            // children. The scroll surface's background fills the viewport when
            // the list is short, so the floor isn't needed for looks.
            padding: t.spacing.lg(),
            gap: t.spacing.xs(),
            flex_direction: FlexDirection::Column,
            flex_shrink: 0.0,
        }
    }
}

stylesheet! {
    pub SidebarHeader<IdeaThemeRef> {
        base(t) {
            padding_bottom: t.spacing.md(),
            border_bottom_width: 1.0,
            border_bottom_color: t.color.border(),
            margin_bottom: t.spacing.sm(),
            gap: t.spacing.xs(),
            flex_direction: FlexDirection::Column,
        }
    }
}

stylesheet! {
    pub SidebarSection<IdeaThemeRef> {
        base(t) {
            padding_top: t.spacing.md(),
            padding_bottom: t.spacing.xs(),
            padding_horizontal: t.spacing.md(),
            color: t.color.text_muted(),
            font_size: 11.0,
            font_weight: runtime_core::FontWeight::SemiBold,
            letter_spacing: 0.8,
            text_transform: runtime_core::TextTransform::Uppercase,
            text_align: TextAlign::Left,
        }
    }
}

stylesheet! {
    pub SidebarFooter<IdeaThemeRef> {
        base(t) {
            padding_top: t.spacing.md(),
            border_top_width: 1.0,
            border_top_color: t.color.border(),
            flex_direction: FlexDirection::Column,
            gap: t.spacing.sm(),
        }
    }
}

stylesheet! {
    pub NavLink<IdeaThemeRef> {
        base(t) {
            padding_vertical: t.spacing.sm(),
            padding_horizontal: t.spacing.md(),
            border_radius: t.radius.md(),
            background: Color("transparent".into()),
            color: t.color.text_muted(),
            font_size: 14.0,
            text_align: TextAlign::Left,
        }
        variant active {
            #[default]
            off(_t) {}
            on(t) {
                background: t.intent.primary.soft_bg(),
                color: t.intent.primary.fg(),
            }
        }
        state hovered(t) {
            color: t.color.text(),
        }
        transitions {
            background: 180ms EaseOut,
            color: 180ms EaseOut,
        }
    }
}

// ---- Mobile header (hamburger + title, narrow viewports) -------------------
//
// Visible only while the AppShell sidebar is collapsed into the drawer:
// the `breakpoint lg` overlay zeroes the bar out at/above the pin width
// (`app()` aligns `lg_min` with the tutorial's 900-dp collapse point).
// Static breakpoint styling — a real `@media` rule on web + SSR — so the
// bar is correct on first paint and flips at EXACTLY the width where the
// AppShell sidebar collapses (no navigation dead zone).

stylesheet! {
    pub MobileHeader<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 4.0,
            width: Length::pct(100.0),
            height: 56.0,
            padding_horizontal: 8.0,
            border_bottom_width: 1.0,
            background: t.color.surface(),
            border_bottom_color: t.color.border(),
            overflow: Overflow::Hidden,
            flex_shrink: 0.0,
        }
        breakpoint lg(_t) {
            height: 0.0,
            padding_horizontal: 0.0,
            border_bottom_width: 0.0,
        }
        transitions {
            background: 250ms EaseInOut,
            border_bottom_color: 250ms EaseInOut,
        }
    }
}

// Pressable hamburger — square 40x40 touch target, rounded, subtle
// hover dim.
stylesheet! {
    pub MobileHeaderButton<IdeaThemeRef> {
        base(t) {
            width: 40.0,
            height: 40.0,
            border_radius: t.radius.md(),
            background: Color("transparent".into()),
            color: t.color.text(),
            font_size: 22.0,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            flex_shrink: 0.0,
        }
        state hovered(t) {
            background: t.color.surface_alt(),
        }
        transitions {
            background: 150ms EaseOut,
        }
    }
}

// Title wrapper — grows to fill the space after the menu button; the
// title text inside is left-aligned.
stylesheet! {
    pub MobileHeaderTitleWrap<()> {
        base(_t) {
            flex_basis: 0.0,
            flex_grow: 1.0,
            min_width: 0.0,
            padding_horizontal: 4.0,
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
        }
    }
}

stylesheet! {
    pub MobileHeaderTitle<IdeaThemeRef> {
        base(t) {
            color: t.color.text(),
            font_size: 17.0,
            font_weight: runtime_core::FontWeight::SemiBold,
            text_align: TextAlign::Left,
        }
        transitions {
            color: 250ms EaseInOut,
        }
    }
}

// ---- Code panel ------------------------------------------------------------

stylesheet! {
    pub CodePanel<IdeaThemeRef> {
        base(t) {
            background: t.color.surface_alt(),
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.lg(),
            padding: 20.0,
            overflow: Overflow::Hidden,
            min_width: 0.0,
        }
    }
}

stylesheet! {
    pub CodeText<IdeaThemeRef> {
        base(t) {
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace",
            font_size: 14.0,
            line_height: 22.0,
            color: t.color.text(),
        }
        transitions {
            color: 250ms EaseInOut,
        }
    }
}

// ---- Live demo panels ------------------------------------------------------
//
// The Reactivity and Foundations tracks each embed a running panel so a
// reader can watch a staged write land at the flush instead of taking the
// prose's word for it. Surface-tinted so it reads as "this is live", not
// "this is a snippet".

stylesheet! {
    pub DemoPanel<IdeaThemeRef> {
        base(t) {
            background: t.color.surface(),
            border_width: 1.0,
            border_color: t.intent.primary.fg(),
            border_radius: t.radius.lg(),
            padding: 20.0,
            gap: 12.0,
            flex_direction: FlexDirection::Column,
            min_width: 0.0,
        }
        transitions {
            background: 250ms EaseInOut,
            border_color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub DemoRow<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: runtime_core::FlexWrap::Wrap,
            align_items: AlignItems::Center,
            gap: 8.0,
        }
    }
}

stylesheet! {
    pub DemoReadout<IdeaThemeRef> {
        base(t) {
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace",
            font_size: 15.0,
            line_height: 22.0,
            color: t.color.text(),
        }
        transitions { color: 250ms EaseInOut, }
    }
}

stylesheet! {
    pub DemoTrace<IdeaThemeRef> {
        base(t) {
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace",
            font_size: 13.0,
            line_height: 20.0,
            color: t.color.text_muted(),
        }
        transitions { color: 250ms EaseInOut, }
    }
}

stylesheet! {
    pub DemoButton<IdeaThemeRef> {
        base(t) {
            padding_vertical: 8.0,
            padding_horizontal: 14.0,
            border_radius: t.radius.md(),
            border_width: 1.0,
            border_color: t.color.border(),
            background: t.color.surface_alt(),
            color: t.color.text(),
            font_size: 14.0,
            font_weight: runtime_core::FontWeight::SemiBold,
            cursor: runtime_core::Cursor::Pointer,
        }
        state hovered(t) {
            background: t.intent.primary.soft_bg(),
            border_color: t.intent.primary.fg(),
        }
        transitions {
            background: 150ms EaseOut,
            border_color: 150ms EaseOut,
        }
    }
}

// ---- Callout (tips + "read more in the docs") ------------------------------

stylesheet! {
    pub Callout<IdeaThemeRef> {
        base(t) {
            background: t.intent.primary.soft_bg(),
            border_left_width: 3.0,
            border_left_color: t.intent.primary.fg(),
            border_radius: t.radius.md(),
            padding: 16.0,
            gap: 6.0,
            flex_direction: FlexDirection::Column,
        }
    }
}

stylesheet! {
    pub DocsLink<IdeaThemeRef> {
        base(t) {
            color: t.intent.primary.fg(),
            font_size: 14.0,
            font_weight: runtime_core::FontWeight::SemiBold,
            text_align: TextAlign::Left,
        }
        state hovered(t) {
            color: t.color.text(),
        }
        transitions {
            color: 150ms EaseOut,
        }
    }
}

// ---- Prev / next step bar --------------------------------------------------

stylesheet! {
    pub StepNavRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::SpaceBetween,
            align_items: AlignItems::Center,
            gap: 16.0,
            margin_top: 24.0,
            padding_top: 20.0,
            border_top_width: 1.0,
            border_top_color: t.color.border(),
        }
    }
}

stylesheet! {
    pub StepNavLink<IdeaThemeRef> {
        base(t) {
            color: t.intent.primary.fg(),
            font_size: 15.0,
            font_weight: runtime_core::FontWeight::SemiBold,
        }
        state hovered(t) {
            color: t.color.text(),
        }
        transitions {
            color: 150ms EaseOut,
        }
    }
}
