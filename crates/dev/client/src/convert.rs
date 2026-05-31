//! Wire → in-memory conversions used by [`crate::WireBackend`] when
//! replaying commands. Mirrors are intentionally lossy for the
//! prototype: `Tokenized<T>` becomes a plain literal (the dev side
//! resolves tokens before serialization), unsupported wire variants
//! are mapped to sensible defaults.

use std::rc::Rc;

use runtime_core::accessibility::{
    AccessibilityProps, AccessibilityTraits, LiveRegionPriority, Role,
};
use runtime_core::primitives;
use runtime_core::{
    AlignItems, AssetId as CoreAssetId, AssetSource, AssetTag, Color, Easing, FlexDirection,
    FontFamily, FontStyle, FontWeight, Gradient, GradientKind, GradientStop, JustifyContent,
    Length, Overflow, Position, RadialExtent, StateBits, StyleRules, SystemFallback, TextAlign,
    Tokenized, Transform, TypefaceFace, TypefaceId as CoreTypefaceId,
};
use wire::{
    AssetId as WireAssetId, HandlerId, TypefaceId as WireTypefaceId, WireAccessibilityProps,
    WireAlignItems, WireAssetSource, WireAssetTag, WireColor, WireEasing, WireFillRule,
    WireFlexDirection, WireFontFamily, WireFontStyle, WireFontWeight, WireGradient,
    WireGradientKind, WireGradientStop, WireIconData, WireJustifyContent, WireLength,
    WireLiveRegionPriority, WireMountPolicy, WireOverflow, WirePosition, WirePresenceState,
    WireRadialExtent, WireRole, WireScreenOptions, WireStateBit, WireStyleRules, WireSystemFallback,
    WireTextAlign, WireTransform, WireTypefaceFace,
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

/// Wire-to-framework reverse of `dev_server::convert_out::anim_prop_to_wire`.
/// Returns `None` for the forward-compat `Unknown` variant — caller
/// drops the animation tick rather than aborting the batch (the next
/// tick supersedes anyway).
pub fn wire_anim_prop(w: wire::WireAnimProp) -> Option<runtime_core::animation::AnimProp> {
    use runtime_core::animation::AnimProp;
    Some(match w {
        wire::WireAnimProp::Opacity => AnimProp::Opacity,
        wire::WireAnimProp::TranslateX => AnimProp::TranslateX,
        wire::WireAnimProp::TranslateY => AnimProp::TranslateY,
        wire::WireAnimProp::Scale => AnimProp::Scale,
        wire::WireAnimProp::ScaleX => AnimProp::ScaleX,
        wire::WireAnimProp::ScaleY => AnimProp::ScaleY,
        wire::WireAnimProp::RotateZ => AnimProp::RotateZ,
        wire::WireAnimProp::ZIndex => AnimProp::ZIndex,
        wire::WireAnimProp::MaxHeight => AnimProp::MaxHeight,
        wire::WireAnimProp::BackgroundColor => AnimProp::BackgroundColor,
        wire::WireAnimProp::ForegroundColor => AnimProp::ForegroundColor,
        wire::WireAnimProp::GradientStopColor(idx) => AnimProp::GradientStopColor(idx),
        wire::WireAnimProp::Unknown => return None,
    })
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
    s: wire::WireActivityIndicatorSize,
) -> primitives::activity_indicator::ActivityIndicatorSize {
    match s {
        wire::WireActivityIndicatorSize::Small => {
            primitives::activity_indicator::ActivityIndicatorSize::Small
        }
        wire::WireActivityIndicatorSize::Large => {
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

/// Wire → mount-policy converter. The runtime-core mount-policy
/// enum moved into per-SDK type space; this stub preserves the call
/// site shape but returns a unit value (consumers ignore the result
/// pending the wire-protocol redesign).
pub fn wire_mount_policy(_p: WireMountPolicy) {
    // Pending wire-protocol redesign.
}

/// Wire → screen-options stub. Returns an opaque `Box<dyn Any>` —
/// the runtime-core `ScreenOptions` type was removed when the
/// navigator substrate moved out of core. The dev-client navigator
/// path is no-op until the new SDK-driven wire format lands.
pub fn wire_screen_options(
    _w: &WireScreenOptions,
    _handler_factory: impl FnMut(HandlerId) -> Rc<dyn Fn()>,
) -> Box<dyn std::any::Any> {
    Box::new(())
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
    s.aspect_ratio = w.aspect_ratio;

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
    s.font_family = w.font_family.map(wire_font_family);
    s.text_align = w.text_align.map(wire_text_align);

    // PROTOCOL_VERSION 8 additions. These are what made the runtime-server-hosted
    // welcome example show only the headline text — sun glare / vignette
    // / planets all depend on position:absolute + transform +
    // background_gradient + overflow:hidden, none of which crossed the
    // wire in v7.
    s.position = w.position.map(wire_position);
    s.top = w.top.map(|l| Tokenized::Literal(wire_length(l)));
    s.right = w.right.map(|l| Tokenized::Literal(wire_length(l)));
    s.bottom = w.bottom.map(|l| Tokenized::Literal(wire_length(l)));
    s.left = w.left.map(|l| Tokenized::Literal(wire_length(l)));
    s.overflow = w.overflow.map(wire_overflow);
    s.transform = w.transform.map(|t| t.into_iter().map(wire_transform).collect());
    s.background_gradient = w.background_gradient.map(wire_gradient);

    s
}

pub fn wire_position(p: WirePosition) -> Position {
    match p {
        WirePosition::Relative => Position::Relative,
        WirePosition::Absolute => Position::Absolute,
        WirePosition::Sticky => Position::Sticky,
    }
}

pub fn wire_overflow(o: WireOverflow) -> Overflow {
    match o {
        WireOverflow::Visible => Overflow::Visible,
        WireOverflow::Hidden => Overflow::Hidden,
    }
}

pub fn wire_transform(t: WireTransform) -> Transform {
    match t {
        WireTransform::TranslateX(l) => Transform::TranslateX(wire_length(l)),
        WireTransform::TranslateY(l) => Transform::TranslateY(wire_length(l)),
        WireTransform::Scale(s) => Transform::Scale(s),
        WireTransform::ScaleXY { x, y } => Transform::ScaleXY { x, y },
        WireTransform::Rotate(deg) => Transform::Rotate(deg),
        WireTransform::SkewX(deg) => Transform::SkewX(deg),
        WireTransform::SkewY(deg) => Transform::SkewY(deg),
    }
}

pub fn wire_gradient(g: WireGradient) -> Gradient {
    Gradient {
        kind: wire_gradient_kind(g.kind),
        stops: g.stops.into_iter().map(wire_gradient_stop).collect(),
    }
}

pub fn wire_gradient_kind(k: WireGradientKind) -> GradientKind {
    match k {
        WireGradientKind::Linear { angle_deg } => GradientKind::Linear { angle_deg },
        WireGradientKind::Radial { center, radius, extent } => GradientKind::Radial {
            center,
            radius,
            extent: wire_radial_extent(extent),
        },
    }
}

pub fn wire_gradient_stop(s: WireGradientStop) -> GradientStop {
    GradientStop {
        offset: s.offset,
        color: wire_color_to_color(s.color),
    }
}

pub fn wire_radial_extent(e: WireRadialExtent) -> RadialExtent {
    match e {
        WireRadialExtent::ClosestSide => RadialExtent::ClosestSide,
        WireRadialExtent::FarthestCorner => RadialExtent::FarthestCorner,
    }
}

/// `WireFontFamily::Typeface` only carries an id — the dev side
/// shipped the corresponding `RegisterTypeface` already, and the
/// app-side backend keeps the registration in its own table. The
/// replay path here reconstructs a [`FontFamily::Typeface`] from
/// a synthetic placeholder so the [`StyleRules`] is well-formed;
/// the web backend reads `tf.family_name` to emit
/// `font-family: "{name}"`, which works as long as the matching
/// `@font-face` rule has been injected.
///
/// We don't keep the registered name on the wire side at replay
/// time because the recording side already serialized the family
/// name into the registered typeface; the web backend looks it up
/// via its own [`Backend::register_typeface`] handler.
/// Reconstruct a `FontFamily` from its wire form. The `Typeface`
/// variant rehydrates an in-memory [`Typeface`](runtime_core::Typeface)
/// from the wire's `(id, family_name)` pair, leaking the name into a
/// `&'static str` so the struct matches the type of an
/// authoring-side `typeface!{}` literal. The face list is left empty
/// — the corresponding `Command::RegisterTypeface` arrived earlier
/// and the backend already holds the full registration.
pub fn wire_font_family(w: WireFontFamily) -> FontFamily {
    match w {
        WireFontFamily::System(name) => FontFamily::System(name),
        WireFontFamily::Typeface { id, family_name } => {
            let family_name_static: &'static str = Box::leak(family_name.into_boxed_str());
            FontFamily::Typeface(runtime_core::Typeface {
                id: wire_typeface_id(id),
                family_name: family_name_static,
                faces: &[],
                fallback: SystemFallback::None,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Asset conversions
// ---------------------------------------------------------------------------

pub fn wire_asset_id(id: WireAssetId) -> CoreAssetId {
    CoreAssetId(id.0)
}

pub fn wire_typeface_id(id: WireTypefaceId) -> CoreTypefaceId {
    CoreTypefaceId(id.0)
}

pub fn wire_asset_tag(t: WireAssetTag) -> AssetTag {
    match t {
        WireAssetTag::Font => AssetTag::Font,
        WireAssetTag::Image => AssetTag::Image,
        WireAssetTag::Audio => AssetTag::Audio,
        WireAssetTag::Video => AssetTag::Video,
        WireAssetTag::Blob => AssetTag::Blob,
    }
}

pub fn wire_font_style(s: WireFontStyle) -> FontStyle {
    match s {
        WireFontStyle::Normal => FontStyle::Normal,
        WireFontStyle::Italic => FontStyle::Italic,
    }
}

pub fn wire_system_fallback(f: WireSystemFallback) -> SystemFallback {
    match f {
        WireSystemFallback::Serif => SystemFallback::Serif,
        WireSystemFallback::SansSerif => SystemFallback::SansSerif,
        WireSystemFallback::Monospace => SystemFallback::Monospace,
        WireSystemFallback::None => SystemFallback::None,
    }
}

/// `AssetSource` cannot reference the wire's owned `String` / `Vec<u8>`
/// once the command is consumed — `runtime_core::AssetSource` keeps
/// `&'static` slices. To bridge them at runtime we leak the bytes /
/// path / URL into a static box. This is acceptable because (a) on the
/// authoring side `asset!` always produces literally-static data, so a
/// matching runtime lifetime is the natural reconstruction, and (b)
/// each unique asset is leaked at most once per session — the
/// `WireBackend` dedupes by [`AssetId`] before calling this. Callers
/// that re-register with new sources will leak proportionally; that
/// matches the dev-mode-only lifetime of the wire path.
pub fn wire_asset_source(s: WireAssetSource) -> AssetSource {
    match s {
        WireAssetSource::Bundled { path } => AssetSource::Bundled {
            path: Box::leak(path.into_boxed_str()),
        },
        WireAssetSource::Remote { url } => AssetSource::Remote {
            url: Box::leak(url.into_boxed_str()),
        },
        WireAssetSource::Embedded { bytes, extension } => AssetSource::Embedded {
            bytes: Box::leak(bytes.into_boxed_slice()),
            extension: Box::leak(extension.into_boxed_str()),
        },
    }
}

// ---------------------------------------------------------------------------
// Accessibility: wire → runtime_core.
// ---------------------------------------------------------------------------

/// Reconstruct an `AccessibilityProps` from its wire mirror. Traits
/// decode via `from_bits_truncate` so an unknown future bit silently
/// drops on this side rather than failing the whole batch. Each
/// [`wire::WireAccessibilityAction`] becomes an
/// [`runtime_core::accessibility::AccessibilityAction`] whose
/// `handler` is built via `handler_factory(id)` — the standard
/// trampoline that posts `AppToDev::Event { handler, args: Unit }`
/// over the reverse channel, matching how `on_click` / `on_change`
/// resolve.
///
/// `handler_factory` mirrors the signature used by
/// [`wire_screen_options`]: callers pass a closure that captures the
/// app-side `WireBackend`'s outbound channel sender. See
/// `dev-client/src/lib.rs` for the canonical call sites.
pub fn wire_a11y_to_props(
    w: WireAccessibilityProps,
    mut handler_factory: impl FnMut(HandlerId) -> Rc<dyn Fn()>,
) -> AccessibilityProps {
    AccessibilityProps {
        label: w.label,
        hint: w.hint,
        identifier: w.identifier,
        hidden: w.hidden,
        role: w.role.and_then(wire_role_to_role),
        traits: AccessibilityTraits::from_bits_truncate(w.traits),
        live_region: w.live_region.map(wire_live_region_to_priority),
        actions: w
            .actions
            .into_iter()
            .map(|a| runtime_core::accessibility::AccessibilityAction {
                name: a.name,
                handler: handler_factory(a.handler),
            })
            .collect(),
    }
}

/// Reverse of `WireRole`. `Unknown` decodes to `None` (caller treats
/// it as "no override; let the primitive's inferred role stand"), per
/// the design note on `WireRole::Unknown`.
pub fn wire_role_to_role(r: WireRole) -> Option<Role> {
    Some(match r {
        WireRole::Button => Role::Button,
        WireRole::Link => Role::Link,
        WireRole::Image => Role::Image,
        WireRole::Text => Role::Text,
        WireRole::Header => Role::Header,
        WireRole::List => Role::List,
        WireRole::ListItem => Role::ListItem,
        WireRole::Group => Role::Group,
        WireRole::Separator => Role::Separator,
        WireRole::TextField => Role::TextField,
        WireRole::TextArea => Role::TextArea,
        WireRole::Switch => Role::Switch,
        WireRole::Slider => Role::Slider,
        WireRole::Checkbox => Role::Checkbox,
        WireRole::RadioButton => Role::RadioButton,
        WireRole::RadioGroup => Role::RadioGroup,
        WireRole::ComboBox => Role::ComboBox,
        WireRole::SearchField => Role::SearchField,
        WireRole::Tab => Role::Tab,
        WireRole::TabList => Role::TabList,
        WireRole::TabPanel => Role::TabPanel,
        WireRole::NavigationLink => Role::NavigationLink,
        WireRole::MenuItem => Role::MenuItem,
        WireRole::Menu => Role::Menu,
        WireRole::MenuBar => Role::MenuBar,
        WireRole::Toolbar => Role::Toolbar,
        WireRole::Alert => Role::Alert,
        WireRole::Status => Role::Status,
        WireRole::ProgressBar => Role::ProgressBar,
        WireRole::Spinner => Role::Spinner,
        WireRole::Dialog => Role::Dialog,
        WireRole::AlertDialog => Role::AlertDialog,
        WireRole::Drawer => Role::Drawer,
        WireRole::Popover => Role::Popover,
        WireRole::Tooltip => Role::Tooltip,
        WireRole::Region => Role::Region,
        WireRole::Unknown => return None,
    })
}

pub fn wire_live_region_to_priority(p: WireLiveRegionPriority) -> LiveRegionPriority {
    match p {
        WireLiveRegionPriority::Polite => LiveRegionPriority::Polite,
        WireLiveRegionPriority::Assertive => LiveRegionPriority::Assertive,
    }
}

pub fn wire_typeface_face(f: WireTypefaceFace) -> TypefaceFace {
    TypefaceFace {
        weight: wire_font_weight(f.weight),
        style: wire_font_style(f.style),
        asset: wire_asset_id(f.asset),
        // The face's source rides on the corresponding RegisterAsset
        // command — the wire form keeps them separate. At replay
        // time the backend uses `face.asset` (the id) for lookup
        // and ignores `source` on the face itself; we synthesize a
        // placeholder so the struct is well-formed. Web's
        // `register_typeface` queries `asset_urls` by id and never
        // touches this field.
        source: AssetSource::Bundled { path: "" },
    }
}
