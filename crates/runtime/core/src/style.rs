//! Style declarations and tokenization infrastructure.
//!
//! The framework owns the data model — what a "style" looks like, what
//! variant axes exist, and how named tokens propagate — but does **not**
//! own the rendering strategy or the "theme-as-struct" pattern. Each
//! backend interprets a `StyleRules` value however suits its platform:
//!
//! - **Web** can lazily mint CSS classes per unique rule set and swap
//!   `className` on the node when the style changes.
//! - **iOS** can update `CALayer` / `UIView` properties directly.
//! - **Android** can call `View` setters or apply theme attributes.
//!
//! # Tokens
//!
//! Stylesheets are **closures** from a `VariantSet` to concrete
//! `StyleRules`. Property values can be either literals or named
//! `Tokenized::Token { name, fallback }` references. Token values are
//! installed via [`install_tokens`] and updated via [`update_tokens`].
//!
//! The "theme as a typed struct" pattern is provided by `idea-ui`'s
//! theme runtime as a thin wrapper over these primitives.
//!
//! Token updates flow through the existing reactive system: each styled
//! node's apply-style call lives inside an `Effect` that reads token
//! values via `Tokenized::<T>::resolve()`. `resolve` subscribes the
//! active Effect to the per-token `Signal<TokenValue>` in the
//! registry, so an `update_tokens(["a"])` call only re-fires effects
//! that referenced `"a"` — token swaps are O(referencing nodes), not
//! O(styled nodes).
//!
//! # Identity for caching
//!
//! The framework memoizes resolution per `(stylesheet pointer, variants)`
//! and returns an `Rc<StyleRules>`. Backends cache their native form
//! keyed on the rule set's content (a hash or serialization), making
//! caching immune to allocator-reuse hazards.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use crate::assets::TypefaceId;

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

// ----------------------------------------------------------------------------
// Tokenized<T> — values that may resolve through a named theme token
// ----------------------------------------------------------------------------

/// A property value that is either a literal or a reference to a named
/// theme token. The `name` is theme-independent; the `fallback` is the
/// concrete value that should be used when no theme variable system is
/// available (mobile backends, SSR, etc.) or when the variable hasn't
/// been installed yet.
///
/// **Why this exists.** Backends that support runtime variables (web's
/// CSS custom properties) can emit `var(--name, fallback)` instead of
/// the literal value. Theme swap then becomes a single write per token
/// — no class regeneration, no per-element style mutation. Backends
/// without a variable system (iOS, Android) just read `.value()` and
/// behave like the literal was set.
///
/// **Identity for caching.** [`StyleRules::content_key`] hashes the
/// **token name** for `Tokenized::Token` (not the fallback). So two
/// themes that bind `color-accent` to different colors produce the
/// **same** content key for a stylesheet that uses `color-accent` —
/// which is what makes class names theme-stable.
#[derive(Clone, Debug, PartialEq)]
pub enum Tokenized<T> {
    Literal(T),
    Token { name: &'static str, fallback: T },
}

impl<T> Tokenized<T> {
    /// The concrete value to use when no variable system is available.
    /// For `Literal(v)` returns `v`; for `Token { fallback, .. }`
    /// returns the fallback.
    pub fn value(&self) -> &T {
        match self {
            Tokenized::Literal(v) => v,
            Tokenized::Token { fallback, .. } => fallback,
        }
    }

    /// The token name, if this is a token reference.
    pub fn name(&self) -> Option<&'static str> {
        match self {
            Tokenized::Token { name, .. } => Some(name),
            Tokenized::Literal(_) => None,
        }
    }

    /// Construct a token reference. Authors typically don't call this
    /// directly — themes expose `Tokenized<T>` fields built once at
    /// theme construction.
    pub const fn token(name: &'static str, fallback: T) -> Self {
        Tokenized::Token { name, fallback }
    }
}

impl<T: Copy> Copy for Tokenized<T> where T: Copy {}

// Per-token reactive resolution. Backends inside an `apply_style` Effect
// call `.resolve()` instead of `.value()` so the effect subscribes to
// the per-token signal in `TOKEN_REGISTRY` — only nodes that read a
// token re-fire on that token's update.
//
// One `resolve()` per `T` (Color / Length / f32) because each variant
// of `TokenValue` carries a different concrete type — there is no
// generic extraction helper that would work for all three.

impl Tokenized<Color> {
    /// Reactive read. For `Literal(v)` returns `v` (no subscription).
    /// For `Token { name, fallback }`, subscribes the active Effect to
    /// the per-token signal in the registry, extracts the `Color` value
    /// (or returns `fallback` if the registry has no entry / the
    /// installed value is the wrong variant).
    pub fn resolve(&self) -> Color {
        match self {
            Tokenized::Literal(v) => v.clone(),
            Tokenized::Token { name, fallback } => {
                debug_warn_resolve_on_unthemed_thread(name);
                with_or_create_token_signal(name, || TokenValue::Color(fallback.clone()))
                    .map(|sig| match sig.get() {
                        TokenValue::Color(c) => c,
                        other => {
                            debug_warn_token_type_mismatch(name, "Color", &other);
                            fallback.clone()
                        }
                    })
                    .unwrap_or_else(|| fallback.clone())
            }
        }
    }
}

impl Tokenized<Length> {
    /// Reactive read — see `Tokenized<Color>::resolve`.
    pub fn resolve(&self) -> Length {
        match self {
            Tokenized::Literal(v) => *v,
            Tokenized::Token { name, fallback } => {
                debug_warn_resolve_on_unthemed_thread(name);
                with_or_create_token_signal(name, || TokenValue::Length(*fallback))
                    .map(|sig| match sig.get() {
                        TokenValue::Length(l) => l,
                        other => {
                            debug_warn_token_type_mismatch(name, "Length", &other);
                            *fallback
                        }
                    })
                    .unwrap_or(*fallback)
            }
        }
    }
}

impl Tokenized<f32> {
    /// Reactive read — see `Tokenized<Color>::resolve`.
    pub fn resolve(&self) -> f32 {
        match self {
            Tokenized::Literal(v) => *v,
            Tokenized::Token { name, fallback } => {
                debug_warn_resolve_on_unthemed_thread(name);
                with_or_create_token_signal(name, || TokenValue::Number(*fallback))
                    .map(|sig| match sig.get() {
                        TokenValue::Number(n) => n,
                        other => {
                            debug_warn_token_type_mismatch(name, "Number", &other);
                            *fallback
                        }
                    })
                    .unwrap_or(*fallback)
            }
        }
    }
}

// `From<T> for Tokenized<T>` so the stylesheet macro's
// `Some(Into::into(expr))` accepts plain literal values.
impl<T> From<T> for Tokenized<T> {
    fn from(v: T) -> Self {
        Tokenized::Literal(v)
    }
}

// Allow `f32`/`i32` to flow into `Tokenized<Length>` so existing
// authoring patterns like `padding: 16` still work after the field
// type change. Two-step `From` chains aren't transitive in Rust, so
// we provide the bridges explicitly.
impl From<f32> for Tokenized<Length> {
    fn from(v: f32) -> Self {
        Tokenized::Literal(Length::Px(v))
    }
}
impl From<i32> for Tokenized<Length> {
    fn from(v: i32) -> Self {
        Tokenized::Literal(Length::Px(v as f32))
    }
}

// Border widths are `Tokenized<f32>` (not `Tokenized<Length>`) on
// purpose: a border can't be a percentage of anything, so the type
// excludes that invalid state. But authors reasonably reach for the
// same length spellings they use everywhere else (`Length::Px(2.0)`,
// or a `px(..)`-style helper). Bridge `Length` → `Tokenized<f32>` so
// `border_width: Length::Px(2.0)` type-checks; the px component is
// taken and `Percent`/`Auto` collapse to `0.0` (they're meaningless
// for a border) with a debug-only warning, rather than a confusing
// trait-mismatch error at the call site.
impl From<Length> for Tokenized<f32> {
    fn from(l: Length) -> Self {
        match l {
            Length::Px(v) => Tokenized::Literal(v),
            other => {
                #[cfg(debug_assertions)]
                eprintln!(
                    "[runtime-core] border width was given {:?}, but borders only \
                     support pixel widths (percent/auto don't apply) — using 0.0",
                    other
                );
                let _ = other;
                Tokenized::Literal(0.0)
            }
        }
    }
}

// `Color` from `&str`/`String` is already provided; bridge those into
// `Tokenized<Color>` so authors can keep writing `background: "#fff"`.
impl From<&str> for Tokenized<Color> {
    fn from(s: &str) -> Self {
        Tokenized::Literal(Color(s.to_string()))
    }
}
impl From<String> for Tokenized<Color> {
    fn from(s: String) -> Self {
        Tokenized::Literal(Color(s))
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

/// Which layout algorithm lays out this node's *children*.
///
/// The framework is flex-first: every node is `Flex` unless a style
/// explicitly opts into `Grid`. `Grid` exists for the narrow set of
/// primitives that need cross-row/column track alignment a single flex
/// container can't express — most notably the `table` SDK, whose native
/// lowering pins every column to one width across all rows the way a
/// browser's `<table>` does. Keep this minimal: it is a layout-engine
/// capability, not a general CSS-grid authoring surface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DisplayKind {
    /// Children follow the flexbox algorithm (the framework default).
    #[default]
    Flex,
    /// Children follow the CSS Grid algorithm. Pair with
    /// [`StyleRules::grid_template_columns`] to declare the column
    /// tracks; cells become grid items placed by row-major auto-flow.
    Grid,
}

/// One grid column (or row) track's sizing function. A subset of CSS
/// grid track sizing — only the forms tables actually need. `Minmax`
/// is the single nested form (e.g. `Minmax(MinContent, Fr(1.0))` =
/// "at least fit the content, then share leftover width to fill").
#[derive(Clone, Debug, PartialEq)]
pub enum TrackSize {
    /// Content-sized; in a definite-width grid, `Auto` tracks also
    /// absorb leftover space so the grid fills its container.
    Auto,
    /// Sized to the column's narrowest cell (`min-content`).
    MinContent,
    /// Sized to the column's widest cell (`max-content`).
    MaxContent,
    /// A fraction of the leftover space (CSS `fr` unit).
    Fr(f32),
    /// A fixed pixel width.
    Px(f32),
    /// `minmax(min, max)` — a floor track plus a (usually flexible)
    /// ceiling track. The only nested form; neither side may itself be
    /// `Minmax`.
    Minmax(Box<TrackSize>, Box<TrackSize>),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Position {
    #[default]
    Relative,
    Absolute,
    /// Acts like `Relative` until the element would scroll past one
    /// of the edges of its enclosing scroll container, at which
    /// point it pins to that edge. The pin threshold comes from
    /// the matching side field on [`StyleRules`] — typically `top`,
    /// less commonly `bottom` / `left` / `right`. With no side set,
    /// pins to the leading edge of the scroll container.
    ///
    /// **Per-backend coverage**:
    /// - **Web** — emits CSS `position: sticky`; the browser owns
    ///   the pinning. Full support.
    /// - **iOS** — walks up to the enclosing `UIScrollView` at
    ///   `apply_style` time, registers a per-vsync
    ///   `CADisplayLink` that applies a `CGAffineTransform`
    ///   translate to pin the view at `top` from the scroll
    ///   container's top edge once scrolled past the threshold.
    ///   Vertical (`top`) only in v1; horizontal (`left`) is a
    ///   follow-up. Falls back to `Relative` when no enclosing
    ///   scroll view exists (matches CSS).
    /// - **wgpu** — walks up to the enclosing `ScrollView` at
    ///   `apply_style` time, registers the node in a per-backend
    ///   sticky registry, and the render walker applies the pin
    ///   translate at draw time. `refresh_layout_positions`
    ///   refreshes cached natural-y values after each Taffy
    ///   compute. Vertical (`top`) only in v1; falls back to
    ///   `Relative` when there's no enclosing `ScrollView`.
    /// - **Android** — same model as iOS but driven by a per-
    ///   scroll-event `View.OnScrollChangeListener` (Android
    ///   delivers scroll events only when the position actually
    ///   changes, so per-event is strictly cheaper than the
    ///   per-vsync display-link tick iOS uses). The Kotlin
    ///   `RustStickyScrollListener` trampolines back into Rust
    ///   via JNI and writes `View.setTranslationY` (device
    ///   pixels, dp→px via the view's display density) on each
    ///   registered sticky child. Walks up to the enclosing
    ///   `ScrollView`/`HorizontalScrollView` at `apply_style`
    ///   time; deferred to `insert` for first-mount children
    ///   whose parent chain isn't yet wired up. Vertical (`top`)
    ///   only in v1, same scope as iOS. Falls back to `Relative`
    ///   when no enclosing scroll-view ancestor exists.
    /// - **Terminal / Roku / CPU** — silently treated as `Relative`.
    ///   Scrolling on these targets is either inapplicable
    ///   (terminal) or driven by a different model (Roku
    ///   SceneGraph, ESP32 displays).
    Sticky,
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

/// `font-family` value. Either a free-form CSS-style family name
/// (`"Helvetica, sans-serif"`, `"monospace"`) or a declarative
/// [`Typeface`](crate::assets::Typeface) handle, which the framework
/// registers with the backend on first use before any rule that
/// references it is applied.
///
/// Authors usually don't construct this directly. The `stylesheet!`
/// macro wraps every property value in `Into::into(...)`, so:
///
/// ```ignore
/// stylesheet! {
///     pub Body<MyTheme> {
///         base(_) {
///             font_family: "system-ui, sans-serif",       // → System
///             // or
///             font_family: &INTER,                        // → Typeface
///         }
///     }
/// }
/// ```
///
/// goes through `From<&str>` / `From<&'static Typeface>` respectively.
#[derive(Clone, Debug)]
pub enum FontFamily {
    /// A CSS-style family name. Passed verbatim to the platform's
    /// font lookup (web's `font-family`, iOS's `UIFont(name:)`,
    /// Android's `Typeface.create(name)`). Use for system fonts and
    /// for typefaces the OS already knows about.
    System(String),
    /// A declarative typeface registered with the backend on first
    /// observation. The framework calls
    /// [`Backend::register_asset`](crate::Backend::register_asset)
    /// for each face plus
    /// [`Backend::register_typeface`](crate::Backend::register_typeface)
    /// before any `apply_style` that references it; backends then
    /// resolve fonts via the typeface's `family_name`.
    Typeface(crate::assets::Typeface),
}

impl From<String> for FontFamily {
    fn from(s: String) -> Self {
        FontFamily::System(s)
    }
}
impl From<&str> for FontFamily {
    fn from(s: &str) -> Self {
        FontFamily::System(s.to_string())
    }
}
impl From<crate::assets::Typeface> for FontFamily {
    fn from(t: crate::assets::Typeface) -> Self {
        FontFamily::Typeface(t)
    }
}
impl From<&'static crate::assets::Typeface> for FontFamily {
    fn from(t: &'static crate::assets::Typeface) -> Self {
        FontFamily::Typeface(*t)
    }
}

impl PartialEq for FontFamily {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (FontFamily::System(a), FontFamily::System(b)) => a == b,
            // Typefaces are equal iff their ids match. Cheaper than
            // comparing `&'static` slices structurally and matches the
            // backend's dedup key.
            (FontFamily::Typeface(a), FontFamily::Typeface(b)) => a.id == b.id,
            _ => false,
        }
    }
}
impl Eq for FontFamily {}
impl std::hash::Hash for FontFamily {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            FontFamily::System(s) => {
                state.write_u8(0);
                s.hash(state);
            }
            FontFamily::Typeface(t) => {
                state.write_u8(1);
                t.id.hash(state);
            }
        }
    }
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

/// Pointer affordance for a node — the shape the OS pointer takes when
/// hovering it. A **desktop / web** concern: it has no meaning on touch
/// backends (there is no pointer), so iOS and Android silently ignore
/// it. Mapping:
/// - Web/SSR: CSS `cursor` keyword (`pointer`, `text`, `not-allowed`, …).
/// - macOS (AppKit): the matching [`NSCursor`] pushed over the view's
///   tracking rect; values without a system equivalent fall back to the
///   arrow.
/// - iOS / Android: no-op (touch has no hover pointer).
///
/// The framework imposes **no** default cursor on any primitive — a bare
/// `Pressable`/`Button` shows the platform default. Component libraries
/// (e.g. idea-ui) opt their clickables into [`Cursor::Pointer`] via this
/// property; that is the single source of truth (the old hardcoded inline
/// `cursor: pointer` on the web pressable is gone, so an author setting
/// `cursor` here is never overridden by an un-overridable inline style).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Cursor {
    /// Browser/OS picks based on context (CSS `auto`).
    #[default]
    Auto,
    /// The standard arrow (CSS `default`).
    Default,
    /// Hand / pointing finger — the "this is clickable" affordance
    /// (CSS `pointer`, `NSCursor::pointingHandCursor`).
    Pointer,
    /// I-beam for selectable/editable text (CSS `text`,
    /// `NSCursor::IBeamCursor`).
    Text,
    /// Busy indicator (CSS `wait`). macOS has no public busy cursor →
    /// arrow.
    Wait,
    /// In-progress but still interactive (CSS `progress`). macOS → arrow.
    Progress,
    /// Help affordance (CSS `help`). macOS → arrow.
    Help,
    /// Action not permitted (CSS `not-allowed`,
    /// `NSCursor::operationNotAllowedCursor`).
    NotAllowed,
    /// Draggable/movable target (CSS `move`). macOS → arrow.
    Move,
    /// Grabbable (CSS `grab`, `NSCursor::openHandCursor`).
    Grab,
    /// Mid-grab (CSS `grabbing`, `NSCursor::closedHandCursor`).
    Grabbing,
    /// Precision crosshair (CSS `crosshair`,
    /// `NSCursor::crosshairCursor`).
    Crosshair,
    /// Column / horizontal resize (CSS `col-resize`,
    /// `NSCursor::resizeLeftRightCursor`).
    ColResize,
    /// Row / vertical resize (CSS `row-resize`,
    /// `NSCursor::resizeUpDownCursor`).
    RowResize,
    /// East-west resize (CSS `ew-resize`, same NSCursor as `ColResize`).
    EwResize,
    /// North-south resize (CSS `ns-resize`, same NSCursor as `RowResize`).
    NsResize,
}

/// Whether (and how) a node's text can be selected by the user. Like
/// [`Cursor`], a **desktop / web** concern — touch backends don't have a
/// drag-to-select gesture for arbitrary UI text and ignore it. Mapping:
/// - Web/SSR: CSS `user-select` (emitted with the `-webkit-` prefix for
///   Safari).
/// - macOS (AppKit): toggles `NSTextField`/`NSTextView` `isSelectable`
///   on text nodes; ignored on non-text views.
/// - iOS / Android: no-op (their labels aren't selectable by default).
///
/// The canonical use is [`UserSelect::None`] on a clickable's subtree so
/// double-clicking a button doesn't select its label text. The framework
/// sets no default; component libraries opt in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum UserSelect {
    /// Default selection behavior (CSS `auto`).
    #[default]
    Auto,
    /// Text cannot be selected (CSS `none`).
    None,
    /// Text is selectable (CSS `text`).
    Text,
    /// Selecting selects the whole element's text at once (CSS `all`).
    All,
}

/// Whether an element participates in pointer hit-testing.
///
/// The canonical use is [`PointerEvents::None`] on a purely *decorative* overlay
/// — a drag preview, a highlight, a non-interactive scrim — so pointer events
/// pass straight through it to the content beneath instead of being swallowed.
///
/// - Web: emits CSS `pointer-events`.
/// - Native backends: no-op today (the layering hazard this solves is a web /
///   stacked-DOM problem; native overlays don't intercept the same way).
///
/// The framework sets no default; only an author/SDK opt-in produces a
/// non-default value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PointerEvents {
    /// Default — the element hit-tests normally (CSS `auto`).
    #[default]
    Auto,
    /// The element is transparent to pointer events; they pass through to
    /// whatever is behind it (CSS `none`).
    None,
}

/// Drop shadow. Mobile-shaped — no CSS `spread` (which doesn't map
/// cleanly to UIView/Android shadow APIs). Backends translate:
/// - Web: `box-shadow: {x}px {y}px {blur}px {color}`
/// - iOS: `layer.shadowOffset/Opacity/Radius/Color` setters
/// - Android: `setElevation` + tinting (approximation)
///
/// Note: `blur` here is the *shadow's* blur radius. There is no
/// **backdrop-filter / content blur** (the "glassmorphism" effect of
/// blurring what's behind a translucent panel) — it has no portable
/// equivalent across UIView/Android/DOM that the framework will commit
/// to. Approximate it with a more-opaque translucent `background` fill.
#[derive(Clone, Debug, PartialEq)]
pub struct Shadow {
    pub x: f32,
    pub y: f32,
    pub blur: f32,
    pub color: Color,
}

