//! Author-facing DSL: `Bound<H>`, `Bindable<H>`, `ChildList`, the
//! primitive constructors (`view`, `text`, `button`, `when`,
//! `switch`), and `IntoElement` / `IntoDisabledSource`.
//!
//! These compose into the fluent builder pattern call sites use:
//!
//! ```ignore
//! button("Click me", || count.update(|n| *n += 1))
//!     .with_style(primary_button_style())
//!     .bind(button_ref)
//! ```
//!
//! Each constructor returns a `Bound<HandleType>` so `.bind(r)` is
//! type-checked against the call-site `Ref<HandleType>`.

use crate::accessibility::{
    AccessibilityProps, AccessibilityTraits, LiveRegionPriority, Role,
};
use crate::handles::{ButtonHandle, PressableHandle, RefFill, TextHandle, ViewHandle};
use crate::element::{EachKey, EachRowBuild, Element};
use crate::reactive::Ref;
use crate::sources::{IntoStyleSource, IntoTextSource};
use std::cell::RefCell;
use std::hash::Hash;
use std::rc::Rc;

// =============================================================================
// Bound<H> — primitive + phantom handle type for .bind() type-checking
// =============================================================================
//
// A constructor like `button(...)` returns `Bound<ButtonHandle>` rather
// than a bare `Element`. Carrying the handle type in the type system
// makes `.bind(r: Ref<ButtonHandle>)` a compile-time check — passing
// `Ref<ViewHandle>` to a button's `.bind` is a type error, no runtime
// dispatch needed.
//
// `Bound<H>` implements `Into<Element>` and `ChildList`, so call sites
// and the rest of the framework continue to work with `Element` after
// `.bind()` (or without ever calling it). Authors who don't care about
// refs never see `Bound` — the constructors return it, the children
// macro coerces it, no friction.

/// A `Element` plus a phantom handle type. Constructed by primitive
/// builder functions (`button(...)`, `view(...)`, …); coerced back to
/// `Element` automatically for child lists. Only purpose: type-check
/// `.bind(r)` against the call-site `Ref<H>`.
pub struct Bound<H> {
    pub(crate) primitive: Element,
    _handle: std::marker::PhantomData<H>,
}

impl<H> Bound<H> {
    /// Wrap a `Element` in a typed `Bound<H>`. The handle marker `H`
    /// is purely a type-check hook for `.bind(r: Ref<H>)`; it doesn't
    /// affect the wrapped primitive.
    ///
    /// First-party primitives use their dedicated builder functions
    /// (`view(...)`, `button(...)`, …) which call this internally.
    /// Third-party SDK crates that want a typed handle (e.g.
    /// `Bound<WebViewHandle>` for `webview::WebView(...)`) build a
    /// `Element::External` and wrap it here.
    pub fn new(primitive: Element) -> Self {
        Self { primitive, _handle: std::marker::PhantomData }
    }

    /// Mutable access to the wrapped `Element`. Public so the
    /// `ui!` macro's structured-emission paths can fill in
    /// per-primitive fields that the closure-shape builders don't
    /// expose — e.g. patching `Virtualizer.row_template` and
    /// `row_index_signal_id` after going through
    /// `primitives::virtualizer::virtualizer(...)`.
    #[doc(hidden)]
    pub fn primitive_mut(&mut self) -> &mut Element {
        &mut self.primitive
    }

    /// Attaches a style. Same semantics as `Element::with_style`.
    pub fn with_style<S: IntoStyleSource>(mut self, style: S) -> Self {
        self.primitive = self.primitive.with_style(style);
        self
    }

    /// Assigns a test ID for robot/automation queries. Always present (so the
    /// `ui!` macro can emit `.test_id(...)` without depending on the `robot`
    /// feature at expansion time): a real store under `robot`, an inert no-op
    /// otherwise (the `test_id` element fields only exist under `robot`).
    #[cfg(feature = "robot")]
    pub fn test_id(mut self, id: &'static str) -> Self {
        self.primitive = self.primitive.with_test_id(id);
        self
    }

    /// No-op stub when `robot` is off — keeps `ui! { view(test_id = …) }`
    /// compiling in production builds (the id is simply discarded).
    #[cfg(not(feature = "robot"))]
    pub fn test_id(self, _id: &'static str) -> Self {
        self
    }

    /// Replace this primitive's accessibility props wholesale. Use the
    /// granular `a11y_*` setters below for the common single-field case;
    /// reach for this when building the whole [`AccessibilityProps`] at
    /// once (e.g. attaching custom `actions`). Same semantics as
    /// [`Element::with_accessibility`].
    pub fn accessibility(mut self, a11y: AccessibilityProps) -> Self {
        self.primitive = self.primitive.with_accessibility(a11y);
        self
    }

    /// Set the spoken accessibility label (screen-reader name).
    /// `None`-by-default means backends derive a name from the
    /// primitive's natural content; setting this overrides that.
    pub fn a11y_label(mut self, label: impl Into<String>) -> Self {
        if let Some(a) = self.primitive.accessibility_mut() {
            a.label = Some(label.into());
        }
        self
    }

    /// Set the longer accessibility hint ("Double tap to open menu").
    pub fn a11y_hint(mut self, hint: impl Into<String>) -> Self {
        if let Some(a) = self.primitive.accessibility_mut() {
            a.hint = Some(hint.into());
        }
        self
    }

    /// Override the inferred accessibility [`Role`]. By default every
    /// primitive ships a sensible role; set this when the visible shape
    /// differs from the a11y intent (e.g. a styled `pressable` that is
    /// semantically a link).
    pub fn a11y_role(mut self, role: Role) -> Self {
        if let Some(a) = self.primitive.accessibility_mut() {
            a.role = Some(role);
        }
        self
    }

