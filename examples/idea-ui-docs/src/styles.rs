//! Stylesheets for the idea-ui docs chrome — sidebar, reading column,
//! per-page demo surfaces, props table.
//!
//! Token names match idea-ui's theme so the installed light/dark
//! palette drives this chrome; the fallbacks keep it legible if a
//! token name drifts.

use runtime_core::stylesheet;
use runtime_core::{
    AlignItems, Color, FlexDirection, FontWeight, JustifyContent, Length, Overflow, TextAlign,
    TextTransform, Tokenized,
};

// ---- Page-level scroll surface --------------------------------------------

stylesheet! {
    pub ScreenScroll<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            width: Length::pct(100.0),
            // Fill the remaining height under the (conditional) top bar
            // so the page body scrolls inside its own region.
            flex_grow: 1.0,
            flex_shrink: 1.0,
            flex_basis: 0.0,
            background: Tokenized::token("color-background", Color("#f7f8fb".into())),
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
        }
        transitions {
            background: 250ms EaseInOut,
            color: 250ms EaseInOut,
        }
    }
}

// Outer column for each screen: the (conditional) hamburger top bar
// stacked over the scrolling page body. Fills the navigator's body
// outlet so the scroll view can take the leftover height.
stylesheet! {
    pub PageColumn<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            width: Length::pct(100.0),
            height: Length::pct(100.0),
            background: Tokenized::token("color-background", Color("#f7f8fb".into())),
        }
        transitions {
            background: 250ms EaseInOut,
        }
    }
}

// Centered, padded reading column for each page.
stylesheet! {
    pub PagePad<()> {
        // Mobile-first: 16pt padding + 20pt gap on phones so the
        // reading column doesn't waste half the screen on chrome.
        // Each successive breakpoint relaxes back toward the
        // desktop-comfortable 48pt / 28pt. Defaults map to Tailwind-
        // style buckets (sm ≥ 640dp, md ≥ 768dp) — see
        // `runtime_core::breakpoint::Breakpoints::DEFAULT`.
        base(_t) {
            flex_direction: FlexDirection::Column,
            max_width: 880.0,
            align_self: runtime_core::AlignSelf::Center,
            width: Length::pct(100.0),
            padding: 16.0,
            gap: 20.0,
        }
        breakpoint sm(_t) {
            padding: 24.0,
            gap: 24.0,
        }
        breakpoint md(_t) {
            padding: 48.0,
            gap: 28.0,
        }
    }
}

// ---- Mobile top bar + hamburger -------------------------------------------

// Persistent top strip that hosts the hamburger on narrow viewports.
// Only rendered when the drawer is collapsed (below the pin width), so
// it never shows on wide layouts where the sidebar is pinned.
stylesheet! {
    pub TopBar<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            padding_horizontal: Tokenized::token("spacing-md", Length::Px(12.0)),
            padding_vertical: Tokenized::token("spacing-sm", Length::Px(8.0)),
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            border_bottom_width: 1.0,
            border_bottom_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
        }
        transitions {
            background: 250ms EaseInOut,
            border_bottom_color: 250ms EaseInOut,
        }
    }
}

// The hamburger itself — a square pressable that tints the menu glyph
// with the theme's text color. Used in the custom header's leading edge
// (web only) when the sidebar collapses below the pin width; a soft
// hover fill gives it the same affordance as the sidebar nav items.
stylesheet! {
    pub MenuButton<()> {
        base(_t) {
            width: Length::Px(34.0),
            height: Length::Px(34.0),
            border_radius: Tokenized::token("radius-md", Length::Px(8.0)),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            // Pull the icon toward the header's leading edge so it lines
            // up with the sidebar's own left inset.
            margin_left: -6.0,
            background: Color("transparent".into()),
        }
        state hovered(_t) {
            background: Tokenized::token("color-surface-alt", Color("#f1f5f9".into())),
        }
        transitions { background: 150ms EaseOut, }
    }
}

// The ☰ glyph inside `MenuButton`, tinted with the theme's text color.
stylesheet! {
    pub MenuGlyph<()> {
        base(_t) {
            font_size: 19.0,
            text_align: TextAlign::Center,
            color: Tokenized::token("color-text", Color("#0f172a".into())),
        }
        transitions { color: 250ms EaseInOut, }
    }
}

