//! Minimal TCP bridge for the robot module.
//!
//! Runs inside the app and exposes the Robot API over a simple
//! newline-delimited JSON protocol. No MCP knowledge, no tokio — just
//! `std::net` and `serde_json`.
//!
//! Wire protocol:
//!   request:  {"id":N, "cmd":"...", "args":{...}}
//!   response: {"id":N, "ok":...} or {"id":N, "err":"..."}

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::Duration;

use super::{Element, ElementId, ElementKind, Query, Robot, TreeNode};

/// Default port for the robot bridge.
pub const DEFAULT_PORT: u16 = 9718;

/// A pending command, with a oneshot reply channel for the result.
pub struct BridgeCommand {
    pub(crate) id: u64,
    pub(crate) cmd: String,
    pub(crate) args: serde_json::Value,
    pub(crate) reply: mpsc::Sender<String>,
}

/// Handle to the bridge's command channel. Poll this on the UI thread.
pub struct BridgeHandle {
    rx: mpsc::Receiver<BridgeCommand>,
}

impl BridgeHandle {
    /// Drain all pending commands and execute them via the Robot.
    /// Call on the UI thread (where the Robot registry lives).
    pub fn poll(&self) {
        let robot = Robot::new();
        while let Ok(cmd) = self.rx.try_recv() {
            let result = dispatch(&robot, &cmd.cmd, &cmd.args);
            let response = match result {
                Ok(value) => format!("{{\"id\":{},\"ok\":{}}}", cmd.id, value),
                Err(msg) => format!(
                    "{{\"id\":{},\"err\":{}}}",
                    cmd.id,
                    serde_json::to_string(&msg).unwrap_or_else(|_| "\"unknown error\"".into())
                ),
            };
            let _ = cmd.reply.send(response);
        }
    }
}

/// Start the robot bridge TCP listener on a background thread.
/// Returns a `BridgeHandle` to poll on the UI thread.
pub fn start(port: u16) -> BridgeHandle {
    let (tx, rx) = mpsc::channel();

    std::thread::spawn(move || {
        let listener = match TcpListener::bind(("0.0.0.0", port)) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[robot-bridge] failed to bind port {}: {}", port, e);
                return;
            }
        };
        eprintln!("[robot-bridge] listening on port {}", port);

        for stream in listener.incoming() {
            let stream = match stream {
                Ok(s) => s,
                Err(_) => continue,
            };
            let tx = tx.clone();
            std::thread::spawn(move || {
                handle_connection(stream, tx);
            });
        }
    });

    BridgeHandle { rx }
}

