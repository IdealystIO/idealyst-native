//! Applying one node's edits to the `Element` it just built.
//!
//! Called from [`super::tag`], which is the point every `ui!` node
//! passes through — so a patch is re-applied on every rebuild of the
//! site, automatically. That matters more than it looks: a site's
//! `Element` is rebuilt whenever its reactive scope re-runs (a branch
//! flips, a keyed row re-renders, a component re-renders), and a patch
//! applied once to a tree that is then rebuilt from the compiled code
//! would silently revert.
//!
//! # What is refused, and why refusing is the feature
//!
//! [`Outcome`] counts what applied and what did not. Nothing here
//! panics and nothing is applied partially-by-guess: an edit either
//! lands or is reported. A dev server reading the counts can tell the
//! author "that change needs a rebuild" instead of showing them a tree
//! that is half the old version.
//!
//! The refusals that matter:
//!
//! - **a prop that is code** — reactive content, a style, anything the
//!   descriptor recorded as a path. See [`super::prims`].
//! - **children that are not plain nodes.** `SetChildren` replaces a
//!   child list wholesale, and a list containing a reactive region
//!   (`Dyn`), a keyed list, a fragment or a component boundary
//!   (`Owned`) is one whose contents are decided at runtime. Replacing
//!   it would delete live code. The applier checks the LIVE children
//!   rather than trusting the differ, because this is the guarantee
//!   that must hold even if a patch arrives from somewhere else.
//! - **a subtree nothing knows how to build** — see
//!   [`super::construct`].

use runtime_scene::Element;
use runtime_template::Edit;

/// What an apply did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Edits that took effect.
    pub applied: usize,
    /// Edits this program cannot honour without a rebuild.
    pub refused: usize,
}

impl Outcome {
    pub fn is_complete(&self) -> bool {
        self.refused == 0
    }

    fn record(&mut self, ok: bool) {
        if ok {
            self.applied += 1;
        } else {
            self.refused += 1;
        }
    }
}

/// Apply every edit addressed to this node.
pub(crate) fn apply(element: &mut Element, edits: &[Edit]) -> Outcome {
    let mut outcome = Outcome::default();
    for edit in edits {
        match edit {
            Edit::SetProp { name, value, .. } => {
                outcome.record(match &*element {
                    Element::Item { data, .. } => {
                        super::prims::set_literal(&**data, name.as_ref(), value)
                    }
                    // A component's props were consumed when it built;
                    // its patch is applied to the PROPS instead, at the
                    // call site, before `build` runs. Reaching one here
                    // means the edit was addressed to a component node
                    // by something that did not go through that path.
                    _ => false,
                });
            }
            Edit::SetChildren { children, .. } => {
                outcome.record(set_children(element, children));
            }
        }
    }
    outcome
}

fn set_children(element: &mut Element, new: &[runtime_template::NewNode]) -> bool {
    let Element::Item { children, .. } = element else { return false };
    if !children.iter().all(is_plain_node) {
        return false;
    }
    // Build ALL of them before touching the tree: a half-replaced child
    // list is worse than an unapplied edit, and `build` can refuse at
    // any depth.
    let mut built = Vec::with_capacity(new.len());
    for node in new {
        match super::construct::build(node) {
            Some(e) => built.push(e),
            None => return false,
        }
    }
    *children = built;
    true
}

/// Whether a child is a plain node whose position is decided at build
/// time. Everything else is structure the running program owns.
fn is_plain_node(child: &Element) -> bool {
    matches!(child, Element::Item { .. })
}
