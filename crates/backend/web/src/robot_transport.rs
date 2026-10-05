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
use web_glue::{JsCast, JsValue};
use web_glue::dom::{MessageEvent, WebSocket};

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

/// The page-origin path the dev servers splice to the relay
/// (`dev_http::ROBOT_RELAY_URL`, `server::dev_stream::RELAY_PATH`).
pub const SAME_ORIGIN_RELAY_PATH: &str = "/__idealyst/relay";

/// Wait before redialing after a connected session drops (the relay or the
/// same-origin proxy restarting).
const REDIAL_MS: i32 = 500;
/// Cap of the backoff between rounds in which no candidate answered.
const RETRY_MAX_MS: i32 = 5_000;

/// The URLs to dial, in order: the page's own origin first, then the
/// relay URL the dev server injected.
///
/// The injected URL is the relay's own listener, `ws://127.0.0.1:<port>`
/// on the machine running `idealyst dev`. A browser elsewhere — on the
/// host of a devcontainer, where only the app's port is forwarded — cannot
/// reach it, and the robot was silently unplugged ("no app connected to
/// the relay") while the app ran. Both dev servers also splice the relay
/// at [`SAME_ORIGIN_RELAY_PATH`] on the page's origin, the one address a
/// browser that loaded the page can always reach; the injected URL stays
/// as the fallback for a page served by something that does not (a server
/// not built on the framework's router, a hand-served bundle).
///
/// Only `http:`/`https:` pages have an origin to dial (`ws:`/`wss:`, the
/// page's scheme decides, so an https page never dials plain ws).
pub(crate) fn relay_candidates(page_protocol: &str, page_host: &str, injected: &str) -> Vec<String> {
    let scheme = match page_protocol {
        "http:" => Some("ws"),
        "https:" => Some("wss"),
        _ => None,
    };
    let mut out = Vec::with_capacity(2);
    if let (Some(scheme), false) = (scheme, page_host.is_empty()) {
        out.push(format!("{scheme}://{page_host}{SAME_ORIGIN_RELAY_PATH}"));
    }
    if !injected.is_empty() && !out.iter().any(|u| u == injected) {
        out.push(injected.to_string());
    }
    out
}

/// The connection state: which candidate is being tried, the live socket
/// and its listeners, and the pending redial timer.
struct Dialer {
    candidates: Vec<String>,
    socket: Option<WebSocket>,
    listeners: Vec<web_glue::dom::Listener>,
    /// Pending redial (the timer's closure lives here until it fires or is
    /// replaced; a closure dropping itself mid-call is safe in web-glue).
    timer: Option<web_glue::Closure>,
    backoff_ms: i32,
    /// Whether the relay asked this connection to push change events.
    subscribed: Rc<Cell<bool>>,
}

/// Kept alive for the page lifetime so the dialer (socket + closures) and
/// the push pump aren't dropped (which would tear the connection down).
struct RobotRelayState {
    _dialer: Rc<RefCell<Dialer>>,
    _push_pump: runtime_shared::scheduling::RafLoop,
}

thread_local! {
    static INSTALLED: RefCell<Option<RobotRelayState>> = const { RefCell::new(None) };
}

