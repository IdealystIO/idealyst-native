//! Framework core: primitives, Backend trait, render walker, reactivity.

mod reactive;
mod style;
pub mod primitives;

#[cfg(feature = "debug-stats")]
pub mod debug;

pub use reactive::{untrack, Effect, Ref, Signal};

use std::any::Any;
pub use style::{
    install_theme, pregenerate_for_theme, resolve as resolve_style, set_theme,
    AlignContent, AlignItems, AlignSelf, Color, Easing, FlexDirection, FlexWrap, FontStyle,
    FontWeight, IntoOverrideSource, IntoVariantSource, JustifyContent, Length, Overflow, Position,
    Shadow, StyleApplication, StyleRules, StyleSheet, TextAlign, TextTransform, Transform,
    Transition, VariantAxis, VariantEnum, VariantSet, VariantValue,
};

pub use framework_macros::{component, jsx, stylesheet, ui};

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Source for a text node. Static is rendered once; Reactive is wrapped in
/// an `Effect` during rendering so the node updates whenever its dependencies
/// change. Author code never names this enum directly.
pub enum TextSource {
    Static(String),
    Reactive(Box<dyn Fn() -> String>),
}

/// Allows `text(...)` to accept strings, owned strings, or closures.
/// The `#[component]` macro rewrites reactive call sites into closures;
/// this trait makes the rewrite type-check transparently.
pub trait IntoTextSource {
    fn into_text_source(self) -> TextSource;
}

impl IntoTextSource for &str {
    fn into_text_source(self) -> TextSource {
        TextSource::Static(self.to_string())
    }
}

impl IntoTextSource for String {
    fn into_text_source(self) -> TextSource {
        TextSource::Static(self)
    }
}

impl<F> IntoTextSource for F
where
    F: Fn() -> String + 'static,
{
    fn into_text_source(self) -> TextSource {
        TextSource::Reactive(Box::new(self))
    }
}

/// A style source: either a fixed application (resolved once) or a
/// closure that re-runs (resolved every effect fire, picking up signal
/// changes the closure reads).
pub type StyleSource = Box<dyn Fn() -> StyleApplication>;

// =============================================================================
// Primitive handles + backend ops
// =============================================================================
//
// Each primitive kind has a corresponding handle type that the parent
// reaches via a `Ref<Handle>`. A handle is a thin record:
//   - `node`: an `Rc<dyn Any>` holding the backend's concrete node value
//     (`web_sys::HtmlButtonElement` on web, `View` on Android, …).
//   - `ops`: a `&'static dyn …Ops` trait object providing the kind's
//     methods. Backends ship a single ZST `Ops` impl per kind.
//
// This shape keeps `Ref<Handle>` backend-agnostic in user code while
// letting the backend implement methods against its native node type
// via a single downcast inside each op.

/// Bitflags for interaction states the framework recognizes. Backends
/// flip these bits when corresponding native events fire (hover,
/// press, focus, disabled state). Each bit corresponds to one of the
/// `__state_*` axes a `stylesheet!` may declare as `state hovered`
/// etc. — when the bit is on, the framework adds the axis to the
/// node's `StyleApplication` so the overlay applies.
///
/// Only the listed states are supported, matching the cross-platform
/// contract enforced by the `stylesheet!` macro. Adding more would
/// need backend + macro updates in lockstep.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct StateBits(pub u8);

impl StateBits {
    pub const HOVERED: StateBits = StateBits(1 << 0);
    pub const PRESSED: StateBits = StateBits(1 << 1);
    pub const FOCUSED: StateBits = StateBits(1 << 2);
    pub const DISABLED: StateBits = StateBits(1 << 3);

    pub const NONE: StateBits = StateBits(0);

    pub fn contains(self, other: StateBits) -> bool {
        (self.0 & other.0) == other.0
    }

    pub fn with(self, other: StateBits) -> StateBits {
        StateBits(self.0 | other.0)
    }

    pub fn without(self, other: StateBits) -> StateBits {
        StateBits(self.0 & !other.0)
    }

    /// The CSS-axis name for this bit, used in `StyleApplication`
    /// variant lookups. Returns `None` for empty (zero) bits.
    pub fn axis_name(self) -> Option<&'static str> {
        match self {
            Self::HOVERED => Some("__state_hovered"),
            Self::PRESSED => Some("__state_pressed"),
            Self::FOCUSED => Some("__state_focused"),
            Self::DISABLED => Some("__state_disabled"),
            _ => None,
        }
    }

    /// Iterate the set bits in this bitmask, yielding their
    /// `__state_*` axis names. Used by the framework to build a
    /// `VariantSet` for resolution from the current active states.
    pub fn active_axes(self) -> impl Iterator<Item = &'static str> {
        [Self::HOVERED, Self::PRESSED, Self::FOCUSED, Self::DISABLED]
            .into_iter()
            .filter(move |&bit| self.contains(bit))
            .filter_map(|bit| bit.axis_name())
    }
}

/// A handle to a mounted `Button` primitive.
///
/// `Clone` is cheap: an `Rc` bump plus copying a `'static` pointer.
/// Cloning is what lets `Ref::get()` hand back an owned handle rather
/// than forcing callers through a `.with(|h| ...)` closure.
#[derive(Clone)]
pub struct ButtonHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn ButtonOps,
}

impl ButtonHandle {
    /// Backend constructor. Called by `Backend::make_button_handle`
    /// impls. The `node` is type-erased here so user code can hold
    /// `Ref<ButtonHandle>` without naming the backend's node type.
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn ButtonOps) -> Self {
        Self { node, ops }
    }

    /// Programmatically triggers the button's click handler (and on
    /// platforms with native click semantics, dispatches the native
    /// event).
    pub fn click(&self) {
        self.ops.click(&*self.node);
    }
}

pub trait ButtonOps {
    fn click(&self, node: &dyn Any);
}

/// A handle to a mounted `View` primitive.
#[derive(Clone)]
pub struct ViewHandle {
    #[allow(dead_code)]
    node: Rc<dyn Any>,
    #[allow(dead_code)]
    ops: &'static dyn ViewOps,
}

impl ViewHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn ViewOps) -> Self {
        Self { node, ops }
    }
}

pub trait ViewOps {
    // No methods yet — reserved for measure(), scroll_to(), etc.
}

/// A handle to a mounted `Text` primitive.
#[derive(Clone)]
pub struct TextHandle {
    #[allow(dead_code)]
    node: Rc<dyn Any>,
    #[allow(dead_code)]
    ops: &'static dyn TextOps,
}

impl TextHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn TextOps) -> Self {
        Self { node, ops }
    }
}

pub trait TextOps {
    // Reserved for future text-specific operations.
}

/// Per-backend bundle of `Ops` trait objects, returned from
/// `Backend::ref_ops()`. The framework asks the backend for these once
/// (during render setup) and uses them to construct primitive handles
/// at mount time.
pub struct RefOps {
    pub button: &'static dyn ButtonOps,
    pub view: &'static dyn ViewOps,
    pub text: &'static dyn TextOps,
}

/// The mount-time closure that populates a `Ref<H>` slot. One variant
/// per primitive kind so the framework can build the correctly-typed
/// handle without runtime kind-matching on the closure itself. The
/// closure is monomorphic to `H`, so type-checking against the
/// call-site `Ref<H>` happens at `.bind()`. User code never constructs
/// this directly; it's exposed only because `Primitive`'s variants
/// carry it.
pub enum RefFill {
    Button(Box<dyn FnOnce(ButtonHandle)>),
    View(Box<dyn FnOnce(ViewHandle)>),
    Text(Box<dyn FnOnce(TextHandle)>),
    Image(Box<dyn FnOnce(primitives::image::ImageHandle)>),
    TextInput(Box<dyn FnOnce(primitives::text_input::TextInputHandle)>),
    Toggle(Box<dyn FnOnce(primitives::toggle::ToggleHandle)>),
    ScrollView(Box<dyn FnOnce(primitives::scroll_view::ScrollViewHandle)>),
    Slider(Box<dyn FnOnce(primitives::slider::SliderHandle)>),
    WebView(Box<dyn FnOnce(primitives::web_view::WebViewHandle)>),
    Video(Box<dyn FnOnce(primitives::video::VideoHandle)>),
    ActivityIndicator(Box<dyn FnOnce(primitives::activity_indicator::ActivityIndicatorHandle)>),
    Virtualizer(Box<dyn FnOnce(primitives::virtualizer::VirtualizerHandle)>),
}

