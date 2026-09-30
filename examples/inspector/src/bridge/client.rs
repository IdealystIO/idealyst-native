//! The Inspector's one connection: a WebSocket to the Inspector server
//! (`idealyst inspect`), speaking `inspector-protocol`.
//!
//! It runs on the UI thread as an async task (`net::WebSocket`: the
//! browser's own socket on web, an I/O thread bridged into the
//! framework's scheduler on desktop), so every inbound frame lands
//! straight in the signals the screens read. There's no polling and no
//! locks. The server decides when anything changes.
//!
//! The server keeps no memory of a front end across connections, so the
//! client remembers what it asked for (the attached app, the focus) and
//! replays it whenever the socket reconnects.

use std::cell::RefCell;

use inspector_protocol::{ClientMsg, ServerMsg, PROTOCOL_VERSION};
use net::{WebSocket, WsMessage, WsSender};
use runtime_core::Signal;
use serde_json::Value;

pub use inspector_protocol::Focus;

use super::model::{AppInfo, Snapshot, Status};

/// Backoff before reconnecting to the server.
const RECONNECT_MS: i32 = 1000;

/// The connection to the Inspector server itself (the attached app's
/// connection is `Snapshot::status`).
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ServerLink {
    #[default]
    Connecting,
    Connected,
    /// Unreachable; the client keeps retrying.
    Down(String),
    /// The server speaks another protocol version.
    Incompatible { server: u32 },
}

/// Where inbound frames land.
#[derive(Clone, Copy)]
pub struct Sinks {
    pub link: Signal<ServerLink>,
    pub apps: Signal<Vec<AppInfo>>,
    pub snapshot: Signal<Snapshot>,
}

#[derive(Default)]
struct State {
    sender: Option<WsSender>,
    /// The attach to replay on reconnect, and the key snapshot frames
    /// for it carry.
    attached: Option<(ClientMsg, String)>,
    focus: Focus,
    /// The element last sent as highlighted, so a hover that stays on
    /// one row sends one message.
    highlighted: Option<u64>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// Connect to `url` and keep reconnecting for the life of the app.
pub fn start(url: String, sinks: Sinks) {
    runtime_core::driver::spawn_async(run(url, sinks));
}

async fn run(url: String, sinks: Sinks) {
    match WebSocket::connect(&url).await {
        Ok(mut ws) => {
            let sender = ws.sender();
            let (attach, focus) = STATE.with(|s| {
                let mut s = s.borrow_mut();
                s.sender = Some(sender.clone());
                // A new connection starts with no box showing.
                s.highlighted = None;
                (s.attached.as_ref().map(|(m, _)| m.clone()), s.focus)
            });
            if let Some(attach) = attach {
                send_on(&sender, &focus_msg(focus));
                send_on(&sender, &attach);
            }
            sinks.link.set(ServerLink::Connected);
            while let Some(Ok(frame)) = ws.recv().await {
                let text = match frame {
                    WsMessage::Text(t) => t,
                    WsMessage::Binary(b) => String::from_utf8_lossy(&b).into_owned(),
                };
                if let Ok(msg) = serde_json::from_str::<ServerMsg>(&text) {
                    receive(msg, sinks);
                }
            }
            STATE.with(|s| s.borrow_mut().sender = None);
            let reason = format!("lost the Inspector server at {url}");
            sinks.link.set(ServerLink::Down(reason.clone()));
            if STATE.with(|s| s.borrow().attached.is_some()) {
                let mut snap = sinks.snapshot.get();
                snap.status = Status::Down(reason);
                sinks.snapshot.set(snap);
            }
        }
        Err(e) => sinks.link.set(ServerLink::Down(format!("can't reach the Inspector server at {url}: {e}"))),
    }
    runtime_core::after_ms_detached(RECONNECT_MS, move || start(url, sinks));
}

/// Apply one server frame.
pub fn receive(msg: ServerMsg, sinks: Sinks) {
    match msg {
        ServerMsg::Hello { protocol } if protocol != PROTOCOL_VERSION => {
            sinks.link.set(ServerLink::Incompatible { server: protocol });
        }
        ServerMsg::Hello { .. } => {}
        ServerMsg::Apps { apps } => sinks.apps.set(apps),
        ServerMsg::Snapshot { app, snapshot } => {
            // A frame already in flight when the user switched apps (or
            // disconnected) belongs to the previous app.
            let current = STATE.with(|s| s.borrow().attached.as_ref().is_some_and(|(_, key)| *key == app));
            if current {
                sinks.snapshot.set(*snapshot);
            }
        }
    }
}

fn send_on(sender: &WsSender, msg: &ClientMsg) {
    let _ = sender.send(WsMessage::Text(msg.to_json()));
}

fn send(msg: &ClientMsg) {
    STATE.with(|s| {
        if let Some(sender) = &s.borrow().sender {
            send_on(sender, msg);
        }
    });
}

fn focus_msg(focus: Focus) -> ClientMsg {
    ClientMsg::Focus { component: focus.component, signal: focus.signal }
}

/// Inspect a discovered app (by [`AppInfo::id`]).
pub fn attach(app_id: String) {
    let msg = ClientMsg::Attach { app: app_id.clone() };
    STATE.with(|s| s.borrow_mut().attached = Some((msg.clone(), app_id)));
    send(&msg);
}

/// Inspect the bridge at `addr`, which registered nowhere.
pub fn attach_addr(addr: String) {
    let msg = ClientMsg::AttachAddr { addr: addr.clone() };
    STATE.with(|s| s.borrow_mut().attached = Some((msg.clone(), addr)));
    send(&msg);
}

pub fn detach() {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.attached = None;
        s.focus = Focus::default();
        // The server clears the app's box when this front end detaches.
        s.highlighted = None;
    });
    send(&ClientMsg::Detach);
}

/// Change what the per-item verbs fetch.
pub fn set_focus(focus: Focus) {
    let changed = STATE.with(|s| std::mem::replace(&mut s.borrow_mut().focus, focus) != focus);
    if changed {
        send(&focus_msg(focus));
    }
}

/// Run an action verb on the attached app; its outcome arrives as the
/// next snapshot's `last_action`.
pub fn action(label: impl Into<String>, cmd: &str, args: Value) {
    send(&ClientMsg::Action { label: label.into(), cmd: cmd.to_string(), args });
}

/// Box `element` in the running app (`None` clears it): hover-to-highlight.
/// Sends only when the highlighted element changes.
pub fn highlight(element: Option<u64>) {
    let changed = STATE.with(|s| std::mem::replace(&mut s.borrow_mut().highlighted, element) != element);
    if changed {
        send(&ClientMsg::Highlight { element });
    }
}

pub fn rescan() {
    send(&ClientMsg::Rescan);
}
