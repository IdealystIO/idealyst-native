//! Full-struct golden pins for `rules_to_css` / `rules_to_css_text`.
//!
//! The CSS writer is the single source of truth for web AND SSR class
//! bodies — its output feeds `content_key`-minted class names, so ANY
//! byte change silently splits web/SSR class identity and breaks
//! hydration adoption. These tests set EVERY `StyleRules` field (no
//! `..Default::default()` — a newly added field must fail compilation
//! here so its golden coverage is added deliberately) and pin the exact
//! output. If a test fails after an intentional format change, update
//! the expected string AND bump anything keyed off minted class names.

use runtime_shared::assets::{SystemFallback, Typeface, TypefaceId};
use runtime_shared::{
    AlignContent, AlignItems, AlignSelf, Color, Cursor, DisplayKind, Easing, FlexDirection,
    FlexWrap, FontFamily, FontStyle, FontWeight, Gradient, GradientKind, GradientStop,
    JustifyContent, Length, ObjectFit, Overflow, OverscrollBehavior, PointerEvents, Position,
    Shadow, StyleRules,
    TextAlign, TextTransform, Tokenized, TrackSize, Transform, Transition, UserSelect,
};

fn lit_color(s: &str) -> Option<Tokenized<Color>> {
    Some(Tokenized::Literal(Color(s.to_string())))
}

fn tok_color(name: &'static str, fallback: &str) -> Option<Tokenized<Color>> {
    Some(Tokenized::token(name, Color(fallback.to_string())))
}

fn px(v: f32) -> Option<Tokenized<Length>> {
    Some(Tokenized::Literal(Length::Px(v)))
}

fn pct(v: f32) -> Option<Tokenized<Length>> {
    Some(Tokenized::Literal(Length::Percent(v)))
}

fn tok_len(name: &'static str, fallback: Length) -> Option<Tokenized<Length>> {
    Some(Tokenized::token(name, fallback))
}

fn num(v: f32) -> Option<Tokenized<f32>> {
    Some(Tokenized::Literal(v))
}

fn tok_num(name: &'static str, fallback: f32) -> Option<Tokenized<f32>> {
    Some(Tokenized::token(name, fallback))
}

fn tr(ms: u32, easing: Easing) -> Option<Transition> {
    Some(Transition::new(ms, easing))
}