/// Primitives are the structural skeleton of the UI. Every primitive
/// optionally carries a `style` slot — styling is orthogonal to
/// structure, so authors can style any primitive without each primitive
/// having to know about styling. The renderer applies the style via an
/// independent `Effect` per primitive, so a content signal change
/// doesn't re-fire the style effect and vice versa.
pub enum Primitive {
    View {
        children: Vec<Primitive>,
        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
    },
    Text {
        source: TextSource,
        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
    },
    Button {
        label: String,
        on_click: Rc<dyn Fn()>,
        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
        /// Optional reactive disabled flag. When the closure returns
        /// true, the framework: (1) flips the `DISABLED` state bit on
        /// the styled node so any `state disabled { ... }` overlay
        /// applies, (2) tells the backend to mark the native widget
        /// inert (`disabled` attr on web, `setEnabled(false)` on
        /// native). The closure is wrapped in an `Effect` so changes
        /// propagate automatically.
        disabled: Option<Box<dyn Fn() -> bool>>,
    },
    /// Image primitive. Source is reactive (`Box<dyn Fn() -> String>`)
    /// so authors can pass a static URL or a closure reading a signal.
    Image {
        src: Box<dyn Fn() -> String>,
        /// Optional accessibility label. Maps to `alt` on web,
        /// `accessibilityLabel` on iOS, `contentDescription` on Android.
        alt: Option<String>,
        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
    },
    /// Controlled text input. The parent owns the value as a
    /// `Signal<String>`; on every native input event the framework
    /// fires `on_change` with the new text, the parent updates the
    /// signal, the framework's effect re-fires and writes the new
    /// value back to the native widget. Cyclic but stable — widgets
    /// no-op when set to their current value.
    TextInput {
        value: crate::Signal<String>,
        on_change: Rc<dyn Fn(String)>,
        placeholder: Option<String>,
        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
    },
    /// Controlled toggle (switch / checkbox). Same controlled
    /// pattern as `TextInput`: `value: Signal<bool>` round-trips
    /// through `on_change`.
    Toggle {
        value: crate::Signal<bool>,
        on_change: Rc<dyn Fn(bool)>,
        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
    },
    /// Scroll container. Children scroll along `horizontal`'s opposite
    /// axis (vertical by default). Web: a div with `overflow: scroll`.
    /// iOS: `UIScrollView`. Android: `ScrollView` or
    /// `HorizontalScrollView`.
    ScrollView {
        children: Vec<Primitive>,
        horizontal: bool,
        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
    },
    /// Controlled numeric slider. Like `TextInput`/`Toggle`, the parent
    /// owns the value signal. If `step` is set, the framework snaps
    /// the incoming `on_change` value to the nearest step before
    /// dispatching — so behavior is identical across web (which clamps
    /// natively), iOS (no native step), and Android.
    Slider {
        value: crate::Signal<f32>,
        on_change: Rc<dyn Fn(f32)>,
        min: f32,
        max: f32,
        step: Option<f32>,
        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
    },
    /// Embedded web content view. Web: a (sandboxed-by-default-no)
    /// `<iframe>`. iOS: `WKWebView`. Android: `android.webkit.WebView`.
    WebView {
        url: Box<dyn Fn() -> String>,
        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
    },
    /// Video playback. URL-only; backends use their native players
    /// so codec/format support is whatever the platform handles.
    Video {
        src: Box<dyn Fn() -> String>,
        autoplay: bool,
        controls: bool,
        /// Field name is `loop_playback` to avoid the `loop` keyword.
        loop_playback: bool,
        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
    },
    /// Indeterminate loading spinner. No methods — passive widget.
    ActivityIndicator {
        size: primitives::activity_indicator::ActivityIndicatorSize,
        color: Option<Color>,
        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
    },
    /// Virtualized list. The framework supplies the backend with
    /// type-erased callbacks; the backend manages scroll position,
    /// visible-window math, and (on native) cell recycling. The
    /// `flat_list<T>(...)` wrapper in `primitives::flat_list` is the
    /// author-facing typed entry point.
    Virtualizer {
        item_count: Box<dyn Fn() -> usize>,
        item_key: Box<dyn Fn(usize) -> primitives::virtualizer::ItemKey>,
        item_size: primitives::virtualizer::ItemSize,
        render_item: Rc<dyn Fn(usize) -> Primitive>,
        overscan: f32,
        horizontal: bool,
        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
    },
    /// Reactive conditional. Renders `then()` while `cond()` is true and
    /// `otherwise()` when it's false. The renderer wraps the subtree
    /// construction in an `Effect` so the choice re-evaluates when any
    /// signal `cond()` reads changes; the prior subtree's effects are
    /// dropped on each flip, so state in the hidden branch is gone.
    When {
        cond: Box<dyn Fn() -> bool>,
        then: Box<dyn Fn() -> Primitive>,
        otherwise: Box<dyn Fn() -> Primitive>,
        style: Option<StyleSource>,
    },
}

/// Allows `with_style(...)` to accept any of:
///   - a bare `Rc<StyleSheet>` — applies the stylesheet with no
///     variant selection, no overrides. Best for static one-shot
///     styles like `banner_style()`.
///   - a fixed `StyleApplication` — for the case where you already
///     have a built-up application with variants/overrides.
///   - a closure returning a `StyleApplication` — enables reactive
///     styling: signals read inside the closure become dependencies
///     and changes re-fire the apply-style effect.
///
/// The `Rc<StyleSheet>` impl exists so authors don't have to write
/// `StyleApplication::new(sheet)` for the trivial case — most styles
/// are like that, and the wrapping was pure ceremony.
pub trait IntoStyleSource {
    fn into_style_source(self) -> StyleSource;
}

impl IntoStyleSource for Rc<StyleSheet> {
    fn into_style_source(self) -> StyleSource {
        let app = StyleApplication::new(self);
        Box::new(move || app.clone())
    }
}

impl IntoStyleSource for StyleApplication {
    fn into_style_source(self) -> StyleSource {
        Box::new(move || self.clone())
    }
}

impl<F> IntoStyleSource for F
where
    F: Fn() -> StyleApplication + 'static,
{
    fn into_style_source(self) -> StyleSource {
        Box::new(self)
    }
}

impl Primitive {
    /// Attaches a style to this primitive. Replaces any previously-set
    /// style. The style argument can be either a `StyleApplication`
    /// (static) or a closure returning one (reactive).
    pub fn with_style<S: IntoStyleSource>(mut self, style: S) -> Self {
        let src = style.into_style_source();
        match &mut self {
            Primitive::View { style, .. }
            | Primitive::Text { style, .. }
            | Primitive::Button { style, .. }
            | Primitive::Image { style, .. }
            | Primitive::TextInput { style, .. }
            | Primitive::Toggle { style, .. }
            | Primitive::ScrollView { style, .. }
            | Primitive::Slider { style, .. }
            | Primitive::WebView { style, .. }
            | Primitive::Video { style, .. }
            | Primitive::ActivityIndicator { style, .. }
            | Primitive::Virtualizer { style, .. }
            | Primitive::When { style, .. } => {
                *style = Some(src);
            }
        }
        self
    }
}

// =============================================================================
// Bound<H> — primitive + phantom handle type for .bind() type-checking
// =============================================================================
//
// A constructor like `button(...)` returns `Bound<ButtonHandle>` rather
// than a bare `Primitive`. Carrying the handle type in the type system
// makes `.bind(r: Ref<ButtonHandle>)` a compile-time check — passing
// `Ref<ViewHandle>` to a button's `.bind` is a type error, no runtime
// dispatch needed.
//
// `Bound<H>` implements `Into<Primitive>` and `ChildList`, so call sites
// and the rest of the framework continue to work with `Primitive` after
// `.bind()` (or without ever calling it). Authors who don't care about
// refs never see `Bound` — the constructors return it, the children
// macro coerces it, no friction.

/// A `Primitive` plus a phantom handle type. Constructed by primitive
/// builder functions (`button(...)`, `view(...)`, …); coerced back to
/// `Primitive` automatically for child lists. Only purpose: type-check
/// `.bind(r)` against the call-site `Ref<H>`.
pub struct Bound<H> {
    pub(crate) primitive: Primitive,
    _handle: std::marker::PhantomData<H>,
}

impl<H> Bound<H> {
    pub(crate) fn new(primitive: Primitive) -> Self {
        Self { primitive, _handle: std::marker::PhantomData }
    }

    /// Attaches a style. Same semantics as `Primitive::with_style`.
    pub fn with_style<S: IntoStyleSource>(mut self, style: S) -> Self {
        self.primitive = self.primitive.with_style(style);
        self
    }
}

