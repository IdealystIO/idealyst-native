//! Animation end to end on the host-mock substrate: the playback clock on
//! the framework's animation tick (driven here by `clock::tick_for_test`),
//! a painter that reads it repainting per frame, and posed models picking
//! and measuring where they are drawn.

use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;

use canvas3d_core::prelude::*;
use canvas3d_core::{Canvas3dPrim, Frame3d, ModelDesc, Node, Part, Skin, SkinWeights};
use host_mock::Harness;
use runtime_scene::{MountCx, Realized};
use runtime_shared::animation::clock;
use runtime_vocabulary::caps::DocumentOps;
use runtime_vocabulary::glue::IntoElement;

const FRAME: Duration = Duration::from_millis(16);

thread_local! {
    static FRAMES: RefCell<Vec<Frame3d>> = const { RefCell::new(Vec::new()) };
}

fn harness() -> Harness {
    FRAMES.with(|f| f.borrow_mut().clear());
    Harness::with_registry(|r| {
        r.register::<Canvas3dPrim, _>(|cx: &mut MountCx<'_, _>, prim, _children| {
            let node = cx.backend().borrow_mut().create_element("canvas");
            let prim = prim.clone();
            runtime_world::effect(move || {
                let frame = prim.paint();
                FRAMES.with(|f| f.borrow_mut().push(frame));
            });
            node
        });
    })
}

fn frames() -> usize {
    FRAMES.with(|f| f.borrow().len())
}

#[test]
fn clock_advances_only_while_playing_and_unregisters_when_paused() {
    let h = harness();
    let clock = h.world.enter(AnimationClock::new);
    assert_eq!(clock::registered_count(), 0, "a new clock is paused and costs nothing");

    h.world.enter(|| clock.play());
    h.flush();
    assert!(h.world.enter(|| clock.is_playing()));
    assert_eq!(clock::registered_count(), 1);
    clock::tick_for_test(FRAME);
    clock::tick_for_test(FRAME);
    h.flush();
    let t = h.world.enter(|| clock.time());
    assert!((t - 0.032).abs() < 1e-4, "two 16 ms frames: {t}");

    h.world.enter(|| clock.pause());
    h.flush();
    assert_eq!(clock::registered_count(), 0, "paused: off the frame tick");
    clock::tick_for_test(FRAME);
    h.flush();
    assert_eq!(h.world.enter(|| clock.time()), t, "time holds while paused");
    assert!(!h.world.enter(|| clock.is_playing()));
}

#[test]
fn clock_seek_and_speed() {
    let h = harness();
    let clock = h.world.enter(AnimationClock::new);
    h.world.enter(|| {
        clock.seek(2.0);
        clock.set_speed(-0.5);
        clock.play();
    });
    h.flush();
    clock::tick_for_test(FRAME);
    h.flush();
    let t = h.world.enter(|| clock.time());
    assert!((t - (2.0 - 0.008)).abs() < 1e-4, "half speed backwards: {t}");
    h.world.enter(|| clock.seek(-3.0));
    h.flush();
    assert_eq!(h.world.enter(|| clock.time()), 0.0, "clamped at 0");
    h.world.enter(|| clock.pause());
}

#[test]
fn clock_stops_when_its_scope_is_torn_down() {
    let h = harness();
    let (clock, owned) = h.world.enter(|| runtime_world::collect_owned(AnimationClock::new));
    h.world.enter(|| clock.play());
    h.flush();
    assert_eq!(clock::registered_count(), 1);
    drop(owned);
    h.flush();
    // The scope that owned the clock is gone: its tick must not write the
    // (now disposed) signal — that would abort with a stale-handle write.
    clock::tick_for_test(FRAME);
    assert_eq!(clock::registered_count(), 0, "the tick retired with its scope");
}

