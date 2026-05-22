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

/// Project-identifying metadata captured by [`set_app_identity`].
/// The bridge writes these into the mDNS TXT record so the MCP
/// server can route Robot calls by app — without env-var plumbing.
#[derive(Debug, Clone, Default)]
pub struct AppIdentity {
    /// Short package name (e.g. `mcp_test_app`). Falls back to
    /// `env!("CARGO_PKG_NAME")` of the framework when unset, which
    /// is wrong — callers should always set this explicitly.
    pub name: String,
    /// Reverse-DNS bundle id (e.g. `com.example.mcp_test_app`).
    /// Optional; only the iOS / Android backends need this for
    /// platform-side identification.
    pub bundle_id: Option<String>,
    /// Absolute project root path. Lets the MCP server cross-reference
    /// with the user-level registry / catalog binary path.
    pub project_root: Option<String>,
}

thread_local! {
    /// Per-process project identity for the bridge advertisement.
    /// Set via [`set_app_identity`] before `mount()`; the bridge
    /// reads from here when binding + advertising.
    static APP_IDENTITY: std::cell::RefCell<Option<AppIdentity>> =
        const { std::cell::RefCell::new(None) };

    /// Stashed bridge handle from [`start_auto_polling`]. Held so it
    /// outlives `mount()` returning; dropped only when the process
    /// exits. The TCP listener thread doesn't observe this drop, so
    /// the bridge keeps accepting connections for the program's life.
    static AUTO_POLLED_BRIDGE: std::cell::RefCell<Option<BridgeHandle>> =
        const { std::cell::RefCell::new(None) };

    /// mDNS advertiser kept alive for the process lifetime. Dropping
    /// it unregisters the service from the local-network browse.
    #[cfg(not(target_arch = "wasm32"))]
    static MDNS_ADVERTISER: std::cell::RefCell<Option<MdnsAdvertiser>> =
        const { std::cell::RefCell::new(None) };

    /// The currently-scheduled poll task. Held to keep the
    /// `after_ms` handle alive — dropping it would cancel the pending
    /// callback before it fires. Replaced on each reschedule.
    static POLL_TASK: std::cell::RefCell<Option<crate::scheduling::ScheduledTask>> =
        const { std::cell::RefCell::new(None) };
}

/// Register this app's identity for the bridge advertisement.
/// Idiomatic call site: a wrapper template's entry point (web's
/// `#[wasm_bindgen(start)]`, iOS's `ios_main`, Android's
/// `Java_..._attach`, macOS's `main`) calls this BEFORE invoking
/// `framework_core::mount(...)`.
///
/// The bridge reads `(name, bundle_id, project_root)` from here to
/// populate the mDNS service instance name and TXT record. Setting
/// the identity is cheap — a clone into a `thread_local`. Calling
/// twice replaces the previous value; subsequent `mount()` calls
/// pick up the new identity. For the bridge specifically, only the
/// value at the time of `start_auto_polling` matters.
///
/// Authors writing their own `app()` without going through the CLI
/// can call this directly:
///
/// ```ignore
/// framework_core::robot::bridge::set_app_identity(
///     framework_core::robot::bridge::AppIdentity {
///         name: "my-app".to_string(),
///         bundle_id: Some("com.example.my_app".to_string()),
///         project_root: None,
///     },
/// );
/// ```
pub fn set_app_identity(identity: AppIdentity) {
    APP_IDENTITY.with(|slot| {
        *slot.borrow_mut() = Some(identity);
    });
}

/// Read the currently-registered identity. Returns a default
/// `AppIdentity { name: "app", bundle_id: None, project_root: None }`
/// when none has been registered — keeps the bridge advertisement
/// running with a generic name rather than silently disabling
/// itself.
fn current_identity() -> AppIdentity {
    APP_IDENTITY.with(|slot| {
        slot.borrow()
            .clone()
            .unwrap_or_else(|| AppIdentity {
                name: std::env::var("IDEALYST_APP_NAME")
                    .ok()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "app".to_string()),
                bundle_id: std::env::var("IDEALYST_BUNDLE_ID").ok().filter(|s| !s.is_empty()),
                project_root: std::env::var("IDEALYST_PROJECT_ROOT").ok().filter(|s| !s.is_empty()),
            })
    })
}

/// mDNS advertiser handle — held in a thread-local so the daemon
/// outlives `mount()`. Dropping it unregisters the service.
#[cfg(not(target_arch = "wasm32"))]
struct MdnsAdvertiser {
    #[allow(dead_code)]
    daemon: mdns_sd::ServiceDaemon,
    #[allow(dead_code)]
    fullname: String,
}