/// Every field set, exercising each formatter shape at least once:
/// literal + `var()` token colors/lengths/numbers, `calc()` px tokens,
/// grid tracks, every `Transform` variant, gradient, shadow, the
/// underline+strikethrough combined shorthand, the `-webkit-` user-select
/// double emit, the border `width`+`style: solid` pairing, and all 35
/// transition fields (covering every `Easing` arm).
fn maximal_rules() -> StyleRules {
    StyleRules {
        background: tok_color("color-surface", "#112233"),
        color: lit_color("#abcdef"),
        caret_color: lit_color("red"),
        font_size: tok_len("font-size-md", Length::Px(14.0)),
        display: Some(DisplayKind::Grid),
        grid_template_columns: Some(vec![
            TrackSize::Auto,
            TrackSize::MinContent,
            TrackSize::MaxContent,
            TrackSize::Fr(1.5),
            TrackSize::Px(120.0),
            TrackSize::Minmax(Box::new(TrackSize::MinContent), Box::new(TrackSize::Fr(1.0))),
        ]),
        flex_direction: Some(FlexDirection::Row),
        flex_wrap: Some(FlexWrap::WrapReverse),
        justify_content: Some(JustifyContent::SpaceBetween),
        align_items: Some(AlignItems::Baseline),
        align_content: Some(AlignContent::SpaceAround),
        gap: px(8.0),
        row_gap: pct(50.0),
        column_gap: tok_len("space-sm", Length::Px(4.0)),
        flex_grow: num(1.0),
        flex_shrink: tok_num("shrink-factor", 0.5),
        flex_basis: Some(Tokenized::Literal(Length::Auto)),
        align_self: Some(AlignSelf::Stretch),
        width: px(320.0),
        height: pct(100.0),
        min_width: px(0.0),
        min_height: tok_len("min-h", Length::Percent(10.0)),
        max_width: px(1280.5),
        max_height: Some(Tokenized::Literal(Length::Auto)),
        aspect_ratio: Some(1.777_777_8),
        padding_top: px(1.0),
        padding_right: px(2.5),
        padding_bottom: px(3.0),
        padding_left: tok_len("pad-l", Length::Px(4.0)),
        margin_top: px(5.0),
        margin_right: Some(Tokenized::Literal(Length::Auto)),
        margin_bottom: px(7.0),
        margin_left: pct(12.5),
        border_top_left_radius: px(9.0),
        border_top_right_radius: px(10.0),
        border_bottom_left_radius: tok_len("radius-md", Length::Px(6.0)),
        border_bottom_right_radius: px(12.0),
        border_top_width: num(1.0),
        border_right_width: num(2.0),
        border_bottom_width: tok_num("border-w", 3.0),
        border_left_width: num(0.5),
        border_top_color: lit_color("#000"),
        border_right_color: tok_color("color-border", "#ddd"),
        border_bottom_color: lit_color("rgba(0, 0, 0, 0.25)"),
        border_left_color: lit_color("#fff"),
        position: Some(Position::Sticky),
        top: px(0.0),
        right: px(16.0),
        bottom: pct(5.0),
        left: tok_len("inset-l", Length::Px(24.0)),
        font_family: Some(FontFamily::System("Inter, sans-serif".to_string())),
        font_weight: Some(FontWeight::SemiBold),
        font_style: Some(FontStyle::Italic),
        line_height: num(21.0),
        letter_spacing: tok_num("tracking", 0.25),
        text_align: Some(TextAlign::Center),
        underline: Some(true),
        strikethrough: Some(true),
        text_transform: Some(TextTransform::Uppercase),
        opacity: num(0.666_666_7),
        overflow: Some(Overflow::Hidden),
        overscroll_behavior: Some(OverscrollBehavior::Contain),
        object_fit: Some(ObjectFit::Cover),
        shadow: Some(Shadow {
            x: 0.0,
            y: 2.0,
            blur: 4.5,
            color: Color("rgba(0,0,0,0.5)".to_string()),
        }),
        text_shadow: Some(Shadow {
            x: 1.0,
            y: -1.0,
            blur: 2.0,
            color: Color("#222222".into()),
        }),
        background_gradient: Some(Gradient {
            kind: GradientKind::Linear { angle_deg: 45.0 },
            stops: vec![
                GradientStop { offset: 0.0, color: Color("#000000".to_string()) },
                GradientStop { offset: 0.335, color: Color("#808080".to_string()) },
                GradientStop { offset: 1.0, color: Color("#ffffff".to_string()) },
            ],
        }),
        transform: Some(vec![
            Transform::TranslateX(Length::Px(10.0)),
            Transform::TranslateY(Length::Percent(-50.0)),
            Transform::Scale(1.05),
            Transform::ScaleXY { x: 2.0, y: 0.5 },
            Transform::Rotate(45.0),
            Transform::SkewX(10.0),
            Transform::SkewY(-5.0),
        ]),
        transform_origin: Some((Length::Px(10.0), Length::Percent(50.0))),
        cursor: Some(Cursor::Grabbing),
        user_select: Some(UserSelect::None),
        pointer_events: Some(PointerEvents::None),
        background_transition: tr(100, Easing::Linear),
        color_transition: tr(110, Easing::Ease),
        caret_color_transition: tr(120, Easing::EaseIn),
        opacity_transition: tr(130, Easing::EaseOut),
        transform_transition: tr(140, Easing::EaseInOut),
        width_transition: tr(150, Easing::CubicBezier(0.4, 0.0, 0.2, 1.0)),
        height_transition: tr(160, Easing::EaseOut),
        max_width_transition: tr(170, Easing::EaseOut),
        max_height_transition: tr(180, Easing::EaseOut),
        min_width_transition: tr(190, Easing::EaseOut),
        min_height_transition: tr(200, Easing::EaseOut),
        top_transition: tr(210, Easing::EaseOut),
        right_transition: tr(220, Easing::EaseOut),
        bottom_transition: tr(230, Easing::EaseOut),
        left_transition: tr(240, Easing::EaseOut),
        padding_top_transition: tr(250, Easing::EaseOut),
        padding_right_transition: tr(260, Easing::EaseOut),
        padding_bottom_transition: tr(270, Easing::EaseOut),
        padding_left_transition: tr(280, Easing::EaseOut),
        margin_top_transition: tr(290, Easing::EaseOut),
        margin_right_transition: tr(300, Easing::EaseOut),
        margin_bottom_transition: tr(310, Easing::EaseOut),
        margin_left_transition: tr(320, Easing::EaseOut),
        border_top_left_radius_transition: tr(330, Easing::EaseOut),
        border_top_right_radius_transition: tr(340, Easing::EaseOut),
        border_bottom_left_radius_transition: tr(350, Easing::EaseOut),
        border_bottom_right_radius_transition: tr(360, Easing::EaseOut),
        border_top_width_transition: tr(370, Easing::EaseOut),
        border_right_width_transition: tr(380, Easing::EaseOut),
        border_bottom_width_transition: tr(390, Easing::EaseOut),
        border_left_width_transition: tr(400, Easing::EaseOut),
        border_top_color_transition: tr(410, Easing::EaseOut),
        border_right_color_transition: tr(420, Easing::EaseOut),
        border_bottom_color_transition: tr(430, Easing::EaseOut),
        border_left_color_transition: tr(440, Easing::EaseOut),
    }
}

