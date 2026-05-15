//! Style declarations and theme infrastructure.
//!
//! The framework owns the data model — what a "style" looks like, what
//! variant axes exist, how the active theme propagates — but does **not**
//! own the rendering strategy. Each backend interprets a `StyleRules`
//! value however suits its platform:
//!
//! - **Web** can lazily mint CSS classes per unique rule set and swap
//!   `className` on the node when the style changes.
//! - **iOS** can update `CALayer` / `UIView` properties directly.
//! - **Android** can call `View` setters or apply theme attributes.
//!
//! # Themes
//!
//! Stylesheets are **closures** from the active theme to concrete
//! `StyleRules`. The stylesheet's `base(|theme: &MyTheme| StyleRules { ... })`
//! takes a typed reference to the app's theme and returns a rule set
//! with concrete values. There is no token enum, no per-property
//! indirection — just a function from theme to style.
//!
//! Theme changes flow through the existing reactive system: each styled
//! node's apply-style call lives inside an `Effect` that reads the
//! theme signal, so swapping the theme re-fires every styled effect
//! and re-applies with the new theme's values. No re-render.
//!
//! # Identity for caching
//!
//! The framework memoizes resolution per `(stylesheet pointer, variants,
//! theme pointer)` and returns an `Rc<StyleRules>`. Backends cache
//! their native form keyed on the rule set's content (a hash or
//! serialization), making caching immune to allocator-reuse hazards.

use std::any::Any;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

// ----------------------------------------------------------------------------
// Values
// ----------------------------------------------------------------------------

/// Color value as a backend-portable string. Backends translate to their
/// native form (CSS string, UIColor, Android color int).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Color(pub String);

impl From<&str> for Color {
    fn from(s: &str) -> Self {
        Color(s.to_string())
    }
}

impl From<String> for Color {
    fn from(s: String) -> Self {
        Color(s)
    }
}

/// A measurable length value. Authors mostly write `Length::Px(16.0)`
/// — or just `16.0`/`16` directly, since `From<f32>` and `From<i32>`
/// produce `Length::Px`. Percent is for "X% of parent on the relevant
/// axis". Auto defers to layout (only meaningful on a subset of
/// properties — `width`, `height`, `margin`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Length {
    Px(f32),
    Percent(f32),
    Auto,
}

impl Length {
    /// Shorthand for `Length::Percent(value)`.
    pub fn pct(value: f32) -> Self { Length::Percent(value) }
}

impl From<f32> for Length {
    fn from(v: f32) -> Self { Length::Px(v) }
}

impl From<i32> for Length {
    fn from(v: i32) -> Self { Length::Px(v as f32) }
}

/// Bit-cast for hashing, since `f32` isn't `Eq`/`Hash`. Variant tag in
/// the high byte so `Px(0.0)` and `Percent(0.0)` hash differently.
fn length_bits(l: Length) -> u64 {
    match l {
        Length::Px(v) => (1u64 << 32) | v.to_bits() as u64,
        Length::Percent(v) => (2u64 << 32) | v.to_bits() as u64,
        Length::Auto => 3u64 << 32,
    }
}

