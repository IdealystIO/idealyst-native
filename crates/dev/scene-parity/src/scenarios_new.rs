//! The parity scenarios, re-targeted at the new scene core per the
//! README's contract: SAME names, SAME modes, SAME step labels, SAME
//! mutations — built through `runtime-scene`'s element constructors and
//! driven by `runtime-world` signals (each step ends in a `world.flush()`
//! inside `NewCx::step`).
//!
//! Lowering map (old public constructor → new-core form):
//!
//! | old                      | new                                         |
//! |--------------------------|---------------------------------------------|
//! | `view(children)`         | `item(NcView, children)`                    |
//! | `text(s)`                | `item(NcText(s), vec![])`                   |
//! | `when(cond, a, b)`       | `dyn_keyed(cond, \|&b\| ...)` (guarded)     |
//! | `switch(scrutinee, arm)` | `dyn_keyed(scrutinee, arm)` (guarded)       |
//! | `dynamic(f)`             | `dyn_element(f)` (unguarded)                |
//! | `fragment(children)`     | `fragment(children)`                        |
//! | `each_keyed(snapshot)`   | `keyed(items, key, render)`                 |
//! | multi-node row           | row renders a `fragment(...)`               |
//! | `on_cleanup(marker)`     | probe `effect` returning the marker closure |
//!
//! The `on_cleanup` mapping: the kernel's `on_cleanup` requires a running
//! effect, and row/branch builders run inside the subtree's collector but
//! not inside an effect of their own — so scenarios register a probe
//! `effect` whose returned cleanup fires when the subtree's `Owned` drops.
//! Same observable moment (scope teardown), same marker line.

use runtime_scene::{dyn_element, dyn_keyed, fragment, item, keyed, Element};
use runtime_world::{effect, signal, Signal};

use crate::new_core::{NcText, NcView, NewCx};
use crate::{Mode, Recorder};

/// A parity scenario. Names/modes are pinned against the frozen
/// inventory (asserted by
/// `scenario_registry_matches_the_frozen_inventory`).
pub struct NewScenario {
    pub name: &'static str,
    pub modes: &'static [Mode],
    pub run: fn(&mut NewCx),
}

/// The scenario registry — one entry per frozen golden pair.
pub fn new_scenarios() -> Vec<NewScenario> {
    vec![
        NewScenario { name: "when_toggle", modes: &[Mode::Anchored, Mode::Spliced], run: when_toggle },
        NewScenario {
            name: "when_dedup_extra_signal",
            modes: &[Mode::Anchored, Mode::Spliced],
            run: when_dedup_extra_signal,
        },
        NewScenario {
            name: "switch_rotation",
            modes: &[Mode::Anchored, Mode::Spliced],
            run: switch_rotation,
        },
        NewScenario { name: "each_append", modes: &[Mode::Anchored, Mode::Spliced], run: each_append },
        NewScenario { name: "each_remove_middle", modes: &[Mode::Spliced], run: each_remove_middle },
        NewScenario { name: "each_reverse", modes: &[Mode::Spliced], run: each_reverse },
        NewScenario {
            name: "each_insert_middle_survivors",
            modes: &[Mode::Spliced],
            run: each_insert_middle_survivors,
        },
        NewScenario { name: "each_multi_node_rows", modes: &[Mode::Spliced], run: each_multi_node_rows },
        NewScenario { name: "fragment_base_index", modes: &[Mode::Spliced], run: fragment_base_index },
        NewScenario { name: "dynamic_swap", modes: &[Mode::Anchored], run: dynamic_swap },
        NewScenario {
            name: "nested_when_in_each_row",
            modes: &[Mode::Anchored, Mode::Spliced],
            run: nested_when_in_each_row,
        },
        NewScenario { name: "dispose_order_each", modes: &[Mode::Spliced], run: dispose_order_each },
        NewScenario {
            name: "dispose_order_when",
            modes: &[Mode::Anchored, Mode::Spliced],
            run: dispose_order_when,
        },
    ]
}

// ===========================================================================
// Helpers (mirror scenarios.rs's)
// ===========================================================================

fn view(children: Vec<Element>) -> Element {
    item(NcView, children)
}

