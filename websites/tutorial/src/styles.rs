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
            // The secondary background, shared with the backdrop behind
            // the page card — the card's shadow is the only divider.
            background: t.color.surface_alt(),
        }
        transitions {
            background: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub ScreenScroll<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            width: Length::pct(100.0),
            // Fill the navigator body so the page scrolls inside its own
            // region — the navigator outlet doesn't scroll, the screen does.
            flex_grow: 1.0,
            flex_shrink: 1.0,
            flex_basis: 0.0,
            // The page card's own white.
            background: t.color.surface(),
            color: t.color.text(),
        }
        transitions {
            background: 250ms EaseInOut,
            color: 250ms EaseInOut,
        }
    }
}

// The padded prose column of a lesson page. It flexes to the space beside
// the workspace rather than capping at a reading width.
stylesheet! {
    pub PagePad<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            width: Length::pct(100.0),
            padding: 24.0,
            gap: 24.0,
        }
        // The card's own edge already frames the top; a full 48 there
        // pushed every title down.
        breakpoint lg(_t) {
            padding_top: 24.0,
            padding_horizontal: 48.0,
            padding_bottom: 48.0,
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

// A chapter with no written lessons yet, under its header.
stylesheet! {
    pub SidebarSoon<IdeaThemeRef> {
        base(t) {
            padding_vertical: t.spacing.xs(),
            padding_horizontal: t.spacing.md(),
            font_size: 13.0,
            color: t.color.text_muted(),
            font_style: runtime_core::FontStyle::Italic,
        }
    }
}

// The «/» toggle beside a page's title. Zero-sized below the pin width,
// where the sidebar is a drawer the hamburger opens instead.
stylesheet! {
    pub SidebarToggle<IdeaThemeRef> {
        base(t) {
            width: 0.0,
            height: 0.0,
            overflow: Overflow::Hidden,
            flex_shrink: 0.0,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            border_radius: t.radius.md(),
            cursor: runtime_core::Cursor::Pointer,
        }
        breakpoint lg(_t) {
            width: 28.0,
            height: 28.0,
        }
        state hovered(t) {
            background: t.color.surface_alt(),
        }
        transitions {
            background: 150ms EaseOut,
        }
    }
}

stylesheet! {
    pub SidebarToggleGlyph<IdeaThemeRef> {
        base(t) {
            font_size: 20.0,
            line_height: 20.0,
            color: t.color.text_muted(),
        }
    }
}

// The sidebar toggle and a page's eyebrow label, side by side; on a
// lesson, the Next button closes the row on the right.
stylesheet! {
    pub EyebrowRow<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 8.0,
        }
    }
}

stylesheet! {
    pub EyebrowSpacer<()> {
        base(_t) { flex_grow: 1.0 }
    }
}

// The header's "Next →" link, drawn as a small outlined button.
stylesheet! {
    pub HeaderNext<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            padding_vertical: 5.0,
            padding_horizontal: 12.0,
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.md(),
            cursor: runtime_core::Cursor::Pointer,
        }
        state hovered(t) {
            background: t.color.surface_alt(),
        }
        transitions {
            background: 150ms EaseOut,
        }
    }
}

stylesheet! {
    pub HeaderNextText<IdeaThemeRef> {
        base(t) {
            font_size: 13.0,
            font_weight: runtime_core::FontWeight::Medium,
            color: t.color.text(),
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
            // On the backdrop, above the page card: no divider of its own.
            background: t.color.surface_alt(),
            overflow: Overflow::Hidden,
            flex_shrink: 0.0,
        }
        breakpoint lg(_t) {
            height: 0.0,
            padding_horizontal: 0.0,
        }
        transitions {
            background: 250ms EaseInOut,
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
            // Positions the Copy corner.
            position: Position::Relative,
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

// ---- Fiddle: the code workspace --------------------------------------------
//
// Fills the lesson's workspace pane top to bottom: the toolbar, the editor
// (which takes the spare height and scrolls), compiler output when there
// is any, a draggable divider, then the preview — the framed bundle. The
// toolbar sits on the page card's white; the editor and the preview are
// the darker cards, and the output is a card of its own.

stylesheet! {
    pub FiddleFrame<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            min_height: 0.0,
            min_width: 0.0,
            gap: t.spacing.md(),
        }
    }
}

stylesheet! {
    pub FiddleToolbar<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
            flex_shrink: 0.0,
        }
    }
}

