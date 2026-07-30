//! The scene suite: idea-lite's scene-layer tests (28–33), ported for
//! real (P0 deferred them to P1), plus driver-invariant tests that assert
//! the golden-pinned behaviors directly at the unit level — the parity
//! crate (`scene-parity`) then re-asserts them against the shared goldens.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use runtime_world::{effect, signal, World};

use super::*;

// ============================================================================
// Test host + vocabulary
// ============================================================================

/// A recording host. Nodes are creation-order `u32`s; ops mirror the
/// scene-parity recorder's format so sequences read the same everywhere.
///
/// It ALSO keeps a real parent→children model (`tree`). The op log alone
/// cannot answer "did these two mount strategies produce the same tree?",
/// because two different op sequences can converge on the same structure —
/// which is exactly the question the deferred-payload tests must settle.
struct TestHost {
    ops: Rc<RefCell<Vec<String>>>,
    labels: Rc<RefCell<HashMap<u32, String>>>,
    /// Parent → children, in order. Maintained by applying the same op
    /// semantics a real host implements (DOM `insertBefore` for
    /// `insert_at`, including its move-an-existing-child behavior).
    tree: Rc<RefCell<HashMap<u32, Vec<u32>>>>,
    next: u32,
    splice: bool,
}

impl TestHost {
    fn create(&mut self, label: &str) -> u32 {
        let n = self.next;
        self.next += 1;
        self.ops.borrow_mut().push(format!("create n{n} {label}"));
        self.labels.borrow_mut().insert(n, label.to_string());
        n
    }
}

impl Host for TestHost {
    type Node = u32;

    fn insert(&mut self, parent: &mut u32, child: u32) {
        self.ops
            .borrow_mut()
            .push(format!("insert n{parent} <- n{child}"));
        self.tree.borrow_mut().entry(*parent).or_default().push(child);
    }

    fn insert_many(&mut self, parent: &mut u32, children: Vec<u32>) {
        let kids: Vec<String> = children.iter().map(|c| format!("n{c}")).collect();
        self.ops
            .borrow_mut()
            .push(format!("insert_many n{parent} <- [{}]", kids.join(", ")));
        self.tree
            .borrow_mut()
            .entry(*parent)
            .or_default()
            .extend(children);
    }

    fn insert_at(&mut self, parent: &mut u32, child: u32, index: usize) {
        self.ops
            .borrow_mut()
            .push(format!("insert_at n{parent} <- n{child} @ {index}"));
        let mut tree = self.tree.borrow_mut();
        let kids = tree.entry(*parent).or_default();
        // DOM `insertBefore` semantics: the reference child is resolved
        // BEFORE the node is detached from its old position (the keyed
        // reconciler's reorder pass depends on that ordering).
        let reference = kids.get(index).copied();
        kids.retain(|c| *c != child);
        match reference.and_then(|r| kids.iter().position(|c| *c == r)) {
            Some(pos) => kids.insert(pos, child),
            None => kids.push(child),
        }
    }

    fn remove_child(&mut self, parent: &u32, child: &u32) {
        self.ops
            .borrow_mut()
            .push(format!("remove_child n{parent} -x n{child}"));
        if let Some(kids) = self.tree.borrow_mut().get_mut(parent) {
            kids.retain(|c| c != child);
        }
    }

    fn clear_children(&mut self, node: &u32) {
        self.ops.borrow_mut().push(format!("clear_children n{node}"));
        if let Some(kids) = self.tree.borrow_mut().get_mut(node) {
            kids.clear();
        }
    }

    fn create_anchor(&mut self) -> u32 {
        self.create("anchor")
    }

    fn supports_splice(&self) -> bool {
        self.splice
    }
}

/// Container payload: mounts a node, realizes children into it.
struct V;
/// Leaf payload: mounts a node labeled with its text.
struct T(String);
/// A payload nobody registers (miss diagnostics).
struct Unregistered;
/// Multi-node payload (`Element::Many`): N sibling leaf rows.
struct Rep(usize);
/// A Many payload nobody registers (miss diagnostics).
struct UnregisteredMany;
/// A payload whose handler exercises `MountCx::realize_detached`.
struct Portal {
    content: RefCell<Option<Element>>,
    grabbed: Rc<RefCell<Option<(u32, Realized<u32>)>>>,
}

fn v(children: Vec<Element>) -> Element {
    item(V, children)
}

fn t(label: &str) -> Element {
    item(T(label.to_string()), vec![])
}

struct Rig {
    world: World,
    backend: Rc<RefCell<TestHost>>,
    registry: Rc<Registry<TestHost>>,
    ops: Rc<RefCell<Vec<String>>>,
    labels: Rc<RefCell<HashMap<u32, String>>>,
    tree: Rc<RefCell<HashMap<u32, Vec<u32>>>>,
}

impl Rig {
    fn new(splice: bool) -> Rig {
        Rig::with_registry(splice, |_| {})
    }

    /// [`Rig::new`] plus extra boot registration/declaration — the seam
    /// the deferred-payload tests need.
    fn with_registry(splice: bool, boot: impl FnOnce(&mut Registry<TestHost>)) -> Rig {
        let ops: Rc<RefCell<Vec<String>>> = Rc::default();
        let labels: Rc<RefCell<HashMap<u32, String>>> = Rc::default();
        let tree: Rc<RefCell<HashMap<u32, Vec<u32>>>> = Rc::default();
        let backend = Rc::new(RefCell::new(TestHost {
            ops: ops.clone(),
            labels: labels.clone(),
            tree: tree.clone(),
            next: 0,
            splice,
        }));
        let mut registry: Registry<TestHost> = Registry::new();
        registry.register::<V, _>(|cx, _p, children| {
            let mut node = cx.backend().borrow_mut().create("view");
            cx.realize_children_into(&mut node, children);
            node
        });
        registry.register::<T, _>(|cx, p, _children| cx.backend().borrow_mut().create(&p.0));
        registry.register::<Portal, _>(|cx, p, _children| {
            let content = p.content.borrow_mut().take().expect("portal content");
            let pair = cx.realize_detached(content);
            *p.grabbed.borrow_mut() = Some(pair);
            cx.backend().borrow_mut().create("portal-host")
        });
        // Multi-node payload: `count` leaf rows realized in place
        // (ambient-collector-owned) and attached with ONE insert_many —
        // the vocabulary repeat handler's fallback shape.
        registry.register_many::<Rep, _>(|cx, p, parent| {
            let mut lives = Vec::with_capacity(p.0);
            let mut nodes = Vec::with_capacity(p.0);
            for i in 0..p.0 {
                let live = cx.realize_in_place(t(&format!("row{i}")));
                nodes.extend(live.collect_nodes());
                lives.push(live);
            }
            let count = nodes.len();
            cx.backend().borrow_mut().insert_many(parent, nodes);
            (LiveNode::Fragment(lives), count)
        });
        boot(&mut registry);
        Rig {
            world: World::new(),
            backend,
            registry: Rc::new(registry),
            ops,
            labels,
            tree,
        }
    }

    fn realize(&self, element: Element) -> Realized<u32> {
        self.world
            .enter(|| realize(&self.backend, &self.registry, element))
    }

