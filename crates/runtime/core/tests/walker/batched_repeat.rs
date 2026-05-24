//! Batched-`Repeat` fast path — `execute_batch_with_attach`.
//!
//! When a backend opts into `supports_batched_repeat() = true` and a
//! `Primitive::Repeat` has rows that match the batchable shape (View
//! + Text + static-style, no `ref_fill`, no `on_touch`, no
//! `safe_area_sides`), the walker collapses the whole expansion into
//! one [`Backend::execute_batch_with_attach`] call.
//!
//! Without the opt-in, the same Repeat goes through per-row
//! `create_*` + `apply_style` + `insert` plus a final `insert_many`.
//!
//! These tests use [`MockBackend`] with
//! [`MockBackendConfig::supports_batched_repeat`] flipped to `true`
//! to exercise the fast path, and the default (false) to verify the
//! fallback. The mock backend records [`Event::ExecuteBatch`] /
//! [`Event::ExecuteBatchWithAttach`] when the batched path fires —
//! events that NEVER appear when batching is off.

use std::cell::Cell;
use std::rc::Rc;

use runtime_core::{
    text, view, IntoPrimitive, Primitive, StyleApplication, StyleRules, StyleSheet, VariantSet,
};

use crate::common::{BatchOpSummary, Event, MockBackendConfig, TestRuntime};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a trivial static stylesheet for tests. Content doesn't matter
/// — the walker only cares that the resulting `StyleSource` is
/// `Static` so the batched path takes it.
fn make_static_sheet() -> Rc<StyleSheet> {
    Rc::new(StyleSheet::new(|_vs: &VariantSet| StyleRules::default()))
}

/// The bench's batchable shape: `View > [Repeat { rows of View > Text
/// with static style }]`. Returns the outer container Primitive that
/// the test runtime will render.
fn batchable_tree(count: usize, sheet: Rc<StyleSheet>) -> Primitive {
    let sheet_for_rows = sheet;
    let row_builder: Box<dyn Fn(usize) -> Primitive> = Box::new(move |i| {
        view(vec![text(format!("Row #{}", i)).into_primitive()])
            .with_style(sheet_for_rows.clone())
            .into_primitive()
    });
    view(vec![Primitive::Repeat { count, row_builder }]).into_primitive()
}

/// Count `ExecuteBatchWithAttach` events.
fn count_batched_with_attach(events: &[Event]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, Event::ExecuteBatchWithAttach { .. }))
        .count()
}

/// Count per-row `CreateText` events. Used to verify the fallback
/// path emits the granular event stream when batching is off.
fn count_row_create_texts(events: &[Event]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, Event::CreateText { content } if content.starts_with("Row #")))
        .count()
}

// ---------------------------------------------------------------------------
// Shape coverage
// ---------------------------------------------------------------------------

/// When the backend opts in via `supports_batched_repeat = true` and
/// the row shape is batchable, the walker takes the fast path: one
/// `execute_batch_with_attach` event, NO per-row `CreateText` /
/// `CreateView` / `Insert` / `ApplyStyle` events.
#[test]
fn batched_path_fires_when_backend_opts_in_and_shape_is_batchable() {
    let rt = TestRuntime::with_config(MockBackendConfig {
        supports_batched_repeat: true,
    });
    let _owner = rt.render(batchable_tree(10, make_static_sheet()));

    let events = rt.events();
    assert_eq!(
        count_batched_with_attach(&events),
        1,
        "expected exactly one ExecuteBatchWithAttach event; got {} (events: {:#?})",
        count_batched_with_attach(&events),
        events,
    );

    // No per-row CreateText events — those ops live inside the batch
    // stream, not in the top-level event log.
    assert_eq!(
        count_row_create_texts(&events),
        0,
        "batched path should NOT emit per-row CreateText events; got {}",
        count_row_create_texts(&events),
    );
}

/// Default config (`supports_batched_repeat = false`) keeps the
/// walker on the per-call path even for an otherwise-batchable shape.
/// Every row produces its own `CreateView`/`CreateText`/etc.
#[test]
fn per_call_path_when_backend_opts_out() {
    let rt = TestRuntime::new(); // default: supports_batched_repeat = false
    let _owner = rt.render(batchable_tree(10, make_static_sheet()));

    let events = rt.events();
    assert_eq!(
        count_batched_with_attach(&events),
        0,
        "backend opted out; no batched events should fire",
    );

    // Per-row events fire: 10 CreateText for the row labels.
    assert_eq!(
        count_row_create_texts(&events),
        10,
        "expected 10 per-row CreateText events on the fallback path",
    );
}

