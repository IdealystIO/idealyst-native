//! Local stylesheets for the marketing site's chrome (page background,
//! sidebar column, hero panel, nav links). The page content itself
//! uses `idea-ui` components, which read their styling from the
//! installed theme; these stylesheets handle the framework-level
//! layout primitives `idea-ui` doesn't have a vocabulary for.
//!
//! Palette is borrowed from the welcome example so the marketing site
//! reads as a continuation of the scaffold experience:
//!   - cream `#f7f5ef`     — page background (light)
//!   - navy  `#0a0c11`     — page background (dark)
//!   - warm sun gradient   — hero corner accent

use runtime_core::stylesheet;
use runtime_core::{
    AlignItems, Color, FlexDirection, Gradient, GradientKind, GradientStop, JustifyContent, Length, Shadow,
    Overflow, Position, RadialExtent, TextAlign, Tokenized, Transform,
};
use idea_ui::IdeaThemeRef;

use crate::typeface::INTER;

// =============================================================================
// Layout root — the View that wraps `mobile_header` + the screen's
// ScrollView. Fills the screen so the ScrollView's `height: 100%`
// has a real reference; `position: Relative` (framework default,
// stated explicitly) makes it the positioning context for the
// `position: Absolute` mobile header.
// =============================================================================

stylesheet! {
    pub LayoutRoot<IdeaThemeRef> {
        base(_t) {
            width: Length::pct(100.0),
            height: Length::pct(100.0),
            position: Position::Relative,
            flex_direction: FlexDirection::Column,
        }
    }
}

// =============================================================================
// Mobile header — narrow-viewport top bar
//
// Lives in the navigator's `top` slot (set via
// `.top_with(TopSlot::Custom(...))` in `lib.rs`). The slot mounts
// ONCE at navigator init and survives every screen swap — the
// header doesn't rebuild on navigation any more.
//
// Visibility is controlled via the `size` variant:
//   - `wide`: height 0, padding 0, border 0 — the
//     `.ui-nav-drawer-top` flex parent (`flex: 0 0 auto`)
//     collapses to zero height and the bar effectively disappears.
//   - `narrow`: the visible 56-px bar.
// We don't conditionally mount/unmount the subtree because the
// outer slot is already persistent — and CSS-collapsing is
// cheaper than re-running a `when()` branch on every breakpoint
// transition.
// =============================================================================

stylesheet! {
    pub MobileHeader<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 4.0,
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            border_bottom_color: Tokenized::token("color-border", Color("#e7e2d3".into())),
            font_family: &INTER,
            width: Length::pct(100.0),
            overflow: runtime_core::Overflow::Hidden,
        }
        variant size {
            #[default]
            wide(_t) {
                height: 0.0,
                padding_horizontal: 0.0,
                border_bottom_width: 0.0,
            }
            narrow(_t) {
                height: 56.0,
                padding_horizontal: 8.0,
                border_bottom_width: 1.0,
            }
        }
        transitions {
            background: 250ms EaseInOut,
            border_bottom_color: 250ms EaseInOut,
        }
    }
}

/// Pressable button for the leading menu / trailing action slot.
/// Square 40x40 touch target, rounded, subtle hover dim.
stylesheet! {
    pub MobileHeaderButton<IdeaThemeRef> {
        base(_t) {
            width: 40.0,
            height: 40.0,
            border_radius: Tokenized::token("radius-md", Length::Px(8.0)),
            background: Color("transparent".into()),
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
            font_size: 22.0,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            flex_shrink: 0.0,
        }
        state hovered(_t) {
            background: Tokenized::token("color-surface-alt", Color("#f4eedb".into())),
        }
        transitions {
            background: 150ms EaseOut,
        }
    }
}

