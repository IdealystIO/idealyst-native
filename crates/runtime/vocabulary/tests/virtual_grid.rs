//! `virtual_grid` handler tests — the two-axis sibling of the
//! virtualizer suite in `virtualizer_graphics.rs`.
//!
//! What's asserted here is the FRAMEWORK contract: the mount sequence,
//! that both count closures feed the data effect, per-cell ownership
//! scopes, and the scroll surface. The windowing arithmetic is tested
//! where it lives — `runtime_shared::primitives::virtual_grid` — and
//! the DOM recycling in the web shim's own browser test.
//!
//! Per CLAUDE.md §8, each `#[test]` is named after the bug it prevents.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use host_mock::harness;
use runtime_shared::{StyleRules, Tokenized};
use runtime_scene::realize;
use runtime_vocabulary::builders::{text, virtual_grid};
use runtime_world::signal;

fn px(w: f32) -> StyleRules {
    StyleRules {
        width: Some(Tokenized::Literal(runtime_shared::Length::Px(w))),
        ..Default::default()
    }
}

/// A 4×3 grid of 100×40 cells, rendering `"c{col}r{row}"`.
fn grid_4x3() -> runtime_scene::Element {
    virtual_grid(
        || 4,
        || 3,
        |_| 100.0,
        |_| 40.0,
        |c, r| (c * 1000 + r) as u64,
        |c, r| text().content(format!("c{c}r{r}")).build(),
    )
    .build()
}

#[test]
fn mount_sequence_is_create_then_data_effect_then_style() {
    let h = harness();
    let _realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            virtual_grid(
                || 4,
                || 3,
                |_| 100.0,
                |_| 40.0,
                |c, r| (c * 1000 + r) as u64,
                |c, r| text().content(format!("c{c}r{r}")).build(),
            )
            .style(px(320.0))
            .build(),
        )
    });
    assert_eq!(
        h.take_log(),
        vec![
            "create n0 virtual_grid cols=4 rows=3 overscan=1".to_string(),
            "virtual_grid_data_changed n0".to_string(),
            "apply_style n0 width=Literal(Px(320.0))".to_string(),
        ],
        "same create → data-effect → style order as the virtualizer"
    );
}

/// Both axes must feed the data effect. Reading only one count would
/// leave the other axis's signal out of the effect's dependency set —
/// so adding a COLUMN would silently not re-window, which is exactly
/// the failure a schedule grid hits when its day range grows.
#[test]
fn regression_changing_only_one_axis_fails_to_rewindow() {
    let h = harness();
    let (_realized, cols, rows) = h.world.enter(|| {
        let cols = signal(4usize);
        let rows = signal(3usize);
        let realized = realize(
            &h.backend,
            &h.registry,
            virtual_grid(
                move || cols.get(),
                move || rows.get(),
                |_| 100.0,
                |_| 40.0,
                |c, r| (c * 1000 + r) as u64,
                |c, r| text().content(format!("c{c}r{r}")).build(),
            )
            .build(),
        );
        (realized, cols, rows)
    });
    h.take_log();

    // Column-only change must re-window.
    h.world.enter(|| cols.set(9));
    h.flush();
    assert_eq!(
        h.take_log(),
        vec!["virtual_grid_data_changed n0".to_string()],
        "a column-count change must re-window"
    );

    // Row-only change must too.
    h.world.enter(|| rows.set(7));
    h.flush();
    assert_eq!(
        h.take_log(),
        vec!["virtual_grid_data_changed n0".to_string()],
        "a row-count change must re-window"
    );
}

/// Cells are supplied lazily, by `(col, row)` — the backend asks, the
/// handler realizes. A grid that eagerly built its cells at mount
/// would defeat the entire primitive.
#[test]
fn cells_are_realized_lazily_on_backend_request() {
    let h = harness();
    let _realized = h
        .world
        .enter(|| realize(&h.backend, &h.registry, grid_4x3()));
    h.take_log();

    let cbs = h.virtual_grid(0);
    // Nothing mounted until the backend asks.
    assert!(h.take_log().is_empty(), "no cells before the backend asks");

    let (_node, scope) = h.world.enter(|| (cbs.mount_cell)(2, 1));
    assert_eq!(h.take_log(), vec!["create n1 text \"c2r1\"".to_string()]);

    h.world.enter(|| (cbs.release_cell)(scope));
}