/// Gradient fill for a view's background. Sits alongside the
/// plain `background` color: when both are set, the gradient
/// renders over (z-replaces) the solid background. Each backend
/// maps onto its native gradient primitive:
/// - Web: `background-image: linear-gradient(...)` / `radial-gradient(...)`.
/// - iOS: `CAGradientLayer` (`.axial` for linear, `.radial` for radial).
/// - Android: `GradientDrawable` with the corresponding gradient type,
///   or a manual `RadialGradient` + `Paint` when the type isn't expressible.
#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    pub kind: GradientKind,
    /// Color stops ordered by ascending offset. Each `(offset, color)`
    /// pair lives in normalized 0..=1 space: `0.0` is the start of the
    /// gradient (axial origin / radial center) and `1.0` is the far
    /// end (axial terminus / radius edge). Stops outside this range
    /// are clamped by each backend.
    pub stops: Vec<GradientStop>,
}

/// One color stop in a [`Gradient`].
#[derive(Clone, Debug, PartialEq)]
pub struct GradientStop {
    /// Offset along the gradient's axis (linear) or radius (radial),
    /// in normalized 0..=1 space.
    pub offset: f32,
    pub color: Color,
}

/// The shape of a gradient — linear or radial. Each variant carries
/// only the parameters specific to its shape; the color stops live
/// on the parent [`Gradient`].
#[derive(Clone, Debug, PartialEq)]
pub enum GradientKind {
    /// Linear gradient along an axis defined by an angle.
    Linear {
        /// Direction of the gradient axis in degrees, clockwise from
        /// straight-up (CSS convention): `0` = bottom→top,
        /// `90` = left→right, `180` = top→bottom, `270` = right→left.
        angle_deg: f32,
    },
    /// Radial gradient emanating from a center point.
    Radial {
        /// Center of the radial gradient, normalized 0..=1 in the
        /// view's local space. `(0.5, 0.5)` puts the center in the
        /// middle of the view; `(1.0, 0.0)` puts it at top-right.
        center: (f32, f32),
        /// Distance at which the last stop (offset=1.0) sits,
        /// expressed as a multiple of the chosen `extent`. With
        /// `extent: ClosestSide` and `radius: 1.0`, the outermost
        /// stop sits at the closest edge midpoint; with `radius: 2.0`
        /// it sits twice as far. Values >1.0 push the last stop
        /// past the box, which is useful when the view is clipped
        /// to rounded corners and you don't want the gradient cut
        /// short of the visible edge.
        radius: f32,
        /// What "100%" means for the gradient — the reference
        /// distance multiplied by `radius`. Mirrors CSS's
        /// `closest-side` / `farthest-corner` keywords on
        /// `radial-gradient`. Use `FarthestCorner` for vignettes
        /// that must reach the screen corners on non-square
        /// viewports; the default `ClosestSide` works for
        /// aspect-ratio:1 discs (suns, dots, badges).
        extent: RadialExtent,
    },
}

/// Reference distance for a [`GradientKind::Radial`]. Determines
/// what "100% of radius" means in the view's local coordinate
/// space — matches the equivalent CSS `radial-gradient(<extent>, …)`
/// keywords.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum RadialExtent {
    /// Distance to the closest edge midpoint. On a 100×200 box
    /// centered, the reference is 50px (half the shorter side).
    /// Best for circular content on square boxes — the disc
    /// reaches the view edge at `radius: 1.0`.
    #[default]
    ClosestSide,
    /// Distance to the farthest corner. On the same 100×200 box
    /// centered, the reference is √(50² + 100²) ≈ 112px. Use this
    /// when the gradient should reach the corners of a non-square
    /// box — vignettes, screen-filling glows.
    FarthestCorner,
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
    pub background: Option<Tokenized<Color>>,
    pub color: Option<Tokenized<Color>>,
    /// Caret color for text-input primitives (`TextInput`, `TextArea`).
    /// Maps to CSS `caret-color` on web, `tintColor` on UIKit, and
    /// `setTextCursorDrawable` (API 29+) on Android. Has no effect on
    /// non-input nodes — backends silently ignore it elsewhere. The
    /// browser's `caret-color: auto` default follows `color`, so an
    /// editor that paints `color: transparent` (to defer rendering to
    /// a syntax-highlight overlay) MUST pin `caret_color` explicitly
    /// or the caret disappears too.
    pub caret_color: Option<Tokenized<Color>>,
    pub font_size: Option<Tokenized<Length>>,

    // --- Display mode (which algorithm lays out this node's children) ---
    /// `None` ⇒ the framework default (`Flex`). Set `Grid` to lay
    /// children out as grid items; pair with `grid_template_columns`.
    pub display: Option<DisplayKind>,
    /// Grid column tracks, one [`TrackSize`] per column. Only meaningful
    /// when `display == Some(DisplayKind::Grid)`. Rows are implicit
    /// (row-major auto-flow): direct children fill the tracks
    /// left-to-right, wrapping to a new row every `len()` cells — which
    /// is how the `table` SDK aligns every column to one width across
    /// all rows. Ignored under flex.
    pub grid_template_columns: Option<Vec<TrackSize>>,

    // --- Flex container (applies when this node has children) ---
    pub flex_direction: Option<FlexDirection>,
    pub flex_wrap: Option<FlexWrap>,
    pub justify_content: Option<JustifyContent>,
    pub align_items: Option<AlignItems>,
    pub align_content: Option<AlignContent>,
    pub gap: Option<Tokenized<Length>>,
    pub row_gap: Option<Tokenized<Length>>,
    pub column_gap: Option<Tokenized<Length>>,

    // --- Flex item (this node's behavior inside its parent) ---
    pub flex_grow: Option<Tokenized<f32>>,
    pub flex_shrink: Option<Tokenized<f32>>,
    pub flex_basis: Option<Tokenized<Length>>,
    pub align_self: Option<AlignSelf>,

    // --- Sizing ---
    pub width: Option<Tokenized<Length>>,
    pub height: Option<Tokenized<Length>>,
    pub min_width: Option<Tokenized<Length>>,
    pub min_height: Option<Tokenized<Length>>,
    pub max_width: Option<Tokenized<Length>>,
    pub max_height: Option<Tokenized<Length>>,
    /// Preferred width-to-height ratio (`width / height`). When set,
    /// the layout engine sizes the unspecified dimension to satisfy
    /// the ratio. Useful for keeping a square (`1.0`) or
    /// fixed-aspect (e.g. `16.0 / 9.0`) box even when only one
    /// dimension is sized as a percentage of the parent. Mirrors
    /// CSS `aspect-ratio` and Taffy's `aspect_ratio` field.
    pub aspect_ratio: Option<f32>,

    // --- Padding (per-side; no shorthand field) ---
    pub padding_top: Option<Tokenized<Length>>,
    pub padding_right: Option<Tokenized<Length>>,
    pub padding_bottom: Option<Tokenized<Length>>,
    pub padding_left: Option<Tokenized<Length>>,

    // --- Margin (per-side; no shorthand field) ---
    pub margin_top: Option<Tokenized<Length>>,
    pub margin_right: Option<Tokenized<Length>>,
    pub margin_bottom: Option<Tokenized<Length>>,
    pub margin_left: Option<Tokenized<Length>>,

    // --- Border radius (per-corner) ---
    pub border_top_left_radius: Option<Tokenized<Length>>,
    pub border_top_right_radius: Option<Tokenized<Length>>,
    pub border_bottom_left_radius: Option<Tokenized<Length>>,
    pub border_bottom_right_radius: Option<Tokenized<Length>>,

    // --- Border widths (per-side, `f32` not `Length` — borders aren't
    //     percentages). All four are independent. A `Length` coerces in
    //     (`border_left_width: Length::Px(2.0)`) via `From<Length>`;
    //     percent/auto are rejected (→ 0 + debug warning). ---
    pub border_top_width: Option<Tokenized<f32>>,
    pub border_right_width: Option<Tokenized<f32>>,
    pub border_bottom_width: Option<Tokenized<f32>>,
    pub border_left_width: Option<Tokenized<f32>>,

    // --- Border colors (per-side). ---
    pub border_top_color: Option<Tokenized<Color>>,
    pub border_right_color: Option<Tokenized<Color>>,
    pub border_bottom_color: Option<Tokenized<Color>>,
    pub border_left_color: Option<Tokenized<Color>>,

    // --- Position ---
    pub position: Option<Position>,
    pub top: Option<Tokenized<Length>>,
    pub right: Option<Tokenized<Length>>,
    pub bottom: Option<Tokenized<Length>>,
    pub left: Option<Tokenized<Length>>,

    // --- Typography (text-only on native; cascade on web) ---
    pub font_family: Option<FontFamily>,
    pub font_weight: Option<FontWeight>,
    pub font_style: Option<FontStyle>,
    pub line_height: Option<Tokenized<f32>>,
    pub letter_spacing: Option<Tokenized<f32>>,
    pub text_align: Option<TextAlign>,
    pub underline: Option<bool>,
    pub strikethrough: Option<bool>,
    pub text_transform: Option<TextTransform>,

    // --- Visual ---
    pub opacity: Option<Tokenized<f32>>,
    pub overflow: Option<Overflow>,
    pub shadow: Option<Shadow>,
    /// Gradient background, rendered over (replacing) the solid
    /// `background` color when both are set. Each backend maps to its
    /// native gradient primitive — see [`Gradient`]'s doc for the
    /// mapping table.
    pub background_gradient: Option<Gradient>,
    /// Empty vec means "no transforms"; the field's `Option` distinguishes
    /// "not set, fall through to other layers" from "explicitly empty".
    pub transform: Option<Vec<Transform>>,
    /// Origin point for `transform` (and per-frame animated scale /
    /// rotate / translate). Defaults to the element's center on every
    /// platform when `None`. Components are the X and Y origin —
    /// `(pct(0.0), pct(0.0))` = top-left, `(pct(100.0), pct(0.0))` =
    /// top-right, `(pct(50.0), pct(100.0))` = bottom-center. Percent
    /// units are relative to the element's own box, NOT its parent —
    /// matches CSS `transform-origin`.
    pub transform_origin: Option<(Length, Length)>,

    // --- Interaction (desktop/web only; touch backends no-op) ---
    /// Pointer shape on hover. See [`Cursor`] for the per-backend mapping.
    /// `None` = inherit the platform default; the framework imposes no
    /// default, so only an author/component opt-in produces a non-default
    /// cursor.
    pub cursor: Option<Cursor>,
    /// Text-selection behavior. See [`UserSelect`]. The common opt-in is
    /// [`UserSelect::None`] on a clickable so its label can't be selected.
    pub user_select: Option<UserSelect>,
    /// Pointer hit-testing. See [`PointerEvents`]. The common opt-in is
    /// [`PointerEvents::None`] on a decorative overlay (e.g. a drag preview) so
    /// it doesn't swallow the clicks/drags meant for the content beneath.
    pub pointer_events: Option<PointerEvents>,

    // --- Transitions ---
    // One per animatable property. Set via `transitions { ... }` in
    // the `stylesheet!` macro. When the property's resolved value
    // changes, the backend interpolates over `duration_ms` using
    // `easing`. Properties without a transition spec change instantly.
    pub background_transition: Option<Transition>,
    pub color_transition: Option<Transition>,
    pub caret_color_transition: Option<Transition>,
    pub opacity_transition: Option<Transition>,
    pub transform_transition: Option<Transition>,
    pub width_transition: Option<Transition>,
    pub height_transition: Option<Transition>,
    pub max_width_transition: Option<Transition>,
    pub max_height_transition: Option<Transition>,
    pub min_width_transition: Option<Transition>,
    pub min_height_transition: Option<Transition>,
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
            background, color, caret_color, font_size,
            display, grid_template_columns,
            flex_direction, flex_wrap, justify_content, align_items, align_content,
            gap, row_gap, column_gap,
            flex_grow, flex_shrink, flex_basis, align_self,
            width, height, min_width, min_height, max_width, max_height, aspect_ratio,
            padding_top, padding_right, padding_bottom, padding_left,
            margin_top, margin_right, margin_bottom, margin_left,
            border_top_left_radius, border_top_right_radius,
            border_bottom_left_radius, border_bottom_right_radius,
            border_top_width, border_right_width, border_bottom_width, border_left_width,
            border_top_color, border_right_color, border_bottom_color, border_left_color,
            position, top, right, bottom, left,
            font_family, font_weight, font_style, line_height, letter_spacing,
            text_align, underline, strikethrough, text_transform,
            opacity, overflow, shadow, background_gradient, transform, transform_origin,
            cursor, user_select, pointer_events,
            background_transition, color_transition, caret_color_transition,
            opacity_transition,
            transform_transition, width_transition, height_transition,
            max_width_transition, max_height_transition,
            min_width_transition, min_height_transition,
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
    ///
    /// **Tokenized fields hash the token name, not the fallback value.**
    /// Two themes that bind `color-accent` to different concrete colors
    /// produce the same content key — so the same `(sheet, variants)`
    /// always maps to the same minted class regardless of which theme
    /// is active. Theme swap then only updates the variable values, not
    /// any element's `className`.
    pub fn content_key(&self) -> String {
        let mut s = String::with_capacity(256);
        write_tokenized_color(&mut s, "bg", &self.background);
        write_tokenized_color(&mut s, "fg", &self.color);
        write_tokenized_color(&mut s, "cc", &self.caret_color);
        write_tokenized_length(&mut s, "fs", &self.font_size);

        write_enum(&mut s, "disp", self.display.map(|x| x as u8));
        if let Some(cols) = self.grid_template_columns.as_ref() {
            s.push_str("gtc=");
            for t in cols {
                write_track_size(&mut s, t);
                s.push(',');
            }
            s.push(';');
        }

        write_enum(&mut s, "fd", self.flex_direction.map(|x| x as u8));
        write_enum(&mut s, "fw", self.flex_wrap.map(|x| x as u8));
        write_enum(&mut s, "jc", self.justify_content.map(|x| x as u8));
        write_enum(&mut s, "ai", self.align_items.map(|x| x as u8));
        write_enum(&mut s, "ac", self.align_content.map(|x| x as u8));
        write_tokenized_length(&mut s, "gap", &self.gap);
        write_tokenized_length(&mut s, "rgap", &self.row_gap);
        write_tokenized_length(&mut s, "cgap", &self.column_gap);

        write_tokenized_f32(&mut s, "fg-grow", &self.flex_grow);
        write_tokenized_f32(&mut s, "fs-shrink", &self.flex_shrink);
        write_tokenized_length(&mut s, "fb", &self.flex_basis);
        write_enum(&mut s, "as", self.align_self.map(|x| x as u8));

        write_tokenized_length(&mut s, "w", &self.width);
        write_tokenized_length(&mut s, "h", &self.height);
        write_tokenized_length(&mut s, "minw", &self.min_width);
        write_tokenized_length(&mut s, "minh", &self.min_height);
        write_tokenized_length(&mut s, "maxw", &self.max_width);
        write_tokenized_length(&mut s, "maxh", &self.max_height);
        if let Some(ar) = self.aspect_ratio {
            s.push_str("ar=");
            push_u32_hex(&mut s, ar.to_bits());
            s.push(';');
        }

        write_tokenized_length(&mut s, "pt", &self.padding_top);
        write_tokenized_length(&mut s, "pr", &self.padding_right);
        write_tokenized_length(&mut s, "pb", &self.padding_bottom);
        write_tokenized_length(&mut s, "pl", &self.padding_left);
        write_tokenized_length(&mut s, "mt", &self.margin_top);
        write_tokenized_length(&mut s, "mr", &self.margin_right);
        write_tokenized_length(&mut s, "mb", &self.margin_bottom);
        write_tokenized_length(&mut s, "ml", &self.margin_left);

        write_tokenized_length(&mut s, "rtl", &self.border_top_left_radius);
        write_tokenized_length(&mut s, "rtr", &self.border_top_right_radius);
        write_tokenized_length(&mut s, "rbl", &self.border_bottom_left_radius);
        write_tokenized_length(&mut s, "rbr", &self.border_bottom_right_radius);

        write_tokenized_f32(&mut s, "bwt", &self.border_top_width);
        write_tokenized_f32(&mut s, "bwr", &self.border_right_width);
        write_tokenized_f32(&mut s, "bwb", &self.border_bottom_width);
        write_tokenized_f32(&mut s, "bwl", &self.border_left_width);
        write_tokenized_color(&mut s, "bct", &self.border_top_color);
        write_tokenized_color(&mut s, "bcr", &self.border_right_color);
        write_tokenized_color(&mut s, "bcb", &self.border_bottom_color);
        write_tokenized_color(&mut s, "bcl", &self.border_left_color);

        write_enum(&mut s, "pos", self.position.map(|x| x as u8));
        write_tokenized_length(&mut s, "top", &self.top);
        write_tokenized_length(&mut s, "right", &self.right);
        write_tokenized_length(&mut s, "bot", &self.bottom);
        write_tokenized_length(&mut s, "left", &self.left);

        // Typography
        let ff_buf: Option<String> = self.font_family.as_ref().map(|ff| match ff {
            FontFamily::System(name) => name.clone(),
            // Typeface key is the id — two stylesheets that reference
            // the same `Typeface` produce identical content keys
            // regardless of the family-name string.
            FontFamily::Typeface(t) => format!("tf:{}", t.id.0),
        });
        write_str(&mut s, "ff", ff_buf.as_deref());
        write_enum(&mut s, "fw", self.font_weight.map(|x| x as u8));
        write_enum(&mut s, "fst", self.font_style.map(|x| x as u8));
        write_tokenized_f32(&mut s, "lh", &self.line_height);
        write_tokenized_f32(&mut s, "ls", &self.letter_spacing);
        write_enum(&mut s, "ta", self.text_align.map(|x| x as u8));
        write_enum(&mut s, "ul", self.underline.map(|b| b as u8));
        write_enum(&mut s, "st", self.strikethrough.map(|b| b as u8));
        write_enum(&mut s, "tt", self.text_transform.map(|x| x as u8));

        // Visual
        write_tokenized_f32(&mut s, "op", &self.opacity);
        write_enum(&mut s, "ov", self.overflow.map(|x| x as u8));
        if let Some(sh) = &self.shadow {
            s.push_str("sh=");
            push_u32_hex(&mut s, sh.x.to_bits());
            push_u32_hex(&mut s, sh.y.to_bits());
            push_u32_hex(&mut s, sh.blur.to_bits());
            s.push_str(&sh.color.0);
            s.push(';');
        }
        if let Some(g) = &self.background_gradient {
            s.push_str("bg=");
            match g.kind {
                GradientKind::Linear { angle_deg } => {
                    s.push_str("lin");
                    push_u32_hex(&mut s, angle_deg.to_bits());
                }
                GradientKind::Radial { center, radius, extent } => {
                    s.push_str("rad");
                    push_u32_hex(&mut s, center.0.to_bits());
                    push_u32_hex(&mut s, center.1.to_bits());
                    push_u32_hex(&mut s, radius.to_bits());
                    s.push_str(match extent {
                        RadialExtent::ClosestSide => "cs",
                        RadialExtent::FarthestCorner => "fc",
                    });
                }
            }
            for stop in &g.stops {
                push_u32_hex(&mut s, stop.offset.to_bits());
                s.push_str(&stop.color.0);
                s.push(',');
            }
            s.push(';');
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
        }
        if let Some((ox, oy)) = self.transform_origin {
            s.push_str("to=");
            push_u64_hex(&mut s, length_bits(ox));
            push_u64_hex(&mut s, length_bits(oy));
            s.push(';');
        }

        // Interaction
        write_enum(&mut s, "cur", self.cursor.map(|x| x as u8));
        write_enum(&mut s, "usel", self.user_select.map(|x| x as u8));
        write_enum(&mut s, "pev", self.pointer_events.map(|x| x as u8));

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
        tr!("tcc", caret_color_transition);
        tr!("top_t", opacity_transition);
        tr!("ttr", transform_transition);
        tr!("tw", width_transition);
        tr!("th", height_transition);
        tr!("tmaxw", max_width_transition);
        tr!("tmaxh", max_height_transition);
        tr!("tminw", min_width_transition);
        tr!("tminh", min_height_transition);
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
    let Some(t) = t else { return };
    out.push_str(label);
    out.push('=');
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
    out.push(';');
}

fn write_str(out: &mut String, label: &str, v: Option<&str>) {
    let Some(v) = v else { return };
    out.push_str(label);
    out.push('=');
    out.push_str(v);
    out.push(';');
}

