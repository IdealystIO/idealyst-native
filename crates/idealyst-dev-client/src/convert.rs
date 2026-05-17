//! Wire → in-memory conversions used by [`crate::WireBackend`] when
//! replaying commands. Mirrors are intentionally lossy for the
//! prototype: `Tokenized<T>` becomes a plain literal (the dev side
//! resolves tokens before serialization), unsupported wire variants
//! are mapped to sensible defaults.

use std::rc::Rc;

use framework_core::primitives;
use framework_core::{
    AlignItems, Color, Easing, FlexDirection, FontWeight, JustifyContent, Length, StateBits,
    StyleRules, TextAlign, Tokenized,
};
use idealyst_wire::{
    HandlerId, WireAlignItems, WireColor, WireEasing, WireFillRule, WireFlexDirection,
    WireFontWeight, WireIconData, WireJustifyContent, WireLength, WireMountPolicy,
    WirePresenceState, WireScreenOptions, WireStateBit, WireStyleRules, WireTextAlign,
};

pub fn wire_color_to_color(c: WireColor) -> Color {
    Color(c.0)
}

pub fn wire_length(l: WireLength) -> Length {
    match l {
        WireLength::Px(v) => Length::Px(v),
        WireLength::Pct(v) => Length::Percent(v),
        WireLength::Auto => Length::Auto,
    }
}

pub fn wire_flex_direction(d: WireFlexDirection) -> FlexDirection {
    match d {
        WireFlexDirection::Row => FlexDirection::Row,
        WireFlexDirection::Column => FlexDirection::Column,
        WireFlexDirection::RowReverse => FlexDirection::RowReverse,
        WireFlexDirection::ColumnReverse => FlexDirection::ColumnReverse,
    }
}

pub fn wire_justify_content(j: WireJustifyContent) -> JustifyContent {
    match j {
        WireJustifyContent::FlexStart => JustifyContent::FlexStart,
        WireJustifyContent::FlexEnd => JustifyContent::FlexEnd,
        WireJustifyContent::Center => JustifyContent::Center,
        WireJustifyContent::SpaceBetween => JustifyContent::SpaceBetween,
        WireJustifyContent::SpaceAround => JustifyContent::SpaceAround,
        WireJustifyContent::SpaceEvenly => JustifyContent::SpaceEvenly,
    }
}

pub fn wire_align_items(a: WireAlignItems) -> AlignItems {
    match a {
        WireAlignItems::FlexStart => AlignItems::FlexStart,
        WireAlignItems::FlexEnd => AlignItems::FlexEnd,
        WireAlignItems::Center => AlignItems::Center,
        WireAlignItems::Stretch => AlignItems::Stretch,
        WireAlignItems::Baseline => AlignItems::Baseline,
    }
}

pub fn wire_font_weight(w: WireFontWeight) -> FontWeight {
    match w {
        WireFontWeight::Thin => FontWeight::Thin,
        WireFontWeight::ExtraLight => FontWeight::ExtraLight,
        WireFontWeight::Light => FontWeight::Light,
        WireFontWeight::Regular => FontWeight::Normal,
        WireFontWeight::Medium => FontWeight::Medium,
        WireFontWeight::SemiBold => FontWeight::SemiBold,
        WireFontWeight::Bold => FontWeight::Bold,
        WireFontWeight::ExtraBold => FontWeight::ExtraBold,
        WireFontWeight::Black => FontWeight::Black,
    }
}

pub fn wire_text_align(t: WireTextAlign) -> TextAlign {
    match t {
        WireTextAlign::Left | WireTextAlign::Start => TextAlign::Left,
        WireTextAlign::Right | WireTextAlign::End => TextAlign::Right,
        WireTextAlign::Center => TextAlign::Center,
        WireTextAlign::Justify => TextAlign::Justify,
    }
}

pub fn wire_easing(e: WireEasing) -> Easing {
    match e {
        WireEasing::Linear => Easing::Linear,
        WireEasing::EaseIn => Easing::EaseIn,
        WireEasing::EaseOut => Easing::EaseOut,
        WireEasing::EaseInOut => Easing::EaseInOut,
        WireEasing::Cubic(a, b, c, d) => Easing::CubicBezier(a, b, c, d),
    }
}