/// Even with the backend opting in, the walker must bail out of the
/// batched path when ANY row breaks the batchable shape (here:
/// reactive-style row). The whole Repeat falls back to per-call
/// expansion — no partial batches.
#[test]
fn non_batchable_shape_falls_back_to_per_call() {
    let rt = TestRuntime::with_config(MockBackendConfig {
        supports_batched_repeat: true,
    });
    let sheet = make_static_sheet();
    let row_builder: Box<dyn Fn(usize) -> Primitive> = Box::new(move |i| {
        let sheet_for_row = sheet.clone();
        view(vec![text(format!("Row #{}", i)).into_primitive()])
            // Closure form → `StyleSource::Reactive`, which the
            // walker explicitly bails on at
            // `enqueue_primitive`'s View arm.
            .with_style(move || StyleApplication::new(sheet_for_row.clone()))
            .into_primitive()
    });
    let tree = view(vec![Primitive::Repeat {
        count: 3,
        row_builder,
    }])
    .into_primitive();
    let _owner = rt.render(tree);

    let events = rt.events();
    assert_eq!(
        count_batched_with_attach(&events),
        0,
        "reactive style breaks the batchable shape; walker should bail to per-call",
    );
    assert_eq!(
        count_row_create_texts(&events),
        3,
        "fallback should emit one CreateText per row",
    );
}

// ---------------------------------------------------------------------------
// Batch contents
// ---------------------------------------------------------------------------