// =============================================================================
// Flex layout enums (mobile-first defaults match React Native)
// =============================================================================

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FlexDirection {
    /// Children stack top-to-bottom. RN default; what `View {}` does
    /// without explicit configuration.
    #[default]
    Column,
    Row,
    ColumnReverse,
    RowReverse,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FlexWrap {
    #[default]
    NoWrap,
    Wrap,
    WrapReverse,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum JustifyContent {
    #[default]
    FlexStart,
    FlexEnd,
    Center,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum AlignItems {
    FlexStart,
    FlexEnd,
    Center,
    /// RN default. Children fill the cross axis.
    #[default]
    Stretch,
    Baseline,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum AlignContent {
    #[default]
    FlexStart,
    FlexEnd,
    Center,
    Stretch,
    SpaceBetween,
    SpaceAround,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum AlignSelf {
    #[default]
    Auto,
    FlexStart,
    FlexEnd,
    Center,
    Stretch,
    Baseline,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Position {
    #[default]
    Relative,
    Absolute,
}

// =============================================================================
// Typography enums
// =============================================================================

/// Font weight, ladder-style. Backends map to their native weight axis:
/// CSS numeric weights (100..900), iOS `UIFontWeight`, Android typeface
/// constants. RN-compatible enum; authors don't think in numeric scales.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FontWeight {
    Thin,
    ExtraLight,
    Light,
    #[default]
    Normal,
    Medium,
    SemiBold,
    Bold,
    ExtraBold,
    Black,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FontStyle {
    #[default]
    Normal,
    Italic,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TextAlign {
    #[default]
    Left,
    Right,
    Center,
    Justify,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TextTransform {
    #[default]
    None,
    Uppercase,
    Lowercase,
    Capitalize,
}

// =============================================================================
// Visual: Overflow / Shadow / Transform
// =============================================================================

/// Overflow handling at the node's edges. `Scroll` is intentionally not
/// supported as a style property — scrolling needs a `ScrollView`
/// primitive (separate concern). Authors who want overflow:hidden for
/// clipping (e.g. rounded-corner clipping of children) get the option.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Overflow {
    #[default]
    Visible,
    Hidden,
}

/// Drop shadow. Mobile-shaped — no CSS `spread` (which doesn't map
/// cleanly to UIView/Android shadow APIs). Backends translate:
/// - Web: `box-shadow: {x}px {y}px {blur}px {color}`
/// - iOS: `layer.shadowOffset/Opacity/Radius/Color` setters
/// - Android: `setElevation` + tinting (approximation)
#[derive(Clone, Debug, PartialEq)]
pub struct Shadow {
    pub x: f32,
    pub y: f32,
    pub blur: f32,
    pub color: Color,
}

/// One element of a transform stack. The full transform is a
/// `Vec<Transform>` applied in order — matches RN's `transform: [...]`
/// shape. Backends:
/// - Web: emits a single `transform: ...` string joining all entries.
/// - Native: applies each transform to the view's layer matrix in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Transform {
    TranslateX(Length),
    TranslateY(Length),
    /// Uniform scale on both axes.
    Scale(f32),
    /// Independent scale per axis.
    ScaleXY { x: f32, y: f32 },
    /// Rotation in degrees, clockwise.
    Rotate(f32),
    SkewX(f32),
    SkewY(f32),
}

// =============================================================================
// Animated transitions
// =============================================================================
//
// A `Transition` declares "when this property's resolved value changes,
// interpolate over `duration_ms` using `easing`." It does NOT drive
// per-frame ticking — the backend's native transition machinery does
// that (CSS `transition` on web, `CATransaction` / `UIView.animate` on
// iOS, `ObjectAnimator` on Android). The framework just declares
// intent; backends interpolate.
//
// Each animatable property in `StyleRules` has a sibling
// `*_transition: Option<Transition>` field. The macro's per-property
// transition shorthands (`padding: 200ms EaseOut`) fan out to all
// four sides, matching the property shorthand fanout.

/// Easing curve for an animated transition. Five named curves plus a
/// cubic-bezier escape hatch — covers the cross-platform set.
/// Backends map to their native primitive:
/// - Web: CSS timing-function names + `cubic-bezier(...)`
/// - iOS: `CAMediaTimingFunction` named constants + custom control points
/// - Android: `Interpolator` subclasses + `PathInterpolator` for custom
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Easing {
    Linear,
    /// CSS default — quick start, slow end. Equivalent to
    /// `cubic-bezier(0.25, 0.1, 0.25, 1.0)`.
    #[default]
    Ease,
    EaseIn,
    EaseOut,
    EaseInOut,
    /// Custom cubic-bezier control points `(x1, y1, x2, y2)`.
    CubicBezier(f32, f32, f32, f32),
}

/// Animation timing for a single property. `duration_ms` is integer
/// milliseconds (no floats — keeps `Hash`/`Eq` straightforward, and
/// sub-millisecond timing isn't meaningful for UI transitions).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transition {
    pub duration_ms: u32,
    pub easing: Easing,
}

impl Transition {
    pub fn new(duration_ms: u32, easing: Easing) -> Self {
        Self { duration_ms, easing }
    }
}

// ----------------------------------------------------------------------------
// StyleRules — concrete property bag
// ----------------------------------------------------------------------------

/// A bag of style property values. Every field is optional so a rule set
/// only carries properties the author cared about. Values are concrete —
/// no tokens, no indirection. Stylesheets produce these by running their
/// theme-fed closure.
///
/// Property scope is **flex layout only**: this struct intentionally has
/// no display/grid/float/etc. properties. Every node lays out its
/// children via flexbox; the framework relies on Yoga (or the web
/// browser) to do the actual math. RN defaults apply: `flex_direction`
/// = `Column`, `align_items` = `Stretch`, `flex_shrink` = 0.
///
/// Per-side properties (padding/margin/border-radius/border-width) are
/// stored as four separate fields per axis. Author-facing shorthand
/// like `padding: 16` is expanded by the `stylesheet!` macro at
/// compile time and by builder methods at runtime — the data model
/// itself has only per-side state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StyleRules {
    // --- Color + text ---
    pub background: Option<Color>,
    pub color: Option<Color>,
    pub font_size: Option<Length>,

    // --- Flex container (applies when this node has children) ---
    pub flex_direction: Option<FlexDirection>,
    pub flex_wrap: Option<FlexWrap>,
    pub justify_content: Option<JustifyContent>,
    pub align_items: Option<AlignItems>,
    pub align_content: Option<AlignContent>,
    pub gap: Option<Length>,
    pub row_gap: Option<Length>,
    pub column_gap: Option<Length>,

    // --- Flex item (this node's behavior inside its parent) ---
    pub flex_grow: Option<f32>,
    pub flex_shrink: Option<f32>,
    pub flex_basis: Option<Length>,
    pub align_self: Option<AlignSelf>,

    // --- Sizing ---
    pub width: Option<Length>,
    pub height: Option<Length>,
    pub min_width: Option<Length>,
    pub min_height: Option<Length>,
    pub max_width: Option<Length>,
    pub max_height: Option<Length>,

    // --- Padding (per-side; no shorthand field) ---
    pub padding_top: Option<Length>,
    pub padding_right: Option<Length>,
    pub padding_bottom: Option<Length>,
    pub padding_left: Option<Length>,

    // --- Margin (per-side; no shorthand field) ---
    pub margin_top: Option<Length>,
    pub margin_right: Option<Length>,
    pub margin_bottom: Option<Length>,
    pub margin_left: Option<Length>,

    // --- Border radius (per-corner) ---
    pub border_top_left_radius: Option<Length>,
    pub border_top_right_radius: Option<Length>,
    pub border_bottom_left_radius: Option<Length>,
    pub border_bottom_right_radius: Option<Length>,

    // --- Border widths (per-side, `f32` not `Length` — borders aren't
    //     percentages). All four are independent. ---
    pub border_top_width: Option<f32>,
    pub border_right_width: Option<f32>,
    pub border_bottom_width: Option<f32>,
    pub border_left_width: Option<f32>,

    // --- Border colors (per-side). ---
    pub border_top_color: Option<Color>,
    pub border_right_color: Option<Color>,
    pub border_bottom_color: Option<Color>,
    pub border_left_color: Option<Color>,

    // --- Position ---
    pub position: Option<Position>,
    pub top: Option<Length>,
    pub right: Option<Length>,
    pub bottom: Option<Length>,
    pub left: Option<Length>,

    // --- Typography (text-only on native; cascade on web) ---
    pub font_family: Option<String>,
    pub font_weight: Option<FontWeight>,
    pub font_style: Option<FontStyle>,
    pub line_height: Option<f32>,
    pub letter_spacing: Option<f32>,
    pub text_align: Option<TextAlign>,
    pub underline: Option<bool>,
    pub strikethrough: Option<bool>,
    pub text_transform: Option<TextTransform>,

    // --- Visual ---
    pub opacity: Option<f32>,
    pub overflow: Option<Overflow>,
    pub shadow: Option<Shadow>,
    /// Empty vec means "no transforms"; the field's `Option` distinguishes
    /// "not set, fall through to other layers" from "explicitly empty".
    pub transform: Option<Vec<Transform>>,

    // --- Transitions ---
    // One per animatable property. Set via `transitions { ... }` in
    // the `stylesheet!` macro. When the property's resolved value
    // changes, the backend interpolates over `duration_ms` using
    // `easing`. Properties without a transition spec change instantly.
    pub background_transition: Option<Transition>,
    pub color_transition: Option<Transition>,
    pub opacity_transition: Option<Transition>,
    pub transform_transition: Option<Transition>,
    pub width_transition: Option<Transition>,
    pub height_transition: Option<Transition>,
    pub top_transition: Option<Transition>,
    pub right_transition: Option<Transition>,
    pub bottom_transition: Option<Transition>,
    pub left_transition: Option<Transition>,
    pub padding_top_transition: Option<Transition>,
    pub padding_right_transition: Option<Transition>,
    pub padding_bottom_transition: Option<Transition>,
    pub padding_left_transition: Option<Transition>,
    pub margin_top_transition: Option<Transition>,
    pub margin_right_transition: Option<Transition>,
    pub margin_bottom_transition: Option<Transition>,
    pub margin_left_transition: Option<Transition>,
    pub border_top_left_radius_transition: Option<Transition>,
    pub border_top_right_radius_transition: Option<Transition>,
    pub border_bottom_left_radius_transition: Option<Transition>,
    pub border_bottom_right_radius_transition: Option<Transition>,
    pub border_top_width_transition: Option<Transition>,
    pub border_right_width_transition: Option<Transition>,
    pub border_bottom_width_transition: Option<Transition>,
    pub border_left_width_transition: Option<Transition>,
    pub border_top_color_transition: Option<Transition>,
    pub border_right_color_transition: Option<Transition>,
    pub border_bottom_color_transition: Option<Transition>,
    pub border_left_color_transition: Option<Transition>,
}

impl StyleRules {
    /// Layer `other` on top of `self`: properties set in `other` override
    /// the corresponding fields in `self`.
    pub fn merge(mut self, other: &StyleRules) -> Self {
        macro_rules! overlay {
            ($($f:ident),* $(,)?) => {
                $(
                    if other.$f.is_some() {
                        self.$f = other.$f.clone();
                    }
                )*
            };
        }
        overlay!(
            background, color, font_size,
            flex_direction, flex_wrap, justify_content, align_items, align_content,
            gap, row_gap, column_gap,
            flex_grow, flex_shrink, flex_basis, align_self,
            width, height, min_width, min_height, max_width, max_height,
            padding_top, padding_right, padding_bottom, padding_left,
            margin_top, margin_right, margin_bottom, margin_left,
            border_top_left_radius, border_top_right_radius,
            border_bottom_left_radius, border_bottom_right_radius,
            border_top_width, border_right_width, border_bottom_width, border_left_width,
            border_top_color, border_right_color, border_bottom_color, border_left_color,
            position, top, right, bottom, left,
            font_family, font_weight, font_style, line_height, letter_spacing,
            text_align, underline, strikethrough, text_transform,
            opacity, overflow, shadow, transform,
            background_transition, color_transition, opacity_transition,
            transform_transition, width_transition, height_transition,
            top_transition, right_transition, bottom_transition, left_transition,
            padding_top_transition, padding_right_transition,
            padding_bottom_transition, padding_left_transition,
            margin_top_transition, margin_right_transition,
            margin_bottom_transition, margin_left_transition,
            border_top_left_radius_transition, border_top_right_radius_transition,
            border_bottom_left_radius_transition, border_bottom_right_radius_transition,
            border_top_width_transition, border_right_width_transition,
            border_bottom_width_transition, border_left_width_transition,
            border_top_color_transition, border_right_color_transition,
            border_bottom_color_transition, border_left_color_transition,
        );
        self
    }

    /// Stable content key suitable for backend caches that should be
    /// immune to allocator-reuse hazards. Each property writes a tagged
    /// segment so distinct values always produce distinct keys.
    pub fn content_key(&self) -> String {
        let mut s = String::with_capacity(256);
        write_color(&mut s, "bg", &self.background);
        write_color(&mut s, "fg", &self.color);
        write_length(&mut s, "fs", self.font_size);

        write_enum(&mut s, "fd", self.flex_direction.map(|x| x as u8));
        write_enum(&mut s, "fw", self.flex_wrap.map(|x| x as u8));
        write_enum(&mut s, "jc", self.justify_content.map(|x| x as u8));
        write_enum(&mut s, "ai", self.align_items.map(|x| x as u8));
        write_enum(&mut s, "ac", self.align_content.map(|x| x as u8));
        write_length(&mut s, "gap", self.gap);
        write_length(&mut s, "rgap", self.row_gap);
        write_length(&mut s, "cgap", self.column_gap);

        write_f32(&mut s, "fg-grow", self.flex_grow);
        write_f32(&mut s, "fs-shrink", self.flex_shrink);
        write_length(&mut s, "fb", self.flex_basis);
        write_enum(&mut s, "as", self.align_self.map(|x| x as u8));

        write_length(&mut s, "w", self.width);
        write_length(&mut s, "h", self.height);
        write_length(&mut s, "minw", self.min_width);
        write_length(&mut s, "minh", self.min_height);
        write_length(&mut s, "maxw", self.max_width);
        write_length(&mut s, "maxh", self.max_height);

        write_length(&mut s, "pt", self.padding_top);
        write_length(&mut s, "pr", self.padding_right);
        write_length(&mut s, "pb", self.padding_bottom);
        write_length(&mut s, "pl", self.padding_left);
        write_length(&mut s, "mt", self.margin_top);
        write_length(&mut s, "mr", self.margin_right);
        write_length(&mut s, "mb", self.margin_bottom);
        write_length(&mut s, "ml", self.margin_left);

        write_length(&mut s, "rtl", self.border_top_left_radius);
        write_length(&mut s, "rtr", self.border_top_right_radius);
        write_length(&mut s, "rbl", self.border_bottom_left_radius);
        write_length(&mut s, "rbr", self.border_bottom_right_radius);

        write_f32(&mut s, "bwt", self.border_top_width);
        write_f32(&mut s, "bwr", self.border_right_width);
        write_f32(&mut s, "bwb", self.border_bottom_width);
        write_f32(&mut s, "bwl", self.border_left_width);
        write_color(&mut s, "bct", &self.border_top_color);
        write_color(&mut s, "bcr", &self.border_right_color);
        write_color(&mut s, "bcb", &self.border_bottom_color);
        write_color(&mut s, "bcl", &self.border_left_color);

        write_enum(&mut s, "pos", self.position.map(|x| x as u8));
        write_length(&mut s, "top", self.top);
        write_length(&mut s, "right", self.right);
        write_length(&mut s, "bot", self.bottom);
        write_length(&mut s, "left", self.left);

        // Typography
        write_str(&mut s, "ff", self.font_family.as_deref());
        write_enum(&mut s, "fw", self.font_weight.map(|x| x as u8));
        write_enum(&mut s, "fst", self.font_style.map(|x| x as u8));
        write_f32(&mut s, "lh", self.line_height);
        write_f32(&mut s, "ls", self.letter_spacing);
        write_enum(&mut s, "ta", self.text_align.map(|x| x as u8));
        write_enum(&mut s, "ul", self.underline.map(|b| b as u8));
        write_enum(&mut s, "st", self.strikethrough.map(|b| b as u8));
        write_enum(&mut s, "tt", self.text_transform.map(|x| x as u8));

        // Visual
        write_f32(&mut s, "op", self.opacity);
        write_enum(&mut s, "ov", self.overflow.map(|x| x as u8));
        if let Some(sh) = &self.shadow {
            s.push_str("sh=");
            push_u32_hex(&mut s, sh.x.to_bits());
            push_u32_hex(&mut s, sh.y.to_bits());
            push_u32_hex(&mut s, sh.blur.to_bits());
            s.push_str(&sh.color.0);
            s.push(';');
        } else {
            s.push_str("sh=;");
        }
        if let Some(xs) = &self.transform {
            s.push_str("tr=");
            for t in xs {
                match t {
                    Transform::TranslateX(l) => { s.push_str("tx"); push_u64_hex(&mut s, length_bits(*l)); }
                    Transform::TranslateY(l) => { s.push_str("ty"); push_u64_hex(&mut s, length_bits(*l)); }
                    Transform::Scale(v) => { s.push_str("sc"); push_u32_hex(&mut s, v.to_bits()); }
                    Transform::ScaleXY { x, y } => { s.push_str("sxy"); push_u32_hex(&mut s, x.to_bits()); push_u32_hex(&mut s, y.to_bits()); }
                    Transform::Rotate(v) => { s.push_str("rt"); push_u32_hex(&mut s, v.to_bits()); }
                    Transform::SkewX(v) => { s.push_str("skx"); push_u32_hex(&mut s, v.to_bits()); }
                    Transform::SkewY(v) => { s.push_str("sky"); push_u32_hex(&mut s, v.to_bits()); }
                }
            }
            s.push(';');
        } else {
            s.push_str("tr=;");
        }

        // Transitions — one labeled segment per animatable property.
        // Inactive (None) transitions write an empty value so the
        // cache key remains stable in shape regardless of which
        // transitions are set.
        macro_rules! tr {
            ($label:literal, $field:ident) => {
                write_transition(&mut s, $label, self.$field);
            };
        }
        tr!("tbg", background_transition);
        tr!("tco", color_transition);
        tr!("top_t", opacity_transition);
        tr!("ttr", transform_transition);
        tr!("tw", width_transition);
        tr!("th", height_transition);
        tr!("ttt", top_transition);
        tr!("trt", right_transition);
        tr!("tbt", bottom_transition);
        tr!("tlt", left_transition);
        tr!("tpt", padding_top_transition);
        tr!("tpr", padding_right_transition);
        tr!("tpb", padding_bottom_transition);
        tr!("tpl", padding_left_transition);
        tr!("tmt", margin_top_transition);
        tr!("tmr", margin_right_transition);
        tr!("tmb", margin_bottom_transition);
        tr!("tml", margin_left_transition);
        tr!("trtl", border_top_left_radius_transition);
        tr!("trtr", border_top_right_radius_transition);
        tr!("trbl", border_bottom_left_radius_transition);
        tr!("trbr", border_bottom_right_radius_transition);
        tr!("tbwt", border_top_width_transition);
        tr!("tbwr", border_right_width_transition);
        tr!("tbwb", border_bottom_width_transition);
        tr!("tbwl", border_left_width_transition);
        tr!("tbct", border_top_color_transition);
        tr!("tbcr", border_right_color_transition);
        tr!("tbcb", border_bottom_color_transition);
        tr!("tbcl", border_left_color_transition);

        s
    }
}

fn write_transition(out: &mut String, label: &str, t: Option<Transition>) {
    out.push_str(label);
    out.push('=');
    if let Some(t) = t {
        push_u32_hex(out, t.duration_ms);
        // Easing encodes as a small tag; CubicBezier appends four f32s.
        match t.easing {
            Easing::Linear => out.push_str("lin"),
            Easing::Ease => out.push_str("eas"),
            Easing::EaseIn => out.push_str("ein"),
            Easing::EaseOut => out.push_str("eou"),
            Easing::EaseInOut => out.push_str("eio"),
            Easing::CubicBezier(a, b, c, d) => {
                out.push_str("cb");
                push_u32_hex(out, a.to_bits());
                push_u32_hex(out, b.to_bits());
                push_u32_hex(out, c.to_bits());
                push_u32_hex(out, d.to_bits());
            }
        }
    }
    out.push(';');
}

fn write_str(out: &mut String, label: &str, v: Option<&str>) {
    out.push_str(label);
    out.push('=');
    if let Some(v) = v { out.push_str(v); }
    out.push(';');
}

fn write_color(out: &mut String, label: &str, c: &Option<Color>) {
    out.push_str(label);
    out.push('=');
    if let Some(c) = c { out.push_str(&c.0); }
    out.push(';');
}

fn write_length(out: &mut String, label: &str, l: Option<Length>) {
    out.push_str(label);
    out.push('=');
    if let Some(l) = l {
        push_u64_hex(out, length_bits(l));
    }
    out.push(';');
}

fn write_f32(out: &mut String, label: &str, v: Option<f32>) {
    out.push_str(label);
    out.push('=');
    if let Some(v) = v {
        push_u32_hex(out, v.to_bits());
    }
    out.push(';');
}

fn write_enum(out: &mut String, label: &str, v: Option<u8>) {
    out.push_str(label);
    out.push('=');
    if let Some(v) = v {
        push_u32_hex(out, v as u32);
    }
    out.push(';');
}

fn push_u64_hex(out: &mut String, n: u64) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for shift in (0..16).rev() {
        let nibble = ((n >> (shift * 4)) & 0xf) as usize;
        out.push(HEX[nibble] as char);
    }
}

/// Writes the 8-char lowercase hex representation of `n` to `out`.
/// Used by `content_key` to encode `f32::to_bits()` results without
/// the `format!` machinery.
fn push_u32_hex(out: &mut String, n: u32) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for shift in (0..8).rev() {
        let nibble = ((n >> (shift * 4)) & 0xf) as usize;
        out.push(HEX[nibble] as char);
    }
}

// ----------------------------------------------------------------------------
// StyleSheet — closures from theme to rules, with variants and compounds
// ----------------------------------------------------------------------------

type RulesFn = Box<dyn Fn(&dyn Any) -> StyleRules>;

pub type VariantAxis = String;
pub type VariantValue = String;

/// One axis of variants on a stylesheet — its declared values and the
/// optional default value used when the call site doesn't pick a value.
pub struct VariantAxisDef {
    /// The value treated as active when the call site omits this axis.
    pub default: Option<VariantValue>,
    /// Per-value overlay closures. Each runs against the theme.
    pub values: BTreeMap<VariantValue, RulesFn>,
}

/// A compound variant: only applied when *all* of `when`'s
/// axis=value pairs are active at apply time.
pub struct CompoundVariant {
    pub when: BTreeMap<VariantAxis, VariantValue>,
    pub rules: RulesFn,
}

/// A stylesheet declaration. Authors construct one of these once and
/// wrap it in `Rc` to pass around.
///
/// Each entry — `base`, every variant overlay, every compound variant —
/// is a closure that takes the active theme (typed as the app's theme)
/// and returns concrete `StyleRules`. There are no tokens; closures
/// are the only mechanism for theme-aware values.
///
/// # Resolution order
/// 1. `base`
/// 2. For each declared axis, layer the closure for the value selected
///    in the `VariantSet` (or the axis's default if unselected).
/// 3. For each declared compound variant, layer its closure iff every
///    `(axis, value)` in `when` matches the *effective* variant set
///    (defaults included).
/// 4. Any `StyleApplication::overrides` field.
pub struct StyleSheet {
    base: RulesFn,
    /// axis → axis definition (default + per-value closures)
    variants: BTreeMap<VariantAxis, VariantAxisDef>,
    /// Compound variants are stored as a list (order-preserving).
    compounds: Vec<CompoundVariant>,
    /// Cached list of state-overlay axes the sheet declares. Populated
    /// in `.variant(...)` whenever an axis named `__state_*` is added.
    /// Empty for the very common case of sheets with no `state` blocks
    /// — `resolve_state_overlays` short-circuits on `is_empty()` and
    /// avoids walking the variants BTreeMap per styled node.
    ///
    /// On the perf screen (10k rows × no state blocks) this removed
    /// ~10μs of per-row variant-map iteration, which compounded to
    /// ~100ms across the whole render.
    state_axes: Vec<(crate::StateBits, VariantAxis)>,
    /// Per-sheet pointer-friendly variant cache. Keyed on
    /// `(theme_ptr, variants)`; value is the pre-resolved
    /// `Rc<StyleRules>` for the no-overrides case. Populated by
    /// [`ensure_registered_with`] at registration time; pruned on
    /// `unregister`.
    ///
    /// This is the hot lookup path in `resolve()` — when an
    /// application has no overrides (the common case for
    /// stylesheet-only styling), the resolve function consults
    /// this map first and bypasses the global `RESOLUTION_CACHE`
    /// entirely. Saves one thread_local RefCell borrow + one
    /// global HashMap lookup + one `Rc::as_ptr(&app.sheet)`
    /// indirection per styled node.
    ///
    /// `RefCell` for interior mutability — the sheet itself is
    /// shared via `Rc<StyleSheet>` so we can't take `&mut self` to
    /// populate the cache at registration time.
    variant_cache: std::cell::RefCell<
        HashMap<(*const (), VariantSet), Rc<StyleRules>>,
    >,
}

impl StyleSheet {
    /// Constructs a stylesheet whose base rules are produced by `f`.
    pub fn new<Theme, F>(f: F) -> Self
    where
        Theme: Any + 'static,
        F: Fn(&Theme) -> StyleRules + 'static,
    {
        Self {
            base: wrap_theme_fn::<Theme, F>(f),
            variants: BTreeMap::new(),
            compounds: Vec::new(),
            state_axes: Vec::new(),
            variant_cache: std::cell::RefCell::new(HashMap::new()),
        }
    }

    /// A stylesheet that doesn't read the theme.
    pub fn r#static(rules: StyleRules) -> Self {
        Self {
            base: Box::new(move |_any: &dyn Any| rules.clone()),
            variants: BTreeMap::new(),
            compounds: Vec::new(),
            state_axes: Vec::new(),
            variant_cache: std::cell::RefCell::new(HashMap::new()),
        }
    }

    /// Adds (or replaces) a variant overlay on the given axis-value.
    /// If the axis didn't exist yet it's created with no default.
    pub fn variant<Theme, F>(
        mut self,
        axis: impl Into<VariantAxis>,
        value: impl Into<VariantValue>,
        f: F,
    ) -> Self
    where
        Theme: Any + 'static,
        F: Fn(&Theme) -> StyleRules + 'static,
    {
        let axis = axis.into();
        let value = value.into();
        // Cache state-axis presence at construction so
        // `resolve_state_overlays` can short-circuit per styled node
        // instead of walking the variants map. Only add once per
        // axis even if the user declares multiple values for the
        // same state (unusual — states only have "on" — but defensive).
        if let Some(bit) = state_axis_bit(&axis) {
            if !self.state_axes.iter().any(|(_, a)| a == &axis) {
                self.state_axes.push((bit, axis.clone()));
            }
        }
        let entry = self.variants.entry(axis).or_insert_with(|| VariantAxisDef {
            default: None,
            values: BTreeMap::new(),
        });
        entry.values.insert(value, wrap_theme_fn::<Theme, F>(f));
        self
    }

    /// The cached set of state-overlay axes declared on this
    /// stylesheet. Returns an empty slice for the common case of
    /// sheets with no `state` blocks. Used by
    /// `resolve_state_overlays` to skip per-call iteration of the
    /// full variants map.
    pub(crate) fn state_axes(&self) -> &[(crate::StateBits, VariantAxis)] {
        &self.state_axes
    }

    /// Per-sheet variant-cache lookup. Returns the pre-resolved
    /// `Rc<StyleRules>` if `(theme_ptr, variants)` has been
    /// registered, `None` otherwise. The hot path in [`resolve`]
    /// hits this before the global resolution cache.
    pub(crate) fn lookup_variant(
        &self,
        theme_ptr: *const (),
        variants: &VariantSet,
    ) -> Option<Rc<StyleRules>> {
        // The hash key tuple takes `(theme_ptr, variants)` by value.
        // Cloning the `VariantSet` here would defeat the purpose —
        // for the no-overrides path we want to avoid that clone.
        //
        // HashMap's `get` takes a `&K` (so it'd require a key by
        // value to hash against), which means we'd clone. Trick:
        // use a borrowed-key adapter via `raw_entry` on nightly,
        // OR change the cache to use a `BTreeMap<&VariantSet>`-
        // style structure. Simpler: hash the variants once and use
        // the HashMap's `Q: Borrow<...>` lookup via `&(theme_ptr,
        // variants)` only after wrapping the key in a referenced
        // form.
        //
        // For this v1 we accept the small `variants.clone()` cost
        // on the hot path — the previous code path was already
        // cloning the `VariantSet` *and* doing a global RefCell
        // borrow + global HashMap lookup. We just keep the clone
        // and drop the rest.
        let key = (theme_ptr, variants.clone());
        self.variant_cache.borrow().get(&key).cloned()
    }

    /// Insert a pre-resolved rule into the variant cache. Called
    /// from [`ensure_registered_with`] for each pregen entry.
    pub(crate) fn insert_variant(
        &self,
        theme_ptr: *const (),
        variants: VariantSet,
        rc: Rc<StyleRules>,
    ) {
        self.variant_cache
            .borrow_mut()
            .insert((theme_ptr, variants), rc);
    }

    /// Drop every entry for `theme_ptr`. Called from
    /// [`ensure_registered_with`] on the dead-registration sweep
    /// (theme change, sheet drop).
    pub(crate) fn forget_theme(&self, theme_ptr: *const ()) {
        self.variant_cache
            .borrow_mut()
            .retain(|(t, _), _| *t != theme_ptr);
    }

    /// Sets the default value for an axis. When a call site omits this
    /// axis from the `VariantSet`, the default value's overlay is
    /// applied. The default value must also be added via `.variant(...)`
    /// (or it will silently apply nothing — same as today).
    pub fn variant_default(
        mut self,
        axis: impl Into<VariantAxis>,
        value: impl Into<VariantValue>,
    ) -> Self {
        let axis = axis.into();
        let value = value.into();
        let entry = self.variants.entry(axis).or_insert_with(|| VariantAxisDef {
            default: None,
            values: BTreeMap::new(),
        });
        entry.default = Some(value);
        self
    }

    /// Adds a compound variant: an overlay applied only when every
    /// `(axis, value)` pair in `when` is active at apply time.
    pub fn compound<Theme, F>(
        mut self,
        when: Vec<(impl Into<VariantAxis>, impl Into<VariantValue>)>,
        f: F,
    ) -> Self
    where
        Theme: Any + 'static,
        F: Fn(&Theme) -> StyleRules + 'static,
    {
        let when: BTreeMap<VariantAxis, VariantValue> =
            when.into_iter().map(|(a, v)| (a.into(), v.into())).collect();
        self.compounds.push(CompoundVariant {
            when,
            rules: wrap_theme_fn::<Theme, F>(f),
        });
        self
    }

    /// Returns the effective `VariantSet` for resolution — the call site's
    /// `VariantSet` overlaid with each axis's declared default (if any)
    /// for axes the call site didn't specify.
    fn effective_variants(&self, requested: &VariantSet) -> VariantSet {
        let mut out = requested.clone();
        for (axis, def) in &self.variants {
            if !out.0.contains_key(axis) {
                if let Some(default) = &def.default {
                    out.0.insert(axis.clone(), default.clone());
                }
            }
        }
        out
    }

    /// Resolves the stylesheet against the given variants and theme.
    pub fn resolve(&self, variants: &VariantSet, theme: &dyn Any) -> StyleRules {
        let effective_variants = self.effective_variants(variants);
        let mut effective = (self.base)(theme);

        // Per-axis variants.
        for (axis, def) in &self.variants {
            if let Some(value) = effective_variants.0.get(axis) {
                if let Some(f) = def.values.get(value) {
                    effective = effective.merge(&f(theme));
                }
            }
        }

        // Compound variants — apply when every (axis, value) matches.
        for c in &self.compounds {
            let matches = c
                .when
                .iter()
                .all(|(axis, val)| effective_variants.0.get(axis) == Some(val));
            if matches {
                effective = effective.merge(&(c.rules)(theme));
            }
        }

        effective
    }

    // -----------------------------------------------------------------
    // Introspection for pre-generation
    // -----------------------------------------------------------------

    /// Returns every (axis, value) pair declared on this stylesheet.
    /// The pre-generator can walk these to mint a class per single-axis
    /// selection.
    pub fn variant_keys(&self) -> Vec<(VariantAxis, VariantValue)> {
        let mut out = Vec::new();
        for (axis, def) in &self.variants {
            for value in def.values.keys() {
                out.push((axis.clone(), value.clone()));
            }
        }
        out
    }

    /// Returns the declared compound variants' match conditions.
    pub fn compound_keys(&self) -> Vec<BTreeMap<VariantAxis, VariantValue>> {
        self.compounds.iter().map(|c| c.when.clone()).collect()
    }

    /// Returns the default value declared for an axis, if any.
    pub fn axis_default(&self, axis: &str) -> Option<&VariantValue> {
        self.variants.get(axis).and_then(|d| d.default.as_ref())
    }
}

/// Wraps an `Fn(&Theme) -> StyleRules` in the `Fn(&dyn Any) -> StyleRules`
/// shape we store inside `StyleSheet`. The downcast happens once per
/// call at the closure boundary — not per property.
/// Map a variant axis name to its `StateBits` flag, or `None` if
/// the axis isn't a state overlay. The stylesheet macro emits state
/// axes namespaced as `__state_<name>` so they don't collide with
/// regular author variants.
fn state_axis_bit(axis: &str) -> Option<crate::StateBits> {
    match axis {
        "__state_hovered" => Some(crate::StateBits::HOVERED),
        "__state_pressed" => Some(crate::StateBits::PRESSED),
        "__state_focused" => Some(crate::StateBits::FOCUSED),
        "__state_disabled" => Some(crate::StateBits::DISABLED),
        _ => None,
    }
}

fn wrap_theme_fn<Theme: Any + 'static, F: Fn(&Theme) -> StyleRules + 'static>(f: F) -> RulesFn {
    Box::new(move |any: &dyn Any| {
        let theme = any
            .downcast_ref::<Theme>()
            .expect("theme type mismatch — stylesheet expected a different theme type");
        f(theme)
    })
}

// ----------------------------------------------------------------------------
// VariantSet & StyleApplication
// ----------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct VariantSet(pub BTreeMap<VariantAxis, VariantValue>);

impl VariantSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(
        mut self,
        axis: impl Into<VariantAxis>,
        value: impl Into<VariantValue>,
    ) -> Self {
        self.0.insert(axis.into(), value.into());
        self
    }
}

