//! Stylesheets for the idea-ui docs chrome — sidebar, reading column,
//! per-page demo surfaces, props table.
//!
//! Token names match idea-ui's theme so the installed light/dark
//! palette drives this chrome; the fallbacks keep it legible if a
//! token name drifts.

use runtime_core::stylesheet;
use runtime_core::{
    AlignItems, Color, Cursor, FlexDirection, FontWeight, JustifyContent, Length, Overflow,
    Position, TextAlign, TextTransform, Tokenized,
};
use idea_ui::IdeaThemeRef;

// ---- Page-level scroll surface --------------------------------------------

stylesheet! {
    pub ScreenScroll<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            width: Length::pct(100.0),
            // Fill the remaining height under the (conditional) top bar
            // so the page body scrolls inside its own region.
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

// Scroll-viewport wrapper around each screen's `scroll_view` — its
// bound handle reports the visible scroll box for the TOC scroll-spy.
// Same fill rules as `ScreenScroll`, which it tightly wraps.
stylesheet! {
    pub ScreenFill<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            width: Length::pct(100.0),
            flex_grow: 1.0,
            flex_shrink: 1.0,
            flex_basis: 0.0,
        }
    }
}

// Content-height column wrapping the whole scrolled tree — its bound
// handle reports the total scrollable content height for the spy.
stylesheet! {
    pub ScrollContent<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            width: Length::pct(100.0),
        }
    }
}

// Full-screen fallback while a page's wasm chunk fetches (or fails):
// the whole page frame lives inside the chunk, so this is the only
// thing on screen and must fill the scroller's viewport to center the
// loader / error UI.
stylesheet! {
    pub ChunkLoaderBar<()> {
        base(_t) {
            // Full-bleed route loader: the Progress track is 100% of its
            // parent, and the fallback column centers its children, so
            // the wrapper must claim the full outlet width itself.
            width: Length::pct(100.0),
        }
    }
}

stylesheet! {
    pub ChunkFallback<()> {
        base(_t) {
            width: Length::pct(100.0),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            // Fill the scroller's viewport. `flex_shrink: 0` is
            // load-bearing: inside a flex-column scroller,
            // `min_height: 100%` replaces the default content-size
            // shrink floor, and without it the box clamps back to
            // content height and the spinner hugs the top edge.
            min_height: Length::pct(100.0),
            flex_shrink: 0.0,
        }
    }
}

// Outer column for each screen: the (conditional) hamburger top bar
// stacked over the scrolling page body. Fills the navigator's body
// outlet so the scroll view can take the leftover height.
stylesheet! {
    pub PageColumn<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            width: Length::pct(100.0),
            height: Length::pct(100.0),
            background: t.color.background(),
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

// ---- "On this page" (TOC) column ------------------------------------------
// Ported from websites/website's `layout_with_toc` panel (same look,
// same tokens) — the docs' page frame renders it to the right of the
// reading column at Lg+ and drives it with the shared scroll-spy in
// `shell.rs`.

// Row wrapping the reading column + the TOC: the main column GROWS to
// take all leftover width (pushing the fixed-width TOC to the right
// edge), and `PagePad` centers itself inside it via `align_self`.
// `FlexStart` keeps the TOC content-height so its sticky positioning
// can track the scroll.
stylesheet! {
    pub PageRow<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::FlexStart,
            width: Length::pct(100.0),
        }
    }
}

// The growing main slot of `PageRow`. `min_width: 0` overrides the
// flex min-content floor so wide demo content (tables, code panels)
// shrinks/scrolls inside the reading column instead of shoving the
// TOC off the right edge.
stylesheet! {
    pub PageRowMain<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            flex_shrink: 1.0,
            min_width: 0.0,
        }
    }
}

stylesheet! {
    pub TocPanel<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.xs(),
            width: 200.0,
            min_width: 200.0,
            flex_shrink: 0.0,
            // Sit near the top of the page (the reading column's group
            // overline), not sunk below it; keep clear of the right
            // edge.
            padding_top: 16.0,
            padding_right: 24.0,
            // Sticky so the panel stays in view as the page scrolls —
            // web emits CSS `sticky`; native backends pin via their
            // sticky registries. The stuck offset matches the resting
            // padding so pinning doesn't jump the panel.
            position: Position::Sticky,
            top: Length::Px(16.0),
        }
    }
}