stylesheet! {
    pub FiddleFileName<IdeaThemeRef> {
        base(t) {
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace",
            font_size: 13.0,
            font_weight: runtime_core::FontWeight::SemiBold,
            padding_vertical: 4.0,
            padding_horizontal: 10.0,
            border_radius: 6.0,
            background: t.color.surface_alt(),
            color: t.color.text(),
        }
    }
}

stylesheet! {
    pub FiddleStatus<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            text_align: TextAlign::Right,
            font_size: 12.0,
            color: t.color.text_muted(),
        }
        variant tone {
            #[default]
            quiet(_t) {}
            good(t) { color: t.intent.success.solid_bg() }
            bad(t) { color: t.intent.danger.solid_bg() }
        }
    }
}

stylesheet! {
    pub FiddleEditorPane<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            flex_shrink: 1.0,
            flex_basis: 0.0,
            min_height: 140.0,
            min_width: 0.0,
            background: t.color.surface_alt(),
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.lg(),
        }
    }
}

stylesheet! {
    pub FiddleEditorRow<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::FlexStart,
            flex_shrink: 0.0,
            min_width: Length::pct(100.0),
        }
    }
}

// Metrics match the editor's (13px font, 20px rows, 14px top inset), so
// each number sits on its line.
stylesheet! {
    pub FiddleGutter<()> {
        base(_t) {
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace",
            font_size: 13.0,
            line_height: 20.0,
            padding_top: 14.0,
            padding_left: 16.0,
            padding_right: 4.0,
            text_align: TextAlign::Right,
            flex_shrink: 0.0,
            user_select: runtime_core::UserSelect::None,
        }
    }
}

stylesheet! {
    pub FiddleEditor<()> {
        base(_t) {
            // The editor measures to its longest line; the scroller around
            // it owns the overflow.
            flex_grow: 1.0,
            flex_shrink: 0.0,
            align_self: runtime_core::AlignSelf::FlexStart,
        }
    }
}

stylesheet! {
    pub FiddleOutput<IdeaThemeRef> {
        base(t) {
            flex_shrink: 0.0,
            max_height: 200.0,
            padding: t.spacing.md(),
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.lg(),
            background: t.color.surface(),
        }
    }
}

stylesheet! {
    pub FiddleOutputText<()> {
        base(_t) {
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace",
            font_size: 12.0,
            line_height: 18.0,
        }
    }
}

stylesheet! {
    pub FiddlePreviewSection<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            flex_shrink: 0.0,
            gap: t.spacing.sm(),
        }
    }
}

// The draggable divider between the editor and the preview: a strip as
// tall as the gap it replaces, with a grip tinted while it is dragged.
stylesheet! {
    pub FiddleDivider<IdeaThemeRef> {
        base(_t) {
            height: 14.0,
            flex_shrink: 0.0,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            cursor: runtime_core::Cursor::RowResize,
            // The frame's gap already spaces its children; the strip IS
            // that space here.
            margin_vertical: -7.0,
        }
    }
}

stylesheet! {
    pub FiddleDividerGrip<IdeaThemeRef> {
        base(t) {
            width: 40.0,
            height: 4.0,
            border_radius: 2.0,
            background: t.color.border(),
        }
        variant active {
            #[default]
            off(_t) {}
            on(t) { background: t.intent.primary.solid_bg() }
        }
        transitions {
            background: 120ms EaseOut,
        }
    }
}

// The preview's card: the same darker card as the editor, with the
// framed bundle inset in it. Its height is
// the divider's (an inline layer on top of this sheet).
stylesheet! {
    pub FiddlePreviewCard<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            padding: t.spacing.md(),
            background: t.color.surface_alt(),
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.lg(),
        }
    }
}

// The framed bundle: a white page with a hairline edge, inside the card.
stylesheet! {
    pub BrowserFrame<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            width: Length::pct(100.0),
            flex_grow: 1.0,
            min_height: 0.0,
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.lg(),
            overflow: Overflow::Hidden,
            background: t.color.surface(),
        }
    }
}

stylesheet! {
    pub FiddlePreview<()> {
        base(_t) {
            width: Length::pct(100.0),
            flex_grow: 1.0,
            min_height: 0.0,
        }
    }
}