/// Service type for the Robot bridge mDNS advertisement.
/// `_idealyst-robot._tcp.local.` — generic so the MCP server browses
/// one thing; per-app routing is via the TXT record's `app` key.
#[cfg(not(target_arch = "wasm32"))]
pub const MDNS_SERVICE_TYPE: &str = "_idealyst-robot._tcp.local.";

/// Advertise the bound bridge port via mDNS. Best-effort: any
/// failure (daemon init, registration error) is logged and the
/// bridge keeps running — the registry-file fallback still works
/// on hosts where mDNS is unavailable (corporate VPN, etc.).
#[cfg(not(target_arch = "wasm32"))]
fn advertise_mdns(port: u16, identity: &AppIdentity) {
    let pid = std::process::id();
    // DNS-SD labels can't contain `.` or whitespace. The bundle id is
    // reverse-DNS so we strip it to hyphens; the package name is
    // already snake_case (CARGO_PKG_NAME conventions).
    let name_label = identity.name.replace('.', "-").replace(' ', "-");
    let instance_name = format!("{}-{}", name_label, pid);
    let hostname = format!("idealyst-{}-{}.local.", name_label, pid);

    let pid_s = pid.to_string();
    let port_s = port.to_string();
    let proto = "1".to_string();
    let bundle_id_s = identity.bundle_id.clone().unwrap_or_default();
    let project_root_s = identity.project_root.clone().unwrap_or_default();
    let catalog_bin_s = std::env::var("IDEALYST_CATALOG_BIN").unwrap_or_default();

    let txt: [(&str, &str); 6] = [
        ("app", identity.name.as_str()),
        ("bundle_id", bundle_id_s.as_str()),
        ("project_root", project_root_s.as_str()),
        ("catalog_bin", catalog_bin_s.as_str()),
        ("pid", pid_s.as_str()),
        ("proto", proto.as_str()),
    ];
    // Note: TXT records have a per-record size cap (~255 bytes is
    // safe). Long `project_root` paths on iOS/Android sandboxes
    // could overshoot; if that becomes a problem in the wild, drop
    // `project_root` from the record and have the MCP server ask
    // the bridge via the `get_identity` command instead.

    let daemon = match mdns_sd::ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("[robot-bridge] mDNS daemon init failed: {} — discovery limited to registry fallback", e);
            return;
        }
    };
    let info = match mdns_sd::ServiceInfo::new(
        MDNS_SERVICE_TYPE,
        &instance_name,
        &hostname,
        "",
        port,
        &txt[..],
    ) {
        Ok(i) => i.enable_addr_auto(),
        Err(e) => {
            eprintln!("[robot-bridge] mDNS ServiceInfo build failed: {}", e);
            return;
        }
    };
    let fullname = info.get_fullname().to_string();
    if let Err(e) = daemon.register(info) {
        eprintln!("[robot-bridge] mDNS register failed: {}", e);
        return;
    }
    eprintln!(
        "[robot-bridge] advertised via mDNS as {} (port {}, app={}, port_s={})",
        fullname, port, identity.name, port_s
    );
    MDNS_ADVERTISER.with(|slot| {
        *slot.borrow_mut() = Some(MdnsAdvertiser { daemon, fullname });
    });
}

/// Auto-polling variant of [`start`]: spawns the TCP listener AND
/// schedules a periodic `poll()` on the UI thread via the framework
/// scheduler. Caller doesn't need to thread a `BridgeHandle` through
/// their own tick loop — the bridge self-drives until the process
/// exits.
///
/// Port-selection rules (in order of precedence):
/// 1. `IDEALYST_BRIDGE_PORT` env var if set and non-zero → bind that
///    explicit port. Conflict (already in use) → log a warning and
///    fall back to ephemeral.
/// 2. `default_port` arg if non-zero → bind it. Conflict → ephemeral.
/// 3. Else → ephemeral (bind port 0; OS picks).
///
/// When `IDEALYST_BRIDGE_PORT_FILE` is set, the bridge writes a JSON
/// document `{port, project_root, pid}` to that path after binding.
/// `mcp-server` reads this file to discover the bridge and verifies
/// `project_root` matches its own cwd — so a Claude session in
/// project A can't accidentally drive project B's app.
///
/// Requires the platform scheduler to be installed (typically done
/// by the host before `mount()` — `backend_ios::install_scheduler`,
/// `backend_web::install_scheduler`, etc.).
///
/// Calling twice is idempotent: subsequent calls bail out early
/// rather than binding a second listener.
pub fn start_auto_polling(default_port: u16) {
    AUTO_POLLED_BRIDGE.with(|slot| {
        if slot.borrow().is_some() {
            return;
        }
        let chosen_port = resolve_requested_port(default_port);
        let (handle, bound_port) = match start_on_port(chosen_port) {
            Ok(pair) => pair,
            Err(e) if chosen_port != 0 => {
                // Specific port wanted but unavailable. Surface a
                // clear warning so simultaneous-app collisions are
                // diagnosable, then fall back to ephemeral so the
                // app still gets a bridge.
                eprintln!(
                    "[robot-bridge] WARN: requested port {} unavailable ({}); \
                     falling back to ephemeral. Another idealyst app is likely \
                     already running on that port.",
                    chosen_port, e
                );
                match start_on_port(0) {
                    Ok(pair) => pair,
                    Err(e2) => {
                        eprintln!("[robot-bridge] could not bind any port: {}", e2);
                        return;
                    }
                }
            }
            Err(e) => {
                eprintln!("[robot-bridge] could not bind any port: {}", e);
                return;
            }
        };
        write_port_file(bound_port);
        // mDNS advertisement — non-wasm only. Failure here is
        // non-fatal (logged inside `advertise_mdns`); the registry
        // file fallback still wires up discovery for the MCP server.
        #[cfg(not(target_arch = "wasm32"))]
        {
            let identity = current_identity();
            advertise_mdns(bound_port, &identity);
        }
        *slot.borrow_mut() = Some(handle);
    });
    schedule_periodic_poll();
}

