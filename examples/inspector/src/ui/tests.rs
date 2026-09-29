//! Screen mount tests: each screen realized against the recording mock
//! host from a fixture [`Snapshot`], asserting on the text it shows — and,
//! for the parts that must follow the target live, that a new snapshot
//! updates the mounted text in place.

use std::rc::Rc;

use idea_ui::{dark_theme, install_idea_theme};
use runtime_core::{signal, ui, Element, Signal};

use super::components::TestDetail;
use super::logs::LogsScreen;
use super::navigation::NavigationScreen;
use super::shell::Shell;
use super::signals::SignalsScreen;
use crate::bridge::model::*;
use crate::Target;

struct Mounted {
    harness: host_mock::Harness,
    _realized: runtime_scene::Realized<host_mock::Node>,
}

impl Mounted {
    fn ops(&self) -> String {
        self.harness.ops().join("\n")
    }
    fn shows(&self, text: &str) -> bool {
        self.ops().contains(&format!("{text:?}"))
    }
}

fn mount(build: impl FnOnce() -> Element) -> Mounted {
    let harness = host_mock::Harness::new();
    let tree = harness.world.enter(|| {
        install_idea_theme(dark_theme());
        build()
    });
    let realized = harness.mount(tree);
    harness.flush();
    Mounted { harness, _realized: realized }
}

fn node(id: u64, kind: &str, test_id: Option<&str>, comps: &[(u64, &str)], children: Vec<ElementNode>) -> ElementNode {
    ElementNode {
        id,
        kind: kind.into(),
        test_id: test_id.map(str::to_string),
        label: None,
        components: comps.iter().map(|(i, n)| ComponentRef { instance_id: *i, name: n.to_string() }).collect(),
        children,
    }
}

fn fixture() -> Snapshot {
    Snapshot {
        status: Status::Live { rtt_ms: 14 },
        tree: vec![node(
            1,
            "View",
            None,
            &[(1, "App")],
            vec![node(2, "View", Some("card"), &[(2, "Card")], vec![node(3, "Text", Some("counter"), &[(3, "Counter")], vec![])])],
        )],
        component_count: 3,
        component: Some(ComponentDetail {
            instance_id: 3,
            name: "Counter".into(),
            file: "src/counter.rs".into(),
            line: 14,
            element_id: Some(3),
            methods: vec![Method {
                name: "bump_by".into(),
                args: vec![MethodArg { name: "n".into(), ty: "i32".into() }],
            }],
            props: vec![
                Prop { name: "label".into(), ty: "Reactive<String>".into(), mode: "static".into(), value: Some("\"Count\"".into()) },
                Prop { name: "step".into(), ty: "Reactive<i32>".into(), mode: "live".into(), value: Some("2".into()) },
            ],
        }),
        element: Some(ElementDetail {
            element_id: 3,
            frame: Some(Rect { x: 24.0, y: 312.0, width: 342.0, height: 96.0 }),
            native: None,
        }),
        signals: vec![SignalRow {
            id: 9,
            name: "count".into(),
            value: serde_json::json!("4"),
            writes: 17,
            changed_ago_ms: Some(2_000),
            writable: true,
        }],
        signal_history: Some(SignalHistory {
            id: 9,
            name: "count".into(),
            writes: 2,
            history: vec![
                HistoryPoint { ago_ms: Some(9_000), value: "1".into() },
                HistoryPoint { ago_ms: Some(2_000), value: "4".into() },
            ],
        }),
        navigators: vec![Navigator {
            nav_id: 0,
            element_id: Some(7),
            type_name: "stack_navigator".into(),
            active_route: "counter".into(),
            active_path: "/counter?step=2".into(),
            depth: 2,
            can_go_back: true,
            is_current: true,
            base: String::new(),
            stack: vec![
                StackEntry { route: "home".into(), path: "/".into() },
                StackEntry { route: "counter".into(), path: "/counter?step=2".into() },
            ],
            controllable: true,
        }],
        logs: vec![LogRow { ts: 1_790_000_000_000, source: "stdout".into(), text: "value changed 3 → 4".into() }],
        perf: Perf::Rows(vec![PhaseRow { phase: "execute_batch_total".into(), call_count: 12, total_us: 38_200, max_us: 1_900 }]),
        last_action: None,
    }
}

#[test]
fn shell_shows_the_target_and_the_component_tree() {
    let m = mount(|| {
        let snapshot: Signal<Snapshot> = signal(fixture());
        ui! {
            Shell(
                snapshot = snapshot,
                target = Target { name: "conformance".into(), detail: "macOS · pid 48213".into() },
                on_disconnect = Rc::new(|| {}) as Rc<dyn Fn()>,
            )
        }
    });
    for text in ["conformance", "macOS · pid 48213", "Live · 14 ms round trip", "App", "Card", "Counter", "3 components · 3 elements"] {
        assert!(m.shows(text), "missing {text:?} in:\n{}", m.ops());
    }
    assert!(m.shows("Components") && m.shows("Signals") && m.shows("Logs & perf"));
}

