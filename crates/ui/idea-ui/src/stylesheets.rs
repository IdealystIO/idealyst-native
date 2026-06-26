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
    AlignItems, Color, Cursor, FlexDirection, FontWeight, JustifyContent, Length, Position,
    TextAlign, TextTransform, Tokenized,
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
            stretch(_t)  { align_items: AlignItems::Stretch }
            start(_t)    { align_items: AlignItems::FlexStart }
            center(_t)   { align_items: AlignItems::Center }
            end(_t)      { align_items: AlignItems::FlexEnd }
            // Align children on their text baseline — for inline rows that mix
            // prose and a Link/Badge so they sit on a common baseline.
            baseline(_t) { align_items: AlignItems::Baseline }
        }
        variant justify {
            #[default]
            start(_t)   { justify_content: JustifyContent::FlexStart }
            center(_t)  { justify_content: JustifyContent::Center }
            end(_t)     { justify_content: JustifyContent::FlexEnd }
            between(_t) { justify_content: JustifyContent::SpaceBetween }
            around(_t)  { justify_content: JustifyContent::SpaceAround }
        }
        // Opt-in line wrapping. `off` (default) keeps the row/column on a
        // single line (may overflow); `on` lets children wrap onto new
        // lines when they don't fit — the natural choice for a Row of
        // chips/buttons/badges on a narrow viewport.
        variant wrap {
            #[default]
            off(_t) { flex_wrap: runtime_core::FlexWrap::NoWrap }
            on(_t)  { flex_wrap: runtime_core::FlexWrap::Wrap }
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
            font_size: Tokenized::token("typography-body-size", Length::Px(14.0)),
            text_align: TextAlign::Center,
            letter_spacing: 0.2,
        }
        variant size {
            sm(t) {
                padding_vertical: Tokenized::token("spacing-xs", Length::Px(4.0)),
                padding_horizontal: Tokenized::token("spacing-md", Length::Px(12.0)),
                font_size: Tokenized::token("typography-body-sm-size", Length::Px(13.0)),
            }
            #[default]
            md(t) {
                padding_vertical: Tokenized::token("spacing-sm", Length::Px(8.0)),
                padding_horizontal: Tokenized::token("spacing-lg", Length::Px(16.0)),
                font_size: Tokenized::token("typography-body-size", Length::Px(14.0)),
            }
            lg(t) {
                padding_vertical: Tokenized::token("spacing-md", Length::Px(12.0)),
                padding_horizontal: Tokenized::token("spacing-xl", Length::Px(24.0)),
                font_size: Tokenized::token("typography-body-lg-size", Length::Px(18.0)),
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
// Typography — unified text component
//
// Single component for every kind of text on a page. The `kind` axis
// picks the size + weight + spacing (Display, H1-H3, BodyXl/Lg/_/Sm,
// Caption, Overline); the `tone` axis picks the color (Default,
// Muted, Primary, Danger, Success, Warning, Info, Inverse); the
// `align` axis picks horizontal alignment.
//
// Replaces the older Heading / Body / Caption split — keeping all
// type styling in one place means an app's typography scale is one
// theme block, not three components × three stylesheets.
// =============================================================================

stylesheet! {
    pub Typography<IdeaThemeRef> {
        base(t) {
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
            font_size: Tokenized::token("typography-body-size", Length::Px(14.0)),
            font_weight: FontWeight::Normal,
            line_height: 20.0,
        }
        variant kind {
            display(t) {
                font_size: Tokenized::token("typography-display-size", Length::Px(56.0)),
                font_weight: FontWeight::Bold,
                letter_spacing: -1.4,
                line_height: 60.0,
            }
            h1(t) {
                font_size: Tokenized::token("typography-h1-size", Length::Px(36.0)),
                font_weight: FontWeight::Bold,
                letter_spacing: -1.0,
                line_height: 42.0,
            }
            h2(t) {
                font_size: Tokenized::token("typography-h2-size", Length::Px(28.0)),
                font_weight: FontWeight::SemiBold,
                letter_spacing: -0.3,
                line_height: 34.0,
            }
            h3(t) {
                font_size: Tokenized::token("typography-h3-size", Length::Px(20.0)),
                font_weight: FontWeight::SemiBold,
                letter_spacing: -0.2,
                line_height: 26.0,
            }
            body_xl(t) {
                font_size: Tokenized::token("typography-body-xl-size", Length::Px(20.0)),
                line_height: 30.0,
            }
            body_lg(t) {
                font_size: Tokenized::token("typography-body-lg-size", Length::Px(18.0)),
                line_height: 26.0,
            }
            #[default]
            body(t) {
                font_size: Tokenized::token("typography-body-size", Length::Px(14.0)),
                line_height: 20.0,
            }
            body_sm(t) {
                font_size: Tokenized::token("typography-body-sm-size", Length::Px(13.0)),
                line_height: 18.0,
            }
            caption(t) {
                color: Tokenized::token("color-text-muted", Color("#6b7280".into())),
                font_size: Tokenized::token("typography-caption-size", Length::Px(12.0)),
                line_height: 16.0,
            }
            overline(t) {
                color: Tokenized::token("color-text-muted", Color("#6b7280".into())),
                font_size: Tokenized::token("typography-overline-size", Length::Px(11.0)),
                font_weight: FontWeight::SemiBold,
                letter_spacing: 0.8,
                line_height: 16.0,
                text_transform: TextTransform::Uppercase,
            }
        }
        variant tone {
            #[default]
            default(_t) {}
            muted(t)    { color: Tokenized::token("color-text-muted", Color("#6b7280".into())) }
            primary(t)  { color: Tokenized::token("intent-primary-fg", Color("#3947d6".into())) }
            danger(t)   { color: Tokenized::token("intent-danger-fg", Color("#b91c1c".into())) }
            success(t)  { color: Tokenized::token("intent-success-fg", Color("#107a37".into())) }
            warning(t)  { color: Tokenized::token("intent-warning-fg", Color("#b45309".into())) }
            info(t)     { color: Tokenized::token("intent-info-fg", Color("#065e85".into())) }
            inverse(t)  { color: Tokenized::token("color-text-inverse", Color("#ffffff".into())) }
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
            font_size: Tokenized::token("typography-body-size", Length::Px(14.0)),
        }
        variant size {
            sm(t) {
                padding_vertical: Tokenized::token("spacing-xs", Length::Px(4.0)),
                padding_horizontal: Tokenized::token("spacing-sm", Length::Px(8.0)),
                font_size: Tokenized::token("typography-body-sm-size", Length::Px(13.0)),
            }
            #[default]
            md(t) {
                padding_vertical: Tokenized::token("spacing-sm", Length::Px(8.0)),
                padding_horizontal: Tokenized::token("spacing-md", Length::Px(12.0)),
                font_size: Tokenized::token("typography-body-size", Length::Px(14.0)),
            }
            lg(t) {
                padding_vertical: Tokenized::token("spacing-md", Length::Px(12.0)),
                padding_horizontal: Tokenized::token("spacing-lg", Length::Px(16.0)),
                font_size: Tokenized::token("typography-body-lg-size", Length::Px(18.0)),
            }
        }
        variant tone {
            #[default]
            default(_t) {}
            error(t) {
                border_color: Tokenized::token("intent-danger-solid-bg", Color("#dc2626".into())),
            }
        }
        // The input "shell": outline (bordered, the default), contained
        // (filled, borderless), bare (no chrome). Border width stays 1 in
        // every arm so the focused-state ring still renders. The live
        // styling lives in `build_field_input_sheet`; these arms exist to
        // generate the `FieldAppearance` enum and document the axis.
        variant appearance {
            #[default]
            outline(_t) {}
            contained(t) {
                background: Tokenized::token("color-surface-alt", Color("#eef0f7".into())),
                border_color: Color("transparent".into()),
            }
            bare(_t) {
                background: Color("transparent".into()),
                border_color: Color("transparent".into()),
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
            font_size: Tokenized::token("typography-body-sm-size", Length::Px(13.0)),
            font_weight: FontWeight::Medium,
        }
    }
}

stylesheet! {
    pub FieldHelp<IdeaThemeRef> {
        base(t) {
            color: Tokenized::token("color-text-muted", Color("#6b7280".into())),
            font_size: Tokenized::token("typography-body-sm-size", Length::Px(13.0)),
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
// Selection controls — Switch thumb + shared label row
// =============================================================================
//
// The tone-bearing surfaces (Switch track, Checkbox box, Radio ring)
// live in idea-theme's extensible sheet builders so apps can register
// custom tones. The thumb and the label-row layout carry no tone, so
// they're plain static stylesheets here.

stylesheet! {
    pub SwitchThumb<IdeaThemeRef> {
        base(t) {
            background: Color("#ffffff".into()),
            border_radius: Tokenized::token("radius-pill", Length::Px(999.0)),
            // Center an optional thumb icon (without this it sits in the corner).
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            shadow: runtime_core::Shadow {
                x: 0.0,
                y: 1.0,
                blur: 3.0,
                color: Color("rgba(15, 17, 21, 0.30)".into()),
            },
        }
        // Diameter = track height − 4 (2px inset on each edge). Mirrors
        // `SWITCH_TRACK_DIMS` in idea-theme; keep in lockstep.
        variant size {
            sm(_t) { width: 14.0, height: 14.0 }
            #[default]
            md(_t) { width: 18.0, height: 18.0 }
            lg(_t) { width: 24.0, height: 24.0 }
        }
    }
}

// A horizontal label row shared by Switch / Checkbox / Radio: control
// on one side, label text on the other, vertically centered.
stylesheet! {
    pub ControlRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: Tokenized::token("spacing-sm", Length::Px(8.0)),
            // Clickable control: pointer cursor on web (inherits to the inner
            // box/track + label). macOS maps it to NSCursor; touch backends
            // no-op. Mirrors Button/IconButton.
            cursor: Cursor::Pointer,
        }
    }
}

// Toast stack — the column of floating toasts inside the ToastHost
// overlay. Capped width so a long message wraps rather than spanning
// the viewport.
stylesheet! {
    pub ToastStack<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Stretch,
            gap: Tokenized::token("spacing-sm", Length::Px(8.0)),
            padding: Tokenized::token("spacing-md", Length::Px(12.0)),
            width: 360.0,
            max_width: Length::pct(100.0),
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
            font_size: Tokenized::token("typography-body-size", Length::Px(14.0)),
            text_align: TextAlign::Left,
            min_width: 160.0,
            // Row: label on the left, chevron on the right.
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            gap: Tokenized::token("spacing-sm", Length::Px(8.0)),
            cursor: Cursor::Pointer,
        }
        // When the menu is open, highlight with the focus ring (like Field).
        variant open {
            #[default]
            off(_t) {}
            on(t) {
                border_color: Tokenized::token("color-focus-ring", Color("#5b6cff".into())),
            }
        }
        variant size {
            sm(t) {
                padding_vertical: Tokenized::token("spacing-xs", Length::Px(4.0)),
                padding_horizontal: Tokenized::token("spacing-sm", Length::Px(8.0)),
                font_size: Tokenized::token("typography-body-sm-size", Length::Px(13.0)),
            }
            #[default]
            md(t) {
                padding_vertical: Tokenized::token("spacing-sm", Length::Px(8.0)),
                padding_horizontal: Tokenized::token("spacing-md", Length::Px(12.0)),
                font_size: Tokenized::token("typography-body-size", Length::Px(14.0)),
            }
            lg(t) {
                padding_vertical: Tokenized::token("spacing-md", Length::Px(12.0)),
                padding_horizontal: Tokenized::token("spacing-lg", Length::Px(16.0)),
                font_size: Tokenized::token("typography-body-lg-size", Length::Px(18.0)),
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
            font_size: Tokenized::token("typography-body-size", Length::Px(14.0)),
            text_align: TextAlign::Left,
            cursor: Cursor::Pointer,
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
// Autocomplete — searchable combobox (input + chevron + filtered menu)
// =============================================================================
//
// `AutocompleteBox` is a thin positioning shell: the editable input carries
// the bordered chrome (so the native focus ring lands on the focusable
// element, exactly like `Field`), and the disclosure chevron is pinned over
// the input's right edge — hence `position: relative` on the box so the
// absolutely-placed chevron resolves against it.
//
// `AutocompleteInput` is the text input itself: same box shape as
// `FieldInput`/`SelectTrigger` (so a combobox sits flush beside a Field or
// Select) with extra right padding reserving room for the chevron, plus the
// focused/disabled state overlays.
//
// The dropdown deliberately REUSES `SelectMenu` (panel) and `SelectOption`
// (rows) so a Select and an Autocomplete drop the same menu — one less
// surface to keep in visual sync. `AutocompleteChevron` is the caret;
// `AutocompleteEmpty` styles the "no matches" row.

stylesheet! {
    pub AutocompleteBox<IdeaThemeRef> {
        base(_t) {
            position: Position::Relative,
            flex_direction: FlexDirection::Column,
            min_width: 200.0,
        }
    }
}

stylesheet! {
    pub AutocompleteInput<IdeaThemeRef> {
        base(t) {
            width: Length::pct(100.0),
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
            border_radius: Tokenized::token("radius-md", Length::Px(8.0)),
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            padding_vertical: Tokenized::token("spacing-sm", Length::Px(8.0)),
            padding_left: Tokenized::token("spacing-md", Length::Px(12.0)),
            // Reserve room for the chevron pinned over the right edge.
            padding_right: Tokenized::token("spacing-xl", Length::Px(28.0)),
            font_size: Tokenized::token("typography-body-size", Length::Px(14.0)),
            text_align: TextAlign::Left,
        }
        variant size {
            sm(t) {
                padding_vertical: Tokenized::token("spacing-xs", Length::Px(4.0)),
                padding_left: Tokenized::token("spacing-sm", Length::Px(8.0)),
                padding_right: Tokenized::token("spacing-lg", Length::Px(24.0)),
                font_size: Tokenized::token("typography-body-sm-size", Length::Px(13.0)),
            }
            #[default]
            md(t) {
                padding_vertical: Tokenized::token("spacing-sm", Length::Px(8.0)),
                padding_left: Tokenized::token("spacing-md", Length::Px(12.0)),
                padding_right: Tokenized::token("spacing-xl", Length::Px(28.0)),
                font_size: Tokenized::token("typography-body-size", Length::Px(14.0)),
            }
            lg(t) {
                padding_vertical: Tokenized::token("spacing-md", Length::Px(12.0)),
                padding_left: Tokenized::token("spacing-lg", Length::Px(16.0)),
                padding_right: Tokenized::token("spacing-xl", Length::Px(32.0)),
                font_size: Tokenized::token("typography-body-lg-size", Length::Px(18.0)),
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
    pub AutocompleteChevron<IdeaThemeRef> {
        base(t) {
            position: Position::Absolute,
            right: Tokenized::token("spacing-sm", Length::Px(8.0)),
            top: Length::Px(0.0),
            bottom: Length::Px(0.0),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            color: Tokenized::token("color-text-muted", Color("#6b7280".into())),
            font_size: Tokenized::token("typography-body-size", Length::Px(14.0)),
        }
    }
}

stylesheet! {
    pub AutocompleteEmpty<IdeaThemeRef> {
        base(t) {
            color: Tokenized::token("color-text-muted", Color("#6b7280".into())),
            padding_vertical: Tokenized::token("spacing-xs", Length::Px(4.0)),
            padding_horizontal: Tokenized::token("spacing-sm", Length::Px(8.0)),
            font_size: Tokenized::token("typography-body-size", Length::Px(14.0)),
            text_align: TextAlign::Left,
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
            font_size: Tokenized::token("typography-body-size", Length::Px(14.0)),
            font_weight: FontWeight::SemiBold,
            text_align: TextAlign::Center,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
        }
        variant size {
            sm(t) {
                padding: Tokenized::token("spacing-xs", Length::Px(4.0)),
                font_size: Tokenized::token("typography-body-sm-size", Length::Px(13.0)),
                width: 24.0,
                height: 24.0,
            }
            #[default]
            md(t) {
                padding: Tokenized::token("spacing-sm", Length::Px(8.0)),
                font_size: Tokenized::token("typography-body-size", Length::Px(14.0)),
                width: 32.0,
                height: 32.0,
            }
            lg(t) {
                padding: Tokenized::token("spacing-md", Length::Px(12.0)),
                font_size: Tokenized::token("typography-body-lg-size", Length::Px(18.0)),
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
            sm(_t) { font_size: Tokenized::token("typography-body-sm-size", Length::Px(13.0)), line_height: 32.0 }
            #[default]
            md(_t) { font_size: Tokenized::token("typography-body-size", Length::Px(14.0)), line_height: 40.0 }
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
            font_size: Tokenized::token("typography-body-sm-size", Length::Px(13.0)),
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
            // The `×` is a child text node, so flex-center it within the
            // 16×16 box — `text_align` alone only centers glyphs inside a
            // text node, not the node within this container.
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            // Clickable affordance: pointer on web, NSCursor on macOS, inert
            // on touch backends.
            cursor: Cursor::Pointer,
            font_size: Tokenized::token("typography-body-size", Length::Px(14.0)),
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
            font_size: Tokenized::token("typography-body-size", Length::Px(14.0)),
            font_weight: FontWeight::SemiBold,
            line_height: 20.0,
        }
    }
}

stylesheet! {
    pub AlertBody<IdeaThemeRef> {
        base(t) {
            font_size: Tokenized::token("typography-body-sm-size", Length::Px(13.0)),
            line_height: 18.0,
        }
    }
}

// The title/body text column. `flex_grow: 1` + `min_width: 0` lets it
// take the available width and shrink (wrapping text) so the trailing
// `action` and `close` slots align to the banner's far edge instead of
// clustering right after the text.
stylesheet! {
    pub AlertContent<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            min_width: 0.0,
            gap: Tokenized::token("spacing-2xs", Length::Px(2.0)),
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
            font_size: Tokenized::token("typography-body-size", Length::Px(14.0)),
            border_radius: 0.0,
            cursor: Cursor::Pointer,
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
        // Hover/press now carry a translucent surface scrim (not just a text
        // brighten) so tabs/segments read as interactive controls — the
        // toolbar-button feel. `state` blocks are global (appearance-blind), but
        // a neutral surface wash reads on the transparent-resting tab base.
        state hovered(t) {
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
            background: Tokenized::token("color-surface-alt", Color("#eef0f7".into())),
        }
        state pressed(t) {
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
            background: Tokenized::token("color-border", Color("#e4e6ef".into())),
        }
        transitions {
            color: 150ms EaseOut,
            background: 120ms EaseOut,
            border_bottom_color: 200ms EaseOut,
        }
    }
}

// Dot-indicator tab: instead of an underline, the active tab gets a chip
// (surface-alt) background and a colored leading dot. A parallel sheet (rather
// than a variant axis on TabButton) keeps the `active` arm single-axis, so the
// "active ⇒ chip background" rule resolves cleanly on every backend.
stylesheet! {
    pub TabButtonDot<IdeaThemeRef> {
        base(t) {
            background: Color("transparent".into()),
            color: Tokenized::token("color-text-muted", Color("#6b7280".into())),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: Tokenized::token("spacing-sm", Length::Px(8.0)),
            padding_vertical: Tokenized::token("spacing-sm", Length::Px(8.0)),
            padding_horizontal: Tokenized::token("spacing-md", Length::Px(12.0)),
            border_radius: Tokenized::token("radius-md", Length::Px(8.0)),
            font_weight: FontWeight::Medium,
            font_size: Tokenized::token("typography-body-size", Length::Px(14.0)),
        }
        variant active {
            #[default]
            off(_t) {}
            on(t) {
                color: Tokenized::token("color-text", Color("#1a1a1f".into())),
                background: Tokenized::token("color-surface-alt", Color("#eef0f7".into())),
            }
        }
        state hovered(t) {
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
            background: Tokenized::token("color-surface-alt", Color("#eef0f7".into())),
        }
        state pressed(t) {
            background: Tokenized::token("color-border", Color("#e4e6ef".into())),
        }
        transitions {
            color: 150ms EaseOut,
            background: 120ms EaseOut,
        }
    }
}

// The colored leading dot for a dot-indicator tab: muted when inactive, the
// primary intent color when active.
stylesheet! {
    pub TabDot<IdeaThemeRef> {
        base(t) {
            width: 7.0,
            height: 7.0,
            border_radius: Tokenized::token("radius-pill", Length::Px(999.0)),
            background: Tokenized::token("color-text-muted", Color("#6b7280".into())),
        }
        variant active {
            #[default]
            off(_t) {}
            on(t) {
                background: Tokenized::token("intent-primary-solid-bg", Color("#5b6cff".into())),
            }
        }
        transitions {
            background: 150ms EaseOut,
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

// =============================================================================
// Table — themed wrapper over the `table` SDK.
//
// `Table` is the outer surface (rounded corners + hairline border +
// surface bg) applied to the `<table>` itself; `TableHeadCell` and
// `TableBodyCell` are applied to `<th>` and `<td>` (padding + row
// divider). Border-bottom on each cell + `border-collapse: collapse`
// on the table merges into one continuous row boundary per row.
// =============================================================================

stylesheet! {
    pub Table<IdeaThemeRef> {
        base(_t) {
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            border_top_width: 1.0,
            border_right_width: 1.0,
            border_bottom_width: 1.0,
            border_left_width: 1.0,
            border_top_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            border_right_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            border_bottom_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            border_left_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            border_top_left_radius: Tokenized::token("radius-lg", Length::Px(12.0)),
            border_top_right_radius: Tokenized::token("radius-lg", Length::Px(12.0)),
            border_bottom_left_radius: Tokenized::token("radius-lg", Length::Px(12.0)),
            border_bottom_right_radius: Tokenized::token("radius-lg", Length::Px(12.0)),
        }
        transitions {
            background: 250ms EaseInOut,
            border_top_color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub TableHeadCell<IdeaThemeRef> {
        base(_t) {
            background: Tokenized::token("color-surface-alt", Color("#f4f5f9".into())),
            padding_vertical: Tokenized::token("spacing-md", Length::Px(12.0)),
            padding_horizontal: Tokenized::token("spacing-lg", Length::Px(16.0)),
            border_bottom_width: 1.0,
            border_bottom_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            // Override the browser UA default `th { text-align: center }`.
            // The inner text node shrink-wraps (display: inline), so its
            // own `text_align: Left` can't win — the cell's alignment is
            // what positions the inline span. Pin it Left so header cells
            // match body cells on web (native is unaffected: there the
            // text node's alignment already applies). See `TableBodyCell`.
            text_align: TextAlign::Left,
        }
        transitions {
            background: 250ms EaseInOut,
            border_bottom_color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub TableBodyCell<IdeaThemeRef> {
        base(_t) {
            padding_vertical: Tokenized::token("spacing-md", Length::Px(12.0)),
            padding_horizontal: Tokenized::token("spacing-lg", Length::Px(16.0)),
            border_bottom_width: 1.0,
            border_bottom_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            // Explicit (matches the UA `td` default) so head + body cells
            // share one alignment source of truth — see `TableHeadCell`.
            text_align: TextAlign::Left,
        }
        transitions {
            border_bottom_color: 250ms EaseInOut,
        }
    }
}

// Text styling applied to the `text` node INSIDE each cell. The cell
// stylesheets above handle the table-cell concerns (padding +
// border); these handle typography. Splitting keeps the cell's
// `display: table-cell` intact while letting the inner text inherit
// the theme's font + color tokens.
stylesheet! {
    pub TableHeadText<IdeaThemeRef> {
        base(_t) {
            font_size: 11.0,
            font_weight: FontWeight::SemiBold,
            letter_spacing: 0.8,
            text_transform: TextTransform::Uppercase,
            color: Tokenized::token("color-text-muted", Color("#6b7280".into())),
            text_align: TextAlign::Left,
        }
        transitions {
            color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub TableBodyText<IdeaThemeRef> {
        base(_t) {
            font_size: 14.0,
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
            text_align: TextAlign::Left,
        }
        transitions {
            color: 250ms EaseInOut,
        }
    }
}

// Inner wrapper for `TableCell { … }` rich-children blocks. A `<div
// display: flex>` placed directly inside a `<td>` expands to the
// cell's full width (a quirk of flex containers under `display:
// table-cell`). Setting `justify_content: FlexStart` keeps flex-grow
// children (Tags, Buttons) at their natural width, sitting left-
// aligned inside the cell instead of stretching across it. Authors
// who want stretched children can override at the call site.
stylesheet! {
    pub TableCellInner<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::FlexStart,
            gap: Tokenized::token("spacing-sm", Length::Px(8.0)),
        }
    }
}

// =============================================================================
// Collapsible / Accordion
//
// `CollapsibleContainer` is the outer surface — rounded corners +
// hairline border, matching Card/Table. `CollapsibleHeader` is the
// always-visible Pressable that toggles open/closed. `CollapsibleBody`
// is the revealed content area (mounted/unmounted by the framework's
// `presence` primitive with a fade-and-slide animation).
//
// `AccordionContainer` is similar but groups multiple Collapsibles
// with shared dividers — the outer border is the group's, individual
// items don't redraw it.
// =============================================================================

stylesheet! {
    pub CollapsibleContainer<IdeaThemeRef> {
        base(_t) {
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            border_top_width: 1.0,
            border_right_width: 1.0,
            border_bottom_width: 1.0,
            border_left_width: 1.0,
            border_top_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            border_right_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            border_bottom_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            border_left_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            border_top_left_radius: Tokenized::token("radius-lg", Length::Px(12.0)),
            border_top_right_radius: Tokenized::token("radius-lg", Length::Px(12.0)),
            border_bottom_left_radius: Tokenized::token("radius-lg", Length::Px(12.0)),
            border_bottom_right_radius: Tokenized::token("radius-lg", Length::Px(12.0)),
            flex_direction: FlexDirection::Column,
            overflow: runtime_core::Overflow::Hidden,
        }
        transitions {
            background: 250ms EaseInOut,
            border_top_color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub CollapsibleHeader<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            padding_vertical: Tokenized::token("spacing-md", Length::Px(12.0)),
            padding_horizontal: Tokenized::token("spacing-lg", Length::Px(16.0)),
            gap: Tokenized::token("spacing-sm", Length::Px(8.0)),
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
            font_size: 14.0,
            font_weight: FontWeight::SemiBold,
            text_align: TextAlign::Left,
        }
        state hovered(_t) {
            background: Tokenized::token("color-surface-alt", Color("#f4f5f9".into())),
        }
        transitions {
            background: 150ms EaseOut,
            color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub CollapsibleChevron<IdeaThemeRef> {
        base(_t) {
            font_size: 13.0,
            color: Tokenized::token("color-text-muted", Color("#6b7280".into())),
        }
        transitions {
            color: 250ms EaseInOut,
        }
    }
}

// Snap-mode body: state changes apply instantly. Cheap, predictable,
// no perceived animation. Pick this via `CollapsibleTransition::Snap`
// when the disclosure should feel like a single click → done.
stylesheet! {
    pub CollapsibleBody<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            padding_horizontal: Tokenized::token("spacing-lg", Length::Px(16.0)),
            border_top_width: 1.0,
            border_top_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            gap: Tokenized::token("spacing-sm", Length::Px(8.0)),
            overflow: runtime_core::Overflow::Hidden,
        }
        variant open {
            #[default]
            closed(_t) {
                max_height: Length::Px(0.0),
                padding_top: Length::Px(0.0),
                padding_bottom: Length::Px(0.0),
                border_top_width: 0.0,
            }
            shown(_t) {
                max_height: Length::Px(2000.0),
                padding_top: Tokenized::token("spacing-md", Length::Px(12.0)),
                padding_bottom: Tokenized::token("spacing-md", Length::Px(12.0)),
                border_top_width: 1.0,
            }
        }
        transitions {
            border_top_color: 250ms EaseInOut,
        }
    }
}

// Measured-mode body: the chrome (padding, opacity, border-top)
// CSS-transitions on variant flip, while `max-height` is driven per
// frame by an `AnimatedValue<f32>` in `measured_body` — the stylesheet
// deliberately does NOT declare `max_height` on either variant so the
// inline-style writes from `set_animated_f32(MaxHeight, …)` aren't
// fighting a class-rule baseline.
//
// If the chrome timings here change (e.g. `padding_top: 240ms EaseOut`
// becomes 180ms), update [`COLLAPSIBLE_DURATION_DEFAULT_MS`] in
// `components/collapsible.rs` in lockstep — the constant is the
// recommended AV tween length for matching perceptual feel.
stylesheet! {
    pub CollapsibleBodyAnimated<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            padding_horizontal: Tokenized::token("spacing-lg", Length::Px(16.0)),
            border_top_width: 1.0,
            border_top_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            gap: Tokenized::token("spacing-sm", Length::Px(8.0)),
            overflow: runtime_core::Overflow::Hidden,
        }
        variant open {
            #[default]
            closed(_t) {
                padding_top: Length::Px(0.0),
                padding_bottom: Length::Px(0.0),
                opacity: 0.0,
                border_top_width: 0.0,
            }
            shown(_t) {
                padding_top: Tokenized::token("spacing-md", Length::Px(12.0)),
                padding_bottom: Tokenized::token("spacing-md", Length::Px(12.0)),
                opacity: 1.0,
                border_top_width: 1.0,
            }
        }
        transitions {
            border_top_color: 250ms EaseInOut,
            opacity: 200ms EaseOut,
            padding_top: 240ms EaseOut,
            padding_bottom: 240ms EaseOut,
        }
    }
}

// Accordion item — same header/body shape as a Collapsible, but
// without the per-item border/radius (the Accordion container owns
// those, and items just contribute internal dividers).
stylesheet! {
    pub AccordionContainer<IdeaThemeRef> {
        base(_t) {
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            border_top_width: 1.0,
            border_right_width: 1.0,
            border_bottom_width: 1.0,
            border_left_width: 1.0,
            border_top_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            border_right_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            border_bottom_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            border_left_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            border_top_left_radius: Tokenized::token("radius-lg", Length::Px(12.0)),
            border_top_right_radius: Tokenized::token("radius-lg", Length::Px(12.0)),
            border_bottom_left_radius: Tokenized::token("radius-lg", Length::Px(12.0)),
            border_bottom_right_radius: Tokenized::token("radius-lg", Length::Px(12.0)),
            flex_direction: FlexDirection::Column,
            overflow: runtime_core::Overflow::Hidden,
        }
        transitions {
            background: 250ms EaseInOut,
            border_top_color: 250ms EaseInOut,
        }
    }
}

// Per-item divider — top border on items 1..N so the first item has
// no top border and the rest separate cleanly.
stylesheet! {
    pub AccordionItemSeparator<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            border_top_width: 1.0,
            border_top_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
        }
        transitions {
            border_top_color: 250ms EaseInOut,
        }
    }
}

// =============================================================================
// Tooltip — compact high-contrast bubble
// =============================================================================

stylesheet! {
    pub TooltipBubble<IdeaThemeRef> {
        base(t) {
            background: Tokenized::token("color-text", Color("#1a1a1f".into())),
            color: Tokenized::token("color-text-inverse", Color("#ffffff".into())),
            padding_vertical: Tokenized::token("spacing-xs", Length::Px(4.0)),
            padding_horizontal: Tokenized::token("spacing-sm", Length::Px(8.0)),
            border_radius: Tokenized::token("radius-sm", Length::Px(4.0)),
            font_size: Tokenized::token("typography-body-sm-size", Length::Px(13.0)),
            max_width: 260.0,
            shadow: runtime_core::Shadow {
                x: 0.0, y: 4.0, blur: 12.0, color: Color("rgba(15, 17, 21, 0.22)".into()),
            },
        }
    }
}

// =============================================================================
// Menu — panel rows, section labels, separators
// =============================================================================
// The panel surface reuses `SelectMenu`. These style the contents.

stylesheet! {
    pub MenuItemRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: Tokenized::token("spacing-sm", Length::Px(8.0)),
            background: Color("transparent".into()),
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
            padding_vertical: Tokenized::token("spacing-xs", Length::Px(4.0)),
            padding_horizontal: Tokenized::token("spacing-sm", Length::Px(8.0)),
            border_radius: Tokenized::token("radius-sm", Length::Px(4.0)),
            font_size: Tokenized::token("typography-body-size", Length::Px(14.0)),
            text_align: TextAlign::Left,
            cursor: Cursor::Pointer,
        }
        variant active {
            #[default]
            off(_t) {}
            on(t) { background: Tokenized::token("color-surface-alt", Color("#eef0f7".into())) }
        }
        state hovered(t) {
            background: Tokenized::token("color-surface-alt", Color("#eef0f7".into())),
        }
        transitions { background: 120ms EaseOut }
    }
}

stylesheet! {
    pub MenuLabel<IdeaThemeRef> {
        base(t) {
            color: Tokenized::token("color-text-muted", Color("#6b7280".into())),
            font_size: Tokenized::token("typography-overline-size", Length::Px(11.0)),
            font_weight: FontWeight::SemiBold,
            letter_spacing: 0.8,
            text_transform: TextTransform::Uppercase,
            padding_vertical: Tokenized::token("spacing-xs", Length::Px(4.0)),
            padding_horizontal: Tokenized::token("spacing-sm", Length::Px(8.0)),
        }
    }
}

stylesheet! {
    pub MenuSeparator<IdeaThemeRef> {
        base(t) {
            height: 1.0,
            width: Length::pct(100.0),
            background: Tokenized::token("color-border", Color("#e4e6ef".into())),
            margin_top: 4.0,
            margin_bottom: 4.0,
        }
    }
}

// Trailing chevron for SubMenu rows.
stylesheet! {
    pub MenuChevron<IdeaThemeRef> {
        base(t) {
            color: Tokenized::token("color-text-muted", Color("#6b7280".into())),
            font_size: Tokenized::token("typography-body-size", Length::Px(14.0)),
        }
    }
}

// =============================================================================
// Breadcrumbs
// =============================================================================

stylesheet! {
    pub BreadcrumbRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: Tokenized::token("spacing-xs", Length::Px(4.0)),
        }
    }
}

stylesheet! {
    pub BreadcrumbItem<IdeaThemeRef> {
        base(t) {
            color: Tokenized::token("color-text-muted", Color("#6b7280".into())),
            font_size: Tokenized::token("typography-body-sm-size", Length::Px(13.0)),
            background: Color("transparent".into()),
            padding_vertical: 0.0,
            padding_horizontal: Tokenized::token("spacing-xs", Length::Px(4.0)),
            border_radius: Tokenized::token("radius-sm", Length::Px(4.0)),
        }
        variant current {
            #[default]
            off(_t) {}
            on(t) {
                color: Tokenized::token("color-text", Color("#1a1a1f".into())),
                font_weight: FontWeight::SemiBold,
            }
        }
        state hovered(t) { color: Tokenized::token("color-text", Color("#1a1a1f".into())) }
        transitions { color: 120ms EaseOut }
    }
}

stylesheet! {
    pub BreadcrumbSeparator<IdeaThemeRef> {
        base(t) {
            color: Tokenized::token("color-text-muted", Color("#6b7280".into())),
            font_size: Tokenized::token("typography-body-sm-size", Length::Px(13.0)),
        }
    }
}

// =============================================================================
// Pagination
// =============================================================================

stylesheet! {
    pub PaginationRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: Tokenized::token("spacing-xs", Length::Px(4.0)),
        }
    }
}

stylesheet! {
    pub PageButton<IdeaThemeRef> {
        base(t) {
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            min_width: 32.0,
            height: 32.0,
            padding_horizontal: Tokenized::token("spacing-sm", Length::Px(8.0)),
            border_radius: Tokenized::token("radius-md", Length::Px(8.0)),
            background: Color("transparent".into()),
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
            font_size: Tokenized::token("typography-body-sm-size", Length::Px(13.0)),
            font_weight: FontWeight::Medium,
            text_align: TextAlign::Center,
            cursor: Cursor::Pointer,
        }
        variant active {
            #[default]
            off(_t) {}
            on(t) {
                background: Tokenized::token("intent-primary-solid-bg", Color("#5b6cff".into())),
                color: Tokenized::token("intent-primary-solid-text", Color("#ffffff".into())),
            }
        }
        state hovered(t) { background: Tokenized::token("color-surface-alt", Color("#eef0f7".into())) }
        state disabled(_t) { opacity: 0.4 }
        transitions { background: 120ms EaseOut, color: 120ms EaseOut }
    }
}

// =============================================================================
// List / ListItem
// =============================================================================

stylesheet! {
    pub ListContainer<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            border_top_width: 1.0,
            border_right_width: 1.0,
            border_bottom_width: 1.0,
            border_left_width: 1.0,
            border_top_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            border_right_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            border_bottom_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            border_left_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            border_top_left_radius: Tokenized::token("radius-lg", Length::Px(12.0)),
            border_top_right_radius: Tokenized::token("radius-lg", Length::Px(12.0)),
            border_bottom_left_radius: Tokenized::token("radius-lg", Length::Px(12.0)),
            border_bottom_right_radius: Tokenized::token("radius-lg", Length::Px(12.0)),
            overflow: runtime_core::Overflow::Hidden,
        }
    }
}

stylesheet! {
    pub ListItemRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: Tokenized::token("spacing-sm", Length::Px(8.0)),
            padding_vertical: Tokenized::token("spacing-sm", Length::Px(8.0)),
            padding_horizontal: Tokenized::token("spacing-md", Length::Px(12.0)),
            background: Color("transparent".into()),
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
            font_size: Tokenized::token("typography-body-size", Length::Px(14.0)),
            text_align: TextAlign::Left,
        }
        variant divided {
            #[default]
            off(_t) {}
            on(t) {
                border_top_width: 1.0,
                border_top_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            }
        }
        variant active {
            #[default]
            off(_t) {}
            on(t) { background: Tokenized::token("color-surface-alt", Color("#eef0f7".into())) }
        }
        state hovered(t) { background: Tokenized::token("color-surface-alt", Color("#eef0f7".into())) }
        transitions { background: 120ms EaseOut }
    }
}

// =============================================================================
// Grid — N equal columns via chunked flex rows
// =============================================================================

stylesheet! {
    pub GridContainer<IdeaThemeRef> {
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
    }
}

stylesheet! {
    pub GridRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Stretch,
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
    }
}

// Each grid cell flexes equally and is allowed to shrink below content.
stylesheet! {
    pub GridCell<IdeaThemeRef> {
        base(_t) {
            flex_grow: 1.0,
            flex_basis: 0.0,
            min_width: 0.0,
        }
    }
}

// =============================================================================
// Link — inline navigational text
// =============================================================================

stylesheet! {
    pub LinkText<IdeaThemeRef> {
        base(t) {
            color: Tokenized::token("intent-primary-fg", Color("#3947d6".into())),
            font_size: Tokenized::token("typography-body-size", Length::Px(14.0)),
            font_weight: FontWeight::Medium,
        }
        state hovered(t) { color: Tokenized::token("intent-primary-solid-bg", Color("#5b6cff".into())) }
        transitions { color: 120ms EaseOut }
    }
}

// =============================================================================
// Image — clipping box
// =============================================================================

stylesheet! {
    pub ImageBox<IdeaThemeRef> {
        base(_t) {
            overflow: runtime_core::Overflow::Hidden,
        }
    }
}