#[test]
fn a_painter_reading_the_clock_repaints_every_frame_and_stops_when_paused() {
    let h = harness();
    let clock = h.world.enter(AnimationClock::new);
    let painter_clock = clock.clone();
    let el = h.world.enter(|| {
        Canvas3d(Canvas3dProps {
            draw: canvas3d_core::draw(move |_| {
                let _ = painter_clock.time();
            }),
            ..Default::default()
        })
        .into_element()
    });
    let _r: Realized<u32> = h.mount(el);
    h.flush();
    h.world.enter(|| clock.play());
    h.flush();
    let before = frames();
    for _ in 0..3 {
        clock::tick_for_test(FRAME);
        h.flush();
    }
    assert_eq!(frames(), before + 3, "one repaint per frame while playing");
    h.world.enter(|| clock.pause());
    h.flush();
    let paused = frames();
    clock::tick_for_test(FRAME);
    h.flush();
    assert_eq!(frames(), paused, "no repaint while paused");
}

/// A two-joint "arm": a triangle whose tip vertex is bound to the second
/// joint, one unit up the first. Rotating the second joint swings the tip.
fn arm() -> (Model, usize) {
    let mesh = MeshData::new(
        vec![[-0.25, 0.0, 0.0], [0.25, 0.0, 0.0], [0.0, 2.0, 0.0]],
        None,
        None,
        Some(vec![0, 1, 2]),
    )
    .with_skin(SkinWeights {
        joints: vec![[0, 0, 0, 0], [0, 0, 0, 0], [1, 0, 0, 0]],
        weights: vec![[1.0, 0.0, 0.0, 0.0]; 3],
    });
    let nodes = vec![
        Node { name: Some("root".into()), parent: None, rest: Transform::IDENTITY },
        Node { name: Some("elbow".into()), parent: Some(0), rest: Transform::from_translation(Vec3::Y) },
        Node { name: Some("mesh".into()), parent: None, rest: Transform::IDENTITY },
    ];
    let skin = Skin { joints: vec![0, 1], inverse_bind: vec![Mat4::IDENTITY, Mat4::from_translation(-Vec3::Y)] };
    let model = Model::from_desc(ModelDesc {
        parts: vec![Part::new(Arc::new(mesh), 0, Mat4::IDENTITY).on_node(2).skinned(0)],
        materials: vec![Material::default().double_sided()],
        nodes,
        skins: vec![skin],
        animations: Vec::new(),
    });
    let elbow = model.node("elbow").expect("named node");
    (model, elbow)
}

fn swung(model: &Model, elbow: usize) -> Pose {
    // Elbow bent 90° about Z: the tip, one unit above the elbow, swings to −X.
    let mut pose = Pose::rest(model);
    pose.set(elbow, Transform::new(Vec3::Y, Quat::from_rotation_z(std::f32::consts::FRAC_PI_2), Vec3::ONE));
    pose
}

#[test]
fn posed_bounds_follow_the_joints() {
    let (model, elbow) = arm();
    let rest = model.bounds();
    assert!((rest.max.y - 2.0).abs() < 1e-4, "rest tip at y = 2: {rest:?}");
    let posed = model.posed_bounds(&swung(&model, elbow));
    assert!((posed.min.x + 1.0).abs() < 1e-4 && (posed.max.y - 1.0).abs() < 1e-4, "{posed:?}");
}

#[test]
fn picking_hits_the_posed_skinned_mesh_not_its_rest_pose() {
    let (model, elbow) = arm();
    let pose = swung(&model, elbow);
    let mut s = Scene3d::with_size(400.0, 400.0);
    s.camera(Camera::look_at(Vec3::new(0.0, 1.0, 10.0), Vec3::new(0.0, 1.0, 0.0)));
    s.model(&model, Mat4::IDENTITY).pick_id(9).pose(pose);

    // Near the bent tip: (-0.9, 1.0) is inside the swung triangle (which
    // runs from the base to (-1, 1)), and nowhere near the rest pose's.
    let bent_tip = s.project(Vec3::new(-0.6, 0.6, 0.0)).unwrap();
    assert_eq!(s.pick(bent_tip).map(|h| h.pick_id), Some(9), "hit where the arm is drawn");
    let rest_tip = s.project(Vec3::new(0.0, 1.8, 0.0)).unwrap();
    assert_eq!(s.pick(rest_tip), None, "the rest-pose tip is empty space now");
}

#[test]
#[should_panic(expected = "a pose of another model")]
fn a_pose_of_another_model_is_rejected() {
    let (a, _) = arm();
    let (b, _) = arm();
    let mut s = Scene3d::new();
    s.model(&a, Mat4::IDENTITY).pose(Pose::rest(&b));
}