// ---- Sidebar ---------------------------------------------------------------

stylesheet! {
    // The scrolling panel: background + right border span the WHOLE panel
    // (so they stay put while content scrolls). Padding lives on the inner
    // `SidebarBody`, NOT here — a scroll view's own `padding` doesn't reliably
    // inset its content (its documentView isn't Taffy-positioned on macOS), so
    // a `width: 100%` child resolves against the full panel width and the
    // search field reads as too wide. Keep padding on a plain inner view.
    pub SidebarScroll<()> {
        base(_t) {
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            border_right_width: 1.0,
            border_right_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            flex_direction: FlexDirection::Column,
            width: Length::pct(100.0),
        }
        transitions {
            background: 250ms EaseInOut,
            border_right_color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    // Inner content column: a PLAIN view (not the scroll view) so its padding
    // correctly insets the children — the search field + nav then sit inside
    // the panel padding on every side.
    pub SidebarBody<()> {
        base(_t) {
            padding: Tokenized::token("spacing-lg", Length::Px(16.0)),
            gap: Tokenized::token("spacing-xs", Length::Px(4.0)),
            flex_direction: FlexDirection::Column,
            width: Length::pct(100.0),
        }
    }
}

stylesheet! {
    pub SidebarHeader<()> {
        base(_t) {
            padding_bottom: Tokenized::token("spacing-md", Length::Px(12.0)),
            border_bottom_width: 1.0,
            border_bottom_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
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
            border_top_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            flex_direction: FlexDirection::Column,
            gap: Tokenized::token("spacing-sm", Length::Px(8.0)),
        }
    }
}

// NavLink is split into two stylesheets — container (padding +
// background + border-radius) and text (color + font + alignment) —
// because on Android `apply_style` does NOT propagate padding to
// `View.setPadding`; padding only takes effect via Taffy shifting
// *children* inside the parent's content box. A text node has no
// children, so padding on it would silently no-op on native. By
// wrapping the text in a view with container styles, Taffy shifts the
// text child by the padding and the visual matches web (where CSS
// padding on the underlying `<a>` is just text-padded).
//
// Both stylesheets share the same `active` variant axis so the SDK
// can flip both with a single signal read at the call site.
stylesheet! {
    pub NavLink<()> {
        base(_t) {
            padding_vertical: Tokenized::token("spacing-sm", Length::Px(8.0)),
            padding_horizontal: Tokenized::token("spacing-md", Length::Px(12.0)),
            border_radius: Tokenized::token("radius-md", Length::Px(8.0)),
            background: Color("transparent".into()),
            flex_direction: FlexDirection::Column,
        }
        variant active {
            #[default]
            off(_t) {}
            on(_t) {
                background: Tokenized::token(
                    "intent-primary-soft-bg",
                    Color("rgba(91, 108, 255, 0.12)".into()),
                ),
            }
        }
        transitions {
            background: 180ms EaseOut,
        }
    }
}

stylesheet! {
    pub NavLinkText<()> {
        base(_t) {
            color: Tokenized::token("color-text-muted", Color("#6b7280".into())),
            font_size: 14.0,
            text_align: TextAlign::Left,
        }
        variant active {
            #[default]
            off(_t) {}
            on(_t) {
                color: Tokenized::token("intent-primary-fg", Color("#3947d6".into())),
            }
        }
        state hovered(_t) {
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
        }
        transitions {
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
            border_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            border_radius: Tokenized::token("radius-lg", Length::Px(12.0)),
            // Padding lives INSIDE the codeblock (on its inner column,
            // inside the horizontal scroll view) so it scrolls with
            // content — keeping it here would clip the rightmost
            // content behind the right padding when scrolled.
            overflow: Overflow::Hidden,
            min_width: 0.0,
        }
    }
}

stylesheet! {
    pub CodeText<()> {
        base(_t) {
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace",
            font_size: 13.0,
            line_height: 20.0,
            color: Tokenized::token("color-text", Color("#1f2328".into())),
        }
        transitions {
            color: 250ms EaseInOut,
        }
    }
}

// ---- Demo surface — boxed live-preview area on a component page ----------

stylesheet! {
    pub DemoSurface<()> {
        base(_t) {
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            border_radius: Tokenized::token("radius-lg", Length::Px(12.0)),
            padding: Tokenized::token("spacing-xl", Length::Px(24.0)),
            gap: Tokenized::token("spacing-lg", Length::Px(16.0)),
            flex_direction: FlexDirection::Column,
            min_height: 120.0,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
        }
        transitions {
            background: 250ms EaseInOut,
            border_color: 250ms EaseInOut,
        }
    }
}

// Side-by-side wrapping row: preview on the left, controls on the right.
stylesheet! {
    pub DemoRow<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            gap: Tokenized::token("spacing-xl", Length::Px(24.0)),
            align_items: AlignItems::FlexStart,
            flex_wrap: runtime_core::FlexWrap::Wrap,
        }
    }
}