/// Title wrapper — grows to fill the space between leading + trailing
/// slots; the title text inside is left-aligned (a center-aligned
/// header competes with the menu button visually on short titles).
stylesheet! {
    pub MobileHeaderTitleWrap<IdeaThemeRef> {
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
        base(_t) {
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
            font_size: 17.0,
            font_weight: runtime_core::FontWeight::SemiBold,
            text_align: TextAlign::Left,
            // Single-line; long titles ellipsize via the
            // browser-default for inline overflow.
        }
        transitions {
            color: 250ms EaseInOut,
        }
    }
}

// =============================================================================
// Sidebar body
//
// Drawer-navigator on web supplies the outer chrome:
//   `.ui-nav-drawer-root` — flex-row, 100% × 100% viewport.
//   `.ui-nav-drawer-sidebar` — flex:0 0 auto, height: 100%, overflow-y: auto.
//   `.ui-nav-drawer-body` — flex:1 1 auto, height: 100%, overflow: hidden.
//
// We don't need our own PageRoot / Sidebar / Content layout
// stylesheets anymore — the navigator owns that. `SidebarBody`
// styles the inner column the sidebar builder returns
// (background, padding, gap, divider border on the right edge,
// vertical flex layout).
// =============================================================================

stylesheet! {
    pub SidebarBody<IdeaThemeRef> {
        base(_t) {
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            border_right_width: 1.0,
            border_right_color: Tokenized::token("color-border", Color("#e7e2d3".into())),
            padding: Tokenized::token("spacing-lg", Length::Px(16.0)),
            gap: Tokenized::token("spacing-xs", Length::Px(4.0)),
            flex_direction: FlexDirection::Column,
            // Match the drawer-sidebar's full height so the right-edge
            // divider line spans the whole viewport even when the
            // nav-link list is short.
            min_height: Length::pct(100.0),
            // Inter for sidebar text; CSS inherits to every Text child.
            // Sibling to the screen-scroll subtree, so we set it here
            // too rather than relying on a shared ancestor.
            font_family: &INTER,
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
            border_bottom_color: Tokenized::token("color-border", Color("#e7e2d3".into())),
            margin_bottom: Tokenized::token("spacing-sm", Length::Px(8.0)),
            gap: Tokenized::token("spacing-xs", Length::Px(4.0)),
            flex_direction: FlexDirection::Column,
        }
        transitions {
            border_bottom_color: 250ms EaseInOut,
        }
    }
}

/// Footer row at the bottom of the sidebar — holds the dark-mode
/// switch. Preceded by an `idea_ui::Spacer` so the footer is pushed
/// to the visual bottom of the sidebar column even when the
/// nav-link list is short.
stylesheet! {
    pub SidebarFooter<IdeaThemeRef> {
        base(_t) {
            padding_top: Tokenized::token("spacing-md", Length::Px(12.0)),
            border_top_width: 1.0,
            border_top_color: Tokenized::token("color-border", Color("#e7e2d3".into())),
            flex_direction: FlexDirection::Column,
            gap: Tokenized::token("spacing-sm", Length::Px(8.0)),
        }
        transitions {
            border_top_color: 250ms EaseInOut,
        }
    }
}

/// Section header above a group of nav links in the sidebar.
stylesheet! {
    pub SidebarSection<IdeaThemeRef> {
        base(_t) {
            padding_top: Tokenized::token("spacing-md", Length::Px(12.0)),
            padding_bottom: Tokenized::token("spacing-xs", Length::Px(4.0)),
            padding_horizontal: Tokenized::token("spacing-md", Length::Px(12.0)),
            color: Tokenized::token("color-text-muted", Color("#8a8270".into())),
            font_size: 11.0,
            font_weight: runtime_core::FontWeight::SemiBold,
            letter_spacing: 0.8,
            text_align: TextAlign::Left,
        }
    }
}

