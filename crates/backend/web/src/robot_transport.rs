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

/// Kept alive for the page lifetime so the socket + closures + push pump aren't
/// dropped (which would tear the connection down).
struct RobotRelayState {
    _socket: WebSocket,
    _on_open: Closure<dyn FnMut(JsValue)>,
    _on_message: Closure<dyn FnMut(MessageEvent)>,
    _push_pump: runtime_core::scheduling::RafLoop,
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

        // Same wrapping the native `BridgeHandle::poll` does.
        let resp = match runtime_core::robot::bridge::invoke_command(cmd, &args) {
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
    let last_rev = Cell::new(runtime_core::robot::current_revision());
    let push_pump = runtime_core::raf_loop(move || {
        if socket_for_push.ready_state() != WebSocket::OPEN || !subscribed_push.get() {
            return;
        }
        let rev = runtime_core::robot::current_revision();
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