/// The batch's op stream must contain, per row:
///   1 CreateView + 1 CreateText + 1 Insert + 1 ApplyStyleStatic.
/// Row Text contents must appear in iteration order (`Row #0`,
/// `Row #1`, …).
#[test]
fn batch_contains_one_op_per_row_in_iteration_order() {
    let rt = TestRuntime::with_config(MockBackendConfig {
        supports_batched_repeat: true,
    });
    let _owner = rt.render(batchable_tree(5, make_static_sheet()));

    let events = rt.events();
    let (ops, node_count) = events
        .iter()
        .find_map(|e| match e {
            Event::ExecuteBatchWithAttach {
                ops, node_count, ..
            } => Some((ops.clone(), *node_count)),
            _ => None,
        })
        .expect("ExecuteBatchWithAttach event must be present");

    // 5 rows × (1 View + 1 Text) = 10 created nodes.
    assert_eq!(
        node_count, 10,
        "5 rows of View+Text should mint 10 local ids",
    );

    // Op kind counts.
    let create_view_count = ops
        .iter()
        .filter(|op| matches!(op, BatchOpSummary::CreateView { .. }))
        .count();
    let create_text_count = ops
        .iter()
        .filter(|op| matches!(op, BatchOpSummary::CreateText { .. }))
        .count();
    let insert_count = ops
        .iter()
        .filter(|op| matches!(op, BatchOpSummary::Insert { .. }))
        .count();
    let apply_style_count = ops
        .iter()
        .filter(|op| matches!(op, BatchOpSummary::ApplyStyleStatic { .. }))
        .count();

    assert_eq!(create_view_count, 5, "1 View per row × 5 rows");
    assert_eq!(create_text_count, 5, "1 Text per row × 5 rows");
    assert_eq!(insert_count, 5, "1 Insert per row (Text → View)");
    assert_eq!(apply_style_count, 5, "1 ApplyStyleStatic per row (the styled View)");

    // CreateText contents must come out in iteration order.
    let text_contents: Vec<&str> = ops
        .iter()
        .filter_map(|op| match op {
            BatchOpSummary::CreateText { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .collect();
    let expected: Vec<String> = (0..5).map(|i| format!("Row #{}", i)).collect();
    let expected_refs: Vec<&str> = expected.iter().map(String::as_str).collect();
    assert_eq!(
        text_contents, expected_refs,
        "row Texts must appear in iteration order",
    );
}

/// `attach_locals` must list each row top's `local_id`, in iteration
/// order — that's what the walker hands the backend to parent rows
/// under the containing View. Each id must correspond to a
/// `CreateView` op in the same batch (rows tops are Views).
#[test]
fn attach_locals_matches_row_top_create_view_ops() {
    let rt = TestRuntime::with_config(MockBackendConfig {
        supports_batched_repeat: true,
    });
    let _owner = rt.render(batchable_tree(4, make_static_sheet()));

    let events = rt.events();
    let (ops, attach_locals) = events
        .iter()
        .find_map(|e| match e {
            Event::ExecuteBatchWithAttach {
                ops,
                attach_locals,
                ..
            } => Some((ops.clone(), attach_locals.clone())),
            _ => None,
        })
        .expect("ExecuteBatchWithAttach event must be present");

    assert_eq!(
        attach_locals.len(),
        4,
        "4 rows should produce 4 attach_locals entries",
    );

    // Each `attach_locals[i]` must reference a `CreateView` op in the
    // batch's op stream (row top is a View).
    for (idx, &local) in attach_locals.iter().enumerate() {
        let hit = ops
            .iter()
            .any(|op| matches!(op, BatchOpSummary::CreateView { local_id } if *local_id == local));
        assert!(
            hit,
            "attach_locals[{}] = {} doesn't match any CreateView op; ops were: {:#?}",
            idx, local, ops,
        );
    }
}

/// The parent passed to `execute_batch_with_attach` must be the node
/// id of the surrounding `View` (the Repeat's container). The outer
/// `View` is minted by `create_view` before the batched call —
/// `MockBackend` records that as a `CreateView` event — and its id
/// is the first one minted (`NodeId(0)`).
#[test]
fn attach_parent_is_the_containing_view() {
    let rt = TestRuntime::with_config(MockBackendConfig {
        supports_batched_repeat: true,
    });
    let _owner = rt.render(batchable_tree(2, make_static_sheet()));

    let events = rt.events();

    // The outer `view(vec![Repeat])` is the first thing created.
    let outer_view_pos = events
        .iter()
        .position(|e| matches!(e, Event::CreateView))
        .expect("outer CreateView must precede the batched call");
    let batch_pos = events
        .iter()
        .position(|e| matches!(e, Event::ExecuteBatchWithAttach { .. }))
        .expect("batched call must fire");
    assert!(
        outer_view_pos < batch_pos,
        "outer CreateView ({}) should precede ExecuteBatchWithAttach ({})",
        outer_view_pos,
        batch_pos,
    );

    // The parent id should be the first node minted (NodeId(0)). The
    // MockBackend mints monotonically from 0; the outer view gets 0;
    // the batch's nodes start at 1. (This couples to MockBackend's id
    // policy — but that policy is also documented at the top of
    // `mock_backend.rs`, so the assertion is intentional.)
    let parent = match &events[batch_pos] {
        Event::ExecuteBatchWithAttach { parent, .. } => *parent,
        _ => unreachable!(),
    };
    assert_eq!(
        parent.raw(),
        0,
        "parent should be the outer view (first mint), got NodeId({})",
        parent.raw(),
    );
}

// ---------------------------------------------------------------------------
// Re-rebuild via Switch (mirrors the rebuild bench's setRows pattern)
// ---------------------------------------------------------------------------

/// When the Repeat re-renders under a Switch (the rebuild bench
/// pattern), the second mount must ALSO take the batched path. This
/// catches a regression where the batched path is only taken on the
/// initial render but not on rebuild.
#[test]
fn batched_path_is_taken_on_each_rebuild_via_switch() {
    use runtime_core::{signal, switch, Signal};

    let rt = TestRuntime::with_config(MockBackendConfig {
        supports_batched_repeat: true,
    });
    let mode: Signal<u32> = signal!(0u32);
    let count: Signal<usize> = signal!(2usize);
    let sheet = make_static_sheet();

    let tree = switch(
        move || mode.get(),
        move |m| match m {
            0 => {
                let n = count.get();
                let sheet_for_rows = sheet.clone();
                let row_builder: Box<dyn Fn(usize) -> Primitive> = Box::new(move |i| {
                    view(vec![text(format!("Row #{}", i)).into_primitive()])
                        .with_style(sheet_for_rows.clone())
                        .into_primitive()
                });
                view(vec![Primitive::Repeat {
                    count: n,
                    row_builder,
                }])
                .into_primitive()
            }
            _ => view(Vec::<Primitive>::new()).into_primitive(),
        },
    );
    let _owner = rt.render(tree);

    // Initial mount: 1 batched call.
    assert_eq!(
        count_batched_with_attach(&rt.events()),
        1,
        "initial mount should produce 1 batched call",
    );

    // Trigger rebuild — the bench's setRows pattern: write count
    // then touch the discriminant.
    rt.backend_mut().clear_events();
    count.set(7);
    mode.set(0);

    let after = rt.events();
    assert_eq!(
        count_batched_with_attach(&after),
        1,
        "rebuild must also take the batched path (got {}, events: {:#?})",
        count_batched_with_attach(&after),
        after,
    );

    // The rebuild's batch must contain 7 row tops, not 2.
    let (_, attach_locals) = after
        .iter()
        .find_map(|e| match e {
            Event::ExecuteBatchWithAttach {
                ops,
                attach_locals,
                ..
            } => Some((ops.clone(), attach_locals.clone())),
            _ => None,
        })
        .expect("batched event in rebuild");
    assert_eq!(attach_locals.len(), 7, "rebuild should batch 7 rows");
}

// ---------------------------------------------------------------------------
// Style registration still fires on the batched path
// ---------------------------------------------------------------------------

/// The batched fast path doesn't bypass stylesheet registration —
/// `register_stylesheet` must still fire for the row sheet. Without
/// this, theme-aware backends would never learn about the sheet and
/// the cohort wouldn't re-apply on theme swaps.
#[test]
fn batched_path_still_registers_stylesheet() {
    let rt = TestRuntime::with_config(MockBackendConfig {
        supports_batched_repeat: true,
    });
    let _owner = rt.render(batchable_tree(3, make_static_sheet()));

    let events = rt.events();
    let registered = events
        .iter()
        .filter(|e| matches!(e, Event::RegisterStylesheet { .. }))
        .count();
    assert!(
        registered >= 1,
        "expected at least one RegisterStylesheet event for the row sheet; got {} (events: {:#?})",
        registered,
        events,
    );
}

// ---------------------------------------------------------------------------
// Suppress `Cell` import warning when the file is read fresh.
// ---------------------------------------------------------------------------
#[allow(dead_code)]
fn _force_cell_use() -> Cell<u32> {
    Cell::new(0)
}
