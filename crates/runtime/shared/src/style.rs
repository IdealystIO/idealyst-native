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
use std::collections::BTreeMap;
use rustc_hash::{FxHashMap, FxHashSet};
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
    /// - **macOS** — same registry model as iOS, but scroll-driven
    ///   instead of vsync-polled: an `NSViewBoundsDidChangeNotification`
    ///   observer on the scroll view's clip view reticks the pin, and
    ///   the pin moves the view's FRAME (`setFrameOrigin:` = Taffy y +
    ///   translate) rather than a layer transform — AppKit culls/purges
    ///   scroll-view drawing by frame, so a transform-pinned view's
    ///   drawn text goes blank once its frame scrolls out of the
    ///   prepared content rect. Vertical (`top`) only in v1; falls
    ///   back to `Relative` with no enclosing `NSScrollView`. See
    ///   `backend-macos/src/imp/sticky.rs`.
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

/// How a replaced element's content (an `image`'s bitmap) is fitted into
/// its layout box. Mirrors CSS `object-fit`. Only meaningful on the
/// `image` primitive — every other primitive silently ignores it (there is
/// no replaced content to fit).
///
/// The framework's default (a `None` field) is [`ObjectFit::Contain`] on
/// **every** backend — aspect-fit, no distortion, letterboxed if the box's
/// aspect differs. This deliberately overrides the browser's native `<img>`
/// default of `fill` (stretch): a bare `image` sized to a non-matching box
/// must look the same on web as on the native backends. Authors opt into
/// [`ObjectFit::Cover`] for the thumbnail/tile "fill and center-crop"
/// pattern.
///
/// Per-backend mapping:
/// - Web / SSR: CSS `object-fit: {fill|contain|cover}`.
/// - iOS (UIKit): `UIView.contentMode` = `scaleToFill` / `scaleAspectFit` /
///   `scaleAspectFill`.
/// - Android: `ImageView.ScaleType` = `FIT_XY` / `FIT_CENTER` / `CENTER_CROP`.
/// - macOS (AppKit): the image view's backing-layer `contentsGravity` =
///   `resize` / `resizeAspect` / `resizeAspectFill` (`NSImageView.imageScaling`
///   has no aspect-fill mode, so the macOS image path renders through the
///   layer's `contents`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ObjectFit {
    /// Stretch to fill the box exactly, ignoring aspect ratio (CSS `fill`).
    Fill,
    /// Scale to fit inside the box, preserving aspect ratio; letterbox the
    /// remainder (CSS `contain`). The framework default.
    #[default]
    Contain,
    /// Scale to fill the box, preserving aspect ratio; crop the overflow
    /// (CSS `cover`). The thumbnail/tile "fill + center-crop" mode.
    Cover,
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
#[derive(Debug, Default, PartialEq)]
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
    /// How an `image`'s bitmap fits its box (CSS `object-fit`). `None` ⇒
    /// the framework default [`ObjectFit::Contain`] on every backend.
    /// Ignored by every primitive except `image` (no replaced content to
    /// fit). See [`ObjectFit`].
    pub object_fit: Option<ObjectFit>,
    /// Drop shadow on the element's BOX (web `box-shadow`; native layer
    /// shadows). Always the box on every node kind — a glyph shadow on
    /// text is [`Self::text_shadow`]. The split is what makes shadowed
    /// sheets premintable: one field per CSS property, no node-kind
    /// dispatch at lowering time.
    pub shadow: Option<Shadow>,
    /// Drop shadow on a text node's GLYPHS (web `text-shadow`; native
    /// glyph/layer shadow on the label). Ignored by non-text primitives.
    pub text_shadow: Option<Shadow>,
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

