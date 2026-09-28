//! The robot component registry end to end: real `#[component]`s mounted
//! through the realize path with the vocabulary `robot` feature on, read
//! back through the same bridge verbs the Inspector calls.
//!
//! What is pinned:
//!
//! - registering every component adds NO host node (the old `#[method]`
//!   link wrapped each component in a `Dyn` hole, an anchor on anchored
//!   hosts — dev and release trees differed);
//! - each instance links to the element it renders as, nested components
//!   sharing one element outer-first, so the bridge's `components` field
//!   on a tree node reads as the RENDERED hierarchy;
//! - a component rooted in a reactive region re-links when it swaps;
//! - `get_component` reports each prop's mode and CURRENT value;
//! - unmount deregisters, and a component that mounted nothing never
//!   claims its sibling's element.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_macros::{component, ui};
use runtime_scene::{realize, Realized, Registry};
use runtime_vocabulary::glue::__serde_json as serde_json;
use runtime_vocabulary::glue::{signal, Element, Signal};
use runtime_vocabulary::rx;
use runtime_vocabulary::robot::{self, bridge, list_components, Query, Robot};
use runtime_world::World;
use scene_parity::full::FullRecorder;
use scene_parity::{Mode, PNode, Recorder};
use serde_json::{json, Value};

#[component]
fn Leaf(label: String) -> Element {
    ui! { text(test_id = "leaf") { "{label}" } }
}

/// Renders as another component's element.
#[component]
fn Wrapper(label: String) -> Element {
    ui! { Leaf(label = label) }
}

#[component]
fn Card(children: Vec<Element>) -> Element {
    ui! { view(test_id = "card") { children } }
}

/// Rooted in a reactive region.
#[component]
fn Toggler(on: bool) -> Element {
    ui! {
        if on {
            view(test_id = "yes") {}
        } else {
            view(test_id = "no") {}
        }
    }
}

/// Carries `#[method]`s — the one kind of component the old link wrapped.
#[component]
fn Tally(initial: i32) -> Element {
    let value = signal(initial.get());
    #[method]
    fn reset() {
        value.set(0);
    }
    ui! { text(test_id = "tally") { "{value}" } }
}

/// Mounts nothing.
#[component]
fn Nothing() -> Element {
    runtime_scene::fragment(Vec::new())
}

struct Mounted {
    world: World,
    rec: Recorder,
    _realized: Realized<PNode>,
}

impl Mounted {
    fn flush(&self) {
        self.world.flush();
    }
}

fn mount(mode: Mode, build: impl FnOnce() -> Element) -> Mounted {
    Robot::new().reset();
    let rec = Recorder::default();
    let backend = Rc::new(RefCell::new(FullRecorder::new(rec.clone(), mode)));
    let mut registry: Registry<FullRecorder> = Registry::new();
    runtime_vocabulary::register_builtins(&mut registry);
    let registry = Rc::new(registry);
    let world = World::new();
    let root = world.enter(build);
    let realized = world.enter(|| realize(&backend, &registry, root));
    {
        let enter_world = world.clone();
        let settle_world = world.clone();
        robot::install_driver_env(move |f| enter_world.enter(|| f()), move || settle_world.flush());
    }
    Mounted { world, rec, _realized: realized }
}

fn bridge_json(cmd: &str, args: Value) -> Value {
    serde_json::from_str(&bridge::invoke_command(cmd, &args).expect(cmd)).expect("json")
}

/// Every node of the `get_snapshot` tree, with its parent's id.
fn flatten(nodes: &[Value], parent: Option<u64>, out: &mut Vec<(Option<u64>, Value)>) {
    for n in nodes {
        out.push((parent, n.clone()));
        flatten(n["children"].as_array().map(|v| v.as_slice()).unwrap_or(&[]), n["id"].as_u64(), out);
    }
}

fn component_names(node: &Value) -> Vec<String> {
    node["components"]
        .as_array()
        .map(|cs| cs.iter().map(|c| c["name"].as_str().unwrap().to_string()).collect())
        .unwrap_or_default()
}

fn element_id(test_id: &'static str) -> u32 {
    Robot::new().find(Query::test_id(test_id)).unwrap_or_else(|| panic!("no {test_id}")).id.0
}

fn instance_of(name: &str) -> u64 {
    list_components()
        .into_iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("{name} not registered"))
        .id
        .0 as u64
}

#[test]
fn regression_registering_components_adds_no_host_node() {
    // Anchored is the host kind the old wrapper added an anchor on.
    let m = mount(Mode::Anchored, || {
        ui! {
            view {
                Card { Leaf(label = "a") }
                Wrapper(label = "b")
                Tally(initial = 3)
            }
        }
    });
    let ops = m.rec.take_ops();
    assert!(
        !ops.iter().any(|op| op.ends_with(" anchor")),
        "a component tree without reactive regions must create no anchor: {ops:#?}"
    );
    assert_eq!(list_components().len(), 5, "Card, Leaf, Wrapper, Leaf, Tally");
    let tally = list_components().into_iter().find(|c| c.name == "Tally").unwrap();
    assert_eq!(tally.methods, vec![("reset", &[][..])], "methods ride the same registration");
    assert_eq!(tally.element_id.map(|e| e.0), Some(element_id("tally")));
}