pub fn wire_state_bit(b: WireStateBit) -> StateBits {
    match b {
        WireStateBit::Hovered => StateBits::HOVERED,
        WireStateBit::Pressed => StateBits::PRESSED,
        WireStateBit::Focused => StateBits::FOCUSED,
        WireStateBit::Disabled => StateBits::DISABLED,
    }
}

pub fn axis_name_to_wire_state(axis: &'static str) -> Option<WireStateBit> {
    match axis {
        "__state_hovered" => Some(WireStateBit::Hovered),
        "__state_pressed" => Some(WireStateBit::Pressed),
        "__state_focused" => Some(WireStateBit::Focused),
        "__state_disabled" => Some(WireStateBit::Disabled),
        _ => None,
    }
}

pub fn wire_presence_state(s: WirePresenceState) -> primitives::presence::PresenceState {
    let mut state = primitives::presence::PresenceState::rest();
    if let Some(v) = s.opacity {
        state.opacity = Some(v);
    }
    if let Some(v) = s.tx {
        state.translate_x = Some(v);
    }
    if let Some(v) = s.ty {
        state.translate_y = Some(v);
    }
    if let Some(v) = s.scale {
        state.scale = Some(v);
    }
    state
}

pub fn wire_activity_size(
    s: idealyst_wire::WireActivityIndicatorSize,
) -> primitives::activity_indicator::ActivityIndicatorSize {
    match s {
        idealyst_wire::WireActivityIndicatorSize::Small => {
            primitives::activity_indicator::ActivityIndicatorSize::Small
        }
        idealyst_wire::WireActivityIndicatorSize::Large => {
            primitives::activity_indicator::ActivityIndicatorSize::Large
        }
    }
}

/// Convert a wire icon to the framework's static-borrow form.
///
/// Framework `IconData` requires `&'static [&'static str]` for its
/// path list because in normal usage icons are `const`-built and live
/// in `.rodata`. The wire ships dynamic strings, so we lean on a
/// thread-local arena to give each replayed icon a stable 'static
/// lifetime. Memory leaks accumulate across reloads but this is dev
/// mode — same lifetime as the dev session.
pub fn wire_icon_to_static(w: WireIconData) -> primitives::icon::IconData {
    let paths_static: &'static [&'static str] = leak_paths(w.paths);
    primitives::icon::IconData {
        view_box: w.view_box,
        paths: paths_static,
        fill_rule: match w.fill_rule {
            WireFillRule::NonZero => primitives::icon::FillRule::NonZero,
            WireFillRule::EvenOdd => primitives::icon::FillRule::EvenOdd,
        },
    }
}

