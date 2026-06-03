//! Stylesheets for the idea-ui docs chrome — sidebar, reading column,
//! per-page demo surfaces, props table.
//!
//! Token names match idea-ui's theme so the installed light/dark
//! palette drives this chrome; the fallbacks keep it legible if a
//! token name drifts.

use runtime_core::stylesheet;
use runtime_core::{
    AlignItems, Color, FlexDirection, JustifyContent, Length, Overflow, TextAlign, Tokenized,
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
// with the theme's text color.
stylesheet! {
    pub MenuButton<()> {
        base(_t) {
            width: Length::Px(40.0),
            height: Length::Px(40.0),
            border_radius: Tokenized::token("radius-md", Length::Px(8.0)),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
        }
    }
}

// ---- Sidebar ---------------------------------------------------------------

stylesheet! {
    pub SidebarBody<()> {
        base(_t) {
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            border_right_width: 1.0,
            border_right_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            padding: Tokenized::token("spacing-lg", Length::Px(16.0)),
            gap: Tokenized::token("spacing-xs", Length::Px(4.0)),
            flex_direction: FlexDirection::Column,
            // NB: do NOT set `min_height: Percent(100)` here. Taffy
            // clamps SidebarBody to the scroll view's height when
            // min_height: 100% is set, so the bottom of the sidebar
            // (overflow children — the dark-mode toggle, the last
            // few nav links) renders outside SidebarBody's frame.
            // They're still visible (scroll content extends past
            // SidebarBody), but iOS UIView hit-testing won't descend
            // into children outside the parent's frame — taps on the
            // bottom half of the sidebar fall through to nothing.
            // The SDK's scroll_view wrapper carries the same
            // `color-surface` background so removing the min_height
            // here doesn't leave a transparent gap.
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
                Color("rgba(91, 108, 255, 0.10)".into()),
            ),
            border_left_width: 3.0,
            border_left_color: Tokenized::token("intent-primary-fg", Color("#3947d6".into())),
            border_radius: Tokenized::token("radius-md", Length::Px(8.0)),
            padding: 16.0,
            gap: 6.0,
            flex_direction: FlexDirection::Column,
        }
    }
}

// ---- Search trigger (looks like a text input) -----------------------------
// A bordered, rounded, padded row with muted placeholder text — visually an
// input, but it's a `pressable` that opens the search modal.
stylesheet! {
    pub SearchBox<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 8.0,
            padding_vertical: Tokenized::token("spacing-sm", Length::Px(8.0)),
            padding_horizontal: Tokenized::token("spacing-md", Length::Px(12.0)),
            border_radius: Tokenized::token("radius-md", Length::Px(8.0)),
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            background: Tokenized::token("color-background", Color("#f7f8fb".into())),
        }
        state hovered(_t) {
            border_color: Tokenized::token("intent-primary-fg", Color("#3947d6".into())),
        }
        transitions {
            border_color: 160ms EaseOut,
        }
    }
}

stylesheet! {
    pub SearchBoxText<()> {
        base(_t) {
            color: Tokenized::token("color-text-muted", Color("#6b7280".into())),
            font_size: 14.0,
            text_align: TextAlign::Left,
        }
    }
}

// ---- Reference link text (blue, hyperlink-like) ---------------------------
// Applied to in-content links that reference other catalog entries
// (composes graph, scope members, prop type links, search results).
stylesheet! {
    pub LinkText<()> {
        base(_t) {
            color: Tokenized::token("intent-primary-fg", Color("#3947d6".into())),
            font_size: 14.0,
            text_align: TextAlign::Left,
        }
        state hovered(_t) {
            color: Tokenized::token("intent-primary-strong-fg", Color("#2730a8".into())),
        }
        transitions {
            color: 150ms EaseOut,
        }
    }
}
