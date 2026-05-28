//! `ui!` iteration + conditionals + match: what the DSL supports for
//! turning data into components, in both static and reactive forms.
//!
//! Observation tool is the `MockBackend` event log. Concrete leaves
//! are `Text` nodes, so `CreateText { content }` tells us exactly which
//! children mounted and when.
//!
//! Coverage map (and the lowering each exercises):
//!   - static array / Vec / HashMap / `.map().collect()` → plain Rust
//!     `for` accumulating a flat `Vec<Primitive>` (the React
//!     `arr.map(e => <C/>)` equivalent).
//!   - static `0..n` range → `Primitive::Repeat`.
//!   - REACTIVE iteration is TYPE-DRIVEN, not heuristic: `for x in sig`
//!     (sig: `Signal<Vec<_>>`) → reactive `Primitive::Each` because the
//!     *type* is a signal; `for x in sig.get()` iterates a `Vec`
//!     snapshot → STATIC. A reactive count `for i in 0..n.get()` is the
//!     one narrow range-bound special case that still rebuilds.
//!   - `FlatList(data = sig, ...)` → reactive `Primitive::Virtualizer`
//!     (the keyed/windowed reactive-list path for large/scrolling lists).
//!   - static / reactive `if`/`else` → plain Rust `if` / `when(...)`.
//!   - static / reactive `match` → plain Rust `match` / `switch(...)`.
//!
//! NOTE: the `MockBackend`'s `create_virtualizer` stores the callbacks
//! rather than driving them inline (driving inline would re-enter the
//! framework's backend borrow — real backends mount from scroll/rAF).
//! Tests drive mount/release out-of-band via `TestRuntime::
//! sync_virtualizers`, so reactive-list ROW CONTENT, incremental mount,
//! and per-row scope teardown ARE observable here.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use runtime_core::{on_cleanup, signal, ui, Primitive, Signal};

use crate::common::{Event, TestRuntime};

/// Plain (non-`#[method]`) helper used by the structured-count test:
/// `for i in count(sig)` only requires `count` to be a single-segment
/// call whose args are bare signal paths — the macro reads it
/// syntactically.
fn structured_count(n: usize) -> usize {
    n
}

fn texts(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::CreateText { content } => Some(content.clone()),
            _ => None,
        })
        .collect()
}

fn count_text(events: &[Event], needle: &str) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, Event::CreateText { content } if content == needle))
        .count()
}

// ---------------------------------------------------------------------------
// Static iteration — array / Vec / HashMap / iterator passthrough
// ---------------------------------------------------------------------------

#[test]
fn static_array_maps_to_components() {
    let rt = TestRuntime::new();
    let tree: Primitive = ui! {
        View {
            for label in ["one", "two", "three"] {
                Text { label.to_string() }
            }
        }
    };
    let _owner = rt.render(tree);

    let t = texts(&rt.events());
    assert_eq!(t, vec!["one", "two", "three"], "array order preserved");
}

#[test]
fn static_vec_ref_maps_to_components() {
    let rt = TestRuntime::new();
    let items = vec![10, 20, 30];
    let tree: Primitive = ui! {
        View {
            for n in &items {
                Text { format!("n={}", n) }
            }
        }
    };
    let _owner = rt.render(tree);

    assert_eq!(texts(&rt.events()), vec!["n=10", "n=20", "n=30"]);
}

#[test]
fn static_hashmap_maps_to_components() {
    let rt = TestRuntime::new();
    let mut map: HashMap<&str, i32> = HashMap::new();
    map.insert("a", 1);
    map.insert("b", 2);
    let tree: Primitive = ui! {
        View {
            for (k, v) in &map {
                Text { format!("{}={}", k, v) }
            }
        }
    };
    let _owner = rt.render(tree);

    // HashMap iteration order is unspecified — assert the SET.
    let mut got = texts(&rt.events());
    got.sort();
    assert_eq!(got, vec!["a=1".to_string(), "b=2".to_string()]);
}

