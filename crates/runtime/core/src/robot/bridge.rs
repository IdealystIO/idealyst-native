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
    // Sweep ghost registrations from prior runs that crashed / were
    // SIGKILLed before their RAII `Drop` could remove their file. The MCP
    // server also prunes at scan time, but a proactive sweep here keeps the
    // directory honest even when no scan happens, and prevents the MCP
    // server from trying to dial a dead app's port. Best-effort.
    prune_dead_registrations(&dir);
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

/// Remove `*.json` registration files in `dir` whose process is gone.
/// Each file carries a `"pid":N` field; we read it and drop the file when
/// `pid_is_live(N)` is false. Files we can't parse a pid out of are left
/// alone (conservative — don't delete something we don't understand).
/// Best-effort: I/O errors are ignored so a transient FS hiccup never
/// blocks the bind path.
#[cfg(not(target_arch = "wasm32"))]
fn prune_dead_registrations(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(pid) = parse_pid_field(&body) else {
            continue;
        };
        // Never prune our own (not-yet-written) entry; `std::process::id()`
        // is this live process, so the liveness check below already keeps
        // it. The check is the single source of truth.
        if !pid_is_live(pid) {
            if std::fs::remove_file(&path).is_ok() {
                eprintln!(
                    "[robot-bridge] pruned stale registration {} (pid {} gone)",
                    path.display(),
                    pid,
                );
            }
        }
    }
}

/// Pull the `pid` integer out of a registration JSON body without a full
/// serde parse (the bridge writes the JSON by hand via `format!`, so the
/// shape is stable). Returns `None` if the field is absent or malformed.
#[cfg(not(target_arch = "wasm32"))]
fn parse_pid_field(body: &str) -> Option<u32> {
    let after = body.split("\"pid\":").nth(1)?;
    let digits: String = after
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// `kill(pid, 0)` — succeeds if the process is alive (or alive but not
/// signalable by us → EPERM); fails with ESRCH when gone. Mirrors the MCP
/// server's `app_discovery::pid_is_live` so both sides agree on liveness.
#[cfg(all(not(target_arch = "wasm32"), unix))]
fn pid_is_live(pid: u32) -> bool {
    // SAFETY: `kill` with signal 0 is a no-op liveness probe on POSIX.
    let rc = unsafe { libc::kill(pid as i32, 0) };
    if rc == 0 {
        return true;
    }
    matches!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::EPERM)
    )
}

