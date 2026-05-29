//! Platform-neutral `StyleRules` → CSS string conversion.
//!
//! Each framework style enum (`FlexDirection`, `AlignItems`, …) has a
//! tiny `fn _css(v) -> &'static str` mapping it to the matching CSS
//! keyword. The top-level [`rules_to_css`] walks every `StyleRules`
//! field and produces a CSS declaration body suitable for one class
//! (or an inline `style="…"` attribute).
//!
//! This lives in its own crate — not `runtime-core` (CSS is not a
//! core primitive; core stays platform-agnostic) and not `backend-web`
//! (which pulls in `web-sys`/`wasm-bindgen` and cannot build for a
//! native server). Both the web backend and the SSR backend depend on
//! it so a node's first-paint CSS is byte-identical across the two.

use runtime_core::StyleRules;

// ---------------------------------------------------------------------------
// Navigator chrome layout — single source of truth
// ---------------------------------------------------------------------------

/// Class names stamped on navigator chrome nodes. Both the live web
/// backend (`web-navigator-helpers`, via `set_attribute("class", …)`)
/// and the generic SSR chrome handlers (`Backend::attach_html_class`)
/// stamp these exact strings, so the first-paint DOM structure is
/// identical and [`NAVIGATOR_LAYOUT_CSS`] styles both the same way.
pub mod nav_class {
    /// Stack/drawer navigator container (always paired with a kind class).
    pub const ROOT: &str = "ui-nav-root";
    /// A single mounted screen inside a stack navigator (full-bleed).
    pub const SCREEN: &str = "ui-nav-screen";
    /// Drawer navigator container (paired with [`ROOT`]).
    pub const DRAWER_ROOT: &str = "ui-nav-drawer-root";
    /// Optional top slot wrapper.
    pub const DRAWER_TOP: &str = "ui-nav-drawer-top";
    /// Optional bottom slot wrapper.
    pub const DRAWER_BOTTOM: &str = "ui-nav-drawer-bottom";
    /// The row holding sidebar + body outlet + trailing.
    pub const DRAWER_MIDDLE: &str = "ui-nav-drawer-middle";
    /// Leading (sidebar) slot wrapper.
    pub const DRAWER_SIDEBAR: &str = "ui-nav-drawer-sidebar";
    /// Trailing slot wrapper.
    pub const DRAWER_TRAILING: &str = "ui-nav-drawer-trailing";
    /// Body outlet (screens mount here).
    pub const DRAWER_BODY: &str = "ui-nav-drawer-body";
    /// Body outlet in `bottom_in_scroll` mode (body is the scroll context).
    pub const DRAWER_BODY_SCROLLS: &str = "ui-nav-drawer-body-scrolls";
}

/// The canonical navigator layout stylesheet — the **single definition**
/// of how navigator chrome lays out. The web backend injects it into
/// `<head>` at navigator-init time; the SSR backend ships it in the
/// rendered document `<head>`. One definition guarantees the server's
/// first paint matches the live web layout exactly (no style-flash on
/// hydration).
///
/// See [`nav_class`] for the class names and the layout diagram in
/// `web-navigator-helpers`'s `ensure_navigator_css`. Scrolling here is
/// expressed as `overflow-y: auto` on the chrome wrappers (a layout
/// concern owned by the navigator), distinct from the framework's
/// `ScrollView` primitive.
pub const NAVIGATOR_LAYOUT_CSS: &str = concat!(
    ".ui-nav-root{position:relative;width:100%;height:100%;}",
    ".ui-nav-screen{position:absolute!important;inset:0!important;width:100%;height:100%;}",
    ".ui-nav-drawer-root{display:flex;flex-direction:column;width:100%;height:100%;}",
    ".ui-nav-drawer-top{flex:0 0 auto;width:100%;}",
    ".ui-nav-drawer-bottom{flex:0 0 auto;width:100%;}",
    ".ui-nav-drawer-middle{flex:1 1 auto;display:flex;flex-direction:row;width:100%;min-height:0;}",
    ".ui-nav-drawer-sidebar{flex:0 0 auto;height:100%;overflow-y:auto;}",
    ".ui-nav-drawer-trailing{flex:0 0 auto;height:100%;overflow-y:auto;}",
    ".ui-nav-drawer-body{flex:1 1 auto;position:relative;height:100%;overflow:hidden;}",
    ".ui-nav-drawer-body-scrolls{flex:1 1 auto;position:relative;height:100%;overflow-y:auto;display:flex;flex-direction:column;}",
    ".ui-nav-drawer-body-scrolls>*{flex-shrink:0;}",
);