/// Connect this web app's Robot bridge to the dev session's relay.
/// `url` is the relay URL the dev server injected (e.g.
/// `ws://127.0.0.1:9719`); the page's own origin is tried first — see
/// [`relay_candidates`]. A dropped connection is redialed, so the app
/// survives the relay or the app server restarting. Idempotent per page.
/// Called from the generated web wrapper when the build enabled robot and
/// the dev server injected a relay URL.
pub fn install_robot_relay_client(url: &str) -> Result<(), JsValue> {
    if INSTALLED.with(|s| s.borrow().is_some()) {
        return Ok(());
    }
    let (protocol, host) = web_glue::dom::window()
        .map(|w| {
            let loc = w.location();
            (loc.protocol().unwrap_or_default(), loc.host().unwrap_or_default())
        })
        .unwrap_or_default();
    let candidates = relay_candidates(&protocol, &host, url);
    if candidates.is_empty() {
        return Err(JsValue::from_str("no robot relay URL to dial"));
    }

    let subscribed = Rc::new(Cell::new(false));
    let dialer = Rc::new(RefCell::new(Dialer {
        candidates,
        socket: None,
        listeners: Vec::new(),
        timer: None,
        backoff_ms: REDIAL_MS,
        subscribed: subscribed.clone(),
    }));
    dial(&dialer, 0);

    // --- push pump: emit {event:changed,rev} when the registry advances -----
    let dialer_for_push = Rc::downgrade(&dialer);
    let last_rev = Cell::new(robot_revision());
    let push_pump = runtime_shared::raf_loop(move || {
        if !subscribed.get() {
            return;
        }
        let Some(dialer) = dialer_for_push.upgrade() else { return };
        let dialer = dialer.borrow();
        let Some(socket) = dialer.socket.as_ref().filter(|s| s.ready_state() == WebSocket::OPEN) else {
            return;
        };
        let rev = robot_revision();
        if rev != last_rev.get() {
            last_rev.set(rev);
            let _ = socket.send_with_str(&format!("{{\"event\":\"changed\",\"rev\":{rev}}}"));
        }
    });

    INSTALLED.with(|s| {
        *s.borrow_mut() = Some(RobotRelayState { _dialer: dialer, _push_pump: push_pump });
    });
    Ok(())
}

/// Redial candidate `idx` after `ms`. Always through a timer, so a socket's
/// own `close` handler never tears down its listeners while running.
fn schedule(dialer: &Rc<RefCell<Dialer>>, idx: usize, ms: i32) {
    let Some(window) = web_glue::dom::window() else { return };
    let weak = Rc::downgrade(dialer);
    let closure = web_glue::Closure::once(move |_| {
        if let Some(d) = weak.upgrade() {
            dial(&d, idx);
        }
    });
    window.set_timeout(&closure, ms);
    dialer.borrow_mut().timer = Some(closure);
}

/// The candidate after `idx` failed to connect: the next one now, or —
/// when every candidate failed this round — the first again after a
/// backoff that doubles up to [`RETRY_MAX_MS`].
fn after_failure(dialer: &Rc<RefCell<Dialer>>, idx: usize) {
    let (count, backoff) = {
        let d = dialer.borrow();
        (d.candidates.len(), d.backoff_ms)
    };
    if idx + 1 < count {
        schedule(dialer, idx + 1, 0);
    } else {
        dialer.borrow_mut().backoff_ms = (backoff * 2).min(RETRY_MAX_MS);
        schedule(dialer, 0, backoff);
    }
}

