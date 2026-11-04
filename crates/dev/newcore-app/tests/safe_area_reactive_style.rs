//! want_9bbf5f4f, macro half: `ui!` lowers `view(style = Sheet().axis(derived(..)),
//! safe_area = ..)` to the reactive style path, the same as the view without
//! `safe_area`. The class-level half runs in a browser
//! (`backend-web`'s `regression_safe_area_view_follows_a_reactive_style_variant`).

use std::cell::RefCell;
use std::rc::Rc;

use runtime_macros::{stylesheet, ui};
use runtime_scene::{realize, Registry};
use runtime_vocabulary::glue::{derived, signal, viewport_size, SafeAreaSides, Signal, ViewportSize};
use runtime_vocabulary::viewport::viewport_ctx;
use runtime_world::World;
use scene_parity::full::FullRecorder;
use scene_parity::{Mode, Recorder};

stylesheet! {
    pub Rail<()> {
        base(_t) {}
        variant stand {
            #[default]
            landscape(_t) { width: 280 }
            portrait(_t) { width: 0 }
        }
    }
}

fn rail(portrait: Signal<bool>) -> Rail {
    Rail::new().stand(derived(move || {
        if portrait.get() {
            RailStand::Portrait
        } else {
            RailStand::Landscape
        }
    }))
}

/// The rule digest of each style apply, in order.
fn style_applies(log: &[String]) -> Vec<String> {
    log.iter()
        .filter(|l| l.starts_with("apply_style"))
        .map(|l| l.splitn(3, ' ').nth(2).unwrap_or_default().to_string())
        .collect()
}

fn script(with_safe_area: bool) -> Vec<String> {
    let world = World::new();
    let recorder = Recorder::default();
    let backend = Rc::new(RefCell::new(FullRecorder::new(recorder.clone(), Mode::Anchored)));
    let mut registry: Registry<FullRecorder> = Registry::new();
    runtime_vocabulary::register_builtins(&mut registry);
    let registry = Rc::new(registry);

    let (portrait, _realized) = world.enter(|| {
        let portrait = signal(true);
        let tree = if with_safe_area {
            ui! { view(style = rail(portrait), safe_area = SafeAreaSides::ALL) {} }
        } else {
            ui! { view(style = rail(portrait)) {} }
        };
        (portrait, realize(&backend, &registry, tree))
    });
    world.enter(|| portrait.set(false));
    world.flush();
    style_applies(&recorder.take_ops())
}

/// The shape the report came from: the style reads the viewport signal, and
/// the view is built inside a reactive `match`.
fn script_in_a_reactive_branch(with_safe_area: bool) -> Vec<String> {
    let world = World::new();
    let recorder = Recorder::default();
    let backend = Rc::new(RefCell::new(FullRecorder::new(recorder.clone(), Mode::Anchored)));
    let mut registry: Registry<FullRecorder> = Registry::new();
    runtime_vocabulary::register_builtins(&mut registry);
    let registry = Rc::new(registry);

    let upright = || {
        let v = viewport_size().get();
        v.height > v.width
    };
    let stand = move || {
        Rail::new().stand(derived(move || {
            if upright() {
                RailStand::Portrait
            } else {
                RailStand::Landscape
            }
        }))
    };
    // A viewport write is staged: flush it, so the tree mounts in portrait.
    let ctx = world.enter(viewport_ctx);
    world.enter(|| ctx.set(ViewportSize { width: 800.0, height: 1200.0 }));
    world.flush();
    let _realized = world.enter(|| {
        let shown = signal(true);
        let tree = ui! {
            view() {
                match shown.get() {
                    true => {
                        if with_safe_area {
                            view(style = stand(), safe_area = SafeAreaSides::TOP | SafeAreaSides::RIGHT) {}
                        } else {
                            view(style = stand()) {}
                        }
                    }
                    false => { view() {} }
                }
            }
        };
        realize(&backend, &registry, tree)
    });
    world.enter(|| ctx.set(ViewportSize { width: 1200.0, height: 800.0 }));
    world.flush();
    style_applies(&recorder.take_ops())
}

#[test]
fn regression_a_safe_area_view_in_a_reactive_branch_follows_the_viewport() {
    let without = script_in_a_reactive_branch(false);
    assert_eq!(without.len(), 2, "mounted portrait, then rotated: {without:?}");
    assert_ne!(without[0], without[1], "the two arms differ");
    assert!(without[0].contains("Px(0.0)"), "mounted in portrait: {without:?}");
    assert_eq!(script_in_a_reactive_branch(true), without);
}

#[test]
fn regression_a_safe_area_view_takes_the_reactive_style_path() {
    let without = script(false);
    assert!(without.len() >= 2, "the plain view re-applied on the flip: {without:?}");
    assert_eq!(script(true), without, "safe_area does not change what the style does");
}
