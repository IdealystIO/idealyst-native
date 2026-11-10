//! Stylesheets for the idea-ui docs chrome — sidebar, reading column,
//! per-page demo surfaces, props table.
//!
//! Token names match idea-ui's theme so the installed light/dark
//! palette drives this chrome; the fallbacks keep it legible if a
//! token name drifts.

use runtime_core::stylesheet;
use runtime_core::{
    AlignItems, Color, FlexDirection, FlexWrap, JustifyContent, Length, Overflow, Position, TextAlign,
    Tokenized,
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
            // The content is the primary surface — the raised card the
            // sidebar's secondary surface frames. Full-bleed on a phone
            // (the sidebar is a drawer there, nothing to frame against).
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
        }
        // From `lg` the sidebar is pinned beside the content (`app()` pins
        // the `AppShell` at `Breakpoint::Lg`): the content becomes a card,
        // bordered, rounded and lifted off the frame.
        breakpoint lg(_t) {
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            border_radius: Tokenized::token("radius-lg", Length::Px(12.0)),
            shadow: runtime_core::Shadow {
                x: 0.0,
                y: 1.0,
                blur: 3.0,
                color: Tokenized::token("effect-shadow", Color("rgba(15, 17, 21, 0.10)".into())),
            },
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
            // The secondary surface, shared with the sidebar: the frame
            // around the content card.
            background: Tokenized::token("color-background", Color("#f7f8fb".into())),
        }
        // Inset the card from the frame's top, right and bottom; on the
        // left the sidebar's own padding is the gap.
        breakpoint lg(_t) {
            padding_top: Tokenized::token("spacing-sm", Length::Px(8.0)),
            padding_right: Tokenized::token("spacing-sm", Length::Px(8.0)),
            padding_bottom: Tokenized::token("spacing-sm", Length::Px(8.0)),
        }
        transitions {
            background: 250ms EaseInOut,
        }
    }
}

// The padded page body: the full width of the content card.
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

// The sidebar list's scroll surface, under the pinned search box: the
// AppShell panel is a plain fixed-height surface, so the sidebar provides
// its own. Carries the
// secondary surface (`color-background`, the frame around the content
// card), so SidebarBody's
// deliberate lack of `min_height: 100%` (see the note there) doesn't
// leave a transparent gap below short content.
stylesheet! {
    pub SidebarScroll<()> {
        base(_t) {
            // The frame's remaining height. A zero minimum, or the list's
            // content height becomes its floor and nothing scrolls.
            flex_grow: 1.0,
            flex_basis: 0.0,
            min_height: 0.0,
            flex_direction: FlexDirection::Column,
            background: Tokenized::token("color-background", Color("#f7f8fb".into())),
        }
        transitions {
            background: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub SidebarBody<()> {
        base(_t) {
            // The secondary surface; no divider — the content card's own
            // edge separates the two.
            background: Tokenized::token("color-background", Color("#f7f8fb".into())),
            padding: Tokenized::token("spacing-lg", Length::Px(16.0)),
            gap: Tokenized::token("spacing-xs", Length::Px(4.0)),
            flex_direction: FlexDirection::Column,
            // NB: `min_height: Percent(100)` alone is a trap here (and
            // it's spec flexbox, not a Taffy quirk — web browsers do the
            // identical thing): a definite min-height REPLACES flexbox's
            // content-size shrink floor, so the default `flex_shrink: 1`
            // clamps SidebarBody to the scroll view's height and the
            // bottom of the sidebar (the dark-mode toggle, the last few
            // nav links) renders outside SidebarBody's frame. Still
            // visible (scroll content extends past it), but iOS UIView
            // hit-testing won't descend into children outside the
            // parent's frame — taps on the bottom half fall through.
            // Either omit min_height (done here; the scroll_view wrapper
            // carries the same `color-background` so there's no
            // transparent gap) or pair it with `flex_shrink: 0` (the
            // website's SidebarBody; pinned by runtime-layout's
            // `regression_min_height_pct_scroll_body_keeps_content_height_with_shrink_zero`).
        }
        transitions {
            background: 250ms EaseInOut,
        }
    }
}

// The sidebar: the search box pinned at the top, the scrolling list
// filling the rest.
stylesheet! {
    pub SidebarFrame<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            height: Length::pct(100.0),
            background: Tokenized::token("color-background", Color("#f7f8fb".into())),
        }
        transitions {
            background: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub SidebarSearchBar<()> {
        base(_t) {
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Column,
            gap: Tokenized::token("spacing-sm", Length::Px(8.0)),
            padding_left: Tokenized::token("spacing-lg", Length::Px(16.0)),
            padding_right: Tokenized::token("spacing-lg", Length::Px(16.0)),
            padding_top: Tokenized::token("spacing-lg", Length::Px(16.0)),
            padding_bottom: Tokenized::token("spacing-xs", Length::Px(4.0)),
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
            // The code surface's inset, INSIDE the block's scroll region
            // (the codeblock SDK diverts it to the text widget's insets
            // on native), so it scrolls with the content. Author-driven:
            // the SDK's native handlers bake in no inset of their own.
            padding: 20.0,
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
            // A recipe's live render, on the same canvas as the
            // playground's (`CANVAS_LIGHT` / `CANVAS_DARK`).
            background: Color(CANVAS_LIGHT.into()),
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            border_radius: Tokenized::token("radius-lg", Length::Px(12.0)),
            padding: Tokenized::token("spacing-xl", Length::Px(24.0)),
            gap: Tokenized::token("spacing-md", Length::Px(12.0)),
            // Sized by its render (a column child: no flex basis, which
            // would pin it at `min_height` and spill a taller render over
            // the source below).
            flex_direction: FlexDirection::Column,
            min_height: 160.0,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            position: Position::Relative,
            overflow: Overflow::Hidden,
        }
        variant mode {
            light(_t) {}
            dark(_t) {
                background: Color(CANVAS_DARK.into()),
            }
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
            // Positioned, to paint above the canvas's dot layer.
            position: Position::Relative,
            width: Length::pct(100.0),
            flex_direction: FlexDirection::Column,
            // Stretched, as a page column lays its content out: a recipe is
            // a composition, and centering would shrink its layout to its
            // content — a Stack of progress bars to nothing, a toolbar's
            // Spacer to no gap.
            align_items: AlignItems::Stretch,
            justify_content: JustifyContent::Center,
            gap: Tokenized::token("spacing-md", Length::Px(12.0)),
        }
    }
}

// (Props-table styling lives in idea-ui's themed `Table` / `TableCell`
// components.)

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
            // The primary surface: a field on the sidebar's secondary one.
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
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

// ---- Search filter chips --------------------------------------------------
// A wrapping row of kind-filter chips; each chip has an `active` variant
// (mirrors NavLink/NavLinkText so one signal flips container + text).
stylesheet! {
    pub ChipRow<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            gap: 8.0,
        }
    }
}