/// Open a socket to candidate `idx`, replacing any previous one.
fn dial(dialer: &Rc<RefCell<Dialer>>, idx: usize) {
    let (url, subscribed) = {
        let mut d = dialer.borrow_mut();
        d.listeners.clear();
        if let Some(old) = d.socket.take() {
            let _ = old.close();
        }
        d.subscribed.set(false);
        (d.candidates[idx].clone(), d.subscribed.clone())
    };
    // A malformed URL throws synchronously; treat it as a failed attempt.
    let socket = match WebSocket::new(&url) {
        Ok(s) => s,
        Err(_) => return after_failure(dialer, idx),
    };
    let opened = Rc::new(Cell::new(false));

    // --- on_open: announce identity -----------------------------------------
    let socket_for_open = socket.clone();
    let opened_for_open = opened.clone();
    let weak = Rc::downgrade(dialer);
    let on_open = crate::glue_dom::listen(&socket, "open", Default::default(), move |_evt| {
        opened_for_open.set(true);
        if let Some(d) = weak.upgrade() {
            d.borrow_mut().backoff_ms = REDIAL_MS;
        }
        let hello = serde_json::json!({
            "hello": { "name": env!("CARGO_PKG_NAME"), "platform": "web" }
        });
        let _ = socket_for_open.send_with_str(&hello.to_string());
    });

    // --- on_close: redial ---------------------------------------------------
    // A handshake that fails (the origin has no relay route: a 404, a
    // catch-all's index.html) also ends here, with `open` never fired.
    let weak = Rc::downgrade(dialer);
    let on_close = crate::glue_dom::listen(&socket, "close", Default::default(), move |_evt| {
        let Some(d) = weak.upgrade() else { return };
        if opened.get() {
            schedule(&d, 0, REDIAL_MS);
        } else {
            after_failure(&d, idx);
        }
    });

    // --- on_message: dispatch forwarded verbs -------------------------------
    let socket_for_msg = socket.clone();
    let on_message = crate::glue_dom::listen(&socket, "message", Default::default(), move |evt| {
        let evt: MessageEvent = evt.unchecked_into();
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
            subscribed.set(true);
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
    });

    let mut d = dialer.borrow_mut();
    d.socket = Some(socket);
    d.listeners = vec![on_open, on_close, on_message];
}

#[cfg(test)]
mod candidate_tests {
    //! Pure URL selection — no DOM. `#[wasm_bindgen_test]`, not `#[test]`:
    //! backend-web only builds for wasm32, where the test runner skips plain
    //! `#[test]` functions.
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    /// Regression (CrewForge, `idealyst dev --web --local` in a
    /// devcontainer): the page dialed only the injected
    /// `ws://127.0.0.1:<port>`, the container's loopback, which the host
    /// browser cannot reach. The page's own origin comes first now.
    #[wasm_bindgen_test]
    fn regression_same_origin_relay_is_dialed_before_the_loopback_url() {
        assert_eq!(
            relay_candidates("http:", "localhost:3100", "ws://127.0.0.1:35109"),
            vec!["ws://localhost:3100/__idealyst/relay".to_string(), "ws://127.0.0.1:35109".to_string()]
        );
    }

    #[wasm_bindgen_test]
    fn an_https_page_dials_wss_on_its_origin() {
        assert_eq!(
            relay_candidates("https:", "app.example.dev", "ws://127.0.0.1:1")[0],
            "wss://app.example.dev/__idealyst/relay"
        );
    }

    #[wasm_bindgen_test]
    fn a_page_with_no_http_origin_dials_only_the_injected_url() {
        assert_eq!(relay_candidates("file:", "", "ws://127.0.0.1:1"), vec!["ws://127.0.0.1:1".to_string()]);
        assert_eq!(relay_candidates("http:", "", "ws://127.0.0.1:1"), vec!["ws://127.0.0.1:1".to_string()]);
    }

    #[wasm_bindgen_test]
    fn an_injected_url_equal_to_the_origin_route_is_not_dialed_twice() {
        assert_eq!(
            relay_candidates("http:", "h:1", "ws://h:1/__idealyst/relay"),
            vec!["ws://h:1/__idealyst/relay".to_string()]
        );
    }
}

// ===========================================================================
// Browser-side regression tests (new-core transport adapter). Run with
// plain cargo, not wasm-pack (see the `tests.rs` module docs):
//   cd crates/backend/web
//   CHROMEDRIVER=<path> cargo test --target wasm32-unknown-unknown --features robot --lib
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    fn setup_mount() -> web_glue::dom::Element {
        let document = web_glue::dom::window().unwrap().document().unwrap();
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
        let promise = web_glue::js::Promise::resolve(&web_glue::JsValue::UNDEFINED);
        let _ = web_glue::JsFuture::new(&promise).await;
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

        let promise = web_glue::js::Promise::resolve(&web_glue::JsValue::UNDEFINED);
        let _ = web_glue::JsFuture::new(&promise).await;
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
