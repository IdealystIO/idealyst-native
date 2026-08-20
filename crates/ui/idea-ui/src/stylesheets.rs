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
    AlignItems, Color, Cursor, DisplayKind, FlexDirection, FontWeight, JustifyContent, Length,
    Overflow, Position, TextAlign, TextTransform,
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
            gap: t.spacing.md(),
        }
        variant gap {
            none(_t) { gap: Length::Px(0.0) }
            xs(t)    { gap: t.spacing.xs() }
            sm(t)    { gap: t.spacing.sm() }
            #[default]
            md(t)    { gap: t.spacing.md() }
            lg(t)    { gap: t.spacing.lg() }
            xl(t)    { gap: t.spacing.xl() }
        }
        variant padding {
            #[default]
            none(_t) { padding: Length::Px(0.0) }
            xs(t)    { padding: t.spacing.xs() }
            sm(t)    { padding: t.spacing.sm() }
            md(t)    { padding: t.spacing.md() }
            lg(t)    { padding: t.spacing.lg() }
            xl(t)    { padding: t.spacing.xl() }
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
            padding_vertical: t.spacing.sm(),
            padding_horizontal: t.spacing.lg(),
            border_radius: t.radius.md(),
            font_weight: FontWeight::SemiBold,
            font_size: t.typography.body_size(),
            text_align: TextAlign::Center,
            letter_spacing: 0.2,
        }
        variant size {
            sm(t) {
                padding_vertical: t.spacing.xs(),
                padding_horizontal: t.spacing.md(),
                font_size: t.typography.body_sm_size(),
            }
            #[default]
            md(t) {
                padding_vertical: t.spacing.sm(),
                padding_horizontal: t.spacing.lg(),
                font_size: t.typography.body_size(),
            }
            lg(t) {
                padding_vertical: t.spacing.md(),
                padding_horizontal: t.spacing.xl(),
                font_size: t.typography.body_lg_size(),
            }
        }
        variant appearance {
            #[default]
            primary_solid(t) {
                background: t.intent.primary.solid_bg(),
                color: t.intent.primary.solid_text(),
                border_width: 0.0,
            }
            primary_soft(t) {
                background: t.intent.primary.soft_bg(),
                color: t.intent.primary.soft_text(),
                border_width: 0.0,
            }
            primary_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.primary.fg(),
                border_width: 1.0,
                border_color: t.intent.primary.border(),
            }
            primary_ghost(t) {
                background: Color("transparent".into()),
                color: t.intent.primary.fg(),
                border_width: 0.0,
            }
            secondary_solid(t) {
                background: t.intent.secondary.solid_bg(),
                color: t.intent.secondary.solid_text(),
                border_width: 0.0,
            }
            secondary_soft(t) {
                background: t.intent.secondary.soft_bg(),
                color: t.intent.secondary.soft_text(),
                border_width: 0.0,
            }
            secondary_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.secondary.fg(),
                border_width: 1.0,
                border_color: t.intent.secondary.border(),
            }
            secondary_ghost(t) {
                background: Color("transparent".into()),
                color: t.intent.secondary.fg(),
                border_width: 0.0,
            }
            neutral_solid(t) {
                background: t.intent.neutral.solid_bg(),
                color: t.intent.neutral.solid_text(),
                border_width: 0.0,
            }
            neutral_soft(t) {
                background: t.intent.neutral.soft_bg(),
                color: t.intent.neutral.soft_text(),
                border_width: 0.0,
            }
            neutral_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.neutral.fg(),
                border_width: 1.0,
                border_color: t.intent.neutral.border(),
            }
            neutral_ghost(t) {
                background: Color("transparent".into()),
                color: t.intent.neutral.fg(),
                border_width: 0.0,
            }
            success_solid(t) {
                background: t.intent.success.solid_bg(),
                color: t.intent.success.solid_text(),
                border_width: 0.0,
            }
            success_soft(t) {
                background: t.intent.success.soft_bg(),
                color: t.intent.success.soft_text(),
                border_width: 0.0,
            }
            success_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.success.fg(),
                border_width: 1.0,
                border_color: t.intent.success.border(),
            }
            success_ghost(t) {
                background: Color("transparent".into()),
                color: t.intent.success.fg(),
                border_width: 0.0,
            }
            danger_solid(t) {
                background: t.intent.danger.solid_bg(),
                color: t.intent.danger.solid_text(),
                border_width: 0.0,
            }
            danger_soft(t) {
                background: t.intent.danger.soft_bg(),
                color: t.intent.danger.soft_text(),
                border_width: 0.0,
            }
            danger_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.danger.fg(),
                border_width: 1.0,
                border_color: t.intent.danger.border(),
            }
            danger_ghost(t) {
                background: Color("transparent".into()),
                color: t.intent.danger.fg(),
                border_width: 0.0,
            }
            warning_solid(t) {
                background: t.intent.warning.solid_bg(),
                color: t.intent.warning.solid_text(),
                border_width: 0.0,
            }
            warning_soft(t) {
                background: t.intent.warning.soft_bg(),
                color: t.intent.warning.soft_text(),
                border_width: 0.0,
            }
            warning_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.warning.fg(),
                border_width: 1.0,
                border_color: t.intent.warning.border(),
            }
            warning_ghost(t) {
                background: Color("transparent".into()),
                color: t.intent.warning.fg(),
                border_width: 0.0,
            }
            info_solid(t) {
                background: t.intent.info.solid_bg(),
                color: t.intent.info.solid_text(),
                border_width: 0.0,
            }
            info_soft(t) {
                background: t.intent.info.soft_bg(),
                color: t.intent.info.soft_text(),
                border_width: 0.0,
            }
            info_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.info.fg(),
                border_width: 1.0,
                border_color: t.intent.info.border(),
            }
            info_ghost(t) {
                background: Color("transparent".into()),
                color: t.intent.info.fg(),
                border_width: 0.0,
            }
        }
        state hovered(_t) {
            opacity: 0.92,
        }
        state pressed(_t) {
            opacity: 0.85,
        }
        // Keyboard/pointer focus paints an indigo ring — the cross-platform
        // focus indicator that replaces AppKit's native ring (now suppressed on
        // the pressable host). `border_width: 1` also covers the borderless
        // (solid/soft/ghost) variants, which reserve no border of their own;
        // the outlined variants just recolor their existing 1px border. On
        // macOS the CALayer border draws inside the bounds and never nudges the
        // Taffy-positioned content; on web border-box keeps the outer size fixed.
        state focused(t) {
            border_width: 1.0,
            border_color: t.color.focus_ring(),
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
            color: t.color.text(),
            font_size: t.typography.body_size(),
            font_weight: FontWeight::Normal,
            line_height: 20.0,
        }
        variant kind {
            display(t) {
                font_size: t.typography.display_size(),
                font_weight: FontWeight::Bold,
                letter_spacing: -1.4,
                line_height: 60.0,
            }
            h1(t) {
                font_size: t.typography.h1_size(),
                font_weight: FontWeight::Bold,
                letter_spacing: -1.0,
                line_height: 42.0,
            }
            h2(t) {
                font_size: t.typography.h2_size(),
                font_weight: FontWeight::SemiBold,
                letter_spacing: -0.3,
                line_height: 34.0,
            }
            h3(t) {
                font_size: t.typography.h3_size(),
                font_weight: FontWeight::SemiBold,
                letter_spacing: -0.2,
                line_height: 26.0,
            }
            body_xl(t) {
                font_size: t.typography.body_xl_size(),
                line_height: 30.0,
            }
            body_lg(t) {
                font_size: t.typography.body_lg_size(),
                line_height: 26.0,
            }
            #[default]
            body(t) {
                font_size: t.typography.body_size(),
                line_height: 20.0,
            }
            body_sm(t) {
                font_size: t.typography.body_sm_size(),
                line_height: 18.0,
            }
            caption(t) {
                color: t.color.text_muted(),
                font_size: t.typography.caption_size(),
                line_height: 16.0,
            }
            overline(t) {
                color: t.color.text_muted(),
                font_size: t.typography.overline_size(),
                font_weight: FontWeight::SemiBold,
                letter_spacing: 0.8,
                line_height: 16.0,
                text_transform: TextTransform::Uppercase,
            }
        }
        variant tone {
            #[default]
            default(_t) {}
            muted(t)    { color: t.color.text_muted() }
            primary(t)  { color: t.intent.primary.fg() }
            danger(t)   { color: t.intent.danger.fg() }
            success(t)  { color: t.intent.success.fg() }
            warning(t)  { color: t.intent.warning.fg() }
            info(t)     { color: t.intent.info.fg() }
            inverse(t)  { color: t.color.text_inverse() }
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
            background: t.color.surface(),
            padding: t.spacing.lg(),
            border_radius: t.radius.lg(),
            gap: t.spacing.sm(),
            border_width: 1.0,
            border_color: t.color.border(),
        }
        variant tone {
            #[default]
            surface(t) {
                background: t.color.surface(),
            }
            elevated(t) {
                background: t.color.surface(),
                shadow: runtime_core::Shadow {
                    x: 0.0,
                    y: 4.0,
                    blur: 16.0,
                    color: Color("rgba(15, 17, 21, 0.10)".into()),
                },
            }
            primary(t) {
                background: t.intent.primary.solid_bg(),
                color: t.intent.primary.solid_text(),
                border_color: t.intent.primary.solid_bg(),
            }
            muted(t) {
                background: t.color.surface_alt(),
            }
        }
        variant padding {
            none(_t) { padding: 0.0 }
            sm(t)    { padding: t.spacing.sm() }
            #[default]
            md(t)    { padding: t.spacing.lg() }
            lg(t)    { padding: t.spacing.xl() }
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
            background: t.color.surface(),
            color: t.color.text(),
            padding_vertical: t.spacing.sm(),
            padding_horizontal: t.spacing.md(),
            border_radius: t.radius.md(),
            border_width: 1.0,
            border_color: t.color.border(),
            font_size: t.typography.body_size(),
        }
        variant size {
            sm(t) {
                padding_vertical: t.spacing.xs(),
                padding_horizontal: t.spacing.sm(),
                font_size: t.typography.body_sm_size(),
            }
            #[default]
            md(t) {
                padding_vertical: t.spacing.sm(),
                padding_horizontal: t.spacing.md(),
                font_size: t.typography.body_size(),
            }
            lg(t) {
                padding_vertical: t.spacing.md(),
                padding_horizontal: t.spacing.lg(),
                font_size: t.typography.body_lg_size(),
            }
        }
        variant tone {
            #[default]
            default(_t) {}
            error(t) {
                border_color: t.intent.danger.solid_bg(),
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
                background: t.color.surface_alt(),
                border_color: Color("transparent".into()),
            }
            bare(_t) {
                background: Color("transparent".into()),
                border_color: Color("transparent".into()),
            }
        }
        state focused(t) {
            border_color: t.color.focus_ring(),
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
            gap: t.spacing.xs(),
            // A form field fills its container width by default (web `<input>`
            // is block-level; native should match — §7 parity). Without this
            // the field hugs its content and a centering parent — e.g. the
            // docs `DemoSurface` (`align_items: center`) — collapses it to the
            // content width (an icon-only adorned field shrank to ~54pt and
            // centered). `align_self: stretch` overrides a centering parent for
            // this child only; width stays auto so the field's height is still
            // content-driven and an explicit `width` prop still constrains it.
            align_self: runtime_core::AlignSelf::Stretch,
        }
    }
}