/// The detail pane renders the selected instance: props with their
/// modes, methods, and the root element — and follows a new snapshot in
/// place (a Live prop's value changing must not need a reselect).
#[test]
fn component_detail_shows_props_methods_and_follows_the_target() {
    let slot: Rc<std::cell::Cell<Option<(Signal<Snapshot>, Signal<Option<u64>>)>>> = Rc::default();
    let slot_in = slot.clone();
    let m = mount(move || {
        let snapshot: Signal<Snapshot> = signal(fixture());
        let selected: Signal<Option<u64>> = signal(Some(3));
        slot_in.set(Some((snapshot, selected)));
        ui! { TestDetail(snapshot = snapshot, selected = selected) }
    });
    for text in [
        "App › Card › Counter",
        "instance #3 · src/counter.rs:14",
        "label",
        "Reactive<i32>",
        "\"Count\"",
        "Live",
        "Static",
        "bump_by(n: i32)",
        "x 24 · y 312 · 342 × 96",
    ] {
        assert!(m.shows(text), "missing {text:?} in:\n{}", m.ops());
    }

    let (snapshot, _) = slot.get().unwrap();
    let mut next = fixture();
    next.component.as_mut().unwrap().props[1].value = Some("5".into());
    m.harness.world.enter(|| snapshot.set(next));
    m.harness.flush();
    assert!(m.shows("5"), "the Live prop's new value renders:\n{}", m.ops());
}

#[test]
fn signals_screen_lists_and_details_a_signal() {
    let m = mount(|| {
        let snapshot: Signal<Snapshot> = signal(fixture());
        ui! { SignalsScreen(snapshot = snapshot) }
    });
    for text in ["count", "4", "17", "2 s ago"] {
        assert!(m.shows(text), "missing {text:?} in:\n{}", m.ops());
    }
}

#[test]
fn navigation_screen_shows_the_back_stack_and_controls() {
    let m = mount(|| {
        let snapshot: Signal<Snapshot> = signal(fixture());
        ui! { NavigationScreen(snapshot = snapshot) }
    });
    for text in ["Stack navigator #0", "/counter?step=2", "home", "counter", "step", "2", "Push", "Pop"] {
        assert!(m.shows(text), "missing {text:?} in:\n{}", m.ops());
    }
}

#[test]
fn logs_screen_shows_lines_and_accumulated_timers() {
    let m = mount(|| {
        let snapshot: Signal<Snapshot> = signal(fixture());
        ui! { LogsScreen(snapshot = snapshot) }
    });
    for text in ["value changed 3 → 4", "stdout", "execute_batch_total", "38.2 ms", "1.9 ms"] {
        assert!(m.shows(text), "missing {text:?} in:\n{}", m.ops());
    }
}

#[test]
fn logs_screen_says_why_timers_are_missing() {
    let m = mount(|| {
        let mut snap = fixture();
        snap.perf = Perf::Unavailable("perf disabled: rebuild the target app with the `debug-stats` feature".into());
        let snapshot: Signal<Snapshot> = signal(snap);
        ui! { LogsScreen(snapshot = snapshot) }
    });
    assert!(m.ops().contains("perf disabled"), "{}", m.ops());
}

fn app_info(id: &str, name: &str) -> AppInfo {
    AppInfo { id: id.into(), name: name.into(), port: 5001, pid: 48213, platform: Some("web".into()), ..AppInfo::default() }
}

/// The picker lists what the SERVER pushed (the front end can't read
/// `~/.idealyst/apps` in a browser), and follows a new list in place.
#[test]
fn connect_lists_the_servers_apps_and_its_state() {
    use crate::bridge::client::ServerLink;
    use super::connect::Connect;

    let slot: Rc<std::cell::Cell<Option<(Signal<Vec<AppInfo>>, Signal<ServerLink>)>>> = Rc::default();
    let slot_in = slot.clone();
    let m = mount(move || {
        let apps: Signal<Vec<AppInfo>> = signal(vec![app_info("Todo-48213", "Todo")]);
        let link: Signal<ServerLink> = signal(ServerLink::Connected);
        slot_in.set(Some((apps, link)));
        ui! { Connect(apps = apps.read_only(), link = link.read_only()) }
    });
    for text in ["Todo", "Web", "pid 48213 · 127.0.0.1:5001", "Connected to the Inspector server."] {
        assert!(m.shows(text), "missing {text:?} in:\n{}", m.ops());
    }

    let (apps, link) = slot.get().unwrap();
    m.harness.world.enter(|| {
        apps.set(vec![app_info("Todo-48213", "Todo"), app_info("Notes-7", "Notes")]);
        link.set(ServerLink::Down("can't reach the Inspector server at ws://127.0.0.1:9719/ws".into()));
    });
    m.harness.flush();
    assert!(m.shows("Notes"), "a newly pushed app appears:\n{}", m.ops());
    assert!(m.ops().contains("Start it with idealyst inspect"), "{}", m.ops());
}