    /// Hide this primitive (and its descendants) from the accessibility
    /// tree — for purely decorative content. Maps to `aria-hidden`,
    /// `accessibilityElementsHidden`, etc.
    pub fn a11y_hidden(mut self, hidden: bool) -> Self {
        if let Some(a) = self.primitive.accessibility_mut() {
            a.hidden = hidden;
        }
        self
    }

    /// Set the orthogonal accessibility state flags
    /// ([`AccessibilityTraits`]) — selected, disabled, expanded, etc.
    pub fn a11y_traits(mut self, traits: AccessibilityTraits) -> Self {
        if let Some(a) = self.primitive.accessibility_mut() {
            a.traits = traits;
        }
        self
    }

    /// Mark this primitive as a live region so platform AX announces
    /// updates to its label at the given [`LiveRegionPriority`].
    pub fn live_region(mut self, priority: LiveRegionPriority) -> Self {
        if let Some(a) = self.primitive.accessibility_mut() {
            a.live_region = Some(priority);
        }
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
        if let Element::Button { ref_fill, .. } = &mut self.primitive {
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
        if let Element::Button { disabled: slot, .. } = &mut self.primitive {
            *slot = Some(disabled.into_disabled_source());
        }
        self
    }

    /// Set a leading icon (rendered before the label).
    pub fn leading_icon(mut self, icon: crate::IconData) -> Self {
        if let Element::Button { leading_icon, .. } = &mut self.primitive {
            *leading_icon = Some(icon);
        }
        self
    }

    /// Set a trailing icon (rendered after the label).
    pub fn trailing_icon(mut self, icon: crate::IconData) -> Self {
        if let Element::Button { trailing_icon, .. } = &mut self.primitive {
            *trailing_icon = Some(icon);
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
        if let Element::View { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::View(Box::new(move |h| r.fill(h))));
        }
        self
    }

    /// Opt this view into safe-area-aware padding on the given
    /// sides. The platform's safe-area inset is added to the
    /// matching side of the view's padding reactively — orientation
    /// flips and similar updates re-apply without a rebuild.
    ///
    /// Examples:
    /// ```ignore
    /// View(...).safe_area(SafeAreaSides::TOP)
    /// View(...).safe_area(SafeAreaSides::TOP | SafeAreaSides::BOTTOM)
    /// View(...).safe_area(SafeAreaSides::ALL)
    /// ```
    ///
    /// Calling twice OR-merges the flags rather than replacing —
    /// `.safe_area(TOP).safe_area(BOTTOM)` is the same as
    /// `.safe_area(TOP | BOTTOM)`.
    ///
    /// Nesting: if a parent and a child both opt into the same side
    /// the inset stacks (each adds its own padding). Author code
    /// should put `.safe_area(...)` on the outermost container that
    /// needs it.
    pub fn safe_area(mut self, sides: crate::SafeAreaSides) -> Self {
        if let Element::View { safe_area_sides, .. } = &mut self.primitive {
            *safe_area_sides |= sides;
        }
        self
    }

    /// Mark this view as a **container-query containment context**.
    /// Descendant `container (min_width: N)` style overlays then resolve
    /// against *this* view's resolved inline-size, not the global
    /// viewport — so the same component lays itself out differently in a
    /// narrow sidebar vs. a wide main column.
    ///
    /// **Inline-size containment invariant:** the container's width must
    /// be determinate from its parent (an explicit width, a percentage,
    /// or a flex track) — never shrink-to-fit from the descendants that
    /// query it. Querying a content-sized width is a cycle and is
    /// unsupported (web enforces this via `container-type: inline-size`;
    /// native relies on it for convergence). See
    /// [`crate::container_query`].
    ///
    /// No-op on non-`view` primitives.
    pub fn container(mut self) -> Self {
        if let Element::View { is_container, .. } = &mut self.primitive {
            *is_container = true;
        }
        self
    }

    /// Install a raw touch handler. The closure receives every
    /// [`TouchEvent`](crate::TouchEvent) the backend delivers to this
    /// view and returns a [`TouchResponse`](crate::TouchResponse) that
    /// drives the responder-chain bubble (`consumed`) and the claim
    /// protocol (`claim`).
    ///
    /// This is the lowest-level interaction primitive. Higher-level
    /// recognizers (tap, long-press, pan, …) are built on top of it
    /// in pure Rust — see the `touch::recognizers` module (TBD) for
    /// the prebuilt ones, or write your own.
    ///
    /// Calling twice replaces the handler.
    pub fn on_touch<F>(mut self, handler: F) -> Self
    where
        F: Fn(&crate::TouchEvent) -> crate::TouchResponse + 'static,
    {
        if let Element::View { on_touch, .. } = &mut self.primitive {
            // Born batched: every backend invocation of this handler runs
            // as one reactive cycle, so signal writes inside it coalesce.
            // See `reactive::cycle`. (A backend that also wraps the call in
            // `cycle`/`batch` just nests harmlessly.)
            *on_touch = Some(std::rc::Rc::new(move |e: &crate::TouchEvent| {
                crate::cycle(|| handler(e))
            }));
        }
        self
    }

    /// Install a wheel / magnify handler — the desktop zoom/scroll channel,
    /// parallel to [`Bound::on_touch`]. The closure receives every
    /// [`WheelEvent`](crate::WheelEvent) the backend delivers to this view
    /// (web `wheel`, macOS `magnify:`/`scrollWheel:`) and returns a
    /// [`TouchResponse`](crate::TouchResponse) whose `consumed` flag asks the
    /// backend to suppress the platform default (page scroll / browser zoom).
    ///
    /// No-op on iOS / Android (no trackpad/wheel — use a `pinch` handler via
    /// [`Bound::on_touch`] there). The zoom SDK pairs the two for you.
    ///
    /// Calling twice replaces the handler.
    pub fn on_wheel<F>(mut self, handler: F) -> Self
    where
        F: Fn(&crate::WheelEvent) -> crate::TouchResponse + 'static,
    {
        if let Element::View { on_wheel, .. } = &mut self.primitive {
            // Born batched — see `on_touch` / `reactive::cycle`.
            *on_wheel = Some(std::rc::Rc::new(move |e: &crate::WheelEvent| {
                crate::cycle(|| handler(e))
            }));
        }
        self
    }

    /// Install a hover (pointer-over) handler — the desktop/web "is the
    /// cursor over me" channel. The closure fires `true` when the pointer
    /// enters this view and `false` when it leaves.
    ///
    /// A pointer concept: delivered on web (`pointerenter`/`pointerleave`)
    /// and macOS (`NSTrackingArea`); a **no-op on touch-only backends**
    /// (iOS / Android) — there is no hovering with a finger. Pair it with a
    /// `long_press` recognizer via [`Bound::on_touch`] for the touch
    /// affordance (this is what `idea-ui`'s `Tooltip` does).
    ///
    /// Calling twice replaces the handler.
    pub fn on_hover<F>(mut self, handler: F) -> Self
    where
        F: Fn(bool) + 'static,
    {
        if let Element::View { on_hover, .. } = &mut self.primitive {
            // Born batched — see `on_touch` / `reactive::cycle`.
            *on_hover = Some(std::rc::Rc::new(move |entering: bool| {
                crate::cycle(|| handler(entering))
            }));
        }
        self
    }
}

impl Bound<PressableHandle> {
    /// Same shape as [`Bound::<ButtonHandle>::bind`] — the framework
    /// fills the ref with a `PressableHandle` constructed from the
    /// just-mounted backend node.
    pub fn bind(mut self, r: Ref<PressableHandle>) -> Self {
        if let Element::Pressable { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::Pressable(Box::new(move |h| r.fill(h))));
        }
        self
    }