/// Tokenized-color content-key segment. Token references hash by
/// **name** (`t:color-accent`) so two themes binding the same name to
/// different colors produce identical keys; literals hash by value.
/// The literal/token discriminator (`L:` / `T:`) prevents a token
/// named "ff0000" from colliding with the literal hex `#ff0000`.
// Note on sparse encoding: each writer emits ONLY when the field is
// `Some`. The previous emit-`label=;`-always shape wasted ~580 bytes
// per `content_key` call on overrides that set 1-2 fields (the bulk
// of reactive-style use cases). At hierarchy scale (20k Effects
// firing per shared-signal bump) the per-call savings translate to
// ~30ms / bump — pure waste because the empty `label=;` carried no
// information the `Some(_)` writes don't already encode. Two
// distinct override sets still produce distinct keys: the field
// labels in `Some` writes are unique, and unset fields contribute
// nothing rather than contributing a fixed prefix.

fn write_tokenized_color(out: &mut String, label: &str, c: &Option<Tokenized<Color>>) {
    let Some(t) = c else { return };
    out.push_str(label);
    out.push('=');
    match t {
        Tokenized::Literal(c) => {
            out.push_str("L:");
            out.push_str(&c.0);
        }
        Tokenized::Token { name, .. } => {
            out.push_str("T:");
            out.push_str(name);
        }
    }
    out.push(';');
}

fn write_tokenized_length(out: &mut String, label: &str, l: &Option<Tokenized<Length>>) {
    let Some(t) = l else { return };
    out.push_str(label);
    out.push('=');
    match t {
        Tokenized::Literal(v) => {
            out.push_str("L:");
            push_u64_hex(out, length_bits(*v));
        }
        Tokenized::Token { name, .. } => {
            out.push_str("T:");
            out.push_str(name);
        }
    }
    out.push(';');
}

fn write_tokenized_f32(out: &mut String, label: &str, v: &Option<Tokenized<f32>>) {
    let Some(t) = v else { return };
    out.push_str(label);
    out.push('=');
    match t {
        Tokenized::Literal(v) => {
            out.push_str("L:");
            push_u32_hex(out, v.to_bits());
        }
        Tokenized::Token { name, .. } => {
            out.push_str("T:");
            out.push_str(name);
        }
    }
    out.push(';');
}

fn write_enum(out: &mut String, label: &str, v: Option<u8>) {
    let Some(v) = v else { return };
    out.push_str(label);
    out.push('=');
    push_u32_hex(out, v as u32);
    out.push(';');
}

/// Encodes a [`TrackSize`] into a `content_key` segment. Recurses once
/// for `Minmax`; the bit pattern of `f32` values keeps distinct sizes
/// distinct without `format!`.
fn write_track_size(out: &mut String, t: &TrackSize) {
    match t {
        TrackSize::Auto => out.push('a'),
        TrackSize::MinContent => out.push_str("mn"),
        TrackSize::MaxContent => out.push_str("mx"),
        TrackSize::Fr(v) => {
            out.push('f');
            push_u32_hex(out, v.to_bits());
        }
        TrackSize::Px(v) => {
            out.push('p');
            push_u32_hex(out, v.to_bits());
        }
        TrackSize::Minmax(lo, hi) => {
            out.push('[');
            write_track_size(out, lo);
            out.push(':');
            write_track_size(out, hi);
            out.push(']');
        }
    }
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
// StyleSheet — closures from variants to rules, with variants and compounds
// ----------------------------------------------------------------------------

type RulesFn = Box<dyn Fn(&VariantSet) -> StyleRules>;

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
/// is a closure that takes the effective `VariantSet` and returns
/// concrete `StyleRules`. Stylesheets emit `Tokenized<T>` references by
/// name; token values are managed separately via [`install_tokens`].
///
/// # Resolution order
/// 1. `base`
/// 2. For each declared axis, layer the closure for the value selected
///    in the `VariantSet` (or the axis's default if unselected).
/// 3. For each declared compound variant, layer its closure iff every
///    `(axis, value)` in `when` matches the *effective* variant set
///    (defaults included).
/// 4. Any `StyleApplication::overrides` field.
thread_local! {
    /// Single shared per-sheet cache backing every `stylesheet!`-generated
    /// `*_style()` constructor. Each generated fn passes a process-unique
    /// key (the address of a function-local `static`) and its built
    /// `Rc<StyleSheet>` is minted once per thread, then reused.
    ///
    /// Why ONE shared registry rather than a `thread_local!` *per* sheet:
    /// Android's bionic libc caps total pthread TLS keys at
    /// `PTHREAD_KEYS_MAX` (128, minus runtime-reserved), and Rust's std
    /// uses a pthread-key-backed TLS model on Android — so every
    /// `thread_local!` burns one key. idea-ui alone declares 70+
    /// stylesheets; a key apiece exhausted the table and aborted in
    /// `LazyKey::lazy_init` during mount (the abort surfaced under
    /// whichever sheet happened to allocate the key past the cap —
    /// `grid_row_style` in the idea-ui-docs build). Collapsing all sheet
    /// caches into this single key keeps the key count flat no matter how
    /// many stylesheets the binary links.
    static STYLESHEET_CACHE: RefCell<HashMap<usize, Rc<StyleSheet>>> =
        RefCell::new(HashMap::new());
}

/// Returns the thread-cached `Rc<StyleSheet>` for `key`, building and
/// caching it on first call. `key` must be process-unique per logical
/// stylesheet — the `stylesheet!` macro passes the address of a
/// function-local `static` so distinct sheets never collide and the same
/// sheet always maps to the same entry.
///
/// Reentrancy-safe: `build` runs with no borrow of the cache held, so a
/// stylesheet whose construction references another `*_style()` (nested
/// sheet reference) cannot double-borrow the registry.
pub fn cached_stylesheet(
    key: usize,
    build: impl FnOnce() -> Rc<StyleSheet>,
) -> Rc<StyleSheet> {
    if let Some(rc) = STYLESHEET_CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return rc;
    }
    let rc = build();
    STYLESHEET_CACHE.with(|c| {
        c.borrow_mut().insert(key, rc.clone());
    });
    rc
}

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
    state_axes: Vec<(crate::StateBits, VariantAxis)>,
    /// Cached list of breakpoint-overlay axes the sheet declares.
    /// Populated in `.variant(...)` whenever an axis named `__bp_*` is
    /// added (a `stylesheet!`'s `breakpoint md { … }` block). Empty for
    /// the common case of sheets with no breakpoint blocks —
    /// `resolve_breakpoint_overlays` short-circuits on `is_empty()` and
    /// avoids walking the variants BTreeMap per styled node. Exactly
    /// parallel to [`Self::state_axes`].
    breakpoint_axes: Vec<(crate::Breakpoint, VariantAxis)>,
    /// Cached list of container-query overlay axes the sheet declares,
    /// each paired with its `min_width` threshold in px. Populated in
    /// `.variant(...)` whenever an axis named `__cq_minw_*` is added (a
    /// `stylesheet!`'s `container (min_width: N) { … }` block). Empty for
    /// the common case of sheets with no container blocks —
    /// `resolve_container_overlays` short-circuits on `is_empty()` and
    /// avoids walking the variants BTreeMap per styled node. Parallel to
    /// [`Self::breakpoint_axes`], but keyed on an arbitrary `f32`
    /// threshold rather than a fixed bucket enum.
    container_axes: Vec<(f32, VariantAxis)>,
    /// Per-sheet variant cache. Keyed on the effective `VariantSet`;
    /// value is the pre-resolved `Rc<StyleRules>` for the no-overrides
    /// case. Populated by [`ensure_registered_with`] at registration
    /// time. The cache survives token updates because tokenized
    /// `StyleRules` carry token *names* (not values) so the rule
    /// content is token-stable.
    variant_cache: std::cell::RefCell<HashMap<VariantSet, Rc<StyleRules>>>,
}

impl StyleSheet {
    /// Constructs a stylesheet whose base rules are produced by `f`.
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(&VariantSet) -> StyleRules + 'static,
    {
        Self {
            base: Box::new(f),
            variants: BTreeMap::new(),
            compounds: Vec::new(),
            state_axes: Vec::new(),
            breakpoint_axes: Vec::new(),
            container_axes: Vec::new(),
            variant_cache: std::cell::RefCell::new(HashMap::new()),
        }
    }