// ---------------------------------------------------------------------------
// Base reset + per-primitive default styles — single source of truth
// shared by the web backend (applied at create/init time) and the SSR
// backend (emitted in `<head>` / set inline on the same nodes), so the
// SSR first paint inherits the same primitive defaults the live app has.
// ---------------------------------------------------------------------------

/// Universal `box-sizing: border-box`. The framework's box model is
/// React-Native-style — padding/border live INSIDE the declared
/// width/height. Without this the browser's default `content-box` adds
/// padding OUTSIDE the size, so e.g. a 100%-height sidebar with padding
/// ends up taller than the viewport and overflows/scrolls. Specificity 0,
/// so any author class rule that sets `box-sizing` still wins.
pub const BOX_SIZING_RESET: &str = "*, *::before, *::after { box-sizing: border-box; }";

/// `<button>` element reset. `:where(button)` is specificity 0 so author
/// `apply_style` classes win; this just strips the browser's chunky
/// default chrome and restores a pointer cursor + flex centering.
pub const BUTTON_RESET: &str = ":where(button) { all: unset; box-sizing: border-box; \
    cursor: pointer; font: inherit; color: inherit; display: inline-flex; \
    align-items: center; justify-content: center; }";

/// The full base reset stylesheet ([`BOX_SIZING_RESET`] + [`BUTTON_RESET`]).
/// The SSR backend emits this once in `<head>`; the web backend inserts
/// the two rules at sheet indices 0/1.
pub fn base_reset_css() -> String {
    format!("{BOX_SIZING_RESET}{BUTTON_RESET}")
}

/// Default inline style for a `Link` primitive's `<a>`: strip the
/// browser's blue/underlined anchor defaults so the wrapping content's
/// styling shows through (authors override via their own style).
pub const LINK_RESET_STYLE: &str = "color: inherit; text-decoration: none; display: inline-flex;";

/// Default inline style for a `Button`'s content box (icon + label row).
pub const BUTTON_CONTENT_STYLE: &str = "display:inline-flex;align-items:center;gap:0.4em;";

/// Default inline style for an `Icon`'s inline element.
pub const ICON_INLINE_STYLE: &str = "display:inline-block;vertical-align:middle;";

/// Default inline style for a `Pressable` (bare clickable `<div>`): a
/// hand cursor. `cursor` isn't in the framework's styled-property model,
/// so it's set inline at create time on both backends.
pub const PRESSABLE_CURSOR_STYLE: &str = "cursor: pointer;";

/// Inline style for a reactive `when`/`switch`/`each` anchor placeholder:
/// `display: contents` makes it **layout-transparent** so the branch's
/// children inherit the surrounding flex/sizing context (and form their
/// containing block from the real parent, not the anchor). Without it an
/// opaque `<div>` collapses widths, breaks `flex:1`/`width:100%`, and —
/// critically — gives a `position: sticky` child a too-short containing
/// block so it stops sticking. Both backends stamp this on anchors.
pub const REACTIVE_ANCHOR_STYLE: &str = "display: contents";

/// Mint the deterministic class name for a resolved style — `"ui-"` plus
/// the 16-char hex of a `DefaultHasher` over `content_key`. **Single
/// source of truth shared by the web backend and SSR**, so a given style
/// gets the *same* class name on both: the SSR first paint stamps the
/// identical `class="ui-…"` the live web backend would, and ships a
/// matching `.ui-…{…}` rule — structurally identical to the WASM render,
/// not approximated with inline styles.
pub fn hash_class_name(content_key: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    content_key.hash(&mut h);
    let n = h.finish();
    let mut s = String::with_capacity(19);
    s.push_str("ui-");
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for shift in (0..16).rev() {
        let nibble = ((n >> (shift * 4)) & 0xf) as usize;
        s.push(HEX[nibble] as char);
    }
    s
}

/// The class name for a resolved `StyleRules` (`hash_class_name` over its
/// `content_key`). The matching rule body is [`rules_to_css`].
pub fn style_class_name(rules: &StyleRules) -> String {
    hash_class_name(&rules.content_key())
}

/// The CSS pseudo-class suffix for an interaction-state bit, so a state
/// overlay becomes `.ui-<hash><pseudo> { … }`. Shared by the web backend
/// (`apply_styled_states`) and SSR so hover/press/focus/disabled styles
/// resolve identically. `None` for unsupported / empty bits.
pub fn state_pseudo(state: runtime_core::StateBits) -> Option<&'static str> {
    use runtime_core::StateBits;
    match state {
        StateBits::HOVERED => Some(":hover"),
        StateBits::PRESSED => Some(":active"),
        StateBits::FOCUSED => Some(":focus"),
        StateBits::DISABLED => Some(":disabled"),
        _ => None,
    }
}