    /// Reactively disable the pressable. See
    /// [`Bound::<ButtonHandle>::disabled`] for semantics — same
    /// state-bit + native-inert behavior.
    pub fn disabled<D: IntoDisabledSource>(mut self, disabled: D) -> Self {
        if let Element::Pressable { disabled: slot, .. } = &mut self.primitive {
            *slot = Some(disabled.into_disabled_source());
        }
        self
    }
}

impl Bound<TextHandle> {
    pub fn bind(mut self, r: Ref<TextHandle>) -> Self {
        if let Element::Text { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::Text(Box::new(move |h| r.fill(h))));
        }
        self
    }
}

impl<H> From<Bound<H>> for Element {
    fn from(b: Bound<H>) -> Element { b.primitive }
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
//   plumbing through `Element` needed.
//
// Both implement `Into<Element>` and `ChildList` so the rest of the
// framework (children lists, `IntoElement` coercion) doesn't care
// whether the call site uses one or the other.

/// A `Element` plus an already-constructed component handle. Returned
/// by user `#[component]` functions that expose imperative methods.
/// Authors construct this in their component body and `.bind(r)` to
/// hook it up to a `Ref<H>` owned by the parent.
pub struct Bindable<H> {
    primitive: Element,
    handle: H,
}

impl<H: 'static> Bindable<H> {
    /// Constructs a `Bindable` from the component's primitive tree and
    /// the handle it exposes. Called from inside the component body —
    /// typically as the final expression.
    pub fn new(primitive: Element, handle: H) -> Self {
        Self { primitive, handle }
    }

    /// Attaches a style to the component's root primitive. Same
    /// semantics as `Element::with_style` / `Bound::with_style` —
    /// the inner primitive's style slot is overwritten, and the chain
    /// returns `Self` so subsequent calls like `.bind(r)` keep the
    /// handle type.
    pub fn with_style<S: IntoStyleSource>(mut self, style: S) -> Self {
        self.primitive = self.primitive.with_style(style);
        self
    }

    /// Fills `r` with this component's handle and returns the
    /// underlying `Element`. The fill happens *immediately* — the
    /// handle exists by the time the component function returned, so
    /// there's no mount-time deferral.
    ///
    /// Compile-time type checking: `r: Ref<H>` and the component
    /// returns `Bindable<H>`, so passing the wrong ref type is a type
    /// error.
    pub fn bind(self, r: Ref<H>) -> Element {
        r.fill(self.handle);
        self.primitive
    }
}

impl<H> From<Bindable<H>> for Element {
    fn from(b: Bindable<H>) -> Element { b.primitive }
}

impl<H> ChildList for Bindable<H> {
    fn append_to(self, out: &mut Vec<Element>) {
        out.push(self.primitive);
    }
}

impl<H> ChildList for Option<Bindable<H>> {
    fn append_to(self, out: &mut Vec<Element>) {
        if let Some(b) = self {
            out.push(b.primitive);
        }
    }
}

// =============================================================================
// ChildList trait + impls
// =============================================================================

/// Flexible-shape source for a child-list slot. Implementors say how to
/// append themselves (zero or more primitives) to a growing Vec. Used by
/// the `children!(...)` macro so call sites can mix:
///   - a single `Element`
///   - `Option<Element>` (often from `cond.then(|| ...)`)
///   - `Vec<Element>` (e.g. from a `.map().collect()`)
pub trait ChildList {
    fn append_to(self, out: &mut Vec<Element>);
}

impl ChildList for Element {
    fn append_to(self, out: &mut Vec<Element>) {
        out.push(self);
    }
}

impl<H> ChildList for Bound<H> {
    fn append_to(self, out: &mut Vec<Element>) {
        out.push(self.primitive);
    }
}