stylesheet! {
    pub PreviewBox<()> {
        base(_t) {
            // Same surface as `DemoSurface` but designed to share a row
            // with the controls panel.
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            border_radius: Tokenized::token("radius-lg", Length::Px(12.0)),
            padding: Tokenized::token("spacing-xl", Length::Px(24.0)),
            gap: Tokenized::token("spacing-md", Length::Px(12.0)),
            flex_direction: FlexDirection::Column,
            flex_basis: 0.0,
            flex_grow: 2.0,
            flex_shrink: 1.0,
            min_width: 280.0,
            min_height: 160.0,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
        }
        transitions {
            background: 250ms EaseInOut,
            border_color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub ControlsBox<()> {
        base(_t) {
            flex_basis: 0.0,
            flex_grow: 1.0,
            min_width: 240.0,
            max_width: 360.0,
            gap: Tokenized::token("spacing-sm", Length::Px(8.0)),
            flex_direction: FlexDirection::Column,
        }
    }
}

// Inner wrapper that gives previews a known `width: 100%` container,
// so full-width children (Divider, Alert) resolve `100%` against the
// preview's real width rather than a `display: block` collapsed
// placeholder. See the historical layout-fix commit message.
stylesheet! {
    pub PreviewSlot<()> {
        base(_t) {
            width: Length::pct(100.0),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            gap: Tokenized::token("spacing-md", Length::Px(12.0)),
        }
    }
}

// (Props-table styling now lives in idea-ui's themed `Table` /
// `TableCell` components — this file no longer needs PropsTable /
// PropCell* sheets.)

// ---- Callout (tips / notes / cross-links) ---------------------------------

stylesheet! {
    pub Callout<()> {
        base(_t) {
            background: Tokenized::token(
                "intent-primary-soft-bg",
                Color("#eef2ff".into()),
            ),
            border_left_width: 3.0,
            border_left_color: Tokenized::token("intent-primary-fg", Color("#4f46e5".into())),
            border_radius: Tokenized::token("radius-md", Length::Px(8.0)),
            padding: 16.0,
            gap: 6.0,
            flex_direction: FlexDirection::Column,
        }
    }
}

// ===========================================================================
// Design chrome — header bar, segmented theme toggle, sidebar search,
// status dots, group overline, status badge, Usage label. These mirror
// the idea-ui reference design.
// ===========================================================================

// ---- Header bar (top slot) -------------------------------------------------

stylesheet! {
    pub DocHeader<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            height: Length::Px(58.0),
            padding_horizontal: 22.0,
            gap: 16.0,
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            border_bottom_width: 1.0,
            border_bottom_color: Tokenized::token("color-border", Color("#e3e8ef".into())),
        }
        transitions {
            background: 250ms EaseInOut,
            border_bottom_color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub HeaderBrand<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 11.0,
        }
    }
}

stylesheet! {
    pub LogoBox<()> {
        base(_t) {
            width: Length::Px(27.0),
            height: Length::Px(27.0),
            border_radius: 8.0,
            background: Tokenized::token("intent-primary-solid-bg", Color("#4f46e5".into())),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
        }
    }
}

stylesheet! {
    pub LogoGlyph<()> {
        base(_t) {
            color: Tokenized::token("intent-primary-solid-text", Color("#ffffff".into())),
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace",
            font_size: 14.0,
            font_weight: FontWeight::Bold,
        }
    }
}

