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

use runtime_core::stylesheet;
use runtime_core::{
    AlignItems, Color, FlexDirection, FontWeight, JustifyContent, Length, TextAlign, TextTransform,
    Tokenized,
};

#[allow(unused_imports)]
use crate::theme::{IdeaTheme, IdeaThemeRef};

// =============================================================================
// Layout — Stack
// =============================================================================

stylesheet! {
    pub Stack<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: Tokenized::token("spacing-md", Length::Px(12.0)),
        }
        variant gap {
            none(_t) { gap: Length::Px(0.0) }
            xs(t)    { gap: Tokenized::token("spacing-xs", Length::Px(4.0)) }
            sm(t)    { gap: Tokenized::token("spacing-sm", Length::Px(8.0)) }
            #[default]
            md(t)    { gap: Tokenized::token("spacing-md", Length::Px(12.0)) }
            lg(t)    { gap: Tokenized::token("spacing-lg", Length::Px(16.0)) }
            xl(t)    { gap: Tokenized::token("spacing-xl", Length::Px(24.0)) }
        }
        variant padding {
            #[default]
            none(_t) { padding: Length::Px(0.0) }
            xs(t)    { padding: Tokenized::token("spacing-xs", Length::Px(4.0)) }
            sm(t)    { padding: Tokenized::token("spacing-sm", Length::Px(8.0)) }
            md(t)    { padding: Tokenized::token("spacing-md", Length::Px(12.0)) }
            lg(t)    { padding: Tokenized::token("spacing-lg", Length::Px(16.0)) }
            xl(t)    { padding: Tokenized::token("spacing-xl", Length::Px(24.0)) }
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
// Button — the styled clickable.
// =============================================================================
//
// Visual is driven by an `appearance` variant axis that encodes
// (intent × kind) — 7 intents × 4 kinds = 28 arms. Each arm sets the
// base background / text / border for the (intent, kind) pair.
//
// Hover and pressed feedback are uniform across appearances: a subtle
// opacity dim. (A future framework feature for per-state property
// overrides will let us shift colors per-state instead; the opacity
// dim is the v1 placeholder.)
//
// The Button component never speaks the appearance variant directly;
// it takes `intent` + `kind` props and joins them with an `_` to
// produce the appearance key (e.g. `(Danger, Outlined) → "danger_outlined"`).

stylesheet! {
    pub Button<IdeaThemeRef> {
        base(t) {
            padding_vertical: Tokenized::token("spacing-sm", Length::Px(8.0)),
            padding_horizontal: Tokenized::token("spacing-lg", Length::Px(16.0)),
            border_radius: Tokenized::token("radius-md", Length::Px(8.0)),
            font_weight: FontWeight::SemiBold,
            font_size: Tokenized::token("typography-size-md", Length::Px(14.0)),
            text_align: TextAlign::Center,
            letter_spacing: 0.2,
        }
        variant size {
            sm(t) {
                padding_vertical: Tokenized::token("spacing-xs", Length::Px(4.0)),
                padding_horizontal: Tokenized::token("spacing-md", Length::Px(12.0)),
                font_size: Tokenized::token("typography-size-sm", Length::Px(12.0)),
            }
            #[default]
            md(t) {
                padding_vertical: Tokenized::token("spacing-sm", Length::Px(8.0)),
                padding_horizontal: Tokenized::token("spacing-lg", Length::Px(16.0)),
                font_size: Tokenized::token("typography-size-md", Length::Px(14.0)),
            }
            lg(t) {
                padding_vertical: Tokenized::token("spacing-md", Length::Px(12.0)),
                padding_horizontal: Tokenized::token("spacing-xl", Length::Px(24.0)),
                font_size: Tokenized::token("typography-size-lg", Length::Px(16.0)),
            }
        }
        variant appearance {
            #[default]
            primary_solid(t) {
                background: Tokenized::token("intent-primary-solid-bg", Color("#5b6cff".into())),
                color: Tokenized::token("intent-primary-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            primary_soft(t) {
                background: Tokenized::token("intent-primary-soft-bg", Color("rgba(91, 108, 255, 0.12)".into())),
                color: Tokenized::token("intent-primary-soft-text", Color("#3947d6".into())),
                border_width: 0.0,
            }
            primary_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-primary-fg", Color("#3947d6".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-primary-border", Color("#5b6cff".into())),
            }
            primary_ghost(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-primary-fg", Color("#3947d6".into())),
                border_width: 0.0,
            }
            secondary_solid(t) {
                background: Tokenized::token("intent-secondary-solid-bg", Color("#475569".into())),
                color: Tokenized::token("intent-secondary-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            secondary_soft(t) {
                background: Tokenized::token("intent-secondary-soft-bg", Color("rgba(71, 85, 105, 0.10)".into())),
                color: Tokenized::token("intent-secondary-soft-text", Color("#334155".into())),
                border_width: 0.0,
            }
            secondary_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-secondary-fg", Color("#334155".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-secondary-border", Color("#475569".into())),
            }
            secondary_ghost(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-secondary-fg", Color("#334155".into())),
                border_width: 0.0,
            }
            neutral_solid(t) {
                background: Tokenized::token("intent-neutral-solid-bg", Color("#1a1a1f".into())),
                color: Tokenized::token("intent-neutral-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            neutral_soft(t) {
                background: Tokenized::token("intent-neutral-soft-bg", Color("#eef0f7".into())),
                color: Tokenized::token("intent-neutral-soft-text", Color("#1a1a1f".into())),
                border_width: 0.0,
            }
            neutral_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-neutral-fg", Color("#1a1a1f".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-neutral-border", Color("#cbd0db".into())),
            }
            neutral_ghost(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-neutral-fg", Color("#1a1a1f".into())),
                border_width: 0.0,
            }
            success_solid(t) {
                background: Tokenized::token("intent-success-solid-bg", Color("#16a34a".into())),
                color: Tokenized::token("intent-success-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            success_soft(t) {
                background: Tokenized::token("intent-success-soft-bg", Color("rgba(22, 163, 74, 0.12)".into())),
                color: Tokenized::token("intent-success-soft-text", Color("#107a37".into())),
                border_width: 0.0,
            }
            success_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-success-fg", Color("#107a37".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-success-border", Color("#16a34a".into())),
            }
            success_ghost(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-success-fg", Color("#107a37".into())),
                border_width: 0.0,
            }
            danger_solid(t) {
                background: Tokenized::token("intent-danger-solid-bg", Color("#dc2626".into())),
                color: Tokenized::token("intent-danger-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            danger_soft(t) {
                background: Tokenized::token("intent-danger-soft-bg", Color("rgba(220, 38, 38, 0.10)".into())),
                color: Tokenized::token("intent-danger-soft-text", Color("#b91c1c".into())),
                border_width: 0.0,
            }
            danger_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-danger-fg", Color("#b91c1c".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-danger-border", Color("#dc2626".into())),
            }
            danger_ghost(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-danger-fg", Color("#b91c1c".into())),
                border_width: 0.0,
            }
            warning_solid(t) {
                background: Tokenized::token("intent-warning-solid-bg", Color("#d97706".into())),
                color: Tokenized::token("intent-warning-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            warning_soft(t) {
                background: Tokenized::token("intent-warning-soft-bg", Color("rgba(217, 119, 6, 0.12)".into())),
                color: Tokenized::token("intent-warning-soft-text", Color("#b45309".into())),
                border_width: 0.0,
            }
            warning_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-warning-fg", Color("#b45309".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-warning-border", Color("#d97706".into())),
            }
            warning_ghost(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-warning-fg", Color("#b45309".into())),
                border_width: 0.0,
            }
            info_solid(t) {
                background: Tokenized::token("intent-info-solid-bg", Color("#0284c7".into())),
                color: Tokenized::token("intent-info-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            info_soft(t) {
                background: Tokenized::token("intent-info-soft-bg", Color("rgba(2, 132, 199, 0.12)".into())),
                color: Tokenized::token("intent-info-soft-text", Color("#075985".into())),
                border_width: 0.0,
            }
            info_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-info-fg", Color("#075985".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-info-border", Color("#0284c7".into())),
            }
            info_ghost(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-info-fg", Color("#075985".into())),
                border_width: 0.0,
            }
        }
        state hovered(_t) {
            opacity: 0.92,
        }
        state pressed(_t) {
            opacity: 0.85,
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
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
            font_weight: FontWeight::SemiBold,
            letter_spacing: -0.3,
        }
        variant kind {
            display(t) {
                font_size: Tokenized::token("typography-size-display", Length::Px(36.0)),
                font_weight: FontWeight::Bold,
                letter_spacing: -1.0,
                line_height: 42.0,
            }
            #[default]
            h1(t) {
                font_size: Tokenized::token("typography-size-xxl", Length::Px(28.0)),
                line_height: 34.0,
            }
            h2(t) {
                font_size: Tokenized::token("typography-size-xl", Length::Px(20.0)),
                line_height: 26.0,
            }
            h3(t) {
                font_size: Tokenized::token("typography-size-lg", Length::Px(16.0)),
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
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
            font_size: Tokenized::token("typography-size-md", Length::Px(14.0)),
            line_height: 20.0,
        }
        variant tone {
            #[default]
            default(_t) {}
            muted(t)    { color: Tokenized::token("color-text-muted", Color("#6b7280".into())) }
            primary(t)  { color: Tokenized::token("intent-primary-fg", Color("#3947d6".into())) }
            danger(t)   { color: Tokenized::token("intent-danger-fg", Color("#b91c1c".into())) }
            success(t)  { color: Tokenized::token("intent-success-fg", Color("#107a37".into())) }
            warning(t)  { color: Tokenized::token("intent-warning-fg", Color("#b45309".into())) }
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
            color: Tokenized::token("color-text-muted", Color("#6b7280".into())),
            font_size: Tokenized::token("typography-size-sm", Length::Px(12.0)),
            line_height: 16.0,
        }
        variant tone {
            #[default]
            default(t) { color: Tokenized::token("color-text-muted", Color("#6b7280".into())) }
            danger(t)  { color: Tokenized::token("intent-danger-fg", Color("#b91c1c".into())) }
            success(t) { color: Tokenized::token("intent-success-fg", Color("#107a37".into())) }
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
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            padding: Tokenized::token("spacing-lg", Length::Px(16.0)),
            border_radius: Tokenized::token("radius-lg", Length::Px(12.0)),
            gap: Tokenized::token("spacing-sm", Length::Px(8.0)),
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
        }
        variant tone {
            #[default]
            surface(t) {
                background: Tokenized::token("color-surface", Color("#ffffff".into())),
            }
            elevated(t) {
                background: Tokenized::token("color-surface", Color("#ffffff".into())),
                shadow: runtime_core::Shadow {
                    x: 0.0,
                    y: 4.0,
                    blur: 16.0,
                    color: Color("rgba(15, 17, 21, 0.10)".into()),
                },
            }
            primary(t) {
                background: Tokenized::token("intent-primary-solid-bg", Color("#5b6cff".into())),
                color: Tokenized::token("intent-primary-solid-text", Color("#ffffff".into())),
                border_color: Tokenized::token("intent-primary-solid-bg", Color("#5b6cff".into())),
            }
            muted(t) {
                background: Tokenized::token("color-surface-alt", Color("#eef0f7".into())),
            }
        }
        variant padding {
            none(_t) { padding: 0.0 }
            sm(t)    { padding: Tokenized::token("spacing-sm", Length::Px(8.0)) }
            #[default]
            md(t)    { padding: Tokenized::token("spacing-lg", Length::Px(16.0)) }
            lg(t)    { padding: Tokenized::token("spacing-xl", Length::Px(24.0)) }
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
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
            padding_vertical: Tokenized::token("spacing-sm", Length::Px(8.0)),
            padding_horizontal: Tokenized::token("spacing-md", Length::Px(12.0)),
            border_radius: Tokenized::token("radius-md", Length::Px(8.0)),
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            font_size: Tokenized::token("typography-size-md", Length::Px(14.0)),
        }
        variant size {
            sm(t) {
                padding_vertical: Tokenized::token("spacing-xs", Length::Px(4.0)),
                padding_horizontal: Tokenized::token("spacing-sm", Length::Px(8.0)),
                font_size: Tokenized::token("typography-size-sm", Length::Px(12.0)),
            }
            #[default]
            md(t) {
                padding_vertical: Tokenized::token("spacing-sm", Length::Px(8.0)),
                padding_horizontal: Tokenized::token("spacing-md", Length::Px(12.0)),
                font_size: Tokenized::token("typography-size-md", Length::Px(14.0)),
            }
            lg(t) {
                padding_vertical: Tokenized::token("spacing-md", Length::Px(12.0)),
                padding_horizontal: Tokenized::token("spacing-lg", Length::Px(16.0)),
                font_size: Tokenized::token("typography-size-lg", Length::Px(16.0)),
            }
        }
        variant tone {
            #[default]
            default(_t) {}
            error(t) {
                border_color: Tokenized::token("intent-danger-solid-bg", Color("#dc2626".into())),
            }
        }
        state focused(t) {
            border_color: Tokenized::token("color-focus-ring", Color("#5b6cff".into())),
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
            gap: Tokenized::token("spacing-xs", Length::Px(4.0)),
        }
    }
}

stylesheet! {
    pub FieldLabel<IdeaThemeRef> {
        base(t) {
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
            font_size: Tokenized::token("typography-size-sm", Length::Px(12.0)),
            font_weight: FontWeight::Medium,
        }
    }
}

stylesheet! {
    pub FieldHelp<IdeaThemeRef> {
        base(t) {
            color: Tokenized::token("color-text-muted", Color("#6b7280".into())),
            font_size: Tokenized::token("typography-size-sm", Length::Px(12.0)),
        }
        variant tone {
            #[default]
            default(t) { color: Tokenized::token("color-text-muted", Color("#6b7280".into())) }
            error(t)   { color: Tokenized::token("intent-danger-fg", Color("#b91c1c".into())) }
        }
    }
}

// =============================================================================
// Divider
// =============================================================================

stylesheet! {
    pub Divider<IdeaThemeRef> {
        base(t) {
            background: Tokenized::token("color-border", Color("#e4e6ef".into())),
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
                align_self: runtime_core::AlignSelf::Stretch,
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
// Same intent × kind appearance model as Button, but only three kinds
// (Solid / Soft / Outlined — no Ghost, since a badge needs a visible
// surface to read as a chip).

stylesheet! {
    pub Badge<IdeaThemeRef> {
        base(t) {
            padding_vertical: 2.0,
            padding_horizontal: Tokenized::token("spacing-sm", Length::Px(8.0)),
            border_radius: Tokenized::token("radius-pill", Length::Px(999.0)),
            font_size: Tokenized::token("typography-size-xs", Length::Px(11.0)),
            font_weight: FontWeight::SemiBold,
            letter_spacing: 0.4,
            text_transform: TextTransform::Uppercase,
            text_align: TextAlign::Center,
        }
        variant appearance {
            primary_solid(t) {
                background: Tokenized::token("intent-primary-solid-bg", Color("#5b6cff".into())),
                color: Tokenized::token("intent-primary-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            primary_soft(t) {
                background: Tokenized::token("intent-primary-soft-bg", Color("rgba(91, 108, 255, 0.12)".into())),
                color: Tokenized::token("intent-primary-soft-text", Color("#3947d6".into())),
                border_width: 0.0,
            }
            primary_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-primary-fg", Color("#3947d6".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-primary-border", Color("#5b6cff".into())),
            }
            secondary_solid(t) {
                background: Tokenized::token("intent-secondary-solid-bg", Color("#475569".into())),
                color: Tokenized::token("intent-secondary-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            secondary_soft(t) {
                background: Tokenized::token("intent-secondary-soft-bg", Color("rgba(71, 85, 105, 0.10)".into())),
                color: Tokenized::token("intent-secondary-soft-text", Color("#334155".into())),
                border_width: 0.0,
            }
            secondary_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-secondary-fg", Color("#334155".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-secondary-border", Color("#475569".into())),
            }
            neutral_solid(t) {
                background: Tokenized::token("intent-neutral-solid-bg", Color("#1a1a1f".into())),
                color: Tokenized::token("intent-neutral-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            #[default]
            neutral_soft(t) {
                background: Tokenized::token("intent-neutral-soft-bg", Color("#eef0f7".into())),
                color: Tokenized::token("intent-neutral-soft-text", Color("#1a1a1f".into())),
                border_width: 0.0,
            }
            neutral_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-neutral-fg", Color("#1a1a1f".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-neutral-border", Color("#cbd0db".into())),
            }
            success_solid(t) {
                background: Tokenized::token("intent-success-solid-bg", Color("#16a34a".into())),
                color: Tokenized::token("intent-success-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            success_soft(t) {
                background: Tokenized::token("intent-success-soft-bg", Color("rgba(22, 163, 74, 0.12)".into())),
                color: Tokenized::token("intent-success-soft-text", Color("#107a37".into())),
                border_width: 0.0,
            }
            success_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-success-fg", Color("#107a37".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-success-border", Color("#16a34a".into())),
            }
            danger_solid(t) {
                background: Tokenized::token("intent-danger-solid-bg", Color("#dc2626".into())),
                color: Tokenized::token("intent-danger-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            danger_soft(t) {
                background: Tokenized::token("intent-danger-soft-bg", Color("rgba(220, 38, 38, 0.10)".into())),
                color: Tokenized::token("intent-danger-soft-text", Color("#b91c1c".into())),
                border_width: 0.0,
            }
            danger_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-danger-fg", Color("#b91c1c".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-danger-border", Color("#dc2626".into())),
            }
            warning_solid(t) {
                background: Tokenized::token("intent-warning-solid-bg", Color("#d97706".into())),
                color: Tokenized::token("intent-warning-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            warning_soft(t) {
                background: Tokenized::token("intent-warning-soft-bg", Color("rgba(217, 119, 6, 0.12)".into())),
                color: Tokenized::token("intent-warning-soft-text", Color("#b45309".into())),
                border_width: 0.0,
            }
            warning_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-warning-fg", Color("#b45309".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-warning-border", Color("#d97706".into())),
            }
            info_solid(t) {
                background: Tokenized::token("intent-info-solid-bg", Color("#0284c7".into())),
                color: Tokenized::token("intent-info-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            info_soft(t) {
                background: Tokenized::token("intent-info-soft-bg", Color("rgba(2, 132, 199, 0.12)".into())),
                color: Tokenized::token("intent-info-soft-text", Color("#075985".into())),
                border_width: 0.0,
            }
            info_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-info-fg", Color("#075985".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-info-border", Color("#0284c7".into())),
            }
        }
        transitions {
            background: 250ms EaseInOut,
            color: 250ms EaseInOut,
            border_color: 250ms EaseInOut,
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
            gap: Tokenized::token("spacing-sm", Length::Px(8.0)),
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
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
            padding_vertical: Tokenized::token("spacing-sm", Length::Px(8.0)),
            padding_horizontal: Tokenized::token("spacing-md", Length::Px(12.0)),
            border_radius: Tokenized::token("radius-md", Length::Px(8.0)),
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            font_size: Tokenized::token("typography-size-md", Length::Px(14.0)),
            text_align: TextAlign::Left,
            min_width: 160.0,
        }
        variant size {
            sm(t) {
                padding_vertical: Tokenized::token("spacing-xs", Length::Px(4.0)),
                padding_horizontal: Tokenized::token("spacing-sm", Length::Px(8.0)),
                font_size: Tokenized::token("typography-size-sm", Length::Px(12.0)),
            }
            #[default]
            md(t) {
                padding_vertical: Tokenized::token("spacing-sm", Length::Px(8.0)),
                padding_horizontal: Tokenized::token("spacing-md", Length::Px(12.0)),
                font_size: Tokenized::token("typography-size-md", Length::Px(14.0)),
            }
            lg(t) {
                padding_vertical: Tokenized::token("spacing-md", Length::Px(12.0)),
                padding_horizontal: Tokenized::token("spacing-lg", Length::Px(16.0)),
                font_size: Tokenized::token("typography-size-lg", Length::Px(16.0)),
            }
        }
        state hovered(t) {
            border_color: Tokenized::token("color-border-hover", Color("#b9bdcc".into())),
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
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            border_radius: Tokenized::token("radius-md", Length::Px(8.0)),
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            padding: Tokenized::token("spacing-xs", Length::Px(4.0)),
            gap: Length::Px(2.0),
            flex_direction: FlexDirection::Column,
            min_width: 200.0,
            shadow: runtime_core::Shadow {
                x: 0.0,
                y: 8.0,
                blur: 24.0,
                color: runtime_core::Color("rgba(15, 17, 21, 0.18)".into()),
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
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
            padding_vertical: Tokenized::token("spacing-xs", Length::Px(4.0)),
            padding_horizontal: Tokenized::token("spacing-sm", Length::Px(8.0)),
            border_radius: Tokenized::token("radius-sm", Length::Px(4.0)),
            font_size: Tokenized::token("typography-size-md", Length::Px(14.0)),
            text_align: TextAlign::Left,
        }
        variant active {
            #[default]
            off(_t) {}
            on(t) {
                background: Tokenized::token("intent-primary-solid-bg", Color("#5b6cff".into())),
                color: Tokenized::token("intent-primary-solid-text", Color("#ffffff".into())),
            }
        }
        state hovered(t) {
            background: Tokenized::token("color-surface-alt", Color("#eef0f7".into())),
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
            padding: Tokenized::token("spacing-sm", Length::Px(8.0)),
            border_radius: Tokenized::token("radius-pill", Length::Px(999.0)),
            font_size: Tokenized::token("typography-size-md", Length::Px(14.0)),
            font_weight: FontWeight::SemiBold,
            text_align: TextAlign::Center,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
        }
        variant size {
            sm(t) {
                padding: Tokenized::token("spacing-xs", Length::Px(4.0)),
                font_size: Tokenized::token("typography-size-sm", Length::Px(12.0)),
                width: 24.0,
                height: 24.0,
            }
            #[default]
            md(t) {
                padding: Tokenized::token("spacing-sm", Length::Px(8.0)),
                font_size: Tokenized::token("typography-size-md", Length::Px(14.0)),
                width: 32.0,
                height: 32.0,
            }
            lg(t) {
                padding: Tokenized::token("spacing-md", Length::Px(12.0)),
                font_size: Tokenized::token("typography-size-lg", Length::Px(16.0)),
                width: 48.0,
                height: 48.0,
            }
        }
        // Identical `appearance` axis as Button — same 7 intents × 4 kinds.
        variant appearance {
            primary_solid(t) {
                background: Tokenized::token("intent-primary-solid-bg", Color("#5b6cff".into())),
                color: Tokenized::token("intent-primary-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            primary_soft(t) {
                background: Tokenized::token("intent-primary-soft-bg", Color("rgba(91, 108, 255, 0.12)".into())),
                color: Tokenized::token("intent-primary-soft-text", Color("#3947d6".into())),
                border_width: 0.0,
            }
            primary_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-primary-fg", Color("#3947d6".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-primary-border", Color("#5b6cff".into())),
            }
            primary_ghost(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-primary-fg", Color("#3947d6".into())),
                border_width: 0.0,
            }
            secondary_solid(t) {
                background: Tokenized::token("intent-secondary-solid-bg", Color("#475569".into())),
                color: Tokenized::token("intent-secondary-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            secondary_soft(t) {
                background: Tokenized::token("intent-secondary-soft-bg", Color("rgba(71, 85, 105, 0.10)".into())),
                color: Tokenized::token("intent-secondary-soft-text", Color("#334155".into())),
                border_width: 0.0,
            }
            secondary_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-secondary-fg", Color("#334155".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-secondary-border", Color("#475569".into())),
            }
            secondary_ghost(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-secondary-fg", Color("#334155".into())),
                border_width: 0.0,
            }
            #[default]
            neutral_solid(t) {
                background: Tokenized::token("intent-neutral-solid-bg", Color("#1a1a1f".into())),
                color: Tokenized::token("intent-neutral-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            neutral_soft(t) {
                background: Tokenized::token("intent-neutral-soft-bg", Color("#eef0f7".into())),
                color: Tokenized::token("intent-neutral-soft-text", Color("#1a1a1f".into())),
                border_width: 0.0,
            }
            neutral_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-neutral-fg", Color("#1a1a1f".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-neutral-border", Color("#cbd0db".into())),
            }
            neutral_ghost(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-neutral-fg", Color("#1a1a1f".into())),
                border_width: 0.0,
            }
            success_solid(t) {
                background: Tokenized::token("intent-success-solid-bg", Color("#16a34a".into())),
                color: Tokenized::token("intent-success-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            success_soft(t) {
                background: Tokenized::token("intent-success-soft-bg", Color("rgba(22, 163, 74, 0.12)".into())),
                color: Tokenized::token("intent-success-soft-text", Color("#107a37".into())),
                border_width: 0.0,
            }
            success_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-success-fg", Color("#107a37".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-success-border", Color("#16a34a".into())),
            }
            success_ghost(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-success-fg", Color("#107a37".into())),
                border_width: 0.0,
            }
            danger_solid(t) {
                background: Tokenized::token("intent-danger-solid-bg", Color("#dc2626".into())),
                color: Tokenized::token("intent-danger-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            danger_soft(t) {
                background: Tokenized::token("intent-danger-soft-bg", Color("rgba(220, 38, 38, 0.10)".into())),
                color: Tokenized::token("intent-danger-soft-text", Color("#b91c1c".into())),
                border_width: 0.0,
            }
            danger_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-danger-fg", Color("#b91c1c".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-danger-border", Color("#dc2626".into())),
            }
            danger_ghost(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-danger-fg", Color("#b91c1c".into())),
                border_width: 0.0,
            }
            warning_solid(t) {
                background: Tokenized::token("intent-warning-solid-bg", Color("#d97706".into())),
                color: Tokenized::token("intent-warning-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            warning_soft(t) {
                background: Tokenized::token("intent-warning-soft-bg", Color("rgba(217, 119, 6, 0.12)".into())),
                color: Tokenized::token("intent-warning-soft-text", Color("#b45309".into())),
                border_width: 0.0,
            }
            warning_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-warning-fg", Color("#b45309".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-warning-border", Color("#d97706".into())),
            }
            warning_ghost(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-warning-fg", Color("#b45309".into())),
                border_width: 0.0,
            }
            info_solid(t) {
                background: Tokenized::token("intent-info-solid-bg", Color("#0284c7".into())),
                color: Tokenized::token("intent-info-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            info_soft(t) {
                background: Tokenized::token("intent-info-soft-bg", Color("rgba(2, 132, 199, 0.12)".into())),
                color: Tokenized::token("intent-info-soft-text", Color("#075985".into())),
                border_width: 0.0,
            }
            info_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-info-fg", Color("#075985".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-info-border", Color("#0284c7".into())),
            }
            info_ghost(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-info-fg", Color("#075985".into())),
                border_width: 0.0,
            }
        }
        state hovered(_t) {
            opacity: 0.92,
        }
        state pressed(_t) {
            opacity: 0.85,
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

// Avatar takes a `color` axis (not an intent) — the placeholder
// background uses the named color's soft tint, with the matching
// soft_text on top. Picked separately from Intent because Avatar is
// not a semantic action; it's a person/object placeholder.
stylesheet! {
    pub Avatar<IdeaThemeRef> {
        base(t) {
            border_radius: Tokenized::token("radius-pill", Length::Px(999.0)),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            overflow: runtime_core::Overflow::Hidden,
            // Default to neutral wash so a no-prop Avatar reads as a
            // generic placeholder rather than a colored chip.
            background: Tokenized::token("intent-neutral-soft-bg", Color("#eef0f7".into())),
            color: Tokenized::token("intent-neutral-soft-text", Color("#1a1a1f".into())),
        }
        variant size {
            xs(_t) { width: 24.0, height: 24.0 }
            sm(_t) { width: 32.0, height: 32.0 }
            #[default]
            md(_t) { width: 40.0, height: 40.0 }
            lg(_t) { width: 56.0, height: 56.0 }
            xl(_t) { width: 80.0, height: 80.0 }
        }
        variant color {
            #[default]
            neutral(t) {
                background: Tokenized::token("intent-neutral-soft-bg", Color("#eef0f7".into())),
                color: Tokenized::token("intent-neutral-soft-text", Color("#1a1a1f".into())),
            }
            primary(t) {
                background: Tokenized::token("intent-primary-soft-bg", Color("rgba(91, 108, 255, 0.12)".into())),
                color: Tokenized::token("intent-primary-soft-text", Color("#3947d6".into())),
            }
            secondary(t) {
                background: Tokenized::token("intent-secondary-soft-bg", Color("rgba(71, 85, 105, 0.10)".into())),
                color: Tokenized::token("intent-secondary-soft-text", Color("#334155".into())),
            }
            success(t) {
                background: Tokenized::token("intent-success-soft-bg", Color("rgba(22, 163, 74, 0.12)".into())),
                color: Tokenized::token("intent-success-soft-text", Color("#107a37".into())),
            }
            danger(t) {
                background: Tokenized::token("intent-danger-soft-bg", Color("rgba(220, 38, 38, 0.10)".into())),
                color: Tokenized::token("intent-danger-soft-text", Color("#b91c1c".into())),
            }
            warning(t) {
                background: Tokenized::token("intent-warning-soft-bg", Color("rgba(217, 119, 6, 0.12)".into())),
                color: Tokenized::token("intent-warning-soft-text", Color("#b45309".into())),
            }
            info(t) {
                background: Tokenized::token("intent-info-soft-bg", Color("rgba(2, 132, 199, 0.12)".into())),
                color: Tokenized::token("intent-info-soft-text", Color("#075985".into())),
            }
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
            sm(_t) { font_size: Tokenized::token("typography-size-sm", Length::Px(12.0)), line_height: 32.0 }
            #[default]
            md(_t) { font_size: Tokenized::token("typography-size-md", Length::Px(14.0)), line_height: 40.0 }
            lg(_t) { font_size: Tokenized::token("typography-size-xl", Length::Px(20.0)), line_height: 56.0 }
            xl(_t) { font_size: Tokenized::token("typography-size-xxl", Length::Px(28.0)), line_height: 80.0 }
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
            gap: Tokenized::token("spacing-xs", Length::Px(4.0)),
            padding_vertical: Tokenized::token("spacing-xs", Length::Px(4.0)),
            padding_horizontal: Tokenized::token("spacing-sm", Length::Px(8.0)),
            border_radius: Tokenized::token("radius-pill", Length::Px(999.0)),
        }
        variant appearance {
            primary_solid(t) {
                background: Tokenized::token("intent-primary-solid-bg", Color("#5b6cff".into())),
                color: Tokenized::token("intent-primary-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            primary_soft(t) {
                background: Tokenized::token("intent-primary-soft-bg", Color("rgba(91, 108, 255, 0.12)".into())),
                color: Tokenized::token("intent-primary-soft-text", Color("#3947d6".into())),
                border_width: 0.0,
            }
            primary_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-primary-fg", Color("#3947d6".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-primary-border", Color("#5b6cff".into())),
            }
            secondary_solid(t) {
                background: Tokenized::token("intent-secondary-solid-bg", Color("#475569".into())),
                color: Tokenized::token("intent-secondary-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            secondary_soft(t) {
                background: Tokenized::token("intent-secondary-soft-bg", Color("rgba(71, 85, 105, 0.10)".into())),
                color: Tokenized::token("intent-secondary-soft-text", Color("#334155".into())),
                border_width: 0.0,
            }
            secondary_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-secondary-fg", Color("#334155".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-secondary-border", Color("#475569".into())),
            }
            neutral_solid(t) {
                background: Tokenized::token("intent-neutral-solid-bg", Color("#1a1a1f".into())),
                color: Tokenized::token("intent-neutral-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            #[default]
            neutral_soft(t) {
                background: Tokenized::token("intent-neutral-soft-bg", Color("#eef0f7".into())),
                color: Tokenized::token("intent-neutral-soft-text", Color("#1a1a1f".into())),
                border_width: 0.0,
            }
            neutral_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-neutral-fg", Color("#1a1a1f".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-neutral-border", Color("#cbd0db".into())),
            }
            success_solid(t) {
                background: Tokenized::token("intent-success-solid-bg", Color("#16a34a".into())),
                color: Tokenized::token("intent-success-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            success_soft(t) {
                background: Tokenized::token("intent-success-soft-bg", Color("rgba(22, 163, 74, 0.12)".into())),
                color: Tokenized::token("intent-success-soft-text", Color("#107a37".into())),
                border_width: 0.0,
            }
            success_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-success-fg", Color("#107a37".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-success-border", Color("#16a34a".into())),
            }
            danger_solid(t) {
                background: Tokenized::token("intent-danger-solid-bg", Color("#dc2626".into())),
                color: Tokenized::token("intent-danger-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            danger_soft(t) {
                background: Tokenized::token("intent-danger-soft-bg", Color("rgba(220, 38, 38, 0.10)".into())),
                color: Tokenized::token("intent-danger-soft-text", Color("#b91c1c".into())),
                border_width: 0.0,
            }
            danger_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-danger-fg", Color("#b91c1c".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-danger-border", Color("#dc2626".into())),
            }
            warning_solid(t) {
                background: Tokenized::token("intent-warning-solid-bg", Color("#d97706".into())),
                color: Tokenized::token("intent-warning-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            warning_soft(t) {
                background: Tokenized::token("intent-warning-soft-bg", Color("rgba(217, 119, 6, 0.12)".into())),
                color: Tokenized::token("intent-warning-soft-text", Color("#b45309".into())),
                border_width: 0.0,
            }
            warning_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-warning-fg", Color("#b45309".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-warning-border", Color("#d97706".into())),
            }
            info_solid(t) {
                background: Tokenized::token("intent-info-solid-bg", Color("#0284c7".into())),
                color: Tokenized::token("intent-info-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            info_soft(t) {
                background: Tokenized::token("intent-info-soft-bg", Color("rgba(2, 132, 199, 0.12)".into())),
                color: Tokenized::token("intent-info-soft-text", Color("#075985".into())),
                border_width: 0.0,
            }
            info_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-info-fg", Color("#075985".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-info-border", Color("#0284c7".into())),
            }
        }
        transitions {
            background: 250ms EaseInOut,
            color: 250ms EaseInOut,
            border_color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub TagLabel<IdeaThemeRef> {
        base(t) {
            font_size: Tokenized::token("typography-size-sm", Length::Px(12.0)),
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
            font_size: Tokenized::token("typography-size-md", Length::Px(14.0)),
            font_weight: FontWeight::Bold,
            text_align: TextAlign::Center,
            line_height: 14.0,
            width: 16.0,
            height: 16.0,
            border_radius: Tokenized::token("radius-pill", Length::Px(999.0)),
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
            gap: Tokenized::token("spacing-md", Length::Px(12.0)),
            padding: Tokenized::token("spacing-lg", Length::Px(16.0)),
            border_radius: Tokenized::token("radius-md", Length::Px(8.0)),
        }
        variant appearance {
            primary_solid(t) {
                background: Tokenized::token("intent-primary-solid-bg", Color("#5b6cff".into())),
                color: Tokenized::token("intent-primary-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            primary_soft(t) {
                background: Tokenized::token("intent-primary-soft-bg", Color("rgba(91, 108, 255, 0.12)".into())),
                color: Tokenized::token("intent-primary-soft-text", Color("#3947d6".into())),
                border_width: 0.0,
            }
            primary_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-primary-fg", Color("#3947d6".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-primary-border", Color("#5b6cff".into())),
            }
            secondary_solid(t) {
                background: Tokenized::token("intent-secondary-solid-bg", Color("#475569".into())),
                color: Tokenized::token("intent-secondary-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            secondary_soft(t) {
                background: Tokenized::token("intent-secondary-soft-bg", Color("rgba(71, 85, 105, 0.10)".into())),
                color: Tokenized::token("intent-secondary-soft-text", Color("#334155".into())),
                border_width: 0.0,
            }
            secondary_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-secondary-fg", Color("#334155".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-secondary-border", Color("#475569".into())),
            }
            neutral_solid(t) {
                background: Tokenized::token("intent-neutral-solid-bg", Color("#1a1a1f".into())),
                color: Tokenized::token("intent-neutral-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            #[default]
            neutral_soft(t) {
                background: Tokenized::token("intent-neutral-soft-bg", Color("#eef0f7".into())),
                color: Tokenized::token("intent-neutral-soft-text", Color("#1a1a1f".into())),
                border_width: 0.0,
            }
            neutral_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-neutral-fg", Color("#1a1a1f".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-neutral-border", Color("#cbd0db".into())),
            }
            success_solid(t) {
                background: Tokenized::token("intent-success-solid-bg", Color("#16a34a".into())),
                color: Tokenized::token("intent-success-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            success_soft(t) {
                background: Tokenized::token("intent-success-soft-bg", Color("rgba(22, 163, 74, 0.12)".into())),
                color: Tokenized::token("intent-success-soft-text", Color("#107a37".into())),
                border_width: 0.0,
            }
            success_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-success-fg", Color("#107a37".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-success-border", Color("#16a34a".into())),
            }
            danger_solid(t) {
                background: Tokenized::token("intent-danger-solid-bg", Color("#dc2626".into())),
                color: Tokenized::token("intent-danger-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            danger_soft(t) {
                background: Tokenized::token("intent-danger-soft-bg", Color("rgba(220, 38, 38, 0.10)".into())),
                color: Tokenized::token("intent-danger-soft-text", Color("#b91c1c".into())),
                border_width: 0.0,
            }
            danger_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-danger-fg", Color("#b91c1c".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-danger-border", Color("#dc2626".into())),
            }
            warning_solid(t) {
                background: Tokenized::token("intent-warning-solid-bg", Color("#d97706".into())),
                color: Tokenized::token("intent-warning-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            warning_soft(t) {
                background: Tokenized::token("intent-warning-soft-bg", Color("rgba(217, 119, 6, 0.12)".into())),
                color: Tokenized::token("intent-warning-soft-text", Color("#b45309".into())),
                border_width: 0.0,
            }
            warning_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-warning-fg", Color("#b45309".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-warning-border", Color("#d97706".into())),
            }
            info_solid(t) {
                background: Tokenized::token("intent-info-solid-bg", Color("#0284c7".into())),
                color: Tokenized::token("intent-info-solid-text", Color("#ffffff".into())),
                border_width: 0.0,
            }
            info_soft(t) {
                background: Tokenized::token("intent-info-soft-bg", Color("rgba(2, 132, 199, 0.12)".into())),
                color: Tokenized::token("intent-info-soft-text", Color("#075985".into())),
                border_width: 0.0,
            }
            info_outlined(t) {
                background: Color("transparent".into()),
                color: Tokenized::token("intent-info-fg", Color("#075985".into())),
                border_width: 1.0,
                border_color: Tokenized::token("intent-info-border", Color("#0284c7".into())),
            }
        }
        transitions {
            background: 250ms EaseInOut,
            color: 250ms EaseInOut,
            border_color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub AlertTitle<IdeaThemeRef> {
        base(t) {
            font_size: Tokenized::token("typography-size-md", Length::Px(14.0)),
            font_weight: FontWeight::SemiBold,
            line_height: 20.0,
        }
    }
}

stylesheet! {
    pub AlertBody<IdeaThemeRef> {
        base(t) {
            font_size: Tokenized::token("typography-size-sm", Length::Px(12.0)),
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
            background: Tokenized::token("color-surface-alt", Color("#eef0f7".into())),
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
            gap: Tokenized::token("spacing-xs", Length::Px(4.0)),
            border_bottom_width: 1.0,
            border_bottom_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
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
            color: Tokenized::token("color-text-muted", Color("#6b7280".into())),
            padding_vertical: Tokenized::token("spacing-sm", Length::Px(8.0)),
            padding_horizontal: Tokenized::token("spacing-md", Length::Px(12.0)),
            font_weight: FontWeight::Medium,
            font_size: Tokenized::token("typography-size-md", Length::Px(14.0)),
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
                color: Tokenized::token("color-text", Color("#1a1a1f".into())),
                border_bottom_color: Tokenized::token("intent-primary-solid-bg", Color("#5b6cff".into())),
            }
        }
        state hovered(t) {
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
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
            padding_vertical: Tokenized::token("spacing-lg", Length::Px(16.0)),
            flex_direction: FlexDirection::Column,
            gap: Tokenized::token("spacing-md", Length::Px(12.0)),
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
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            padding: Tokenized::token("spacing-lg", Length::Px(16.0)),
            border_radius: Tokenized::token("radius-lg", Length::Px(12.0)),
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            gap: Tokenized::token("spacing-md", Length::Px(12.0)),
            flex_direction: FlexDirection::Column,
            min_width: 320.0,
            max_width: 560.0,
            shadow: runtime_core::Shadow {
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
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            padding: Tokenized::token("spacing-sm", Length::Px(8.0)),
            border_radius: Tokenized::token("radius-md", Length::Px(8.0)),
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            gap: Tokenized::token("spacing-xs", Length::Px(4.0)),
            flex_direction: FlexDirection::Column,
            min_width: 180.0,
            shadow: runtime_core::Shadow {
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