fn text(content: impl Into<String>) -> Element {
    item(NcText(content.into()), vec![])
}

/// `when` lowered to the guarded hole — the boolean is the guard key.
fn when(
    cond: impl Fn() -> bool + 'static,
    then: impl Fn() -> Element + 'static,
    otherwise: impl Fn() -> Element + 'static,
) -> Element {
    dyn_keyed(cond, move |&on| if on { then() } else { otherwise() })
}

/// A cleanup marker scoped to the CURRENT build (row/branch): a probe
/// effect whose returned cleanup fires when the enclosing subtree's
/// `Owned` drops — the new-core mapping of the old scenarios'
/// `on_cleanup(|| rec.note_cleanup(..))`.
fn cleanup_marker(rec: &Recorder, label: String) {
    let rec = rec.clone();
    effect(move || {
        let rec = rec.clone();
        let label = label.clone();
        move || rec.note_cleanup(&label)
    });
}

/// A keyed list over a `Signal<Vec<u32>>` — one `text("row-{n}")` per row,
/// keyed by the number itself.
fn keyed_rows(items: Signal<Vec<u32>>) -> Element {
    keyed(move || items.get(), |n| *n, |n: u32| text(format!("row-{n}")))
}

/// Mounts a keyed list between a static header and footer, so the spliced
/// region's `base_index` (1) is visible in the goldens.
fn mount_keyed_list(cx: &mut NewCx, items: Signal<Vec<u32>>) {
    cx.mount(view(vec![text("header"), keyed_rows(items), text("footer")]));
}

// ===========================================================================
// when
// ===========================================================================

fn when_toggle(cx: &mut NewCx) {
    let show: Signal<bool> = signal(true);
    cx.mount(view(vec![
        text("before"),
        when(move || show.get(), || text("shown"), || text("hidden")),
        text("after"),
    ]));
    cx.step("set show = false", || show.set(false));
    cx.step("set show = true", || show.set(true));
}

fn when_dedup_extra_signal(cx: &mut NewCx) {
    let show: Signal<bool> = signal(true);
    let version: Signal<i32> = signal(0);
    cx.mount(view(vec![when(
        move || {
            // The predicate reads an extra signal beyond the boolean —
            // the shape the guard (last_active) dedup exists for.
            let _tick = version.get();
            show.get()
        },
        || text("on"),
        || text("off"),
    )]));
    cx.step("bump version (boolean unchanged — expect NO rebuild)", || {
        version.set(1)
    });
    cx.step("set show = false (boolean changed — rebuild proves effect alive)", || {
        show.set(false)
    });
}

// ===========================================================================
// switch
// ===========================================================================

fn switch_rotation(cx: &mut NewCx) {
    let arm: Signal<u32> = signal(0);
    cx.mount(view(vec![dyn_keyed(
        move || arm.get(),
        |m: &u32| match *m {
            0 => text("arm-a"),
            1 => text("arm-b"),
            _ => text("arm-c"),
        },
    )]));
    cx.step("set arm = 1", || arm.set(1));
    cx.step("set arm = 2", || arm.set(2));
    cx.step("set arm = 0 (full rotation)", || arm.set(0));
}

// ===========================================================================
// each — keyed list
// ===========================================================================

fn each_append(cx: &mut NewCx) {
    let items: Signal<Vec<u32>> = signal(vec![1, 2, 3]);
    mount_keyed_list(cx, items);
    cx.step("append 4 -> [1,2,3,4]", || items.set(vec![1, 2, 3, 4]));
}

fn each_remove_middle(cx: &mut NewCx) {
    let items: Signal<Vec<u32>> = signal(vec![1, 2, 3]);
    mount_keyed_list(cx, items);
    cx.step("remove middle -> [1,3]", || items.set(vec![1, 3]));
}

fn each_reverse(cx: &mut NewCx) {
    let items: Signal<Vec<u32>> = signal(vec![1, 2, 3]);
    mount_keyed_list(cx, items);
    cx.step("reverse -> [3,2,1]", || items.set(vec![3, 2, 1]));
}