stylesheet! {
    pub FieldLabel<IdeaThemeRef> {
        base(t) {
            color: t.color.text(),
            font_size: t.typography.body_sm_size(),
            font_weight: FontWeight::Medium,
        }
    }
}

stylesheet! {
    pub FieldHelp<IdeaThemeRef> {
        base(t) {
            color: t.color.text_muted(),
            font_size: t.typography.body_sm_size(),
        }
        variant tone {
            #[default]
            default(t) { color: t.color.text_muted() }
            error(t)   { color: t.intent.danger.fg() }
        }
    }
}

// =============================================================================
// Divider
// =============================================================================

stylesheet! {
    pub Divider<IdeaThemeRef> {
        base(t) {
            background: t.color.border(),
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
            padding_horizontal: t.spacing.sm(),
            border_radius: t.radius.pill(),
            // `typography-size-xs` before — a name no theme installs, so
            // it always rendered its 11px fallback. The overline step is
            // that same 11px and is the semantic match for a caps lockup,
            // so the badge now actually follows the type scale.
            font_size: t.typography.overline_size(),
            font_weight: FontWeight::SemiBold,
            letter_spacing: 0.4,
            text_transform: TextTransform::Uppercase,
            text_align: TextAlign::Center,
        }
        variant appearance {
            primary_solid(t) {
                background: t.intent.primary.solid_bg(),
                color: t.intent.primary.solid_text(),
                border_width: 0.0,
            }
            primary_soft(t) {
                background: t.intent.primary.soft_bg(),
                color: t.intent.primary.soft_text(),
                border_width: 0.0,
            }
            primary_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.primary.fg(),
                border_width: 1.0,
                border_color: t.intent.primary.border(),
            }
            secondary_solid(t) {
                background: t.intent.secondary.solid_bg(),
                color: t.intent.secondary.solid_text(),
                border_width: 0.0,
            }
            secondary_soft(t) {
                background: t.intent.secondary.soft_bg(),
                color: t.intent.secondary.soft_text(),
                border_width: 0.0,
            }
            secondary_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.secondary.fg(),
                border_width: 1.0,
                border_color: t.intent.secondary.border(),
            }
            neutral_solid(t) {
                background: t.intent.neutral.solid_bg(),
                color: t.intent.neutral.solid_text(),
                border_width: 0.0,
            }
            #[default]
            neutral_soft(t) {
                background: t.intent.neutral.soft_bg(),
                color: t.intent.neutral.soft_text(),
                border_width: 0.0,
            }
            neutral_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.neutral.fg(),
                border_width: 1.0,
                border_color: t.intent.neutral.border(),
            }
            success_solid(t) {
                background: t.intent.success.solid_bg(),
                color: t.intent.success.solid_text(),
                border_width: 0.0,
            }
            success_soft(t) {
                background: t.intent.success.soft_bg(),
                color: t.intent.success.soft_text(),
                border_width: 0.0,
            }
            success_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.success.fg(),
                border_width: 1.0,
                border_color: t.intent.success.border(),
            }
            danger_solid(t) {
                background: t.intent.danger.solid_bg(),
                color: t.intent.danger.solid_text(),
                border_width: 0.0,
            }
            danger_soft(t) {
                background: t.intent.danger.soft_bg(),
                color: t.intent.danger.soft_text(),
                border_width: 0.0,
            }
            danger_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.danger.fg(),
                border_width: 1.0,
                border_color: t.intent.danger.border(),
            }
            warning_solid(t) {
                background: t.intent.warning.solid_bg(),
                color: t.intent.warning.solid_text(),
                border_width: 0.0,
            }
            warning_soft(t) {
                background: t.intent.warning.soft_bg(),
                color: t.intent.warning.soft_text(),
                border_width: 0.0,
            }
            warning_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.warning.fg(),
                border_width: 1.0,
                border_color: t.intent.warning.border(),
            }
            info_solid(t) {
                background: t.intent.info.solid_bg(),
                color: t.intent.info.solid_text(),
                border_width: 0.0,
            }
            info_soft(t) {
                background: t.intent.info.soft_bg(),
                color: t.intent.info.soft_text(),
                border_width: 0.0,
            }
            info_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.info.fg(),
                border_width: 1.0,
                border_color: t.intent.info.border(),
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
            gap: t.spacing.sm(),
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
            border_radius: t.radius.pill(),
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
            gap: t.spacing.sm(),
            // Clickable control: pointer cursor on web (inherits to the inner
            // box/track + label). macOS maps it to NSCursor; touch backends
            // no-op. Mirrors Button/IconButton.
            cursor: Cursor::Pointer,
        }
        // NO `state focused` here. The row is a plain layout view whose
        // focusable host is the CONTROL inside it (the Switch track, the
        // Checkbox box, the Radio ring — each a `pressable` carrying its
        // sheet's `__state_focused` ring). Ringing the row instead drew a
        // border around control *and* label, which reads as a stray box
        // around the text rather than a focus indicator on the control.
    }
}