// `Clone` is written by hand ONLY to carry `#[inline(never)]`: the derived
// impl compiles to ~6 KB of wasm (104 per-field `Option` clones), and LTO
// inlined a second full copy of it into the walker's
// `with_default_text_font`, shipping the struct-clone twice. Outlining pins
// exactly one copy; the call overhead is irrelevant next to the clone
// itself. The struct-literal body is compiler-checked for completeness — a
// new field fails compilation here until it's added. Correctness (no
// crossed same-typed fields) is pinned by
// `clone_round_trips_a_fully_populated_struct` in this module's tests.
impl Clone for StyleRules {
    #[inline(never)]
    fn clone(&self) -> Self {
        Self {
            background: self.background.clone(),
            color: self.color.clone(),
            caret_color: self.caret_color.clone(),
            font_size: self.font_size.clone(),
            display: self.display.clone(),
            grid_template_columns: self.grid_template_columns.clone(),
            flex_direction: self.flex_direction.clone(),
            flex_wrap: self.flex_wrap.clone(),
            justify_content: self.justify_content.clone(),
            align_items: self.align_items.clone(),
            align_content: self.align_content.clone(),
            gap: self.gap.clone(),
            row_gap: self.row_gap.clone(),
            column_gap: self.column_gap.clone(),
            flex_grow: self.flex_grow.clone(),
            flex_shrink: self.flex_shrink.clone(),
            flex_basis: self.flex_basis.clone(),
            align_self: self.align_self.clone(),
            width: self.width.clone(),
            height: self.height.clone(),
            min_width: self.min_width.clone(),
            min_height: self.min_height.clone(),
            max_width: self.max_width.clone(),
            max_height: self.max_height.clone(),
            aspect_ratio: self.aspect_ratio.clone(),
            padding_top: self.padding_top.clone(),
            padding_right: self.padding_right.clone(),
            padding_bottom: self.padding_bottom.clone(),
            padding_left: self.padding_left.clone(),
            margin_top: self.margin_top.clone(),
            margin_right: self.margin_right.clone(),
            margin_bottom: self.margin_bottom.clone(),
            margin_left: self.margin_left.clone(),
            border_top_left_radius: self.border_top_left_radius.clone(),
            border_top_right_radius: self.border_top_right_radius.clone(),
            border_bottom_left_radius: self.border_bottom_left_radius.clone(),
            border_bottom_right_radius: self.border_bottom_right_radius.clone(),
            border_top_width: self.border_top_width.clone(),
            border_right_width: self.border_right_width.clone(),
            border_bottom_width: self.border_bottom_width.clone(),
            border_left_width: self.border_left_width.clone(),
            border_top_color: self.border_top_color.clone(),
            border_right_color: self.border_right_color.clone(),
            border_bottom_color: self.border_bottom_color.clone(),
            border_left_color: self.border_left_color.clone(),
            position: self.position.clone(),
            top: self.top.clone(),
            right: self.right.clone(),
            bottom: self.bottom.clone(),
            left: self.left.clone(),
            font_family: self.font_family.clone(),
            font_weight: self.font_weight.clone(),
            font_style: self.font_style.clone(),
            line_height: self.line_height.clone(),
            letter_spacing: self.letter_spacing.clone(),
            text_align: self.text_align.clone(),
            underline: self.underline.clone(),
            strikethrough: self.strikethrough.clone(),
            text_transform: self.text_transform.clone(),
            opacity: self.opacity.clone(),
            overflow: self.overflow.clone(),
            object_fit: self.object_fit.clone(),
            shadow: self.shadow.clone(),
            text_shadow: self.text_shadow.clone(),
            background_gradient: self.background_gradient.clone(),
            transform: self.transform.clone(),
            transform_origin: self.transform_origin.clone(),
            cursor: self.cursor.clone(),
            user_select: self.user_select.clone(),
            pointer_events: self.pointer_events.clone(),
            background_transition: self.background_transition.clone(),
            color_transition: self.color_transition.clone(),
            caret_color_transition: self.caret_color_transition.clone(),
            opacity_transition: self.opacity_transition.clone(),
            transform_transition: self.transform_transition.clone(),
            width_transition: self.width_transition.clone(),
            height_transition: self.height_transition.clone(),
            max_width_transition: self.max_width_transition.clone(),
            max_height_transition: self.max_height_transition.clone(),
            min_width_transition: self.min_width_transition.clone(),
            min_height_transition: self.min_height_transition.clone(),
            top_transition: self.top_transition.clone(),
            right_transition: self.right_transition.clone(),
            bottom_transition: self.bottom_transition.clone(),
            left_transition: self.left_transition.clone(),
            padding_top_transition: self.padding_top_transition.clone(),
            padding_right_transition: self.padding_right_transition.clone(),
            padding_bottom_transition: self.padding_bottom_transition.clone(),
            padding_left_transition: self.padding_left_transition.clone(),
            margin_top_transition: self.margin_top_transition.clone(),
            margin_right_transition: self.margin_right_transition.clone(),
            margin_bottom_transition: self.margin_bottom_transition.clone(),
            margin_left_transition: self.margin_left_transition.clone(),
            border_top_left_radius_transition: self.border_top_left_radius_transition.clone(),
            border_top_right_radius_transition: self.border_top_right_radius_transition.clone(),
            border_bottom_left_radius_transition: self.border_bottom_left_radius_transition.clone(),
            border_bottom_right_radius_transition: self.border_bottom_right_radius_transition.clone(),
            border_top_width_transition: self.border_top_width_transition.clone(),
            border_right_width_transition: self.border_right_width_transition.clone(),
            border_bottom_width_transition: self.border_bottom_width_transition.clone(),
            border_left_width_transition: self.border_left_width_transition.clone(),
            border_top_color_transition: self.border_top_color_transition.clone(),
            border_right_color_transition: self.border_right_color_transition.clone(),
            border_bottom_color_transition: self.border_bottom_color_transition.clone(),
            border_left_color_transition: self.border_left_color_transition.clone(),
        }
    }
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
            opacity, overflow, object_fit, shadow, text_shadow, background_gradient,
            transform, transform_origin,
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
        write_enum(&mut s, "objf", self.object_fit.map(|x| x as u8));
        if let Some(sh) = &self.shadow {
            s.push_str("sh=");
            push_u32_hex(&mut s, sh.x.to_bits());
            push_u32_hex(&mut s, sh.y.to_bits());
            push_u32_hex(&mut s, sh.blur.to_bits());
            s.push_str(&sh.color.0);
            s.push(';');
        }
        if let Some(sh) = &self.text_shadow {
            s.push_str("tsh=");
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

/// Insert (or update the default of) an author axis in the premint
/// cache, keeping it sorted by axis name.
///
/// Sorted, not declaration-ordered, because this list has to match the
/// order `premint-dump` emits author-axis rules in — and that order is
/// `BTreeMap`-alphabetical, because the dump walks `variants` directly.
/// Class-list order does not decide the CSS cascade (every emitted
/// selector is specificity (0,1,0), so source order in the STYLESHEET
/// wins), but keeping the two sides literally identical makes the
/// agreement checkable, which is what
/// `every_runtime_stamped_class_has_a_dumped_rule` checks. It also keeps
/// the stamped attribute byte-stable for SSR/SSG diffing.
fn insert_author_axis(
    axes: &mut Vec<(VariantAxis, Option<VariantValue>)>,
    axis: &str,
    default: Option<VariantValue>,
) {
    match axes.binary_search_by(|(a, _)| a.as_str().cmp(axis)) {
        Ok(i) => {
            if default.is_some() {
                axes[i].1 = default;
            }
        }
        Err(i) => axes.insert(i, (axis.to_string(), default)),
    }
}

/// The preminted base class for a sheet identity — `iy-` plus the low
/// 48 bits of FNV-1a 64 over `identity`, hex.
///
/// Byte-for-byte the scheme `stylesheet!` uses over its own source text
/// (`macros::stylesheet::content_hash` + the `iy-{:012x}` format), so
/// macro sheets and runtime-assembled ones share one namespace and one
/// CSS file. FNV rather than `DefaultHasher` because the value has to
/// mean the same thing in two separately-compiled binaries — the dump
/// and the shipped bundle — and `DefaultHasher`'s output is explicitly
/// not guaranteed stable across toolchain versions.
pub fn premint_class_name(identity: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in identity.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("iy-{:012x}", h & 0xffff_ffff_ffff)
}

thread_local! {
    /// The set of premint base classes the LOADED CSS asset actually
    /// contains — `None` until (unless) a backend installs it. See
    /// [`install_minted_classes`].
    static MINTED_CLASSES: std::cell::RefCell<Option<rustc_hash::FxHashSet<String>>> =
        const { std::cell::RefCell::new(None) };
}

/// Install the set of premint classes the shipped CSS asset contains.
/// The web backend scans the loaded stylesheets at boot and calls this,
/// which arms the guard in [`StyleApplication::preminted_attach_class_list`]:
/// a sheet whose class is NOT in the asset (it was constructed on a path
/// the dump's crawl never reached) falls back to the live engine instead
/// of silently stamping a class no CSS rule matches. Backends that never
/// call this (native, tests, a build whose CSS failed to load) leave the
/// guard disarmed — classes are assumed minted, which is exactly the
/// pre-guard behavior.
pub fn install_minted_classes(classes: impl IntoIterator<Item = String>) {
    MINTED_CLASSES.with(|m| {
        *m.borrow_mut() = Some(classes.into_iter().collect());
    });
}

/// Whether `base` is known to have CSS in the shipped premint asset.
/// `true` when the guard is disarmed (no set installed).
pub fn minted_class_known(base: &str) -> bool {
    MINTED_CLASSES.with(|m| match m.borrow().as_ref() {
        Some(set) => set.contains(base),
        None => true,
    })
}

/// Class the REACTIVE preminted attach paths stamp alongside the sheet's
/// class list, restoring CSS font inheritance over the dump's default-font
/// hook. Shared name between the dump (which emits its one rule,
/// `.iy-font-inherit { font-family: inherit }`, FIRST in the asset) and
/// the runtime (which stamps it).
///
/// Why it exists — the live engine's default-font semantics are
/// per-PATH: a STATIC application folds the theme default into rules
/// that name no `font_family`; a REACTIVE one deliberately doesn't, so
/// the node inherits — and under an author font on an ancestor (a brand
/// `font_family: &TYPEFACE` on the root container) inheritance yields
/// the AUTHOR's font, not the theme default. One build-time rule body
/// can't encode both, so the cascade does it:
///
/// - the dump's hook rides a specificity-(0,0,0) companion
///   (`:where(.iy-<hash>) { font-family: var(--iy-default-font, inherit) }`),
/// - this class is (0,1,0) and emitted before every sheet, so it beats
///   the hook on specificity and loses to any DECLARED `font-family`
///   (base, arm, or overlay) on order or specificity.
///
/// Static preminted nodes don't carry it → hook applies → theme default,
/// exactly the live fold. Reactive preminted nodes do → inherit, exactly
/// the live reactive path. Measured: without this, every reactive-styled
/// node on a brand-fonted site rendered the theme default under
/// `--premint` while the live build rendered the brand font.
pub const PREMINT_FONT_INHERIT_CLASS: &str = "iy-font-inherit";

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
    static STYLESHEET_CACHE: RefCell<FxHashMap<usize, Rc<StyleSheet>>> =
        RefCell::new(FxHashMap::default());
}

/// Returns the thread-cached `Rc<StyleSheet>` for `key`, building and
/// caching it on first call. `key` must be process-unique per logical
/// stylesheet — the `stylesheet!` macro passes the address of a
/// function-local `static` so distinct sheets never collide and the same
/// sheet always maps to the same entry.
///
/// Reentrancy-safe: `build` runs with no borrow of the cache held, so a
/// stylesheet whose construction references another `*_style()` (nested
/// Class for the `if`-empty-branch anchor sheet
/// ([`empty_absolute_sheet`]) — a hand-picked literal rather than a
/// content hash because the runtime and the dump share it as a plain
/// constant (same trick as the macro's source-hash classes, minus the
/// hashing: there is exactly one of these).
pub const EMPTY_ABSOLUTE_CLASS: &str = "iy-empty-absolute";

/// The layout-neutral empty-branch sheet: `position: absolute`, so a
/// false `if` contributes no flex slot (see the vocabulary's
/// `empty_absolute_view`). A NAMED, link-time-registered sheet rather
/// than raw `StyleRules` so the one style the FRAMEWORK itself emits
/// premints — as the last un-preminted style on the website corpus it
/// single-handedly kept the live engine reachable under
/// `--premint-only`. Registered below via the same distributed slice
/// the `stylesheet!` macro uses, because an `if` may be true throughout
/// the dump crawl and false for the first time at runtime — first-use
/// registration would leave the class without CSS exactly then.
pub fn empty_absolute_sheet() -> Rc<StyleSheet> {
    static KEY: u8 = 0;
    cached_stylesheet(&KEY as *const u8 as usize, || {
        StyleSheet::r#static(StyleRules {
            position: Some(Position::Absolute),
            ..Default::default()
        })
        .premint_with_class(EMPTY_ABSOLUTE_CLASS)
    })
}


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
/// 4. Any [`StyleApplication::with_computed`] layer.
/// 5. Any `StyleApplication::overrides` field.
///
/// ## Axes merge in ALPHABETICAL axis-name order
///
/// Step 2 walks [`Self::variants`], which is a [`BTreeMap`] — so when two
/// axes set the SAME property, the alphabetically later axis name wins,
/// regardless of the order the axes were declared in. A `"tone"` arm and a
/// `"variant"` arm that both set `background` resolve to the `"variant"`
/// one, because `"tone" < "variant"`.
///
/// This is load-bearing and easy to trip over. Two ways to get a
/// deterministic winner without depending on the names:
///
/// - **Fold the conflicting axes into one.** idea-theme's Badge/Tag/Alert
///   sheets key a single `appearance` axis as `{tone}_{variant}` for exactly
///   this reason — one axis, no cross-axis ordering to reason about.
/// - **Use a later resolution step.** A computed layer (step 4) resolves
///   after every axis. idea-ui's `Card` tints via a computed layer because
///   its tint must beat the `variant` axis' surface background.
///
/// Compound variants (step 3) also resolve after all axes, but they are
/// runtime-API-only and the premint dump rejects them — reach for one only
/// on a sheet that never premints.
pub struct StyleSheet {
    base: RulesFn,
    /// axis → axis definition (default + per-value closures).
    ///
    /// `BTreeMap`, so iteration — and therefore merge precedence between
    /// axes that touch the same property — is alphabetical by axis name.
    /// See the type-level "Axes merge in ALPHABETICAL axis-name order"
    /// section; `axis_merge_precedence_is_alphabetical_not_declaration_order`
    /// pins it.
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
    variant_cache: std::cell::RefCell<FxHashMap<VariantSet, Rc<StyleRules>>>,
    /// Cached list of AUTHOR variant axes (the `__`-prefixed overlay
    /// axes excluded), each paired with its declared default. Populated
    /// in `.variant(...)` / `.variant_default(...)` like the overlay
    /// caches beside it, but kept SORTED by axis name rather than in
    /// declaration order — see [`insert_author_axis`].
    ///
    /// Exists for the preminted class assembly, which must name one
    /// class per author axis — including axes the call site left unset,
    /// whose default arm the live resolver would have applied. The
    /// `style-dump`-only `premint_variant_axes()` can't serve that: it
    /// allocates a `Vec<(String, Vec<String>, Option<String>)>` per
    /// call, and shipped premint builds hit this per styled node.
    author_axes: Vec<(VariantAxis, Option<VariantValue>)>,
    /// Base class for a sheet that premints (see
    /// [`Self::premint_as`]). `None` — the default — means the sheet
    /// only ever resolves through the live engine.
    premint_class: Option<Rc<str>>,
    /// Whether `premint_class` is the CONTENT-DERIVED auto class a
    /// [`Self::r#static`] constructor assigned (vs. an explicit
    /// `premint_as`/`premint_with_class` identity). The auto class names
    /// exactly the base rules, so any layer added afterwards
    /// (`variant`/`variant_default`/`compound`) RETRACTS it — the sheet
    /// falls back to explicit identity or the live engine rather than
    /// stamping a class whose CSS misses the added layers.
    auto_preminted: bool,
    /// Source location this sheet was constructed at — `--premint-report`
    /// only, and the whole reason that flag is usable.
    ///
    /// The report's job is to name every style that falls through to the
    /// live engine, so an author can decide whether to convert it. Without
    /// an origin it printed a hash and a hex dump of resolved rules, which
    /// identifies a sheet only to whoever already knows the codebase — the
    /// three framework-owned fall-throughs found on the catalog took a
    /// property-by-property decode of that dump to locate. Captured via
    /// `#[track_caller]` on the constructors, so it points at the author's
    /// line, not at this file.
    #[cfg(idealyst_premint_report)]
    origin: Option<&'static std::panic::Location<'static>>,
}


