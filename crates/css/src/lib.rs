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
/// default chrome and restores flex centering. Cursor is intentionally
/// NOT set here — it's an author/component style property
/// (`StyleRules::cursor`) now, so the framework imposes no default
/// pointer; component libraries opt their buttons into `Cursor::Pointer`.
pub const BUTTON_RESET: &str = ":where(button) { all: unset; box-sizing: border-box; \
    font: inherit; color: inherit; display: inline-flex; \
    align-items: center; justify-content: center; }";

/// Form-control font reset. The browser UA stylesheet gives `<textarea>`
/// a `font-family: monospace` default (and form controls in general don't
/// inherit the document font the way `<div>`/`<span>` do). Left alone, a
/// framework `<textarea>` renders in a monospace face while every other
/// piece of UI text uses the host's sans body font — so the idea-ui
/// Textarea came out monospace even though nothing in its stylesheet asked
/// for it. `:where(...)` is specificity 0, so the fiddle's code-editor
/// stylesheet (which explicitly pins `font-family: ui-monospace, …`) and
/// any other author class still win; this only supplies a sane default for
/// controls that don't set their own family. Author origin beats the UA
/// origin regardless of specificity, so this defeats the UA monospace rule.
///
/// Also resets the UA **focus outline**: framework primitives are unstyled
/// leaves and components own their focus indication (idea-ui's Field draws its
/// own border-color focus ring), so the browser's default focus outline is at
/// best a redundant double-ring and at worst unwanted chrome on a bare,
/// transparently-styled input (e.g. an in-canvas text editor). `:where(...)`
/// is specificity 0, so a component that *does* want a UA outline can still
/// opt back in via any author class; nothing in the framework currently does.
pub const FORM_FONT_RESET: &str =
    ":where(input, textarea) { font-family: inherit; outline: none; }";

/// `<img>` object-fit default. The browser's UA default is `fill` (stretch),
/// but the framework's cross-backend default for [`ObjectFit`] is `Contain`
/// (aspect-fit) — the native backends letterbox, so web must not stretch or
/// the same author code looks different per platform. `:where(img)` is
/// specificity 0, so a minted author class that sets `object-fit` (e.g.
/// `ObjectFit::Cover`, specificity 0,1,0) always wins; this only supplies the
/// default for images whose sheet leaves `object_fit` unset.
///
/// [`ObjectFit`]: runtime_core::ObjectFit
pub const IMG_FIT_RESET: &str = ":where(img) { object-fit: contain; }";

/// The full base reset stylesheet ([`BOX_SIZING_RESET`] + [`BUTTON_RESET`]
/// + [`FORM_FONT_RESET`] + [`IMG_FIT_RESET`]). The SSR backend emits this
/// once in `<head>`; the web backend inserts the rules at low sheet indices.
///
/// Host-surface theming (body background, scrollbar) is **not** part of
/// the reset — it's owned by the theme SDK and routed through
/// `Backend::set_app_background` / `Backend::set_scrollbar_theme`, which
/// each backend applies however native (DOM rules on web/SSR, UIWindow
/// background on iOS, etc.). Keeping the reset theme-agnostic means a
/// vanilla framework user with no theme SDK still gets a sensible
/// `box-sizing` + `<button>` baseline without inheriting opinions about
/// color tokens that may not exist.
pub fn base_reset_css() -> String {
    format!("{BOX_SIZING_RESET}{BUTTON_RESET}{FORM_FONT_RESET}{IMG_FIT_RESET}")
}

/// Default inline style for a `Link` primitive's `<a>`: strip the
/// browser's blue/underlined anchor defaults so the wrapping content's
/// styling shows through (authors override via their own style).
pub const LINK_RESET_STYLE: &str = "color: inherit; text-decoration: none; display: inline-flex;";

/// Default inline style for a `Button`'s content box (icon + label row).
pub const BUTTON_CONTENT_STYLE: &str = "display:inline-flex;align-items:center;gap:0.4em;";

/// Default inline style for an `Icon`'s inline element.
pub const ICON_INLINE_STYLE: &str = "display:inline-block;vertical-align:middle;";

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

/// Stable single-letter tag for an interaction-state bit, used only as a
/// disambiguator inside [`variant_class_key`] (NOT a CSS selector — that's
/// [`state_pseudo`]). Kept separate from the pseudo so the cache key stays
/// compact and never changes if the CSS pseudo spelling does.
fn state_key_tag(state: runtime_core::StateBits) -> &'static str {
    use runtime_core::StateBits;
    match state {
        StateBits::HOVERED => "h",
        StateBits::PRESSED => "p",
        StateBits::FOCUSED => "f",
        StateBits::DISABLED => "d",
        _ => "?",
    }
}

/// Canonical combined class/cache key for a styled element carrying
/// interaction-state and/or breakpoint overlays. **SINGLE SOURCE OF
/// TRUTH** — every CSS backend (web + SSR) MUST build the key through
/// this, so the same `(base, states, breakpoints)` mints the IDENTICAL
/// `ui-<hash>` class on both. Without it the server-rendered class and
/// the client-computed class diverge for any stateful/responsive style
/// (e.g. a hover button), and SSR→web hydration can't reuse the server's
/// styling — the adopted node's class gets swapped, re-painting it.
///
/// `base_key` is the caller's already-computed `base.content_key()`
/// (the web backend computes it once for its fast-path caches, so it's
/// passed in rather than recomputed here). Overlays are appended in a
/// fixed order: every state overlay (`;<tag>:<overlay-key>`), then every
/// breakpoint overlay (`;@<axis>:<overlay-key>`). Callers pass overlays
/// in the walker's stable order, so the key is deterministic.
pub fn variant_class_key(
    base_key: &str,
    overlays: &[(runtime_core::StateBits, std::rc::Rc<StyleRules>)],
    breakpoint_overlays: &[(runtime_core::Breakpoint, std::rc::Rc<StyleRules>)],
    container_overlays: &[(f32, std::rc::Rc<StyleRules>)],
) -> String {
    let mut key = String::with_capacity(base_key.len() + 64);
    key.push_str(base_key);
    for (bit, overlay) in overlays {
        key.push(';');
        key.push_str(state_key_tag(*bit));
        key.push(':');
        key.push_str(&overlay.content_key());
    }
    for (bp, overlay) in breakpoint_overlays {
        key.push(';');
        key.push('@');
        key.push_str(bp.axis_name().unwrap_or("__bp_xs"));
        key.push(':');
        key.push_str(&overlay.content_key());
    }
    // Container overlays carry their px threshold in the key (via the
    // `__cq_minw_<bits>` axis name) so two sheets that differ only in a
    // `container (min_width: …)` block mint distinct classes.
    for (threshold, overlay) in container_overlays {
        key.push(';');
        key.push('@');
        key.push_str(&runtime_core::container_axis_name(*threshold));
        key.push(':');
        key.push_str(&overlay.content_key());
    }
    key
}