/// Frames the client applies — and the one it must drop.
#[test]
fn client_applies_server_frames_and_drops_a_previous_apps_snapshot() {
    use crate::bridge::client::{self, ServerLink, Sinks};
    use inspector_protocol::ServerMsg;

    let harness = host_mock::Harness::new();
    // Writes inside `enter` are staged until the flush, as they are
    // inside any reactive pass; the real client receives from an async
    // task, outside one.
    let step = |f: &dyn Fn()| {
        harness.world.enter(f);
        harness.flush();
    };
    let sinks = harness.world.enter(|| Sinks {
        link: signal(ServerLink::Connected),
        apps: signal(Vec::new()),
        snapshot: signal(Snapshot::default()),
    });
    let snap = || harness.world.enter(|| sinks.snapshot.get());

    step(&|| client::receive(ServerMsg::Apps { apps: vec![app_info("Todo-1", "Todo")] }, sinks));
    assert_eq!(harness.world.enter(|| sinks.apps.get())[0].id, "Todo-1");

    let live = Snapshot { status: Status::Live { rtt_ms: 3 }, ..Snapshot::default() };
    client::attach("Todo-1".into());
    step(&|| client::receive(ServerMsg::Snapshot { app: "Todo-1".into(), snapshot: Box::new(live.clone()) }, sinks));
    assert_eq!(snap(), live);

    // The user switched to Notes while a Todo frame was in flight.
    client::attach("Notes-2".into());
    let stale = Snapshot { status: Status::Live { rtt_ms: 99 }, ..Snapshot::default() };
    step(&|| client::receive(ServerMsg::Snapshot { app: "Todo-1".into(), snapshot: Box::new(stale.clone()) }, sinks));
    assert_eq!(snap(), live, "a frame for the previous app is dropped");

    client::detach();
    step(&|| client::receive(ServerMsg::Snapshot { app: "Notes-2".into(), snapshot: Box::new(Snapshot::default()) }, sinks));
    assert_eq!(snap(), live, "nothing lands after detaching");

    let newer = inspector_protocol::PROTOCOL_VERSION + 1;
    step(&|| client::receive(ServerMsg::Hello { protocol: newer }, sinks));
    assert_eq!(harness.world.enter(|| sinks.link.get()), ServerLink::Incompatible { server: newer });
}

/// Disconnect unmounts the Shell from inside the Shell's own button
/// handler.
#[test]
fn regression_disconnect_from_the_shell_does_not_touch_a_freed_signal() {
    let slot: Rc<std::cell::Cell<Option<Signal<Snapshot>>>> = Rc::default();
    let slot_in = slot.clone();
    let m = mount(move || {
        let snapshot: Signal<Snapshot> = signal(fixture());
        slot_in.set(Some(snapshot));
        let connected = signal(true);
        let on_disconnect = move || connected.set(false);
        ui! {
            view() {
                if connected {
                    Shell(
                        snapshot = snapshot,
                        target = Target { name: "conformance".into(), detail: "macOS · pid 48213".into() },
                        on_disconnect = Rc::new(on_disconnect) as Rc<dyn Fn()>,
                    )
                } else {
                    text() { "picker" }
                }
            }
        }
    });
    let snapshot = slot.get().unwrap();
    // Press handlers in creation order: the four section links, the
    // sidebar's Disconnect, the "Show elements" checkbox, then the tree
    // rows' chevron + label pairs (App 6-7, Card 8-9; Counter has no
    // children, so its label is 10).
    let select_counter = m.harness.press_handler(10);
    let disconnect = m.harness.press_handler(4);
    m.harness.world.enter(|| select_counter());
    m.harness.flush();
    assert!(m.shows("instance #3 · src/counter.rs:14"), "the detail pane is mounted:\n{}", m.ops());
    // A snapshot frame lands in the same flush as the click.
    m.harness.world.enter(|| {
        let mut next = fixture();
        next.status = Status::Live { rtt_ms: 15 };
        next.component.as_mut().unwrap().line = 15;
        snapshot.set(next);
        disconnect();
    });
    m.harness.flush();
    assert!(m.shows("picker"), "{}", m.ops());
}