/// The rule closure a `--premint-only` build stores in place of a real
/// one — see [`StyleSheet::variant`].
///
/// Under that flag the bundle ships build-time CSS and no style engine,
/// so no arm is ever resolved: the class list comes from the sheet's
/// premint class plus its author axes, none of which needs a `StyleRules`
/// body. Dropping the real closures is what removes them from the wasm
/// (78,813 bytes of idea-theme arms alone, measured on login-demo).
///
/// It PANICS rather than returning `StyleRules::default()` because a few
/// call sites resolve a sheet at runtime for a value they need in Rust —
/// `Icon` reads the resolved `color` to tint its SVG, `Tabs` likewise.
/// Empty rules would tint those silently wrong; this names the constraint
/// instead. Resolving a sheet at runtime IS the engine, so an app doing it
/// was already outside `--premint-only`'s contract.
#[cfg(idealyst_premint_only)]
fn premint_only_stripped_rules(_vs: &VariantSet) -> StyleRules {
    panic!(
        "this bundle was built with --premint-only, which ships build-time \
         CSS and NO style engine, so stylesheets carry no rule closures — \
         but something resolved one at runtime.\n\n\
         Styles that only get APPLIED to a node are fine: they resolve to a \
         preminted class. This is a style whose resolved `StyleRules` were \
         read back in Rust — `Icon` reading the resolved color to tint its \
         SVG is the usual source.\n\n\
         Drop --premint-only (--premint alone is always safe), or move the \
         value off style resolution (an icon can inherit `currentColor`)."
    )
}

impl StyleSheet {
    /// Constructs a stylesheet whose base rules are produced by `f`.
    #[cfg_attr(idealyst_premint_report, track_caller)]
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(&VariantSet) -> StyleRules + 'static,
    {
        Self {
            // `--premint-only`: drop the author's closure so its body is
            // never named and LLVM removes it. See
            // `premint_only_stripped_rules`.
            #[cfg(idealyst_premint_only)]
            base: {
                drop(f);
                Box::new(premint_only_stripped_rules)
            },
            #[cfg(not(idealyst_premint_only))]
            base: Box::new(f),
            variants: BTreeMap::new(),
            compounds: Vec::new(),
            state_axes: Vec::new(),
            breakpoint_axes: Vec::new(),
            container_axes: Vec::new(),
            author_axes: Vec::new(),
            premint_class: None,
            auto_preminted: false,
            #[cfg(idealyst_premint_report)]
            origin: Some(std::panic::Location::caller()),
            variant_cache: std::cell::RefCell::new(FxHashMap::default()),
        }
    }