// Surface — the themed background container. Two closed axes (which
// neutral token fills it, which spacing step pads it); the continuous
// `grow` weight rides the INLINE layer at the call site
// (`StyleApplication::with_inline`), so the sheet premints. This
// replaces a per-instance `StyleSheet::new` in `surface.rs` that was
// invisible to the premint dump.
stylesheet! {
    pub SurfaceSheet<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            // Children fill the cross axis so inner content spans the surface.
            align_items: AlignItems::Stretch,
        }
        variant bg {
            background(t) { background: t.color.background() }
            #[default]
            surface(t) { background: t.color.surface() }
            surface_alt(t) { background: t.color.surface_alt() }
        }
        variant pad {
            #[default]
            none(_t) {}
            xs(t) { padding: t.spacing.xs() }
            sm(t) { padding: t.spacing.sm() }
            md(t) { padding: t.spacing.md() }
            lg(t) { padding: t.spacing.lg() }
            xl(t) { padding: t.spacing.xl() }
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
            gap: t.spacing.sm(),
            padding: t.spacing.md(),
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
            background: t.color.surface(),
            color: t.color.text(),
            padding_vertical: t.spacing.sm(),
            padding_horizontal: t.spacing.md(),
            border_radius: t.radius.md(),
            border_width: 1.0,
            border_color: t.color.border(),
            font_size: t.typography.body_size(),
            text_align: TextAlign::Left,
            min_width: 160.0,
            // Row: label on the left, chevron on the right.
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            gap: t.spacing.sm(),
            cursor: Cursor::Pointer,
        }
        // When the menu is open, highlight with the focus ring (like Field).
        variant open {
            #[default]
            off(_t) {}
            on(t) {
                border_color: t.color.focus_ring(),
            }
        }
        variant size {
            sm(t) {
                padding_vertical: t.spacing.xs(),
                padding_horizontal: t.spacing.sm(),
                font_size: t.typography.body_sm_size(),
            }
            #[default]
            md(t) {
                padding_vertical: t.spacing.sm(),
                padding_horizontal: t.spacing.md(),
                font_size: t.typography.body_size(),
            }
            lg(t) {
                padding_vertical: t.spacing.md(),
                padding_horizontal: t.spacing.lg(),
                font_size: t.typography.body_lg_size(),
            }
        }
        state hovered(t) {
            border_color: t.color.border_hover(),
        }
        // Keyboard/pointer focus paints the same indigo ring as `open` and as
        // Field's `state focused` — the cross-platform focus indicator that
        // replaces the native macOS ring (suppressed in the pressable host).
        state focused(t) {
            border_color: t.color.focus_ring(),
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
            // NO font_family pin, deliberately. The menu portals under
            // `<body>` (outside the app tree's inherited font), which used
            // to need an `active_font_family()` pin here against the
            // browser-serif fallback — but a dynamic font is the one thing
            // that disqualifies a sheet from preminting, and the pin is
            // redundant now: the theme's default text font is DECLARED ON
            // THE DOCUMENT ROOT on every build (live publication +
            // premint's `--iy-default-font` hook), so a portal inherits it
            // through `<body>`, and the live static path additionally
            // folds it into fontless rules at apply. Pinned by
            // `select_menu_sheet_premints_and_rides_the_default_font`.
            background: t.color.surface(),
            border_radius: t.radius.md(),
            border_width: 1.0,
            border_color: t.color.border(),
            padding: t.spacing.xs(),
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
            color: t.color.text(),
            padding_vertical: t.spacing.xs(),
            padding_horizontal: t.spacing.sm(),
            border_radius: t.radius.sm(),
            font_size: t.typography.body_size(),
            text_align: TextAlign::Left,
            cursor: Cursor::Pointer,
        }
        // `on` = the COMMITTED selection (solid primary). `cursor` = the
        // keyboard cursor resting on a row that is NOT the selection
        // (Autocomplete's ArrowUp/ArrowDown position) — styled like the
        // hover state, since pointer hover and keyboard cursor mean the
        // same thing: "Enter/click commits this row". Two distinct looks so
        // an open menu never shows two solid rows (the old shape rendered
        // cursor and selection identically). Select uses only `on`/`off`.
        variant active {
            #[default]
            off(_t) {}
            cursor(t) {
                background: t.color.surface_alt(),
            }
            on(t) {
                background: t.intent.primary.solid_bg(),
                color: t.intent.primary.solid_text(),
            }
        }
        state hovered(t) {
            background: t.color.surface_alt(),
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
            background: t.color.surface(),
            color: t.color.text(),
            border_radius: t.radius.md(),
            border_width: 1.0,
            border_color: t.color.border(),
            padding_vertical: t.spacing.sm(),
            padding_left: t.spacing.md(),
            // Reserve room for the chevron pinned over the right edge.
            padding_right: t.spacing.xl(),
            font_size: t.typography.body_size(),
            text_align: TextAlign::Left,
        }
        variant size {
            sm(t) {
                padding_vertical: t.spacing.xs(),
                padding_left: t.spacing.sm(),
                padding_right: t.spacing.lg(),
                font_size: t.typography.body_sm_size(),
            }
            #[default]
            md(t) {
                padding_vertical: t.spacing.sm(),
                padding_left: t.spacing.md(),
                padding_right: t.spacing.xl(),
                font_size: t.typography.body_size(),
            }
            lg(t) {
                padding_vertical: t.spacing.md(),
                padding_left: t.spacing.lg(),
                padding_right: t.spacing.xl(),
                font_size: t.typography.body_lg_size(),
            }
        }
        state focused(t) {
            border_color: t.color.focus_ring(),
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
            right: t.spacing.sm(),
            top: Length::Px(0.0),
            bottom: Length::Px(0.0),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            color: t.color.text_muted(),
            font_size: t.typography.body_size(),
        }
    }
}