stylesheet! {
    pub BrandName<()> {
        base(_t) {
            color: Tokenized::token("color-text", Color("#0f172a".into())),
            font_size: 15.0,
            font_weight: FontWeight::Bold,
        }
        transitions { color: 250ms EaseInOut, }
    }
}

stylesheet! {
    pub VersionPill<()> {
        base(_t) {
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace",
            font_size: 11.0,
            font_weight: FontWeight::Medium,
            color: Tokenized::token("color-text-muted", Color("#64748b".into())),
            background: Tokenized::token("color-surface-alt", Color("#f1f5f9".into())),
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e3e8ef".into())),
            padding_horizontal: 8.0,
            padding_vertical: 2.0,
            border_radius: 999.0,
        }
    }
}

stylesheet! {
    pub HeaderMono<()> {
        base(_t) {
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace",
            font_size: 12.0,
            font_weight: FontWeight::Medium,
            color: Tokenized::token("color-text-muted", Color("#64748b".into())),
        }
    }
}

// A flex-grow spacer that pushes the trailing header content right.
stylesheet! {
    pub HeaderSpacer<()> {
        base(_t) {
            flex_grow: 1.0,
            flex_shrink: 1.0,
        }
    }
}

// ---- Segmented Light/Dark toggle ------------------------------------------

stylesheet! {
    pub SegToggle<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            background: Tokenized::token("color-surface-alt", Color("#f1f5f9".into())),
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e3e8ef".into())),
            border_radius: 999.0,
            padding: 3.0,
            gap: 2.0,
        }
        transitions { background: 250ms EaseInOut, }
    }
}

stylesheet! {
    pub SegBtn<()> {
        base(_t) {
            padding_horizontal: 14.0,
            padding_vertical: 5.0,
            border_radius: 999.0,
            background: Color("transparent".into()),
        }
        variant active {
            #[default]
            off(_t) {}
            on(_t) {
                background: Tokenized::token("color-surface", Color("#ffffff".into())),
            }
        }
        transitions { background: 150ms EaseOut, }
    }
}

stylesheet! {
    pub SegBtnText<()> {
        base(_t) {
            font_size: 13.0,
            text_align: TextAlign::Center,
            color: Tokenized::token("color-text-muted", Color("#64748b".into())),
        }
        variant active {
            #[default]
            off(_t) {}
            on(_t) {
                color: Tokenized::token("color-text", Color("#0f172a".into())),
                font_weight: FontWeight::SemiBold,
            }
        }
        transitions { color: 150ms EaseOut, }
    }
}

// ---- Sidebar search --------------------------------------------------------

stylesheet! {
    pub SearchInput<()> {
        base(_t) {
            width: Length::pct(100.0),
            padding_horizontal: 12.0,
            padding_vertical: 8.0,
            font_size: 13.0,
            background: Tokenized::token("color-surface-alt", Color("#f1f5f9".into())),
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e3e8ef".into())),
            border_radius: 8.0,
            color: Tokenized::token("color-text", Color("#0f172a".into())),
            margin_bottom: 12.0,
        }
        // Focus ring: recolor the existing 1px border to the theme accent —
        // thin + themed, not a heavy native ring. Web gets this from CSS
        // `:focus`; macOS drives it off StateBits::FOCUSED (NSTextField
        // begin/end editing). Same observable result on both (§7).
        state focused(_t) {
            border_color: Tokenized::token("color-focus-ring", Color("#5b6cff".into())),
        }
        transitions { background: 250ms EaseInOut, border_color: 150ms EaseOut, }
    }
}

// ---- Sidebar nav item (row: label + status dot) ---------------------------

stylesheet! {
    pub NavItem<()> {
        base(_t) {
            // Span the sidebar width so `space-between` pushes the status dot
            // to the right edge; vertically center the label + dot.
            width: Length::pct(100.0),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            padding_vertical: 7.0,
            padding_horizontal: 11.0,
            border_radius: 8.0,
            background: Color("transparent".into()),
        }
        variant active {
            #[default]
            off(_t) {}
            on(_t) {
                background: Tokenized::token("intent-primary-soft-bg", Color("#eef2ff".into())),
            }
        }
        transitions { background: 150ms EaseOut, }
    }
}