// ---- Scope member list ----------------------------------------------------
// A scope's members render side-by-side and wrap to new lines, rather than
// as one long vertical column — far more compact for scopes with many
// entries. Used by `scope_member_group`.
stylesheet! {
    pub MemberRow<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            gap: 12.0,
        }
    }
}

stylesheet! {
    pub Chip<()> {
        base(_t) {
            padding_vertical: 4.0,
            padding_horizontal: 10.0,
            border_radius: Tokenized::token("radius-pill", Length::Full),
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            background: Color("transparent".into()),
        }
        variant active {
            #[default]
            off(_t) {}
            on(_t) {
                background: Tokenized::token(
                    "intent-primary-soft-bg",
                    Color("rgba(91, 108, 255, 0.12)".into()),
                ),
                border_color: Tokenized::token("intent-primary-fg", Color("#3947d6".into())),
            }
        }
        transitions {
            background: 140ms EaseOut,
            border_color: 140ms EaseOut,
        }
    }
}

stylesheet! {
    pub ChipText<()> {
        base(_t) {
            color: Tokenized::token("color-text-muted", Color("#6b7280".into())),
            font_size: 13.0,
            text_align: TextAlign::Center,
        }
        variant active {
            #[default]
            off(_t) {}
            on(_t) {
                color: Tokenized::token("intent-primary-fg", Color("#3947d6".into())),
            }
        }
    }
}

// ---- Search results body (reserve height so the modal doesn't jump) -------
// The result list grows/shrinks as the query + filter change; a min-height
// keeps the modal a stable size (it still scrolls inside the Modal cap when
// results overflow).
stylesheet! {
    pub SearchResultsBody<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: 12.0,
        }
    }
}

// The scrolling results region — a fixed-height viewport so the search
// field + filter chips stay pinned above while only the results scroll
// (and the modal height stays stable regardless of result count).
stylesheet! {
    pub ResultsScroll<()> {
        base(_t) {
            min_height: 340.0,
            max_height: 340.0,
        }
    }
}

// ---- Search result heading row (name + kind tag, inline) ------------------
stylesheet! {
    pub ResultHead<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            flex_wrap: FlexWrap::Wrap,
            gap: 8.0,
        }
    }
}