/// Each cell owns its reactive state: releasing a cell must drop the
/// signals and effects created inside it, or a queued platform event
/// fires into freed state ("signal used after its scope was dropped").
#[test]
fn regression_released_cell_effects_keep_firing() {
    let h = harness();
    let fires = Rc::new(Cell::new(0usize));
    let probe = fires.clone();
    let tick = h.world.enter(|| signal(0usize));

    let _realized = h.world.enter(|| {
        let probe = probe.clone();
        realize(
            &h.backend,
            &h.registry,
            virtual_grid(
                || 2,
                || 2,
                |_| 50.0,
                |_| 50.0,
                |c, r| (c * 10 + r) as u64,
                move |_c, _r| {
                    let probe = probe.clone();
                    // An effect created during cell construction — the
                    // class of state that must die with the cell.
                    runtime_world::effect(move || {
                        let _ = tick.get();
                        probe.set(probe.get() + 1);
                    });
                    text().content("cell").build()
                },
            )
            .build(),
        )
    });

    let cbs = h.virtual_grid(0);
    let (_n, scope) = h.world.enter(|| (cbs.mount_cell)(0, 0));
    let after_mount = fires.get();
    assert!(after_mount >= 1, "effect runs once at creation");

    h.world.enter(|| tick.set(1));
    h.flush();
    let while_mounted = fires.get();
    assert!(while_mounted > after_mount, "effect fires while mounted");

    h.world.enter(|| (cbs.release_cell)(scope));
    h.world.enter(|| tick.set(2));
    h.flush();
    assert_eq!(
        fires.get(),
        while_mounted,
        "a released cell's effects must not fire again"
    );
}

/// The author's `on_scroll` must reach the backend bundle, with both
/// components meaningful — a grid scrolls on both axes, unlike the 1-D
/// primitives where the off-axis value is always 0.
#[test]
fn on_scroll_reports_both_axes() {
    let h = harness();
    let seen: Rc<RefCell<Vec<(f32, f32)>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    let _realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            virtual_grid(
                || 4,
                || 3,
                |_| 100.0,
                |_| 40.0,
                |c, r| (c * 1000 + r) as u64,
                |c, r| text().content(format!("c{c}r{r}")).build(),
            )
            .on_scroll(move |x, y| sink.borrow_mut().push((x, y)))
            .build(),
        )
    });
    let cbs = h.virtual_grid(0);
    let on_scroll = cbs.on_scroll.as_ref().expect("on_scroll must reach the bundle");
    on_scroll(120.0, 45.0);
    assert_eq!(*seen.borrow(), vec![(120.0, 45.0)]);
}

#[test]
fn without_on_scroll_the_bundle_carries_none() {
    let h = harness();
    let _realized = h
        .world
        .enter(|| realize(&h.backend, &h.registry, grid_4x3()));
    assert!(
        h.virtual_grid(0).on_scroll.is_none(),
        "no handler must mean no observer installed"
    );
}

/// Teardown order matches the virtualizer's: unstyle, then release.
/// Both are independent probes, so the release fires even though a
/// style effect exists.
#[test]
fn teardown_unstyles_then_releases() {
    let h = harness();
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            virtual_grid(
                || 1,
                || 1,
                |_| 10.0,
                |_| 10.0,
                |_, _| 0,
                |_, _| text().content("x").build(),
            )
            .style(px(10.0))
            .build(),
        )
    });
    h.take_log();
    drop(realized);
    assert_eq!(
        h.take_log(),
        vec![
            "on_node_unstyled n0".to_string(),
            "release_virtual_grid n0".to_string(),
        ]
    );
}

/// A multi-root cell is a programming error with a diagnostic, not a
/// silent mis-render — same contract as the virtualizer's rows.
#[test]
fn multi_root_cell_panics_with_a_diagnostic() {
    let h = harness();
    let _realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            virtual_grid(
                || 1,
                || 1,
                |_| 10.0,
                |_| 10.0,
                |_, _| 0,
                |_, _| runtime_scene::Element::Fragment(vec![
                    text().content("a").build(),
                    text().content("b").build(),
                ]),
            )
            .build(),
        )
    });
    let cbs = h.virtual_grid(0);
    let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        h.world.enter(|| (cbs.mount_cell)(0, 0));
    }))
    .unwrap_err();
    let msg = err
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()).unwrap_or_default());
    assert!(
        msg.contains("single-root") && msg.contains("render_cell(0, 0)"),
        "diagnostic must name the offending cell: {msg}"
    );
}

/// `on_handle` fires at mount and the handle is safe to call on a
/// backend with no grid ops — the defaulted contract every
/// not-yet-adopting backend relies on.
#[test]
fn handle_is_inert_but_safe_without_backend_ops() {
    let h = harness();
    let handle: Rc<
        RefCell<Option<runtime_shared::primitives::virtual_grid::VirtualGridHandle>>,
    > = Rc::new(RefCell::new(None));
    let sink = handle.clone();
    let _realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            virtual_grid(
                || 1,
                || 1,
                |_| 10.0,
                |_| 10.0,
                |_, _| 0,
                |_, _| text().content("x").build(),
            )
            .on_handle(move |hd| *sink.borrow_mut() = Some(hd))
            .build(),
        )
    });
    let handle = handle.borrow();
    let handle = handle.as_ref().expect("on_handle must fire at mount");
    assert_eq!(handle.scroll_offset(), (0.0, 0.0));
    handle.scroll_to(10.0, 20.0);
    handle.scroll_to_cell(3, 4);
}
