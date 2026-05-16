//! Stylesheets for every idea-ui component.
//!
//! Each `stylesheet!` block declares a typed style function (snake_case
//! `name_style()`) plus per-variant builder methods (`Name().tone(...)`
//! etc). Components in `components/*` consume these directly.
//!
//! All stylesheets close over [`IdeaThemeRef`](crate::theme::IdeaThemeRef)
//! — the framework-side wrapper that hides the trait object behind a
//! concrete type. Inside each closure, calls like `t.colors().primary`
//! dispatch through the `IdeaTheme` trait, so apps that install a
//! custom theme implementation see their values flow into every
//! stylesheet automatically.

use framework_core::stylesheet;
use framework_core::{
    AlignItems, Color, FlexDirection, FontWeight, JustifyContent, Length, TextAlign, TextTransform,
};

use crate::theme::{IdeaTheme, IdeaThemeRef};

// =============================================================================
// Layout — Stack
// =============================================================================

stylesheet! {
    pub Stack<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: Length::Px(t.spacing().md),
        }
        variant gap {
            none(_t) { gap: Length::Px(0.0) }
            xs(t)    { gap: Length::Px(t.spacing().xs) }
            sm(t)    { gap: Length::Px(t.spacing().sm) }
            #[default]
            md(t)    { gap: Length::Px(t.spacing().md) }
            lg(t)    { gap: Length::Px(t.spacing().lg) }
            xl(t)    { gap: Length::Px(t.spacing().xl) }
        }
        variant axis {
            #[default]
            column(_t) { flex_direction: FlexDirection::Column }
            row(_t)    { flex_direction: FlexDirection::Row }
        }
        variant align {
            #[default]
            stretch(_t) { align_items: AlignItems::Stretch }
            start(_t)   { align_items: AlignItems::FlexStart }
            center(_t)  { align_items: AlignItems::Center }
            end(_t)     { align_items: AlignItems::FlexEnd }
        }
        variant justify {
            #[default]
            start(_t)   { justify_content: JustifyContent::FlexStart }
            center(_t)  { justify_content: JustifyContent::Center }
            end(_t)     { justify_content: JustifyContent::FlexEnd }
            between(_t) { justify_content: JustifyContent::SpaceBetween }
            around(_t)  { justify_content: JustifyContent::SpaceAround }
        }
    }
}

// =============================================================================
// Pressable (button)
// =============================================================================
//
// Pressable's *visual* (background, text color, hover/pressed shades)
// is now driven by an `Intent` trait object the component applies as
// overrides on top of this stylesheet. The stylesheet handles the
// intent-agnostic bits: size, padding, radius, type weight, and the
// disabled state.

stylesheet! {
    pub Pressable<IdeaThemeRef> {
        base(t) {
            padding_vertical: t.spacing().sm,
            padding_horizontal: t.spacing().lg,
            border_radius: t.radius().md,
            font_weight: FontWeight::SemiBold,
            font_size: t.typography().size_md,
            text_align: TextAlign::Center,
            letter_spacing: 0.2,
        }
        variant size {
            sm(t) {
                padding_vertical: t.spacing().xs,
                padding_horizontal: t.spacing().md,
                font_size: t.typography().size_sm,
            }
            #[default]
            md(t) {
                padding_vertical: t.spacing().sm,
                padding_horizontal: t.spacing().lg,
                font_size: t.typography().size_md,
            }
            lg(t) {
                padding_vertical: t.spacing().md,
                padding_horizontal: t.spacing().xl,
                font_size: t.typography().size_lg,
            }
        }
        state disabled(_t) {
            opacity: 0.45,
        }
        transitions {
            background: 150ms EaseOut,
            color: 200ms EaseOut,
            opacity: 200ms EaseOut,
            border_color: 150ms EaseOut,
        }
    }
}

// =============================================================================
// Typography — Heading / Body / Caption
// =============================================================================