    /// The live tree under `root`, as nested `label[children]` text. Two
    /// rigs that produce equal strings produced equal trees, regardless of
    /// which ops got them there.
    fn dom(&self, root: u32) -> String {
        fn walk(
            labels: &HashMap<u32, String>,
            tree: &HashMap<u32, Vec<u32>>,
            node: u32,
            out: &mut String,
        ) {
            out.push_str(&labels[&node]);
            let kids = tree.get(&node).map(|k| k.as_slice()).unwrap_or(&[]);
            if kids.is_empty() {
                return;
            }
            out.push('[');
            for (i, kid) in kids.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                walk(labels, tree, *kid, out);
            }
            out.push(']');
        }
        let mut out = String::new();
        walk(&self.labels.borrow(), &self.tree.borrow(), root, &mut out);
        out
    }

    /// Drain the ops recorded since the last call.
    fn take_ops(&self) -> Vec<String> {
        std::mem::take(&mut self.ops.borrow_mut())
    }

    fn flush(&self) {
        self.world.flush();
    }
}

fn counter() -> Rc<Cell<usize>> {
    Rc::new(Cell::new(0))
}

/// Register a probe effect in the CURRENT build scope whose cleanup bumps
/// `drops` — makes scope teardown observable (idea-lite's
/// `label_with_drop_probe`).
fn drop_probe(drops: &Rc<Cell<usize>>) {
    let drops = drops.clone();
    effect(move || {
        let drops = drops.clone();
        move || drops.set(drops.get() + 1)
    });
}

/// Register a probe effect whose cleanup appends `cleanup <label>` to the
/// shared op log — pins dispose ORDERING relative to structural ops.
fn cleanup_marker(ops: &Rc<RefCell<Vec<String>>>, label: &str) {
    let ops = ops.clone();
    let label = label.to_string();
    effect(move || {
        let ops = ops.clone();
        let label = label.clone();
        move || ops.borrow_mut().push(format!("cleanup {label}"))
    });
}

/// Flatten the live tree into visible labels, in order — idea-lite's
/// `render()` walk, following holes and keyed rows through their slots.
/// Anchors are structural, not content, and are skipped.
fn render_labels(rig: &Rig, realized: &Realized<u32>) -> Vec<String> {
    fn walk(labels: &HashMap<u32, String>, node: &LiveNode<u32>, out: &mut Vec<String>) {
        match node {
            LiveNode::Item { node, children } => {
                out.push(labels[node].clone());
                for child in children {
                    walk(labels, child, out);
                }
            }
            LiveNode::Fragment(children) => {
                for child in children {
                    walk(labels, child, out);
                }
            }
            LiveNode::Dyn(dyn_live) => dyn_live.with_current(|current| {
                if let Some(realized) = current {
                    walk(labels, &realized.root, out);
                }
            }),
            LiveNode::Keyed(keyed) => match keyed {
                KeyedLive::Anchored { slot, .. } => {
                    if let Some(realized) = slot.borrow().as_ref() {
                        walk(labels, &realized.root, out);
                    }
                }
                KeyedLive::Spliced { state } => {
                    for entry in &state.borrow().rows {
                        walk(labels, &entry.realized.root, out);
                    }
                }
            },
            // Parked contributes nothing visible (its placeholder is an
            // anchor, and anchors are structure, not content); drained
            // walks the subtree the late handler produced.
            LiveNode::Deferred(deferred) => deferred.with_current(|current| {
                if let Some(realized) = current {
                    walk(labels, &realized.root, out);
                }
            }),
        }
    }
    let mut out = Vec::new();
    walk(&rig.labels.borrow(), &realized.root, &mut out);
    out
}

// ============================================================================
// idea-lite ports (tests 28–33)
// ============================================================================

#[test]
fn key_conversions() {
    // idea-lite test 33, deferred from P0 to land with `Key`.
    assert_eq!(Key::from(3i32), Key::Int(3));
    assert_eq!(Key::from(-7i64), Key::Int(-7));
    assert_eq!(Key::from(3u32), Key::UInt(3));
    assert_eq!(Key::from(3usize), Key::UInt(3));
    assert_eq!(Key::from("a"), Key::Str("a".to_string()));
    assert_eq!(Key::from(String::from("b")), Key::Str("b".to_string()));
}

#[test]
fn realize_walks_structure() {
    let rig = Rig::new(true);
    let shell = rig.realize(v(vec![t("a"), fragment(vec![t("b"), t("c")])]));
    assert_eq!(render_labels(&rig, &shell), ["view", "a", "b", "c"]);
    // The walk emitted real structural ops: fragment children splice flat
    // into the view as ordinary inserts.
    assert_eq!(
        rig.take_ops(),
        [
            "create n0 view",
            "create n1 a",
            "insert n0 <- n1",
            "create n2 b",
            "insert n0 <- n2",
            "create n3 c",
            "insert n0 <- n3",
        ]
    );
}

#[test]
fn dyn_holes_swap_and_tear_down() {
    let rig = Rig::new(true);
    let drops = counter();
    let s = rig.world.enter(|| signal(true));
    let drops_b = drops.clone();
    let shell = rig.realize(v(vec![dyn_keyed(
        move || s.get(),
        move |&on| {
            if on {
                drop_probe(&drops_b);
                t("on")
            } else {
                t("off")
            }
        },
    )]));
    assert_eq!(render_labels(&rig, &shell), ["view", "on"]);
    assert_eq!(drops.get(), 0);

    s.set(false);
    rig.flush();
    assert_eq!(render_labels(&rig, &shell), ["view", "off"]);
    assert_eq!(drops.get(), 1, "the replaced subtree's effects were dropped");
}

#[test]
fn dropping_the_realized_tree_retires_its_effects() {
    let rig = Rig::new(true);
    let runs = counter();
    let s = rig.world.enter(|| signal(0));
    let shell = rig.world.enter(|| {
        let runs = runs.clone();
        // A component boundary: the body's effect rides in Element::Owned
        // and is folded into the Realized at mount.
        let element = component_scope(move || {
            effect(move || {
                let _ = s.get();
                runs.set(runs.get() + 1);
            });
            t("x")
        });
        realize(&rig.backend, &rig.registry, element)
    });
    s.set(1);
    rig.flush();
    assert_eq!(runs.get(), 2);
    drop(shell);
    s.set(2);
    rig.flush();
    assert_eq!(runs.get(), 2, "unmounting is dropping");
}

#[test]
fn keyed_lists_reconcile_by_identity() {
    let rig = Rig::new(true);
    let renders = counter();
    let drops = counter();
    let list = rig.world.enter(|| signal(vec![1i32, 2]));
    let renders_b = renders.clone();
    let drops_b = drops.clone();
    let shell = rig.realize(v(vec![keyed(
        move || list.get(),
        |n| *n,
        move |n: i32| {
            renders_b.set(renders_b.get() + 1);
            drop_probe(&drops_b);
            t(&format!("r{n}"))
        },
    )]));
    assert_eq!(render_labels(&rig, &shell), ["view", "r1", "r2"]);
    assert_eq!((renders.get(), drops.get()), (2, 0));

    list.set(vec![2, 1, 3]); // reorder the kept rows, insert one
    rig.flush();
    assert_eq!(render_labels(&rig, &shell), ["view", "r2", "r1", "r3"]);
    assert_eq!(
        (renders.get(), drops.get()),
        (3, 0),
        "kept keys keep their subtrees: one render, zero drops"
    );

    list.set(vec![3]);
    rig.flush();
    assert_eq!(render_labels(&rig, &shell), ["view", "r3"]);
    assert_eq!((renders.get(), drops.get()), (3, 2));

    drop(shell);
    assert_eq!(drops.get(), 3, "unrealizing the list unmounts the remaining rows");
}

#[test]
#[should_panic(expected = "duplicate key")]
fn duplicate_keys_panic() {
    let rig = Rig::new(true);
    let _shell = rig.realize(v(vec![keyed(
        || vec![1, 1],
        |n| *n,
        |n: i32| t(&format!("r{n}")),
    )]));
}