// ---- Course labels ----------------------------------------------------------

// The small uppercase label over a title: "THE IDEALYST COURSE" on the
// landing page, "LESSON 1.2 · 12 MIN · BUILDING UI" over a lesson.
stylesheet! {
    pub Eyebrow<IdeaThemeRef> {
        base(t) {
            font_size: 13.0,
            font_weight: runtime_core::FontWeight::SemiBold,
            letter_spacing: 1.4,
            text_transform: runtime_core::TextTransform::Uppercase,
            color: t.intent.primary.solid_bg(),
        }
    }
}

// The "Coming from React" aside: the Callout shape, in the success hue so
// it reads as a different kind of note.
stylesheet! {
    pub ReactNote<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.xs(),
            padding: t.spacing.md(),
            border_left_width: 3.0,
            border_left_color: t.intent.success.solid_bg(),
            border_radius: t.radius.md(),
            background: t.intent.success.soft_bg(),
        }
    }
}

// ---- Course landing page ----------------------------------------------------

stylesheet! {
    pub LandingPad<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            width: Length::pct(100.0),
            padding_vertical: 32.0,
            padding_horizontal: 24.0,
            gap: 20.0,
        }
        breakpoint lg(_t) {
            padding_top: 32.0,
            padding_bottom: 56.0,
            padding_horizontal: 44.0,
        }
    }
}

stylesheet! {
    pub HeroTitle<IdeaThemeRef> {
        base(t) {
            font_size: 56.0,
            line_height: 64.0,
            letter_spacing: -1.6,
            font_weight: runtime_core::FontWeight::Bold,
            color: t.color.text(),
        }
    }
}

stylesheet! {
    pub HeroLead<IdeaThemeRef> {
        base(t) {
            font_size: 22.0,
            line_height: 34.0,
            max_width: 860.0,
            color: t.color.text_muted(),
        }
    }
}

stylesheet! {
    pub StatRow<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: runtime_core::FlexWrap::Wrap,
            column_gap: 28.0,
            row_gap: 6.0,
            margin_bottom: 28.0,
        }
    }
}

stylesheet! {
    pub StatText<IdeaThemeRef> {
        base(t) {
            font_size: 16.0,
            color: t.color.text_muted(),
        }
    }
}

stylesheet! {
    pub PersonaCard<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: runtime_core::FlexWrap::Wrap,
            align_items: AlignItems::Center,
            gap: 32.0,
            padding: 40.0,
            background: t.color.surface(),
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: 20.0,
            margin_bottom: 36.0,
        }
    }
}

stylesheet! {
    pub PersonaPick<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::FlexStart,
            gap: 16.0,
            flex_grow: 1.0,
            flex_shrink: 1.0,
            flex_basis: 420.0,
            min_width: 0.0,
        }
    }
}

stylesheet! {
    pub PersonaQuestion<IdeaThemeRef> {
        base(t) {
            font_size: 18.0,
            font_weight: runtime_core::FontWeight::SemiBold,
            color: t.color.text(),
        }
    }
}

stylesheet! {
    pub PersonaAdvice<IdeaThemeRef> {
        base(t) {
            font_size: 17.0,
            line_height: 28.0,
            color: t.color.text_muted(),
        }
    }
}

stylesheet! {
    pub StartCard<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::FlexStart,
            gap: 8.0,
            padding: 28.0,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: 300.0,
            min_width: 0.0,
            border_radius: 16.0,
            background: t.intent.primary.soft_bg(),
        }
    }
}

stylesheet! {
    pub StartLessonTitle<IdeaThemeRef> {
        base(t) {
            font_size: 24.0,
            line_height: 30.0,
            font_weight: runtime_core::FontWeight::SemiBold,
            color: t.color.text(),
        }
    }
}

stylesheet! {
    pub StartLessonMeta<IdeaThemeRef> {
        base(t) {
            font_size: 16.0,
            color: t.color.text_muted(),
            margin_bottom: 12.0,
        }
    }
}

stylesheet! {
    pub StartButton<IdeaThemeRef> {
        base(t) {
            padding_vertical: 12.0,
            padding_horizontal: 24.0,
            border_radius: 10.0,
            background: t.intent.primary.solid_bg(),
            cursor: runtime_core::Cursor::Pointer,
        }
        state hovered(t) {
            background: t.intent.primary.fg(),
        }
        transitions {
            background: 150ms EaseOut,
        }
    }
}