/// The value passed from author code to the framework. The framework
/// resolves it against the active theme into an `Rc<StyleRules>` before
/// handing off to the backend.
///
/// Resolution order (each layer overrides the previous for any
/// `Some(...)` property):
///
/// 1. **Base**: the stylesheet's `new(|theme| ...)` closure output.
/// 2. **Variants**: each active variant's overlay closure output.
/// 3. **Overrides**: per-call-site continuous values (this struct's
///    `overrides` field). Used for values that can't be enumerated as
///    discrete variants — e.g. a user-controlled font scale.
///
/// The backend sees the merged result; it doesn't know which layer
/// contributed what. Backend caches (web CSS classes, etc.) key on the
/// resolved content so each unique combination still gets its own
/// entry — overrides preserve cacheability without inline styles.
#[derive(Clone)]
pub struct StyleApplication {
    pub sheet: Rc<StyleSheet>,
    pub variants: VariantSet,
    pub overrides: StyleRules,
    /// `true` iff any `override_*` builder has been called on this
    /// application. Lets `resolve()` skip `overrides.content_key()`
    /// (a ~600-byte string format walking every field) when there
    /// are no overrides — the common case for stylesheet-only
    /// styling. On 10k styled rows this saved ~80ms.
    has_overrides: bool,
}