fn each_insert_middle_survivors(cx: &mut NewCx) {
    let items: Signal<Vec<u32>> = signal(vec![1, 2, 4]);
    mount_keyed_list(cx, items);
    cx.step("insert 3 mid-list -> [1,2,3,4] (survivors must NOT move)", || {
        items.set(vec![1, 2, 3, 4])
    });
}

fn each_multi_node_rows(cx: &mut NewCx) {
    let items: Signal<Vec<u32>> = signal(vec![1, 2]);
    cx.mount(view(vec![
        text("header"),
        keyed(
            move || items.get(),
            |n| *n,
            // Two flat sibling nodes per row — the "fragment row".
            |n: u32| fragment(vec![text(format!("r{n}-a")), text(format!("r{n}-b"))]),
        ),
    ]));
    cx.step("append row 3 -> [1,2,3]", || items.set(vec![1, 2, 3]));
    cx.step("reverse -> [3,2,1]", || items.set(vec![3, 2, 1]));
}

// ===========================================================================
// fragment index math
// ===========================================================================

fn fragment_base_index(cx: &mut NewCx) {
    let show: Signal<bool> = signal(true);
    cx.mount(view(vec![
        text("s0"),
        fragment(vec![text("f1"), text("f2")]),
        when(move || show.get(), || text("cond-on"), || text("cond-off")),
        text("tail"),
    ]));
    cx.step("set show = false (must re-insert at index 3)", || {
        show.set(false)
    });
    cx.step("set show = true (still index 3)", || show.set(true));
}

// ===========================================================================
// dynamic
// ===========================================================================

fn dynamic_swap(cx: &mut NewCx) {
    let which: Signal<u32> = signal(0);
    cx.mount(view(vec![dyn_element(move || {
        if which.get() == 0 {
            view(vec![text("dyn-a")])
        } else {
            text("dyn-b")
        }
    })]));
    cx.step("set which = 1 (view subtree -> bare text)", || which.set(1));
    cx.step("set which = 0 (back to view subtree)", || which.set(0));
}

// ===========================================================================
// nested: when inside an each row
// ===========================================================================

fn nested_when_in_each_row(cx: &mut NewCx) {
    let items: Signal<Vec<u32>> = signal(vec![1, 2]);
    let flag: Signal<bool> = signal(true);
    cx.mount(view(vec![keyed(
        move || items.get(),
        |n| *n,
        move |n: u32| {
            view(vec![
                text(format!("row-{n}")),
                when(move || flag.get(), || text("inner-on"), || text("inner-off")),
            ])
        },
    )]));
    cx.step("toggle inner flag (both rows rebuild branch in place)", || {
        flag.set(false)
    });
    cx.step("append row 3", || items.set(vec![1, 2, 3]));
    cx.step("remove row 1 (its inner when dies with the row scope)", || {
        items.set(vec![2, 3])
    });
    cx.step("toggle inner flag again (only surviving rows fire)", || {
        flag.set(true)
    });
}

// ===========================================================================
// dispose ordering
// ===========================================================================

fn dispose_order_each(cx: &mut NewCx) {
    let rec = cx.recorder();
    let items: Signal<Vec<u32>> = signal(vec![1, 2, 3]);
    cx.mount(view(vec![keyed(
        move || items.get(),
        |n| *n,
        move |n: u32| {
            // Registered in the row's scope: fires when the row's Owned
            // drops, marking WHERE in the op stream teardown happens.
            cleanup_marker(&rec, format!("row-{n}"));
            text(format!("row-{n}"))
        },
    )]));
    cx.step("remove middle row (nodes out BEFORE scope drop)", || {
        items.set(vec![1, 3])
    });
}

fn dispose_order_when(cx: &mut NewCx) {
    let rec = cx.recorder();
    let show: Signal<bool> = signal(true);
    let rec_then = rec.clone();
    let rec_else = rec;
    cx.mount(view(vec![when(
        move || show.get(),
        move || {
            cleanup_marker(&rec_then, "then-branch".into());
            text("then")
        },
        move || {
            cleanup_marker(&rec_else, "else-branch".into());
            text("else")
        },
    )]));
    cx.step("set show = false (then-branch teardown ordering)", || {
        show.set(false)
    });
    cx.step("set show = true (else-branch teardown ordering)", || {
        show.set(true)
    });
}