fn leak_paths(paths: Vec<String>) -> &'static [&'static str] {
    // Leak each String to get a 'static &str, then leak the Vec to
    // get a 'static slice. This is dev-mode only; the leak is
    // proportional to "icons-seen-this-dev-session" which is bounded
    // by the application's vocabulary.
    let static_paths: Vec<&'static str> =
        paths.into_iter().map(|s| Box::leak(s.into_boxed_str()) as &'static str).collect();
    Box::leak(static_paths.into_boxed_slice())
}

pub fn wire_mount_policy(p: WireMountPolicy) -> primitives::navigator::MountPolicy {
    match p {
        WireMountPolicy::EagerPersistent => primitives::navigator::MountPolicy::EagerPersistent,
        WireMountPolicy::LazyPersistent => primitives::navigator::MountPolicy::LazyPersistent,
        WireMountPolicy::LazyDisposing => primitives::navigator::MountPolicy::LazyDisposing,
    }
}

/// Convert a wire `WireScreenOptions` into the framework's
/// `ScreenOptions`, resolving any `HandlerId` references via the
/// supplied `handler_factory` (which builds an
/// `Rc<dyn Fn()>` that ships an `AppToDev::Event` back to dev).
pub fn wire_screen_options(
    w: &WireScreenOptions,
    mut handler_factory: impl FnMut(HandlerId) -> Rc<dyn Fn()>,
) -> primitives::navigator::ScreenOptions {
    let mut opts = primitives::navigator::ScreenOptions::new();
    if let Some(ref t) = w.title {
        opts = opts.title(t.clone());
    }
    if let Some(shown) = w.header_shown {
        opts = opts.header_shown(shown);
    }
    if let Some(ref hb) = w.header_left {
        let on_press_cb = handler_factory(hb.on_press);
        let mut btn = primitives::navigator::HeaderButton {
            icon: hb.icon.clone(),
            on_press: on_press_cb,
            tint: hb.tint.as_ref().map(|c| wire_color_to_color(c.clone())),
        };
        // HeaderButton has Clone so we can hand it straight in.
        let _ = &mut btn;
        opts = opts.header_left(btn);
    }
    if let Some(ref hb) = w.header_right {
        let on_press_cb = handler_factory(hb.on_press);
        let btn = primitives::navigator::HeaderButton {
            icon: hb.icon.clone(),
            on_press: on_press_cb,
            tint: hb.tint.as_ref().map(|c| wire_color_to_color(c.clone())),
        };
        opts = opts.header_right(btn);
    }
    opts
}

/// Materialize a wire style into a concrete `StyleRules`. Tokenized
/// values are emitted as `Tokenized::Literal` since the dev side
/// already resolved tokens against the active theme.
pub fn wire_style_to_rules(w: WireStyleRules) -> StyleRules {
    let mut s = StyleRules::default();

    s.background = w.background.map(|c| Tokenized::Literal(wire_color_to_color(c)));
    s.color = w.color.map(|c| Tokenized::Literal(wire_color_to_color(c)));
    s.font_size = w.font_size.map(|l| Tokenized::Literal(wire_length(l)));

    s.flex_direction = w.flex_direction.map(wire_flex_direction);
    s.justify_content = w.justify_content.map(wire_justify_content);
    s.align_items = w.align_items.map(wire_align_items);
    s.gap = w.gap.map(|l| Tokenized::Literal(wire_length(l)));

    s.flex_grow = w.flex_grow.map(Tokenized::Literal);
    s.flex_shrink = w.flex_shrink.map(Tokenized::Literal);
    s.flex_basis = w.flex_basis.map(|l| Tokenized::Literal(wire_length(l)));

    s.width = w.width.map(|l| Tokenized::Literal(wire_length(l)));
    s.height = w.height.map(|l| Tokenized::Literal(wire_length(l)));
    s.min_width = w.min_width.map(|l| Tokenized::Literal(wire_length(l)));
    s.min_height = w.min_height.map(|l| Tokenized::Literal(wire_length(l)));
    s.max_width = w.max_width.map(|l| Tokenized::Literal(wire_length(l)));
    s.max_height = w.max_height.map(|l| Tokenized::Literal(wire_length(l)));

    s.padding_top = w.padding_top.map(|l| Tokenized::Literal(wire_length(l)));
    s.padding_right = w.padding_right.map(|l| Tokenized::Literal(wire_length(l)));
    s.padding_bottom = w.padding_bottom.map(|l| Tokenized::Literal(wire_length(l)));
    s.padding_left = w.padding_left.map(|l| Tokenized::Literal(wire_length(l)));

    s.margin_top = w.margin_top.map(|l| Tokenized::Literal(wire_length(l)));
    s.margin_right = w.margin_right.map(|l| Tokenized::Literal(wire_length(l)));
    s.margin_bottom = w.margin_bottom.map(|l| Tokenized::Literal(wire_length(l)));
    s.margin_left = w.margin_left.map(|l| Tokenized::Literal(wire_length(l)));

    s.border_top_left_radius = w
        .border_top_left_radius
        .map(|l| Tokenized::Literal(wire_length(l)));
    s.border_top_right_radius = w
        .border_top_right_radius
        .map(|l| Tokenized::Literal(wire_length(l)));
    s.border_bottom_left_radius = w
        .border_bottom_left_radius
        .map(|l| Tokenized::Literal(wire_length(l)));
    s.border_bottom_right_radius = w
        .border_bottom_right_radius
        .map(|l| Tokenized::Literal(wire_length(l)));

    s.opacity = w.opacity.map(Tokenized::Literal);
    s.font_weight = w.font_weight.map(wire_font_weight);
    s.text_align = w.text_align.map(wire_text_align);

    s
}
