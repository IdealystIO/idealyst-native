//! `ui!` iteration + conditionals + match: what the DSL supports for
//! turning data into components, in both static and reactive forms.
//!
//! Observation tool is the `MockBackend` event log. Concrete leaves
//! are `Text` nodes, so `CreateText { content }` tells us exactly which
//! children mounted and when.
//!
//! Coverage map (and the lowering each exercises):
//!   - static array / Vec / HashMap / `.map().collect()` → plain Rust
//!     `for` accumulating a flat `Vec<Element>` (the React
//!     `arr.map(e => <C/>)` equivalent).
//!   - static `0..n` range → `Element::Repeat`.
//!   - REACTIVE iteration is TYPE-DRIVEN, not heuristic: `for x in sig`
//!     (sig: `Signal<Vec<_>>`) → reactive `Element::Each` because the
//!     *type* is a signal; `for x in sig.get()` iterates a `Vec`
//!     snapshot → STATIC. A reactive count `for i in 0..n.get()` is the
//!     one narrow range-bound special case that still rebuilds.
//!   - `flat_list(data = sig, ...)` → reactive `Element::Virtualizer`
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

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use runtime_core::{memo, on_cleanup, signal, text, ui, view, when, Element, IntoElement, Signal};

use crate::common::{Event, MockBackendConfig, TestRuntime};

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
    let tree: Element = ui! {
        view {
            for label in ["one", "two", "three"] {
                text { label.to_string() }
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
    let tree: Element = ui! {
        view {
            for n in &items {
                text { format!("n={}", n) }
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
    let tree: Element = ui! {
        view {
            for (k, v) in &map {
                text { format!("{}={}", k, v) }
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
    let tree: Element = ui! {
        view {
            ["x", "y"].iter().map(|s| ui! { text { s.to_string() } }).collect::<Vec<_>>()
        }
    };
    let _owner = rt.render(tree);

    assert_eq!(texts(&rt.events()), vec!["x", "y"]);
}

#[test]
fn static_range_maps_to_components() {
    let rt = TestRuntime::new();
    let tree: Element = ui! {
        view {
            for i in 0..3 {
                text { format!("r{}", i) }
            }
        }
    };
    let _owner = rt.render(tree);

    let mut got = texts(&rt.events());
    got.sort();
    assert_eq!(got, vec!["r0".to_string(), "r1".to_string(), "r2".to_string()]);
}

// ---------------------------------------------------------------------------
// Type-driven reactive iteration — Signal<Vec<_>> via Element::Each
// ---------------------------------------------------------------------------

/// `for x in sig { ... }` (where `sig: Signal<Vec<_>>`) is reactive
/// because the *type* is a signal — it lowers to a `Element::Each`
/// that rebuilds the whole list whenever the signal changes. No
/// `.get()` needed; push / shrink / replace / empty all re-render.
#[test]
fn signal_iteration_rebuilds_on_change() {
    let rt = TestRuntime::new();
    let data: Signal<Vec<&'static str>> = signal!(vec!["a", "b"]);
    let tree: Element = ui! {
        view {
            for s in data, key = *s {
                text { s.to_string() }
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

/// Non-splice fallback: a backend WITHOUT `supports_child_splice` can't
/// reconcile by key, so `Element::Each` full-rebuilds — dropping the
/// PREVIOUS list's scope before building the new one (freeing every
/// row's signals/effects). Each row registers an `on_cleanup`; growing
/// the list fires exactly the old rows' cleanups (no leak, atomic
/// teardown). The keyed scope-preservation tests below cover the
/// splice-capable path where this does NOT happen.
#[test]
fn each_releases_old_row_scopes_on_rebuild() {
    let rt = TestRuntime::new();
    let cleaned = Rc::new(Cell::new(0usize));
    let data: Signal<Vec<i32>> = signal!(vec![1, 2]);
    let c = cleaned.clone();
    let tree: Element = ui! {
        view {
            for n in data, key = *n {
                {
                    let c2 = c.clone();
                    on_cleanup(move || c2.set(c2.get() + 1));
                    ui! { text { n.to_string() } }
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
    let tree: Element = ui! {
        view {
            for s in data.get() {
                text { s.to_string() }
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
    let tree: Element = ui! {
        view {
            for i in 0..n.get() {
                text { format!("#{}", i) }
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
    let tree: Element = ui! {
        view {
            for n in data, key = *n {
                text { format!("h{}", n) }
                text { format!("b{}", n) }
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
// Reactive list — flat_list (the supported reactive-iteration path)
// ---------------------------------------------------------------------------

/// `flat_list(data = sig, render = ...)` builds a reactive Virtualizer:
/// it mounts as a virtualizer node, and a data-signal change notifies
/// the backend via `virtualizer_data_changed`. (Row content isn't
/// observable in the mock — it doesn't drive `mount_item`.)
#[test]
fn reactive_flat_list_creates_virtualizer_and_reacts_to_data() {
    let rt = TestRuntime::new();
    let data: Signal<Vec<i32>> = signal!(vec![1, 2, 3]);
    let tree: Element = ui! {
        view {
            flat_list(
                data = data,
                render = |_idx, item: &i32| ui! { text { format!("item {}", item) } },
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
    let tree: Element = ui! {
        flat_list(
            data = data,
            render = |_idx, item: &i32| ui! { text { format!("item {}", item) } },
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
    let tree: Element = ui! {
        flat_list(
            data = data,
            render = |_idx, item: &i32| ui! { text { format!("v{}", item) } },
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
    let tree: Element = ui! {
        flat_list(
            data = data,
            render = move |_idx, item: &i32| {
                let c2 = c.clone();
                on_cleanup(move || c2.set(c2.get() + 1));
                ui! { text { format!("v{}", item) } }
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
/// `Element::Virtualizer` (count `Derived` + `row_template` for
/// generator backends). Regression: its runtime `render_item` must
/// build the REAL row body — previously it was a hardcoded empty-View
/// placeholder, so this construct rendered blank rows on every runtime
/// backend (web/iOS/Android/wgpu).
#[test]
fn for_count_signal_renders_real_rows_not_placeholder() {
    let count_sig: Signal<usize> = signal!(3);
    let tree: Element = ui! {
        for _i in structured_count(count_sig) {
            text { "row".to_string() }
        }
    };
    match tree {
        Element::Virtualizer { render_item, item_count, row_template, .. } => {
            // Count derives from the signal-reading call.
            assert_eq!((item_count.compute)(), 3, "item_count derives from count(sig)");
            // Structured form preserved for generator backends (Roku).
            assert!(row_template.is_some(), "row_template kept for generator backends");
            // THE FIX: runtime render builds the real row, not a placeholder View.
            assert!(
                matches!(render_item(0), Element::Text { .. }),
                "render_item must build the real row body (Text), not an empty placeholder View",
            );
        }
        _ => panic!("`for i in count(sig)` must lower to Element::Virtualizer"),
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
    let tree: Element = ui! {
        for _i in structured_count(n) {
            text { "cell".to_string() }
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
    let tree: Element = ui! {
        view {
            if 3 < 4 {
                text { "yes".to_string() }
            } else {
                text { "no".to_string() }
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
    let tree: Element = ui! {
        view {
            if flag.get() {
                text { "on".to_string() }
            } else {
                text { "off".to_string() }
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
    let tree: Element = ui! {
        view {
            if show.get() {
                text { "visible".to_string() }
            }
        }
    };
    let _owner = rt.render(tree);
    assert_eq!(count_text(&rt.events(), "visible"), 0, "hidden initially");

    rt.backend_mut().clear_events();
    show.set(true);
    assert_eq!(count_text(&rt.events(), "visible"), 1, "shown after toggle");
}

/// Type-carried reactivity: an `if` whose condition is a BARE reactive bool
/// — a `Signal<bool>` (what `memo(...)` returns) — re-evaluates on every
/// change. The `ui!` macro dispatches on the condition's *type*
/// (`ReactiveCond` for `Signal<bool>`), so no `.get()` is needed at the call
/// site. This is the canonical fix for the whiteboard "delete button won't
/// disappear" bug: `del_visible` is authored as a `memo`, and `if del_visible`
/// reacts when the underlying list shrinks to one. Mirrors `for x in sig`
/// being reactive by type.
#[test]
fn reactive_if_bare_signal_bool_reevaluates() {
    let rt = TestRuntime::new();
    let ids: Signal<Vec<i32>> = signal!(vec![1, 2]);
    let del_visible: Signal<bool> = memo(move || ids.get().len() > 1);
    let tree: Element = ui! {
        view {
            if del_visible {
                text { "del".to_string() }
            }
        }
    };
    let _owner = rt.render(tree);
    assert_eq!(count_text(&rt.events(), "del"), 1, "marker present while >1 item");

    // Shrink to one → the memo recomputes false and the branch is TORN DOWN.
    // The default backend uses the anchored `when` path, so the teardown is
    // observable as a `ClearChildren` on the region's anchor.
    rt.backend_mut().clear_events();
    ids.set(vec![1]);
    assert!(
        rt.events().iter().any(|e| matches!(e, Event::ClearChildren { .. })),
        "shrinking must tear the branch down (reactive `when`), not leave a stale marker: {:?}",
        rt.events()
    );

    // Grow back → the branch is rebuilt, proving the subscription is live both ways.
    rt.backend_mut().clear_events();
    ids.set(vec![1, 2, 3]);
    assert_eq!(count_text(&rt.events(), "del"), 1, "marker returns when the list grows again");
}

/// The Tier-1 idiom: a VISIBLE `.get()` read on a reactive bool
/// (`if del_visible.get()`) is reactive too — the macro detects the inline
/// `.get()` syntactically and wraps it (its *type* is plain `bool`, so the
/// type-driven dispatch alone would treat it as static). Proves the visible-
/// read path still works alongside the bare-type path.
#[test]
fn reactive_if_visible_get_on_memo_reevaluates() {
    let rt = TestRuntime::new();
    let ids: Signal<Vec<i32>> = signal!(vec![1, 2]);
    let del_visible: Signal<bool> = memo(move || ids.get().len() > 1);
    let tree: Element = ui! {
        view {
            if del_visible.get() {
                text { "del".to_string() }
            }
        }
    };
    let _owner = rt.render(tree);
    assert_eq!(count_text(&rt.events(), "del"), 1, "marker present while >1 item");

    rt.backend_mut().clear_events();
    ids.set(vec![1]);
    assert!(
        rt.events().iter().any(|e| matches!(e, Event::ClearChildren { .. })),
        "visible `.get()` condition must react on change: {:?}",
        rt.events()
    );
}

/// The deliberate counterpart: an `if` whose condition is an OPAQUE bare
/// closure CALL returning `bool` (`if del_visible()`) is STATIC under the
/// type-carried model — its type is `bool`, so it dispatches to `StaticCond`
/// and is built exactly once, allocating zero reactive machinery. Changing
/// the underlying signal produces NO teardown and NO rebuild. This is the
/// intended trade: reactivity must be expressed by a reactive TYPE
/// (`memo`/`Signal<bool>`) or a visible `.get()`, not inferred from an opaque
/// call — so a genuinely static `if helper()` never pays for an unused
/// reactive region.
#[test]
fn static_if_opaque_bool_call_does_not_react() {
    let rt = TestRuntime::new();
    let ids: Signal<Vec<i32>> = signal!(vec![1, 2]);
    let del_visible = move || ids.get().len() > 1; // plain `impl Fn() -> bool`
    let tree: Element = ui! {
        view {
            if del_visible() {
                text { "del".to_string() }
            }
        }
    };
    let _owner = rt.render(tree);
    assert_eq!(count_text(&rt.events(), "del"), 1, "static branch built once at len 2");

    // Mutating the signal does nothing — the condition was a build-time call,
    // not a reactive region: no anchor, no Effect, no teardown, no rebuild.
    rt.backend_mut().clear_events();
    ids.set(vec![1]);
    assert!(
        rt.events().is_empty(),
        "opaque-call condition is static; signal mutation must produce no events: {:?}",
        rt.events()
    );
}

/// Flipping a reactive `if` drops the OLD branch's scope before
/// building the new one — freeing its signals/effects. The then-branch
/// registers an `on_cleanup`; flipping to else must fire it exactly
/// once (no leak, atomic teardown — the conditional analog of
/// `each_releases_old_row_scopes_on_rebuild`).
#[test]
fn reactive_if_flip_releases_old_branch_scope() {
    let rt = TestRuntime::new();
    let cleaned = Rc::new(Cell::new(0usize));
    let flag: Signal<bool> = signal!(true);
    let c = cleaned.clone();
    let tree: Element = ui! {
        view {
            if flag.get() {
                {
                    let c2 = c.clone();
                    on_cleanup(move || c2.set(c2.get() + 1));
                    ui! { text { "on".to_string() } }
                }
            } else {
                text { "off".to_string() }
            }
        }
    };
    let _owner = rt.render(tree); // then-branch built, 1 on_cleanup registered
    assert_eq!(cleaned.get(), 0, "nothing torn down on first build");

    flag.set(false); // flip → drop then-scope → cleanup fires
    assert_eq!(cleaned.get(), 1, "old branch scope freed on flip");
}

/// `if let PAT = EXPR { … }` keeps its plain-Rust lowering under the
/// type-driven dispatch: the condition is a refutable `let`, not a `bool` or
/// `Signal<bool>`, so it must NOT route through `__idealyst_if` (which would
/// try to call a method on a `let` expression). The binding is visible in the
/// then-branch and the else-branch renders when the pattern doesn't match.
#[test]
fn if_let_binding_lowers_to_plain_if_let() {
    let rt = TestRuntime::new();
    let some: Option<&str> = Some("hi");
    let none: Option<&str> = None;
    let tree: Element = ui! {
        view {
            if let Some(s) = some {
                text { format!("got {s}") }
            } else {
                text { "empty-some".to_string() }
            }
            if let Some(s) = none {
                text { format!("got {s}") }
            } else {
                text { "empty-none".to_string() }
            }
        }
    };
    let _owner = rt.render(tree);
    let t = texts(&rt.events());
    assert_eq!(t, vec!["got hi", "empty-none"], "if let binds + else falls through: {t:?}");
}

// ---------------------------------------------------------------------------
// Match — static and reactive
// ---------------------------------------------------------------------------

#[test]
fn static_match_mounts_single_arm() {
    let rt = TestRuntime::new();
    let mode = 2;
    let tree: Element = ui! {
        view {
            match mode {
                1 => { text { "one".to_string() } }
                2 => { text { "two".to_string() } }
                _ => { text { "other".to_string() } }
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
    let tree: Element = ui! {
        view {
            match mode.get() {
                0 => { text { "zero".to_string() } }
                1 => { text { "one".to_string() } }
                _ => { text { "fallback".to_string() } }
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

/// Changing a reactive `match`'s arm drops the OLD arm's scope before
/// building the new one. The matched arm registers an `on_cleanup`;
/// switching to a different arm must fire it (no leak — the `switch`
/// analog of the `if`-flip teardown).
#[test]
fn reactive_match_arm_change_releases_old_arm_scope() {
    let rt = TestRuntime::new();
    let cleaned = Rc::new(Cell::new(0usize));
    let mode: Signal<u32> = signal!(0u32);
    let c = cleaned.clone();
    let tree: Element = ui! {
        view {
            match mode.get() {
                0 => {
                    {
                        let c2 = c.clone();
                        on_cleanup(move || c2.set(c2.get() + 1));
                        ui! { text { "zero".to_string() } }
                    }
                }
                _ => { text { "other".to_string() } }
            }
        }
    };
    let _owner = rt.render(tree); // arm 0 built, 1 on_cleanup registered
    assert_eq!(cleaned.get(), 0, "nothing torn down on first build");

    mode.set(1); // switch to the default arm → drop arm-0 scope → cleanup
    assert_eq!(cleaned.get(), 1, "old arm scope freed on arm change");
}

// ---------------------------------------------------------------------------
// Static if/match in children position — FLAT siblings, no wrapper View
// ---------------------------------------------------------------------------

/// A static `if` with a multi-node branch in children position emits its
/// branch nodes as FLAT siblings — no wrapper `View`. Lever: a wrapper
/// would show up as an extra `CreateView` (before the context-aware fix
/// this was 2: outer + wrapper).
#[test]
fn static_if_multi_node_branch_flattens_in_children() {
    let rt = TestRuntime::new();
    let tree: Element = ui! {
        view {
            if 1 < 2 {
                text { "a".to_string() }
                text { "b".to_string() }
            }
        }
    };
    let _owner = rt.render(tree);
    let ev = rt.events();
    assert_eq!(texts(&ev), vec!["a", "b"], "branch nodes are flat siblings");
    assert_eq!(
        ev.iter().filter(|e| matches!(e, Event::CreateView)).count(),
        1,
        "only the outer View — no wrapper around the if branch: {ev:?}"
    );
}

/// A static `if` with no `else`, condition false, contributes NOTHING —
/// not an empty-`View` placeholder. Before the fix the absent `else`
/// emitted `into_element(view(vec![]))`, creating a stray View.
#[test]
fn static_if_no_else_false_adds_no_empty_view() {
    let rt = TestRuntime::new();
    let tree: Element = ui! {
        view {
            if 1 > 2 {
                text { "never".to_string() }
            }
        }
    };
    let _owner = rt.render(tree);
    let ev = rt.events();
    assert_eq!(count_text(&ev, "never"), 0, "false branch renders nothing");
    assert_eq!(
        ev.iter().filter(|e| matches!(e, Event::CreateView)).count(),
        1,
        "no empty-View placeholder for the absent else: {ev:?}"
    );
}

/// A static `match` with a multi-node arm in children position flattens
/// the arm's nodes as siblings — no per-arm wrapper View.
#[test]
fn static_match_multi_node_arm_flattens_in_children() {
    let rt = TestRuntime::new();
    let mode = 1;
    let tree: Element = ui! {
        view {
            match mode {
                1 => {
                    text { "x".to_string() }
                    text { "y".to_string() }
                }
                _ => { text { "z".to_string() } }
            }
        }
    };
    let _owner = rt.render(tree);
    let ev = rt.events();
    assert_eq!(texts(&ev), vec!["x", "y"], "multi-node arm flattens");
    assert_eq!(
        ev.iter().filter(|e| matches!(e, Event::CreateView)).count(),
        1,
        "no wrapper around the match arm: {ev:?}"
    );
}

// ---------------------------------------------------------------------------
// Anchorless reactive regions (capability-gated) — foundation for the
// runtime-decided control-flow lowering. NOT wired into the macro yet;
// exercised here directly via `for x in signal` (→ Element::Each).
// ---------------------------------------------------------------------------

/// With `supports_child_splice`, a reactive region splices its rows
/// DIRECTLY into the parent — no `create_reactive_anchor` wrapper — and
/// rebuilds in place by removing exactly its prior rows via
/// `remove_child`. This is the anchorless boundary that lets static
/// control flow stay flat on every backend once the macro flips.
#[test]
fn anchorless_region_splices_flat_in_place() {
    let rt = TestRuntime::with_config(MockBackendConfig {
        supports_child_splice: true,
        ..Default::default()
    });
    let data: Signal<Vec<&'static str>> = signal!(vec!["a", "b"]);
    let tree: Element = ui! {
        view {
            for x in data, key = *x {
                text { x.to_string() }
            }
        }
    };
    let _owner = rt.render(tree);
    let ev = rt.events();
    assert_eq!(
        ev.iter().filter(|e| matches!(e, Event::CreateReactiveAnchor)).count(),
        0,
        "anchorless region creates NO wrapper anchor: {ev:?}"
    );
    assert_eq!(texts(&ev), vec!["a", "b"], "rows are direct children");

    // Rebuild: prior rows removed one-by-one via remove_child (not
    // clear_children), new row built — splice in place.
    rt.backend_mut().clear_events();
    data.set(vec!["c"]);
    let ev2 = rt.events();
    assert_eq!(
        ev2.iter().filter(|e| matches!(e, Event::RemoveChild { .. })).count(),
        2,
        "both prior rows removed via remove_child: {ev2:?}"
    );
    assert_eq!(texts(&ev2), vec!["c"], "rebuilt row content");
    assert_eq!(
        ev2.iter().filter(|e| matches!(e, Event::CreateReactiveAnchor)).count(),
        0,
        "still no anchor on rebuild"
    );
}

/// Capability gate: a backend WITHOUT `supports_child_splice` keeps the
/// anchored path — the same reactive region nests its rows under one
/// `create_reactive_anchor`. Proves the anchorless behavior is opt-in
/// and the default (and unmigrated backends) are unaffected.
#[test]
fn default_backend_uses_anchored_region() {
    let rt = TestRuntime::new(); // no splice support
    let data: Signal<Vec<&'static str>> = signal!(vec!["a", "b"]);
    let tree: Element = ui! {
        view {
            for x in data, key = *x {
                text { x.to_string() }
            }
        }
    };
    let _owner = rt.render(tree);
    let ev = rt.events();
    assert_eq!(
        ev.iter().filter(|e| matches!(e, Event::CreateReactiveAnchor)).count(),
        1,
        "without splice support the region uses an anchor: {ev:?}"
    );
    assert_eq!(texts(&ev), vec!["a", "b"]);
}

/// Mid-list correctness: a region with siblings BEFORE and AFTER it
/// splices at its stable position (`base_index`) via `insert_at`, not
/// at the end. On rebuild the new rows land before the trailing sibling
/// and the sibling is left untouched.
#[test]
fn anchorless_region_splices_at_position_among_siblings() {
    let rt = TestRuntime::with_config(MockBackendConfig {
        supports_child_splice: true,
        ..Default::default()
    });
    let data: Signal<Vec<&'static str>> = signal!(vec!["a", "b"]);
    let tree: Element = ui! {
        view {
            text { "header".to_string() }
            for x in data, key = *x {
                text { x.to_string() }
            }
            text { "footer".to_string() }
        }
    };
    let _owner = rt.render(tree);
    let ev = rt.events();
    // header is child 0 (plain insert); the region splices its rows at
    // base_index 1 → a@1, b@2; footer appends after.
    let at: Vec<usize> = ev
        .iter()
        .filter_map(|e| match e {
            Event::InsertAt { index, .. } => Some(*index),
            _ => None,
        })
        .collect();
    assert_eq!(at, vec![1, 2], "region rows splice right after header: {ev:?}");
    assert_eq!(texts(&ev), vec!["header", "a", "b", "footer"]);

    // Rebuild (shrink to one): remove a,b; splice the new row back at
    // the region's position (before footer); footer untouched.
    rt.backend_mut().clear_events();
    data.set(vec!["c"]);
    let ev2 = rt.events();
    assert_eq!(
        ev2.iter().filter(|e| matches!(e, Event::RemoveChild { .. })).count(),
        2,
        "the two prior rows are removed: {ev2:?}"
    );
    let reinsert: Vec<usize> = ev2
        .iter()
        .filter_map(|e| match e {
            Event::InsertAt { index, .. } => Some(*index),
            _ => None,
        })
        .collect();
    assert_eq!(reinsert, vec![1], "new row re-spliced at the region's position: {ev2:?}");
    assert_eq!(count_text(&ev2, "c"), 1, "new row built");
    assert_eq!(count_text(&ev2, "footer"), 0, "trailing sibling untouched on rebuild");
}

// ---------------------------------------------------------------------------
// Anchorless `when` (reactive conditional) — the fix for the Android
// "`when`-mounted box never appears" bug. On a splice-capable backend a
// style-less `when` mounts its active branch DIRECTLY into the parent (no
// `create_reactive_anchor` wrapper), so an absolutely-positioned branch
// resolves its containing block against the real parent — matching web's
// `display:contents` anchor — instead of a wrapper view that collapses to
// 0×0 (and on a native FrameLayout never paints its absolute child). The
// wrapper-collapse can't be observed against the mock (it has no layout),
// so these tests pin the mechanism that fixes it: the branch is spliced
// into the real parent via `insert_at` / `remove_child`, never nested
// under an anchor. The device screenshot in the PR is the visual proof.
// ---------------------------------------------------------------------------

/// With `supports_child_splice`, a `when` splices its active branch
/// directly into the parent at the region's `base_index` (no wrapper
/// anchor). Toggling the condition removes exactly the prior branch node
/// via `remove_child` and re-splices the new branch via `insert_at` — it
/// never creates a `create_reactive_anchor`.
#[test]
fn anchorless_when_splices_branch_without_anchor() {
    let rt = TestRuntime::with_config(MockBackendConfig {
        supports_child_splice: true,
        ..Default::default()
    });
    let on: Signal<bool> = signal!(false);
    let tree: Element = view(vec![
        text("header").into_element(),
        when(
            move || on.get(),
            || text("then").into_element(),
            || text("otherwise").into_element(),
        ),
        text("footer").into_element(),
    ])
    .into_element();
    let _owner = rt.render(tree);
    let ev = rt.events();
    assert_eq!(
        ev.iter().filter(|e| matches!(e, Event::CreateReactiveAnchor)).count(),
        0,
        "anchorless `when` creates NO wrapper anchor: {ev:?}"
    );
    // Initial: cond=false → "otherwise", spliced at base_index 1 (after
    // the static "header"), before the trailing "footer".
    assert_eq!(texts(&ev), vec!["header", "otherwise", "footer"]);
    let at: Vec<usize> = ev
        .iter()
        .filter_map(|e| match e {
            Event::InsertAt { index, .. } => Some(*index),
            _ => None,
        })
        .collect();
    assert_eq!(at, vec![1], "branch splices right after header: {ev:?}");

    // Toggle true: the prior "otherwise" node is removed via remove_child
    // (NOT clear_children), and the new "then" branch is spliced back at
    // the region's position. The footer is left untouched.
    rt.backend_mut().clear_events();
    on.set(true);
    let ev2 = rt.events();
    assert_eq!(
        ev2.iter().filter(|e| matches!(e, Event::RemoveChild { .. })).count(),
        1,
        "the prior branch node is removed via remove_child: {ev2:?}"
    );
    assert_eq!(
        ev2.iter().filter(|e| matches!(e, Event::ClearChildren { .. })).count(),
        0,
        "anchorless `when` never clears a wrapper's children: {ev2:?}"
    );
    let reinsert: Vec<usize> = ev2
        .iter()
        .filter_map(|e| match e {
            Event::InsertAt { index, .. } => Some(*index),
            _ => None,
        })
        .collect();
    assert_eq!(reinsert, vec![1], "new branch re-spliced at the region's position: {ev2:?}");
    assert_eq!(count_text(&ev2, "then"), 1, "active branch rebuilt");
    assert_eq!(
        ev2.iter().filter(|e| matches!(e, Event::CreateReactiveAnchor)).count(),
        0,
        "still no anchor on toggle"
    );
}

/// Capability gate: WITHOUT `supports_child_splice` the same `when` keeps
/// the anchored path — its branch nests under one `create_reactive_anchor`
/// and toggling clears that wrapper's children. Proves the anchorless
/// behavior is opt-in; backends that haven't implemented splice are
/// unaffected (the framework default stays the closure/anchor path).
#[test]
fn default_backend_when_uses_anchored_path() {
    let rt = TestRuntime::new(); // no splice support
    let on: Signal<bool> = signal!(false);
    let tree: Element = view(vec![when(
        move || on.get(),
        || text("then").into_element(),
        || text("otherwise").into_element(),
    )])
    .into_element();
    let _owner = rt.render(tree);
    let ev = rt.events();
    assert_eq!(
        ev.iter().filter(|e| matches!(e, Event::CreateReactiveAnchor)).count(),
        1,
        "without splice support `when` uses an anchor: {ev:?}"
    );
    assert_eq!(count_text(&ev, "otherwise"), 1);

    // Toggle: anchored path tears down the branch via clear_children on
    // the wrapper (not remove_child), and never splices via insert_at.
    rt.backend_mut().clear_events();
    on.set(true);
    let ev2 = rt.events();
    assert_eq!(
        ev2.iter().filter(|e| matches!(e, Event::ClearChildren { .. })).count(),
        1,
        "anchored `when` clears the wrapper on toggle: {ev2:?}"
    );
    assert_eq!(
        ev2.iter().filter(|e| matches!(e, Event::InsertAt { .. })).count(),
        0,
        "anchored `when` never uses insert_at: {ev2:?}"
    );
    assert_eq!(count_text(&ev2, "then"), 1, "active branch rebuilt");
}

// ---------------------------------------------------------------------------
// Keyed reconciliation — the reason a reactive `for` requires a `key`.
//
// On a splice-capable backend, a keyed list matches rows across a
// rebuild by key: unchanged keys keep their backend nodes AND their
// render scope (so component-local signals/effects inside a row survive
// the list mutation), removed keys are dropped, new keys are built,
// surviving rows are moved into the new order. These tests pin the
// scope-lifecycle contract that makes that true: they would FAIL under
// the old full-rebuild model (which tore down and rebuilt every row).
// ---------------------------------------------------------------------------

/// Adding a row keeps the existing rows' scopes intact: no `on_cleanup`
/// fires and only the NEW row is built. (Full rebuild would have fired
/// both existing cleanups and re-created every row.)
#[test]
fn keyed_add_preserves_existing_row_scopes() {
    let rt = TestRuntime::with_config(MockBackendConfig {
        supports_child_splice: true,
        ..Default::default()
    });
    let cleaned = Rc::new(Cell::new(0usize));
    let data: Signal<Vec<i32>> = signal!(vec![1, 2]);
    let c = cleaned.clone();
    let tree: Element = ui! {
        view {
            for n in data, key = *n {
                {
                    let c2 = c.clone();
                    on_cleanup(move || c2.set(c2.get() + 1));
                    ui! { text { n.to_string() } }
                }
            }
        }
    };
    let _owner = rt.render(tree);
    assert_eq!(cleaned.get(), 0, "nothing torn down on first build");
    assert_eq!(texts(&rt.events()), vec!["1", "2"], "both rows built initially");

    // keys [1,2] → [1,2,3]: 1 and 2 are unchanged → preserved.
    rt.backend_mut().clear_events();
    data.set(vec![1, 2, 3]);
    assert_eq!(cleaned.get(), 0, "unchanged rows' scopes preserved on add");
    assert_eq!(texts(&rt.events()), vec!["3"], "only the new row is built");
}

/// Removing a middle row drops EXACTLY that row's scope (its `on_cleanup`
/// fires, its node is removed) and leaves every other row's scope and
/// nodes untouched — no surviving row is rebuilt.
#[test]
fn keyed_remove_drops_only_removed_row_scope() {
    let rt = TestRuntime::with_config(MockBackendConfig {
        supports_child_splice: true,
        ..Default::default()
    });
    let removed = Rc::new(RefCell::new(Vec::<i32>::new()));
    let data: Signal<Vec<i32>> = signal!(vec![1, 2, 3]);
    let r = removed.clone();
    let tree: Element = ui! {
        view {
            for n in data, key = *n {
                {
                    let r2 = r.clone();
                    on_cleanup(move || r2.borrow_mut().push(n));
                    ui! { text { n.to_string() } }
                }
            }
        }
    };
    let _owner = rt.render(tree);
    assert!(removed.borrow().is_empty(), "nothing dropped on first build");

    // Remove the middle key (2); 1 and 3 survive.
    rt.backend_mut().clear_events();
    data.set(vec![1, 3]);
    assert_eq!(*removed.borrow(), vec![2], "only the removed row's scope is dropped");
    assert_eq!(texts(&rt.events()), Vec::<String>::new(), "surviving rows are NOT rebuilt");
    assert_eq!(
        rt.events().iter().filter(|e| matches!(e, Event::RemoveChild { .. })).count(),
        1,
        "exactly one node removed (the dropped row's): {:?}",
        rt.events()
    );
}

/// Removing a key then re-adding the SAME key gives the re-added row a
/// FRESH scope — its prior scope was dropped on removal, so any
/// component-local state resets. (Identity is the key's *presence*, not
/// its value-ever-having-existed: a key that left and came back is a new
/// row.) Proven by the cleanup ledger: the row is torn down on removal,
/// rebuilt on re-add, and torn down AGAIN when removed a second time —
/// which can only happen if the re-add installed a fresh scope + cleanup.
#[test]
fn keyed_readd_same_key_gets_fresh_scope() {
    let rt = TestRuntime::with_config(MockBackendConfig {
        supports_child_splice: true,
        ..Default::default()
    });
    let removed = Rc::new(RefCell::new(Vec::<i32>::new()));
    let data: Signal<Vec<i32>> = signal!(vec![1, 2, 3]);
    let r = removed.clone();
    let tree: Element = ui! {
        view {
            for n in data, key = *n {
                {
                    let r2 = r.clone();
                    on_cleanup(move || r2.borrow_mut().push(n));
                    ui! { text { n.to_string() } }
                }
            }
        }
    };
    let _owner = rt.render(tree);

    // Remove key 2 → its scope is dropped (cleanup fires once).
    data.set(vec![1, 3]);
    assert_eq!(*removed.borrow(), vec![2], "removed row 2's scope dropped");

    // Re-add key 2 → it was NOT mounted, so it's built fresh (a new
    // `CreateText "2"`), not resurrected.
    rt.backend_mut().clear_events();
    data.set(vec![1, 3, 2]);
    assert_eq!(texts(&rt.events()), vec!["2"], "re-added key is built fresh");
    assert_eq!(*removed.borrow(), vec![2], "no extra cleanup yet (fresh row still mounted)");

    // Remove it again → the FRESH scope's cleanup fires, proving the
    // re-add installed a brand-new scope (state would have reset).
    data.set(vec![1, 3]);
    assert_eq!(*removed.borrow(), vec![2, 2], "the re-added row had its own fresh scope");
}

/// Reordering keeps every row's scope (no cleanup fires) and rebuilds
/// nothing (no new `CreateText`); the existing nodes are repositioned via
/// `insert_at` into the new order.
#[test]
fn keyed_reorder_preserves_scopes_without_rebuild() {
    let rt = TestRuntime::with_config(MockBackendConfig {
        supports_child_splice: true,
        ..Default::default()
    });
    let cleaned = Rc::new(Cell::new(0usize));
    let data: Signal<Vec<i32>> = signal!(vec![1, 2, 3]);
    let c = cleaned.clone();
    let tree: Element = ui! {
        view {
            for n in data, key = *n {
                {
                    let c2 = c.clone();
                    on_cleanup(move || c2.set(c2.get() + 1));
                    ui! { text { n.to_string() } }
                }
            }
        }
    };
    let _owner = rt.render(tree);

    // Rotate [1,2,3] → [3,1,2]: same keys, new order.
    rt.backend_mut().clear_events();
    data.set(vec![3, 1, 2]);
    assert_eq!(cleaned.get(), 0, "no row dropped on reorder");
    assert_eq!(texts(&rt.events()), Vec::<String>::new(), "no row rebuilt on reorder");
    assert_eq!(
        rt.events().iter().filter(|e| matches!(e, Event::InsertAt { .. })).count(),
        3,
        "all three nodes repositioned into the new order: {:?}",
        rt.events()
    );
}