#[test]
fn snapshot_nodes_carry_their_components_outer_first() {
    let _m = mount(Mode::Spliced, || {
        ui! {
            view(test_id = "root") {
                Card { Leaf(label = "in-card") }
                Wrapper(label = "wrapped")
            }
        }
    });
    let tree = bridge_json("get_snapshot", json!({}));
    let mut nodes = Vec::new();
    flatten(tree.as_array().unwrap(), None, &mut nodes);
    let by_id = |id: u64| nodes.iter().find(|(_, n)| n["id"].as_u64() == Some(id)).map(|(_, n)| n);

    let card = by_id(element_id("card") as u64).unwrap();
    assert_eq!(component_names(card), ["Card"]);

    let leaves: Vec<&(Option<u64>, Value)> =
        nodes.iter().filter(|(_, n)| n["test_id"] == "leaf").collect();
    assert_eq!(leaves.len(), 2);
    let names: Vec<Vec<String>> = leaves.iter().map(|(_, n)| component_names(n)).collect();
    assert!(names.contains(&vec!["Leaf".to_string()]), "{names:?}");
    assert!(
        names.contains(&vec!["Wrapper".to_string(), "Leaf".to_string()]),
        "a component rendering another's element shares it, outer first: {names:?}"
    );

    // The rendered hierarchy: the in-card Leaf's nearest linked ancestor is
    // the Card's element, even though both were written in the same site.
    let (parent, _) = leaves.iter().find(|(_, n)| component_names(n) == ["Leaf"]).unwrap();
    assert_eq!(*parent, Some(element_id("card") as u64));

    // An element no component links to keeps the old wire shape.
    let root = by_id(element_id("root") as u64).unwrap();
    assert!(root.get("components").is_none(), "{root}");
}

#[test]
fn a_region_rooted_component_relinks_when_it_swaps() {
    let flag: Rc<RefCell<Option<Signal<bool>>>> = Rc::default();
    let flag_in = flag.clone();
    let m = mount(Mode::Anchored, move || {
        let on = signal(false);
        *flag_in.borrow_mut() = Some(on);
        ui! { view { Toggler(on = on) } }
    });
    let linked = || {
        list_components().into_iter().find(|c| c.name == "Toggler").unwrap().element_id.map(|e| e.0)
    };
    assert_eq!(linked(), Some(element_id("no")));

    let on = flag.borrow().unwrap();
    m.world.enter(|| on.set(true));
    m.flush();
    assert_eq!(linked(), Some(element_id("yes")), "the link follows the region's current branch");
}

#[test]
fn get_component_reports_prop_modes_and_current_values() {
    let count: Rc<RefCell<Option<Signal<i32>>>> = Rc::default();
    let count_in = count.clone();
    let m = mount(Mode::Spliced, move || {
        let n = signal(1);
        *count_in.borrow_mut() = Some(n);
        ui! {
            view {
                Card { Leaf(label = "fixed") }
                Wrapper(label = rx!(format!("n={}", n.get())))
            }
        }
    });
    let props_of = |name: &str| {
        let c = bridge_json("get_component", json!({ "instance_id": instance_of(name) }));
        assert_eq!(c["name"], name);
        assert!(c["file"].as_str().unwrap().ends_with("component_tree.rs"), "{c}");
        c["props"].as_array().unwrap().clone()
    };

    let wrapper = props_of("Wrapper");
    assert_eq!(wrapper[0]["name"], "label");
    assert_eq!(wrapper[0]["type"], "Reactive<String>");
    assert_eq!(wrapper[0]["mode"], "live");
    assert_eq!(wrapper[0]["value"], "\"n=1\"");

    let n = count.borrow().unwrap();
    m.world.enter(|| n.set(2));
    m.flush();
    assert_eq!(props_of("Wrapper")[0]["value"], "\"n=2\"", "a Live prop reads its current value");

    let card = props_of("Card");
    assert_eq!((card[0]["name"].as_str(), card[0]["mode"].as_str()), (Some("children"), Some("children")));
    assert_eq!(card[0]["value"], "1 element");

    let fixed = list_components()
        .into_iter()
        .filter(|c| c.name == "Leaf")
        .map(|c| bridge_json("get_component", json!({ "instance_id": c.id.0 })))
        .find(|c| c["props"][0]["mode"] == "static")
        .expect("the literal-label Leaf reports a Static prop");
    assert_eq!(fixed["props"][0]["value"], "\"fixed\"");
}

#[test]
fn unmount_deregisters_every_component() {
    let m = mount(Mode::Spliced, || ui! { view { Card { Leaf(label = "a") } } });
    assert_eq!(list_components().len(), 2);
    drop(m);
    assert!(list_components().is_empty(), "{:?}", list_components().iter().map(|c| c.name).collect::<Vec<_>>());
}

#[test]
fn regression_a_component_that_mounts_nothing_claims_no_sibling() {
    let _m = mount(Mode::Spliced, || ui! { view { Nothing() Leaf(label = "after") } });
    let nothing = list_components().into_iter().find(|c| c.name == "Nothing").unwrap();
    assert_eq!(nothing.element_id, None, "nothing mounted, nothing linked");
    let leaf = list_components().into_iter().find(|c| c.name == "Leaf").unwrap();
    assert_eq!(leaf.element_id.map(|e| e.0), Some(element_id("leaf")));
}
