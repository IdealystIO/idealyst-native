//! Conversion from the framework's parallel a11y semantics tree
//! (`framework_core::accessibility::AccessibilityTree`) into AccessKit's
//! flat `TreeUpdate` representation.
//!
//! AccessKit's tree shape:
//! - `TreeUpdate.nodes: Vec<(NodeId, Node)>` — a flat list. Parent/child
//!   relationships are expressed by each parent calling `push_child(id)`
//!   on its `Node`; the tree is reconstructed by AccessKit from those
//!   relations. Order of the vec doesn't matter.
//! - The same `NodeId` overwrites the previous node. We push the **full**
//!   subtree every sync because the wgpu backend rebuilds its tree from
//!   scratch on every `dump_accessibility_tree()` call (it has no
//!   incremental diff to give us).
//!
//! Coordinate space: the framework's `AccessibilityRect` reports bounds
//! in **the parent's** coordinate space (per the contract documented on
//! `AccessibilityNode::bounds`). AccessKit expects each node's bounds
//! in the coordinate space of the nearest ancestor with a non-`None`
//! `transform` — or, when no node sets a transform, in the tree's
//! container (window) space. Flutter-style: parent-relative coords are
//! the natural choice, so we leave `transform` unset and emit each
//! node's bounds verbatim relative to its parent. AT clients then walk
//! the tree to compose the final screen-space rect.

use accesskit::{Live, Node, NodeId, Rect, Tree, TreeUpdate};
use framework_core::accessibility::{
    AccessibilityNode, AccessibilityProps, AccessibilityRect, AccessibilityTraits,
    AccessibilityTree, LiveRegionPriority, Role,
};

/// Map a framework `Role` to AccessKit's `Role`. Every variant of the
/// framework's `Role` enum is covered explicitly — the framework's
/// `#[non_exhaustive]` attribute means new variants can be added later
/// without a build break here, so a wildcard arm is required by the
/// compiler. We pick `accesskit::Role::GenericContainer` as the
/// fallback because it explicitly signals "ignore me" to ATs, which
/// is the safe failure mode (announce nothing, rather than mis-announce).
pub fn role_to_accesskit(role: Role) -> accesskit::Role {
    use accesskit::Role as A;
    match role {
        // Structural.
        Role::Button => A::Button,
        Role::Link => A::Link,
        Role::Image => A::Image,
        Role::Text => A::Paragraph,
        Role::Header => A::Header,
        Role::List => A::List,
        Role::ListItem => A::ListItem,
        Role::Group => A::Group,
        Role::Separator => A::Splitter,
        // Input.
        Role::TextField => A::TextInput,
        Role::TextArea => A::MultilineTextInput,
        Role::Switch => A::Switch,
        Role::Slider => A::Slider,
        Role::Checkbox => A::CheckBox,
        Role::RadioButton => A::RadioButton,
        Role::RadioGroup => A::RadioGroup,
        Role::ComboBox => A::ComboBox,
        Role::SearchField => A::SearchInput,
        // Disclosure / navigation.
        Role::Tab => A::Tab,
        Role::TabList => A::TabList,
        Role::TabPanel => A::TabPanel,
        Role::NavigationLink => A::Link,
        Role::MenuItem => A::MenuItem,
        Role::Menu => A::Menu,
        Role::MenuBar => A::MenuBar,
        Role::Toolbar => A::Toolbar,
        // Feedback.
        Role::Alert => A::Alert,
        Role::Status => A::Status,
        Role::ProgressBar => A::ProgressIndicator,
        Role::Spinner => A::ProgressIndicator,
        // Container / overlay.
        Role::Dialog => A::Dialog,
        Role::AlertDialog => A::AlertDialog,
        Role::Drawer => A::Dialog,
        Role::Popover => A::Dialog,
        Role::Tooltip => A::Tooltip,
        Role::Region => A::Region,
        // The framework's `Role` is `#[non_exhaustive]`; future variants
        // need an entry here. Until then, fall through to a container
        // role that ATs filter out — safer than guessing the wrong role.
        _ => A::GenericContainer,
    }
}