impl ChildList for Option<Element> {
    fn append_to(self, out: &mut Vec<Element>) {
        if let Some(p) = self {
            out.push(p);
        }
    }
}

impl<H> ChildList for Option<Bound<H>> {
    fn append_to(self, out: &mut Vec<Element>) {
        if let Some(b) = self {
            out.push(b.primitive);
        }
    }
}

impl ChildList for Vec<Element> {
    fn append_to(self, out: &mut Vec<Element>) {
        out.extend(self);
    }
}

// =============================================================================
// Element constructors: view, text, button, when, switch
// =============================================================================

pub fn view(children: Vec<Element>) -> Bound<ViewHandle> {
    Bound::new(Element::View {
        children,
        style: None,
        ref_fill: None,
        safe_area_sides: crate::SafeAreaSides::NONE,
        on_touch: None,
        on_wheel: None,
        on_hover: None,
        is_container: false,
        accessibility: crate::accessibility::AccessibilityProps::default(),
        #[cfg(feature = "robot")]
        test_id: None,
    })
}

pub fn text<T: IntoTextSource>(source: T) -> Bound<TextHandle> {
    Bound::new(Element::Text {
        source: source.into_text_source(),
        style: None,
        ref_fill: None,
        accessibility: crate::accessibility::AccessibilityProps::default(),
        #[cfg(feature = "robot")]
        test_id: None,
    })
}

pub fn button<L, A>(label: L, on_click: A) -> Bound<ButtonHandle>
where
    L: IntoTextSource,
    A: crate::derive::IntoAction,
{
    Bound::new(Element::Button {
        label: label.into_text_source(),
        on_click: on_click.into_action(),
        leading_icon: None,
        trailing_icon: None,
        style: None,
        ref_fill: None,
        disabled: None,
        accessibility: crate::accessibility::AccessibilityProps::default(),
        #[cfg(feature = "robot")]
        test_id: None,
    })
}

/// Clickable container — like [`view`] but with a press callback.
/// See [`Element::Pressable`] for why a separate primitive exists
/// rather than `View` gaining an optional `on_click`.
pub fn pressable<F: Fn() + 'static>(
    children: Vec<Element>,
    on_click: F,
) -> Bound<PressableHandle> {
    Bound::new(Element::Pressable {
        children,
        // Born batched — see `Bound::on_touch` / `reactive::cycle`.
        on_click: Rc::new(move || crate::cycle(|| on_click())),
        style: None,
        ref_fill: None,
        disabled: None,
        accessibility: crate::accessibility::AccessibilityProps::default(),
        #[cfg(feature = "robot")]
        test_id: None,
    })
}

/// Reactive conditional. Author code provides three closures:
/// - `cond` reads one or more signals and returns a `bool`.
/// - `then` and `otherwise` each return a `Element` to render.
///
/// When any signal `cond()` reads changes, the active branch is rebuilt
/// from scratch. The hidden branch's effects are dropped, so any signal
/// subscriptions in it are released. State in the hidden branch is lost
/// on toggle — this is the "dispose on hide" model.
pub fn when<C, T, O>(cond: C, then: T, otherwise: O) -> Element
where
    C: crate::derive::IntoDerived<bool>,
    T: Fn() -> Element + 'static,
    O: Fn() -> Element + 'static,
{
    Element::When {
        cond: cond.into_derived(),
        then: Box::new(then),
        otherwise: Box::new(otherwise),
        style: None,
    }
}

/// A **fragment** — a layout-transparent group of sibling elements
/// that render as flat children of the surrounding parent, with no
/// wrapper view.
///
/// Use it when a function (typically a `#[component]`) conceptually
/// produces *several siblings* but must return a single [`Element`]:
/// instead of returning `Vec<Element>` (which `#[component]` can't) or
/// wrapping the children in a `view(...)` (which would introduce a
/// layout box and break absolutely-positioned overlays / `flex: 1`
/// children), return `fragment(children)`. In a children list the
/// walker splices the children directly into the parent — the exact
/// same result as if the caller had spread the `Vec` inline.
///
/// ```ignore
/// #[component]
/// pub fn Chrome(props: &ChromeProps) -> Element {
///     fragment(vec![brand_bar(...), toolbar(...), status_pill(...)])
/// }
/// ```
///
/// Built once, never reconciled. For a reactive child set use
/// [`switch`]/`when`/`for`; for a count-based loop use the `ui!` `for`
/// lowering. See [`Element::Fragment`].
pub fn fragment(children: Vec<Element>) -> Element {
    Element::Fragment { children }
}

