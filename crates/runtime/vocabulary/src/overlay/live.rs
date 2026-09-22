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

use runtime_scene::{realize, LiveOrigin, Realized, Registry};
use runtime_template::{Edit, LiteralValue, NewNode, Patch};

// The individual `*Ops` traits come in through `AllCaps`, which is
// the bound this module is generic over.
use crate::caps::AllCaps;
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

/// Apply `patch` to every mounted instance of its site.
pub fn apply_live<H: AllCaps + 'static>(
    backend: &Rc<RefCell<H>>,
    registry: &Rc<Registry<H>>,
    patch: &Patch,
) -> Outcome {
    apply_live_to(backend, registry, patch.key(), &patch.edits)
}

/// As [`apply_live`], addressed by the key a tag carries.
///
/// The primitive of the pair, for the same reason `stage_key` is: a
/// caller that read the key off a mounted tree has the number, and a
/// `SiteId` cannot be hashed backwards into one.
///
/// Takes no root. Instances come from `runtime_scene::live`, which
/// registers every node `mount_item` builds — so a subtree a handler
/// realized into its OWN storage (every navigator screen) is reachable,
/// and a subtree that has unmounted is not.
pub fn apply_live_to<H: AllCaps + 'static>(
    backend: &Rc<RefCell<H>>,
    registry: &Rc<Registry<H>>,
    site: u64,
    edits: &[Edit],
) -> Outcome {

    let mut outcome = Outcome::default();
    // A component instance takes ALL of its `SetProp`s at once: a
    // rebuild runs the component body, and running it once per changed
    // prop would both waste the work and leave the earlier props
    // reverted, since each rebuild starts from the remembered copy.
    let props_for_node = |index: u32| -> Vec<(String, LiteralValue)> {
        edits
            .iter()
            .filter_map(|e| match e {
                Edit::SetProp { node, name, value } if *node == index => {
                    Some((name.to_string(), value.clone()))
                }
                _ => None,
            })
            .collect()
    };
    let mut rebuilt: Vec<u32> = Vec::new();

    for edit in edits.iter() {
        let node_index = match edit {
            Edit::SetProp { node, .. } | Edit::SetChildren { node, .. } => *node,
        };
        // An edit addressed to a node that is not currently mounted (an
        // `if` branch that is false right now) is not a refusal: the
        // `Element` path has it, and it will be there when the branch
        // flips. Counting it as refused would tell a dev server to warn
        // about something that is going to work.
        if matches!(edit, Edit::SetProp { .. }) && rebuilt.contains(&node_index) {
            continue;
        }
        for (slot, instance) in
            runtime_scene::instances::<H::Node>(site, node_index).into_iter().enumerate()
        {
            let ok = match edit {
                Edit::SetProp { name, value, .. } => {
                    // A primitive has a setter on the seam. A component
                    // does not — its props were consumed and its body
                    // already ran — so the only way to show the new
                    // value is to run it again from the copy the call
                    // site kept, and swap the subtree.
                    if instance.rebuild.is_some() {
                        let props = props_for_node(node_index);
                        let ok = rebuild_instance(
                            backend,
                            registry,
                            (site, node_index, slot),
                            &instance,
                            &props,
                        );
                        if ok {
                            rebuilt.push(node_index);
                        }
                        ok
                    } else {
                        set_prop_live(backend, &instance, name, value)
                    }
                }
                Edit::SetChildren { children, .. } => set_children_live(
                    backend,
                    registry,
                    (site, node_index, slot),
                    &instance,
                    children,
                ),
            };
            if ok {
                outcome.applied += 1;
            } else {
                outcome.refused += 1;
            }
        }
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
    instance: &LiveOrigin<H::Node>,
    name: &str,
    value: &LiteralValue,
) -> bool {
    let (type_id, node) = (instance.type_id, &instance.node);

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
/// Realize first, then detach, then attach: a failure at any depth
/// leaves the tree exactly as it was rather than half-replaced.
///
/// Works from the registry's recorded child HANDLES rather than from a
/// live tree, because the registry is handle-based. The differ has
/// already guaranteed every old child is fully static, so nothing that
/// outlives them is bound to them.
fn set_children_live<H: AllCaps + 'static>(
    backend: &Rc<RefCell<H>>,
    registry: &Rc<Registry<H>>,
    key: (u64, u32, usize),
    instance: &LiveOrigin<H::Node>,
    children: &[NewNode],
) -> bool {
    let mut built: Vec<(Realized<H::Node>, Vec<H::Node>)> = Vec::with_capacity(children.len());
    for new in children {
        let Some(element) = super::construct::build(new) else {
            return false;
        };
        let realized = realize(backend, registry, element);
        let nodes = realized.collect_nodes();
        built.push((realized, nodes));
    }

    let parent = instance.node.clone();
    let old = instance.children.borrow().clone();
    {
        let mut b = backend.borrow_mut();
        // Detach each old child individually and tell the seam it is
        // going away, rather than `clear_children`: `release_subtree` is
        // the hook a backend uses to drop whatever it hung off those
        // nodes.
        for n in &old {
            b.release_subtree(n);
            b.remove_child(&parent, n);
        }
        for (_, nodes) in &built {
            for n in nodes {
                let mut p = parent.clone();
                b.insert(&mut p, n.clone());
            }
        }
    }

    // Same reason as a rebuild: these were realized on their own, so
    // nothing recorded where they sit.
    {
        let mut at = 0usize;
        for (realized, nodes) in &built {
            if let Some(origin) = runtime_scene::root_origin(&realized.root) {
                *origin.parent.borrow_mut() = Some((parent.clone(), at));
            }
            at += nodes.len();
        }
    }

    *instance.children.borrow_mut() =
        built.iter().flat_map(|(_, nodes)| nodes.iter().cloned()).collect();
    // The new subtrees have no enclosing region to hold them, and a
    // `Realized` IS its scope. Held per instance, so a later patch to a
    // DIFFERENT node cannot unmount these.
    let scopes: Vec<Realized<H::Node>> = built.into_iter().map(|(r, _)| r).collect();
    super::rebuild::hold(key, Box::new(scopes));
    true
}

/// Run a component again with new literal props and swap its subtree in
/// place.
///
/// Realize first, then detach, then attach — a failure anywhere leaves
/// the old subtree mounted rather than the screen half-empty.
///
/// The old instance is RETIRED rather than left registered: its node is
/// gone from the backend, and a second patch that found it would issue
/// calls against a node nothing is showing. The new subtree registers
/// itself on the way through `mount_item`, so the next patch finds it
/// instead.
fn rebuild_instance<H: AllCaps + 'static>(
    backend: &Rc<RefCell<H>>,
    registry: &Rc<Registry<H>>,
    key: (u64, u32, usize),
    instance: &LiveOrigin<H::Node>,
    props: &[(String, LiteralValue)],
) -> bool {
    let Some(erased) = instance.rebuild.as_ref() else { return false };
    let Some(rebuilder) = super::rebuild::recover(erased) else { return false };
    // A node whose parent is unknown cannot be swapped: a root, or one a
    // handler placed itself. Refusing is right — the alternative is
    // realizing a subtree nothing is holding.
    let Some((parent, index)) = instance.parent.borrow().clone() else { return false };

    // The rebuilt element comes out of the component's own body, so it
    // carries the tags of the component's INTERNAL `ui!` — not the
    // caller's tag for this instance, which `exit` applied at the call
    // site and which is not running now. Re-apply both, or the
    // replacement is unreachable and the NEXT patch silently finds
    // nothing.
    let element = rebuilder(props);
    let element = runtime_scene::with_tag(element, instance.tag);
    let element = runtime_scene::with_rebuild(element, Rc::clone(erased));
    let realized = realize(backend, registry, element);
    let fresh = realized.collect_nodes();

    {
        let mut b = backend.borrow_mut();
        b.release_subtree(&instance.node);
        b.remove_child(&parent, &instance.node);
        let mut at = index;
        for node in &fresh {
            let mut p = parent.clone();
            b.insert_at(&mut p, node.clone(), at);
            at += 1;
        }
    }

    // The replacement was realized on its own, so no parent's
    // `mount_item` recorded where it sits. Say so here, or the NEXT
    // patch to this instance finds it and cannot swap it.
    if let Some(origin) = runtime_scene::root_origin(&realized.root) {
        *origin.parent.borrow_mut() = Some((parent, index));
    }

    instance.retired.set(true);
    super::rebuild::hold(key, Box::new(realized));
    true
}