stylesheet! {
    pub NavDot<()> {
        base(_t) {
            width: Length::Px(6.0),
            height: Length::Px(6.0),
            border_radius: 999.0,
            background: Tokenized::token("color-border-strong", Color("#94a3b8".into())),
        }
        // "ready"/Detailed entries get a green dot; Preview keep the base grey.
        variant ready {
            #[default]
            off(_t) {}
            on(_t) {
                background: Tokenized::token("intent-success-fg", Color("#16a34a".into())),
            }
        }
    }
}

// ---- Page frame: group overline, status badge, Usage label ----------------

stylesheet! {
    pub GroupOverline<()> {
        base(_t) {
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace",
            font_size: 11.0,
            font_weight: FontWeight::SemiBold,
            letter_spacing: 1.3,
            text_transform: TextTransform::Uppercase,
            color: Tokenized::token("intent-primary-fg", Color("#4f46e5".into())),
        }
    }
}

stylesheet! {
    pub TitleRow<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 14.0,
            flex_wrap: runtime_core::FlexWrap::Wrap,
        }
    }
}

stylesheet! {
    pub StatusBadge<()> {
        base(_t) {
            padding_horizontal: 10.0,
            padding_vertical: 4.0,
            border_radius: 999.0,
            border_width: 1.0,
            background: Tokenized::token("color-surface-alt", Color("#f1f5f9".into())),
            border_color: Tokenized::token("color-border", Color("#e3e8ef".into())),
        }
        variant detailed {
            #[default]
            off(_t) {}
            on(_t) {
                background: Tokenized::token("intent-success-soft-bg", Color("#f0fdf4".into())),
                border_color: Tokenized::token("intent-success-border", Color("#bbf7d0".into())),
            }
        }
    }
}

stylesheet! {
    pub StatusBadgeText<()> {
        base(_t) {
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace",
            font_size: 11.0,
            font_weight: FontWeight::SemiBold,
            color: Tokenized::token("color-text-muted", Color("#64748b".into())),
        }
        variant detailed {
            #[default]
            off(_t) {}
            on(_t) {
                color: Tokenized::token("intent-success-soft-text", Color("#15803d".into())),
            }
        }
    }
}

// A definite-width column for demos whose children use *percentage* widths
// (e.g. Skeleton's Full/Half/ThreeQuarter). DemoSurface centers + shrink-wraps
// its content, so a bare %-width child resolves against a zero-width column and
// renders nothing; this frame gives it a real width to resolve against.
stylesheet! {
    pub PercentWidthFrame<()> {
        base(_t) {
            width: Length::pct(100.0),
            max_width: 380.0,
            flex_direction: FlexDirection::Column,
        }
    }
}

// ---- Foundations visuals: color swatch + radius box ----------------------

// A color swatch block. The actual fill is applied per-token via
// `override_background(Tokenized::token(...))` so it re-tints on theme swap.
stylesheet! {
    pub SwatchBlock<()> {
        base(_t) {
            height: Length::Px(64.0),
            width: Length::pct(100.0),
            border_radius: Tokenized::token("radius-md", Length::Px(8.0)),
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e3e8ef".into())),
        }
    }
}

// One box per radius token, picked by the `r` variant.
stylesheet! {
    pub RadiusBox<()> {
        base(_t) {
            width: Length::Px(76.0),
            height: Length::Px(76.0),
            background: Tokenized::token("intent-primary-soft-bg", Color("#eef2ff".into())),
            border_width: 1.5,
            border_color: Tokenized::token("intent-primary-fg", Color("#4f46e5".into())),
        }
        variant r {
            #[default]
            sm(_t) { border_radius: Tokenized::token("radius-sm", Length::Px(4.0)), }
            md(_t) { border_radius: Tokenized::token("radius-md", Length::Px(8.0)), }
            lg(_t) { border_radius: Tokenized::token("radius-lg", Length::Px(12.0)), }
            pill(_t) { border_radius: Tokenized::token("radius-pill", Length::Px(999.0)), }
        }
    }
}