stylesheet! {
    pub ScreenScroll<IdeaThemeRef> {
        base(_t) {
            // Screen container — flow content inside the drawer
            // navigator's scrollable body. No `height: 100%` (that
            // would clip content above body height and prevent
            // body-level scroll). `width: 100%` ensures full-bleed
            // children (hero, footer) span the viewport.
            // Background + font are inherited by every Text/View
            // descendant of the screen.
            flex_direction: FlexDirection::Column,
            width: Length::pct(100.0),
            background: Tokenized::token("color-background", Color("#f7f5ef".into())),
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
            font_family: &INTER,
        }
        transitions {
            background: 250ms EaseInOut,
            color: 250ms EaseInOut,
        }
    }
}

/// Padded wrapper for non-hero pages — gives them the same gutters
/// the hero would use, without forcing it on the home page.
///
/// Numbers are literal-pixel (not theme-tokenized) because they're
/// website-specific layout decisions, not theme tokens that should
/// scale with `set_theme`. The values target a Material-UI-style
/// reading experience: ~780 px max column for prose, 72 px between
/// top-level sections (so H2 headings have a clear vertical gap
/// from the preceding paragraph), 56 px outer padding so the column
/// sits comfortably inside the drawer body.
stylesheet! {
    pub PagePad<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            max_width: 820.0,
            // Centers the column within the screen-scroll wrapper.
            // Without this the content sits flush-left against the
            // sidebar's right edge; with it, the column gets equal
            // gutters on both sides and reads as the intentional
            // focal point.
            align_self: runtime_core::AlignSelf::Center,
            width: Length::pct(100.0),
        }
        variant size {
            #[default]
            wide(_t) {
                padding: 56.0,
                gap: 72.0,
            }
            narrow(_t) {
                padding: 24.0,
                gap: 48.0,
            }
        }
    }
}

// =============================================================================
// Page row + table-of-contents panel
//
// `layout_with_toc(...)` (see `shell.rs`) wraps the page content in a
// flex row: the PagePad column on the left, a sticky TOC on the
// right. Wide-viewport docs sites (Material UI, the React docs, etc.)
// have this pattern \u{2014} the TOC shows the H2/H3 outline and
// highlights the section currently in view.
// =============================================================================

stylesheet! {
    pub PageRow<IdeaThemeRef> {
        base(_t) {
            align_items: AlignItems::FlexStart,
            justify_content: JustifyContent::Center,
            width: Length::pct(100.0),
            max_width: 1200.0,
            align_self: runtime_core::AlignSelf::Center,
        }
        variant size {
            #[default]
            wide(_t) {
                flex_direction: FlexDirection::Row,
                gap: 64.0,
                padding: 56.0,
            }
            narrow(_t) {
                // Single-column stack on narrow viewports — the
                // PageColumn loses its TOC sibling, so a row layout
                // adds nothing but extra side gutters.
                flex_direction: FlexDirection::Column,
                gap: 32.0,
                padding: 24.0,
            }
        }
    }
}

stylesheet! {
    pub PageColumn<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: 72.0,
            flex_basis: 0.0,
            flex_grow: 1.0,
            min_width: 0.0,
            max_width: 820.0,
        }
    }
}

stylesheet! {
    pub SectionWrap<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: 16.0,
        }
    }
}

stylesheet! {
    pub TocPanel<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: Tokenized::token("spacing-xs", Length::Px(4.0)),
            width: 220.0,
            min_width: 220.0,
            flex_shrink: 0.0,
            padding_top: 8.0,
            // Sticky positioning so the TOC stays in view as the
            // page content scrolls. Web honours this directly; on
            // native targets the SDK will fall back to Relative
            // until the scroll-listener implementation lands (see
            // `Position::Sticky` doc comment in runtime-core).
            position: Position::Sticky,
            top: Length::Px(32.0),
        }
    }
}

stylesheet! {
    pub TocHeader<IdeaThemeRef> {
        base(_t) {
            color: Tokenized::token("color-text-muted", Color("#6b7280".into())),
            font_size: 11.0,
            font_weight: runtime_core::FontWeight::SemiBold,
            letter_spacing: 0.8,
            text_transform: runtime_core::TextTransform::Uppercase,
            padding_bottom: 8.0,
        }
    }
}