    /// A stylesheet whose base rules ignore the variant set.
    #[cfg_attr(idealyst_premint_report, track_caller)]
    pub fn r#static(rules: StyleRules) -> Self {
        // AUTO-PREMINT: a static sheet's whole identity IS its rules, so
        // the build-time class derives from the content key — the dump
        // binary and the shipped bundle compute the same name from the
        // same rules independently, with no hand-written `premint_as`
        // identity. Content-equal sheets share one class (the dump dedups
        // on it). `.premint_as(...)` / `.premint_with_class(...)` still
        // REPLACE this class for sheets that want a stable name (the
        // parameterized identities like per-px icon sizes); they also
        // retract the auto registration so the dump doesn't emit a dead
        // duplicate rule under the auto class.
        let auto_class: Rc<str> =
            crate::premint_class_name(&format!("static|{}", rules.content_key())).into();
        #[cfg(feature = "style-dump")]
        crate::premint::register_static_rules(Rc::clone(&auto_class), rules.clone());
        Self {
            base: Box::new(move |_vs: &VariantSet| rules.clone()),
            variants: BTreeMap::new(),
            compounds: Vec::new(),
            state_axes: Vec::new(),
            breakpoint_axes: Vec::new(),
            container_axes: Vec::new(),
            author_axes: Vec::new(),
            premint_class: Some(auto_class),
            auto_preminted: true,
            #[cfg(idealyst_premint_report)]
            origin: Some(std::panic::Location::caller()),
            variant_cache: std::cell::RefCell::new(FxHashMap::default()),
        }
    }

    /// Drops a [`Self::r#static`]-assigned content-derived class. Called
    /// by every layer-adding mutator: the auto class names exactly the
    /// base rules, so once another layer exists the class's CSS would be
    /// missing that layer — retract rather than stamp a lie. Explicit
    /// `premint_as`/`premint_with_class` identities call this too, so the
    /// dump doesn't also emit a dead rule under the retired auto class.
    fn retract_auto_premint(&mut self) {
        if self.auto_preminted {
            #[cfg(feature = "style-dump")]
            if let Some(class) = self.premint_class.as_ref() {
                crate::premint::unregister_static_rules(class);
            }
            self.premint_class = None;
            self.auto_preminted = false;
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
        self.retract_auto_premint();
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
        // Author axes (everything not `__state_*` / `__bp_*` / `__cq_*`)
        // cached for preminted class assembly — see `author_axes`. The
        // default is filled in later by `variant_default`, which may run
        // before or after the arm that declares the value.
        if !axis.starts_with("__") {
            insert_author_axis(&mut self.author_axes, &axis, None);
        }
        let entry = self.variants.entry(axis).or_insert_with(|| VariantAxisDef {
            default: None,
            values: BTreeMap::new(),
        });
        // The arm's VALUE still registers — `premint_variant_axes` (dump)
        // and the class assembly both read the axis/value metadata. Only
        // the rules BODY goes.
        #[cfg(idealyst_premint_only)]
        {
            drop(f);
            entry.values.insert(value, Box::new(premint_only_stripped_rules));
        }
        #[cfg(not(idealyst_premint_only))]
        entry.values.insert(value, Box::new(f));
        self
    }

    /// The cached set of state-overlay axes declared on this
    /// stylesheet. Returns an empty slice for the common case of
    /// sheets with no `state` blocks. Used by
    /// `resolve_state_overlays` to skip per-call iteration of the
    /// full variants map.
    /// Pub (was crate-private) for the new-core vocabulary's overlay
    /// resolution: scanning `variant_keys()` per call allocates the full
    /// key list per styled node per fire — the cached slice is the
    /// empty-slice fast path the old walker used.
    pub fn state_axes(&self) -> &[(crate::StateBits, VariantAxis)] {
        &self.state_axes
    }

    /// The cached set of breakpoint-overlay axes declared on this
    /// stylesheet, in declaration order. Returns an empty slice for the
    /// common case of sheets with no `breakpoint` blocks. Used by
    /// `resolve_breakpoint_overlays` to skip per-call iteration of the
    /// full variants map. Parallel to [`Self::state_axes`].
    /// Pub for the same reason as [`Self::state_axes`].
    pub fn breakpoint_axes(&self) -> &[(crate::Breakpoint, VariantAxis)] {
        &self.breakpoint_axes
    }

    /// The cached set of container-query overlay axes declared on this
    /// stylesheet, each with its `min_width` threshold (px). Returns an
    /// empty slice for the common case of sheets with no `container`
    /// blocks. Used by `resolve_container_overlays` to skip per-call
    /// iteration of the full variants map. Parallel to
    /// [`Self::breakpoint_axes`].
    /// Pub for the same reason as [`Self::state_axes`].
    pub fn container_axes(&self) -> &[(f32, VariantAxis)] {
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
        self.retract_auto_premint();
        let axis = axis.into();
        let value = value.into();
        // Keep the premint author-axis cache in step. `variant_default`
        // may run before ANY `.variant(...)` arm for the axis, so the
        // axis is registered here too rather than only there.
        if !axis.starts_with("__") {
            insert_author_axis(&mut self.author_axes, &axis, Some(value.clone()));
        }
        let entry = self.variants.entry(axis).or_insert_with(|| VariantAxisDef {
            default: None,
            values: BTreeMap::new(),
        });
        entry.default = Some(value);
        self
    }

    /// The cached author variant axes, each with its declared default —
    /// the axes a preminted class must name. See [`Self::author_axes`].
    pub fn premint_author_axes(&self) -> &[(VariantAxis, Option<VariantValue>)] {
        &self.author_axes
    }

    /// This sheet's preminted base class, or `None` when it has no
    /// build-time CSS and must resolve through the live engine.
    pub fn premint_class(&self) -> Option<&str> {
        self.premint_class.as_deref()
    }

    /// Where this sheet was constructed (`--premint-report` builds only).
    /// See the [`origin`](Self::origin) field.
    #[cfg(idealyst_premint_report)]
    pub fn origin(&self) -> Option<&'static std::panic::Location<'static>> {
        self.origin
    }

    /// Finish an assembled sheet with a stable premint identity, so its
    /// applications can resolve to a build-time class instead of the
    /// runtime style engine.
    ///
    /// `identity` must describe the sheet's CONTENT: two runs that
    /// produce the same rules must pass the same string, and any change
    /// to the assembled rules must change it. The build-time dump and
    /// the shipped bundle both derive the class from this string, and
    /// they only agree because they run this same code over the same
    /// app — there is no manifest tying the halves together (the
    /// `stylesheet!` macro plays the identical trick with a hash of its
    /// own source tokens).
    ///
    /// This exists because premint was previously reachable only from
    /// the macro, whose whole variant space is known at expansion. A
    /// sheet assembled at RUNTIME — idea-theme's `TypographySheetBuilder`
    /// and friends, whose kinds/tones an app extends before
    /// `install_idea_theme` — has no such expansion site, so every one
    /// of those components fell through to the live engine and kept it
    /// linked. The dump build already runs `app()`, so by the time it
    /// asks for CSS the assembled sheets exist; registering here is what
    /// lets it see them.
    /// # The sheet MUST be constructed during the dump's build pass
    ///
    /// The dump binary runs `app()` — it BUILDS the element tree, it never
    /// mounts it. So a sheet created lazily at mount time (inside a
    /// component body, or behind a `move ||` style closure that only runs
    /// when a node attaches) is never registered, the dump emits no CSS for
    /// it, and the shipped bundle then stamps a build-time class with
    /// nothing behind it — a silently unstyled node.
    ///
    /// This is not hypothetical: `AppShell`'s scrim/panel/content sheets
    /// were converted and broke 144 of 400 elements on the component
    /// catalog (the shell collapsed — no `position: relative`, no
    /// `margin-left`), and Tooltip's and Spinner's were caught stamping
    /// three classes with no rules. Both were reverted.
    ///
    /// Safe: anything reached from `install_idea_theme(...)` or otherwise
    /// constructed while the tree is being built. Unsafe: anything created
    /// on first mount. When in doubt, build with `--premint-report` and
    /// check the page for stamped classes the stylesheet has no rule for.
    ///
    /// # Eligibility
    ///
    /// Compound variants premint (they lower to CSS compound selectors
    /// over the per-axis classes — see [`Self::premint_as`]); shadows
    /// premint too since the `shadow`/`text_shadow` split gave each
    /// field exactly one CSS property. The remaining macro-side
    /// disqualifier is a non-literal `font_family` (see the macro's
    /// `premintable` check).
    ///
    /// Callers are still responsible for the property-level rule: a layer
    /// that varies with the theme must be `Tokenized`, so the emitted CSS
    /// says `var(--token)` and a theme swap re-resolves it. Baking a
    /// concrete theme value (a `FontFamily`, an `f32` from
    /// `Spacing`/`Radius`) freezes whichever theme the dump build
    /// installed.
    /// Stamp a PRE-COMPUTED premint class — the `stylesheet!` macro's
    /// content hash, which it already spells into its own builder fast
    /// path and into `PREMINT_SHEETS`.
    ///
    /// Unlike [`Self::premint_as`] this does NOT register for the dump:
    /// macro sheets register at link time through the distributed slice,
    /// so their CSS is emitted either way. What stamping adds is the
    /// RUNTIME half. `StyleApplication::new(Foo::sheet())` — 112 call
    /// sites across idea-ui and the websites, and the dominant idiom by
    /// a wide margin — bypasses the generated builder, so it never
    /// reached the macro's premint branch and fell through to the live
    /// engine even though the class it needed was already sitting in the
    /// shipped `.css`. With the class on the sheet, the
    /// `IntoStyleProp for StyleApplication` path picks it up.
    ///
    /// Only called for macro-premintable sheets (no non-literal
    /// `font_family`), because only those register CSS. Compounds are
    /// fine — the dump emits them as compound selectors through the same
    /// `dump_sheet_parts` both registration paths share.
    pub fn premint_with_class(mut self, class: &'static str) -> Rc<StyleSheet> {
        self.retract_auto_premint();
        self.premint_class = Some(class.into());
        Rc::new(self)
    }

    /// Give this sheet a build-time class identity so the premint dump can
    /// emit its CSS and the runtime can stamp classes instead of resolving.
    ///
    /// Compound variants are premintable. A compound fires when several
    /// axes coincide, and since the runtime stamps ONE CLASS PER AXIS, that
    /// condition is just a CSS compound selector over those classes
    /// (`.iy-abc-appearance-solid.iy-abc-size-md`, or
    /// `.iy-abc-appearance-solid:hover` when one leg is a state axis). It
    /// needs no extra stamped class — the selector does the matching — and
    /// it lands at specificity (0,2,0), above the (0,1,0) single-axis arms,
    /// which reproduces `resolve`'s "compounds merge after every axis".
    ///
    /// This used to bail out (`if self.compounds.is_empty()`) and silently
    /// leave the sheet classless, which sent every application of it to the
    /// live engine. That silently disqualified Button and IconButton — the
    /// state-overlay helpers attach a hover and a press compound per
    /// appearance arm — and the failure was invisible: `premint_as` returned
    /// a sheet that simply had no class.
    pub fn premint_as(mut self, identity: &str) -> Rc<StyleSheet> {
        self.retract_auto_premint();
        self.premint_class = Some(crate::premint_class_name(identity).into());
        let sheet = Rc::new(self);
        #[cfg(feature = "style-dump")]
        if sheet.premint_class.is_some() {
            crate::premint::register_assembled_sheet(&sheet);
        }
        sheet
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
        self.retract_auto_premint();
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

    // -----------------------------------------------------------------
    // Premint delta introspection (style-dump builds only)
    // -----------------------------------------------------------------

    /// The base rules alone, with declared defaults filled into the
    /// variant set the closure receives. Premint-dump only: the dump
    /// emits this once per sheet as the `.iy-<hash>` rule; every arm is
    /// emitted separately as a DELTA rule and the CSS cascade performs
    /// the merge [`resolve`](Self::resolve) does at runtime.
    #[cfg(feature = "style-dump")]
    pub fn premint_base(&self) -> StyleRules {
        (self.base)(&self.effective_variants(&VariantSet::default()))
    }

    /// One arm's rules ALONE (not merged onto base) — the delta the
    /// premint dump emits as `.iy-<hash>-<axis>-<value>` (regular axes)
    /// or as a pseudo/`@media`/`@container` rule (`__state_*` /
    /// `__bp_*` / `__cq_*` axes, value `"on"`). Arms in the
    /// `stylesheet!` grammar never read the variant set (their theme
    /// binding is enforced-unused), so passing the defaults-filled
    /// empty set is exact.
    #[cfg(feature = "style-dump")]
    pub fn premint_delta(&self, axis: &str, value: &str) -> Option<StyleRules> {
        let vs = self.effective_variants(&VariantSet::default());
        self.variants
            .get(axis)
            .and_then(|def| def.values.get(value))
            .map(|f| f(&vs))
    }

    /// Every compound variant, in declaration order (the order
    /// [`resolve`](Self::resolve) applies them), as
    /// `(when, rules)`.
    ///
    /// `rules` is the compound's own contribution — the resolver merges
    /// exactly this on top of the already-merged base+axes, so it IS the
    /// delta and needs no subtraction.
    ///
    /// The closure is evaluated against the compound's own condition
    /// filled in over the sheet's defaults, matching what `resolve` would
    /// pass when that compound fires. (Like [`Self::premint_delta`], this
    /// is exact for `stylesheet!`-authored sheets, whose arm closures do
    /// not read the variant set.)
    #[cfg(feature = "style-dump")]
    pub fn premint_compounds(&self) -> Vec<(Vec<(String, String)>, StyleRules)> {
        self.compounds
            .iter()
            .map(|c| {
                let mut sel = VariantSet::default();
                for (axis, value) in &c.when {
                    sel = sel.with(axis.clone(), value.clone());
                }
                let vs = self.effective_variants(&sel);
                let when = c
                    .when
                    .iter()
                    .map(|(a, v)| (a.to_string(), v.to_string()))
                    .collect();
                (when, (c.rules)(&vs))
            })
            .collect()
    }

    /// The REGULAR variant axes (author `variant` blocks — state /
    /// breakpoint / container axes excluded), in the same
    /// alphabetical-`BTreeMap` order [`resolve`](Self::resolve) merges
    /// them in. The premint dump emits axis-delta rules in exactly this
    /// order and the runtime stamps one class per axis, so the CSS
    /// source-order cascade reproduces the resolver's later-wins merge.
    /// Each entry is `(axis, declared values, default value)`.
    #[cfg(feature = "style-dump")]
    pub fn premint_variant_axes(&self) -> Vec<(String, Vec<String>, Option<String>)> {
        self.variants
            .iter()
            .filter(|(axis, _)| !axis.starts_with("__"))
            .map(|(axis, def)| {
                (
                    axis.clone(),
                    def.values.keys().cloned().collect(),
                    def.default.clone(),
                )
            })
            .collect()
    }

    /// Style-dump-visible re-exports of the overlay-axis lists (the
    /// crate-internal accessors the walker uses). Orders are
    /// load-bearing: states in declaration order (matches
    /// `resolve_state_overlays` → live web's rule order); breakpoints /
    /// containers get sorted by the dump (rank / threshold ascending,
    /// matching the walker's resolvers).
    #[cfg(feature = "style-dump")]
    pub fn premint_state_axes(&self) -> &[(crate::StateBits, VariantAxis)] {
        self.state_axes()
    }

    /// The [`StateBits`](crate::StateBits) an axis name denotes, if it is one
    /// of this sheet's declared `__state_*` axes.
    ///
    /// The dump needs this to lower a compound variant: a compound leg naming
    /// a state axis becomes that state's pseudo-class rather than a stamped
    /// class, because states are never stamped as classes.
    #[cfg(feature = "style-dump")]
    pub fn premint_state_axis_bit(&self, axis: &str) -> Option<crate::StateBits> {
        self.state_axes()
            .iter()
            .find(|(_, declared)| declared.as_str() == axis)
            .map(|(bit, _)| *bit)
    }

    #[cfg(feature = "style-dump")]
    pub fn premint_breakpoint_axes(&self) -> &[(crate::Breakpoint, VariantAxis)] {
        self.breakpoint_axes()
    }

    #[cfg(feature = "style-dump")]
    pub fn premint_container_axes(&self) -> &[(f32, VariantAxis)] {
        self.container_axes()
    }

    /// `true` when the sheet declares runtime-API compound variants —
    /// which the premint delta model cannot express (the `stylesheet!`
    /// grammar never emits them, so macro-registered sheets are always
    /// compound-free; this is the dump's defensive check).
    #[cfg(feature = "style-dump")]
    pub fn has_compounds(&self) -> bool {
        !self.compounds.is_empty()
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
    /// Per-instance values that are NOT part of the resolution cache
    /// identity — see [`Self::with_inline`].
    ///
    /// Deliberately last in the merge and deliberately absent from
    /// [`ResolutionKey`]: this is the layer for continuously-varying
    /// geometry (a slider thumb's `left`, a grid's track count), where
    /// every distinct value would otherwise mint its own cache entry and
    /// its own CSS class.
    inline: Option<Rc<StyleRules>>,
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
            inline: None,
        }
    }

    /// Attach per-instance rules that bypass the resolution cache and are
    /// applied as an INLINE style rather than a class.
    ///
    /// This is the layer for values that vary continuously per instance —
    /// a slider thumb's `left`, a progress bar's `width`, a grid's track
    /// count, a modal's measured `max_height`. Those cannot be keyed:
    /// [`ResolutionKey`] is `(sheet, variants, computed_key, overrides)`,
    /// so a `with_computed` layer keyed on the value, or an `override_*`
    /// carrying it, mints a fresh cache entry — and on web a fresh CSS
    /// class — for EVERY distinct value. Dragging a slider across a 300px
    /// track that way generates on the order of 300 classes, none of which
    /// can ever be reused, and the cache it populates can never hit.
    ///
    /// The inline layer is excluded from the cache identity precisely so
    /// that doesn't happen. The consequences follow from that:
    ///
    /// - The application stays PREMINTABLE. `preminted_class_list` ignores
    ///   this layer, so the sheet's arms still ship as build-time CSS and
    ///   the node gets the preminted classes plus an inline style.
    /// - It resolves LAST, after overrides — matching CSS, where an inline
    ///   `style` attribute beats any class rule.
    /// - Two applications differing only here share a cache entry. That is
    ///   the point, and it is why the values must be applied OUT of band
    ///   rather than merged into the cached rules.
    ///
    /// Use a variant arm for anything enumerable, `with_computed` for a
    /// bounded set of expensive-to-derive rules, and this for values drawn
    /// from a continuum.
    pub fn with_inline(mut self, rules: StyleRules) -> Self {
        self.inline = Some(Rc::new(rules));
        self
    }

    /// The inline layer, if any. Backends that stamp preminted classes
    /// apply this separately; the live engine folds it in during `resolve`.
    pub fn inline(&self) -> Option<&Rc<StyleRules>> {
        self.inline.as_ref()
    }

    /// Lookup-friendly accessor for the overrides flag. Used by
    /// `resolve()` to pick between the empty-overrides key (just an
    /// empty string) and the full content-keyed path.
    /// The preminted class list for this application: the sheet's base
    /// class, then one class per AUTHOR axis — or `None` when the sheet
    /// has no build-time CSS, or this application carries runtime-valued
    /// layers no build-time class could have named.
    ///
    /// Lives here, beside the sheet, because two independently-compiled
    /// binaries have to agree on it: `premint-dump` emits `{base}` and
    /// `{base}-{axis}-{value}` selectors, and the shipped bundle stamps
    /// the classes. Nothing but the string format tied those halves
    /// together, so they share this one function rather than each
    /// spelling the format out.
    ///
    /// Axis order is the sheet's cached author-axis order, which is
    /// `BTreeMap`-alphabetical, which is the order [`StyleSheet::resolve`]
    /// merges arms in and the order the dump emits them in — so the
    /// equal-specificity CSS cascade lands on the same winner the live
    /// resolver picks.
    ///
    /// Axes the call site left unset contribute their DECLARED DEFAULT,
    /// because that is the arm `resolve` would apply; omitting it would
    /// silently drop a layer. An axis with neither a set value nor a
    /// default contributes nothing, also matching `resolve`.
    pub fn preminted_class_list(&self) -> Option<String> {
        // Both disqualifiers are runtime-valued layers the dump could
        // not have seen: `overrides` are per-call-site rules and
        // `computed` is an arbitrary closure, so this application's
        // resolved rules are not the ones any build-time class names.
        //
        // The INLINE layer is deliberately NOT a disqualifier. It is also
        // runtime-valued, but it never claims to be part of a class: the
        // caller stamps these classes AND applies the inline rules as an
        // inline style, which beats them in the cascade exactly as the
        // merge order says it should. That is what lets a slider or a grid
        // ship its sheet as build-time CSS while its one continuous value
        // stays per-instance.
        if self.has_overrides || self.computed.is_some() {
            return None;
        }
        let base = self.sheet.premint_class()?;
        let mut class = String::with_capacity(base.len());
        class.push_str(base);
        for (axis, default) in self.sheet.premint_author_axes() {
            let Some(value) = self.variants.0.get(axis).or(default.as_ref()) else {
                continue;
            };
            class.push(' ');
            class.push_str(base);
            class.push('-');
            class.push_str(axis);
            class.push('-');
            class.push_str(value);
        }
        Some(class)
    }

    pub fn has_overrides(&self) -> bool {
        self.has_overrides
    }

    /// Whether ATTACHING this application stamps preminted classes
    /// instead of resolving through the live engine — the exact
    /// condition the attach paths (`IntoStyleProp for StyleApplication`,
    /// the `SheetDynamic` per-evaluation divert) use: the build carries
    /// build-time CSS (`--cfg idealyst_premint`, which `--premint-only`
    /// implies) AND this application premints
    /// ([`Self::preminted_class_list`]).
    ///
    /// This is the gate for components that read a resolved value back
    /// in Rust (Button tints its icon with the resolved fill color).
    /// When the application premints, the value already ships in the
    /// build-time CSS — on web the icon inherits it as `currentColor` —
    /// and under `--premint-only` the read-back would panic (sheets
    /// carry no rule closures). When it doesn't (native builds, live
    /// web builds, runtime-overridden applications), the resolved read
    /// is both safe and required — native nodes don't inherit color.
    ///
    /// Checked per evaluation, not once per component: the same closure
    /// can produce a preminting application on one evaluation and an
    /// overridden (non-preminting) one on the next.
    pub fn attaches_preminted(&self) -> bool {
        #[cfg(idealyst_premint)]
        {
            self.preminted_attach_class_list().is_some()
        }
        #[cfg(not(idealyst_premint))]
        false
    }

    /// [`Self::preminted_class_list`] with the MINTED-CLASS GUARD: the
    /// attach paths use this, so a sheet whose class has no CSS in the
    /// shipped asset (it was constructed on a path the dump's crawl
    /// never reached — a modal opened for the first time at runtime,
    /// say) resolves through the live engine instead of silently
    /// stamping a class nothing matches. Only the BASE class is
    /// checked: axis-arm rules ship in the same asset as their base.
    /// The guard is disarmed (`None` installed) everywhere except web
    /// premint boots — see [`install_minted_classes`].
    pub fn preminted_attach_class_list(&self) -> Option<String> {
        let list = self.preminted_class_list()?;
        let base = list.split(' ').next().unwrap_or(list.as_str());
        if !crate::minted_class_known(base) {
            return None;
        }
        Some(list)
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
///
/// `PartialEq` matters: token signals use the equality-guarded
/// `Signal::set`, so a theme (re)install wakes only subscribers of
/// tokens whose value actually changed. Unconditional re-tint on
/// re-install is carried separately by `bump_tokens_version()`.
#[derive(Clone, Debug, PartialEq)]
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
    static TOKEN_REGISTRY: RefCell<FxHashMap<&'static str, crate::Signal<TokenValue>>> =
        RefCell::new(FxHashMap::default());

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
    static RESOLUTION_CACHE: RefCell<FxHashMap<ResolutionKey, Rc<StyleRules>>> =
        RefCell::new(FxHashMap::default());

    /// Each currently-registered stylesheet, with the rules that were
    /// pre-generated for it and a `Weak<StyleSheet>` used to detect
    /// when the stylesheet has been dropped by all holders. The
    /// framework calls `Backend::register_stylesheet` exactly once per
    /// sheet and tracks the rules so we can later call
    /// `unregister_stylesheet` to free backend-side state.
    static REGISTRATIONS: RefCell<FxHashMap<RegKey, Registration>> =
        RefCell::new(FxHashMap::default());

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

    /// The installed theme's default text [`FontFamily`], if any. A text
    /// node whose resolved style sets no `font_family` inherits this at
    /// apply time (see `walker::style::apply_one`) so the theme's font is
    /// the true global default on EVERY platform — not just where a sheet
    /// opted in. Crucially this keeps web text out of the browser's serif
    /// fallback without relying on CSS inheritance (which native lacks).
    /// `idea-theme` sets it from `theme.font` on install + theme swap; the
    /// author's explicit `font_family` always wins (fill only when `None`).
    static DEFAULT_TEXT_FONT: RefCell<Option<FontFamily>> =
        const { RefCell::new(None) };

    /// Typefaces already registered with the backend this session.
    /// Drives the dedup in [`ensure_typefaces_registered_with`]: the
    /// framework calls `register_asset` + `register_typeface` once
    /// per unique `TypefaceId` no matter how many stylesheets — or
    /// rules within a stylesheet — reference the same typeface.
    static REGISTERED_TYPEFACES: RefCell<crate::collections::SmallIdSet<TypefaceId>> =
        RefCell::new(crate::collections::SmallIdSet::new());

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
    static REGISTERED_FAMILY_NAMES: RefCell<FxHashSet<&'static str>> =
        RefCell::new(FxHashSet::default());

    /// Debug-only dedup for [`maybe_warn_unregistered_system_font`]:
    /// each suspicious `System(name)` warns exactly once per thread, so
    /// a stylesheet applied to thousands of nodes doesn't spam the log.
    #[cfg(debug_assertions)]
    static WARNED_SYSTEM_FONTS: RefCell<FxHashSet<String>> =
        RefCell::new(FxHashSet::default());

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
#[doc(hidden)]
pub fn subscribe_to_all_token_signals() {
    TOKEN_REGISTRY.with(|r| {
        for sig in r.borrow().values() {
            let _ = sig.get();
        }
    });
}

thread_local! {
    /// Monotone tokens VERSION, bumped once per `update_tokens` /
    /// `install_tokens` batch. Style effects subscribe to THIS signal
    /// (via [`tokens_version_signal`]) instead of relying on the
    /// per-token signal reads inside `Tokenized::resolve()` — those
    /// reads only happen on resolution-cache MISSES, so among N
    /// effects sharing one resolution key only the first resolver
    /// subscribed and every later one went permanently deaf to theme
    /// changes. The visible collapse: each theme toggle re-applied
    /// fewer nodes than the last, until `color-surface` had 2-3
    /// subscribers left and a toggle repainted almost nothing
    /// (native only — web re-tints via the CSS `var()` cascade).
    static TOKENS_VERSION: std::cell::OnceCell<crate::Signal<u64>> =
        const { std::cell::OnceCell::new() };
}

/// The thread's tokens-version signal. Reading it inside a style
/// effect subscribes that effect to every subsequent theme/token
/// change, cache hits notwithstanding. Created unscoped (thread
/// lifetime), same contract as the per-token signals.
#[doc(hidden)]
pub fn tokens_version_signal() -> crate::Signal<u64> {
    TOKENS_VERSION.with(|cell| {
        *cell.get_or_init(|| crate::reactive::unscope(|| crate::Signal::new(0u64)))
    })
}

/// Bump the tokens version. Call inside the same `batch` as the
/// per-token sets so subscribers coalesce into one re-run.
fn bump_tokens_version() {
    let sig = tokens_version_signal();
    sig.update(|v| *v += 1);
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
    // Theme (re)install counts as a token change for version
    // subscribers — a test or hot-swap that re-installs must re-tint.
    bump_tokens_version();
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
        bump_tokens_version();
        for entry in tokens {
            let existing = TOKEN_REGISTRY.with(|r| r.borrow().get(entry.name).copied());
            match existing {
                Some(sig) => {
                    #[cfg(feature = "debug-stats")]
                    if entry.name == "color-surface" {
                        crate::logging::log(
                            crate::logging::LogLevel::Info,
                            &format!(
                                "[tokens] color-surface → {:?} (signal {} subscribers {})",
                                entry.value,
                                sig.id(),
                                crate::reactive::debug_subscriber_count_by_raw(sig.id()),
                            ),
                        );
                    }
                    sig.set(entry.value.clone());
                }
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

/// Install the theme's default text [`FontFamily`] — the font a text node
/// falls back to when its resolved style sets no `font_family`. `idea-theme`
/// calls this from `install_idea_theme` (and on theme swap) with
/// `theme.font`. Pass `None` to clear (text then uses the platform default).
/// Applied cross-platform at style-apply time; an explicit `font_family` on
/// the node always wins. See [`default_text_font`].
pub fn set_default_text_font(font: Option<FontFamily>) {
    DEFAULT_TEXT_FONT.with(|f| *f.borrow_mut() = font);
}

/// The installed theme's default text [`FontFamily`], if one is set. Read by
/// [`apply_one`](crate::walker::style) to fill a text node's absent
/// `font_family`. See [`set_default_text_font`].
pub fn default_text_font() -> Option<FontFamily> {
    DEFAULT_TEXT_FONT.with(|f| f.borrow().clone())
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
    registered: &FxHashSet<&'static str>,
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
#[doc(hidden)]
pub fn maybe_warn_unregistered_system_font(name: &str) {
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

/// Drain the queued host-level state — pending token installs/updates,
/// app background, scrollbar theme, app key handler — into the backend
/// via the given closures.
///
/// Historically this ran only as the prologue of
/// [`ensure_registered_with`], i.e. it rode *sheet registration*. A
/// fully-preminted app registers no sheets (its rules ship as a static
/// `.css` asset), so the walker's premint host driver calls this
/// directly — otherwise `install_tokens` / `set_theme` state queued by
/// the theme SDK would never reach the backend and every `var(--…)`
/// in the preminted CSS would fall back. Both callers share this one
/// drain so the ordering invariant (tokens before host-surface
/// settings, see the inline comment) can't diverge.
#[doc(hidden)]
pub fn flush_pending_host_state<I, UPD, SAB, SST, SAK>(
    install_tokens: I,
    update_tokens: UPD,
    set_app_background: SAB,
    set_scrollbar_theme: SST,
    set_app_key_handler: SAK,
) where
    I: FnOnce(&[TokenEntry]),
    UPD: FnMut(&[TokenEntry]),
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
    flush_pending_host_state(
        install_tokens,
        update_tokens,
        set_app_background,
        set_scrollbar_theme,
        set_app_key_handler,
    );

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

/// [`pregenerate`] + SEED the sheet's pointer-keyed resolution fast
/// path — the same `insert_variant` population `ensure_registered_with`
/// performs. For EXTERNAL registration tables (the new core's per-world
/// sheet registry in `runtime-vocabulary`) that re-implement the
/// registration prologue outside this module: without the seed, every
/// `resolve()` for a registered sheet takes the slow `ResolutionKey`
/// path (variant-map clone + hash per call) AND returns content-equal
/// but pointer-distinct `Rc<StyleRules>`, defeating the backends'
/// pointer-keyed class caches (`mint_style_class_ptr_hit`) — measured
/// at ~2× the whole batched-repeat enqueue loop on the
/// js-framework-bench create_10k gate.
pub fn pregenerate_and_seed(sheet: &Rc<StyleSheet>) -> Vec<Rc<StyleRules>> {
    let keyed = pregenerate_keyed(sheet);
    for (variants, rc) in &keyed {
        sheet.insert_variant(variants.clone(), rc.clone());
    }
    keyed.into_iter().map(|(_, rc)| rc).collect()
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
    let cached = resolve_cached(app);
    // The inline layer merges AFTER the cache, never into it. Two
    // applications that differ only here share the cached entry — which is
    // the entire reason this layer exists (see
    // `StyleApplication::with_inline`): a slider thumb keyed on its pixel
    // position would otherwise mint a cache entry, and a CSS class, per
    // pixel dragged.
    //
    // The cost is a fresh `Rc` per resolve for inline-carrying nodes. That
    // is inherent — the values differ per instance — and it is bounded by
    // the number of such nodes, not by the number of distinct values.
    match &app.inline {
        None => cached,
        Some(inline) => Rc::new((*cached).clone().merge(inline)),
    }
}

/// The cached half of [`resolve`] — everything through the override layer.
/// Split out so the inline layer can sit outside the memo.
fn resolve_cached(app: &StyleApplication) -> Rc<StyleRules> {
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
mod tests;

#[cfg(test)]
mod premint_identity_tests {
    use super::*;

    /// `--premint-report`'s origin capture must stay on BOTH stylesheet
    /// constructors.
    ///
    /// The cfg that turns the field on is not one this test binary is
    /// built with, so no assertion on a value can observe a regression
    /// here (the same limitation `premint_only_surface.rs` documents).
    /// The spelling is what is observable, and it is load-bearing: without
    /// `#[track_caller]` the captured location is this file's constructor
    /// line for every sheet in the program, which is worse than no field
    /// at all — it looks like an answer.
    #[test]
    fn premint_report_origin_capture_stays_on_both_constructors() {
        let src = include_str!("style.rs");
        // Needles are assembled rather than written whole: this test lives
        // in the file it scans, so a literal would match itself.
        let track = concat!("#[cfg_attr(idealyst_premint_report, ", "track_caller)]");
        assert_eq!(
            src.matches(track).count(),
            2,
            "both `StyleSheet::new` and `StyleSheet::r#static` must capture \
             the author's line, not this file's"
        );
        let capture = concat!("origin: Some(std::panic::", "Location::caller())");
        assert_eq!(
            src.matches(capture).count(),
            2,
            "every StyleSheet constructor must fill the origin field"
        );
    }

    /// The class name is the ONLY thing joining the dump binary to the
    /// shipped bundle, so its scheme is a wire format: `iy-` + 12 hex.
    /// It must also match what `stylesheet!` produces for macro sheets,
    /// since both land in one CSS file and one namespace.
    #[test]
    fn premint_class_name_shape_matches_the_macro_scheme() {
        let c = premint_class_name("idea-theme.v1.badge|neutral,primary|filled");
        assert!(c.starts_with("iy-"), "{c}");
        assert_eq!(c.len(), 3 + 12, "{c}");
        assert!(c[3..].chars().all(|ch| ch.is_ascii_hexdigit()), "{c}");
        // Deterministic — two binaries derive it independently.
        assert_eq!(c, premint_class_name("idea-theme.v1.badge|neutral,primary|filled"));
        assert_ne!(c, premint_class_name("idea-theme.v1.badge|neutral,primary,brand|filled"));
    }

    /// A sheet with compound variants DOES premint.
    ///
    /// This test previously asserted the opposite — that a compound sheet
    /// declines, on the reasoning that "a compound cannot be expressed as
    /// per-axis classes." That reasoning was wrong. The runtime stamps one
    /// class per axis, so "these axes coincide" is precisely a CSS compound
    /// selector over those classes (`.iy-x-appearance-filled:hover`), which
    /// needs no extra stamped class and lands at (0,2,0) — above the (0,1,0)
    /// arms, reproducing `resolve`'s base → axes → compounds order.
    ///
    /// The old behavior was also silent: `premint_as` returned a sheet with
    /// no class, so idea-theme's Button and IconButton — whose hover/press
    /// feedback is keyed on `(appearance, __state_*)` — sent every
    /// application to the live engine with nothing to indicate why.
    /// `premint-dump`'s `compound_*` tests cover the lowering; this pins that
    /// the sheet is not disqualified up front.
    #[test]
    fn compound_sheet_premints() {
        let sheet = StyleSheet::new(|_vs: &VariantSet| StyleRules::default())
            .variant("appearance", "filled", |_vs| StyleRules::default())
            .compound(vec![("appearance", "filled"), ("__state_hovered", "on")], |_vs| {
                StyleRules::default()
            })
            .premint_as("test.compound.v1");
        assert!(
            sheet.premint_class().is_some(),
            "a compound sheet must premint — its condition is a compound selector"
        );
    }

    /// The author-axis cache drives class assembly: overlay axes are
    /// excluded (they ship as pseudo-class / `@media` CSS on the base
    /// class), defaults are recorded whichever order they are declared
    /// in, and the list is sorted to match the dump's emission order.
    #[test]
    fn author_axis_cache_excludes_overlays_and_sorts() {
        let sheet = StyleSheet::new(|_vs: &VariantSet| StyleRules::default())
            .variant("kind", "h1", |_vs| StyleRules::default())
            .variant_default("align", "left")
            .variant("align", "left", |_vs| StyleRules::default())
            .variant("__state_hovered", "on", |_vs| StyleRules::default())
            .variant("__bp_md", "on", |_vs| StyleRules::default())
            .variant_default("kind", "body");
        let axes: Vec<_> = sheet
            .premint_author_axes()
            .iter()
            .map(|(a, d)| (a.as_str(), d.as_deref()))
            .collect();
        assert_eq!(axes, vec![("align", Some("left")), ("kind", Some("body"))]);
    }
}