fn handle_connection(stream: TcpStream, tx: mpsc::Sender<BridgeCommand>) {
    let mut writer = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let reader = BufReader::new(stream);

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let parsed: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                let err = format!("{{\"id\":0,\"err\":\"parse error: {}\"}}\n", e);
                let _ = writer.write_all(err.as_bytes());
                let _ = writer.flush();
                continue;
            }
        };

        let id = parsed["id"].as_u64().unwrap_or(0);
        let cmd = parsed["cmd"].as_str().unwrap_or("").to_string();
        let args = parsed.get("args").cloned().unwrap_or(serde_json::Value::Null);

        let (reply_tx, reply_rx) = mpsc::channel();
        let command = BridgeCommand { id, cmd, args, reply: reply_tx };

        if tx.send(command).is_err() {
            break;
        }

        // Block waiting for the UI thread to execute and reply.
        // Timeout indicates the polling timer isn't running.
        match reply_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(response) => {
                let line = format!("{}\n", response);
                if writer.write_all(line.as_bytes()).is_err() {
                    break;
                }
                if writer.flush().is_err() {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let err = format!(
                    "{{\"id\":{},\"err\":\"timeout: UI thread polling not running\"}}\n",
                    id
                );
                let _ = writer.write_all(err.as_bytes());
                let _ = writer.flush();
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

// =============================================================================
// Command dispatch (runs on UI thread via poll)
// =============================================================================

fn dispatch(robot: &Robot, cmd: &str, args: &serde_json::Value) -> Result<String, String> {
    match cmd {
        "ping" => Ok("\"pong\"".into()),
        "find_element" => {
            let query = parse_query(args)?;
            match robot.find(query) {
                Some(el) => Ok(element_json(&el)),
                None => Ok("null".into()),
            }
        }
        "find_all_elements" => {
            let query = parse_query(args)?;
            let els: Vec<String> = robot.find_all(query).iter().map(element_json).collect();
            Ok(format!("[{}]", els.join(",")))
        }
        "click" => {
            let el = resolve_element(args)?;
            robot.click(&el).map_err(|e| e.to_string())?;
            Ok("\"ok\"".into())
        }
        "type_text" => {
            let el = resolve_element(args)?;
            let text = args["text"].as_str().ok_or("missing 'text' argument")?;
            robot.type_text(&el, text).map_err(|e| e.to_string())?;
            Ok("\"ok\"".into())
        }
        "set_toggle" => {
            let el = resolve_element(args)?;
            let value = args["value"].as_bool().ok_or("missing 'value' argument")?;
            robot.set_toggle(&el, value).map_err(|e| e.to_string())?;
            Ok("\"ok\"".into())
        }
        "set_slider" => {
            let el = resolve_element(args)?;
            let value = args["value"].as_f64().ok_or("missing 'value' argument")? as f32;
            robot.set_slider(&el, value).map_err(|e| e.to_string())?;
            Ok("\"ok\"".into())
        }
        "focus" => {
            let el = resolve_element(args)?;
            robot.focus(&el).map_err(|e| e.to_string())?;
            Ok("\"ok\"".into())
        }
        "blur" => {
            let el = resolve_element(args)?;
            robot.blur(&el).map_err(|e| e.to_string())?;
            Ok("\"ok\"".into())
        }
        "get_snapshot" => {
            let tree = robot.snapshot();
            let nodes: Vec<String> = tree.iter().map(tree_node_json).collect();
            Ok(format!("[{}]", nodes.join(",")))
        }
        "get_children" => {
            let el = resolve_element(args)?;
            let children: Vec<String> = robot.children_of(&el).iter().map(element_json).collect();
            Ok(format!("[{}]", children.join(",")))
        }
        "get_parent" => {
            let el = resolve_element(args)?;
            match robot.parent_of(&el) {
                Some(p) => Ok(element_json(&p)),
                None => Ok("null".into()),
            }
        }
        "count_elements" => {
            let kind = args["kind"].as_str().and_then(parse_element_kind);
            Ok(robot.count(kind).to_string())
        }
        "get_logs" => {
            // Either `since` (ms timestamp) for incremental polling
            // or `limit` (N most recent). `limit` defaults to 200 when
            // neither is given.
            let entries = if let Some(since) = args["since"].as_u64() {
                super::logs::since(since)
            } else {
                let limit = args["limit"].as_u64().unwrap_or(200) as usize;
                super::logs::recent(limit)
            };
            let rendered: Vec<String> = entries
                .iter()
                .map(|e| {
                    format!(
                        "{{\"ts\":{},\"source\":{},\"text\":{}}}",
                        e.timestamp_ms,
                        serde_json::to_string(&e.source)
                            .unwrap_or_else(|_| "\"\"".into()),
                        serde_json::to_string(&e.text)
                            .unwrap_or_else(|_| "\"\"".into()),
                    )
                })
                .collect();
            Ok(format!("[{}]", rendered.join(",")))
        }
        "clear_logs" => {
            super::logs::clear();
            Ok("\"ok\"".into())
        }
        "list_components" => {
            let snaps = super::list_components();
            let entries: Vec<String> = snaps
                .iter()
                .map(|s| {
                    let methods: Vec<String> = s
                        .methods
                        .iter()
                        .map(|(name, args)| {
                            let args_json: Vec<String> = args
                                .iter()
                                .map(|(arg_name, arg_type)| {
                                    format!(
                                        "{{\"name\":{},\"type\":{}}}",
                                        serde_json::to_string(arg_name).unwrap(),
                                        serde_json::to_string(arg_type).unwrap(),
                                    )
                                })
                                .collect();
                            format!(
                                "{{\"name\":{},\"args\":[{}]}}",
                                serde_json::to_string(name).unwrap(),
                                args_json.join(",")
                            )
                        })
                        .collect();
                    format!(
                        "{{\"instance_id\":{},\"name\":{},\"methods\":[{}]}}",
                        s.id.0,
                        serde_json::to_string(s.name).unwrap(),
                        methods.join(",")
                    )
                })
                .collect();
            Ok(format!("[{}]", entries.join(",")))
        }
        "get_frame" => {
            let el = resolve_element(args)?;
            match robot.frame(&el).map_err(|e| e.to_string())? {
                Some(r) => Ok(format!(
                    "{{\"x\":{},\"y\":{},\"width\":{},\"height\":{}}}",
                    r.x, r.y, r.width, r.height
                )),
                None => Ok("null".into()),
            }
        }
        "get_absolute_frame" => {
            let el = resolve_element(args)?;
            match robot.absolute_frame(&el).map_err(|e| e.to_string())? {
                Some(r) => Ok(format!(
                    "{{\"x\":{},\"y\":{},\"width\":{},\"height\":{}}}",
                    r.x, r.y, r.width, r.height
                )),
                None => Ok("null".into()),
            }
        }
        "invoke_method" => {
            let instance_id = args["instance_id"]
                .as_u64()
                .ok_or("missing 'instance_id' argument")? as u32;
            let method = args["method"]
                .as_str()
                .ok_or("missing 'method' argument")?;
            let method_args = args
                .get("args")
                .cloned()
                .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
            super::invoke_method(
                super::ComponentInstanceId(instance_id),
                method,
                &method_args,
            )?;
            Ok("\"ok\"".into())
        }
        _ => Err(format!("unknown command: {}", cmd)),
    }
}

// =============================================================================
// Helpers
// =============================================================================

fn parse_query(args: &serde_json::Value) -> Result<Query, String> {
    if let Some(id) = args["test_id"].as_str() {
        let leaked: &'static str = Box::leak(id.to_string().into_boxed_str());
        Ok(Query::TestId(leaked))
    } else if let Some(label) = args["label"].as_str() {
        Ok(Query::Label(label.to_string()))
    } else if let Some(sub) = args["label_contains"].as_str() {
        Ok(Query::LabelContains(sub.to_string()))
    } else if let Some(kind_str) = args["kind"].as_str() {
        match parse_element_kind(kind_str) {
            Some(k) => Ok(Query::Kind(k)),
            None => Err(format!("unknown element kind: {}", kind_str)),
        }
    } else {
        Ok(Query::All)
    }
}

fn parse_element_kind(s: &str) -> Option<ElementKind> {
    match s {
        "View" => Some(ElementKind::View),
        "Text" => Some(ElementKind::Text),
        "Button" => Some(ElementKind::Button),
        "Pressable" => Some(ElementKind::Pressable),
        "Image" => Some(ElementKind::Image),
        "Icon" => Some(ElementKind::Icon),
        "TextInput" => Some(ElementKind::TextInput),
        "Toggle" => Some(ElementKind::Toggle),
        "ScrollView" => Some(ElementKind::ScrollView),
        "Slider" => Some(ElementKind::Slider),
        "Video" => Some(ElementKind::Video),
        "ActivityIndicator" => Some(ElementKind::ActivityIndicator),
        "Virtualizer" => Some(ElementKind::Virtualizer),
        "Graphics" => Some(ElementKind::Graphics),
        "Navigator" => Some(ElementKind::Navigator),
        "TabNavigator" => Some(ElementKind::TabNavigator),
        "DrawerNavigator" => Some(ElementKind::DrawerNavigator),
        "Link" => Some(ElementKind::Link),
        "Overlay" => Some(ElementKind::Overlay),
        "Presence" => Some(ElementKind::Presence),
        _ => None,
    }
}

fn resolve_element(args: &serde_json::Value) -> Result<Element, String> {
    let id = args["element_id"]
        .as_u64()
        .ok_or("missing 'element_id' argument")?;
    Ok(Element {
        id: ElementId(id as u32),
        kind: ElementKind::View,
        test_id: None,
        label: None,
    })
}

fn element_json(el: &Element) -> String {
    format!(
        "{{\"id\":{},\"kind\":\"{:?}\",\"test_id\":{},\"label\":{}}}",
        el.id.0,
        el.kind,
        opt_str_json(el.test_id),
        opt_string_json(el.label.as_deref()),
    )
}

fn tree_node_json(node: &TreeNode) -> String {
    let children: Vec<String> = node.children.iter().map(tree_node_json).collect();
    format!(
        "{{\"id\":{},\"kind\":\"{:?}\",\"test_id\":{},\"label\":{},\"children\":[{}]}}",
        node.id.0,
        node.kind,
        opt_str_json(node.test_id),
        opt_string_json(node.label.as_deref()),
        children.join(","),
    )
}

fn opt_str_json(s: Option<&str>) -> String {
    match s {
        Some(v) => format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\"")),
        None => "null".into(),
    }
}

fn opt_string_json(s: Option<&str>) -> String {
    opt_str_json(s)
}