stylesheet! {
    pub TocLink<IdeaThemeRef> {
        base(_t) {
            padding_vertical: 6.0,
            padding_left: 12.0,
            border_left_width: 2.0,
            border_left_color: Tokenized::token("color-border", Color("#e7e2d3".into())),
            color: Tokenized::token("color-text-muted", Color("#6b7280".into())),
            font_size: 13.0,
            line_height: 18.0,
            text_align: TextAlign::Left,
        }
        variant active {
            #[default]
            off(_t) {}
            on(t) {
                border_left_color: Tokenized::token("intent-primary-fg", Color("#3947d6".into())),
                color: Tokenized::token("intent-primary-fg", Color("#3947d6".into())),
                font_weight: runtime_core::FontWeight::SemiBold,
            }
        }
        state hovered(_t) {
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
        }
        transitions {
            color: 180ms EaseOut,
            border_left_color: 180ms EaseOut,
        }
    }
}

// =============================================================================
// Nav link
// =============================================================================

stylesheet! {
    pub NavLink<IdeaThemeRef> {
        base(_t) {
            padding_vertical: Tokenized::token("spacing-sm", Length::Px(8.0)),
            padding_horizontal: Tokenized::token("spacing-md", Length::Px(12.0)),
            border_radius: Tokenized::token("radius-md", Length::Px(8.0)),
            background: Color("transparent".into()),
            color: Tokenized::token("color-text-muted", Color("#6b7280".into())),
            font_size: Tokenized::token("typography-size-md", Length::Px(14.0)),
            text_align: TextAlign::Left,
        }
        variant active {
            #[default]
            off(_t) {}
            on(_t) {
                // Theme-tokenized so the active-link highlight adapts
                // to dark mode. Uses idea-ui's intent-primary-soft
                // tokens (the same pair Badge/Tag/Alert use for the
                // Soft kind), so the active link reads as "selected"
                // in the framework's vocabulary on either palette.
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

// =============================================================================
// Hero — full-bleed top section on the Home page
// =============================================================================

/// Hero band — the first thing on / . Embeds a live wgpu simulator
/// alongside the headline, so vertical padding is tighter than a
/// text-only hero would use (the device's ~700 px height carries the
/// vertical rhythm).
stylesheet! {
    pub Hero<IdeaThemeRef> {
        base(_t) {
            position: Position::Relative,
            overflow: Overflow::Hidden,
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            // Slight surface lift over the page background.
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            border_bottom_width: 1.0,
            border_bottom_color: Tokenized::token("color-border", Color("#e7e2d3".into())),
        }
        variant size {
            #[default]
            wide(_t) {
                padding_horizontal: 64.0,
                padding_top: 72.0,
                padding_bottom: 72.0,
            }
            narrow(_t) {
                padding_horizontal: 24.0,
                padding_top: 48.0,
                padding_bottom: 48.0,
            }
        }
        transitions {
            background: 250ms EaseInOut,
            border_bottom_color: 250ms EaseInOut,
        }
    }
}

/// Static sun-glare disc anchored to the hero's top-right corner.
/// Same gradient stops as `welcome::sun_glare`'s bright frame, just
/// frozen — no animation. Pointer-events stay off the absolute child
/// so clicks on the hero text below pass through.
pub fn hero_glare_sheet() -> std::rc::Rc<runtime_core::StyleSheet> {
    use runtime_core::StyleRules;
    std::rc::Rc::new(runtime_core::StyleSheet::r#static(StyleRules {
        position: Some(Position::Absolute),
        top: Some(Tokenized::Literal(Length::Px(0.0))),
        right: Some(Tokenized::Literal(Length::Px(0.0))),
        width: Some(Tokenized::Literal(Length::Px(520.0))),
        height: Some(Tokenized::Literal(Length::Px(520.0))),
        transform: Some(vec![
            Transform::TranslateX(Length::Percent(45.0)),
            Transform::TranslateY(Length::Percent(-45.0)),
        ]),
        border_top_left_radius: Some(Tokenized::Literal(Length::Px(999.0))),
        border_top_right_radius: Some(Tokenized::Literal(Length::Px(999.0))),
        border_bottom_left_radius: Some(Tokenized::Literal(Length::Px(999.0))),
        border_bottom_right_radius: Some(Tokenized::Literal(Length::Px(999.0))),
        overflow: Some(Overflow::Hidden),
        background_gradient: Some(Gradient {
            kind: GradientKind::Radial {
                center: (0.5, 0.5),
                radius: 1.0,
                extent: RadialExtent::ClosestSide,
            },
            stops: vec![
                GradientStop { offset: 0.0,  color: Color("#fff6d8".into()) },
                GradientStop { offset: 0.30, color: Color("rgba(255, 210, 110, 0.55)".into()) },
                GradientStop { offset: 0.60, color: Color("rgba(255, 168, 60, 0.18)".into()) },
                GradientStop { offset: 0.85, color: Color("rgba(255, 168, 60, 0.04)".into()) },
                GradientStop { offset: 1.0,  color: Color("rgba(255, 168, 60, 0.0)".into()) },
            ],
        }),
        ..Default::default()
    }))
}

/// Headline wrapper so the text claims the column width without the
/// glare overlapping it visually. Inside `HeroRow`, this is the
/// LEFT column \u{2014} `flex_basis: 0` + `flex_grow: 1` so it
/// absorbs whatever width the device column doesn't claim;
/// `min_width: 0` so long subhead lines wrap inside the column
/// instead of forcing horizontal scroll.
stylesheet! {
    pub HeroText<IdeaThemeRef> {
        base(_t) {
            position: Position::Relative,
            gap: 16.0,
            flex_direction: FlexDirection::Column,
            flex_basis: 0.0,
            flex_grow: 1.0,
            min_width: 0.0,
            max_width: 720.0,
        }
    }
}

stylesheet! {
    pub HeroHeadline<IdeaThemeRef> {
        base(_t) {
            color: Tokenized::token("color-text", Color("#0a0c11".into())),
            font_weight: runtime_core::FontWeight::Bold,
        }
        variant size {
            #[default]
            wide(_t) {
                font_size: 56.0,
                letter_spacing: -1.4,
                line_height: 60.0,
            }
            narrow(_t) {
                font_size: 36.0,
                letter_spacing: -0.8,
                line_height: 42.0,
            }
        }
    }
}

stylesheet! {
    pub HeroSubhead<IdeaThemeRef> {
        base(_t) {
            color: Tokenized::token("color-text-muted", Color("#5b5446".into())),
            font_weight: runtime_core::FontWeight::Normal,
            max_width: 680.0,
        }
        variant size {
            #[default]
            wide(_t) {
                font_size: 21.0,
                line_height: 32.0,
            }
            narrow(_t) {
                font_size: 17.0,
                line_height: 26.0,
            }
        }
    }
}

/// Side-by-side row for the hero CTA buttons.
stylesheet! {
    pub HeroCtaRow<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            gap: 12.0,
            margin_top: 12.0,
            flex_wrap: runtime_core::FlexWrap::Wrap,
        }
    }
}

