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