// ============================================================================
// Driver invariants (unit-level assertions of the golden-pinned behaviors)
// ============================================================================

#[test]
fn spliced_dispose_order_removes_nodes_before_scope_drop() {
    // The spliced ordering pin (dispose_order_when.spliced.golden):
    // remove_child FIRST, THEN the old scope's cleanup.
    let rig = Rig::new(true);
    let s = rig.world.enter(|| signal(true));
    let ops = rig.ops.clone();
    let _shell = rig.realize(v(vec![dyn_keyed(
        move || s.get(),
        move |&on| {
            cleanup_marker(&ops, if on { "then" } else { "else" });
            if on {
                t("shown")
            } else {
                t("hidden")
            }
        },
    )]));
    rig.take_ops();

    s.set(false);
    rig.flush();
    assert_eq!(
        rig.take_ops(),
        [
            "remove_child n0 -x n1",
            "cleanup then",
            "create n2 hidden",
            "insert_at n0 <- n2 @ 0",
        ]
    );
}

#[test]
fn anchored_dispose_order_drops_scope_before_clear() {
    // The anchored ordering pin (dispose_order_when.anchored.golden):
    // cleanup FIRST, THEN clear_children — the OPPOSITE of spliced.
    let rig = Rig::new(false);
    let s = rig.world.enter(|| signal(true));
    let ops = rig.ops.clone();
    let _shell = rig.realize(v(vec![dyn_keyed(
        move || s.get(),
        move |&on| {
            cleanup_marker(&ops, if on { "then" } else { "else" });
            if on {
                t("shown")
            } else {
                t("hidden")
            }
        },
    )]));
    // Mount: no first-fire clear_children on the virgin anchor (sanctioned
    // divergence #2 — the old walker emitted a no-op clear here).
    assert_eq!(
        rig.take_ops(),
        [
            "create n0 view",
            "create n1 anchor",
            "create n2 shown",
            "insert n1 <- n2",
            "insert n0 <- n1",
        ]
    );

    s.set(false);
    rig.flush();
    assert_eq!(
        rig.take_ops(),
        [
            "cleanup then",
            "clear_children n1",
            "create n3 hidden",
            "insert n1 <- n3",
        ]
    );
}

#[test]
fn guarded_dyn_skips_rebuild_when_key_unchanged() {
    // The last_active dedup pin (when_dedup_extra_signal.*.golden): a
    // select() that reads extra signals must not rebuild while its guard
    // value is unchanged.
    let rig = Rig::new(true);
    let (show, version) = rig.world.enter(|| (signal(true), signal(0)));
    let _shell = rig.realize(v(vec![dyn_keyed(
        move || {
            let _tick = version.get();
            show.get()
        },
        |&on| if on { t("on") } else { t("off") },
    )]));
    rig.take_ops();

    version.set(1);
    rig.flush();
    assert!(
        rig.take_ops().is_empty(),
        "guard value unchanged — zero structural ops"
    );

    show.set(false);
    rig.flush();
    assert!(!rig.take_ops().is_empty(), "the driver effect is still live");
}

#[test]
fn plain_dyn_rebuilds_on_every_dependency_change() {
    // The dynamic_swap pin: no guard exists on dyn_element — the closure
    // IS the dependency source, so each fire rebuilds even when the output
    // would be identical.
    let rig = Rig::new(false);
    let s = rig.world.enter(|| signal(0));
    let _shell = rig.realize(v(vec![dyn_element(move || {
        let _ = s.get();
        t("same")
    })]));
    rig.take_ops();

    s.set(1);
    rig.flush();
    assert_eq!(
        rig.take_ops(),
        [
            "clear_children n1",
            "create n3 same",
            "insert n1 <- n3",
        ],
        "identical output still tears down and rebuilds"
    );
}

#[test]
fn retire_hook_intercepts_swapped_out_realized() {
    // The retire hook (plan §4, the presence contract): the swapped-out
    // subtree is handed to the hook as a `Retired` with its nodes STILL
    // ATTACHED — the driver emits NO remove_child/clear_children for it.
    // The hook owns both detachment and drop timing (an exit animation
    // on an already-detached node would be invisible, which is the bug
    // this contract prevents).
    let rig = Rig::new(true);
    let drops = counter();
    let s = rig.world.enter(|| signal(true));
    let retired: Rc<RefCell<Option<Box<dyn Any>>>> = Rc::default();
    let retired_hook = retired.clone();
    let drops_b = drops.clone();
    let _shell = rig.realize(v(vec![dyn_keyed(
        move || s.get(),
        move |&on| {
            if on {
                drop_probe(&drops_b);
            }
            t(if on { "on" } else { "off" })
        },
    )
    .with_retire(move |old| {
        *retired_hook.borrow_mut() = Some(old);
    })]));
    rig.take_ops();

    s.set(false);
    rig.flush();
    // The incoming branch mounted, but the outgoing node was NOT removed
    // — detachment now belongs to the hook.
    assert_eq!(
        rig.take_ops(),
        ["create n2 off", "insert_at n0 <- n2 @ 0"],
        "driver must not detach a retired subtree's nodes"
    );
    // …and the outgoing subtree's scope is alive in the hook's hands.
    assert_eq!(drops.get(), 0, "retired scope not dropped by the driver");
    let held = retired
        .borrow_mut()
        .take()
        .expect("hook received the outgoing subtree");
    let held = held
        .downcast::<Retired<u32>>()
        .expect("retired payload downcasts to Retired<Node>");
    let Retired {
        realized,
        parent,
        nodes,
    } = *held;
    assert_eq!(parent, 0, "retired parent is the node the old subtree hangs off");
    assert_eq!(nodes, [1], "the retired package carries the attached nodes");
    // The hook's teardown: detach first, then drop the scope (the
    // spliced dispose rule, now the hook's responsibility).
    rig.backend.borrow_mut().remove_child(&parent, &nodes[0]);
    assert_eq!(rig.take_ops(), ["remove_child n0 -x n1"]);
    drop(realized);
    assert_eq!(drops.get(), 1, "dropping the retired Realized is the teardown");
}

#[test]
fn retire_hook_anchored_skips_clear_children() {
    // Anchored counterpart: the driver normally clears the anchor after
    // dropping the old branch; with a retire hook the anchor is NOT
    // cleared (the exiting node must keep rendering), and the incoming
    // branch mounts alongside until the hook detaches the old one.
    let rig = Rig::new(false);
    let s = rig.world.enter(|| signal(true));
    let retired: Rc<RefCell<Option<Box<dyn Any>>>> = Rc::default();
    let retired_hook = retired.clone();
    let _shell = rig.realize(v(vec![dyn_keyed(
        move || s.get(),
        |&on| t(if on { "on" } else { "off" }),
    )
    .with_retire(move |old| {
        *retired_hook.borrow_mut() = Some(old);
    })]));
    rig.take_ops();

    s.set(false);
    rig.flush();
    assert_eq!(
        rig.take_ops(),
        ["create n3 off", "insert n1 <- n3"],
        "anchored driver must not clear_children a retired subtree's anchor"
    );
    let held = retired
        .borrow_mut()
        .take()
        .expect("hook received the outgoing subtree")
        .downcast::<Retired<u32>>()
        .expect("retired payload downcasts to Retired<Node>");
    assert_eq!(held.parent, 1, "retired parent is the driver's anchor");
    assert_eq!(held.nodes, [2]);
}

