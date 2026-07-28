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
struct TestHost {
    ops: Rc<RefCell<Vec<String>>>,
    labels: Rc<RefCell<HashMap<u32, String>>>,
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
    }

    fn insert_at(&mut self, parent: &mut u32, child: u32, index: usize) {
        self.ops
            .borrow_mut()
            .push(format!("insert_at n{parent} <- n{child} @ {index}"));
    }

    fn remove_child(&mut self, parent: &u32, child: &u32) {
        self.ops
            .borrow_mut()
            .push(format!("remove_child n{parent} -x n{child}"));
    }

    fn clear_children(&mut self, node: &u32) {
        self.ops.borrow_mut().push(format!("clear_children n{node}"));
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
}

impl Rig {
    fn new(splice: bool) -> Rig {
        let ops: Rc<RefCell<Vec<String>>> = Rc::default();
        let labels: Rc<RefCell<HashMap<u32, String>>> = Rc::default();
        let backend = Rc::new(RefCell::new(TestHost {
            ops: ops.clone(),
            labels: labels.clone(),
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
        Rig {
            world: World::new(),
            backend,
            registry: Rc::new(registry),
            ops,
            labels,
        }
    }

    fn realize(&self, element: Element) -> Realized<u32> {
        self.world
            .enter(|| realize(&self.backend, &self.registry, element))
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
