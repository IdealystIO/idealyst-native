//! Building a subtree a patch asked for.
//!
//! An [`Edit::SetChildren`] hands the applier a tree of [`NewNode`]s —
//! data — and expects `Element`s back. This module is the name →
//! constructor table that answers it.
//!
//! # Two tables, for two reasons
//!
//! **Primitives** are a fixed set this crate owns, so their
//! constructors are a `match` on the canonical name. Only `view` and
//! `text` are here, and that is not an oversight: they are the
//! primitives whose every constructor argument is data. `image` needs an
//! `ImageSource`, `link` a route, `text_input` a value signal AND a
//! change handler — arguments a patch cannot carry, because they are
//! code. A primitive that cannot be built from literals alone is
//! refused, and a differ that meets one asks for a rebuild.
//!
//! **Components** cannot be a `match`: this crate does not know their
//! names or their props types, and the props type is what
//! `__apply_literal` needs to resolve inherently. So a component
//! registers its own constructor — a plain `fn` pointer generated at the
//! `ui!` call site, where the type IS known — the first time the program
//! builds one.
//!
//! That registration point has a consequence worth stating plainly: a
//! component the running program has never rendered has no constructor,
//! so inserting one is refused. A component that appears anywhere in the
//! current tree can be inserted anywhere else. The alternative — a
//! link-time registry via `inventory` — would cover the unrendered case
//! at the cost of a submission per component in every build with the
//! feature on, for a case (insert a component that is nowhere on screen)
//! the dev loop rarely needs. Revisit if that stops being true.
//!
//! [`Edit::SetChildren`]: runtime_template::Edit::SetChildren

use std::cell::RefCell;
use std::collections::HashMap;

use runtime_scene::Element;
use runtime_template::{LiteralValue, NewNode, PropValue};

/// Build a component from literal props and children.
///
/// Returns `None` when the component cannot take the children it was
/// given (its props struct has no `children` field). Generated at the
/// call site, so `__apply_literal` and `__apply_children` resolve as
/// INHERENT methods on the concrete props type — a generic function
/// here would bind the blanket-trait fallbacks instead and silently
/// apply nothing.
pub type ComponentCtor = fn(&[(&str, &LiteralValue)], Vec<Element>) -> Option<Element>;

thread_local! {
    static CTORS: RefCell<HashMap<&'static str, ComponentCtor>> =
        RefCell::new(HashMap::new());
}

/// Record how to build `name`. Called by the emission at every
/// `#[component]` call site under `ui-overlay`; idempotent and cheap.
pub fn register_ctor(name: &'static str, ctor: ComponentCtor) {
    CTORS.with(|c| {
        c.borrow_mut().entry(name).or_insert(ctor);
    });
}

/// How many component constructors are known. For tests and a dev-tools
/// readout.
pub fn ctor_count() -> usize {
    CTORS.with(|c| c.borrow().len())
}

pub(crate) fn clear_ctors() {
    CTORS.with(|c| c.borrow_mut().clear());
}

/// Build one `NewNode`, or `None` if nothing here knows how.
pub(crate) fn build(node: &NewNode) -> Option<Element> {
    let mut children = Vec::with_capacity(node.children.len());
    for child in node.children.iter() {
        children.push(build(child)?);
    }
    match node.kind.as_ref() {
        "view" => Some(finish(crate::glue::view(children), node)),
        "text" => {
            // `text` takes its content as a constructor argument, so it
            // is read from the props before building rather than set
            // after. Children of a `text` are its content in author
            // syntax; a NewNode spells that as the `content` prop, which
            // is the form the descriptor records either way.
            let content = literal_str(node, "content").unwrap_or_default();
            if !children.is_empty() {
                return None;
            }
            Some(finish(crate::glue::text(content), node))
        }
        other => {
            let ctor = CTORS.with(|c| c.borrow().get(other).copied())?;
            let props: Vec<(&str, &LiteralValue)> = node
                .props
                .iter()
                .filter_map(|p| match &p.value {
                    PropValue::Lit(v) => Some((p.name.as_ref(), v)),
                    PropValue::Slot(_) => None,
                })
                .collect();
            ctor(&props, children)
        }
    }
}

/// Coerce a freshly built primitive and write its literal props onto it,
/// reusing the same table the applier uses for an existing node — so a
/// prop that can be CHANGED can also be SET on a new node, by
/// construction rather than by two lists kept in step.
fn finish(builder: impl crate::glue::IntoElement, node: &NewNode) -> Element {
    let mut element = crate::glue::IntoElement::into_element(builder);
    if let Element::Item { data, .. } = &element {
        for prop in node.props.iter() {
            if let PropValue::Lit(value) = &prop.value {
                super::prims::set_literal(&**data, prop.name.as_ref(), value);
            }
        }
    }
    element
}

fn literal_str(node: &NewNode, name: &str) -> Option<String> {
    node.props.iter().find(|p| p.name == name).and_then(|p| match &p.value {
        PropValue::Lit(LiteralValue::Str(s)) => Some(s.to_string()),
        _ => None,
    })
}