stylesheet! {
    pub TocHeader<IdeaThemeRef> {
        base(t) {
            color: t.color.text_muted(),
            font_size: 11.0,
            font_weight: FontWeight::SemiBold,
            letter_spacing: 0.8,
            text_transform: TextTransform::Uppercase,
            padding_bottom: 8.0,
        }
    }
}

stylesheet! {
    pub TocLink<IdeaThemeRef> {
        base(t) {
            padding_vertical: 6.0,
            padding_left: 12.0,
            border_left_width: 2.0,
            border_left_color: t.color.border(),
            color: t.color.text_muted(),
            font_size: 13.0,
            line_height: 18.0,
            text_align: TextAlign::Left,
            cursor: Cursor::Pointer,
        }
        variant active {
            #[default]
            off(_t) {}
            on(t) {
                border_left_color: t.intent.primary.fg(),
                color: t.intent.primary.fg(),
                font_weight: FontWeight::SemiBold,
            }
        }
        state hovered(t) {
            color: t.color.text(),
        }
        transitions {
            color: 180ms EaseOut,
            border_left_color: 180ms EaseOut,
        }
    }
}

// ---- Mobile top bar + hamburger -------------------------------------------

// Persistent top strip that hosts the hamburger on narrow viewports.
// Only rendered when the drawer is collapsed (below the pin width), so
// it never shows on wide layouts where the sidebar is pinned.
stylesheet! {
    pub TopBar<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            padding_horizontal: t.spacing.md(),
            padding_vertical: t.spacing.sm(),
            background: t.color.surface(),
            border_bottom_width: 1.0,
            border_bottom_color: t.color.border(),
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
    pub MenuButton<IdeaThemeRef> {
        base(t) {
            cursor: Cursor::Pointer,
            width: Length::Px(34.0),
            height: Length::Px(34.0),
            border_radius: t.radius.md(),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            // Pull the icon toward the header's leading edge so it lines
            // up with the sidebar's own left inset.
            margin_left: -6.0,
            background: Color("transparent".into()),
        }
        state hovered(t) {
            background: t.color.surface_alt(),
        }
        transitions { background: 150ms EaseOut, }
    }
}

