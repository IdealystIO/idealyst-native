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
//! app → relay   {"hello":{"platform":"web","label":"Chrome 131"}}   once, on open
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

/// What the page knows about its own Robot connection, published for the
/// dev overlay's badge ([`publish_status`]).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RelayStatus {
    /// Dialing `url`, with no failure to report yet.
    Connecting { url: String },
    /// The relay accepted the connection.
    Connected { url: String },
    /// Every candidate failed this round; the next round starts in
    /// `retry_ms`. `error` says what each attempt got.
    Retrying { error: String, retry_ms: i32 },
    /// A connected session closed; it is redialed.
    Dropped { url: String, error: String },
    /// There is nothing to dial; the client does not run.
    Failed { error: String },
}

impl RelayStatus {
    fn to_json(&self) -> serde_json::Value {
        use serde_json::json;
        match self {
            RelayStatus::Connecting { url } => json!({ "state": "connecting", "url": url }),
            RelayStatus::Connected { url } => json!({ "state": "connected", "url": url }),
            RelayStatus::Retrying { error, retry_ms } => {
                json!({ "state": "retrying", "error": error, "retry_ms": retry_ms })
            }
            RelayStatus::Dropped { url, error } => {
                json!({ "state": "dropped", "url": url, "error": error })
            }
            RelayStatus::Failed { error } => json!({ "state": "failed", "error": error }),
        }
    }
}

/// The global holding the latest status (a JSON string), for an overlay
/// that mounts after the client started. `dev-http`'s reload script reads
/// both names; its `status_overlay` test holds them to these.
pub const ROBOT_STATUS_GLOBAL: &str = "__idealyst_dev_robot";
/// The function the dev overlay defines to be told each change.
pub const ROBOT_STATUS_HOOK: &str = "__idealyst_dev_robot_status";

/// Tell the page's dev overlay (`dev-http`'s `status_overlay.js`) how the
/// Robot connection is doing. Without it the only sign of a page that could
/// not reach the relay was the MCP's "no app connected to the relay" —
/// nothing on the page, nothing in the console. A page with no overlay
/// (a production-like serve) just carries the global.
fn publish_status(status: &RelayStatus) {
    let json = JsValue::from_str(&status.to_json().to_string());
    let global = JsValue::global();
    let _ = global.set(ROBOT_STATUS_GLOBAL, &json);
    if let Ok(hook) = global.get(ROBOT_STATUS_HOOK) {
        if hook.is_function() {
            let _ = hook.call(&global, &[&json]);
        }
    }
}

/// `Chrome 131`, `Firefox 133`, `Safari 18`, `Edge 131` from a user agent —
/// what tells two tabs apart in the dev panel's robot column. `None` for a
/// browser it doesn't recognise. Order matters: Edge's agent also names
/// Chrome and Safari, Chrome's also names Safari.
pub(crate) fn browser_label(user_agent: &str) -> Option<String> {
    let version = |marker: &str| -> Option<String> {
        let rest = &user_agent[user_agent.find(marker)? + marker.len()..];
        let major: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        (!major.is_empty()).then_some(major)
    };
    for (marker, name) in [("Edg/", "Edge"), ("Firefox/", "Firefox"), ("Chrome/", "Chrome")] {
        if let Some(v) = version(marker) {
            return Some(format!("{name} {v}"));
        }
    }
    if user_agent.contains("Safari/") {
        return version("Version/").map(|v| format!("Safari {v}"));
    }
    None
}

