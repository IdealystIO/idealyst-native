//! Web Robot transport — the dial-out client that gives a browser app the
//! Robot bridge it can't host itself.
//!
//! A wasm app can't bind a TCP listener, so it can't run the native Robot
//! bridge. Instead it **dials out** to a `robot-relay` over a WebSocket and
//! services the exact same verbs the native bridge does. The relay exposes the
//! ordinary TCP bridge to the MCP server, so the MCP/evaluator side is
//! unchanged. This is the web implementation of the relay's canonical protocol;
//! native conforms to it later.
//!
//! Protocol (text frames):
//! ```text
//! app → relay   {"hello":{"name":…,"platform":"web"}}     once, on open
//! relay → app   {"id":N,"cmd":"find_element","args":{…}}  a forwarded request
//! app → relay   {"id":N,"ok":<value>} | {"id":N,"err":…}  the dispatched result
//! app → relay   {"event":"changed","rev":R}               a push, while subscribed
//! ```
//!
//! `invoke_command` runs the same dispatch the native bridge's `poll` does, on
//! the UI thread — which on web is exactly where this `onmessage` closure fires,
//! so the thread-local Robot registry is in scope.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{MessageEvent, WebSocket};

// ---------------------------------------------------------------------------
// Core selection — old registry vs the new-core vocabulary registry
// ---------------------------------------------------------------------------

/// Dispatch one bridge verb against whichever core is running.
///
/// A new-core boot (`newcore::start`) leaves the OLD registry empty —
/// routing verbs there would answer `find_element` with `null` instead
/// of an error, silently blinding every driver. So when the new-core
/// app is booted, verbs go to `runtime_vocabulary::robot::bridge`
/// (wire-identical responses); verbs that registry doesn't own
/// (`get_logs`, custom commands like the dev-server's) fall back to the
/// old dispatch, whose log/custom machinery is registry-independent.
/// The fallback keys on the exact `unknown command:` marker so a REAL
/// verb error (missing argument, deferred seam) is never masked.
fn dispatch_verb(cmd: &str, args: &serde_json::Value) -> Result<String, String> {
        if crate::newcore::is_booted() {
        return match runtime_vocabulary::robot::bridge::invoke_command(cmd, args) {
            Err(e) if e.starts_with("unknown command:") => {
                runtime_shared::robot::bridge::invoke_command(cmd, args)
            }
            other => other,
        };
    }
    runtime_shared::robot::bridge::invoke_command(cmd, args)
}

/// The live-update revision for the push pump — the new-core registry's
/// counter when that core is booted, the old one otherwise.
fn robot_revision() -> u64 {
        if crate::newcore::is_booted() {
        return runtime_vocabulary::robot::current_revision();
    }
    runtime_shared::robot::current_revision()
}

/// Install the vocabulary robot driver env over this host's boot seams:
/// queries enter the mounted world (label_fn reads world signals),
/// actions settle via `flush_sync` (staged writes commit before the
/// verb returns — the old core's synchronous-apply parity). Called by
/// `newcore::start_in` once the flush world exists.
pub(crate) fn install_newcore_driver_env() {
    runtime_vocabulary::robot::install_driver_env(
        |f| {
            // Pre-boot / post-stop there is no world; run plainly so a
            // query still resolves static labels instead of panicking.
            if crate::newcore::with_world_entered(|| f()).is_none() {
                f();
            }
        },
        crate::newcore::flush_sync,
    );
}

/// Uninstall the env (host `stop()` — tests boot repeatedly).
pub(crate) fn clear_newcore_driver_env() {
    runtime_vocabulary::robot::clear_driver_env();
}

/// Kept alive for the page lifetime so the socket + closures + push pump aren't
/// dropped (which would tear the connection down).
struct RobotRelayState {
    _socket: WebSocket,
    _on_open: Closure<dyn FnMut(JsValue)>,
    _on_message: Closure<dyn FnMut(MessageEvent)>,
    _push_pump: runtime_shared::scheduling::RafLoop,
}