/// Reactive multi-way conditional. `scrutinee` reads one or more
/// signals and returns a value of any `PartialEq + 'static` type
/// (typically an enum or a small key). `branches` is a function that
/// builds the active subtree for a given scrutinee value — usually a
/// `match` over the enum.
///
/// The walker wraps the scrutinee in an `Effect` so any signal change
/// the closure reads re-runs it; the result is compared with the
/// previously-seen value via `PartialEq` and the subtree is rebuilt
/// only when the value actually changes. State inside the prior
/// subtree is freed atomically, mirroring `when()`.
///
/// Idiomatic use is via `ui!`'s `match` lowering — author code writes
/// a normal `match expr { Variant => ui!{...}, … }` and the macro
/// emits this call. Direct calls work too:
///
/// ```ignore
/// switch(|| screen.get(), |s| match s {
///     Screen::Summary => summary().into(),
///     Screen::Performance => performance().into(),
/// })
/// ```
pub fn switch<S, F, B>(scrutinee: F, branches: B) -> Element
where
    S: PartialEq + 'static,
    F: Fn() -> S + 'static,
    B: Fn(&S) -> Element + 'static,
{
    use std::rc::Rc;

    // Closure-driven path. We don't ship the scrutinee value over
    // any wire — the discriminant `compute` is *opaque* (returns
    // `Null` after re-running the scrutinee for signal-subscription
    // purposes) and the `default` closure does the real arm
    // dispatch using the typed scrutinee directly. This keeps the
    // closure-driven API constraint-free (only `PartialEq` is
    // required, same as before the refactor) while still routing
    // through the structured `Element::Switch` shape that
    // generator backends consume.
    //
    // Authors who need generator-backend-compatible reactivity
    // (Roku) should construct the primitive through the structured
    // entry point (a `#[method]`-backed discriminant + literal-key
    // arms) — `Element::Switch` with a non-opaque `discriminant`
    // and a non-empty `arms` vec. The macro layer (`ui!`'s `match`
    // lowering, eventually) will emit that form.
    let scrutinee = Rc::new(scrutinee);
    let scrutinee_for_disc = scrutinee.clone();
    let last_key: Rc<RefCell<Option<S>>> = Rc::new(RefCell::new(None));
    let last_key_for_disc = last_key.clone();
    let discriminant = crate::derive::Derived::<crate::__serde_json::Value> {
        method: "",
        inputs: Vec::new(),
        initial: Vec::new(),
        compute: Rc::new(move || {
            // Subscribe to whatever signals the scrutinee reads, and
            // stash the result so `default()` can use it without
            // re-evaluating (which would double-subscribe in the
            // same Effect run on some reactivity backends).
            let v = scrutinee_for_disc();
            *last_key_for_disc.borrow_mut() = Some(v);
            crate::__serde_json::Value::Null
        }),
    };
    let dispatch: Box<dyn Fn() -> Element> = Box::new(move || {
        // Use the cached scrutinee value from the most-recent
        // discriminant evaluation. The walker's Effect always calls
        // `discriminant.compute()` immediately before `default()`,
        // so the cache is freshly populated.
        if let Some(s) = last_key.borrow().as_ref() {
            branches(s)
        } else {
            // Defensive fallback: re-read the scrutinee if the cache
            // somehow wasn't populated. Shouldn't happen under the
            // walker's contract.
            branches(&scrutinee())
        }
    });
    Element::Switch {
        discriminant,
        arms: Vec::new(),
        default: dispatch,
        style: None,
    }
}

/// Reactive list constructor — the full-rebuild dual of [`when`] /
/// [`switch`] for a *vector* of children. `build` reads one or more
/// Construct a keyed reactive list. `snapshot` reads the backing
/// signal(s) and returns the current ordered `(key, row-builder)` pairs;
/// the framework reconciles by key on every change — building only new
/// rows, dropping removed ones, and preserving the scope (and any
/// component-local signals) of rows whose key is unchanged. See
/// [`Element::Each`] for the full lifecycle and tracking contract.
///
/// Emitted by `ui!`'s `for PAT in ITER, key = K { … }` when `ITER` is a
/// signal; can also be called directly:
///
/// ```ignore
/// let items: Signal<Vec<Row>> = signal!(vec![]);
/// each_keyed(move || {
///     items.get().into_iter().map(|row| {
///         let id = row.id;
///         let build: EachRowBuild = Box::new(move || vec![text(row.label)]);
///         (EachKey::new(id), build)
///     }).collect()
/// })
/// ```
///
/// A keyed reactive `for` compiles:
///
/// ```no_run
/// use runtime_core::{signal, ui, Element, Signal};
/// let items: Signal<Vec<i32>> = signal!(vec![1, 2, 3]);
/// let _tree: Element = ui! {
///     view {
///         for n in items, key = *n {
///             text { n.to_string() }
///         }
///     }
/// };
/// ```
///
/// A keyless reactive `for` does NOT (the missing key is a compile
/// error — reactive lists must be keyed so per-row state survives):
///
/// ```compile_fail
/// use runtime_core::{signal, ui, Element, Signal};
/// let items: Signal<Vec<i32>> = signal!(vec![1, 2, 3]);
/// let _tree: Element = ui! {
///     view {
///         for n in items {
///             text { n.to_string() }
///         }
///     }
/// };
/// ```
pub fn each_keyed(
    snapshot: impl Fn() -> Vec<(EachKey, EachRowBuild)> + 'static,
) -> Element {
    Element::Each {
        snapshot: Box::new(snapshot),
        style: None,
    }
}

// =============================================================================
// for-each dispatch — type-driven reactive-vs-static iteration.
// =============================================================================
//
// `ui!`'s `for PAT in ITER { … }` lowers to
// `ITER.__idealyst_for_each(|item| row_children)` (keyless) or
// `ITER.__idealyst_for_each_keyed(|item| KEY, |item| row_children)`
// (when `, key = KEY` is present) inside a block that brings BOTH
// traits below into scope. Rust method resolution then picks the impl
// from ITER's *type*:
//
//   - `Signal<C>` (a signal of any cloneable iterable) → `ReactiveForEach`
//     → a keyed `Element::Each` that reconciles on change.
//   - any other `IntoIterator` (Vec, &Vec, array, range, HashMap, …) →
//     `StaticForEach` → a flat `Vec<Element>` built once.
//
// There is NO `.get()` substring heuristic involved — the *type*
// decides, so a `HashMap::get()` or any incidental `.get()` in the
// iterable can never accidentally make a loop reactive, and a real
// signal iterable can never be silently missed. The two traits don't
// overlap because `Signal<C>` does not implement `IntoIterator`, so no
// type satisfies both and method resolution is unambiguous.
//
// **Compile-time key requirement.** A reactive list MUST have a key so
// the reconciler can preserve per-row state across mutations. We enforce
// that in the type system rather than at runtime: `ReactiveForEach`'s
// keyless `__idealyst_for_each` is gated behind `Self: ReactiveListKeyed`
// — a bound that's never satisfied — so a keyless `for x in signal { … }`
// fails to compile with the `ReactiveListKeyed` diagnostic. The static
// path keeps its keyless method (static lists never reconcile, so a key
// would be meaningless), and also accepts a key for call-site symmetry.

