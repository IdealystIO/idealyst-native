//! Regression: native drawer NAVIGATION over the wire.
//!
//! The bug: over runtime-server, tapping a drawer navlink neither
//! navigated nor closed the drawer. The drawer *opened* (client-side
//! chrome) but `NavigatorSelect` replayed through `dispatch_push_like`,
//! which — for a NATIVE navigator (one reconstructed by driving the
//! client backend's real `create_navigator` to a registered handler) —
//! inserted the new screen into the structural `state.outlet`, a node the
//! native handler ignores. The handler's own dispatcher (which performs
//! the screen swap AND auto-closes the drawer) was never invoked, so
//! nothing happened.
//!
//! The fix: `dispatch_push_like` now has a `state.native` branch that
//! stages the wire-built screen node in `pending_mount` and dispatches the
//! `NavCommand` to `state.control` — the SAME path local (non-wire) mode
//! uses. The handler's `mount_screen` (wired to `pending_mount`) hands
//! back the staged node, and the handler's `Select` dispatcher auto-closes.
//!
//! This drives the full chain — DrawerNavigator → recording handler →
//! wire → dev-client NATIVE reconstruction → a faithful spy handler on
//! MockBackend — and asserts a navlink-triggered select reaches the
//! handler with the right screen and flips its open state shut. The
//! per-platform screen attachment + close animation are the registered
//! handler's job (exercised by local iOS/Android runs); the mock stands
//! in for "a registered native handler" to pin the dev-client routing.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use dev_client::WireBackend;
use dev_server::WireRecordingBackend;
use drawer_navigator::{
    DrawerBuilder, DrawerCmd, DrawerHandle, DrawerNavigator, DrawerPresentation, DrawerScreenExt,
};
use mock_backend::MockBackend;
use runtime_core::primitives::navigator::{
    NavCommand, NavigatorHandler, NavigatorHost, Screen,
};
use runtime_core::{text, view, Backend, Ref, Route, Signal};
use wire::{codec, DevToApp};

const HOME: Route<()> = Route::<()>::new("home", "/");
const ABOUT: Route<()> = Route::<()>::new("about", "/about");

/// Minimal but faithful native drawer handler for MockBackend: mirrors
/// what the real iOS/Android handlers do on the commands the dev-client
/// drives — `Select` mounts the staged screen and auto-closes; the
/// `Custom(DrawerCmd)` verbs flip the open signal. Records each mounted
/// screen node so the test can assert navigation actually reached it.
struct SpyDrawerHandler {
    mounts: Rc<RefCell<Vec<u64>>>,
    is_open_out: Rc<RefCell<Option<Signal<bool>>>>,
}

impl NavigatorHandler<MockBackend> for SpyDrawerHandler {
    fn init(
        &mut self,
        backend: &mut MockBackend,
        host: NavigatorHost<u64>,
        presentation: Rc<dyn Any>,
    ) -> u64 {
        let pres = presentation
            .downcast::<DrawerPresentation>()
            .expect("native drawer presentation");
        let is_open = pres.is_open;
        *self.is_open_out.borrow_mut() = Some(is_open);

        let mount_screen = host.mount_screen.clone();
        let mounts = self.mounts.clone();
        host.control.install(Box::new(move |cmd| match cmd {
            NavCommand::Select { name, params, state, .. } => {
                // The dev-client staged the wire-built screen node in
                // `pending_mount`; `mount_screen` hands it back here.
                let r = mount_screen(name, params, state);
                mounts.borrow_mut().push(r.node);
                // Navigating shuts the drawer — same as the real handlers.
                if is_open.get() {
                    is_open.set(false);
                }
            }
            NavCommand::Custom(payload) => {
                if let Some(c) = payload.downcast_ref::<DrawerCmd>() {
                    match c {
                        DrawerCmd::Open => is_open.set(true),
                        DrawerCmd::Close => is_open.set(false),
                        DrawerCmd::Toggle => {
                            let v = is_open.get();
                            is_open.set(!v);
                        }
                    }
                }
            }
            _ => {}
        }));

        backend.create_view(&Default::default())
    }

    fn attach_initial(
        &mut self,
        _backend: &mut MockBackend,
        screen: u64,
        _scope_id: u64,
        _options: Box<dyn Any>,
    ) {
        self.mounts.borrow_mut().push(screen);
    }
}

