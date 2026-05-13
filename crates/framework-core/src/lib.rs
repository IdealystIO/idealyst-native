//! Framework core: primitives, Backend trait, render walker, reactivity.

mod reactive;
mod style;

pub use reactive::{untrack, Effect, Signal};
pub use style::{
    install_theme, pregenerate_for_theme, resolve as resolve_style, set_theme, Border, Color,
    Length, StyleApplication, StyleRules, StyleSheet, VariantAxis, VariantSet, VariantValue,
};

pub use framework_macros::{component, ui};

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
    },
    Text {
        source: TextSource,
        style: Option<StyleSource>,
    },
    Button {
        label: String,
        on_click: Rc<dyn Fn()>,
        style: Option<StyleSource>,
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

/// Allows `with_style(...)` to accept either a fixed `StyleApplication`
/// or a closure that produces one. Closures enable reactive styling —
/// any signal the closure reads becomes a dependency, and changes
/// re-fire the apply-style effect.
pub trait IntoStyleSource {
    fn into_style_source(self) -> StyleSource;
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

pub fn view(children: Vec<Primitive>) -> Primitive {
    Primitive::View { children, style: None }
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

impl ChildList for Option<Primitive> {
    fn append_to(self, out: &mut Vec<Primitive>) {
        if let Some(p) = self {
            out.push(p);
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

pub fn text<T: IntoTextSource>(source: T) -> Primitive {
    Primitive::Text { source: source.into_text_source(), style: None }
}

pub fn button<F: Fn() + 'static>(label: impl Into<String>, on_click: F) -> Primitive {
    Primitive::Button {
        label: label.into(),
        on_click: Rc::new(on_click),
        style: None,
    }
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
    fn finish(&mut self, root: Self::Node);
}

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
        Primitive::Text { source, style } => {
            let n = build_text(backend, source);
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            n
        }
        Primitive::View { children, style } => {
            let n = build_view(backend, children);
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            n
        }
        Primitive::Button { label, on_click, style } => {
            let n = backend.borrow_mut().create_button(&label, on_click);
            if let Some(s) = style {
                attach_style(backend, &n, s);
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