fn user_agent() -> String {
    JsValue::global()
        .get("navigator")
        .and_then(|n| n.get("userAgent"))
        .ok()
        .and_then(|ua| ua.as_string())
        .unwrap_or_default()
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
    /// What each candidate got this round (`url (close 1006)`), reported
    /// once the round has failed.
    failures: Vec<String>,
    /// Whether a failure is on show: a retry round then keeps showing it
    /// rather than flickering back to "connecting".
    failing: bool,
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
        let error = "no robot relay URL to dial";
        publish_status(&RelayStatus::Failed { error: error.into() });
        return Err(JsValue::from_str(error));
    }

    let subscribed = Rc::new(Cell::new(false));
    let dialer = Rc::new(RefCell::new(Dialer {
        candidates,
        socket: None,
        listeners: Vec::new(),
        timer: None,
        backoff_ms: REDIAL_MS,
        subscribed: subscribed.clone(),
        failures: Vec::new(),
        failing: false,
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
fn after_failure(dialer: &Rc<RefCell<Dialer>>, idx: usize, why: String) {
    let (count, backoff) = {
        let mut d = dialer.borrow_mut();
        let url = d.candidates[idx].clone();
        d.failures.push(format!("{url} ({why})"));
        (d.candidates.len(), d.backoff_ms)
    };
    if idx + 1 < count {
        schedule(dialer, idx + 1, 0);
    } else {
        let error = {
            let mut d = dialer.borrow_mut();
            d.backoff_ms = (backoff * 2).min(RETRY_MAX_MS);
            d.failing = true;
            format!("can't reach the robot relay: {}", std::mem::take(&mut d.failures).join(", "))
        };
        publish_status(&RelayStatus::Retrying { error, retry_ms: backoff });
        schedule(dialer, 0, backoff);
    }
}

/// `close 1006`, plus the reason when the close carried one. A browser
/// says nothing more about a failed WebSocket (no HTTP status, no network
/// error) — deliberately, so a page can't port-scan — so the code and the
/// URL are what there is to show.
fn close_description(evt: &JsValue) -> String {
    let code = evt.get("code").ok().and_then(|c| c.as_f64()).map(|c| c as u32);
    let reason = evt.get("reason").ok().and_then(|r| r.as_string()).filter(|r| !r.is_empty());
    match (code, reason) {
        (Some(code), Some(reason)) => format!("close {code}: {reason}"),
        (Some(code), None) => format!("close {code}"),
        (None, _) => "closed".into(),
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
        if idx == 0 {
            d.failures.clear();
        }
        (d.candidates[idx].clone(), d.subscribed.clone())
    };
    if !dialer.borrow().failing {
        publish_status(&RelayStatus::Connecting { url: url.clone() });
    }
    // A malformed URL throws synchronously; treat it as a failed attempt.
    let socket = match WebSocket::new(&url) {
        Ok(s) => s,
        Err(_) => return after_failure(dialer, idx, "not a valid WebSocket URL".into()),
    };
    let opened = Rc::new(Cell::new(false));

    // --- on_open: announce identity -----------------------------------------
    let socket_for_open = socket.clone();
    let opened_for_open = opened.clone();
    let weak = Rc::downgrade(dialer);
    let url_for_open = url.clone();
    let on_open = crate::glue_dom::listen(&socket, "open", Default::default(), move |_evt| {
        opened_for_open.set(true);
        if let Some(d) = weak.upgrade() {
            let mut d = d.borrow_mut();
            d.backoff_ms = REDIAL_MS;
            d.failing = false;
            d.failures.clear();
        }
        // No `name`: this crate would say `backend-web` (it once sent
        // `env!("CARGO_PKG_NAME")`); the relay knows the app's name from
        // the dev session. `label` tells this tab from another.
        let mut hello = serde_json::json!({ "hello": { "platform": "web" } });
        if let Some(label) = browser_label(&user_agent()) {
            hello["hello"]["label"] = label.into();
        }
        let _ = socket_for_open.send_with_str(&hello.to_string());
        publish_status(&RelayStatus::Connected { url: url_for_open.clone() });
    });

    // --- on_close: redial ---------------------------------------------------
    // A handshake that fails (the origin has no relay route: a 404, a
    // catch-all's index.html) also ends here, with `open` never fired.
    let weak = Rc::downgrade(dialer);
    let url_for_close = url.clone();
    let on_close = crate::glue_dom::listen(&socket, "close", Default::default(), move |evt| {
        let Some(d) = weak.upgrade() else { return };
        let why = close_description(evt.as_ref());
        if opened.get() {
            d.borrow_mut().failing = true;
            publish_status(&RelayStatus::Dropped { url: url_for_close.clone(), error: why });
            schedule(&d, 0, REDIAL_MS);
        } else {
            after_failure(&d, idx, why);
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

    /// The dev panel's robot column tells two tabs apart by this label.
    /// Edge's agent names Chrome and Safari too, Chrome's names Safari:
    /// the most specific browser wins.
    #[wasm_bindgen_test]
    fn the_browser_label_names_the_browser_and_its_major_version() {
        let chrome = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
        let edge = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.2903.70";
        let safari = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Safari/605.1.15";
        let firefox = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:133.0) Gecko/20100101 Firefox/133.0";
        assert_eq!(browser_label(chrome).as_deref(), Some("Chrome 131"));
        assert_eq!(browser_label(edge).as_deref(), Some("Edge 131"));
        assert_eq!(browser_label(safari).as_deref(), Some("Safari 18"));
        assert_eq!(browser_label(firefox).as_deref(), Some("Firefox 133"));
        assert_eq!(browser_label("curl/8.4.0"), None);
    }

    /// What the overlay reads: a `state` it switches on, and the URL or
    /// error it shows.
    #[wasm_bindgen_test]
    fn the_status_the_overlay_reads() {
        assert_eq!(
            RelayStatus::Retrying { error: "can't reach the robot relay: ws://h/__idealyst/relay (close 1006)".into(), retry_ms: 1000 }
                .to_json(),
            serde_json::json!({
                "state": "retrying",
                "error": "can't reach the robot relay: ws://h/__idealyst/relay (close 1006)",
                "retry_ms": 1000,
            })
        );
        assert_eq!(
            RelayStatus::Connected { url: "ws://h/__idealyst/relay".into() }.to_json(),
            serde_json::json!({ "state": "connected", "url": "ws://h/__idealyst/relay" })
        );
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

    /// Await a real macrotask boundary (`setTimeout(ms)`).
    async fn sleep_ms(ms: i32) {
        let promise = web_glue::js::Promise::new(&mut |resolve, _reject| {
            web_glue::dom::window()
                .unwrap()
                .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms)
                .unwrap();
        });
        let _ = web_glue::JsFuture::new(&promise).await;
    }

    /// Regression: a page that could not reach the Robot relay said so
    /// nowhere — no console line, nothing on the page — so the only sign
    /// was the MCP answering "no app connected to the relay". The client
    /// now publishes each state, and the dev overlay's hook hears it. The
    /// test page's origin has no relay route and port 1 has no relay, so
    /// both candidates fail and the round's error names them.
    #[wasm_bindgen_test]
    async fn regression_an_unreachable_relay_is_reported_to_the_page() {
        let heard: Rc<RefCell<Vec<String>>> = Rc::default();
        let sink = heard.clone();
        let hook = web_glue::Closure::new(move |status: JsValue| {
            sink.borrow_mut().push(status.as_string().unwrap_or_default());
        });
        JsValue::global().set(ROBOT_STATUS_HOOK, hook.as_js()).unwrap();

        install_robot_relay_client("ws://127.0.0.1:1").unwrap();
        let mut retrying = None;
        for _ in 0..100 {
            retrying = heard
                .borrow()
                .iter()
                .map(|s| serde_json::from_str::<serde_json::Value>(s).unwrap())
                .find(|v| v["state"] == "retrying");
            if retrying.is_some() {
                break;
            }
            sleep_ms(50).await;
        }
        let retrying = retrying.expect("a failed round is reported");
        let error = retrying["error"].as_str().unwrap();
        assert!(error.contains("/__idealyst/relay ("), "the origin route is named: {error}");
        assert!(error.contains("ws://127.0.0.1:1 ("), "the injected URL is named: {error}");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&heard.borrow()[0]).unwrap()["state"],
            "connecting",
            "the first dial says it is connecting"
        );
        let latest = JsValue::global().get(ROBOT_STATUS_GLOBAL).unwrap().as_string().unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&latest).unwrap()["state"],
            "retrying",
            "an overlay that mounts later reads the latest from the global"
        );
        let _ = JsValue::global().set(ROBOT_STATUS_HOOK, &JsValue::UNDEFINED);
        drop(hook);
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