// `bind` is implemented per handle type so it can both (a) take the
// correctly-typed `Ref<H>` and (b) install the appropriate `RefFill`
// variant on the underlying primitive.

impl Bound<ButtonHandle> {
    /// Binds this button to `r`. At mount time the framework constructs
    /// a `ButtonHandle` from the just-created backend node and calls
    /// `r.fill(handle)`. Pre-mount calls on `r` are no-ops.
    pub fn bind(mut self, r: Ref<ButtonHandle>) -> Self {
        if let Primitive::Button { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::Button(Box::new(move |h| r.fill(h))));
        }
        self
    }

    /// Reactively disable the button. Accepts a bare `bool` or a
    /// closure returning a `bool` (typically reading a `Signal<bool>`).
    /// When the value is `true`, the framework flips the
    /// `DISABLED` state bit on the node (so any
    /// `state disabled { ... }` overlay applies) and tells the backend
    /// to mark the native widget inert.
    pub fn disabled<D: IntoDisabledSource>(mut self, disabled: D) -> Self {
        if let Primitive::Button { disabled: slot, .. } = &mut self.primitive {
            *slot = Some(disabled.into_disabled_source());
        }
        self
    }
}

/// Trait for the `Bound<ButtonHandle>::disabled` setter. Lets authors
/// pass either a static `bool` (`.disabled(true)`) or a closure
/// (`.disabled(move || is_disabled.get())`). Reactivity falls out of
/// the closure case naturally: the closure is invoked inside an
/// `Effect`, which subscribes to the signals it reads.
pub trait IntoDisabledSource {
    fn into_disabled_source(self) -> Box<dyn Fn() -> bool>;
}

impl IntoDisabledSource for bool {
    fn into_disabled_source(self) -> Box<dyn Fn() -> bool> {
        Box::new(move || self)
    }
}

impl<F> IntoDisabledSource for F
where
    F: Fn() -> bool + 'static,
{
    fn into_disabled_source(self) -> Box<dyn Fn() -> bool> {
        Box::new(self)
    }
}

impl Bound<ViewHandle> {
    pub fn bind(mut self, r: Ref<ViewHandle>) -> Self {
        if let Primitive::View { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::View(Box::new(move |h| r.fill(h))));
        }
        self
    }
}

impl Bound<TextHandle> {
    pub fn bind(mut self, r: Ref<TextHandle>) -> Self {
        if let Primitive::Text { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::Text(Box::new(move |h| r.fill(h))));
        }
        self
    }
}

impl<H> From<Bound<H>> for Primitive {
    fn from(b: Bound<H>) -> Primitive { b.primitive }
}

// =============================================================================
// Bindable<H> — user-component primitive + already-constructed handle
// =============================================================================
//
// Sister type to `Bound<H>` for *user components* (the things wrapped by
// `#[component]`), not primitives. Differences:
//
// - `Bound<H>` is for primitives. The framework constructs the `H` lazily
//   at mount, using `Backend::make_*_handle` against the just-created
//   backend node. The ref is filled inside the framework's `build`
//   walker via a `RefFill` closure.
//
// - `Bindable<H>` is for user components. By the time the component
//   function returns, the handle `H` already exists (the component
//   body constructed it explicitly, closing over its own Signals or
//   Refs). `.bind(r)` fills the ref synchronously — no `RefFill`
//   plumbing through `Primitive` needed.
//
// Both implement `Into<Primitive>` and `ChildList` so the rest of the
// framework (children lists, `IntoPrimitive` coercion) doesn't care
// whether the call site uses one or the other.

/// A `Primitive` plus an already-constructed component handle. Returned
/// by user `#[component]` functions that expose imperative methods.
/// Authors construct this in their component body and `.bind(r)` to
/// hook it up to a `Ref<H>` owned by the parent.
pub struct Bindable<H> {
    primitive: Primitive,
    handle: H,
}

impl<H: 'static> Bindable<H> {
    /// Constructs a `Bindable` from the component's primitive tree and
    /// the handle it exposes. Called from inside the component body —
    /// typically as the final expression.
    pub fn new(primitive: Primitive, handle: H) -> Self {
        Self { primitive, handle }
    }

    /// Attaches a style to the component's root primitive. Same
    /// semantics as `Primitive::with_style` / `Bound::with_style` —
    /// the inner primitive's style slot is overwritten, and the chain
    /// returns `Self` so subsequent calls like `.bind(r)` keep the
    /// handle type.
    pub fn with_style<S: IntoStyleSource>(mut self, style: S) -> Self {
        self.primitive = self.primitive.with_style(style);
        self
    }

    /// Fills `r` with this component's handle and returns the
    /// underlying `Primitive`. The fill happens *immediately* — the
    /// handle exists by the time the component function returned, so
    /// there's no mount-time deferral.
    ///
    /// Compile-time type checking: `r: Ref<H>` and the component
    /// returns `Bindable<H>`, so passing the wrong ref type is a type
    /// error.
    pub fn bind(self, r: Ref<H>) -> Primitive {
        r.fill(self.handle);
        self.primitive
    }
}

impl<H> From<Bindable<H>> for Primitive {
    fn from(b: Bindable<H>) -> Primitive { b.primitive }
}

impl<H> ChildList for Bindable<H> {
    fn append_to(self, out: &mut Vec<Primitive>) {
        out.push(self.primitive);
    }
}

impl<H> ChildList for Option<Bindable<H>> {
    fn append_to(self, out: &mut Vec<Primitive>) {
        if let Some(b) = self {
            out.push(b.primitive);
        }
    }
}

pub fn view(children: Vec<Primitive>) -> Bound<ViewHandle> {
    Bound::new(Primitive::View { children, style: None, ref_fill: None })
}

/// Flexible-shape source for a child-list slot. Implementors say how to
/// append themselves (zero or more primitives) to a growing Vec. Used by
/// the `children!(...)` macro so call sites can mix:
///   - a single `Primitive`
///   - `Option<Primitive>` (often from `cond.then(|| ...)`)
///   - `Vec<Primitive>` (e.g. from a `.map().collect()`)
pub trait ChildList {
    fn append_to(self, out: &mut Vec<Primitive>);
}

impl ChildList for Primitive {
    fn append_to(self, out: &mut Vec<Primitive>) {
        out.push(self);
    }
}

impl<H> ChildList for Bound<H> {
    fn append_to(self, out: &mut Vec<Primitive>) {
        out.push(self.primitive);
    }
}

impl ChildList for Option<Primitive> {
    fn append_to(self, out: &mut Vec<Primitive>) {
        if let Some(p) = self {
            out.push(p);
        }
    }
}

impl<H> ChildList for Option<Bound<H>> {
    fn append_to(self, out: &mut Vec<Primitive>) {
        if let Some(b) = self {
            out.push(b.primitive);
        }
    }
}

impl ChildList for Vec<Primitive> {
    fn append_to(self, out: &mut Vec<Primitive>) {
        out.extend(self);
    }
}

/// Shorthand for `Signal::new(value)`. Equivalent in every way; just less
/// typing at the call site.
///
/// ```ignore
/// let count = signal!(0);
/// // same as: let count = Signal::new(0);
/// ```
#[macro_export]
macro_rules! signal {
    ($value:expr) => {
        $crate::Signal::new($value)
    };
}

/// Builds a `Vec<Primitive>` from a mixed-shape list of children.
///
/// Each argument must implement [`ChildList`]; the macro flattens
/// `Option<Primitive>` (skipping `None`) and `Vec<Primitive>` (extending
/// inline) so call sites can write conditionals naturally.
///
/// ```ignore
/// view(children![
///     text("always"),
///     logged_in.then(|| text("conditional")),
///     items.into_iter().map(|i| text(i)).collect::<Vec<_>>(),
/// ])
/// ```
#[macro_export]
macro_rules! children {
    ($($child:expr),* $(,)?) => {{
        let mut __c: ::std::vec::Vec<$crate::Primitive> = ::std::vec::Vec::new();
        $( $crate::ChildList::append_to($child, &mut __c); )*
        __c
    }};
}

pub fn text<T: IntoTextSource>(source: T) -> Bound<TextHandle> {
    Bound::new(Primitive::Text {
        source: source.into_text_source(),
        style: None,
        ref_fill: None,
    })
}

pub fn button<F: Fn() + 'static>(label: impl Into<String>, on_click: F) -> Bound<ButtonHandle> {
    Bound::new(Primitive::Button {
        label: label.into(),
        on_click: Rc::new(on_click),
        style: None,
        ref_fill: None,
        disabled: None,
    })
}