// Two small blocks used to demonstrate a Stack gap visually.
stylesheet! {
    pub GapBlock<()> {
        base(_t) {
            width: Length::Px(40.0),
            height: Length::Px(28.0),
            border_radius: Tokenized::token("radius-sm", Length::Px(4.0)),
            background: Tokenized::token("intent-primary-soft-bg", Color("#eef2ff".into())),
            border_width: 1.0,
            border_color: Tokenized::token("intent-primary-border", Color("#c7d2fe".into())),
        }
    }
}

stylesheet! {
    pub UsageLabel<()> {
        base(_t) {
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace",
            font_size: 11.0,
            font_weight: FontWeight::SemiBold,
            letter_spacing: 0.9,
            text_transform: TextTransform::Uppercase,
            color: Tokenized::token("color-text-muted", Color("#64748b".into())),
            margin_bottom: 11.0,
        }
    }
}

// ===========================================================================
// Overview / landing page — the design's `D.home` screen: hero card, stat
// cards, principle features, the dark token-resolution strip, and the
// catalog grid. Rendered full-bleed (no page frame) via
// `shell::landing_frame`, so these sheets own all the page's chrome.
// ===========================================================================

const MONO: &str = "ui-monospace, SFMono-Regular, Menlo, monospace";

// Wide centered reading column — the landing uses a roomier 1000px max
// (vs. the component pages' 880px) so the hero + two-up grids breathe.
stylesheet! {
    pub LandingPad<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            max_width: 1000.0,
            align_self: runtime_core::AlignSelf::Center,
            width: Length::pct(100.0),
            padding: 16.0,
            gap: 16.0,
        }
        breakpoint sm(_t) { padding: 28.0, }
        breakpoint md(_t) { padding: 40.0, }
    }
}

// ---- Hero --------------------------------------------------------------

stylesheet! {
    pub HeroCard<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::FlexStart,
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e3e8ef".into())),
            border_radius: 20.0,
            padding: 32.0,
            gap: 18.0,
        }
        breakpoint md(_t) {
            padding_top: 52.0,
            padding_bottom: 46.0,
            padding_horizontal: 48.0,
        }
        transitions { background: 250ms EaseInOut, border_color: 250ms EaseInOut, }
    }
}

stylesheet! {
    pub HeroBadge<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 8.0,
            padding_horizontal: 13.0,
            padding_vertical: 5.0,
            border_radius: 999.0,
            background: Tokenized::token("color-surface-alt", Color("#f1f5f9".into())),
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e3e8ef".into())),
        }
        transitions { background: 250ms EaseInOut, }
    }
}

stylesheet! {
    pub HeroDot<()> {
        base(_t) {
            width: Length::Px(7.0),
            height: Length::Px(7.0),
            border_radius: 999.0,
            background: Tokenized::token("intent-success-solid-bg", Color("#15803d".into())),
        }
    }
}

stylesheet! {
    pub HeroBadgeText<()> {
        base(_t) {
            font_family: MONO,
            font_size: 11.5,
            font_weight: FontWeight::SemiBold,
            letter_spacing: 0.4,
            color: Tokenized::token("color-text-muted", Color("#64748b".into())),
        }
    }
}

stylesheet! {
    pub HeroCtaRow<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: runtime_core::FlexWrap::Wrap,
            align_items: AlignItems::Center,
            gap: 12.0,
            margin_top: 10.0,
        }
    }
}

// ---- CTA buttons (link-wrapped styled views, not interactive Buttons —
// the whole CTA is a navigation `link`, so its content is a plain view) --

stylesheet! {
    pub CtaPrimary<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            gap: 9.0,
            padding_horizontal: 22.0,
            padding_vertical: 11.0,
            border_radius: Tokenized::token("radius-md", Length::Px(8.0)),
            background: Tokenized::token("intent-primary-solid-bg", Color("#4f46e5".into())),
        }
        transitions { background: 150ms EaseOut, }
    }
}

stylesheet! {
    pub CtaPrimaryText<()> {
        base(_t) {
            font_size: 16.0,
            font_weight: FontWeight::SemiBold,
            text_align: TextAlign::Center,
            color: Tokenized::token("intent-primary-solid-text", Color("#ffffff".into())),
        }
    }
}

