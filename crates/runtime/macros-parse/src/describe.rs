//! Parsed tree → [`Descriptor`].
//!
//! This is the build-time half of the overlay: the CLI hands it a `ui!`
//! body from a source file and gets back the data a differ compares.
//! The same function is NOT run by the proc macro — the macro emits only
//! tags — but it walks the tree in the same order the macro numbers it,
//! and it checks that at every node (see [`crate::number::check_stamp`]).
//!
//! # What ends up in the descriptor
//!
//! | source | node |
//! |---|---|
//! | a primitive tag | [`Node::Prim`], `kind` = its canonical snake_case name |
//! | a `#[component]` tag | [`Node::Component`], `path` = the tag as written |
//! | a primitive or component carrying a trailing `.method(…)` chain | [`Node::Opaque`] — the chain is raw tokens over an open-ended builder surface |
//! | `if` / `match` / `for` | [`Node::Opaque`], `expr` = the condition / scrutinee / iterable |
//! | a bare Rust expression child | [`Node::Opaque`], `expr` = the expression |
//!
//! A prop's value is descriptor DATA when [`classify_static`] says so,
//! a [`PropValue::Slot`] when the split pass hoisted it to a `Prelude`
//! local (the local's name carries the index), and otherwise the
//! expression's squashed source text as a [`LiteralValue::Path`].
//!
//! That last case — a closure, a macro, a reactive-call shape, anything
//! the split pass leaves in place — is deliberate. Such a value is code
//! and can never be patched. Recording its TEXT still lets a differ see
//! that it changed, which is the difference between "rebuild needed" and
//! "silently served a stale tree".

use std::collections::BTreeMap;

use quote::ToTokens;
use runtime_template::{
    Descriptor, LiteralValue, Node, PropEntry, PropValue, SiteId, SlotInfo, SlotSig,
};

use crate::ast::{MatchArm, Prop, Ui, UiNode};
use crate::number::{check_stamp, number_elements, StampMismatch};
use crate::primitives::canonical_primitive;
use crate::split::{
    children_kind, classify_static, reset_slot_counter, slot_index_of, split, ChildrenKind, Scope,
    StaticValue,
};

/// Describe one `ui!` site.
///
/// `ui` is numbered in place first, so the stamps the descriptor is
/// checked against are the same ones an expansion of these tokens would
/// tag with.
pub fn describe(site: SiteId, ui: &mut Ui) -> Result<Descriptor, StampMismatch> {
    let numbering = number_elements(&mut ui.elements);
    reset_slot_counter();

    let mut b = Builder {
        nodes: vec![None; numbering.count as usize],
        slots: BTreeMap::new(),
        counter: 0,
    };
    let roots = b.scope(&ui.elements)?;

    let slot_count = b.slots.keys().next_back().map_or(0, |k| k + 1);
    let slots: Vec<SlotInfo> = (0..slot_count)
        .map(|i| {
            b.slots.remove(&i).unwrap_or(SlotInfo {
                name: None,
                role: "unused".into(),
                kind: "unused".into(),
            })
        })
        .collect();

    Ok(Descriptor {
        site,
        slots: SlotSig { slots: slots.into() },
        // A node left `None` would be a walk bug; there is no such path
        // (every index the counter hands out is filled by the same
        // iteration), and an `Opaque` is the safe filling if one ever
        // appears.
        nodes: b
            .nodes
            .into_iter()
            .map(|n| n.unwrap_or(Node::Opaque { expr: None, children: [].as_slice().into() }))
            .collect::<Vec<_>>()
            .into(),
        roots: roots.into(),
    })
}

struct Builder {
    nodes: Vec<Option<Node>>,
    slots: BTreeMap<usize, SlotInfo>,
    counter: u32,
}

impl Builder {
    /// Describe one template SCOPE: split it, record its slots, then
    /// walk the rewritten nodes.
    ///
    /// Splitting per scope rather than once is not an optimization — a
    /// scope is a Rust scope, and the split pass's whole job is deciding
    /// what gets hoisted to the head of one. The emission splits the
    /// same scopes at the same boundaries (`children_kind` decides
    /// them), which is why both sides see the same `Prelude` locals.
    fn scope(&mut self, nodes: &[UiNode]) -> Result<Vec<u32>, StampMismatch> {
        let scope: Scope = split(nodes);
        for slot in &scope.slots {
            self.slots.insert(
                slot.index,
                SlotInfo {
                    name: slot.name.map(Into::into),
                    role: slot.role.as_str().into(),
                    kind: slot.kind.into(),
                },
            );
        }
        self.nodes_in_scope(&scope.nodes)
    }

    fn nodes_in_scope(&mut self, nodes: &[UiNode]) -> Result<Vec<u32>, StampMismatch> {
        let mut out = Vec::with_capacity(nodes.len());
        for node in nodes {
            out.push(self.node(node)?);
        }
        Ok(out)
    }