/// Format a single theme token value as a CSS value string. Shared by
/// the web backend (`setProperty` on `:root`) and the SSR backend
/// (`:root { … }` in the document head) so a token resolves identically
/// across both — single source of truth, like [`NAVIGATOR_LAYOUT_CSS`].
pub fn token_value_css(v: &runtime_core::TokenValue) -> String {
    use runtime_core::TokenValue;
    match v {
        TokenValue::Color(c) => c.0.clone(),
        TokenValue::Length(l) => length_css(*l),
        TokenValue::Number(n) => n.to_string(),
    }
}

/// Serialize a theme's tokens into a `:root { --name: value; … }` rule.
/// Empty string when there are no tokens (so the caller can skip an
/// empty `<style>`). The SSR backend emits this in `<head>` so the
/// server's first paint resolves `var(--token, fallback)` to the real
/// theme value — matching the live web build, which installs the same
/// variables at runtime via `install_tokens`.
pub fn tokens_to_root_css(tokens: &[runtime_core::TokenEntry]) -> String {
    if tokens.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(tokens.len() * 32 + 16);
    out.push_str(":root{");
    for entry in tokens {
        out.push_str("--");
        out.push_str(entry.name);
        out.push(':');
        out.push_str(&token_value_css(&entry.value));
        out.push(';');
    }
    out.push('}');
    out
}

// ---------------------------------------------------------------------------
// Assets + @font-face — single source of truth shared by web + SSR
// ---------------------------------------------------------------------------

/// Path prefix under which non-font `Bundled` assets are served — the
/// CLI build stages each declared asset at `{ASSET_ROUTE}/{path}`. Fonts
/// are the exception: they're served root-absolute (`/{path}`) so
/// `@font-face { src: url(...) }` resolves regardless of the SPA route.
pub const ASSET_ROUTE: &str = "assets";

/// Resolve the served-file URL for an asset, shared by the web backend
/// (`@font-face`/`<img>` links) and the SSR backend. Returns `None` for
/// `Embedded` sources — those need a runtime blob URL (web-only); a
/// headless server has no served path for them.
pub fn asset_url(
    kind: runtime_core::assets::AssetTag,
    source: &runtime_core::assets::AssetSource,
) -> Option<String> {
    use runtime_core::assets::{AssetSource, AssetTag};
    match source {
        // Fonts link root-absolute so the URL is stable under any SPA
        // route; other bundled assets live under the asset route.
        AssetSource::Bundled { path } | AssetSource::BundledEmbedded { path, .. }
            if kind == AssetTag::Font =>
        {
            Some(format!("/{path}"))
        }
        AssetSource::Bundled { path } | AssetSource::BundledEmbedded { path, .. } => {
            Some(format!("{ASSET_ROUTE}/{path}"))
        }
        AssetSource::Remote { url } => Some((*url).to_string()),
        AssetSource::Embedded { .. } => None,
    }
}

/// Format one `@font-face { … }` rule for a single weight/style face,
/// linking the served `url`. Used by both the web backend (injected at
/// `register_typeface`) and the SSR backend (emitted in `<head>`), so a
/// face resolves identically across the two.
pub fn font_face_css(
    family_name: &str,
    face: &runtime_core::assets::TypefaceFace,
    url: &str,
) -> String {
    let weight = font_weight_css(face.weight);
    let style = font_style_css(face.style);
    let format_hint = font_format_hint(&face.source);
    let mut s = String::with_capacity(family_name.len() + url.len() + 96);
    s.push_str("@font-face{font-family:\"");
    s.push_str(family_name);
    s.push_str("\";font-style:");
    s.push_str(style);
    s.push_str(";font-weight:");
    s.push_str(weight);
    s.push_str(";src:url(\"");
    s.push_str(url);
    s.push_str("\")");
    if let Some(format) = format_hint {
        s.push_str(" format(\"");
        s.push_str(format);
        s.push_str("\")");
    }
    s.push_str(";}");
    s
}

/// `@font-face` `format()` hint from an asset source's file extension.
pub fn font_format_hint(source: &runtime_core::assets::AssetSource) -> Option<&'static str> {
    use runtime_core::assets::AssetSource;
    let path = match source {
        AssetSource::Bundled { path } => *path,
        AssetSource::BundledEmbedded { path, .. } => *path,
        AssetSource::Remote { url } => *url,
        AssetSource::Embedded { extension, .. } => extension,
    };
    let ext = path.rsplit('.').next()?;
    Some(match ext.to_ascii_lowercase().as_str() {
        "ttf" => "truetype",
        "otf" => "opentype",
        "woff" => "woff",
        "woff2" => "woff2",
        "eot" => "embedded-opentype",
        "svg" => "svg",
        _ => return None,
    })
}