/// Reactive conditional. Author code provides three closures:
/// - `cond` reads one or more signals and returns a `bool`.
/// - `then` and `otherwise` each return a `Primitive` to render.
///
/// When any signal `cond()` reads changes, the active branch is rebuilt
/// from scratch. The hidden branch's effects are dropped, so any signal
/// subscriptions in it are released. State in the hidden branch is lost
/// on toggle — this is the "dispose on hide" model.
pub fn when<C, T, O>(cond: C, then: T, otherwise: O) -> Primitive
where
    C: Fn() -> bool + 'static,
    T: Fn() -> Primitive + 'static,
    O: Fn() -> Primitive + 'static,
{
    Primitive::When {
        cond: Box::new(cond),
        then: Box::new(then),
        otherwise: Box::new(otherwise),
        style: None,
    }
}

/// Coercion helper: lets `when()`'s `then`/`otherwise` closures return
/// either a bare `Primitive` or a `Bound<H>`. `Into<Primitive>` is
/// already implemented for `Bound<H>`; this trait makes the implicit
/// conversion happen in argument position so users don't have to spell
/// `.into()`. Used by the `ui!` macro and by direct `when(...)` callers.
pub trait IntoPrimitive {
    fn into_primitive(self) -> Primitive;
}

impl IntoPrimitive for Primitive {
    fn into_primitive(self) -> Primitive { self }
}

impl<H> IntoPrimitive for Bound<H> {
    fn into_primitive(self) -> Primitive { self.primitive }
}

impl<H> IntoPrimitive for Bindable<H> {
    fn into_primitive(self) -> Primitive { self.primitive }
}

/// Callbacks handed to `Backend::create_virtualizer`. All Rc'd so
/// the backend can clone into per-event closures (scroll handler,
/// cell binder, etc.). Generic over the backend's `Node` type so
/// the mount callback returns the backend's actual native node
/// type, no type erasure.
pub struct VirtualizerCallbacks<N: Clone + 'static> {
    /// Current item count. Backend calls this on data-changed.
    pub item_count: Rc<dyn Fn() -> usize>,
    /// Stable identity for an index. Backend uses this to do
    /// keyed diffs across data updates.
    pub item_key: Rc<dyn Fn(usize) -> primitives::virtualizer::ItemKey>,
    /// Initial size for an index (Known: authoritative;
    /// Measured: estimate). For Measured mode, the backend should
    /// observe the rendered size after mount and update its
    /// internal layout when the value changes.
    pub item_size: Rc<dyn Fn(usize) -> f32>,
    /// True if `item_size` is an estimate that should be refined
    /// by measuring the mounted node. False if the size is
    /// authoritative.
    pub measure_sizes: bool,
    /// Mount an item: build its subtree inside a fresh per-item
    /// Scope. Returns the freshly-built native node plus the
    /// scope's id. The backend should hold the id alongside its
    /// pooled/mounted cell so it can call `release_item` later.
    pub mount_item: Rc<dyn Fn(usize) -> (N, u64)>,
    /// Release a previously-mounted item by scope id. Drops the
    /// scope, freeing every signal/effect/ref inside the item's
    /// subtree. Backend should NOT try to use the node after this;
    /// it should also detach the node from its parent.
    pub release_item: Rc<dyn Fn(u64)>,
    /// Backend may call this to inform the framework that an
    /// observed item's measured size has changed (Measured mode).
    /// The framework stores the new size and the backend uses it
    /// for future layout passes.
    pub set_measured_size: Rc<dyn Fn(u64, f32)>,
}

pub trait Backend {
    type Node: Clone;