#[test]
fn iterator_map_collect_passthrough() {
    // The literal React `arr.map(e => <Comp/>)` shape: a `.map().collect()`
    // expression in child position, flattened by `ChildList`.
    let rt = TestRuntime::new();
    let tree: Primitive = ui! {
        View {
            ["x", "y"].iter().map(|s| ui! { Text { s.to_string() } }).collect::<Vec<_>>()
        }
    };
    let _owner = rt.render(tree);

    assert_eq!(texts(&rt.events()), vec!["x", "y"]);
}

#[test]
fn static_range_maps_to_components() {
    let rt = TestRuntime::new();
    let tree: Primitive = ui! {
        View {
            for i in 0..3 {
                Text { format!("r{}", i) }
            }
        }
    };
    let _owner = rt.render(tree);

    let mut got = texts(&rt.events());
    got.sort();
    assert_eq!(got, vec!["r0".to_string(), "r1".to_string(), "r2".to_string()]);
}

// ---------------------------------------------------------------------------
// Type-driven reactive iteration — Signal<Vec<_>> via Primitive::Each
// ---------------------------------------------------------------------------

/// `for x in sig { ... }` (where `sig: Signal<Vec<_>>`) is reactive
/// because the *type* is a signal — it lowers to a `Primitive::Each`
/// that rebuilds the whole list whenever the signal changes. No
/// `.get()` needed; push / shrink / replace / empty all re-render.
#[test]
fn signal_iteration_rebuilds_on_change() {
    let rt = TestRuntime::new();
    let data: Signal<Vec<&'static str>> = signal!(vec!["a", "b"]);
    let tree: Primitive = ui! {
        View {
            for s in data {
                Text { s.to_string() }
            }
        }
    };
    let _owner = rt.render(tree);
    assert_eq!(texts(&rt.events()), vec!["a", "b"], "initial render");

    // Push → rebuild reflects all current items.
    rt.backend_mut().clear_events();
    data.set(vec!["a", "b", "c"]);
    assert_eq!(texts(&rt.events()), vec!["a", "b", "c"], "rebuilt after push");

    // Shrink → rebuild reflects the shorter list.
    rt.backend_mut().clear_events();
    data.set(vec!["x"]);
    assert_eq!(texts(&rt.events()), vec!["x"], "rebuilt after shrink");

    // Empty → no rows.
    rt.backend_mut().clear_events();
    data.set(vec![]);
    assert_eq!(texts(&rt.events()), Vec::<String>::new(), "rebuilt empty");
}

/// On rebuild, `Primitive::Each` drops the PREVIOUS list's scope before
/// building the new one — freeing every row's signals/effects. Each row
/// registers an `on_cleanup`; growing the list must fire exactly the old
/// rows' cleanups (no leak, atomic teardown).
#[test]
fn each_releases_old_row_scopes_on_rebuild() {
    let rt = TestRuntime::new();
    let cleaned = Rc::new(Cell::new(0usize));
    let data: Signal<Vec<i32>> = signal!(vec![1, 2]);
    let c = cleaned.clone();
    let tree: Primitive = ui! {
        View {
            for n in data {
                {
                    let c2 = c.clone();
                    on_cleanup(move || c2.set(c2.get() + 1));
                    ui! { Text { n.to_string() } }
                }
            }
        }
    };
    let _owner = rt.render(tree); // 2 rows built, 2 on_cleanups in scope v1
    assert_eq!(cleaned.get(), 0, "nothing torn down on first build");

    data.set(vec![1, 2, 3]); // Each rebuilds: drop scope v1, build v2
    assert_eq!(cleaned.get(), 2, "both old row scopes freed on rebuild");
}

/// The type decides reactivity, not a `.get()` substring: iterating a
/// `Vec` SNAPSHOT (`for s in sig.get()`) is STATIC — `sig.get()` returns
/// a `Vec`, whose type resolves to the static impl. This is the
/// counterpart to `signal_iteration_rebuilds_on_change` and proves the
/// `.get()` heuristic is gone: a `.get()` in the iterable no longer
/// makes the loop reactive.
#[test]
fn snapshot_get_iteration_is_static() {
    let rt = TestRuntime::new();
    let data: Signal<Vec<&'static str>> = signal!(vec!["a", "b"]);
    let tree: Primitive = ui! {
        View {
            for s in data.get() {
                Text { s.to_string() }
            }
        }
    };
    let _owner = rt.render(tree);
    assert_eq!(texts(&rt.events()), vec!["a", "b"], "snapshot rendered once");

    // Mutating the signal does nothing — we iterated a detached Vec.
    rt.backend_mut().clear_events();
    data.set(vec!["a", "b", "c", "d"]);
    assert!(
        rt.events().is_empty(),
        "iterating a .get() snapshot is static; mutation must not re-render: {:?}",
        rt.events()
    );
}