/// Render a `Length` as a CSS value string.
pub fn length_css(l: runtime_core::Length) -> String {
    use runtime_core::Length;
    match l {
        Length::Px(v) => format!("{}px", v),
        Length::Percent(v) => format!("{}%", v),
        Length::Auto => "auto".to_string(),
    }
}

pub fn tokenized_color_css(t: &runtime_core::Tokenized<runtime_core::Color>) -> String {
    use runtime_core::Tokenized;
    match t {
        Tokenized::Literal(c) => c.0.clone(),
        Tokenized::Token { name, fallback } => {
            format!("var(--{}, {})", name, fallback.0)
        }
    }
}

/// Render a `Gradient` as a CSS `linear-gradient(...)` / `radial-gradient(...)`
/// value suitable for the `background-image` property.
pub fn gradient_css(g: &runtime_core::Gradient) -> String {
    let stops: Vec<String> = g
        .stops
        .iter()
        .map(|s| format!("{} {:.2}%", s.color.0, s.offset * 100.0))
        .collect();
    let stops_joined = stops.join(", ");
    match g.kind {
        runtime_core::GradientKind::Linear { angle_deg } => {
            // CSS `linear-gradient(angle, stops)`: `0deg` is
            // bottom→top, matching the framework's convention.
            format!("linear-gradient({}deg, {})", angle_deg, stops_joined)
        }
        runtime_core::GradientKind::Radial { center, radius, extent } => {
            // CSS doesn't allow percentage sizing with the `circle`
            // keyword, so we use the `ellipse` form with two
            // percentages (relative to box width/height).
            // - ClosestSide: `radius * 50%` → inscribed ellipse.
            // - FarthestCorner: `radius * 70.71%` → corner-passing ellipse.
            let base_pct = match extent {
                runtime_core::RadialExtent::ClosestSide => 50.0,
                runtime_core::RadialExtent::FarthestCorner => 70.7106781,
            };
            let pct = (radius * base_pct).max(0.0);
            format!(
                "radial-gradient(ellipse {pct}% {pct}% at {x}% {y}%, {stops})",
                pct = pct,
                x = center.0 * 100.0,
                y = center.1 * 100.0,
                stops = stops_joined,
            )
        }
    }
}

/// Render a tokenized length: literal as `{n}px` / `{n}%` / `auto`,
/// token as `var(--name, fallback)`.
pub fn tokenized_length_css(t: &runtime_core::Tokenized<runtime_core::Length>) -> String {
    use runtime_core::Tokenized;
    match t {
        Tokenized::Literal(l) => length_css(*l),
        Tokenized::Token { name, fallback } => {
            format!("var(--{}, {})", name, length_css(*fallback))
        }
    }
}

/// Render a tokenized raw number (used for `opacity`, `flex_grow`).
pub fn tokenized_f32_css(t: &runtime_core::Tokenized<f32>) -> String {
    use runtime_core::Tokenized;
    match t {
        Tokenized::Literal(v) => v.to_string(),
        Tokenized::Token { name, fallback } => {
            format!("var(--{}, {})", name, fallback)
        }
    }
}

/// Render a tokenized number with the `px` suffix (border widths,
/// line-height, letter-spacing). Token form uses `calc(... * 1px)` so
/// the unit applies regardless of how the variable resolves.
pub fn tokenized_border_width_css(t: &runtime_core::Tokenized<f32>) -> String {
    use runtime_core::Tokenized;
    match t {
        Tokenized::Literal(v) => format!("{}px", v),
        Tokenized::Token { name, fallback } => {
            format!("calc(var(--{}, {}) * 1px)", name, fallback)
        }
    }
}

/// Same shape as `tokenized_border_width_css` — kept as a separate
/// helper so semantic call sites read clearly.
pub fn tokenized_px_f32_css(t: &runtime_core::Tokenized<f32>) -> String {
    tokenized_border_width_css(t)
}

pub fn flex_direction_css(v: runtime_core::FlexDirection) -> &'static str {
    use runtime_core::FlexDirection;
    match v {
        FlexDirection::Row => "row",
        FlexDirection::Column => "column",
        FlexDirection::RowReverse => "row-reverse",
        FlexDirection::ColumnReverse => "column-reverse",
    }
}

pub fn flex_wrap_css(v: runtime_core::FlexWrap) -> &'static str {
    use runtime_core::FlexWrap;
    match v {
        FlexWrap::NoWrap => "nowrap",
        FlexWrap::Wrap => "wrap",
        FlexWrap::WrapReverse => "wrap-reverse",
    }
}

