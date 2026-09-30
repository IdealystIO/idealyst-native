//! The Inspector's wire protocol.
//!
//! The Inspector is split in two. The **server** (`inspector-server`,
//! hosted by `idealyst inspect`) owns everything about the target apps:
//! it discovers them, holds the one robot-bridge connection per app,
//! polls and accumulates their state, and runs the actions. The **front
//! end** (`examples/inspector`, in a browser or as a desktop app) renders
//! what the server pushes and sends back what the user did. This crate
//! is everything the two agree on.
//!
//! ## Transport
//!
//! One WebSocket per front end, at [`WS_PATH`] on the server's HTTP port
//! ([`DEFAULT_PORT`] unless `idealyst inspect --port` says otherwise).
//! Each text frame is one JSON-encoded [`ClientMsg`] or [`ServerMsg`].
//!
//! ```text
//! server → client  {"type":"hello","protocol":1}              once, on open
//! server → client  {"type":"apps","apps":[…]}                 on open, then whenever the set changes
//! client → server  {"type":"attach","app":"Todo-48213"}       inspect a discovered app
//! server → client  {"type":"snapshot","app":"Todo-48213","snapshot":{…}}   whenever its state changes
//! client → server  {"type":"focus","component":7,"signal":null}
//! client → server  {"type":"action","label":"navigate push","cmd":"navigate","args":{…}}
//! client → server  {"type":"highlight","element":42}         box element 42 in the app (null clears)
//! ```
//!
//! A front end never talks to an app. Everything it learns arrives in a
//! `snapshot`, and each `action` is run by the server on the app's
//! connection, with its outcome coming back as the next snapshot's
//! `last_action`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod model;

pub use model::*;

/// The server's port when `idealyst inspect` is given none. One above the
/// robot bridge's default (9718).
pub const DEFAULT_PORT: u16 = 9719;

/// The path of the Inspector WebSocket on the server's HTTP port.
pub const WS_PATH: &str = "/ws";

/// The path a launcher probes to tell an Inspector server from some other
/// process on the port. Answers `200` with [`HEALTH_BODY`].
pub const HEALTH_PATH: &str = "/health";

/// The body [`HEALTH_PATH`] answers with.
pub const HEALTH_BODY: &str = "idealyst-inspector";

/// Bumped on any incompatible change to the messages below. The server
/// announces it in [`ServerMsg::Hello`]; a front end built against a
/// different version says so instead of misreading frames.
pub const PROTOCOL_VERSION: u32 = 1;

/// One app the server found running on this machine: a robot bridge, or
/// the `idealyst dev` relay an app dialed, registered under
/// `~/.idealyst/apps`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AppInfo {
    /// The registration's file stem (`Todo-48213`). Unique per running
    /// app; what [`ClientMsg::Attach`] names.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub bundle_id: Option<String>,
    /// The bridge's TCP port on 127.0.0.1.
    pub port: u16,
    /// The registering process: the app itself, or the `idealyst dev`
    /// process hosting its relay.
    pub pid: u32,
    /// `macos`, `web`, `ios`, … when the registration says.
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub project_root: Option<String>,
}

impl AppInfo {
    /// The bridge address, `127.0.0.1:<port>`.
    pub fn addr(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }
}

/// What the per-item verbs fetch for one front end: the component and
/// signal its detail panes show. Everything else in a snapshot is shared
/// by every front end attached to the same app.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Focus {
    #[serde(default)]
    pub component: Option<u64>,
    #[serde(default)]
    pub signal: Option<u64>,
}

/// Front end → server.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    /// Inspect the discovered app with this [`AppInfo::id`]. Replaces any
    /// current attachment.
    Attach { app: String },
    /// Inspect whatever bridge answers at `addr` (`127.0.0.1:53817`), for
    /// a bridge that registered nowhere. Replaces any current attachment.
    AttachAddr { addr: String },
    /// Stop inspecting. The server drops its app connection once no front
    /// end is attached to it.
    Detach,
    /// Change what the per-item verbs fetch.
    Focus {
        #[serde(default)]
        component: Option<u64>,
        #[serde(default)]
        signal: Option<u64>,
    },
    /// Run a bridge verb on the attached app. Its outcome arrives as the
    /// next snapshot's `last_action`, under `label`.
    Action { label: String, cmd: String, args: Value },
    /// Rescan for running apps now instead of on the server's cadence.
    Rescan,
    /// Draw a box over this element in the running app (`None` clears
    /// it) — hover-to-highlight. Unlike an `action` it records no
    /// `last_action` and forces no refresh: it fires on every hover.
    Highlight {
        #[serde(default)]
        element: Option<u64>,
    },
}

