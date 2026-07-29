//! Virtualizer + graphics handler tests: the lazy row contract
//! (mount/release via backend-held callbacks, per-row ownership scopes,
//! the measured-size cache), the data-changed effect, and both
//! primitives' teardown-release probes — driven through the `host-mock`
//! recording host (the caps surface implemented natively, no `Backend`,
//! no `LegacyBridge`; op-stream parity with the old walker lives in
//! `scene-parity`'s golden suite).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use host_mock::harness;
use runtime_core::primitives::graphics::OnResizeEvent;
use runtime_core::primitives::virtualizer::{Axis, ItemSize, Lanes, VirtualLayout};
use runtime_core::{StyleRules, Tokenized};
use runtime_scene::realize;
use runtime_vocabulary::builders::{graphics, text, virtualizer};
use runtime_world::signal;

fn px(w: f32) -> StyleRules {
    StyleRules {
        width: Some(Tokenized::Literal(runtime_core::Length::Px(w))),
        ..Default::default()
    }
}

// ===========================================================================
// Virtualizer: mount sequence + data effect
// ===========================================================================

#[test]
fn virtualizer_mount_sequence_create_then_data_effect_then_style() {
    let h = harness();
    let world = h.world.clone();
    let (realized, rows) = world.enter(|| {
        let rows = signal(3usize);
        let realized = realize(
            &h.backend,
            &h.registry,
            virtualizer(
                move || rows.get(),
                |i| i as u64,
                ItemSize::Known(Rc::new(|_| 40.0)),
                |idx| text().content(format!("row {idx}")).build(),
            )
            .style(px(120.0))
            .build(),
        );
        (realized, rows)
    });
    // Walker order: create → data effect's first fire → apply_style.
    assert_eq!(
        h.take_log(),
        vec![
            format!(
                "create n0 virtualizer overscan=1 layout={:?}",
                VirtualLayout::default()
            ),
            "virtualizer_data_changed n0".to_string(),
            "apply_style n0 width=Literal(Px(120.0))".to_string(),
        ]
    );

    // A count change notifies the backend exactly once per flush.
    rows.set(5);
    world.flush();
    assert_eq!(h.take_log(), vec!["virtualizer_data_changed n0".to_string()]);

    // Teardown: unstyle BEFORE release (effect-creation order — the
    // old core's Effect order, preserved by `Owned`'s creation-order
    // free).
    drop(realized);
    assert_eq!(
        h.take_log(),
        vec![
            "on_node_unstyled n0".to_string(),
            "release_virtualizer n0".to_string(),
        ]
    );
}

#[test]
fn virtualizer_builder_layout_setters_reach_create() {
    let h = harness();
    let _realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            virtualizer(
                || 0,
                |i| i as u64,
                ItemSize::Known(Rc::new(|_| 40.0)),
                |_| text().content("x").build(),
            )
            .overscan(2.5)
            .axis(Axis::Horizontal)
            .lanes(Lanes::Fixed(4))
            .spacing(6.0, 10.0)
            .build(),
        )
    });
    let expected_layout = VirtualLayout {
        axis: Axis::Horizontal,
        lanes: Lanes::Fixed(4),
        main_spacing: 6.0,
        cross_spacing: 10.0,
    };
    assert_eq!(
        h.take_log(),
        vec![
            format!("create n0 virtualizer overscan=2.5 layout={expected_layout:?}"),
            "virtualizer_data_changed n0".to_string(),
        ]
    );
}

// ===========================================================================
// Virtualizer: lazy row mount/release via the backend-held callbacks
// ===========================================================================

