//! Minimal TCP bridge for the robot module.
//!
//! Runs inside the app and exposes the Robot API over a simple
//! newline-delimited JSON protocol. No MCP knowledge, no tokio — just
//! `std::net` and `serde_json`.
//!
//! Wire protocol:
//!   request:  {"id":N, "cmd":"...", "args":{...}}
//!   response: {"id":N, "ok":...} or {"id":N, "err":"..."}
//!
//! Discovery: the bridge does NOT advertise itself on the network.
//! Instead, after binding it writes a JSON registration file the MCP
//! server can read:
//!
//! - `IDEALYST_BRIDGE_PORT_FILE`: project-scoped path the CLI passes
//!   in (typically `<project>/.idealyst/bridge.port`). Used by the MCP
//!   server when running in the same project's cwd.
//! - `~/.idealyst/apps/<name>-<pid>.json`: a per-process entry written
//!   whenever the bridge starts, removed on graceful shutdown. Used by
//!   the MCP server to enumerate every live Idealyst app on the host.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use super::{Element, ElementId, ElementKind, Query, Robot, TreeNode};

/// Default port for the robot bridge.
pub const DEFAULT_PORT: u16 = 9718;

/// A custom bridge-command handler. Receives the request's `args` JSON
/// and returns the response `ok` payload as a JSON string (or an error
/// string that becomes the `err` field).
pub type CommandHandler = Rc<dyn Fn(&serde_json::Value) -> Result<String, String>>;

thread_local! {
    /// Verbs registered by dev-mode tooling that `runtime-core` can't
    /// implement itself (it can't see the renderer, the wire layer,
    /// etc.). The canonical example is `"screenshot"`: a dev-server /
    /// host crate that owns the scene command stream AND a headless
    /// renderer registers a handler here, so `dispatch` can route the
    /// verb without `runtime-core` ever depending on `render-wgpu` /
    /// `dev-client` (which would be a cycle). Handlers run on the UI
    /// thread inside [`BridgeHandle::poll`] / [`invoke_command`].
    static CUSTOM_COMMANDS: RefCell<HashMap<String, CommandHandler>> =
        RefCell::new(HashMap::new());
}

/// Register a custom bridge verb. `name` is the `cmd` an external
/// client sends; `handler` runs on the UI thread and produces the
/// response payload. Re-registering the same name replaces the prior
/// handler. Built-in verbs (`click`, `find_element`, …) take priority —
/// a custom name that collides with a built-in is never reached.
///
/// Must be called on the same thread that polls the bridge (the UI /
/// dev-server thread), because the registry is thread-local — the
/// handler will be invoked from `dispatch` on that thread.
pub fn register_command(
    name: impl Into<String>,
    handler: impl Fn(&serde_json::Value) -> Result<String, String> + 'static,
) {
    let name = name.into();
    CUSTOM_COMMANDS.with(|c| {
        c.borrow_mut().insert(name, Rc::new(handler));
    });
}

/// Remove a previously-registered custom verb. No-op if absent.
pub fn unregister_command(name: &str) {
    CUSTOM_COMMANDS.with(|c| {
        c.borrow_mut().remove(name);
    });
}

/// Look up + invoke a registered custom verb, if any. Clones the
/// handler `Rc` out of the registry before calling so the handler may
/// itself (un)register commands without a re-entrant borrow panic.
fn try_custom_command(cmd: &str, args: &serde_json::Value) -> Option<Result<String, String>> {
    let handler = CUSTOM_COMMANDS.with(|c| c.borrow().get(cmd).cloned())?;
    Some(handler(args))
}

/// Invoke a bridge verb in-process — the same dispatch path
/// [`BridgeHandle::poll`] runs per TCP command, minus the socket.
/// Built-in verbs plus any [`register_command`] customs. Lets in-process
/// drivers (and tests) exercise a verb directly. Must run on the UI
/// thread (where the Robot registry + custom handlers live).
pub fn invoke_command(cmd: &str, args: &serde_json::Value) -> Result<String, String> {
    dispatch(&Robot::new(), cmd, args)
}

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
/// The bridge writes these into the per-process registration JSON the
/// MCP server reads to route Robot calls by app.
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

    /// Per-process registration file written next to other live apps
    /// (`~/.idealyst/apps/<name>-<pid>.json`). Held in a thread-local
    /// so the `Drop` impl removes the file on graceful shutdown — the
    /// MCP server's directory scan won't see ghost apps after exit.
    #[cfg(not(target_arch = "wasm32"))]
    static APP_REGISTRATION: std::cell::RefCell<Option<AppRegistrationFile>> =
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
/// `runtime_core::mount(...)`.
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
/// runtime_core::robot::bridge::set_app_identity(
///     runtime_core::robot::bridge::AppIdentity {
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
        slot.borrow().clone().unwrap_or_else(|| AppIdentity {
            name: "app".to_string(),
            bundle_id: None,
            project_root: None,
        })
    })
}