fn drawer_app(nav: Ref<DrawerHandle>) -> runtime_core::Element {
    DrawerNavigator::new(&HOME)
        .sidebar(view(vec![text("SIDEBAR NAV").into()]).into())
        .screen(HOME, |_| {
            Screen::new(view(vec![text("HOME BODY").into()])).title("Home")
        })
        .screen(ABOUT, |_| {
            Screen::new(view(vec![text("ABOUT BODY").into()])).title("About")
        })
        .drawer_width(280.0)
        .bind(nav)
        .into()
}

/// Drain the recorder, round-trip through the wire codec, replay into the
/// client. Returns commands applied.
fn sync(recorder: &WireRecordingBackend, client: &mut WireBackend<MockBackend>) -> usize {
    let cmds = recorder.drain_commands();
    if cmds.is_empty() {
        return 0;
    }
    let n = cmds.len();
    let bytes = codec::encode(&DevToApp::Commands(cmds)).expect("encode");
    match codec::decode::<DevToApp>(&bytes).expect("decode") {
        DevToApp::Commands(c) => client.apply_batch(c).expect("replay"),
        other => panic!("expected Commands, got {other:?}"),
    }
    n
}

/// Recursive subtree text search over the mock scene.
fn subtree_has_text(scene: &MockBackend, root: u64, needle: &str) -> bool {
    let Some(node) = scene.node(root) else {
        return false;
    };
    if node.text.as_deref() == Some(needle) {
        return true;
    }
    node.children
        .iter()
        .any(|&c| subtree_has_text(scene, c, needle))
}

#[test]
fn native_select_routes_through_control_and_closes_drawer() {
    // ---- Server (recorder) side: real DrawerNavigator + recording handler.
    dev_server::scheduler::install();
    let mut recorder = WireRecordingBackend::new();
    drawer_navigator::recording::register(&mut recorder);

    let nav_ref: Ref<DrawerHandle> = Ref::new();
    let backend_rc = Rc::new(RefCell::new(recorder.clone()));
    let _owner = {
        let nav_ref = nav_ref.clone();
        runtime_core::mount(backend_rc, move || drawer_app(nav_ref.clone()))
    };
    // Fire the deferred sidebar build before the first sync.
    recorder.tick_animations(Duration::from_millis(16));

    // ---- Client side: register the wire factory + a NATIVE handler so the
    // dev-client takes `create_drawer_navigator_native`, then sync.
    drawer_navigator::register_wire_drawer_factory();
    let (tx, _rx) = mpsc::channel();
    let mut client = WireBackend::new(MockBackend::new(), tx);

    let mounts: Rc<RefCell<Vec<u64>>> = Rc::new(RefCell::new(Vec::new()));
    let is_open_out: Rc<RefCell<Option<Signal<bool>>>> = Rc::new(RefCell::new(None));
    {
        let mounts = mounts.clone();
        let is_open_out = is_open_out.clone();
        client
            .backend()
            .borrow_mut()
            .register_navigator::<DrawerPresentation, _>(move || {
                Box::new(SpyDrawerHandler {
                    mounts: mounts.clone(),
                    is_open_out: is_open_out.clone(),
                })
            });
    }

    sync(&recorder, &mut client);

    // Initial mount: the home screen reached the native handler.
    assert_eq!(
        mounts.borrow().len(),
        1,
        "initial screen should attach via the native handler"
    );
    let home_node = mounts.borrow()[0];
    assert!(
        subtree_has_text(&client.backend().borrow(), home_node, "HOME BODY"),
        "initial mounted node is the home screen"
    );

    let is_open = is_open_out
        .borrow()
        .expect("handler published the is_open signal");

    // Simulate the client drawer being open (user tapped the hamburger —
    // that path is client-local and drives this same signal).
    is_open.set(true);
    assert!(is_open.get(), "drawer open before navigating");

    // ---- The actual regression: a navlink select. Driving the server
    // handle mirrors the wire round-trip a sidebar Link triggers (link tap
    // → event to server → server Select → NavigatorSelect over the wire).
    let handle = nav_ref.get().expect("DrawerHandle bound after render");
    handle.select(&ABOUT, ());
    recorder.tick_animations(Duration::from_millis(16));
    sync(&recorder, &mut client);

    // Navigation reached the handler with the ABOUT screen...
    assert_eq!(
        mounts.borrow().len(),
        2,
        "select must dispatch through state.control to the native handler \
         (pre-fix it stuffed the unused outlet and never dispatched)"
    );
    let about_node = mounts.borrow()[1];
    assert!(
        subtree_has_text(&client.backend().borrow(), about_node, "ABOUT BODY"),
        "the selected screen routed to the handler is the about screen"
    );

    // ...and the drawer auto-closed (handler's Select arm), no wire
    // CloseDrawer required.
    assert!(
        !is_open.get(),
        "selecting a navlink must auto-close the drawer over the wire"
    );
}