/// Static (built-once) `for`-loop lowering for any `IntoIterator`. See
/// the module-level note above; the `ui!` macro selects this vs
/// [`ReactiveForEach`] by the iterable's type.
#[doc(hidden)]
pub trait StaticForEach<Item> {
    fn __idealyst_for_each<F: Fn(Item) -> Vec<Element> + 'static>(self, f: F)
        -> Vec<Element>;

    fn __idealyst_for_each_keyed<K, KF, F>(self, key_fn: KF, build_fn: F) -> Vec<Element>
    where
        K: Eq + Hash + 'static,
        KF: Fn(&Item) -> K + 'static,
        F: Fn(Item) -> Vec<Element> + 'static;
}

/// Marker that gates the keyless reactive `for` lowering so that a
/// missing `key` is a **compile error** rather than a silent per-row
/// state reset. It is intentionally implemented for nothing: iterating a
/// reactive collection without a key can never type-check.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "this reactive `for` loop needs a `key`",
    label = "iterates a reactive collection — add `, key = <unique expr>` before the `{{`",
    note = "A `Signal<Vec<_>>` rebuilds its rows when it changes. Without a key the framework \
can't tell which rows are the same across updates, so each row's own state (component-local \
signals, text-input focus, scroll position) would be reset on every change.",
    note = "Give each row a stable, unique key derived from the item, e.g.:\n    \
for item in items, key = item.id {{ /* … */ }}"
)]
pub trait ReactiveListKeyed {}

/// Reactive `for`-loop lowering for a `Signal` of a cloneable iterable.
/// The keyed method produces a single keyed [`Element::Each`] (wrapped
/// in a one-element vec so the call site flattens it uniformly with the
/// static path); the keyless method is uncallable (see
/// [`ReactiveListKeyed`]).
#[doc(hidden)]
pub trait ReactiveForEach<Item> {
    fn __idealyst_for_each<F>(self, f: F) -> Vec<Element>
    where
        Self: ReactiveListKeyed,
        F: Fn(Item) -> Vec<Element> + 'static;

    fn __idealyst_for_each_keyed<K, KF, F>(self, key_fn: KF, build_fn: F) -> Vec<Element>
    where
        K: Eq + Hash + 'static,
        KF: Fn(&Item) -> K + 'static,
        F: Fn(Item) -> Vec<Element> + 'static;
}

impl<I, Item> StaticForEach<Item> for I
where
    I: IntoIterator<Item = Item>,
{
    fn __idealyst_for_each<F: Fn(Item) -> Vec<Element> + 'static>(
        self,
        f: F,
    ) -> Vec<Element> {
        let mut out = Vec::new();
        for item in self {
            out.extend(f(item));
        }
        out
    }

    fn __idealyst_for_each_keyed<K, KF, F>(self, _key_fn: KF, build_fn: F) -> Vec<Element>
    where
        K: Eq + Hash + 'static,
        KF: Fn(&Item) -> K + 'static,
        F: Fn(Item) -> Vec<Element> + 'static,
    {
        // A static list is built exactly once and never reconciled, so
        // the key is irrelevant — it exists only so a `key = …` clause is
        // *accepted* on a static `for` (e.g. while a list is being made
        // reactive). Build every row eagerly, same as the keyless path.
        let mut out = Vec::new();
        for item in self {
            out.extend(build_fn(item));
        }
        out
    }
}

impl<C, Item> ReactiveForEach<Item> for crate::Signal<C>
where
    C: Clone + IntoIterator<Item = Item> + 'static,
    Item: 'static,
{
    fn __idealyst_for_each<F>(self, _f: F) -> Vec<Element>
    where
        Self: ReactiveListKeyed,
        F: Fn(Item) -> Vec<Element> + 'static,
    {
        // Unreachable: `Signal<C>: ReactiveListKeyed` is never satisfied,
        // so this method cannot be called. Its `Self: ReactiveListKeyed`
        // bound is what turns a keyless reactive `for` into a compile
        // error (carrying the `ReactiveListKeyed` diagnostic).
        unreachable!("keyless reactive for-each is gated by ReactiveListKeyed")
    }

    fn __idealyst_for_each_keyed<K, KF, F>(self, key_fn: KF, build_fn: F) -> Vec<Element>
    where
        K: Eq + Hash + 'static,
        KF: Fn(&Item) -> K + 'static,
        F: Fn(Item) -> Vec<Element> + 'static,
    {
        let sig = self;
        let key_fn = Rc::new(key_fn);
        let build_fn = Rc::new(build_fn);
        // One reactive region. On each rebuild `snapshot` clones the
        // signal's current value and emits a (key, deferred-builder) pair
        // per item — cheap, no rows are built here. The reconciler then
        // calls a builder only for keys it hasn't already mounted, so an
        // unchanged row keeps its scope (and component-local signals).
        // The `each_keyed` Effect tracks the `sig.get()` read, so any
        // write to `sig` re-runs this.
        vec![each_keyed(move || {
            let mut out: Vec<(EachKey, EachRowBuild)> = Vec::new();
            for item in sig.get() {
                let key = EachKey::new(key_fn(&item));
                let build_fn = build_fn.clone();
                let build: EachRowBuild = Box::new(move || build_fn(item));
                out.push((key, build));
            }
            out
        })]
    }
}