stylesheet! {
    pub CtaOutline<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            gap: 9.0,
            padding_horizontal: 22.0,
            padding_vertical: 11.0,
            border_radius: Tokenized::token("radius-md", Length::Px(8.0)),
            border_width: 1.0,
            border_color: Tokenized::token("intent-neutral-border", Color("#e2e8f0".into())),
            background: Color("transparent".into()),
        }
        state hovered(_t) {
            border_color: Tokenized::token("color-border-strong", Color("#94a3b8".into())),
        }
        transitions { border_color: 150ms EaseOut, }
    }
}

stylesheet! {
    pub CtaOutlineText<()> {
        base(_t) {
            font_size: 16.0,
            font_weight: FontWeight::SemiBold,
            text_align: TextAlign::Center,
            color: Tokenized::token("intent-neutral-fg", Color("#475569".into())),
        }
    }
}

// ---- Stat cards --------------------------------------------------------

stylesheet! {
    pub StatCard<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: 4.0,
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e3e8ef".into())),
            border_radius: 14.0,
            padding_horizontal: 22.0,
            padding_vertical: 20.0,
        }
        transitions { background: 250ms EaseInOut, border_color: 250ms EaseInOut, }
    }
}

// The big stat number, tinted per-intent. Tone is the variant axis so the
// color re-binds on theme swap (vs. a baked-in hex).
stylesheet! {
    pub StatNumber<()> {
        base(_t) {
            font_size: 34.0,
            font_weight: FontWeight::Bold,
            letter_spacing: -0.6,
        }
        variant tone {
            #[default]
            primary(_t) { color: Tokenized::token("intent-primary-fg", Color("#4f46e5".into())), }
            success(_t) { color: Tokenized::token("intent-success-fg", Color("#16a34a".into())), }
            info(_t) { color: Tokenized::token("intent-info-fg", Color("#0891b2".into())), }
            warning(_t) { color: Tokenized::token("intent-warning-fg", Color("#b45309".into())), }
        }
    }
}

stylesheet! {
    pub StatLabel<()> {
        base(_t) {
            font_size: 13.5,
            font_weight: FontWeight::Medium,
            color: Tokenized::token("color-text-muted", Color("#64748b".into())),
        }
    }
}

// ---- Section label (the uppercase rules between landing sections) ------

stylesheet! {
    pub SectionLabel<()> {
        base(_t) {
            font_family: MONO,
            font_size: 11.0,
            font_weight: FontWeight::SemiBold,
            letter_spacing: 1.3,
            text_transform: TextTransform::Uppercase,
            color: Tokenized::token("color-text-muted", Color("#64748b".into())),
            margin_top: 26.0,
        }
    }
}

// ---- Principle feature cards -------------------------------------------

stylesheet! {
    pub FeatureCard<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::FlexStart,
            gap: 14.0,
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e3e8ef".into())),
            border_radius: 14.0,
            padding_horizontal: 22.0,
            padding_vertical: 20.0,
        }
        transitions { background: 250ms EaseInOut, border_color: 250ms EaseInOut, }
    }
}

// The 38px tinted glyph chip. Soft-bg + border per intent; the Icon inside
// carries the matching `fg` tint via its own `tone` prop.
stylesheet! {
    pub FeatureIconBox<()> {
        base(_t) {
            width: Length::Px(38.0),
            height: Length::Px(38.0),
            border_radius: 10.0,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            border_width: 1.0,
        }
        variant tone {
            #[default]
            primary(_t) {
                background: Tokenized::token("intent-primary-soft-bg", Color("#eef2ff".into())),
                border_color: Tokenized::token("intent-primary-border", Color("#c7d2fe".into())),
            }
            info(_t) {
                background: Tokenized::token("intent-info-soft-bg", Color("#ecfeff".into())),
                border_color: Tokenized::token("intent-info-border", Color("#a5f3fc".into())),
            }
            success(_t) {
                background: Tokenized::token("intent-success-soft-bg", Color("#f0fdf4".into())),
                border_color: Tokenized::token("intent-success-border", Color("#bbf7d0".into())),
            }
            warning(_t) {
                background: Tokenized::token("intent-warning-soft-bg", Color("#fffbeb".into())),
                border_color: Tokenized::token("intent-warning-border", Color("#fde68a".into())),
            }
        }
    }
}