    fn node(&mut self, node: &UiNode) -> Result<u32, StampMismatch> {
        let index = self.counter;
        self.counter += 1;
        check_stamp(node, index)?;

        let described = match node {
            UiNode::Component { name, props, children, chain, .. } => {
                let name_str = name.to_string();
                let canonical = canonical_primitive(&name_str);
                let children = self.component_children(canonical, children)?;
                if chain.is_empty() {
                    let entries = prop_entries(props, canonical, &children, node);
                    match canonical {
                        Some(kind) => Node::Prim {
                            kind: kind.into(),
                            props: entries.into(),
                            children: children.into(),
                        },
                        None => {
                            let dynamic: Vec<runtime_template::Text> = entries
                                .iter()
                                .filter(|e| !is_data(&e.value))
                                .map(|e| e.name.clone())
                                .collect();
                            Node::Component {
                                path: name_str.into(),
                                props: entries.into(),
                                children: children.into(),
                                dynamic: dynamic.into(),
                            }
                        }
                    }
                } else {
                    // A trailing chain is raw tokens over an open-ended
                    // builder surface: the node is addressable, what the
                    // chain did to it is not describable.
                    Node::Opaque { expr: None, children: children.into() }
                }
            }
            UiNode::For { iter, body, .. } => {
                let children = self.scope(body)?;
                Node::Opaque { expr: Some(squash(iter).into()), children: children.into() }
            }
            UiNode::If { cond, then_body, else_body } => {
                let mut children = self.scope(then_body)?;
                if let Some(other) = else_body {
                    children.extend(self.scope(other)?);
                }
                Node::Opaque { expr: Some(squash(cond).into()), children: children.into() }
            }
            UiNode::Match { scrutinee, arms } => {
                let mut children = Vec::new();
                for MatchArm { body, .. } in arms {
                    children.extend(self.scope(body)?);
                }
                Node::Opaque { expr: Some(squash(scrutinee).into()), children: children.into() }
            }
            UiNode::Expr(e) => {
                Node::Opaque { expr: Some(squash(e).into()), children: [].as_slice().into() }
            }
        };
        self.nodes[index as usize] = Some(described);
        Ok(index)
    }

    /// A component's children, recursed by the SAME rule the split pass
    /// and the emission use: a child list built inline shares its
    /// parent's scope; one built inside a closure is its own.
    fn component_children(
        &mut self,
        canonical: Option<&'static str>,
        children: &Option<Vec<UiNode>>,
    ) -> Result<Vec<u32>, StampMismatch> {
        let Some(kids) = children else { return Ok(Vec::new()) };
        match children_kind(canonical, canonical.is_some()) {
            ChildrenKind::List | ChildrenKind::Content | ChildrenKind::Ignored => {
                self.nodes_in_scope(kids)
            }
            ChildrenKind::NestedScope => self.scope(kids),
        }
    }
}

fn is_data(value: &PropValue) -> bool {
    matches!(value, PropValue::Lit(v) if !matches!(v, LiteralValue::Path(_)))
}

/// Every prop of a node, plus `text`'s literal body promoted to a
/// `content` prop.
///
/// `text { "hi" }` and `text(content = "hi")` are the same tree to an
/// author and the same emission; recording them the same way is what
/// lets a differ see an edit to either spelling as one changed prop
/// rather than as a changed child.
fn prop_entries(
    props: &[Prop],
    canonical: Option<&'static str>,
    _children: &[u32],
    node: &UiNode,
) -> Vec<PropEntry> {
    let mut out: Vec<PropEntry> =
        props.iter().map(|p| PropEntry { name: p.name.to_string().into(), value: value_of(p) }).collect();

    if canonical == Some("text") && !props.iter().any(|p| p.name == "content") {
        if let UiNode::Component { children: Some(kids), .. } = node {
            if let [UiNode::Expr(e)] = kids.as_slice() {
                if let Some(v) = classify_static(e) {
                    out.push(PropEntry { name: "content".into(), value: PropValue::Lit(lit(v)) });
                }
            }
        }
    }
    out
}

fn value_of(prop: &Prop) -> PropValue {
    if let Some(index) = slot_index_of(&prop.value) {
        return PropValue::Slot(index as u32);
    }
    match classify_static(&prop.value) {
        Some(v) => PropValue::Lit(lit(v)),
        None => PropValue::Lit(LiteralValue::Path(squash(&prop.value).into())),
    }
}

fn lit(value: StaticValue) -> LiteralValue {
    match value {
        StaticValue::Str(s) => LiteralValue::Str(s.into()),
        StaticValue::Int(i) => LiteralValue::Int(i),
        StaticValue::Float(f) => LiteralValue::Float(f),
        StaticValue::Bool(b) => LiteralValue::Bool(b),
        StaticValue::Path(p) => LiteralValue::Path(p.into()),
    }
}

/// Whitespace-squashed source text of an expression. Squashed so that
/// reformatting alone never reads as a change.
fn squash(expr: &impl ToTokens) -> String {
    expr.to_token_stream().to_string().chars().filter(|c| !c.is_whitespace()).collect()
}
