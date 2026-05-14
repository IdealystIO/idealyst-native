//! Framework core: primitives, Backend trait, render walker, reactivity.

mod reactive;
mod style;

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
    primitive: Primitive,
    _handle: std::marker::PhantomData<H>,
}

impl<H> Bound<H> {
    fn new(primitive: Primitive) -> Self {
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

pub trait Backend {
    type Node: Clone;

    fn create_view(&mut self) -> Self::Node;
    fn create_text(&mut self, content: &str) -> Self::Node;
    fn create_button(&mut self, label: &str, on_click: Rc<dyn Fn()>) -> Self::Node;
    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node);
    fn update_text(&mut self, node: &Self::Node, content: &str);
    /// Remove every child from `node`. Used by reactive conditionals when
    /// the active branch flips and the old subtree needs to be unmounted.
    fn clear_children(&mut self, node: &Self::Node);
    /// Apply a resolved style to a node. The framework has already run
    /// the stylesheet's closure against the active theme; the backend
    /// receives concrete `StyleRules` with literal values.
    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>);

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

    fn finish(&mut self, root: Self::Node);
}

// Default ZST `Ops` impls used by backends that haven't opted into ref
// support yet (or by the `()` Node used in tests).

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
        Primitive::Button { label, on_click, style, ref_fill } => {
            let n = backend.borrow_mut().create_button(&label, on_click);
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            if let Some(RefFill::Button(fill)) = ref_fill {
                let handle = backend.borrow().make_button_handle(&n);
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
) {
    let node_for_effect = node.clone();
    let backend_for_effect = backend.clone();

    let handle = StyleHandle {
        backend: backend.clone(),
        node: node.clone(),
    };

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

        let resolved = resolve_style(&app);
        backend_for_effect
            .borrow_mut()
            .apply_style(&node_for_effect, &resolved);
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