// ---- Search result namespace (small label above the name) -----------------
stylesheet! {
    pub ResultNamespace<()> {
        base(_t) {
            color: Tokenized::token("color-text-muted", Color("#6b7280".into())),
            font_size: 12.0,
            text_align: TextAlign::Left,
        }
    }
}

// ---- Component playground (src/previews.rs) -------------------------------
//
// One bordered card: a canvas column (toolbar over the canvas) beside the
// props panel, with the Usage bar and snippet underneath.

/// A phone's CSS viewport width — what the playground's "Phone" width
/// frames the component at (the common 390 pt iPhone width).
pub const PHONE_WIDTH_PX: f32 = 390.0;
/// The props panel's width beside the canvas.
pub const PROPS_PANEL_WIDTH_PX: f32 = 300.0;
/// The canvas's height floor, so a small component still sits in a
/// generous stage.
pub const CANVAS_MIN_HEIGHT_PX: f32 = 420.0;

stylesheet! {
    pub PlaygroundCard<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            width: Length::pct(100.0),
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            border_radius: Tokenized::token("radius-lg", Length::Px(12.0)),
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            overflow: Overflow::Hidden,
        }
    }
}

stylesheet! {
    pub PlaygroundBody<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            align_items: AlignItems::Stretch,
            width: Length::pct(100.0),
        }
    }
}

stylesheet! {
    pub CanvasColumn<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            flex_shrink: 1.0,
            flex_basis: 0.0,
            min_width: 320.0,
        }
    }
}

stylesheet! {
    pub CanvasToolbar<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: Tokenized::token("spacing-sm", Length::Px(8.0)),
            padding_left: Tokenized::token("spacing-md", Length::Px(12.0)),
            padding_right: Tokenized::token("spacing-md", Length::Px(12.0)),
            padding_top: Tokenized::token("spacing-sm", Length::Px(8.0)),
            padding_bottom: Tokenized::token("spacing-sm", Length::Px(8.0)),
            border_bottom_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
        }
    }
}

stylesheet! {
    pub ToolbarSpacer<()> {
        base(_t) {
            flex_grow: 1.0,
        }
    }
}

/// The canvas a preview sits on — the playground's and each recipe's: a
/// fixed neutral under a dot grid (`previews::CanvasDots`), not a theme
/// color. No flat color works for both kinds of component. The theme's
/// border (`color-border`) sits between its surfaces (`color-surface`,
/// `-surface-alt`, `-background`), so a canvas darker than the surfaces
/// hides the rings and tracks of outlined controls (a Radio, a Switch),
/// and one made of a surface color hides a fill of that color (a `Card`
/// on a white canvas). The base is near-white, so borders read as they do
/// on a page; the dots stop at every filled box, so a fill shows even
/// where its color matches the base. Picked by `previews::canvas_style`
/// from the docs' light/dark switch.
pub const CANVAS_LIGHT: &str = "#fbfcfd";
pub const CANVAS_DARK: &str = "#07090c";
/// The dot grid's tiles (`previews::CanvasDots`): their side, and how
/// many run across and down — enough to cover the widest canvas and the
/// tallest (the playground's grows with its props panel).
pub const DOT_TILE_PX: u16 = 256;
pub const DOT_TILE_COLUMNS: usize = 8;
pub const DOT_TILE_ROWS: usize = 8;
const DOT_LAYER_WIDTH_PX: f32 = DOT_TILE_PX as f32 * DOT_TILE_COLUMNS as f32;
const DOT_LAYER_HEIGHT_PX: f32 = DOT_TILE_PX as f32 * DOT_TILE_ROWS as f32;
/// The dot grid's color on each canvas.
pub const CANVAS_DOTS_LIGHT: &str = "#cdd3dc";
pub const CANVAS_DOTS_DARK: &str = "#262c36";

stylesheet! {
    pub Canvas<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            min_height: CANVAS_MIN_HEIGHT_PX,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            padding: Tokenized::token("spacing-xl", Length::Px(24.0)),
            background: Color(CANVAS_LIGHT.into()),
            position: Position::Relative,
            overflow: Overflow::Hidden,
        }
        variant mode {
            light(_t) {}
            dark(_t) {
                background: Color(CANVAS_DARK.into()),
            }
        }
    }
}

stylesheet! {
    /// The dot grid behind a preview: a fixed block of tiles from the
    /// canvas's top-left corner, larger than any canvas, which clips it.
    /// Sized, not stretched to the canvas: a canvas-wide row would wrap
    /// after however many whole tiles fit, leaving a bare strip at the
    /// right edge.
    pub CanvasDotLayer<()> {
        base(_t) {
            position: Position::Absolute,
            top: 0.0,
            left: 0.0,
            width: DOT_LAYER_WIDTH_PX,
            height: DOT_LAYER_HEIGHT_PX,
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
        }
    }
}

