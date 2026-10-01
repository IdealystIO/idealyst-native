//! End-to-end behavior of the streamed-component boundary: the real
//! `spike-guest` wasm, loaded through wasmi, mounted into a host-mock
//! scene. Every assertion is about what lands on screen or in the host's
//! reactive graph — the same observable surface a native component has.

use host_mock::Harness;
use stream_host::{Bundle, LoadError, StreamEngine};
use stream_spike::{host_exports, GUEST_WASM};

fn load() -> Bundle {
    Bundle::load(&StreamEngine::new(), GUEST_WASM, host_exports()).expect("spike guest loads")
}

/// Fire the `i`th button the mock created (buttons record their action in
/// `button_presses`; `press_handler` is the pressable list).
fn press_button(h: &Harness, i: usize) {
    let fire = h.shared.button_presses.borrow()[i].clone();
    fire();
}

fn screen(h: &Harness) -> String {
    h.live_roots().into_iter().map(|r| h.live_tree(r)).collect::<Vec<_>>().join("\n")
}

#[test]
fn manifest_carries_each_components_prop_schema() {
    let b = load();
    let names: Vec<_> = b.manifest().components.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Counter", "Stepper", "Label", "Camera"]);
    assert_eq!(b.manifest().imports, ["Badge", "CameraPreview"]);
    let counter = &b.manifest().components[0];
    let props: Vec<_> = counter.props.iter().map(|p| (p.name.as_str(), p.ty.as_str(), p.required)).collect();
    assert_eq!(
        props,
        [("title", "String", true), ("external", "ReadSignal<i64>", true), ("step", "i64", false)]
    );
}