// =============================================================================
// if dispatch — type-driven reactive-vs-static conditionals.
// =============================================================================
//
// `ui!`'s `if COND { … } else { … }` lowers — AFTER the macro's syntactic
// reactive paths (a visible inline `.get()` like `if sig.get() > 1`, and the
// structured `if key(state)` call shape) — to
// `(COND).__idealyst_if(|| then_nodes, || else_nodes)` inside a block that
// brings BOTH traits below into scope. Rust method resolution then picks the
// impl from COND's *type*:
//
//   - `bool` → `StaticCond` → the taken branch's flat node list, built once.
//   - `Signal<bool>` (what `memo(|| …)` returns) / `Derived<bool>` →
//     `ReactiveCond` → one reactive `when` that re-evaluates the condition and
//     swaps branches on change.
//
// This mirrors the for-loop's `StaticForEach`/`ReactiveForEach` dispatch: the
// *type* decides, so a bare `fn() -> bool` call is static (and allocates no
// reactive machinery) while a reactive `Signal<bool>` is reactive. There is no
// `.get()` substring guess at THIS step. The macro still detects an INLINE
// `.get()` read syntactically and wraps it in a `when` BEFORE reaching here,
// because such a condition's *type* is a plain `bool` and would otherwise
// dispatch to `StaticCond` — the same special-case the reactive `for` range
// (`for i in 0..n.get()`) needs.
//
// **Fn vs FnOnce — deliberate asymmetry.** `StaticCond` takes `FnOnce` branch
// thunks: a static branch is built exactly once, so it may freely MOVE
// captured non-`Copy` values, exactly like a plain Rust `if` block (no
// regression). `ReactiveCond` takes `Fn + 'static` thunks: a reactive branch
// is rebuilt on every change, so its captures must be re-readable — the same
// constraint a hand-written `when(...)` imposes. Because only ONE trait
// applies for a given condition type, the SAME emitted closure is checked
// against whichever bound the selected impl carries: static `if`s keep FnOnce
// ergonomics, reactive `if`s correctly require `Fn`.

/// Collapse a branch's flat node list to a single `Element` root: an empty
/// list → an empty `view`, exactly one node → that node verbatim (no wrapper),
/// many → one `view` wrapping them. Used by [`ReactiveCond`] (a reactive
/// branch needs one root per the anchor) and by the `ui!` macro's single-slot
/// `if`/`match` normalization (where a one-element result must NOT pick up a
/// spurious wrapper `view`).
#[doc(hidden)]
pub fn one_or_view(mut nodes: Vec<Element>) -> Element {
    if nodes.len() == 1 {
        nodes.pop().unwrap()
    } else {
        crate::IntoElement::into_element(view(nodes))
    }
}

/// Static (built-once) `if` lowering for a plain `bool`. See the module note
/// above; the `ui!` macro selects this vs [`ReactiveCond`] by the condition's
/// *type*. `FnOnce` branches — a static branch may move captured values.
#[doc(hidden)]
pub trait StaticCond {
    fn __idealyst_if<T, E>(self, then: T, else_: E) -> Vec<Element>
    where
        T: FnOnce() -> Vec<Element>,
        E: FnOnce() -> Vec<Element>;
}

impl StaticCond for bool {
    fn __idealyst_if<T, E>(self, then: T, else_: E) -> Vec<Element>
    where
        T: FnOnce() -> Vec<Element>,
        E: FnOnce() -> Vec<Element>,
    {
        if self {
            then()
        } else {
            else_()
        }
    }
}

/// Reactive `if` lowering for a reactive bool — a `Signal<bool>` (what `memo`
/// returns) or a `Derived<bool>`. Produces a single reactive `when` (wrapped
/// in a one-element vec so the call site flattens it uniformly with the static
/// path). `Fn + 'static` branches — a reactive branch is rebuilt on change.
#[doc(hidden)]
pub trait ReactiveCond {
    fn __idealyst_if<T, E>(self, then: T, else_: E) -> Vec<Element>
    where
        T: Fn() -> Vec<Element> + 'static,
        E: Fn() -> Vec<Element> + 'static;
}

impl ReactiveCond for crate::Signal<bool> {
    fn __idealyst_if<T, E>(self, then: T, else_: E) -> Vec<Element>
    where
        T: Fn() -> Vec<Element> + 'static,
        E: Fn() -> Vec<Element> + 'static,
    {
        let sig = self;
        vec![when(
            move || sig.get(),
            move || one_or_view(then()),
            move || one_or_view(else_()),
        )]
    }
}

impl ReactiveCond for crate::derive::Derived<bool> {
    fn __idealyst_if<T, E>(self, then: T, else_: E) -> Vec<Element>
    where
        T: Fn() -> Vec<Element> + 'static,
        E: Fn() -> Vec<Element> + 'static,
    {
        vec![when(
            self,
            move || one_or_view(then()),
            move || one_or_view(else_()),
        )]
    }
}

// =============================================================================
// IntoElement — coercion helper used by the `ui!` macro and direct
// when/switch callers.
// =============================================================================

/// Coercion helper: lets `when()`'s `then`/`otherwise` closures return
/// either a bare `Element` or a `Bound<H>`. `Into<Element>` is
/// already implemented for `Bound<H>`; this trait makes the implicit
/// conversion happen in argument position so users don't have to spell
/// `.into()`. Used by the `ui!` macro and by direct `when(...)` callers.
pub trait IntoElement {
    fn into_element(self) -> Element;
}

impl IntoElement for Element {
    fn into_element(self) -> Element { self }
}

impl<H> IntoElement for Bound<H> {
    fn into_element(self) -> Element { self.primitive }
}

impl<H> IntoElement for Bindable<H> {
    fn into_element(self) -> Element { self.primitive }
}

