//! Robot MCP Server — the host-side MCP server that connects to
//! running app instances via the robot bridge protocol.
//!
//! This binary is what you give to an MCP client (Claude Desktop, etc.)
//! as its `command`. It:
//! 1. Implements the full MCP protocol (initialize, tools/list, tools/call)
//!    over stdio (newline-delimited JSON-RPC).
//! 2. Connects to one or more running apps via TCP (the robot bridge).
//! 3. Translates MCP tool calls into bridge commands and returns results.
//!
//! Usage:
//!   robot-mcp-proxy [--host HOST] [--port PORT]
//!
//! Defaults: host=127.0.0.1, port=9718
//!
//! Claude Desktop config:
//! ```json
//! {
//!   "mcpServers": {
//!     "my-app": {
//!       "command": "robot-mcp-proxy",
//!       "args": ["--port", "9718"]
//!     }
//!   }
//! }
//! ```

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};

const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 9718;

static BRIDGE_ID: AtomicU64 = AtomicU64::new(1);

fn main() {
    let (host, port) = parse_args();
    let addr = format!("{}:{}", host, port);

    // Start with no connection — connect lazily on first tool call so
    // Claude Code can load this MCP server even when the app isn't
    // running yet. Reconnects automatically after disconnect.
    let bridge: Mutex<Option<BridgeConn>> = Mutex::new(None);
    eprintln!("robot-mcp-proxy: target app at {} (lazy connect)", addr);

    // MCP stdio loop.
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let reader = BufReader::new(stdin.lock());

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                let resp = json_rpc_error(None, -32700, &format!("parse error: {}", e));
                write_line(&mut stdout, &resp);
                continue;
            }
        };

        let id = request.get("id").cloned();
        let method = request["method"].as_str().unwrap_or("");

        let response = match method {
            "initialize" => Some(json_rpc_success(id, json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "idealyst-robot", "version": "0.1.0" }
            }))),

            "notifications/initialized" => None,

            "tools/list" => Some(json_rpc_success(id, json!({
                "tools": tool_definitions()
            }))),

            "tools/call" => {
                let name = request["params"]["name"].as_str().unwrap_or("");
                let args = request["params"].get("arguments")
                    .cloned()
                    .unwrap_or(json!({}));

                let result = call_bridge(&bridge, &addr, name, &args);
                Some(json_rpc_success(id, result))
            }

            "ping" => Some(json_rpc_success(id, json!({}))),

            _ => {
                if id.is_some() {
                    Some(json_rpc_error(id, -32601, &format!("method not found: {}", method)))
                } else {
                    None
                }
            }
        };

        if let Some(resp) = response {
            write_line(&mut stdout, &resp);
        }
    }
}

// =============================================================================
// Bridge communication
// =============================================================================

struct BridgeConn {
    writer: TcpStream,
    reader: BufReader<TcpStream>,
}

impl BridgeConn {
    fn connect(addr: &str) -> std::io::Result<Self> {
        let stream = TcpStream::connect(addr)?;
        // Short read timeout so reconnect detection is snappy on dead sockets.
        // Long enough for normal commands (5s).
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        Ok(Self {
            writer: stream.try_clone()?,
            reader: BufReader::new(stream),
        })
    }
}