stylesheet! {
    pub AutocompleteEmpty<IdeaThemeRef> {
        base(t) {
            color: t.color.text_muted(),
            padding_vertical: t.spacing.xs(),
            padding_horizontal: t.spacing.sm(),
            font_size: t.typography.body_size(),
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
            padding: t.spacing.sm(),
            border_radius: t.radius.pill(),
            font_size: t.typography.body_size(),
            font_weight: FontWeight::SemiBold,
            text_align: TextAlign::Center,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
        }
        variant size {
            sm(t) {
                padding: t.spacing.xs(),
                font_size: t.typography.body_sm_size(),
                width: 24.0,
                height: 24.0,
            }
            #[default]
            md(t) {
                padding: t.spacing.sm(),
                font_size: t.typography.body_size(),
                width: 32.0,
                height: 32.0,
            }
            lg(t) {
                padding: t.spacing.md(),
                font_size: t.typography.body_lg_size(),
                width: 48.0,
                height: 48.0,
            }
        }
        // Identical `appearance` axis as Button — same 7 intents × 4 kinds.
        variant appearance {
            primary_solid(t) {
                background: t.intent.primary.solid_bg(),
                color: t.intent.primary.solid_text(),
                border_width: 0.0,
            }
            primary_soft(t) {
                background: t.intent.primary.soft_bg(),
                color: t.intent.primary.soft_text(),
                border_width: 0.0,
            }
            primary_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.primary.fg(),
                border_width: 1.0,
                border_color: t.intent.primary.border(),
            }
            primary_ghost(t) {
                background: Color("transparent".into()),
                color: t.intent.primary.fg(),
                border_width: 0.0,
            }
            secondary_solid(t) {
                background: t.intent.secondary.solid_bg(),
                color: t.intent.secondary.solid_text(),
                border_width: 0.0,
            }
            secondary_soft(t) {
                background: t.intent.secondary.soft_bg(),
                color: t.intent.secondary.soft_text(),
                border_width: 0.0,
            }
            secondary_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.secondary.fg(),
                border_width: 1.0,
                border_color: t.intent.secondary.border(),
            }
            secondary_ghost(t) {
                background: Color("transparent".into()),
                color: t.intent.secondary.fg(),
                border_width: 0.0,
            }
            #[default]
            neutral_solid(t) {
                background: t.intent.neutral.solid_bg(),
                color: t.intent.neutral.solid_text(),
                border_width: 0.0,
            }
            neutral_soft(t) {
                background: t.intent.neutral.soft_bg(),
                color: t.intent.neutral.soft_text(),
                border_width: 0.0,
            }
            neutral_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.neutral.fg(),
                border_width: 1.0,
                border_color: t.intent.neutral.border(),
            }
            neutral_ghost(t) {
                background: Color("transparent".into()),
                color: t.intent.neutral.fg(),
                border_width: 0.0,
            }
            success_solid(t) {
                background: t.intent.success.solid_bg(),
                color: t.intent.success.solid_text(),
                border_width: 0.0,
            }
            success_soft(t) {
                background: t.intent.success.soft_bg(),
                color: t.intent.success.soft_text(),
                border_width: 0.0,
            }
            success_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.success.fg(),
                border_width: 1.0,
                border_color: t.intent.success.border(),
            }
            success_ghost(t) {
                background: Color("transparent".into()),
                color: t.intent.success.fg(),
                border_width: 0.0,
            }
            danger_solid(t) {
                background: t.intent.danger.solid_bg(),
                color: t.intent.danger.solid_text(),
                border_width: 0.0,
            }
            danger_soft(t) {
                background: t.intent.danger.soft_bg(),
                color: t.intent.danger.soft_text(),
                border_width: 0.0,
            }
            danger_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.danger.fg(),
                border_width: 1.0,
                border_color: t.intent.danger.border(),
            }
            danger_ghost(t) {
                background: Color("transparent".into()),
                color: t.intent.danger.fg(),
                border_width: 0.0,
            }
            warning_solid(t) {
                background: t.intent.warning.solid_bg(),
                color: t.intent.warning.solid_text(),
                border_width: 0.0,
            }
            warning_soft(t) {
                background: t.intent.warning.soft_bg(),
                color: t.intent.warning.soft_text(),
                border_width: 0.0,
            }
            warning_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.warning.fg(),
                border_width: 1.0,
                border_color: t.intent.warning.border(),
            }
            warning_ghost(t) {
                background: Color("transparent".into()),
                color: t.intent.warning.fg(),
                border_width: 0.0,
            }
            info_solid(t) {
                background: t.intent.info.solid_bg(),
                color: t.intent.info.solid_text(),
                border_width: 0.0,
            }
            info_soft(t) {
                background: t.intent.info.soft_bg(),
                color: t.intent.info.soft_text(),
                border_width: 0.0,
            }
            info_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.info.fg(),
                border_width: 1.0,
                border_color: t.intent.info.border(),
            }
            info_ghost(t) {
                background: Color("transparent".into()),
                color: t.intent.info.fg(),
                border_width: 0.0,
            }
        }
        state hovered(_t) {
            opacity: 0.92,
        }
        state pressed(_t) {
            opacity: 0.85,
        }
        // Keyboard/pointer focus paints an indigo ring — the cross-platform
        // focus indicator that replaces AppKit's native ring (now suppressed on
        // the pressable host). `border_width: 1` also covers the borderless
        // (solid/soft/ghost) variants, which reserve no border of their own;
        // the outlined variants just recolor their existing 1px border. On
        // macOS the CALayer border draws inside the bounds and never nudges the
        // Taffy-positioned content; on web border-box keeps the outer size fixed.
        state focused(t) {
            border_width: 1.0,
            border_color: t.color.focus_ring(),
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
            border_radius: t.radius.pill(),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            // Hug: a row of mixed-size avatars centers rather than
            // top-aligning under the parent's default `align-items: stretch`.
            // Base rule, not a computed layer — it's constant, and a constant
            // closure blocks premint for the whole sheet.
            align_self: runtime_core::AlignSelf::Center,
            overflow: runtime_core::Overflow::Hidden,
            // Default to neutral wash so a no-prop Avatar reads as a
            // generic placeholder rather than a colored chip.
            background: t.intent.neutral.soft_bg(),
            color: t.intent.neutral.soft_text(),
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
                background: t.intent.neutral.soft_bg(),
                color: t.intent.neutral.soft_text(),
            }
            primary(t) {
                background: t.intent.primary.soft_bg(),
                color: t.intent.primary.soft_text(),
            }
            secondary(t) {
                background: t.intent.secondary.soft_bg(),
                color: t.intent.secondary.soft_text(),
            }
            success(t) {
                background: t.intent.success.soft_bg(),
                color: t.intent.success.soft_text(),
            }
            danger(t) {
                background: t.intent.danger.soft_bg(),
                color: t.intent.danger.soft_text(),
            }
            warning(t) {
                background: t.intent.warning.soft_bg(),
                color: t.intent.warning.soft_text(),
            }
            info(t) {
                background: t.intent.info.soft_bg(),
                color: t.intent.info.soft_text(),
            }
        }
        transitions {
            background: 250ms EaseInOut,
            color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    // The photo inside the disc. Fill + `Cover`, because the container
    // is a fixed-size clipped circle: an UNSIZED image renders at its
    // natural pixel size and the disc's `overflow: hidden` shows only
    // its center — a 300px avatar in a 40px disc read as a heavy zoom
    // onto the middle of the face. Cover keeps the disc filled for any
    // source aspect without distortion.
    pub AvatarImage<IdeaThemeRef> {
        base(_t) {
            width: Length::pct(100.0),
            height: Length::pct(100.0),
            object_fit: runtime_core::ObjectFit::Cover,
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
            sm(t) { font_size: t.typography.body_sm_size(), line_height: 32.0 }
            #[default]
            md(t) { font_size: t.typography.body_size(), line_height: 40.0 }
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
            gap: t.spacing.xs(),
            padding_vertical: t.spacing.xs(),
            padding_horizontal: t.spacing.sm(),
            border_radius: t.radius.pill(),
        }
        variant appearance {
            primary_solid(t) {
                background: t.intent.primary.solid_bg(),
                color: t.intent.primary.solid_text(),
                border_width: 0.0,
            }
            primary_soft(t) {
                background: t.intent.primary.soft_bg(),
                color: t.intent.primary.soft_text(),
                border_width: 0.0,
            }
            primary_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.primary.fg(),
                border_width: 1.0,
                border_color: t.intent.primary.border(),
            }
            secondary_solid(t) {
                background: t.intent.secondary.solid_bg(),
                color: t.intent.secondary.solid_text(),
                border_width: 0.0,
            }
            secondary_soft(t) {
                background: t.intent.secondary.soft_bg(),
                color: t.intent.secondary.soft_text(),
                border_width: 0.0,
            }
            secondary_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.secondary.fg(),
                border_width: 1.0,
                border_color: t.intent.secondary.border(),
            }
            neutral_solid(t) {
                background: t.intent.neutral.solid_bg(),
                color: t.intent.neutral.solid_text(),
                border_width: 0.0,
            }
            #[default]
            neutral_soft(t) {
                background: t.intent.neutral.soft_bg(),
                color: t.intent.neutral.soft_text(),
                border_width: 0.0,
            }
            neutral_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.neutral.fg(),
                border_width: 1.0,
                border_color: t.intent.neutral.border(),
            }
            success_solid(t) {
                background: t.intent.success.solid_bg(),
                color: t.intent.success.solid_text(),
                border_width: 0.0,
            }
            success_soft(t) {
                background: t.intent.success.soft_bg(),
                color: t.intent.success.soft_text(),
                border_width: 0.0,
            }
            success_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.success.fg(),
                border_width: 1.0,
                border_color: t.intent.success.border(),
            }
            danger_solid(t) {
                background: t.intent.danger.solid_bg(),
                color: t.intent.danger.solid_text(),
                border_width: 0.0,
            }
            danger_soft(t) {
                background: t.intent.danger.soft_bg(),
                color: t.intent.danger.soft_text(),
                border_width: 0.0,
            }
            danger_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.danger.fg(),
                border_width: 1.0,
                border_color: t.intent.danger.border(),
            }
            warning_solid(t) {
                background: t.intent.warning.solid_bg(),
                color: t.intent.warning.solid_text(),
                border_width: 0.0,
            }
            warning_soft(t) {
                background: t.intent.warning.soft_bg(),
                color: t.intent.warning.soft_text(),
                border_width: 0.0,
            }
            warning_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.warning.fg(),
                border_width: 1.0,
                border_color: t.intent.warning.border(),
            }
            info_solid(t) {
                background: t.intent.info.solid_bg(),
                color: t.intent.info.solid_text(),
                border_width: 0.0,
            }
            info_soft(t) {
                background: t.intent.info.soft_bg(),
                color: t.intent.info.soft_text(),
                border_width: 0.0,
            }
            info_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.info.fg(),
                border_width: 1.0,
                border_color: t.intent.info.border(),
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
            font_size: t.typography.body_sm_size(),
            font_weight: FontWeight::SemiBold,
            letter_spacing: 0.3,
        }
    }
}