/// Rows realize DETACHED when the platform asks, their reactivity lives
/// while mounted, and `release_item` frees it — the "queued platform
/// events fire into freed per-item scopes" regression class.
#[test]
fn regression_released_row_effects_stop_firing() {
    let h = harness();
    let world = h.world.clone();
    let (_realized, version) = world.enter(|| {
        let version = signal(0i32);
        let realized = realize(
            &h.backend,
            &h.registry,
            virtualizer(
                || 10,
                |i| i as u64,
                ItemSize::Known(Rc::new(|_| 40.0)),
                move |idx| {
                    text()
                        .content(move || format!("item {idx} v{}", version.get()))
                        .build()
                },
            )
            .build(),
        );
        (realized, version)
    });
    h.take_log();

    // The platform mounts rows 0 and 1 (with the world ambient — the
    // documented host-driver contract for callback invocation).
    let cbs = h.virtualizer(0);
    let ((n0, s0), (n1, s1)) = world.enter(|| ((cbs.mount_item)(0), (cbs.mount_item)(1)));
    assert_ne!(n0, n1, "each row realizes its own detached node");
    assert_ne!(s0, s1, "scope ids are unique");
    let log = h.take_log();
    assert_eq!(
        log,
        vec![
            format!("create n{n0} text \"\""),
            format!("update_text n{n0} \"item 0 v0\""),
            format!("create n{n1} text \"\""),
            format!("update_text n{n1} \"item 1 v0\""),
        ],
        "rows realize detached: create + binding first-fire, NO insert into any parent"
    );

    // Both rows' bindings fire on a data change.
    version.set(1);
    world.flush();
    assert_eq!(
        h.take_log(),
        vec![
            format!("update_text n{n0} \"item 0 v1\""),
            format!("update_text n{n1} \"item 1 v1\""),
        ]
    );

    // Release row 0: its binding must never fire again.
    (cbs.release_item)(s0);
    version.set(2);
    world.flush();
    assert_eq!(
        h.take_log(),
        vec![format!("update_text n{n1} \"item 1 v2\"")],
        "released row's effects are freed; only the live row updates"
    );
}

/// Signals created while CONSTRUCTING the row element (not just during
/// the mount walk) must die with the row — the old walker's
/// `with_scope` wrapped `render(idx)` too. A leak here pins row state
/// until world drop.
#[test]
fn row_construction_creations_die_with_the_row() {
    let h = harness();
    let world = h.world.clone();
    let row_signal = Rc::new(RefCell::new(None));
    let sink = row_signal.clone();
    let _realized = world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            virtualizer(
                || 1,
                |i| i as u64,
                ItemSize::Known(Rc::new(|_| 40.0)),
                move |_idx| {
                    // Created during element construction, before any
                    // mount walk runs.
                    let local = signal(0i32);
                    *sink.borrow_mut() = Some(local);
                    text().content(move || format!("local {}", local.get())).build()
                },
            )
            .build(),
        )
    });
    let cbs = h.virtualizer(0);
    let (_, scope) = world.enter(|| (cbs.mount_item)(0));
    let local = row_signal.borrow().expect("render ran");
    h.take_log();

    // Live row: the construction-time signal drives the binding.
    local.set(1);
    world.flush();
    assert_eq!(h.take_log().len(), 1, "binding fires while mounted");

    // Released row: the slot is freed — a stale write panics with the
    // generational-slot diagnostic instead of silently leaking.
    (cbs.release_item)(scope);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| local.set(2)));
    assert!(
        result.is_err(),
        "construction-time signal must be freed with the row scope"
    );
}

/// The measured-size cache: keyed by item KEY, survives release (a
/// re-entering row reuses its measured size), bridged from scope ids
/// via the reverse map that IS cleaned on release.
#[test]
fn measured_size_cache_survives_release_and_keys_by_item() {
    let h = harness();
    let world = h.world.clone();
    let _realized = world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            virtualizer(
                || 3,
                |i| 100 + i as u64, // non-trivial keys
                ItemSize::Measured(Rc::new(|_| 40.0)),
                |_| text().content("row").build(),
            )
            .build(),
        )
    });
    let cbs = h.virtualizer(0);
    assert!(cbs.measure_sizes, "Measured mode asks the backend to observe");
    assert_eq!((cbs.item_size)(0), 40.0, "estimate before any measurement");

    let (_, s0) = world.enter(|| (cbs.mount_item)(0));
    (cbs.set_measured_size)(s0, 55.0);
    assert_eq!((cbs.item_size)(0), 55.0, "measured value overrides the estimate");
    assert_eq!((cbs.item_size)(1), 40.0, "other keys keep the estimate");

    // Release, then remount: the cache survives (keyed by key, not
    // scope), so the re-entering row starts from the measured size.
    (cbs.release_item)(s0);
    assert_eq!((cbs.item_size)(0), 55.0, "cache survives unmount");

    // The reverse-map entry died with the release: a stale scope id
    // can no longer write the cache.
    (cbs.set_measured_size)(s0, 99.0);
    assert_eq!((cbs.item_size)(0), 55.0, "stale scope id writes nowhere");

    // A fresh mount re-establishes the bridge.
    let (_, s0b) = world.enter(|| (cbs.mount_item)(0));
    assert_ne!(s0, s0b, "fresh scope id per mount");
    (cbs.set_measured_size)(s0b, 61.0);
    assert_eq!((cbs.item_size)(0), 61.0);
}

