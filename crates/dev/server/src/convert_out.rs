//! In-memory → wire conversions used by [`crate::WireRecordingBackend`]
//! when recording walker calls. Symmetric counterpart to the
//! `convert` module in `dev-client`.
//!
//! Style conversions are intentionally one-way + lossy in the same
//! way the in-memory `Tokenized<T>` collapses to its concrete
//! resolved literal when sent on the wire. Tokens are resolved
//! against the dev-side active theme before serialization.

use runtime_core::accessibility::{AccessibilityProps, LiveRegionPriority, Role};
use runtime_core::primitives;
use runtime_core::{
    AlignItems, AssetId, AssetSource, AssetTag, Color, Easing, FlexDirection, FontFamily,
    FontStyle, FontWeight, Gradient, GradientKind, GradientStop, JustifyContent, Length, Overflow,
    Position, RadialExtent, StateBits, StyleRules, SystemFallback, TextAlign, Tokenized,
    Transform, TypefaceFace, TypefaceId,
};
use wire::{
    AssetId as WireAssetId, TypefaceId as WireTypefaceId, WireAccessibilityAction,
    WireAccessibilityProps, WireAlignItems, WireAssetSource, WireAssetTag, WireColor, WireEasing,
    WireFillRule, WireFlexDirection, WireFontFamily, WireFontStyle, WireFontWeight, WireGradient,
    WireGradientKind, WireGradientStop, WireIconData, WireJustifyContent, WireLength,
    WireLiveRegionPriority, WireOverflow, WirePosition, WireRadialExtent, WireRole, WireStateBit,
    WireStyleRules, WireSystemFallback, WireTextAlign, WireTransform, WireTypefaceFace,
};

use crate::HandlerTable;

pub fn icon_data_to_wire(d: &primitives::icon::IconData) -> WireIconData {
    WireIconData {
        view_box: d.view_box,
        paths: d.paths.iter().map(|s| s.to_string()).collect(),
        fill_rule: match d.fill_rule {
            primitives::icon::FillRule::NonZero => WireFillRule::NonZero,
            primitives::icon::FillRule::EvenOdd => WireFillRule::EvenOdd,
        },
    }
}

pub fn easing_to_wire(e: Easing) -> WireEasing {
    match e {
        Easing::Linear => WireEasing::Linear,
        // CSS-default Ease collapses to EaseInOut on the wire — same
        // perceptual category, simplifies the wire enum.
        Easing::Ease => WireEasing::EaseInOut,
        Easing::EaseIn => WireEasing::EaseIn,
        Easing::EaseOut => WireEasing::EaseOut,
        Easing::EaseInOut => WireEasing::EaseInOut,
        Easing::CubicBezier(a, b, c, d) => WireEasing::Cubic(a, b, c, d),
    }
}

/// Bridge `runtime_core::animation::AnimProp` to its wire mirror.
/// One-to-one map; `GradientStopColor(idx)` carries the same `u8`
/// stop index inline.
pub fn anim_prop_to_wire(p: runtime_core::animation::AnimProp) -> wire::WireAnimProp {
    use runtime_core::animation::AnimProp;
    match p {
        AnimProp::Opacity => wire::WireAnimProp::Opacity,
        AnimProp::TranslateX => wire::WireAnimProp::TranslateX,
        AnimProp::TranslateY => wire::WireAnimProp::TranslateY,
        AnimProp::Scale => wire::WireAnimProp::Scale,
        AnimProp::ScaleX => wire::WireAnimProp::ScaleX,
        AnimProp::ScaleY => wire::WireAnimProp::ScaleY,
        AnimProp::RotateZ => wire::WireAnimProp::RotateZ,
        AnimProp::ZIndex => wire::WireAnimProp::ZIndex,
        AnimProp::MaxHeight => wire::WireAnimProp::MaxHeight,
        AnimProp::BackgroundColor => wire::WireAnimProp::BackgroundColor,
        AnimProp::ForegroundColor => wire::WireAnimProp::ForegroundColor,
        AnimProp::GradientStopColor(idx) => wire::WireAnimProp::GradientStopColor(idx),
    }
}

