//! Wave 2b: robot verbs AND the MCP catalog coexist in a `--new-core`
//! dev session.
//!
//! Pre-wave-2b, a new-core sidecar session had neither: the bridge
//! never started, element verbs would have dispatched against the
//! EMPTY old registry, and the catalog inventory wasn't even linked
//! (`runtime-core/dev` was unbuildable in new-core graphs while the
//! recipes compiled `ui!` inside runtime-core). This test drives the
//! exact wiring the sidecar session thread now installs —
//! [`dev_server::newcore::install_robot_env`] — through the shared TCP
//! bridge's in-process dispatch (`runtime_shared::robot::bridge::
//! invoke_command`, the same path `BridgeHandle::poll` runs per socket
//! command) and asserts the routing contract from both sides:
//!
//! - element verbs (`find_element`, `click`) answer from the VOCABULARY
//!   registry, with queries world-entered and actions settled through
//!   `SceneSession::flush` (a click's staged write is visible in the
//!   registry's label immediately after the verb returns);
//! - registry-independent verbs (`get_catalog`, custom commands) fall
//!   through to the shared bridge's own dispatch — the catalog JSON
//!   (with the static core recipes anchored in runtime-shared) is served
//!   in the same session.
//!
//! This file used to open with `#![cfg(feature = "new-core")]`. When that
//! core-selector feature was deleted the attribute silently reduced the
//! whole file to ZERO tests instead of failing to compile — the exact
//! "goes silent, not red" failure the deletion baseline warns about
//! (§4.1 #8). It is unconditional now.

use std::cell::RefCell;
use std::rc::Rc;

use dev_server::newcore::{clear_robot_env, install_robot_env, SceneSession};
use dev_server::WireRecordingBackend;
use runtime_vocabulary::builders::{button, text, view};

fn invoke(cmd: &str, args: serde_json::Value) -> Result<String, String> {
    runtime_shared::robot::bridge::invoke_command(cmd, &args)
}

#[test]
fn newcore_session_serves_robot_verbs_and_catalog_over_one_bridge() {
    let recorder = WireRecordingBackend::new();

    // The app: a counter whose button label reads a world signal, so
    // the click → settle → re-query loop proves the driver env commits
    // staged writes synchronously (world-entered label resolution).
    let session = SceneSession::mount(&recorder, |_r| {}, || {
        let count = runtime_world::signal(0i32);
        view()
            .test_id("root")
            .child(
                button()
                    .label(move || format!("count = {}", count.get()))
                    .on_press(move || count.set(count.get() + 1))
                    .test_id("inc")
                    .build(),
            )
            .child(text().content("static").test_id("caption").build())
            .build()
    });
    let holder: Rc<RefCell<Option<SceneSession>>> = Rc::new(RefCell::new(Some(session)));
    install_robot_env(&holder);

    // --- Element verbs route to the vocabulary registry -------------
    let found = invoke("find_element", serde_json::json!({ "test_id": "inc" }))
        .expect("find_element routes to the vocab registry");
    let el: serde_json::Value = serde_json::from_str(&found).expect("element JSON");
    assert_eq!(el["kind"], "Button", "vocab registry entry: {el}");
    assert_eq!(el["label"], "count = 0", "world-entered label read: {el}");
    let id = el["id"].as_u64().expect("element id");

    // Action + settle: the staged write must be committed (and the
    // label re-resolved) by the time the NEXT query answers.
    invoke("click", serde_json::json!({ "element_id": id })).expect("click dispatches");
    let after = invoke("find_element", serde_json::json!({ "test_id": "inc" }))
        .expect("re-query after click");
    let el2: serde_json::Value = serde_json::from_str(&after).unwrap();
    assert_eq!(
        el2["label"], "count = 1",
        "click's staged write must settle before the verb returns: {el2}"
    );

    // --- Registry-independent verbs fall through to old dispatch ----
    // `get_catalog`: the vocab bridge doesn't own it; the router must
    // return None so the shared dispatch serves the linked catalog —
    // including the core recipes re-anchored into runtime-shared.
    let catalog = invoke("get_catalog", serde_json::json!({}))
        .expect("get_catalog falls through to the old dispatch");
    let cat: serde_json::Value = serde_json::from_str(&catalog).expect("catalog JSON");
    let recipes = cat["recipes"].as_array().expect("recipes slice in catalog");
    let names: Vec<&str> = recipes
        .iter()
        .filter_map(|r| r["name"].as_str())
        .collect();
    for expected in [
        "input_with_submit",
        "keyed_list_add_remove",
        "animated_toast",
        "confirm_dialog_overlay",
    ] {
        assert!(
            names.contains(&expected),
            "core recipe {expected:?} missing from get_catalog: {names:?}"
        );
    }

    // Custom commands (the sidecar's `screenshot` shape) still reach
    // the old dispatch's custom table through the fallback.
    runtime_shared::robot::bridge::register_command("wave2b_probe", |_args| {
        Ok("\"probe-ok\"".into())
    });
    assert_eq!(
        invoke("wave2b_probe", serde_json::json!({})).as_deref(),
        Ok("\"probe-ok\""),
    );
    runtime_shared::robot::bridge::unregister_command("wave2b_probe");

    // A REAL vocab verb error (bad args) must NOT be masked by the
    // fallback — only the `unknown command:` marker falls through.
    let err = invoke("click", serde_json::json!({})).expect_err("click without id errors");
    assert!(
        !err.starts_with("unknown command:"),
        "real verb errors surface as-is: {err}"
    );

    // --- Teardown: dropping the session deregisters everything ------
    clear_robot_env();
    holder.borrow_mut().take();
    runtime_vocabulary::robot::Robot::new().reset();
}