/// JSON shape the bridge writes on bind. Kept inline (rather than via
/// `serde_derive`) so `runtime-core` doesn't pull `serde_derive` into
/// every host build; the document is tiny and stable.
#[cfg(not(target_arch = "wasm32"))]
fn bridge_registration_json(port: u16, identity: &AppIdentity) -> String {
    let pid = std::process::id();
    let bundle = identity
        .bundle_id
        .as_deref()
        .and_then(|s| serde_json::to_string(s).ok())
        .unwrap_or_else(|| "null".into());
    let root = identity
        .project_root
        .as_deref()
        .and_then(|s| serde_json::to_string(s).ok())
        .unwrap_or_else(|| "null".into());
    let name = serde_json::to_string(&identity.name).unwrap_or_else(|_| "\"app\"".into());
    format!(
        "{{\"port\":{port},\"pid\":{pid},\"name\":{name},\"bundle_id\":{bundle},\"project_root\":{root},\"proto\":1}}",
    )
}

/// `~/.idealyst/apps/<name>-<pid>.json` — per-process registration the
/// MCP server scans to discover live apps. RAII: dropped on graceful
/// shutdown so the directory doesn't accumulate ghosts.
#[cfg(not(target_arch = "wasm32"))]
struct AppRegistrationFile {
    path: PathBuf,
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for AppRegistrationFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Write `~/.idealyst/apps/<name>-<pid>.json` with the bound port +
/// identity, and stash the handle in a thread-local so its `Drop`
/// removes the file on graceful shutdown.
#[cfg(not(target_arch = "wasm32"))]
fn write_app_registration(port: u16, identity: &AppIdentity) {
    let Some(dir) = apps_registry_dir() else {
        return;
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!(
            "[robot-bridge] could not create {}: {}",
            dir.display(),
            e
        );
        return;
    }
    let name_label = identity.name.replace('.', "-").replace(' ', "-");
    let pid = std::process::id();
    let path = dir.join(format!("{name_label}-{pid}.json"));
    let body = bridge_registration_json(port, identity);
    if let Err(e) = std::fs::write(&path, body.as_bytes()) {
        eprintln!(
            "[robot-bridge] could not write {}: {}",
            path.display(),
            e
        );
        return;
    }
    eprintln!(
        "[robot-bridge] registered live app at {} (port {}, app={})",
        path.display(),
        port,
        identity.name,
    );
    APP_REGISTRATION.with(|slot| {
        *slot.borrow_mut() = Some(AppRegistrationFile { path });
    });
}

/// Write the per-project port-file `IDEALYST_BRIDGE_PORT_FILE` points
/// at. Lets a project-scoped MCP server (e.g. cwd-anchored) find the
/// bridge without scanning the home-dir registry. Best-effort.
#[cfg(not(target_arch = "wasm32"))]
fn write_project_port_file(port: u16, identity: &AppIdentity) {
    let Ok(path) = std::env::var("IDEALYST_BRIDGE_PORT_FILE") else {
        return;
    };
    let path = PathBuf::from(path);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let body = bridge_registration_json(port, identity);
    if let Err(e) = std::fs::write(&path, body.as_bytes()) {
        eprintln!(
            "[robot-bridge] could not write {}: {}",
            path.display(),
            e
        );
    }
}

/// `~/.idealyst/apps/`. Returns `None` if the home dir is undiscoverable
/// — registration silently no-ops in that case.
#[cfg(not(target_arch = "wasm32"))]
fn apps_registry_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)?;
    Some(home.join(".idealyst").join("apps"))
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
/// After binding, the bridge writes two files so the MCP server can
/// discover it without any network broadcast:
///
/// - `~/.idealyst/apps/<name>-<pid>.json` — always written; removed
///   on graceful shutdown via RAII. The MCP server scans this dir to
///   enumerate every live app on the host.
/// - `$IDEALYST_BRIDGE_PORT_FILE` if set — same JSON, written wherever
///   the CLI pointed via env var. Used by project-scoped MCP flows
///   (cwd-anchored) that don't need the full directory scan.
///
/// Both files contain `{port, pid, name, bundle_id, project_root,
/// proto}` so the MCP server can verify the project before issuing
/// Robot calls.
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
        // Write discovery files so the MCP server can find us.
        // Non-wasm only — `~/.idealyst/apps/` doesn't exist in the
        // browser sandbox, and the MCP server doesn't run there
        // anyway.
        #[cfg(not(target_arch = "wasm32"))]
        {
            let identity = current_identity();
            write_app_registration(bound_port, &identity);
            write_project_port_file(bound_port, &identity);
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
        #[cfg(feature = "catalog")]
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
        // Fall back to a dev-mode-registered custom verb (e.g.
        // "screenshot") before declaring the command unknown.
        _ => match try_custom_command(cmd, args) {
            Some(result) => result,
            None => Err(format!("unknown command: {}", cmd)),
        },
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

// =============================================================================
// Tests
// =============================================================================

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    //! Regression coverage for the file-based discovery write path
    //! that replaced mDNS advertising. The MCP server's
    //! `~/.idealyst/apps/` scanner depends on the exact JSON shape
    //! `bridge_registration_json` produces.

    use super::*;

    #[test]
    fn registration_json_carries_port_pid_name_and_optionals() {
        let identity = AppIdentity {
            name: "demo".to_string(),
            bundle_id: Some("com.example.demo".to_string()),
            project_root: Some("/tmp/demo".to_string()),
        };
        let json = bridge_registration_json(42, &identity);
        let parsed: serde_json::Value = serde_json::from_str(&json)
            .expect("bridge registration JSON must round-trip");
        assert_eq!(parsed["port"].as_u64(), Some(42));
        assert_eq!(parsed["name"].as_str(), Some("demo"));
        assert_eq!(parsed["bundle_id"].as_str(), Some("com.example.demo"));
        assert_eq!(parsed["project_root"].as_str(), Some("/tmp/demo"));
        assert_eq!(parsed["proto"].as_u64(), Some(1));
        assert!(parsed["pid"].as_u64().is_some(), "pid must be numeric");
    }

    #[test]
    fn registration_json_null_optionals_when_unset() {
        let identity = AppIdentity {
            name: "demo".to_string(),
            bundle_id: None,
            project_root: None,
        };
        let json = bridge_registration_json(9000, &identity);
        let parsed: serde_json::Value = serde_json::from_str(&json)
            .expect("bridge registration JSON must round-trip");
        assert!(parsed["bundle_id"].is_null());
        assert!(parsed["project_root"].is_null());
    }

    #[test]
    fn write_project_port_file_writes_when_env_var_is_set() {
        // Use a sandboxed temp path. `tempfile` isn't a dep of
        // runtime-core (kept minimal); roll our own under
        // `std::env::temp_dir`.
        let dir = std::env::temp_dir().join(format!(
            "idealyst-bridge-port-test-{}",
            std::process::id(),
        ));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("bridge.port");
        let _ = std::fs::remove_file(&path);
        // Scope the env var so a concurrent test isn't poisoned.
        std::env::set_var("IDEALYST_BRIDGE_PORT_FILE", &path);
        write_project_port_file(
            12345,
            &AppIdentity {
                name: "demo".to_string(),
                bundle_id: None,
                project_root: None,
            },
        );
        std::env::remove_var("IDEALYST_BRIDGE_PORT_FILE");

        let raw = std::fs::read_to_string(&path)
            .expect("write_project_port_file must have written the path");
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed["port"].as_u64(), Some(12345));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn write_project_port_file_no_op_when_env_var_unset() {
        // Make absolutely sure the var isn't set from a prior test or
        // a parallel runner.
        std::env::remove_var("IDEALYST_BRIDGE_PORT_FILE");
        write_project_port_file(
            12345,
            &AppIdentity {
                name: "demo".to_string(),
                bundle_id: None,
                project_root: None,
            },
        );
        // No assertion needed: the function should just return
        // without panicking when there's no env var to point at.
    }

    #[test]
    fn custom_command_dispatches_and_unregisters() {
        // Unknown verb errors before registration.
        let before = invoke_command("unit_echo", &serde_json::json!({"v": 7}));
        assert!(
            before.is_err(),
            "unregistered verb must be 'unknown command', got {before:?}"
        );

        // Register a custom verb that echoes an arg back as the payload.
        register_command("unit_echo", |args| {
            let v = args.get("v").and_then(|v| v.as_i64()).unwrap_or(-1);
            Ok(format!("{{\"echo\":{v}}}"))
        });

        let ok = invoke_command("unit_echo", &serde_json::json!({"v": 7}))
            .expect("registered verb must dispatch");
        let parsed: serde_json::Value = serde_json::from_str(&ok).unwrap();
        assert_eq!(parsed["echo"], 7, "custom handler payload must round-trip");

        // A built-in verb name is never shadowed by a custom one.
        // (`ping` is built-in.) Registering `ping` must NOT override it.
        register_command("ping", |_| Ok("\"hijacked\"".into()));
        let ping = invoke_command("ping", &serde_json::json!({})).unwrap();
        assert_eq!(ping, "\"pong\"", "built-in verbs take priority over customs");

        // Unregister restores 'unknown command'.
        unregister_command("unit_echo");
        assert!(
            invoke_command("unit_echo", &serde_json::json!({})).is_err(),
            "unregistered verb must error again"
        );
        unregister_command("ping");
    }
}
