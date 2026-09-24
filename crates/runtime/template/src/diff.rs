//! Two descriptors in, a [`Patch`] or a refusal out.
//!
//! This is where "is this edit safe?" is answered, and it is answered
//! here because here is the only place BOTH source versions exist. A
//! running app has one; the compiled code that would receive the patch
//! has the other baked in and cannot describe it. Trying to check this
//! inside the app was what put descriptors in the binary, and that cost
//! 1.4 s on every rebuild.
//!
//! # Edits are addressed by the OLD numbering
//!
//! Node indices are positions in a preorder walk, so inserting a node
//! renumbers everything after it. The binary carries the old numbering —
//! it is the build that is running. So every edit names an OLD index,
//! and everything a patch ASKS FOR is data ([`NewNode`]), never an index
//! into the new descriptor. A patch is meaningful without the new
//! descriptor in hand, which is what lets the applier work from the
//! patch alone.
//!
//! # What is refused
//!
//! Everything that would mean the compiled code changed. The slots ARE
//! the compiled code: an expression the split pass left in place is
//! machine code in the running binary, and no patch can replace it. So:
//!
//! - a different slot signature — the site's code changed shape;
//! - a prop that moved between data and a slot, or to a different slot —
//!   the value is now computed where it was written, or vice versa;
//! - a changed `if` condition, `for` iterable or `match` scrutinee —
//!   compiled code, recorded as text precisely so a differ can SEE it
//!   change and refuse;
//! - a new subtree that references a slot or a style path — it cannot be
//!   built from data;
//! - a structural change around a control-flow node, whose position in
//!   its parent's child list is decided at runtime.
//!
//! A refusal is not a failure. It is the differ saying "this one needs a
//! rebuild", which is the correct and available answer.

use crate::{Descriptor, Edit, LiteralValue, NewNode, Node, Patch, PropEntry, PropValue, SiteId, SlotSig};
use std::fmt;

/// Why two descriptors cannot be bridged by a patch.
#[derive(Clone, Debug, PartialEq)]
pub enum Rejection {
    /// The two descriptors are not the same site.
    DifferentSite { old: SiteId, new: SiteId },
    /// The slot list changed shape. The slots are compiled code; a
    /// patch cannot supply new code.
    SlotsChanged { old: usize, new: usize },
    /// One slot's recorded role/kind changed at the same position.
    SlotShapeChanged { index: usize },
    /// A prop moved between descriptor data and compiled code, or to a
    /// different slot.
    PropMovedToCode { node: u32, prop: String },
    /// The tree changed shape somewhere a patch cannot reach: a
    /// control-flow node gained or lost siblings, or a node changed kind
    /// under a parent whose child list is not replaceable.
    ShapeChanged { node: u32 },
    /// A construct's defining expression changed — an `if` condition, a
    /// `for` iterable, a `match` scrutinee, a bare expression child.
    CodeChanged { node: u32 },
    /// A slot's expression changed — the compiled code that supplies a
    /// value, not the value. Only a compiler can carry it. (A wrapped
    /// literal whose wrapper stayed put is the exception: see
    /// [`crate::SlotInfo::literal`].)
    SlotCodeChanged { index: usize },
    /// A subtree the patch would have to CONSTRUCT references something
    /// that is not data.
    NotConstructible { node: u32, why: &'static str },
}

impl fmt::Display for Rejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Rejection::DifferentSite { old, new } => {
                write!(f, "not the same site: {old} vs {new}")
            }
            Rejection::SlotsChanged { old, new } => write!(
                f,
                "the site's compiled expressions changed ({old} slots before, {new} after)"
            ),
            Rejection::SlotShapeChanged { index } => {
                write!(f, "slot {index} changed shape")
            }
            Rejection::PropMovedToCode { node, prop } => write!(
                f,
                "node {node}'s `{prop}` moved between a literal and compiled code"
            ),
            Rejection::ShapeChanged { node } => {
                write!(f, "the tree changed shape around node {node}")
            }
            Rejection::CodeChanged { node } => {
                write!(f, "node {node}'s defining expression changed")
            }
            Rejection::SlotCodeChanged { index } => {
                write!(f, "slot {index}'s expression changed")
            }
            Rejection::NotConstructible { node, why } => {
                write!(f, "node {node} of the new tree cannot be built from data: {why}")
            }
        }
    }
}

impl std::error::Error for Rejection {}