    fn create_view(&mut self) -> Self::Node;
    fn create_text(&mut self, content: &str) -> Self::Node;
    fn create_button(&mut self, label: &str, on_click: Rc<dyn Fn()>) -> Self::Node;
    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node);
    fn update_text(&mut self, node: &Self::Node, content: &str);

    /// Create an image node with the initial URL. The framework
    /// wraps the user's `src` source in an effect that calls
    /// `update_image_src` whenever the source changes.
    #[allow(unused_variables)]
    fn create_image(&mut self, src: &str, alt: Option<&str>) -> Self::Node {
        unimplemented!("create_image not implemented for this backend")
    }
    #[allow(unused_variables)]
    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        // default: no-op; backends that don't implement images just
        // leave the URL static.
    }

    /// Create a text input with the initial value, placeholder, and
    /// an `on_change` callback fired on every native input event.
    /// The framework wraps the controlled `value` signal in an
    /// effect that calls `update_text_input_value` on signal change.
    #[allow(unused_variables)]
    fn create_text_input(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
    ) -> Self::Node {
        unimplemented!("create_text_input not implemented for this backend")
    }
    #[allow(unused_variables)]
    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {}

    /// Create a toggle (switch / checkbox) with the initial value and
    /// an `on_change` callback. Same controlled-update pattern as
    /// text input.
    #[allow(unused_variables)]
    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
    ) -> Self::Node {
        unimplemented!("create_toggle not implemented for this backend")
    }
    #[allow(unused_variables)]
    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {}

    /// Create a scrolling container. `horizontal` selects the
    /// scrolling axis (false = vertical, the default; true = horizontal).
    #[allow(unused_variables)]
    fn create_scroll_view(&mut self, horizontal: bool) -> Self::Node {
        unimplemented!("create_scroll_view not implemented for this backend")
    }

    /// Create a slider widget. `min`/`max`/`step` are static after
    /// creation; controlled value updates flow through
    /// `update_slider_value`. `on_change` fires on every drag tick.
    #[allow(unused_variables)]
    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
    ) -> Self::Node {
        unimplemented!("create_slider not implemented for this backend")
    }
    #[allow(unused_variables)]
    fn update_slider_value(&mut self, node: &Self::Node, value: f32) {}

    /// Create a WebView with the initial URL. `update_web_view_url`
    /// drives subsequent navigations from the reactive source.
    #[allow(unused_variables)]
    fn create_web_view(&mut self, url: &str) -> Self::Node {
        unimplemented!("create_web_view not implemented for this backend")
    }
    #[allow(unused_variables)]
    fn update_web_view_url(&mut self, node: &Self::Node, url: &str) {}

    /// Create a Video element. Static autoplay/controls/loop are
    /// passed at construction time; reactive `src` updates flow
    /// through `update_video_src`.
    #[allow(unused_variables)]
    fn create_video(
        &mut self,
        src: &str,
        autoplay: bool,
        controls: bool,
        loop_playback: bool,
    ) -> Self::Node {
        unimplemented!("create_video not implemented for this backend")
    }
    #[allow(unused_variables)]
    fn update_video_src(&mut self, node: &Self::Node, src: &str) {}

    /// Create a loading spinner. Size/color are static at construction.
    #[allow(unused_variables)]
    fn create_activity_indicator(
        &mut self,
        size: primitives::activity_indicator::ActivityIndicatorSize,
        color: Option<&Color>,
    ) -> Self::Node {
        unimplemented!("create_activity_indicator not implemented for this backend")
    }

    /// Create a virtualized list. The backend gets a bundle of
    /// callbacks (via `VirtualizerCallbacks`) it uses to query the
    /// current data set, request mounted subtrees, and release
    /// them when items leave the viewport / get recycled.
    ///
    /// The backend owns the scroll handler and the visible-window
    /// math. It calls `mount_item(idx)` when an index needs to
    /// become visible, getting back `(node, scope_id)`. When the
    /// index leaves the visible window (web: scrolled out; native:
    /// cell recycled), the backend calls `release_item(scope_id)`
    /// to free the framework's per-item Scope — which drops every
    /// signal, effect, and ref nested inside that item.
    #[allow(unused_variables)]
    fn create_virtualizer(
        &mut self,
        callbacks: VirtualizerCallbacks<Self::Node>,
        overscan: f32,
        horizontal: bool,
    ) -> Self::Node {
        unimplemented!("create_virtualizer not implemented for this backend")
    }

    /// Signal that the underlying data set has changed. The backend
    /// re-queries item_count + item_key + item_size to figure out
    /// what changed, runs its diff, and updates the mounted set
    /// accordingly. Called from an Effect that reads the data signal,
    /// so it fires on every data update automatically.
    #[allow(unused_variables)]
    fn virtualizer_data_changed(&mut self, node: &Self::Node) {}

    /// Remove every child from `node`. Used by reactive conditionals when
    /// the active branch flips and the old subtree needs to be unmounted.
    fn clear_children(&mut self, node: &Self::Node);
    /// Apply a resolved style to a node. The framework has already run
    /// the stylesheet's closure against the active theme; the backend
    /// receives concrete `StyleRules` with literal values.
    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>);

    /// Apply a base style plus per-state overlays. Called when the
    /// stylesheet declares interaction-state blocks (`state hovered`,
    /// `state pressed`, etc.) AND the backend reports native state
    /// handling via [`Backend::handles_states_natively`].
    ///
    /// Web overrides this to emit the overlays as CSS pseudo-class
    /// rules scoped to the base class — the browser then handles
    /// state tracking natively. No Rust↔JS round trip per event.
    ///
    /// Backends that rely on event-driven state activation
    /// (`attach_states` + signal-driven re-resolve) leave both the
    /// default impl AND `handles_states_natively() = false`. State
    /// overlays reach those backends through the regular
    /// `apply_style` path when the state signal flips.
    fn apply_styled_states(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        #[allow(unused_variables)] overlays: &[(StateBits, Rc<StyleRules>)],
    ) {
        // Default: just apply the base style. Mobile backends drive
        // state overlays via signal-flip → re-resolve → apply_style.
        self.apply_style(node, base);
    }

    /// Backend capability flag. `true` means the backend wants to
    /// receive state overlays declaratively via `apply_styled_states`
    /// and handle state tracking natively (e.g. CSS pseudo-classes
    /// on web). `false` means the backend uses the event-driven path:
    /// `attach_states` registers native event listeners that flip the
    /// framework's per-node state signal, and each state change
    /// re-fires the style effect with the appropriate overlay merged
    /// into a fresh `StyleApplication`.
    ///
    /// The framework reads this once per `attach_style` to choose
    /// between the two paths. Default is `false` — backends opt in.
    fn handles_states_natively(&self) -> bool {
        false
    }

    /// Pre-generate any backend-side state for a stylesheet against the
    /// current theme. Web backends typically use this to mint CSS
    /// classes for every variant + compound combination up front, so
    /// `apply_style` is a cache hit. Other backends can leave the
    /// default no-op implementation.
    ///
    /// Called by the framework:
    /// - The first time a stylesheet is `resolve`d.
    /// - After every `set_theme(...)`, for every still-live stylesheet,
    ///   so the backend's pre-generated state is refreshed.
    ///
    /// The framework passes pre-resolved `StyleRules` (one per relevant
    /// variant combination) so the backend doesn't have to think about
    /// theme tokens — it gets concrete property bags.
    #[allow(unused_variables)]
    fn register_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        // default: no-op
    }

    /// Release a previously-registered stylesheet's pre-generated state.
    /// Called when the stylesheet is no longer reachable (its last
    /// `Rc<StyleSheet>` has been dropped) and after every theme change
    /// (before re-registering, so old state is cleaned up).
    #[allow(unused_variables)]
    fn unregister_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        // default: no-op
    }

    /// Called when a styled node is being torn down (its surrounding
    /// `Effect` scope is dropping). Lets backends free per-node state —
    /// e.g. the web backend drops the node's dynamic CSS class slot
    /// and its node-id entry. Other backends typically don't need this.
    #[allow(unused_variables)]
    fn on_node_unstyled(&mut self, node: &Self::Node) {
        // default: no-op
    }

    /// Wires the backend's native interaction events (hover, press,
    /// focus) to the framework's per-node state machinery. The
    /// framework allocates a `Signal<StateBits>` per styled node and
    /// passes a setter closure here; backends call the setter when
    /// the corresponding native event fires.
    ///
    /// The setter takes `(state, on)` where `state` is a
    /// `StateBits` flag (`StateBits::HOVERED`, etc.) and `on` is
    /// true for entering / false for leaving the state. The framework
    /// re-resolves and re-applies the node's style when state bits
    /// change — backends don't need to do any style work themselves.
    ///
    /// Default impl is a no-op for backends that don't yet support
    /// interaction states (states declared in the stylesheet simply
    /// never activate on those platforms — a documented no-op).
    #[allow(unused_variables)]
    fn attach_states(&mut self, node: &Self::Node, setter: Rc<dyn Fn(StateBits, bool)>) {
        // default: no-op
    }

    /// Mark the native widget as disabled or enabled. Distinct from
    /// the `DISABLED` style-state bit (which controls overlay
    /// styling) — this one is about the widget being inert: web's
    /// `disabled` attribute, `setEnabled(false)` on native. Backends
    /// that don't distinguish leave the default no-op.
    #[allow(unused_variables)]
    fn set_disabled(&mut self, node: &Self::Node, disabled: bool) {
        // default: no-op
    }

    /// Constructs a `ButtonHandle` for the just-created button `node`.
    /// Called by the framework when a `Bound<ButtonHandle>` with a
    /// `.bind(r)` is mounted. The handle internally holds an
    /// `Rc<dyn Any>` wrapping the backend's concrete node value, so
    /// `ButtonOps` methods can downcast to operate on it. Default
    /// impl returns a handle with a no-op ops table — backends that
    /// don't support refs don't have to think about it.
    #[allow(unused_variables)]
    fn make_button_handle(&self, node: &Self::Node) -> ButtonHandle {
        ButtonHandle { node: Rc::new(()), ops: &NoopButtonOps }
    }

    #[allow(unused_variables)]
    fn make_view_handle(&self, node: &Self::Node) -> ViewHandle {
        ViewHandle { node: Rc::new(()), ops: &NoopViewOps }
    }

    #[allow(unused_variables)]
    fn make_text_handle(&self, node: &Self::Node) -> TextHandle {
        TextHandle { node: Rc::new(()), ops: &NoopTextOps }
    }

    #[allow(unused_variables)]
    fn make_image_handle(&self, node: &Self::Node) -> primitives::image::ImageHandle {
        primitives::image::ImageHandle::new(Rc::new(()), &NoopImageOps)
    }

    #[allow(unused_variables)]
    fn make_text_input_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::text_input::TextInputHandle {
        primitives::text_input::TextInputHandle::new(Rc::new(()), &NoopTextInputOps)
    }

    #[allow(unused_variables)]
    fn make_toggle_handle(&self, node: &Self::Node) -> primitives::toggle::ToggleHandle {
        primitives::toggle::ToggleHandle::new(Rc::new(()), &NoopToggleOps)
    }

    #[allow(unused_variables)]
    fn make_scroll_view_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::scroll_view::ScrollViewHandle {
        primitives::scroll_view::ScrollViewHandle::new(Rc::new(()), &NoopScrollViewOps)
    }

    #[allow(unused_variables)]
    fn make_slider_handle(&self, node: &Self::Node) -> primitives::slider::SliderHandle {
        primitives::slider::SliderHandle::new(Rc::new(()), &NoopSliderOps)
    }

    #[allow(unused_variables)]
    fn make_web_view_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::web_view::WebViewHandle {
        primitives::web_view::WebViewHandle::new(Rc::new(()), &NoopWebViewOps)
    }

    #[allow(unused_variables)]
    fn make_video_handle(&self, node: &Self::Node) -> primitives::video::VideoHandle {
        primitives::video::VideoHandle::new(Rc::new(()), &NoopVideoOps)
    }

    #[allow(unused_variables)]
    fn make_activity_indicator_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::activity_indicator::ActivityIndicatorHandle {
        primitives::activity_indicator::ActivityIndicatorHandle::new(
            Rc::new(()),
            &NoopActivityIndicatorOps,
        )
    }

    #[allow(unused_variables)]
    fn make_virtualizer_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::virtualizer::VirtualizerHandle {
        primitives::virtualizer::VirtualizerHandle::new(
            Rc::new(()),
            &NoopVirtualizerOps,
        )
    }

    fn finish(&mut self, root: Self::Node);
}

// Default ZST `Ops` impls used by backends that haven't opted into ref
// support yet (or by the `()` Node used in tests).

struct NoopImageOps;
impl primitives::image::ImageOps for NoopImageOps {}

struct NoopTextInputOps;
impl primitives::text_input::TextInputOps for NoopTextInputOps {
    fn focus(&self, _: &dyn Any) {}
    fn blur(&self, _: &dyn Any) {}
    fn select_all(&self, _: &dyn Any) {}
}

struct NoopToggleOps;
impl primitives::toggle::ToggleOps for NoopToggleOps {}

struct NoopScrollViewOps;
impl primitives::scroll_view::ScrollViewOps for NoopScrollViewOps {
    fn scroll_to(&self, _: &dyn Any, _: f32, _: f32) {}
}