pub fn wire_state_bit_to_bits(b: WireStateBit) -> StateBits {
    match b {
        WireStateBit::Hovered => StateBits::HOVERED,
        WireStateBit::Pressed => StateBits::PRESSED,
        WireStateBit::Focused => StateBits::FOCUSED,
        WireStateBit::Disabled => StateBits::DISABLED,
    }
}

/// Expand a `StateBits` bitmask into a list of wire bits. Most
/// overlays carry a single bit, but the framework supports composite
/// bits (`HOVERED | FOCUSED`) by passing them all in one
/// `apply_styled_states` call.
pub fn expand_state_bits(bits: StateBits) -> Vec<WireStateBit> {
    let mut out = Vec::new();
    if bits.contains(StateBits::HOVERED) {
        out.push(WireStateBit::Hovered);
    }
    if bits.contains(StateBits::PRESSED) {
        out.push(WireStateBit::Pressed);
    }
    if bits.contains(StateBits::FOCUSED) {
        out.push(WireStateBit::Focused);
    }
    if bits.contains(StateBits::DISABLED) {
        out.push(WireStateBit::Disabled);
    }
    out
}

fn color_to_wire(c: &Color) -> WireColor {
    WireColor(c.0.clone())
}

fn length_to_wire(l: Length) -> WireLength {
    match l {
        Length::Px(v) => WireLength::Px(v),
        Length::Percent(v) => WireLength::Pct(v),
        Length::Auto => WireLength::Auto,
    }
}

fn flex_direction_to_wire(d: FlexDirection) -> WireFlexDirection {
    match d {
        FlexDirection::Row => WireFlexDirection::Row,
        FlexDirection::Column => WireFlexDirection::Column,
        FlexDirection::RowReverse => WireFlexDirection::RowReverse,
        FlexDirection::ColumnReverse => WireFlexDirection::ColumnReverse,
    }
}

fn justify_content_to_wire(j: JustifyContent) -> WireJustifyContent {
    match j {
        JustifyContent::FlexStart => WireJustifyContent::FlexStart,
        JustifyContent::FlexEnd => WireJustifyContent::FlexEnd,
        JustifyContent::Center => WireJustifyContent::Center,
        JustifyContent::SpaceBetween => WireJustifyContent::SpaceBetween,
        JustifyContent::SpaceAround => WireJustifyContent::SpaceAround,
        JustifyContent::SpaceEvenly => WireJustifyContent::SpaceEvenly,
    }
}

fn align_items_to_wire(a: AlignItems) -> WireAlignItems {
    match a {
        AlignItems::FlexStart => WireAlignItems::FlexStart,
        AlignItems::FlexEnd => WireAlignItems::FlexEnd,
        AlignItems::Center => WireAlignItems::Center,
        AlignItems::Stretch => WireAlignItems::Stretch,
        AlignItems::Baseline => WireAlignItems::Baseline,
    }
}

fn font_weight_to_wire(w: FontWeight) -> WireFontWeight {
    match w {
        FontWeight::Thin => WireFontWeight::Thin,
        FontWeight::ExtraLight => WireFontWeight::ExtraLight,
        FontWeight::Light => WireFontWeight::Light,
        FontWeight::Normal => WireFontWeight::Regular,
        FontWeight::Medium => WireFontWeight::Medium,
        FontWeight::SemiBold => WireFontWeight::SemiBold,
        FontWeight::Bold => WireFontWeight::Bold,
        FontWeight::ExtraBold => WireFontWeight::ExtraBold,
        FontWeight::Black => WireFontWeight::Black,
    }
}

fn text_align_to_wire(a: TextAlign) -> WireTextAlign {
    match a {
        TextAlign::Left => WireTextAlign::Left,
        TextAlign::Right => WireTextAlign::Right,
        TextAlign::Center => WireTextAlign::Center,
        TextAlign::Justify => WireTextAlign::Justify,
    }
}

fn tokenized_color(t: &Tokenized<Color>) -> WireColor {
    color_to_wire(t.value())
}

fn tokenized_length(t: &Tokenized<Length>) -> WireLength {
    length_to_wire(*t.value())
}