stylesheet! {
    pub FeatureTextCol<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: 5.0,
            flex_grow: 1.0,
            flex_shrink: 1.0,
            min_width: 0.0,
        }
    }
}

stylesheet! {
    pub FeatureTitle<()> {
        base(_t) {
            font_size: 15.0,
            font_weight: FontWeight::SemiBold,
            color: Tokenized::token("color-text", Color("#0f172a".into())),
            text_align: TextAlign::Left,
        }
    }
}

stylesheet! {
    pub FeatureBody<()> {
        base(_t) {
            font_size: 13.5,
            line_height: 21.0,
            color: Tokenized::token("color-text-muted", Color("#64748b".into())),
            text_align: TextAlign::Left,
        }
    }
}

// ---- Token-resolution strip (dark code-bg card) ------------------------

stylesheet! {
    pub TokenStrip<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: runtime_core::FlexWrap::Wrap,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            gap: 30.0,
            background: Tokenized::token("code-bg", Color("#0f172a".into())),
            border_radius: 16.0,
            padding_horizontal: 28.0,
            padding_vertical: 26.0,
        }
        transitions { background: 250ms EaseInOut, }
    }
}

stylesheet! {
    pub TokenStripCol<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: 8.0,
        }
    }
}

stylesheet! {
    pub TokenStripLabel<()> {
        base(_t) {
            font_family: MONO,
            font_size: 11.0,
            font_weight: FontWeight::SemiBold,
            letter_spacing: 1.1,
            text_transform: TextTransform::Uppercase,
            // Fixed slate against the always-dark strip background.
            color: Color("#94a3b8".into()),
        }
    }
}

stylesheet! {
    pub TokenStripCode<()> {
        base(_t) {
            font_family: MONO,
            font_size: 13.5,
            line_height: 23.0,
            color: Tokenized::token("code-fg", Color("#cbd5e1".into())),
        }
    }
}

stylesheet! {
    pub TokenStripCodeAccent<()> {
        base(_t) {
            font_family: MONO,
            font_size: 13.5,
            line_height: 23.0,
            color: Color("#34d399".into()),
        }
    }
}

// ---- Catalog grid (one card per component group) -----------------------

stylesheet! {
    pub CatCard<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: 11.0,
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e3e8ef".into())),
            border_radius: 14.0,
            padding_horizontal: 20.0,
            padding_vertical: 18.0,
        }
        state hovered(_t) {
            border_color: Tokenized::token("intent-primary-fg", Color("#4f46e5".into())),
        }
        transitions { background: 250ms EaseInOut, border_color: 150ms EaseOut, }
    }
}

stylesheet! {
    pub CatHead<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
        }
    }
}

stylesheet! {
    pub CatGroupLabel<()> {
        base(_t) {
            font_family: MONO,
            font_size: 11.0,
            font_weight: FontWeight::SemiBold,
            letter_spacing: 1.1,
            text_transform: TextTransform::Uppercase,
            color: Tokenized::token("color-text-muted", Color("#64748b".into())),
            text_align: TextAlign::Left,
        }
    }
}

stylesheet! {
    pub CatCount<()> {
        base(_t) {
            font_family: MONO,
            font_size: 12.0,
            font_weight: FontWeight::SemiBold,
            color: Tokenized::token("intent-primary-fg", Color("#4f46e5".into())),
        }
    }
}

stylesheet! {
    pub CatChips<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: runtime_core::FlexWrap::Wrap,
            gap: 6.0,
        }
    }
}

stylesheet! {
    pub CatChip<()> {
        base(_t) {
            font_size: 12.0,
            font_weight: FontWeight::Medium,
            color: Tokenized::token("color-text", Color("#0f172a".into())),
            background: Tokenized::token("color-surface-alt", Color("#f1f5f9".into())),
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e3e8ef".into())),
            border_radius: 6.0,
            padding_horizontal: 8.0,
            padding_vertical: 3.0,
        }
    }
}