// The ☰ glyph inside `MenuButton`, tinted with the theme's text color.
stylesheet! {
    pub MenuGlyph<IdeaThemeRef> {
        base(t) {
            font_size: 19.0,
            text_align: TextAlign::Center,
            color: t.color.text(),
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
    pub SidebarScroll<IdeaThemeRef> {
        base(t) {
            background: t.color.surface(),
            border_right_width: 1.0,
            border_right_color: t.color.border(),
            flex_direction: FlexDirection::Column,
            width: Length::pct(100.0),
            // Fill the AppShell panel's height: a short nav list still
            // draws the divider full-height, and a tall one is clamped
            // here so THIS node overflows and scrolls. An author style
            // replaces the scroll_view's default grow seed, so it must
            // be restated.
            flex_grow: 1.0,
            flex_basis: Length::Px(0.0),
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
    pub SidebarBody<IdeaThemeRef> {
        base(t) {
            padding: t.spacing.lg(),
            gap: t.spacing.xs(),
            flex_direction: FlexDirection::Column,
            width: Length::pct(100.0),
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
    pub NavLink<IdeaThemeRef> {
        base(t) {
            padding_vertical: t.spacing.sm(),
            padding_horizontal: t.spacing.md(),
            border_radius: t.radius.md(),
            background: Color("transparent".into()),
            flex_direction: FlexDirection::Column,
        }
        variant active {
            #[default]
            off(_t) {}
            on(t) {
                background: t.intent.primary.soft_bg(),
            }
        }
        transitions {
            background: 180ms EaseOut,
        }
    }
}

stylesheet! {
    pub NavLinkText<IdeaThemeRef> {
        base(t) {
            color: t.color.text_muted(),
            font_size: 14.0,
            text_align: TextAlign::Left,
        }
        variant active {
            #[default]
            off(_t) {}
            on(t) {
                color: t.intent.primary.fg(),
            }
        }
        state hovered(t) {
            color: t.color.text(),
        }
        transitions {
            color: 180ms EaseOut,
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
    pub CodeText<IdeaThemeRef> {
        base(t) {
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace",
            font_size: 13.0,
            line_height: 20.0,
            color: t.color.text(),
        }
        transitions {
            color: 250ms EaseInOut,
        }
    }
}

// ---- Demo surface — boxed live-preview area on a component page ----------

stylesheet! {
    pub DemoSurface<IdeaThemeRef> {
        base(t) {
            background: t.color.surface(),
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.lg(),
            padding: t.spacing.xl(),
            gap: t.spacing.lg(),
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

// Inner content column for a DemoSurface. The card spans the page, but its
// content is capped and centered so a FULL-WIDTH component (a `Field` fills its
// container by design) renders at a sensible width instead of sprawling the
// whole card. `align_items: center` keeps content-sized previews (Button, Badge)
// centered; a Field's own `align_self: stretch` fills this capped column.
stylesheet! {
    pub DemoSurfaceContent<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            gap: t.spacing.lg(),
            width: Length::pct(100.0),
            max_width: 480.0,
        }
    }
}

// Side-by-side wrapping row: preview on the left, controls on the right.
stylesheet! {
    pub DemoRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            gap: t.spacing.xl(),
            align_items: AlignItems::FlexStart,
            flex_wrap: runtime_core::FlexWrap::Wrap,
        }
    }
}

stylesheet! {
    pub PreviewBox<IdeaThemeRef> {
        base(t) {
            // Same surface as `DemoSurface` but designed to share a row
            // with the controls panel.
            background: t.color.surface(),
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.lg(),
            padding: t.spacing.xl(),
            gap: t.spacing.md(),
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
    pub ControlsBox<IdeaThemeRef> {
        base(t) {
            flex_basis: 0.0,
            flex_grow: 1.0,
            min_width: 240.0,
            max_width: 360.0,
            gap: t.spacing.sm(),
            flex_direction: FlexDirection::Column,
        }
    }
}

// Inner wrapper that gives previews a known `width: 100%` container,
// so full-width children (Divider, Alert) resolve `100%` against the
// preview's real width rather than a `display: block` collapsed
// placeholder. See the historical layout-fix commit message.
stylesheet! {
    pub PreviewSlot<IdeaThemeRef> {
        base(t) {
            width: Length::pct(100.0),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            gap: t.spacing.md(),
        }
    }
}

// (Props-table styling now lives in idea-ui's themed `Table` /
// `TableCell` components — this file no longer needs PropsTable /
// PropCell* sheets.)

// ---- Callout (tips / notes / cross-links) ---------------------------------

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

// ===========================================================================
// Design chrome — header bar, segmented theme toggle, sidebar search,
// status dots, group overline, status badge, Usage label. These mirror
// the idea-ui reference design.
// ===========================================================================

// ---- Header bar (top slot) -------------------------------------------------

stylesheet! {
    pub DocHeader<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            height: Length::Px(58.0),
            padding_horizontal: 22.0,
            gap: 16.0,
            background: t.color.surface(),
            border_bottom_width: 1.0,
            border_bottom_color: t.color.border(),
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
    pub LogoBox<IdeaThemeRef> {
        base(t) {
            width: Length::Px(27.0),
            height: Length::Px(27.0),
            border_radius: 8.0,
            background: t.intent.primary.solid_bg(),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
        }
    }
}

stylesheet! {
    pub LogoGlyph<IdeaThemeRef> {
        base(t) {
            color: t.intent.primary.solid_text(),
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace",
            font_size: 14.0,
            font_weight: FontWeight::Bold,
        }
    }
}

stylesheet! {
    pub BrandName<IdeaThemeRef> {
        base(t) {
            color: t.color.text(),
            font_size: 15.0,
            font_weight: FontWeight::Bold,
        }
        transitions { color: 250ms EaseInOut, }
    }
}

stylesheet! {
    pub VersionPill<IdeaThemeRef> {
        base(t) {
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace",
            font_size: 11.0,
            font_weight: FontWeight::Medium,
            color: t.color.text_muted(),
            background: t.color.surface_alt(),
            border_width: 1.0,
            border_color: t.color.border(),
            padding_horizontal: 8.0,
            padding_vertical: 2.0,
            border_radius: Length::Full,
        }
    }
}

stylesheet! {
    pub HeaderMono<IdeaThemeRef> {
        base(t) {
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace",
            font_size: 12.0,
            font_weight: FontWeight::Medium,
            color: t.color.text_muted(),
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
    pub SegToggle<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            background: t.color.surface_alt(),
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: Length::Full,
            padding: 3.0,
            gap: 2.0,
        }
        transitions { background: 250ms EaseInOut, }
    }
}

stylesheet! {
    pub SegBtn<IdeaThemeRef> {
        base(_t) {
            padding_horizontal: 14.0,
            padding_vertical: 5.0,
            border_radius: Length::Full,
            background: Color("transparent".into()),
            cursor: Cursor::Pointer,
        }
        variant active {
            #[default]
            off(_t) {}
            on(t) {
                background: t.color.surface(),
            }
        }
        transitions { background: 150ms EaseOut, }
    }
}

stylesheet! {
    pub SegBtnText<IdeaThemeRef> {
        base(t) {
            font_size: 13.0,
            text_align: TextAlign::Center,
            color: t.color.text_muted(),
        }
        variant active {
            #[default]
            off(_t) {}
            on(t) {
                color: t.color.text(),
                font_weight: FontWeight::SemiBold,
            }
        }
        transitions { color: 150ms EaseOut, }
    }
}

// ---- Sidebar search --------------------------------------------------------

stylesheet! {
    pub SearchInput<IdeaThemeRef> {
        base(t) {
            width: Length::pct(100.0),
            padding_horizontal: 12.0,
            padding_vertical: 8.0,
            font_size: 13.0,
            background: t.color.surface_alt(),
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: 8.0,
            color: t.color.text(),
            margin_bottom: 12.0,
        }
        // Focus ring: recolor the existing 1px border to the theme accent —
        // thin + themed, not a heavy native ring. Web gets this from CSS
        // `:focus`; macOS drives it off StateBits::FOCUSED (NSTextField
        // begin/end editing). Same observable result on both (§7).
        state focused(t) {
            border_color: t.color.focus_ring(),
        }
        transitions { background: 250ms EaseInOut, border_color: 150ms EaseOut, }
    }
}

// Search TRIGGER — a button styled like the old search field that opens the
// search dialog. The actual text input now lives inside the modal.
stylesheet! {
    pub SearchTrigger<IdeaThemeRef> {
        base(t) {
            cursor: Cursor::Pointer,
            width: Length::pct(100.0),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            padding_horizontal: 12.0,
            padding_vertical: 9.0,
            background: t.color.surface_alt(),
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: 8.0,
            margin_bottom: 12.0,
        }
        state hovered(t) {
            border_color: t.color.focus_ring(),
        }
        transitions { border_color: 150ms EaseOut, }
    }
}

stylesheet! {
    pub SearchTriggerText<IdeaThemeRef> {
        base(t) {
            font_size: 13.0,
            color: t.color.text_muted(),
        }
    }
}

// Search DIALOG body — a fixed-feeling palette: the input stays pinned at the
// top while the result list scrolls. `min_height` keeps the dialog from
// collapsing to a sliver on few/no matches; `max_height` clamps it so a long
// list scrolls inside instead of stretching the modal. (The modal's own
// viewport cap clamps these further on a short screen.)
stylesheet! {
    pub SearchDialogBody<()> {
        base(_t) {
            width: Length::pct(100.0),
            flex_direction: FlexDirection::Column,
            gap: 8.0,
            min_height: 360.0,
            max_height: 560.0,
        }
    }
}

// The scrolling result region — takes the height left under the input
// (`flex_grow` is seeded on every scroll_view) and scrolls its overflow.
// `min_height: 0` lets it actually shrink inside the flex column so the
// scroller bounds (without it a flex child's implicit min-content floor can
// push past the parent and break the inner scroll).
stylesheet! {
    pub SearchResultsScroll<()> {
        base(_t) {
            width: Length::pct(100.0),
            min_height: 0.0,
        }
    }
}

// Search field with a leading icon: the ROW carries the chrome (bg + border +
// radius + padding); the icon sits left and the input fills the rest
// borderless. The input can't host a leading slot itself, so we compose.
stylesheet! {
    pub SearchFieldRow<IdeaThemeRef> {
        base(t) {
            width: Length::pct(100.0),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 8.0,
            padding_horizontal: 12.0,
            padding_vertical: 8.0,
            background: t.color.surface_alt(),
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: 8.0,
        }
    }
}

// The borderless input inside `SearchFieldRow` — fills the remaining width;
// the row draws the visible chrome, so this is transparent with no border.
stylesheet! {
    pub SearchInputBare<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            font_size: 13.0,
            color: t.color.text(),
            background: Color("transparent".into()),
            border_width: 0.0,
        }
    }
}

// ---- Sidebar nav item (row: label + status dot) ---------------------------

stylesheet! {
    pub NavItem<IdeaThemeRef> {
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
            // These rows sit inside a `link(route = …)`. On web that lowers
            // to a real `<a href>`, which the UA stylesheet gives a hand for
            // free; no native backend has that, and the framework defaults
            // no cursor on any primitive. Declare it so the sidebar reads as
            // clickable on GTK/AppKit too.
            cursor: Cursor::Pointer,
        }
        variant active {
            #[default]
            off(_t) {}
            on(t) {
                background: t.intent.primary.soft_bg(),
            }
        }
        transitions { background: 150ms EaseOut, }
    }
}

stylesheet! {
    pub NavDot<IdeaThemeRef> {
        base(t) {
            width: Length::Px(6.0),
            height: Length::Px(6.0),
            border_radius: Length::Full,
            background: t.color.border_strong(),
        }
        // "ready"/Detailed entries get a green dot; Preview keep the base grey.
        variant ready {
            #[default]
            off(_t) {}
            on(t) {
                background: t.intent.success.fg(),
            }
        }
    }
}

// ---- Page frame: group overline, status badge, Usage label ----------------

stylesheet! {
    pub GroupOverline<IdeaThemeRef> {
        base(t) {
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace",
            font_size: 11.0,
            font_weight: FontWeight::SemiBold,
            letter_spacing: 1.3,
            text_transform: TextTransform::Uppercase,
            color: t.intent.primary.fg(),
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
    pub StatusBadge<IdeaThemeRef> {
        base(t) {
            padding_horizontal: 10.0,
            padding_vertical: 4.0,
            border_radius: Length::Full,
            border_width: 1.0,
            background: t.color.surface_alt(),
            border_color: t.color.border(),
        }
        variant detailed {
            #[default]
            off(_t) {}
            on(t) {
                background: t.intent.success.soft_bg(),
                border_color: t.intent.success.border(),
            }
        }
    }
}

stylesheet! {
    pub StatusBadgeText<IdeaThemeRef> {
        base(t) {
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace",
            font_size: 11.0,
            font_weight: FontWeight::SemiBold,
            color: t.color.text_muted(),
        }
        variant detailed {
            #[default]
            off(_t) {}
            on(t) {
                color: t.intent.success.soft_text(),
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
    pub SwatchBlock<IdeaThemeRef> {
        base(t) {
            height: Length::Px(64.0),
            width: Length::pct(100.0),
            border_radius: t.radius.md(),
            border_width: 1.0,
            border_color: t.color.border(),
        }
    }
}

// One box per radius token, picked by the `r` variant.
stylesheet! {
    pub RadiusBox<IdeaThemeRef> {
        base(t) {
            width: Length::Px(76.0),
            height: Length::Px(76.0),
            background: t.intent.primary.soft_bg(),
            border_width: 1.5,
            border_color: t.intent.primary.fg(),
        }
        variant r {
            #[default]
            sm(t) { border_radius: t.radius.sm(), }
            md(t) { border_radius: t.radius.md(), }
            lg(t) { border_radius: t.radius.lg(), }
            pill(t) { border_radius: t.radius.pill(), }
        }
    }
}

// Two small blocks used to demonstrate a Stack gap visually.
stylesheet! {
    pub GapBlock<IdeaThemeRef> {
        base(t) {
            width: Length::Px(40.0),
            height: Length::Px(28.0),
            border_radius: t.radius.sm(),
            background: t.intent.primary.soft_bg(),
            border_width: 1.0,
            border_color: t.intent.primary.border(),
        }
    }
}

stylesheet! {
    pub UsageLabel<IdeaThemeRef> {
        base(t) {
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace",
            font_size: 11.0,
            font_weight: FontWeight::SemiBold,
            letter_spacing: 0.9,
            text_transform: TextTransform::Uppercase,
            color: t.color.text_muted(),
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
        // Full-bleed: the landing spans the whole outlet (no reading-
        // column max-width) — its grids/cards do their own wrapping.
        base(_t) {
            flex_direction: FlexDirection::Column,
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
    pub HeroCard<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::FlexStart,
            background: t.color.surface(),
            border_width: 1.0,
            border_color: t.color.border(),
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
    pub HeroBadge<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 8.0,
            padding_horizontal: 13.0,
            padding_vertical: 5.0,
            border_radius: Length::Full,
            background: t.color.surface_alt(),
            border_width: 1.0,
            border_color: t.color.border(),
        }
        transitions { background: 250ms EaseInOut, }
    }
}

stylesheet! {
    pub HeroDot<IdeaThemeRef> {
        base(t) {
            width: Length::Px(7.0),
            height: Length::Px(7.0),
            border_radius: Length::Full,
            background: t.intent.success.solid_bg(),
        }
    }
}

stylesheet! {
    pub HeroBadgeText<IdeaThemeRef> {
        base(t) {
            font_family: MONO,
            font_size: 11.5,
            font_weight: FontWeight::SemiBold,
            letter_spacing: 0.4,
            color: t.color.text_muted(),
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
    pub CtaPrimary<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            gap: 9.0,
            padding_horizontal: 22.0,
            padding_vertical: 11.0,
            border_radius: t.radius.md(),
            background: t.intent.primary.solid_bg(),
        }
        transitions { background: 150ms EaseOut, }
    }
}

stylesheet! {
    pub CtaPrimaryText<IdeaThemeRef> {
        base(t) {
            font_size: 16.0,
            font_weight: FontWeight::SemiBold,
            text_align: TextAlign::Center,
            color: t.intent.primary.solid_text(),
        }
    }
}

stylesheet! {
    pub CtaOutline<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            gap: 9.0,
            padding_horizontal: 22.0,
            padding_vertical: 11.0,
            border_radius: t.radius.md(),
            border_width: 1.0,
            border_color: t.intent.neutral.border(),
            background: Color("transparent".into()),
        }
        state hovered(t) {
            border_color: t.color.border_strong(),
        }
        transitions { border_color: 150ms EaseOut, }
    }
}

stylesheet! {
    pub CtaOutlineText<IdeaThemeRef> {
        base(t) {
            font_size: 16.0,
            font_weight: FontWeight::SemiBold,
            text_align: TextAlign::Center,
            color: t.intent.neutral.fg(),
        }
    }
}

// ---- Stat cards --------------------------------------------------------

stylesheet! {
    pub StatCard<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: 4.0,
            background: t.color.surface(),
            border_width: 1.0,
            border_color: t.color.border(),
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
    pub StatNumber<IdeaThemeRef> {
        base(_t) {
            font_size: 34.0,
            font_weight: FontWeight::Bold,
            letter_spacing: -0.6,
        }
        variant tone {
            #[default]
            primary(t) { color: t.intent.primary.fg(), }
            success(t) { color: t.intent.success.fg(), }
            info(t) { color: t.intent.info.fg(), }
            warning(t) { color: t.intent.warning.fg(), }
        }
    }
}

stylesheet! {
    pub StatLabel<IdeaThemeRef> {
        base(t) {
            font_size: 13.5,
            font_weight: FontWeight::Medium,
            color: t.color.text_muted(),
        }
    }
}

// ---- Section label (the uppercase rules between landing sections) ------

stylesheet! {
    pub SectionLabel<IdeaThemeRef> {
        base(t) {
            font_family: MONO,
            font_size: 11.0,
            font_weight: FontWeight::SemiBold,
            letter_spacing: 1.3,
            text_transform: TextTransform::Uppercase,
            color: t.color.text_muted(),
            margin_top: 26.0,
        }
    }
}

// ---- Principle feature cards -------------------------------------------

stylesheet! {
    pub FeatureCard<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::FlexStart,
            gap: 14.0,
            background: t.color.surface(),
            border_width: 1.0,
            border_color: t.color.border(),
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
    pub FeatureIconBox<IdeaThemeRef> {
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
            primary(t) {
                background: t.intent.primary.soft_bg(),
                border_color: t.intent.primary.border(),
            }
            info(t) {
                background: t.intent.info.soft_bg(),
                border_color: t.intent.info.border(),
            }
            success(t) {
                background: t.intent.success.soft_bg(),
                border_color: t.intent.success.border(),
            }
            warning(t) {
                background: t.intent.warning.soft_bg(),
                border_color: t.intent.warning.border(),
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
    pub FeatureTitle<IdeaThemeRef> {
        base(t) {
            font_size: 15.0,
            font_weight: FontWeight::SemiBold,
            color: t.color.text(),
            text_align: TextAlign::Left,
        }
    }
}

stylesheet! {
    pub FeatureBody<IdeaThemeRef> {
        base(t) {
            font_size: 13.5,
            line_height: 21.0,
            color: t.color.text_muted(),
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
            // Literal: `code-bg` was a token name nothing installs, so this
            // slate has always been what rendered. The code surface is
            // deliberately fixed-dark in both color schemes.
            background: Color("#0f172a".into()),
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
            // Literal — peer of the `code-bg` note above.
            color: Color("#cbd5e1".into()),
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
    pub CatCard<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: 11.0,
            background: t.color.surface(),
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: 14.0,
            padding_horizontal: 20.0,
            padding_vertical: 18.0,
        }
        state hovered(t) {
            border_color: t.intent.primary.fg(),
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
    pub CatGroupLabel<IdeaThemeRef> {
        base(t) {
            font_family: MONO,
            font_size: 11.0,
            font_weight: FontWeight::SemiBold,
            letter_spacing: 1.1,
            text_transform: TextTransform::Uppercase,
            color: t.color.text_muted(),
            text_align: TextAlign::Left,
        }
    }
}

stylesheet! {
    pub CatCount<IdeaThemeRef> {
        base(t) {
            font_family: MONO,
            font_size: 12.0,
            font_weight: FontWeight::SemiBold,
            color: t.intent.primary.fg(),
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
    pub CatChip<IdeaThemeRef> {
        base(t) {
            font_size: 12.0,
            font_weight: FontWeight::Medium,
            color: t.color.text(),
            background: t.color.surface_alt(),
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: 6.0,
            padding_horizontal: 8.0,
            padding_vertical: 3.0,
        }
    }
}

// A bare icon-sized pressable (the password-visibility eye toggles on the
// forms pages). Exists so those pressables carry the pointer cursor —
// a pressable with NO style renders the default arrow on web.
stylesheet! {
    pub IconToggleBtn<()> {
        base(_t) {
            cursor: Cursor::Pointer,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_core::{resolve_style, StyleApplication};

    /// User-reported: the header's Light/Dark toggle rendered the default
    /// arrow cursor. Root cause: none of this app's hand-rolled pressable
    /// sheets set `cursor` (idea-ui components carry their own; bare
    /// `pressable` + app sheet does not). Pin all four so the next custom
    /// pressable surface copies a correct example.
    #[test]
    fn regression_custom_pressable_sheets_carry_pointer_cursor() {
        let world = runtime_world::World::new();
        world.enter(|| {
            for (name, sheet) in [
                ("SegBtn", SegBtn::sheet()),
                ("MenuButton", MenuButton::sheet()),
                ("SearchTrigger", SearchTrigger::sheet()),
                ("IconToggleBtn", IconToggleBtn::sheet()),
            ] {
                let rules = resolve_style(&StyleApplication::new(sheet));
                assert_eq!(
                    rules.cursor,
                    Some(Cursor::Pointer),
                    "{name} styles a pressable and must show the pointer cursor"
                );
            }
        });
    }
}