fn resolve_requested_port(default_port: u16) -> u16 {
    std::env::var("IDEALYST_BRIDGE_PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .filter(|p| *p != 0)
        .unwrap_or(default_port)
}

/// Publish the bound port. Two channels, used in different ways:
///
/// 1. **`IDEALYST_BRIDGE_PORT_FILE`** (legacy / debug): writes
///    `{port, project_root, pid}` to that path. Per-project. Useful
///    when poking the bridge directly from a shell without going
///    through the MCP server.
/// 2. **User-level registry** (`~/.idealyst/registry.json`): adds a
///    full `AppEntry` so `idealyst mcp` (a single instance per
///    Claude Code session) can route Robot calls across multiple
///    simultaneously-running apps.
///
/// Both happen on every bind. Both are best-effort — write failures
/// are logged but never panic, since the running app is the source
/// of truth for the bridge regardless.
fn write_port_file(port: u16) {
    write_legacy_per_project_file(port);
    write_registry_entry(port);
}

fn write_legacy_per_project_file(port: u16) {
    let Ok(path_str) = std::env::var("IDEALYST_BRIDGE_PORT_FILE") else {
        return;
    };
    let path = std::path::PathBuf::from(&path_str);
    let project_root = path
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let pid = std::process::id();
    let body = format!(
        "{{\n  \"port\": {},\n  \"project_root\": {},\n  \"pid\": {}\n}}\n",
        port,
        serde_json::to_string(&project_root).unwrap_or_else(|_| "\"\"".into()),
        pid,
    );
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&path, body) {
        eprintln!(
            "[robot-bridge] could not write port file {}: {}",
            path.display(),
            e
        );
    }
}

/// Register this app in the user-level `~/.idealyst/registry.json`
/// so the MCP server can find it. Requires the `mcp` feature
/// (already on when `dev` is — `dev = ["robot", "mcp"]`).
#[cfg(feature = "mcp")]
fn write_registry_entry(port: u16) {
    use crate::__mcp::registry::{now_secs, update_with, AppEntry};

    let identity = current_identity();
    let project_root = identity
        .project_root
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(resolve_project_root);
    let name = if !identity.name.is_empty() {
        identity.name.clone()
    } else {
        std::path::PathBuf::from(&project_root)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "app".to_string())
    };
    let catalog_bin = std::env::var("IDEALYST_CATALOG_BIN")
        .ok()
        .filter(|s| !s.is_empty());

    let entry = AppEntry {
        name,
        project_root: project_root.clone(),
        bridge_addr: format!("127.0.0.1:{}", port),
        catalog_bin,
        pid: std::process::id(),
        registered_at: now_secs(),
    };
    if let Err(e) = update_with(|reg| reg.register(entry)) {
        eprintln!(
            "[robot-bridge] could not write registry: {}",
            e
        );
    }
}

#[cfg(not(feature = "mcp"))]
fn write_registry_entry(_port: u16) {
    // No MCP — single-app mode. Per-project port file
    // (`write_legacy_per_project_file`) is the only discovery path.
}