fn call_bridge(bridge: &Mutex<Option<BridgeConn>>, addr: &str, cmd: &str, args: &Value) -> Value {
    let id = BRIDGE_ID.fetch_add(1, Ordering::Relaxed);
    let request = json!({ "id": id, "cmd": cmd, "args": args });
    let line = serde_json::to_string(&request).unwrap();

    // Try twice — once with the existing connection (if any), once after
    // forcing a reconnect. This handles the case where the app was
    // restarted between MCP calls.
    for attempt in 0..2 {
        let mut guard = bridge.lock().unwrap();

        // Ensure we have a live connection.
        if guard.is_none() {
            match BridgeConn::connect(addr) {
                Ok(conn) => {
                    eprintln!("robot-mcp-proxy: connected to {}", addr);
                    *guard = Some(conn);
                }
                Err(e) => {
                    return tool_error(&format!(
                        "could not connect to app at {}: {} (is the app running?)",
                        addr, e
                    ));
                }
            }
        }

        let conn = guard.as_mut().unwrap();

        // Send command.
        if writeln!(conn.writer, "{}", line).is_err() || conn.writer.flush().is_err() {
            // Connection dead — drop it and retry once.
            *guard = None;
            if attempt == 0 { continue; }
            return tool_error("bridge write failed after reconnect");
        }

        // Read response.
        let mut response_line = String::new();
        if conn.reader.read_line(&mut response_line).is_err() || response_line.is_empty() {
            *guard = None;
            if attempt == 0 { continue; }
            return tool_error("bridge read failed after reconnect");
        }

        let parsed: Value = match serde_json::from_str(response_line.trim()) {
            Ok(v) => v,
            Err(e) => return tool_error(&format!("bridge response parse error: {}", e)),
        };

        if let Some(ok) = parsed.get("ok") {
            return tool_result(&serde_json::to_string_pretty(ok).unwrap_or_else(|_| ok.to_string()));
        } else if let Some(err) = parsed.get("err") {
            return tool_error(err.as_str().unwrap_or("unknown error"));
        } else {
            return tool_error("unexpected bridge response format");
        }
    }

    tool_error("bridge unreachable")
}

// =============================================================================
// MCP tool definitions
// =============================================================================

fn tool_definitions() -> Value {
    json!([
        {
            "name": "find_element",
            "description": "Find a UI element by test_id, label, label substring, or kind. Returns the first match.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "test_id": { "type": "string", "description": "Find by test ID (exact)" },
                    "label": { "type": "string", "description": "Find by label (exact)" },
                    "label_contains": { "type": "string", "description": "Find by label (substring)" },
                    "kind": { "type": "string", "description": "Element kind: View, Text, Button, Pressable, TextInput, Toggle, Slider, etc." }
                }
            }
        },
        {
            "name": "find_all_elements",
            "description": "Find all UI elements matching criteria.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "test_id": { "type": "string" },
                    "label": { "type": "string" },
                    "label_contains": { "type": "string" },
                    "kind": { "type": "string" }
                }
            }
        },
        {
            "name": "click",
            "description": "Click/press a button or pressable element.",
            "inputSchema": {
                "type": "object",
                "properties": { "element_id": { "type": "integer" } },
                "required": ["element_id"]
            }
        },
        {
            "name": "type_text",
            "description": "Type text into a TextInput (replaces current value).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "element_id": { "type": "integer" },
                    "text": { "type": "string" }
                },
                "required": ["element_id", "text"]
            }
        },
        {
            "name": "set_toggle",
            "description": "Set a Toggle's value.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "element_id": { "type": "integer" },
                    "value": { "type": "boolean" }
                },
                "required": ["element_id", "value"]
            }
        },
        {
            "name": "set_slider",
            "description": "Set a Slider's value.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "element_id": { "type": "integer" },
                    "value": { "type": "number" }
                },
                "required": ["element_id", "value"]
            }
        },
        {
            "name": "focus",
            "description": "Focus a TextInput element.",
            "inputSchema": {
                "type": "object",
                "properties": { "element_id": { "type": "integer" } },
                "required": ["element_id"]
            }
        },
        {
            "name": "blur",
            "description": "Remove focus from an element.",
            "inputSchema": {
                "type": "object",
                "properties": { "element_id": { "type": "integer" } },
                "required": ["element_id"]
            }
        },
        {
            "name": "get_snapshot",
            "description": "Get the full component hierarchy tree with IDs, kinds, labels, and children.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "get_children",
            "description": "Get direct children of an element.",
            "inputSchema": {
                "type": "object",
                "properties": { "element_id": { "type": "integer" } },
                "required": ["element_id"]
            }
        },
        {
            "name": "get_parent",
            "description": "Get the parent of an element.",
            "inputSchema": {
                "type": "object",
                "properties": { "element_id": { "type": "integer" } },
                "required": ["element_id"]
            }
        },
        {
            "name": "count_elements",
            "description": "Count mounted elements, optionally by kind.",
            "inputSchema": {
                "type": "object",
                "properties": { "kind": { "type": "string" } }
            }
        },
        {
            "name": "get_logs",
            "description": "Fetch captured log entries from the running app. Captures both framework/backend logs and anything written to stdout/stderr (NSLog mirrors, Rust eprintln, etc.). Each entry has {ts, source, text}: ts = unix ms, source = 'stdout' | 'stderr' | a label, text = the line. Pass `since` (a previously-returned `ts`) to poll for new entries, or `limit` (default 200) for the N most recent.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "since": { "type": "integer", "description": "Unix ms timestamp; returns entries newer than this" },
                    "limit": { "type": "integer", "description": "Cap on entries returned (used when `since` is omitted)" }
                }
            }
        },
        {
            "name": "clear_logs",
            "description": "Drop all captured log entries. Useful before reproducing an issue so the buffer only contains relevant lines.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "get_frame",
            "description": "Read the element's bounding rect in its PARENT's coordinate system. Returns {x, y, width, height} in pixels, or null if the element exists but hasn't been laid out yet. Use this to ask 'how is X positioned relative to its container'.",
            "inputSchema": {
                "type": "object",
                "properties": { "element_id": { "type": "integer" } },
                "required": ["element_id"]
            }
        },
        {
            "name": "get_absolute_frame",
            "description": "Read the element's bounding rect in VIEWPORT (window) coordinates. Returns {x, y, width, height} in pixels, or null if the element isn't mounted in a window yet. Use this for 'where is X on screen'.",
            "inputSchema": {
                "type": "object",
                "properties": { "element_id": { "type": "integer" } },
                "required": ["element_id"]
            }
        },
        {
            "name": "list_components",
            "description": "List every mounted #[component] instance that declared a methods! block. Each entry has an instance_id, the component's fn name, and its methods. Each method's args is a list of {name, type} objects where 'type' is the Rust source-form type (i32, String, Vec<u8>, custom serde structs, etc.) — use it to pick the right JSON shape when calling invoke_method.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "invoke_method",
            "description": "Invoke a methods!-declared function on a mounted component instance. Pass instance_id from list_components, the method name, and an args object keyed by parameter name. The args must JSON-deserialize into the parameter types reported by list_components — e.g. for fn set_to(&self, n: i32) pass {\"n\": 42}. Returns 'ok' on success or a typed error.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "instance_id": { "type": "integer" },
                    "method": { "type": "string" },
                    "args": { "type": "object", "description": "Method args keyed by parameter name. Omit or pass {} for no-arg methods." }
                },
                "required": ["instance_id", "method"]
            }
        }
    ])
}