thread_local! {
    static INSTALLED: RefCell<Option<RobotRelayState>> = const { RefCell::new(None) };
}

/// Connect this web app's Robot bridge to a relay at `url` (e.g.
/// `ws://127.0.0.1:9719`). Idempotent per page; the connection persists for the
/// page lifetime. Called from the generated web wrapper when the build enabled
/// robot and the dev sidecar injected a relay URL.
pub fn install_robot_relay_client(url: &str) -> Result<(), JsValue> {
    if INSTALLED.with(|s| s.borrow().is_some()) {
        return Ok(());
    }

    let socket = WebSocket::new(url)?;

    // --- on_open: announce identity -----------------------------------------
    let socket_for_open = socket.clone();
    let on_open = Closure::wrap(Box::new(move |_evt: JsValue| {
        let hello = serde_json::json!({
            "hello": { "name": env!("CARGO_PKG_NAME"), "platform": "web" }
        });
        let _ = socket_for_open.send_with_str(&hello.to_string());
    }) as Box<dyn FnMut(JsValue)>);
    socket.set_onopen(Some(on_open.as_ref().unchecked_ref()));

    // --- subscription state (shared with the push pump) ---------------------
    let subscribed = Rc::new(Cell::new(false));

    // --- on_message: dispatch forwarded verbs -------------------------------
    let socket_for_msg = socket.clone();
    let subscribed_msg = subscribed.clone();
    let on_message = Closure::wrap(Box::new(move |evt: MessageEvent| {
        let Some(text) = evt.data().as_string() else {
            return;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            return;
        };
        let id = v.get("id").cloned().unwrap_or(serde_json::Value::from(0));
        let cmd = v.get("cmd").and_then(|c| c.as_str()).unwrap_or("");
        let args = v
            .get("args")
            .cloned()
            .unwrap_or_else(|| serde_json::Value::Object(Default::default()));

        // `subscribe` is handled by the transport (like the native bridge's
        // connection loop), not the dispatch core: ack, then let the push pump
        // emit change events.
        if cmd == "subscribe" {
            subscribed_msg.set(true);
            let _ = socket_for_msg.send_with_str(&format!("{{\"id\":{id},\"ok\":\"subscribed\"}}"));
            return;
        }

        // `screenshot` can't go through the sync `invoke_command` path — DOM
        // rasterization is async (image load). Capture off-band and send the
        // bridge response when it completes; the relay just forwards it.
        if cmd == "screenshot" {
            let socket = socket_for_msg.clone();
            let id_for_shot = id.clone();
            crate::robot_screenshot::capture(Box::new(move |res| {
                let resp = match res {
                    Ok((b64, w, h)) => format!(
                        "{{\"id\":{id_for_shot},\"ok\":{{\"png_base64\":\"{b64}\",\"width\":{w},\"height\":{h}}}}}"
                    ),
                    Err(e) => format!(
                        "{{\"id\":{id_for_shot},\"err\":{}}}",
                        serde_json::to_string(&e).unwrap_or_else(|_| "\"screenshot error\"".into())
                    ),
                };
                let _ = socket.send_with_str(&resp);
            }));
            return;
        }

        // Same wrapping the native `BridgeHandle::poll` does. Routed by
        // running core — see `dispatch_verb`.
        let resp = match dispatch_verb(cmd, &args) {
            Ok(value) => format!("{{\"id\":{id},\"ok\":{value}}}"),
            Err(msg) => format!(
                "{{\"id\":{id},\"err\":{}}}",
                serde_json::to_string(&msg).unwrap_or_else(|_| "\"unknown error\"".into())
            ),
        };
        let _ = socket_for_msg.send_with_str(&resp);
    }) as Box<dyn FnMut(MessageEvent)>);
    socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));

    // --- push pump: emit {event:changed,rev} when the registry advances -----
    let socket_for_push = socket.clone();
    let subscribed_push = subscribed.clone();
    let last_rev = Cell::new(robot_revision());
    let push_pump = runtime_shared::raf_loop(move || {
        if socket_for_push.ready_state() != WebSocket::OPEN || !subscribed_push.get() {
            return;
        }
        let rev = robot_revision();
        if rev != last_rev.get() {
            last_rev.set(rev);
            let _ = socket_for_push.send_with_str(&format!("{{\"event\":\"changed\",\"rev\":{rev}}}"));
        }
    });

    INSTALLED.with(|s| {
        *s.borrow_mut() = Some(RobotRelayState {
            _socket: socket,
            _on_open: on_open,
            _on_message: on_message,
            _push_pump: push_pump,
        });
    });
    Ok(())
}