    /// A stylesheet whose base rules ignore the variant set.
    pub fn r#static(rules: StyleRules) -> Self {
        Self {
            base: Box::new(move |_vs: &VariantSet| rules.clone()),
            variants: BTreeMap::new(),
            compounds: Vec::new(),
            state_axes: Vec::new(),
            breakpoint_axes: Vec::new(),
            container_axes: Vec::new(),
            variant_cache: std::cell::RefCell::new(HashMap::new()),
        }
    }

    /// Adds (or replaces) a variant overlay on the given axis-value.
    /// If the axis didn't exist yet it's created with no default.
    pub fn variant<F>(
        mut self,
        axis: impl Into<VariantAxis>,
        value: impl Into<VariantValue>,
        f: F,
    ) -> Self
    where
        F: Fn(&VariantSet) -> StyleRules + 'static,
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
        // Same caching for breakpoint overlays (`__bp_*` axes), so
        // `resolve_breakpoint_overlays` short-circuits on the common
        // no-breakpoint-blocks case instead of walking the variants map.
        if let Some(bp) = crate::Breakpoint::from_axis_name(&axis) {
            if !self.breakpoint_axes.iter().any(|(_, a)| a == &axis) {
                self.breakpoint_axes.push((bp, axis.clone()));
            }
        }
        // Same caching for container-query overlays (`__cq_minw_*` axes),
        // so `resolve_container_overlays` short-circuits on the common
        // no-container-blocks case. Keyed on the decoded px threshold.
        if let Some(threshold) = crate::container_axis_threshold(&axis) {
            if !self.container_axes.iter().any(|(_, a)| a == &axis) {
                self.container_axes.push((threshold, axis.clone()));
            }
        }
        let entry = self.variants.entry(axis).or_insert_with(|| VariantAxisDef {
            default: None,
            values: BTreeMap::new(),
        });
        entry.values.insert(value, Box::new(f));
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

    /// The cached set of breakpoint-overlay axes declared on this
    /// stylesheet, in declaration order. Returns an empty slice for the
    /// common case of sheets with no `breakpoint` blocks. Used by
    /// `resolve_breakpoint_overlays` to skip per-call iteration of the
    /// full variants map. Parallel to [`Self::state_axes`].
    pub(crate) fn breakpoint_axes(&self) -> &[(crate::Breakpoint, VariantAxis)] {
        &self.breakpoint_axes
    }

    /// The cached set of container-query overlay axes declared on this
    /// stylesheet, each with its `min_width` threshold (px). Returns an
    /// empty slice for the common case of sheets with no `container`
    /// blocks. Used by `resolve_container_overlays` to skip per-call
    /// iteration of the full variants map. Parallel to
    /// [`Self::breakpoint_axes`].
    pub(crate) fn container_axes(&self) -> &[(f32, VariantAxis)] {
        &self.container_axes
    }

    /// Per-sheet variant-cache lookup. Returns the pre-resolved
    /// `Rc<StyleRules>` if `variants` has been registered, `None`
    /// otherwise. The hot path in [`resolve`] hits this before the
    /// global resolution cache.
    pub(crate) fn lookup_variant(&self, variants: &VariantSet) -> Option<Rc<StyleRules>> {
        self.variant_cache.borrow().get(variants).cloned()
    }

    /// Insert a pre-resolved rule into the variant cache. Called
    /// from [`ensure_registered_with`] for each pregen entry.
    pub(crate) fn insert_variant(&self, variants: VariantSet, rc: Rc<StyleRules>) {
        self.variant_cache.borrow_mut().insert(variants, rc);
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
    pub fn compound<F>(
        mut self,
        when: Vec<(impl Into<VariantAxis>, impl Into<VariantValue>)>,
        f: F,
    ) -> Self
    where
        F: Fn(&VariantSet) -> StyleRules + 'static,
    {
        let when: BTreeMap<VariantAxis, VariantValue> =
            when.into_iter().map(|(a, v)| (a.into(), v.into())).collect();
        self.compounds.push(CompoundVariant {
            when,
            rules: Box::new(f),
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

    /// Resolves the stylesheet against the given variant set.
    pub fn resolve(&self, variants: &VariantSet) -> StyleRules {
        let effective_variants = self.effective_variants(variants);
        let mut effective = (self.base)(&effective_variants);

        // Per-axis variants.
        for (axis, def) in &self.variants {
            if let Some(value) = effective_variants.0.get(axis) {
                if let Some(f) = def.values.get(value) {
                    effective = effective.merge(&f(&effective_variants));
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
                effective = effective.merge(&(c.rules)(&effective_variants));
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
/// 2. **Variants**: each active variant's overlay closure output —
///    the *closed* matrix declared at `stylesheet!` macro time.
/// 3. **Computed**: a runtime closure that returns `StyleRules`, paired
///    with a caller-supplied cache key. Used by *open-extension*
///    variant systems (e.g. idea-ui's trait-based Variant/Tone/Size)
///    where the modifier set isn't enumerable at compile time. The
///    closure runs once per unique key per theme; results are memoized
///    in `RESOLUTION_CACHE` alongside variant/override resolutions.
/// 4. **Overrides**: per-call-site continuous values. Used for values
///    that can't be keyed at all — e.g. a user-controlled font scale.
///
/// The backend sees the merged result; it doesn't know which layer
/// contributed what. Backend caches (web CSS classes, etc.) key on the
/// resolved content so each unique combination still gets its own
/// entry.
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
    /// Optional runtime-computed layer. When present, the closure is
    /// invoked between the variant and override merges, and its key
    /// becomes part of the resolution cache key so identical modifier
    /// sets across instances share a single class.
    computed: Option<ComputedLayer>,
}

/// A runtime-evaluated `StyleRules` contribution, paired with a stable
/// cache key. The framework treats `(sheet, variants, computed.key,
/// overrides)` as the resolution-cache identity — equal keys reuse the
/// previously-computed `Rc<StyleRules>`; the closure runs only on cache
/// misses (first apply or after `update_tokens` invalidates the cache).
///
/// Cloneable because the `compute` field is an `Rc`; the closure itself
/// is heap-allocated once and shared.
#[derive(Clone)]
pub struct ComputedLayer {
    /// Stable identifier for what this closure produces. Two closures
    /// that yield equivalent `StyleRules` MUST share the same key; two
    /// closures that yield different outputs MUST have different keys.
    /// Caller's responsibility — typically derived from the
    /// modifier-set identity (e.g. `"filled+danger+md+pill"`).
    pub key: String,
    /// Returns the property contributions for this layer. Called
    /// inside the active apply-style `Effect`, so reactive reads
    /// (token resolutions, signal `.get()` calls) subscribe correctly.
    pub compute: Rc<dyn Fn() -> StyleRules>,
}

impl StyleApplication {
    pub fn new(sheet: Rc<StyleSheet>) -> Self {
        Self {
            sheet,
            variants: VariantSet::new(),
            overrides: StyleRules::default(),
            has_overrides: false,
            computed: None,
        }
    }

    /// Lookup-friendly accessor for the overrides flag. Used by
    /// `resolve()` to pick between the empty-overrides key (just an
    /// empty string) and the full content-keyed path.
    pub fn has_overrides(&self) -> bool {
        self.has_overrides
    }

    /// Attach a computed layer — a closure that produces `StyleRules`
    /// at apply time, paired with a stable cache key. The framework
    /// invokes the closure between the variant and override merges and
    /// memoizes the result in the resolution cache keyed by `key`.
    ///
    /// Typical use: open-extension variant systems where the modifier
    /// matrix isn't enumerable at compile time. The closure pulls
    /// property values from the active theme (via whatever theme
    /// runtime the consumer uses) and returns a `StyleRules`. Two
    /// `StyleApplication`s with the same `key` share a cached result —
    /// so identical modifier sets across many element instances yield
    /// one class on the backend, not N.
    ///
    /// The closure runs:
    /// - On first apply for a given `(sheet, variants, key, overrides)`
    ///   combination.
    /// - Again after `update_tokens` (a theme swap) wipes the cache, so
    ///   theme-dependent reads inside the closure pick up new values.
    pub fn with_computed(
        mut self,
        key: impl Into<String>,
        compute: impl Fn() -> StyleRules + 'static,
    ) -> Self {
        self.computed = Some(ComputedLayer {
            key: key.into(),
            compute: Rc::new(compute),
        });
        self
    }

    /// Read-only access to the attached computed layer, if any.
    pub fn computed(&self) -> Option<&ComputedLayer> {
        self.computed.as_ref()
    }

    pub fn with(
        mut self,
        axis: impl Into<VariantAxis>,
        value: impl Into<VariantValue>,
    ) -> Self {
        self.variants.0.insert(axis.into(), value.into());
        self
    }

    /// Merge an entire `StyleRules` into the override layer — the wholesale
    /// counterpart to the per-field `override_*` setters.
    ///
    /// The override layer resolves LAST (after the sheet, its variants, and any
    /// computed layer), so every field `rules` sets wins. Existing overrides are
    /// preserved unless `rules` also sets that field, in which case `rules`
    /// wins. This is the primitive behind idea-ui's per-slot `*_style` override
    /// props: resolve a component's theme style for a slot, then layer the
    /// author's override sheet on top so ad-hoc tweaks (a custom label color, a
    /// flush/zero-padding modal body) beat the theme without editing it.
    pub fn with_overrides(mut self, rules: StyleRules) -> Self {
        self.has_overrides = true;
        self.overrides = std::mem::take(&mut self.overrides).merge(&rules);
        self
    }

    /// Override the background color with a per-call-site value.
    pub fn override_background(mut self, c: impl Into<Tokenized<Color>>) -> Self {
        self.has_overrides = true;
        self.overrides.background = Some(c.into());
        self
    }

    /// Override the foreground color with a per-call-site value.
    pub fn override_color(mut self, c: impl Into<Tokenized<Color>>) -> Self {
        self.has_overrides = true;
        self.overrides.color = Some(c.into());
        self
    }

    /// Override the caret color with a per-call-site value. See
    /// [`StyleRules::caret_color`] for the cross-platform mapping.
    pub fn override_caret_color(mut self, c: impl Into<Tokenized<Color>>) -> Self {
        self.has_overrides = true;
        self.overrides.caret_color = Some(c.into());
        self
    }

    /// Override font size with a per-call-site value.
    pub fn override_font_size(mut self, v: impl Into<Tokenized<Length>>) -> Self {
        self.has_overrides = true;
        self.overrides.font_size = Some(v.into());
        self
    }

    /// Shorthand override: set padding on all four sides. Equivalent to
    /// calling `override_padding_top`, `_right`, `_bottom`, `_left`
    /// with the same value.
    pub fn override_padding(mut self, v: impl Into<Tokenized<Length>>) -> Self {
        self.has_overrides = true;
        let v = v.into();
        self.overrides.padding_top = Some(v.clone());
        self.overrides.padding_right = Some(v.clone());
        self.overrides.padding_bottom = Some(v.clone());
        self.overrides.padding_left = Some(v);
        self
    }

    pub fn override_padding_horizontal(mut self, v: impl Into<Tokenized<Length>>) -> Self {
        self.has_overrides = true;
        let v = v.into();
        self.overrides.padding_left = Some(v.clone());
        self.overrides.padding_right = Some(v);
        self
    }

    pub fn override_padding_vertical(mut self, v: impl Into<Tokenized<Length>>) -> Self {
        self.has_overrides = true;
        let v = v.into();
        self.overrides.padding_top = Some(v.clone());
        self.overrides.padding_bottom = Some(v);
        self
    }

    pub fn override_padding_top(mut self, v: impl Into<Tokenized<Length>>) -> Self {
        self.has_overrides = true;
        self.overrides.padding_top = Some(v.into()); self
    }
    pub fn override_padding_right(mut self, v: impl Into<Tokenized<Length>>) -> Self {
        self.has_overrides = true;
        self.overrides.padding_right = Some(v.into()); self
    }
    pub fn override_padding_bottom(mut self, v: impl Into<Tokenized<Length>>) -> Self {
        self.has_overrides = true;
        self.overrides.padding_bottom = Some(v.into()); self
    }
    pub fn override_padding_left(mut self, v: impl Into<Tokenized<Length>>) -> Self {
        self.has_overrides = true;
        self.overrides.padding_left = Some(v.into()); self
    }

    /// Shorthand override: margin on all four sides.
    pub fn override_margin(mut self, v: impl Into<Tokenized<Length>>) -> Self {
        self.has_overrides = true;
        let v = v.into();
        self.overrides.margin_top = Some(v.clone());
        self.overrides.margin_right = Some(v.clone());
        self.overrides.margin_bottom = Some(v.clone());
        self.overrides.margin_left = Some(v);
        self
    }

    pub fn override_margin_horizontal(mut self, v: impl Into<Tokenized<Length>>) -> Self {
        self.has_overrides = true;
        let v = v.into();
        self.overrides.margin_left = Some(v.clone());
        self.overrides.margin_right = Some(v);
        self
    }

    pub fn override_margin_vertical(mut self, v: impl Into<Tokenized<Length>>) -> Self {
        self.has_overrides = true;
        let v = v.into();
        self.overrides.margin_top = Some(v.clone());
        self.overrides.margin_bottom = Some(v);
        self
    }

    pub fn override_margin_top(mut self, v: impl Into<Tokenized<Length>>) -> Self {
        self.has_overrides = true;
        self.overrides.margin_top = Some(v.into()); self
    }
    pub fn override_margin_right(mut self, v: impl Into<Tokenized<Length>>) -> Self {
        self.has_overrides = true;
        self.overrides.margin_right = Some(v.into()); self
    }
    pub fn override_margin_bottom(mut self, v: impl Into<Tokenized<Length>>) -> Self {
        self.has_overrides = true;
        self.overrides.margin_bottom = Some(v.into()); self
    }
    pub fn override_margin_left(mut self, v: impl Into<Tokenized<Length>>) -> Self {
        self.has_overrides = true;
        self.overrides.margin_left = Some(v.into()); self
    }

    /// Shorthand override: border-radius on all four corners.
    pub fn override_border_radius(mut self, v: impl Into<Tokenized<Length>>) -> Self {
        self.has_overrides = true;
        let v = v.into();
        self.overrides.border_top_left_radius = Some(v.clone());
        self.overrides.border_top_right_radius = Some(v.clone());
        self.overrides.border_bottom_left_radius = Some(v.clone());
        self.overrides.border_bottom_right_radius = Some(v);
        self
    }
}

// ----------------------------------------------------------------------------
// TokenEntry / TokenValue — runtime values for `Tokenized<T>` references
// ----------------------------------------------------------------------------

/// A single token entry — name plus concrete value. The backend
/// translates the value to its variable system (e.g. CSS
/// `--{name}: {value}`).
#[derive(Clone, Debug)]
pub struct TokenEntry {
    pub name: &'static str,
    pub value: TokenValue,
}

/// The concrete value carried by a token. The variant determines how
/// the backend formats it (color string, pixel length, raw number).
#[derive(Clone, Debug)]
pub enum TokenValue {
    Color(Color),
    Length(Length),
    Number(f32),
}

// ----------------------------------------------------------------------------
// Global token state & resolution cache
// ----------------------------------------------------------------------------

thread_local! {
    /// Per-token reactive registry. Each token name maps to a
    /// `Signal<TokenValue>` carrying the current value. `install_tokens`
    /// creates entries; `update_tokens` calls `.set(..)` on existing
    /// entries (creating them if missing). `Tokenized::<T>::resolve()`
    /// reads from here so each styled effect subscribes ONLY to the
    /// token signals it actually reads — `update_tokens(["a"])` wakes
    /// nodes that reference `"a"` and leaves the rest alone.
    ///
    /// Signals are created lazily-on-first-touch when called from
    /// outside an `install_tokens` call (e.g. `resolve()` reaches a
    /// token that hasn't been installed yet). That keeps subscriptions
    /// consistent across install order — the same `Signal` exists
    /// whether install happens before or after the first resolve.
    static TOKEN_REGISTRY: RefCell<HashMap<&'static str, crate::Signal<TokenValue>>> =
        RefCell::new(HashMap::new());

    /// Memoization: `(stylesheet pointer, variants, override content)`
    /// → `Rc<StyleRules>`. Strong refs are held by `REGISTRATIONS`
    /// for pre-generated styles, and transiently by the caller of
    /// `resolve(...)` for dynamic ones.
    ///
    /// Tokenized fields hash by token name (token-stable), so the same
    /// `(sheet, variants)` produces the same key regardless of which
    /// token values are currently installed. Token updates don't
    /// invalidate this cache — they update the backend's variable
    /// layer (web) and re-fire styled effects (mobile) so the cached
    /// rules are re-applied with the new fallbacks.
    static RESOLUTION_CACHE: RefCell<HashMap<ResolutionKey, Rc<StyleRules>>> =
        RefCell::new(HashMap::new());

    /// Each currently-registered stylesheet, with the rules that were
    /// pre-generated for it and a `Weak<StyleSheet>` used to detect
    /// when the stylesheet has been dropped by all holders. The
    /// framework calls `Backend::register_stylesheet` exactly once per
    /// sheet and tracks the rules so we can later call
    /// `unregister_stylesheet` to free backend-side state.
    static REGISTRATIONS: RefCell<HashMap<RegKey, Registration>> =
        RefCell::new(HashMap::new());

    /// Rule sets queued for `unregister_stylesheet` calls. Populated
    /// by the sweep-dead-stylesheets pass. Drained by
    /// `ensure_registered_with`, which has the backend in scope.
    static PENDING_UNREGISTER: RefCell<Vec<Vec<Rc<StyleRules>>>> =
        RefCell::new(Vec::new());

    /// Tokens queued for the next backend interaction. `install_tokens`
    /// pushes here; `ensure_registered_with` flushes via
    /// `Backend::install_tokens`. We can't call the backend directly
    /// from `install_tokens` because the backend doesn't exist yet at
    /// app boot.
    static PENDING_TOKENS: RefCell<Option<Vec<TokenEntry>>> =
        const { RefCell::new(None) };

    /// Token updates queued for the next backend interaction. Each
    /// `update_tokens` call appends here; `ensure_registered_with`
    /// drains and dispatches via `Backend::update_tokens`. Unlike
    /// `PENDING_TOKENS`, updates accumulate — multiple updates in a
    /// frame all reach the backend.
    static PENDING_TOKEN_UPDATES: RefCell<Vec<Vec<TokenEntry>>> =
        RefCell::new(Vec::new());

    /// Latest host-surface background queued for `Backend::set_app_background`.
    /// `set_app_background` pushes; `ensure_registered_with` flushes.
    /// Single slot (latest wins) because the host has exactly one
    /// background and re-applying intermediate values would just churn.
    static PENDING_APP_BG: RefCell<Option<Tokenized<Color>>> =
        const { RefCell::new(None) };

    /// Latest scrollbar theme (thumb, track) queued for
    /// `Backend::set_scrollbar_theme`. Same single-slot rule as
    /// [`PENDING_APP_BG`].
    static PENDING_SCROLLBAR: RefCell<Option<(Tokenized<Color>, Tokenized<Color>)>> =
        const { RefCell::new(None) };

    /// Latest app-level key handler queued for `Backend::set_app_key_handler`.
    /// Outer `Option` = "a `set_app_key_handler` call happened this cycle, drain
    /// it"; inner = the handler (`Some` installs, `None` clears). Single slot
    /// (latest wins) — there is exactly one app-level handler.
    static PENDING_APP_KEY_HANDLER:
        RefCell<Option<Option<crate::primitives::key::KeyDownHandler>>> =
        const { RefCell::new(None) };

    /// Typefaces already registered with the backend this session.
    /// Drives the dedup in [`ensure_typefaces_registered_with`]: the
    /// framework calls `register_asset` + `register_typeface` once
    /// per unique `TypefaceId` no matter how many stylesheets — or
    /// rules within a stylesheet — reference the same typeface.
    static REGISTERED_TYPEFACES: RefCell<HashSet<TypefaceId>> =
        RefCell::new(HashSet::new());

    /// Debug-only: the `family_name` of every typeface registered this
    /// session (populated alongside [`REGISTERED_TYPEFACES`] in
    /// [`ensure_typefaces_registered_with`]). Used by
    /// [`maybe_warn_unregistered_system_font`] to tell whether a bare
    /// `FontFamily::System(name)` matched a `typeface!` family the
    /// author then deleted — the string path carries no compile-time
    /// link, so that deletion is otherwise silent (text falls back to
    /// the OS generic, usually serif). Names are `&'static str` because
    /// `Typeface::family_name` is always a string literal.
    ///
    /// **Why debug-only.** This is a dev-time DX guardrail with no
    /// runtime behavior — it must be stripped from release builds
    /// (CLAUDE.md §7: dev markers live behind `#[cfg(debug_assertions)]`,
    /// not a runtime predicate). The whole machinery compiles out when
    /// `debug_assertions` is off.
    #[cfg(debug_assertions)]
    static REGISTERED_FAMILY_NAMES: RefCell<HashSet<&'static str>> =
        RefCell::new(HashSet::new());

    /// Debug-only dedup for [`maybe_warn_unregistered_system_font`]:
    /// each suspicious `System(name)` warns exactly once per thread, so
    /// a stylesheet applied to thousands of nodes doesn't spam the log.
    #[cfg(debug_assertions)]
    static WARNED_SYSTEM_FONTS: RefCell<HashSet<String>> =
        RefCell::new(HashSet::new());

    /// Debug-only dedup for the "resolve on an unthemed thread while
    /// *another* thread is themed" warning (see
    /// [`debug_warn_resolve_on_unthemed_thread`]). One warning per
    /// thread, not one per token, so a stylesheet applied to thousands
    /// of nodes doesn't spam the log.
    #[cfg(debug_assertions)]
    static WARNED_UNTHEMED_RESOLVE: Cell<bool> = const { Cell::new(false) };

    /// Tripwire-support flag: `true` once `install_tokens` (or
    /// `update_tokens`) has been called on this thread. Read in
    /// [`debug_warn_resolve_on_unthemed_thread`] (the
    /// `Tokenized::<T>::resolve()` path for `Tokenized::Token`).
    ///
    /// **Why thread-local.** The token registry above is itself
    /// thread-local — every supported backend today renders on a
    /// single thread, and the registry, resolution cache, and signal
    /// state all live on that thread. A render thread that hasn't
    /// installed tokens falls back to `Tokenized::fallback` and misses
    /// every theme value.
    ///
    /// **Why a separate flag and not "registry non-empty".** The
    /// registry can be non-empty on this thread because *some other
    /// code path* (e.g. `with_or_create_token_signal` from a prior
    /// resolve) lazily inserted a slot. That's not a theme install.
    /// The flag tracks the explicit theme-install event, not registry
    /// shape.
    static THEME_INSTALLED: Cell<bool> = const { Cell::new(false) };
}

/// Process-global companion to the thread-local [`THEME_INSTALLED`]:
/// `true` once *any* thread has installed a theme. Lets
/// [`debug_warn_resolve_on_unthemed_thread`] distinguish the two cases
/// it must treat differently:
///
/// - **No theme installed anywhere** → an app that styles entirely
///   with literal values (or just leans on primitive default tokens)
///   and never calls `install_theme`. Resolving to the embedded
///   `Tokenized::fallback` is exactly what we want, and it's exactly
///   what the web backend already does (`var(--name, fallback)` with
///   no `:root` definition). Stay silent — native must match web here
///   (CLAUDE.md §7).
/// - **A theme exists, but not on this thread** → the genuine
///   cross-thread footgun. The resolve silently misses every theme
///   value. Warn (debug only) so it's visible.
static ANY_THEME_INSTALLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Mark this thread as having an installed theme, and record globally
/// that *some* thread is now themed. Idempotent. `install_tokens` /
/// `update_tokens` call this so [`debug_warn_resolve_on_unthemed_thread`]
/// can distinguish a genuinely-unthemed thread from one that just
/// hasn't lazily registered every individual token signal yet.
#[inline]
fn mark_theme_installed() {
    THEME_INSTALLED.with(|f| f.set(true));
    ANY_THEME_INSTALLED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Debug-only dev guardrail for `Tokenized::<T>::resolve()` on the
/// `Tokenized::Token` path. **Never panics, never changes behavior** —
/// resolving without a theme always falls back to the embedded
/// `Tokenized::fallback`, on every backend, in every build.
///
/// It only emits a one-time warning for the genuine misuse: a token
/// resolved on a thread with no installed theme *while another thread
/// is themed*. That's the cross-thread footgun — the resolve silently
/// misses real theme values. When no theme exists anywhere (a
/// deliberately theme-less app), it stays silent so native matches the
/// web backend's silent `var(--name, fallback)` behavior.
///
/// `Tokenized::Literal` resolves never reach here — literals don't read
/// the registry and need no theme. In release builds the whole body
/// compiles out.
#[inline]
fn debug_warn_resolve_on_unthemed_thread(_token_name: &'static str) {
    #[cfg(debug_assertions)]
    {
        // This thread is themed → nothing to warn about.
        if THEME_INSTALLED.with(|f| f.get()) {
            return;
        }
        // No theme anywhere → benign, web-parity fallback. Stay silent.
        if !ANY_THEME_INSTALLED.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        // A theme exists on another thread but not here: the genuine
        // cross-thread footgun. Warn once per thread.
        let first = WARNED_UNTHEMED_RESOLVE.with(|c| {
            let was = c.get();
            c.set(true);
            !was
        });
        if first {
            eprintln!(
                "[runtime-core] token '{}' resolved on thread '{}', which has no \
                 installed theme, but another thread does — this resolve falls back \
                 to the literal default and misses every theme value. Call \
                 `runtime_core::style::install_tokens(...)` on this thread, or move \
                 the resolve to the host render thread.",
                _token_name,
                std::thread::current().name().unwrap_or("<unnamed>")
            );
        }
    }
}

#[derive(PartialEq, Eq, Hash, Clone)]
struct RegKey {
    sheet: *const StyleSheet,
}

struct Registration {
    weak: std::rc::Weak<StyleSheet>,
    rules: Vec<Rc<StyleRules>>,
}

#[derive(PartialEq, Eq, Hash)]
struct ResolutionKey {
    sheet: *const StyleSheet,
    variants: VariantSet,
    /// The computed layer's caller-supplied key, or empty string when
    /// no computed layer is attached. Two `StyleApplication`s with the
    /// same `(sheet, variants, computed_key, overrides)` reuse the
    /// cached `Rc<StyleRules>`; differing `computed_key`s produce
    /// distinct cache entries so the closure runs once per unique
    /// modifier set per theme.
    computed_key: String,
    /// Overrides are part of the cache key — same sheet + variants
    /// but different override values yield different rules and must
    /// be cached separately. Serialized to a content key so we have a
    /// comparable form.
    overrides: String,
}

/// Look up the registry signal for `name`, or create one with
/// `make_initial()` if no entry exists. Returns `None` only if signal
/// creation panics inside a no-Owner context — but we rely on the
/// caller's `Owner` to keep slots alive. In practice this always
/// returns `Some`.
///
/// Used by both `Tokenized::<T>::resolve()` (lazy create on first
/// touch so subscriptions are consistent regardless of install order)
/// and by `install_tokens` / `update_tokens` (eager create on install).
/// Read every currently-registered token signal so the calling
/// `Effect` subscribes to all of them. Used by the theme-cohort
/// driver in `walker.rs` to ensure the driver re-fires on *any*
/// `update_tokens` call — even before any cohort entries have
/// registered (the driver's first iteration runs against an empty
/// slab, so it'd otherwise touch no signals and subscribe to
/// nothing).
///
/// Tokens added *after* this call still trigger the driver
/// indirectly: cohort entries that read them via `Tokenized::resolve()`
/// subscribe inside their reapply closures, so the driver picks
/// up the new dependency on its next re-run.
pub(crate) fn subscribe_to_all_token_signals() {
    TOKEN_REGISTRY.with(|r| {
        for sig in r.borrow().values() {
            let _ = sig.get();
        }
    });
}

fn with_or_create_token_signal<F>(
    name: &'static str,
    make_initial: F,
) -> Option<crate::Signal<TokenValue>>
where
    F: FnOnce() -> TokenValue,
{
    // Fast path: existing entry. Done in a separate scope so the
    // borrow is dropped before we possibly mutate below.
    let existing = TOKEN_REGISTRY.with(|r| r.borrow().get(name).copied());
    if existing.is_some() {
        return existing;
    }
    // Miss — create. Token signals are thread-lifetime by contract
    // (`TOKEN_REGISTRY` is a process-wide thread-local), so the signal
    // must NOT be adopted by whatever render scope happens to be
    // active when the first read lands. `crate::reactive::unscope`
    // temporarily empties the active-scope stack while we allocate,
    // so the resulting slot has no owner and is freed only on thread
    // exit — exactly the lifetime the registry needs.
    //
    // Regression: before this guard, the first scope to resolve an
    // uninstalled token became its owner; when that scope dropped, the
    // registry still pointed at a freed slot and subsequent resolves
    // panicked ("signal used after its scope was dropped") or, after
    // freelist recycling, silently hit unrelated signal data.
    let sig = crate::reactive::unscope(|| crate::Signal::new(make_initial()));
    TOKEN_REGISTRY.with(|r| {
        r.borrow_mut().insert(name, sig);
    });
    Some(sig)
}

/// Debug-only warning when a token's installed `TokenValue` variant
/// doesn't match the `Tokenized<T>` reading it. Indicates a theme bug
/// — silently returning the fallback would mask it.
fn debug_warn_token_type_mismatch(
    name: &'static str,
    expected: &str,
    got: &TokenValue,
) {
    #[cfg(debug_assertions)]
    {
        let got_label = match got {
            TokenValue::Color(_) => "Color",
            TokenValue::Length(_) => "Length",
            TokenValue::Number(_) => "Number",
        };
        eprintln!(
            "[runtime-core] token '{}' resolved as {} but installed as {} — using fallback",
            name, expected, got_label
        );
    }
    let _ = (name, expected, got);
}

/// Push the initial token set. Call once at app startup before
/// rendering. Creates a `Signal<TokenValue>` in the registry for each
/// token so subsequent `Tokenized::<T>::resolve()` reads can subscribe.
/// Tokens are also queued and flushed to the backend via
/// `Backend::install_tokens` on the first `ensure_registered_with`
/// call (which has the backend in scope).
pub fn install_tokens(tokens: &[TokenEntry]) {
    // Tripwire for the debug-only "resolve on unthemed thread" check.
    // Set unconditionally — the cost is a single thread-local store
    // and idempotency is fine.
    mark_theme_installed();
    // Seed the per-token registry. If a token name was already
    // registered (re-install — e.g. tests calling `install_theme`
    // multiple times), update the existing signal instead of leaking
    // a fresh slot.
    for entry in tokens {
        let installed = TOKEN_REGISTRY.with(|r| r.borrow().get(entry.name).copied());
        match installed {
            Some(sig) => sig.set(entry.value.clone()),
            None => {
                let _ = with_or_create_token_signal(entry.name, || entry.value.clone());
            }
        }
    }
    let owned: Vec<TokenEntry> = tokens.to_vec();
    PENDING_TOKENS.with(|p| *p.borrow_mut() = Some(owned));
}

/// Push new token values. For each entry, calls `.set(..)` on the
/// existing `Signal<TokenValue>` in the registry (creates one if the
/// caller skipped `install_tokens` for that name — permissive). Only
/// the signals for the names in `tokens` fire, so styled effects that
/// subscribed via `Tokenized::<T>::resolve()` only re-run if they
/// referenced one of these tokens.
///
/// Pushes deltas to the backend on the next `ensure_registered_with`
/// flush. Also wipes the framework's resolution cache so subsequent
/// resolves see fresh `Rc<StyleRules>` (token names are stable, so
/// the cache shape doesn't change — but content keys hash by name so
/// the wipe is the simplest way to keep cached rules in sync with
/// fresh fallback values).
pub fn update_tokens(tokens: &[TokenEntry]) {
    // Tripwire for the debug-only "resolve on unthemed thread" check.
    // `update_tokens` is the permissive partner to `install_tokens` —
    // a thread that has only ever called `update_tokens` is still a
    // themed thread.
    mark_theme_installed();
    // Stash the pending update + clear the resolution cache BEFORE
    // firing any signal subscribers. The theme-cohort driver `Effect`
    // (subscribed via `subscribe_to_all_token_signals`) re-runs
    // synchronously the moment we `sig.set` on the first token, and
    // its body calls `take_pending_token_updates()` to flush new
    // `:root` variables to the backend. If we did the push AFTER the
    // fires, the cohort driver would see an EMPTY queue on this
    // call — and end up flushing this theme's tokens on the *next*
    // `set_theme` invocation, with a visible one-toggle delay (after
    // `setTheme('dark')` the page still renders light; after the
    // subsequent `setTheme('light')` it renders dark; etc.). The
    // toggle suite catches this; the L→D→L verify trips because the
    // light update never landed in the DOM.
    let owned: Vec<TokenEntry> = tokens.to_vec();
    PENDING_TOKEN_UPDATES.with(|p| p.borrow_mut().push(owned));
    RESOLUTION_CACHE.with(|c| c.borrow_mut().clear());

    // Wrap the per-token signal fires in `batch(...)` so each Effect
    // subscribed to multiple tokens re-runs ONCE at the end rather
    // than once per token. A theme switch typically writes ~50 tokens
    // and a styled Effect reads 2–5 of them; without batching the
    // same Effect re-runs 2–5 times in sequence, each redoing the
    // full `apply_style` work (msg_send'ing every property on the
    // view, scheduling animators). On a docs-sized tree (490 views,
    // hundreds of effects) that's the difference between a snappy
    // theme toggle and one that visibly hangs the main thread for
    // hundreds of ms.
    crate::reactive::batch(|| {
        for entry in tokens {
            let existing = TOKEN_REGISTRY.with(|r| r.borrow().get(entry.name).copied());
            match existing {
                Some(sig) => sig.set(entry.value.clone()),
                None => {
                    // Permissive: register a fresh signal for tokens that
                    // were updated before being installed.
                    let _ = with_or_create_token_signal(entry.name, || entry.value.clone());
                }
            }
        }
    });
}

/// Drain the queue of pending token-update batches. Used by the
/// theme-cohort driver when fan-out is short-circuited (cascade
/// backends) so the queue gets flushed even when no `apply_one`
/// runs.
pub fn take_pending_token_updates() -> Vec<Vec<TokenEntry>> {
    PENDING_TOKEN_UPDATES.with(|p| std::mem::take(&mut *p.borrow_mut()))
}

/// Theme the host surface behind the framework's rendered tree
/// (`<body>` on web, `UIWindow` on iOS, etc.). Routes through
/// [`Backend::set_app_background`] on the next walker pass. The
/// argument is a [`Tokenized<Color>`] so backends with a CSS-variable
/// surface can wire the host to `var(--<name>)` and stay reactive
/// across `update_tokens` calls without a second invocation here.
///
/// Single-slot: a second call before the next flush replaces the
/// first. The theme SDK calls this at `install_theme` time and on
/// `set_theme` swap (so non-web backends, which apply the resolved
/// value directly, re-resolve).
pub fn set_app_background(color: Tokenized<Color>) {
    PENDING_APP_BG.with(|p| *p.borrow_mut() = Some(color));
}

/// Theme the platform scrollbar where the backend supports it.
/// Same single-slot, next-flush semantics as [`set_app_background`].
/// Default no-op on most backends — only web/SSR honor it today.
pub fn set_scrollbar_theme(thumb: Tokenized<Color>, track: Tokenized<Color>) {
    PENDING_SCROLLBAR.with(|p| *p.borrow_mut() = Some((thumb, track)));
}

/// Install (or, with `None`, remove) an APP-LEVEL keyboard handler that fires on
/// every key press regardless of focus. Routes through
/// [`Backend::set_app_key_handler`](crate::Backend::set_app_key_handler) on the
/// next walker flush. Single-slot: a second call before the flush replaces the
/// first (`Some(handler)` installs, `None` clears). Backends without an
/// app-level key source ignore it.
///
/// Call once near app start, e.g.
/// `set_app_key_handler(Some(Rc::new(|e| { /* … */ KeyOutcome::Default })))`.
/// The handler sees EVERY key (including typing into a focused input), so act
/// only on the keys you care about and return `KeyOutcome::Default` otherwise.
pub fn set_app_key_handler(handler: Option<crate::primitives::key::KeyDownHandler>) {
    // Born batched — every key the backend delivers runs the handler as one
    // reactive cycle, so signal writes inside it coalesce. See `reactive::cycle`.
    let handler = handler.map(|h| {
        std::rc::Rc::new(move |e: &crate::primitives::key::KeyEvent| {
            crate::cycle(|| h(e))
        }) as crate::primitives::key::KeyDownHandler
    });
    PENDING_APP_KEY_HANDLER.with(|p| *p.borrow_mut() = Some(handler));
}


/// Ensures the backend has been asked to pre-generate state for this
/// stylesheet against the active theme. Calls `register` with the
/// resolved rules exactly once per `(sheet, theme)` pair.
///
/// Also opportunistically:
/// - Flushes the pending-unregister queue, calling `unregister` for
///   each rule set queued by `set_theme` or a dead-stylesheet sweep.
/// - Flushes the pending-tokens queue, calling `install_tokens` with
///   the most recent theme's token list (if any was queued by
///   `install_theme` / `set_theme`).
/// Walk `rules` for `FontFamily::Typeface` references and, for any
/// typeface not yet observed this session, emit `register_asset` for
/// each face's asset followed by `register_typeface` for the family.
///
/// Called by the framework before [`ensure_registered_with`] hands the
/// rules to the backend — every `apply_style` that references a
/// typeface is guaranteed to find the family already registered.
///
/// Dedup is session-wide (thread-local) and keyed by [`TypefaceId`].
/// Backends do their own dedup as a safety net (see
/// `WebBackend::impl_register_typeface` and the `@font-face` rule
/// table), but the framework-side short-circuit keeps the hot path
/// off the backend round-trip.
pub fn ensure_typefaces_registered_with<RA, RT>(
    rules: &[Rc<StyleRules>],
    mut register_asset: RA,
    mut register_typeface: RT,
) where
    RA: FnMut(crate::assets::AssetId, crate::assets::AssetTag, &crate::assets::AssetSource),
    RT: FnMut(
        TypefaceId,
        &'static str,
        &'static [crate::assets::TypefaceFace],
        crate::assets::SystemFallback,
    ),
{
    // Walk rules in order; collect unseen typefaces. We don't
    // deduplicate the per-rules walk itself — typically `rules` has a
    // handful of entries and any typeface is the same `Typeface` value
    // across all variants of a stylesheet — so the hot path is the
    // thread-local set's O(1) miss check.
    REGISTERED_TYPEFACES.with(|set| {
        let mut set = set.borrow_mut();
        for r in rules {
            if let Some(FontFamily::Typeface(tf)) = &r.font_family {
                if set.insert(tf.id) {
                    for face in tf.faces {
                        register_asset(
                            face.asset,
                            crate::assets::AssetTag::Font,
                            &face.source,
                        );
                    }
                    register_typeface(tf.id, tf.family_name, tf.faces, tf.fallback);
                    // Debug-only: remember the registered family name so a
                    // sibling `FontFamily::System("<family_name>")` resolves
                    // as "known" rather than tripping the deleted-typeface
                    // warning. No-op in release.
                    #[cfg(debug_assertions)]
                    REGISTERED_FAMILY_NAMES
                        .with(|n| n.borrow_mut().insert(tf.family_name));
                }
            }
        }
    });
}

/// Known generic / system family names that a bare
/// `FontFamily::System(name)` may legitimately carry without any
/// `typeface!` registration. CSS generics plus the common platform
/// system-UI aliases. Compared case-insensitively against the bare
/// (single-token) name.
///
/// **Why these specific names.** The CSS generic families
/// (`sans-serif`, `serif`, `monospace`, `cursive`, `fantasy`,
/// `system-ui`, `ui-*`, `math`, `emoji`) plus the de-facto system-font
/// aliases every platform recognizes (`-apple-system`,
/// `BlinkMacSystemFont`, `Segoe UI`, `Roboto`, `Helvetica`,
/// `Helvetica Neue`, `Arial`). A `System(name)` matching one of these
/// is intentional and resolves to a real OS font — never a deleted
/// `typeface!`.
#[cfg(debug_assertions)]
const KNOWN_SYSTEM_FAMILIES: &[&str] = &[
    "sans-serif",
    "serif",
    "monospace",
    "cursive",
    "fantasy",
    "system-ui",
    "ui-sans-serif",
    "ui-serif",
    "ui-monospace",
    "ui-rounded",
    "math",
    "emoji",
    "fangsong",
    "-apple-system",
    "blinkmacsystemfont",
    "segoe ui",
    "roboto",
    "helvetica",
    "helvetica neue",
    "arial",
];

/// Pure decision for the deleted-`typeface!` DX warning.
///
/// Returns `true` iff `name` is a *bare* family name that matches
/// neither a registered typeface family nor a known system/generic
/// family — i.e. it looks like an author wrote `font_family: "Inter"`
/// against a `typeface!` they later removed, and the text will now
/// fall back to the platform default.
///
/// Conservative by construction, to avoid false-positive spam:
///
/// - **Comma stacks short-circuit to `false`.** `"Inter, sans-serif"`
///   is an explicit, intentional multi-family fallback — the author
///   already provided a generic tail, so there's nothing to warn about
///   even if `Inter` isn't registered. We only flag a *single bare*
///   token that reads like it was meant to resolve to one registered
///   face.
/// - **Generic / system families are never flagged** (see
///   [`KNOWN_SYSTEM_FAMILIES`]), matched case-insensitively.
/// - **Registered families are never flagged**, matched exactly
///   against the `typeface!`-declared `family_name`.
/// - Empty / whitespace-only names are ignored (nothing actionable).
///
/// This is a free function over its inputs (no thread-locals) so it can
/// be unit-tested deterministically; the thread-local registry +
/// one-time dedup live in [`maybe_warn_unregistered_system_font`].
#[cfg(debug_assertions)]
pub(crate) fn should_warn_for_system_font(
    name: &str,
    registered: &HashSet<&'static str>,
) -> bool {
    let trimmed = name.trim();
    // A comma stack is an intentional fallback list — not a bare face.
    if trimmed.is_empty() || trimmed.contains(',') {
        return false;
    }
    // Registered typeface family (exact match — that's the key the
    // backend resolves against).
    if registered.contains(trimmed) {
        return false;
    }
    // Quoted family names (`"Inter"`) are still bare; strip surrounding
    // quotes before the generic check so e.g. `"sans-serif"` isn't
    // mis-flagged. (Authors rarely quote, but the macro `From<&str>`
    // preserves whatever they wrote.)
    let unquoted = trimmed.trim_matches(|c| c == '"' || c == '\'');
    if registered.contains(unquoted) {
        return false;
    }
    let lowered = unquoted.to_ascii_lowercase();
    if KNOWN_SYSTEM_FAMILIES.contains(&lowered.as_str()) {
        return false;
    }
    true
}

/// Debug-only: emit a one-time, actionable warning when a
/// `FontFamily::System(name)` resolves to a bare family that is neither
/// a registered `typeface!` nor a known system font. See CLAUDE.md §7
/// (dev-only marker) and [`should_warn_for_system_font`] for the
/// decision and why it's deliberately conservative.
///
/// **Why warn at apply time, not at registration.** Typefaces register
/// lazily — the first time any stylesheet that references one is
/// applied. A bare `System(name)` matching a typeface that lives in a
/// *different* stylesheet is only knowable after that other sheet has
/// also been applied. Checking here (after the node's own sheet
/// registered) catches the overwhelmingly common case (the typeface and
/// the string live in the same theme, registered together) while the
/// one-time dedup keeps a rare cross-sheet ordering miss to a single
/// spurious line rather than a flood. The check is free in release
/// (whole function compiles out).
#[cfg(debug_assertions)]
pub(crate) fn maybe_warn_unregistered_system_font(name: &str) {
    let suspicious = REGISTERED_FAMILY_NAMES
        .with(|reg| should_warn_for_system_font(name, &reg.borrow()));
    if !suspicious {
        return;
    }
    // De-dupe: warn once per distinct name per thread.
    let first_time =
        WARNED_SYSTEM_FONTS.with(|seen| seen.borrow_mut().insert(name.to_string()));
    if first_time {
        eprintln!(
            "[idealyst] font_family {:?} matches no registered typeface and no \
             known system font; text will fall back to the platform default. \
             Did you remove a typeface! registration?",
            name
        );
    }
}

/// Reset the framework's session-wide registration dedup so the NEXT
/// `ensure_registered_with` call republishes everything to a fresh
/// backend. The SSG driver in `backend-ssr` (`render_all`) calls this
/// between iterations — each page render uses a fresh `SsrBackend`
/// instance, but the dedup thread-locals were designed assuming "one
/// app session = one backend forever," so a second render would
/// otherwise short-circuit and the new backend would miss every
/// stylesheet registration + typeface registration.
///
/// Cleared:
/// - `REGISTRATIONS` (stylesheet → backend-side state) so
///   `register_stylesheet` fires on the next backend.
/// - `REGISTERED_TYPEFACES` (per-`TypefaceId` dedup) so
///   `register_asset` + `register_typeface` fire on the next backend
///   (otherwise the new backend's `head_css` has no `@font-face`).
/// - `RESOLUTION_CACHE` (memoized `Rc<StyleRules>`) — the cache holds
///   `Rc`s the OLD backend was supposed to dedup against; the next
///   `ensure_registered_with` will repopulate with fresh `Rc`s.
/// - `PENDING_UNREGISTER` + `PENDING_TOKEN_UPDATES` — stale queues
///   from the previous render that the fresh backend should not see.
///
/// Not cleared: `TOKEN_REGISTRY` (token `Signal`s have global lifetime
/// and the same names resolve to the same signals across renders).
pub fn reset_for_ssg_render() {
    REGISTRATIONS.with(|r| r.borrow_mut().clear());
    REGISTERED_TYPEFACES.with(|s| s.borrow_mut().clear());
    RESOLUTION_CACHE.with(|c| c.borrow_mut().clear());
    PENDING_UNREGISTER.with(|p| p.borrow_mut().clear());
    PENDING_TOKEN_UPDATES.with(|p| p.borrow_mut().clear());
    // Debug-only registries follow the same per-render lifecycle as
    // REGISTERED_TYPEFACES: a fresh backend must re-observe the
    // typefaces, so the family-name set (and its one-time warning
    // dedup) reset too — otherwise the second render would suppress a
    // genuinely-missing-family warning, or carry a stale dedup.
    #[cfg(debug_assertions)]
    REGISTERED_FAMILY_NAMES.with(|s| s.borrow_mut().clear());
    #[cfg(debug_assertions)]
    WARNED_SYSTEM_FONTS.with(|s| s.borrow_mut().clear());
}

/// Pointer-keyed peek at the registration table — `true` iff a live
/// registration exists for this exact `StyleSheet` instance (compared
/// by `Rc` pointer, not content).
///
/// This is the cheap fast-path the batched-Repeat walker uses to skip
/// the full [`ensure_registered_with`] call after the sheet's first
/// row in a build. The full function ALWAYS flushes pending-token
/// queues + sweeps dead `Weak<StyleSheet>` registrations before its
/// own `already-registered` early-return; that's correct but
/// per-row-expensive when N rows share one sheet. The walker can
/// safely skip when this returns `true` because:
///   - registrations don't change mid-build (no one writes
///     `register_stylesheet` from inside `enqueue_primitive`), and
///   - any pending-token flushing the first call did is still in
///     effect for the remaining rows.
pub fn is_registered(sheet: &Rc<StyleSheet>) -> bool {
    let key = RegKey { sheet: Rc::as_ptr(sheet) };
    REGISTRATIONS.with(|r| r.borrow().contains_key(&key))
}

/// - Sweeps registrations whose `Weak<StyleSheet>` no longer upgrades
///   into the pending-unregister queue.
pub fn ensure_registered_with<R, U, I, UPD, RA, RT, SAB, SST, SAK>(
    sheet: &Rc<StyleSheet>,
    register: R,
    unregister: U,
    install_tokens: I,
    update_tokens: UPD,
    register_asset: RA,
    register_typeface: RT,
    set_app_background: SAB,
    set_scrollbar_theme: SST,
    set_app_key_handler: SAK,
) where
    R: FnOnce(&[Rc<StyleRules>]),
    U: Fn(&[Rc<StyleRules>]),
    I: FnOnce(&[TokenEntry]),
    UPD: FnMut(&[TokenEntry]),
    RA: FnMut(crate::assets::AssetId, crate::assets::AssetTag, &crate::assets::AssetSource),
    RT: FnMut(
        TypefaceId,
        &'static str,
        &'static [crate::assets::TypefaceFace],
        crate::assets::SystemFallback,
    ),
    SAB: FnOnce(&Tokenized<Color>),
    SST: FnOnce(&Tokenized<Color>, &Tokenized<Color>),
    SAK: FnOnce(Option<crate::primitives::key::KeyDownHandler>),
{
    // Flush pending tokens first — backends that emit `var(--…)` need
    // the variables installed before any rule that references them
    // is parsed, otherwise the initial paint uses the fallback.
    let pending_tokens = PENDING_TOKENS.with(|p| p.borrow_mut().take());
    if let Some(tokens) = pending_tokens {
        install_tokens(&tokens);
    }

    // Flush any pending token updates. These accumulate across all
    // `update_tokens` calls between walker passes.
    let pending_updates: Vec<Vec<TokenEntry>> =
        PENDING_TOKEN_UPDATES.with(|p| std::mem::take(&mut *p.borrow_mut()));
    let mut update_tokens = update_tokens;
    for upd in &pending_updates {
        update_tokens(upd);
    }

    // Flush queued host-surface settings. Same "the backend's in
    // scope now — sync queued user state to it" intent as the token
    // flush above; placed AFTER tokens so a backend that emits
    // `body { background: var(--<name>); }` can rely on the var
    // already being defined on `:root` when the body rule installs.
    if let Some(c) = PENDING_APP_BG.with(|p| p.borrow_mut().take()) {
        set_app_background(&c);
    }
    if let Some((thumb, track)) = PENDING_SCROLLBAR.with(|p| p.borrow_mut().take()) {
        set_scrollbar_theme(&thumb, &track);
    }
    // Drain the queued app-level key handler (outer Some = a call happened;
    // inner Some installs, None clears). Single-slot, like the host bg above.
    if let Some(handler) = PENDING_APP_KEY_HANDLER.with(|p| p.borrow_mut().take()) {
        set_app_key_handler(handler);
    }

    let sheet_ptr = Rc::as_ptr(sheet);
    let key = RegKey { sheet: sheet_ptr };

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

    // Register fresh. We pre-populate the resolution cache with the
    // pregen Rcs so `resolve()` for a known (sheet, variants,
    // no-overrides) combination returns the *same Rc instance* the
    // backend just registered. That lets the backend short-circuit
    // on `Rc::as_ptr` identity instead of paying for `content_key()`
    // on every node.
    let keyed = pregenerate_keyed(sheet);
    let sheet_ptr = Rc::as_ptr(sheet);
    RESOLUTION_CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        for (variants, rc) in &keyed {
            let cache_key = ResolutionKey {
                sheet: sheet_ptr,
                variants: variants.clone(),
                computed_key: String::new(),
                overrides: String::new(),
            };
            cache.insert(cache_key, rc.clone());
        }
    });
    // Also populate the per-sheet pointer-keyed cache. This is the
    // fast path `resolve()` consults first.
    for (variants, rc) in &keyed {
        sheet.insert_variant(variants.clone(), rc.clone());
    }
    let rules: Vec<Rc<StyleRules>> = keyed.into_iter().map(|(_, rc)| rc).collect();
    // Register any typefaces (and their per-face assets) the sheet
    // references before shipping the stylesheet itself.
    ensure_typefaces_registered_with(&rules, register_asset, register_typeface);
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

/// Returns the set of pre-resolvable `StyleRules` for a stylesheet.
/// Includes:
/// - The base rules (no variants active).
/// - One entry per declared (axis, value) — variant overlay layered on
///   base.
/// - One entry per declared compound variant — the matched compound
///   layered on the base + the compound's `when` clause's variants.
///
/// Continuous overrides are NOT pre-generatable and aren't included.
/// Backends like the web backend use this to mint CSS classes ahead of
/// time so `apply_style` is a cache hit.
pub fn pregenerate(sheet: &StyleSheet) -> Vec<Rc<StyleRules>> {
    pregenerate_keyed(sheet)
        .into_iter()
        .map(|(_, rc)| rc)
        .collect()
}

/// Same as `pregenerate` but also returns the `VariantSet` each rule
/// was resolved for. Used by `ensure_registered_with` to populate the
/// resolution cache so `resolve()` returns the *same* `Rc<StyleRules>`
/// instances the backend registered.
pub(crate) fn pregenerate_keyed(sheet: &StyleSheet) -> Vec<(VariantSet, Rc<StyleRules>)> {
    let mut out: Vec<(VariantSet, Rc<StyleRules>)> = Vec::new();

    // 1. Base.
    let base_vs = VariantSet::new();
    out.push((base_vs.clone(), Rc::new(sheet.resolve(&base_vs))));

    // 2. Each (axis, value) — every single-axis variant selection.
    for (axis, value) in sheet.variant_keys() {
        let variants = VariantSet::new().with(axis, value);
        out.push((variants.clone(), Rc::new(sheet.resolve(&variants))));
    }

    // 3. Each compound — the compound's `when` clause defines the
    //    minimum variant selection that triggers it.
    for compound_keys in sheet.compound_keys() {
        let mut variants = VariantSet::new();
        for (axis, value) in compound_keys {
            variants.0.insert(axis, value);
        }
        out.push((variants.clone(), Rc::new(sheet.resolve(&variants))));
    }

    out
}

/// Resolve a style application. Memoized: same key always returns
/// the same `Rc<StyleRules>` across calls until the cache is wiped
/// (by [`update_tokens`]) or pruned (stylesheet dropped).
///
/// Cache entries are strong `Rc`s — that's what makes back-to-back
/// applies of the same style hit the cache.
pub fn resolve(app: &StyleApplication) -> Rc<StyleRules> {
    // Fast path: no overrides, no computed layer, pre-registered
    // variants. Skips the full ResolutionKey hash and goes straight
    // to the stylesheet's pre-resolved arm map.
    if !app.has_overrides && app.computed.is_none() {
        #[cfg(feature = "debug-stats")]
        let _t_fast = crate::debug::now_micros();
        if let Some(rc) = app.sheet.lookup_variant(&app.variants) {
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
    let computed_key = app
        .computed
        .as_ref()
        .map(|c| c.key.clone())
        .unwrap_or_default();
    let key = ResolutionKey {
        sheet: Rc::as_ptr(&app.sheet),
        variants: app.variants.clone(),
        computed_key,
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
    //
    // Merge order matches the four-layer model: base+variants form
    // the floor, then the computed closure's output layers on top,
    // then per-call-site overrides have the final say.
    let mut rules = app.sheet.resolve(&app.variants);
    if let Some(comp) = &app.computed {
        rules = rules.merge(&(comp.compute)());
    }
    let final_rules = rules.merge(&app.overrides);
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
    /// Whether this source reads reactive state — a [`Signal`] or a
    /// [`derived`] closure. A `stylesheet!` builder that receives a
    /// reactive source must emit [`crate::StyleSource::Reactive`] so
    /// signal changes re-apply the style; constant sources (plain enum
    /// values) stay on the cheaper `Static` fast path. Defaults to
    /// `false` (constant).
    fn is_reactive(&self) -> bool {
        false
    }
}

pub trait IntoOverrideSource<T: Clone + 'static> {
    fn into_override_source(self) -> Box<dyn Fn() -> T>;
    /// See [`IntoVariantSource::is_reactive`]. Defaults to `false`.
    fn is_reactive(&self) -> bool {
        false
    }
}

// A bit of plumbing: variant enums have `as_variant_str`. We can't
// require it via a trait the macro defines (orphan rules), so we
// instead expose a marker trait `VariantEnum` that the macro impl's
// on each generated enum.

pub trait VariantEnum: Copy + 'static {
    fn as_variant_str(self) -> &'static str;
    /// Every variant of this enum, in declaration order. Used by
    /// reflective tooling (the docs-app `DocControls` derive) to
    /// build a control that cycles through all values.
    ///
    /// Default returns an empty slice for hand-rolled implementors
    /// of this trait — `stylesheet!`-generated enums override.
    fn all_variants() -> &'static [Self]
    where
        Self: Sized,
    {
        &[]
    }
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
    fn is_reactive(&self) -> bool {
        true
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
/// at the call site (and so a `use runtime_core::derived;` doesn't
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
    fn is_reactive(&self) -> bool {
        true
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
    fn is_reactive(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Interaction properties: cursor + user_select -----------------------

    // The framework imposes no cursor/selection default: a fresh StyleRules
    // leaves both unset, so a bare primitive inherits the platform default and
    // only an author/component opt-in produces a non-default value.
    #[test]
    fn cursor_and_user_select_default_to_unset() {
        let r = StyleRules::default();
        assert_eq!(r.cursor, None);
        assert_eq!(r.user_select, None);
    }

    // `merge` overlays cursor/user_select like any other property: a set value
    // in `other` wins, an unset value leaves the base untouched.
    #[test]
    fn merge_overlays_cursor_and_user_select() {
        let base = StyleRules {
            cursor: Some(Cursor::Pointer),
            user_select: Some(UserSelect::None),
            ..Default::default()
        };
        // An overlay that sets neither leaves the base values intact.
        let unchanged = base.clone().merge(&StyleRules::default());
        assert_eq!(unchanged.cursor, Some(Cursor::Pointer));
        assert_eq!(unchanged.user_select, Some(UserSelect::None));
        // An overlay that sets them wins.
        let over = StyleRules {
            cursor: Some(Cursor::Text),
            user_select: Some(UserSelect::Text),
            ..Default::default()
        };
        let merged = base.merge(&over);
        assert_eq!(merged.cursor, Some(Cursor::Text));
        assert_eq!(merged.user_select, Some(UserSelect::Text));
    }

    // Distinct cursor / user_select values must produce distinct content keys
    // (else the backend's class cache would collide a pointer button with a
    // text one and mint a single shared class). Equal values share a key.
    #[test]
    fn content_key_distinguishes_cursor_and_user_select() {
        let pointer = StyleRules { cursor: Some(Cursor::Pointer), ..Default::default() };
        let text = StyleRules { cursor: Some(Cursor::Text), ..Default::default() };
        let none = StyleRules::default();
        assert_ne!(pointer.content_key(), text.content_key());
        assert_ne!(pointer.content_key(), none.content_key());
        assert_eq!(pointer.content_key(), pointer.clone().content_key());

        let no_sel = StyleRules { user_select: Some(UserSelect::None), ..Default::default() };
        let all_sel = StyleRules { user_select: Some(UserSelect::All), ..Default::default() };
        assert_ne!(no_sel.content_key(), all_sel.content_key());
        assert_ne!(no_sel.content_key(), none.content_key());
    }

    /// Pass-through no-op closures for the non-key params of
    /// `ensure_registered_with`, so a test can focus on one slot.
    fn drain_with_key_recorder(
        sheet: &Rc<StyleSheet>,
        record: impl FnOnce(Option<crate::primitives::key::KeyDownHandler>),
    ) {
        ensure_registered_with(
            sheet,
            |_| {},
            |_| {},
            |_| {},
            |_| {},
            |_, _, _| {},
            |_, _, _, _| {},
            |_| {},
            |_, _| {},
            record,
        );
    }

    // The app-level key handler queued by `set_app_key_handler` must reach the
    // backend (via `Backend::set_app_key_handler`) on the next flush — and only
    // once (single-slot). Regression for the cross-backend global-keyboard path:
    // without the drain in `ensure_registered_with`, the handler would be stashed
    // forever and never installed, so app shortcuts would silently do nothing.
    #[test]
    fn set_app_key_handler_routes_to_backend_once() {
        use std::cell::Cell;
        let sheet = Rc::new(StyleSheet::r#static(StyleRules::default()));

        // Install a handler → it drains to the recorder as `Some`.
        let handler: crate::primitives::key::KeyDownHandler =
            Rc::new(|_e| crate::primitives::key::KeyOutcome::Default);
        set_app_key_handler(Some(handler));
        let drained: Rc<Cell<Option<bool>>> = Rc::new(Cell::new(None));
        {
            let d = drained.clone();
            drain_with_key_recorder(&sheet, move |h| d.set(Some(h.is_some())));
        }
        assert_eq!(drained.get(), Some(true), "installed handler reached the backend");

        // Single-slot: a second flush with nothing queued doesn't call through.
        let called_again = Rc::new(Cell::new(false));
        {
            let c = called_again.clone();
            drain_with_key_recorder(&sheet, move |_h| c.set(true));
        }
        assert!(!called_again.get(), "no pending handler → no second backend call");

        // Clearing (`None`) also routes through as `Some(None)`.
        set_app_key_handler(None);
        let cleared: Rc<Cell<Option<bool>>> = Rc::new(Cell::new(None));
        {
            let c = cleared.clone();
            drain_with_key_recorder(&sheet, move |h| c.set(Some(h.is_some())));
        }
        assert_eq!(cleared.get(), Some(false), "clear routes through as None");
    }

    /// Helper: assert a `Tokenized<Color>` resolves to a particular
    /// fallback string. Tests express the visible color, not whether
    /// the rule used a token vs literal.
    fn color_eq(actual: &Option<Tokenized<Color>>, expected_hex: &str) {
        let value = actual
            .as_ref()
            .expect("expected Some color")
            .value();
        assert_eq!(value.0, expected_hex);
    }

    #[test]
    fn closure_stylesheet_emits_rules() {
        let sheet = StyleSheet::new(|_vs: &VariantSet| StyleRules {
            background: Some(Tokenized::token("surface", Color("#fff".into()))),
            padding_top: Some(Tokenized::Literal(Length::Px(16.0))),
            ..Default::default()
        });
        let r = sheet.resolve(&VariantSet::new());
        color_eq(&r.background, "#fff");
        assert_eq!(r.padding_top, Some(Tokenized::Literal(Length::Px(16.0))));
    }

    #[test]
    fn static_stylesheet_returns_fixed_rules() {
        let sheet = StyleSheet::r#static(StyleRules {
            background: Some(Tokenized::Literal(Color("#abc".into()))),
            ..Default::default()
        });
        let r = sheet.resolve(&VariantSet::new());
        color_eq(&r.background, "#abc");
    }

    #[test]
    fn variant_overlays_layer_on_top_of_base() {
        let sheet = StyleSheet::new(|_vs: &VariantSet| StyleRules {
            background: Some(Tokenized::token("surface", Color("#fff".into()))),
            padding_top: Some(Tokenized::Literal(Length::Px(16.0))),
            ..Default::default()
        })
        .variant("size", "large", |_vs: &VariantSet| StyleRules {
            padding_top: Some(Tokenized::Literal(Length::Px(32.0))),
            ..Default::default()
        });
        let r = sheet.resolve(&VariantSet::new().with("size", "large"));
        color_eq(&r.background, "#fff");
        assert_eq!(r.padding_top, Some(Tokenized::Literal(Length::Px(32.0))));
    }

    #[test]
    fn update_tokens_clears_resolution_cache() {
        let sheet = Rc::new(StyleSheet::new(|_vs: &VariantSet| StyleRules {
            background: Some(Tokenized::token("surface", Color("#fff".into()))),
            ..Default::default()
        }));
        let app = StyleApplication::new(sheet);

        let r1 = resolve(&app);
        color_eq(&r1.background, "#fff");

        // Subsequent resolves hit the cache and return the same Rc.
        let r2 = resolve(&app);
        assert!(Rc::ptr_eq(&r1, &r2));

        // `update_tokens` wipes the cache; the next resolve produces
        // a fresh Rc (token names are stable so the content matches).
        update_tokens(&[TokenEntry {
            name: "surface",
            value: TokenValue::Color(Color("#111".into())),
        }]);
        let r3 = resolve(&app);
        assert!(!Rc::ptr_eq(&r1, &r3));
    }

    #[test]
    fn overrides_layer_on_top_of_base_and_variants() {
        let sheet = Rc::new(
            StyleSheet::new(|_vs: &VariantSet| StyleRules {
                background: Some(Tokenized::token("surface", Color("#fff".into()))),
                font_size: Some(Tokenized::Literal(Length::Px(14.0))),
                padding_top: Some(Tokenized::Literal(Length::Px(16.0))),
                ..Default::default()
            })
            .variant("size", "large", |_vs: &VariantSet| StyleRules {
                font_size: Some(Tokenized::Literal(Length::Px(20.0))),
                ..Default::default()
            }),
        );

        // Base only.
        let r1 = resolve(&StyleApplication::new(sheet.clone()));
        assert_eq!(r1.font_size, Some(Tokenized::Literal(Length::Px(14.0))));

        // With variant: font becomes 20.
        let r2 = resolve(&StyleApplication::new(sheet.clone()).with("size", "large"));
        assert_eq!(r2.font_size, Some(Tokenized::Literal(Length::Px(20.0))));

        // With variant + override: override wins.
        let r3 = resolve(
            &StyleApplication::new(sheet.clone())
                .with("size", "large")
                .override_font_size(17.5),
        );
        assert_eq!(r3.font_size, Some(Tokenized::Literal(Length::Px(17.5))));
        // Other properties unaffected by the override.
        assert_eq!(r3.padding_top, Some(Tokenized::Literal(Length::Px(16.0))));

        // Different override values produce distinct cache entries.
        let r4 = resolve(
            &StyleApplication::new(sheet.clone())
                .with("size", "large")
                .override_font_size(99.0),
        );
        assert_eq!(r4.font_size, Some(Tokenized::Literal(Length::Px(99.0))));
        assert!(!Rc::ptr_eq(&r3, &r4));
    }

    // The bulk `with_overrides` counterpart to the per-field `override_*`
    // setters: a whole `StyleRules` layered on top, each set field winning over
    // the sheet, and preserving any prior overrides it doesn't touch. This is
    // the primitive behind idea-ui's per-slot `*_style` override props.
    #[test]
    fn with_overrides_layers_a_whole_rules_and_preserves_prior() {
        let sheet = Rc::new(StyleSheet::new(|_vs: &VariantSet| StyleRules {
            color: Some(Tokenized::Literal(Color("#111111".into()))),
            padding_top: Some(Tokenized::Literal(Length::Px(16.0))),
            font_size: Some(Tokenized::Literal(Length::Px(14.0))),
            ..Default::default()
        }));

        // A wholesale override wins for every field it sets; untouched sheet
        // fields (font_size) survive.
        let app = StyleApplication::new(sheet.clone()).with_overrides(StyleRules {
            color: Some(Tokenized::Literal(Color("#0b6b3a".into()))),
            padding_top: Some(Tokenized::Literal(Length::Px(0.0))),
            ..Default::default()
        });
        let r = resolve(&app);
        assert_eq!(r.color, Some(Tokenized::Literal(Color("#0b6b3a".into()))), "override color wins");
        assert_eq!(r.padding_top, Some(Tokenized::Literal(Length::Px(0.0))), "override zero-padding wins (flush)");
        assert_eq!(r.font_size, Some(Tokenized::Literal(Length::Px(14.0))), "untouched sheet field survives");

        // A prior per-field override is preserved when the bulk override doesn't
        // set that field, and beaten when it does.
        let app2 = StyleApplication::new(sheet)
            .override_color(Color("#ff0000".into()))
            .with_overrides(StyleRules {
                padding_top: Some(Tokenized::Literal(Length::Px(4.0))),
                ..Default::default()
            });
        let r2 = resolve(&app2);
        assert_eq!(r2.color, Some(Tokenized::Literal(Color("#ff0000".into()))), "prior override_color preserved");
        assert_eq!(r2.padding_top, Some(Tokenized::Literal(Length::Px(4.0))), "bulk override applied on top");
    }

    // ------------------------------------------------------------------
    // Computed layer — runtime-evaluated `StyleRules` between variants
    // and overrides. Used by open-extension variant systems where the
    // modifier matrix isn't enumerable at compile time.
    // ------------------------------------------------------------------

    #[test]
    fn computed_layer_merges_between_variants_and_overrides() {
        let sheet = Rc::new(
            StyleSheet::new(|_vs: &VariantSet| StyleRules {
                background: Some(Tokenized::token("surface", Color("#fff".into()))),
                color: Some(Tokenized::Literal(Color("#111".into()))),
                font_size: Some(Tokenized::Literal(Length::Px(14.0))),
                ..Default::default()
            })
            .variant("size", "large", |_vs: &VariantSet| StyleRules {
                font_size: Some(Tokenized::Literal(Length::Px(20.0))),
                ..Default::default()
            }),
        );

        // Computed layer sets background + color. Variant sets font_size.
        // Override sets font_size to a third value. Result should pick:
        //   background ← computed (since base+variants didn't override)
        //   color ← computed (since base set it, computed overrides)
        //   font_size ← override (override is last layer)
        let app = StyleApplication::new(sheet.clone())
            .with("size", "large")
            .with_computed("filled+danger", || StyleRules {
                background: Some(Tokenized::Literal(Color("#e5484d".into()))),
                color: Some(Tokenized::Literal(Color("#ffffff".into()))),
                ..Default::default()
            })
            .override_font_size(99.0);
        let r = resolve(&app);
        color_eq(&r.background, "#e5484d");
        color_eq(&r.color, "#ffffff");
        assert_eq!(r.font_size, Some(Tokenized::Literal(Length::Px(99.0))));
    }

    #[test]
    fn computed_layer_shares_cache_entry_across_equivalent_keys() {
        let sheet = Rc::new(StyleSheet::new(|_vs: &VariantSet| StyleRules {
            ..Default::default()
        }));

        // Two separate apps with the same computed key produce closures
        // that return equivalent StyleRules. The framework must reuse a
        // single cached Rc<StyleRules> — that's what makes 1000 buttons
        // with `tone=Danger, variant=Filled, size=Md` materialize one
        // class on the backend, not 1000.
        let make_app = || {
            StyleApplication::new(sheet.clone()).with_computed("filled+danger+md", || StyleRules {
                background: Some(Tokenized::Literal(Color("#e5484d".into()))),
                ..Default::default()
            })
        };
        let r1 = resolve(&make_app());
        let r2 = resolve(&make_app());
        assert!(
            Rc::ptr_eq(&r1, &r2),
            "equal computed keys must share the cached Rc<StyleRules>",
        );
    }

    #[test]
    fn computed_layer_distinct_keys_produce_distinct_cache_entries() {
        let sheet = Rc::new(StyleSheet::new(|_vs: &VariantSet| StyleRules {
            ..Default::default()
        }));

        let app_a = StyleApplication::new(sheet.clone()).with_computed("filled+danger", || StyleRules {
            background: Some(Tokenized::Literal(Color("#e5484d".into()))),
            ..Default::default()
        });
        let app_b = StyleApplication::new(sheet.clone()).with_computed("filled+success", || StyleRules {
            background: Some(Tokenized::Literal(Color("#3ba55d".into()))),
            ..Default::default()
        });

        let r_a = resolve(&app_a);
        let r_b = resolve(&app_b);
        assert!(!Rc::ptr_eq(&r_a, &r_b));
        color_eq(&r_a.background, "#e5484d");
        color_eq(&r_b.background, "#3ba55d");
    }

    #[test]
    fn computed_layer_reruns_after_token_update() {
        // Closure reads a token-backed value. After `update_tokens`
        // wipes the cache, the next resolve must re-run the closure so
        // theme-dependent reads pick up the new value. This is the
        // mechanism that makes a custom Tone re-render correctly on
        // light/dark swap.
        let sheet = Rc::new(StyleSheet::new(|_vs: &VariantSet| StyleRules {
            ..Default::default()
        }));
        let app = StyleApplication::new(sheet).with_computed("hype-tone", || StyleRules {
            background: Some(Tokenized::token(
                "tone-hype-fill-bg",
                Color("#ff00aa".into()),
            )),
            ..Default::default()
        });

        let r1 = resolve(&app);
        // Same key + no cache wipe → same Rc.
        let r2 = resolve(&app);
        assert!(Rc::ptr_eq(&r1, &r2));

        // Token update wipes the cache. The closure re-runs; the
        // returned `StyleRules` carries the same token name (so the
        // resolved class name is theme-stable), but its `Rc` identity
        // is fresh.
        update_tokens(&[TokenEntry {
            name: "tone-hype-fill-bg",
            value: TokenValue::Color(Color("#cc0088".into())),
        }]);
        let r3 = resolve(&app);
        assert!(!Rc::ptr_eq(&r1, &r3));
        // Token name is preserved (theme-stable identity) even though
        // a fresh closure execution constructed the value.
        assert_eq!(
            r3.background.as_ref().and_then(|t| t.name()),
            Some("tone-hype-fill-bg"),
        );
    }

    #[test]
    fn computed_layer_fast_path_disabled_when_attached() {
        // The fast path (sheet.lookup_variant) skips the resolution
        // cache entirely and would miss the computed layer. The fast
        // path must therefore be disabled whenever a computed layer is
        // present — verify by attaching a computed layer that shadows a
        // variant's property and confirming the computed value wins.
        let sheet = Rc::new(
            StyleSheet::new(|_vs: &VariantSet| StyleRules {
                color: Some(Tokenized::Literal(Color("#000000".into()))),
                ..Default::default()
            })
            .variant("size", "large", |_vs: &VariantSet| StyleRules {
                color: Some(Tokenized::Literal(Color("#222222".into()))),
                ..Default::default()
            }),
        );

        let app = StyleApplication::new(sheet)
            .with("size", "large")
            .with_computed("custom-color", || StyleRules {
                color: Some(Tokenized::Literal(Color("#ff00aa".into()))),
                ..Default::default()
            });
        let r = resolve(&app);
        color_eq(&r.color, "#ff00aa");
    }

    #[test]
    fn variant_default_applies_when_axis_unselected() {
        let sheet = StyleSheet::new(|_vs: &VariantSet| StyleRules {
            padding_top: Some(Tokenized::Literal(Length::Px(8.0))),
            ..Default::default()
        })
        .variant("size", "small", |_vs: &VariantSet| StyleRules {
            padding_top: Some(Tokenized::Literal(Length::Px(4.0))),
            ..Default::default()
        })
        .variant("size", "large", |_vs: &VariantSet| StyleRules {
            padding_top: Some(Tokenized::Literal(Length::Px(16.0))),
            ..Default::default()
        })
        .variant_default("size", "large");

        // Call site omits `size` → default "large" applies → padding 16.
        let r = sheet.resolve(&VariantSet::new());
        assert_eq!(r.padding_top, Some(Tokenized::Literal(Length::Px(16.0))));

        // Call site picks "small" → padding 4.
        let r2 = sheet.resolve(&VariantSet::new().with("size", "small"));
        assert_eq!(r2.padding_top, Some(Tokenized::Literal(Length::Px(4.0))));
    }

    #[test]
    fn compound_variant_applies_only_when_all_match() {
        let sheet = StyleSheet::new(|_vs: &VariantSet| StyleRules::default())
            .variant("size", "large", |_vs: &VariantSet| StyleRules {
                padding_top: Some(Tokenized::Literal(Length::Px(16.0))),
                ..Default::default()
            })
            .variant("kind", "primary", |_vs: &VariantSet| StyleRules {
                background: Some(Tokenized::Literal(Color("primary-bg".into()))),
                ..Default::default()
            })
            .compound(
                vec![("size", "large"), ("kind", "primary")],
                |_vs: &VariantSet| StyleRules {
                    font_size: Some(Tokenized::Literal(Length::Px(24.0))),
                    ..Default::default()
                },
            );

        // Only size=large → compound NOT applied.
        let r1 = sheet.resolve(&VariantSet::new().with("size", "large"));
        assert_eq!(r1.padding_top, Some(Tokenized::Literal(Length::Px(16.0))));
        assert_eq!(r1.font_size, None);

        // Both axes match → compound APPLIED.
        let r2 = sheet.resolve(
            &VariantSet::new().with("size", "large").with("kind", "primary"),
        );
        assert_eq!(r2.padding_top, Some(Tokenized::Literal(Length::Px(16.0))));
        color_eq(&r2.background, "primary-bg");
        assert_eq!(r2.font_size, Some(Tokenized::Literal(Length::Px(24.0))));
    }

    #[test]
    fn variant_keys_lists_every_axis_value() {
        let sheet = StyleSheet::new(|_vs: &VariantSet| StyleRules::default())
            .variant("size", "small", |_vs: &VariantSet| StyleRules::default())
            .variant("size", "large", |_vs: &VariantSet| StyleRules::default())
            .variant("kind", "primary", |_vs: &VariantSet| StyleRules::default());
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
        let sheet = Rc::new(StyleSheet::r#static(StyleRules {
            background: Some(Tokenized::Literal(Color("#abc".into()))),
            ..Default::default()
        }));
        let app = StyleApplication::new(sheet);
        let r1 = resolve(&app);
        let r2 = resolve(&app);
        assert!(Rc::ptr_eq(&r1, &r2));
    }

    /// **The core invariant of the tokenization rework**: two
    /// stylesheets producing the same token references must hash to
    /// the same content key regardless of installed token values.
    #[test]
    fn tokenized_rules_have_token_stable_content_keys() {
        let sheet_a = StyleSheet::new(|_vs: &VariantSet| StyleRules {
            background: Some(Tokenized::token("surface", Color("#fff".into()))),
            ..Default::default()
        });
        let sheet_b = StyleSheet::new(|_vs: &VariantSet| StyleRules {
            background: Some(Tokenized::token("surface", Color("#111".into()))),
            ..Default::default()
        });
        let r_a = sheet_a.resolve(&VariantSet::new());
        let r_b = sheet_b.resolve(&VariantSet::new());
        assert_eq!(r_a.content_key(), r_b.content_key());
        // Sanity: the *fallbacks* differ so we know the test is real.
        assert_ne!(r_a.background.as_ref().unwrap().value().0,
                   r_b.background.as_ref().unwrap().value().0);
    }

    /// Literal values should NOT collide with token references that
    /// happen to share a string. The content key encoder tags
    /// literals with `L:` and tokens with `T:` to disambiguate.
    #[test]
    fn literal_and_token_with_same_string_have_distinct_keys() {
        let lit_rules = StyleRules {
            background: Some(Tokenized::Literal(Color("surface".into()))),
            ..Default::default()
        };
        let tok_rules = StyleRules {
            background: Some(Tokenized::token("surface", Color("anything".into()))),
            ..Default::default()
        };
        assert_ne!(lit_rules.content_key(), tok_rules.content_key());
    }

    // -----------------------------------------------------------------
    // Per-token reactivity (TOKEN_REGISTRY / Tokenized::resolve)
    // -----------------------------------------------------------------
    //
    // Tests use globally-unique token names ("tk_<test>_<token>") to
    // avoid cross-test contamination from the thread-local registry —
    // the registry persists across tests on the same thread because
    // it lives outside any `Scope`.

    #[test]
    fn install_tokens_populates_registry_and_resolve_returns_installed_value() {
        install_tokens(&[
            TokenEntry {
                name: "tk_install_color",
                value: TokenValue::Color(Color("#123".into())),
            },
            TokenEntry {
                name: "tk_install_len",
                value: TokenValue::Length(Length::Px(24.0)),
            },
            TokenEntry {
                name: "tk_install_num",
                value: TokenValue::Number(0.5),
            },
        ]);

        let c: Tokenized<Color> = Tokenized::token("tk_install_color", Color("#fff".into()));
        let l: Tokenized<Length> = Tokenized::token("tk_install_len", Length::Px(0.0));
        let n: Tokenized<f32> = Tokenized::token("tk_install_num", 0.0);
        assert_eq!(c.resolve().0, "#123");
        assert_eq!(l.resolve(), Length::Px(24.0));
        assert_eq!(n.resolve(), 0.5);
    }

    #[test]
    fn resolve_literal_returns_value_and_does_not_touch_registry() {
        let c: Tokenized<Color> = Tokenized::Literal(Color("#abc".into()));
        assert_eq!(c.resolve().0, "#abc");
        let l: Tokenized<Length> = Tokenized::Literal(Length::Px(8.0));
        assert_eq!(l.resolve(), Length::Px(8.0));
        let n: Tokenized<f32> = Tokenized::Literal(7.5);
        assert_eq!(n.resolve(), 7.5);
    }

    #[test]
    fn resolve_uninstalled_token_returns_fallback() {
        // Token name never installed — resolve still works and lazily
        // creates a registry entry seeded with the fallback so subsequent
        // `update_tokens` for the same name can propagate.
        //
        // Install a *different* token first so the thread is marked
        // themed; the permissive lazy-fallback semantics we're
        // exercising apply to individual missing tokens, not to a
        // totally-unthemed thread.
        install_tokens(&[TokenEntry {
            name: "tk_uninstalled_sentinel",
            value: TokenValue::Color(Color("#000".into())),
        }]);
        let c: Tokenized<Color> = Tokenized::token("tk_uninstalled", Color("#fall".into()));
        assert_eq!(c.resolve().0, "#fall");
    }

    /// Regression test for the `with_or_create_token_signal` scope-adoption
    /// audit finding. Token signals stashed in the thread-local
    /// `TOKEN_REGISTRY` must outlive any render scope — they're the
    /// theme system's authoritative store and need thread lifetime to
    /// survive re-mounts (hot reload, fixture teardown, page-rebuild
    /// in dev tools).
    ///
    /// Bug before fix: `Signal::new` inside `with_or_create_token_signal`
    /// gets registered with the currently-active `Scope`. When that
    /// scope drops (e.g. app unmount), the slot is freed but
    /// `TOKEN_REGISTRY` still holds a stale `Signal` handle. The next
    /// resolve of the same token either panics with
    /// "signal used after its scope was dropped" or — worse — silently
    /// hits a recycled slot of an unrelated signal.
    #[test]
    fn token_signal_survives_creating_scope_drop() {
        use crate::reactive::{with_scope, Scope};

        // Use a unique token name so this test doesn't collide with the
        // other registry-touching tests (registry is process-wide).
        const NAME: &str = "tk_scope_survival_color";

        // Mark the thread as themed. The bug under test is about lazy
        // creation of a *single missing* token's signal slot, not about
        // a totally-unthemed thread.
        install_tokens(&[TokenEntry {
            name: "tk_scope_survival_sentinel",
            value: TokenValue::Color(Color("#000".into())),
        }]);

        // First read happens inside scope A. Resolves the token, which
        // creates the registry signal lazily.
        {
            let mut scope_a = Scope::new();
            with_scope(&mut scope_a, || {
                let c: Tokenized<Color> = Tokenized::token(NAME, Color("#aaa".into()));
                let v = c.resolve();
                assert_eq!(v.0, "#aaa", "first resolve returns the fallback");
            });
            // scope_a drops here. With the bug, the token signal's
            // arena slot is freed; the registry still holds the stale
            // Signal handle.
        }

        // Second read happens inside an unrelated scope B. Must NOT
        // panic — the token registry is supposed to be thread-lifetime.
        let mut scope_b = Scope::new();
        let observed = with_scope(&mut scope_b, || {
            let c: Tokenized<Color> = Tokenized::token(NAME, Color("#bbb".into()));
            c.resolve()
        });
        // We expect the fallback that was installed on the first
        // resolve to still be returned (registry preserved its
        // contents). What we don't expect is a panic.
        assert_eq!(
            observed.0, "#aaa",
            "second resolve must return the originally-installed fallback, \
             proving the token signal outlived its creating scope"
        );

        // `update_tokens` should also work after the creator scope dropped.
        update_tokens(&[TokenEntry {
            name: NAME,
            value: TokenValue::Color(Color("#ccc".into())),
        }]);
        let mut scope_c = Scope::new();
        let updated = with_scope(&mut scope_c, || {
            let c: Tokenized<Color> = Tokenized::token(NAME, Color("#bbb".into()));
            c.resolve()
        });
        assert_eq!(
            updated.0, "#ccc",
            "update_tokens through the registry-stashed signal must still work \
             after the creating scope dropped"
        );
    }

    /// `update_tokens(["a"])` must fire only the signal for `"a"` — the
    /// signal for `"b"` stays still. This is the per-token isolation
    /// invariant at the signal layer.
    #[test]
    fn update_tokens_fires_only_changed_token_signal() {
        use std::cell::Cell;
        use std::rc::Rc;

        install_tokens(&[
            TokenEntry {
                name: "tk_isolate_a",
                value: TokenValue::Color(Color("#a0".into())),
            },
            TokenEntry {
                name: "tk_isolate_b",
                value: TokenValue::Color(Color("#b0".into())),
            },
        ]);

        let a_runs = Rc::new(Cell::new(0u32));
        let b_runs = Rc::new(Cell::new(0u32));
        let a_runs_c = a_runs.clone();
        let b_runs_c = b_runs.clone();

        let tok_a: Tokenized<Color> =
            Tokenized::token("tk_isolate_a", Color("#fall".into()));
        let tok_b: Tokenized<Color> =
            Tokenized::token("tk_isolate_b", Color("#fall".into()));

        let _ea = crate::Effect::new(move || {
            let _ = tok_a.resolve();
            a_runs_c.set(a_runs_c.get() + 1);
        });
        let _eb = crate::Effect::new(move || {
            let _ = tok_b.resolve();
            b_runs_c.set(b_runs_c.get() + 1);
        });
        assert_eq!(a_runs.get(), 1, "effect A fired once on install");
        assert_eq!(b_runs.get(), 1, "effect B fired once on install");

        update_tokens(&[TokenEntry {
            name: "tk_isolate_a",
            value: TokenValue::Color(Color("#a1".into())),
        }]);
        assert_eq!(a_runs.get(), 2, "effect A re-fires on its token's update");
        assert_eq!(b_runs.get(), 1, "effect B did NOT re-fire on A's update");

        update_tokens(&[TokenEntry {
            name: "tk_isolate_b",
            value: TokenValue::Color(Color("#b1".into())),
        }]);
        assert_eq!(a_runs.get(), 2, "effect A unchanged by B's update");
        assert_eq!(
            b_runs.get(),
            2,
            "effect B re-fires on its token's update"
        );
    }

    /// **Load-bearing test for the whole refactor.** A styled-effect-
    /// like setup: a `Tokenized::resolve()` read inside an Effect
    /// subscribes that effect to ONLY the specific token signal — so
    /// an `update_tokens` for a *different* token leaves the effect
    /// untouched. This is the property that lets a 10k-row scoreboard
    /// avoid waking nodes that don't reference the changed token.
    #[test]
    fn per_token_isolation_in_styled_effect() {
        use std::cell::Cell;
        use std::rc::Rc;

        install_tokens(&[
            TokenEntry {
                name: "tk_styled_a",
                value: TokenValue::Color(Color("#aaaaaa".into())),
            },
            TokenEntry {
                name: "tk_styled_b",
                value: TokenValue::Color(Color("#bbbbbb".into())),
            },
        ]);

        // Effect that reads ONLY token A (like a node whose stylesheet
        // references `tk_styled_a` for background).
        let runs = Rc::new(Cell::new(0u32));
        let last_value = Rc::new(Cell::new(String::new()));
        let runs_c = runs.clone();
        let last_value_c = last_value.clone();
        let tok_a: Tokenized<Color> =
            Tokenized::token("tk_styled_a", Color("#fff".into()));
        let _e = crate::Effect::new(move || {
            // Mirror what a backend's apply_style does — resolve the
            // tokenized property to a concrete value.
            let resolved = tok_a.resolve();
            last_value_c.set(resolved.0);
            runs_c.set(runs_c.get() + 1);
        });
        assert_eq!(runs.get(), 1, "initial run on install");
        assert_eq!(last_value.take(), "#aaaaaa");

        // Update an UNRELATED token (B). Our effect must NOT re-fire.
        update_tokens(&[TokenEntry {
            name: "tk_styled_b",
            value: TokenValue::Color(Color("#b1b1b1".into())),
        }]);
        assert_eq!(
            runs.get(),
            1,
            "styled effect reading only tk_styled_a must not wake on tk_styled_b updates"
        );

        // Update the SUBSCRIBED token (A). Effect re-fires with new value.
        update_tokens(&[TokenEntry {
            name: "tk_styled_a",
            value: TokenValue::Color(Color("#a1a1a1".into())),
        }]);
        assert_eq!(runs.get(), 2, "styled effect re-fires on its own token");
        assert_eq!(last_value.take(), "#a1a1a1");
    }

    #[test]
    fn update_tokens_before_install_is_permissive() {
        // Calling update_tokens for a never-installed name creates
        // the registry entry — subsequent resolves see the value.
        update_tokens(&[TokenEntry {
            name: "tk_permissive",
            value: TokenValue::Length(Length::Px(99.0)),
        }]);
        let t: Tokenized<Length> = Tokenized::token("tk_permissive", Length::Px(0.0));
        assert_eq!(t.resolve(), Length::Px(99.0));
    }

    #[test]
    fn resolve_with_wrong_variant_falls_back() {
        // Install a token as Length, then read via Tokenized<Color>.
        // Should return the fallback (and emit a debug eprintln in
        // debug builds — not asserted to avoid coupling).
        install_tokens(&[TokenEntry {
            name: "tk_wrong_variant",
            value: TokenValue::Length(Length::Px(10.0)),
        }]);
        let c: Tokenized<Color> = Tokenized::token("tk_wrong_variant", Color("#fb".into()));
        assert_eq!(c.resolve().0, "#fb");
    }

    /// Regression test for the "native aborts when no theme is installed"
    /// report (Whiteboard Pro feedback, 2026-06): an app that styles with
    /// literal colors — or just leans on primitive default tokens like
    /// `color-text` — and never calls `install_theme` rendered fine on web
    /// (`var(--color-text, #1a1a1f)`) but `SIGABRT`-ed deep in style
    /// resolution on macOS via a `debug_assert!` tripwire.
    ///
    /// Resolving a `Tokenized::Token` on a thread with no installed theme
    /// must return the embedded fallback and **never panic**, matching the
    /// web backend (CLAUDE.md §7: backends converge in output). The
    /// cross-thread footgun the tripwire originally guarded is now a
    /// debug-only *warning* (see `debug_warn_resolve_on_unthemed_thread`),
    /// not an abort.
    ///
    /// Spawning a fresh, never-themed thread is the only way to
    /// deterministically exercise an unthemed thread — the test runner
    /// reuses threads across tests and the parent thread gets themed by
    /// the other registry-touching tests in this module. The assertion
    /// holds in both debug and release builds (the behavior is identical),
    /// so this test is intentionally *not* `#[cfg(debug_assertions)]`.
    #[test]
    fn resolve_on_unthemed_thread_falls_back_without_panicking() {
        let handle = std::thread::Builder::new()
            .name("unthemed_resolve_thread".into())
            .spawn(|| {
                let c: Tokenized<Color> =
                    Tokenized::token("tk_unthemed_thread", Color("#fall".into()));
                // Must return the fallback, not panic.
                c.resolve().0
            })
            .expect("spawn worker thread");

        let resolved = handle
            .join()
            .expect("resolving a token on an unthemed thread must not panic");
        assert_eq!(
            resolved, "#fall",
            "resolving a token on an unthemed thread must return the literal \
             fallback — native must match the web backend's silent \
             `var(--name, fallback)` behavior"
        );
    }

    /// Regression test for the "border width type uniformity" papercut
    /// (Whiteboard Pro feedback): `border_*_width` is `Tokenized<f32>`
    /// while every other length field is `Tokenized<Length>`, so passing
    /// a `Length` used to fail with a confusing trait error. A `Length`
    /// now coerces into `Tokenized<f32>` — pixels pass through; percent
    /// and auto are invalid for a border and collapse to `0.0`.
    #[test]
    fn length_coerces_into_tokenized_f32_for_border_widths() {
        let px: Tokenized<f32> = Length::Px(2.5).into();
        assert_eq!(px.resolve(), 2.5);

        // Percent/Auto are meaningless for a border → 0.0 (not a panic,
        // not a type error).
        let pct: Tokenized<f32> = Length::Percent(50.0).into();
        assert_eq!(pct.resolve(), 0.0);
        let auto: Tokenized<f32> = Length::Auto.into();
        assert_eq!(auto.resolve(), 0.0);
    }

    // -----------------------------------------------------------------
    // FontFamily + typeface registration
    // -----------------------------------------------------------------

    // `face!` embeds via `include_bytes!`, so its src paths must
    // point at real files. We use sibling `runtime-core` sources
    // as test-only embed targets — the bytes are irrelevant; the
    // tests only exercise `Typeface`/`FontFamily` identity + struct
    // shape.
    fn sample_typeface() -> crate::assets::Typeface {
        crate::typeface! {
            name: "TestSans",
            faces: [
                crate::face!(weight: FontWeight::Normal, style: FontStyle::Normal,
                             src: "assets.rs"),
                crate::face!(weight: FontWeight::Bold, style: FontStyle::Normal,
                             src: "lib.rs"),
            ],
            fallback: crate::assets::SystemFallback::SansSerif,
        }
    }

    fn other_typeface() -> crate::assets::Typeface {
        crate::typeface! {
            name: "TestMono",
            faces: [
                crate::face!(weight: FontWeight::Normal, style: FontStyle::Normal,
                             src: "reactive.rs"),
            ],
            fallback: crate::assets::SystemFallback::Monospace,
        }
    }

    #[test]
    fn font_family_from_string_and_str_produce_system() {
        let from_str: FontFamily = "Helvetica".into();
        let from_string: FontFamily = String::from("Helvetica").into();
        assert_eq!(from_str, FontFamily::System("Helvetica".to_string()));
        assert_eq!(from_string, from_str);
    }

    // The deleted-`typeface!` DX warning's pure decision. Gated on
    // `debug_assertions` because `should_warn_for_system_font` itself is
    // debug-only (the whole guardrail compiles out in release).
    #[cfg(debug_assertions)]
    #[test]
    fn should_warn_for_system_font_decision_table() {
        use super::should_warn_for_system_font;
        use std::collections::HashSet;

        let registered: HashSet<&'static str> = ["Inter", "Source Code Pro"]
            .into_iter()
            .collect();

        // Bare, unregistered, non-generic → looks like a removed
        // `typeface!` registration. WARN.
        assert!(should_warn_for_system_font("Roboto Mono", &registered));
        assert!(should_warn_for_system_font("MyCustomFace", &registered));

        // Registered typeface family → resolves fine, no warning.
        assert!(!should_warn_for_system_font("Inter", &registered));
        assert!(!should_warn_for_system_font("Source Code Pro", &registered));

        // Known generic / system families → intentional, no warning
        // (case-insensitive).
        assert!(!should_warn_for_system_font("sans-serif", &registered));
        assert!(!should_warn_for_system_font("serif", &registered));
        assert!(!should_warn_for_system_font("monospace", &registered));
        assert!(!should_warn_for_system_font("system-ui", &registered));
        assert!(!should_warn_for_system_font("-apple-system", &registered));
        assert!(!should_warn_for_system_font("BlinkMacSystemFont", &registered));
        assert!(!should_warn_for_system_font("Segoe UI", &registered));
        assert!(!should_warn_for_system_font("ARIAL", &registered));

        // Comma stack → explicit fallback list, never a bare face.
        assert!(!should_warn_for_system_font("Inter, sans-serif", &registered));
        assert!(!should_warn_for_system_font(
            "NotRegistered, sans-serif",
            &registered
        ));

        // Empty / whitespace → nothing actionable.
        assert!(!should_warn_for_system_font("", &registered));
        assert!(!should_warn_for_system_font("   ", &registered));

        // Quoted bare generic is still recognized as generic.
        assert!(!should_warn_for_system_font("\"sans-serif\"", &registered));
        // Quoted registered family is still recognized as registered.
        assert!(!should_warn_for_system_font("\"Inter\"", &registered));
        // Quoted unregistered non-generic still warns.
        assert!(should_warn_for_system_font("\"Ghost\"", &registered));

        // Surrounding whitespace is trimmed before matching.
        assert!(!should_warn_for_system_font("  Inter  ", &registered));
    }

    #[test]
    fn font_family_from_typeface_wraps_value() {
        let tf = sample_typeface();
        let ff: FontFamily = tf.into();
        match ff {
            FontFamily::Typeface(t) => assert_eq!(t.id, tf.id),
            _ => panic!("expected Typeface variant"),
        }
    }

    #[test]
    fn font_family_eq_by_typeface_id_not_struct() {
        let tf = sample_typeface();
        // Same id but synthetic struct missing the static metadata —
        // exercises the manual `PartialEq` that compares on id only.
        let synthetic = crate::assets::Typeface {
            id: tf.id,
            family_name: "",
            faces: &[],
            fallback: crate::assets::SystemFallback::None,
        };
        let a = FontFamily::Typeface(tf);
        let b = FontFamily::Typeface(synthetic);
        assert_eq!(a, b);
    }

    #[test]
    fn font_family_system_and_typeface_never_equal() {
        let tf = sample_typeface();
        let a = FontFamily::System("X".to_string());
        let b = FontFamily::Typeface(tf);
        assert_ne!(a, b);
    }

    #[test]
    fn font_family_hash_matches_eq() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        fn hash<T: Hash>(t: &T) -> u64 {
            let mut h = DefaultHasher::new();
            t.hash(&mut h);
            h.finish()
        }

        let tf = sample_typeface();
        let a = FontFamily::Typeface(tf);
        let synthetic = crate::assets::Typeface {
            id: tf.id,
            family_name: "different-but-same-id",
            faces: &[],
            fallback: crate::assets::SystemFallback::None,
        };
        let b = FontFamily::Typeface(synthetic);
        assert_eq!(a, b);
        assert_eq!(hash(&a), hash(&b), "equal values must hash equal");

        let s1 = FontFamily::System("X".to_string());
        let s2 = FontFamily::System("X".to_string());
        assert_eq!(hash(&s1), hash(&s2));
        let s3 = FontFamily::System("Y".to_string());
        assert_ne!(hash(&s1), hash(&s3));
    }

    #[test]
    fn content_key_typeface_distinct_from_same_named_system() {
        let tf = sample_typeface();
        let from_typeface = StyleRules {
            font_family: Some(FontFamily::Typeface(tf)),
            ..Default::default()
        };
        let from_system = StyleRules {
            font_family: Some(FontFamily::System(tf.family_name.to_string())),
            ..Default::default()
        };
        // A typeface and a same-name system reference describe
        // semantically different things (the typeface registration is
        // a separate backend artifact). Content keys must differ so
        // the backend doesn't conflate them.
        assert_ne!(from_typeface.content_key(), from_system.content_key());
    }

    #[test]
    fn content_key_same_typeface_collapses_to_same_key() {
        let tf = sample_typeface();
        let a = StyleRules {
            font_family: Some(FontFamily::Typeface(tf)),
            ..Default::default()
        };
        let b = StyleRules {
            font_family: Some(FontFamily::Typeface(tf)),
            ..Default::default()
        };
        assert_eq!(a.content_key(), b.content_key());
    }

    #[test]
    fn ensure_typefaces_registered_dedups_by_id() {
        // Two rules referencing the same typeface — register
        // callbacks fire exactly once. Different typefaces in the
        // same call register separately.
        let tf_a = sample_typeface();
        let tf_b = other_typeface();
        let rules: Vec<Rc<StyleRules>> = vec![
            Rc::new(StyleRules {
                font_family: Some(FontFamily::Typeface(tf_a)),
                ..Default::default()
            }),
            Rc::new(StyleRules {
                font_family: Some(FontFamily::Typeface(tf_a)),
                ..Default::default()
            }),
            Rc::new(StyleRules {
                font_family: Some(FontFamily::Typeface(tf_b)),
                ..Default::default()
            }),
            // System reference — must NOT trigger registration.
            Rc::new(StyleRules {
                font_family: Some(FontFamily::System("system-ui".to_string())),
                ..Default::default()
            }),
        ];
        let mut asset_calls: Vec<crate::assets::AssetId> = Vec::new();
        let mut typeface_calls: Vec<TypefaceId> = Vec::new();
        ensure_typefaces_registered_with(
            &rules,
            |id, kind, _src| {
                assert_eq!(kind, crate::assets::AssetTag::Font);
                asset_calls.push(id);
            },
            |id, _name, _faces, _fallback| {
                typeface_calls.push(id);
            },
        );
        // 2 faces for tf_a + 1 face for tf_b = 3 asset registrations.
        assert_eq!(asset_calls.len(), 3);
        assert_eq!(typeface_calls, vec![tf_a.id, tf_b.id]);

        // A *second* call for an overlapping set is a no-op for the
        // already-seen typeface. tf_b would also dedup, so a re-call
        // with [tf_a, tf_b] only fires for items already registered:
        // both are known → zero new calls.
        let mut asset_calls2: Vec<crate::assets::AssetId> = Vec::new();
        let mut typeface_calls2: Vec<TypefaceId> = Vec::new();
        ensure_typefaces_registered_with(
            &rules,
            |id, _, _| asset_calls2.push(id),
            |id, _, _, _| typeface_calls2.push(id),
        );
        assert!(asset_calls2.is_empty(), "no new asset registrations on dedup");
        assert!(typeface_calls2.is_empty(), "no new typeface registrations on dedup");
    }

    // ====================================================================
    // Gradient merge + content_key + RadialExtent + aspect_ratio
    //
    // Regression tests for the "manual `overlay!()` macro silently
    // drops new fields" bug class. `merge()` and `content_key()` are
    // hand-listed; the welcome example's vignette pulse went dark on
    // web for an entire release because `background_gradient` was
    // omitted from `merge`'s list, and the resolved StyleRules
    // backends received had `background_gradient: None` despite the
    // sheet declaring one.
    //
    // These tests pin the property "every gradient-relevant field
    // round-trips through merge AND distinguishes content_key" so
    // any future field addition that forgets either path fails
    // loudly in CI.
    // ====================================================================

    /// Helper: a Linear gradient with one stop. Specific values
    /// don't matter for the merge/key tests; we just need a
    /// distinct, recognizable `Some(Gradient)`.
    fn linear_gradient(angle_deg: f32) -> Gradient {
        Gradient {
            kind: GradientKind::Linear { angle_deg },
            stops: vec![GradientStop {
                offset: 0.0,
                color: Color("#000".into()),
            }],
        }
    }

    fn radial_gradient(radius: f32, extent: RadialExtent) -> Gradient {
        Gradient {
            kind: GradientKind::Radial {
                center: (0.5, 0.5),
                radius,
                extent,
            },
            stops: vec![GradientStop {
                offset: 0.0,
                color: Color("#000".into()),
            }],
        }
    }

    /// `merge` must carry `background_gradient` from `other` when
    /// `self` has none. This was the original gradient bug — `merge`
    /// stripped the field, so backends got `None` and animation
    /// snapshotting silently failed.
    #[test]
    fn merge_overlays_background_gradient_onto_empty_base() {
        let base = StyleRules::default();
        let overlay = StyleRules {
            background_gradient: Some(linear_gradient(45.0)),
            ..Default::default()
        };
        let merged = base.merge(&overlay);
        assert!(
            merged.background_gradient.is_some(),
            "merge dropped background_gradient on empty-base + Some-overlay; \
             this is the welcome-vignette bug class — verify the overlay! \
             macro lists `background_gradient`",
        );
        assert_eq!(
            merged.background_gradient.as_ref().unwrap(),
            &linear_gradient(45.0),
        );
    }

    /// `merge` must NOT clobber a base gradient when `other` has
    /// none. (`overlay!` only overwrites when the other field is
    /// `Some`; verifying we didn't accidentally write the wrong
    /// branch.)
    #[test]
    fn merge_keeps_base_gradient_when_overlay_has_none() {
        let base = StyleRules {
            background_gradient: Some(linear_gradient(30.0)),
            ..Default::default()
        };
        let overlay = StyleRules::default();
        let merged = base.merge(&overlay);
        assert_eq!(
            merged.background_gradient.as_ref().unwrap(),
            &linear_gradient(30.0),
            "an empty overlay must not strip the base gradient",
        );
    }

    /// When both base and overlay set a gradient, overlay wins.
    /// Standard `overlay!()` semantics — the existence of this
    /// behaviour for gradient is what makes state-overlay
    /// transitions on gradient backgrounds work.
    #[test]
    fn merge_overlay_gradient_wins_over_base_gradient() {
        let base = StyleRules {
            background_gradient: Some(linear_gradient(0.0)),
            ..Default::default()
        };
        let overlay = StyleRules {
            background_gradient: Some(linear_gradient(90.0)),
            ..Default::default()
        };
        let merged = base.merge(&overlay);
        assert_eq!(
            merged.background_gradient.as_ref().unwrap(),
            &linear_gradient(90.0),
        );
    }

    /// `content_key` must distinguish two gradients that differ
    /// only in angle. Pre-fix, two distinct gradients could collide
    /// on the same minted CSS class because `content_key` ignored
    /// the gradient field entirely.
    #[test]
    fn content_key_differentiates_gradients_by_angle() {
        let a = StyleRules {
            background_gradient: Some(linear_gradient(0.0)),
            ..Default::default()
        };
        let b = StyleRules {
            background_gradient: Some(linear_gradient(45.0)),
            ..Default::default()
        };
        assert_ne!(
            a.content_key(),
            b.content_key(),
            "content_key must hash the angle so distinct gradients don't share a class",
        );
    }

    /// content_key must distinguish Linear from Radial even when
    /// they're otherwise unrelated.
    #[test]
    fn content_key_differentiates_linear_vs_radial() {
        let lin = StyleRules {
            background_gradient: Some(linear_gradient(45.0)),
            ..Default::default()
        };
        let rad = StyleRules {
            background_gradient: Some(radial_gradient(1.0, RadialExtent::ClosestSide)),
            ..Default::default()
        };
        assert_ne!(lin.content_key(), rad.content_key());
    }

    /// content_key must distinguish two radial gradients whose only
    /// difference is the `extent` (`ClosestSide` vs `FarthestCorner`).
    /// This is the property that prevents the welcome-vignette
    /// stops from collapsing onto the same class as a sun-disc
    /// gradient that happens to share radius+center.
    #[test]
    fn content_key_differentiates_radial_extents() {
        let cs = StyleRules {
            background_gradient: Some(radial_gradient(1.0, RadialExtent::ClosestSide)),
            ..Default::default()
        };
        let fc = StyleRules {
            background_gradient: Some(radial_gradient(1.0, RadialExtent::FarthestCorner)),
            ..Default::default()
        };
        assert_ne!(
            cs.content_key(),
            fc.content_key(),
            "RadialExtent must contribute to content_key — otherwise gradients with \
             identical center/radius but different extents share a CSS class and the \
             wrong one wins on apply",
        );
    }

    /// content_key for two identical gradients must MATCH (the dedup
    /// path relies on this — same content → same minted class).
    #[test]
    fn content_key_matches_for_identical_gradients() {
        let a = StyleRules {
            background_gradient: Some(radial_gradient(1.5, RadialExtent::FarthestCorner)),
            ..Default::default()
        };
        let b = StyleRules {
            background_gradient: Some(radial_gradient(1.5, RadialExtent::FarthestCorner)),
            ..Default::default()
        };
        assert_eq!(
            a.content_key(),
            b.content_key(),
            "identical gradient shape must collapse to one cached class",
        );
    }

    /// content_key must distinguish radial gradients with different
    /// stop offsets. Important for animations that interpolate stop
    /// positions independently of color.
    #[test]
    fn content_key_differentiates_gradient_stop_offsets() {
        let g_a = Gradient {
            kind: GradientKind::Linear { angle_deg: 45.0 },
            stops: vec![
                GradientStop { offset: 0.0, color: Color("#000".into()) },
                GradientStop { offset: 0.5, color: Color("#fff".into()) },
            ],
        };
        let g_b = Gradient {
            kind: GradientKind::Linear { angle_deg: 45.0 },
            stops: vec![
                GradientStop { offset: 0.0, color: Color("#000".into()) },
                GradientStop { offset: 0.8, color: Color("#fff".into()) },
            ],
        };
        let a = StyleRules { background_gradient: Some(g_a), ..Default::default() };
        let b = StyleRules { background_gradient: Some(g_b), ..Default::default() };
        assert_ne!(a.content_key(), b.content_key());
    }

    // ----- RadialExtent: default + round-trip ---------------------------

    /// `RadialExtent::default()` is `ClosestSide`. The agent's
    /// report calls this out as the documented default; pinning the
    /// constant down here so a future change to the default
    /// surfaces explicitly.
    #[test]
    fn radial_extent_default_is_closest_side() {
        let d: RadialExtent = RadialExtent::default();
        assert_eq!(d, RadialExtent::ClosestSide);
    }

    /// RadialExtent must round-trip through Clone + Copy + PartialEq
    /// — these are the derives the public API depends on.
    #[test]
    fn radial_extent_clone_and_eq_round_trip() {
        let cs = RadialExtent::ClosestSide;
        let fc = RadialExtent::FarthestCorner;
        // Copy semantics — moving doesn't consume.
        let cs_again = cs;
        let _still_cs = cs;
        assert_eq!(cs, cs_again);
        // Distinct variants compare unequal.
        assert_ne!(cs, fc);
        // Clone is independent of Copy (both must work).
        let fc_cloned = fc.clone();
        assert_eq!(fc, fc_cloned);
    }

    /// A `Gradient` carrying a non-default `extent` must survive
    /// being wrapped in a `Some(StyleRules { background_gradient:
    /// Some(...) })` and pulled back out. Catches the "field
    /// silently dropped" failure mode at the type-system level for
    /// the gradient struct itself.
    #[test]
    fn radial_extent_round_trips_through_stylerules_field() {
        let g = radial_gradient(1.5, RadialExtent::FarthestCorner);
        let rules = StyleRules {
            background_gradient: Some(g.clone()),
            ..Default::default()
        };
        let back = rules.background_gradient.unwrap();
        match back.kind {
            GradientKind::Radial { extent, .. } => {
                assert_eq!(extent, RadialExtent::FarthestCorner);
            }
            other => panic!("expected Radial, got {:?}", other),
        }
        assert_eq!(back, g);
    }

    // ----- aspect_ratio: round-trip + key + merge -----------------------

    /// `aspect_ratio` round-trips through `merge` (overlay-wins
    /// semantics). Same regression class as the gradient bug — if
    /// `aspect_ratio` were dropped from `overlay!()`, this fails.
    #[test]
    fn merge_overlays_aspect_ratio() {
        let base = StyleRules::default();
        let overlay = StyleRules {
            aspect_ratio: Some(1.5),
            ..Default::default()
        };
        let merged = base.merge(&overlay);
        assert_eq!(merged.aspect_ratio, Some(1.5));
    }

    /// `merge` preserves a base `aspect_ratio` when overlay is empty.
    #[test]
    fn merge_keeps_aspect_ratio_when_overlay_has_none() {
        let base = StyleRules {
            aspect_ratio: Some(2.0),
            ..Default::default()
        };
        let overlay = StyleRules::default();
        let merged = base.merge(&overlay);
        assert_eq!(merged.aspect_ratio, Some(2.0));
    }

    /// Overlay's `aspect_ratio` wins over base.
    #[test]
    fn merge_overlay_aspect_ratio_wins() {
        let base = StyleRules {
            aspect_ratio: Some(1.0),
            ..Default::default()
        };
        let overlay = StyleRules {
            aspect_ratio: Some(16.0 / 9.0),
            ..Default::default()
        };
        let merged = base.merge(&overlay);
        assert_eq!(merged.aspect_ratio, Some(16.0 / 9.0));
    }

    /// `content_key` distinguishes different aspect ratios — two
    /// otherwise-identical rule sets must mint distinct classes.
    /// Bench-relevant: a 16:9 video card and a 4:3 video card must
    /// not collapse onto the same `.uiX` class.
    #[test]
    fn content_key_differentiates_aspect_ratios() {
        let a = StyleRules {
            aspect_ratio: Some(16.0 / 9.0),
            ..Default::default()
        };
        let b = StyleRules {
            aspect_ratio: Some(4.0 / 3.0),
            ..Default::default()
        };
        assert_ne!(
            a.content_key(),
            b.content_key(),
            "content_key must include aspect_ratio so different ratios mint different classes",
        );
    }

    /// content_key for the same aspect ratio collapses (dedup
    /// invariant).
    #[test]
    fn content_key_matches_for_same_aspect_ratio() {
        let a = StyleRules {
            aspect_ratio: Some(1.5),
            ..Default::default()
        };
        let b = StyleRules {
            aspect_ratio: Some(1.5),
            ..Default::default()
        };
        assert_eq!(a.content_key(), b.content_key());
    }

    /// content_key distinguishes Some(ratio) from None — the
    /// "ratio of None" path is its own bucket.
    #[test]
    fn content_key_differentiates_some_aspect_ratio_from_none() {
        let with_ratio = StyleRules {
            aspect_ratio: Some(1.0),
            ..Default::default()
        };
        let without = StyleRules::default();
        assert_ne!(with_ratio.content_key(), without.content_key());
    }

    // --- cached_stylesheet (shared per-sheet registry) ---------------
    //
    // These exercise the registry that replaced the per-sheet
    // `thread_local!` the `stylesheet!` macro used to emit. The bug
    // being prevented is Android bionic exhausting its 128 pthread TLS
    // keys when a binary links 70+ stylesheets (each old `thread_local!`
    // burned a key → abort in `LazyKey::lazy_init` at mount). Key
    // exhaustion isn't reproducible on a host with a large key table, so
    // the closest reachable coverage is the registry's behavioral
    // contract: same key → same `Rc` (caching identity preserved),
    // distinct keys → distinct sheets, and reentrancy safety (a build
    // closure that itself caches another sheet must not double-borrow).

    fn empty_sheet() -> Rc<StyleSheet> {
        Rc::new(StyleSheet::new(|_vs| StyleRules::default()))
    }

    #[test]
    fn cached_stylesheet_same_key_returns_same_rc() {
        static K: u8 = 0;
        let key = &K as *const u8 as usize;
        let mut built = 0;
        let a = cached_stylesheet(key, || {
            built += 1;
            empty_sheet()
        });
        let b = cached_stylesheet(key, || {
            built += 1;
            empty_sheet()
        });
        // Built exactly once; both calls hand back the same allocation.
        assert_eq!(built, 1, "build closure must run only on first call");
        assert!(Rc::ptr_eq(&a, &b), "same key must return the cached Rc");
    }

    #[test]
    fn cached_stylesheet_distinct_keys_are_independent() {
        static K1: u8 = 0;
        static K2: u8 = 0;
        let a = cached_stylesheet(&K1 as *const u8 as usize, empty_sheet);
        let b = cached_stylesheet(&K2 as *const u8 as usize, empty_sheet);
        assert!(!Rc::ptr_eq(&a, &b), "distinct keys must not collide");
    }

    #[test]
    fn cached_stylesheet_reentrant_build_does_not_double_borrow() {
        // A sheet whose construction references another `*_style()` (a
        // nested sheet) caches the inner one mid-build. The outer build
        // must hold no borrow of the registry while it runs.
        static OUTER: u8 = 0;
        static INNER: u8 = 0;
        let outer = cached_stylesheet(&OUTER as *const u8 as usize, || {
            let _inner = cached_stylesheet(&INNER as *const u8 as usize, empty_sheet);
            empty_sheet()
        });
        // Both entries are now resident and resolve to themselves.
        let outer_again = cached_stylesheet(&OUTER as *const u8 as usize, empty_sheet);
        assert!(Rc::ptr_eq(&outer, &outer_again));
    }
}