fn tokenized_f32(t: &Tokenized<f32>) -> f32 {
    *t.value()
}

pub fn style_rules_to_wire(r: &StyleRules) -> WireStyleRules {
    WireStyleRules {
        background: r.background.as_ref().map(tokenized_color),
        color: r.color.as_ref().map(tokenized_color),
        font_size: r.font_size.as_ref().map(tokenized_length),

        flex_direction: r.flex_direction.map(flex_direction_to_wire),
        justify_content: r.justify_content.map(justify_content_to_wire),
        align_items: r.align_items.map(align_items_to_wire),
        gap: r.gap.as_ref().map(tokenized_length),

        flex_grow: r.flex_grow.as_ref().map(tokenized_f32),
        flex_shrink: r.flex_shrink.as_ref().map(tokenized_f32),
        flex_basis: r.flex_basis.as_ref().map(tokenized_length),

        width: r.width.as_ref().map(tokenized_length),
        height: r.height.as_ref().map(tokenized_length),
        min_width: r.min_width.as_ref().map(tokenized_length),
        min_height: r.min_height.as_ref().map(tokenized_length),
        max_width: r.max_width.as_ref().map(tokenized_length),
        max_height: r.max_height.as_ref().map(tokenized_length),
        aspect_ratio: r.aspect_ratio,

        padding_top: r.padding_top.as_ref().map(tokenized_length),
        padding_right: r.padding_right.as_ref().map(tokenized_length),
        padding_bottom: r.padding_bottom.as_ref().map(tokenized_length),
        padding_left: r.padding_left.as_ref().map(tokenized_length),
        margin_top: r.margin_top.as_ref().map(tokenized_length),
        margin_right: r.margin_right.as_ref().map(tokenized_length),
        margin_bottom: r.margin_bottom.as_ref().map(tokenized_length),
        margin_left: r.margin_left.as_ref().map(tokenized_length),

        border_top_left_radius: r.border_top_left_radius.as_ref().map(tokenized_length),
        border_top_right_radius: r.border_top_right_radius.as_ref().map(tokenized_length),
        border_bottom_left_radius: r.border_bottom_left_radius.as_ref().map(tokenized_length),
        border_bottom_right_radius: r.border_bottom_right_radius.as_ref().map(tokenized_length),

        opacity: r.opacity.as_ref().map(tokenized_f32),
        font_weight: r.font_weight.map(font_weight_to_wire),
        font_family: r.font_family.as_ref().map(font_family_to_wire),
        text_align: r.text_align.map(text_align_to_wire),

        position: r.position.map(position_to_wire),
        top: r.top.as_ref().map(tokenized_length),
        right: r.right.as_ref().map(tokenized_length),
        bottom: r.bottom.as_ref().map(tokenized_length),
        left: r.left.as_ref().map(tokenized_length),

        overflow: r.overflow.map(overflow_to_wire),
        transform: r.transform.as_ref().map(|t| {
            t.iter().map(transform_to_wire).collect()
        }),
        background_gradient: r.background_gradient.as_ref().map(gradient_to_wire),
    }
}

pub fn position_to_wire(p: Position) -> WirePosition {
    match p {
        Position::Relative => WirePosition::Relative,
        Position::Absolute => WirePosition::Absolute,
        Position::Sticky => WirePosition::Sticky,
    }
}

pub fn overflow_to_wire(o: Overflow) -> WireOverflow {
    match o {
        Overflow::Visible => WireOverflow::Visible,
        Overflow::Hidden => WireOverflow::Hidden,
    }
}

pub fn transform_to_wire(t: &Transform) -> WireTransform {
    match t {
        Transform::TranslateX(l) => WireTransform::TranslateX(length_to_wire(*l)),
        Transform::TranslateY(l) => WireTransform::TranslateY(length_to_wire(*l)),
        Transform::Scale(s) => WireTransform::Scale(*s),
        Transform::ScaleXY { x, y } => WireTransform::ScaleXY { x: *x, y: *y },
        Transform::Rotate(deg) => WireTransform::Rotate(*deg),
        Transform::SkewX(deg) => WireTransform::SkewX(*deg),
        Transform::SkewY(deg) => WireTransform::SkewY(*deg),
    }
}

