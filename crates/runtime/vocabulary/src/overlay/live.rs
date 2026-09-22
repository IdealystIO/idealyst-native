//! Applying a patch to instances that are ALREADY mounted.
//!
//! A patch has two application paths and both are needed:
//!
//! - the `Element` path (`super::tag`) changes what the NEXT build of a
//!   site produces, so the edit survives every state-driven rebuild;
//! - this one reaches what is on screen right now, so a saved edit shows
//!   up without waiting for something to re-render.
//!
//! Neither subsumes the other. Without the first, a patch evaporates the
//! moment a signal fires. Without the second, nothing visible happens
//! until it does.
//!
//! # Host-agnostic by construction
//!
//! Everything here is generic over `H: AllCaps` and issues ordinary
//! capability calls — `update_text`, `update_button_label`,
//! `set_disabled`, `insert_at`, `remove_child`. There is no backend in
//! this file and no `cfg(target)` anywhere in it: web, iOS, macOS,
//! Android and the recording mock all get the live path through the same
//! replay, which is the framework's standing rule about where platform
//! differences are allowed to live.
//!
//! # What applies live, and what waits for the next build
//!
//! A live edit needs a SETTER on the seam. Text content, a button's
//! label and disabled state, and a text input's placeholder and secure
//! flag have one. A `#[component]`'s prop does not — the component's body
//! already ran, and re-running it would need every dynamic prop it was
//! given, which is compiled code the patch does not carry. Those apply on
//! the next build of the site, which the `Element` path guarantees.
//!
//! That split is reported, not hidden: [`Outcome::refused`] counts what
//! the live pass could not do, so a dev server can say "showing on next
//! render" instead of leaving the author wondering.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_scene::{realize, visit_tagged, Element, Host, LiveNode, Realized, Registry};
use runtime_template::{Edit, LiteralValue, NewNode, Patch};

use crate::caps::{AllCaps, ButtonOps, StyleOps, TextInputOps, TextOps};
use crate::prims::{ButtonPrim, PrimCell, TextInputPrim, TextPrim};

use super::apply::Outcome;

thread_local! {
    /// Reactive scopes owned by subtrees a patch inserted.
    ///
    /// A realized subtree's `Realized` IS its scope: dropping it unmounts.
    /// A patched-in subtree has no enclosing region to hold it, so it is
    /// held here, keyed by site, and dropped when that site's patch is
    /// replaced or cleared. The subtrees are static by construction
    /// (`NewNode` carries literals only), so the scope is normally
    /// empty — keeping it is about correctness under a future widening,
    /// not about anything these scopes hold today.
    static INSERTED: RefCell<Vec<(u64, Box<dyn std::any::Any>)>> = const {
        RefCell::new(Vec::new())
    };
}

/// Drop the scopes of every subtree a previous patch inserted for `site`.
pub fn release_inserted(site: u64) {
    INSERTED.with(|i| i.borrow_mut().retain(|(s, _)| *s != site));
}

/// Drop every inserted scope. Test support; see [`super::reset`].
pub(crate) fn release_all() {
    INSERTED.with(|i| i.borrow_mut().clear());
}

/// Apply `patch` to the mounted tree under `root`.
///
/// `root` is a live tree the caller owns — an app's root `Realized`, or a
/// subtree in a test. Every instance the patch addresses is visited: one
/// `(site, node)` pair matches several live nodes when the node is inside
/// a `for`, and all of them are updated.
pub fn apply_live<H: AllCaps + 'static>(
    backend: &Rc<RefCell<H>>,
    registry: &Rc<Registry<H>>,
    root: &mut LiveNode<H::Node>,
    patch: &Patch,
) -> Outcome {
    apply_live_to(backend, registry, root, patch.key(), &patch.edits)
}

/// As [`apply_live`], addressed by the key a tag carries.
///
/// The primitive of the pair, for the same reason `stage_key` is: a
/// caller that read the key off a mounted tree has the number, and a
/// `SiteId` cannot be hashed backwards into one.
pub fn apply_live_to<H: AllCaps + 'static>(
    backend: &Rc<RefCell<H>>,
    registry: &Rc<Registry<H>>,
    root: &mut LiveNode<H::Node>,
    site: u64,
    edits: &[Edit],
) -> Outcome {
    release_inserted(site);

    let mut outcome = Outcome::default();
    for edit in edits.iter() {
        let mut matched = false;
        visit_tagged(root, site, &mut |tag, type_id, live: &mut LiveNode<H::Node>| {
            let node_index = match edit {
                Edit::SetProp { node, .. } | Edit::SetChildren { node, .. } => *node,
            };
            if tag.node != node_index {
                return;
            }
            matched = true;
            let ok = match edit {
                Edit::SetProp { name, value, .. } => {
                    set_prop_live(backend, type_id, live, name, value)
                }
                Edit::SetChildren { children, .. } => {
                    set_children_live(backend, registry, site, live, children)
                }
            };
            if ok {
                outcome.applied += 1;
            } else {
                outcome.refused += 1;
            }
        });
        // An edit addressed to a node that is not currently mounted (an
        // `if` branch that is false right now) is not a refusal: the
        // `Element` path has it, and it will be there when the branch
        // flips. Counting it as refused would tell a dev server to warn
        // about something that is going to work.
        let _ = matched;
    }
    outcome
}

