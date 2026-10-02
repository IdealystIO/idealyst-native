//! `#[component(remote)]` end to end. `spike-remoteattr`'s `Greeting` is
//! linked here natively — where the macro made it a stub — and its wasm
//! build is the bundle the stub mounts from. A `ui!` call site in app code
//! is all the app writes.

use host_mock::Harness;
use runtime_core::{ui, Element};
use runtime_world::{signal, Signal};
use spike_remoteattr::Greeting;
use stream_spike::REMOTE_ATTR_WASM;

struct App {
    h: Harness,
    count: Signal<i64>,
    likes: Signal<i64>,
    remote: stream_host::remote::RemoteApp,
}

fn app() -> App {
    let remote = stream_host::remote::install(REMOTE_ATTR_WASM).expect("bundle loads");
    let h = Harness::new();
    let (count, likes) = h.world.enter(|| (signal(1i64), signal(0i64)));
    App { h, count, likes, remote }
}

fn tree(a: &App) -> Element {
    let (count, likes) = (a.count, a.likes);
    a.h.world.enter(|| ui! { Greeting(name = "ada".to_string(), count = count.read_only(), likes = likes) })
}

fn text(a: &App, realized: &runtime_scene::Realized<host_mock::Node>) -> String {
    realized.collect_nodes().iter().map(|n| a.h.live_tree(*n)).collect::<Vec<_>>().join("\n")
}

#[test]
fn a_remote_component_mounts_from_its_bundle_with_the_apps_state() {
    let a = app();
    let realized = a.h.mount(tree(&a));
    a.h.flush();
    let t = text(&a, &realized);
    for want in ["hello ada", "count 1", "taps 0"] {
        assert!(t.contains(want), "missing {want:?}:\n{t}");
    }

    // The app's signal, read live by the bundle.
    a.count.set(7);
    a.h.flush();
    assert!(text(&a, &realized).contains("count 7"));

    // Bundle state, and a bundle write to the app's signal.
    let presses = a.h.shared.button_presses.borrow().clone();
    presses[0]();
    presses[1]();
    presses[1]();
    a.h.flush();
    assert!(text(&a, &realized).contains("taps 1"));
    assert_eq!(a.likes.get(), 2, "the bundle wrote the app's signal");
}

/// A reload remounts every mounted remote component from the new bundle.
#[test]
fn reloading_the_bundle_remounts_the_component() {
    let a = app();
    let realized = a.h.mount(tree(&a));
    a.h.flush();
    let presses = a.h.shared.button_presses.borrow().clone();
    presses[0]();
    a.h.flush();
    assert!(text(&a, &realized).contains("taps 1"));

    a.remote.reload(REMOTE_ATTR_WASM).expect("reloads");
    a.h.flush();
    let t = text(&a, &realized);
    assert!(t.contains("taps 0"), "remounted from the new bundle, with fresh bundle state:\n{t}");
    assert!(t.contains("count 1"), "app state survives the reload:\n{t}");
}

/// Unmounting releases the props' exports: the app's signals are no longer
/// read by anything.
#[test]
fn unmounting_releases_the_props() {
    let a = app();
    let realized = a.h.mount(tree(&a));
    a.h.flush();
    assert!(a.count.subscriber_count() > 0);
    drop(realized);
    a.h.flush();
    a.h.shared.button_presses.borrow_mut().clear();
    assert_eq!(a.count.subscriber_count(), 0);
    assert_eq!(runtime_world::remote::pending_scopes(), 0);
}

/// A bundle that does not define the component the app asks for shows the
/// failure in its place.
#[test]
fn a_bundle_without_the_component_shows_an_error() {
    let a = app();
    a.remote.reload(stream_spike::KERNEL_GUEST_WASM).expect("loads");
    let realized = a.h.mount(tree(&a));
    a.h.flush();
    let t = text(&a, &realized);
    assert!(t.contains("remote component `Greeting`") && t.contains("__idealyst_remote_Greeting"), "{t}");
}

/// The remote component renders an APP component (`Panel`, not `remote`):
/// the bundle imports the app's copy by name, and every prop crosses.
#[test]
fn a_remote_component_uses_an_app_component_with_live_props() {
    let a = app();
    let realized = a.h.mount(tree(&a));
    a.h.flush();
    let t = text(&a, &realized);
    for want in ["panel of ada", "panel sees 0", "panel edit draft", "bundle child sees draft"] {
        assert!(t.contains(want), "missing {want:?}:\n{t}");
    }
    let presses = a.h.shared.button_presses.borrow().clone();
    // Greeting: tap, like; Panel (the app's own code): reset, app edits.
    assert_eq!(presses.len(), 4, "{t}");

    // Bundle state the app component reads natively (the signal was promoted).
    presses[0]();
    presses[0]();
    a.h.flush();
    let t = text(&a, &realized);
    assert!(t.contains("taps 2") && t.contains("panel sees 2"), "{t}");

    // The app component calls back into the bundle.
    presses[2]();
    a.h.flush();
    let t = text(&a, &realized);
    assert!(t.contains("taps 0") && t.contains("panel sees 0"), "{t}");

    // The app component writes a signal the bundle created; the bundle's own
    // text (the children it passed in) follows.
    presses[3]();
    a.h.flush();
    let t = text(&a, &realized);
    assert!(t.contains("panel edit from app") && t.contains("bundle child sees from app"), "{t}");
}

/// The app component's body is the app's: it is registered for import, and
/// the bundle does not carry it.
#[test]
fn app_components_are_registered_for_import() {
    let names = runtime_vocabulary::remote::host::app_component_names();
    assert!(names.contains(&"spike_remoteattr::Panel"), "{names:?}");
    assert!(!names.contains(&"spike_remoteattr::Greeting"), "a remote component is not an import: {names:?}");
}

/// Regression: wasmi's tail-call dispatch keeps the native stack flat only
/// if LLVM turns every handler call into a sibling call, which depends on
/// how wasmi and its dependencies are compiled — with wasmi at opt-level 3
/// it grew the stack per interpreted instruction, and a large bundle
/// overflowed a 2 MB thread mid-mount. `stream-host` uses portable (loop)
/// dispatch, which never grows the stack. The thread is sized explicitly so
/// the test does not depend on RUST_MIN_STACK or the harness default.
///
/// The workload is the heaviest the bridged design runs: a remote component
/// importing an app component, promoting two signals, and re-rendering
/// through both kernels on every update.
#[test]
fn regression_bundle_runs_on_a_2mb_thread() {
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(|| {
            let a = app();
            let realized = a.h.mount(tree(&a));
            a.h.flush();
            let presses = a.h.shared.button_presses.borrow().clone();
            for i in 0..50 {
                a.count.set(i);
                presses[0]();
                a.h.flush();
            }
            let t = text(&a, &realized);
            assert!(t.contains("count 49") && t.contains("panel sees 50"), "{t}");
        })
        .unwrap()
        .join()
        .expect("mount + updates on a 2 MB thread");
}