pub fn justify_content_css(v: runtime_core::JustifyContent) -> &'static str {
    use runtime_core::JustifyContent;
    match v {
        JustifyContent::FlexStart => "flex-start",
        JustifyContent::FlexEnd => "flex-end",
        JustifyContent::Center => "center",
        JustifyContent::SpaceBetween => "space-between",
        JustifyContent::SpaceAround => "space-around",
        JustifyContent::SpaceEvenly => "space-evenly",
    }
}

pub fn align_items_css(v: runtime_core::AlignItems) -> &'static str {
    use runtime_core::AlignItems;
    match v {
        AlignItems::FlexStart => "flex-start",
        AlignItems::FlexEnd => "flex-end",
        AlignItems::Center => "center",
        AlignItems::Stretch => "stretch",
        AlignItems::Baseline => "baseline",
    }
}

pub fn align_content_css(v: runtime_core::AlignContent) -> &'static str {
    use runtime_core::AlignContent;
    match v {
        AlignContent::FlexStart => "flex-start",
        AlignContent::FlexEnd => "flex-end",
        AlignContent::Center => "center",
        AlignContent::Stretch => "stretch",
        AlignContent::SpaceBetween => "space-between",
        AlignContent::SpaceAround => "space-around",
    }
}

pub fn align_self_css(v: runtime_core::AlignSelf) -> &'static str {
    use runtime_core::AlignSelf;
    match v {
        AlignSelf::Auto => "auto",
        AlignSelf::FlexStart => "flex-start",
        AlignSelf::FlexEnd => "flex-end",
        AlignSelf::Center => "center",
        AlignSelf::Stretch => "stretch",
        AlignSelf::Baseline => "baseline",
    }
}

pub fn position_css(v: runtime_core::Position) -> &'static str {
    use runtime_core::Position;
    match v {
        Position::Relative => "relative",
        Position::Absolute => "absolute",
        Position::Sticky => "sticky",
    }
}

pub fn font_weight_css(v: runtime_core::FontWeight) -> &'static str {
    use runtime_core::FontWeight;
    match v {
        FontWeight::Thin => "100",
        FontWeight::ExtraLight => "200",
        FontWeight::Light => "300",
        FontWeight::Normal => "400",
        FontWeight::Medium => "500",
        FontWeight::SemiBold => "600",
        FontWeight::Bold => "700",
        FontWeight::ExtraBold => "800",
        FontWeight::Black => "900",
    }
}

pub fn font_style_css(v: runtime_core::FontStyle) -> &'static str {
    use runtime_core::FontStyle;
    match v {
        FontStyle::Normal => "normal",
        FontStyle::Italic => "italic",
    }
}

pub fn text_align_css(v: runtime_core::TextAlign) -> &'static str {
    use runtime_core::TextAlign;
    match v {
        TextAlign::Left => "left",
        TextAlign::Right => "right",
        TextAlign::Center => "center",
        TextAlign::Justify => "justify",
    }
}

pub fn text_transform_css(v: runtime_core::TextTransform) -> &'static str {
    use runtime_core::TextTransform;
    match v {
        TextTransform::None => "none",
        TextTransform::Uppercase => "uppercase",
        TextTransform::Lowercase => "lowercase",
        TextTransform::Capitalize => "capitalize",
    }
}

pub fn overflow_css(v: runtime_core::Overflow) -> &'static str {
    use runtime_core::Overflow;
    match v {
        Overflow::Visible => "visible",
        Overflow::Hidden => "hidden",
    }
}

pub fn transform_css(t: &runtime_core::Transform) -> String {
    use runtime_core::Transform;
    match t {
        Transform::TranslateX(l) => format!("translateX({})", length_css(*l)),
        Transform::TranslateY(l) => format!("translateY({})", length_css(*l)),
        Transform::Scale(v) => format!("scale({})", v),
        Transform::ScaleXY { x, y } => format!("scale({}, {})", x, y),
        Transform::Rotate(v) => format!("rotate({}deg)", v),
        Transform::SkewX(v) => format!("skewX({}deg)", v),
        Transform::SkewY(v) => format!("skewY({}deg)", v),
    }
}

pub fn easing_css(e: runtime_core::Easing) -> String {
    use runtime_core::Easing;
    match e {
        Easing::Linear => "linear".to_string(),
        Easing::Ease => "ease".to_string(),
        Easing::EaseIn => "ease-in".to_string(),
        Easing::EaseOut => "ease-out".to_string(),
        Easing::EaseInOut => "ease-in-out".to_string(),
        Easing::CubicBezier(a, b, c, d) => {
            format!("cubic-bezier({}, {}, {}, {})", a, b, c, d)
        }
    }
}