// =============================================================================
// Code panel — wraps an `idea-codeblock` so it sits on a neutral
// surface card with monospace font + comfortable padding.
// =============================================================================

stylesheet! {
    pub CodePanel<IdeaThemeRef> {
        base(_t) {
            // Theme-tokenized so the code panel adapts in dark mode.
            // `color-surface-alt` is idea-ui's secondary surface
            // token \u{2014} a touch off the page background so the
            // panel reads as a distinct surface in both themes.
            background: Tokenized::token("color-surface-alt", Color("#f4eedb".into())),
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e7e2d3".into())),
            border_radius: Tokenized::token("radius-lg", Length::Px(12.0)),
            padding: 20.0,
            overflow: Overflow::Hidden,
            // Without this, the flexbox default `min-width: auto` on
            // the panel equals its content's intrinsic min-content —
            // and the inner `<pre>`'s no-wrap text has min-content of
            // "the entire longest code line" (~650 px). The panel
            // then refuses to shrink below that, pushing its parent
            // column wider than the viewport on narrow screens.
            // Setting `min_width: 0` lets the panel shrink to fit
            // its parent; `overflow: Hidden` (above) clips the
            // overflowing code text. Paired with the responsive
            // `<pre>` wrap rule in `responsive.rs`'s CSS, the code
            // wraps at narrow viewports so nothing is actually lost.
            min_width: 0.0,
        }
        transitions {
            background: 250ms EaseInOut,
            border_color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub CodeText<IdeaThemeRef> {
        base(_t) {
            // Inherited by the code-block leaf so each colored span
            // renders monospace without per-span font wiring. The
            // base color is theme-tokenized (this is the color the
            // \"ink\" runs of the syntax highlighter will inherit
            // when they don't override). Individual span colors are
            // still stamped by the highlighter \u{2014} see
            // `common::highlight` for the palette.
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

// =============================================================================
// Differentiator grid — the cards under the hero on Home.
// =============================================================================

stylesheet! {
    pub PillarGrid<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            gap: 20.0,
            flex_wrap: runtime_core::FlexWrap::Wrap,
        }
    }
}

stylesheet! {
    pub PillarCard<IdeaThemeRef> {
        base(_t) {
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e7e2d3".into())),
            border_radius: Tokenized::token("radius-lg", Length::Px(12.0)),
            padding: 20.0,
            gap: 10.0,
            flex_direction: FlexDirection::Column,
            flex_basis: 0.0,
            flex_grow: 1.0,
            min_width: 240.0,
        }
        transitions {
            background: 250ms EaseInOut,
            border_color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub PillarCta<IdeaThemeRef> {
        base(_t) {
            // Footer "Read more \u{2192}" link inside each pillar card.
            // `margin_top: auto` (inside the card's flex-column) sticks
            // the CTA to the bottom regardless of blurb length, so
            // cards on the same row align their CTAs even when the
            // body text varies.
            color: Color("#7a6f5a".into()),
            font_size: 13.0,
            font_weight: runtime_core::FontWeight::SemiBold,
            margin_top: Length::Px(8.0),
        }
    }
}

// =============================================================================
// Footer — full-bleed band at the bottom of every screen, inside the
// ScrollView so it scrolls with content. Provides extra scroll
// height for the sticky TOC's spy logic, and a place for project
// links / copyright.
// =============================================================================

stylesheet! {
    pub Footer<IdeaThemeRef> {
        base(_t) {
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            border_top_width: 1.0,
            border_top_color: Tokenized::token("color-border", Color("#e7e2d3".into())),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            width: Length::pct(100.0),
        }
        variant size {
            #[default]
            wide(_t) {
                padding_horizontal: 64.0,
                padding_vertical: 56.0,
                gap: 40.0,
            }
            narrow(_t) {
                padding_horizontal: 24.0,
                padding_vertical: 40.0,
                gap: 32.0,
            }
        }
        transitions {
            background: 250ms EaseInOut,
            border_top_color: 250ms EaseInOut,
        }
    }
}

/// Inner column-grid: at wide, the brand block sits on the left
/// and the link columns flex along the right; at narrow they stack
/// vertically. `max_width` caps the grid so on ultrawides the
/// columns don't drift apart.
stylesheet! {
    pub FooterGrid<IdeaThemeRef> {
        base(_t) {
            width: Length::pct(100.0),
            max_width: 1120.0,
            align_self: runtime_core::AlignSelf::Center,
        }
        variant size {
            #[default]
            wide(_t) {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::FlexStart,
                gap: 64.0,
            }
            narrow(_t) {
                flex_direction: FlexDirection::Column,
                gap: 32.0,
            }
        }
    }
}

/// Brand block on the left of the footer grid. Slightly wider than
/// the link columns so the tagline can breathe.
stylesheet! {
    pub FooterBrand<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: 8.0,
            flex_basis: 0.0,
            flex_grow: 1.5,
            min_width: 0.0,
            max_width: 320.0,
        }
    }
}

/// One link column. Equal flex on the right side of the grid.
stylesheet! {
    pub FooterColumn<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: 10.0,
            flex_basis: 0.0,
            flex_grow: 1.0,
            min_width: 140.0,
        }
    }
}

/// Section header above each link column ("Project", "Resources").
stylesheet! {
    pub FooterTitle<IdeaThemeRef> {
        base(_t) {
            color: Tokenized::token("color-text-muted", Color("#8a8270".into())),
            font_size: 11.0,
            font_weight: runtime_core::FontWeight::SemiBold,
            letter_spacing: 0.8,
            text_transform: runtime_core::TextTransform::Uppercase,
            padding_bottom: 4.0,
        }
        transitions {
            color: 250ms EaseInOut,
        }
    }
}

/// Footer link text. Same hover-brighten pattern as `NavLink` so
/// the footer reads as "more nav" rather than "ad surface".
stylesheet! {
    pub FooterLink<IdeaThemeRef> {
        base(_t) {
            color: Tokenized::token("color-text-muted", Color("#6b7280".into())),
            font_size: 14.0,
            line_height: 20.0,
            text_align: TextAlign::Left,
        }
        state hovered(_t) {
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
        }
        transitions {
            color: 180ms EaseOut,
        }
    }
}

/// "Idealyst" wordmark in the brand block — slightly larger than
/// the tagline. Reuses the typography H3 size token so dark-mode
/// + theme-typography overrides apply consistently.
stylesheet! {
    pub FooterWordmark<IdeaThemeRef> {
        base(_t) {
            color: Tokenized::token("color-text", Color("#0a0c11".into())),
            font_size: Tokenized::token("typography-h3-size", Length::Px(22.0)),
            font_weight: runtime_core::FontWeight::SemiBold,
        }
        transitions {
            color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub FooterTagline<IdeaThemeRef> {
        base(_t) {
            color: Tokenized::token("color-text-muted", Color("#5b5446".into())),
            font_size: 14.0,
            line_height: 20.0,
        }
        transitions {
            color: 250ms EaseInOut,
        }
    }
}

/// Bottom strip with the copyright line. Sits below the grid,
/// separated by a thin top border that runs the full width of the
/// content max-width.
stylesheet! {
    pub FooterBottom<IdeaThemeRef> {
        base(_t) {
            width: Length::pct(100.0),
            max_width: 1120.0,
            align_self: runtime_core::AlignSelf::Center,
            padding_top: 24.0,
            border_top_width: 1.0,
            border_top_color: Tokenized::token("color-border", Color("#e7e2d3".into())),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
        }
        transitions {
            border_top_color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub FooterCopy<IdeaThemeRef> {
        base(_t) {
            color: Tokenized::token("color-text-muted", Color("#8a8270".into())),
            font_size: 12.0,
            text_align: TextAlign::Center,
        }
        transitions {
            color: 250ms EaseInOut,
        }
    }
}

/// Centered, padded wrapper for the home page sections under the hero.
stylesheet! {
    pub HomeSection<IdeaThemeRef> {
        base(_t) {
            gap: 28.0,
            flex_direction: FlexDirection::Column,
            max_width: 1120.0,
            align_self: runtime_core::AlignSelf::Center,
            width: Length::pct(100.0),
        }
        variant size {
            #[default]
            wide(_t) {
                padding_horizontal: 64.0,
                padding_vertical: 72.0,
            }
            narrow(_t) {
                padding_horizontal: 24.0,
                padding_vertical: 48.0,
            }
        }
    }
}

/// Vertical stack that hosts the iOS/Android tab strip and the
/// embedded Simulator preview. Stays narrow so its parent
/// `SimulatorRow` can sit it alongside an explanatory copy column
/// instead of stacking the preview above/below the text. Width is
/// `auto` (the canvas wrapper's own fixed 300 px provides the
/// inner dimension).
stylesheet! {
    pub SimulatorStage<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            gap: 24.0,
            padding_top: 8.0,
            flex_shrink: 0.0,
        }
    }
}

/// Horizontal row inside the Hero band: headline + CTA column on
/// the left, embedded Simulator on the right. `align_items: Center`
/// vertically centers the text column against the taller device,
/// so the headline lands at the device's mid-height; `gap: 72`
/// separates the two columns without competing with the hero's own
/// horizontal padding.
stylesheet! {
    pub HeroRow<IdeaThemeRef> {
        base(_t) {
            width: Length::pct(100.0),
            // Sits above the absolutely-positioned glare gradient
            // (which has `position: Absolute` + no z-index) so the
            // text and device both render in front of the wash.
            position: Position::Relative,
        }
        variant size {
            #[default]
            wide(_t) {
                // Side-by-side: headline + CTAs on the left, simulator
                // on the right.
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                gap: 72.0,
            }
            narrow(_t) {
                // Stacked: headline on top, simulator below. Keep the
                // device centered horizontally so it doesn't read as
                // mis-anchored against the left-aligned text column.
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                gap: 32.0,
            }
        }
    }
}

/// Bezel that wraps the embedded wgpu canvas. The simulator's
/// painter draws the *inner* bezel (the strip immediately around
/// the screen); this stylesheet adds the *outer* chassis around
/// THAT so the two read as one continuous device.
///
/// Chassis color is BLACK regardless of skin. The wgpu engine's
/// `device_frame_pipeline` paints opaque black on the canvas
/// outside the screen's rounded rect on every skin (see
/// `gpu-backend/engine/src/renderer.rs::device_frame`) \u{2014} the
/// painters' `BEZEL_TITANIUM` / `BEZEL_GRAPHITE` constants are
/// internal classification only and don't reach the canvas pixels.
/// Using titanium here would leave a visible color seam between the
/// chassis and the canvas's black outer band.
stylesheet! {
    pub SimulatorBezel<IdeaThemeRef> {
        base(_t) {
            background: Color("#000000".to_string()),
            border_radius: 44.0,
            padding: 12.0,
            // `overflow: Hidden` so the canvas + painter chrome
            // clip to the bezel's rounded corners. Without it the
            // painter's edge-to-edge fills (sun-glare gradient,
            // background washes) bleed past the chassis curve and
            // the device reads as a square canvas under a
            // rounded-frame overlay.
            overflow: Overflow::Hidden,
            shadow: Shadow {
                x: 0.0,
                y: 18.0,
                blur: 48.0,
                color: Color("rgba(15, 17, 30, 0.28)".to_string()),
            },
            flex_shrink: 0.0,
        }
    }
}

/// Stage for an animation demo: a fixed-size colored box that one
/// or more `AnimatedValue`s push transform / opacity / color
/// updates into. Centered inside a host View so its transform-origin
/// is the box's center (matters for scale animations).
stylesheet! {
    pub DemoStage<IdeaThemeRef> {
        base(_t) {
            width: 80.0,
            height: 80.0,
            border_radius: 16.0,
            background: Color("#5a4fcf".into()),
        }
    }
}

stylesheet! {
    pub DemoStageRow<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            gap: 24.0,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            padding_vertical: 24.0,
            min_height: 140.0,
            background: Tokenized::token("color-surface-alt", Color("#f0ead7".into())),
            border_radius: 12.0,
        }
    }
}

stylesheet! {
    pub PlaceholderBox<IdeaThemeRef> {
        base(_t) {
            background: Tokenized::token("color-surface-alt", Color("#f0ead7".into())),
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e7e2d3".into())),
            border_radius: 12.0,
            padding: 24.0,
            gap: 12.0,
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::FlexStart,
            justify_content: JustifyContent::Center,
            min_height: 120.0,
        }
    }
}