stylesheet! {
    pub StartButtonText<IdeaThemeRef> {
        base(t) {
            font_size: 18.0,
            font_weight: runtime_core::FontWeight::SemiBold,
            color: t.intent.primary.solid_text(),
        }
    }
}

stylesheet! {
    pub ChapterRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: runtime_core::FlexWrap::Wrap,
            gap: 40.0,
            padding_vertical: 48.0,
            border_top_width: 1.0,
            border_top_color: t.color.border(),
        }
    }
}

stylesheet! {
    pub ChapterInfo<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: 10.0,
            flex_grow: 1.0,
            flex_shrink: 1.0,
            flex_basis: 300.0,
            max_width: 560.0,
            min_width: 0.0,
        }
    }
}

stylesheet! {
    pub ChapterNumber<IdeaThemeRef> {
        base(t) {
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace",
            font_size: 20.0,
            font_weight: runtime_core::FontWeight::Bold,
            color: t.intent.primary.solid_bg(),
        }
    }
}

stylesheet! {
    pub ChapterTitle<IdeaThemeRef> {
        base(t) {
            font_size: 30.0,
            line_height: 36.0,
            font_weight: runtime_core::FontWeight::Bold,
            color: t.color.text(),
        }
    }
}

stylesheet! {
    pub ChapterSummary<IdeaThemeRef> {
        base(t) {
            font_size: 18.0,
            line_height: 28.0,
            color: t.color.text_muted(),
        }
    }
}

stylesheet! {
    pub ChapterMeta<IdeaThemeRef> {
        base(t) {
            font_size: 15.0,
            color: t.color.text_muted(),
        }
    }
}

stylesheet! {
    pub LessonGrid<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: runtime_core::FlexWrap::Wrap,
            align_content: runtime_core::AlignContent::FlexStart,
            column_gap: 24.0,
            flex_grow: 2.0,
            flex_shrink: 1.0,
            flex_basis: 560.0,
            min_width: 0.0,
        }
    }
}

stylesheet! {
    pub LessonItem<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 14.0,
            padding_vertical: 16.0,
            padding_horizontal: 8.0,
            border_bottom_width: 1.0,
            border_bottom_color: t.color.border(),
            cursor: runtime_core::Cursor::Pointer,
        }
        state hovered(t) {
            background: t.color.surface_alt(),
        }
        transitions {
            background: 120ms EaseOut,
        }
    }
}

// One lesson's slot in the grid: two columns where the row is wide
// enough, one where it isn't.
stylesheet! {
    pub LessonCell<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            flex_shrink: 1.0,
            flex_basis: 300.0,
            min_width: 0.0,
        }
    }
}

// A lesson not written yet: the same row, receded.
stylesheet! {
    pub LessonItemPlanned<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 14.0,
            padding_vertical: 16.0,
            padding_horizontal: 8.0,
            border_bottom_width: 1.0,
            border_bottom_color: t.color.border(),
            opacity: 0.55,
        }
    }
}

stylesheet! {
    pub LessonNumber<IdeaThemeRef> {
        base(t) {
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace",
            font_size: 15.0,
            width: 34.0,
            flex_shrink: 0.0,
            color: t.color.text_muted(),
        }
    }
}

stylesheet! {
    pub LessonTitle<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            flex_shrink: 1.0,
            font_size: 17.0,
            line_height: 24.0,
            font_weight: runtime_core::FontWeight::Medium,
            color: t.color.text(),
        }
    }
}

stylesheet! {
    pub LessonMinutes<IdeaThemeRef> {
        base(t) {
            flex_shrink: 0.0,
            font_size: 15.0,
            color: t.color.text_muted(),
        }
    }
}

stylesheet! {
    pub LessonSoon<IdeaThemeRef> {
        base(t) {
            flex_shrink: 0.0,
            padding_vertical: 3.0,
            padding_horizontal: 10.0,
            border_radius: 999.0,
            font_size: 12.0,
            font_weight: runtime_core::FontWeight::SemiBold,
            letter_spacing: 1.0,
            text_transform: runtime_core::TextTransform::Uppercase,
            background: t.color.surface_alt(),
            color: t.color.text_muted(),
        }
    }
}