#[test]
fn threaded_base_index_counts_through_variable_size_fragments() {
    // The fragment_base_index pin, with two fragments of different sizes:
    // the threaded `inserted` counter runs THROUGH both, so the hole after
    // them captures the absolute base index — and every re-splice lands
    // there.
    let rig = Rig::new(true);
    let show = rig.world.enter(|| signal(true));
    let _shell = rig.realize(v(vec![
        t("s0"),
        fragment(vec![t("f1")]),
        fragment(vec![t("f2"), t("f3"), t("f4")]),
        dyn_keyed(move || show.get(), |&on| t(if on { "cond-on" } else { "cond-off" })),
        t("tail"),
    ]));
    let mount = rig.take_ops();
    assert!(
        mount.contains(&"insert_at n0 <- n6 @ 5".to_string()),
        "initial splice at base 5 (1 static + 1 + 3 fragment children): {mount:?}"
    );

    show.set(false);
    rig.flush();
    assert_eq!(
        rig.take_ops(),
        [
            "remove_child n0 -x n6",
            "create n8 cond-off",
            "insert_at n0 <- n8 @ 5",
        ],
        "re-splice lands at the same absolute base"
    );
}

#[test]
fn survivors_do_not_move_on_mid_list_insert() {
    // The monotonic-survivor pin (each_insert_middle_survivors): survivor
    // old positions stay strictly increasing → reorder=false → ONLY the
    // new row emits an insert_at; the backend displaces the rest.
    let rig = Rig::new(true);
    let list = rig.world.enter(|| signal(vec![1, 2, 4]));
    let _shell = rig.realize(v(vec![keyed(
        move || list.get(),
        |n| *n,
        |n: i32| t(&format!("r{n}")),
    )]));
    rig.take_ops();

    list.set(vec![1, 2, 3, 4]);
    rig.flush();
    assert_eq!(
        rig.take_ops(),
        ["create n4 r3", "insert_at n0 <- n4 @ 2"],
        "one build + one insert_at; survivors untouched"
    );
}

#[test]
fn real_reorder_repositions_every_row_in_target_order() {
    // The each_reverse pin: non-monotonic survivors → every node
    // repositioned via insert_at in target order, nothing built/removed.
    let rig = Rig::new(true);
    let list = rig.world.enter(|| signal(vec![1, 2, 3]));
    let _shell = rig.realize(v(vec![keyed(
        move || list.get(),
        |n| *n,
        |n: i32| t(&format!("r{n}")),
    )]));
    rig.take_ops();

    list.set(vec![3, 2, 1]);
    rig.flush();
    assert_eq!(
        rig.take_ops(),
        [
            "insert_at n0 <- n3 @ 0",
            "insert_at n0 <- n2 @ 1",
            "insert_at n0 <- n1 @ 2",
        ]
    );
}

#[test]
fn keyed_unmount_removes_nodes_before_row_scope_drop() {
    // The dispose_order_each pin at unit level.
    let rig = Rig::new(true);
    let list = rig.world.enter(|| signal(vec![1, 2, 3]));
    let ops = rig.ops.clone();
    let _shell = rig.realize(v(vec![keyed(
        move || list.get(),
        |n| *n,
        move |n: i32| {
            cleanup_marker(&ops, &format!("row-{n}"));
            t(&format!("r{n}"))
        },
    )]));
    rig.take_ops();

    list.set(vec![1, 3]);
    rig.flush();
    assert_eq!(
        rig.take_ops(),
        ["remove_child n0 -x n2", "cleanup row-2"],
        "nodes out BEFORE the row scope drops"
    );
}

#[test]
fn collect_nodes_accounts_fragment_rows() {
    // Multi-node rows (each_multi_node_rows): the per-row nodes vec — not
    // one node — is the accounting unit, and collect_nodes sees the
    // current list order through the keyed slot.
    let rig = Rig::new(true);
    let list = rig.world.enter(|| signal(vec![1, 2]));
    let shell = rig.realize(v(vec![keyed(
        move || list.get(),
        |n| *n,
        |n: i32| fragment(vec![t(&format!("r{n}-a")), t(&format!("r{n}-b"))]),
    )]));
    let keyed_live = match &shell.root {
        LiveNode::Item { children, .. } => &children[0],
        _ => panic!("root is the view item"),
    };
    assert_eq!(keyed_live.collect_nodes(), [1, 2, 3, 4], "two nodes per row");
    rig.take_ops();

    list.set(vec![2, 1]);
    rig.flush();
    assert_eq!(
        rig.take_ops(),
        [
            "insert_at n0 <- n3 @ 0",
            "insert_at n0 <- n4 @ 1",
            "insert_at n0 <- n1 @ 2",
            "insert_at n0 <- n2 @ 3",
        ],
        "reorder repositions every node of every row"
    );
    assert_eq!(
        keyed_live.collect_nodes(),
        [3, 4, 1, 2],
        "collect_nodes reflects the new order"
    );
}

#[test]
fn anchored_keyed_fallback_fully_rebuilds() {
    // The each_append.anchored pin: without splice support the list is a
    // full rebuild — clear + recreate every row (per-row state lost by
    // contract).
    let rig = Rig::new(false);
    let list = rig.world.enter(|| signal(vec![1, 2]));
    let _shell = rig.realize(v(vec![keyed(
        move || list.get(),
        |n| *n,
        |n: i32| t(&format!("r{n}")),
    )]));
    assert_eq!(
        rig.take_ops(),
        [
            "create n0 view",
            "create n1 anchor",
            "create n2 r1",
            "insert n1 <- n2",
            "create n3 r2",
            "insert n1 <- n3",
            "insert n0 <- n1",
        ],
        "mount builds rows into the virgin anchor without a first-fire clear"
    );

    list.set(vec![1, 2, 3]);
    rig.flush();
    assert_eq!(
        rig.take_ops(),
        [
            "clear_children n1",
            "create n4 r1",
            "insert n1 <- n4",
            "create n5 r2",
            "insert n1 <- n5",
            "create n6 r3",
            "insert n1 <- n6",
        ],
        "append rebuilds every row under the anchor"
    );
}

#[test]
fn nested_dyn_in_keyed_row_rebuilds_in_place_and_dies_with_the_row() {
    // The nested_when_in_each_row pin at unit level: a hole inside a kept
    // row swaps in place (no row remount); a removed row's hole dies with
    // the row scope and never fires again.
    let rig = Rig::new(true);
    let (list, flag) = rig.world.enter(|| (signal(vec![1, 2]), signal(true)));
    let row = move |n: i32| {
        v(vec![
            t(&format!("row-{n}")),
            dyn_keyed(move || flag.get(), |&on| t(if on { "inner-on" } else { "inner-off" })),
        ])
    };
    let _shell = rig.realize(v(vec![keyed(move || list.get(), |n| *n, row)]));
    rig.take_ops();

    flag.set(false);
    rig.flush();
    assert_eq!(
        rig.take_ops(),
        [
            "remove_child n1 -x n3",
            "create n7 inner-off",
            "insert_at n1 <- n7 @ 1",
            "remove_child n4 -x n6",
            "create n8 inner-off",
            "insert_at n4 <- n8 @ 1",
        ],
        "both rows rebuild ONLY their hole, in creation order, in place"
    );

    list.set(vec![2]);
    rig.flush();
    assert_eq!(
        rig.take_ops(),
        ["remove_child n0 -x n1"],
        "the removed row detaches as one node; its hole tears down silently"
    );

    flag.set(true);
    rig.flush();
    assert_eq!(
        rig.take_ops(),
        [
            "remove_child n4 -x n8",
            "create n9 inner-on",
            "insert_at n4 <- n9 @ 1",
        ],
        "only the surviving row's hole fires"
    );
}