// =============================================================================
// JSON-RPC helpers
// =============================================================================

fn json_rpc_success(id: Option<Value>, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn json_rpc_error(id: Option<Value>, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn tool_result(text: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": text }] })
}

fn tool_error(message: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": message }], "isError": true })
}

fn write_line(out: &mut impl Write, value: &Value) {
    let s = serde_json::to_string(value).unwrap();
    let _ = writeln!(out, "{}", s);
    let _ = out.flush();
}

// =============================================================================
// Arg parsing
// =============================================================================

fn parse_args() -> (String, u16) {
    let args: Vec<String> = std::env::args().collect();
    let mut host = DEFAULT_HOST.to_string();
    let mut port = DEFAULT_PORT;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--host" | "-h" => {
                i += 1;
                if i < args.len() { host = args[i].clone(); }
            }
            "--port" | "-p" => {
                i += 1;
                if i < args.len() { port = args[i].parse().unwrap_or(DEFAULT_PORT); }
            }
            "--help" => {
                eprintln!("Usage: robot-mcp-proxy [--host HOST] [--port PORT]");
                eprintln!();
                eprintln!("MCP server that connects to a running app's robot bridge.");
                eprintln!();
                eprintln!("Options:");
                eprintln!("  --host, -h   App host (default: 127.0.0.1)");
                eprintln!("  --port, -p   App port (default: 9718)");
                std::process::exit(0);
            }
            _ => {}
        }
        i += 1;
    }

    (host, port)
}
