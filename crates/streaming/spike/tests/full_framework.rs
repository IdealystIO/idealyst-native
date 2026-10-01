//! Model B end to end: the REAL framework inside a bundle. `RemoteCounter`
//! is an ordinary `#[component]` + `ui!`, realized inside the wasm bundle
//! against the wire recorder; the app replays the commands through the real
//! replay client (`dev_client::WireBackend`) into `mock_backend`.

use std::sync::mpsc;

use dev_client::WireBackend;
use dev_server::newcore::SceneSession;
use dev_server::WireRecordingBackend;
use mock_backend::MockBackend;
use runtime_core::{provide, signal, ui};
use spike_components::{CurrentUser, RemoteCounter};
use stream_spike::full::{button_handler, engine, FullGuest};
use stream_spike::FULL_GUEST_WASM;
use wire::{AppToDev, Command};

struct App {
    guest: FullGuest,
    client: WireBackend<MockBackend>,
    _taps: mpsc::Receiver<AppToDev>,
    add: u64,
}

impl App {
    fn mount(user: &str) -> App {
        let mut guest = FullGuest::load(&engine(wasmi::CompilationMode::LazyTranslation), FULL_GUEST_WASM)
            .expect("bundle loads");
        let cmds = guest.mount("Hello from wasm", 1, user);
        let add = button_handler(&cmds, "add").expect("the component rendered its button");
        let (tx, rx) = mpsc::channel();
        let mut client = WireBackend::new(MockBackend::new(), tx);
        client.apply_batch(cmds).expect("replay initial scene");
        App { guest, client, _taps: rx, add }
    }

    fn apply(&mut self, cmds: Vec<Command>) {
        self.client.apply_batch(cmds).expect("replay delta");
    }

    fn has(&self, text: &str) -> bool {
        self.client.backend().borrow().contains_text(text)
    }

    fn dump(&self) -> String {
        self.client.backend().borrow().dump()
    }
}

#[test]
fn ui_component_renders_inside_the_bundle_with_props_and_context() {
    let app = App::mount("ada");
    for t in ["Hello from wasm", "external: 1", "clicks: 0", "signed in as ada"] {
        assert!(app.has(t), "missing {t:?}:\n{}", app.dump());
    }
}

#[test]
fn host_prop_change_reaches_the_bundle() {
    let mut app = App::mount("ada");
    let cmds = app.guest.set_external(42);
    app.apply(cmds);
    assert!(app.has("external: 42"), "{}", app.dump());
}

/// Context passes through: the app's provided `CurrentUser` is mirrored
/// into the bundle's world, read with a plain `inject`, and stays live.
#[test]
fn host_context_change_reaches_the_bundle() {
    let mut app = App::mount("ada");
    let cmds = app.guest.set_user("grace");
    app.apply(cmds);
    assert!(app.has("signed in as grace"), "{}", app.dump());
    assert!(!app.has("signed in as ada"), "{}", app.dump());
}

#[test]
fn tap_on_the_replayed_tree_runs_bundle_state() {
    let mut app = App::mount("ada");
    for _ in 0..2 {
        let cmds = app.guest.dispatch(app.add);
        app.apply(cmds);
    }
    assert!(app.has("clicks: 2"), "{}", app.dump());
}

/// Unmount frees the bundle's world; the APP owns the mount point and drops
/// the replayed subtree itself (the recorder emits no teardown commands —
/// in runtime-server, teardown is the whole session). What must hold is
/// that the bundle can mount again cleanly, with fresh state.
#[test]
fn remount_after_unmount_starts_fresh() {
    let mut app = App::mount("ada");
    let cmds = app.guest.dispatch(app.add);
    app.apply(cmds);
    assert!(app.has("clicks: 1"), "{}", app.dump());

    app.guest.unmount();
    let cmds = app.guest.mount("Hello again", 7, "grace");
    let (tx, _rx) = mpsc::channel();
    let mut fresh = WireBackend::new(MockBackend::new(), tx);
    fresh.apply_batch(cmds).expect("replay remount");
    let scene = fresh.backend().borrow();
    for t in ["Hello again", "external: 7", "clicks: 0", "signed in as grace"] {
        assert!(scene.contains_text(t), "missing {t:?}:\n{}", scene.dump());
    }
}

/// The strongest form of "the bundle is the framework": mounting the same
/// component natively against the same recorder emits the IDENTICAL wire
/// command stream. A stand-in author API could not pass this.
#[test]
fn bundle_emits_exactly_what_the_native_build_emits() {
    let mut guest = FullGuest::load(&engine(wasmi::CompilationMode::LazyTranslation), FULL_GUEST_WASM).unwrap();
    let from_bundle = guest.mount("Hello from wasm", 1, "ada");

    dev_server::scheduler::install();
    let recorder = WireRecordingBackend::new();
    let session = SceneSession::mount(&recorder, |_| {}, || {
        let external = signal(1i64);
        let user = signal("ada".to_string());
        provide(CurrentUser(user.read_only()));
        let external = external.read_only();
        let title = "Hello from wasm".to_string();
        ui! { RemoteCounter(title = title, external = external) }
    });
    session.flush();
    let native = recorder.drain_commands();
    assert_eq!(
        wire::codec::encode(&native).unwrap(),
        wire::codec::encode(&from_bundle).unwrap(),
        "bundle and native renders diverged"
    );
}


/// Regression: wasmi's default tail-call dispatch grew the native stack
/// once per interpreted instruction under some profile settings (the
/// workspace dev profile, wasmi at opt-level 3), and the full-framework
/// bundle overflowed a 2 MB test thread mid-mount. The host now uses
/// portable (loop) dispatch. The thread is sized explicitly so the test does
/// not depend on RUST_MIN_STACK or the harness default.
#[test]
fn regression_full_framework_bundle_fits_a_2mb_thread() {
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(|| {
            let mut app = App::mount("ada");
            for i in 0..50 {
                let cmds = app.guest.set_external(i);
                app.apply(cmds);
            }
            assert!(app.has("external: 49"), "{}", app.dump());
        })
        .unwrap()
        .join()
        .expect("mount + updates on a 2 MB thread");
}
