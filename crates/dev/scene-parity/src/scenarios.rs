//! The parity scenario suite — the P1 gate's behavioral definition.
//!
//! Every scenario builds its tree through runtime-core's PUBLIC element
//! constructors (`view`, `text`, `when`, `switch`, `dynamic`, `fragment`,
//! `each_keyed`) and drives it with plain `Signal` writes. No
//! old-core-internal APIs, no walker types: the bodies are `build tree →
//! mutate → snapshot` and nothing else, so re-targeting them at the new
//! scene core means swapping `Cx::mount`'s internals, not rewriting the
//! scenarios.
//!
//! Deliberately excluded (documented in README.md): `presence` (needs
//! scheduler/time hooks for exit animations — out of scope for P1
//! structural parity), hydration-mode `when`/`switch` (needs the web
//! backend's hydration cursor; pinned separately by
//! `runtime-core/tests/walker/hydration.rs`), and the batched-Repeat
//! fast path (an optimization contract of `execute_batch`, not of the 7
//! structural ops).

use runtime_core::{
    dynamic, each_keyed, fragment, on_cleanup, signal, switch, text, view, when, EachKey,
    EachRowBuild, Element, IntoElement, Signal,
};

use crate::{Cx, Mode, Scenario};

/// The full scenario registry. Golden filenames derive from
/// `name` + mode; the `about` lines are written into the golden header.
pub fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "when_toggle",
            about: &[
                "`when` toggled true -> false -> true between two static siblings.",
                "Anchored: anchor created up front, every flip = drop-scope +",
                "clear_children(anchor) + rebuild + insert (the first Effect fire",
                "also clear_children's the freshly created, still-empty anchor).",
                "Spliced: no anchor; every flip = remove_child(old) +",
                "insert_at(parent, new, base_index=1) directly in the real parent.",
            ],
            modes: &[Mode::Anchored, Mode::Spliced],
            run: when_toggle,
        },
        Scenario {
            name: "when_dedup_extra_signal",
            about: &[
                "A `when` predicate reads an EXTRA signal beyond the boolean",
                "(a `version` tick). Bumping the tick re-fires the Effect but the",
                "boolean is unchanged -> the last_active dedup guard must skip the",
                "teardown/rebuild entirely (zero structural ops). Without the",
                "guard the branch's gesture nodes would be recreated mid-press.",
                "The following bool flip proves the effect is still live.",
            ],
            modes: &[Mode::Anchored, Mode::Spliced],
            run: when_dedup_extra_signal,
        },
        Scenario {
            name: "switch_rotation",
            about: &[
                "3-branch `switch` (typed/opaque discriminant) rotated",
                "0 -> 1 -> 2 -> 0. Anchored: the closure path defers the rebuild",
                "to a microtask (synchronous here: no scheduler installed) and",
                "swaps under the anchor via drop-scope + clear_children + insert.",
                "Spliced: synchronous remove_child + insert_at(base_index).",
            ],
            modes: &[Mode::Anchored, Mode::Spliced],
            run: switch_rotation,
        },
        Scenario {
            name: "each_append",
            about: &[
                "Keyed list [1,2,3] between static header/footer siblings;",
                "append 4. Spliced: the reconciler builds ONLY the new row and",
                "insert_at's it at base_index+3; survivors emit nothing.",
                "Anchored: the no-splice fallback is a FULL rebuild —",
                "clear_children(anchor) + rebuild every row (per-row state lost).",
            ],
            modes: &[Mode::Anchored, Mode::Spliced],
            run: each_append,
        },
        Scenario {
            name: "each_remove_middle",
            about: &[
                "Keyed list [1,2,3] -> [1,3]: the removed row's node is",
                "remove_child'd; survivors kept their relative order (old",
                "positions strictly increasing) so NO insert_at fires at all —",
                "surviving rows are never touched (focus/IME preservation).",
            ],
            modes: &[Mode::Spliced],
            run: each_remove_middle,
        },
        Scenario {
            name: "each_reverse",
            about: &[
                "Keyed list [1,2,3] -> [3,2,1]: a real reorder (survivor old",
                "positions not monotonic) repositions EVERY row node via",
                "insert_at in target order (DOM insertBefore move semantics),",
                "building nothing and removing nothing.",
            ],
            modes: &[Mode::Spliced],
            run: each_reverse,
        },
        Scenario {
            name: "each_insert_middle_survivors",
            about: &[
                "Keyed list [1,2,4] -> [1,2,3,4]: one NEW row in the middle.",
                "Survivor old positions (0,1,2) stay strictly increasing ->",
                "reorder=false -> ONLY the new row emits insert_at (at",
                "base_index+2). Existing rows — including 4, which shifts",
                "visually — emit NO moves; the backend's insert_at displaces it.",
            ],
            modes: &[Mode::Spliced],
            run: each_insert_middle_survivors,
        },
        Scenario {
            name: "each_multi_node_rows",
            about: &[
                "Keyed rows whose body is TWO flat sibling nodes (a fragment",
                "row). Append builds both nodes and insert_at's them at",
                "consecutive indices; reverse repositions every node of every",
                "row in target order — the per-row `nodes` vec (not one node)",
                "is the reconciler's accounting unit.",
            ],
            modes: &[Mode::Spliced],
            run: each_multi_node_rows,
        },
        Scenario {
            name: "fragment_base_index",
            about: &[
                "Children list = [static, Fragment(2 children), when, static].",
                "The fragment splices its children flat and the threaded",
                "`inserted` counter keeps counting THROUGH it, so the spliced",
                "`when` after the fragment captures base_index=3 (1 static + 2",
                "fragment children) and every re-splice lands at index 3.",
            ],
            modes: &[Mode::Spliced],
            run: fragment_base_index,
        },
        Scenario {
            name: "dynamic_swap",
            about: &[
                "`dynamic(closure)` subtree swap driven by a signal. Always",
                "anchored (there is no spliced Dynamic path): every dependency",
                "change = drop-scope + clear_children(anchor) + rebuild + insert.",
                "No dedup guard exists here — the closure IS the dependency",
                "source, so each fire rebuilds even if the output would match.",
            ],
            modes: &[Mode::Anchored],
            run: dynamic_swap,
        },
        Scenario {
            name: "nested_when_in_each_row",
            about: &[
                "A `when` inside each keyed row (row = view[text, when]).",
                "Toggling the shared flag rebuilds each row's branch IN PLACE",
                "(in row-creation order) with no row remount; appending a row",
                "builds a fresh nested when; removing a row tears its when down",
                "with the row scope, so later flag flips only fire survivors.",
            ],
            modes: &[Mode::Anchored, Mode::Spliced],
            run: nested_when_in_each_row,
        },
        Scenario {
            name: "dispose_order_each",
            about: &[
                "Dispose ordering for keyed-row unmount (each.rs `unmount`):",
                "the removed row's nodes are remove_child'd BEFORE its scope",
                "drops (so a reactive effect in the row can't fire against a",
                "half-detached node) — the golden shows remove_child, THEN the",
                "row's `cleanup` marker.",
            ],
            modes: &[Mode::Spliced],
            run: dispose_order_each,
        },
        Scenario {
            name: "dispose_order_when",
            about: &[
                "Dispose ordering for `when` branch teardown — the two modes",
                "deliberately DIFFER and both orders are pinned:",
                "Anchored: old scope drops FIRST (cleanup marker), THEN",
                "clear_children(anchor), then the new branch builds.",
                "Spliced: the old node is remove_child'd FIRST, THEN the scope",
                "drops (cleanup marker), then the new branch builds + splices.",
            ],
            modes: &[Mode::Anchored, Mode::Spliced],
            run: dispose_order_when,
        },
    ]
}

