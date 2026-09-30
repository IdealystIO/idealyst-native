//! The `highlight_element` / `clear_highlight` bridge verbs: a box drawn
//! over an element with the framework's own `overlay` + `view`, mounted
//! into the first view only while a highlight is up, invisible to
//! introspection.
#![cfg(feature = "robot")]

use host_mock::Harness;
use runtime_scene::{dyn_keyed, realize};
use runtime_shared::__serde_json as serde_json;
use runtime_shared::primitives::portal::ViewportRect;
use runtime_shared::{Length, StyleRules, Tokenized};
use runtime_vocabulary::builders::view;
use runtime_vocabulary::robot::bridge::invoke_command;
use runtime_vocabulary::robot::{Query, Robot};
use runtime_world::signal;
use serde_json::json;

const CARD: ViewportRect = ViewportRect { x: 10.0, y: 20.0, width: 30.0, height: 40.0 };

fn px(v: &Option<Tokenized<Length>>) -> String {
    match v {
        Some(Tokenized::Literal(Length::Px(p))) => format!("{p}"),
        _ => "-".into(),
    }
}

/// A harness whose op log shows each style's box geometry + opacity, and
/// whose robot verbs enter its world (as a real host's driver env does —
/// the refresh timer fires outside `World::enter`).
fn harness() -> Harness {
    Robot::new().reset();
    let h = Harness::new();
    h.set_style_line(|node, r: &StyleRules| {
        let opacity = match &r.opacity {
            Some(Tokenized::Literal(o)) => format!("{o}"),
            _ => "-".into(),
        };
        format!(
            "style n{node} left={} top={} width={} height={} opacity={opacity}",
            px(&r.left),
            px(&r.top),
            px(&r.width),
            px(&r.height)
        )
    });
    let (enter_world, flush_world) = (h.world.clone(), h.world.clone());
    runtime_vocabulary::robot::install_driver_env(move |f| enter_world.enter(|| f()), move || flush_world.flush());
    h
}

fn app() -> runtime_scene::Element {
    view().test_id("root").child(view().test_id("card").build()).build()
}

fn id_of(test_id: &'static str) -> u64 {
    Robot::new().find(Query::test_id(test_id)).expect("registered").id.0 as u64
}

/// Give every node the mock minted `rect` (the verb reads the target's
/// frame; which node number that is doesn't matter here).
fn place_everything_at(h: &Harness, rect: ViewportRect) {
    for n in 0..64 {
        h.shared.frames.borrow_mut().insert(n, rect);
    }
}

fn portals(h: &Harness) -> usize {
    h.ops().iter().filter(|o| o.starts_with("create ") && o.ends_with(" portal")).count()
}

fn last_style_with(h: &Harness, needle: &str) -> Option<String> {
    h.ops().into_iter().filter(|o| o.starts_with("style ") && o.contains(needle)).last()
}

fn highlight(id: u64) -> Result<String, String> {
    invoke_command("highlight_element", &json!({ "element_id": id }))
}

/// A dev build's tree is the release build's tree until a tool asks.
#[test]
fn nothing_is_mounted_until_a_highlight_is_asked_for() {
    let h = harness();
    let _app = h.world.enter(|| realize(&h.backend, &h.registry, app()));
    h.flush();
    assert_eq!(portals(&h), 0, "{}", h.ops().join("\n"));
    assert!(!h.ops().iter().any(|o| o.contains("anchor")), "not even an idle anchor");
}

#[test]
fn highlighting_draws_a_box_at_the_elements_frame_and_clearing_removes_it() {
    let h = harness();
    let _app = h.world.enter(|| realize(&h.backend, &h.registry, app()));
    place_everything_at(&h, CARD);

    assert_eq!(highlight(id_of("card")).as_deref(), Ok("\"ok\""));
    assert_eq!(portals(&h), 1);
    let shown = last_style_with(&h, "left=10 top=20 width=30 height=40");
    assert!(shown.is_some_and(|s| s.ends_with("opacity=1")), "the box sits on the card:\n{}", h.ops().join("\n"));

    // Highlighting another element moves the one box.
    assert!(highlight(id_of("root")).is_ok());
    assert_eq!(portals(&h), 1, "one layer, moved — not a second one");

    let before = h.ops().len();
    invoke_command("clear_highlight", &json!({})).unwrap();
    let after: Vec<String> = h.ops()[before..].to_vec();
    assert!(after.iter().any(|o| o.starts_with("release_portal ")), "the layer is detached: {after:?}");
}