/// Whether a text node's style must mint a text-shadow class variant.
/// A `shadow` renders as `text-shadow` on text (see [`rules_to_css_text`])
/// but `box-shadow` on box elements, so a text node carrying a shadow can't
/// safely share a box element's content-keyed class. Callers that already
/// know the node is text gate on this so the ONLY divergence is the rare
/// shadowed-text case — an unshadowed text node keeps sharing view classes.
pub fn text_needs_shadow_variant(rules: &StyleRules) -> bool {
    rules.shadow.is_some()
}

/// Disambiguate a text node's class key from the box element that shares its
/// `StyleRules`, so the `text-shadow` variant mints a distinct class. Applied
/// identically on web and SSR (over the full [`variant_class_key`] output)
/// so the minted class name matches for SSR→web hydration. Only used when
/// [`text_needs_shadow_variant`] holds.
pub fn text_shadow_class_key(base_or_variant_key: &str) -> String {
    // `@t` mirrors the `@`-prefixed axis markers `variant_class_key` already
    // appends, keeping the key grammar uniform.
    format!("{base_or_variant_key};@t")
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

/// The `@media (min-width: …)` prelude for a breakpoint overlay, using
/// the app's active [`runtime_core::breakpoints`] threshold table.
/// `None` for `Breakpoint::Xs` (the mobile-first base, which has no
/// media query) and for any breakpoint with no installed threshold.
///
/// Reads the *installed* table so a custom `install_breakpoints(...)`
/// shifts the emitted query to match — and so the web `@media`
/// boundary lands at exactly the same width the native classifier uses
/// for the same bucket. Single source of truth shared by the web
/// backend (`apply_styled_variants`) and SSR.
pub fn breakpoint_media_query(bp: runtime_core::Breakpoint) -> Option<String> {
    let px = runtime_core::breakpoints().min_width(bp)?;
    Some(format!("@media (min-width: {})", px_value(px)))
}

/// A full breakpoint-overlay rule: the `@media (min-width: …)` query
/// wrapping `.<class_name> { <body> }`. `None` for `Breakpoint::Xs`
/// (no media query — its rules are the base class itself). `body` is
/// the overlay's [`rules_to_css`] output.
///
/// Single source of truth shared by the web backend (which inserts this
/// into the live stylesheet) and SSR (which emits it into `<head>`), so
/// a `breakpoint md { … }` overlay produces a byte-identical rule on
/// both — the SSR first paint already carries the responsive layout the
/// hydrated web build would, no JS round trip needed.
pub fn breakpoint_media_rule(class_name: &str, bp: runtime_core::Breakpoint, body: &str) -> Option<String> {
    let query = breakpoint_media_query(bp)?;
    Some(format!("{query} {{ .{class_name} {{ {body} }} }}"))
}

/// Shared class name that marks a node as a container-query containment
/// context (`container-type: inline-size`). Used by both the web backend
/// (live stylesheet) and SSR (`<head>`) so the rule is byte-identical and
/// hydration reuses the server's class. Set by `Backend::mark_container`
/// in response to the `.container()` modifier.
pub const CONTAINER_TYPE_CLASS: &str = "ui-cq-container";

/// The CSS body for [`CONTAINER_TYPE_CLASS`]. `inline-size` containment
/// is the only mode v1 supports — descendants may query the container's
/// width only, which is what makes the query non-cyclic.
pub const CONTAINER_TYPE_BODY: &str = "container-type: inline-size";

/// A full container-query overlay rule:
/// `@container (min-width: <threshold>px) { .<class_name> { <body> } }`.
/// The browser resolves it against the nearest ancestor carrying
/// `container-type: inline-size` (set by [`runtime_core`]'s `.container()`
/// modifier via `Backend::mark_container`), so the overlay activates on
/// the *container's* width, not the viewport's. `body` is the overlay's
/// [`rules_to_css`] output.
///
/// Single source of truth shared by the web backend (live stylesheet
/// insert) and SSR (`<head>` emit), so a `container (min_width: N) { … }`
/// block produces a byte-identical rule on both — the SSR first paint
/// already carries the container-responsive layout.
pub fn container_query_rule(class_name: &str, threshold_px: f32, body: &str) -> String {
    format!(
        "@container (min-width: {}) {{ .{class_name} {{ {body} }} }}",
        px_value(threshold_px)
    )
}

/// Format a `min-width` threshold (always carried as `f32` dp) as a CSS
/// `px` length, trimming a redundant `.0` so `768.0` renders as `768px`
/// — both for byte-stable SSR/web class dedup and for readable output.
fn px_value(v: f32) -> String {
    format!("{}px", css_num(v))
}

/// Fixed-precision CSS number formatter: at most 3 decimals, trailing
/// zeros (and a bare `.0`) trimmed, so `768.0` → `768`, `1.5` → `1.5`,
/// `0.6666667` → `0.667`.
///
/// Exists because `f32: Display` drags core's shortest-representation
/// float machinery (`flt2dec` dragon + grisu, ~12–15 KB of wasm) into
/// every web bundle. CSS never needs more than millipixel/millidegree
/// precision, so this formats through pure integer math and the cheap
/// integer `Display` path instead. Every CSS-value float in this crate
/// must go through it — a single `{}` on an `f32` reinstates the whole
/// flt2dec stack.
///
/// Deterministic, which keeps minted-class content keys byte-stable
/// across web and SSR (both route through this crate). Non-finite
/// values render as `0`.
pub fn css_num(v: f32) -> CssNum {
    CssNum(v)
}

/// See [`css_num`].
pub struct CssNum(pub f32);

impl core::fmt::Display for CssNum {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let v = self.0;
        if !v.is_finite() {
            return f.write_str("0");
        }
        // Scale to thousandths in f64 (every f32 is exact in f64) and
        // round half-up. `as u64` saturates, so absurd magnitudes clamp
        // instead of wrapping.
        let scaled = (v.abs() as f64 * 1000.0 + 0.5) as u64;
        let int = scaled / 1000;
        let frac = (scaled % 1000) as u32;
        if v.is_sign_negative() && (int != 0 || frac != 0) {
            f.write_str("-")?;
        }
        core::fmt::Display::fmt(&int, f)?;
        if frac != 0 {
            let (width, frac) = if frac % 100 == 0 {
                (1, frac / 100)
            } else if frac % 10 == 0 {
                (2, frac / 10)
            } else {
                (3, frac)
            };
            write!(f, ".{frac:0width$}")?;
        }
        Ok(())
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
        TokenValue::Number(n) => css_num(*n).to_string(),
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
    // No `font-display` declared — uses the browser default (`auto`,
    // ~`block` with a 3s timeout). When the framework's runtime
    // `register_typeface` injects this rule after wasm boot, the page
    // text is already painted in the fallback; the browser then fetches
    // the font and the swap-in looks like a smooth re-flow rather than
    // the abrupt flip `font-display: swap` produces. Pre-SSR behavior.
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
        Length::Px(v) => format!("{}px", css_num(v)),
        Length::Percent(v) => format!("{}%", css_num(v)),
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

/// CSS declaration body for one styled-text run's deltas
/// (`runtime_core::TextRunStyle`) — the inline `style` attribute the
/// web + SSR backends stamp on a run's `<span>`. Shared here so both
/// emit byte-identical declarations (the same reason `rules_to_css`
/// lives in this crate). Tokenized values emit as `var(--token,
/// fallback)`, so run colors ride the CSS cascade on theme swaps —
/// no per-node re-realization needed on web (see
/// `Backend::update_styled_text`).
pub fn text_run_style_css(style: &runtime_core::TextRunStyle) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(ff) = &style.font_family {
        match ff {
            runtime_core::FontFamily::System(name) => {
                parts.push(format!("font-family: {}", name));
            }
            runtime_core::FontFamily::Typeface(tf) => {
                parts.push(format!("font-family: \"{}\"", tf.family_name));
            }
        }
    }
    if let Some(w) = style.font_weight {
        parts.push(format!("font-weight: {}", font_weight_css(w)));
    }
    if let Some(s) = &style.font_size {
        parts.push(format!("font-size: {}", tokenized_length_css(s)));
    }
    if let Some(c) = &style.color {
        parts.push(format!("color: {}", tokenized_color_css(c)));
    }
    if let Some(b) = &style.background {
        parts.push(format!("background-color: {}", tokenized_color_css(b)));
    }
    parts.join("; ")
}

/// Render a `Gradient` as a CSS `linear-gradient(...)` / `radial-gradient(...)`
/// value suitable for the `background-image` property.
pub fn gradient_css(g: &runtime_core::Gradient) -> String {
    let stops: Vec<String> = g
        .stops
        .iter()
        .map(|s| format!("{} {}%", s.color.0, css_num(s.offset * 100.0)))
        .collect();
    let stops_joined = stops.join(", ");
    match g.kind {
        runtime_core::GradientKind::Linear { angle_deg } => {
            // CSS `linear-gradient(angle, stops)`: `0deg` is
            // bottom→top, matching the framework's convention.
            format!("linear-gradient({}deg, {})", css_num(angle_deg), stops_joined)
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
                pct = css_num(pct),
                x = css_num(center.0 * 100.0),
                y = css_num(center.1 * 100.0),
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
        Tokenized::Literal(v) => css_num(*v).to_string(),
        Tokenized::Token { name, fallback } => {
            format!("var(--{}, {})", name, css_num(*fallback))
        }
    }
}

/// Render a tokenized number with the `px` suffix (border widths,
/// line-height, letter-spacing). Token form uses `calc(... * 1px)` so
/// the unit applies regardless of how the variable resolves.
pub fn tokenized_border_width_css(t: &runtime_core::Tokenized<f32>) -> String {
    use runtime_core::Tokenized;
    match t {
        Tokenized::Literal(v) => format!("{}px", css_num(*v)),
        Tokenized::Token { name, fallback } => {
            format!("calc(var(--{}, {}) * 1px)", name, css_num(*fallback))
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

/// Lower a `grid-template-columns` track list to its CSS value
/// (space-separated tracks, e.g. `1fr 1fr 1fr`).
pub fn track_list_css(tracks: &[runtime_core::TrackSize]) -> String {
    tracks
        .iter()
        .map(track_size_css)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Lower a single [`runtime_core::TrackSize`] to a CSS track sizing
/// function. `Fr(1.0)` → `1fr`, `Minmax(a, b)` → `minmax(a, b)`.
pub fn track_size_css(t: &runtime_core::TrackSize) -> String {
    use runtime_core::TrackSize;
    match t {
        TrackSize::Auto => "auto".to_string(),
        TrackSize::MinContent => "min-content".to_string(),
        TrackSize::MaxContent => "max-content".to_string(),
        TrackSize::Fr(v) => format!("{}fr", css_num(*v)),
        TrackSize::Px(v) => format!("{}px", css_num(*v)),
        TrackSize::Minmax(lo, hi) => {
            format!("minmax({}, {})", track_size_css(lo), track_size_css(hi))
        }
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

/// CSS `object-fit` keyword for a [`runtime_core::ObjectFit`]. Meaningful
/// only on `<img>` (replaced content); harmless on other elements.
pub fn object_fit_css(v: runtime_core::ObjectFit) -> &'static str {
    use runtime_core::ObjectFit;
    match v {
        ObjectFit::Fill => "fill",
        ObjectFit::Contain => "contain",
        ObjectFit::Cover => "cover",
    }
}

/// CSS `cursor` keyword for a [`runtime_core::Cursor`].
pub fn cursor_css(v: runtime_core::Cursor) -> &'static str {
    use runtime_core::Cursor;
    match v {
        Cursor::Auto => "auto",
        Cursor::Default => "default",
        Cursor::Pointer => "pointer",
        Cursor::Text => "text",
        Cursor::Wait => "wait",
        Cursor::Progress => "progress",
        Cursor::Help => "help",
        Cursor::NotAllowed => "not-allowed",
        Cursor::Move => "move",
        Cursor::Grab => "grab",
        Cursor::Grabbing => "grabbing",
        Cursor::Crosshair => "crosshair",
        Cursor::ColResize => "col-resize",
        Cursor::RowResize => "row-resize",
        Cursor::EwResize => "ew-resize",
        Cursor::NsResize => "ns-resize",
    }
}

/// CSS `user-select` keyword for a [`runtime_core::UserSelect`].
pub fn user_select_css(v: runtime_core::UserSelect) -> &'static str {
    use runtime_core::UserSelect;
    match v {
        UserSelect::Auto => "auto",
        UserSelect::None => "none",
        UserSelect::Text => "text",
        UserSelect::All => "all",
    }
}

/// CSS `pointer-events` keyword for a [`runtime_core::PointerEvents`].
pub fn pointer_events_css(v: runtime_core::PointerEvents) -> &'static str {
    use runtime_core::PointerEvents;
    match v {
        PointerEvents::Auto => "auto",
        PointerEvents::None => "none",
    }
}

pub fn transform_css(t: &runtime_core::Transform) -> String {
    use runtime_core::Transform;
    match t {
        Transform::TranslateX(l) => format!("translateX({})", length_css(*l)),
        Transform::TranslateY(l) => format!("translateY({})", length_css(*l)),
        Transform::Scale(v) => format!("scale({})", css_num(*v)),
        Transform::ScaleXY { x, y } => format!("scale({}, {})", css_num(*x), css_num(*y)),
        Transform::Rotate(v) => format!("rotate({}deg)", css_num(*v)),
        Transform::SkewX(v) => format!("skewX({}deg)", css_num(*v)),
        Transform::SkewY(v) => format!("skewY({}deg)", css_num(*v)),
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
            format!(
                "cubic-bezier({}, {}, {}, {})",
                css_num(a),
                css_num(b),
                css_num(c),
                css_num(d)
            )
        }
    }
}

/// How the `shadow` style field lowers to CSS. A box-shaped element
/// (view/image/pressable) renders it as `box-shadow`; the text
/// primitive renders it as `text-shadow` so the shadow follows the
/// glyph outlines rather than the inline box. Both CSS properties take
/// the identical `<x> <y> <blur> <color>` syntax (neither uses spread),
/// so `Shadow { x, y, blur, color }` maps onto each with no loss.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShadowKind {
    /// `box-shadow` — the default for box elements.
    Box,
    /// `text-shadow` — for the text primitive.
    Text,
}

/// Compile a `StyleRules` to a CSS declaration body (`;`-joined,
/// no surrounding braces). Suitable for a class body or an inline
/// `style="…"` attribute.
///
/// **Display** — an explicit `display` always wins. `Some(Grid)` emits
/// `display: grid` and lowers `grid_template_columns` to
/// `grid-template-columns`; `Some(Flex)` emits `display: flex`. Only
/// when `display` is `None` does the **flex auto-promotion** kick in: if
/// the rules use any flex-container property (`gap`, `flex_direction`,
/// `align_items`, `justify_content`, `align_content`, `flex_wrap`,
/// `row_gap`, `column_gap`), `display: flex` is emitted. Either way
/// `flex-direction: column` is pinned when unset (the framework's
/// mobile-first default). Nodes with no `display` and no flex property
/// stay normal blocks — no flex-tracker cost for unstyled rows.
///
/// The `shadow` field lowers to `box-shadow`. For text nodes call
/// [`rules_to_css_text`], which lowers it to `text-shadow` instead.
pub fn rules_to_css(rules: &StyleRules) -> String {
    rules_to_css_with_shadow(rules, ShadowKind::Box)
}

/// Like [`rules_to_css`] but lowers the `shadow` field to `text-shadow`
/// so it hugs the glyphs — the correct rendering for the text primitive.
pub fn rules_to_css_text(rules: &StyleRules) -> String {
    rules_to_css_with_shadow(rules, ShadowKind::Text)
}

/// [`rules_to_css`] with an explicit [`ShadowKind`] for the `shadow`
/// field. All other declarations are identical regardless of kind.
pub fn rules_to_css_with_shadow(rules: &StyleRules, shadow_kind: ShadowKind) -> String {
    let mut parts: Vec<String> = Vec::new();

    let uses_flex = rules.flex_direction.is_some()
        || rules.flex_wrap.is_some()
        || rules.justify_content.is_some()
        || rules.align_items.is_some()
        || rules.align_content.is_some()
        || rules.gap.is_some()
        || rules.row_gap.is_some()
        || rules.column_gap.is_some();
    // Display. An explicit `display` always wins; only when it's unset does a
    // node that uses a flex-container property (gap, justify, …) get
    // auto-promoted to `display: flex`. Without this precedence, a `display:
    // grid` container that also sets `gap` (every `Grid`) would be forced to
    // `flex` and collapse to one column — the exact bug this guards against.
    match rules.display {
        Some(runtime_core::DisplayKind::Grid) => {
            parts.push("display: grid".to_string());
        }
        Some(runtime_core::DisplayKind::Flex) => {
            parts.push("display: flex".to_string());
            if rules.flex_direction.is_none() {
                parts.push("flex-direction: column".to_string());
            }
        }
        None if uses_flex => {
            parts.push("display: flex".to_string());
            if rules.flex_direction.is_none() {
                parts.push("flex-direction: column".to_string());
            }
        }
        None => {}
    }
    if let Some(cols) = &rules.grid_template_columns {
        parts.push(format!("grid-template-columns: {}", track_list_css(cols)));
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
    if let Some(ar) = rules.aspect_ratio { parts.push(format!("aspect-ratio: {}", css_num(ar))); }

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
    if let Some(v) = rules.object_fit { parts.push(format!("object-fit: {}", object_fit_css(v))); }
    if let Some(sh) = &rules.shadow {
        // `text-shadow` and `box-shadow` share the same
        // `<x> <y> <blur> <color>` value grammar (text-shadow simply has
        // no spread term, which our `Shadow` doesn't carry either), so
        // only the property name differs by node kind.
        let prop = match shadow_kind {
            ShadowKind::Box => "box-shadow",
            ShadowKind::Text => "text-shadow",
        };
        parts.push(format!(
            "{}: {}px {}px {}px {}",
            prop,
            css_num(sh.x),
            css_num(sh.y),
            css_num(sh.blur),
            sh.color.0
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

    // Interaction. `user-select` is emitted with the `-webkit-` prefix so
    // Safari (which still needs it) honors it; both share one keyword.
    if let Some(c) = rules.cursor {
        parts.push(format!("cursor: {}", cursor_css(c)));
    }
    if let Some(u) = rules.user_select {
        let v = user_select_css(u);
        parts.push(format!("-webkit-user-select: {v}"));
        parts.push(format!("user-select: {v}"));
    }
    if let Some(p) = rules.pointer_events {
        parts.push(format!("pointer-events: {}", pointer_events_css(p)));
    }

    // Transitions: a single CSS `transition` listing every active
    // per-property transition. The browser interpolates on value change.
    let transitions = collect_transitions(rules);
    if !transitions.is_empty() {
        parts.push(format!("transition: {}", transitions.join(", ")));
    }

    parts.join("; ")
}

// ---------------------------------------------------------------------------
// Token-resolved inline CSS — for backends with no CSS-variable / `<head>`
// stylesheet surface (email).
//
// `rules_to_css` emits `var(--name, fallback)` for every tokenized value,
// which relies on the page carrying a `:root { --name: … }` block AND the
// client supporting CSS custom properties. Email has NEITHER: many clients
// (older Outlook) drop a whole declaration that contains an unsupported
// `var()` rather than falling back to its embedded default, and Gmail strips
// `<style>` / `:root` blocks so the variables are never even defined. So the
// email backend needs the token's *resolved* value baked directly into an
// inline `style="…"`.
//
// `rules_to_css_resolved` produces exactly that: it resolves every
// `Tokenized::Token` against the installed theme tokens (falling back to the
// value embedded in the token reference) into a `Literal`, then runs the
// SAME `rules_to_css` — so property formatting is byte-identical to the
// literal path web/SSR already emit, only without the `var()` indirection.
// Isolating the resolve as a pre-pass keeps `rules_to_css` (the shared
// web/SSR hot path) untouched and byte-stable.
// ---------------------------------------------------------------------------

/// Compile a `StyleRules` to a CSS declaration body with every
/// `Tokenized::Token` resolved to a concrete value against `tokens` (the
/// theme installed via `Backend::install_tokens`), falling back to the token
/// reference's embedded fallback when a name isn't present. Unlike
/// [`rules_to_css`] this emits NO `var(--…)` — suitable for an inline
/// `style="…"` in email, where CSS variables and `<head>` `:root` blocks are
/// unavailable.
pub fn rules_to_css_resolved(rules: &StyleRules, tokens: &[runtime_core::TokenEntry]) -> String {
    rules_to_css(&resolve_style_tokens(rules, tokens))
}

/// Text-primitive counterpart of [`rules_to_css_resolved`]: bakes tokens
/// to literals and lowers `shadow` to `text-shadow`. Used by the
/// email/static renderers when emitting a text node's style.
pub fn rules_to_css_resolved_text(rules: &StyleRules, tokens: &[runtime_core::TokenEntry]) -> String {
    rules_to_css_text(&resolve_style_tokens(rules, tokens))
}

/// Clone `rules`, replacing every `Tokenized::Token` field with a
/// `Tokenized::Literal` holding the token's resolved value. The field lists
/// enumerate every tokenized field of `StyleRules` by value type; a new
/// tokenized field must be added to the matching list (guarded by
/// `resolve_style_tokens_bakes_every_token_type`). See [`rules_to_css_resolved`].
fn resolve_style_tokens(rules: &StyleRules, tokens: &[runtime_core::TokenEntry]) -> StyleRules {
    let mut r = rules.clone();
    macro_rules! resolve {
        ($resolver:ident; $($field:ident),* $(,)?) => {
            $( r.$field = r.$field.as_ref().map(|t| $resolver(t, tokens)); )*
        };
    }
    resolve!(resolve_color;
        background, color, caret_color,
        border_top_color, border_right_color, border_bottom_color, border_left_color);
    resolve!(resolve_length;
        font_size, gap, row_gap, column_gap, flex_basis,
        width, height, min_width, min_height, max_width, max_height,
        padding_top, padding_right, padding_bottom, padding_left,
        margin_top, margin_right, margin_bottom, margin_left,
        border_top_left_radius, border_top_right_radius,
        border_bottom_left_radius, border_bottom_right_radius,
        top, right, bottom, left);
    resolve!(resolve_f32;
        flex_grow, flex_shrink, opacity,
        border_top_width, border_right_width, border_bottom_width, border_left_width,
        line_height, letter_spacing);
    r
}

/// Look up a token's installed value by name.
fn token_lookup<'a>(
    tokens: &'a [runtime_core::TokenEntry],
    name: &str,
) -> Option<&'a runtime_core::TokenValue> {
    tokens.iter().find(|e| e.name == name).map(|e| &e.value)
}

fn resolve_color(
    t: &runtime_core::Tokenized<runtime_core::Color>,
    tokens: &[runtime_core::TokenEntry],
) -> runtime_core::Tokenized<runtime_core::Color> {
    use runtime_core::{TokenValue, Tokenized};
    match t {
        Tokenized::Literal(_) => t.clone(),
        Tokenized::Token { name, fallback } => Tokenized::Literal(
            match token_lookup(tokens, name) {
                Some(TokenValue::Color(c)) => c.clone(),
                _ => fallback.clone(),
            },
        ),
    }
}

fn resolve_length(
    t: &runtime_core::Tokenized<runtime_core::Length>,
    tokens: &[runtime_core::TokenEntry],
) -> runtime_core::Tokenized<runtime_core::Length> {
    use runtime_core::{TokenValue, Tokenized};
    match t {
        Tokenized::Literal(_) => t.clone(),
        Tokenized::Token { name, fallback } => Tokenized::Literal(
            match token_lookup(tokens, name) {
                Some(TokenValue::Length(l)) => *l,
                _ => *fallback,
            },
        ),
    }
}

fn resolve_f32(
    t: &runtime_core::Tokenized<f32>,
    tokens: &[runtime_core::TokenEntry],
) -> runtime_core::Tokenized<f32> {
    use runtime_core::{Length, TokenValue, Tokenized};
    match t {
        Tokenized::Literal(_) => t.clone(),
        Tokenized::Token { name, fallback } => Tokenized::Literal(match token_lookup(tokens, name) {
            Some(TokenValue::Number(n)) => *n,
            // A Length token consumed where an f32 is expected (border width,
            // line-height, letter-spacing) resolves through its px magnitude.
            Some(TokenValue::Length(Length::Px(px))) => *px,
            _ => *fallback,
        }),
    }
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
    tr!(max_width_transition, "max-width");
    tr!(max_height_transition, "max-height");
    tr!(min_width_transition, "min-width");
    tr!(min_height_transition, "min-height");
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

    /// `css_num` must agree with what `f32: Display` produced for the values
    /// CSS actually uses — it replaced Display everywhere in this crate to
    /// keep core's flt2dec float formatter out of web bundles, so any
    /// divergence here would change minted class content keys between
    /// releases (and SSR/web must stay byte-identical, which they do since
    /// both route through this crate).
    #[test]
    fn css_num_matches_display_for_common_values() {
        for (v, expect) in [
            (0.0_f32, "0"),
            (-0.0, "0"),
            (1.0, "1"),
            (768.0, "768"),
            (-16.0, "-16"),
            (1.5, "1.5"),
            (0.1, "0.1"),
            (0.25, "0.25"),
            (12.75, "12.75"),
            (0.125, "0.125"),
            (-0.5, "-0.5"),
            (100.0, "100"),
        ] {
            assert_eq!(css_num(v).to_string(), expect, "css_num({v})");
        }
    }

    /// Beyond 3 decimals `css_num` rounds (that's the point — CSS never
    /// needs shortest-representation precision), trims trailing zeros, and
    /// degrades non-finite input to `0` instead of emitting invalid CSS.
    #[test]
    fn css_num_rounds_trims_and_handles_edge_cases() {
        assert_eq!(css_num(2.0 / 3.0).to_string(), "0.667");
        assert_eq!(css_num(33.3333_f32).to_string(), "33.333");
        // (0.9995 itself stores as 0.99949997f32 and correctly stays "0.999")
        assert_eq!(css_num(0.99955).to_string(), "1");
        assert_eq!(css_num(1.100_f32).to_string(), "1.1");
        assert_eq!(css_num(1.120_f32).to_string(), "1.12");
        assert_eq!(css_num(-0.0004).to_string(), "0"); // rounds to zero → no "-"
        assert_eq!(css_num(f32::NAN).to_string(), "0");
        assert_eq!(css_num(f32::INFINITY).to_string(), "0");
        assert_eq!(css_num(f32::NEG_INFINITY).to_string(), "0");
    }

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

    // A `shadow` on a box element lowers to `box-shadow`; the SAME shadow on
    // the text primitive lowers to `text-shadow` so it hugs the glyphs. Both
    // share the `<x> <y> <blur> <color>` grammar, so only the property name
    // differs — the numbers and color are byte-identical.
    #[test]
    fn shadow_lowers_to_box_or_text_by_node_kind() {
        use runtime_core::{Color, Shadow, StyleRules};
        let rules = StyleRules {
            shadow: Some(Shadow { x: 1.0, y: 2.0, blur: 3.0, color: Color("#00000080".to_string()) }),
            ..Default::default()
        };
        let boxed = rules_to_css(&rules);
        let text = rules_to_css_text(&rules);
        assert!(boxed.contains("box-shadow: 1px 2px 3px #00000080"), "box: {boxed}");
        assert!(!boxed.contains("text-shadow"));
        assert!(text.contains("text-shadow: 1px 2px 3px #00000080"), "text: {text}");
        assert!(!text.contains("box-shadow"));
        // No `shadow` → neither property, regardless of kind.
        let empty = StyleRules::default();
        assert!(!rules_to_css(&empty).contains("shadow"));
        assert!(!rules_to_css_text(&empty).contains("shadow"));
    }

    // `object_fit` emits the matching CSS `object-fit` keyword. Cover is the
    // one that fixes the tile "fill + center-crop" pattern; only an explicit
    // value emits (an unset field defers to the `:where(img)` reset default).
    #[test]
    fn rules_to_css_emits_object_fit() {
        use runtime_core::ObjectFit;
        let cover = StyleRules { object_fit: Some(ObjectFit::Cover), ..Default::default() };
        assert!(rules_to_css(&cover).contains("object-fit: cover"));
        let fill = StyleRules { object_fit: Some(ObjectFit::Fill), ..Default::default() };
        assert!(rules_to_css(&fill).contains("object-fit: fill"));
        // Unset → nothing emitted (the base `:where(img)` reset supplies contain).
        assert!(!rules_to_css(&StyleRules::default()).contains("object-fit"));
    }

    // A `display: grid` container with `grid_template_columns` emits a real CSS
    // grid — `display: grid` + `grid-template-columns: 1fr 1fr 1fr` — and is NOT
    // auto-promoted to `display: flex` even though it also sets `gap`. Regression
    // for `Grid` collapsing to a single vertical column on web because the
    // emitter never read `display`/`grid_template_columns` and the flex heuristic
    // forced `display: flex; flex-direction: column`.
    #[test]
    fn rules_to_css_emits_grid_display_and_tracks_not_flex() {
        use runtime_core::{DisplayKind, Length, TrackSize, Tokenized};
        let css = rules_to_css(&StyleRules {
            display: Some(DisplayKind::Grid),
            grid_template_columns: Some(vec![TrackSize::Fr(1.0); 3]),
            gap: Some(Tokenized::Literal(Length::Px(24.0))),
            ..Default::default()
        });
        assert!(css.contains("display: grid"), "grid display: {css}");
        assert!(
            css.contains("grid-template-columns: 1fr 1fr 1fr"),
            "three 1fr tracks: {css}"
        );
        assert!(css.contains("gap: 24px"), "gap still emitted on grid: {css}");
        // The gap-driven flex heuristic must NOT fire — an explicit display wins.
        assert!(!css.contains("display: flex"), "no flex promotion: {css}");
        assert!(!css.contains("flex-direction"), "no flex-direction: {css}");
    }

    // An explicit `display: flex` still emits flex + the mobile-first
    // `flex-direction: column` default; and grid track lowering covers the
    // non-`Fr` sizing functions.
    #[test]
    fn rules_to_css_display_flex_and_track_forms() {
        use runtime_core::{DisplayKind, TrackSize};
        let flex = rules_to_css(&StyleRules {
            display: Some(DisplayKind::Flex),
            ..Default::default()
        });
        assert!(flex.contains("display: flex"), "{flex}");
        assert!(flex.contains("flex-direction: column"), "{flex}");

        assert_eq!(track_size_css(&TrackSize::Auto), "auto");
        assert_eq!(track_size_css(&TrackSize::MinContent), "min-content");
        assert_eq!(track_size_css(&TrackSize::MaxContent), "max-content");
        assert_eq!(track_size_css(&TrackSize::Px(120.0)), "120px");
        assert_eq!(
            track_size_css(&TrackSize::Minmax(
                Box::new(TrackSize::MinContent),
                Box::new(TrackSize::Fr(1.0)),
            )),
            "minmax(min-content, 1fr)"
        );
    }

    // The base reset pins `<img>` to `object-fit: contain` at specificity 0
    // (`:where(img)`), so the framework's cross-backend default matches the
    // native letterbox instead of the UA `fill` stretch. A minted author
    // class wins (0,1,0 > 0). Guards the web-stretch regression.
    #[test]
    fn base_reset_includes_img_object_fit_contain() {
        let reset = base_reset_css();
        assert!(reset.contains(":where(img)"));
        assert!(reset.contains("object-fit: contain"));
    }

    // `cursor` emits the CSS keyword; `user-select` emits BOTH the prefixed
    // `-webkit-` form (Safari still needs it) and the unprefixed form, sharing
    // one keyword. This is what makes "buttons use a pointer + their label
    // can't be drag-selected" real on web.
    #[test]
    fn rules_to_css_emits_cursor_and_user_select() {
        use runtime_core::{Cursor, StyleRules, UserSelect};
        let css = rules_to_css(&StyleRules {
            cursor: Some(Cursor::Pointer),
            user_select: Some(UserSelect::None),
            ..Default::default()
        });
        assert!(css.contains("cursor: pointer"), "got: {css}");
        assert!(css.contains("-webkit-user-select: none"), "got: {css}");
        assert!(css.contains("user-select: none"), "got: {css}");
    }

    // Regression (ToastHost click-through): a click-through overlay lowers to
    // `pointer-events: none` on its portal root, and its interactive children
    // opt back in with `Auto`. Both must emit the exact CSS keyword or the
    // empty toast strip keeps swallowing clicks (none) / the cards stay dead
    // (auto). An unset value emits nothing (framework imposes no default).
    #[test]
    fn rules_to_css_emits_pointer_events() {
        use runtime_core::{PointerEvents, StyleRules};
        let none = rules_to_css(&StyleRules {
            pointer_events: Some(PointerEvents::None),
            ..Default::default()
        });
        assert!(none.contains("pointer-events: none"), "got: {none}");

        let auto = rules_to_css(&StyleRules {
            pointer_events: Some(PointerEvents::Auto),
            ..Default::default()
        });
        assert!(auto.contains("pointer-events: auto"), "got: {auto}");

        let unset = rules_to_css(&StyleRules::default());
        assert!(!unset.contains("pointer-events"), "got: {unset}");
    }

    // The hyphenated CSS keywords must match the spec spelling (snake_case
    // enum → kebab-case CSS), or the browser silently ignores the declaration.
    #[test]
    fn cursor_css_uses_spec_keywords() {
        use runtime_core::Cursor;
        assert_eq!(cursor_css(Cursor::NotAllowed), "not-allowed");
        assert_eq!(cursor_css(Cursor::ColResize), "col-resize");
        assert_eq!(cursor_css(Cursor::Grabbing), "grabbing");
    }

    // An unset cursor/user_select emits nothing — the framework imposes no
    // default, so a bare styled node carries no cursor/selection declaration.
    #[test]
    fn rules_to_css_omits_unset_interaction_props() {
        use runtime_core::StyleRules;
        let css = rules_to_css(&StyleRules::default());
        assert!(!css.contains("cursor"), "got: {css}");
        assert!(!css.contains("user-select"), "got: {css}");
    }

    // Regression: a framework `<textarea>` rendered in the browser's UA
    // monospace face because nothing reset the form-control font. The base
    // reset (seeded on web at index 2, emitted by SSR in <head>) must carry
    // a specificity-0 `font-family: inherit` for input/textarea so they pick
    // up the host's sans body font instead. A tighter test isn't reachable
    // here — the monospace default lives in the browser UA stylesheet, which
    // no Rust-level test can exercise — so we assert the reset string the
    // backends actually inject.
    #[test]
    fn regression_textarea_does_not_default_to_monospace_font() {
        assert_eq!(
            FORM_FONT_RESET,
            ":where(input, textarea) { font-family: inherit; outline: none; }"
        );
        let reset = base_reset_css();
        assert!(
            reset.contains(FORM_FONT_RESET),
            "base reset must include the form-control font reset so textareas \
             inherit the body font rather than the UA monospace default; got: {reset}"
        );
    }

    #[test]
    fn breakpoint_media_query_uses_installed_thresholds() {
        use runtime_core::Breakpoint;
        // Xs is the mobile-first base — no media query.
        assert_eq!(breakpoint_media_query(Breakpoint::Xs), None);
        // The default tailwind-scale thresholds, rendered without a
        // redundant `.0` so the px value reads cleanly.
        assert_eq!(
            breakpoint_media_query(Breakpoint::Sm).as_deref(),
            Some("@media (min-width: 640px)")
        );
        assert_eq!(
            breakpoint_media_query(Breakpoint::Md).as_deref(),
            Some("@media (min-width: 768px)")
        );
        assert_eq!(
            breakpoint_media_query(Breakpoint::Lg).as_deref(),
            Some("@media (min-width: 1024px)")
        );
        assert_eq!(
            breakpoint_media_query(Breakpoint::Xl).as_deref(),
            Some("@media (min-width: 1280px)")
        );
    }

    /// REGRESSION: SSR and web must mint the IDENTICAL class for a
    /// stateful/responsive style. The bug was two independent key
    /// builders — web used `;<tag>:` for state overlays, SSR used
    /// `|<bits>:` — so the same hover button got different `ui-<hash>`
    /// classes server vs client and hydration couldn't reuse the
    /// server's styling. Both backends now route through
    /// `variant_class_key`; this pins its canonical shape so neither can
    /// drift back.
    #[test]
    fn variant_class_key_is_canonical_and_deterministic() {
        use runtime_core::{Breakpoint, StateBits, StyleRules};
        use std::rc::Rc;

        let base_key = "fg=T:color-text;fs=L:1234";
        let overlay = Rc::new(StyleRules::default());

        // State overlays use the shared `;<tag>:` form — NOT SSR's old
        // `|<bits>:` form. `;h:` for HOVERED specifically.
        let with_hover =
            variant_class_key(base_key, &[(StateBits::HOVERED, overlay.clone())], &[], &[]);
        assert!(
            with_hover.starts_with(base_key),
            "key must begin with the base content key, got {with_hover}"
        );
        assert!(
            with_hover.contains(";h:"),
            "HOVERED overlay must use the canonical `;h:` tag, got {with_hover}"
        );
        assert!(
            !with_hover.contains('|'),
            "must NOT use the old SSR `|<bits>:` form (the divergence bug), got {with_hover}"
        );

        // Deterministic: same inputs → same key (so the hash matches
        // across the SSR render and the web rebuild).
        let again =
            variant_class_key(base_key, &[(StateBits::HOVERED, overlay.clone())], &[], &[]);
        assert_eq!(with_hover, again);

        // Distinct state bits → distinct keys (so a base shared across
        // hover vs focus styling still gets distinct classes).
        let with_focus =
            variant_class_key(base_key, &[(StateBits::FOCUSED, overlay.clone())], &[], &[]);
        assert_ne!(with_hover, with_focus);

        // Breakpoint overlays append the `;@<axis>:` form.
        let with_bp = variant_class_key(base_key, &[], &[(Breakpoint::Md, overlay.clone())], &[]);
        assert!(
            with_bp.contains(";@"),
            "breakpoint overlay must use the `;@<axis>:` form, got {with_bp}"
        );
    }

    /// REGRESSION — Issue B of the "duplicated SSG nav" bug: SSR and web
    /// must mint the BYTE-IDENTICAL `ui-<hash>` class for a text node that
    /// carries a `shadow`, or the client's first render diverges from the
    /// (correct) SSR DOM at that span and the hydrator remounts the subtree.
    ///
    /// The two backends reach the class name by DIFFERENT expressions, and
    /// this test pins that they still agree:
    ///
    /// - SSR (`backend-ssr`'s `apply_style`, the no-overlay path) feeds the
    ///   content key STRAIGHT into `text_shadow_class_key`:
    ///     `hash_class_name(text_shadow_class_key(rules.content_key()))`
    /// - web (`backend-web`'s `apply_text_shadow`) routes through
    ///   `variant_class_key` first (it also handles state/breakpoint/
    ///   container overlays):
    ///     `hash_class_name(text_shadow_class_key(variant_class_key(k, &[], &[], &[])))`
    ///
    /// They match ONLY because `variant_class_key(k, &[], &[], &[]) == k`
    /// (no overlays ⇒ no appended segments). That identity is the linchpin;
    /// the first assertion locks it down so a future change to
    /// `variant_class_key` (e.g. unconditionally appending a marker) can't
    /// silently re-diverge SSR from web and reintroduce the double-nav.
    #[test]
    fn ssr_and_web_mint_identical_text_shadow_class() {
        use runtime_core::{Color, Shadow, StyleRules};

        // The linchpin identity both derivations rest on.
        let base_key = "fg=T:color-text;fs=L:1234";
        assert_eq!(
            variant_class_key(base_key, &[], &[], &[]),
            base_key,
            "variant_class_key with no overlays MUST be the identity on the base key — \
             web's text-shadow path depends on this to match SSR's content-key path",
        );

        // End-to-end over a real shadowed text `StyleRules`: compute the
        // class both backends' ways and assert they're byte-identical.
        let rules = StyleRules {
            shadow: Some(Shadow {
                x: 0.0,
                y: 2.0,
                blur: 4.0,
                // `Color` is a newtype over a CSS color string.
                color: Color("rgba(0,0,0,0.5)".to_string()),
            }),
            ..Default::default()
        };
        assert!(
            text_needs_shadow_variant(&rules),
            "a StyleRules carrying a shadow must route through the text-shadow variant",
        );

        let content_key = rules.content_key();
        let ssr_class = hash_class_name(&text_shadow_class_key(&content_key));
        let web_class = hash_class_name(&text_shadow_class_key(&variant_class_key(
            &content_key,
            &[],
            &[],
            &[],
        )));
        assert_eq!(
            ssr_class, web_class,
            "SSR and web must mint the same text-shadow class for identical StyleRules; \
             a mismatch diverges hydration at the shadowed span (Issue B / double-nav)",
        );

        // And the text-shadow key is genuinely DISTINCT from the plain
        // box-shadow class the same content key would mint on a view — so a
        // shadowed text node never adopts a box element's class (the reason
        // `text_shadow_class_key` exists at all).
        assert_ne!(
            hash_class_name(&text_shadow_class_key(&content_key)),
            hash_class_name(&content_key),
            "text-shadow class must not collide with the plain box-shadow class",
        );
    }

    #[test]
    fn breakpoint_media_rule_wraps_class_in_media_query() {
        use runtime_core::Breakpoint;
        // The overlay body is whatever `rules_to_css` produced; here we
        // pass a fixed body to pin the exact wrapping the web backend
        // inserts (and SSR emits) — single source of truth.
        let rule = breakpoint_media_rule("ui-abc123", Breakpoint::Md, "width: 500px")
            .expect("md is an overlay bucket");
        assert_eq!(rule, "@media (min-width: 768px) { .ui-abc123 { width: 500px } }");
        // Xs has no media query → no rule.
        assert_eq!(breakpoint_media_rule("ui-abc123", Breakpoint::Xs, "width: 100px"), None);
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

    // `rules_to_css` (web/SSR) emits `var(--name, fallback)` for a token,
    // relying on a `:root` block + CSS-variable support the client may not
    // have. `rules_to_css_resolved` must instead bake the INSTALLED token
    // value in as a literal — no `var(…)` anywhere — for email.
    #[test]
    fn rules_to_css_resolved_bakes_installed_token_values() {
        use runtime_core::Tokenized;
        let rules = StyleRules {
            background: Some(Tokenized::token("color-surface", Color("#ffffff".into()))),
            padding_top: Some(Tokenized::token("spacing-md", Length::Px(8.0))),
            ..Default::default()
        };
        let tokens = vec![
            TokenEntry { name: "color-surface", value: TokenValue::Color(Color("#101828".into())) },
            TokenEntry { name: "spacing-md", value: TokenValue::Length(Length::Px(16.0)) },
        ];

        // Baseline: the shared path emits var() (unusable in email).
        assert!(rules_to_css(&rules).contains("var(--color-surface"));

        // Resolved: the installed theme value is baked in; no var() at all.
        let out = rules_to_css_resolved(&rules, &tokens);
        assert!(out.contains("background: #101828"), "got: {out}");
        assert!(out.contains("padding-top: 16px"), "got: {out}");
        assert!(!out.contains("var("), "email CSS must carry no var(); got: {out}");
    }

    // When a token name isn't in the installed set, resolution falls back to
    // the value embedded in the token reference — never emits var().
    #[test]
    fn rules_to_css_resolved_falls_back_to_embedded_default() {
        use runtime_core::Tokenized;
        let rules = StyleRules {
            color: Some(Tokenized::token("color-text", Color("#333333".into()))),
            ..Default::default()
        };
        let out = rules_to_css_resolved(&rules, &[]); // nothing installed
        assert!(out.contains("color: #333333"), "got: {out}");
        assert!(!out.contains("var("), "got: {out}");
    }

    // A literal (non-token) style is unchanged by resolution — the resolved
    // path must equal the shared path when there are no tokens to bake.
    #[test]
    fn rules_to_css_resolved_matches_literal_path() {
        use runtime_core::Tokenized;
        let rules = StyleRules {
            background: Some(Tokenized::Literal(Color("#abcdef".into()))),
            width: Some(Tokenized::Literal(Length::Px(320.0))),
            ..Default::default()
        };
        assert_eq!(rules_to_css_resolved(&rules, &[]), rules_to_css(&rules));
    }
}