// ===========================================================================
// Helpers
// ===========================================================================

/// A keyed list over a `Signal<Vec<u32>>` — one `text("row-{n}")` per
/// row, keyed by the number itself.
fn keyed_rows(items: Signal<Vec<u32>>) -> Element {
    each_keyed(move || {
        items
            .get()
            .into_iter()
            .map(|n| {
                let build: EachRowBuild =
                    Box::new(move || vec![text(format!("row-{n}")).into_element()]);
                (EachKey::new(n), build)
            })
            .collect()
    })
}

/// Mounts a keyed list between a static header and footer, so the
/// spliced region's `base_index` (1) is visible in the goldens.
fn mount_keyed_list(cx: &mut Cx, items: Signal<Vec<u32>>) {
    cx.mount(
        view(vec![
            text("header").into_element(),
            keyed_rows(items),
            text("footer").into_element(),
        ])
        .into_element(),
    );
}

// ===========================================================================
// when
// ===========================================================================

fn when_toggle(cx: &mut Cx) {
    let show: Signal<bool> = signal(true);
    cx.mount(
        view(vec![
            text("before").into_element(),
            when(
                move || show.get(),
                || text("shown").into_element(),
                || text("hidden").into_element(),
            ),
            text("after").into_element(),
        ])
        .into_element(),
    );
    cx.step("set show = false", || show.set(false));
    cx.step("set show = true", || show.set(true));
}