/// A reactive *count* (range whose bound reads a signal) rebuilds too —
/// `for i in 0..n.get()` is detected as reactive and lowered to `Each`,
/// NOT snapshot once into a static `Repeat`.
#[test]
fn reactive_range_count_rebuilds() {
    let rt = TestRuntime::new();
    let n: Signal<usize> = signal!(2usize);
    let tree: Primitive = ui! {
        View {
            for i in 0..n.get() {
                Text { format!("#{}", i) }
            }
        }
    };
    let _owner = rt.render(tree);
    assert_eq!(texts(&rt.events()), vec!["#0", "#1"], "initial count");

    rt.backend_mut().clear_events();
    n.set(4);
    assert_eq!(texts(&rt.events()), vec!["#0", "#1", "#2", "#3"], "grew with count");
}

/// A reactive `for` with a multi-node body rebuilds as FLAT siblings —
/// no per-iteration wrapper View. Proven on the rebuild: the anchor
/// already exists, so two items × two nodes create zero new Views.
#[test]
fn reactive_for_multi_node_body_flattens_on_rebuild() {
    let rt = TestRuntime::new();
    let data: Signal<Vec<i32>> = signal!(vec![1]);
    let tree: Primitive = ui! {
        View {
            for n in data {
                Text { format!("h{}", n) }
                Text { format!("b{}", n) }
            }
        }
    };
    let _owner = rt.render(tree);
    assert_eq!(texts(&rt.events()), vec!["h1", "b1"], "initial flat render");

    rt.backend_mut().clear_events();
    data.set(vec![1, 2]);
    let ev = rt.events();
    assert_eq!(texts(&ev), vec!["h1", "b1", "h2", "b2"], "flat siblings on rebuild");
    assert_eq!(
        ev.iter().filter(|e| matches!(e, Event::CreateView)).count(),
        0,
        "no per-iteration wrapper views on rebuild: {ev:?}"
    );
}

// ---------------------------------------------------------------------------
// Reactive list — FlatList (the supported reactive-iteration path)
// ---------------------------------------------------------------------------

/// `FlatList(data = sig, render = ...)` builds a reactive Virtualizer:
/// it mounts as a virtualizer node, and a data-signal change notifies
/// the backend via `virtualizer_data_changed`. (Row content isn't
/// observable in the mock — it doesn't drive `mount_item`.)
#[test]
fn reactive_flat_list_creates_virtualizer_and_reacts_to_data() {
    let rt = TestRuntime::new();
    let data: Signal<Vec<i32>> = signal!(vec![1, 2, 3]);
    let tree: Primitive = ui! {
        View {
            FlatList(
                data = data,
                render = |_idx, item: &i32| ui! { Text { format!("item {}", item) } },
            )
        }
    };
    let _owner = rt.render(tree);

    assert!(
        rt.events().iter().any(|e| matches!(e, Event::CreateVirtualizer { .. })),
        "FlatList mounts a Virtualizer: {:?}",
        rt.events()
    );

    rt.backend_mut().clear_events();
    data.set(vec![1, 2, 3, 4, 5]);
    assert!(
        rt.events().iter().any(|e| matches!(e, Event::VirtualizerDataChanged { .. })),
        "data signal change must notify the backend: {:?}",
        rt.events()
    );
}

