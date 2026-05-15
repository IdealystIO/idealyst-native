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
// Layout — Stack / HStack / VStack
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
            color: Color(t.colors().text.clone()),
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
            color: Color(t.colors().text.clone()),
            font_size: t.typography().size_md,
            line_height: 20.0,
        }
        variant tone {
            #[default]
            default(_t) {}
            muted(t)    { color: Color(t.colors().text_muted.clone()) }
            primary(t)  { color: Color(t.colors().primary.clone()) }
            danger(t)   { color: Color(t.colors().danger.clone()) }
            success(t)  { color: Color(t.colors().success.clone()) }
            warning(t)  { color: Color(t.colors().warning.clone()) }
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
            color: Color(t.colors().text_muted.clone()),
            font_size: t.typography().size_sm,
            line_height: 16.0,
        }
        variant tone {
            #[default]
            default(t) { color: Color(t.colors().text_muted.clone()) }
            danger(t)  { color: Color(t.colors().danger.clone()) }
            success(t) { color: Color(t.colors().success.clone()) }
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
            background: Color(t.colors().surface.clone()),
            padding: t.spacing().lg,
            border_radius: t.radius().lg,
            gap: Length::Px(t.spacing().sm),
            border_width: 1.0,
            border_color: Color(t.colors().border.clone()),
        }
        variant tone {
            #[default]
            surface(t) {
                background: Color(t.colors().surface.clone()),
            }
            elevated(t) {
                background: Color(t.colors().surface.clone()),
                shadow: framework_core::Shadow {
                    x: 0.0,
                    y: 4.0,
                    blur: 16.0,
                    color: Color("rgba(15, 17, 21, 0.10)".into()),
                },
            }
            primary(t) {
                background: Color(t.colors().primary.clone()),
                color: Color(t.colors().primary_text.clone()),
                border_color: Color(t.colors().primary.clone()),
            }
            muted(t) {
                background: Color(t.colors().surface_alt.clone()),
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
            background: Color(t.colors().surface.clone()),
            color: Color(t.colors().text.clone()),
            padding_vertical: t.spacing().sm,
            padding_horizontal: t.spacing().md,
            border_radius: t.radius().md,
            border_width: 1.0,
            border_color: Color(t.colors().border.clone()),
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
                border_color: Color(t.colors().danger.clone()),
            }
        }
        state focused(t) {
            border_color: Color(t.colors().focus_ring.clone()),
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
            color: Color(t.colors().text.clone()),
            font_size: t.typography().size_sm,
            font_weight: FontWeight::Medium,
        }
    }
}

stylesheet! {
    pub FieldHelp<IdeaThemeRef> {
        base(t) {
            color: Color(t.colors().text_muted.clone()),
            font_size: t.typography().size_sm,
        }
        variant tone {
            #[default]
            default(t) { color: Color(t.colors().text_muted.clone()) }
            error(t)   { color: Color(t.colors().danger.clone()) }
        }
    }
}

// =============================================================================
// Divider
// =============================================================================

stylesheet! {
    pub Divider<IdeaThemeRef> {
        base(t) {
            background: Color(t.colors().border.clone()),
            height: 1.0,
            width: Length::pct(100.0),
        }
        variant axis {
            #[default]
            horizontal(_t) {
                height: 1.0,
                width: Length::pct(100.0),
            }
            vertical(_t) {
                width: 1.0,
                height: Length::pct(100.0),
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