fn when_dedup_extra_signal(cx: &mut Cx) {
    let show: Signal<bool> = signal(true);
    let version: Signal<i32> = signal(0);
    cx.mount(
        view(vec![when(
            move || {
                // The predicate reads an extra signal beyond the boolean —
                // the shape the last_active dedup guard exists for.
                let _tick = version.get();
                show.get()
            },
            || text("on").into_element(),
            || text("off").into_element(),
        )])
        .into_element(),
    );
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

fn switch_rotation(cx: &mut Cx) {
    let arm: Signal<u32> = signal(0);
    cx.mount(
        view(vec![switch(
            move || arm.get(),
            |m: &u32| match *m {
                0 => text("arm-a").into_element(),
                1 => text("arm-b").into_element(),
                _ => text("arm-c").into_element(),
            },
        )])
        .into_element(),
    );
    cx.step("set arm = 1", || arm.set(1));
    cx.step("set arm = 2", || arm.set(2));
    cx.step("set arm = 0 (full rotation)", || arm.set(0));
}

// ===========================================================================
// each — keyed list
// ===========================================================================

fn each_append(cx: &mut Cx) {
    let items: Signal<Vec<u32>> = signal(vec![1, 2, 3]);
    mount_keyed_list(cx, items);
    cx.step("append 4 -> [1,2,3,4]", || items.set(vec![1, 2, 3, 4]));
}

fn each_remove_middle(cx: &mut Cx) {
    let items: Signal<Vec<u32>> = signal(vec![1, 2, 3]);
    mount_keyed_list(cx, items);
    cx.step("remove middle -> [1,3]", || items.set(vec![1, 3]));
}

fn each_reverse(cx: &mut Cx) {
    let items: Signal<Vec<u32>> = signal(vec![1, 2, 3]);
    mount_keyed_list(cx, items);
    cx.step("reverse -> [3,2,1]", || items.set(vec![3, 2, 1]));
}

fn each_insert_middle_survivors(cx: &mut Cx) {
    let items: Signal<Vec<u32>> = signal(vec![1, 2, 4]);
    mount_keyed_list(cx, items);
    cx.step("insert 3 mid-list -> [1,2,3,4] (survivors must NOT move)", || {
        items.set(vec![1, 2, 3, 4])
    });
}

fn each_multi_node_rows(cx: &mut Cx) {
    let items: Signal<Vec<u32>> = signal(vec![1, 2]);
    cx.mount(
        view(vec![
            text("header").into_element(),
            each_keyed(move || {
                items
                    .get()
                    .into_iter()
                    .map(|n| {
                        // Two flat sibling nodes per row — the "fragment row".
                        let build: EachRowBuild = Box::new(move || {
                            vec![
                                text(format!("r{n}-a")).into_element(),
                                text(format!("r{n}-b")).into_element(),
                            ]
                        });
                        (EachKey::new(n), build)
                    })
                    .collect()
            }),
        ])
        .into_element(),
    );
    cx.step("append row 3 -> [1,2,3]", || items.set(vec![1, 2, 3]));
    cx.step("reverse -> [3,2,1]", || items.set(vec![3, 2, 1]));
}

// ===========================================================================
// fragment index math
// ===========================================================================

fn fragment_base_index(cx: &mut Cx) {
    let show: Signal<bool> = signal(true);
    cx.mount(
        view(vec![
            text("s0").into_element(),
            fragment(vec![
                text("f1").into_element(),
                text("f2").into_element(),
            ]),
            when(
                move || show.get(),
                || text("cond-on").into_element(),
                || text("cond-off").into_element(),
            ),
            text("tail").into_element(),
        ])
        .into_element(),
    );
    cx.step("set show = false (must re-insert at index 3)", || {
        show.set(false)
    });
    cx.step("set show = true (still index 3)", || show.set(true));
}

// ===========================================================================
// dynamic
// ===========================================================================

fn dynamic_swap(cx: &mut Cx) {
    let which: Signal<u32> = signal(0);
    cx.mount(
        view(vec![dynamic(move || {
            if which.get() == 0 {
                view(vec![text("dyn-a").into_element()]).into_element()
            } else {
                text("dyn-b").into_element()
            }
        })])
        .into_element(),
    );
    cx.step("set which = 1 (view subtree -> bare text)", || which.set(1));
    cx.step("set which = 0 (back to view subtree)", || which.set(0));
}

// ===========================================================================
// nested: when inside an each row
// ===========================================================================

fn nested_when_in_each_row(cx: &mut Cx) {
    let items: Signal<Vec<u32>> = signal(vec![1, 2]);
    let flag: Signal<bool> = signal(true);
    cx.mount(
        view(vec![each_keyed(move || {
            items
                .get()
                .into_iter()
                .map(|n| {
                    let build: EachRowBuild = Box::new(move || {
                        vec![view(vec![
                            text(format!("row-{n}")).into_element(),
                            when(
                                move || flag.get(),
                                || text("inner-on").into_element(),
                                || text("inner-off").into_element(),
                            ),
                        ])
                        .into_element()]
                    });
                    (EachKey::new(n), build)
                })
                .collect()
        })])
        .into_element(),
    );
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

fn dispose_order_each(cx: &mut Cx) {
    let rec = cx.recorder();
    let items: Signal<Vec<u32>> = signal(vec![1, 2, 3]);
    cx.mount(
        view(vec![each_keyed(move || {
            let rec = rec.clone();
            items
                .get()
                .into_iter()
                .map(|n| {
                    let rec = rec.clone();
                    let build: EachRowBuild = Box::new(move || {
                        // Registered in the row's scope: fires when the row
                        // scope drops, marking WHERE in the op stream the
                        // scope teardown happens.
                        on_cleanup(move || rec.note_cleanup(&format!("row-{n}")));
                        vec![text(format!("row-{n}")).into_element()]
                    });
                    (EachKey::new(n), build)
                })
                .collect()
        })])
        .into_element(),
    );
    cx.step("remove middle row (nodes out BEFORE scope drop)", || {
        items.set(vec![1, 3])
    });
}

fn dispose_order_when(cx: &mut Cx) {
    let rec = cx.recorder();
    let show: Signal<bool> = signal(true);
    let rec_then = rec.clone();
    let rec_else = rec;
    cx.mount(
        view(vec![when(
            move || show.get(),
            move || {
                let rec = rec_then.clone();
                on_cleanup(move || rec.note_cleanup("then-branch"));
                text("then").into_element()
            },
            move || {
                let rec = rec_else.clone();
                on_cleanup(move || rec.note_cleanup("else-branch"));
                text("else").into_element()
            },
        )])
        .into_element(),
    );
    cx.step("set show = false (then-branch teardown ordering)", || {
        show.set(false)
    });
    cx.step("set show = true (else-branch teardown ordering)", || {
        show.set(true)
    });
}