/// Drive the virtualizer's mount callbacks (via `sync_virtualizers`)
/// and assert each row builds its REAL content — `mount_item` runs the
/// `render` closure in a per-item scope and produces the expected
/// `Text`. This is the row-content coverage the create/data-changed
/// test can't give (the mock doesn't auto-mount).
#[test]
fn flat_list_mounts_rows_with_real_content() {
    let rt = TestRuntime::new();
    let data: Signal<Vec<i32>> = signal!(vec![10, 20, 30]);
    let tree: Primitive = ui! {
        FlatList(
            data = data,
            render = |_idx, item: &i32| ui! { Text { format!("item {}", item) } },
        )
    };
    let _owner = rt.render(tree);

    rt.backend_mut().clear_events();
    rt.sync_virtualizers();
    assert_eq!(texts(&rt.events()), vec!["item 10", "item 20", "item 30"]);
}

/// Growing the data signal mounts ONLY the new row on the next sync —
/// `render_item` reads the current signal value, so the new index
/// renders the freshly-added item.
#[test]
fn flat_list_mounts_only_new_row_on_growth() {
    let rt = TestRuntime::new();
    let data: Signal<Vec<i32>> = signal!(vec![1, 2]);
    let tree: Primitive = ui! {
        FlatList(
            data = data,
            render = |_idx, item: &i32| ui! { Text { format!("v{}", item) } },
        )
    };
    let _owner = rt.render(tree);
    rt.sync_virtualizers(); // mount rows 0,1

    rt.backend_mut().clear_events();
    data.set(vec![1, 2, 3]);
    rt.sync_virtualizers(); // only index 2 is new
    assert_eq!(texts(&rt.events()), vec!["v3"], "only the newly-added row mounts");
}

/// Shrinking the data signal RELEASES the dropped rows' scopes on the
/// next sync — `release_item` drops each row's `Scope`, firing its
/// `on_cleanup`. Proves per-row teardown (no leak) through the real
/// Virtualizer machinery.
#[test]
fn flat_list_releases_row_scopes_on_shrink() {
    let rt = TestRuntime::new();
    let cleaned = Rc::new(Cell::new(0usize));
    let data: Signal<Vec<i32>> = signal!(vec![1, 2, 3]);
    let c = cleaned.clone();
    let tree: Primitive = ui! {
        FlatList(
            data = data,
            render = move |_idx, item: &i32| {
                let c2 = c.clone();
                on_cleanup(move || c2.set(c2.get() + 1));
                ui! { Text { format!("v{}", item) } }
            },
        )
    };
    let _owner = rt.render(tree);
    rt.sync_virtualizers(); // mount 3 rows, register 3 on_cleanups
    assert_eq!(cleaned.get(), 0, "no rows released yet");

    data.set(vec![1]); // shrink to a single row
    rt.sync_virtualizers(); // release rows 1 and 2
    assert_eq!(cleaned.get(), 2, "two row scopes torn down on shrink");
}

// ---------------------------------------------------------------------------
// Structured count `for i in count(sig)` — Virtualizer with REAL rows
// ---------------------------------------------------------------------------

/// `for i in count(sig) { body }` lowers to a structured
/// `Primitive::Virtualizer` (count `Derived` + `row_template` for
/// generator backends). Regression: its runtime `render_item` must
/// build the REAL row body — previously it was a hardcoded empty-View
/// placeholder, so this construct rendered blank rows on every runtime
/// backend (web/iOS/Android/wgpu).
#[test]
fn for_count_signal_renders_real_rows_not_placeholder() {
    let count_sig: Signal<usize> = signal!(3);
    let tree: Primitive = ui! {
        for _i in structured_count(count_sig) {
            Text { "row".to_string() }
        }
    };
    match tree {
        Primitive::Virtualizer { render_item, item_count, row_template, .. } => {
            // Count derives from the signal-reading call.
            assert_eq!((item_count.compute)(), 3, "item_count derives from count(sig)");
            // Structured form preserved for generator backends (Roku).
            assert!(row_template.is_some(), "row_template kept for generator backends");
            // THE FIX: runtime render builds the real row, not a placeholder View.
            assert!(
                matches!(render_item(0), Primitive::Text { .. }),
                "render_item must build the real row body (Text), not an empty placeholder View",
            );
        }
        _ => panic!("`for i in count(sig)` must lower to Primitive::Virtualizer"),
    }
}