#[test]
fn streamed_component_renders_and_tracks_a_host_signal_prop() {
    let h = Harness::new();
    let b = load();
    let external = h.world.signal(5i64);
    let tree = h.world.enter(|| {
        b.mount("Counter", b.props().value("title", "Hello".to_string()).read_signal("external", external.read_only()))
    });
    let _realized = h.mount(tree);
    h.flush();
    let s = screen(&h);
    assert!(s.contains(r#"text "Hello""#), "{s}");
    assert!(s.contains(r#"text "external: 5""#), "{s}");
    assert!(s.contains(r#"text "clicks: 0 doubled: 0""#), "{s}");
    // The imported host component rendered inside the guest's tree.
    assert!(s.contains(r#"text "[streamed]""#), "{s}");

    external.set(6);
    h.flush();
    let s = screen(&h);
    assert!(s.contains(r#"text "external: 6""#), "{s}");
}

/// Regression guard for the update path: two presses inside one batch must
/// compose against the STAGED value (0 → 1 → 2). A guest `update` built as
/// `set(get() + 1)` reads the committed 0 twice and shows 1.
#[test]
fn guest_updates_compose_within_one_batch_and_guest_effects_rerun() {
    let h = Harness::new();
    let b = load();
    let external = h.world.signal(0i64);
    let tree = h.world.enter(|| {
        b.mount("Counter", b.props().value("title", "t".to_string()).read_signal("external", external.read_only()))
    });
    let _realized = h.mount(tree);
    h.flush();

    press_button(&h, 0);
    press_button(&h, 0);
    h.flush();
    let s = screen(&h);
    // `doubled` is written by a GUEST effect reading `clicks`; seeing 4
    // proves the effect is in the host graph and re-ran on the commit.
    assert!(s.contains(r#"text "clicks: 2 doubled: 4""#), "{s}");
}

#[test]
fn guest_writes_a_two_way_host_signal() {
    let h = Harness::new();
    let b = load();
    let value = h.world.signal(10i64);
    let tree = h.world.enter(|| b.mount("Stepper", b.props().signal("value", value)));
    let _realized = h.mount(tree);
    h.flush();

    press_button(&h, 0);
    h.flush();
    assert_eq!(value.get(), 11);
    assert!(screen(&h).contains(r#"text "value: 11""#));
}

#[test]
fn old_binary_refuses_a_bundle_that_needs_an_unknown_host_component() {
    // Everything the bundle needs except the `Badge` component.
    let exports = stream_host::HostExports::new()
        .export("CameraPreview", |_| runtime_vocabulary::builders::view().build())
        .host_fn(spike_camera::battery_level::export())
        .host_fn(spike_camera::take_photo::export());
    let err = Bundle::load(&StreamEngine::new(), GUEST_WASM, exports).err().expect("load must fail");
    match err {
        LoadError::MissingHostComponents(names) => assert_eq!(names, ["Badge"]),
        other => panic!("wrong error: {other}"),
    }
}

#[test]
fn unmount_releases_guest_callbacks_and_signal_handles() {
    let h = Harness::new();
    let b = load();
    assert_eq!(b.live_callbacks(), 0);
    let external = h.world.signal(1i64);
    let tree = h.world.enter(|| b.mount("Label", b.props().read_signal("external", external.read_only())));
    let realized = h.mount(tree);
    h.flush();
    assert!(screen(&h).contains(r#"text "1 + 1""#));
    // One effect body + one text closure in the guest; the host prop plus
    // the guest's own signal in the handle table.
    assert_eq!(b.live_callbacks(), 2);
    assert_eq!(b.live_handles(), 2);

    drop(realized);
    h.flush();
    assert_eq!(b.live_callbacks(), 0, "guest closures outlived their scope");
    assert_eq!(b.live_handles(), 0, "signal handles outlived their scope");
}

#[test]
fn one_instance_serves_many_mounts() {
    let h = Harness::new();
    let b = load();
    let a = h.world.signal(1i64);
    let c = h.world.signal(2i64);
    let tree = h.world.enter(|| {
        runtime_vocabulary::builders::view()
            .child(b.mount("Label", b.props().read_signal("external", a.read_only())))
            .child(b.mount("Label", b.props().read_signal("external", c.read_only())))
            .build()
    });
    let _realized = h.mount(tree);
    h.flush();
    c.set(7);
    h.flush();
    let s = screen(&h);
    assert!(s.contains(r#"text "1 + 1""#) && s.contains(r#"text "7 + 1""#), "{s}");
}

/// What the demo's "Refresh bundle" does: a reactive region that remounts
/// from whichever bundle is current. Swapping must tear the OLD instance's
/// mounts down completely (callbacks + handles released, so the old wasm
/// instance can be freed) while host-owned state carries straight over.
#[test]
fn swapping_bundles_releases_the_old_instance_and_keeps_host_state() {
    use std::cell::RefCell;
    use std::rc::Rc;

    let h = Harness::new();
    let old = load();
    let current = Rc::new(RefCell::new(old.clone()));
    let generation = h.world.signal(0u32);
    let external = h.world.signal(41i64);
    let tree = h.world.enter(|| {
        let current = current.clone();
        runtime_scene::dyn_element(move || {
            let _ = generation.get();
            let b = current.borrow().clone();
            b.mount("Label", b.props().read_signal("external", external.read_only()))
        })
    });
    let _realized = h.mount(tree);
    h.flush();
    assert!(screen(&h).contains(r#"text "41 + 1""#));
    assert!(old.live_callbacks() > 0);

    let new = load();
    *current.borrow_mut() = new.clone();
    generation.set(1);
    external.set(42);
    h.flush();

    assert!(screen(&h).contains(r#"text "42 + 1""#), "{}", screen(&h));
    assert_eq!(old.live_callbacks(), 0, "old bundle's closures outlived the swap");
    assert_eq!(old.live_handles(), 0, "old bundle's signal handles outlived the swap");
    assert_eq!(new.live_callbacks(), 2);
}

// ---------------------------------------------------------------------------
// Prop contract: what happens when the bundle's props and the app's drift.
// Each test varies the APP side against the one real bundle; the check is a
// diff of the two, so that covers the same cases as varying the bundle.
// ---------------------------------------------------------------------------

use stream_host::{MountError, PropProblem};

fn counter_props(b: &Bundle, external: runtime_world::Signal<i64>) -> stream_host::HostProps {
    b.props().value("title", "t".to_string()).read_signal("external", external.read_only())
}

#[test]
fn optional_prop_not_sent_uses_the_bundles_default() {
    let h = Harness::new();
    let b = load();
    let external = h.world.signal(0i64);
    let _r = h.mount(h.world.enter(|| b.mount("Counter", counter_props(&b, external))));
    h.flush();
    assert!(screen(&h).contains(r#"button "add 1""#), "{}", screen(&h));
}

#[test]
fn optional_prop_sent_overrides_the_default() {
    let h = Harness::new();
    let b = load();
    let external = h.world.signal(0i64);
    let props = counter_props(&b, external).value("step", 5i64);
    let _r = h.mount(h.world.enter(|| b.mount("Counter", props)));
    h.flush();
    press_button(&h, 0);
    h.flush();
    let s = screen(&h);
    assert!(s.contains(r#"button "add 5""#) && s.contains(r#"text "clicks: 5 doubled: 10""#), "{s}");
}

#[test]
fn prop_type_change_is_refused_before_anything_runs_in_the_guest() {
    let h = Harness::new();
    let b = load();
    let external = h.world.signal(0i64);
    let props = b.props().value("title", 7i64).read_signal("external", external.read_only());
    let expected = MountError::IncompatibleProps {
        component: "Counter".into(),
        problems: vec![PropProblem::TypeChanged { prop: "title".into(), app: "i64".into(), bundle: "String".into() }],
    };
    assert_eq!(b.check("Counter", &props), Err(expected.clone()));
    let result = h.world.enter(|| b.try_mount("Counter", props));
    assert_eq!(result.err(), Some(expected));
    assert_eq!(b.live_callbacks(), 0, "a refused mount must not run the component");
}

#[test]
fn missing_required_prop_is_refused() {
    let b = load();
    let props = b.props().value("title", "t".to_string());
    assert_eq!(
        b.check("Counter", &props),
        Err(MountError::IncompatibleProps {
            component: "Counter".into(),
            problems: vec![PropProblem::MissingRequired { prop: "external".into(), ty: "ReadSignal<i64>".into() }],
        })
    );
}

#[test]
fn prop_the_bundle_no_longer_declares_is_ignored() {
    let h = Harness::new();
    let b = load();
    let external = h.world.signal(3i64);
    let props = b.props().read_signal("external", external.read_only()).value("legacy", true);
    let _r = h.mount(h.world.enter(|| b.mount("Label", props)));
    h.flush();
    assert!(screen(&h).contains(r#"text "3 + 1""#));
}

#[test]
fn two_way_signal_satisfies_a_read_signal_prop() {
    let h = Harness::new();
    let b = load();
    let external = h.world.signal(4i64);
    let _r = h.mount(h.world.enter(|| b.mount("Label", b.props().signal("external", external))));
    h.flush();
    assert!(screen(&h).contains(r#"text "4 + 1""#));
}

#[test]
fn read_signal_does_not_satisfy_a_two_way_prop() {
    let h = Harness::new();
    let b = load();
    let value = h.world.signal(0i64);
    assert_eq!(
        b.check("Stepper", &b.props().read_signal("value", value.read_only())),
        Err(MountError::IncompatibleProps {
            component: "Stepper".into(),
            problems: vec![PropProblem::TypeChanged {
                prop: "value".into(),
                app: "ReadSignal<i64>".into(),
                bundle: "Signal<i64>".into(),
            }],
        })
    );
}

#[test]
fn unknown_component_is_refused() {
    let b = load();
    assert_eq!(
        b.check("Gone", &b.props()),
        Err(MountError::NoSuchComponent { component: "Gone".into() })
    );
}

// ---------------------------------------------------------------------------
// Host functions: app code the bundle calls by name.
// ---------------------------------------------------------------------------

use stream_host::{HostExports, HostFnMismatch};

/// The spike's exports minus some host functions, for load-check tests.
fn exports_with(fns: &[stream_abi::host_fn::HostFnDef]) -> HostExports {
    let mut e = HostExports::new()
        .export("Badge", |_| runtime_vocabulary::builders::view().build())
        .export("CameraPreview", |_| runtime_vocabulary::builders::view().build());
    for f in fns {
        e = e.host_fn(*f);
    }
    e
}

#[test]
fn bundle_calling_a_host_fn_the_app_does_not_allow_is_refused_at_load() {
    let err = Bundle::load(&StreamEngine::new(), GUEST_WASM, exports_with(&[spike_camera::battery_level::export()]))
        .err()
        .expect("load must fail");
    match err {
        LoadError::MissingHostFunctions(names) => assert_eq!(names, ["spike_camera::take_photo"]),
        other => panic!("wrong error: {other}"),
    }
}

#[test]
fn host_fn_signature_drift_is_refused_at_load() {
    let mut drifted = spike_camera::take_photo::export();
    let real = drifted.schema;
    drifted.schema ^= 1;
    let err = Bundle::load(
        &StreamEngine::new(),
        GUEST_WASM,
        exports_with(&[spike_camera::battery_level::export(), drifted]),
    )
    .err()
    .expect("load must fail");
    assert_eq!(
        err.to_string(),
        LoadError::IncompatibleHostFunctions(vec![HostFnMismatch {
            path: "spike_camera::take_photo".into(),
            app_schema: real ^ 1,
            bundle_schema: real,
        }])
        .to_string()
    );
}

/// Install the hand-pumped executor + scheduler and mount `Camera`.
fn mount_camera(h: &Harness, b: &Bundle, facing: &str) -> runtime_scene::Realized<host_mock::Node> {
    host_mock::pump::install_executor();
    host_mock::pump::install_scheduler();
    let props = b.props().value("facing", facing.to_string());
    let r = h.mount(h.world.enter(|| b.mount("Camera", props)));
    h.flush();
    r
}

/// Drive an in-flight `take_photo` to completion: first poll arms the
/// shutter timer, the timer fires, the second poll resolves, `then` runs.
fn finish_capture(h: &Harness) {
    host_mock::pump::pump_tasks();
    host_mock::pump::pump_timers();
    host_mock::pump::pump_tasks();
    h.flush();
}

#[test]
fn sync_host_fn_returns_inline_and_native_preview_mounts_by_name() {
    let h = Harness::new();
    let b = load();
    let _r = mount_camera(&h, &b, "back");
    let s = screen(&h);
    assert!(s.contains(r#"text "battery at mount: "#), "{s}");
    assert!(s.contains(r#"text "▣ native back camera preview""#), "{s}");
}

#[test]
fn async_host_fn_result_lands_in_guest_state_after_the_io_completes() {
    let h = Harness::new();
    let b = load();
    let _r = mount_camera(&h, &b, "front");
    press_button(&h, 0);
    h.flush();
    assert!(screen(&h).contains(r#"text "capturing…""#), "{}", screen(&h));

    finish_capture(&h);
    let s = screen(&h);
    assert!(s.contains("from the front camera"), "{s}");
    assert!(s.contains(r#"text "photos this mount: 1""#), "{s}");
}

#[test]
fn async_host_fn_error_arm_reaches_the_guest() {
    let h = Harness::new();
    let b = load();
    let _r = mount_camera(&h, &b, "periscope");
    press_button(&h, 0);
    finish_capture(&h);
    assert!(screen(&h).contains(r#"text "camera error: no periscope camera""#), "{}", screen(&h));
}

/// The case most likely to go wrong: the component unmounts while the photo
/// is in flight. The IO still completes (spawn_then never cancels it), but
/// the guest's `then` must not run — it would write signals whose scope is
/// gone — and its guest slot must be released.
#[test]
fn unmount_mid_call_drops_then_without_running_it() {
    let h = Harness::new();
    let b = load();
    let r = mount_camera(&h, &b, "back");
    press_button(&h, 0);
    host_mock::pump::pump_tasks(); // in flight: shutter armed
    drop(r);
    h.flush();
    // host-mock keeps every button's press handler for the test to fire; a
    // real backend releases it with the node. Drop them, so the only guest
    // callback that can still be live below is the in-flight `then`.
    h.shared.button_presses.borrow_mut().clear();

    host_mock::pump::pump_timers();
    host_mock::pump::pump_tasks(); // IO completes; scope is dead
    h.flush();
    assert_eq!(host_mock::pump::pending_tasks(), 0, "the IO must still run to completion");
    assert_eq!(b.live_callbacks(), 0, "the dropped `then` must release its guest slot");
    assert_eq!(b.live_handles(), 0);
}