// ===========================================================================
// Browser-side regression tests (new-core transport adapter). Run with:
//   cd crates/backend/web
//   wasm-pack test --headless --chrome -- --features new-core,robot
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    fn setup_mount() -> web_sys::Element {
        let document = web_sys::window().unwrap().document().unwrap();
        if let Some(prior) = document.get_element_by_id("app") {
            prior.remove();
        }
        let el = document.create_element("div").unwrap();
        el.set_id("app");
        document.body().unwrap().append_child(&el).unwrap();
        el
    }

    /// Regression (conformance-wave transport adapter): the relay verb
    /// loop resolves a NEW-core node end-to-end — `find_element` by
    /// test_id against the vocabulary registry, `click` runs the author
    /// callback, the settle commits the staged write, and the follow-up
    /// query reads the post-click reactive label THROUGH the driver
    /// env's `World::enter`. Fails if `dispatch_verb` routes a booted
    /// new-core app to the (empty) old registry, or if the driver env
    /// isn't installed (label read would panic / stay stale).
    #[wasm_bindgen_test]
    async fn regression_verb_loop_resolves_newcore_node_end_to_end() {
        let _mount = setup_mount();
        crate::newcore::start(|| {
            let count = runtime_world::signal(0i32);
            runtime_vocabulary::view()
                .child(
                    runtime_vocabulary::text()
                        .content(move || format!("n={}", count.get()))
                        .test_id("counter"),
                )
                .child(
                    runtime_vocabulary::button()
                        .label("inc")
                        .test_id("inc")
                        .on_press(move || count.update(|n| n + 1)),
                )
                .build()
        });

        // find_element resolves through the NEW registry.
        let found = dispatch_verb("find_element", &json!({"test_id": "inc"}))
            .expect("find_element");
        let parsed: serde_json::Value = serde_json::from_str(&found).unwrap();
        assert_eq!(parsed["kind"], "Button", "new-core node resolved: {found}");
        let id = parsed["id"].as_u64().expect("element id");

        // Reactive label BEFORE the click (world-entered read).
        let counter = dispatch_verb("find_element", &json!({"test_id": "counter"}))
            .expect("find counter");
        let counter: serde_json::Value = serde_json::from_str(&counter).unwrap();
        assert_eq!(counter["label"], "n=0");

        // click → author callback → staged write → settle (flush_sync)
        // → the very next query sees the committed value.
        let ok = dispatch_verb("click", &json!({"element_id": id})).expect("click");
        assert_eq!(ok, "\"ok\"");
        let counter = dispatch_verb("find_element", &json!({"test_id": "counter"}))
            .expect("find counter after click");
        let counter: serde_json::Value = serde_json::from_str(&counter).unwrap();
        assert_eq!(
            counter["label"], "n=1",
            "click settled synchronously and the entered query read the new label"
        );

        // The push pump keys on the NEW registry's revision when booted.
        assert!(robot_revision() > 0, "revision tracks new-core registrations");

        // Registry-independent verbs fall back to the old dispatch.
        assert!(dispatch_verb("ping", &json!({})).is_ok());

        // Drain the batched-text microtask before stop() so no stale
        // flush lands inside a later test's boot window (test hygiene —
        // same await the newcore boot tests do).
        let promise = js_sys::Promise::resolve(&wasm_bindgen::JsValue::UNDEFINED);
        let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
        crate::newcore::stop();
    }

    /// The P5-remainder verbs resolve against a booted new-core app
    /// through the SAME `dispatch_verb` routing (no transport edits):
    /// `list_components`/`invoke_method` drive a registered component
    /// method (the invoke settles via the driver env, so the follow-up
    /// label query reads the committed value), `read_signal`/
    /// `list_watched_signals` serve a `watch_signal` entry, and
    /// `list_navigators` answers (empty here) instead of erroring —
    /// pre-wave, all five returned named P5 errors.
    #[wasm_bindgen_test]
    async fn regression_method_watch_and_nav_verbs_resolve_on_newcore() {
        use std::rc::Rc;
        let _mount = setup_mount();
        crate::newcore::start(|| {
            let count = runtime_world::signal(0i32);
            runtime_vocabulary::robot::watch_signal("count", count);
            // The macro's emission shape by hand: register + keepalive
            // (the guard dies with the app's Owned on stop()).
            let count_in = count;
            let reg = runtime_vocabulary::glue::robot::register_component(
                "Bumper",
                vec![runtime_vocabulary::glue::robot::Method {
                    name: "bump_by",
                    args: &[("n", "i32")],
                    invoke: Rc::new(move |args| {
                        let n = args["n"].as_i64().ok_or("arg 'n': missing")? as i32;
                        count_in.set(count_in.get() + n);
                        Ok(())
                    }),
                }],
            );
            runtime_vocabulary::glue::__component_keepalive_effect(move || {
                let _ = &reg;
            });
            runtime_vocabulary::view()
                .child(
                    runtime_vocabulary::text()
                        .content(move || format!("n={}", count.get()))
                        .test_id("count"),
                )
                .build()
        });

        // list_components surfaces the instance + method schema.
        let list = dispatch_verb("list_components", &json!({})).expect("list_components");
        let v: serde_json::Value = serde_json::from_str(&list).unwrap();
        let comp = &v.as_array().unwrap()[0];
        assert_eq!(comp["name"], "Bumper");
        assert_eq!(comp["methods"][0]["name"], "bump_by");
        let instance = comp["instance_id"].as_u64().unwrap();

        // invoke_method runs the author closure and SETTLES — the next
        // query reads the post-invoke label.
        let ok = dispatch_verb(
            "invoke_method",
            &json!({ "instance_id": instance, "method": "bump_by", "args": { "n": 3 } }),
        )
        .expect("invoke_method");
        assert_eq!(ok, "\"ok\"");
        let el = dispatch_verb("find_element", &json!({ "test_id": "count" })).unwrap();
        let el: serde_json::Value = serde_json::from_str(&el).unwrap();
        assert_eq!(el["label"], "n=3", "invoke settled before the query");

        // Watch verbs read the live value through the entered env.
        assert_eq!(
            dispatch_verb("read_signal", &json!({ "name": "count" })).unwrap(),
            "\"3\""
        );
        let watched = dispatch_verb("list_watched_signals", &json!({})).unwrap();
        assert!(watched.contains("\"name\":\"count\""), "{watched}");

        // Nav verbs answer (no navigator mounted here → empty array),
        // rather than returning the pre-wave P5 error.
        assert_eq!(dispatch_verb("list_navigators", &json!({})).unwrap(), "[]");

        let promise = js_sys::Promise::resolve(&wasm_bindgen::JsValue::UNDEFINED);
        let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
        crate::newcore::stop();
        // The keepalive died with the world: the vocabulary registry is
        // empty. (Asserted on the registry directly — post-stop,
        // dispatch_verb routes to the old core again.)
        assert!(
            runtime_vocabulary::robot::list_components().is_empty(),
            "stop() drops the app Owned → keepalive → registration"
        );
    }
}