pub fn gradient_to_wire(g: &Gradient) -> WireGradient {
    WireGradient {
        kind: gradient_kind_to_wire(&g.kind),
        stops: g.stops.iter().map(gradient_stop_to_wire).collect(),
    }
}

pub fn gradient_kind_to_wire(k: &GradientKind) -> WireGradientKind {
    match k {
        GradientKind::Linear { angle_deg } => WireGradientKind::Linear {
            angle_deg: *angle_deg,
        },
        GradientKind::Radial { center, radius, extent } => WireGradientKind::Radial {
            center: *center,
            radius: *radius,
            extent: radial_extent_to_wire(*extent),
        },
    }
}

pub fn gradient_stop_to_wire(s: &GradientStop) -> WireGradientStop {
    WireGradientStop {
        offset: s.offset,
        color: color_to_wire(&s.color),
    }
}

pub fn radial_extent_to_wire(e: RadialExtent) -> WireRadialExtent {
    match e {
        RadialExtent::ClosestSide => WireRadialExtent::ClosestSide,
        RadialExtent::FarthestCorner => WireRadialExtent::FarthestCorner,
    }
}

pub fn font_family_to_wire(ff: &FontFamily) -> WireFontFamily {
    match ff {
        FontFamily::System(name) => WireFontFamily::System(name.clone()),
        FontFamily::Typeface(t) => WireFontFamily::Typeface {
            id: typeface_id_to_wire(t.id),
            family_name: t.family_name.to_string(),
        },
    }
}

pub fn font_style_to_wire(s: FontStyle) -> WireFontStyle {
    match s {
        FontStyle::Normal => WireFontStyle::Normal,
        FontStyle::Italic => WireFontStyle::Italic,
    }
}

pub fn system_fallback_to_wire(f: SystemFallback) -> WireSystemFallback {
    match f {
        SystemFallback::Serif => WireSystemFallback::Serif,
        SystemFallback::SansSerif => WireSystemFallback::SansSerif,
        SystemFallback::Monospace => WireSystemFallback::Monospace,
        SystemFallback::None => WireSystemFallback::None,
    }
}

pub fn asset_id_to_wire(id: AssetId) -> WireAssetId {
    WireAssetId(id.0)
}

pub fn typeface_id_to_wire(id: TypefaceId) -> WireTypefaceId {
    WireTypefaceId(id.0)
}

pub fn asset_tag_to_wire(t: AssetTag) -> WireAssetTag {
    match t {
        AssetTag::Font => WireAssetTag::Font,
        AssetTag::Image => WireAssetTag::Image,
        AssetTag::Audio => WireAssetTag::Audio,
        AssetTag::Video => WireAssetTag::Video,
        AssetTag::Blob => WireAssetTag::Blob,
    }
}

pub fn asset_source_to_wire(s: &AssetSource) -> WireAssetSource {
    match s {
        AssetSource::Embedded { bytes, extension } => WireAssetSource::Embedded {
            bytes: bytes.to_vec(),
            extension: (*extension).to_string(),
        },
        AssetSource::Bundled { path } => WireAssetSource::Bundled {
            path: (*path).to_string(),
        },
        // The AAS client is always the web backend, which links fonts
        // by URL and never needs the bytes — so collapse to the path
        // and keep the (potentially multi-MB) font bytes off the
        // websocket. The client resolves `Bundled` + `Font` to the
        // same `/{path}` served-file URL.
        AssetSource::BundledEmbedded { path, .. } => WireAssetSource::Bundled {
            path: (*path).to_string(),
        },
        AssetSource::Remote { url } => WireAssetSource::Remote {
            url: (*url).to_string(),
        },
    }
}

pub fn typeface_face_to_wire(f: &TypefaceFace) -> WireTypefaceFace {
    WireTypefaceFace {
        weight: font_weight_to_wire(f.weight),
        style: font_style_to_wire(f.style),
        asset: asset_id_to_wire(f.asset),
    }
}

