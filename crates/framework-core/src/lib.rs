//! Framework core: primitives, Backend trait, render walker, reactivity.

mod reactive;
pub use reactive::{untrack, Effect, Signal};

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

pub enum Primitive {
    View {
        children: Vec<Primitive>,
    },
    Text(TextSource),
    Button {
        label: String,
        on_click: Rc<dyn Fn()>,
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
    },
}

pub fn view(children: Vec<Primitive>) -> Primitive {
    Primitive::View { children }
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
    Primitive::Text(source.into_text_source())
}

pub fn button<F: Fn() + 'static>(label: impl Into<String>, on_click: F) -> Primitive {
    Primitive::Button {
        label: label.into(),
        on_click: Rc::new(on_click),
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
        Primitive::Text(TextSource::Static(content)) => {
            backend.borrow_mut().create_text(&content)
        }
        Primitive::Text(TextSource::Reactive(compute)) => build_reactive_text(backend, compute),
        Primitive::View { children } => build_view(backend, children),
        Primitive::Button { label, on_click } => {
            backend.borrow_mut().create_button(&label, on_click)
        }
        Primitive::When { cond, then, otherwise } => build_when(backend, cond, then, otherwise),
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