// =============================================================================
// BuildElement — component dispatch target for the `ui!`/`jsx!` macros.
// =============================================================================

/// Bridges a component's props struct to its `Element`-producing function.
///
/// `ui! { Foo(a = x) }` lowers to a plain struct literal plus a UFCS call:
///
/// ```ignore
/// ::runtime_core::BuildElement::build(
///     FooProps { a: (x).into(), ..<FooProps as ::runtime_core::BuildElement>::defaults() }
/// )
/// ```
///
/// This replaces the old per-component `macro_rules!` invocation macros.
/// Because dispatch now goes through a normal trait impl on the props
/// struct (not an exported macro), it resolves across crate boundaries by
/// ordinary path rules — no `#[macro_export]`, no `#[macro_use]` ordering
/// — and the call site is a real struct literal, so rust-analyzer gives
/// field completion, hover, and go-to-def on every prop.
///
/// `#[component]` generates the impl automatically; hand-written
/// components (e.g. idea-ui's) provide it directly. `build` absorbs the
/// `fn foo(props: &FooProps)` vs `fn foo(props: FooProps)` distinction so
/// the macro never has to know which a component uses.
///
/// `defaults` supplies the struct-update base for omitted props. The
/// provided impl forwards to `Default`, so a component with no declared
/// defaults needs only `fn build`. A `#[component(default(field = expr,
/// …))]` declaration overrides `defaults` to bake those values in (the
/// type's `Default` stays authoritative for the remaining fields).
///
/// `Default` is a supertrait because `ui!` always emits the struct-update
/// base `..Props::defaults()` — every component's props must therefore be
/// `Default`, which also makes "omit a prop to take its default" work
/// uniformly (the JSX-style ergonomics the hand-written idea-ui macros
/// already relied on).
pub trait BuildElement: Default {
    fn build(self) -> Element;

    fn defaults() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod hover_builder_tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// `.on_hover(cb)` must populate `Element::View::on_hover`, and the
    /// stored handler must forward the enter/leave bool to the callback
    /// (it's wrapped in `cycle`, which runs the closure synchronously).
    #[test]
    fn on_hover_wires_handler_and_forwards_bool() {
        let states: Rc<RefCell<Vec<bool>>> = Rc::new(RefCell::new(Vec::new()));
        let s = states.clone();
        let el: Element =
            view(Vec::new()).on_hover(move |entering| s.borrow_mut().push(entering)).into();
        let handler = match el {
            Element::View { on_hover, .. } => {
                on_hover.expect("on_hover must be Some after .on_hover()")
            }
            _ => panic!("view() must build Element::View"),
        };
        handler(true);
        handler(false);
        assert_eq!(*states.borrow(), vec![true, false]);
    }

    /// A plain `view()` carries no hover handler.
    #[test]
    fn view_without_on_hover_is_none() {
        let el: Element = view(Vec::new()).into();
        assert!(matches!(el, Element::View { on_hover: None, .. }));
    }
}

#[cfg(test)]
mod a11y_builder_tests {
    use super::*;
    use crate::accessibility::{AccessibilityProps, AccessibilityTraits, LiveRegionPriority, Role};

    /// Pull the `accessibility` field out of a node-bearing element.
    fn a11y_of(el: &Element) -> &AccessibilityProps {
        match el {
            Element::View { accessibility, .. }
            | Element::Text { accessibility, .. }
            | Element::Button { accessibility, .. } => accessibility,
            _ => panic!("unexpected element variant in a11y test"),
        }
    }

    #[test]
    fn granular_setters_write_their_field() {
        let el: Element = view(Vec::new())
            .a11y_label("Toolbar")
            .a11y_hint("Main actions")
            .a11y_role(Role::Toolbar)
            .a11y_hidden(true)
            .live_region(LiveRegionPriority::Polite)
            .into();
        let a = a11y_of(&el);
        assert_eq!(a.label.as_deref(), Some("Toolbar"));
        assert_eq!(a.hint.as_deref(), Some("Main actions"));
        assert_eq!(a.role, Some(Role::Toolbar));
        assert!(a.hidden);
        assert_eq!(a.live_region, Some(LiveRegionPriority::Polite));
    }

    #[test]
    fn a11y_traits_setter_writes_flags() {
        let el: Element = button("Save", || {})
            .a11y_traits(AccessibilityTraits::SELECTED | AccessibilityTraits::DISABLED)
            .into();
        let a = a11y_of(&el);
        assert!(a.traits.contains(AccessibilityTraits::SELECTED));
        assert!(a.traits.contains(AccessibilityTraits::DISABLED));
        assert!(!a.traits.contains(AccessibilityTraits::CHECKED));
    }

    #[test]
    fn accessibility_bag_replaces_wholesale() {
        let props = AccessibilityProps {
            label: Some("Custom".into()),
            role: Some(Role::Image),
            hidden: false,
            ..Default::default()
        };
        let el: Element = text("hi").accessibility(props).into();
        let a = a11y_of(&el);
        assert_eq!(a.label.as_deref(), Some("Custom"));
        assert_eq!(a.role, Some(Role::Image));
    }

    #[test]
    fn plain_primitive_has_default_a11y() {
        let el: Element = view(Vec::new()).into();
        assert!(a11y_of(&el).is_default());
    }

    /// The setter is generic over `Bound<H>`, so it must work uniformly
    /// across primitives, not just `view`.
    #[test]
    fn setters_are_generic_across_primitives() {
        let b: Element = button("x", || {}).a11y_label("B").into();
        let t: Element = text("y").a11y_label("T").into();
        assert_eq!(a11y_of(&b).label.as_deref(), Some("B"));
        assert_eq!(a11y_of(&t).label.as_deref(), Some("T"));
    }
}