#[test]
fn dyn_as_row_root_anchors_even_on_splice_hosts() {
    // A hole at a subtree ROOT (here: the entire keyed row body) has no
    // parent to splice into at build time, so it takes the anchored path
    // even on a splice-capable host — matching the old walker, where only
    // children-list positions splice.
    let rig = Rig::new(true);
    let (list, flag) = rig.world.enter(|| (signal(vec![1]), signal(true)));
    let _shell = rig.realize(v(vec![keyed(
        move || list.get(),
        |n| *n,
        move |_n: i32| dyn_keyed(move || flag.get(), |&on| t(if on { "on" } else { "off" })),
    )]));
    assert_eq!(
        rig.take_ops(),
        [
            "create n0 view",
            "create n1 anchor",
            "create n2 on",
            "insert n1 <- n2",
            "insert_at n0 <- n1 @ 0",
        ],
        "the row contributes its anchor; the hole lives under it"
    );

    flag.set(false);
    rig.flush();
    assert_eq!(
        rig.take_ops(),
        ["clear_children n1", "create n3 off", "insert n1 <- n3"],
        "swaps happen under the row's anchor"
    );
}

// ============================================================================
// Registry + detached realization
// ============================================================================

#[test]
#[should_panic(expected = "no handler registered")]
fn unregistered_payload_panics_loudly() {
    let rig = Rig::new(true);
    let _ = rig.realize(item(Unregistered, vec![]));
}

#[test]
fn double_registration_returns_previous_handler() {
    let mut registry: Registry<TestHost> = Registry::new();
    assert!(registry
        .register::<T, _>(|cx, p, _| cx.backend().borrow_mut().create(&p.0))
        .is_none());
    assert!(
        registry
            .register::<T, _>(|cx, p, _| cx.backend().borrow_mut().create(&p.0))
            .is_some(),
        "re-registering the same payload type surfaces the old handler"
    );
    assert!(registry.has::<T>());
    assert!(!registry.has::<V>());
}

#[test]
fn realize_detached_returns_single_root_and_owning_realized() {
    let rig = Rig::new(true);
    let grabbed: Rc<RefCell<Option<(u32, Realized<u32>)>>> = Rc::default();
    let runs = counter();
    let s = rig.world.enter(|| signal(0));
    let runs_b = runs.clone();
    // component_scope creates the effect at ELEMENT BUILD time, so the
    // element must be constructed with the world ambient.
    let portal = rig.world.enter(|| Portal {
        content: RefCell::new(Some(component_scope(move || {
            effect(move || {
                let _ = s.get();
                runs_b.set(runs_b.get() + 1);
            });
            t("screen")
        }))),
        grabbed: grabbed.clone(),
    });
    let _shell = rig.realize(item(portal, vec![]));
    let (node, detached) = grabbed.borrow_mut().take().expect("handler grabbed the pair");
    assert_eq!(rig.labels.borrow()[&node], "screen");
    // The detached Realized owns the screen's reactive scope…
    s.set(1);
    rig.flush();
    assert_eq!(runs.get(), 2);
    // …and dropping it (release_screen) is the teardown.
    drop(detached);
    s.set(2);
    rig.flush();
    assert_eq!(runs.get(), 2, "released screens stop reacting");
}

// ============================================================================
// Many — the multi-node primitive (static-repeat seam)
// ============================================================================

/// A Many in a children list mounts its siblings through the registered
/// many-handler and advances the splice counter by its node count, so a
/// reactive region AFTER it captures the correct absolute base index.
#[test]
fn many_counts_toward_following_region_base_index() {
    let rig = Rig::new(true);
    let s = rig.world.enter(|| signal(0));
    let realized = rig.realize(v(vec![
        many(Rep(2)),
        dyn_keyed(move || s.get(), |_| t("tail")),
    ]));
    assert_eq!(
        rig.take_ops(),
        vec![
            "create n0 view",
            "create n1 row0",
            "create n2 row1",
            "insert_many n0 <- [n1, n2]",
            // The spliced hole after the Many bases at 2, not 0.
            "create n3 tail",
            "insert_at n0 <- n3 @ 2",
        ],
        "many contributes its count to the threaded inserted counter"
    );
    drop(realized);
}

/// In-place rows are owned by the ENCLOSING subtree: dropping the
/// Realized retires their effects/cleanups like per-node children.
#[test]
fn many_rows_die_with_the_enclosing_realized() {
    let rig = Rig::new(true);
    let (realized, ran) = rig.world.enter(|| {
        let ran = counter();
        let ran_c = ran.clone();
        let realized = realize(
            &rig.backend,
            &rig.registry,
            v(vec![many(Rep(1)), {
                // A probe effect collected by the same realization.
                let _ = effect(move || {
                    ran_c.set(ran_c.get() + 1);
                });
                t("static")
            }]),
        );
        (realized, ran)
    });
    assert_eq!(ran.get(), 1);
    drop(realized);
    // Owned dropped — nothing left alive to re-fire (smoke: no panic on
    // world drop; the row LiveNodes were folded into the same tree).
}

/// A Many with no registered many-handler is a loud panic naming the
/// registration seam.
#[test]
#[should_panic(expected = "no MANY handler registered")]
fn many_without_handler_panics() {
    let rig = Rig::new(true);
    let _ = rig.realize(v(vec![many(UnregisteredMany)]));
}

/// A Many as a detached subtree root mirrors the old walker's
/// `Element::Repeat` standalone-root panic.
#[test]
#[should_panic(expected = "standalone subtree root")]
fn many_as_detached_root_panics() {
    let rig = Rig::new(true);
    let _ = rig.realize(many(Rep(2)));
}

// ============================================================================
// Recursion depth budget (deletion-baseline §4.1 #9)
// ============================================================================
//
// The old walker's only guarantee against deep trees was a per-call frame
// budget, pinned by `runtime-core/tests/walker/stack_depth.rs` on a
// thread constrained to wasm-ld's 1 MiB default stack. That test dies
// with the walker; these two are its successors, and the second adds the
// named cap the old core never had (see `realize::depth`).

/// Build `depth` levels of nested `view(...)` around a leaf `text`,
/// realize them, and tear the tree back down — all on a thread whose
/// stack is constrained to `stack` bytes. Returns normally or aborts the
/// process (a stack overflow is not a catchable panic).
fn realize_nested_on_constrained_stack(depth: usize, stack: usize) {
    let handle = std::thread::Builder::new()
        .name("scene-stack-depth-regression".into())
        .stack_size(stack)
        .spawn(move || {
            // Tree CONSTRUCTION is recursive-free (a loop), but the
            // realize walk and the `Realized`/`Element` drops are all
            // recursive, so every phase runs inside the constrained
            // thread.
            let mut tree: Element = t("leaf");
            for _ in 0..depth {
                tree = v(vec![tree]);
            }
            let rig = Rig::new(true);
            let realized = rig.realize(tree);
            let mut expected: Vec<String> = vec!["view".to_string(); depth];
            expected.push("leaf".to_string());
            assert_eq!(
                render_labels(&rig, &realized),
                expected,
                "every level must have mounted"
            );
            drop(realized);
        })
        .expect("spawn constrained-stack test thread");
    handle
        .join()
        .expect("realize thread aborted — likely a per-level stack-frame regression");
}

/// The direct successor to the old walker's `stack_depth.rs`: 30 levels
/// of nesting realize on a wasm-sized 1 MiB stack.
///
/// Same depth and same stack size as the old test, deliberately, so the
/// old core's guarantee is provably carried over rather than merely
/// re-stated. The old walker needed ~20 KiB per level (~600 KiB at this
/// depth, under 1 MiB with ~40% headroom); this core needs ~1.1 KiB.
#[test]
fn deep_nested_items_realize_within_wasm_stack_budget() {
    const WASM_DEFAULT_STACK: usize = 1024 * 1024;
    const DEPTH: usize = 30;
    realize_nested_on_constrained_stack(DEPTH, WASM_DEFAULT_STACK);
}