/// Convert a framework parent-relative `AccessibilityRect` to AccessKit's
/// `Rect` (x0/y0/x1/y1, `f64`). Framework rects are device-independent
/// pixels with origin top-left; AccessKit's spec is the same modulo the
/// caller-applied transform (we don't set one — see module doc).
pub fn rect_to_accesskit(r: &AccessibilityRect) -> Rect {
    Rect::new(
        r.x as f64,
        r.y as f64,
        (r.x + r.width) as f64,
        (r.y + r.height) as f64,
    )
}

/// Convert `LiveRegionPriority` to AccessKit's `Live`. `None` from the
/// caller is "not a live region" and the corresponding AccessKit
/// behavior is "don't set the property at all" — handled at the call
/// site (we only call this for nodes with `Some(priority)`).
pub fn live_to_accesskit(p: LiveRegionPriority) -> Live {
    match p {
        LiveRegionPriority::Polite => Live::Polite,
        LiveRegionPriority::Assertive => Live::Assertive,
    }
}

/// Apply the framework's `AccessibilityProps` to an AccessKit `Node`.
/// Skipping every absent field rather than clearing leaves AccessKit's
/// defaults intact, which is what we want — `update_if_active` overwrites
/// the whole node on every update so we don't need to actively clear
/// previously-set fields.
fn apply_props(node: &mut Node, props: &AccessibilityProps) {
    if let Some(label) = &props.label {
        node.set_label(label.as_str());
    }
    if let Some(hint) = &props.hint {
        node.set_description(hint.as_str());
    }
    if let Some(id) = &props.identifier {
        // The framework's `identifier` is the AX hook visible to
        // external tooling (XCUITest, web `id`, UIAutomator resource
        // name). AccessKit has no direct cross-platform equivalent;
        // `class_name` is the platform-AX-attribute escape hatch
        // (maps to UIA `ClassName`, etc.). Documented as the
        // recommended stash slot in upstream AccessKit examples.
        node.set_class_name(id.as_str());
    }
    if props.hidden {
        node.set_hidden();
    }
    if let Some(live) = props.live_region {
        node.set_live(live_to_accesskit(live));
    }

    // Trait bits. Each maps to an AccessKit flag setter; absent flags
    // stay unset (AccessKit treats unset as "concept doesn't apply",
    // which is what we want for orthogonal state).
    let t = props.traits;
    if t.contains(AccessibilityTraits::SELECTED) {
        node.set_selected(true);
    }
    if t.contains(AccessibilityTraits::DISABLED) {
        node.set_disabled();
    }
    if t.contains(AccessibilityTraits::EXPANDED) {
        node.set_expanded(true);
    }
    if t.contains(AccessibilityTraits::COLLAPSED) {
        node.set_expanded(false);
    }
    if t.contains(AccessibilityTraits::CHECKED) {
        node.set_toggled(accesskit::Toggled::True);
    }
    if t.contains(AccessibilityTraits::MIXED) {
        node.set_toggled(accesskit::Toggled::Mixed);
    }
    if t.contains(AccessibilityTraits::BUSY) {
        node.set_busy();
    }
    if t.contains(AccessibilityTraits::REQUIRED) {
        node.set_required();
    }
    if t.contains(AccessibilityTraits::READONLY) {
        node.set_read_only();
    }
    if t.contains(AccessibilityTraits::INVALID) {
        node.set_invalid(accesskit::Invalid::True);
    }
    // `UPDATES_FREQUENTLY` is a hint that maps to `Live::Polite` on
    // platforms that don't separate "coalesce updates" from
    // "announce updates"; if a `live_region` priority is already set
    // it takes precedence (the more explicit signal wins).
    if t.contains(AccessibilityTraits::UPDATES_FREQUENTLY) && props.live_region.is_none() {
        node.set_live(Live::Polite);
    }
}