/// Server → front end.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    /// First frame on every connection.
    Hello { protocol: u32 },
    /// Every app currently running, name-sorted.
    Apps { apps: Vec<AppInfo> },
    /// The attached app's state, for this front end's focus. `app` echoes
    /// what the front end attached with (an [`AppInfo::id`], or the
    /// address given to [`ClientMsg::AttachAddr`]), so a frame already in
    /// flight when the front end switched apps is recognisably stale.
    Snapshot { app: String, snapshot: Box<Snapshot> },
}

impl ClientMsg {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("ClientMsg serializes")
    }
}

impl ServerMsg {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("ServerMsg serializes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip_client(msg: ClientMsg) {
        let back: ClientMsg = serde_json::from_str(&msg.to_json()).unwrap();
        assert_eq!(back, msg);
    }

    #[test]
    fn client_messages_round_trip() {
        round_trip_client(ClientMsg::Attach { app: "Todo-1".into() });
        round_trip_client(ClientMsg::AttachAddr { addr: "127.0.0.1:1".into() });
        round_trip_client(ClientMsg::Detach);
        round_trip_client(ClientMsg::Focus { component: Some(3), signal: None });
        round_trip_client(ClientMsg::Action {
            label: "navigate push".into(),
            cmd: "navigate".into(),
            args: serde_json::json!({ "path": "/a" }),
        });
        round_trip_client(ClientMsg::Rescan);
        round_trip_client(ClientMsg::Highlight { element: Some(42) });
        round_trip_client(ClientMsg::Highlight { element: None });
    }

    #[test]
    fn wire_shape_is_the_documented_one() {
        assert_eq!(ClientMsg::Attach { app: "Todo-48213".into() }.to_json(), r#"{"type":"attach","app":"Todo-48213"}"#);
        assert_eq!(ServerMsg::Hello { protocol: 1 }.to_json(), r#"{"type":"hello","protocol":1}"#);
    }

    /// A full snapshot — every enum arm the server produces — survives the
    /// trip to the front end intact.
    #[test]
    fn snapshot_round_trips() {
        let snap = Snapshot {
            status: Status::Down("no app connected to the relay".into()),
            tree: vec![ElementNode {
                id: 1,
                kind: "View".into(),
                test_id: Some("root".into()),
                label: None,
                components: vec![ComponentRef { instance_id: 4, name: "App".into() }],
                children: vec![],
            }],
            component_count: 1,
            element: Some(ElementDetail {
                element_id: 1,
                frame: Some(Rect { x: 0.0, y: 0.0, width: 10.0, height: 20.0 }),
                native: None,
            }),
            perf: Perf::Unavailable("build with debug-stats".into()),
            last_action: Some(ActionResult { label: "clear logs".into(), result: Err("nope".into()), rtt_ms: 3 }),
            ..Snapshot::default()
        };
        let json = ServerMsg::Snapshot { app: "Todo-1".into(), snapshot: Box::new(snap.clone()) }.to_json();
        let ServerMsg::Snapshot { snapshot, .. } = serde_json::from_str(&json).unwrap() else { panic!("{json}") };
        assert_eq!(*snapshot, snap);
        let live = Snapshot { status: Status::Live { rtt_ms: 9 }, ..Snapshot::default() };
        let back: Snapshot = serde_json::from_str(&serde_json::to_string(&live).unwrap()).unwrap();
        assert_eq!(back, live);
    }

    /// The bridge's own reply shapes still parse (the server reads them).
    #[test]
    fn parses_the_bridge_shapes() {
        let nav: Navigator = serde_json::from_str(
            r#"{"nav_id":0,"element_id":3,"type_name":"stack_navigator","active_route":"c","active_path":"/c?step=2&x","depth":2,"can_go_back":true,"is_current":true,"base":"","stack":[{"route":"h","path":"/"}],"controllable":true}"#,
        )
        .unwrap();
        assert_eq!(nav.kind_label(), "Stack");
        assert_eq!(nav.query(), [("step".to_string(), "2".to_string()), ("x".to_string(), String::new())]);
        let v: NativeValue = serde_json::from_str(r#"{"type":"color","value":[1.0,0.5,0.0,1.0]}"#).unwrap();
        assert_eq!(v.display(), "#ff8000");
    }
}
