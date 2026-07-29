//! Dual-core compile gate for the MCP catalog's core-primitive recipes.
//!
//! The recipe *data* (name/docs/source/uses) lives in
//! `runtime_shared::recipes` as static text — `include_str!` of the
//! files under `crates/runtime/shared/recipes/`. That anchoring is what
//! lets the catalog survive runtime-core's deletion and compile in
//! `--new-core` dev graphs (see the module docs there). This test is
//! the other half of the old `recipe!` contract: the snippet sources
//! stay REAL, compile-checked Rust against the live author surface —
//! on BOTH cores — so a prop/API change that would rot a served example
//! fails here.
//!
//! - `cargo test -p newcore-app --features new-core` compiles them via
//!   the facade alias (`extern crate runtime_facade as runtime_core;`)
//!   with the retargeted `ui!` emission, and mounts each against the
//!   scene-parity mock host (a recipe must not just parse — it must
//!   build a scene).
//! - `cargo test -p newcore-app --features old-core` compiles the SAME
//!   files against the real old core (build-only leg: constructing old
//!   `Element`s needs no world, so building each fn's tree is the gate).

#![cfg(any(feature = "new-core", feature = "old-core"))]

// The facade alias — same-source recipes spell `::runtime_core::…`.
#[cfg(feature = "new-core")]
extern crate runtime_facade as runtime_core;

mod recipes {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../runtime/shared/recipes/input_with_submit.rs"
    ));
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../runtime/shared/recipes/keyed_list_add_remove.rs"
    ));
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../runtime/shared/recipes/animated_toast.rs"
    ));
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../runtime/shared/recipes/confirm_dialog_overlay.rs"
    ));
}

/// Old core: `Element` construction is world-free — building each tree
/// IS the compile-and-run gate.
#[cfg(feature = "old-core")]
#[test]
fn recipes_build_on_the_old_core() {
    let _ = recipes::input_with_submit();
    let _ = recipes::keyed_list_add_remove();
    let _ = recipes::animated_toast();
    let _ = recipes::confirm_dialog_overlay();
}

/// New core: building creates world signals/effects, so each recipe
/// builds (and is realized + torn down) inside a fresh `World::enter`
/// against the scene-parity mock host (the e2e.rs harness shape).
#[cfg(feature = "new-core")]
#[test]
fn recipes_build_and_realize_on_the_new_core() {
    use std::cell::RefCell;
    use std::rc::Rc;

    use runtime_scene::{realize, Registry};
    use runtime_vocabulary::LegacyBridge;
    use runtime_world::World;
    use scene_parity::full::FullRecorder;
    use scene_parity::{Mode, Recorder};

    type Bridged = LegacyBridge<FullRecorder>;

    let builds: Vec<(&str, fn() -> runtime_core::Element)> = vec![
        ("input_with_submit", recipes::input_with_submit),
        ("keyed_list_add_remove", recipes::keyed_list_add_remove),
        ("animated_toast", recipes::animated_toast),
        ("confirm_dialog_overlay", recipes::confirm_dialog_overlay),
    ];
    for (name, build) in builds {
        let rec = Recorder::default();
        let backend = Rc::new(RefCell::new(LegacyBridge(FullRecorder::new(
            rec.clone(),
            Mode::Spliced,
        ))));
        let mut registry: Registry<Bridged> = Registry::new();
        runtime_vocabulary::register_builtins(&mut registry);
        let registry = Rc::new(registry);
        let world = World::new();
        let realized = world.enter(|| {
            let element = build();
            realize(&backend, &registry, element)
        });
        world.flush();
        let ops = rec.take_ops();
        assert!(
            !ops.is_empty(),
            "recipe {name:?} realized zero backend ops on the new core"
        );
        drop(realized);
        drop(world);
    }
}
