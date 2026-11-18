//! A subtree the scene discards is released on BOTH ends of the wire:
//! the client forgets its nodes, and the dev server frees the handler
//! closures they owned.
//!
//! The recorder's stream is replayed into the real client replayer
//! (`dev_client::WireBackend`) over the all-caps mock host — the same
//! path a `idealyst dev --web` page runs, with the DOM swapped for a
//! recording host.
//!
//! Run with `cargo test -p dev-server --test subtree_release_wire`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;

use dev_client::WireBackend;
use dev_server::newcore::SceneSession;
use dev_server::WireRecordingBackend;
use host_mock::{HostMock, Shared};
use runtime_shared::primitives::navigator::Route;
use runtime_shared::{StyleRules, Tokenized};
use runtime_vocabulary::builders::{button, navigator_outlet, swap_navigator, text, view};
use runtime_vocabulary::prims::{MountPolicy, NavHandle};
use runtime_shared::accessibility::{AccessibilityAction, AccessibilityProps};
use wire::{AppToDev, Command, EventArgs};

const HOME: Route<()> = Route::new("home", "/");
const ABOUT: Route<()> = Route::new("about", "/about");

/// A styled screen: its style teardown emits `OnNodeUnstyled` for its
/// root after the scene has released it, which is what the release's
/// ordering has to survive.
fn screen(label: &'static str, presses: Rc<Cell<u32>>) -> impl Fn(()) -> runtime_scene::Element + 'static {
    move |_| {
        let presses = presses.clone();
        view()
            .style(StyleRules {
                width: Some(Tokenized::Literal(runtime_shared::Length::Px(10.0))),
                ..Default::default()
            })
            .child(text().content(label))
            .child(view().child(text().content("row one")).child(text().content("row two")))
            .child(button().label("go").on_press(move || presses.set(presses.get() + 1)))
            .build()
    }
}

struct Wire {
    recorder: WireRecordingBackend,
    session: SceneSession,
    handle: NavHandle,
    client: WireBackend<HostMock>,
    shared: Rc<Shared>,
    /// Presses of either screen's button that reached its handler.
    presses: Rc<Cell<u32>>,
    _outbound: mpsc::Receiver<AppToDev>,
}

impl Wire {
    /// Mount a disposing swap navigator (HOME, ABOUT) on a recorder and
    /// replay the mount into a fresh client.
    fn mount() -> Wire {
        let recorder = WireRecordingBackend::new();
        let handle_slot: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
        let slot = handle_slot.clone();
        let presses = Rc::new(Cell::new(0));
        let p = presses.clone();
        let session = SceneSession::mount(&recorder, |_r| {}, move || {
            swap_navigator(&HOME)
                .screen(HOME, screen("home", p.clone()))
                .screen(ABOUT, screen("about", p))
                .mount_policy(MountPolicy::LazyDisposing)
                .layout(|| view().child(text().content("chrome")).child(navigator_outlet()).build())
                .on_handle(move |handle| *slot.borrow_mut() = Some(handle))
                .build()
        });
        let handle = handle_slot.borrow_mut().take().expect("on_handle filled at mount");
        let shared: Rc<Shared> = Rc::new(Shared::default());
        let (tx, rx) = mpsc::channel::<AppToDev>();
        let client = WireBackend::new_with_shared(Rc::new(RefCell::new(HostMock::new(shared.clone()))), tx);
        let mut wire = Wire { recorder, session, handle, client, shared, presses, _outbound: rx };
        wire.ship();
        wire
    }

    /// Send the recorder's new commands to the client. Every one must
    /// apply: a command naming a node the client already forgot is
    /// skipped, and whatever it would have freed stays.
    fn ship(&mut self) -> Vec<Command> {
        let cmds = self.recorder.drain_commands();
        if let Err(e) = self.client.apply_batch(cmds.clone()) {
            panic!("the client refused a command: {e:?}");
        }
        cmds
    }

    fn go(&mut self, route: &Route<()>) -> Vec<Command> {
        self.session.world().enter(|| self.handle.select(route, ()));
        self.session.flush();
        self.ship()
    }
}

/// REGRESSION (dev web builds, 10.3.2: `Memory.getDOMCounters` nodes
/// climbed a fixed amount per round trip between two routes while the
/// live document stayed flat): a screen the navigator disposed was
/// never released on the client. The recorder ignored
/// `Host::release_subtree`, so the client's node table kept a backend
/// handle to every node of every screen ever left — on web, the whole
/// screen as detached DOM — and its backend's own release never ran.
#[test]
fn regression_leaving_a_screen_over_the_wire_frees_it_on_the_client() {
    let mut wire = Wire::mount();
    // The first trip creates what is made once (the second screen's
    // style registration, the client's first sight of each kind).
    wire.go(&ABOUT);
    wire.go(&HOME);
    let baseline = wire.client.node_count();

    for _ in 0..5 {
        let away = wire.go(&ABOUT);
        assert!(
            away.iter().any(|c| matches!(c, Command::ReleaseSubtree { .. })),
            "leaving a disposing screen sends its release: {away:?}"
        );
        wire.go(&HOME);
    }

    assert_eq!(
        wire.client.node_count(),
        baseline,
        "five round trips grew the client's node table: each left screen \
         is still held"
    );
    let log = wire.shared.log.borrow();
    assert!(
        log.iter().filter(|l| l.starts_with("release_subtree")).count() >= 10,
        "the client's backend is told to release each left screen: {log:?}"
    );
}

/// The release is the last word on its nodes: the teardown of the
/// released screen (its style's `OnNodeUnstyled`) is sent before it, so
/// the client still knows the node when the teardown lands and the
/// backend state that teardown frees is freed.
#[test]
fn a_subtree_release_follows_its_own_teardown() {
    let mut wire = Wire::mount();
    let away = wire.go(&ABOUT);
    let (at, nodes) = away
        .iter()
        .enumerate()
        .find_map(|(i, c)| match c {
            Command::ReleaseSubtree { nodes, .. } => Some((i, nodes.clone())),
            _ => None,
        })
        .expect("leaving HOME releases it");
    let unstyled: Vec<usize> = away
        .iter()
        .enumerate()
        .filter(|(_, c)| matches!(c, Command::OnNodeUnstyled { node } if nodes.contains(node)))
        .map(|(i, _)| i)
        .collect();
    assert!(!unstyled.is_empty(), "the styled screen's teardown reaches the wire: {away:?}");
    assert!(
        unstyled.iter().all(|&i| i < at),
        "every teardown command for the released nodes precedes the release: {away:?}"
    );
}

/// REGRESSION: the dev server never freed a handler closure. Every
/// button, input, link and accessibility action registers the author's
/// callback in the recorder's handler table, and nothing removed it, so
/// each screen ever left kept its callbacks, and everything they
/// captured, for the life of the session. Releasing a node now frees
/// the closures it owns, and the live screen's still answer.
#[test]
fn regression_leaving_a_screen_frees_its_handlers_on_the_dev_server() {
    let mut wire = Wire::mount();
    wire.go(&ABOUT);
    wire.go(&HOME);
    let baseline = wire.recorder.handler_count();

    let mut home = Vec::new();
    for _ in 0..5 {
        wire.go(&ABOUT);
        home = wire.go(&HOME);
    }
    assert_eq!(
        wire.recorder.handler_count(),
        baseline,
        "five round trips grew the handler table: each left screen's callbacks are still registered"
    );

    let on_click = home
        .iter()
        .find_map(|c| match c {
            Command::CreateButton { on_click, .. } => Some(*on_click),
            _ => None,
        })
        .expect("the HOME screen mounts its button");
    assert!(wire.recorder.dispatch_event(on_click, EventArgs::Unit), "the live screen's handler is registered");
    assert_eq!(wire.presses.get(), 1, "and it runs the author's callback");
}

/// An accessibility update registers its actions' handlers anew; the
/// ones it replaces are freed, and the rest go with the node.
#[test]
fn an_accessibility_update_frees_the_actions_it_replaces() {
    let mut recorder = WireRecordingBackend::new();
    let baseline = recorder.handler_count();
    let with_action = || AccessibilityProps {
        actions: vec![AccessibilityAction { name: "Archive".into(), handler: Rc::new(|| {}) }],
        ..Default::default()
    };
    let node = recorder.create_view(&with_action());
    for _ in 0..5 {
        recorder.update_accessibility(&node, &with_action(), None);
    }
    assert_eq!(
        recorder.handler_count(),
        baseline + 2,
        "the create's action and the latest update's, not one per update"
    );
    recorder.release_subtree(&node);
    recorder.drain_commands();
    assert_eq!(recorder.handler_count(), baseline, "a released node frees every handler it owned");
}