/// End-to-end: `for i in count(sig)` mounts real rows through the
/// actual Virtualizer machinery (driven via `sync_virtualizers`), not
/// just at the primitive level — the full mount path complements the
/// primitive-level assertion above.
#[test]
fn for_count_signal_mounts_real_rows_end_to_end() {
    let rt = TestRuntime::new();
    let n: Signal<usize> = signal!(2usize);
    let tree: Primitive = ui! {
        for _i in structured_count(n) {
            Text { "cell".to_string() }
        }
    };
    let _owner = rt.render(tree);

    rt.backend_mut().clear_events();
    rt.sync_virtualizers();
    assert_eq!(
        count_text(&rt.events(), "cell"),
        2,
        "both rows mount with real content: {:?}",
        rt.events()
    );
}

// ---------------------------------------------------------------------------
// Conditionals — static `if` and reactive `if`/`else`
// ---------------------------------------------------------------------------

#[test]
fn static_if_else_mounts_single_branch() {
    let rt = TestRuntime::new();
    let tree: Primitive = ui! {
        View {
            if 3 < 4 {
                Text { "yes".to_string() }
            } else {
                Text { "no".to_string() }
            }
        }
    };
    let _owner = rt.render(tree);

    let events = rt.events();
    assert_eq!(count_text(&events, "yes"), 1);
    assert_eq!(count_text(&events, "no"), 0);
}

#[test]
fn reactive_if_else_flips_on_signal() {
    let rt = TestRuntime::new();
    let flag: Signal<bool> = signal!(true);
    let tree: Primitive = ui! {
        View {
            if flag.get() {
                Text { "on".to_string() }
            } else {
                Text { "off".to_string() }
            }
        }
    };
    let _owner = rt.render(tree);

    assert_eq!(count_text(&rt.events(), "on"), 1, "initial branch");
    assert_eq!(count_text(&rt.events(), "off"), 0);

    rt.backend_mut().clear_events();
    flag.set(false);
    assert_eq!(count_text(&rt.events(), "off"), 1, "else branch after flip");
    assert_eq!(count_text(&rt.events(), "on"), 0);
}

#[test]
fn reactive_if_without_else_toggles_presence() {
    let rt = TestRuntime::new();
    let show: Signal<bool> = signal!(false);
    let tree: Primitive = ui! {
        View {
            if show.get() {
                Text { "visible".to_string() }
            }
        }
    };
    let _owner = rt.render(tree);
    assert_eq!(count_text(&rt.events(), "visible"), 0, "hidden initially");

    rt.backend_mut().clear_events();
    show.set(true);
    assert_eq!(count_text(&rt.events(), "visible"), 1, "shown after toggle");
}

// ---------------------------------------------------------------------------
// Match — static and reactive
// ---------------------------------------------------------------------------

#[test]
fn static_match_mounts_single_arm() {
    let rt = TestRuntime::new();
    let mode = 2;
    let tree: Primitive = ui! {
        View {
            match mode {
                1 => { Text { "one".to_string() } }
                2 => { Text { "two".to_string() } }
                _ => { Text { "other".to_string() } }
            }
        }
    };
    let _owner = rt.render(tree);

    let events = rt.events();
    assert_eq!(count_text(&events, "two"), 1);
    assert_eq!(count_text(&events, "one"), 0);
    assert_eq!(count_text(&events, "other"), 0);
}

#[test]
fn reactive_match_switches_on_signal_with_default() {
    let rt = TestRuntime::new();
    let mode: Signal<u32> = signal!(0u32);
    let tree: Primitive = ui! {
        View {
            match mode.get() {
                0 => { Text { "zero".to_string() } }
                1 => { Text { "one".to_string() } }
                _ => { Text { "fallback".to_string() } }
            }
        }
    };
    let _owner = rt.render(tree);
    assert_eq!(count_text(&rt.events(), "zero"), 1, "initial arm");

    rt.backend_mut().clear_events();
    mode.set(1);
    assert_eq!(count_text(&rt.events(), "one"), 1, "arm 1 after change");

    rt.backend_mut().clear_events();
    mode.set(99);
    assert_eq!(count_text(&rt.events(), "fallback"), 1, "default arm for unmatched");
}