stylesheet! {
    pub Heading<IdeaThemeRef> {
        base(t) {
            color: t.colors().text.clone(),
            font_weight: FontWeight::SemiBold,
            letter_spacing: -0.3,
        }
        variant kind {
            display(t) {
                font_size: t.typography().size_display,
                font_weight: FontWeight::Bold,
                letter_spacing: -1.0,
                line_height: 42.0,
            }
            #[default]
            h1(t) {
                font_size: t.typography().size_xxl,
                line_height: 34.0,
            }
            h2(t) {
                font_size: t.typography().size_xl,
                line_height: 26.0,
            }
            h3(t) {
                font_size: t.typography().size_lg,
                line_height: 22.0,
            }
        }
        variant align {
            #[default]
            start(_t)  { text_align: TextAlign::Left }
            center(_t) { text_align: TextAlign::Center }
            end(_t)    { text_align: TextAlign::Right }
        }
        transitions {
            color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub Body<IdeaThemeRef> {
        base(t) {
            color: t.colors().text.clone(),
            font_size: t.typography().size_md,
            line_height: 20.0,
        }
        variant tone {
            #[default]
            default(_t) {}
            muted(t)    { color: t.colors().text_muted.clone() }
            primary(t)  { color: t.colors().primary.clone() }
            danger(t)   { color: t.colors().danger.clone() }
            success(t)  { color: t.colors().success.clone() }
            warning(t)  { color: t.colors().warning.clone() }
        }
        variant align {
            #[default]
            start(_t)  { text_align: TextAlign::Left }
            center(_t) { text_align: TextAlign::Center }
            end(_t)    { text_align: TextAlign::Right }
        }
        transitions {
            color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub Caption<IdeaThemeRef> {
        base(t) {
            color: t.colors().text_muted.clone(),
            font_size: t.typography().size_sm,
            line_height: 16.0,
        }
        variant tone {
            #[default]
            default(t) { color: t.colors().text_muted.clone() }
            danger(t)  { color: t.colors().danger.clone() }
            success(t) { color: t.colors().success.clone() }
        }
        variant align {
            #[default]
            start(_t)  { text_align: TextAlign::Left }
            center(_t) { text_align: TextAlign::Center }
            end(_t)    { text_align: TextAlign::Right }
        }
        transitions {
            color: 250ms EaseInOut,
        }
    }
}

// =============================================================================
// Card
// =============================================================================

stylesheet! {
    pub Card<IdeaThemeRef> {
        base(t) {
            background: t.colors().surface.clone(),
            padding: t.spacing().lg,
            border_radius: t.radius().lg,
            gap: Length::Px(t.spacing().sm),
            border_width: 1.0,
            border_color: t.colors().border.clone(),
        }
        variant tone {
            #[default]
            surface(t) {
                background: t.colors().surface.clone(),
            }
            elevated(t) {
                background: t.colors().surface.clone(),
                shadow: framework_core::Shadow {
                    x: 0.0,
                    y: 4.0,
                    blur: 16.0,
                    color: Color("rgba(15, 17, 21, 0.10)".into()),
                },
            }
            primary(t) {
                background: t.colors().primary.clone(),
                color: t.colors().primary_text.clone(),
                border_color: t.colors().primary.clone(),
            }
            muted(t) {
                background: t.colors().surface_alt.clone(),
            }
        }
        variant padding {
            none(_t) { padding: 0.0 }
            sm(t)    { padding: t.spacing().sm }
            #[default]
            md(t)    { padding: t.spacing().lg }
            lg(t)    { padding: t.spacing().xl }
        }
        transitions {
            background: 250ms EaseInOut,
            border_color: 250ms EaseInOut,
            color: 250ms EaseInOut,
        }
    }
}

// =============================================================================
// Field (text input wrapper)
// =============================================================================

stylesheet! {
    pub Field<IdeaThemeRef> {
        base(t) {
            background: t.colors().surface.clone(),
            color: t.colors().text.clone(),
            padding_vertical: t.spacing().sm,
            padding_horizontal: t.spacing().md,
            border_radius: t.radius().md,
            border_width: 1.0,
            border_color: t.colors().border.clone(),
            font_size: t.typography().size_md,
        }
        variant size {
            sm(t) {
                padding_vertical: t.spacing().xs,
                padding_horizontal: t.spacing().sm,
                font_size: t.typography().size_sm,
            }
            #[default]
            md(t) {
                padding_vertical: t.spacing().sm,
                padding_horizontal: t.spacing().md,
                font_size: t.typography().size_md,
            }
            lg(t) {
                padding_vertical: t.spacing().md,
                padding_horizontal: t.spacing().lg,
                font_size: t.typography().size_lg,
            }
        }
        variant tone {
            #[default]
            default(_t) {}
            error(t) {
                border_color: t.colors().danger.clone(),
            }
        }
        state focused(t) {
            border_color: t.colors().focus_ring.clone(),
        }
        state disabled(_t) {
            opacity: 0.55,
        }
        transitions {
            border_color: 150ms EaseOut,
            background: 250ms EaseInOut,
            color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub FieldGroup<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: Length::Px(t.spacing().xs),
        }
    }
}

stylesheet! {
    pub FieldLabel<IdeaThemeRef> {
        base(t) {
            color: t.colors().text.clone(),
            font_size: t.typography().size_sm,
            font_weight: FontWeight::Medium,
        }
    }
}

stylesheet! {
    pub FieldHelp<IdeaThemeRef> {
        base(t) {
            color: t.colors().text_muted.clone(),
            font_size: t.typography().size_sm,
        }
        variant tone {
            #[default]
            default(t) { color: t.colors().text_muted.clone() }
            error(t)   { color: t.colors().danger.clone() }
        }
    }
}

// =============================================================================
// Divider
// =============================================================================

stylesheet! {
    pub Divider<IdeaThemeRef> {
        base(t) {
            background: t.colors().border.clone(),
            height: 1.0,
            width: Length::pct(100.0),
        }
        variant axis {
            #[default]
            horizontal(_t) {
                height: 1.0,
                width: Length::pct(100.0),
            }
            // Vertical dividers fill their parent's cross axis via
            // `align_self: stretch` (so a vertical divider inside a
            // flex-row container stretches to the row's height).
            // `min_height` provides a sensible fallback when the
            // parent doesn't have a definite height — without it,
            // `height: 100%` resolves to 0 and the divider becomes
            // invisible.
            vertical(_t) {
                width: 1.0,
                height: Length::pct(100.0),
                min_height: 24.0,
                align_self: framework_core::AlignSelf::Stretch,
            }
        }
        transitions {
            background: 250ms EaseInOut,
        }
    }
}

// =============================================================================
// Badge
// =============================================================================
//
// Like Pressable, Badge's coloring is driven by an Intent applied as
// overrides. The stylesheet handles the shape: padding, radius,
// typography. Background/color come from the intent.

stylesheet! {
    pub Badge<IdeaThemeRef> {
        base(t) {
            padding_vertical: 2.0,
            padding_horizontal: t.spacing().sm,
            border_radius: t.radius().pill,
            font_size: t.typography().size_xs,
            font_weight: FontWeight::SemiBold,
            letter_spacing: 0.4,
            text_transform: TextTransform::Uppercase,
            text_align: TextAlign::Center,
        }
    }
}

// =============================================================================
// Switch row — packs label + Toggle.
// =============================================================================

stylesheet! {
    pub SwitchRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: Length::Px(t.spacing().sm),
        }
    }
}

// =============================================================================
// Select — trigger + menu surfaces
// =============================================================================
//
// `SelectTrigger` is the always-visible button. Mirrors Field's
// shape (background / border / size variants) so a Select sits
// visually next to a Field without juddering.
//
// `SelectMenu` is the popover panel rendered inside an Overlay.
// `SelectOption` styles each row in the menu, with an `active`
// variant that highlights the currently-selected option.

stylesheet! {
    pub SelectTrigger<IdeaThemeRef> {
        base(t) {
            background: t.colors().surface.clone(),
            color: t.colors().text.clone(),
            padding_vertical: t.spacing().sm,
            padding_horizontal: t.spacing().md,
            border_radius: t.radius().md,
            border_width: 1.0,
            border_color: t.colors().border.clone(),
            font_size: t.typography().size_md,
            text_align: TextAlign::Left,
            min_width: 160.0,
        }
        variant size {
            sm(t) {
                padding_vertical: t.spacing().xs,
                padding_horizontal: t.spacing().sm,
                font_size: t.typography().size_sm,
            }
            #[default]
            md(t) {
                padding_vertical: t.spacing().sm,
                padding_horizontal: t.spacing().md,
                font_size: t.typography().size_md,
            }
            lg(t) {
                padding_vertical: t.spacing().md,
                padding_horizontal: t.spacing().lg,
                font_size: t.typography().size_lg,
            }
        }
        state hovered(t) {
            border_color: t.colors().border_hover.clone(),
        }
        state disabled(_t) {
            opacity: 0.55,
        }
        transitions {
            background: 250ms EaseInOut,
            border_color: 150ms EaseOut,
            color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub SelectMenu<IdeaThemeRef> {
        base(t) {
            background: t.colors().surface.clone(),
            border_radius: t.radius().md,
            border_width: 1.0,
            border_color: t.colors().border.clone(),
            padding: t.spacing().xs,
            gap: Length::Px(2.0),
            flex_direction: FlexDirection::Column,
            min_width: 200.0,
            shadow: framework_core::Shadow {
                x: 0.0,
                y: 8.0,
                blur: 24.0,
                color: framework_core::Color("rgba(15, 17, 21, 0.18)".into()),
            },
        }
        transitions {
            background: 250ms EaseInOut,
            border_color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub SelectOption<IdeaThemeRef> {
        base(t) {
            background: Color("transparent".into()),
            color: t.colors().text.clone(),
            padding_vertical: t.spacing().xs,
            padding_horizontal: t.spacing().sm,
            border_radius: t.radius().sm,
            font_size: t.typography().size_md,
            text_align: TextAlign::Left,
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
            background: t.colors().surface_alt.clone(),
        }
        transitions {
            background: 150ms EaseOut,
            color: 150ms EaseOut,
        }
    }
}

// =============================================================================
// Spacer — grow to fill.
// =============================================================================

stylesheet! {
    pub Spacer<IdeaThemeRef> {
        base(_t) {
            flex_grow: 1.0,
        }
    }
}

// =============================================================================
// Center — align/justify both axes.
// =============================================================================

stylesheet! {
    pub Center<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
        }
    }
}

// =============================================================================
// IconButton — square, content-sized variant of Pressable.
// =============================================================================

stylesheet! {
    pub IconButton<IdeaThemeRef> {
        base(t) {
            padding: t.spacing().sm,
            border_radius: t.radius().pill,
            font_size: t.typography().size_md,
            font_weight: FontWeight::SemiBold,
            text_align: TextAlign::Center,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
        }
        variant size {
            sm(t) {
                padding: t.spacing().xs,
                font_size: t.typography().size_sm,
                width: t.spacing().xl,
                height: t.spacing().xl,
            }
            #[default]
            md(t) {
                padding: t.spacing().sm,
                font_size: t.typography().size_md,
                width: t.spacing().xxl,
                height: t.spacing().xxl,
            }
            lg(t) {
                padding: t.spacing().md,
                font_size: t.typography().size_lg,
                width: 48.0,
                height: 48.0,
            }
        }
        state disabled(_t) {
            opacity: 0.45,
        }
        transitions {
            background: 150ms EaseOut,
            color: 200ms EaseOut,
            opacity: 200ms EaseOut,
            border_color: 150ms EaseOut,
        }
    }
}

// =============================================================================
// Avatar — circular container + text overlay.
// =============================================================================

stylesheet! {
    pub Avatar<IdeaThemeRef> {
        base(t) {
            border_radius: t.radius().pill,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            overflow: framework_core::Overflow::Hidden,
        }
        variant size {
            xs(_t) { width: 24.0, height: 24.0 }
            sm(_t) { width: 32.0, height: 32.0 }
            #[default]
            md(_t) { width: 40.0, height: 40.0 }
            lg(_t) { width: 56.0, height: 56.0 }
            xl(_t) { width: 80.0, height: 80.0 }
        }
        transitions {
            background: 250ms EaseInOut,
            color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub AvatarText<IdeaThemeRef> {
        base(_t) {
            font_weight: FontWeight::SemiBold,
            text_align: TextAlign::Center,
            letter_spacing: 0.5,
            text_transform: TextTransform::Uppercase,
        }
        variant size {
            xs(_t) { font_size: 10.0, line_height: 24.0 }
            sm(_t) { font_size: 12.0, line_height: 32.0 }
            #[default]
            md(_t) { font_size: 14.0, line_height: 40.0 }
            lg(_t) { font_size: 20.0, line_height: 56.0 }
            xl(_t) { font_size: 28.0, line_height: 80.0 }
        }
    }
}

// =============================================================================
// Tag — pill container with optional close button.
// =============================================================================

stylesheet! {
    pub Tag<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: Length::Px(t.spacing().xs),
            padding_vertical: t.spacing().xs,
            padding_horizontal: t.spacing().sm,
            border_radius: t.radius().pill,
        }
        transitions {
            background: 250ms EaseInOut,
            color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub TagLabel<IdeaThemeRef> {
        base(t) {
            font_size: t.typography().size_sm,
            font_weight: FontWeight::SemiBold,
            letter_spacing: 0.3,
        }
    }
}

stylesheet! {
    pub TagClose<IdeaThemeRef> {
        base(_t) {
            // Inherit the parent's foreground; no fill of its own.
            background: Color("transparent".into()),
            padding: 0.0,
            font_size: 14.0,
            font_weight: FontWeight::Bold,
            text_align: TextAlign::Center,
            line_height: 14.0,
            width: 16.0,
            height: 16.0,
            border_radius: 999.0,
        }
        transitions {
            background: 150ms EaseOut,
            opacity: 150ms EaseOut,
        }
    }
}

// =============================================================================
// Alert — full-width banner with title + body + dismiss.
// =============================================================================

stylesheet! {
    pub Alert<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::FlexStart,
            gap: Length::Px(t.spacing().md),
            padding: t.spacing().lg,
            border_radius: t.radius().md,
        }
        transitions {
            background: 250ms EaseInOut,
            color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub AlertTitle<IdeaThemeRef> {
        base(t) {
            font_size: t.typography().size_md,
            font_weight: FontWeight::SemiBold,
            line_height: 20.0,
        }
    }
}

stylesheet! {
    pub AlertBody<IdeaThemeRef> {
        base(t) {
            font_size: t.typography().size_sm,
            line_height: 18.0,
        }
    }
}

// =============================================================================
// Skeleton — muted placeholder block.
// =============================================================================

stylesheet! {
    pub Skeleton<IdeaThemeRef> {
        base(t) {
            background: t.colors().surface_alt.clone(),
        }
        transitions {
            background: 250ms EaseInOut,
        }
    }
}

// =============================================================================
// Tabs — horizontal tab bar + content panel.
// =============================================================================
//
// `TabBar` is the row holding tab buttons. The active button gets
// the `on` variant on the `active` axis — that styles its background
// + foreground to look selected. `TabPanel` is the content area
// below the bar; padding sits there, not on the bar, so the active
// row sits flush with the bar's bottom border.

stylesheet! {
    pub TabBar<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            gap: Length::Px(t.spacing().xs),
            border_bottom_width: 1.0,
            border_bottom_color: t.colors().border.clone(),
        }
        transitions {
            border_bottom_color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub TabButton<IdeaThemeRef> {
        base(t) {
            background: Color("transparent".into()),
            color: t.colors().text_muted.clone(),
            padding_vertical: t.spacing().sm,
            padding_horizontal: t.spacing().md,
            font_weight: FontWeight::Medium,
            font_size: t.typography().size_md,
            border_radius: 0.0,
            // Bottom border draws under the active tab to mark
            // selection; off-state is transparent so the bar's
            // own bottom border shows through.
            border_bottom_width: 2.0,
            border_bottom_color: Color("transparent".into()),
        }
        variant active {
            #[default]
            off(_t) {}
            on(t) {
                color: t.colors().text.clone(),
                border_bottom_color: t.colors().primary.clone(),
            }
        }
        state hovered(t) {
            color: t.colors().text.clone(),
        }
        transitions {
            color: 150ms EaseOut,
            border_bottom_color: 200ms EaseOut,
        }
    }
}

stylesheet! {
    pub TabPanel<IdeaThemeRef> {
        base(t) {
            padding_vertical: t.spacing().lg,
            flex_direction: FlexDirection::Column,
            gap: Length::Px(t.spacing().md),
        }
    }
}

// =============================================================================
// Modal / Popover — overlay content surfaces.
// =============================================================================
//
// These style the inner content container of an Overlay, not the
// overlay itself (which is positioned by the framework). Modal is
// the card-like centered surface; Popover is a smaller floating
// panel anchored to a trigger.

stylesheet! {
    pub Modal<IdeaThemeRef> {
        base(t) {
            background: t.colors().surface.clone(),
            padding: t.spacing().lg,
            border_radius: t.radius().lg,
            border_width: 1.0,
            border_color: t.colors().border.clone(),
            gap: Length::Px(t.spacing().md),
            flex_direction: FlexDirection::Column,
            min_width: 320.0,
            max_width: 560.0,
            shadow: framework_core::Shadow {
                x: 0.0,
                y: 12.0,
                blur: 32.0,
                color: Color("rgba(15, 17, 21, 0.25)".into()),
            },
        }
        transitions {
            background: 250ms EaseInOut,
            border_color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub Popover<IdeaThemeRef> {
        base(t) {
            background: t.colors().surface.clone(),
            padding: t.spacing().sm,
            border_radius: t.radius().md,
            border_width: 1.0,
            border_color: t.colors().border.clone(),
            gap: Length::Px(t.spacing().xs),
            flex_direction: FlexDirection::Column,
            min_width: 180.0,
            shadow: framework_core::Shadow {
                x: 0.0,
                y: 6.0,
                blur: 18.0,
                color: Color("rgba(15, 17, 21, 0.18)".into()),
            },
        }
        transitions {
            background: 250ms EaseInOut,
            border_color: 250ms EaseInOut,
        }
    }
}