stylesheet! {
    pub TagClose<IdeaThemeRef> {
        base(t) {
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
            font_size: t.typography.body_size(),
            font_weight: FontWeight::Bold,
            text_align: TextAlign::Center,
            line_height: 14.0,
            width: 16.0,
            height: 16.0,
            border_radius: t.radius.pill(),
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
            gap: t.spacing.md(),
            padding: t.spacing.lg(),
            border_radius: t.radius.md(),
        }
        variant appearance {
            primary_solid(t) {
                background: t.intent.primary.solid_bg(),
                color: t.intent.primary.solid_text(),
                border_width: 0.0,
            }
            primary_soft(t) {
                background: t.intent.primary.soft_bg(),
                color: t.intent.primary.soft_text(),
                border_width: 0.0,
            }
            primary_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.primary.fg(),
                border_width: 1.0,
                border_color: t.intent.primary.border(),
            }
            secondary_solid(t) {
                background: t.intent.secondary.solid_bg(),
                color: t.intent.secondary.solid_text(),
                border_width: 0.0,
            }
            secondary_soft(t) {
                background: t.intent.secondary.soft_bg(),
                color: t.intent.secondary.soft_text(),
                border_width: 0.0,
            }
            secondary_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.secondary.fg(),
                border_width: 1.0,
                border_color: t.intent.secondary.border(),
            }
            neutral_solid(t) {
                background: t.intent.neutral.solid_bg(),
                color: t.intent.neutral.solid_text(),
                border_width: 0.0,
            }
            #[default]
            neutral_soft(t) {
                background: t.intent.neutral.soft_bg(),
                color: t.intent.neutral.soft_text(),
                border_width: 0.0,
            }
            neutral_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.neutral.fg(),
                border_width: 1.0,
                border_color: t.intent.neutral.border(),
            }
            success_solid(t) {
                background: t.intent.success.solid_bg(),
                color: t.intent.success.solid_text(),
                border_width: 0.0,
            }
            success_soft(t) {
                background: t.intent.success.soft_bg(),
                color: t.intent.success.soft_text(),
                border_width: 0.0,
            }
            success_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.success.fg(),
                border_width: 1.0,
                border_color: t.intent.success.border(),
            }
            danger_solid(t) {
                background: t.intent.danger.solid_bg(),
                color: t.intent.danger.solid_text(),
                border_width: 0.0,
            }
            danger_soft(t) {
                background: t.intent.danger.soft_bg(),
                color: t.intent.danger.soft_text(),
                border_width: 0.0,
            }
            danger_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.danger.fg(),
                border_width: 1.0,
                border_color: t.intent.danger.border(),
            }
            warning_solid(t) {
                background: t.intent.warning.solid_bg(),
                color: t.intent.warning.solid_text(),
                border_width: 0.0,
            }
            warning_soft(t) {
                background: t.intent.warning.soft_bg(),
                color: t.intent.warning.soft_text(),
                border_width: 0.0,
            }
            warning_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.warning.fg(),
                border_width: 1.0,
                border_color: t.intent.warning.border(),
            }
            info_solid(t) {
                background: t.intent.info.solid_bg(),
                color: t.intent.info.solid_text(),
                border_width: 0.0,
            }
            info_soft(t) {
                background: t.intent.info.soft_bg(),
                color: t.intent.info.soft_text(),
                border_width: 0.0,
            }
            info_outlined(t) {
                background: Color("transparent".into()),
                color: t.intent.info.fg(),
                border_width: 1.0,
                border_color: t.intent.info.border(),
            }
        }
        transitions {
            background: 250ms EaseInOut,
            color: 250ms EaseInOut,
            border_color: 250ms EaseInOut,
        }
    }
}

// (AlertTitle/AlertBody moved into idea-theme's `AlertSheetBuilder::build_text`
// — the title/body sheets need an enumerated per-tone `appearance` color
// axis, which a compile-time `stylesheet!` cannot declare over the
// runtime-extensible tone list.)

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
            // Literal: 2px is below the theme's smallest spacing step
            // (`spacing-xs` = 4px). It read `spacing-2xs` before — a name
            // nothing installs, so it always rendered this 2px.
            gap: Length::Px(2.0),
        }
    }
}

// =============================================================================
// Skeleton — muted placeholder block.
// =============================================================================