/// The trend safeguard, at the depth the measured per-level cost
/// actually supports.
///
/// Measured on this tree: ~1.1 KiB of stack per nesting level (1 MiB
/// carries ~900 levels; 1000 aborts). 400 levels is ~440 KiB — a ~2.4×
/// margin, so this passes comfortably today and goes red if one level
/// ever costs more than ~2.6 KiB. Native code-gen is not wasm code-gen,
/// but the two track closely enough for this to be the early warning the
/// old core got from its own 30-level test.
#[test]
fn realize_per_level_stack_cost_stays_within_its_measured_budget() {
    const WASM_DEFAULT_STACK: usize = 1024 * 1024;
    const DEPTH: usize = 400;
    realize_nested_on_constrained_stack(DEPTH, WASM_DEFAULT_STACK);
}

/// Past the cap, realize reports itself instead of overflowing.
///
/// This is the half the old core did NOT have: it had no cap at all, so
/// an unbounded-recursion component body produced a stack overflow (an
/// opaque `memory access out of bounds` on wasm). The panic message names
/// the likely cause.
#[test]
#[should_panic(expected = "nests more than 512 levels deep")]
fn realize_past_the_depth_cap_panics_by_name_instead_of_overflowing() {
    let mut tree: Element = t("leaf");
    for _ in 0..(realize::MAX_DEPTH + 1) {
        tree = v(vec![tree]);
    }
    let rig = Rig::new(true);
    let _ = rig.realize(tree);
}

/// The depth counter is RAII: a completed realize leaves it at zero, and
/// so does one that panicked partway down. Without this, a single deep
/// tree (or any handler panic) would poison every later realize on the
/// thread with a permanently-inflated depth.
#[test]
fn depth_counter_unwinds_to_zero_after_success_and_after_a_panic() {
    let rig = Rig::new(true);
    let realized = rig.realize(v(vec![v(vec![t("a")])]));
    assert_eq!(realize::depth_for_test(), 0, "counter must unwind on success");
    drop(realized);

    // A handler panic mid-walk (the unregistered-payload diagnostic)
    // unwinds through the guards.
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = rig.realize(v(vec![v(vec![item(Unregistered, vec![])])]));
    }));
    assert!(caught.is_err(), "expected the no-handler panic");
    assert_eq!(realize::depth_for_test(), 0, "counter must unwind on panic");

    // And the thread is still usable for a normal realize.
    let after = rig.realize(v(vec![t("b")]));
    assert_eq!(render_labels(&rig, &after), vec!["view", "b"]);
}

// ============================================================================
// Deferred (late-bound) payloads — the code-splitting seam
// ============================================================================
//
// What these cover, and why the shape is what it is: an SDK whose mount
// handler ships in a separately loaded wasm chunk cannot register at boot
// without anchoring its whole dependency graph in `main.wasm`. So realize
// must be able to meet a payload whose handler does not exist yet, hold
// its place, and finish the mount later. The bundle-size half of the
// capability is measured for real by `tests/lazy-payload-split`; these
// tests pin the semantics that measurement depends on.

