//! `collision_insets` is an inline `ui!` prop on `anchored_overlay`, and what
//! it says reaches the backend's `create_portal`.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_macros::ui;
use runtime_scene::{realize, Registry};
use runtime_vocabulary::glue::{AnchorTarget, EdgeInsets, Ref, ViewHandle};
use runtime_world::World;
use scene_parity::full::FullRecorder;
use scene_parity::{Mode, Recorder};

#[test]
fn collision_insets_is_an_inline_prop_of_anchored_overlay() {
    let world = World::new();
    let backend = Rc::new(RefCell::new(FullRecorder::new(Recorder::default(), Mode::Anchored)));
    let mut registry: Registry<FullRecorder> = Registry::new();
    runtime_vocabulary::register_builtins(&mut registry);
    let registry = Rc::new(registry);

    let _realized = world.enter(|| {
        let target = AnchorTarget::from(Ref::<ViewHandle>::new());
        let tree = ui! {
            anchored_overlay(
                target = target,
                collision_insets = EdgeInsets { bottom: 57.0, ..EdgeInsets::ZERO },
            ) {
                text { "panel" }
            }
        };
        realize(&backend, &registry, tree)
    });
}