stylesheet! {
    /// The preview above the dot grid: positioned, so it paints over the
    /// absolutely-positioned layer instead of under it.
    pub CanvasContent<()> {
        base(_t) {
            position: Position::Relative,
            width: Length::pct(100.0),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
        }
    }
}

stylesheet! {
    pub FillFrame<()> {
        base(_t) {
            width: Length::pct(100.0),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
        }
    }
}

stylesheet! {
    pub PhoneFrame<()> {
        base(_t) {
            width: PHONE_WIDTH_PX,
            max_width: Length::pct(100.0),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            padding: Tokenized::token("spacing-lg", Length::Px(16.0)),
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            border_radius: Tokenized::token("radius-lg", Length::Px(12.0)),
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
        }
    }
}

stylesheet! {
    /// The props panel's column beside the canvas. In the wide layout it
    /// is as tall as the canvas column and its rows scroll inside it
    /// ([`PropsScroll`]), so a long props list never stretches the canvas
    /// down the page.
    pub PropsColumn<()> {
        base(_t) {
            width: PROPS_PANEL_WIDTH_PX,
            flex_shrink: 0.0,
            max_width: Length::pct(100.0),
            flex_direction: FlexDirection::Column,
            position: Position::Relative,
            border_left_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
        }
    }
}

stylesheet! {
    /// The props panel's scroll view, inside [`PropsColumn`]. Side by side
    /// (`lg`) it is lifted out of flow to fill the column: in flow, its
    /// rows' height would set the playground row's height — a wrapping
    /// row's line is as tall as its tallest item, so no height cap on the
    /// row could bound it. Below `lg` the column can wrap under the canvas,
    /// where it must keep its rows' height, so it stays in flow there.
    pub PropsScroll<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            min_height: 0.0,
        }
        breakpoint lg(_t) {
            position: Position::Absolute,
            top: 0.0,
            left: 0.0,
            right: 0.0,
            bottom: 0.0,
        }
    }
}

stylesheet! {
    /// The props panel's content, inside [`PropsScroll`].
    pub PropsPanel<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: Tokenized::token("spacing-md", Length::Px(12.0)),
            padding: Tokenized::token("spacing-lg", Length::Px(16.0)),
        }
    }
}

stylesheet! {
    pub PanelCaption<()> {
        base(_t) {
            font_size: 13.0,
            color: Tokenized::token("color-text-muted", Color("#5b6275".into())),
        }
    }
}

stylesheet! {
    pub PropRow<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: Tokenized::token("spacing-xs", Length::Px(4.0)),
        }
    }
}

stylesheet! {
    pub PropRowHead<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            gap: Tokenized::token("spacing-sm", Length::Px(8.0)),
        }
    }
}

stylesheet! {
    pub PropName<()> {
        base(_t) {
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace",
            font_size: 13.0,
            font_weight: runtime_core::FontWeight::SemiBold,
            color: Tokenized::token("color-text", Color("#1f2328".into())),
        }
    }
}

stylesheet! {
    pub PropType<()> {
        base(_t) {
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace",
            font_size: 12.0,
            color: Tokenized::token("color-text-muted", Color("#5b6275".into())),
        }
    }
}

stylesheet! {
    /// The playground's code section, full-bleed at the bottom of the
    /// playground card: the Imports block and the Usage snippet on the code
    /// surface's color, each under its own head, ruled off from the canvas.
    pub CodeSection<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            background: Tokenized::token("color-surface-alt", Color("#f4eedb".into())),
            border_top_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            min_width: 0.0,
        }
    }
}

stylesheet! {
    /// One code block in the playground's code section, holding its Copy
    /// button. `divided` rules it off from the block above.
    pub CodeSectionBlock<()> {
        base(_t) {
            position: Position::Relative,
            min_width: 0.0,
            border_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
        }
        variant divided {
            #[default]
            off(_t) {}
            on(_t) { border_top_width: 1.0 }
        }
    }
}

stylesheet! {
    /// A code block's Copy button, pinned to the block's top-right corner.
    pub CodeSectionCopy<()> {
        base(_t) {
            position: Position::Absolute,
            top: 8.0,
            right: 8.0,
        }
    }
}

stylesheet! {
    pub TitleRow<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            flex_wrap: FlexWrap::Wrap,
            gap: Tokenized::token("spacing-md", Length::Px(12.0)),
        }
    }
}

stylesheet! {
    pub TitleGroup<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: Tokenized::token("spacing-sm", Length::Px(8.0)),
        }
    }
}

