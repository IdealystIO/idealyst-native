//! Author-facing DSL: `Bound<H>`, `Bindable<H>`, `ChildList`, the
//! primitive constructors (`view`, `text`, `button`, `when`,
//! `switch`), and `IntoPrimitive` / `IntoDisabledSource`.
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

use crate::handles::{ButtonHandle, PressableHandle, RefFill, TextHandle, ViewHandle};
use crate::primitive::Primitive;
use crate::reactive::Ref;
use crate::sources::{IntoStyleSource, IntoTextSource};
use std::cell::RefCell;
use std::rc::Rc;

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
    /// Wrap a `Primitive` in a typed `Bound<H>`. The handle marker `H`
    /// is purely a type-check hook for `.bind(r: Ref<H>)`; it doesn't
    /// affect the wrapped primitive.
    ///
    /// First-party primitives use their dedicated builder functions
    /// (`view(...)`, `button(...)`, …) which call this internally.
    /// Third-party SDK crates that want a typed handle (e.g.
    /// `Bound<WebViewHandle>` for `webview::WebView(...)`) build a
    /// `Primitive::External` and wrap it here.
    pub fn new(primitive: Primitive) -> Self {
        Self { primitive, _handle: std::marker::PhantomData }
    }

    /// Mutable access to the wrapped `Primitive`. Public so the
    /// `ui!` macro's structured-emission paths can fill in
    /// per-primitive fields that the closure-shape builders don't
    /// expose — e.g. patching `Virtualizer.row_template` and
    /// `row_index_signal_id` after going through
    /// `primitives::virtualizer::virtualizer(...)`.
    #[doc(hidden)]
    pub fn primitive_mut(&mut self) -> &mut Primitive {
        &mut self.primitive
    }

    /// Attaches a style. Same semantics as `Primitive::with_style`.
    pub fn with_style<S: IntoStyleSource>(mut self, style: S) -> Self {
        self.primitive = self.primitive.with_style(style);
        self
    }

    /// Assigns a test ID for robot/automation queries.
    /// Only available when the `robot` feature is enabled.
    #[cfg(feature = "robot")]
    pub fn test_id(mut self, id: &'static str) -> Self {
        self.primitive = self.primitive.with_test_id(id);
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

    /// Set a leading icon (rendered before the label).
    pub fn leading_icon(mut self, icon: crate::IconData) -> Self {
        if let Primitive::Button { leading_icon, .. } = &mut self.primitive {
            *leading_icon = Some(icon);
        }
        self
    }

    /// Set a trailing icon (rendered after the label).
    pub fn trailing_icon(mut self, icon: crate::IconData) -> Self {
        if let Primitive::Button { trailing_icon, .. } = &mut self.primitive {
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
        if let Primitive::View { ref_fill, .. } = &mut self.primitive {
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
        if let Primitive::View { safe_area_sides, .. } = &mut self.primitive {
            *safe_area_sides |= sides;
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
        if let Primitive::View { on_touch, .. } = &mut self.primitive {
            *on_touch = Some(std::rc::Rc::new(handler));
        }
        self
    }
}

impl Bound<PressableHandle> {
    /// Same shape as [`Bound::<ButtonHandle>::bind`] — the framework
    /// fills the ref with a `PressableHandle` constructed from the
    /// just-mounted backend node.
    pub fn bind(mut self, r: Ref<PressableHandle>) -> Self {
        if let Primitive::Pressable { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::Pressable(Box::new(move |h| r.fill(h))));
        }
        self
    }

    /// Reactively disable the pressable. See
    /// [`Bound::<ButtonHandle>::disabled`] for semantics — same
    /// state-bit + native-inert behavior.
    pub fn disabled<D: IntoDisabledSource>(mut self, disabled: D) -> Self {
        if let Primitive::Pressable { disabled: slot, .. } = &mut self.primitive {
            *slot = Some(disabled.into_disabled_source());
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

// =============================================================================
// ChildList trait + impls
// =============================================================================

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

// =============================================================================
// Primitive constructors: view, text, button, when, switch
// =============================================================================

pub fn view(children: Vec<Primitive>) -> Bound<ViewHandle> {
    Bound::new(Primitive::View {
        children,
        style: None,
        ref_fill: None,
        safe_area_sides: crate::SafeAreaSides::NONE,
        on_touch: None,
        #[cfg(feature = "robot")]
        test_id: None,
    })
}

pub fn text<T: IntoTextSource>(source: T) -> Bound<TextHandle> {
    Bound::new(Primitive::Text {
        source: source.into_text_source(),
        style: None,
        ref_fill: None,
        #[cfg(feature = "robot")]
        test_id: None,
    })
}

pub fn button<L, A>(label: L, on_click: A) -> Bound<ButtonHandle>
where
    L: IntoTextSource,
    A: crate::derive::IntoAction,
{
    Bound::new(Primitive::Button {
        label: label.into_text_source(),
        on_click: on_click.into_action(),
        leading_icon: None,
        trailing_icon: None,
        style: None,
        ref_fill: None,
        disabled: None,
        #[cfg(feature = "robot")]
        test_id: None,
    })
}

/// Clickable container — like [`view`] but with a press callback.
/// See [`Primitive::Pressable`] for why a separate primitive exists
/// rather than `View` gaining an optional `on_click`.
pub fn pressable<F: Fn() + 'static>(
    children: Vec<Primitive>,
    on_click: F,
) -> Bound<PressableHandle> {
    Bound::new(Primitive::Pressable {
        children,
        on_click: Rc::new(on_click),
        style: None,
        ref_fill: None,
        disabled: None,
        #[cfg(feature = "robot")]
        test_id: None,
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
    C: crate::derive::IntoDerived<bool>,
    T: Fn() -> Primitive + 'static,
    O: Fn() -> Primitive + 'static,
{
    Primitive::When {
        cond: cond.into_derived(),
        then: Box::new(then),
        otherwise: Box::new(otherwise),
        style: None,
    }
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
pub fn switch<S, F, B>(scrutinee: F, branches: B) -> Primitive
where
    S: PartialEq + 'static,
    F: Fn() -> S + 'static,
    B: Fn(&S) -> Primitive + 'static,
{
    use std::rc::Rc;

    // Closure-driven path. We don't ship the scrutinee value over
    // any wire — the discriminant `compute` is *opaque* (returns
    // `Null` after re-running the scrutinee for signal-subscription
    // purposes) and the `default` closure does the real arm
    // dispatch using the typed scrutinee directly. This keeps the
    // closure-driven API constraint-free (only `PartialEq` is
    // required, same as before the refactor) while still routing
    // through the structured `Primitive::Switch` shape that
    // generator backends consume.
    //
    // Authors who need generator-backend-compatible reactivity
    // (Roku) should construct the primitive through the structured
    // entry point (a `#[method]`-backed discriminant + literal-key
    // arms) — `Primitive::Switch` with a non-opaque `discriminant`
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
    let dispatch: Box<dyn Fn() -> Primitive> = Box::new(move || {
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
    Primitive::Switch {
        discriminant,
        arms: Vec::new(),
        default: dispatch,
        style: None,
    }
}

// =============================================================================
// IntoPrimitive — coercion helper used by the `ui!` macro and direct
// when/switch callers.
// =============================================================================

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