/// The box follows its element (scroll, animation, layout) and comes
/// down on its own when the element unmounts.
#[test]
fn a_highlight_follows_its_element_and_ends_with_it() {
    host_mock::pump::install_scheduler();
    let h = harness();
    let show_card = h.world.enter(|| signal(true));
    let _app = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            view()
                .test_id("root")
                .child(dyn_keyed(move || show_card.get(), |&on| {
                    if on { view().test_id("card").build() } else { runtime_scene::fragment(vec![]) }
                }))
                .build(),
        )
    });
    place_everything_at(&h, CARD);
    highlight(id_of("card")).unwrap();

    place_everything_at(&h, ViewportRect { x: 10.0, y: 120.0, ..CARD });
    host_mock::pump::pump_timers();
    assert!(last_style_with(&h, "top=120").is_some(), "the box followed the card:\n{}", h.ops().join("\n"));

    h.world.enter(|| show_card.set(false));
    h.flush();
    let before = h.ops().len();
    host_mock::pump::pump_timers();
    assert!(h.ops()[before..].iter().any(|o| o.starts_with("release_portal ")), "gone with the card");
    let before = h.ops().len();
    host_mock::pump::pump_timers();
    assert_eq!(h.ops().len(), before, "and the refresh loop stopped");
}

/// The layer is the tool's drawing: it must not show up in what the tool
/// (or the MCP server, or a test) reads back — even while it's mounted.
#[test]
fn the_layer_is_invisible_to_introspection() {
    let h = harness();
    let _app = h.world.enter(|| realize(&h.backend, &h.registry, app()));
    place_everything_at(&h, CARD);
    highlight(id_of("card")).unwrap();
    assert_eq!(portals(&h), 1);
    assert_eq!(invoke_command("count_elements", &json!({})).unwrap(), "2", "root + card only");
    let snapshot = invoke_command("get_snapshot", &json!({})).unwrap();
    let tree: serde_json::Value = serde_json::from_str(&snapshot).unwrap();
    assert_eq!(tree.as_array().map(Vec::len), Some(1), "{snapshot}");
    assert_eq!(tree[0]["children"].as_array().map(Vec::len), Some(1), "only the card under root: {snapshot}");
}

/// The outermost view hosts; when it unmounts, the next view to mount
/// takes over.
#[test]
fn the_first_view_hosts_and_a_new_view_takes_over_after_it_unmounts() {
    let h = harness();
    let first = h.world.enter(|| realize(&h.backend, &h.registry, app()));
    place_everything_at(&h, CARD);
    highlight(id_of("card")).unwrap();
    let root_node = h.ops().iter().find(|o| o.ends_with(" view")).cloned().unwrap();
    let host = root_node.trim_start_matches("create ").trim_end_matches(" view").to_string();
    assert!(
        h.ops().iter().any(|o| o.starts_with(&format!("insert {host} <- "))),
        "the ROOT view ({host}) hosts the layer:\n{}",
        h.ops().join("\n")
    );
    drop(first);
    assert!(highlight(0).is_err(), "nothing mounted: no element, no host");

    let _second = h.world.enter(|| realize(&h.backend, &h.registry, app()));
    place_everything_at(&h, CARD);
    assert!(highlight(id_of("card")).is_ok(), "the new tree's root took over");
    assert_eq!(portals(&h), 2);
}

#[test]
fn highlighting_an_element_without_a_frame_says_so() {
    let h = harness();
    let _app = h.world.enter(|| realize(&h.backend, &h.registry, app()));
    let err = highlight(id_of("card"));
    assert!(err.is_err_and(|e| e.contains("no frame")));
}