/// Compile a `StyleRules` to a CSS declaration body (`;`-joined,
/// no surrounding braces). Suitable for a class body or an inline
/// `style="…"` attribute.
///
/// **Flex semantics** are auto-promoted: if the rules use any
/// flex-container property (`gap`, `flex_direction`, `align_items`,
/// `justify_content`, `align_content`, `flex_wrap`, `row_gap`,
/// `column_gap`), `display: flex` is prepended (and
/// `flex-direction: column` pinned when unset, matching the
/// framework's mobile-first default). Nodes that use no flex property
/// stay normal blocks — no flex-tracker cost for unstyled rows.
pub fn rules_to_css(rules: &StyleRules) -> String {
    let mut parts: Vec<String> = Vec::new();

    let uses_flex = rules.flex_direction.is_some()
        || rules.flex_wrap.is_some()
        || rules.justify_content.is_some()
        || rules.align_items.is_some()
        || rules.align_content.is_some()
        || rules.gap.is_some()
        || rules.row_gap.is_some()
        || rules.column_gap.is_some();
    if uses_flex {
        parts.push("display: flex".to_string());
        if rules.flex_direction.is_none() {
            parts.push("flex-direction: column".to_string());
        }
    }

    // Color + text.
    if let Some(t) = &rules.background { parts.push(format!("background: {}", tokenized_color_css(t))); }
    if let Some(g) = &rules.background_gradient {
        parts.push(format!("background-image: {}", gradient_css(g)));
    }
    if let Some(t) = &rules.color { parts.push(format!("color: {}", tokenized_color_css(t))); }
    if let Some(t) = &rules.caret_color { parts.push(format!("caret-color: {}", tokenized_color_css(t))); }
    if let Some(t) = &rules.font_size { parts.push(format!("font-size: {}", tokenized_length_css(t))); }

    // Flex container.
    if let Some(v) = rules.flex_direction { parts.push(format!("flex-direction: {}", flex_direction_css(v))); }
    if let Some(v) = rules.flex_wrap { parts.push(format!("flex-wrap: {}", flex_wrap_css(v))); }
    if let Some(v) = rules.justify_content { parts.push(format!("justify-content: {}", justify_content_css(v))); }
    if let Some(v) = rules.align_items { parts.push(format!("align-items: {}", align_items_css(v))); }
    if let Some(v) = rules.align_content { parts.push(format!("align-content: {}", align_content_css(v))); }
    if let Some(t) = &rules.gap { parts.push(format!("gap: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.row_gap { parts.push(format!("row-gap: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.column_gap { parts.push(format!("column-gap: {}", tokenized_length_css(t))); }

    // Flex item.
    if let Some(t) = &rules.flex_grow { parts.push(format!("flex-grow: {}", tokenized_f32_css(t))); }
    if let Some(t) = &rules.flex_shrink { parts.push(format!("flex-shrink: {}", tokenized_f32_css(t))); }
    if let Some(t) = &rules.flex_basis { parts.push(format!("flex-basis: {}", tokenized_length_css(t))); }
    if let Some(v) = rules.align_self { parts.push(format!("align-self: {}", align_self_css(v))); }

    // Sizing.
    if let Some(t) = &rules.width { parts.push(format!("width: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.height { parts.push(format!("height: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.min_width { parts.push(format!("min-width: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.min_height { parts.push(format!("min-height: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.max_width { parts.push(format!("max-width: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.max_height { parts.push(format!("max-height: {}", tokenized_length_css(t))); }
    if let Some(ar) = rules.aspect_ratio { parts.push(format!("aspect-ratio: {}", ar)); }

    // Per-side padding.
    if let Some(t) = &rules.padding_top { parts.push(format!("padding-top: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.padding_right { parts.push(format!("padding-right: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.padding_bottom { parts.push(format!("padding-bottom: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.padding_left { parts.push(format!("padding-left: {}", tokenized_length_css(t))); }

    // Per-side margin.
    if let Some(t) = &rules.margin_top { parts.push(format!("margin-top: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.margin_right { parts.push(format!("margin-right: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.margin_bottom { parts.push(format!("margin-bottom: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.margin_left { parts.push(format!("margin-left: {}", tokenized_length_css(t))); }

    // Per-corner border radius.
    if let Some(t) = &rules.border_top_left_radius { parts.push(format!("border-top-left-radius: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.border_top_right_radius { parts.push(format!("border-top-right-radius: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.border_bottom_left_radius { parts.push(format!("border-bottom-left-radius: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.border_bottom_right_radius { parts.push(format!("border-bottom-right-radius: {}", tokenized_length_css(t))); }

    // Per-side border width + color. Emit `solid` style so the browser
    // actually paints the line.
    if let Some(t) = &rules.border_top_width {
        parts.push(format!("border-top-width: {}", tokenized_border_width_css(t)));
        parts.push("border-top-style: solid".to_string());
    }
    if let Some(t) = &rules.border_right_width {
        parts.push(format!("border-right-width: {}", tokenized_border_width_css(t)));
        parts.push("border-right-style: solid".to_string());
    }
    if let Some(t) = &rules.border_bottom_width {
        parts.push(format!("border-bottom-width: {}", tokenized_border_width_css(t)));
        parts.push("border-bottom-style: solid".to_string());
    }
    if let Some(t) = &rules.border_left_width {
        parts.push(format!("border-left-width: {}", tokenized_border_width_css(t)));
        parts.push("border-left-style: solid".to_string());
    }
    if let Some(t) = &rules.border_top_color { parts.push(format!("border-top-color: {}", tokenized_color_css(t))); }
    if let Some(t) = &rules.border_right_color { parts.push(format!("border-right-color: {}", tokenized_color_css(t))); }
    if let Some(t) = &rules.border_bottom_color { parts.push(format!("border-bottom-color: {}", tokenized_color_css(t))); }
    if let Some(t) = &rules.border_left_color { parts.push(format!("border-left-color: {}", tokenized_color_css(t))); }

    // Position.
    if let Some(v) = rules.position { parts.push(format!("position: {}", position_css(v))); }
    if let Some(t) = &rules.top { parts.push(format!("top: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.right { parts.push(format!("right: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.bottom { parts.push(format!("bottom: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.left { parts.push(format!("left: {}", tokenized_length_css(t))); }

    // Typography. `Typeface` family-names are quoted so the CSS engine
    // never confuses them with generic keywords; `System` strings pass
    // through verbatim (they often carry a comma-separated stack).
    if let Some(ff) = &rules.font_family {
        match ff {
            runtime_core::FontFamily::System(name) => {
                parts.push(format!("font-family: {}", name));
            }
            runtime_core::FontFamily::Typeface(tf) => {
                parts.push(format!("font-family: \"{}\"", tf.family_name));
            }
        }
    }
    if let Some(v) = rules.font_weight { parts.push(format!("font-weight: {}", font_weight_css(v))); }
    if let Some(v) = rules.font_style { parts.push(format!("font-style: {}", font_style_css(v))); }
    if let Some(t) = &rules.line_height { parts.push(format!("line-height: {}", tokenized_px_f32_css(t))); }
    if let Some(t) = &rules.letter_spacing { parts.push(format!("letter-spacing: {}", tokenized_px_f32_css(t))); }
    if let Some(v) = rules.text_align { parts.push(format!("text-align: {}", text_align_css(v))); }
    // Underline + strikethrough are independent booleans; combine into
    // one `text-decoration-line` shorthand.
    let underline = rules.underline.unwrap_or(false);
    let strikethrough = rules.strikethrough.unwrap_or(false);
    if underline || strikethrough {
        let mut deco = String::new();
        if underline { deco.push_str("underline"); }
        if strikethrough {
            if !deco.is_empty() { deco.push(' '); }
            deco.push_str("line-through");
        }
        parts.push(format!("text-decoration-line: {}", deco));
    } else if rules.underline == Some(false) || rules.strikethrough == Some(false) {
        parts.push("text-decoration-line: none".to_string());
    }
    if let Some(v) = rules.text_transform { parts.push(format!("text-transform: {}", text_transform_css(v))); }

    // Visual.
    if let Some(t) = &rules.opacity { parts.push(format!("opacity: {}", tokenized_f32_css(t))); }
    if let Some(v) = rules.overflow { parts.push(format!("overflow: {}", overflow_css(v))); }
    if let Some(sh) = &rules.shadow {
        parts.push(format!(
            "box-shadow: {}px {}px {}px {}",
            sh.x, sh.y, sh.blur, sh.color.0
        ));
    }
    if let Some(xs) = &rules.transform {
        if !xs.is_empty() {
            let joined: Vec<String> = xs.iter().map(transform_css).collect();
            parts.push(format!("transform: {}", joined.join(" ")));
        }
    }
    if let Some((ox, oy)) = rules.transform_origin {
        parts.push(format!(
            "transform-origin: {} {}",
            length_css(ox),
            length_css(oy)
        ));
    }

    // Transitions: a single CSS `transition` listing every active
    // per-property transition. The browser interpolates on value change.
    let transitions = collect_transitions(rules);
    if !transitions.is_empty() {
        parts.push(format!("transition: {}", transitions.join(", ")));
    }

    parts.join("; ")
}

/// Walk every per-property transition field and produce CSS transition
/// entries (`"<prop> <duration>ms <easing>"`). Property names use CSS
/// hyphenation, not the Rust field names.
fn collect_transitions(rules: &StyleRules) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    macro_rules! tr {
        ($field:ident, $css_name:literal) => {
            if let Some(t) = rules.$field {
                out.push(format!(
                    "{} {}ms {}",
                    $css_name,
                    t.duration_ms,
                    easing_css(t.easing)
                ));
            }
        };
    }
    tr!(background_transition, "background");
    tr!(color_transition, "color");
    tr!(caret_color_transition, "caret-color");
    tr!(opacity_transition, "opacity");
    tr!(transform_transition, "transform");
    tr!(width_transition, "width");
    tr!(height_transition, "height");
    tr!(top_transition, "top");
    tr!(right_transition, "right");
    tr!(bottom_transition, "bottom");
    tr!(left_transition, "left");
    tr!(padding_top_transition, "padding-top");
    tr!(padding_right_transition, "padding-right");
    tr!(padding_bottom_transition, "padding-bottom");
    tr!(padding_left_transition, "padding-left");
    tr!(margin_top_transition, "margin-top");
    tr!(margin_right_transition, "margin-right");
    tr!(margin_bottom_transition, "margin-bottom");
    tr!(margin_left_transition, "margin-left");
    tr!(border_top_left_radius_transition, "border-top-left-radius");
    tr!(border_top_right_radius_transition, "border-top-right-radius");
    tr!(border_bottom_left_radius_transition, "border-bottom-left-radius");
    tr!(border_bottom_right_radius_transition, "border-bottom-right-radius");
    tr!(border_top_width_transition, "border-top-width");
    tr!(border_right_width_transition, "border-right-width");
    tr!(border_bottom_width_transition, "border-bottom-width");
    tr!(border_left_width_transition, "border-left-width");
    tr!(border_top_color_transition, "border-top-color");
    tr!(border_right_color_transition, "border-right-color");
    tr!(border_bottom_color_transition, "border-bottom-color");
    tr!(border_left_color_transition, "border-left-color");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_core::{Color, Length, TokenEntry, TokenValue};

    #[test]
    fn tokens_to_root_css_emits_root_block() {
        let tokens = vec![
            TokenEntry { name: "color-text", value: TokenValue::Color(Color("#1a1a1f".into())) },
            TokenEntry { name: "spacing-md", value: TokenValue::Length(Length::Px(16.0)) },
            TokenEntry { name: "opacity-soft", value: TokenValue::Number(0.5) },
        ];
        let css = tokens_to_root_css(&tokens);
        assert_eq!(
            css,
            ":root{--color-text:#1a1a1f;--spacing-md:16px;--opacity-soft:0.5;}"
        );
    }

    #[test]
    fn tokens_to_root_css_empty_is_blank() {
        assert_eq!(tokens_to_root_css(&[]), "");
    }

    #[test]
    fn asset_url_routes_by_kind() {
        use runtime_core::assets::{AssetSource, AssetTag};
        // Fonts link root-absolute; other bundled assets under the route.
        assert_eq!(
            asset_url(AssetTag::Font, &AssetSource::Bundled { path: "fonts/Inter-Regular.ttf" }),
            Some("/fonts/Inter-Regular.ttf".into())
        );
        assert_eq!(
            asset_url(AssetTag::Image, &AssetSource::Bundled { path: "images/logo.png" }),
            Some("assets/images/logo.png".into())
        );
        assert_eq!(
            asset_url(AssetTag::Image, &AssetSource::Remote { url: "https://cdn/x.png" }),
            Some("https://cdn/x.png".into())
        );
        // Embedded has no served URL on a headless server.
        assert_eq!(
            asset_url(AssetTag::Font, &AssetSource::Embedded { bytes: &[], extension: "ttf" }),
            None
        );
    }

    #[test]
    fn font_face_css_links_served_url() {
        use runtime_core::assets::{AssetId, AssetSource, TypefaceFace};
        use runtime_core::{FontStyle, FontWeight};
        let face = TypefaceFace {
            weight: FontWeight::Bold,
            style: FontStyle::Normal,
            asset: AssetId(1),
            source: AssetSource::Bundled { path: "fonts/Inter-Bold.ttf" },
        };
        assert_eq!(
            font_face_css("Inter", &face, "/fonts/Inter-Bold.ttf"),
            "@font-face{font-family:\"Inter\";font-style:normal;font-weight:700;\
             src:url(\"/fonts/Inter-Bold.ttf\") format(\"truetype\");}"
        );
    }
}