/// Non-unix, non-wasm (Windows): no cheap liveness probe wired up yet, so
/// treat every registration as live and rely on the RAII `Drop` cleanup.
/// A future pass can use `OpenProcess` + `GetExitCodeProcess`.
#[cfg(all(not(target_arch = "wasm32"), not(unix)))]
fn pid_is_live(_pid: u32) -> bool {
    true
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
    // Bind LOOPBACK, not `0.0.0.0`. Two reasons:
    //   1. Correct conflict detection. Binding `0.0.0.0:P` while another
    //      process already holds `127.0.0.1:P` (e.g. `adb` squats on 9718,
    //      our default) *succeeds* on macOS/BSD — but loopback connections
    //      then route to the more-specific `127.0.0.1` listener (adb), so
    //      our bridge is bound yet unreachable via localhost and never sees
    //      a request. Binding `127.0.0.1:P` instead fails cleanly with
    //      `AddrInUse`, so `start_auto_polling` falls back to an ephemeral
    //      port that IS reachable, and registers that real port.
    //   2. Security: the robot bridge is a local dev-control channel; it
    //      should never be reachable from the network.
    let listener = TcpListener::bind(("127.0.0.1", port))?;
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
                    let element_id = match s.element_id {
                        Some(eid) => eid.0.to_string(),
                        None => "null".to_string(),
                    };
                    format!(
                        "{{\"instance_id\":{},\"name\":{},\"element_id\":{},\"methods\":[{}]}}",
                        s.id.0,
                        serde_json::to_string(s.name).unwrap(),
                        element_id,
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
        "get_device_frame" => {
            // Physical device-screen pixels (origin = display top-left).
            // This is what an OS-level input injector (`adb shell input
            // tap`, XCUITest, CGEvent) taps at — see `Backend::device_frame`.
            let el = resolve_element(args)?;
            match robot.device_frame(&el).map_err(|e| e.to_string())? {
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
        "get_arena_stats" => {
            // Reactive-arena occupancy: signal/effect/ref slot counts plus
            // aggregate subscriber/dependency link totals. A leak shows up
            // as `*_total` (or `total_subscribers`/`total_deps`) climbing
            // while `*_in_use` stays bounded. See `reactive::ArenaStats`.
            let s = robot.arena_stats();
            Ok(format!(
                "{{\"signals_in_use\":{},\"signals_total\":{},\
                   \"effects_in_use\":{},\"effects_total\":{},\
                   \"refs_in_use\":{},\"refs_total\":{},\
                   \"total_subscribers\":{},\"total_deps\":{}}}",
                s.signals_in_use, s.signals_total,
                s.effects_in_use, s.effects_total,
                s.refs_in_use, s.refs_total,
                s.total_subscribers, s.total_deps,
            ))
        }
        "list_watched_signals" => {
            // Every live signal registered via `signal!` (auto) or
            // `robot::watch_signal` (explicit), with its current value.
            let items: Vec<String> = super::watch::list_watched()
                .iter()
                .map(|s| {
                    format!(
                        "{{\"id\":{},\"name\":{},\"value\":{}}}",
                        s.id,
                        serde_json::to_string(&s.name).unwrap_or_else(|_| "\"\"".into()),
                        // `Value`'s Display emits valid compact JSON.
                        s.value,
                    )
                })
                .collect();
            Ok(format!("[{}]", items.join(",")))
        }
        "read_signal" => {
            // Read one watched signal by `name` or raw `id`. `null` when
            // absent or stale (recycled slot — generation mismatch).
            let value = if let Some(name) = args["name"].as_str() {
                super::watch::read_watched_by_name(name)
            } else if let Some(id) = args["id"].as_u64() {
                super::watch::read_watched_by_id(id as u32)
            } else {
                return Err("read_signal requires a 'name' or 'id' argument".into());
            };
            Ok(value.map(|v| v.to_string()).unwrap_or_else(|| "null".into()))
        }
        "list_navigators" => {
            // Every mounted navigator: which is current, its route/path/depth,
            // and its full back-stack. Use `get_children` with an entry's
            // `element_id` to read that navigator's current-screen elements.
            let items: Vec<String> = crate::primitives::navigator::all_navigators()
                .iter()
                .map(nav_snapshot_json)
                .collect();
            Ok(format!("[{}]", items.join(",")))
        }
        "get_navigator_state" => {
            let nav_id = args["nav_id"].as_u64().ok_or("missing 'nav_id' argument")? as u32;
            match crate::primitives::navigator::navigator_snapshot(
                crate::primitives::navigator::NavId(nav_id),
            ) {
                Some(s) => Ok(nav_snapshot_json(&s)),
                None => Ok("null".into()),
            }
        }
        // Perf phase counters live behind `debug-stats`. When the target
        // wasn't built with it, the counters don't exist — return a clear
        // hint so the dashboard explains *why* perf is empty rather than
        // showing a silent blank. `take_phase_counters` drains, so each
        // poll reports activity since the previous read.
        #[cfg(feature = "debug-stats")]
        "get_perf_counters" => {
            let counters = crate::debug::take_phase_counters();
            let mut items: Vec<String> = counters
                .iter()
                .map(|(phase, c)| {
                    format!(
                        "{{\"phase\":{},\"call_count\":{},\"total_us\":{},\"max_us\":{}}}",
                        serde_json::to_string(phase).unwrap_or_else(|_| "\"\"".into()),
                        c.call_count, c.total_us, c.max_us,
                    )
                })
                .collect();
            // HashMap iteration order is nondeterministic; sort the rendered
            // rows (each begins `{"phase":"<name>"`) for a stable display.
            items.sort();
            Ok(format!("[{}]", items.join(",")))
        }
        #[cfg(not(feature = "debug-stats"))]
        "get_perf_counters" => Err(
            "perf disabled: rebuild the target app with the `debug-stats` feature \
             to capture phase counters"
                .into(),
        ),
        #[cfg(feature = "debug-stats")]
        "clear_perf_counters" => {
            crate::debug::clear_phase_counters();
            Ok("\"ok\"".into())
        }
        #[cfg(not(feature = "debug-stats"))]
        "clear_perf_counters" => Err(
            "perf disabled: rebuild the target app with the `debug-stats` feature".into(),
        ),
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
        Ok(Query::TestId(id.to_string()))
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

fn nav_snapshot_json(s: &crate::primitives::navigator::NavSnapshot) -> String {
    let stack: Vec<String> = s
        .stack
        .iter()
        .map(|(route, path)| {
            format!(
                "{{\"route\":{},\"path\":{}}}",
                serde_json::to_string(route).unwrap_or_else(|_| "\"\"".into()),
                serde_json::to_string(path).unwrap_or_else(|_| "\"\"".into()),
            )
        })
        .collect();
    let element_id = match s.element_id {
        Some(id) => id.to_string(),
        None => "null".into(),
    };
    format!(
        "{{\"nav_id\":{},\"element_id\":{},\"type_name\":{},\"active_route\":{},\
           \"active_path\":{},\"depth\":{},\"can_go_back\":{},\"is_current\":{},\
           \"base\":{},\"stack\":[{}]}}",
        s.nav_id,
        element_id,
        serde_json::to_string(s.type_name).unwrap_or_else(|_| "\"\"".into()),
        serde_json::to_string(&s.active_route).unwrap_or_else(|_| "\"\"".into()),
        serde_json::to_string(&s.active_path).unwrap_or_else(|_| "\"\"".into()),
        s.depth,
        s.can_go_back,
        s.is_current,
        serde_json::to_string(&s.base).unwrap_or_else(|_| "\"\"".into()),
        stack.join(","),
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
    fn parse_pid_field_extracts_pid_from_registration_body() {
        let body = bridge_registration_json(
            55,
            &AppIdentity {
                name: "demo".into(),
                bundle_id: None,
                project_root: None,
            },
        );
        let pid = parse_pid_field(&body).expect("pid must parse out of the registration JSON");
        assert_eq!(pid, std::process::id());
        // Malformed / missing → None, never a panic.
        assert_eq!(parse_pid_field("{\"port\":1}"), None);
        assert_eq!(parse_pid_field("{\"pid\":\"oops\"}"), None);
    }

    /// E1(a) regression: a registration file whose process is gone is
    /// swept by `prune_dead_registrations`, while a live one (this test
    /// process) is kept. Before the prune, a crashed prior run left a
    /// ghost the MCP server would try to dial.
    #[test]
    fn prune_removes_dead_pid_keeps_live() {
        let dir = std::env::temp_dir().join(format!(
            "idealyst-prune-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // A guaranteed-dead pid: spawn a no-op child, wait for it to exit,
        // then reuse its (now-reaped) pid. `kill(pid, 0)` reports ESRCH.
        let mut child = std::process::Command::new("true")
            .spawn()
            .or_else(|_| std::process::Command::new("cmd").args(["/C", "exit"]).spawn())
            .expect("spawn a short-lived child for a dead pid");
        let dead_pid = child.id();
        let _ = child.wait();

        let live_body = bridge_registration_json(
            100,
            &AppIdentity {
                name: "live".into(),
                bundle_id: None,
                project_root: None,
            },
        );
        // Forge a dead-pid body by swapping the pid in.
        let dead_body = live_body.replace(
            &format!("\"pid\":{}", std::process::id()),
            &format!("\"pid\":{dead_pid}"),
        );

        let live_path = dir.join(format!("live-{}.json", std::process::id()));
        let dead_path = dir.join(format!("dead-{dead_pid}.json"));
        std::fs::write(&live_path, live_body.as_bytes()).unwrap();
        std::fs::write(&dead_path, dead_body.as_bytes()).unwrap();
        // A non-json sibling must be left untouched.
        let other = dir.join("notes.txt");
        std::fs::write(&other, b"keep me").unwrap();

        prune_dead_registrations(&dir);

        assert!(live_path.exists(), "live-pid registration must be kept");
        assert!(!dead_path.exists(), "dead-pid registration must be pruned");
        assert!(other.exists(), "non-json files must be left alone");

        let _ = std::fs::remove_dir_all(&dir);
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

    /// Regression: the `get_device_frame` verb routes through the
    /// `ElementActions.device_frame` closure the walker attaches and
    /// serializes the physical-pixel rect for the OS-injection driver.
    /// Before `device_frame` existed there was no path from an element
    /// id to its on-screen pixel coordinates over the bridge.
    #[test]
    fn get_device_frame_returns_registered_device_rect() {
        use crate::primitives::portal::ViewportRect;

        // Mirror what `walker.rs` does at mount: register the element,
        // then attach frame closures. The `device_frame` closure reports
        // a known physical-pixel rect; the others are inert.
        let id = crate::robot::register(crate::robot::RegistryEntry {
            kind: ElementKind::Button,
            test_id: Some("dev_frame_probe"),
            label: None,
            label_fn: None,
            actions: crate::robot::ElementActions::empty(),
            parent: None,
            children: Vec::new(),
        });
        crate::robot::attach_frame_actions(
            id,
            Rc::new(|| None),
            Rc::new(|| None),
            Rc::new(|| {
                Some(ViewportRect {
                    x: 120.0,
                    y: 240.0,
                    width: 80.0,
                    height: 40.0,
                })
            }),
        );

        let out = invoke_command("get_device_frame", &serde_json::json!({ "element_id": id.0 }))
            .expect("get_device_frame should dispatch");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["x"], 120.0);
        assert_eq!(v["y"], 240.0);
        assert_eq!(v["width"], 80.0);
        assert_eq!(v["height"], 40.0);

        // An element with no device_frame action reports `null`, not an
        // error — the host driver treats that as "not tappable yet".
        let bare = crate::robot::register(crate::robot::RegistryEntry {
            kind: ElementKind::View,
            test_id: None,
            label: None,
            label_fn: None,
            actions: crate::robot::ElementActions::empty(),
            parent: None,
            children: Vec::new(),
        });
        let none_out = invoke_command("get_device_frame", &serde_json::json!({ "element_id": bare.0 }));
        // No device_frame closure attached → ActionNotAvailable → Err.
        assert!(none_out.is_err(), "missing device_frame action surfaces as err");

        crate::robot::deregister(id);
        crate::robot::deregister(bare);
    }

    /// The `get_arena_stats` verb serializes all eight `ArenaStats`
    /// fields as a flat JSON object so the inspector's perf panel can
    /// read reactive-arena occupancy over the bridge. Before this verb
    /// the arena counts were reachable via `Robot::arena_stats()` only
    /// in-process — never over the wire.
    #[test]
    fn arena_stats_verb_serializes_eight_fields() {
        // Touch the arena so the counts are plausibly non-trivial; the
        // exact values don't matter, only that all keys are present and
        // numeric.
        let s = crate::reactive::Signal::new(7i32);
        let _ = crate::reactive::untrack(|| s.get());

        let out = invoke_command("get_arena_stats", &serde_json::json!({}))
            .expect("get_arena_stats should dispatch");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        for key in [
            "signals_in_use",
            "signals_total",
            "effects_in_use",
            "effects_total",
            "refs_in_use",
            "refs_total",
            "total_subscribers",
            "total_deps",
        ] {
            assert!(
                v.get(key).and_then(|n| n.as_u64()).is_some(),
                "arena stats must expose numeric '{key}', got {v}"
            );
        }
    }

    /// The `list_watched_signals` / `read_signal` verbs expose a
    /// `signal!`- or `watch_signal`-registered signal's live value over
    /// the bridge. Before these, signal values were reachable only via a
    /// typed in-process handle — nothing crossed the JSON wire.
    #[test]
    fn watch_verbs_read_live_signal_value_over_bridge() {
        crate::robot::watch::clear();
        let s = crate::reactive::Signal::new(1i32);
        crate::robot::watch_signal("bridge_counter", s);
        s.set(42);

        // read_signal by name
        let out = invoke_command("read_signal", &serde_json::json!({ "name": "bridge_counter" }))
            .expect("read_signal by name should dispatch");
        assert_eq!(out, "\"42\"");

        // read_signal by raw id
        let by_id = invoke_command("read_signal", &serde_json::json!({ "id": s.id() })).unwrap();
        assert_eq!(by_id, "\"42\"");

        // list_watched_signals carries the row
        let list = invoke_command("list_watched_signals", &serde_json::json!({})).unwrap();
        let v: serde_json::Value = serde_json::from_str(&list).unwrap();
        assert!(
            v.as_array()
                .unwrap()
                .iter()
                .any(|r| r["name"] == "bridge_counter" && r["value"] == "42"),
            "list_watched_signals must include the watched signal, got {v}"
        );

        // Missing both args is a clear error, not a panic.
        assert!(invoke_command("read_signal", &serde_json::json!({})).is_err());
        crate::robot::watch::unwatch_signal(s.id() as u32);
    }

    /// The `list_navigators` / `get_navigator_state` verbs render a
    /// registered navigator's route/depth/back-stack over the bridge. The
    /// navigator registry + JSON shape are what the inspector's Navigators
    /// panel consumes.
    #[test]
    fn navigator_verbs_render_registered_navigator() {
        use crate::primitives::navigator::{
            all_navigators, nav_registry_reset, register_navigator, NavId, NavState,
            NavigatorControl,
        };
        use crate::reactive::{with_scope, Scope, Signal};
        use std::rc::Rc;

        nav_registry_reset();
        let control = Rc::new(NavigatorControl::new());
        let mut scope = Box::new(Scope::new());
        let ns = with_scope(&mut scope, || NavState {
            active_route: Signal::new("detail"),
            active_path: Signal::new("/items/5".to_string()),
            depth: Signal::new(2),
            can_go_back: Signal::new(true),
        });
        control.attach_nav_state(ns);
        control.install_stack_snapshot(Box::new(|| {
            vec![
                ("list".to_string(), "/items".to_string()),
                ("detail".to_string(), "/items/5".to_string()),
            ]
        }));
        let id = register_navigator(&control, "stack_navigator::Presentation", Some(3));
        control.set_nav_id(id);

        let list = invoke_command("list_navigators", &serde_json::json!({})).unwrap();
        let v: serde_json::Value = serde_json::from_str(&list).unwrap();
        let row = &v.as_array().unwrap()[0];
        assert_eq!(row["nav_id"].as_u64(), Some(id.0 as u64));
        assert_eq!(row["element_id"].as_u64(), Some(3));
        assert_eq!(row["type_name"], "stack_navigator::Presentation");
        assert_eq!(row["active_route"], "detail");
        assert_eq!(row["depth"].as_u64(), Some(2));
        assert_eq!(row["can_go_back"], true);
        assert_eq!(row["is_current"], true);
        assert_eq!(row["stack"].as_array().unwrap().len(), 2);
        assert_eq!(row["stack"][1]["route"], "detail");

        // get_navigator_state by id returns the same shape.
        let one =
            invoke_command("get_navigator_state", &serde_json::json!({ "nav_id": id.0 })).unwrap();
        let o: serde_json::Value = serde_json::from_str(&one).unwrap();
        assert_eq!(o["active_path"], "/items/5");

        // Unknown id → null.
        let none = invoke_command("get_navigator_state", &serde_json::json!({ "nav_id": 9999 }))
            .unwrap();
        assert_eq!(none, "null");

        let _ = all_navigators();
        nav_registry_reset();
        let _ = NavId(0);
    }

    /// With `debug-stats` on, `get_perf_counters` drains the phase
    /// counters and renders them as `[{phase,call_count,total_us,max_us}]`.
    /// This is the only path from the backend's apply-phase timers to an
    /// external dashboard.
    #[cfg(feature = "debug-stats")]
    #[test]
    fn perf_counters_verb_round_trips_a_recorded_phase() {
        crate::debug::clear_phase_counters();
        crate::debug::record_apply_phase("unit_test_phase", 1234);

        let out = invoke_command("get_perf_counters", &serde_json::json!({}))
            .expect("get_perf_counters should dispatch under debug-stats");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let row = v
            .as_array()
            .and_then(|rows| rows.iter().find(|r| r["phase"] == "unit_test_phase"))
            .expect("recorded phase must appear in the perf counters");
        assert_eq!(row["call_count"].as_u64(), Some(1));
        assert_eq!(row["total_us"].as_u64(), Some(1234));
        assert_eq!(row["max_us"].as_u64(), Some(1234));

        // `get_perf_counters` drains: a second read no longer sees it.
        let out2 = invoke_command("get_perf_counters", &serde_json::json!({})).unwrap();
        let v2: serde_json::Value = serde_json::from_str(&out2).unwrap();
        assert!(
            v2.as_array()
                .map(|rows| rows.iter().all(|r| r["phase"] != "unit_test_phase"))
                .unwrap_or(true),
            "drain semantics: phase should be gone after the first read"
        );
    }

    /// Without `debug-stats`, the verb returns a clear hint (not a silent
    /// empty array) so the dashboard can explain why perf is unavailable.
    #[cfg(not(feature = "debug-stats"))]
    #[test]
    fn perf_counters_verb_hints_when_debug_stats_off() {
        let out = invoke_command("get_perf_counters", &serde_json::json!({}));
        let err = out.expect_err("perf verb must error when debug-stats is off");
        assert!(
            err.contains("debug-stats"),
            "error must name the missing feature, got {err:?}"
        );
    }
}