stylesheet! {
    pub Skeleton<IdeaThemeRef> {
        base(t) {
            background: t.color.surface_alt(),
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
            gap: t.spacing.xs(),
            border_bottom_width: 1.0,
            border_bottom_color: t.color.border(),
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
            color: t.color.text_muted(),
            padding_vertical: t.spacing.sm(),
            padding_horizontal: t.spacing.md(),
            font_weight: FontWeight::Medium,
            font_size: t.typography.body_size(),
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
                color: t.color.text(),
                border_bottom_color: t.intent.primary.solid_bg(),
            }
        }
        // Hover/press now carry a translucent surface scrim (not just a text
        // brighten) so tabs/segments read as interactive controls — the
        // toolbar-button feel. `state` blocks are global (appearance-blind), but
        // a neutral surface wash reads on the transparent-resting tab base.
        state hovered(t) {
            color: t.color.text(),
            background: t.color.surface_alt(),
        }
        // Focus mirrors hover (a surface wash), not a box border — a rectangular
        // ring reads wrong on an underline tab. Replaces the native macOS ring.
        state focused(t) {
            color: t.color.text(),
            background: t.color.surface_alt(),
        }
        state pressed(t) {
            color: t.color.text(),
            background: t.color.border(),
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
            color: t.color.text_muted(),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
            padding_vertical: t.spacing.sm(),
            padding_horizontal: t.spacing.md(),
            border_radius: t.radius.md(),
            font_weight: FontWeight::Medium,
            font_size: t.typography.body_size(),
        }
        variant active {
            #[default]
            off(_t) {}
            on(t) {
                color: t.color.text(),
                background: t.color.surface_alt(),
            }
        }
        state hovered(t) {
            color: t.color.text(),
            background: t.color.surface_alt(),
        }
        // Focus mirrors hover (chip wash) — replaces the native macOS ring.
        state focused(t) {
            color: t.color.text(),
            background: t.color.surface_alt(),
        }
        state pressed(t) {
            background: t.color.border(),
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
            border_radius: t.radius.pill(),
            background: t.color.text_muted(),
        }
        variant active {
            #[default]
            off(_t) {}
            on(t) {
                background: t.intent.primary.solid_bg(),
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
            padding_vertical: t.spacing.lg(),
            flex_direction: FlexDirection::Column,
            gap: t.spacing.md(),
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
            background: t.color.surface(),
            padding: t.spacing.lg(),
            border_radius: t.radius.lg(),
            border_width: 1.0,
            border_color: t.color.border(),
            gap: t.spacing.md(),
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
            background: t.color.surface(),
            padding: t.spacing.sm(),
            border_radius: t.radius.md(),
            border_width: 1.0,
            border_color: t.color.border(),
            gap: t.spacing.xs(),
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
        base(t) {
            background: t.color.surface(),
            border_top_width: 1.0,
            border_right_width: 1.0,
            border_bottom_width: 1.0,
            border_left_width: 1.0,
            border_top_color: t.color.border(),
            border_right_color: t.color.border(),
            border_bottom_color: t.color.border(),
            border_left_color: t.color.border(),
            border_top_left_radius: t.radius.lg(),
            border_top_right_radius: t.radius.lg(),
            border_bottom_left_radius: t.radius.lg(),
            border_bottom_right_radius: t.radius.lg(),
        }
        // Scroll-x axis: clip children to the rounded frame. Load-
        // bearing there — the sheet lands on the surface WRAPPER (a
        // plain view) around the scroller, and without the clip the
        // scrolling columns paint square over the rounded corners.
        // NOT in `base`: on a plain table this sheet lands on the
        // `<table>` itself, whose `border-collapse: collapse` outer
        // border straddles the box edge — `overflow: hidden` there
        // clips the border's outer half (visibly thinned edges).
        variant scrolling {
            #[default]
            off(_t) {}
            on(_t) { overflow: Overflow::Hidden }
        }
        transitions {
            background: 250ms EaseInOut,
            border_top_color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub TableHeadCell<IdeaThemeRef> {
        base(t) {
            background: t.color.surface_alt(),
            padding_vertical: t.spacing.md(),
            padding_horizontal: t.spacing.lg(),
            border_bottom_width: 1.0,
            border_bottom_color: t.color.border(),
            // Override the browser UA default `th { text-align: center }`.
            // The inner text node shrink-wraps (display: inline), so its
            // own `text_align: Left` can't win — the cell's alignment is
            // what positions the inline span. Pin it Left so header cells
            // match body cells on web (native is unaffected: there the
            // text node's alignment already applies). See `TableBodyCell`.
            text_align: TextAlign::Left,
        }
        // Clickable-row axes — see `TableBodyCell` for the rationale.
        variant interactive {
            #[default]
            off(_t) {}
            on(_t) { cursor: Cursor::Pointer }
        }
        variant row_hovered {
            #[default]
            off(_t) {}
            on(t) { background: t.color.surface_alt() }
        }
        // Frozen-column axis (`TableCell(pinned = …)` in a
        // `Table(scroll_x = true)`). `position: Sticky` + the zero
        // inset pins the cell inside the table's horizontal scroller
        // on every backend — the browser natively on web, the shared
        // sticky registry (which also raises pinned cells above the
        // content sliding beneath them) on native. The head cell's own
        // `surface_alt` background already makes it opaque; the inner-
        // edge hairline marks where content slides underneath.
        variant pinned {
            #[default]
            none(_t) {}
            left(t) {
                position: Position::Sticky,
                left: 0,
                border_right_width: 1.0,
                border_right_color: t.color.border(),
            }
            right(t) {
                position: Position::Sticky,
                right: 0,
                border_left_width: 1.0,
                border_left_color: t.color.border(),
            }
        }
        // Drag-and-drop feedback VOCABULARY for author-wired row drag
        // (idea-ui ships no dnd behavior — see the Table component
        // docs): `dragging` dims an in-flight row, `drop_target`
        // highlights the slot under it. Inert until a custom
        // implementation selects them via `table::set_cell_style` on
        // the base application. Shared-signal axes like `row_hovered`.
        variant dragging {
            #[default]
            off(_t) {}
            on(_t) { opacity: 0.4 }
        }
        variant drop_target {
            #[default]
            off(_t) {}
            on(t) { background: t.color.surface_alt() }
        }
        transitions {
            background: 250ms EaseInOut,
            border_bottom_color: 250ms EaseInOut,
            opacity: 150ms EaseInOut,
        }
    }
}

stylesheet! {
    pub TableBodyCell<IdeaThemeRef> {
        base(t) {
            padding_vertical: t.spacing.md(),
            padding_horizontal: t.spacing.lg(),
            border_bottom_width: 1.0,
            border_bottom_color: t.color.border(),
            // Explicit (matches the UA `td` default) so head + body cells
            // share one alignment source of truth — see `TableHeadCell`.
            text_align: TextAlign::Left,
        }
        // Clickable-row axes (`TableRow` with `on_row_click`). Enumerated
        // variants rather than runtime `with_overrides` so every arm has
        // build-time CSS and the flip is a preminted CLASS SWAP — the
        // override form kept every table cell on the live engine, which
        // was one of the last two `--premint-only` blockers on the docs
        // corpus. `row_hovered` is a shared-signal axis, NOT a
        // `state hovered` pseudo: hovering ANY cell highlights the WHOLE
        // row via the row's one signal, which per-cell `:hover` cannot
        // express — see `components::table::make_row_cell_interactive`.
        variant interactive {
            #[default]
            off(_t) {}
            on(_t) { cursor: Cursor::Pointer }
        }
        variant row_hovered {
            #[default]
            off(_t) {}
            on(t) { background: t.color.surface_alt() }
        }
        // Frozen-column axis — see `TableHeadCell`. A body cell has no
        // background of its own, so the pinned arms pin an OPAQUE
        // surface background too: without it the cells sliding
        // underneath would show straight through the frozen column.
        variant pinned {
            #[default]
            none(_t) {}
            left(t) {
                position: Position::Sticky,
                left: 0,
                background: t.color.surface(),
                border_right_width: 1.0,
                border_right_color: t.color.border(),
            }
            right(t) {
                position: Position::Sticky,
                right: 0,
                background: t.color.surface(),
                border_left_width: 1.0,
                border_left_color: t.color.border(),
            }
        }
        // Author-wired drag-and-drop feedback vocabulary — see
        // `TableHeadCell`.
        variant dragging {
            #[default]
            off(_t) {}
            on(_t) { opacity: 0.4 }
        }
        variant drop_target {
            #[default]
            off(_t) {}
            on(t) { background: t.color.surface_alt() }
        }
        transitions {
            border_bottom_color: 250ms EaseInOut,
            // Fade the whole-row hover highlight the `row_hovered` axis
            // flips on/off.
            background: 150ms EaseInOut,
            opacity: 150ms EaseInOut,
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
        base(t) {
            font_size: 11.0,
            font_weight: FontWeight::SemiBold,
            letter_spacing: 0.8,
            text_transform: TextTransform::Uppercase,
            color: t.color.text_muted(),
            text_align: TextAlign::Left,
        }
        transitions {
            color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub TableBodyText<IdeaThemeRef> {
        base(t) {
            font_size: 14.0,
            color: t.color.text(),
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
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::FlexStart,
            gap: t.spacing.sm(),
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
        base(t) {
            background: t.color.surface(),
            border_top_width: 1.0,
            border_right_width: 1.0,
            border_bottom_width: 1.0,
            border_left_width: 1.0,
            border_top_color: t.color.border(),
            border_right_color: t.color.border(),
            border_bottom_color: t.color.border(),
            border_left_color: t.color.border(),
            border_top_left_radius: t.radius.lg(),
            border_top_right_radius: t.radius.lg(),
            border_bottom_left_radius: t.radius.lg(),
            border_bottom_right_radius: t.radius.lg(),
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
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            padding_vertical: t.spacing.md(),
            padding_horizontal: t.spacing.lg(),
            gap: t.spacing.sm(),
            color: t.color.text(),
            font_size: 14.0,
            font_weight: FontWeight::SemiBold,
            text_align: TextAlign::Left,
        }
        state hovered(t) {
            background: t.color.surface_alt(),
        }
        // Focus mirrors hover (row wash) — replaces the native macOS ring.
        state focused(t) {
            background: t.color.surface_alt(),
        }
        transitions {
            background: 150ms EaseOut,
            color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub CollapsibleChevron<IdeaThemeRef> {
        base(t) {
            font_size: 13.0,
            color: t.color.text_muted(),
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
        base(t) {
            flex_direction: FlexDirection::Column,
            padding_horizontal: t.spacing.lg(),
            border_top_width: 1.0,
            border_top_color: t.color.border(),
            gap: t.spacing.sm(),
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
            shown(t) {
                max_height: Length::Px(2000.0),
                padding_top: t.spacing.md(),
                padding_bottom: t.spacing.md(),
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
        base(t) {
            flex_direction: FlexDirection::Column,
            padding_horizontal: t.spacing.lg(),
            border_top_width: 1.0,
            border_top_color: t.color.border(),
            gap: t.spacing.sm(),
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
            shown(t) {
                padding_top: t.spacing.md(),
                padding_bottom: t.spacing.md(),
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
        base(t) {
            background: t.color.surface(),
            border_top_width: 1.0,
            border_right_width: 1.0,
            border_bottom_width: 1.0,
            border_left_width: 1.0,
            border_top_color: t.color.border(),
            border_right_color: t.color.border(),
            border_bottom_color: t.color.border(),
            border_left_color: t.color.border(),
            border_top_left_radius: t.radius.lg(),
            border_top_right_radius: t.radius.lg(),
            border_bottom_left_radius: t.radius.lg(),
            border_bottom_right_radius: t.radius.lg(),
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
        base(t) {
            flex_direction: FlexDirection::Column,
            border_top_width: 1.0,
            border_top_color: t.color.border(),
        }
        transitions {
            border_top_color: 250ms EaseInOut,
        }
    }
}

// =============================================================================
// Tooltip — compact bubble
//
// TWO sheets, and the split is load-bearing. `TooltipBubble` styles the
// bubble's *box* (a `view`); `TooltipBubbleText` styles the label inside it.
// Painting the box on the text node instead gives a per-LINE background on
// backends that lay text out inline (web: a wrapped `text` paints one ragged
// rect per line, not one bubble) — the box has to be a real container so
// every backend draws the same single rounded rect.
// =============================================================================

stylesheet! {
    pub TooltipBubble<IdeaThemeRef> {
        base(t) {
            background: t.color.surface(),
            border_width: 1.0,
            border_color: t.color.border(),
            padding_vertical: t.spacing.xs(),
            padding_horizontal: t.spacing.sm(),
            border_radius: t.radius.md(),
            flex_direction: FlexDirection::Column,
            // Clamps the bubble so a long hint wraps into a readable column
            // instead of running the width of the viewport.
            max_width: 260.0,
            shadow: runtime_core::Shadow {
                x: 0.0, y: 4.0, blur: 12.0, color: Color("rgba(15, 17, 21, 0.22)".into()),
            },
        }
        transitions {
            background: 250ms EaseInOut,
            border_color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub TooltipBubbleText<IdeaThemeRef> {
        base(t) {
            color: t.color.text(),
            font_size: t.typography.body_sm_size(),
            line_height: 18.0,
            text_align: TextAlign::Left,
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
            gap: t.spacing.sm(),
            background: Color("transparent".into()),
            color: t.color.text(),
            padding_vertical: t.spacing.xs(),
            padding_horizontal: t.spacing.sm(),
            border_radius: t.radius.sm(),
            font_size: t.typography.body_size(),
            text_align: TextAlign::Left,
            cursor: Cursor::Pointer,
        }
        variant active {
            #[default]
            off(_t) {}
            on(t) { background: t.color.surface_alt() }
        }
        state hovered(t) {
            background: t.color.surface_alt(),
        }
        // Focus mirrors hover (row wash) — replaces the native macOS ring.
        state focused(t) {
            background: t.color.surface_alt(),
        }
        transitions { background: 120ms EaseOut }
    }
}

stylesheet! {
    pub MenuLabel<IdeaThemeRef> {
        base(t) {
            color: t.color.text_muted(),
            font_size: t.typography.overline_size(),
            font_weight: FontWeight::SemiBold,
            letter_spacing: 0.8,
            text_transform: TextTransform::Uppercase,
            padding_vertical: t.spacing.xs(),
            padding_horizontal: t.spacing.sm(),
        }
    }
}

stylesheet! {
    pub MenuSeparator<IdeaThemeRef> {
        base(t) {
            height: 1.0,
            width: Length::pct(100.0),
            background: t.color.border(),
            margin_top: 4.0,
            margin_bottom: 4.0,
        }
    }
}

// Trailing chevron for SubMenu rows.
stylesheet! {
    pub MenuChevron<IdeaThemeRef> {
        base(t) {
            color: t.color.text_muted(),
            font_size: t.typography.body_size(),
        }
    }
}

// Leading checkbox for CHECKABLE menu rows (multi-select flyouts and
// composed rows via `menu_checkbox`). Box + mark are separate sheets:
// native text nodes don't inherit color from a parent, so the mark
// carries its own color and flips it with the same variant key.
stylesheet! {
    pub MenuCheckbox<IdeaThemeRef> {
        base(t) {
            width: 16.0,
            height: 16.0,
            flex_shrink: 0.0,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            border_radius: t.radius.sm(),
            border_width: 1.5,
        }
        variant checked {
            #[default]
            off(t) {
                border_color: t.color.border(),
                background: Color("transparent".into()),
            }
            on(t) {
                border_color: t.intent.primary.solid_bg(),
                background: t.intent.primary.solid_bg(),
            }
        }
        transitions { background: 120ms EaseOut, border_color: 120ms EaseOut }
    }
}

stylesheet! {
    // The ✓ inside the box. Always rendered, transparent while off, so
    // toggling never reflows the row.
    pub MenuCheckMark<IdeaThemeRef> {
        base(_t) {
            font_size: 11.0,
            font_weight: FontWeight::Bold,
            line_height: 12.0,
        }
        variant checked {
            #[default]
            off(_t) { color: Color("transparent".into()) }
            on(t) { color: t.intent.primary.solid_text() }
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
            gap: t.spacing.xs(),
        }
    }
}

stylesheet! {
    pub BreadcrumbItem<IdeaThemeRef> {
        base(t) {
            color: t.color.text_muted(),
            font_size: t.typography.body_sm_size(),
            background: Color("transparent".into()),
            padding_vertical: 0.0,
            padding_horizontal: t.spacing.xs(),
            border_radius: t.radius.sm(),
        }
        variant current {
            #[default]
            off(_t) {}
            on(t) {
                color: t.color.text(),
                font_weight: FontWeight::SemiBold,
            }
        }
        // A linked (non-current) crumb shows a pointer. Variant rather than a
        // computed layer so the rule premints — see `ListItemRow::interactive`.
        variant interactive {
            #[default]
            off(_t) {}
            on(_t) { cursor: Cursor::Pointer }
        }
        state hovered(t) { color: t.color.text() }
        // Focus mirrors hover (text brighten) — replaces the native macOS ring.
        state focused(t) { color: t.color.text() }
        transitions { color: 120ms EaseOut }
    }
}

stylesheet! {
    pub BreadcrumbSeparator<IdeaThemeRef> {
        base(t) {
            color: t.color.text_muted(),
            font_size: t.typography.body_sm_size(),
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
            gap: t.spacing.xs(),
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
            padding_horizontal: t.spacing.sm(),
            border_radius: t.radius.md(),
            background: Color("transparent".into()),
            color: t.color.text(),
            font_size: t.typography.body_sm_size(),
            font_weight: FontWeight::Medium,
            text_align: TextAlign::Center,
            cursor: Cursor::Pointer,
        }
        variant active {
            #[default]
            off(_t) {}
            on(t) {
                background: t.intent.primary.solid_bg(),
                color: t.intent.primary.solid_text(),
            }
        }
        state hovered(t) { background: t.color.surface_alt() }
        // Focus mirrors hover (surface wash) — replaces the native macOS ring.
        state focused(t) { background: t.color.surface_alt() }
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
            background: t.color.surface(),
            border_top_width: 1.0,
            border_right_width: 1.0,
            border_bottom_width: 1.0,
            border_left_width: 1.0,
            border_top_color: t.color.border(),
            border_right_color: t.color.border(),
            border_bottom_color: t.color.border(),
            border_left_color: t.color.border(),
            border_top_left_radius: t.radius.lg(),
            border_top_right_radius: t.radius.lg(),
            border_bottom_left_radius: t.radius.lg(),
            border_bottom_right_radius: t.radius.lg(),
            overflow: runtime_core::Overflow::Hidden,
        }
    }
}

stylesheet! {
    pub ListItemRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
            padding_vertical: t.spacing.sm(),
            padding_horizontal: t.spacing.md(),
            background: Color("transparent".into()),
            color: t.color.text(),
            font_size: t.typography.body_size(),
            text_align: TextAlign::Left,
        }
        variant divided {
            #[default]
            off(_t) {}
            on(t) {
                border_top_width: 1.0,
                border_top_color: t.color.border(),
            }
        }
        variant active {
            #[default]
            off(_t) {}
            on(t) { background: t.color.surface_alt() }
        }
        // A row with an `on_press` shows a pointer (the "anything pressable
        // shows a pointer" rule). A variant arm rather than a computed layer
        // so the rule premints — a constant closure defeats premint for the
        // whole sheet without expressing anything a variant can't.
        variant interactive {
            #[default]
            off(_t) {}
            on(_t) { cursor: Cursor::Pointer }
        }
        state hovered(t) { background: t.color.surface_alt() }
        // Focus mirrors hover (row wash) — replaces the native macOS ring.
        state focused(t) { background: t.color.surface_alt() }
        transitions { background: 120ms EaseOut }
    }
}

// =============================================================================
// Grid — N equal columns via real CSS grid (display: grid)
// =============================================================================
//
// The container is `display: grid`; the per-column `1fr` track list is
// attached by the `Grid` component's computed layer (it depends on the live
// `columns` count, which a static sheet can't express). `gap` applies to both
// row and column gaps. See `components/grid.rs`.

stylesheet! {
    pub GridContainer<IdeaThemeRef> {
        base(t) {
            display: DisplayKind::Grid,
            gap: t.spacing.md(),
        }
        variant gap {
            none(_t) { gap: Length::Px(0.0) }
            xs(t)    { gap: t.spacing.xs() }
            sm(t)    { gap: t.spacing.sm() }
            #[default]
            md(t)    { gap: t.spacing.md() }
            lg(t)    { gap: t.spacing.lg() }
            xl(t)    { gap: t.spacing.xl() }
        }
    }
}

// =============================================================================
// Link — inline navigational text
// =============================================================================

stylesheet! {
    pub LinkText<IdeaThemeRef> {
        base(t) {
            color: t.intent.primary.fg(),
            font_size: t.typography.body_size(),
            font_weight: FontWeight::Medium,
            // Web gets this free from the UA stylesheet (`a[href]` is
            // `cursor: pointer`); no native backend has an equivalent, and
            // the framework imposes no default cursor on any primitive. So
            // without declaring it the SAME author code shows a hand on web
            // and an arrow everywhere else — the divergence CLAUDE.md §7
            // exists to prevent. Declaring it converges them and stays
            // overridable, unlike the old hardcoded inline style.
            cursor: Cursor::Pointer,
        }
        state hovered(t) { color: t.intent.primary.solid_bg() }
        // Focus mirrors hover (color brighten) — an inline text link takes a
        // text affordance, not a box ring. Replaces the native macOS ring.
        state focused(t) { color: t.intent.primary.solid_bg() }
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

// =============================================================================
// Calendar family — month grid shared by Calendar / RangeCalendar and the
// DatePicker/DateInput popups
// =============================================================================
//
// `CalendarPanel` is the framed container (border off when embedded in a
// popup panel that already draws chrome). `CalendarDay` styles one grid
// cell across three composable axes: `sel` (selection/range role), `today`,
// and `muted` (outside the visible month). Cells are fixed 36×36 so the
// grid is stable across months and the range band reads as a continuous
// row (the `mid` arm squares its corners; `edge`/`on` keep the radius).

stylesheet! {
    pub CalendarPanel<IdeaThemeRef> {
        base(t) {
            background: t.color.surface(),
            flex_direction: FlexDirection::Column,
            gap: t.spacing.xs(),
            padding: t.spacing.sm(),
            border_radius: t.radius.md(),
            align_self: runtime_core::AlignSelf::FlexStart,
        }
        // `framed` — standalone (inline) calendars draw their own border;
        // popup embeddings turn it off (the menu panel already has one).
        variant framed {
            #[default]
            on(t) {
                border_width: 1.0,
                border_color: t.color.border(),
            }
            off(_t) {}
        }
    }
}

stylesheet! {
    pub CalendarHeader<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            gap: t.spacing.xs(),
        }
    }
}

// The pressable month/year title in the header — opens the month/year
// picker view. Styled like a quiet button.
stylesheet! {
    pub CalendarTitleButton<IdeaThemeRef> {
        base(t) {
            background: Color("transparent".into()),
            color: t.color.text(),
            font_size: t.typography.body_size(),
            font_weight: FontWeight::SemiBold,
            padding_vertical: t.spacing.xs(),
            padding_horizontal: t.spacing.sm(),
            border_radius: t.radius.sm(),
            cursor: Cursor::Pointer,
        }
        state hovered(t) {
            background: t.color.surface_alt(),
        }
        transitions { background: 150ms EaseOut }
    }
}

stylesheet! {
    pub CalendarWeekdayCell<IdeaThemeRef> {
        base(t) {
            width: 36.0,
            color: t.color.text_muted(),
            font_size: t.typography.body_sm_size(),
            font_weight: FontWeight::Medium,
            text_align: TextAlign::Center,
        }
    }
}

stylesheet! {
    pub CalendarWeekRow<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Row,
        }
    }
}

stylesheet! {
    pub CalendarDay<IdeaThemeRef> {
        base(t) {
            width: 36.0,
            height: 36.0,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            border_radius: t.radius.sm(),
            background: Color("transparent".into()),
            color: t.color.text(),
            font_size: t.typography.body_size(),
            cursor: Cursor::Pointer,
        }
        // Selection / range role of the cell.
        variant sel {
            #[default]
            off(_t) {}
            // The single selection, or a range endpoint.
            on(t) {
                background: t.intent.primary.solid_bg(),
                color: t.intent.primary.solid_text(),
            }
            // Interior of a selected range: soft wash, squared corners so
            // consecutive cells read as one band.
            mid(t) {
                background: t.intent.primary.soft_bg(),
                color: t.intent.primary.fg(),
                border_radius: Length::Px(0.0),
            }
        }
        variant today {
            #[default]
            off(_t) {}
            // Quiet marker that composes under every `sel` arm: a ring, not
            // a fill, so "today + selected" still reads as selected.
            on(t) {
                border_width: 1.0,
                border_color: t.intent.primary.fg(),
            }
        }
        // Leading/trailing days of the adjacent months.
        variant muted {
            #[default]
            off(_t) {}
            on(t) {
                color: t.color.text_muted(),
            }
        }
        // Un-pickable (min/max/disabled-fn). Rendered as a plain view (no
        // press handler), so this is a variant, not a pressed-state overlay.
        variant blocked {
            #[default]
            off(_t) {}
            on(_t) {
                opacity: 0.35,
                cursor: Cursor::Default,
            }
        }
        state hovered(t) {
            background: t.color.surface_alt(),
        }
        state focused(t) {
            border_width: 1.0,
            border_color: t.color.focus_ring(),
        }
        transitions {
            background: 120ms EaseOut,
            color: 120ms EaseOut,
        }
    }
}

// Month/year cells of the title-press zoomed-out view (3×4 month grid /
// 4×4 year grid).
stylesheet! {
    pub CalendarZoomCell<IdeaThemeRef> {
        base(t) {
            width: 63.0,
            height: 40.0,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            border_radius: t.radius.sm(),
            background: Color("transparent".into()),
            color: t.color.text(),
            font_size: t.typography.body_size(),
            cursor: Cursor::Pointer,
        }
        variant active {
            #[default]
            off(_t) {}
            on(t) {
                background: t.intent.primary.solid_bg(),
                color: t.intent.primary.solid_text(),
            }
        }
        state hovered(t) {
            background: t.color.surface_alt(),
        }
        transitions { background: 120ms EaseOut, color: 120ms EaseOut }
    }
}