/// Walk an `AccessibilityNode` (and its children) into a flat list of
/// AccessKit `(NodeId, Node)` pairs. The framework node's `id` (a `u64`)
/// becomes the AccessKit `NodeId` directly — the framework guarantees
/// it's stable per-pointer for the lifetime of the underlying primitive.
fn flatten(node: &AccessibilityNode, out: &mut Vec<(NodeId, Node)>) {
    let mut ak = Node::new(role_to_accesskit(node.role));
    ak.set_bounds(rect_to_accesskit(&node.bounds));
    apply_props(&mut ak, &node.props);
    for child in &node.children {
        ak.push_child(NodeId(child.id));
    }
    out.push((NodeId(node.id), ak));
    for child in &node.children {
        flatten(child, out);
    }
}

/// NodeId reserved for the synthetic announcement node. The framework's
/// real node ids are derived from `Rc::as_ptr(...) as u64`, which on
/// every supported target sits in a 16-byte-aligned region — the bottom
/// two bits are always zero, so `u64::MAX` will never collide.
pub const ANNOUNCEMENT_NODE_ID: NodeId = NodeId(u64::MAX);

/// Build a full `TreeUpdate` from an `AccessibilityTree` plus an
/// optional announcement message.
///
/// The synthetic announcement node pattern:
/// AccessKit has no imperative "speak this string" API the way
/// `UIAccessibility.post(notification: .announcement, …)` does. The
/// canonical workaround is to attach a `Live::Polite`-or-`Assertive`
/// node to the tree and mutate its `label` whenever you want speech.
/// Screen readers observe the label change and announce it. We always
/// emit this node as a child of the root so it's part of the AT-visible
/// tree from the first update; its label is empty until the host calls
/// [`build_tree_with_announcement`].
pub fn build_tree(tree: &AccessibilityTree) -> TreeUpdate {
    build_tree_with_announcement(tree, None)
}

/// Like [`build_tree`] but stamps the synthetic announcement node's
/// label and live-region priority. Pass `Some((msg, priority))` to fire
/// a screen-reader announcement on the next AT poll.
pub fn build_tree_with_announcement(
    tree: &AccessibilityTree,
    announcement: Option<(&str, LiveRegionPriority)>,
) -> TreeUpdate {
    let mut nodes = Vec::with_capacity(estimate_node_count(&tree.root) + 1);
    flatten(&tree.root, &mut nodes);

    // Attach the announcement node as a child of the root. We have to
    // mutate the already-pushed root entry's `Node` to add the child
    // id, since AccessKit reconstructs the parent/child graph from
    // each node's `children` list. (The framework's root is always
    // `nodes[0]` because `flatten` pushes parent before children.)
    let root_id = NodeId(tree.root.id);
    if let Some((_, root_node)) = nodes.iter_mut().find(|(id, _)| *id == root_id) {
        root_node.push_child(ANNOUNCEMENT_NODE_ID);
    }

    // Build the announcement node. Default `Live::Polite` keeps it
    // observable to AT even when no announcement is pending — that
    // way the AT has already discovered the node by the time we
    // start mutating its label.
    let mut ann = Node::new(accesskit::Role::GenericContainer);
    let (msg, priority) = announcement.unwrap_or(("", LiveRegionPriority::Polite));
    ann.set_label(msg);
    ann.set_live(live_to_accesskit(priority));
    // Zero-sized rect; the node has no visible geometry. AccessKit
    // requires bounds on positioned nodes but a degenerate rect is
    // accepted for off-screen / virtual nodes.
    ann.set_bounds(Rect::new(0.0, 0.0, 0.0, 0.0));
    nodes.push((ANNOUNCEMENT_NODE_ID, ann));

    TreeUpdate {
        nodes,
        tree: Some(Tree::new(root_id)),
        tree_id: accesskit::TreeId::ROOT,
        // No keyboard focus model in the wgpu backend yet — focus
        // defaults to the root so AccessKit doesn't reject the update.
        // Future: pipe the framework's focus signal through here.
        focus: root_id,
    }
}