stylesheet! {
    pub ModulePathRow<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            flex_wrap: FlexWrap::Wrap,
            gap: Tokenized::token("spacing-xs", Length::Px(4.0)),
        }
    }
}

stylesheet! {
    pub ModulePath<()> {
        base(_t) {
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace",
            font_size: 13.0,
            color: Tokenized::token("color-text", Color("#1f2328".into())),
        }
    }
}

// ---- Sidebar additions -----------------------------------------------------

stylesheet! {
    pub SidebarGroupHead<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::SpaceBetween,
            align_items: AlignItems::Center,
            padding_left: Tokenized::token("spacing-sm", Length::Px(8.0)),
            padding_right: Tokenized::token("spacing-sm", Length::Px(8.0)),
            padding_top: Tokenized::token("spacing-lg", Length::Px(16.0)),
            padding_bottom: Tokenized::token("spacing-xs", Length::Px(4.0)),
        }
    }
}

stylesheet! {
    pub SidebarGroupTitle<()> {
        base(_t) {
            font_size: 13.0,
            font_weight: runtime_core::FontWeight::SemiBold,
            color: Tokenized::token("color-text", Color("#1f2328".into())),
        }
    }
}

stylesheet! {
    pub SidebarCount<()> {
        base(_t) {
            font_size: 12.0,
            color: Tokenized::token("color-text-muted", Color("#5b6275".into())),
        }
    }
}

stylesheet! {
    pub SearchKbd<()> {
        base(_t) {
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace",
            font_size: 12.0,
            color: Tokenized::token("color-text-muted", Color("#5b6275".into())),
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            border_radius: Tokenized::token("radius-sm", Length::Px(4.0)),
            padding_left: 6.0,
            padding_right: 6.0,
        }
    }
}

stylesheet! {
    pub NavRow<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            width: Length::pct(100.0),
        }
    }
}

stylesheet! {
    pub PropDefault<()> {
        base(_t) {
            font_size: 12.0,
            color: Tokenized::token("color-text-muted", Color("#5b6275".into())),
        }
    }
}

// ---- Library switcher (src/shell.rs) ---------------------------------------

stylesheet! {
    /// The sidebar's entries for one library, spaced like the body.
    pub SidebarNavGroup<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: Tokenized::token("spacing-xs", Length::Px(4.0)),
        }
    }
}

stylesheet! {
    /// A library's name over its summary, taking the row's free width.
    pub LibraryCardText<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            flex_shrink: 1.0,
            min_width: 0.0,
            gap: 2.0,
        }
    }
}

stylesheet! {
    pub LibraryPopoverBody<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: Tokenized::token("spacing-sm", Length::Px(8.0)),
        }
    }
}

stylesheet! {
    pub LibraryRowList<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: 2.0,
        }
    }
}

stylesheet! {
    /// One library in the switcher's list; `active` marks the one shown.
    pub LibraryRow<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: Tokenized::token("spacing-sm", Length::Px(8.0)),
            padding_vertical: Tokenized::token("spacing-sm", Length::Px(8.0)),
            padding_horizontal: Tokenized::token("spacing-sm", Length::Px(8.0)),
            border_radius: Tokenized::token("radius-md", Length::Px(8.0)),
        }
        state hovered(_t) {
            background: Tokenized::token("color-surface-alt", Color("#f1f5f9".into())),
        }
        variant active {
            #[default]
            off(_t) {}
            on(_t) {
                background: Tokenized::token("intent-primary-soft-bg", Color("#eef0ff".into())),
            }
        }
    }
}

stylesheet! {
    pub LibraryRowHead<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: Tokenized::token("spacing-sm", Length::Px(8.0)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_core::resolve_style;

    /// Regression: a recipe's preview box carried `flex_basis: 0` and a
    /// grow factor from a side-by-side layout. Stacked in the recipe card's
    /// column, the browser sized it at its 160px minimum — an auto-height
    /// flex column sums its items' flex bases — so a taller render (four
    /// stacked alerts, the type scale) spilled over the source below. The
    /// box must be sized by its render: no basis, no grow.
    ///
    /// Asserted on the rules, not a layout: Taffy (what the tests could lay
    /// out with) sizes that column from content, as the current spec does,
    /// so it grows the box either way and can't show the bug.
    #[test]
    fn regression_preview_box_is_sized_by_its_render() {
        runtime_core::__with_fresh_world(|| {
            let rules = resolve_style(&PreviewBox().into_style_application());
            assert_eq!(rules.flex_basis, None);
            assert_eq!(rules.flex_grow, None);
        });
    }
}