/// Write one literal through the backend seam.
///
/// Dispatch is on the payload `TypeId` the live tree recorded, because
/// the live tree has no payload — only a node handle. That is also why
/// this list is shorter than the `Element` applier's: a prop is here only
/// when the seam has a setter for it.
fn set_prop_live<H: AllCaps + 'static>(
    backend: &Rc<RefCell<H>>,
    type_id: std::any::TypeId,
    live: &LiveNode<H::Node>,
    name: &str,
    value: &LiteralValue,
) -> bool {
    let LiveNode::Item { node, .. } = live else { return false };

    if type_id == std::any::TypeId::of::<PrimCell<TextPrim>>() {
        return match (name, value) {
            ("content", LiteralValue::Str(s)) => {
                backend.borrow_mut().update_text(node, s);
                true
            }
            _ => false,
        };
    }
    if type_id == std::any::TypeId::of::<PrimCell<ButtonPrim>>() {
        return match (name, value) {
            ("label", LiteralValue::Str(s)) => {
                backend.borrow_mut().update_button_label(node, s);
                true
            }
            ("disabled", LiteralValue::Bool(b)) => {
                backend.borrow_mut().set_disabled(node, *b);
                true
            }
            _ => false,
        };
    }
    if type_id == std::any::TypeId::of::<PrimCell<TextInputPrim>>() {
        return match (name, value) {
            ("placeholder", LiteralValue::Str(s)) => {
                backend.borrow_mut().update_text_input_placeholder(node, Some(s));
                true
            }
            ("secure", LiteralValue::Bool(b)) => {
                backend.borrow_mut().update_text_input_secure(node, *b);
                true
            }
            _ => false,
        };
    }
    false
}

/// Replace a mounted node's children with freshly realized subtrees.
///
/// Realize first, then detach, then attach: a failure at any depth leaves
/// the tree exactly as it was rather than half-replaced.
fn set_children_live<H: AllCaps + 'static>(
    backend: &Rc<RefCell<H>>,
    registry: &Rc<Registry<H>>,
    site: u64,
    live: &mut LiveNode<H::Node>,
    children: &[NewNode],
) -> bool {
    let LiveNode::Item { node, children: old, .. } = live else { return false };
    // The same guard the `Element` applier makes: a child list holding a
    // region is one whose contents are decided at runtime, and replacing
    // it would delete live code.
    if !old.iter().all(|c| matches!(c, LiveNode::Item { .. })) {
        return false;
    }

    let mut built: Vec<(Realized<H::Node>, Vec<H::Node>)> = Vec::with_capacity(children.len());
    for new in children {
        let Some(element) = super::construct::build(new) else {
            return false;
        };
        let realized = realize_subtree(backend, registry, element);
        let nodes = realized.collect_nodes();
        built.push((realized, nodes));
    }

    let parent = node.clone();
    {
        let mut b = backend.borrow_mut();
        // Detach each old child individually and tell the seam it is
        // going away, rather than `clear_children`: `release_subtree` is
        // the hook a backend uses to drop whatever it hung off those
        // nodes. The differ has already guaranteed every one of them is
        // fully static, so nothing that outlives them is bound to them.
        for child in old.iter() {
            for n in child.collect_nodes() {
                b.release_subtree(&n);
                b.remove_child(&parent, &n);
            }
        }
        for (_, nodes) in &built {
            for n in nodes {
                let mut p = parent.clone();
                b.insert(&mut p, n.clone());
            }
        }
    }

    // The old children's scopes go with the old `LiveNode`s; the new
    // ones are held for as long as this site's patch stands.
    *old = Vec::new();
    INSERTED.with(|i| {
        for (realized, _) in built {
            i.borrow_mut().push((site, Box::new(realized)));
        }
    });
    true
}

/// Realize a patch-built subtree outside any enclosing region.
fn realize_subtree<H: Host + 'static>(
    backend: &Rc<RefCell<H>>,
    registry: &Rc<Registry<H>>,
    element: Element,
) -> Realized<H::Node> {
    realize(backend, registry, element)
}