/// Turn the difference between two versions of one site into a patch.
pub fn diff(old: &Descriptor, new: &Descriptor) -> Result<Patch, Rejection> {
    if old.site != new.site {
        return Err(Rejection::DifferentSite {
            old: old.site.clone(),
            new: new.site.clone(),
        });
    }
    if old.slots.count() != new.slots.count() {
        return Err(Rejection::SlotsChanged {
            old: old.slots.count(),
            new: new.slots.count(),
        });
    }
    for (index, (a, b)) in old.slots.slots.iter().zip(new.slots.slots.iter()).enumerate() {
        // `name` is deliberately not compared: it is reserved for a
        // name-matched protocol that does not exist, and comparing it
        // would reject a harmless prop rename.
        if a.role != b.role || a.kind != b.kind {
            return Err(Rejection::SlotShapeChanged { index });
        }
        if a.code != b.code {
            // The one code change a patch can carry: the literal inside
            // an unchanged wrapper. `diff_props` turns it into an edit
            // on the prop the slot feeds.
            match (&a.literal, &b.literal) {
                (Some(x), Some(y)) if x.wrapper == y.wrapper => {}
                _ => return Err(Rejection::SlotCodeChanged { index }),
            }
        }
    }

    let mut edits = Vec::new();
    // The roots have no parent to hang a `SetChildren` on, so they must
    // correspond one for one.
    if !lists_correspond(old, new, &old.roots, &new.roots) {
        return Err(Rejection::ShapeChanged { node: *old.roots.first().unwrap_or(&0) });
    }
    for (&a, &b) in old.roots.iter().zip(new.roots.iter()) {
        diff_node(old, new, a, b, &mut edits)?;
    }

    Ok(Patch { site: old.site.clone(), edits: edits.into() })
}

fn diff_node(
    old: &Descriptor,
    new: &Descriptor,
    oi: u32,
    ni: u32,
    edits: &mut Vec<Edit>,
) -> Result<(), Rejection> {
    let (Some(o), Some(n)) = (old.node(oi), new.node(ni)) else {
        return Err(Rejection::ShapeChanged { node: oi });
    };
    match (o, n) {
        (
            Node::Prim { props: op, children: oc, .. },
            Node::Prim { props: np, children: nc, .. },
        )
        | (
            Node::Component { props: op, children: oc, .. },
            Node::Component { props: np, children: nc, .. },
        ) => {
            diff_props(oi, op, np, (&old.slots, &new.slots), edits)?;
            diff_children(old, new, oi, oc, nc, edits)
        }
        (Node::Opaque { expr: oe, children: oc }, Node::Opaque { expr: ne, children: nc }) => {
            if oe != ne {
                return Err(Rejection::CodeChanged { node: oi });
            }
            diff_children(old, new, oi, oc, nc, edits)
        }
        _ => Err(Rejection::ShapeChanged { node: oi }),
    }
}

/// Props are matched by NAME, not position: a descriptor that lists the
/// same props in a different order is the same tree, and treating a
/// reorder as a change would fill every patch with noise.
fn diff_props(
    node: u32,
    old: &[PropEntry],
    new: &[PropEntry],
    (old_slots, new_slots): (&SlotSig, &SlotSig),
    edits: &mut Vec<Edit>,
) -> Result<(), Rejection> {
    for n in new {
        let o = old.iter().find(|o| o.name == n.name);
        match (o.map(|o| &o.value), &n.value) {
            // Same slot. Its code either did not move — the value is
            // compiled code and nothing changed — or it is a wrapped
            // literal whose wrapper did not move (`diff` refused every
            // other code change before getting here), and the literal
            // inside is the edit.
            (Some(PropValue::Slot(a)), PropValue::Slot(b)) if a == b => {
                let literal = |sig: &SlotSig| {
                    sig.slots.get(*a as usize).and_then(|s| s.literal.clone())
                };
                if let (Some(x), Some(y)) = (literal(old_slots), literal(new_slots)) {
                    if x.value != y.value {
                        match patchable(&y.value) {
                            Some(value) => {
                                edits.push(Edit::SetProp { node, name: n.name.clone(), value })
                            }
                            None => {
                                return Err(Rejection::SlotCodeChanged { index: *a as usize })
                            }
                        }
                    }
                }
            }
            // Data to data: a patchable change, or no change.
            (Some(PropValue::Lit(a)), PropValue::Lit(b)) => {
                if a != b {
                    if let Some(value) = patchable(b) {
                        edits.push(Edit::SetProp {
                            node,
                            name: n.name.clone(),
                            value,
                        });
                    } else {
                        // A path (`tone::Danger`, `t.card()`) is source
                        // TEXT. Nothing can rebuild the value from it,
                        // so seeing it change is exactly the signal that
                        // a rebuild is needed.
                        return Err(Rejection::PropMovedToCode {
                            node,
                            prop: n.name.to_string(),
                        });
                    }
                }
            }
            // A prop appeared. Treat it as a set when it is data.
            (None, PropValue::Lit(b)) => match patchable(b) {
                Some(value) => edits.push(Edit::SetProp {
                    node,
                    name: n.name.clone(),
                    value,
                }),
                None => {
                    return Err(Rejection::PropMovedToCode {
                        node,
                        prop: n.name.to_string(),
                    })
                }
            },
            // Anything else is a move between data and code.
            _ => {
                return Err(Rejection::PropMovedToCode {
                    node,
                    prop: n.name.to_string(),
                })
            }
        }
    }
    // A prop that DISAPPEARED cannot be undone: there is no "unset" —
    // the compiled code already passed the old value to the builder, and
    // a builder has no way back to its default.
    for o in old {
        if !new.iter().any(|n| n.name == o.name) {
            return Err(Rejection::PropMovedToCode { node, prop: o.name.to_string() });
        }
    }
    Ok(())
}

