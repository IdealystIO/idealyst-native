//! Compile gate for the MCP catalog's recipes.
//!
//! The recipe *data* (name/docs/source/uses) lives in
//! `runtime_shared::recipes` as static text — `include_str!` of the
//! files under `crates/runtime/shared/recipes/`. That anchoring is what
//! keeps the catalog core-independent (see the module docs there). This
//! test is the other half of the old `recipe!` contract: the snippet
//! sources stay REAL, compile-checked Rust against the live author
//! surface, so a prop/API change that would rot a served example fails
//! here.
//!
//! Each recipe is compiled through the `runtime_core` author-surface
//! root (`runtime_core::…`) and then
//! mounted against the scene-parity mock host — a recipe must not just
//! parse, it must build a scene.
//!
//! Two of the recipes target the navigation SDKs
//! (`swap_three_screens_tab_bar`, `stack_two_screens`). They used to be
//! `recipe!` invocations inside those crates; they live in
//! `runtime_shared::recipes` like the core-primitive ones and are
//! compile-checked HERE, against the SDKs' real public surface.

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
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../runtime/shared/recipes/swap_three_screens_tab_bar.rs"
    ));
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../runtime/shared/recipes/stack_two_screens.rs"
    ));
}

/// Building a recipe creates world signals/effects, so each one builds
/// (and is realized + torn down) inside a fresh `World::enter` against
/// the scene-parity mock host — the e2e.rs harness shape.
#[test]
fn recipes_build_and_realize() {
    use std::cell::RefCell;
    use std::rc::Rc;

    use runtime_scene::{realize, Registry};
    use runtime_world::World;
    use scene_parity::full::FullRecorder;
    use scene_parity::{Mode, Recorder};

    // The scene-parity recorder implements `runtime_scene::Host` + all 30
    // caps traits DIRECTLY — no `Backend` trait in this path.
    type Bridged = FullRecorder;

    let builds: Vec<(&str, fn() -> runtime_core::Element)> = vec![
        ("input_with_submit", recipes::input_with_submit),
        ("keyed_list_add_remove", recipes::keyed_list_add_remove),
        ("animated_toast", recipes::animated_toast),
        ("confirm_dialog_overlay", recipes::confirm_dialog_overlay),
        (
            "swap_three_screens_tab_bar",
            recipes::swap_three_screens_tab_bar,
        ),
        ("stack_two_screens", recipes::stack_two_screens),
    ];
    for (name, build) in builds {
        let rec = Recorder::default();
        let backend = Rc::new(RefCell::new(FullRecorder::new(rec.clone(), Mode::Spliced)));
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
            "recipe {name:?} realized zero backend ops"
        );
        drop(realized);
        drop(world);
    }
}