/// Best-effort recovery of the project root. Prefer the explicit
/// `IDEALYST_PROJECT_ROOT` env var the dev launcher sets; fall back
/// to the legacy port-file location (its grandparent is the project
/// root); finally cwd as last resort.
#[cfg(feature = "mcp")]
fn resolve_project_root() -> String {
    if let Ok(r) = std::env::var("IDEALYST_PROJECT_ROOT") {
        if !r.is_empty() {
            return r;
        }
    }
    if let Ok(p) = std::env::var("IDEALYST_BRIDGE_PORT_FILE") {
        let path = std::path::PathBuf::from(&p);
        if let Some(root) = path.parent().and_then(|p| p.parent()) {
            return root.to_string_lossy().into_owned();
        }
    }
    std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// 16ms ≈ 60Hz — fast enough that interactive MCP calls feel
/// snappy without burning CPU when the queue is empty.
const POLL_INTERVAL_MS: i32 = 16;

fn schedule_periodic_poll() {
    // Without an installed scheduler `after_ms` runs the closure
    // synchronously on native (test-host fallback path), which would
    // re-enter `schedule_periodic_poll()` immediately and overflow the
    // stack. Bail when there's no real scheduler — tests don't need
    // the bridge to poll, and a properly-hosted app installs one
    // before reaching this point.
    if !crate::scheduling::is_scheduler_installed() {
        return;
    }
    let task = crate::scheduling::after_ms(POLL_INTERVAL_MS, || {
        AUTO_POLLED_BRIDGE.with(|slot| {
            if let Some(h) = slot.borrow().as_ref() {
                h.poll();
            }
        });
        // Re-arm. If the bridge slot got cleared (e.g. process is
        // tearing down), the closure becomes a no-op but we still
        // reschedule. Cheap.
        schedule_periodic_poll();
    });
    // Hold the task so its `Drop` doesn't cancel before the
    // callback fires. The previous task (if any) was already
    // consumed by the running callback, so replacing the slot is
    // safe — there's no pending-cancel race.
    POLL_TASK.with(|slot| {
        *slot.borrow_mut() = Some(task);
    });
}

/// Start the robot bridge TCP listener on a background thread.
/// Returns a `BridgeHandle` to poll on the UI thread. Bind failures
/// are logged but don't panic — the returned handle just never sees
/// commands, so calling `poll()` is a no-op.
///
/// Kept for back-compat (`dev-server::transport::serve_with_robot_bridge`
/// is a caller). New code wanting to know the actually-bound port —
/// notably the ephemeral case — should use [`start_on_port`].
pub fn start(port: u16) -> BridgeHandle {
    match start_on_port(port) {
        Ok((handle, _bound)) => handle,
        Err(e) => {
            eprintln!("[robot-bridge] failed to bind port {}: {}", port, e);
            // Return a dead handle whose `poll()` immediately drains
            // an empty channel — caller code is undisturbed.
            let (_tx, rx) = mpsc::channel::<BridgeCommand>();
            BridgeHandle { rx }
        }
    }
}

/// Bind a TCP listener on `port` (use `0` for ephemeral), spawn the
/// accept loop, and return the handle + the *actually-bound* port.
/// Differs from [`start`] only in error handling: caller gets the
/// `io::Error` to react to (e.g. fall back to ephemeral on conflict).
pub fn start_on_port(port: u16) -> std::io::Result<(BridgeHandle, u16)> {
    let listener = TcpListener::bind(("0.0.0.0", port))?;
    let bound_port = listener.local_addr()?.port();
    let (tx, rx) = mpsc::channel();

    std::thread::spawn(move || {
        eprintln!("[robot-bridge] listening on port {}", bound_port);
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

    Ok((BridgeHandle { rx }, bound_port))
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
        "get_identity" => {
            // Hand back what the app advertised via mDNS so the MCP
            // server can verify it's talking to the right project.
            let id = current_identity();
            let bundle = id
                .bundle_id
                .as_deref()
                .and_then(|s| serde_json::to_string(s).ok())
                .unwrap_or_else(|| "null".into());
            let root = id
                .project_root
                .as_deref()
                .and_then(|s| serde_json::to_string(s).ok())
                .unwrap_or_else(|| "null".into());
            Ok(format!(
                "{{\"name\":{},\"bundle_id\":{},\"project_root\":{},\"pid\":{}}}",
                serde_json::to_string(&id.name).unwrap_or_else(|_| "\"app\"".into()),
                bundle,
                root,
                std::process::id(),
            ))
        }
        #[cfg(feature = "mcp")]
        "get_catalog" => {
            // Serve the in-process catalog JSON over the bridge.
            // Removes the need for the MCP server to spawn a
            // separate `--emit-catalog` extractor binary in the
            // common case where the app is already running.
            let json = crate::__mcp::catalog_json();
            Ok(json.to_string())
        }
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