/// The pinned body for [`maximal_rules`] with `ShadowKind::Box`.
const GOLDEN_BOX: &str = "display: grid; grid-template-columns: auto min-content max-content 1.5fr 120px minmax(min-content, 1fr); background: var(--color-surface, #112233); background-image: linear-gradient(45deg, #000000 0%, #808080 33.5%, #ffffff 100%); color: #abcdef; caret-color: red; font-size: var(--font-size-md, 14px); flex-direction: row; flex-wrap: wrap-reverse; justify-content: space-between; align-items: baseline; align-content: space-around; gap: 8px; row-gap: 50%; column-gap: var(--space-sm, 4px); flex-grow: 1; flex-shrink: var(--shrink-factor, 0.5); flex-basis: auto; align-self: stretch; width: 320px; height: 100%; min-width: 0px; min-height: var(--min-h, 10%); max-width: 1280.5px; max-height: auto; aspect-ratio: 1.778; padding-top: 1px; padding-right: 2.5px; padding-bottom: 3px; padding-left: var(--pad-l, 4px); margin-top: 5px; margin-right: auto; margin-bottom: 7px; margin-left: 12.5%; border-top-left-radius: 9px; border-top-right-radius: 10px; border-bottom-left-radius: var(--radius-md, 6px); border-bottom-right-radius: 12px; border-top-width: 1px; border-top-style: solid; border-right-width: 2px; border-right-style: solid; border-bottom-width: calc(var(--border-w, 3) * 1px); border-bottom-style: solid; border-left-width: 0.5px; border-left-style: solid; border-top-color: #000; border-right-color: var(--color-border, #ddd); border-bottom-color: rgba(0, 0, 0, 0.25); border-left-color: #fff; position: sticky; top: 0px; right: 16px; bottom: 5%; left: var(--inset-l, 24px); font-family: Inter, sans-serif; font-weight: 600; font-style: italic; line-height: 21px; letter-spacing: calc(var(--tracking, 0.25) * 1px); text-align: center; text-decoration-line: underline line-through; text-transform: uppercase; opacity: 0.667; overflow: hidden; overscroll-behavior: contain; object-fit: cover; box-shadow: 0px 2px 4.5px rgba(0,0,0,0.5); text-shadow: 1px -1px 2px #222222; transform: translateX(10px) translateY(-50%) scale(1.05) scale(2, 0.5) rotate(45deg) skewX(10deg) skewY(-5deg); transform-origin: 10px 50%; cursor: grabbing; -webkit-user-select: none; user-select: none; pointer-events: none; transition: background 100ms linear, color 110ms ease, caret-color 120ms ease-in, opacity 130ms ease-out, transform 140ms ease-in-out, width 150ms cubic-bezier(0.4, 0, 0.2, 1), height 160ms ease-out, max-width 170ms ease-out, max-height 180ms ease-out, min-width 190ms ease-out, min-height 200ms ease-out, top 210ms ease-out, right 220ms ease-out, bottom 230ms ease-out, left 240ms ease-out, padding-top 250ms ease-out, padding-right 260ms ease-out, padding-bottom 270ms ease-out, padding-left 280ms ease-out, margin-top 290ms ease-out, margin-right 300ms ease-out, margin-bottom 310ms ease-out, margin-left 320ms ease-out, border-top-left-radius 330ms ease-out, border-top-right-radius 340ms ease-out, border-bottom-left-radius 350ms ease-out, border-bottom-right-radius 360ms ease-out, border-top-width 370ms ease-out, border-right-width 380ms ease-out, border-bottom-width 390ms ease-out, border-left-width 400ms ease-out, border-top-color 410ms ease-out, border-right-color 420ms ease-out, border-bottom-color 430ms ease-out, border-left-color 440ms ease-out";