impl StyleApplication {
    pub fn new(sheet: Rc<StyleSheet>) -> Self {
        Self {
            sheet,
            variants: VariantSet::new(),
            overrides: StyleRules::default(),
            has_overrides: false,
        }
    }

    /// Lookup-friendly accessor for the overrides flag. Used by
    /// `resolve()` to pick between the empty-overrides key (just an
    /// empty string) and the full content-keyed path.
    pub fn has_overrides(&self) -> bool {
        self.has_overrides
    }

    pub fn with(
        mut self,
        axis: impl Into<VariantAxis>,
        value: impl Into<VariantValue>,
    ) -> Self {
        self.variants.0.insert(axis.into(), value.into());
        self
    }

    /// Override the background color with a per-call-site value.
    pub fn override_background(mut self, c: impl Into<Color>) -> Self {
        self.has_overrides = true;
        self.overrides.background = Some(c.into());
        self
    }

    /// Override the foreground color with a per-call-site value.
    pub fn override_color(mut self, c: impl Into<Color>) -> Self {
        self.has_overrides = true;
        self.overrides.color = Some(c.into());
        self
    }

    /// Override font size with a per-call-site value.
    pub fn override_font_size(mut self, v: impl Into<Length>) -> Self {
        self.has_overrides = true;
        self.overrides.font_size = Some(v.into());
        self
    }