fn estimate_node_count(node: &AccessibilityNode) -> usize {
    1 + node
        .children
        .iter()
        .map(estimate_node_count)
        .sum::<usize>()
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use framework_core::accessibility::{
        AccessibilityNode, AccessibilityProps, AccessibilityRect, AccessibilityTraits,
        AccessibilityTree, LiveRegionPriority, Role,
    };

    fn rect(x: f32, y: f32, w: f32, h: f32) -> AccessibilityRect {
        AccessibilityRect {
            x,
            y,
            width: w,
            height: h,
        }
    }

    fn node(
        id: u64,
        role: Role,
        bounds: AccessibilityRect,
        props: AccessibilityProps,
        children: Vec<AccessibilityNode>,
    ) -> AccessibilityNode {
        AccessibilityNode {
            id,
            props,
            role,
            bounds,
            children,
        }
    }

    /// Root with the expected role, label, and bounds.
    #[test]
    fn build_tree_root_has_role_label_and_bounds() {
        let root = node(
            7,
            Role::Button,
            rect(10.0, 20.0, 100.0, 30.0),
            AccessibilityProps {
                label: Some("Submit".into()),
                ..Default::default()
            },
            vec![],
        );
        let tree = AccessibilityTree { root };
        let update = build_tree(&tree);

        // Root id + role + label + bounds — and an extra synthetic
        // announcement node always tagged on.
        let (root_id, root_node) = update
            .nodes
            .iter()
            .find(|(id, _)| *id == NodeId(7))
            .expect("root in update");
        assert_eq!(*root_id, NodeId(7));
        assert_eq!(root_node.role(), accesskit::Role::Button);
        assert_eq!(root_node.label().as_deref(), Some("Submit"));
        assert_eq!(
            root_node.bounds(),
            Some(Rect::new(10.0, 20.0, 110.0, 50.0))
        );
        assert_eq!(update.tree.as_ref().unwrap().root, NodeId(7));
        assert_eq!(update.focus, NodeId(7));

        // Two entries total: the real root + the synthetic
        // announcement node.
        assert_eq!(update.nodes.len(), 2);
    }

    /// A nested View>Text tree produces correct parent/child relations.
    #[test]
    fn nested_tree_produces_parent_child_relation() {
        let child = node(
            2,
            Role::Text,
            rect(0.0, 0.0, 80.0, 20.0),
            AccessibilityProps {
                label: Some("Hello".into()),
                ..Default::default()
            },
            vec![],
        );
        let root = node(
            1,
            Role::Group,
            rect(0.0, 0.0, 200.0, 100.0),
            AccessibilityProps::default(),
            vec![child],
        );
        let tree = AccessibilityTree { root };
        let update = build_tree(&tree);

        // Both real nodes are present (plus the synthetic announcement).
        assert_eq!(update.nodes.len(), 3);

        let (_, root_node) = update
            .nodes
            .iter()
            .find(|(id, _)| *id == NodeId(1))
            .expect("root");
        // Root declares the text child + the announcement node.
        let children: Vec<NodeId> = root_node.children().iter().copied().collect();
        assert!(children.contains(&NodeId(2)));
        assert!(children.contains(&ANNOUNCEMENT_NODE_ID));

        // Text node carries its own role + label and no children.
        let (_, text_node) = update
            .nodes
            .iter()
            .find(|(id, _)| *id == NodeId(2))
            .expect("text");
        assert_eq!(text_node.role(), accesskit::Role::Paragraph);
        assert_eq!(text_node.label().as_deref(), Some("Hello"));
        assert!(text_node.children().is_empty());
    }

    /// `traits` bits map to the matching AccessKit setters.
    #[test]
    fn traits_propagate_to_accesskit_flags() {
        let n = node(
            5,
            Role::Checkbox,
            rect(0.0, 0.0, 20.0, 20.0),
            AccessibilityProps {
                label: Some("Accept".into()),
                traits: AccessibilityTraits::SELECTED
                    | AccessibilityTraits::CHECKED
                    | AccessibilityTraits::REQUIRED
                    | AccessibilityTraits::DISABLED,
                ..Default::default()
            },
            vec![],
        );
        let update = build_tree(&AccessibilityTree { root: n });

        let (_, ak) = update
            .nodes
            .iter()
            .find(|(id, _)| *id == NodeId(5))
            .unwrap();
        assert_eq!(ak.is_selected(), Some(true));
        assert_eq!(ak.toggled(), Some(accesskit::Toggled::True));
        assert!(ak.is_required());
        assert!(ak.is_disabled());
    }

    /// `hidden` and `live_region` map onto AccessKit's `hidden` flag +
    /// `live` enum property.
    #[test]
    fn hidden_and_live_region_propagate() {
        let n = node(
            9,
            Role::Status,
            rect(0.0, 0.0, 100.0, 20.0),
            AccessibilityProps {
                label: Some("Saved".into()),
                hidden: true,
                live_region: Some(LiveRegionPriority::Assertive),
                ..Default::default()
            },
            vec![],
        );
        let update = build_tree(&AccessibilityTree { root: n });
        let (_, ak) = update
            .nodes
            .iter()
            .find(|(id, _)| *id == NodeId(9))
            .unwrap();
        assert!(ak.is_hidden());
        assert_eq!(ak.live(), Some(Live::Assertive));
    }

    /// The `identifier` field stashes into AccessKit's `class_name` —
    /// the documented escape hatch for platform-AX identifier hooks.
    #[test]
    fn identifier_stashes_into_class_name() {
        let n = node(
            3,
            Role::Button,
            rect(0.0, 0.0, 10.0, 10.0),
            AccessibilityProps {
                identifier: Some("submit-btn".into()),
                ..Default::default()
            },
            vec![],
        );
        let update = build_tree(&AccessibilityTree { root: n });
        let (_, ak) = update
            .nodes
            .iter()
            .find(|(id, _)| *id == NodeId(3))
            .unwrap();
        assert_eq!(ak.class_name(), Some("submit-btn"));
    }

    /// An announcement payload lights up the synthetic announcement
    /// node's label.
    #[test]
    fn announcement_updates_synthetic_node_label() {
        let root = node(
            1,
            Role::Group,
            rect(0.0, 0.0, 100.0, 100.0),
            AccessibilityProps::default(),
            vec![],
        );
        let update = build_tree_with_announcement(
            &AccessibilityTree { root },
            Some(("Form submitted", LiveRegionPriority::Assertive)),
        );
        let (_, ann) = update
            .nodes
            .iter()
            .find(|(id, _)| *id == ANNOUNCEMENT_NODE_ID)
            .expect("announcement node present");
        assert_eq!(ann.label().as_deref(), Some("Form submitted"));
        assert_eq!(ann.live(), Some(Live::Assertive));
    }

    /// `role_to_accesskit` falls back to `GenericContainer` for any
    /// future `Role` variant we haven't taught it about yet. We can't
    /// construct a hypothetical future variant from outside the crate,
    /// so spot-check the known mappings + the fallback shape.
    #[test]
    fn role_table_covers_known_variants() {
        assert_eq!(role_to_accesskit(Role::Button), accesskit::Role::Button);
        assert_eq!(role_to_accesskit(Role::Slider), accesskit::Role::Slider);
        assert_eq!(
            role_to_accesskit(Role::TextArea),
            accesskit::Role::MultilineTextInput
        );
        assert_eq!(role_to_accesskit(Role::Spinner), accesskit::Role::ProgressIndicator);
        assert_eq!(role_to_accesskit(Role::Drawer), accesskit::Role::Dialog);
    }
}