struct NoopSliderOps;
impl primitives::slider::SliderOps for NoopSliderOps {}

struct NoopWebViewOps;
impl primitives::web_view::WebViewOps for NoopWebViewOps {}

struct NoopVideoOps;
impl primitives::video::VideoOps for NoopVideoOps {
    fn play(&self, _: &dyn Any) {}
    fn pause(&self, _: &dyn Any) {}
    fn seek(&self, _: &dyn Any, _: f32) {}
}

struct NoopActivityIndicatorOps;
impl primitives::activity_indicator::ActivityIndicatorOps for NoopActivityIndicatorOps {}

struct NoopVirtualizerOps;
impl primitives::virtualizer::VirtualizerOps for NoopVirtualizerOps {
    fn scroll_to_index(&self, _: &dyn Any, _: usize) {}
}

struct NoopButtonOps;
impl ButtonOps for NoopButtonOps {
    fn click(&self, _node: &dyn Any) {}
}

struct NoopViewOps;
impl ViewOps for NoopViewOps {}

struct NoopTextOps;
impl TextOps for NoopTextOps {}

/// Owns the reactive state created by a render call. Dropping the `Owner`
/// drops its `Scope`, which frees every signal and effect created during
/// rendering — no leaks across the boundary.
pub struct Owner {
    // Boxed so we can hand out a `&mut Scope` to `with_scope` calls inside
    // reactive subtree rebuilds without invalidating other references.
    // Field is dropped-only: it's never read, but its `Drop` impl is what
    // actually frees the arena slots.
    #[allow(dead_code)]
    scope: Box<reactive::Scope>,
}

#[must_use = "drop the Owner to dispose the UI; keep it alive to keep the UI reactive"]
pub fn render<B: Backend + 'static>(backend: Rc<RefCell<B>>, tree: Primitive) -> Owner {
    let mut scope = Box::new(reactive::Scope::new());
    let root = reactive::with_scope(&mut scope, || build(&backend, tree));
    backend.borrow_mut().finish(root);
    Owner { scope }
}

fn build<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Primitive) -> B::Node {
    match node {
        Primitive::Text { source, style, ref_fill } => {
            let n = build_text(backend, source);
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            if let Some(RefFill::Text(fill)) = ref_fill {
                let handle = backend.borrow().make_text_handle(&n);
                fill(handle);
            }
            n
        }
        Primitive::View { children, style, ref_fill } => {
            let n = build_view(backend, children);
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            if let Some(RefFill::View(fill)) = ref_fill {
                let handle = backend.borrow().make_view_handle(&n);
                fill(handle);
            }
            n
        }
        Primitive::Button { label, on_click, style, ref_fill, disabled } => {
            let n = backend.borrow_mut().create_button(&label, on_click);
            // attach_style returns the state setter so we can drive
            // the DISABLED bit reactively from `disabled` below. If
            // there's no style, we still need to react to disabled to
            // toggle the native widget's inert state, so allocate a
            // no-op-style setter route in that case.
            let state_setter = style.map(|s| attach_style(backend, &n, s));
            if let Some(RefFill::Button(fill)) = ref_fill {
                let handle = backend.borrow().make_button_handle(&n);
                fill(handle);
            }
            if let Some(d) = disabled {
                attach_disabled(backend, &n, d, state_setter);
            }
            n
        }
        Primitive::Image { src, alt, style, ref_fill } => {
            // Initial mount: call the source closure once for the
            // initial URL, then wrap it in an effect that updates the
            // image whenever signals it reads change.
            let initial = src();
            let n = backend.borrow_mut().create_image(&initial, alt.as_deref());
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            // Reactive src: if `src()` re-reads on subsequent fires,
            // the Effect subscribes and `update_image_src` re-runs.
            {
                let backend = backend.clone();
                let node = n.clone();
                let _e = Effect::new(move || {
                    let url = src();
                    backend.borrow_mut().update_image_src(&node, &url);
                });
            }
            if let Some(RefFill::Image(fill)) = ref_fill {
                let handle = backend.borrow().make_image_handle(&n);
                fill(handle);
            }
            n
        }
        Primitive::TextInput { value, on_change, placeholder, style, ref_fill } => {
            let initial = value.get();
            let n = backend.borrow_mut().create_text_input(
                &initial,
                placeholder.as_deref(),
                on_change,
            );
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            // Reactive: whenever the controlled signal changes, push
            // the new value into the widget. Setting to the same
            // value is a no-op on most platforms (web ignores no-change
            // sets on inputs).
            {
                let backend = backend.clone();
                let node = n.clone();
                let _e = Effect::new(move || {
                    let v = value.get();
                    backend.borrow_mut().update_text_input_value(&node, &v);
                });
            }
            if let Some(RefFill::TextInput(fill)) = ref_fill {
                let handle = backend.borrow().make_text_input_handle(&n);
                fill(handle);
            }
            n
        }
        Primitive::Toggle { value, on_change, style, ref_fill } => {
            let initial = value.get();
            let n = backend.borrow_mut().create_toggle(initial, on_change);
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            {
                let backend = backend.clone();
                let node = n.clone();
                let _e = Effect::new(move || {
                    let v = value.get();
                    backend.borrow_mut().update_toggle_value(&node, v);
                });
            }
            if let Some(RefFill::Toggle(fill)) = ref_fill {
                let handle = backend.borrow().make_toggle_handle(&n);
                fill(handle);
            }
            n
        }
        Primitive::ScrollView { children, horizontal, style, ref_fill } => {
            let mut n = backend.borrow_mut().create_scroll_view(horizontal);
            for child in children {
                let child_node = build(backend, child);
                backend.borrow_mut().insert(&mut n, child_node);
            }
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            if let Some(RefFill::ScrollView(fill)) = ref_fill {
                let handle = backend.borrow().make_scroll_view_handle(&n);
                fill(handle);
            }
            n
        }
        Primitive::Slider { value, on_change, min, max, step, style, ref_fill } => {
            let initial = value.get();
            // Wrap the user's on_change to snap to `step` first, so all
            // backends produce identical values regardless of native
            // step handling.
            let on_change_snap: Rc<dyn Fn(f32)> = if let Some(s) = step {
                let user = on_change.clone();
                let min_c = min;
                Rc::new(move |v| {
                    let snapped = min_c + ((v - min_c) / s).round() * s;
                    user(snapped);
                })
            } else {
                on_change.clone()
            };
            let n = backend.borrow_mut().create_slider(initial, min, max, step, on_change_snap);
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            // Reactive: write the controlled value back to the widget
            // whenever the signal changes.
            {
                let backend = backend.clone();
                let node = n.clone();
                let _e = Effect::new(move || {
                    let v = value.get();
                    backend.borrow_mut().update_slider_value(&node, v);
                });
            }
            if let Some(RefFill::Slider(fill)) = ref_fill {
                let handle = backend.borrow().make_slider_handle(&n);
                fill(handle);
            }
            n
        }
        Primitive::WebView { url, style, ref_fill } => {
            let initial = url();
            let n = backend.borrow_mut().create_web_view(&initial);
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            {
                let backend = backend.clone();
                let node = n.clone();
                let _e = Effect::new(move || {
                    let u = url();
                    backend.borrow_mut().update_web_view_url(&node, &u);
                });
            }
            if let Some(RefFill::WebView(fill)) = ref_fill {
                let handle = backend.borrow().make_web_view_handle(&n);
                fill(handle);
            }
            n
        }
        Primitive::Video { src, autoplay, controls, loop_playback, style, ref_fill } => {
            let initial = src();
            let n = backend.borrow_mut().create_video(&initial, autoplay, controls, loop_playback);
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            {
                let backend = backend.clone();
                let node = n.clone();
                let _e = Effect::new(move || {
                    let s = src();
                    backend.borrow_mut().update_video_src(&node, &s);
                });
            }
            if let Some(RefFill::Video(fill)) = ref_fill {
                let handle = backend.borrow().make_video_handle(&n);
                fill(handle);
            }
            n
        }
        Primitive::ActivityIndicator { size, color, style, ref_fill } => {
            let n = backend.borrow_mut().create_activity_indicator(size, color.as_ref());
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            if let Some(RefFill::ActivityIndicator(fill)) = ref_fill {
                let handle = backend.borrow().make_activity_indicator_handle(&n);
                fill(handle);
            }
            n
        }
        Primitive::Virtualizer {
            item_count,
            item_key,
            item_size,
            render_item,
            overscan,
            horizontal,
            style,
            ref_fill,
        } => {
            let n = build_virtualizer(
                backend, item_count, item_key, item_size, render_item, overscan, horizontal,
            );
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            if let Some(RefFill::Virtualizer(fill)) = ref_fill {
                let handle = backend.borrow().make_virtualizer_handle(&n);
                fill(handle);
            }
            n
        }
        Primitive::When { cond, then, otherwise, style } => {
            let n = build_when(backend, cond, then, otherwise);
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            n
        }
    }
}