    /// Shorthand override: set padding on all four sides. Equivalent to
    /// calling `override_padding_top`, `_right`, `_bottom`, `_left`
    /// with the same value.
    pub fn override_padding(mut self, v: impl Into<Length>) -> Self {
        self.has_overrides = true;
        let v = v.into();
        self.overrides.padding_top = Some(v);
        self.overrides.padding_right = Some(v);
        self.overrides.padding_bottom = Some(v);
        self.overrides.padding_left = Some(v);
        self
    }

    pub fn override_padding_horizontal(mut self, v: impl Into<Length>) -> Self {
        self.has_overrides = true;
        let v = v.into();
        self.overrides.padding_left = Some(v);
        self.overrides.padding_right = Some(v);
        self
    }

    pub fn override_padding_vertical(mut self, v: impl Into<Length>) -> Self {
        self.has_overrides = true;
        let v = v.into();
        self.overrides.padding_top = Some(v);
        self.overrides.padding_bottom = Some(v);
        self
    }

    pub fn override_padding_top(mut self, v: impl Into<Length>) -> Self {
        self.has_overrides = true;
        self.overrides.padding_top = Some(v.into()); self
    }
    pub fn override_padding_right(mut self, v: impl Into<Length>) -> Self {
        self.has_overrides = true;
        self.overrides.padding_right = Some(v.into()); self
    }
    pub fn override_padding_bottom(mut self, v: impl Into<Length>) -> Self {
        self.has_overrides = true;
        self.overrides.padding_bottom = Some(v.into()); self
    }
    pub fn override_padding_left(mut self, v: impl Into<Length>) -> Self {
        self.has_overrides = true;
        self.overrides.padding_left = Some(v.into()); self
    }

    /// Shorthand override: margin on all four sides.
    pub fn override_margin(mut self, v: impl Into<Length>) -> Self {
        self.has_overrides = true;
        let v = v.into();
        self.overrides.margin_top = Some(v);
        self.overrides.margin_right = Some(v);
        self.overrides.margin_bottom = Some(v);
        self.overrides.margin_left = Some(v);
        self
    }

    pub fn override_margin_horizontal(mut self, v: impl Into<Length>) -> Self {
        self.has_overrides = true;
        let v = v.into();
        self.overrides.margin_left = Some(v);
        self.overrides.margin_right = Some(v);
        self
    }

    pub fn override_margin_vertical(mut self, v: impl Into<Length>) -> Self {
        self.has_overrides = true;
        let v = v.into();
        self.overrides.margin_top = Some(v);
        self.overrides.margin_bottom = Some(v);
        self
    }

    pub fn override_margin_top(mut self, v: impl Into<Length>) -> Self {
        self.has_overrides = true;
        self.overrides.margin_top = Some(v.into()); self
    }
    pub fn override_margin_right(mut self, v: impl Into<Length>) -> Self {
        self.has_overrides = true;
        self.overrides.margin_right = Some(v.into()); self
    }
    pub fn override_margin_bottom(mut self, v: impl Into<Length>) -> Self {
        self.has_overrides = true;
        self.overrides.margin_bottom = Some(v.into()); self
    }
    pub fn override_margin_left(mut self, v: impl Into<Length>) -> Self {
        self.has_overrides = true;
        self.overrides.margin_left = Some(v.into()); self
    }

    /// Shorthand override: border-radius on all four corners.
    pub fn override_border_radius(mut self, v: impl Into<Length>) -> Self {
        self.has_overrides = true;
        let v = v.into();
        self.overrides.border_top_left_radius = Some(v);
        self.overrides.border_top_right_radius = Some(v);
        self.overrides.border_bottom_left_radius = Some(v);
        self.overrides.border_bottom_right_radius = Some(v);
        self
    }
}