/// A late-bound payload: `Registry::defer::<Heavy>()` at boot, handler
/// later. Carries a label so the drained node is identifiable.
struct Heavy(&'static str);
/// A second late-bound kind, for per-kind drain isolation.
struct Heavy2;

/// The handler `Heavy` eventually gets — deliberately identical whether it
/// arrives at boot or late, so "same handler, different arrival time" is
/// the only variable the identity test changes.
fn mount_heavy(
    cx: &mut MountCx<'_, TestHost>,
    p: &Rc<Heavy>,
    children: Vec<Element>,
) -> u32 {
    let mut node = cx.backend().borrow_mut().create(p.0);
    cx.realize_children_into(&mut node, children);
    node
}

fn heavy(label: &'static str, children: Vec<Element>) -> Element {
    item(Heavy(label), children)
}

/// Guarantee #1: a late handler realizes every parked item of its kind, in
/// document order, at the exact position it reserved — between its real
/// siblings, not appended at the end.
#[test]
fn late_registration_realizes_parked_nodes_in_document_order_and_in_place() {
    let rig = Rig::with_registry(true, |r| r.defer::<Heavy>());
    let shell = rig.realize(v(vec![
        t("before"),
        heavy("H1", vec![t("h1-kid")]),
        t("middle"),
        heavy("H2", vec![]),
        t("after"),
    ]));
    assert_eq!(rig.registry.parked_count::<Heavy>(), 2);
    // Parked: two placeholders hold the two slots, so the parent already
    // has its final child COUNT and every sibling its final index.
    assert_eq!(rig.dom(0), "view[before anchor middle anchor after]");
    assert_eq!(render_labels(&rig, &shell), ["view", "before", "middle", "after"]);

    let _ = rig.take_ops(); // isolate the DRAIN's ops below
    let drained = rig.registry.register_deferred::<Heavy, _>(mount_heavy);
    assert_eq!(drained, 2, "both parked items realized");
    assert_eq!(rig.registry.parked_count::<Heavy>(), 0);

    // Placement: each item landed where its placeholder was, children and
    // all; ordering follows the document, not the registration.
    assert_eq!(rig.dom(0), "view[before H1[h1-kid] middle H2 after]");
    assert_eq!(
        render_labels(&rig, &shell),
        ["view", "before", "H1", "h1-kid", "middle", "H2", "after"]
    );

    // And the drain is a splice, not a remount: the only ops touching the
    // pre-existing siblings are the two placeholder removals.
    let ops = rig.take_ops();
    assert_eq!(
        ops,
        [
            "create n6 H1",
            "create n7 h1-kid",
            "insert n6 <- n7",
            "insert_at n0 <- n6 @ 1",
            "remove_child n0 -x n2",
            "create n8 H2",
            "insert_at n0 <- n8 @ 3",
            "remove_child n0 -x n4",
        ],
        "drain emits one mount + one splice + one placeholder removal per item"
    );
}

/// Guarantee #2 (the one deferral must NOT weaken): a payload kind nobody
/// declared is still a loud panic, even in an app that defers a DIFFERENT
/// kind. Parking is opt-in per kind; a genuine missing registration must
/// keep failing at realize rather than silently rendering nothing.
#[test]
#[should_panic(expected = "no handler registered")]
fn undeclared_unknown_payload_still_panics_in_an_app_that_defers_another_kind() {
    let rig = Rig::with_registry(true, |r| r.defer::<Heavy>());
    let _ = rig.realize(v(vec![heavy("H", vec![]), item(Unregistered, vec![])]));
}

/// Guarantee #3: a handler that arrives BEFORE realize behaves exactly as
/// an ordinary boot handler — no parking, no placeholder, no divergence in
/// the op stream. Pinned against the eagerly-registered op stream directly.
#[test]
fn a_handler_that_arrives_before_realize_mounts_inline_exactly_as_today() {
    let eager = Rig::with_registry(true, |r| {
        r.register::<Heavy, _>(mount_heavy);
    });
    let _ = eager.realize(v(vec![t("before"), heavy("H", vec![t("kid")]), t("after")]));
    let eager_ops = eager.take_ops();

    // Same tree, but the handler arrives through the LATE path — still
    // before realize runs.
    let late = Rig::with_registry(true, |r| r.defer::<Heavy>());
    late.registry.register_deferred::<Heavy, _>(mount_heavy);
    let _ = late.realize(v(vec![t("before"), heavy("H", vec![t("kid")]), t("after")]));

    assert_eq!(
        late.take_ops(),
        eager_ops,
        "an already-installed late handler is indistinguishable from a boot handler"
    );
    assert_eq!(late.registry.parked_count::<Heavy>(), 0, "nothing parked");
}

/// Guarantee #4: parked-then-drained output IS the eagerly-registered
/// output. Asserted against the recorded op streams — both are applied to
/// the same host model, and the resulting trees must be equal.
#[test]
fn parked_then_drained_output_is_identical_to_eagerly_registered_output() {
    fn tree() -> Element {
        v(vec![
            t("before"),
            heavy("H1", vec![t("k1"), v(vec![t("k2")])]),
            v(vec![heavy("H2", vec![t("k3")]), t("nested-after")]),
            t("after"),
        ])
    }

    let eager = Rig::with_registry(true, |r| {
        r.register::<Heavy, _>(mount_heavy);
    });
    let eager_live = eager.realize(tree());

    let deferred = Rig::with_registry(true, |r| r.defer::<Heavy>());
    let deferred_live = deferred.realize(tree());
    assert_eq!(deferred.registry.parked_count::<Heavy>(), 2);
    assert_eq!(
        deferred.registry.register_deferred::<Heavy, _>(mount_heavy),
        2
    );

    // Node ids differ (the deferred run also created placeholders), so
    // compare the LABELLED structure the op streams produced.
    assert_eq!(
        deferred.dom(0),
        eager.dom(0),
        "the drained tree must be byte-identical in structure to the eager one"
    );
    assert_eq!(
        render_labels(&deferred, &deferred_live),
        render_labels(&eager, &eager_live),
        "and the live-node walk must agree too"
    );
    // No placeholder survives the drain.
    let deferred_dom = deferred.dom(0);
    assert!(
        !deferred_dom.contains("anchor"),
        "placeholders are removed on drain, got {deferred_dom}"
    );
}

/// Nested placement: a deferred item inside a reactive hole parks against
/// the hole's own parent (spliced) or anchor (anchored) and drains there.
#[test]
fn deferred_item_inside_a_dyn_hole_realizes_into_the_hole() {
    for splice in [true, false] {
        let rig = Rig::with_registry(splice, |r| r.defer::<Heavy>());
        let show = rig.world.enter(|| signal(true));
        let shell = rig.realize(v(vec![
            t("head"),
            dyn_keyed(
                move || show.get(),
                move |&on| {
                    if on {
                        // A view WRAPS the deferred item: a deferred
                        // payload may not be a region root (see the
                        // subtree-root test below).
                        v(vec![heavy("H", vec![])])
                    } else {
                        t("off")
                    }
                },
            ),
            t("tail"),
        ]));
        assert_eq!(rig.registry.parked_count::<Heavy>(), 1, "splice={splice}");

        assert_eq!(rig.registry.register_deferred::<Heavy, _>(mount_heavy), 1);
        assert_eq!(
            render_labels(&rig, &shell),
            ["view", "head", "view", "H", "tail"],
            "splice={splice}"
        );

        // Swapping the hole tears the drained subtree down with it — the
        // deferred item is owned by the branch, not by the registry.
        show.set(false);
        rig.flush();
        assert_eq!(
            render_labels(&rig, &shell),
            ["view", "head", "off", "tail"],
            "splice={splice}"
        );
    }
}

/// Portal placement: `MountCx::realize_detached` content may contain a
/// deferred item as long as it is not the detached ROOT.
#[test]
fn deferred_item_inside_portal_content_drains_in_place() {
    let rig = Rig::with_registry(true, |r| r.defer::<Heavy>());
    let grabbed: Rc<RefCell<Option<(u32, Realized<u32>)>>> = Rc::default();
    let portal = Portal {
        content: RefCell::new(Some(v(vec![t("screen"), heavy("H", vec![])]))),
        grabbed: grabbed.clone(),
    };
    let _shell = rig.realize(v(vec![item(portal, vec![])]));
    assert_eq!(rig.registry.parked_count::<Heavy>(), 1);

    assert_eq!(rig.registry.register_deferred::<Heavy, _>(mount_heavy), 1);
    let borrowed = grabbed.borrow();
    let (root, realized) = borrowed.as_ref().expect("portal content realized");
    assert_eq!(rig.dom(*root), "view[screen H]");
    assert_eq!(
        render_labels(&rig, realized),
        ["view", "screen", "H"],
        "the detached subtree sees its drained item too"
    );
}

/// A deferred payload as a subtree ROOT is a named panic, not a silent
/// misplacement: the drain places a node with `insert_at(parent, node,
/// index)` and a root has neither, and the enclosing region caches its
/// root node handles. Mirrors the `Element::Many` standalone-root rule.
#[test]
#[should_panic(expected = "appeared as a subtree root")]
fn deferred_payload_as_a_subtree_root_panics_by_name() {
    let rig = Rig::with_registry(true, |r| r.defer::<Heavy>());
    let _ = rig.realize(heavy("H", vec![]));
}

/// Per-kind isolation: registering one deferred kind leaves another's
/// parked items alone.
#[test]
fn draining_one_deferred_kind_leaves_other_kinds_parked() {
    let rig = Rig::with_registry(true, |r| {
        r.defer::<Heavy>();
        r.defer::<Heavy2>();
    });
    let shell = rig.realize(v(vec![heavy("H", vec![]), item(Heavy2, vec![])]));
    assert_eq!(rig.registry.parked_count::<Heavy>(), 1);
    assert_eq!(rig.registry.parked_count::<Heavy2>(), 1);

    assert_eq!(rig.registry.register_deferred::<Heavy, _>(mount_heavy), 1);
    assert_eq!(rig.registry.parked_count::<Heavy2>(), 1, "still waiting");
    assert_eq!(render_labels(&rig, &shell), ["view", "H"]);

    assert_eq!(
        rig.registry
            .register_deferred::<Heavy2, _>(|cx, _p, _c| cx.backend().borrow_mut().create("H2")),
        1
    );
    assert_eq!(render_labels(&rig, &shell), ["view", "H", "H2"]);
    assert_eq!(rig.dom(0), "view[H H2]");
}

/// After the handler lands, a LATER mount of the same kind takes the
/// ordinary inline path — parking is a one-time bridge, not a permanent
/// indirection.
#[test]
fn items_mounted_after_the_drain_take_the_inline_path() {
    let rig = Rig::with_registry(true, |r| r.defer::<Heavy>());
    let _first = rig.realize(v(vec![heavy("H1", vec![])]));
    rig.registry.register_deferred::<Heavy, _>(mount_heavy);
    let _ = rig.take_ops();

    let second = rig.realize(v(vec![heavy("H2", vec![])]));
    assert_eq!(rig.registry.parked_count::<Heavy>(), 0, "nothing parked");
    assert_eq!(render_labels(&rig, &second), ["view", "H2"]);
    let ops = rig.take_ops();
    assert!(
        !ops.iter().any(|op| op.contains("anchor")),
        "no placeholder is created once the handler is resident: {ops:?}"
    );
}

/// A subtree that unmounts while its chunk is still in flight takes its
/// parked items with it: the drain finds a dead weak and skips, rather
/// than realizing into a detached parent.
#[test]
fn parked_items_of_an_unmounted_subtree_are_skipped_by_the_drain() {
    let rig = Rig::with_registry(true, |r| r.defer::<Heavy>());
    let shell = rig.realize(v(vec![heavy("H", vec![])]));
    assert_eq!(rig.registry.parked_count::<Heavy>(), 1);

    drop(shell); // unmount before the handler arrives
    assert_eq!(
        rig.registry.parked_count::<Heavy>(),
        0,
        "a dropped subtree's parked slot is already dead"
    );
    let _ = rig.take_ops();

    assert_eq!(
        rig.registry.register_deferred::<Heavy, _>(mount_heavy),
        0,
        "nothing to realize"
    );
    assert!(rig.take_ops().is_empty(), "and no ops were emitted");
}

/// Declaring a kind that is already registered eagerly is a contradiction
/// and says so.
#[test]
#[should_panic(expected = "already registered as a boot handler")]
fn deferring_an_eagerly_registered_kind_panics() {
    let _ = Rig::with_registry(true, |r| {
        r.register::<Heavy, _>(mount_heavy);
        r.defer::<Heavy>();
    });
}

/// A late registration for a kind nobody declared is a mistake — realize
/// would already have panicked on that payload — so it is rejected rather
/// than silently accepted.
#[test]
#[should_panic(expected = "never declared deferred")]
fn late_registration_for_an_undeclared_kind_panics() {
    let rig = Rig::new(true);
    rig.registry.register_deferred::<Heavy, _>(mount_heavy);
}

/// An SDK that registers eagerly on one platform and late on another can
/// call both paths unconditionally: the boot registration wins and the
/// late one is inert.
#[test]
fn late_registration_is_inert_when_a_boot_handler_already_won() {
    let rig = Rig::with_registry(true, |r| {
        r.register::<Heavy, _>(mount_heavy);
    });
    assert_eq!(
        rig.registry
            .register_deferred::<Heavy, _>(|cx, _p, _c| cx.backend().borrow_mut().create("WRONG")),
        0
    );
    let shell = rig.realize(v(vec![heavy("H", vec![])]));
    assert_eq!(
        render_labels(&rig, &shell),
        ["view", "H"],
        "the boot handler still runs"
    );
}

/// A MANY payload cannot be deferred: a parked `Element::Many` stands for
/// an unknown number of siblings, so it cannot reserve its slice of the
/// parent's index space.
#[test]
#[should_panic(expected = "deferral is single-node only")]
fn deferring_a_many_payload_panics() {
    let _ = Rig::with_registry(true, |r| r.defer::<Rep>());
}

// ---------------------------------------------------------------------------
// The mailbox (`defer_registration`) — the seam a lazy chunk actually uses
// ---------------------------------------------------------------------------

/// A second host type, to prove the mailbox never cross-applies one
/// host's registration to another's registry.
struct OtherHost;
impl Host for OtherHost {
    type Node = ();
    fn insert(&mut self, _parent: &mut (), _child: ()) {}
    fn insert_at(&mut self, _parent: &mut (), _child: (), _index: usize) {}
    fn remove_child(&mut self, _parent: &(), _child: &()) {}
    fn clear_children(&mut self, _node: &()) {}
    fn create_anchor(&mut self) {}
    fn supports_splice(&self) -> bool {
        true
    }
}

/// The end-to-end chunk story: an item parks at first realize; the "chunk"
/// then queues its handler through the mailbox (it has no registry in
/// hand); the very next realization drains the mailbox and the parked item
/// completes — all without the parked item's own tree being re-realized.
#[test]
fn mailbox_registration_drains_at_the_next_realize_and_completes_parked_items() {
    crate::late::clear_pending_registrations();
    let rig = Rig::with_registry(true, |r| r.defer::<Heavy>());
    let shell = rig.realize(v(vec![t("before"), heavy("H", vec![]), t("after")]));
    assert_eq!(rig.registry.parked_count::<Heavy>(), 1);

    // …chunk loads, and its body registers with no registry in hand.
    defer_registration::<TestHost, _>(|registry| {
        registry.register_deferred::<Heavy, _>(mount_heavy);
    });
    assert!(has_pending_registrations());
    // Queuing alone changes nothing — the handler is applied at realize.
    assert_eq!(rig.registry.parked_count::<Heavy>(), 1);

    // The chunk's own element realizing is what drives the drain, exactly
    // as `handlers/lazy.rs` does when it swaps in the loaded body.
    let chunk = rig.realize(v(vec![t("chunk-body")]));
    assert!(!has_pending_registrations(), "mailbox emptied");
    assert_eq!(rig.registry.parked_count::<Heavy>(), 0);

    assert_eq!(
        render_labels(&rig, &shell),
        ["view", "before", "H", "after"],
        "the parked item in the FIRST tree completed"
    );
    assert_eq!(rig.dom(0), "view[before H after]");
    assert_eq!(render_labels(&rig, &chunk), ["view", "chunk-body"]);
}

/// Mailbox entries are keyed by host type: a registration queued for one
/// host survives another host's drain untouched.
#[test]
fn mailbox_only_applies_registrations_queued_for_its_own_host() {
    crate::late::clear_pending_registrations();
    let other_ran = Rc::new(Cell::new(false));
    let flag = other_ran.clone();
    defer_registration::<OtherHost, _>(move |_r| flag.set(true));
    defer_registration::<TestHost, _>(|registry| {
        registry.register_deferred::<Heavy, _>(mount_heavy);
    });

    let rig = Rig::with_registry(true, |r| r.defer::<Heavy>());
    let shell = rig.realize(v(vec![heavy("H", vec![])]));
    assert_eq!(render_labels(&rig, &shell), ["view", "H"]);
    assert!(!other_ran.get(), "the other host's entry did not run");
    assert!(
        has_pending_registrations(),
        "and it is still queued for its own host"
    );

    // It applies when ITS host realizes.
    let other_backend = Rc::new(RefCell::new(OtherHost));
    let other_registry: Rc<Registry<OtherHost>> = Rc::new(Registry::new());
    let n = drain_registrations(&other_registry);
    assert_eq!(n, 1);
    assert!(other_ran.get());
    assert!(!has_pending_registrations());
    drop(other_backend);
    crate::late::clear_pending_registrations();
}

/// The mailbox closure body must not run at queue time — the whole point
/// is that the heavy handler it captures stays unreferenced until the
/// chunk that carries it is live.
#[test]
fn mailbox_closure_body_runs_at_drain_time_not_at_queue_time() {
    crate::late::clear_pending_registrations();
    let ran = Rc::new(Cell::new(false));
    let flag = ran.clone();
    defer_registration::<TestHost, _>(move |registry| {
        flag.set(true);
        registry.register_deferred::<Heavy, _>(mount_heavy);
    });
    assert!(!ran.get(), "closure body must not run at queue time");

    let rig = Rig::with_registry(true, |r| r.defer::<Heavy>());
    let _ = rig.realize(v(vec![t("x")]));
    assert!(ran.get(), "closure body runs at the next realization");
}

/// Counters stay honest: a late handler is NOT counted in the boot
/// handler set, because "not reachable from boot" is exactly what that
/// count measures (`runtime-vocabulary/tests/builtin_surface.rs`).
#[test]
fn late_handlers_are_not_counted_in_the_boot_handler_set() {
    let rig = Rig::with_registry(true, |r| r.defer::<Heavy>());
    let boot = rig.registry.handler_count();
    assert_eq!(rig.registry.late_handler_count(), 0);
    assert_eq!(rig.registry.deferred_kind_count(), 1);

    rig.registry.register_deferred::<Heavy, _>(mount_heavy);
    assert_eq!(
        rig.registry.handler_count(),
        boot,
        "the boot count is the main-module bundle floor and must not move"
    );
    assert_eq!(rig.registry.late_handler_count(), 1);
    assert!(rig.registry.has::<Heavy>(), "but the handler IS resolvable");
}
