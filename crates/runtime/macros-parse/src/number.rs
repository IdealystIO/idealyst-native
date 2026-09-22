//! Node numbering: the one thing the macro and the build-time producer
//! must agree on exactly.
//!
//! Every node of a parsed `ui!` body gets an index from a single
//! PREORDER walk — a node before everything beneath it, siblings in
//! source order. `Component` nodes keep theirs in
//! [`UiNode::Component::node`]; control-flow nodes consume an index
//! without storing one, because only a `Component` ever becomes a
//! tagged `Element` and only it needs to look its number up later.
//!
//! # Why the number lives in the node and not in a counter
//!
//! The macro used to count `emit_component` calls. That is the same
//! thing only while emission order equals source order, and it is not:
//! `anchored_overlay` splits its children into an anchor and an overlay,
//! `presence` builds a child thunk, a `for` may take the virtualizer
//! path. A counter would number those trees differently from a walk of
//! the same tree, and the build-time descriptor — which has only the
//! tree — would address the wrong node.
//!
//! Stamping the tree instead removes the question. The split pass clones
//! nodes rather than mutating them, so the stamp rides along into the
//! rewritten tree the emission actually walks, and every primitive is
//! free to emit its children in whatever order it needs.
//!
//! # Why control flow is numbered at all
//!
//! It never carries a tag, but it IS a descriptor node: a `for` is a
//! `Node::Opaque` whose children are the row body's nodes, and those
//! children are named by index. Numbering the whole tree in one walk is
//! what makes "child index" mean the same thing on both sides.

use crate::ast::{MatchArm, UiNode};

/// How many nodes a site has, and — as a side effect — every
/// `Component`'s index stamped into the tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeNumbering {
    /// Total nodes, control flow included. Equals the length of the
    /// descriptor's flat node array.
    pub count: u32,
}

/// Stamp `elements` in place and report the total.
pub fn number_elements(elements: &mut [UiNode]) -> NodeNumbering {
    let mut next = 0u32;
    number_nodes(elements, &mut next);
    NodeNumbering { count: next }
}

fn number_nodes(nodes: &mut [UiNode], next: &mut u32) {
    for node in nodes {
        number_node(node, next);
    }
}

fn number_node(node: &mut UiNode, next: &mut u32) {
    let index = *next;
    *next += 1;
    match node {
        UiNode::Component { node: slot, children, .. } => {
            *slot = index;
            if let Some(kids) = children {
                number_nodes(kids, next);
            }
        }
        UiNode::For { body, .. } => number_nodes(body, next),
        UiNode::If { then_body, else_body, .. } => {
            number_nodes(then_body, next);
            if let Some(kids) = else_body {
                number_nodes(kids, next);
            }
        }
        UiNode::Match { arms, .. } => {
            for MatchArm { body, .. } in arms {
                number_nodes(body, next);
            }
        }
        UiNode::Expr(_) => {}
    }
}

/// The number [`number_elements`] would give the node at a given point
/// of the same walk, checked against the stamp it left.
///
/// The descriptor producer walks the tree a second time and needs
/// indices for the nodes that carry no stamp (control flow), so it
/// keeps its own counter. This is how that counter is kept honest:
/// every time the walk meets a `Component`, the counter must equal what
/// the stamping walk wrote there. If the two recursions ever drift
/// apart — someone adds a node kind to one and not the other — the
/// descriptor would address a different node than the tags do, silently.
/// This makes it an error instead.
pub fn check_stamp(node: &UiNode, counter: u32) -> Result<(), StampMismatch> {
    match node {
        UiNode::Component { node: stamped, name, .. } if *stamped != counter => {
            Err(StampMismatch { tag: *stamped, descriptor: counter, name: name.to_string() })
        }
        _ => Ok(()),
    }
}

/// The two walks disagree. See [`check_stamp`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StampMismatch {
    /// What the emission will tag this node with.
    pub tag: u32,
    /// What the descriptor would call it.
    pub descriptor: u32,
    pub name: String,
}

impl std::fmt::Display for StampMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "node numbering drift at `{}`: emission tags it {}, the descriptor calls it {}",
            self.name, self.tag, self.descriptor
        )
    }
}

impl std::error::Error for StampMismatch {}