/// Builds a Text primitive (static or reactive). Style application is
/// handled by the caller via `attach_style` so the content effect and
/// the style effect stay independent.
fn build_text<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    source: TextSource,
) -> B::Node {
    match source {
        TextSource::Static(content) => backend.borrow_mut().create_text(&content),
        TextSource::Reactive(compute) => build_reactive_text(backend, compute),
    }
}

/// Creates an empty text node and an effect that re-runs `compute()` and
/// writes the result whenever the signals it reads change.
fn build_reactive_text<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    compute: Box<dyn Fn() -> String>,
) -> B::Node {
    let node = backend.borrow_mut().create_text("");
    let node_for_effect = node.clone();
    let backend = backend.clone();
    // Effect auto-registers with the active scope (set by render() or by a
    // when() rebuild). Drop is a no-op; the scope frees the slot.
    let _e = Effect::new(move || {
        let value = compute();
        backend.borrow_mut().update_text(&node_for_effect, &value);
    });
    node
}

fn build_view<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    children: Vec<Primitive>,
) -> B::Node {
    let mut parent = backend.borrow_mut().create_view();
    for child in children {
        let child_node = build(backend, child);
        backend.borrow_mut().insert(&mut parent, child_node);
    }
    parent
}

/// RAII wrapper that calls `Backend::on_node_unstyled` when dropped.
/// Captured by the styled effect's closure so backend per-node state
/// (e.g. the web backend's dynamic CSS class slot) gets cleaned up
/// when the effect's scope drops — which happens on `when()` rebuilds
/// and on `Owner` teardown.
struct StyleHandle<B: Backend + 'static> {
    backend: Rc<RefCell<B>>,
    node: B::Node,
}

impl<B: Backend + 'static> Drop for StyleHandle<B> {
    fn drop(&mut self) {
        self.backend.borrow_mut().on_node_unstyled(&self.node);
    }
}

/// Attaches a style to an already-constructed node by spawning an
/// independent reactive Effect that re-applies on each signal change.
/// The effect captures a `StyleHandle` so that when its scope drops
/// the backend gets `on_node_unstyled` notification for per-node
/// cleanup (e.g. dropping the web backend's dynamic CSS rule).
///
/// Independent of any content effect on the same node — a content
/// signal change doesn't re-fire the style effect, and vice versa.
fn attach_style<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    node: &B::Node,
    style: StyleSource,
) -> Rc<dyn Fn(StateBits, bool)> {
    let node_for_effect = node.clone();
    let backend_for_effect = backend.clone();

    let handle = StyleHandle {
        backend: backend.clone(),
        node: node.clone(),
    };

    let handles_states_natively = backend.borrow().handles_states_natively();

    // Per-node active interaction states. For backends that don't
    // handle states natively (Android, iOS), we keep a Signal<StateBits>
    // that flips on native events; the style effect re-resolves on
    // each flip and merges the relevant `__state_*` axes.
    //
    // For backends that DO handle states natively (web), the signal
    // exists but is never observed by the style effect — `apply_styled_states`
    // pre-emits all state overlays as CSS pseudo-class rules, so the
    // browser drives state tracking without a Rust round-trip.
    let states_signal: Signal<StateBits> = Signal::new(StateBits::NONE);

    let _e = Effect::new(move || {
        // Anchor the handle inside the closure so it's dropped iff the
        // effect is dropped.
        let _ = &handle.node;

        let app = style();

        let backend_for_register = backend_for_effect.clone();
        let backend_for_unregister = backend_for_effect.clone();
        style::ensure_registered_with(
            &app.sheet,
            |rules| {
                backend_for_register.borrow_mut().register_stylesheet(rules);
            },
            |rules| {
                backend_for_unregister
                    .borrow_mut()
                    .unregister_stylesheet(rules);
            },
        );

        if handles_states_natively {
            // Resolve the base (no state axes) and each declared state
            // overlay separately. The backend will emit CSS rules
            // scoped to each pseudo-class so the browser does the
            // state switching natively.
            //
            // We deliberately do NOT subscribe to `states_signal` here:
            // CSS handles all transitions, so the style effect should
            // re-fire only on theme/variant/override changes, not on
            // hover/press.
            let base = resolve_style(&app);
            let overlays = resolve_state_overlays(&app);
            backend_for_effect
                .borrow_mut()
                .apply_styled_states(&node_for_effect, &base, &overlays);
        } else {
            // Event-driven path: merge active-state axes into the
            // resolved application. Reading the signal subscribes this
            // effect to state changes, so a hover/press flip re-resolves
            // and re-applies through the regular apply_style path.
            let bits = states_signal.get();
            let mut app = app;
            for axis in bits.active_axes() {
                app = app.with(axis, "on");
            }
            let resolved = resolve_style(&app);
            backend_for_effect
                .borrow_mut()
                .apply_style(&node_for_effect, &resolved);
        }
    });

    // Hand the backend a setter so it can flip state bits from native
    // event listeners. The setter is `Rc<dyn Fn(StateBits, bool)>`
    // so the backend can clone it into per-event closures, and also
    // returned to the caller so it can wire prop-driven states like
    // `disabled` from the same signal.
    //
    // On natively-handling backends, the setter still flips the
    // signal (so `attach_disabled` can drive the DISABLED bit through
    // the same path), but the style effect doesn't observe it. The
    // `set_disabled` call inside `attach_disabled` is what actually
    // matters there — the attribute change activates `:disabled` CSS.
    let setter: Rc<dyn Fn(StateBits, bool)> = Rc::new(move |bit, on| {
        states_signal.update(|bits| {
            *bits = if on { bits.with(bit) } else { bits.without(bit) };
        });
    });
    backend.borrow_mut().attach_states(node, setter.clone());
    setter
}

/// For backends that handle states natively, resolve each declared
/// state overlay against the application's variants + theme. Walks
/// the stylesheet's variant keys looking for `__state_*` axes,
/// resolves each one with the corresponding axis set to `"on"`, and
/// returns `(StateBits, Rc<StyleRules>)` pairs the backend can emit
/// as pseudo-class CSS.
fn resolve_state_overlays(app: &StyleApplication) -> Vec<(StateBits, Rc<StyleRules>)> {
    let mut out: Vec<(StateBits, Rc<StyleRules>)> = Vec::new();
    for (axis, _value) in app.sheet.variant_keys() {
        let bit = match axis.as_str() {
            "__state_hovered" => StateBits::HOVERED,
            "__state_pressed" => StateBits::PRESSED,
            "__state_focused" => StateBits::FOCUSED,
            "__state_disabled" => StateBits::DISABLED,
            _ => continue,
        };
        // Skip duplicates (the keys list contains one entry per
        // declared value; each `__state_*` axis only has the single
        // value "on", but check for safety).
        if out.iter().any(|(b, _)| *b == bit) {
            continue;
        }
        // Resolve with this single state axis added on top of the
        // application's existing variants.
        let mut state_app = app.clone();
        state_app = state_app.with(axis, "on");
        let resolved = resolve_style(&state_app);
        out.push((bit, resolved));
    }
    out
}

/// Reactive disabled-state wiring. Runs the user's closure inside an
/// `Effect` so the result tracks any signals it reads. On each
/// firing: (1) calls `Backend::set_disabled` so the native widget
/// is marked inert (web `disabled` attr, Android `setEnabled`); and
/// (2) flips the `DISABLED` state bit on the styled node so any
/// `state disabled { ... }` overlay applies via the existing state
/// machinery. If the button has no styled effect, `state_setter` is
/// `None` and step 2 is skipped.
fn attach_disabled<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    node: &B::Node,
    disabled: Box<dyn Fn() -> bool>,
    state_setter: Option<Rc<dyn Fn(StateBits, bool)>>,
) {
    let node_for_effect = node.clone();
    let backend_for_effect = backend.clone();
    let _e = Effect::new(move || {
        let d = disabled();
        backend_for_effect
            .borrow_mut()
            .set_disabled(&node_for_effect, d);
        if let Some(setter) = state_setter.as_ref() {
            setter(StateBits::DISABLED, d);
        }
    });
}