/// A multi-root row is a usage error, reported loudly (same contract as
/// `MountCx::realize_detached`).
#[test]
fn multi_root_row_panics_with_diagnostic() {
    let h = harness();
    let world = h.world.clone();
    let _realized = world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            virtualizer(
                || 1,
                |i| i as u64,
                ItemSize::Known(Rc::new(|_| 40.0)),
                |_| {
                    runtime_scene::fragment(vec![
                        text().content("a").build(),
                        text().content("b").build(),
                    ])
                },
            )
            .build(),
        )
    });
    let cbs = h.virtualizer(0);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        world.enter(|| (cbs.mount_item)(0))
    }));
    let err = result.expect_err("multi-root row must panic");
    let msg = err
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_default();
    assert!(
        msg.contains("single-root"),
        "diagnostic names the contract: {msg}"
    );
}

#[test]
fn virtualizer_ref_fill_receives_handle() {
    let h = harness();
    let filled = Rc::new(Cell::new(0u32));
    let sink = filled.clone();
    let _realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            virtualizer(
                || 0,
                |i| i as u64,
                ItemSize::Known(Rc::new(|_| 40.0)),
                |_| text().content("x").build(),
            )
            .on_handle(move |_h| sink.set(sink.get() + 1))
            .build(),
        )
    });
    assert_eq!(filled.get(), 1, "ref fill runs exactly once at mount");
}

// ===========================================================================
// Graphics
// ===========================================================================

#[test]
fn graphics_mount_wires_callbacks_and_releases_on_teardown() {
    let h = harness();
    let world = h.world.clone();
    let resized: Rc<RefCell<Vec<(u32, u32)>>> = Rc::new(RefCell::new(Vec::new()));
    let lost = Rc::new(Cell::new(0u32));
    let resized_sink = resized.clone();
    let lost_sink = lost.clone();
    let realized = world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            graphics(|_ready| {})
                .on_resize(move |e| resized_sink.borrow_mut().push(e.size))
                .on_lost(move || lost_sink.set(lost_sink.get() + 1))
                .style(px(100.0))
                .build(),
        )
    });
    assert_eq!(
        h.take_log(),
        vec![
            "create n0 graphics".to_string(),
            "apply_style n0 width=Literal(Px(100.0))".to_string(),
        ]
    );

    // The platform fires the captured lifecycle closures; the author's
    // handlers observe them unwrapped (no cycle() shim — staged commits
    // make event batching structural).
    {
        let mut g = h.shared.graphics.borrow_mut();
        (g[0].on_resize)(OnResizeEvent {
            size: (800, 600),
            scale: 2.0,
        });
        (g[0].on_lost)();
    }
    assert_eq!(*resized.borrow(), vec![(800, 600)]);
    assert_eq!(lost.get(), 1);

    // Teardown: unstyle THEN release (creation-order effect free) —
    // and release fires even though the style effect exists (it's an
    // independent probe, the old GraphicsHandleCleanup contract).
    drop(realized);
    assert_eq!(
        h.take_log(),
        vec![
            "on_node_unstyled n0".to_string(),
            "release_graphics n0".to_string(),
        ]
    );
}

#[test]
fn unstyled_graphics_still_releases() {
    let h = harness();
    let realized = h.world.enter(|| {
        realize(&h.backend, &h.registry, graphics(|_| {}).build())
    });
    h.take_log();
    drop(realized);
    assert_eq!(
        h.take_log(),
        vec!["release_graphics n0".to_string()],
        "release probe is unconditional (independent of the style effect)"
    );
}

#[test]
fn graphics_ref_fill_receives_handle() {
    let h = harness();
    let filled = Rc::new(Cell::new(0u32));
    let sink = filled.clone();
    let _realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            graphics(|_| {})
                .on_handle(move |_h| sink.set(sink.get() + 1))
                .build(),
        )
    });
    assert_eq!(filled.get(), 1);
}