fn diff_children(
    old: &Descriptor,
    new: &Descriptor,
    parent: u32,
    oc: &[u32],
    nc: &[u32],
    edits: &mut Vec<Edit>,
) -> Result<(), Rejection> {
    if lists_correspond(old, new, oc, nc) {
        for (&a, &b) in oc.iter().zip(nc.iter()) {
            diff_node(old, new, a, b, edits)?;
        }
        return Ok(());
    }
    // The lists differ in shape, so the whole list is REPLACED — and
    // that means the old children are unmounted. Every one of them must
    // therefore be fully static, recursively:
    //
    // - a control-flow child's position is decided at runtime, so it
    //   cannot be replaced at all; and
    // - a child holding a SLOT is holding compiled code. Its props may
    //   be bound by effects that outlive the node they were written
    //   for — the enclosing scope owns them, not the child — so tearing
    //   it out live would leave a binding writing to something that is
    //   no longer there.
    //
    // Refusing here rather than in the applier is what keeps the two
    // application paths agreeing about what a patch may contain.
    for &a in oc {
        if !is_fully_static(old, a) {
            return Err(Rejection::ShapeChanged { node: a });
        }
    }
    let children: Vec<NewNode> =
        nc.iter().map(|&b| new_node(new, b)).collect::<Result<_, _>>()?;
    edits.push(Edit::SetChildren { node: parent, children: children.into() });
    Ok(())
}

/// Whether two child lists line up node for node — same length, and each
/// pair the same kind of thing.
///
/// Only the kinds are compared, not the whole subtree: two `text` nodes
/// whose content differs DO correspond, and the difference between them
/// is what the patch is for.
fn lists_correspond(old: &Descriptor, new: &Descriptor, oc: &[u32], nc: &[u32]) -> bool {
    oc.len() == nc.len()
        && oc.iter().zip(nc.iter()).all(|(&a, &b)| match (old.node(a), new.node(b)) {
            (Some(x), Some(y)) => same_kind(x, y),
            _ => false,
        })
}

/// Whether a node and everything under it is descriptor DATA — no
/// control flow, no slot-valued prop.
fn is_fully_static(desc: &Descriptor, index: u32) -> bool {
    let Some(node) = desc.node(index) else { return false };
    let (props, children) = match node {
        Node::Prim { props, children, .. } | Node::Component { props, children, .. } => {
            (props, children)
        }
        Node::Opaque { .. } => return false,
    };
    props.iter().all(|p| matches!(p.value, PropValue::Lit(_)))
        && children.iter().all(|&c| is_fully_static(desc, c))
}

fn same_kind(a: &Node, b: &Node) -> bool {
    match (a, b) {
        (Node::Prim { kind: x, .. }, Node::Prim { kind: y, .. }) => x == y,
        (Node::Component { path: x, .. }, Node::Component { path: y, .. }) => x == y,
        // An `Opaque`'s expression IS its identity: two `if`s with
        // different conditions are different constructs, not the same
        // one changed.
        (Node::Opaque { expr: x, .. }, Node::Opaque { expr: y, .. }) => x == y,
        _ => false,
    }
}