/// Regression: programmatic `drawer.open()/close()/toggle()` on the dev
/// side must drive the client's native handler over the wire. These ride
/// `Command::OpenDrawer`/`CloseDrawer`/`ToggleDrawer`, which dev-client
/// used to drop as no-op stubs. They now translate (via the
/// SDK-registered `wire::register_drawer_state_translator`) into the
/// handler's `Custom(DrawerCmd)` and dispatch through the navigator's
/// control — so the client drawer actually animates.
#[test]
fn native_programmatic_open_close_toggle_over_wire() {
    dev_server::scheduler::install();
    let mut recorder = WireRecordingBackend::new();
    drawer_navigator::recording::register(&mut recorder);

    let nav_ref: Ref<DrawerHandle> = Ref::new();
    let backend_rc = Rc::new(RefCell::new(recorder.clone()));
    let _owner = {
        let nav_ref = nav_ref.clone();
        runtime_core::mount(backend_rc, move || drawer_app(nav_ref.clone()))
    };
    recorder.tick_animations(Duration::from_millis(16));

    drawer_navigator::register_wire_drawer_factory();
    // The SDK-registered translator real backends install in their
    // `register()` produces the per-backend helper `DrawerCmd`; the mock's
    // spy handler downcasts the SDK `DrawerCmd`, so the test registers a
    // translator producing that. This exercises the dev-client wiring
    // (translate → dispatch Custom to control) end to end.
    wire::register_drawer_state_translator(|verb| {
        let cmd = match verb {
            wire::DrawerStateVerb::Open => DrawerCmd::Open,
            wire::DrawerStateVerb::Close => DrawerCmd::Close,
            wire::DrawerStateVerb::Toggle => DrawerCmd::Toggle,
        };
        std::rc::Rc::new(cmd) as std::rc::Rc<dyn Any>
    });

    let (tx, _rx) = mpsc::channel();
    let mut client = WireBackend::new(MockBackend::new(), tx);

    let mounts: Rc<RefCell<Vec<u64>>> = Rc::new(RefCell::new(Vec::new()));
    let is_open_out: Rc<RefCell<Option<Signal<bool>>>> = Rc::new(RefCell::new(None));
    {
        let mounts = mounts.clone();
        let is_open_out = is_open_out.clone();
        client
            .backend()
            .borrow_mut()
            .register_navigator::<DrawerPresentation, _>(move || {
                Box::new(SpyDrawerHandler {
                    mounts: mounts.clone(),
                    is_open_out: is_open_out.clone(),
                })
            });
    }
    sync(&recorder, &mut client);

    let handle = nav_ref.get().expect("DrawerHandle bound after render");
    let is_open = is_open_out
        .borrow()
        .expect("handler published the is_open signal");
    assert!(!is_open.get(), "drawer starts closed");

    // open() over the wire → client drawer opens.
    handle.open();
    sync(&recorder, &mut client);
    assert!(
        is_open.get(),
        "programmatic open() must drive the client handler open over the wire \
         (pre-fix OpenDrawer was a no-op stub)"
    );

    // close() over the wire → client drawer closes.
    handle.close();
    sync(&recorder, &mut client);
    assert!(!is_open.get(), "programmatic close() must close the client drawer");

    // toggle() over the wire → client drawer flips back open.
    handle.toggle();
    sync(&recorder, &mut client);
    assert!(is_open.get(), "programmatic toggle() must flip the client drawer open");
}