// ----------------------------------------------------------------------------
// Global active theme & resolution cache
// ----------------------------------------------------------------------------

thread_local! {
    /// The active theme. Wrapped in a `Signal<Rc<dyn Any>>` so effects
    /// subscribe via the existing reactivity system and re-apply on swap.
    static ACTIVE_THEME: RefCell<Option<crate::Signal<Rc<dyn Any>>>> = const { RefCell::new(None) };

    /// Memoization: `(stylesheet pointer, variants, theme pointer,
    /// override content)` → `Weak<StyleRules>`. Strong refs are held by
    /// `REGISTRATIONS` for pre-generated styles, and transiently by the
    /// caller of `resolve(...)` for dynamic ones. When the last strong
    /// Memoizes resolved `StyleRules` by `ResolutionKey`. Entries
    /// are strong `Rc`s — the cache itself is what keeps resolved
    /// rules alive across back-to-back applies. Without strong
    /// refs, the resolved rules would drop the moment the caller's
    /// transient `Rc` went out of scope (right after each
    /// `apply_styled_states` call), and the next row's resolve
    /// would re-compute from scratch — exactly the leak that
    /// caused 10k-row perf-screen renders to miss the cache 10k
    /// times.
    ///
    /// Memory cost: one `StyleRules` per distinct
    /// `(sheet, variants, theme, overrides)` combination. For most
    /// apps this is bounded by the number of variant combinations
    /// the author actually uses — a few hundred entries at most.
    ///
    /// Cleared on theme change (entries reference the old theme
    /// pointer and would never be reused). Per-stylesheet sweep
    /// runs on `ensure_registered_with` when it detects dead
    /// `Weak<StyleSheet>` registrations.
    static RESOLUTION_CACHE: RefCell<HashMap<ResolutionKey, Rc<StyleRules>>> =
        RefCell::new(HashMap::new());

    /// Each currently-registered `(stylesheet, theme)` pair, with the
    /// rules that were pre-generated for it and a `Weak<StyleSheet>`
    /// used to detect when the stylesheet has been dropped by all
    /// holders. The framework calls `Backend::register_stylesheet`
    /// exactly once per pair and tracks the rules so we can later call
    /// `unregister_stylesheet` to free backend-side state.
    static REGISTRATIONS: RefCell<HashMap<RegKey, Registration>> =
        RefCell::new(HashMap::new());

    /// Rule sets queued for `unregister_stylesheet` calls. Populated by
    /// `set_theme` (moves all current registrations here) and by the
    /// sweep-dead-stylesheets pass (moves dead entries here). Drained
    /// by `ensure_registered_with`, which has the backend in scope.
    static PENDING_UNREGISTER: RefCell<Vec<Vec<Rc<StyleRules>>>> =
        RefCell::new(Vec::new());
}

#[derive(PartialEq, Eq, Hash, Clone)]
struct RegKey {
    sheet: *const StyleSheet,
    theme: *const (),
}

struct Registration {
    weak: std::rc::Weak<StyleSheet>,
    rules: Vec<Rc<StyleRules>>,
}

#[derive(PartialEq, Eq, Hash)]
struct ResolutionKey {
    sheet: *const StyleSheet,
    variants: VariantSet,
    theme: *const (),
    /// Overrides are part of the cache key — same sheet + variants +
    /// theme but different override values yield different rules and
    /// must be cached separately. Serialized to a content key so we
    /// have a comparable form.
    overrides: String,
}

/// Install the initial active theme. Call once at app startup before
/// rendering.
pub fn install_theme<Theme: Any + 'static>(theme: Theme) {
    let rc: Rc<dyn Any> = Rc::new(theme);
    let sig = crate::Signal::new(rc);
    ACTIVE_THEME.with(|t| *t.borrow_mut() = Some(sig));
}

/// Swap the active theme. Every styled component subscribed via the
/// reactive renderer re-fires its apply-style effect and re-applies
/// with the new theme's values.
///
/// All currently-registered `(stylesheet, theme)` pairs are queued for
/// `unregister_stylesheet`; the backend hears about them on the next
/// `ensure_registered_with` call (which has it in scope).
pub fn set_theme<Theme: Any + 'static>(theme: Theme) {
    let rc: Rc<dyn Any> = Rc::new(theme);
    RESOLUTION_CACHE.with(|c| c.borrow_mut().clear());

    // Move every current registration into the pending-unregister queue.
    // The next styled effect that fires will flush it with the backend
    // in scope. Also opportunistically prune each registered sheet's
    // per-sheet variant cache for the *old* theme — those entries
    // would never be looked up again (new theme has a different
    // `theme_ptr`) but would linger on the `Rc<StyleSheet>` until the
    // sheet itself drops.
    let old_theme_ptr = ACTIVE_THEME
        .with(|t| t.borrow().as_ref().map(|sig| Rc::as_ptr(&sig.get()) as *const ()))
        .unwrap_or(std::ptr::null());
    REGISTRATIONS.with(|r| {
        let mut regs = r.borrow_mut();
        PENDING_UNREGISTER.with(|p| {
            let mut pending = p.borrow_mut();
            for (_, reg) in regs.drain() {
                // Try to upgrade the Weak to a live sheet and prune
                // its variant_cache for the old theme. If the
                // upgrade fails, the sheet is already dead and its
                // cache will drop along with it.
                if let Some(sheet) = reg.weak.upgrade() {
                    sheet.forget_theme(old_theme_ptr);
                }
                pending.push(reg.rules);
            }
        });
    });

    ACTIVE_THEME.with(|t| {
        if let Some(sig) = t.borrow().as_ref() {
            sig.set(rc);
        } else {
            let new_sig = crate::Signal::new(rc);
            *t.borrow_mut() = Some(new_sig);
        }
    });
}

/// Ensures the backend has been asked to pre-generate state for this
/// stylesheet against the active theme. Calls `register` with the
/// resolved rules exactly once per `(sheet, theme)` pair.
///
/// Also opportunistically:
/// - Flushes the pending-unregister queue, calling `unregister` for
///   each rule set queued by `set_theme` or a dead-stylesheet sweep.
/// - Sweeps registrations whose `Weak<StyleSheet>` no longer upgrades
///   into the pending-unregister queue.
pub fn ensure_registered_with<R, U>(sheet: &Rc<StyleSheet>, register: R, unregister: U)
where
    R: FnOnce(&[Rc<StyleRules>]),
    U: Fn(&[Rc<StyleRules>]),
{
    let theme = active_theme();
    let sheet_ptr = Rc::as_ptr(sheet);
    let theme_ptr = Rc::as_ptr(&theme) as *const ();
    let key = RegKey { sheet: sheet_ptr, theme: theme_ptr };

    // Sweep dead registrations (Weak no longer upgrades). They go to
    // the pending-unregister queue, and any matching entries in the
    // resolution cache get pruned so we don't pin stale `StyleRules`
    // alive past their stylesheet's lifetime.
    let mut dead_sheet_ptrs: Vec<*const StyleSheet> = Vec::new();
    REGISTRATIONS.with(|r| {
        let mut regs = r.borrow_mut();
        let dead_keys: Vec<RegKey> = regs
            .iter()
            .filter_map(|(k, reg)| {
                if reg.weak.upgrade().is_none() {
                    Some(k.clone())
                } else {
                    None
                }
            })
            .collect();
        if !dead_keys.is_empty() {
            PENDING_UNREGISTER.with(|p| {
                let mut pending = p.borrow_mut();
                for k in dead_keys {
                    dead_sheet_ptrs.push(k.sheet);
                    if let Some(reg) = regs.remove(&k) {
                        pending.push(reg.rules);
                    }
                }
            });
        }
    });
    if !dead_sheet_ptrs.is_empty() {
        RESOLUTION_CACHE.with(|c| {
            c.borrow_mut().retain(|k, _| !dead_sheet_ptrs.contains(&k.sheet));
        });
    }

    // Flush pending unregistrations now that the backend is in scope.
    let pending: Vec<Vec<Rc<StyleRules>>> =
        PENDING_UNREGISTER.with(|p| std::mem::take(&mut *p.borrow_mut()));
    for rules in &pending {
        unregister(rules);
    }

    // Already registered? Done.
    let already = REGISTRATIONS.with(|r| r.borrow().contains_key(&key));
    if already {
        return;
    }

    // Register fresh. We pre-populate the resolution cache with
    // the pregen Rcs so `resolve()` for a known (sheet, variants,
    // theme, no-overrides) combination returns the *same Rc
    // instance* the backend just registered. That lets the backend
    // short-circuit on `Rc::as_ptr` identity instead of paying for
    // `content_key()` on every node — the difference between
    // 60ms+ overhead per 10k rows and ~5ms.
    let keyed = pregenerate_keyed(sheet, &*theme);
    let sheet_ptr = Rc::as_ptr(sheet);
    // Use the empty string as the "no overrides" marker — matches
    // what `resolve()` uses when `StyleApplication::has_overrides`
    // is false. The full `StyleRules::default().content_key()` is
    // a ~600-byte hex dump of all-empty fields and would only
    // create cache-key mismatches.
    RESOLUTION_CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        for (variants, rc) in &keyed {
            let cache_key = ResolutionKey {
                sheet: sheet_ptr,
                variants: variants.clone(),
                theme: theme_ptr,
                overrides: String::new(),
            };
            cache.insert(cache_key, rc.clone());
        }
    });
    // Also populate the per-sheet pointer-keyed cache. This is the
    // fast path `resolve()` consults first — local map on the
    // sheet, no global RefCell, no `Rc::as_ptr(&app.sheet)`. The
    // existing `RESOLUTION_CACHE` insert above is kept so the
    // overrides-bearing path still benefits from caching.
    for (variants, rc) in &keyed {
        sheet.insert_variant(theme_ptr, variants.clone(), rc.clone());
    }
    let rules: Vec<Rc<StyleRules>> = keyed.into_iter().map(|(_, rc)| rc).collect();
    register(&rules);
    REGISTRATIONS.with(|r| {
        r.borrow_mut().insert(
            key,
            Registration {
                weak: Rc::downgrade(sheet),
                rules,
            },
        );
    });
}