// ---- Lesson page: prose beside a workspace ----------------------------------
//
// From the workspace's dock breakpoint (`xl`) the prose and the workspace
// sit side by side, each scrolling on its own, so the editor or terminal
// stays in view while the reader scrolls the lesson. Narrower, the prose
// is the page and the workspace is a drawer over it.

stylesheet! {
    pub LessonSplit<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            min_height: 0.0,
            width: Length::pct(100.0),
            height: Length::pct(100.0),
            // The containing block for the drawer, its scrim and its opener.
            position: Position::Relative,
            // The page card's own white.
            background: t.color.surface(),
            color: t.color.text(),
        }
        // Side by side where the workspace docks (`WORKSPACE_DOCK_AT`);
        // below it the workspace is a drawer and the prose is the page.
        breakpoint xl(_t) {
            flex_direction: FlexDirection::Row,
        }
        transitions {
            background: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub LessonProse<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            flex_shrink: 1.0,
            flex_basis: 0.0,
            min_height: 0.0,
            min_width: 0.0,
        }
    }
}

// The docked workspace: a fixed-width column beside the prose, on the
// page card's white, with the workspace's own cards inset in it.
stylesheet! {
    pub LessonWorkspace<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            width: 640.0,
            flex_shrink: 0.0,
            min_height: 0.0,
            padding_top: t.spacing.md(),
            padding_right: t.spacing.md(),
            padding_bottom: t.spacing.md(),
            padding_left: t.spacing.sm(),
            background: t.color.surface(),
        }
    }
}

// The workspace drawer below the dock breakpoint: the content of a
// right-edge viewport `overlay`, full height, the docked width — or the
// whole width of a screen narrower than that.
stylesheet! {
    pub DrawerPanel<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            height: Length::pct(100.0),
            width: Length::pct(100.0),
            max_width: 640.0,
            padding: t.spacing.md(),
            background: t.color.surface(),
            shadow: runtime_core::Shadow {
                x: -8.0,
                y: 0.0,
                blur: 32.0,
                color: t.effect.shadow(),
            },
        }
    }
}

// The drawer's top bar: its Close button, right-aligned.
stylesheet! {
    pub DrawerBar<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::FlexEnd,
            flex_shrink: 0.0,
            margin_bottom: 8.0,
        }
    }
}

// The drawer overlay's scrim. The overlay's own backdrop (which covers the
// whole window and closes the drawer when tapped) is transparent; its
// `backdrop_style` paints a layer INSIDE it, which must fill it itself.
stylesheet! {
    pub DrawerScrim<()> {
        base(_t) {
            position: Position::Absolute,
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
            background: Color("rgba(15, 23, 42, 0.32)".into()),
        }
    }
}

// The button that opens the drawer, floating at the page's bottom-right.
stylesheet! {
    pub DrawerOpener<()> {
        base(_t) {
            position: Position::Absolute,
            right: 16.0,
            bottom: 16.0,
            z_index: 18,
        }
    }
}


stylesheet! {
    pub LessonMetaRow<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: runtime_core::FlexWrap::Wrap,
            column_gap: 20.0,
        }
    }
}

stylesheet! {
    pub LessonMeta<IdeaThemeRef> {
        base(t) {
            font_size: 15.0,
            color: t.color.text_muted(),
        }
    }
}

stylesheet! {
    pub LearnCard<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: 12.0,
            padding: 24.0,
            border_radius: 16.0,
            border_width: 1.0,
            border_color: t.color.border(),
            background: t.color.surface(),
        }
    }
}

stylesheet! {
    pub LearnRow<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::FlexStart,
            gap: 12.0,
        }
    }
}

stylesheet! {
    pub LearnArrow<IdeaThemeRef> {
        base(t) {
            font_size: 17.0,
            line_height: 26.0,
            color: t.intent.primary.solid_bg(),
        }
    }
}

stylesheet! {
    pub LearnText<IdeaThemeRef> {
        base(t) {
            flex_shrink: 1.0,
            font_size: 17.0,
            line_height: 26.0,
            color: t.color.text(),
        }
    }
}

// The Copy button, pinned to a code panel's top-right corner.
stylesheet! {
    pub CopyCorner<()> {
        base(_t) {
            position: Position::Absolute,
            top: 10.0,
            right: 10.0,
        }
    }
}