/// Lift a node of the NEW descriptor into the data a patch can carry.
fn new_node(new: &Descriptor, index: u32) -> Result<NewNode, Rejection> {
    let node = new.node(index).ok_or(Rejection::ShapeChanged { node: index })?;
    let (kind, props, children) = match node {
        Node::Prim { kind, props, children } => (kind.clone(), props, children),
        Node::Component { path, props, children, .. } => (path.clone(), props, children),
        Node::Opaque { .. } => {
            return Err(Rejection::NotConstructible {
                node: index,
                why: "it is control flow or an expression, which is code",
            })
        }
    };
    for p in props.iter() {
        match &p.value {
            PropValue::Slot(_) => {
                return Err(Rejection::NotConstructible {
                    node: index,
                    why: "one of its props is a compiled expression",
                })
            }
            PropValue::Lit(LiteralValue::Path(_)) => {
                return Err(Rejection::NotConstructible {
                    node: index,
                    why: "one of its props is a path or style token, which is source text only",
                })
            }
            PropValue::Lit(_) => {}
        }
    }
    Ok(NewNode {
        kind,
        props: props.clone(),
        children: children
            .iter()
            .map(|&c| new_node(new, c))
            .collect::<Result<Vec<_>, _>>()?
            .into(),
    })
}

/// A literal an applier can actually write. A [`LiteralValue::Path`] is
/// source text — see the variant's own docs.
fn patchable(value: &LiteralValue) -> Option<LiteralValue> {
    match value {
        LiteralValue::Path(_) => None,
        other => Some(other.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SlotInfo, SlotSig};
    use std::borrow::Cow;

    fn site() -> SiteId {
        SiteId {
            package: Cow::Borrowed("app"),
            file: Cow::Borrowed("src/a.rs"),
            line: 1,
            col: 1,
        }
    }

    fn lit(name: &'static str, value: &'static str) -> PropEntry {
        PropEntry {
            name: Cow::Borrowed(name),
            value: PropValue::Lit(LiteralValue::Str(Cow::Borrowed(value))),
        }
    }

    fn text(content: &'static str) -> Node {
        Node::Prim {
            kind: Cow::Borrowed("text"),
            props: Cow::Owned(vec![lit("content", content)]),
            children: Cow::Borrowed(&[]),
        }
    }

    /// `view > text*`, with the given contents.
    fn tree(contents: &[&'static str]) -> Descriptor {
        let mut nodes = vec![Node::Prim {
            kind: Cow::Borrowed("view"),
            props: Cow::Borrowed(&[]),
            children: Cow::Owned((1..=contents.len() as u32).collect()),
        }];
        nodes.extend(contents.iter().map(|c| text(c)));
        Descriptor {
            site: site(),
            slots: SlotSig { slots: Cow::Borrowed(&[]) },
            nodes: Cow::Owned(nodes),
            roots: Cow::Owned(vec![0]),
        }
    }

    fn slot(role: &'static str, kind: &'static str) -> SlotInfo {
        SlotInfo {
            name: None,
            role: Cow::Borrowed(role),
            kind: Cow::Borrowed(kind),
            code: Cow::Borrowed(""),
            literal: None,
        }
    }

    /// A component node whose one prop, `hint`, is slot 0 with this
    /// code and (optionally) this wrapped literal.
    fn slotted(code: &'static str, literal: Option<(&'static str, &'static str)>) -> Descriptor {
        Descriptor {
            site: site(),
            slots: SlotSig {
                slots: Cow::Owned(vec![SlotInfo {
                    name: Some(Cow::Borrowed("hint")),
                    role: Cow::Borrowed("prop"),
                    kind: Cow::Borrowed("call"),
                    code: Cow::Borrowed(code),
                    literal: literal.map(|(wrapper, value)| crate::WrappedLiteral {
                        wrapper: Cow::Borrowed(wrapper),
                        value: LiteralValue::Str(Cow::Borrowed(value)),
                    }),
                }]),
            },
            nodes: Cow::Owned(vec![Node::Component {
                path: Cow::Borrowed("Field"),
                props: Cow::Owned(vec![PropEntry {
                    name: Cow::Borrowed("hint"),
                    value: PropValue::Slot(0),
                }]),
                children: Cow::Borrowed(&[]),
                dynamic: Cow::Owned(vec![Cow::Borrowed("hint")]),
            }]),
            roots: Cow::Owned(vec![0]),
        }
    }

    /// Regression: a slot's code could change with nothing in the
    /// descriptor moving (`x.clone()` → `y.clone()`), and the save was
    /// dropped as "no change". It is refused now, which sends it to the
    /// compiler.
    #[test]
    fn regression_a_slots_changed_code_is_refused_not_ignored() {
        assert_eq!(
            diff(&slotted("Some(x.clone())", None), &slotted("Some(y.clone())", None)),
            Err(Rejection::SlotCodeChanged { index: 0 })
        );
        assert!(diff(&slotted("Some(x.clone())", None), &slotted("Some(x.clone())", None))
            .unwrap()
            .edits
            .is_empty());
    }

    /// A literal inside an unchanged wrapper is an edit on the prop the
    /// slot feeds, carrying the literal alone.
    #[test]
    fn a_wrapped_literal_under_the_same_wrapper_becomes_a_set_prop() {
        let old = slotted(r#"Some("a".to_string())"#, Some(("Some(to_string)", "a")));
        let new = slotted(r#"Some("b".to_string())"#, Some(("Some(to_string)", "b")));
        assert_eq!(
            diff(&old, &new).unwrap().edits.as_ref(),
            &[Edit::SetProp {
                node: 0,
                name: Cow::Borrowed("hint"),
                value: LiteralValue::Str(Cow::Borrowed("b")),
            }]
        );
    }

    /// The wrapper is the slot's signature: `Some("a".to_string())` →
    /// `Some("a".into())` or `String::from("a")` is different code.
    #[test]
    fn a_changed_wrapper_is_refused() {
        let old = slotted(r#"Some("a".to_string())"#, Some(("Some(to_string)", "a")));
        let new = slotted(r#"Some("b".into())"#, Some(("Some(into)", "b")));
        assert_eq!(diff(&old, &new), Err(Rejection::SlotCodeChanged { index: 0 }));
        let unwrapped = slotted("Some(b)", None);
        assert_eq!(diff(&old, &unwrapped), Err(Rejection::SlotCodeChanged { index: 0 }));
    }

    #[test]
    fn a_changed_literal_becomes_one_set_prop() {
        let patch = diff(&tree(&["a"]), &tree(&["b"])).expect("patchable");
        assert_eq!(
            patch.edits.as_ref(),
            &[Edit::SetProp {
                node: 1,
                name: Cow::Borrowed("content"),
                value: LiteralValue::Str(Cow::Borrowed("b")),
            }]
        );
    }

    /// Adding a sibling renumbers every node after it, so the patch must
    /// name the PARENT — an index that means the same thing in the
    /// running build — and carry the new list as data.
    #[test]
    fn a_new_sibling_becomes_a_set_children_on_the_parent() {
        let patch = diff(&tree(&["a"]), &tree(&["a", "b"])).expect("patchable");
        match patch.edits.as_ref() {
            [Edit::SetChildren { node: 0, children }] => {
                assert_eq!(children.len(), 2);
                assert_eq!(children[1].kind, "text");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_site_diffed_against_itself_produces_nothing() {
        let d = tree(&["a", "b"]);
        assert!(diff(&d, &d).expect("patchable").edits.is_empty());
    }

    /// The slots are the compiled code. A site that gained or lost one
    /// is a site that was recompiled, and no patch can bridge that.
    #[test]
    fn a_changed_slot_count_is_refused() {
        let old = tree(&["a"]);
        let mut new = tree(&["a"]);
        new.slots = SlotSig { slots: Cow::Owned(vec![slot("prop", "call")]) };
        assert_eq!(
            diff(&old, &new),
            Err(Rejection::SlotsChanged { old: 0, new: 1 })
        );
    }

    #[test]
    fn a_slot_that_changed_shape_at_the_same_position_is_refused() {
        let mut old = tree(&["a"]);
        old.slots = SlotSig { slots: Cow::Owned(vec![slot("prop", "call")]) };
        let mut new = tree(&["a"]);
        new.slots = SlotSig { slots: Cow::Owned(vec![slot("cond", "closure")]) };
        assert_eq!(diff(&old, &new), Err(Rejection::SlotShapeChanged { index: 0 }));
    }

    /// A prop that went from data to a slot is a value that moved into
    /// compiled code. The running binary still evaluates the old
    /// expression; nothing a patch says can change that.
    #[test]
    fn a_prop_that_became_a_slot_is_refused() {
        let old = tree(&["a"]);
        let mut new = tree(&["a"]);
        new.slots = SlotSig { slots: Cow::Owned(vec![slot("prop", "closure")]) };
        new.nodes = Cow::Owned(vec![
            Node::Prim {
                kind: Cow::Borrowed("view"),
                props: Cow::Borrowed(&[]),
                children: Cow::Owned(vec![1]),
            },
            Node::Prim {
                kind: Cow::Borrowed("text"),
                props: Cow::Owned(vec![PropEntry {
                    name: Cow::Borrowed("content"),
                    value: PropValue::Slot(0),
                }]),
                children: Cow::Borrowed(&[]),
            },
        ]);
        // The slot-count check fires first, which is the same verdict by
        // a shorter route. Give both descriptors the slot so the prop
        // check is what answers.
        let mut old = old;
        old.slots = new.slots.clone();
        assert_eq!(
            diff(&old, &new),
            Err(Rejection::PropMovedToCode { node: 1, prop: "content".to_string() })
        );
    }

    /// A style token or enum path is recorded as SOURCE TEXT — nothing
    /// can rebuild the value from it. Seeing one change is exactly the
    /// signal that a rebuild is needed.
    #[test]
    fn a_changed_path_valued_prop_is_refused() {
        let mut old = tree(&["a"]);
        let mut new = tree(&["a"]);
        for (d, token) in [(&mut old, "t.card()"), (&mut new, "t.panel()")] {
            d.nodes = Cow::Owned(vec![
                Node::Prim {
                    kind: Cow::Borrowed("view"),
                    props: Cow::Owned(vec![PropEntry {
                        name: Cow::Borrowed("style"),
                        value: PropValue::Lit(LiteralValue::Path(Cow::Borrowed(token))),
                    }]),
                    children: Cow::Owned(vec![1]),
                },
                text("a"),
            ]);
        }
        assert_eq!(
            diff(&old, &new),
            Err(Rejection::PropMovedToCode { node: 0, prop: "style".to_string() })
        );
    }

    /// A control-flow child's position in its parent's list is decided
    /// at runtime, so the list cannot be replaced wholesale.
    #[test]
    fn a_sibling_added_beside_control_flow_is_refused() {
        let opaque = Node::Opaque {
            expr: Some(Cow::Borrowed("count>0")),
            children: Cow::Borrowed(&[]),
        };
        let mut old = tree(&["a"]);
        old.nodes = Cow::Owned(vec![
            Node::Prim {
                kind: Cow::Borrowed("view"),
                props: Cow::Borrowed(&[]),
                children: Cow::Owned(vec![1]),
            },
            opaque.clone(),
        ]);
        let mut new = tree(&["a"]);
        new.nodes = Cow::Owned(vec![
            Node::Prim {
                kind: Cow::Borrowed("view"),
                props: Cow::Borrowed(&[]),
                children: Cow::Owned(vec![1, 2]),
            },
            opaque,
            text("b"),
        ]);
        assert_eq!(diff(&old, &new), Err(Rejection::ShapeChanged { node: 1 }));
    }

    /// A subtree carrying a slot is code, and a patch can only carry
    /// data — so it is refused at the point it would have to be built,
    /// with a reason a dev server can show.
    #[test]
    fn a_new_subtree_that_references_code_is_refused() {
        let old = tree(&["a"]);
        let mut new = tree(&["a"]);
        new.slots = SlotSig { slots: Cow::Owned(vec![slot("prop", "call")]) };
        new.nodes = Cow::Owned(vec![
            Node::Prim {
                kind: Cow::Borrowed("view"),
                props: Cow::Borrowed(&[]),
                children: Cow::Owned(vec![1, 2]),
            },
            text("a"),
            Node::Prim {
                kind: Cow::Borrowed("text"),
                props: Cow::Owned(vec![PropEntry {
                    name: Cow::Borrowed("content"),
                    value: PropValue::Slot(0),
                }]),
                children: Cow::Borrowed(&[]),
            },
        ]);
        let mut old = old;
        old.slots = new.slots.clone();
        assert!(matches!(
            diff(&old, &new),
            Err(Rejection::NotConstructible { node: 2, .. })
        ));
    }

    #[test]
    fn two_different_sites_never_diff() {
        let old = tree(&["a"]);
        let mut new = tree(&["a"]);
        new.site = SiteId {
            package: Cow::Borrowed("app"),
            file: Cow::Borrowed("src/b.rs"),
            line: 1,
            col: 1,
        };
        assert!(matches!(diff(&old, &new), Err(Rejection::DifferentSite { .. })));
    }
}