/// Read the active theme. Subscribes the current effect (if any) to
/// theme changes — that's how reactive style application works.
pub fn active_theme() -> Rc<dyn Any> {
    ACTIVE_THEME.with(|t| {
        t.borrow()
            .as_ref()
            .expect("no theme installed; call install_theme(...) before rendering")
            .get()
    })
}

/// Returns the set of pre-resolvable `StyleRules` for a stylesheet
/// against a given theme. Includes:
/// - The base rules (no variants active).
/// - One entry per declared (axis, value) — variant overlay layered on
///   base.
/// - One entry per declared compound variant — the matched compound
///   layered on the base + the compound's `when` clause's variants.
///
/// Continuous overrides are NOT pre-generatable and aren't included.
/// Backends like the web backend use this to mint CSS classes ahead of
/// time so `apply_style` is a cache hit.
pub fn pregenerate_for_theme(sheet: &StyleSheet, theme: &dyn Any) -> Vec<Rc<StyleRules>> {
    pregenerate_keyed(sheet, theme)
        .into_iter()
        .map(|(_, rc)| rc)
        .collect()
}

/// Same as `pregenerate_for_theme` but also returns the
/// `VariantSet` each rule was resolved for. Used by
/// `ensure_registered_with` to populate the resolution cache so
/// `resolve()` returns the *same* `Rc<StyleRules>` instances the
/// backend registered — letting the backend's apply path
/// short-circuit a `content_key()` recomputation by keying on
/// `Rc::as_ptr` instead.
pub(crate) fn pregenerate_keyed(
    sheet: &StyleSheet,
    theme: &dyn Any,
) -> Vec<(VariantSet, Rc<StyleRules>)> {
    let mut out: Vec<(VariantSet, Rc<StyleRules>)> = Vec::new();

    // 1. Base.
    let base_vs = VariantSet::new();
    out.push((base_vs.clone(), Rc::new(sheet.resolve(&base_vs, theme))));

    // 2. Each (axis, value) — every single-axis variant selection.
    for (axis, value) in sheet.variant_keys() {
        let variants = VariantSet::new().with(axis, value);
        out.push((variants.clone(), Rc::new(sheet.resolve(&variants, theme))));
    }

    // 3. Each compound — the compound's `when` clause defines the
    //    minimum variant selection that triggers it.
    for compound_keys in sheet.compound_keys() {
        let mut variants = VariantSet::new();
        for (axis, value) in compound_keys {
            variants.0.insert(axis, value);
        }
        out.push((variants.clone(), Rc::new(sheet.resolve(&variants, theme))));
    }

    out
}

/// Resolve a style application against the current active theme.
/// Memoized: same key always returns the same `Rc<StyleRules>`
/// across calls until the cache is cleared (theme change) or
/// pruned (stylesheet dropped).
///
/// Cache entries are strong `Rc`s — that's what makes back-to-back
/// applies of the same style hit the cache. With `Weak`, the
/// returned `Rc` is the only strong holder; it dies between
/// successive applies, the cache always misses, and the framework
/// re-resolves rules from scratch for every styled node.
pub fn resolve(app: &StyleApplication) -> Rc<StyleRules> {
    let theme = active_theme();
    let theme_ptr = Rc::as_ptr(&theme) as *const ();

    // Fast path: no overrides + pre-registered variants. The most
    // common shape — a stylesheet-only `View(style = MySheet().axis(value))`
    // hits this. Lookup is a per-sheet RefCell borrow + one
    // HashMap probe, no global state touched.
    //
    // Skips:
    //   - `Rc::as_ptr(&app.sheet)` indirection (we already have the sheet)
    //   - global RESOLUTION_CACHE thread_local + RefCell borrow
    //   - building a full `ResolutionKey` with an empty-string
    //     overrides field
    //
    // Falls through to the slow path for any app with overrides
    // or with variant combinations the pregen didn't enumerate
    // (e.g. ad-hoc multi-axis selections).
    if !app.has_overrides {
        #[cfg(feature = "debug-stats")]
        let _t_fast = crate::debug::now_micros();
        if let Some(rc) = app.sheet.lookup_variant(theme_ptr, &app.variants) {
            #[cfg(feature = "debug-stats")]
            {
                crate::debug::record_apply_phase(
                    "resolve_fast_path_hit",
                    crate::debug::now_micros().saturating_sub(_t_fast),
                );
                crate::debug::record_style_cache_hit();
            }
            return rc;
        }
        #[cfg(feature = "debug-stats")]
        crate::debug::record_apply_phase(
            "resolve_fast_path_miss",
            crate::debug::now_micros().saturating_sub(_t_fast),
        );
    }

    // Slow path: build the full ResolutionKey and consult the
    // global cache.
    let overrides_key = if app.has_overrides {
        app.overrides.content_key()
    } else {
        String::new()
    };
    let key = ResolutionKey {
        sheet: Rc::as_ptr(&app.sheet),
        variants: app.variants.clone(),
        theme: theme_ptr,
        overrides: overrides_key,
    };

    // Cache hit? Return the shared Rc.
    if let Some(rc) = RESOLUTION_CACHE.with(|c| c.borrow().get(&key).cloned()) {
        #[cfg(feature = "debug-stats")]
        crate::debug::record_style_cache_hit();
        return rc;
    }
    #[cfg(feature = "debug-stats")]
    crate::debug::record_style_cache_miss();

    // Miss. Resolve fresh and stash a strong Rc.
    let base_and_variants = app.sheet.resolve(&app.variants, &*theme);
    let final_rules = base_and_variants.merge(&app.overrides);
    let resolved = Rc::new(final_rules);

    RESOLUTION_CACHE.with(|c| {
        c.borrow_mut().insert(key, resolved.clone());
    });

    resolved
}

// ----------------------------------------------------------------------------
// Builder support traits — used by the `stylesheet!` macro
// ----------------------------------------------------------------------------
//
// Variant setters (`.size(...)`) and override setters (`.padding(...)`)
// on a generated builder accept *anything that converts to a closure*
// reading the value. The same setter shape works for:
//
//   - a static enum value:        `.size(CardSize::Small)`
//   - a static primitive value:   `.padding(16.0)`
//   - a reactive signal:          `.padding(my_signal)`
//
// In the reactive case the builder's `IntoStyleSource` closure picks
// up the signal subscription naturally because it reads the value
// inside the apply-style effect.
//
// Each generated variant enum has a `pub fn as_variant_str(self) ->
// &'static str` accessor (emitted by the macro). The
// `IntoVariantSource` trait's impl for `E` uses that method to
// convert; the impl for `Signal<E>` reads the signal and converts.

pub trait IntoVariantSource<E: Copy + 'static> {
    fn into_variant_source(self) -> Box<dyn Fn() -> &'static str>;
}

pub trait IntoOverrideSource<T: Clone + 'static> {
    fn into_override_source(self) -> Box<dyn Fn() -> T>;
}

// A bit of plumbing: variant enums have `as_variant_str`. We can't
// require it via a trait the macro defines (orphan rules), so we
// instead expose a marker trait `VariantEnum` that the macro impl's
// on each generated enum.

pub trait VariantEnum: Copy + 'static {
    fn as_variant_str(self) -> &'static str;
}

impl<E: VariantEnum> IntoVariantSource<E> for E {
    fn into_variant_source(self) -> Box<dyn Fn() -> &'static str> {
        let s = self.as_variant_str();
        Box::new(move || s)
    }
}

impl<E: VariantEnum> IntoVariantSource<E> for crate::Signal<E> {
    fn into_variant_source(self) -> Box<dyn Fn() -> &'static str> {
        Box::new(move || self.get().as_variant_str())
    }
}