// ---------------------------------------------------------------------------
// Accessibility: runtime_core → wire.
// ---------------------------------------------------------------------------

/// Convert an `&AccessibilityProps` into its wire mirror. Carries
/// label / hint / identifier / hidden / role / traits / live-region
/// across faithfully. For each [`runtime_core::accessibility::AccessibilityAction`]
/// the recorder allocates a fresh [`wire::HandlerId`] in `handlers`
/// (registering the action's `Rc<dyn Fn()>` so the reverse-channel
/// `AppToDev::Event { handler, args: Unit }` dispatches it). The shape
/// matches how `on_click` / `on_change` cross the wire — see the
/// `WireAccessibilityAction` docs.
pub fn a11y_to_wire(p: &AccessibilityProps, handlers: &mut HandlerTable) -> WireAccessibilityProps {
    WireAccessibilityProps {
        label: p.label.clone(),
        hint: p.hint.clone(),
        identifier: p.identifier.clone(),
        hidden: p.hidden,
        role: p.role.map(role_to_wire),
        traits: p.traits.bits(),
        live_region: p.live_region.map(live_region_to_wire),
        actions: p
            .actions
            .iter()
            .map(|a| WireAccessibilityAction {
                name: a.name.clone(),
                // Mirror the `on_click` pattern: register the closure
                // into the recorder's `HandlerTable` and put the
                // resulting `HandlerId` on the wire. The replayer
                // synthesizes a trampoline that posts
                // `AppToDev::Event { handler: id, args: Unit }` so
                // AT-triggered AX actions on the app side reach the
                // dev-side closure.
                handler: handlers.register_unit(a.handler.clone()),
            })
            .collect(),
    }
}

pub fn role_to_wire(r: Role) -> WireRole {
    match r {
        Role::Button => WireRole::Button,
        Role::Link => WireRole::Link,
        Role::Image => WireRole::Image,
        Role::Text => WireRole::Text,
        Role::Header => WireRole::Header,
        Role::List => WireRole::List,
        Role::ListItem => WireRole::ListItem,
        Role::Group => WireRole::Group,
        Role::Separator => WireRole::Separator,
        Role::TextField => WireRole::TextField,
        Role::TextArea => WireRole::TextArea,
        Role::Switch => WireRole::Switch,
        Role::Slider => WireRole::Slider,
        Role::Checkbox => WireRole::Checkbox,
        Role::RadioButton => WireRole::RadioButton,
        Role::RadioGroup => WireRole::RadioGroup,
        Role::ComboBox => WireRole::ComboBox,
        Role::SearchField => WireRole::SearchField,
        Role::Tab => WireRole::Tab,
        Role::TabList => WireRole::TabList,
        Role::TabPanel => WireRole::TabPanel,
        Role::NavigationLink => WireRole::NavigationLink,
        Role::MenuItem => WireRole::MenuItem,
        Role::Menu => WireRole::Menu,
        Role::MenuBar => WireRole::MenuBar,
        Role::Toolbar => WireRole::Toolbar,
        Role::Alert => WireRole::Alert,
        Role::Status => WireRole::Status,
        Role::ProgressBar => WireRole::ProgressBar,
        Role::Spinner => WireRole::Spinner,
        Role::Dialog => WireRole::Dialog,
        Role::AlertDialog => WireRole::AlertDialog,
        Role::Drawer => WireRole::Drawer,
        Role::Popover => WireRole::Popover,
        Role::Tooltip => WireRole::Tooltip,
        Role::Region => WireRole::Region,
        // `Role` is `#[non_exhaustive]`; future runtime-core variants
        // that this conversion module hasn't been taught about decode
        // as `Unknown` on the receiver. Acceptable because in dev-mode
        // both sides ship from the same commit — the catch-all just
        // keeps us forward-compatible against a future schema bump.
        _ => WireRole::Unknown,
    }
}

pub fn live_region_to_wire(p: LiveRegionPriority) -> WireLiveRegionPriority {
    match p {
        LiveRegionPriority::Polite => WireLiveRegionPriority::Polite,
        LiveRegionPriority::Assertive => WireLiveRegionPriority::Assertive,
    }
}
