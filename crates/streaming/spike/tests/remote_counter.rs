//! Phase 4: a real `#[component]` + `ui!` component as a REMOTE component.
//!
//! `spike-remoteguest` is `spike-components`' RemoteCounter compiled into a
//! bundle: its kernel is the bridged one (its signals live in this host's
//! graph) and its tree crosses through runtime-vocabulary's element codec.
//! The host realizes that tree with its OWN registry and backend — here the
//! host-mock — so the check is direct: the same component, mounted natively
//! and mounted from the bundle, must drive the backend through the same
//! calls, through the same interactions, and tear down the same way.

use host_mock::Harness;
use runtime_core::{provide, ui, Element};
use runtime_world::Signal;
use spike_components::{CurrentUser, RemoteCounter};
use stream_abi::Wire;
use stream_host::kernel::{export_context, export_read_signal, ExportGuard, KernelBundle};
use stream_spike::REMOTE_GUEST_WASM;

const TITLE: &str = "Hello from wasm";

struct Inputs {
    external: Signal<i64>,
    user: Signal<String>,
}

/// What the native app does to mount the component.
fn native_tree(h: &Harness, i: &Inputs) -> Element {
    let (external, user) = (i.external, i.user);
    h.world.enter(|| {
        provide(CurrentUser(user.read_only()));
        let external = external.read_only();
        let title = TITLE.to_string();
        ui! { RemoteCounter(title = title, external = external) }
    })
}

/// A mounted bundle and what must outlive its tree.
struct Remote {
    bundle: KernelBundle,
    _guards: Vec<ExportGuard>,
}

/// What the app does to mount the same component from the bundle: export
/// the prop and the context, call the mount export, decode.
fn remote_tree(h: &Harness, i: &Inputs) -> (Element, Remote) {
    let engine = stream_host::remote::engine();
    let bundle = KernelBundle::load(&engine, REMOTE_GUEST_WASM).expect("remote bundle loads");
    let (external, user) = (i.external, i.user);
    let (eh, g1) = export_read_signal(external.read_only());
    let (uh, g2) = export_read_signal(user.read_only());
    // The bundle's `CurrentUser` is the handle of the host's user signal.
    let g3 = export_context::<CurrentUser>("CurrentUser", move |_, out| {
        for v in [uh.0, uh.1, uh.2] {
            v.encode(out);
        }
    });
    let mut args = Vec::new();
    TITLE.to_string().encode(&mut args);
    for v in [eh.0, eh.1, eh.2] {
        v.encode(&mut args);
    }
    let tree = h.world.enter(|| {
        provide(CurrentUser(user.read_only()));
        bundle.mount_remote("rc_mount", &args).unwrap_or_else(|e| panic!("{e}"))
    });
    (tree, Remote { bundle, _guards: vec![g1, g2, g3] })
}

/// Mount, then: press the component's button (bundle state), change the
/// host prop, change the host context, unmount. The backend log per step.
fn script(remote: bool) -> Vec<Vec<String>> {
    let h = Harness::new();
    let inputs = h.world.enter(|| Inputs {
        external: runtime_world::signal(1i64),
        user: runtime_world::signal("ada".to_string()),
    });
    let (tree, keep) = if remote {
        let (t, r) = remote_tree(&h, &inputs);
        (t, Some(r))
    } else {
        (native_tree(&h, &inputs), None)
    };
    let mut steps = Vec::new();

    let realized = h.mount(tree);
    h.flush();
    steps.push(h.take_log());

    let press = h.shared.button_presses.borrow()[0].clone();
    press();
    h.flush();
    press();
    h.flush();
    steps.push(h.take_log());

    inputs.external.set(42);
    h.flush();
    steps.push(h.take_log());

    inputs.user.set("grace".to_string());
    h.flush();
    steps.push(h.take_log());

    drop(realized);
    h.flush();
    steps.push(h.take_log());
    drop(press);
    drop(h);
    drop(keep);
    steps
}

#[test]
fn remote_counter_drives_the_backend_exactly_like_the_native_build() {
    let native = script(false);
    let remote = script(true);
    let mount = native[0].join("\n");
    for t in ["Hello from wasm", "external: 1", "clicks: 0", "signed in as ada"] {
        assert!(mount.contains(t), "the native mount renders {t:?}:\n{mount}");
    }
    assert!(native[1].join("\n").contains("clicks: 2"), "{:?}", native[1]);
    for (step, (n, r)) in native.iter().zip(&remote).enumerate() {
        assert_eq!(r, n, "step {step}: the remote component and the native build diverge");
    }
}

/// Unmounting the remote tree releases everything the host held of the
/// bundle — its callbacks, and (through the root `Owned`, a host scope) its
/// signals and effects: afterwards a host prop change reaches nothing.
#[test]
fn unmounting_the_remote_tree_leaves_nothing_subscribed() {
    let h = Harness::new();
    let inputs = h.world.enter(|| Inputs {
        external: runtime_world::signal(1i64),
        user: runtime_world::signal("ada".to_string()),
    });
    let (tree, keep) = remote_tree(&h, &inputs);
    assert_eq!(runtime_world::remote::pending_scopes(), 0, "every scope the bundle sent was claimed at decode");
    let live = || keep.bundle.call::<(), u32>("idealyst_ui_live_callbacks", ());
    assert!(live() > 0);
    let realized = h.mount(tree);
    h.flush();
    assert!(inputs.external.subscriber_count() > 0, "the bundle's text subscribes to the host prop");
    drop(realized);
    h.flush();
    // The mock backend keeps the button's press handler; it is a real owner.
    h.shared.button_presses.borrow_mut().clear();
    assert_eq!(live(), 0, "a bundle callback outlived the tree that used it");
    assert_eq!(runtime_world::remote::pending_scopes(), 0);
    assert_eq!(inputs.external.subscriber_count(), 0, "a bundle effect survived the unmount");
    h.take_log();
    inputs.external.set(5);
    h.flush();
    assert_eq!(h.take_log(), Vec::<String>::new(), "a host prop change reached an unmounted remote tree");
}

/// A bundle that panics while building its tree fails the mount with the
/// bundle's own panic message — the app shows it instead of dying.
#[test]
fn a_panicking_mount_is_an_error_carrying_the_bundles_message() {
    let h = Harness::new();
    let engine = stream_host::remote::engine();
    let bundle = KernelBundle::load(&engine, REMOTE_GUEST_WASM).expect("remote bundle loads");
    // No args: the mount export fails decoding its title.
    let err = h.world.enter(|| bundle.mount_remote("rc_mount", &[])).err().expect("the mount fails");
    match err {
        stream_host::kernel::MountError::Panicked(msg) => assert!(msg.contains("rc_mount: title"), "{msg}"),
        other => panic!("expected a panic, got {other}"),
    }
}