/// Closure-form wrapper. Lets author code derive a variant axis
/// reactively from any combination of signals — useful when the axis
/// is a function of state (e.g. `screen == Summary`) rather than the
/// value of a single `Signal<E>`. The framework's style-effect calls
/// the closure inside its re-resolution pass, so any signal the
/// closure reads becomes a dependency.
///
/// Wrapped via the [`derive`] free function to dodge Rust's coherence
/// rules (a blanket `impl<F: Fn() -> E> IntoVariantSource<E> for F`
/// conflicts with the existing `impl IntoVariantSource<E> for E`).
pub struct Derive<F>(pub F);

/// Convenience constructor: `derived(move || ...)`. Named with a
/// trailing `d` so it doesn't collide visually with `#[derive(...)]`
/// at the call site (and so a `use framework_core::derived;` doesn't
/// shadow std's `derive` attribute, even though they're in distinct
/// namespaces).
pub fn derived<F, T>(f: F) -> Derive<F>
where
    F: Fn() -> T + 'static,
{
    Derive(f)
}

impl<E, F> IntoVariantSource<E> for Derive<F>
where
    E: VariantEnum,
    F: Fn() -> E + 'static,
{
    fn into_variant_source(self) -> Box<dyn Fn() -> &'static str> {
        let f = self.0;
        Box::new(move || f().as_variant_str())
    }
}

impl<T: Clone + 'static> IntoOverrideSource<T> for T {
    fn into_override_source(self) -> Box<dyn Fn() -> T> {
        Box::new(move || self.clone())
    }
}

impl<T: Clone + 'static> IntoOverrideSource<T> for crate::Signal<T> {
    fn into_override_source(self) -> Box<dyn Fn() -> T> {
        Box::new(move || self.get())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestTheme {
        surface: String,
        medium: f32,
    }

    fn light() -> TestTheme {
        TestTheme { surface: "#fff".into(), medium: 16.0 }
    }

    fn dark() -> TestTheme {
        TestTheme { surface: "#111".into(), medium: 24.0 }
    }

    #[test]
    fn closure_stylesheet_reads_theme() {
        let sheet = StyleSheet::new(|t: &TestTheme| StyleRules {
            background: Some(Color(t.surface.clone())),
            padding_top: Some(Length::Px(t.medium)),
            ..Default::default()
        });
        let l = light();
        let r = sheet.resolve(&VariantSet::new(), &l);
        assert_eq!(r.background, Some(Color("#fff".into())));
        assert_eq!(r.padding_top, Some(Length::Px(16.0)));
    }

    #[test]
    fn static_stylesheet_ignores_theme() {
        let sheet = StyleSheet::r#static(StyleRules {
            background: Some(Color("#abc".into())),
            ..Default::default()
        });
        let l = light();
        let r = sheet.resolve(&VariantSet::new(), &l);
        assert_eq!(r.background, Some(Color("#abc".into())));
    }

    #[test]
    fn variant_overlays_layer_on_top_of_base() {
        let sheet = StyleSheet::new(|t: &TestTheme| StyleRules {
            background: Some(Color(t.surface.clone())),
            padding_top: Some(Length::Px(t.medium)),
            ..Default::default()
        })
        .variant("size", "large", |t: &TestTheme| StyleRules {
            padding_top: Some(Length::Px(t.medium * 2.0)),
            ..Default::default()
        });
        let l = light();
        let r = sheet.resolve(&VariantSet::new().with("size", "large"), &l);
        assert_eq!(r.background, Some(Color("#fff".into())));
        assert_eq!(r.padding_top, Some(Length::Px(32.0)));
    }

    #[test]
    fn theme_swap_changes_resolution() {
        install_theme(light());
        let sheet = Rc::new(StyleSheet::new(|t: &TestTheme| StyleRules {
            background: Some(Color(t.surface.clone())),
            ..Default::default()
        }));
        let app = StyleApplication::new(sheet);

        let r1 = resolve(&app);
        assert_eq!(r1.background, Some(Color("#fff".into())));

        set_theme(dark());
        let r2 = resolve(&app);
        assert_eq!(r2.background, Some(Color("#111".into())));
    }

    #[test]
    fn overrides_layer_on_top_of_base_and_variants() {
        install_theme(light());
        let sheet = Rc::new(
            StyleSheet::new(|t: &TestTheme| StyleRules {
                background: Some(Color(t.surface.clone())),
                font_size: Some(Length::Px(14.0)),
                padding_top: Some(Length::Px(t.medium)),
                ..Default::default()
            })
            .variant("size", "large", |_t: &TestTheme| StyleRules {
                font_size: Some(Length::Px(20.0)),
                ..Default::default()
            }),
        );

        // Base only: background from theme, font 14, padding from theme.
        let r1 = resolve(&StyleApplication::new(sheet.clone()));
        assert_eq!(r1.font_size, Some(Length::Px(14.0)));

        // With variant: font becomes 20.
        let r2 = resolve(&StyleApplication::new(sheet.clone()).with("size", "large"));
        assert_eq!(r2.font_size, Some(Length::Px(20.0)));

        // With variant + override: override wins.
        let r3 = resolve(
            &StyleApplication::new(sheet.clone())
                .with("size", "large")
                .override_font_size(17.5),
        );
        assert_eq!(r3.font_size, Some(Length::Px(17.5)));
        // Other properties unaffected by the override.
        assert_eq!(r3.padding_top, Some(Length::Px(16.0)));

        // Different override values produce distinct cache entries.
        let r4 = resolve(
            &StyleApplication::new(sheet.clone())
                .with("size", "large")
                .override_font_size(99.0),
        );
        assert_eq!(r4.font_size, Some(Length::Px(99.0)));
        assert!(!Rc::ptr_eq(&r3, &r4));
    }

    #[test]
    fn variant_default_applies_when_axis_unselected() {
        let sheet = StyleSheet::new(|t: &TestTheme| StyleRules {
            background: Some(Color(t.surface.clone())),
            padding_top: Some(Length::Px(8.0)),
            ..Default::default()
        })
        .variant("size", "small", |_t: &TestTheme| StyleRules {
            padding_top: Some(Length::Px(4.0)),
            ..Default::default()
        })
        .variant("size", "large", |_t: &TestTheme| StyleRules {
            padding_top: Some(Length::Px(16.0)),
            ..Default::default()
        })
        .variant_default("size", "large");

        // Call site omits `size` → default "large" applies → padding 16.
        let r = sheet.resolve(&VariantSet::new(), &light());
        assert_eq!(r.padding_top, Some(Length::Px(16.0)));

        // Call site picks "small" → padding 4.
        let r2 = sheet.resolve(&VariantSet::new().with("size", "small"), &light());
        assert_eq!(r2.padding_top, Some(Length::Px(4.0)));
    }

    #[test]
    fn compound_variant_applies_only_when_all_match() {
        let sheet = StyleSheet::new(|_t: &TestTheme| StyleRules::default())
            .variant("size", "large", |_t: &TestTheme| StyleRules {
                padding_top: Some(Length::Px(16.0)),
                ..Default::default()
            })
            .variant("kind", "primary", |_t: &TestTheme| StyleRules {
                background: Some(Color("primary-bg".into())),
                ..Default::default()
            })
            .compound::<TestTheme, _>(
                vec![("size", "large"), ("kind", "primary")],
                |_t| StyleRules {
                    font_size: Some(Length::Px(24.0)),
                    ..Default::default()
                },
            );

        // Only size=large → compound NOT applied.
        let r1 = sheet.resolve(&VariantSet::new().with("size", "large"), &light());
        assert_eq!(r1.padding_top, Some(Length::Px(16.0)));
        assert_eq!(r1.font_size, None);

        // Both axes match → compound APPLIED.
        let r2 = sheet.resolve(
            &VariantSet::new().with("size", "large").with("kind", "primary"),
            &light(),
        );
        assert_eq!(r2.padding_top, Some(Length::Px(16.0)));
        assert_eq!(r2.background, Some(Color("primary-bg".into())));
        assert_eq!(r2.font_size, Some(Length::Px(24.0)));
    }

    #[test]
    fn variant_keys_lists_every_axis_value() {
        let sheet = StyleSheet::new(|_t: &TestTheme| StyleRules::default())
            .variant("size", "small", |_t: &TestTheme| StyleRules::default())
            .variant("size", "large", |_t: &TestTheme| StyleRules::default())
            .variant("kind", "primary", |_t: &TestTheme| StyleRules::default());
        let mut keys = sheet.variant_keys();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                ("kind".to_string(), "primary".to_string()),
                ("size".to_string(), "large".to_string()),
                ("size".to_string(), "small".to_string()),
            ]
        );
    }

    #[test]
    fn resolve_memoizes_same_inputs() {
        install_theme(light());
        let sheet = Rc::new(StyleSheet::r#static(StyleRules {
            background: Some(Color("#abc".into())),
            ..Default::default()
        }));
        let app = StyleApplication::new(sheet);
        let r1 = resolve(&app);
        let r2 = resolve(&app);
        assert!(Rc::ptr_eq(&r1, &r2));
    }
}