/// Renders a `When` primitive as a placeholder container whose subtree is
/// swapped each time `cond()` flips.
///
/// Lifecycle: the outer effect (registered with the surrounding scope)
/// reads `cond()` to track its dependencies. On every change it drops
/// the previous branch's nested `Scope` — freeing every signal and effect
/// in the old subtree atomically — and builds the new branch inside a
/// fresh nested scope.
/// Build a Virtualizer node. Sets up the callback bundle the
/// backend uses to query data + mount/release items, wraps each
/// `render_item(idx)` call in a fresh per-item Scope so signals
/// and effects nested inside an item are freed when the item is
/// released, and installs an Effect on the data so the backend
/// gets notified when item_count / keys / sizes change.
fn build_virtualizer<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    item_count: Box<dyn Fn() -> usize>,
    item_key: Box<dyn Fn(usize) -> primitives::virtualizer::ItemKey>,
    item_size: primitives::virtualizer::ItemSize,
    render_item: Rc<dyn Fn(usize) -> Primitive>,
    overscan: f32,
    horizontal: bool,
) -> B::Node {
    // Per-item scope registry, owned by an Rc so the mount/release
    // closures (which live in the backend) share it. The framework
    // hands out monotonically-increasing u64 ids to identify each
    // mounted item; the backend stores the id alongside its cell so
    // it can release later.
    //
    // Also store measured sizes here. Backends that measure (web
    // ResizeObserver, native layout listeners) push updates via
    // `set_measured_size`; the framework keeps the canonical map.
    let scopes: Rc<RefCell<HashMap<u64, Box<reactive::Scope>>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let measured_sizes: Rc<RefCell<HashMap<u64, f32>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let next_scope_id: Rc<RefCell<u64>> = Rc::new(RefCell::new(0));

    // Shareable closures for the data side. `Rc` so the backend can
    // clone them into per-event handlers.
    let item_count_rc: Rc<dyn Fn() -> usize> = Rc::from(item_count);
    let item_key_rc: Rc<dyn Fn(usize) -> primitives::virtualizer::ItemKey> = Rc::from(item_key);

    let measure_sizes = item_size.is_measured();
    let item_size_rc: Rc<dyn Fn(usize) -> f32> = match item_size {
        primitives::virtualizer::ItemSize::Known(f)
        | primitives::virtualizer::ItemSize::Measured(f) => f,
    };

    // `item_size` callback wraps the user's known/estimate with the
    // measured-override store: if we have a measured size, use it;
    // otherwise fall back to the user's value.
    let item_size_with_override: Rc<dyn Fn(usize) -> f32> = {
        let user = item_size_rc.clone();
        let measured = measured_sizes.clone();
        let key_fn = item_key_rc.clone();
        Rc::new(move |idx| {
            let key = key_fn(idx);
            // Measured cache is keyed by item key (not index) so it
            // survives reorderings.
            if let Some(v) = measured.borrow().get(&key) {
                return *v;
            }
            user(idx)
        })
    };

    // mount_item: build the subtree for `idx` inside a fresh Scope,
    // return its native node + the scope id.
    let mount_item: Rc<dyn Fn(usize) -> (B::Node, u64)> = {
        let scopes = scopes.clone();
        let next_id = next_scope_id.clone();
        let render = render_item.clone();
        let backend = backend.clone();
        Rc::new(move |idx| {
            let mut scope = Box::new(reactive::Scope::new());
            let primitive = reactive::with_scope(&mut scope, || render(idx));
            let node = build(&backend, primitive);
            let id = {
                let mut n = next_id.borrow_mut();
                let v = *n;
                *n = n.checked_add(1).unwrap_or(0);
                v
            };
            scopes.borrow_mut().insert(id, scope);
            (node, id)
        })
    };

    // release_item: drop the scope, freeing every signal/effect/ref
    // scoped to the item.
    let release_item: Rc<dyn Fn(u64)> = {
        let scopes = scopes.clone();
        let measured = measured_sizes.clone();
        Rc::new(move |id| {
            // Drop the scope. Its Drop impl frees the reactive slots.
            scopes.borrow_mut().remove(&id);
            // We can't safely free the measured-size entry here
            // because the entry is keyed by item *key*, not scope
            // id. The measured cache survives unmount intentionally
            // — when the item re-enters the window, we want to use
            // the previously-measured size rather than start over
            // with an estimate.
            let _ = measured;
        })
    };

    // set_measured_size: backend tells us "this scope's rendered
    // size is X." We store it by item key so the cache survives
    // unmount/remount.
    //
    // Backend identifies the item by scope id; we look up the key
    // by walking which idx this scope was mounted for. Simpler:
    // have the backend pass the *index* too. But scope_id is what
    // it stored, and it doesn't know the current index after
    // reorders. So we maintain a scope_id -> key reverse map.
    let scope_id_to_key: Rc<RefCell<HashMap<u64, primitives::virtualizer::ItemKey>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let set_measured_size: Rc<dyn Fn(u64, f32)> = {
        let measured = measured_sizes.clone();
        let map = scope_id_to_key.clone();
        Rc::new(move |scope_id, size| {
            if let Some(key) = map.borrow().get(&scope_id) {
                measured.borrow_mut().insert(*key, size);
            }
        })
    };

    // Augment mount_item to also record scope_id -> key.
    let mount_item: Rc<dyn Fn(usize) -> (B::Node, u64)> = {
        let inner = mount_item.clone();
        let key_fn = item_key_rc.clone();
        let map = scope_id_to_key.clone();
        Rc::new(move |idx| {
            let (node, id) = inner(idx);
            let k = key_fn(idx);
            map.borrow_mut().insert(id, k);
            (node, id)
        })
    };

    // Augment release_item to clean up the scope_id -> key entry.
    let release_item: Rc<dyn Fn(u64)> = {
        let inner = release_item.clone();
        let map = scope_id_to_key.clone();
        Rc::new(move |id| {
            map.borrow_mut().remove(&id);
            inner(id);
        })
    };

    let callbacks = VirtualizerCallbacks {
        item_count: item_count_rc.clone(),
        item_key: item_key_rc.clone(),
        item_size: item_size_with_override,
        measure_sizes,
        mount_item,
        release_item,
        set_measured_size,
    };

    let node = backend.borrow_mut().create_virtualizer(callbacks, overscan, horizontal);

    // Effect: re-fires whenever the data signal changes (any reads
    // inside item_count / item_key / etc. subscribe). We tell the
    // backend to re-diff its mounted set.
    {
        let backend = backend.clone();
        let node = node.clone();
        let count = item_count_rc.clone();
        let _e = Effect::new(move || {
            // Touch item_count so we subscribe to the data signal.
            // (item_count's body calls data.get().) We don't use the
            // value here directly — the backend re-queries.
            let _ = count();
            backend.borrow_mut().virtualizer_data_changed(&node);
        });
    }

    node
}

fn build_when<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    cond: Box<dyn Fn() -> bool>,
    then: Box<dyn Fn() -> Primitive>,
    otherwise: Box<dyn Fn() -> Primitive>,
) -> B::Node {
    let placeholder = backend.borrow_mut().create_view();
    let backend_for_effect = backend.clone();
    let placeholder_for_effect = placeholder.clone();

    // The branch scope lives across effect re-runs. Rc<RefCell<Option<…>>>
    // so we can replace it atomically when the condition flips.
    let branch_scope: Rc<RefCell<Option<Box<reactive::Scope>>>> = Rc::new(RefCell::new(None));
    let branch_scope_for_effect = branch_scope.clone();

    let _e = Effect::new(move || {
        let active = cond();

        // Drop the previous branch's scope before building the new one,
        // freeing its signals + effects atomically.
        *branch_scope_for_effect.borrow_mut() = None;
        backend_for_effect
            .borrow_mut()
            .clear_children(&placeholder_for_effect);

        // Build inside a fresh nested scope. `untrack` keeps inner setup
        // reads from subscribing to *this* outer effect — inner effects
        // subscribe themselves when they run.
        let mut new_scope = Box::new(reactive::Scope::new());
        untrack(|| {
            reactive::with_scope(&mut new_scope, || {
                let branch = if active { then() } else { otherwise() };
                let child_node = build(&backend_for_effect, branch);
                let mut placeholder_mut = placeholder_for_effect.clone();
                backend_for_effect
                    .borrow_mut()
                    .insert(&mut placeholder_mut, child_node);
            });
        });
        *branch_scope_for_effect.borrow_mut() = Some(new_scope);
    });

    placeholder
}