#[test]
fn golden_full_struct_box_shadow() {
    assert_eq!(css::rules_to_css(&maximal_rules()), GOLDEN_BOX);
}

// (The former `golden_full_struct_text_shadow` — a per-node-kind
// lowering of ONE shadow field — is gone with the `shadow`/`text_shadow`
// split: each field lowers to exactly one property on every node kind,
// pinned inside GOLDEN_BOX itself, which now carries both.)

/// The branches the maximal struct can't reach: flex auto-promotion
/// (display unset + a flex property), explicit `display: flex` pinning
/// `flex-direction: column`, `text-decoration-line: none` (explicit
/// `Some(false)`), the radial gradient form, and the quoted
/// `FontFamily::Typeface` family name.
#[test]
fn golden_edge_branches() {
    static GOLDEN_FACE: Typeface = Typeface {
        id: TypefaceId(7),
        family_name: "Golden Sans",
        faces: &[],
        fallback: SystemFallback::SansSerif,
    };

    let mut auto_promoted = StyleRules::default();
    auto_promoted.gap = px(4.0);
    assert_eq!(
        css::rules_to_css(&auto_promoted),
        "display: flex; flex-direction: column; gap: 4px"
    );

    let mut explicit_flex = StyleRules::default();
    explicit_flex.display = Some(DisplayKind::Flex);
    assert_eq!(css::rules_to_css(&explicit_flex), "display: flex; flex-direction: column");

    let mut deco_off = StyleRules::default();
    deco_off.underline = Some(false);
    assert_eq!(css::rules_to_css(&deco_off), "text-decoration-line: none");

    let mut radial = StyleRules::default();
    radial.background_gradient = Some(Gradient {
        kind: GradientKind::Radial {
            center: (0.25, 0.75),
            radius: 1.5,
            extent: runtime_shared::RadialExtent::FarthestCorner,
        },
        stops: vec![
            GradientStop { offset: 0.0, color: Color("#ff0000".to_string()) },
            GradientStop { offset: 1.0, color: Color("#0000ff".to_string()) },
        ],
    });
    radial.font_family = Some(FontFamily::Typeface(GOLDEN_FACE));
    assert_eq!(
        css::rules_to_css(&radial),
        "background-image: radial-gradient(ellipse 106.066% 106.066% at 25% 75%, \
         #ff0000 0%, #0000ff 100%); font-family: \"Golden Sans\""
    );
}
