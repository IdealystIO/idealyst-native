//! Framework core: primitives, Backend trait, render walker, reactivity.

mod reactive;
pub use reactive::{untrack, Effect, Signal};

pub use framework_macros::component;

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

pub struct Owner {
    _effects: Vec<Effect>,
}

#[must_use = "drop the Owner to dispose the UI; keep it alive to keep the UI reactive"]
pub fn render<B: Backend + 'static>(backend: Rc<RefCell<B>>, tree: Primitive) -> Owner {
    let mut effects = Vec::new();
    let root = build(&backend, tree, &mut effects);
    backend.borrow_mut().finish(root);
    Owner { _effects: effects }
}

fn build<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    node: Primitive,
    effects: &mut Vec<Effect>,
) -> B::Node {
    match node {
        Primitive::Text(TextSource::Static(content)) => {
            backend.borrow_mut().create_text(&content)
        }
        Primitive::Text(TextSource::Reactive(compute)) => {
            let node = backend.borrow_mut().create_text("");
            let node_for_effect = node.clone();
            let backend = backend.clone();
            let effect = Effect::new(move || {
                let value = compute();
                backend.borrow_mut().update_text(&node_for_effect, &value);
            });
            effects.push(effect);
            node
        }
        Primitive::View { children } => {
            let mut parent = backend.borrow_mut().create_view();
            for child in children {
                let child_node = build(backend, child, effects);
                backend.borrow_mut().insert(&mut parent, child_node);
            }
            parent
        }
        Primitive::Button { label, on_click } => {
            backend.borrow_mut().create_button(&label, on_click)
        }
        Primitive::When { cond, then, otherwise } => {
            // Create a placeholder container. Each time the condition
            // changes, we clear its children, rebuild the active branch,
            // and insert the new subtree.
            //
            // The inner subtree's Effects are held in `inner_effects`, a
            // shared RefCell. Each rebuild clears the previous vec
            // (dropping those effects, releasing their signal subscribers)
            // before populating with new ones.
            let placeholder = backend.borrow_mut().create_view();
            let inner_effects: Rc<RefCell<Vec<Effect>>> =
                Rc::new(RefCell::new(Vec::new()));

            let backend_for_effect = backend.clone();
            let placeholder_for_effect = placeholder.clone();
            let inner_for_effect = inner_effects.clone();

            let outer = Effect::new(move || {
                // Subscribing happens here: cond() reads signals while
                // CURRENT effect is set, so we re-fire on changes.
                let active = cond();

                // From here we build the active branch. Wrap that work in
                // `untrack` so any signals read while constructing children
                // (e.g. reactive text initial values, branch closures' own
                // reads) don't subscribe THIS effect. Inner effects spawned
                // during the build will subscribe themselves as normal.
                untrack(|| {
                    let active_branch = if active { then() } else { otherwise() };

                    // Drop the previous subtree's effects, then clear the
                    // placeholder's children. Order matters: drop effects
                    // first so any tasks they own release before the
                    // backend nodes vanish.
                    inner_for_effect.borrow_mut().clear();
                    backend_for_effect
                        .borrow_mut()
                        .clear_children(&placeholder_for_effect);

                    let mut fresh: Vec<Effect> = Vec::new();
                    let child_node =
                        build(&backend_for_effect, active_branch, &mut fresh);
                    let mut placeholder_mut = placeholder_for_effect.clone();
                    backend_for_effect
                        .borrow_mut()
                        .insert(&mut placeholder_mut, child_node);
                    *inner_for_effect.borrow_mut() = fresh;
                });
            });

            effects.push(outer);
            placeholder
        }
    }
}
