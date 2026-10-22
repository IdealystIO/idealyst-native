//! `CatalogService` — the rmcp `ServerHandler` implementation that
//! surfaces the mcp-catalog catalog, the Robot bridge and `idealyst dev`
//! session control as MCP tools, plus the `idealyst://catalog` resource.
//!
//! Tool definitions stay small on purpose: they sit in the client's context
//! for the whole session. Each carries a one-line description and one-line
//! parameter docs; the long form lives in `docs/*.md` and is served on
//! demand by the `get_docs` tool (see `crate::tool_docs`). Advertised input
//! schemas are also passed through `crate::schema_minify`.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

use mcp_catalog::{ComponentEntry, EdgeStatus, EntryRef, ResolvedCatalog};

use crate::adb;
use crate::robot_bridge::RobotBridge;
use rmcp::{
    ErrorData as McpError,
    RoleServer, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars,
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use serde_json::json;

/// How the server reaches a running app's Robot bridge.
#[derive(Clone)]
pub(crate) enum RobotMode {
    /// `--no-robot`: robot control is off entirely. No `~/.idealyst/apps/`
    /// scan, no bridge connection, robot tools return a clear error, and
    /// catalog tools never reach out over a bridge — they serve only the
    /// in-process / subprocess catalog. "No robot" means no robot.
    Disabled,
    /// Default: scan `~/.idealyst/apps/<name>-<pid>.json` registration
    /// files and route per call, picking the unique live app or by `app`.
    Discovery,
    /// `--robot-port` (+ optional `--robot-host`): talk to exactly one
    /// bridge at this explicit `host:port`. Discovery is skipped — the
    /// connection is pinned, matching the "explicit on the network"
    /// model (the CLI can automate establishing it, but the address is
    /// never guessed via multicast).
    Explicit { addr: String },
}

/// The MCP catalog server. Holds the resolved catalog behind an
/// `Arc<RwLock<...>>` so the file-watch thread (`crate::watch`) can swap in
/// a fresh `ResolvedCatalog` without taking the server down.
#[derive(Clone)]
pub struct CatalogService {
    catalog: Arc<RwLock<ResolvedCatalog>>,
    /// Freshness of `catalog` — whether it is current, being rebuilt, or
    /// stale because the last rebuild failed. Kept in its OWN lock so
    /// reading status never queues behind a rebuild holding the catalog
    /// write lock, which is exactly when a caller most wants to ask.
    status: Arc<RwLock<crate::catalog_status::CatalogStatus>>,
    /// Broadcasts the status generation so `catalog_status` can wait for
    /// an in-flight rebuild to land instead of making callers poll.
    status_tx: Arc<tokio::sync::watch::Sender<u64>>,
    /// How robot control is reached (off / discovery / explicit addr).
    robot_mode: RobotMode,
    /// Legacy single-bridge mode — used when the server was
    /// constructed with `with_robot_bridge(...)`, and for
    /// [`RobotMode::Explicit`]. The discovery mode ignores this and
    /// looks up bridges per-call.
    robot: Option<Arc<RobotBridge>>,
    /// Registry-driven resolver: looks up bridges by app name from
    /// `~/.idealyst/registry.json` on each Robot tool call. Bridges
    /// are cached per `(name, addr)` so we don't TCP-handshake on
    /// every call.
    resolver: Arc<RobotResolver>,
    /// Per-app catalog cache (catalog_bin → ResolvedCatalog). Lets
    /// catalog tools fan out across every registered app without
    /// re-spawning the extractor on every call.
    catalog_cache: Arc<CatalogCache>,
    /// Live discovered-app table. Background thread polls
    /// `~/.idealyst/apps/<name>-<pid>.json` registration files the
    /// running app's Robot bridge writes on bind; tools consult this
    /// before falling back to the on-disk catalog. Empty (and
    /// harmlessly so) when no app is running.
    discovery: crate::app_discovery::DiscoveryTable,
    /// Tracks `idealyst dev` sessions launched via the `run_dev` tool so
    /// `list_dev_sessions` / `stop_dev` can manage them. Shared by `Arc`
    /// so every `CatalogService` clone (and the rmcp handler) sees the
    /// same table; the single owning `Arc` drop tears sessions down on
    /// server shutdown.
    dev_runner: Arc<crate::dev_runner::DevRunner>,
    /// The advertised tools, with minified schemas (see
    /// [`CatalogService::advertised_router`]). The `#[tool_handler]` impl is
    /// pointed at this field explicitly; `get_docs` reads definitions from
    /// it too.
    tool_router: ToolRouter<CatalogService>,
}

/// Catalog fetcher — runs `get_catalog` over a live app's Robot
/// bridge. No local cache; the live app is always authoritative.
/// Offline catalogs (no app running) load through
/// `watch::preload_subprocess_catalog`, not through this type.
#[derive(Default)]
pub(crate) struct CatalogCache {}

impl CatalogCache {
    /// Fetch a catalog over an already-running app's Robot bridge
    /// (the `get_catalog` command). Zero subprocess spawn; the live
    /// app's inventory is the most up-to-date source.
    pub async fn get_via_bridge(
        &self,
        bridge: &crate::robot_bridge::RobotBridge,
    ) -> Result<Arc<ResolvedCatalog>, String> {
        let value = bridge
            .call("get_catalog", serde_json::Value::Null)
            .await
            .map_err(|e| e.to_string())?;
        let json = serde_json::to_string(&value).map_err(|e| e.to_string())?;
        let cat =
            ResolvedCatalog::build_from_json(&json).map_err(|e| e.to_string())?;
        Ok(Arc::new(cat))
    }
}

/// Per-app Robot bridge cache. Looks up the app in the user-level
/// registry, builds a [`RobotBridge`] for its address, and reuses
/// it across calls. Reloads the registry on every miss so newly-
/// launched apps appear without restarting the MCP server.
#[derive(Default)]
pub(crate) struct RobotResolver {
    bridges: Mutex<HashMap<String, Arc<RobotBridge>>>,
}

impl RobotResolver {
    /// Resolve a Robot bridge for the named app (or the only-live app
    /// when `app` is omitted). Discovery scans the per-process
    /// `~/.idealyst/apps/<name>-<pid>.json` registration files the
    /// bridge writes on bind — that's the single source of truth for
    /// "what's running." Caches the resulting `RobotBridge` so
    /// subsequent calls reuse the same TCP connection state.
    pub async fn resolve(
        &self,
        app: Option<&str>,
        discovery: &crate::app_discovery::DiscoveryTable,
    ) -> Result<Arc<RobotBridge>, McpError> {
        let live = discovery.snapshot();
        let entry = match app {
            // The selector is `name` or — to disambiguate two platforms of one
            // app — `name:platform` (e.g. `todo:web`).
            Some(sel) => {
                let (name, platform) = match sel.split_once(':') {
                    Some((n, p)) => (n, Some(p)),
                    None => (sel, None),
                };
                let matches: Vec<_> = live
                    .iter()
                    .filter(|a| a.name == name)
                    .filter(|a| platform.map_or(true, |p| a.platform.as_deref() == Some(p)))
                    .collect();
                match matches.as_slice() {
                    [one] => (*one).clone(),
                    [] => {
                        return Err(McpError::invalid_params(
                            format!(
                                "app {:?} not running. Live apps: {:?}. \
                                 Run `idealyst dev` in the target project.",
                                sel,
                                app_selectors(&live)
                            ),
                            None,
                        ));
                    }
                    _ => {
                        return Err(McpError::invalid_params(
                            format!(
                                "{:?} is ambiguous across platforms; pass `name:platform` \
                                 (one of: {:?})",
                                sel,
                                app_selectors(&live)
                            ),
                            None,
                        ));
                    }
                }
            }
            None if live.len() == 1 => live.into_iter().next().unwrap(),
            None if live.is_empty() => {
                return Err(McpError::invalid_params(
                    "no live apps discovered — run `idealyst dev` in a project first"
                        .to_string(),
                    None,
                ));
            }
            None => {
                return Err(McpError::invalid_params(
                    format!(
                        "{} apps live; specify `app` (one of: {:?})",
                        live.len(),
                        app_selectors(&live)
                    ),
                    None,
                ));
            }
        };
        let mut bridges = self.bridges.lock().await;
        // Cache per bridge ADDRESS (unique per app), not per name — two
        // same-name apps must not share one connection.
        let bridge = bridges
            .entry(entry.bridge_addr.clone())
            .or_insert_with(|| Arc::new(RobotBridge::new(entry.bridge_addr.clone())))
            .clone();
        Ok(bridge)
    }
}

/// The unambiguous selector string for each live app: `name` when its name is
/// unique, else `name:platform`. What `list_apps` shows and the agent passes
/// back as `app`.
pub(crate) fn app_selectors(live: &[crate::app_discovery::DiscoveredApp]) -> Vec<String> {
    live.iter()
        .map(|a| {
            let name_count = live.iter().filter(|b| b.name == a.name).count();
            match (name_count, &a.platform) {
                (n, Some(p)) if n > 1 => format!("{}:{}", a.name, p),
                _ => a.name.clone(),
            }
        })
        .collect()
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct NameRequest {
    /// Short or fully-qualified name.
    pub name: String,
    /// Restrict to one crate (`my-app` or `my_app`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

// Every Robot args struct carries a flat, optional `app` field: the
// running-app selector from `list_apps`, optional when exactly one app is
// registered. It is left undocumented in each schema on purpose; the server
// instructions and the `app` docs topic explain it once instead of in
// every tool definition. `strip_app` removes it before the bridge call.

/// Find-by criteria for `find_element` / `find_all_elements` /
/// `count_elements`. The bridge applies ONE criterion, the first set in the
/// order test_id › label › label_contains › kind; with none set every
/// element matches. `skip_serializing_if` keeps the wire payload to the
/// criteria the caller actually set.
#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotFindArgs {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// Exact test_id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_id: Option<String>,
    /// Exact label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Label substring.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label_contains: Option<String>,
    /// Element kind (`Button`, `Text`, `TextInput`, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotElementId {
    /// From `find_element` / `find_all_elements`.
    pub element_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotParityArgs {
    /// `list_apps` selector (e.g. `todo:web`).
    pub app_a: String,
    /// Selector to compare against `app_a`.
    pub app_b: String,
    /// Per channel, 0..1 (default 0.02).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_tolerance: Option<f32>,
    /// Lengths in px (default 0.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length_tolerance: Option<f32>,
    /// Disable cross-platform normalization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<bool>,
    /// Compare only the subtree under this test_id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    /// Size/offset in px (default 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry_tolerance: Option<f32>,
    /// Skip geometry; diff props only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_geometry: Option<bool>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotTypeText {
    pub element_id: u64,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotDrag {
    pub element_id: u64,
    /// `[x, y]` path in logical px, relative to the element's top-left.
    pub points: Vec<[f32; 2]>,
    /// Hold at the first point first (200+ for drag-and-drop on iOS/Android).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotSetToggle {
    pub element_id: u64,
    pub value: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotSetSlider {
    pub element_id: u64,
    pub value: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotTap {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// Tap this element's center (takes precedence).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element_id: Option<u64>,
    /// Tap the element with this test_id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_id: Option<String>,
    /// Raw x in physical device px (with `y`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<i32>,
    /// adb device serial, when several are attached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotInvokeMethod {
    pub instance_id: u64,
    pub method: String,
    /// Args keyed by parameter name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotDragNative {
    pub element_id: u64,
    /// `[x, y]` path in logical px, relative to the element's top-left.
    pub points: Vec<[f32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

/// What `pointer_native` does with the mouse.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PointerNativeAction {
    /// Move over the element with no button held.
    Hover,
    /// Press the primary button down on it and keep it down.
    Press,
    /// Lift the press at it.
    Release,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotPointerNative {
    pub element_id: u64,
    pub action: PointerNativeAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotOpenLink {
    /// e.g. `myapp://items/42`.
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotCallFunction {
    /// Registered name (e.g. `location.set`).
    pub name: String,
    /// Args keyed by parameter name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<serde_json::Value>,
    /// Async wait limit in ms (default 10000).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotSetScroll {
    /// A ScrollView's element id.
    pub element_id: u64,
    /// Content offset in logical px.
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotNavId {
    /// From `list_navigators`.
    pub nav_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotNavigate {
    /// From `list_navigators`.
    pub nav_id: u64,
    /// `push`, `replace`, `reset` or `pop`.
    pub action: String,
    /// Resolved like a deep link (ignored for `pop`).
    #[serde(default)]
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotSignalRef {
    /// Registered signal name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Or its id from `list_watched_signals`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotWriteSignal {
    /// Registered signal name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    /// New value as JSON.
    pub value: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotInstanceId {
    /// From `list_component_instances`.
    pub instance_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotRawCall {
    /// Bridge verb name.
    pub verb: String,
    /// Verb arguments.
    #[serde(default)]
    pub args: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotAppOnly {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotScreenshotArgs {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// Replay capture width, physical px.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// Replay capture height, physical px.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// `auto`; web `dom`/`tab`/`cdp`; runtime-server `client`/`replay`. Which to use: get_docs("robot").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Args for `run_dev` — launch an `idealyst dev` session detached and
/// tracked. Mirrors the subset of `idealyst dev` CLI flags worth
/// driving from MCP: target platforms, the `--local` mode toggle, and
/// the robot knobs.
#[derive(Debug, Default, serde::Deserialize, schemars::JsonSchema)]
pub struct RunDevArgs {
    /// Project dir (default: the server's cwd).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    /// `web`, `ios`, `android`, `macos`, `terminal`, `linux`, `windows` (default: manifest targets).
    #[serde(default)]
    pub platforms: Vec<String>,
    /// Every platform the host can build.
    #[serde(default)]
    pub all: bool,
    /// Native build + file-watch rebuilds instead of hot reload (required for web Robot).
    #[serde(default)]
    pub local: bool,
    /// Force a headless Chromium for web.
    #[serde(default)]
    pub headless_client: bool,
    /// Disable the Robot bridge.
    #[serde(default)]
    pub no_robot: bool,
    /// Fixed Robot bridge port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge_port: Option<u16>,
    /// Where screenshots are saved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot_dir: Option<String>,
    /// Web: serve without the initial build.
    #[serde(default)]
    pub no_build: bool,
    /// Queue saves instead of applying them; `rebuild_dev` applies the queue.
    #[serde(default)]
    pub no_watch: bool,
    /// Web `--local`: hot patch saves that only add items (experimental).
    #[serde(default)]
    pub experimental_build: bool,
}

/// Args for `stop_dev` — stop one tracked dev session by id, or all of
/// them.
#[derive(Debug, Default, serde::Deserialize, schemars::JsonSchema)]
pub struct StopDevArgs {
    /// Session to stop, by the id `run_dev` gave it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<u64>,
    /// Session to stop, by process id (from `list_dev_sessions`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Stop every session this server started (with `include_external`, every session).
    #[serde(default)]
    pub all: bool,
    /// Allow stopping a session a person started, not this server.
    #[serde(default)]
    pub include_external: bool,
}

/// Which session a tool acts on: the id `run_dev` gave it, or its process
/// id (any session `list_dev_sessions` shows).
#[derive(Debug, Default, serde::Deserialize, schemars::JsonSchema)]
pub struct DevSessionArgs {
    /// From `run_dev`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<u64>,
    /// From `list_dev_sessions`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

/// Args for `read_dev_log` — tail a tracked dev session's log file.
#[derive(Debug, Default, serde::Deserialize, schemars::JsonSchema)]
pub struct ReadDevLogArgs {
    /// From `run_dev`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<u64>,
    /// From `list_dev_sessions` (any session, a person's included).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Lines from the end (default 100, max 2000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<usize>,
    /// Case-insensitive substring, applied before the tail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
}

/// Args for `wait_for_app` — block until a running app registers.
#[derive(Debug, Default, serde::Deserialize, schemars::JsonSchema)]
pub struct WaitForAppArgs {
    /// App name (default: any app).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Default 60, max 600.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// Drop the `app` field from a JSON object so the bridge command
/// payload is exactly what the wire protocol expects (the bridge
/// doesn't know about app routing — that's MCP-server concern).
fn strip_app(mut v: serde_json::Value) -> serde_json::Value {
    if let Some(obj) = v.as_object_mut() {
        obj.remove("app");
    }
    v
}

/// Make sure a `screenshot` response carries a `path` to a saved PNG.
///
/// The dev relay already decodes `png_base64` to `~/.idealyst/screenshots/`
/// and injects `path` in the default `idealyst dev` flow, so the common case
/// is a no-op. For a direct-bridge (`--local`) session there's no relay to do
/// it, so we decode + write the file here. Best-effort: any failure leaves the
/// response untouched (the caller still strips `png_base64` either way).
fn ensure_screenshot_saved(label: &str, value: &mut serde_json::Value) {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let dir = std::path::Path::new(&home).join(".idealyst").join("screenshots");
    ensure_screenshot_saved_in(&dir, label, value)
}

/// Inner half of [`ensure_screenshot_saved`] with the target directory
/// explicit. Split out so the unit test can pass a temp dir instead of
/// repointing `HOME` — process-wide env mutation raced parallel tests
/// that resolve `~/.idealyst` (the dev_runner log-glob test flaked).
fn ensure_screenshot_saved_in(
    dir: &std::path::Path,
    label: &str,
    value: &mut serde_json::Value,
) {
    use base64::Engine as _;
    let Some(obj) = value.as_object_mut() else {
        return;
    };
    if obj.get("path").and_then(|p| p.as_str()).is_some() {
        return; // relay (or client) already saved it
    }
    let Some(b64) = obj.get("png_base64").and_then(|v| v.as_str()) else {
        return;
    };
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) else {
        return;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = dir.join(format!("{label}-{millis}.png"));
    if std::fs::write(&path, &bytes).is_ok() {
        obj.insert("path".into(), json!(path.to_string_lossy()));
    }
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SearchRequest {
    /// Words to match (OR'd, ranked by distinct hits).
    pub query: String,
    /// Accepted for symmetry with the other catalog tools but not yet
    /// applied, so it is kept out of the advertised schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    #[allow(dead_code)]
    pub app: Option<String>,
}

/// Shared optional filter for the `list_*` catalog tools. Lets the LLM
/// narrow a large catalog (a project pulling in idea-ui surfaces 40+
/// components) down to the handful it cares about before drilling into
/// one with the matching `describe_*` tool — the intended top-down flow.
#[derive(Debug, Default, serde::Deserialize, schemars::JsonSchema)]
pub struct FilterRequest {
    /// Case-insensitive terms, AND'd; `*` globs.
    #[serde(default)]
    pub filter: Option<String>,
    /// Restrict to one crate (`my-app` or `my_app`).
    #[serde(default)]
    pub app: Option<String>,
}

/// Args for `catalog_status`.
#[derive(Debug, Default, serde::Deserialize, schemars::JsonSchema)]
pub struct CatalogStatusRequest {
    /// Block until the catalog is current.
    #[serde(default)]
    pub wait_for_current: Option<bool>,
    /// Wait limit in seconds (default 120, max 600).
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
}

/// Args for `get_docs`.
#[derive(Debug, Default, serde::Deserialize, schemars::JsonSchema)]
pub struct GetDocsRequest {
    /// Tool or topic name; omit for the index.
    #[serde(default)]
    pub name: Option<String>,
    /// Keep only the sections matching any of these words.
    #[serde(default)]
    pub query: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SlugRequest {
    /// Guide slug (e.g. `getting-started`).
    pub slug: String,
}

/// Args for `describe_icon_set` — a pack name plus a pagination window
/// over its (name-sorted) icon list. An icon pack has ~1600 icons, so the
/// full list is never dumped at once; the caller pages or, better, uses
/// `search_icons`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DescribeIconSetRequest {
    /// Pack: `icons-lucide`, `Lucide`, or `lucide`.
    pub name: String,
    /// Default 0.
    #[serde(default)]
    pub offset: Option<usize>,
    /// Default 100, max 500.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Args for `search_icons` — a name keyword, an optional pack restriction,
/// and a result cap.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct IconSearchRequest {
    /// Name substring (`-`, `_` and space are interchangeable).
    pub query: String,
    /// Restrict to one pack.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set: Option<String>,
    /// Default 50, max 200.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Args for `lint_project` — run the idealyst source linter over a
/// project's un-expanded Rust source.
#[derive(Debug, Default, serde::Deserialize, schemars::JsonSchema)]
pub struct LintRequest {
    /// File or dir (default: the live app's root, else the server's cwd).
    #[serde(default)]
    pub path: Option<String>,
    /// Fail on warnings too.
    #[serde(default)]
    pub deny_warnings: Option<bool>,
    /// Max diagnostics returned (default 200, max 1000).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// One service instruction for `configure_devcontainer`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ConfigureServiceArg {
    /// `database`, `redis`, `minio`, `playwright`, `claude`, `codex`, or `idealyst-cli`.
    pub id: String,
    /// `postgres`/`mysql` (database); `host`/`volume` (claude, codex).
    #[serde(default)]
    pub variant: Option<String>,
    /// `enable` (default), `reconfigure`, or `remove`.
    #[serde(default)]
    pub action: Option<String>,
}

/// Args for `configure_devcontainer` — initialize or update a project's
/// devcontainer + its idealyst-managed sidecar services.
#[derive(Debug, Default, serde::Deserialize, schemars::JsonSchema)]
pub struct ConfigureDevcontainerRequest {
    /// Project dir (default: the live app's root, else the server's cwd).
    #[serde(default)]
    pub dir: Option<String>,
    /// Add-ons to change; empty just ensures a base devcontainer.
    #[serde(default)]
    pub services: Vec<ConfigureServiceArg>,
    /// Named config under `.devcontainer/<config>/`.
    #[serde(default)]
    pub config: Option<String>,
}

/// One aspect instruction for `configure_vscode`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ConfigureVscodeAspectArg {
    /// `extensions` or `lint`.
    pub id: String,
    /// `enable` (default) or `remove`.
    #[serde(default)]
    pub action: Option<String>,
}

/// Args for `configure_vscode` — configure a project's `.vscode/` settings +
/// extension recommendations.
#[derive(Debug, Default, serde::Deserialize, schemars::JsonSchema)]
pub struct ConfigureVscodeRequest {
    /// Project dir (default: the live app's root, else the server's cwd).
    #[serde(default)]
    pub dir: Option<String>,
    /// Omit to enable all.
    #[serde(default)]
    pub aspects: Option<Vec<ConfigureVscodeAspectArg>>,
    /// Remove everything idealyst manages.
    #[serde(default)]
    pub remove_all: Option<bool>,
}

/// How long a session a person started gets to exit on SIGINT before
/// `stop_dev` kills it: its teardown stops simulators and servers, which
/// takes longer than this server's own sessions are given.
const EXTERNAL_STOP_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

impl CatalogService {
    /// The running, registered session a tool names: by the id `run_dev`
    /// gave it, or by process id.
    fn resolve_session(&self, session_id: Option<u64>, pid: Option<u32>) -> Result<crate::dev_sessions::Live, String> {
        let pid = match (session_id, pid) {
            (_, Some(pid)) => pid,
            (Some(id), None) => self
                .dev_runner
                .pid_of(id)
                .ok_or_else(|| format!("no running dev session with id {id} — see list_dev_sessions"))?,
            (None, None) => return Err("name the session: `session_id` (from run_dev) or `pid` (from list_dev_sessions)".into()),
        };
        crate::dev_sessions::find(pid).ok_or_else(|| {
            format!(
                "pid {pid} has no control endpoint: it is not a running `idealyst dev`, or it is still \
                 starting (its endpoint comes up with its first output), or its `idealyst` predates \
                 the session registry (CLI 1.18). read_dev_log still works for this server's sessions."
            )
        })
    }
}

#[tool_router]
impl CatalogService {
    /// Default constructor: Robot discovery on (scans
    /// `~/.idealyst/apps/`). Equivalent to `with_robot_mode(true, None)`.
    pub fn new() -> Self {
        Self::with_robot_mode(true, None)
    }

    /// Construct with an explicit robot policy.
    ///
    /// - `enabled == false` → [`RobotMode::Disabled`]: no discovery
    ///   thread, no bridge contact anywhere (catalog OR control).
    /// - `explicit_addr == Some(host:port)` → [`RobotMode::Explicit`]:
    ///   pin that one bridge, skip discovery.
    /// - otherwise → [`RobotMode::Discovery`]: start the
    ///   `~/.idealyst/apps/` scanner thread and route per call.
    pub fn with_robot_mode(enabled: bool, explicit_addr: Option<String>) -> Self {
        let (robot_mode, robot, discovery) = if !enabled {
            // Inert table — `DiscoveryTable::default()` never spawns the
            // scanner thread, so "disabled" really does nothing.
            (RobotMode::Disabled, None, crate::app_discovery::DiscoveryTable::default())
        } else if let Some(addr) = explicit_addr {
            (
                RobotMode::Explicit { addr: addr.clone() },
                Some(Arc::new(RobotBridge::new(addr))),
                crate::app_discovery::DiscoveryTable::default(),
            )
        } else {
            (RobotMode::Discovery, None, crate::app_discovery::start())
        };
        let (status_tx, _) = tokio::sync::watch::channel(0u64);
        Self {
            catalog: Arc::new(RwLock::new(ResolvedCatalog::build())),
            status: Arc::new(RwLock::new(crate::catalog_status::CatalogStatus::default())),
            status_tx: Arc::new(status_tx),
            robot_mode,
            robot,
            resolver: Arc::new(RobotResolver::default()),
            catalog_cache: Arc::new(CatalogCache::default()),
            discovery,
            dev_runner: Arc::new(crate::dev_runner::DevRunner::new()),
            tool_router: Self::advertised_router(),
        }
    }

    /// The macro-generated router with every input schema minified. The
    /// schemas are what clients load into context, so the noise schemars
    /// emits (`$schema`, `null` unions, integer widths) is stripped once
    /// here rather than paid on every request.
    fn advertised_router() -> ToolRouter<Self> {
        let mut router = Self::tool_router();
        for route in router.map.values_mut() {
            crate::schema_minify::minify(Arc::make_mut(&mut route.attr.input_schema));
        }
        router
    }

    /// `get_docs`: the index, a topic, or a tool's page, optionally
    /// narrowed by `query`.
    fn docs_result(&self, req: GetDocsRequest) -> CallToolResult {
        let query = req.query.as_deref().filter(|q| !q.trim().is_empty());
        let name = req.name.as_deref().map(str::trim).filter(|n| !n.is_empty());
        let tools = self.tool_router.list_all();
        let text = match name {
            None => match query {
                None => crate::tool_docs::render_index(&tools),
                Some(q) => crate::tool_docs::render_search_all(q),
            },
            Some(n) => {
                if let Some(topic) = crate::tool_docs::topic(n) {
                    crate::tool_docs::render_topic(topic, query)
                } else if let Some(tool) = tools.iter().find(|t| t.name == n) {
                    crate::tool_docs::render_tool(tool, query)
                } else {
                    let needle = n.to_lowercase();
                    let close: Vec<&str> = crate::tool_docs::TOPICS
                        .iter()
                        .map(|t| t.name)
                        .chain(tools.iter().map(|t| t.name.as_ref()))
                        .filter(|c| c.contains(&needle) || needle.contains(*c))
                        .collect();
                    let hint = if close.is_empty() {
                        "Call get_docs() for the index.".to_string()
                    } else {
                        format!("Did you mean: {}?", close.join(", "))
                    };
                    return CallToolResult::error(vec![Content::text(format!(
                        "No tool or topic named `{n}`. {hint}"
                    ))]);
                }
            }
        };
        CallToolResult::success(vec![Content::text(text)])
    }

    /// Pin a single Robot bridge (legacy mode). Pre-registry usage:
    /// the server uses this bridge for every Robot tool call,
    /// ignoring the `app` arg. New code should leave this unset
    /// and let the registry route per call.
    pub fn with_robot_bridge(mut self, bridge: Arc<RobotBridge>) -> Self {
        self.robot = Some(bridge);
        self
    }

    /// The set of apps the catalog/robot tools should consider live,
    /// honoring the robot mode:
    /// - Disabled → none (no bridge is ever contacted).
    /// - Discovery → whatever the `~/.idealyst/apps/` scanner found.
    /// - Explicit → a single synthetic entry for the pinned address.
    fn live_apps(&self) -> Vec<crate::app_discovery::DiscoveredApp> {
        match &self.robot_mode {
            RobotMode::Disabled => Vec::new(),
            RobotMode::Discovery => self.discovery.snapshot(),
            RobotMode::Explicit { addr } => vec![crate::app_discovery::DiscoveredApp {
                name: "app".to_string(),
                bundle_id: None,
                project_root: None,
                catalog_bin: None,
                pid: 0,
                bridge_addr: addr.clone(),
                platform: None,
            }],
        }
    }

    /// Swap in a freshly-rebuilt catalog. Used by the file-watch
    /// thread (`crate::watch`). After replacing, the service should send
    /// `notifications/resources/list_changed` via the rmcp peer.
    pub async fn replace_catalog(&self, new_cat: ResolvedCatalog) {
        {
            let mut guard = self.catalog.write().await;
            *guard = new_cat;
        }
        self.mark_build_succeeded(None).await;
    }

    /// Wrap catalog-derived tool output with a freshness marker when the
    /// catalog is not current.
    ///
    /// The marker rides on the response a caller was already going to
    /// read, because a caller only thinks to ask `catalog_status` AFTER
    /// something looks wrong — which is too late to stop them acting on
    /// stale props. Silent in the happy path, so it never becomes noise.
    /// The marker is a SEPARATE content block rather than a prefix on the
    /// payload: a reader sees it first either way, but the JSON block
    /// stays independently parseable for anything that consumes these
    /// results programmatically.
    async fn catalog_text(&self, body: String) -> CallToolResult {
        match self.status().await.marker() {
            Some(marker) => {
                CallToolResult::success(vec![Content::text(marker), Content::text(body)])
            }
            None => CallToolResult::success(vec![Content::text(body)]),
        }
    }

    /// Snapshot the current freshness.
    pub async fn status(&self) -> crate::catalog_status::CatalogStatus {
        self.status.read().await.clone()
    }

    /// Publish the current generation so any `catalog_status` waiter
    /// re-evaluates. Cheap and lossless for our purpose: waiters check
    /// the status itself after each tick, so a coalesced notification
    /// still wakes them.
    async fn publish_status(&self) {
        let generation = self.status.read().await.generation;
        let _ = self.status_tx.send(generation);
    }

    /// No catalog exists yet and the first extraction is about to run.
    ///
    /// Called SYNCHRONOUSLY before stdio is bound, so the very first
    /// `tools/call` already reports `initializing` instead of racing the
    /// background task that sets it.
    pub async fn mark_initializing(&self) {
        {
            let mut st = self.status.write().await;
            st.state = crate::catalog_status::CatalogState::Initializing;
        }
        self.publish_status().await;
    }

    /// A rebuild has started. The catalog keeps serving throughout.
    pub async fn mark_build_started(&self) {
        {
            let mut st = self.status.write().await;
            st.rebuild_started_at = Some(std::time::SystemTime::now());
        }
        self.publish_status().await;
    }

    /// A rebuild produced a catalog. Clears any previous failure — a
    /// successful build supersedes it entirely.
    pub async fn mark_build_succeeded(&self, took: Option<std::time::Duration>) {
        {
            let mut st = self.status.write().await;
            st.state = crate::catalog_status::CatalogState::Current;
            st.built_at = Some(std::time::SystemTime::now());
            st.last_attempt_at = Some(std::time::SystemTime::now());
            if took.is_some() {
                st.build_duration = took;
            }
            st.rebuild_started_at = None;
            st.last_error = None;
            st.reason = None;
            st.generation = st.generation.saturating_add(1);
        }
        self.publish_status().await;
    }

    /// A rebuild failed. The previous catalog stays in place — that is
    /// the point — but it is now marked stale so callers are told the
    /// data predates whatever broke the build.
    pub async fn mark_build_failed(&self, error: impl Into<String>) {
        {
            let mut st = self.status.write().await;
            let error = error.into();
            // A first build that never succeeded leaves nothing to serve;
            // say so rather than calling an empty catalog "stale".
            st.state = if st.built_at.is_some() {
                crate::catalog_status::CatalogState::Stale
            } else {
                st.reason = Some(error.clone());
                crate::catalog_status::CatalogState::Unavailable
            };
            st.last_error = Some(trim_error(&error));
            st.last_attempt_at = Some(std::time::SystemTime::now());
            st.rebuild_started_at = None;
        }
        self.publish_status().await;
    }

    /// No catalog can be produced at all, and no build is running — a
    /// misconfigured project root, say. Distinct from an empty catalog
    /// that built fine, which is the distinction this whole type exists
    /// to preserve.
    pub async fn mark_unavailable(&self, reason: impl Into<String>) {
        {
            let mut st = self.status.write().await;
            st.state = crate::catalog_status::CatalogState::Unavailable;
            st.reason = Some(reason.into());
            st.rebuild_started_at = None;
        }
        self.publish_status().await;
    }

    /// Wait until the catalog is settled and current, or `timeout`
    /// elapses. Returns the status either way — a timeout is an answer
    /// ("still building after Ns"), not an error.
    pub async fn wait_for_current(
        &self,
        timeout: std::time::Duration,
    ) -> crate::catalog_status::CatalogStatus {
        let mut rx = self.status_tx.subscribe();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let st = self.status().await;
            if st.is_settled_current() {
                return st;
            }
            if tokio::time::timeout_at(deadline, rx.changed()).await.is_err() {
                return self.status().await;
            }
        }
    }

    /// Re-extract the catalog from the *current process's* inventory
    /// slice. Useful when the consumer has refreshed inventory state
    /// some other way; in the typical phase-5 deployment a separate
    /// subprocess extracts and JSON-pipes the catalog, which the
    /// watcher feeds into [`replace_catalog`] instead.
    pub async fn reload_from_inventory(&self) {
        self.replace_catalog(ResolvedCatalog::build()).await;
    }

    #[tool(description = "List the project's components ({ app, name, fqn, summary }); narrow with `filter`, then `describe_component`.")]
    async fn list_components(
        &self,
        Parameters(req): Parameters<FilterRequest>,
    ) -> Result<CallToolResult, McpError> {
        let mut json: Vec<serde_json::Value> = Vec::new();

        let live = self.live_apps();

        for app in &live {
            // The `app` filter narrows to one app here just as it does on
            // the static path below. Applying it to only one of the two
            // arms — as this did — meant the filter silently did nothing
            // whenever a live app was registered.
            if !app_matches(req.app.as_deref(), &app.name) {
                continue;
            }
            // Live path: hit the Robot bridge's `get_catalog`.
            let bridge = RobotBridge::new(app.bridge_addr.clone());
            let cat = match self.catalog_cache.get_via_bridge(&bridge).await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(
                        "live catalog fetch for {:?} failed ({}); skipping",
                        app.name,
                        e
                    );
                    continue;
                }
            };
            let mut sorted: Vec<&ComponentEntry> = cat.entries().to_vec();
            sorted.sort_by_key(|e| (e.module_path, e.name));
            for e in sorted {
                let fqn = format!("{}::{}::{}", app.name, e.module_path, e.name);
                if !matches_filter(&req.filter, &[e.name, &fqn]) {
                    continue;
                }
                json.push(serde_json::json!({
                    "app": app.name,
                    "name": e.name,
                    "fqn": fqn,
                    "summary": doc_summary(e.docs),
                }));
            }
        }

        if json.is_empty() {
            // No live apps — serve the in-process catalog (set up at
            // server startup by the project extractor, or empty if the
            // server was started with no project).
            let cat = self.catalog.read().await;
            let mut sorted: Vec<&ComponentEntry> = cat.entries().to_vec();
            sorted.sort_by_key(|e| (e.module_path, e.name));
            for e in sorted {
                let fqn = format!("{}::{}", e.module_path, e.name);
                if !matches_filter(&req.filter, &[e.name, &fqn]) {
                    continue;
                }
                // Provenance: which crate this component came from. One
                // catalog can span several projects (the extractor links
                // them all), so this is what tells two apps' components
                // apart.
                let origin = mcp_catalog::origin_crate(e.module_path);
                if !app_matches(req.app.as_deref(), origin) {
                    continue;
                }
                json.push(serde_json::json!({
                    "app": origin,
                    "name": e.name,
                    "fqn": fqn,
                    "summary": doc_summary(e.docs),
                }));
            }
        } else {
            json.sort_by(|a, b| {
                let ka = (a["app"].as_str().unwrap_or(""), a["fqn"].as_str().unwrap_or(""));
                let kb = (b["app"].as_str().unwrap_or(""), b["fqn"].as_str().unwrap_or(""));
                ka.cmp(&kb)
            });
        }

        Ok(self
            .catalog_text(serde_json::to_string_pretty(&json).unwrap())
            .await)
    }

    #[tool(description = "One component in full: docs, props, file/line, and the components it composes.")]
    async fn describe_component(
        &self,
        Parameters(req): Parameters<NameRequest>,
    ) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let live: Vec<String> = self.live_apps().into_iter().map(|a| a.name).collect();
        let entry = resolve_component(&cat, &req.name, req.app.as_deref(), &live)?;
        let edges = cat.dependencies(&EntryRef::of(entry));
        let json = entry_to_json(&cat, entry, edges);
        Ok(self
            .catalog_text(serde_json::to_string_pretty(&json).unwrap())
            .await)
    }

    #[tool(description = "Which components compose the given one (reverse edges).")]
    async fn find_uses(
        &self,
        Parameters(req): Parameters<NameRequest>,
    ) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let live: Vec<String> = self.live_apps().into_iter().map(|a| a.name).collect();
        let entry = resolve_component(&cat, &req.name, req.app.as_deref(), &live)?;
        let users = cat.uses(&EntryRef::of(entry));
        let json: Vec<String> = users.iter().map(|r| r.fqn()).collect();
        Ok(self
            .catalog_text(serde_json::to_string_pretty(&json).unwrap())
            .await)
    }

    #[tool(description = "Which components the given one composes, with resolution status (forward edges).")]
    async fn find_dependencies(
        &self,
        Parameters(req): Parameters<NameRequest>,
    ) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let live: Vec<String> = self.live_apps().into_iter().map(|a| a.name).collect();
        let entry = resolve_component(&cat, &req.name, req.app.as_deref(), &live)?;
        let edges = cat.dependencies(&EntryRef::of(entry));
        let json: Vec<serde_json::Value> = edges
            .iter()
            .map(|e| {
                let (status, resolved) = match &e.status {
                    EdgeStatus::Resolved { target } => ("resolved", Some(target.fqn())),
                    EdgeStatus::NoMatch => ("unresolved", None),
                    EdgeStatus::Ambiguous { .. } => ("ambiguous", None),
                };
                serde_json::json!({
                    "raw_name": e.raw_name,
                    "line": e.line,
                    "status": status,
                    "resolved_fqn": resolved,
                })
            })
            .collect();
        Ok(self
            .catalog_text(serde_json::to_string_pretty(&json).unwrap())
            .await)
    }

    #[tool(description = "List `#[idealyst_tool]` functions; then `describe_tool`.")]
    async fn list_tools(
        &self,
        Parameters(req): Parameters<FilterRequest>,
    ) -> Result<CallToolResult, McpError> {
        let mut sorted: Vec<&mcp_catalog::ToolEntry> = mcp_catalog::tools().collect();
        sorted.sort_by_key(|t| (t.module_path, t.name));
        let json: Vec<serde_json::Value> = sorted
            .iter()
            .filter_map(|t| {
                let fqn = format!("{}::{}", t.module_path, t.name);
                if !matches_filter(&req.filter, &[t.name, &fqn]) {
                    return None;
                }
                Some(serde_json::json!({
                    "name": t.name,
                    "fqn": fqn,
                    "return_type": t.return_type,
                    "summary": doc_summary(t.docs),
                }))
            })
            .collect();
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json).unwrap(),
        )]))
    }

    #[tool(description = "One `#[idealyst_tool]` function: docs, params, return type.")]
    async fn describe_tool(
        &self,
        Parameters(req): Parameters<NameRequest>,
    ) -> Result<CallToolResult, McpError> {
        let entry = mcp_catalog::tools()
            .find(|t| t.name == req.name || format!("{}::{}", t.module_path, t.name) == req.name)
            .ok_or_else(|| {
                McpError::invalid_params(format!("tool {:?} not found", req.name), None)
            })?;
        let params: Vec<serde_json::Value> = entry
            .params
            .iter()
            .map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "type": p.type_str,
                    "type_short_name": p.type_short_name,
                })
            })
            .collect();
        let json = serde_json::json!({
            "name": entry.name,
            "module_path": entry.module_path,
            "fqn": format!("{}::{}", entry.module_path, entry.name),
            "file": entry.file,
            "line": entry.line,
            "docs": entry.docs,
            "params": params,
            "return_type": entry.return_type,
        });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json).unwrap(),
        )]))
    }

    // -------------------------------------------------------------
    // Robot tools — these proxy to the running app's Robot bridge
    // (see `robot_bridge.rs`). Available only when the server was
    // constructed via `with_robot_bridge`; otherwise every call
    // returns "robot tools disabled" and the catalog tools keep
    // working.

    #[tool(description = "Find the first element in a running app matching test_id, label, or kind.")]
    async fn find_element(
        &self,
        Parameters(args): Parameters<RobotFindArgs>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("find_element", app.as_deref(), body).await
    }

    #[tool(description = "Find every element in a running app matching test_id, label, or kind.")]
    async fn find_all_elements(
        &self,
        Parameters(args): Parameters<RobotFindArgs>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("find_all_elements", app.as_deref(), body).await
    }

    #[tool(description = "Press an element through the framework's handlers (no hit testing); see `tap_native` for real input.")]
    async fn click(
        &self,
        Parameters(args): Parameters<RobotElementId>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("click", app.as_deref(), body).await
    }

    #[tool(description = "Drag a pointer along a path over an element (gestures, sliders, drag-and-drop).")]
    async fn drag(
        &self,
        Parameters(args): Parameters<RobotDrag>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("drag", app.as_deref(), body).await
    }

    #[tool(description = "Replace a TextInput's text.")]
    async fn type_text(
        &self,
        Parameters(args): Parameters<RobotTypeText>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("type_text", app.as_deref(), body).await
    }

    #[tool(description = "Set a Toggle on or off.")]
    async fn set_toggle(
        &self,
        Parameters(args): Parameters<RobotSetToggle>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("set_toggle", app.as_deref(), body).await
    }

    #[tool(description = "Set a Slider's value.")]
    async fn set_slider(
        &self,
        Parameters(args): Parameters<RobotSetSlider>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("set_slider", app.as_deref(), body).await
    }

    #[tool(description = "Focus an element.")]
    async fn focus(
        &self,
        Parameters(args): Parameters<RobotElementId>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("focus", app.as_deref(), body).await
    }

    #[tool(description = "Blur an element.")]
    async fn blur(
        &self,
        Parameters(args): Parameters<RobotElementId>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("blur", app.as_deref(), body).await
    }

    #[tool(description = "The running app's whole element tree.")]
    async fn get_snapshot(
        &self,
        Parameters(args): Parameters<RobotAppOnly>,
    ) -> Result<CallToolResult, McpError> {
        self.robot_call("get_snapshot", args.app.as_deref(), json!({})).await
    }

    #[tool(description = "An element's children.")]
    async fn get_children(
        &self,
        Parameters(args): Parameters<RobotElementId>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("get_children", app.as_deref(), body).await
    }

    #[tool(description = "An element's parent.")]
    async fn get_parent(
        &self,
        Parameters(args): Parameters<RobotElementId>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("get_parent", app.as_deref(), body).await
    }

    #[tool(description = "Count elements in a running app matching test_id, label, or kind.")]
    async fn count_elements(
        &self,
        Parameters(args): Parameters<RobotFindArgs>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("count_elements", app.as_deref(), body).await
    }

    #[tool(description = "The running app's captured logs.")]
    async fn get_logs(
        &self,
        Parameters(args): Parameters<RobotAppOnly>,
    ) -> Result<CallToolResult, McpError> {
        self.robot_call("get_logs", args.app.as_deref(), json!({})).await
    }

    #[tool(description = "Clear the running app's captured logs.")]
    async fn clear_logs(
        &self,
        Parameters(args): Parameters<RobotAppOnly>,
    ) -> Result<CallToolResult, McpError> {
        self.robot_call("clear_logs", args.app.as_deref(), json!({})).await
    }

    #[tool(description = "An element's layout frame relative to its parent.")]
    async fn get_frame(
        &self,
        Parameters(args): Parameters<RobotElementId>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("get_frame", app.as_deref(), body).await
    }

    #[tool(description = "An element's layout frame in window coordinates.")]
    async fn get_absolute_frame(
        &self,
        Parameters(args): Parameters<RobotElementId>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("get_absolute_frame", app.as_deref(), body).await
    }

    #[tool(description = "An element's render state as the platform reports it (native class, frame, colors, fonts).")]
    async fn introspect_native(
        &self,
        Parameters(args): Parameters<RobotElementId>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("introspect_native", app.as_deref(), body).await
    }

    #[tool(description = "Diff the native render (geometry and visual props) of two running apps to find cross-platform divergences.")]
    async fn compare_native_parity(
        &self,
        Parameters(args): Parameters<RobotParityArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.do_compare_native_parity(args).await
    }

    #[tool(description = "Real OS-level tap on an Android device/emulator via adb.")]
    async fn tap(
        &self,
        Parameters(args): Parameters<RobotTap>,
    ) -> Result<CallToolResult, McpError> {
        let msg = self.do_tap(args).await?;
        Ok(CallToolResult::success(vec![Content::text(msg)]))
    }

    #[tool(description = "Tap an element with real platform input, through hit testing and overlays.")]
    async fn tap_native(
        &self,
        Parameters(args): Parameters<RobotElementId>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("tap_native", app.as_deref(), body).await
    }

    #[tool(description = "Drag with real platform input along a path.")]
    async fn drag_native(
        &self,
        Parameters(args): Parameters<RobotDragNative>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("drag_native", app.as_deref(), body).await
    }

    #[tool(description = "Hover, press (and hold) or release on an element with real platform input.")]
    async fn pointer_native(
        &self,
        Parameters(args): Parameters<RobotPointerNative>,
    ) -> Result<CallToolResult, McpError> {
        let verb = match args.action {
            PointerNativeAction::Hover => "hover_native",
            PointerNativeAction::Press => "press_native",
            PointerNativeAction::Release => "release_native",
        };
        self.robot_call(verb, args.app.as_deref(), json!({ "element_id": args.element_id })).await
    }

    #[tool(description = "An element's user-visible state: value, checked, disabled, focused, hovered, pressed.")]
    async fn get_state(
        &self,
        Parameters(args): Parameters<RobotElementId>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("get_state", app.as_deref(), body).await
    }

    #[tool(description = "Scroll a ScrollView to an offset.")]
    async fn set_scroll(
        &self,
        Parameters(args): Parameters<RobotSetScroll>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("set_scroll", app.as_deref(), body).await
    }

    #[tool(description = "List the running app's navigators with their routes and stacks.")]
    async fn list_navigators(
        &self,
        Parameters(args): Parameters<RobotAppOnly>,
    ) -> Result<CallToolResult, McpError> {
        self.robot_call("list_navigators", args.app.as_deref(), json!({})).await
    }

    #[tool(description = "One navigator's routes and stack.")]
    async fn get_navigator_state(
        &self,
        Parameters(args): Parameters<RobotNavId>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("get_navigator_state", app.as_deref(), body).await
    }

    #[tool(description = "Push, replace, reset, or pop a navigator to a path.")]
    async fn navigate(
        &self,
        Parameters(args): Parameters<RobotNavigate>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("navigate", app.as_deref(), body).await
    }

    #[tool(description = "List the signals the app exposes by name, with values.")]
    async fn list_watched_signals(
        &self,
        Parameters(args): Parameters<RobotAppOnly>,
    ) -> Result<CallToolResult, McpError> {
        self.robot_call("list_watched_signals", args.app.as_deref(), json!({})).await
    }

    #[tool(description = "Read a watched signal's value.")]
    async fn read_signal(
        &self,
        Parameters(args): Parameters<RobotSignalRef>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("read_signal", app.as_deref(), body).await
    }

    #[tool(description = "Write a writable watched signal.")]
    async fn write_signal(
        &self,
        Parameters(args): Parameters<RobotWriteSignal>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("write_signal", app.as_deref(), body).await
    }

    #[tool(description = "List the component instances mounted in the running app.")]
    async fn list_component_instances(
        &self,
        Parameters(args): Parameters<RobotAppOnly>,
    ) -> Result<CallToolResult, McpError> {
        self.robot_call("list_components", args.app.as_deref(), json!({})).await
    }

    #[tool(description = "One mounted component instance with its live props.")]
    async fn get_component_instance(
        &self,
        Parameters(args): Parameters<RobotInstanceId>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("get_component", app.as_deref(), body).await
    }

    #[tool(description = "Deliver a deep or universal link to the running app.")]
    async fn open_link(
        &self,
        Parameters(args): Parameters<RobotOpenLink>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("open_link", app.as_deref(), body).await
    }

    #[tool(description = "List test functions the app registered (`#[robot_fn]`, SDK controls like `location.set`).")]
    async fn list_functions(
        &self,
        Parameters(args): Parameters<RobotAppOnly>,
    ) -> Result<CallToolResult, McpError> {
        self.robot_call("list_functions", args.app.as_deref(), json!({})).await
    }

    #[tool(description = "Call a registered test function and return its result.")]
    async fn call_function(
        &self,
        Parameters(args): Parameters<RobotCallFunction>,
    ) -> Result<CallToolResult, McpError> {
        let bridge = self.resolve_bridge(args.app.as_deref()).await?;
        let body = json!({ "name": args.name, "args": args.args.clone().unwrap_or_else(|| json!({})) });
        let text = |v: &serde_json::Value| serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string());
        let first = match bridge.call("call_function", body).await {
            Ok(v) => v,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        };
        if first["done"] == true {
            return Ok(CallToolResult::success(vec![Content::text(text(&first["value"]))]));
        }
        let Some(call) = first["call"].as_u64() else {
            return Ok(CallToolResult::error(vec![Content::text(format!("call_function: unexpected reply {first}"))]));
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(args.timeout_ms.unwrap_or(10_000));
        loop {
            match bridge.call("poll_function", json!({ "call": call })).await {
                Ok(v) if v["done"] == true => {
                    return Ok(CallToolResult::success(vec![Content::text(text(&v["value"]))]))
                }
                Ok(_) if std::time::Instant::now() >= deadline => {
                    return Ok(CallToolResult::error(vec![Content::text(format!(
                        "function '{}' is still running after {} ms (call {call}; `call_verb` poll_function {{call}} collects it later)",
                        args.name,
                        args.timeout_ms.unwrap_or(10_000)
                    ))]))
                }
                Ok(_) => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
                Err(e) => return Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
            }
        }
    }

    #[tool(description = "Call any Robot bridge verb by name (for verbs without a dedicated tool).")]
    async fn call_verb(
        &self,
        Parameters(args): Parameters<RobotRawCall>,
    ) -> Result<CallToolResult, McpError> {
        self.robot_call(&args.verb, args.app.as_deref(), args.args).await
    }

    #[tool(description = "Call a `#[method]` on a mounted component instance.")]
    async fn invoke_method(
        &self,
        Parameters(args): Parameters<RobotInvokeMethod>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("invoke_method", app.as_deref(), body).await
    }

    #[tool(description = "Save a PNG screenshot of the running app; returns its path. Web: `source` dom, tab (user shares) or cdp (headless).")]
    async fn screenshot(
        &self,
        Parameters(args): Parameters<RobotScreenshotArgs>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let label = app.clone().unwrap_or_else(|| "app".to_string());
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        let bridge = self.resolve_bridge(app.as_deref()).await?;
        match bridge.call("screenshot", body).await {
            Ok(mut value) => {
                // Never serialize `png_base64` into the tool result — a
                // full-res PNG is hundreds of KB of base64 that floods the
                // model's context (and on large screens overflows the
                // tool-result token cap entirely). Persist it to a file and
                // hand back only the path. The dev relay already saves +
                // injects `path` in the default `idealyst dev` flow; for a
                // direct-bridge (`--local`) session we save it here.
                ensure_screenshot_saved(&label, &mut value);
                if let Some(obj) = value.as_object_mut() {
                    obj.remove("png_base64");
                }
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()),
                )]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    #[tool(description = "Whether the catalog is current, rebuilding, or stale; can wait for a rebuild to land.")]
    async fn catalog_status(
        &self,
        Parameters(req): Parameters<CatalogStatusRequest>,
    ) -> Result<CallToolResult, McpError> {
        let status = if req.wait_for_current.unwrap_or(false) {
            // Bounded on purpose: an unbounded wait would sit past the
            // client's own tool timeout and fail in a way that looks like
            // a hung server rather than a slow build.
            const DEFAULT: u64 = 120;
            const CAP: u64 = 600;
            let secs = req.timeout_seconds.unwrap_or(DEFAULT).min(CAP);
            self.wait_for_current(std::time::Duration::from_secs(secs))
                .await
        } else {
            self.status().await
        };
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&status.to_json()).unwrap(),
        )]))
    }

    #[tool(description = "List running idealyst apps and the `selector` to pass as `app`.")]
    async fn list_apps(&self) -> Result<CallToolResult, McpError> {
        let live = self.live_apps();
        // `selector` is the exact string to pass as the `app` arg — `name`
        // when unique, else `name:platform` (two platforms of one app).
        let selectors = app_selectors(&live);
        let json: Vec<serde_json::Value> = live
            .iter()
            .zip(&selectors)
            .map(|(a, selector)| {
                serde_json::json!({
                    "name": a.name,
                    "platform": a.platform,
                    "selector": selector,
                    "bundle_id": a.bundle_id,
                    "project_root": a.project_root,
                    "bridge_addr": a.bridge_addr,
                    "pid": a.pid,
                })
            })
            .collect();
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json).unwrap(),
        )]))
    }

    #[tool(description = "Launch an `idealyst dev` session (one or more platforms) in the background.")]
    async fn run_dev(
        &self,
        Parameters(args): Parameters<RunDevArgs>,
    ) -> Result<CallToolResult, McpError> {
        let launch = crate::dev_runner::DevLaunch {
            dir: args.dir.map(std::path::PathBuf::from),
            platforms: args.platforms,
            all: args.all,
            local: args.local,
            no_robot: args.no_robot,
            headless_client: args.headless_client,
            bridge_port: args.bridge_port,
            screenshot_dir: args.screenshot_dir.map(std::path::PathBuf::from),
            no_build: args.no_build,
            no_watch: args.no_watch,
            experimental_build: args.experimental_build,
        };
        match self.dev_runner.start(launch) {
            Ok(info) => Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string_pretty(&info).unwrap(),
            )])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(description = "List every running dev session on this machine: ones a person started and ones this server launched.")]
    async fn list_dev_sessions(&self) -> Result<CallToolResult, McpError> {
        let mine = self.dev_runner.list();
        let live = crate::dev_sessions::live();
        let mut out: Vec<serde_json::Value> = Vec::new();
        for l in &live {
            let e = &l.entry;
            let session_id = self.dev_runner.id_of_pid(e.pid);
            out.push(json!({
                "pid": e.pid,
                "session_id": session_id,
                "started_by": if session_id.is_some() { "this server" } else if e.launched_by == "mcp" { "another idealyst mcp" } else { "a person" },
                "dir": e.dir,
                "app": e.app,
                "targets": e.targets,
                "local": e.local,
                "no_watch": e.no_watch,
                "experimental_build": e.experimental_build,
                "log_path": e.log_file,
                "uptime_secs": std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs().saturating_sub(e.started_at))
                    .unwrap_or(0),
                "control": true,
            }));
        }
        // Launched here but not registered yet (still starting), or exited.
        for s in mine.iter().filter(|s| !live.iter().any(|l| l.entry.pid == s.pid)) {
            let mut v = serde_json::to_value(s).unwrap_or_default();
            if let Some(o) = v.as_object_mut() {
                o.insert("session_id".into(), json!(s.id));
                o.insert("started_by".into(), json!("this server"));
                o.insert("control".into(), json!(false));
            }
            out.push(v);
        }
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&out).unwrap(),
        )]))
    }

    #[tool(description = "Rebuild a dev session now, as its panel's `r` does (under --no-watch: apply the queued saves).")]
    async fn rebuild_dev(
        &self,
        Parameters(args): Parameters<DevSessionArgs>,
    ) -> Result<CallToolResult, McpError> {
        let live = match self.resolve_session(args.session_id, args.pid) {
            Ok(l) => l,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };
        match tokio::task::spawn_blocking(move || crate::dev_sessions::rebuild(&live.entry)).await {
            Ok(Ok(v)) => Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string_pretty(&json!({
                    "requested": v,
                    "next": "dev_status shows the build's progress and outcome",
                }))
                .unwrap(),
            )])),
            Ok(Err(e)) => Ok(CallToolResult::error(vec![Content::text(e)])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!("rebuild request failed: {e}"))])),
        }
    }

    #[tool(description = "A dev session's state: its settings, and per target the latest build or save with its outcome.")]
    async fn dev_status(
        &self,
        Parameters(args): Parameters<DevSessionArgs>,
    ) -> Result<CallToolResult, McpError> {
        let live = match self.resolve_session(args.session_id, args.pid) {
            Ok(l) => l,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };
        match tokio::task::spawn_blocking(move || crate::dev_sessions::status(&live.entry)).await {
            Ok(Ok(v)) => Ok(CallToolResult::success(vec![Content::text(serde_json::to_string_pretty(&v).unwrap())])),
            Ok(Err(e)) => Ok(CallToolResult::error(vec![Content::text(e)])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!("status request failed: {e}"))])),
        }
    }

    #[tool(description = "Stop one or all dev sessions this server launched.")]
    async fn stop_dev(
        &self,
        Parameters(args): Parameters<StopDevArgs>,
    ) -> Result<CallToolResult, McpError> {
        if args.all && args.session_id.is_none() && args.pid.is_none() {
            let stopped = self.dev_runner.stop_all();
            let mut out = json!({ "stopped": stopped });
            if args.include_external {
                let external: Vec<serde_json::Value> = crate::dev_sessions::live()
                    .into_iter()
                    .map(|l| {
                        let pid = l.entry.pid;
                        let how = crate::dev_sessions::stop_by_signal(pid, EXTERNAL_STOP_GRACE);
                        json!({ "pid": pid, "dir": l.entry.dir, "stopped": format!("{how:?}") })
                    })
                    .collect();
                out["external"] = json!(external);
            }
            return Ok(CallToolResult::success(vec![Content::text(serde_json::to_string_pretty(&out).unwrap())]));
        }
        // By pid: this server's own session goes the usual way; any other
        // only with `include_external`.
        let session_id = match (args.session_id, args.pid) {
            (Some(id), _) => Some(id),
            (None, Some(pid)) => match self.dev_runner.id_of_pid(pid) {
                Some(id) => Some(id),
                None => {
                    if !args.include_external {
                        return Ok(CallToolResult::error(vec![Content::text(format!(
                            "pid {pid} is a dev session this server did not start (a person's). \
                             Stopping it needs `include_external: true`."
                        ))]));
                    }
                    let how = tokio::task::spawn_blocking(move || {
                        crate::dev_sessions::stop_by_signal(pid, EXTERNAL_STOP_GRACE)
                    })
                    .await
                    .unwrap_or(crate::dev_sessions::Stopped::AlreadyGone);
                    return Ok(CallToolResult::success(vec![Content::text(
                        serde_json::to_string_pretty(&json!({ "pid": pid, "stopped": format!("{how:?}") }))
                            .unwrap(),
                    )]));
                }
            },
            (None, None) => None,
        };
        match session_id {
            Some(id) => match self.dev_runner.stop(id) {
                Ok(info) => Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&info).unwrap(),
                )])),
                Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
            },
            None => Ok(CallToolResult::error(vec![Content::text(
                "stop_dev needs `session_id`, `pid`, or `all: true`".to_string(),
            )])),
        }
    }

    #[tool(description = "Tail a dev session's log, optionally filtered (e.g. to compile errors).")]
    async fn read_dev_log(
        &self,
        Parameters(args): Parameters<ReadDevLogArgs>,
    ) -> Result<CallToolResult, McpError> {
        // This server's sessions: the log it captured. Anyone else's: the
        // session log its registry entry names.
        let own = args.session_id.or_else(|| args.pid.and_then(|p| self.dev_runner.id_of_pid(p)));
        let path = match (own, args.pid) {
            (Some(id), _) => self.dev_runner.log_path(id),
            (None, Some(pid)) => crate::dev_sessions::find(pid)
                .and_then(|l| l.entry.log_file)
                .map(std::path::PathBuf::from),
            (None, None) => None,
        };
        let Some(path) = path else {
            return Ok(CallToolResult::error(vec![Content::text(
                "no log for that session — pass `session_id` (from run_dev) or `pid` (from \
                 list_dev_sessions) of a session that ran here or is running now."
                    .to_string(),
            )]));
        };
        let lines = args.lines.unwrap_or(100).min(2000);
        match crate::dev_runner::tail_log(&path, lines, args.filter.as_deref()) {
            Ok(content) => {
                let out = json!({
                    "session_id": own,
                    "pid": args.pid,
                    "log_path": path.to_string_lossy(),
                    "lines": content.lines().count(),
                    "content": content,
                });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&out).unwrap(),
                )]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(description = "Wait until a running app registers, then return it.")]
    async fn wait_for_app(
        &self,
        Parameters(args): Parameters<WaitForAppArgs>,
    ) -> Result<CallToolResult, McpError> {
        if matches!(self.robot_mode, RobotMode::Disabled) {
            return Ok(CallToolResult::error(vec![Content::text(
                "robot control is disabled (--no-robot) — no app discovery to wait on."
                    .to_string(),
            )]));
        }
        let timeout = std::time::Duration::from_secs(args.timeout_secs.unwrap_or(60).min(600));
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let live = self.live_apps();
            let mut matched: Vec<_> = live
                .iter()
                .filter(|a| args.name.as_deref().map(|n| a.name == n).unwrap_or(true))
                .cloned()
                .collect();
            let registered: Vec<String> = matched.iter().map(|a| a.name.clone()).collect();
            // Registration precedes drivability: a web app's relay registers
            // at dev startup, but the bridge only answers once the wasm app
            // has dialed in. Returning on registration alone sends agents
            // straight into "no app connected to the relay" errors (observed
            // live: arena run-2, screenshot right after wait_for_app). Only
            // count an app as arrived when its bridge answers `ping`.
            let mut drivable = Vec::with_capacity(matched.len());
            for a in matched.drain(..) {
                let ok = crate::robot_bridge::RobotBridge::new(a.bridge_addr.clone())
                    .call("ping", serde_json::json!({}))
                    .await
                    .is_ok();
                if ok {
                    drivable.push(a);
                }
            }
            let matched = drivable;
            if !matched.is_empty() {
                let selectors = app_selectors(&matched);
                let json: Vec<serde_json::Value> = matched
                    .iter()
                    .zip(&selectors)
                    .map(|(a, selector)| {
                        json!({
                            "name": a.name,
                            "platform": a.platform,
                            "selector": selector,
                            "bundle_id": a.bundle_id,
                            "project_root": a.project_root,
                            "bridge_addr": a.bridge_addr,
                            "pid": a.pid,
                        })
                    })
                    .collect();
                return Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&json).unwrap(),
                )]));
            }
            if std::time::Instant::now() >= deadline {
                let target = args.name.as_deref().unwrap_or("any app");
                return Ok(CallToolResult::error(vec![Content::text(wait_for_app_timeout_message(
                    target,
                    timeout.as_secs(),
                    &registered,
                    &crate::app_discovery::describe_apps_dir(),
                ))]));
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }

    /// Shared helper for every Robot tool. Resolves the target
    /// bridge via the registry (if no legacy single bridge is
    /// pinned), then proxies the command. `app` is the optional
    /// selector — required when more than one app is registered,
    /// inferred when only one is.
    async fn robot_call(
        &self,
        cmd: &str,
        app: Option<&str>,
        args: serde_json::Value,
    ) -> Result<CallToolResult, McpError> {
        let bridge = self.resolve_bridge(app).await?;
        match bridge.call(cmd, args).await {
            Ok(value) => Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()),
            )])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// Read one element's native node over `bridge`. `Ok(None)` when the
    /// backend has no native data for it yet (or doesn't support the read).
    async fn introspect_one(
        &self,
        bridge: &RobotBridge,
        id: u64,
    ) -> Result<Option<native_parity::NativeNode>, McpError> {
        let v = bridge
            .call("introspect_native", json!({ "element_id": id }))
            .await
            .map_err(|e| McpError::invalid_params(format!("introspect_native failed: {e}"), None))?;
        native_parity::parse_native(v)
            .map_err(|e| McpError::invalid_params(format!("bad introspect_native payload: {e}"), None))
    }

    /// Body of the `compare_native_parity` tool — structurally align the two
    /// trees, introspect each aligned pair, diff with cross-platform
    /// normalization, and report structural + prop divergences.
    async fn do_compare_native_parity(
        &self,
        args: RobotParityArgs,
    ) -> Result<CallToolResult, McpError> {
        let bridge_a = self.resolve_bridge(Some(&args.app_a)).await?;
        let bridge_b = self.resolve_bridge(Some(&args.app_b)).await?;

        let snap_a = bridge_a.call("get_snapshot", json!({})).await
            .map_err(|e| McpError::invalid_params(format!("get_snapshot ({}) failed: {e}", args.app_a), None))?;
        let snap_b = bridge_b.call("get_snapshot", json!({})).await
            .map_err(|e| McpError::invalid_params(format!("get_snapshot ({}) failed: {e}", args.app_b), None))?;

        // Structural alignment on the cross-platform framework tree — scoped to
        // the `root` content anchor when given (excludes navigator chrome).
        let roots_a = native_parity::parse_snapshot(&snap_a);
        let roots_b = native_parity::parse_snapshot(&snap_b);
        let (list_a, list_b): (Vec<native_parity::SnapNode>, Vec<native_parity::SnapNode>) =
            match args.root.as_deref() {
                Some(tid) => {
                    let ra = native_parity::subtree_by_test_id(&roots_a, tid).ok_or_else(|| {
                        McpError::invalid_params(
                            format!("root test_id {tid:?} not found in {}", args.app_a),
                            None,
                        )
                    })?;
                    let rb = native_parity::subtree_by_test_id(&roots_b, tid).ok_or_else(|| {
                        McpError::invalid_params(
                            format!("root test_id {tid:?} not found in {}", args.app_b),
                            None,
                        )
                    })?;
                    (vec![ra.clone()], vec![rb.clone()])
                }
                None => (roots_a, roots_b),
            };
        let alignment = native_parity::align(&list_a, &list_b);

        // Introspect only the genuinely-corresponding elements, keyed by the
        // shared aligned path so the diff compares like with like.
        let mut cap_a = native_parity::Capture::new();
        let mut cap_b = native_parity::Capture::new();
        for pair in &alignment.pairs {
            if let Some(n) = self.introspect_one(&bridge_a, pair.id_a).await? {
                cap_a.insert(pair.path.clone(), n);
            }
            if let Some(n) = self.introspect_one(&bridge_b, pair.id_b).await? {
                cap_b.insert(pair.path.clone(), n);
            }
        }

        let opts = native_parity::DiffOptions {
            tol: native_parity::Tolerance {
                color: args.color_tolerance.unwrap_or(0.02),
                length: args.length_tolerance.unwrap_or(0.5),
                number: args.length_tolerance.unwrap_or(0.5),
                geometry: args.geometry_tolerance.unwrap_or(2.0),
            },
            normalize: !args.raw.unwrap_or(false),
            geometry: !args.no_geometry.unwrap_or(false),
        };
        let mismatches = native_parity::diff_with(&cap_a, &cap_b, opts);

        let mismatches_json: Vec<serde_json::Value> = mismatches.iter().map(|m| {
            let (kind, detail) = match &m.kind {
                native_parity::MismatchKind::ElementMissing { in_a } =>
                    ("prop_missing", if *in_a { format!("only in {}", args.app_a) } else { format!("only in {}", args.app_b) }),
                native_parity::MismatchKind::PropMissing { in_a } =>
                    ("prop_missing", if *in_a { format!("only in {}", args.app_a) } else { format!("only in {}", args.app_b) }),
                native_parity::MismatchKind::ValueDiffers { a, b } =>
                    ("value_differs", format!("{}={:?}  {}={:?}", args.app_a, a, args.app_b, b)),
            };
            json!({ "path": m.path, "key": m.key, "kind": kind, "detail": detail })
        }).collect();

        let only_a = alignment.unmatched.iter().filter(|u| u.in_a).count();
        let only_b = alignment.unmatched.iter().filter(|u| !u.in_a).count();
        // Cap the structural list so a wildly-divergent pair doesn't flood the
        // response; the counts above are the full story.
        let structural_items: Vec<serde_json::Value> = alignment.unmatched.iter().take(50).map(|u| {
            json!({ "path": u.path, "kind": u.kind, "only_in": if u.in_a { &args.app_a } else { &args.app_b } })
        }).collect();

        let result = json!({
            "app_a": args.app_a,
            "app_b": args.app_b,
            "root": args.root,
            "aligned": alignment.pairs.len(),
            "structural": { "only_a": only_a, "only_b": only_b, "items": structural_items },
            "prop_divergences": mismatches.len(),
            "mismatches": mismatches_json,
            "note": "For a meaningful result both apps must show the same route at the same viewport size.",
        });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string()),
        )]))
    }

    /// Resolve the target app's Robot bridge — the shared front half of
    /// every Robot tool. Honors `--no-robot`, the pinned-bridge modes,
    /// and discovery-by-`app`. Factored out so multi-step tools (e.g.
    /// `tap`, which calls `find_element` then `get_device_frame`) reuse
    /// one resolution path instead of re-implementing it.
    async fn resolve_bridge(&self, app: Option<&str>) -> Result<Arc<RobotBridge>, McpError> {
        if matches!(self.robot_mode, RobotMode::Disabled) {
            return Err(McpError::invalid_params(
                "Robot control is disabled — the MCP server was started with \
                 --no-robot. Restart it without --no-robot (or pass --robot-port \
                 to target a known bridge) to drive a running app."
                    .to_string(),
                None,
            ));
        }
        if let Some(b) = &self.robot {
            // Pinned-bridge mode: `with_robot_bridge(...)` or
            // `RobotMode::Explicit`. Ignore `app` — there's exactly one.
            Ok(b.clone())
        } else {
            self.resolver.resolve(app, &self.discovery).await
        }
    }

    /// `find_element { test_id }` over the bridge → the element id.
    async fn resolve_test_id(&self, app: Option<&str>, test_id: &str) -> Result<u64, McpError> {
        let bridge = self.resolve_bridge(app).await?;
        let v = bridge
            .call("find_element", json!({ "test_id": test_id }))
            .await
            .map_err(|e| McpError::invalid_params(format!("find_element failed: {e}"), None))?;
        v.get("id").and_then(|x| x.as_u64()).ok_or_else(|| {
            McpError::invalid_params(format!("no element found with test_id {test_id:?}"), None)
        })
    }

    /// `get_device_frame { element_id }` over the bridge → the center
    /// point in **physical device pixels**, ready for `adb input tap`.
    async fn element_device_center(
        &self,
        app: Option<&str>,
        element_id: u64,
    ) -> Result<(i32, i32), McpError> {
        let bridge = self.resolve_bridge(app).await?;
        let v = bridge
            .call("get_device_frame", json!({ "element_id": element_id }))
            .await
            .map_err(|e| McpError::invalid_params(format!("get_device_frame failed: {e}"), None))?;
        if v.is_null() {
            return Err(McpError::invalid_params(
                format!(
                    "element {element_id} has no device frame yet — it isn't laid out, or \
                     this backend doesn't implement device_frame (only Android does today)."
                ),
                None,
            ));
        }
        let f = |k: &str| v.get(k).and_then(|n| n.as_f64()).unwrap_or(0.0);
        let (x, y, w, h) = (f("x"), f("y"), f("width"), f("height"));
        Ok(((x + w / 2.0).round() as i32, (y + h / 2.0).round() as i32))
    }

    /// The `tap` tool's body: resolve a physical-pixel point (from an
    /// element, a `test_id`, or raw coords), then drive `adb` host-side
    /// to inject a real OS touch there.
    async fn do_tap(&self, args: RobotTap) -> Result<String, McpError> {
        let app = args.app.as_deref();
        let (px, py) = if let Some(id) = args.element_id {
            self.element_device_center(app, id).await?
        } else if let Some(test_id) = &args.test_id {
            let id = self.resolve_test_id(app, test_id).await?;
            self.element_device_center(app, id).await?
        } else if let (Some(x), Some(y)) = (args.x, args.y) {
            (x, y)
        } else {
            return Err(McpError::invalid_params(
                "tap needs one of: `element_id`, `test_id`, or both `x` and `y` \
                 (physical device pixels)."
                    .to_string(),
                None,
            ));
        };

        // adb runs on the host (here), not in the app — the app is on the
        // device and has no adb. Resolve the device, then inject.
        let to_err = |e: anyhow::Error| McpError::invalid_params(e.to_string(), None);
        let serial = adb::resolve_serial(args.serial.as_deref())
            .await
            .map_err(to_err)?;
        adb::tap(&serial, px, py).await.map_err(to_err)?;

        Ok(format!("OS-level tap on {serial} at physical ({px}, {py}) px"))
    }

    // -------------------------------------------------------------
    // Framework catalog tools — surface the locked-slice tables and
    // open-author slices (methods, animations, types) so AI / idea-ui
    // can discover the full authoring vocabulary, not just the
    // user's components.

    #[tool(description = "List the `ui!` leaf primitives (view, text, button, …); then `describe_primitive`.")]
    async fn list_primitives(
        &self,
        Parameters(req): Parameters<FilterRequest>,
    ) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let json: Vec<serde_json::Value> = cat
            .primitives()
            .iter()
            .filter(|p| {
                matches_filter(&req.filter, &[p.name, p.pascal_name, p.category.as_str()])
            })
            .map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "pascal_name": p.pascal_name,
                    "category": p.category.as_str(),
                    "summary": doc_summary(p.docs),
                })
            })
            .collect();
        Ok(self
            .catalog_text(serde_json::to_string_pretty(&json).unwrap())
            .await)
    }

    #[tool(description = "One primitive: every prop, backend support, docs.")]
    async fn describe_primitive(
        &self,
        Parameters(req): Parameters<NameRequest>,
    ) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let entry = cat
            .primitives()
            .iter()
            .find(|p| p.name == req.name || p.pascal_name == req.name)
            .ok_or_else(|| {
                McpError::invalid_params(
                    format!("primitive {:?} not found", req.name),
                    None,
                )
            })?;
        let props: Vec<serde_json::Value> = entry
            .props
            .iter()
            .map(|f| {
                serde_json::json!({
                    "name": f.name,
                    "type": f.type_str,
                    "doc": f.doc,
                    "constraint": f.constraint,
                })
            })
            .collect();
        let json = serde_json::json!({
            "name": entry.name,
            "pascal_name": entry.pascal_name,
            "category": entry.category.as_str(),
            "backends": entry.backends,
            "docs": entry.docs,
            "props": props,
        });
        Ok(self
            .catalog_text(serde_json::to_string_pretty(&json).unwrap())
            .await)
    }

    #[tool(description = "List free helper functions (`platform()`, `parse_color()`, …); then `describe_utility`.")]
    async fn list_utilities(
        &self,
        Parameters(req): Parameters<FilterRequest>,
    ) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let json: Vec<serde_json::Value> = cat
            .utilities()
            .iter()
            .filter_map(|u| {
                let fqn = format!("{}::{}", u.module_path, u.name);
                if !matches_filter(&req.filter, &[u.name, &fqn, u.category.as_str()]) {
                    return None;
                }
                Some(serde_json::json!({
                    "name": u.name,
                    "fqn": fqn,
                    "category": u.category.as_str(),
                    "return_type": u.return_type,
                    "summary": doc_summary(u.docs),
                }))
            })
            .collect();
        Ok(self
            .catalog_text(serde_json::to_string_pretty(&json).unwrap())
            .await)
    }

    #[tool(description = "One utility: docs, params, return type, authoring snippet.")]
    async fn describe_utility(
        &self,
        Parameters(req): Parameters<NameRequest>,
    ) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let entry = cat
            .utilities()
            .iter()
            .find(|u| u.name == req.name || format!("{}::{}", u.module_path, u.name) == req.name)
            .ok_or_else(|| {
                McpError::invalid_params(
                    format!("utility {:?} not found", req.name),
                    None,
                )
            })?;
        let params: Vec<serde_json::Value> = entry
            .params
            .iter()
            .map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "type": p.type_str,
                    "type_short_name": p.type_short_name,
                })
            })
            .collect();
        let return_type_inline = if !entry.return_type_short.is_empty() {
            cat.types()
                .iter()
                .find(|t| t.short_name == entry.return_type_short)
                .map(|t| type_entry_json(t))
        } else {
            None
        };
        let mut obj = serde_json::Map::new();
        obj.insert("name".into(), entry.name.into());
        obj.insert("module_path".into(), entry.module_path.into());
        obj.insert(
            "fqn".into(),
            format!("{}::{}", entry.module_path, entry.name).into(),
        );
        obj.insert("docs".into(), entry.docs.into());
        obj.insert("params".into(), serde_json::json!(params));
        obj.insert("return_type".into(), entry.return_type.into());
        obj.insert("return_type_short".into(), entry.return_type_short.into());
        obj.insert("category".into(), entry.category.as_str().into());
        // Paste-ready call shape with the author-supplied arguments marked
        // as `${n:name}` placeholders — the same string the editor
        // extension inserts, so an agent and a human get one canonical
        // spelling.
        obj.insert("snippet".into(), entry.snippet.into());
        if let Some(ty) = return_type_inline {
            obj.insert("return_type_entry".into(), ty);
        }
        // Cross-kind recipes: usage examples that target or use
        // this utility, surfaced the same way `describe_component` does.
        obj.insert(
            "recipes".into(),
            serde_json::json!(recipes_json_for(&cat, entry.name)),
        );
        if let Some(s) = cat.scope_for(entry.module_path) {
            obj.insert("scope".into(), s.slug.into());
        }
        Ok(self
            .catalog_text(serde_json::to_string_pretty(&serde_json::Value::Object(obj)).unwrap())
            .await)
    }

    #[tool(description = "List the authoring macros (`ui!`, `effect!`, `#[component]`, `stylesheet!`, …); then `describe_macro`.")]
    async fn list_macros(
        &self,
        Parameters(req): Parameters<FilterRequest>,
    ) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let json: Vec<serde_json::Value> = cat
            .macros()
            .iter()
            .filter_map(|m| {
                if !matches_filter(&req.filter, &[m.name, m.invocation, m.kind.as_str()]) {
                    return None;
                }
                Some(serde_json::json!({
                    "name": m.name,
                    "invocation": m.invocation,
                    "kind": m.kind.as_str(),
                    "summary": doc_summary(m.docs),
                }))
            })
            .collect();
        Ok(self
            .catalog_text(serde_json::to_string_pretty(&json).unwrap())
            .await)
    }

    #[tool(description = "One macro: invocation syntax, exporting crate, and what it expands to.")]
    async fn describe_macro(
        &self,
        Parameters(req): Parameters<NameRequest>,
    ) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let needle = req.name.trim_end_matches('!');
        let entry = cat
            .macros()
            .iter()
            .find(|m| m.name == needle)
            .ok_or_else(|| {
                McpError::invalid_params(format!("macro {:?} not found", req.name), None)
            })?;
        let json = serde_json::json!({
            "name": entry.name,
            "invocation": entry.invocation,
            "kind": entry.kind.as_str(),
            "module_path": entry.module_path,
            "fqn": format!("{}::{}", entry.module_path, entry.name),
            "docs": entry.docs,
            "expansion": entry.expansion,
            "snippet": entry.snippet,
        });
        Ok(self
            .catalog_text(serde_json::to_string_pretty(&json).unwrap())
            .await)
    }

    #[tool(description = "The interaction states `stylesheet!` arms can name (hovered, pressed, focused, disabled).")]
    async fn list_states(&self) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let json: Vec<serde_json::Value> = cat
            .states()
            .iter()
            .map(|s| {
                serde_json::json!({
                    "name": s.name,
                    "docs": s.docs,
                    "backends": s.backends,
                })
            })
            .collect();
        Ok(self
            .catalog_text(serde_json::to_string_pretty(&json).unwrap())
            .await)
    }

    #[tool(description = "The theme tokens a `stylesheet!` can reference, with accessor path and value type.")]
    async fn list_tokens(&self) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let json: Vec<serde_json::Value> = cat
            .style_tokens()
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "path": t.path,
                    "accessor": format!("t.{}()", t.path),
                    "namespace": t.namespace,
                    "value_type": t.value_type,
                    "default_value": t.default_value.get(),
                    "vocabulary": t.vocabulary,
                })
            })
            .collect();
        Ok(self
            .catalog_text(serde_json::to_string_pretty(&json).unwrap())
            .await)
    }

    #[tool(description = "One theme token by registry name or accessor path.")]
    async fn describe_token(
        &self,
        Parameters(req): Parameters<NameRequest>,
    ) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let needle = req.name.trim();
        let entry = cat
            .style_tokens()
            .iter()
            .find(|t| t.name == needle || t.path == needle)
            .ok_or_else(|| {
                McpError::invalid_params(
                    format!(
                        "no theme token `{needle}` — call list_tokens for the vocabulary. \
                         Tokens are named by registry key (`spacing-md`) or accessor path \
                         (`spacing.md`)."
                    ),
                    None,
                )
            })?;
        let json = serde_json::json!({
            "name": entry.name,
            "path": entry.path,
            "accessor": format!("t.{}()", entry.path),
            "namespace": entry.namespace,
            "value_type": entry.value_type,
            "default_value": entry.default_value.get(),
            "vocabulary": entry.vocabulary,
            "usage": format!(
                "stylesheet! {{ pub S<{}> {{ base(t) {{ /* … */: t.{}() }} }} }}",
                entry.vocabulary, entry.path
            ),
        });
        Ok(self
            .catalog_text(serde_json::to_string_pretty(&json).unwrap())
            .await)
    }

    #[tool(description = "List the bundled framework guides; read one with `read_guide`.")]
    async fn list_guides(&self) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let json: Vec<serde_json::Value> = cat
            .guides()
            .iter()
            .map(|g| {
                serde_json::json!({
                    "slug": g.slug,
                    "title": g.title,
                    "order": g.order,
                    "tags": g.tags,
                })
            })
            .collect();
        Ok(self
            .catalog_text(serde_json::to_string_pretty(&json).unwrap())
            .await)
    }

    #[tool(description = "Full docs for this server's tools and topics (parameters, return shapes, caveats). Omit `name` for the index.")]
    async fn get_docs(
        &self,
        Parameters(req): Parameters<GetDocsRequest>,
    ) -> Result<CallToolResult, McpError> {
        Ok(self.docs_result(req))
    }

    #[tool(description = "Read one framework guide's markdown by slug.")]
    async fn read_guide(
        &self,
        Parameters(req): Parameters<SlugRequest>,
    ) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let entry = cat
            .guides()
            .iter()
            .find(|g| g.slug == req.slug)
            .ok_or_else(|| {
                McpError::invalid_params(format!("guide {:?} not found", req.slug), None)
            })?;
        // Surface the recipe catalog next to the prose: arena feedback
        // (debug-fix run-0) showed an agent reading the reactivity guide,
        // correctly naming the frozen-snapshot pitfall, then writing
        // `for item in items.get()` anyway — because it never discovered
        // the `keyed_list_add_remove` recipe that shows the right form.
        let mut body = entry.body.to_string();
        let related = related_recipe_names(&cat, entry);
        if !related.is_empty() {
            body.push_str(&format!(
                "\n\nRelated recipes: {} — fetch the compile-checked source with \
                 describe_recipe(name); browse all with list_recipes.",
                related.join(", ")
            ));
        } else if matches!(
            entry.slug,
            "reactivity" | "reactivity-in-depth" | "components" | "navigation" | "styling"
        ) {
            // No recipes registered in this catalog build (or none joined):
            // still point core guides at the recipe tools so the pathway is
            // discoverable.
            body.push_str(
                "\n\nRelated recipes: compile-checked usage examples for this area \
                 live in the recipe catalog — browse with list_recipes, fetch one \
                 with describe_recipe.",
            );
        }
        let json = serde_json::json!({
            "slug": entry.slug,
            "title": entry.title,
            "order": entry.order,
            "tags": entry.tags,
            "body": body,
        });
        Ok(self
            .catalog_text(serde_json::to_string_pretty(&json).unwrap())
            .await)
    }

    #[tool(description = "List `#[method]` functions declared in components, optionally for one parent component.")]
    async fn list_methods(
        &self,
        Parameters(req): Parameters<NameRequest>,
    ) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let needle = req.name.trim();
        let json: Vec<serde_json::Value> = cat
            .methods()
            .iter()
            .filter(|m| {
                needle.is_empty()
                    || m.parent_name == needle
                    || format!("{}::{}", m.parent_module_path, m.parent_name) == needle
            })
            .map(|m| {
                let params: Vec<serde_json::Value> = m
                    .params
                    .iter()
                    .map(|p| {
                        serde_json::json!({
                            "name": p.name,
                            "type": p.type_str,
                            "type_short_name": p.type_short_name,
                        })
                    })
                    .collect();
                serde_json::json!({
                    "parent_fqn": format!("{}::{}", m.parent_module_path, m.parent_name),
                    "name": m.name,
                    "summary": doc_summary(m.docs),
                    "params": params,
                    "return_type": m.return_type,
                })
            })
            .collect();
        Ok(self
            .catalog_text(serde_json::to_string_pretty(&json).unwrap())
            .await)
    }

    #[tool(description = "List `animated!` values declared in components, optionally for one parent component.")]
    async fn list_animations(
        &self,
        Parameters(req): Parameters<NameRequest>,
    ) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let needle = req.name.trim();
        let json: Vec<serde_json::Value> = cat
            .animations()
            .iter()
            .filter(|a| {
                needle.is_empty()
                    || a.parent_name == needle
                    || format!("{}::{}", a.parent_module_path, a.parent_name) == needle
            })
            .map(|a| {
                serde_json::json!({
                    "parent_fqn": format!("{}::{}", a.parent_module_path, a.parent_name),
                    "binding": a.binding,
                    "initial": a.initial,
                    "line": a.line,
                })
            })
            .collect();
        Ok(self
            .catalog_text(serde_json::to_string_pretty(&json).unwrap())
            .await)
    }

    #[tool(description = "List `#[derive(IdealystSchema)]` structs and enums; then `describe_type`.")]
    async fn list_types(
        &self,
        Parameters(req): Parameters<FilterRequest>,
    ) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let json: Vec<serde_json::Value> = cat
            .types()
            .iter()
            .filter_map(|t| {
                let kind = match &t.shape {
                    mcp_catalog::TypeShape::Struct { .. } => "struct",
                    mcp_catalog::TypeShape::Enum { .. } => "enum",
                };
                let fqn = format!("{}::{}", t.module_path, t.short_name);
                if !matches_filter(&req.filter, &[t.short_name, &fqn, kind]) {
                    return None;
                }
                Some(serde_json::json!({
                    "short_name": t.short_name,
                    "fqn": fqn,
                    "kind": kind,
                }))
            })
            .collect();
        Ok(self
            .catalog_text(serde_json::to_string_pretty(&json).unwrap())
            .await)
    }

    #[tool(description = "What values an open-set prop accepts (e.g. `tone = tone::Primary`), with the import to use.")]
    async fn list_values(
        &self,
        Parameters(req): Parameters<FilterRequest>,
    ) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let json: Vec<serde_json::Value> = cat
            .values()
            .iter()
            .filter_map(|v| {
                let fqn = format!("{}::{}", v.module_path, v.short_name);
                let spelled = v.spelled();
                if !matches_filter(&req.filter, &[v.value_of, &spelled, &fqn]) {
                    return None;
                }
                Some(serde_json::json!({
                    "value_of": v.value_of,
                    "spelled": spelled,
                    "import": v.import(),
                    "short_name": v.short_name,
                    "fqn": fqn,
                    "docs": v.docs,
                }))
            })
            .collect();
        Ok(self
            .catalog_text(serde_json::to_string_pretty(&json).unwrap())
            .await)
    }

    #[tool(description = "One type: struct fields or enum variants.")]
    async fn describe_type(
        &self,
        Parameters(req): Parameters<NameRequest>,
    ) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let entry = cat
            .types()
            .iter()
            .find(|t| {
                t.short_name == req.name
                    || format!("{}::{}", t.module_path, t.short_name) == req.name
            })
            .ok_or_else(|| {
                // A common dead-end: an LLM sees a component param typed
                // `&FooProps` (from `describe_component`) and calls
                // `describe_type("FooProps")`. Props/types are catalogued
                // only when their struct/enum derives `#[derive(IdealystSchema)]`;
                // an unannotated one simply isn't here. Make the cause
                // actionable rather than a bare "not found".
                //
                // Same dead-end, styling flavor (observed in arena runs): the
                // core style-machinery types show up in signatures
                // (`impl IntoStyleSource`, `StyleApplication`) but are not
                // IdealystSchema types, so a bare "not found" sends the agent
                // source-diving. Redirect to the prose home instead.
                const CORE_STYLE_TYPES: &[&str] = &[
                    "StyleSource",
                    "StyleApplication",
                    "StyleSheet",
                    "StyleRules",
                    "IntoStyleSource",
                ];
                let short = req.name.rsplit("::").next().unwrap_or(&req.name);
                let hint = if CORE_STYLE_TYPES.contains(&short) {
                    " — this is a core style type, not an IdealystSchema type: \
                     see read_guide(\"styling\") and the `stylesheet` macro entry \
                     (describe_macro(\"stylesheet\"))"
                } else if req.name.ends_with("Props") {
                    " — if this is a component's props struct, its fields are \
                     catalogued only when it derives `#[derive(IdealystSchema)]`; \
                     the defining crate may not annotate it yet"
                } else {
                    " — only structs/enums deriving `#[derive(IdealystSchema)]` are \
                     catalogued; use list_types to see what's available"
                };
                McpError::invalid_params(
                    format!("type {:?} not found in the catalog{}", req.name, hint),
                    None,
                )
            })?;
        Ok(self
            .catalog_text(serde_json::to_string_pretty(&type_entry_json(entry)).unwrap())
            .await)
    }

    #[tool(description = "List compile-checked usage examples; then `describe_recipe` for the source.")]
    async fn list_recipes(
        &self,
        Parameters(req): Parameters<FilterRequest>,
    ) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let json: Vec<serde_json::Value> = cat
            .recipes()
            .iter()
            .filter_map(|r| {
                let fqn = format!("{}::{}", r.module_path, r.name);
                if !matches_filter(&req.filter, &[r.name, r.target, &fqn]) {
                    return None;
                }
                Some(serde_json::json!({
                    "name": r.name,
                    "target": r.target,
                    "fqn": fqn,
                    "summary": doc_summary(r.docs),
                }))
            })
            .collect();
        // Disambiguate "your filter matched nothing" from "this catalog build
        // registered no recipes at all" — an empty bare `[]` reads as a query
        // failure and sends agents into retry loops (observed live: arena
        // run-2 probed twice, then went source-diving).
        if json.is_empty() {
            let total = cat.recipes().len();
            let msg = if total == 0 {
                "[] (0 recipes registered in this catalog build — nothing to filter; \
                 recipes come from crates compiled with the `catalog` feature)"
                    .to_string()
            } else {
                format!(
                    "[] (filter matched none of the {total} registered recipe(s) — \
                     try without `filter`)"
                )
            };
            return Ok(CallToolResult::success(vec![Content::text(msg)]));
        }
        Ok(self
            .catalog_text(serde_json::to_string_pretty(&json).unwrap())
            .await)
    }

    #[tool(description = "One recipe's verified, copy-pasteable source and docs.")]
    async fn describe_recipe(
        &self,
        Parameters(req): Parameters<NameRequest>,
    ) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let entry = cat
            .recipes()
            .iter()
            .find(|r| {
                r.name == req.name || format!("{}::{}", r.module_path, r.name) == req.name
            })
            .ok_or_else(|| {
                McpError::invalid_params(
                    format!(
                        "recipe {:?} not found — use list_recipes to see what's available",
                        req.name
                    ),
                    None,
                )
            })?;
        let json = serde_json::json!({
            "name": entry.name,
            "target": entry.target,
            "module_path": entry.module_path,
            "fqn": format!("{}::{}", entry.module_path, entry.name),
            "file": entry.file,
            "line": entry.line,
            "docs": entry.docs,
            "source": entry.source,
            "uses": entry.uses,
        });
        Ok(self
            .catalog_text(serde_json::to_string_pretty(&json).unwrap())
            .await)
    }

    #[tool(description = "List documentation scopes (feature areas); then `describe_scope`.")]
    async fn list_scopes(
        &self,
        Parameters(req): Parameters<FilterRequest>,
    ) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let json: Vec<serde_json::Value> = cat
            .scopes()
            .iter()
            .filter_map(|s| {
                if !matches_filter(&req.filter, &[s.slug, s.title]) {
                    return None;
                }
                Some(serde_json::json!({
                    "slug": s.slug,
                    "title": s.title,
                    "order": s.order,
                    "summary": doc_summary(s.docs),
                }))
            })
            .collect();
        Ok(self
            .catalog_text(serde_json::to_string_pretty(&json).unwrap())
            .await)
    }

    #[tool(description = "One scope: its docs and the entities directly in it.")]
    async fn describe_scope(
        &self,
        Parameters(req): Parameters<NameRequest>,
    ) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let scope = cat
            .scopes()
            .iter()
            .find(|s| s.slug == req.name)
            .ok_or_else(|| {
                McpError::invalid_params(
                    format!("scope {:?} not found — use list_scopes to see what's available", req.name),
                    None,
                )
            })?;
        let components: Vec<serde_json::Value> = cat
            .entries()
            .iter()
            .filter(|e| cat.scope_for(e.module_path).map(|s| s.slug) == Some(scope.slug))
            .map(|e| serde_json::json!({
                "name": e.name,
                "fqn": format!("{}::{}", e.module_path, e.name),
            }))
            .collect();
        let utilities: Vec<serde_json::Value> = cat
            .utilities()
            .iter()
            .filter(|u| cat.scope_for(u.module_path).map(|s| s.slug) == Some(scope.slug))
            .map(|u| serde_json::json!({
                "name": u.name,
                "fqn": format!("{}::{}", u.module_path, u.name),
            }))
            .collect();
        let json = serde_json::json!({
            "slug": scope.slug,
            "title": scope.title,
            "docs": scope.docs,
            "order": scope.order,
            "components": components,
            "utilities": utilities,
        });
        Ok(self
            .catalog_text(serde_json::to_string_pretty(&json).unwrap())
            .await)
    }

    #[tool(description = "List opt-in SDK crates (networking, storage, camera, maps, …) with their Cargo dependency line.")]
    async fn list_sdks(
        &self,
        Parameters(req): Parameters<FilterRequest>,
    ) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let json: Vec<serde_json::Value> = cat
            .sdks()
            .iter()
            .filter(|s| {
                matches_filter(&req.filter, &[s.name, s.category.as_str(), s.kind.as_str()])
            })
            .map(|s| {
                serde_json::json!({
                    "name": s.name,
                    "category": s.category.as_str(),
                    "kind": s.kind.as_str(),
                    "dep_line": s.dep_line,
                    "summary": s.summary,
                })
            })
            .collect();
        Ok(self
            .catalog_text(serde_json::to_string_pretty(&json).unwrap())
            .await)
    }

    #[tool(description = "One SDK crate: summary, dependency line, kind, and its guide.")]
    async fn describe_sdk(
        &self,
        Parameters(req): Parameters<NameRequest>,
    ) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let entry = cat
            .sdks()
            .iter()
            .find(|s| s.name == req.name)
            .ok_or_else(|| {
                McpError::invalid_params(
                    format!(
                        "SDK crate {:?} not found — use list_sdks to see what's available, \
                         or read_guide(\"sdks\") for the full roster",
                        req.name
                    ),
                    None,
                )
            })?;
        let json = serde_json::json!({
            "name": entry.name,
            "summary": entry.summary,
            "dep_line": entry.dep_line,
            "category": entry.category.as_str(),
            "kind": entry.kind.as_str(),
            "guide": entry.guide,
        });
        Ok(self
            .catalog_text(serde_json::to_string_pretty(&json).unwrap())
            .await)
    }

    #[tool(description = "List the icon packs available to the `icon(...)` primitive.")]
    async fn list_icon_sets(
        &self,
        Parameters(req): Parameters<FilterRequest>,
    ) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let json: Vec<serde_json::Value> = cat
            .icon_sets()
            .iter()
            .filter(|s| matches_filter(&req.filter, &[s.name, s.title]))
            .map(|s| {
                serde_json::json!({
                    "name": s.name,
                    "title": s.title,
                    "icon_count": s.icons.len(),
                    "import_path": s.import_path,
                    "license": s.license,
                    "homepage": s.homepage,
                })
            })
            .collect();
        Ok(self
            .catalog_text(serde_json::to_string_pretty(&json).unwrap())
            .await)
    }

    #[tool(description = "Page through one icon pack's icons with their import paths.")]
    async fn describe_icon_set(
        &self,
        Parameters(req): Parameters<DescribeIconSetRequest>,
    ) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let set = cat
            .icon_sets()
            .iter()
            .find(|s| icon_set_matches(s, &req.name))
            .ok_or_else(|| unknown_icon_set(&req.name, cat.icon_sets()))?;
        let offset = req.offset.unwrap_or(0);
        let limit = req.limit.unwrap_or(100).min(500);
        let icons: Vec<serde_json::Value> = set
            .icons
            .iter()
            .skip(offset)
            .take(limit)
            .map(|i| {
                serde_json::json!({
                    "name": i.name,
                    "ident": i.ident,
                    "import": format!("{}::{}", set.import_path, i.ident),
                })
            })
            .collect();
        let json = serde_json::json!({
            "name": set.name,
            "title": set.title,
            "docs": set.docs,
            "import_path": set.import_path,
            "license": set.license,
            "homepage": set.homepage,
            "icon_count": set.icons.len(),
            "offset": offset,
            "limit": limit,
            "icons": icons,
        });
        Ok(self
            .catalog_text(serde_json::to_string_pretty(&json).unwrap())
            .await)
    }

    #[tool(description = "Find icons by name keyword across all packs, with paste-ready import paths.")]
    async fn search_icons(
        &self,
        Parameters(req): Parameters<IconSearchRequest>,
    ) -> Result<CallToolResult, McpError> {
        let q = normalize_icon_key(&req.query);
        if q.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text("[]".to_string())]));
        }
        let limit = req.limit.unwrap_or(50).min(200);
        let cat = self.catalog.read().await;
        // Resolve the pack filter up front, by any of its names — not the
        // crate name byte-for-byte: `set = "lucide"` (the name every caller
        // reaches for) would skip every pack and return `[]`, which reads
        // as "no such icon", the wrong conclusion entirely.
        if let Some(want) = &req.set {
            if !cat.icon_sets().iter().any(|s| icon_set_matches(s, want)) {
                return Err(unknown_icon_set(want, cat.icon_sets()));
            }
        }
        let mut out: Vec<serde_json::Value> = Vec::new();
        'sets: for set in cat.icon_sets() {
            if let Some(want) = &req.set {
                if !icon_set_matches(set, want) {
                    continue;
                }
            }
            for i in set.icons {
                if normalize_icon_key(i.name).contains(&q) {
                    out.push(serde_json::json!({
                        "set": set.name,
                        "name": i.name,
                        "ident": i.ident,
                        "import": format!("{}::{}", set.import_path, i.ident),
                    }));
                    if out.len() >= limit {
                        break 'sets;
                    }
                }
            }
        }
        Ok(self
            .catalog_text(serde_json::to_string_pretty(&out).unwrap())
            .await)
    }

    #[tool(description = "Ranked fulltext search across every catalog slice (components, utilities, guides, types, …).")]
    async fn search(
        &self,
        Parameters(req): Parameters<SearchRequest>,
    ) -> Result<CallToolResult, McpError> {
        // Tokenize on whitespace and lowercase each term. A query with no
        // spaces is a single-term search. Multi-word queries OR across
        // terms and rank by distinct-term hit count — a contiguous-substring
        // match of the whole query returns `[]` for nearly any multi-word
        // query.
        let terms: Vec<String> = req
            .query
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        if terms.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text("[]".to_string())]));
        }

        let cat = self.catalog.read().await;
        // (score, kind, name, fqn, excerpt-source-text, matched-terms)
        let mut hits: Vec<(usize, &'static str, String, String, String, Vec<String>)> = Vec::new();

        // Push a hit if the entry matches at least one term. `fields` are
        // the searchable strings (name + invocation/title/etc.); `body` is
        // the longer text the excerpt is drawn from (docs / guide body).
        let consider =
            |kind: &'static str,
             name: String,
             fqn: String,
             fields: &[&str],
             body: &str,
             hits: &mut Vec<(usize, &'static str, String, String, String, Vec<String>)>| {
                let matched = matched_terms(&terms, fields, body);
                if !matched.is_empty() {
                    hits.push((matched.len(), kind, name, fqn, body.to_string(), matched));
                }
            };

        for e in cat.entries() {
            let fqn = format!("{}::{}", e.module_path, e.name);
            consider("component", e.name.to_string(), fqn, &[e.name], e.docs, &mut hits);
        }
        for p in cat.primitives() {
            // A primitive's props are part of its searchable text. The
            // question an author actually has is "which primitive takes
            // `on_end_reached`?", and with only the name and the summary
            // indexed that query returned nothing — the setter was
            // undiscoverable from the MCP even once the table listed it,
            // and one commit (47d014a2) exists because an author could
            // not find it. Prop docs ride along so "infinite paging"
            // finds it too.
            let mut body = String::from(p.docs);
            for f in p.props {
                body.push('\n');
                body.push_str(f.name);
                body.push_str(": ");
                body.push_str(f.doc);
            }
            consider(
                "primitive",
                p.name.to_string(),
                p.name.to_string(),
                &[p.name, p.pascal_name],
                &body,
                &mut hits,
            );
        }
        for u in cat.utilities() {
            let fqn = format!("{}::{}", u.module_path, u.name);
            consider("utility", u.name.to_string(), fqn, &[u.name], u.docs, &mut hits);
        }
        for m in cat.macros() {
            let fqn = format!("{}::{}", m.module_path, m.name);
            consider(
                "macro",
                m.name.to_string(),
                fqn,
                &[m.name, m.invocation],
                m.docs,
                &mut hits,
            );
        }
        for g in cat.guides() {
            consider(
                "guide",
                g.slug.to_string(),
                g.slug.to_string(),
                &[g.slug, g.title],
                g.body,
                &mut hits,
            );
        }
        for t in cat.types() {
            let fqn = format!("{}::{}", t.module_path, t.short_name);
            consider("type", t.short_name.to_string(), fqn, &[t.short_name], t.docs, &mut hits);
        }
        for m in cat.methods() {
            let fqn = format!("{}::{}.{}", m.parent_module_path, m.parent_name, m.name);
            consider("method", m.name.to_string(), fqn, &[m.name], m.docs, &mut hits);
        }
        for r in cat.recipes() {
            let fqn = format!("{}::{}", r.module_path, r.name);
            consider("recipe", r.name.to_string(), fqn, &[r.name, r.target], r.docs, &mut hits);
        }
        for s in cat.scopes() {
            consider(
                "scope",
                s.slug.to_string(),
                s.slug.to_string(),
                &[s.slug, s.title],
                s.docs,
                &mut hits,
            );
        }
        for t in cat.tools() {
            let fqn = format!("{}::{}", t.module_path, t.name);
            consider("tool", t.name.to_string(), fqn, &[t.name], t.docs, &mut hits);
        }
        for s in cat.states() {
            consider("state", s.name.to_string(), s.name.to_string(), &[s.name], s.docs, &mut hits);
        }
        for t in cat.style_tokens() {
            // Searchable by BOTH spellings: an author migrating from the
            // string form knows `spacing-md`, one writing new code knows
            // `spacing.md`. The docs line carries the accessor to write.
            let accessor = format!("t.{}()", t.path);
            let docs = format!(
                "Theme token `{}` — write `{}` inside a stylesheet! ({} base {})",
                t.name, accessor, t.value_type, t.default_value.get()
            );
            consider(
                "style_token",
                t.path.to_string(),
                t.name.to_string(),
                &[t.name, t.path],
                Box::leak(docs.into_boxed_str()),
                &mut hits,
            );
        }
        for a in cat.animations() {
            let fqn = format!("{}::{}.{}", a.parent_module_path, a.parent_name, a.binding);
            consider("animation", a.binding.to_string(), fqn, &[a.binding], a.initial, &mut hits);
        }
        for s in cat.sdks() {
            consider("sdk", s.name.to_string(), s.name.to_string(), &[s.name], s.summary, &mut hits);
        }
        // Icon packs surface at pack level only — searching individual icon
        // names here would swamp results with ~1600 leaves per pack; that's
        // what `search_icons` is for.
        for s in cat.icon_sets() {
            consider(
                "icon_set",
                s.name.to_string(),
                s.name.to_string(),
                &[s.name, s.title, s.import_path],
                s.docs,
                &mut hits,
            );
        }

        // Rank: most distinct query terms first, then a stable
        // (kind, name) tie-break so output is deterministic.
        hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| (a.1, &a.2).cmp(&(b.1, &b.2))));

        let json: Vec<serde_json::Value> = hits
            .iter()
            .map(|(score, kind, name, fqn, body, matched)| {
                // Excerpt around the first matched term that actually
                // occurs in the body (terms may match only the name).
                let anchor = matched
                    .iter()
                    .find(|t| body.to_lowercase().contains(t.as_str()))
                    .cloned()
                    .unwrap_or_default();
                serde_json::json!({
                    "kind": kind,
                    "name": name,
                    "fqn": fqn,
                    "score": score,
                    "matched_terms": matched,
                    "docs_excerpt": docs_excerpt_around(body, &anchor),
                })
            })
            .collect();

        Ok(self
            .catalog_text(serde_json::to_string_pretty(&json).unwrap())
            .await)
    }

    #[tool(description = "Run the idealyst source linter over a project and return its findings.")]
    async fn lint_project(
        &self,
        Parameters(req): Parameters<LintRequest>,
    ) -> Result<CallToolResult, McpError> {
        use std::path::PathBuf;

        // Resolve the target. Explicit `path` wins; otherwise lint the
        // single live app's project root, falling back to the server's
        // working directory (mirrors the CLI's `.` default).
        let path: PathBuf = match req.path.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(p) => PathBuf::from(p),
            None => {
                let live = self.discovery.snapshot();
                if live.len() == 1 {
                    live[0]
                        .project_root
                        .as_deref()
                        .map(PathBuf::from)
                        .unwrap_or_else(|| PathBuf::from("."))
                } else {
                    PathBuf::from(".")
                }
            }
        };

        if !path.exists() {
            return Err(McpError::invalid_params(
                format!("lint path {:?} does not exist", path.display()),
                None,
            ));
        }

        let deny_warnings = req.deny_warnings.unwrap_or(false);
        let limit = req.limit.unwrap_or(200).min(1000);

        // The lint engine is synchronous and walks the filesystem; keep it
        // off the async reactor.
        let path_for_task = path.clone();
        let run = tokio::task::spawn_blocking(move || {
            // Discover `idealyst-lint.toml` from the target, else defaults.
            let config = lint::Config::discover(&path_for_task)
                .map(|loaded| loaded.config)
                .unwrap_or_default();
            lint::lint_path(&path_for_task, &config)
        })
        .await
        .map_err(|e| McpError::internal_error(format!("lint task panicked: {e}"), None))?;

        let error_count = run.error_count();
        let warn_count = run.warn_count();
        let failed = run.failed() || (deny_warnings && warn_count > 0);
        let total = run.diagnostics.len();

        let diagnostics: Vec<serde_json::Value> = run
            .diagnostics
            .iter()
            .take(limit)
            .map(|d| {
                let severity = match d.severity {
                    lint::Severity::Warn => "warn",
                    lint::Severity::Error => "error",
                };
                json!({
                    "rule": d.rule,
                    "severity": severity,
                    "message": d.message,
                    "help": d.help,
                    "file": d.file.display().to_string(),
                    "line": d.line_start,
                    "column": d.col_start,
                    "end_line": d.line_end,
                    "end_column": d.col_end,
                    "source_line": d.source_line,
                })
            })
            .collect();

        let parse_errors: Vec<serde_json::Value> = run
            .parse_errors
            .iter()
            .map(|(p, err)| json!({ "file": p.display().to_string(), "error": err }))
            .collect();

        let result = json!({
            "path": path.display().to_string(),
            "files_scanned": run.files_scanned,
            "error_count": error_count,
            "warn_count": warn_count,
            "total_diagnostics": total,
            "failed": failed,
            "truncated": total > diagnostics.len(),
            "parse_errors": parse_errors,
            "diagnostics": diagnostics,
        });

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&result).unwrap(),
        )]))
    }

    #[tool(description = "Create or update a project's devcontainer and its add-ons (database, redis, minio, playwright, agent CLIs).")]
    async fn configure_devcontainer(
        &self,
        Parameters(req): Parameters<ConfigureDevcontainerRequest>,
    ) -> Result<CallToolResult, McpError> {
        use configure::devcontainer::{self, Action, ConfigureRequest, ServiceRequest};
        use std::path::PathBuf;

        // Resolve the target dir the same way `lint_project` does: explicit
        // `dir` wins; else the single live app's root; else the server cwd.
        let dir: PathBuf = match req.dir.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(p) => PathBuf::from(p),
            None => {
                let live = self.discovery.snapshot();
                if live.len() == 1 {
                    live[0]
                        .project_root
                        .as_deref()
                        .map(PathBuf::from)
                        .unwrap_or_else(|| PathBuf::from("."))
                } else {
                    PathBuf::from(".")
                }
            }
        };

        // Map the JSON service args into the shared request type.
        let mut services = Vec::with_capacity(req.services.len());
        for s in &req.services {
            let action = match s.action.as_deref().unwrap_or("enable") {
                "enable" => Action::Enable,
                "reconfigure" => Action::Reconfigure,
                "remove" => Action::Remove,
                other => {
                    return Err(McpError::invalid_params(
                        format!("unknown action {other:?}; expected enable|reconfigure|remove"),
                        None,
                    ))
                }
            };
            services.push(ServiceRequest {
                id: s.id.clone(),
                variant: s.variant.clone(),
                action,
            });
        }

        // The engine is synchronous filesystem work; keep it off the reactor.
        let request = ConfigureRequest {
            services,
            config: req.config.clone(),
        };
        let dir_for_task = dir.clone();
        let report = tokio::task::spawn_blocking(move || devcontainer::apply(&dir_for_task, &request))
            .await
            .map_err(|e| McpError::internal_error(format!("configure task panicked: {e}"), None))?
            .map_err(|e| McpError::invalid_params(format!("{e:#}"), None))?;

        let result = json!({
            "dir": dir.display().to_string(),
            "added": report.added,
            "removed": report.removed,
            "reconfigured": report.reconfigured,
            "unchanged": report.unchanged,
            "warnings": report.warnings,
            "wrote": report.wrote.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&result).unwrap(),
        )]))
    }

    #[tool(description = "Set up a project's VS Code workspace (extension recommendations, inline lint).")]
    async fn configure_vscode(
        &self,
        Parameters(req): Parameters<ConfigureVscodeRequest>,
    ) -> Result<CallToolResult, McpError> {
        use configure::vscode::{self, Action, AspectRequest, ConfigureRequest};
        use std::path::PathBuf;

        let dir: PathBuf = match req.dir.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(p) => PathBuf::from(p),
            None => {
                let live = self.discovery.snapshot();
                if live.len() == 1 {
                    live[0]
                        .project_root
                        .as_deref()
                        .map(PathBuf::from)
                        .unwrap_or_else(|| PathBuf::from("."))
                } else {
                    PathBuf::from(".")
                }
            }
        };

        // remove_all wins; else explicit aspects; else enable everything.
        let request = if req.remove_all.unwrap_or(false) {
            ConfigureRequest {
                aspects: vscode::registry()
                    .iter()
                    .map(|a| AspectRequest { id: a.id().to_string(), action: Action::Remove })
                    .collect(),
            }
        } else {
            match req.aspects.as_ref().filter(|a| !a.is_empty()) {
                None => vscode::enable_all_request(),
                Some(list) => {
                    let mut aspects = Vec::with_capacity(list.len());
                    for a in list {
                        let action = match a.action.as_deref().unwrap_or("enable") {
                            "enable" => Action::Enable,
                            "remove" => Action::Remove,
                            other => {
                                return Err(McpError::invalid_params(
                                    format!("unknown action {other:?}; expected enable|remove"),
                                    None,
                                ))
                            }
                        };
                        aspects.push(AspectRequest { id: a.id.clone(), action });
                    }
                    ConfigureRequest { aspects }
                }
            }
        };

        let dir_for_task = dir.clone();
        let report = tokio::task::spawn_blocking(move || vscode::apply(&dir_for_task, &request))
            .await
            .map_err(|e| McpError::internal_error(format!("configure task panicked: {e}"), None))?
            .map_err(|e| McpError::invalid_params(format!("{e:#}"), None))?;

        let result = json!({
            "dir": dir.display().to_string(),
            "added": report.added,
            "removed": report.removed,
            "unchanged": report.unchanged,
            "warnings": report.warnings,
            "wrote": report.wrote.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&result).unwrap(),
        )]))
    }
}

/// Which of `terms` appear (case-insensitively) in any of `fields` or in
/// `body`. Returns the matched subset in query order; empty means the
/// entry is not a hit. The OR + rank-by-count semantics of `search` are
/// built on this — an entry's score is the length of this set.
fn matched_terms(terms: &[String], fields: &[&str], body: &str) -> Vec<String> {
    let haystack: String = {
        let mut s = String::new();
        for f in fields {
            s.push_str(&f.to_lowercase());
            s.push(' ');
        }
        s.push_str(&body.to_lowercase());
        s
    };
    let mut matched = Vec::new();
    for t in terms {
        if haystack.contains(t.as_str()) {
            matched.push(t.clone());
        }
    }
    matched
}

/// The failure reason recorded when the catalog extractor exits
/// unsuccessfully: its stderr, led by a line naming the exit status when
/// that stderr holds no error at all.
///
/// Failure is decided by the exit status alone — a warning never fails a
/// rebuild. But a process that dies without printing an error (killed by
/// a signal, by the CLI's memory cap, by the OOM killer) leaves stderr
/// holding nothing except the build's warnings, and the STALE marker then
/// quoted a dead-code warning as the reason the rebuild failed. Leading
/// with the status keeps the warning from posing as the cause.
pub(crate) fn extractor_failure_message(status: &str, stderr: &str) -> String {
    if has_error_diagnostic(stderr) {
        return stderr.to_string();
    }
    let headline = format!(
        "catalog extractor exited with {status} without reporting an error (its output \
         holds only warnings or build progress) — it may have been killed, e.g. by a \
         signal or a memory limit"
    );
    if stderr.trim().is_empty() {
        headline
    } else {
        format!("{headline}\n{stderr}")
    }
}

/// Trim extractor stderr down to something worth putting in a tool
/// response: the error, not the noise around it. A cargo failure is
/// mostly warnings, and a warning is a multi-line block (`warning: …`,
/// ` --> file:line`, `12 | code`, `= note: …`); dropping only the
/// `warning:` header used to let the block's continuation lines through,
/// so a dead-code warning filled the budget ahead of the real error.
/// Errors (`error: …`, `error[E…]`, anyhow's `Error:` / `Caused by:`
/// chain) are preferred; failing those, whatever non-warning, non-progress
/// line remains; failing that, the raw head — never an empty reason.
fn trim_error(raw: &str) -> String {
    const MAX_LINES: usize = 4;
    const MAX_CHARS: usize = 600;
    // Colour codes (`CARGO_TERM_COLOR=always`) would hide a `warning:`
    // header from the prefix checks below.
    let clean = strip_ansi(raw);

    // Lines outside warning blocks and build progress.
    let mut kept: Vec<&str> = Vec::new();
    let mut in_warning = false;
    for line in clean.lines() {
        let line = line.trim();
        if line.is_empty() {
            in_warning = false;
            continue;
        }
        if is_diagnostic_header(line, "warning") {
            in_warning = true;
            continue;
        }
        if in_warning && is_diagnostic_continuation(line) {
            continue;
        }
        in_warning = false;
        if is_build_progress(line) {
            continue;
        }
        kept.push(line);
    }

    // The error lines among them: rustc/cargo error headers and their
    // location line, and anyhow's error + cause chain.
    let mut errors: Vec<&str> = Vec::new();
    let mut prev_was_error_header = false;
    let mut in_cause_chain = false;
    for line in &kept {
        let header = is_diagnostic_header(line, "error") || line.starts_with("Error:");
        if header || line.starts_with("Caused by:") {
            in_cause_chain = line.starts_with("Caused by:");
            if !in_cause_chain {
                errors.push(line);
            }
        } else if (prev_was_error_header && line.starts_with("-->"))
            || (in_cause_chain && !is_diagnostic_continuation(line))
        {
            errors.push(line);
        }
        prev_was_error_header = header;
    }

    let mut out: Vec<&str> = if !errors.is_empty() {
        errors
    } else if !kept.is_empty() {
        kept
    } else {
        clean.lines().map(str::trim).filter(|l| !l.is_empty()).collect()
    };
    out.truncate(MAX_LINES);
    let mut joined = out.join(" / ");
    if joined.chars().count() > MAX_CHARS {
        joined = joined.chars().take(MAX_CHARS).collect::<String>() + "…";
    }
    joined
}

/// Does `raw` (extractor stderr) contain an actual error, as opposed to
/// only warnings and progress?
fn has_error_diagnostic(raw: &str) -> bool {
    strip_ansi(raw).lines().map(str::trim).any(|l| {
        is_diagnostic_header(l, "error") || l.starts_with("Error:")
    })
}

/// `line` opens a rustc/cargo diagnostic of `level`: `warning: …`,
/// `warning[E0123]: …`, `error: …`, `error[E0432]: …`. A plain prose line
/// that merely begins with the word ("errors were found") is not one.
fn is_diagnostic_header(line: &str, level: &str) -> bool {
    line.strip_prefix(level)
        .is_some_and(|rest| rest.starts_with(':') || rest.starts_with('['))
}

/// A continuation line of a rustc diagnostic block — location, gutter,
/// numbered source, `= note`/`= help`, elision. Never a header.
fn is_diagnostic_continuation(line: &str) -> bool {
    line.starts_with("-->")
        || line.starts_with(":::")
        || line.starts_with('|')
        || line.starts_with('=')
        || line.starts_with("...")
        || line.starts_with('^')
        || {
            // `12 | fn sniff_content_type(...)` / `12 |`
            let digits = line.chars().take_while(|c| c.is_ascii_digit()).count();
            digits > 0 && line[digits..].trim_start().starts_with('|')
        }
}

/// Cargo's progress lines — never the reason for anything.
fn is_build_progress(line: &str) -> bool {
    ["Compiling ", "Checking ", "Finished ", "Blocking ", "Building ", "Fresh ", "Running "]
        .iter()
        .any(|p| line.starts_with(p))
}

/// Remove ANSI CSI escape sequences (`ESC [ … final-byte`).
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            // Parameter/intermediate bytes, then one final byte in 0x40..=0x7E.
            for c in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Does `origin` satisfy an optional `app` filter? An unset filter
/// matches everything. The filter accepts either spelling of a crate
/// name (`crewforge-main` or `crewforge_main`), since users read the
/// hyphenated package name in Cargo.toml but `module_path` reports the
/// underscored lib name.
fn app_matches(filter: Option<&str>, origin: &str) -> bool {
    match filter {
        None => true,
        Some(want) => want.replace('-', "_") == origin.replace('-', "_"),
    }
}

/// Outcome of resolving a component name against the catalog.
enum NameLookup<'a> {
    Found(&'a ComponentEntry),
    /// Several components share this short name. With one catalog
    /// spanning several projects this is reachable in ordinary use — two
    /// apps may each define an `AppRoot` — so it must be reported, not
    /// silently resolved to whichever entry happened to be first.
    Ambiguous(Vec<&'a ComponentEntry>),
    NotFound,
}

/// Resolve a component by fully-qualified name or short name.
///
/// A fully-qualified name is unambiguous by construction and wins
/// outright. A short name may match several components once the catalog
/// covers more than one project; `app` narrows the candidates first, and
/// anything still tied is reported as [`NameLookup::Ambiguous`].
fn find_by_name<'a>(
    cat: &'a ResolvedCatalog,
    needle: &str,
    app: Option<&str>,
) -> NameLookup<'a> {
    // Exact FQN first: the caller was specific, so honour it even if some
    // other crate has a component with the same short name.
    for e in cat.entries() {
        let fqn = format!("{}::{}", e.module_path, e.name);
        if fqn == needle {
            return NameLookup::Found(*e);
        }
    }
    let matches: Vec<&ComponentEntry> = cat
        .entries()
        .iter()
        .copied()
        .filter(|e| e.name == needle)
        .filter(|e| app_matches(app, mcp_catalog::origin_crate(e.module_path)))
        .collect();
    match matches.len() {
        0 => NameLookup::NotFound,
        1 => NameLookup::Found(matches[0]),
        _ => NameLookup::Ambiguous(matches),
    }
}

/// Turn a lookup into either the entry or an actionable MCP error.
///
/// The ambiguous case names every candidate and says exactly how to
/// disambiguate, because the alternative — picking one — produces a
/// confidently wrong answer that reads as correct.
fn resolve_component<'a>(
    cat: &'a ResolvedCatalog,
    needle: &str,
    app: Option<&str>,
    live_apps: &[String],
) -> Result<&'a ComponentEntry, McpError> {
    match find_by_name(cat, needle, app) {
        NameLookup::Found(e) => Ok(e),
        NameLookup::NotFound => Err(McpError::invalid_params(
            // `list_components` fans out across live Robot bridges; this
            // lookup does not, and that asymmetry is a real gap. Until it
            // closes, a miss must not be reported as a flat "not found"
            // when a live app was never consulted — that is a confidently
            // wrong answer, and the caller has no way to know it should
            // be asking somewhere else.
            if live_apps.is_empty() {
                format!("component {needle:?} not found")
            } else {
                format!(
                    "component {needle:?} not found in the project catalog. NOTE: {} live \
                     app(s) are registered ({}) and this lookup does NOT search them — \
                     only list_components does. If the component belongs to a running \
                     app, it may exist despite this answer.",
                    live_apps.len(),
                    live_apps.join(", "),
                )
            },
            None,
        )),
        NameLookup::Ambiguous(candidates) => {
            let mut fqns: Vec<String> = candidates
                .iter()
                .map(|e| format!("{}::{}", e.module_path, e.name))
                .collect();
            fqns.sort();
            Err(McpError::invalid_params(
                format!(
                    "component {needle:?} is ambiguous — {} components share that name: {}. \
                     Pass one of those fully-qualified names, or narrow with the `app` \
                     argument (one of: {}).",
                    fqns.len(),
                    fqns.join(", "),
                    {
                        let mut apps: Vec<&str> = candidates
                            .iter()
                            .map(|e| mcp_catalog::origin_crate(e.module_path))
                            .collect();
                        apps.sort_unstable();
                        apps.dedup();
                        apps.join(", ")
                    }
                ),
                None,
            ))
        }
    }
}

/// Best-effort guide → recipe join for `read_guide`'s "Related recipes"
/// footer. The catalog carries no explicit guide↔recipe link, so this is a
/// fuzzy name join: a recipe is related when its `target`, its full name,
/// or one of its `_`-separated name words matches the guide's slug or one
/// of its tags (case-insensitive). On top of that sit curated pins for
/// joins the name data can't express — `keyed_list_add_remove` targets the
/// `ui` macro but IS the reactivity guide's central "iterate the Signal,
/// not `.get()`" example (the exact recipe the arena debug-fix agent never
/// found). Pins go through the same registered-recipe scan, so the footer
/// never advertises a name `describe_recipe` would 404 on.
fn related_recipe_names(cat: &ResolvedCatalog, guide: &mcp_catalog::GuideEntry) -> Vec<&'static str> {
    let mut keys: Vec<String> = guide.tags.iter().map(|t| t.to_lowercase()).collect();
    keys.push(guide.slug.to_lowercase());
    let pinned: &[&str] = if guide.slug.starts_with("reactivity") {
        &["keyed_list_add_remove"]
    } else {
        &[]
    };
    let mut out: Vec<&'static str> = cat
        .recipes()
        .iter()
        .filter(|r| {
            let target = r.target.to_lowercase();
            let name = r.name.to_lowercase();
            pinned.contains(&r.name)
                || keys.iter().any(|k| {
                    *k == target || *k == name || name.split('_').any(|w| w == k)
                })
        })
        .map(|r| r.name)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Recipes targeting or using `name`, shaped for a `describe_*` payload.
/// Kind-agnostic (works for components, utilities, types, …) via the
/// catalog's `recipes_for` join.
fn recipes_json_for(cat: &ResolvedCatalog, name: &str) -> Vec<serde_json::Value> {
    cat.recipes_for(name)
        .iter()
        .map(|r| {
            serde_json::json!({
                "name": r.name,
                "fqn": format!("{}::{}", r.module_path, r.name),
                "primary": r.target == name,
                "docs": r.docs,
                "source": r.source,
            })
        })
        .collect()
}

fn entry_to_json(
    cat: &ResolvedCatalog,
    entry: &ComponentEntry,
    edges: &[mcp_catalog::ResolvedEdge],
) -> serde_json::Value {
    let composes: Vec<serde_json::Value> = edges
        .iter()
        .map(|edge| {
            let (status, resolved) = match &edge.status {
                EdgeStatus::Resolved { target } => ("resolved", Some(target.fqn())),
                EdgeStatus::NoMatch => ("unresolved", None),
                EdgeStatus::Ambiguous { .. } => ("ambiguous", None),
            };
            serde_json::json!({
                "raw_name": edge.raw_name,
                "line": edge.line,
                "status": status,
                "resolved_fqn": resolved,
            })
        })
        .collect();
    let params: Vec<serde_json::Value> = entry
        .params
        .iter()
        .map(|p| {
            let mut obj = serde_json::Map::new();
            obj.insert("name".into(), p.name.into());
            obj.insert("type".into(), p.type_str.into());
            obj.insert("type_short_name".into(), p.type_short_name.into());
            // Inline the props struct's per-field docs when the
            // parameter's type matches a documented props struct
            // (`#[derive(IdealystSchema)]`). This is the prop-level
            // documentation surface.
            if !p.type_short_name.is_empty() {
                match prop_fields_for(cat, p.type_short_name, entry.module_path) {
                    Some(fields) => {
                        obj.insert("schema".into(), serde_json::json!(fields));
                    }
                    // A `*Props`-shaped param with no catalogued schema means
                    // the props struct isn't annotated with
                    // `#[derive(IdealystSchema)]`, so per-field docs aren't
                    // available. Flag it inline so a consumer knows the field
                    // docs are absent and does NOT dead-end calling
                    // `describe_type` on it (which would 404). Only flagged
                    // for the `*Props` convention to avoid noise on plain
                    // value params (`&str`, `i32`, …).
                    None if p.type_short_name.ends_with("Props") => {
                        obj.insert("props_documented".into(), serde_json::json!(false));
                        obj.insert(
                            "note".into(),
                            serde_json::json!(format!(
                                "`{}` has no catalogued field docs — its struct isn't \
                                 annotated with `#[derive(IdealystSchema)]` in the defining \
                                 crate. The component is usable; per-prop docs are unavailable.",
                                p.type_short_name
                            )),
                        );
                    }
                    None => {}
                }
            }
            serde_json::Value::Object(obj)
        })
        .collect();
    // Compile-checked usage examples for this component — the rich
    // "how do I use it" context. A recipe is linked if it primarily
    // demonstrates this component OR merely uses it in its body.
    let recipes = recipes_json_for(cat, entry.name);
    serde_json::json!({
        "name": entry.name,
        "module_path": entry.module_path,
        "fqn": format!("{}::{}", entry.module_path, entry.name),
        "file": entry.file,
        "line": entry.line,
        "docs": entry.docs,
        "scope": cat.scope_for(entry.module_path).map(|s| s.slug),
        "params": params,
        "composes": composes,
        "recipes": recipes,
    })
}

/// Resolve a component param's props struct to its per-field docs
/// (`{ name, type, doc, constraint }`), for inlining under a param's
/// `schema`. `short_name` is the param type's bare ident (e.g.
/// `ButtonProps`); `near` is the component's module path, which picks
/// between structs that share the name.
///
/// Prefers the loaded catalog's `types` slice — that's the path that
/// survives the wire: a project's `#[derive(IdealystSchema)]` props
/// struct is serialized into the catalog JSON as a `TypeEntry { Struct }`
/// and rebuilt here by `build_from_json`. Falls back to the server's own
/// in-process `PropsSchemaEntry` inventory for the offline / in-binary
/// case. Returns `None` when the props struct isn't documented (no
/// `IdealystSchema` derive) so the param simply carries its type.
fn prop_fields_for(
    cat: &ResolvedCatalog,
    short_name: &str,
    near: &str,
) -> Option<Vec<serde_json::Value>> {
    // Wire path: the props struct as a documented `TypeEntry` — of the
    // structs with this name, the one nearest the component
    // (`nearest_by_module`; apps reuse props-struct names across modules).
    let structs = cat
        .types()
        .iter()
        .copied()
        .filter(|t| t.short_name == short_name && matches!(t.shape, mcp_catalog::TypeShape::Struct { .. }));
    if let Some(t) = mcp_catalog::nearest_by_module(structs, |t| t.module_path, near) {
        if let mcp_catalog::TypeShape::Struct { fields } = &t.shape {
            return Some(
                fields
                    .iter()
                    .map(|f| {
                        serde_json::json!({
                            "name": f.name,
                            "type": f.type_str,
                            "doc": f.doc,
                            "constraint": f.constraint,
                        })
                    })
                    .collect(),
            );
        }
    }
    // In-process fallback (server binary compiled with the components).
    let schema = mcp_catalog::nearest_schema(short_name, near)?;
    Some(
        schema
            .fields
            .iter()
            .map(|f| {
                serde_json::json!({
                    "name": f.name,
                    "type": f.type_str,
                    "doc": f.doc,
                    "constraint": f.constraint,
                })
            })
            .collect(),
    )
}

fn type_entry_json(t: &mcp_catalog::TypeEntry) -> serde_json::Value {
    let (kind, shape_body): (&str, serde_json::Value) = match &t.shape {
        mcp_catalog::TypeShape::Struct { fields } => {
            let fs: Vec<serde_json::Value> = fields
                .iter()
                .map(|f| {
                    serde_json::json!({
                        "name": f.name,
                        "type": f.type_str,
                        "doc": f.doc,
                        "constraint": f.constraint,
                    })
                })
                .collect();
            ("struct", serde_json::json!({ "fields": fs }))
        }
        mcp_catalog::TypeShape::Enum { variants } => {
            let vs: Vec<serde_json::Value> = variants
                .iter()
                .map(|v| {
                    let payload: Vec<serde_json::Value> = v
                        .payload
                        .iter()
                        .map(|f| {
                            serde_json::json!({
                                "name": f.name,
                                "type": f.type_str,
                                "doc": f.doc,
                                "constraint": f.constraint,
                            })
                        })
                        .collect();
                    serde_json::json!({
                        "name": v.name,
                        "docs": v.docs,
                        "payload": payload,
                    })
                })
                .collect();
            ("enum", serde_json::json!({ "variants": vs }))
        }
    };
    serde_json::json!({
        "short_name": t.short_name,
        "module_path": t.module_path,
        "fqn": format!("{}::{}", t.module_path, t.short_name),
        "docs": t.docs,
        "kind": kind,
        "shape": shape_body,
    })
}

/// Pull a ±80-char window around the first occurrence of `needle`
/// inside `docs`. Returns the first 160 chars if no match (defensive
/// — caller already filtered).
/// First non-empty line of a doc string, trimmed and truncated to a
/// short one-liner for list views. Full docs come from `describe_*`.
fn doc_summary(docs: &str) -> String {
    const MAX: usize = 120;
    let line = docs
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if line.chars().count() > MAX {
        let mut s: String = line.chars().take(MAX - 1).collect();
        s.push('…');
        s
    } else {
        line.to_string()
    }
}

/// Canonical form for icon-name matching: lowercase, with `_` and
/// whitespace folded to `-`. Icon names are kebab-case (`qr-code`) while
/// the constants an author reads are SCREAMING_SNAKE (`QR_CODE`); folding
/// both onto one spelling lets either form of query find the icon.
fn normalize_icon_key(s: &str) -> String {
    s.trim()
        .chars()
        .map(|c| if c == '_' || c.is_whitespace() { '-' } else { c.to_ascii_lowercase() })
        .collect()
}

/// Does `set` answer to the pack name `want`? Accepts every spelling a
/// caller plausibly has in hand: the crate name (`icons-lucide`), the
/// title (`Lucide`), the import path (`icons_lucide`) and the bare family
/// with the `icons-` prefix dropped (`lucide`). Case- and
/// separator-insensitive. Deliberately NOT a substring match — `set` is a
/// filter, and `"icons"` silently matching every pack would hide a typo.
fn icon_set_matches(set: &mcp_catalog::IconSetEntry, want: &str) -> bool {
    let want = normalize_icon_key(want);
    if want.is_empty() {
        return false;
    }
    [set.name, set.title, set.import_path].iter().any(|candidate| {
        let c = normalize_icon_key(candidate);
        c == want || c.strip_prefix("icons-") == Some(want.as_str())
    })
}

/// The error for a pack filter that matched nothing, naming the packs
/// that DO exist so the caller can retry without another round trip.
fn unknown_icon_set(want: &str, sets: &[&'static mcp_catalog::IconSetEntry]) -> McpError {
    let available: Vec<&str> = sets.iter().map(|s| s.name).collect();
    McpError::invalid_params(
        format!(
            "icon set {want:?} not found; available packs: {}",
            if available.is_empty() { "(none)".to_string() } else { available.join(", ") }
        ),
        None,
    )
}

/// Robust catalog filter shared by the `list_*` tools. Case-insensitive;
/// ANDs across whitespace-separated terms; each term matches if it is a
/// substring of ANY supplied field, or a `*`-glob match when the term
/// contains a `*`. An empty / absent filter matches everything.
fn matches_filter(filter: &Option<String>, fields: &[&str]) -> bool {
    let Some(q) = filter.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return true;
    };
    let lowered: Vec<String> = fields.iter().map(|f| f.to_lowercase()).collect();
    q.split_whitespace().all(|term| {
        let term = term.to_lowercase();
        lowered.iter().any(|f| {
            if term.contains('*') {
                glob_match(&term, f)
            } else {
                f.contains(&term)
            }
        })
    })
}

/// Minimal `*`-glob matcher, anchored to the whole string. `*` matches
/// any run of characters (including empty); every other char matches
/// literally. Case is the caller's responsibility (both args lowered).
/// Two-pointer match with star-backtracking — no regex dep.
fn glob_match(pat: &str, text: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (None, 0usize);
    while ti < t.len() {
        if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

fn docs_excerpt_around(docs: &str, needle: &str) -> String {
    let lower = docs.to_lowercase();
    let Some(pos) = lower.find(needle) else {
        return docs.chars().take(160).collect();
    };
    // `pos` is an offset into `lower`, which only approximates an offset
    // into `docs` (lowercasing can change byte lengths, e.g. 'İ' → "i̇"),
    // and ±80 can land inside a multi-byte char. Clamp into range and
    // snap outward to char boundaries — the raw `docs[pos-80..pos+80]`
    // slice panicked (process-fatal under panic=abort) whenever the
    // window split a '—' or '…' in the docs.
    let mut start = pos.saturating_sub(80).min(docs.len());
    while !docs.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (pos + needle.len() + 80).min(docs.len());
    while !docs.is_char_boundary(end) {
        end += 1;
    }
    docs[start..end].to_string()
}

// `router = self.tool_router`: the macro's default, `Self::tool_router()`,
// rebuilds the raw router on every list/call, which would advertise the
// unminified schemas and bypass `advertised_router` entirely.
#[tool_handler(router = self.tool_router)]
impl ServerHandler for CatalogService {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::from_build_env())
        .with_instructions({
            // Keyed on the MODE, not on `self.robot`: that field only
            // holds the PINNED bridge of `--robot-port`, and is None in
            // the default Discovery mode — where the robot tools are
            // fully live, routing per call through `~/.idealyst/apps/`.
            let robot_status = match &self.robot_mode {
                RobotMode::Discovery => " Robot tools drive running apps found via `list_apps`; their `app` argument is that list's `selector`, optional when one app is up.",
                RobotMode::Explicit { .. } => " Robot tools drive the one bridge pinned by --robot-port.",
                RobotMode::Disabled => " Robot tools are disabled (--no-robot).",
            };
            // The tool list itself names every tool; this only orients a
            // caller and points at get_docs, so it stays a few sentences.
            format!(
                "Idealyst framework MCP: query the project's catalog (list_* / describe_* / search), \
                 drive running apps (Robot tools), run `idealyst dev`, and lint/configure projects. \
                 Tool descriptions are summaries: call get_docs(name) for a tool's full parameters, \
                 return shape and caveats, or get_docs() for the topic index. \
                 On catalog tools `app` restricts results to one crate.{robot_status}"
            )
        })
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult {
            resources: vec![
                RawResource::new("idealyst://catalog", "catalog".to_string())
                    .no_annotation(),
            ],
            next_cursor: None,
            meta: None,
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        match request.uri.as_str() {
            "idealyst://catalog" => {
                // Serve every catalog slice (v2 schema) from the
                // in-memory `ResolvedCatalog`. The watcher may have
                // swapped the catalog underneath us; reading through
                // the lock guarantees a consistent snapshot.
                let cat = self.catalog.read().await;
                let mut entries: Vec<&ComponentEntry> = cat.entries().to_vec();
                entries.sort_by_key(|e| (e.module_path, e.name));
                let components: Vec<serde_json::Value> = entries
                    .iter()
                    .map(|e| {
                        let edges = cat.dependencies(&EntryRef::of(e));
                        entry_to_json(&cat, e, edges)
                    })
                    .collect();

                let primitives: Vec<serde_json::Value> = cat
                    .primitives()
                    .iter()
                    .map(|p| {
                        let props: Vec<serde_json::Value> = p
                            .props
                            .iter()
                            .map(|f| {
                                serde_json::json!({
                                    "name": f.name,
                                    "type": f.type_str,
                                    "doc": f.doc,
                                    "constraint": f.constraint,
                                })
                            })
                            .collect();
                        serde_json::json!({
                            "name": p.name,
                            "pascal_name": p.pascal_name,
                            "docs": p.docs,
                            "category": p.category.as_str(),
                            "backends": p.backends,
                            "props": props,
                        })
                    })
                    .collect();

                let utilities: Vec<serde_json::Value> = cat
                    .utilities()
                    .iter()
                    .map(|u| {
                        let params: Vec<serde_json::Value> = u
                            .params
                            .iter()
                            .map(|p| {
                                serde_json::json!({
                                    "name": p.name,
                                    "type": p.type_str,
                                    "type_short_name": p.type_short_name,
                                })
                            })
                            .collect();
                        serde_json::json!({
                            "name": u.name,
                            "module_path": u.module_path,
                            "fqn": format!("{}::{}", u.module_path, u.name),
                            "docs": u.docs,
                            "params": params,
                            "return_type": u.return_type,
                            "return_type_short": u.return_type_short,
                            "category": u.category.as_str(),
                            "snippet": u.snippet,
                        })
                    })
                    .collect();

                let states: Vec<serde_json::Value> = cat
                    .states()
                    .iter()
                    .map(|s| {
                        serde_json::json!({
                            "name": s.name,
                            "docs": s.docs,
                            "backends": s.backends,
                        })
                    })
                    .collect();

                let guides: Vec<serde_json::Value> = cat
                    .guides()
                    .iter()
                    .map(|g| {
                        serde_json::json!({
                            "slug": g.slug,
                            "title": g.title,
                            "order": g.order,
                            "tags": g.tags,
                            "body": g.body,
                        })
                    })
                    .collect();

                let methods: Vec<serde_json::Value> = cat
                    .methods()
                    .iter()
                    .map(|m| {
                        let params: Vec<serde_json::Value> = m
                            .params
                            .iter()
                            .map(|p| {
                                serde_json::json!({
                                    "name": p.name,
                                    "type": p.type_str,
                                    "type_short_name": p.type_short_name,
                                })
                            })
                            .collect();
                        serde_json::json!({
                            "parent_module_path": m.parent_module_path,
                            "parent_name": m.parent_name,
                            "parent_fqn": format!("{}::{}", m.parent_module_path, m.parent_name),
                            "name": m.name,
                            "docs": m.docs,
                            "params": params,
                            "return_type": m.return_type,
                        })
                    })
                    .collect();

                let animations: Vec<serde_json::Value> = cat
                    .animations()
                    .iter()
                    .map(|a| {
                        serde_json::json!({
                            "parent_module_path": a.parent_module_path,
                            "parent_name": a.parent_name,
                            "parent_fqn": format!("{}::{}", a.parent_module_path, a.parent_name),
                            "binding": a.binding,
                            "initial": a.initial,
                            "line": a.line,
                        })
                    })
                    .collect();

                let types: Vec<serde_json::Value> = cat
                    .types()
                    .iter()
                    .map(|t| type_entry_json(t))
                    .collect();

                let tools: Vec<serde_json::Value> = cat
                    .tools()
                    .iter()
                    .map(|t| {
                        let params: Vec<serde_json::Value> = t
                            .params
                            .iter()
                            .map(|p| {
                                serde_json::json!({
                                    "name": p.name,
                                    "type": p.type_str,
                                    "type_short_name": p.type_short_name,
                                })
                            })
                            .collect();
                        serde_json::json!({
                            "name": t.name,
                            "module_path": t.module_path,
                            "fqn": format!("{}::{}", t.module_path, t.name),
                            "file": t.file,
                            "line": t.line,
                            "docs": t.docs,
                            "params": params,
                            "return_type": t.return_type,
                        })
                    })
                    .collect();

                let sdks: Vec<serde_json::Value> = cat
                    .sdks()
                    .iter()
                    .map(|s| {
                        serde_json::json!({
                            "name": s.name,
                            "summary": s.summary,
                            "dep_line": s.dep_line,
                            "category": s.category.as_str(),
                            "kind": s.kind.as_str(),
                            "guide": s.guide,
                        })
                    })
                    .collect();

                let body = serde_json::json!({
                    "catalog_version": 2,
                    // Freshness travels with the resource too. It serves
                    // the same catalog the tools do, so it can be just as
                    // stale or as empty — and a consumer parsing this
                    // document has no other way to tell an unbuilt
                    // catalog from a project with nothing in it.
                    "catalog_status": self.status().await.to_json(),
                    "components": components,
                    "primitives": primitives,
                    "utilities": utilities,
                    "states": states,
                    "guides": guides,
                    "methods": methods,
                    "animations": animations,
                    "types": types,
                    "tools": tools,
                    "sdks": sdks,
                });
                let text = serde_json::to_string_pretty(&body).unwrap();
                Ok(ReadResourceResult::new(vec![ResourceContents::text(
                    text,
                    request.uri.clone(),
                )]))
            }
            other => Err(McpError::invalid_params(
                format!("unknown resource {:?}", other),
                None,
            )),
        }
    }

}

/// `wait_for_app`'s timeout, by what the wait actually saw: apps that
/// registered but never answered a ping (the page/process hasn't dialed in),
/// or no registration at all — where the apps dir's state, and the dev log,
/// say why. One fixed message blamed the build in both cases, sending an
/// agent to a build that had finished while the real cause (nothing could
/// register) went unnamed.
fn wait_for_app_timeout_message(
    target: &str,
    secs: u64,
    registered: &[String],
    apps_dir_note: &str,
) -> String {
    if !registered.is_empty() {
        return format!(
            "timed out after {secs}s: {} registered but never answered a bridge ping, so \
             it can't be driven yet. The relay is up; the app hasn't dialed in. For a WEB \
             target, run_dev needs local=true (the relay only exists in local mode) and \
             something must load the served page — on a display-less machine dev \
             auto-launches a headless browser client (or pass headless_client=true). \
             read_dev_log shows the page's progress.",
            registered.join(", ")
        );
    }
    format!(
        "timed out after {secs}s: no registration for {target} appeared — {apps_dir_note}. \
         If you launched it with run_dev, read_dev_log shows whether the build is still \
         running or failed, and warns if the relay could not write its registration."
    )
}

#[cfg(test)]
mod tests {
    //! Tools-level regression: a `#[component]` linked into this
    //! test binary must appear in the `list_components` tool's
    //! response.
    //!
    //! Pre-fix the sidecar wrapper omitted the `dev` feature, so
    //! `#[component]` macro emissions were stubbed out and the
    //! linked inventory was empty even though the user's source had
    //! components in it. `idealyst mcp` returned `[]` in runtime-
    //! server mode. This test wouldn't catch that exact wrapper-
    //! shape bug (the wrapper-shape regression in
    //! `build-runtime-server::regression_tests` does), but it locks
    //! down the OTHER half of the contract: given the dev feature
    //! is on, the catalog → tool surface really does carry the
    //! component through.

    use super::*;
    use runtime_core::Element;
    use runtime_macros::component;

    /// A component whose name we look for in the tool's JSON
    /// response. The body returns an empty view; we never actually
    /// mount it — the inventory entry is produced at macro
    /// expansion time and registered at link.
    #[allow(dead_code)]
    #[component]
    pub fn list_components_regression_canary() -> Element {
        // `view(..)` returns a builder, not an `Element` — coerce it.
        ::runtime_core::IntoElement::into_element(::runtime_core::view(::std::vec::Vec::new()))
    }

    /// Parse a `list_*` tool result into `(name, has_summary, keys)`
    /// triples so tests can assert on shape + contents.
    fn component_names(result: &CallToolResult) -> Vec<String> {
        // Last text block — a freshness marker may precede the payload.
        let payload = result
            .content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.clone()))
            .next_back()
            .expect("tool result has a text content block");
        let v: serde_json::Value =
            serde_json::from_str(&payload).expect("response is valid JSON");
        v.as_array()
            .expect("response is a JSON array")
            .iter()
            .filter_map(|e| e["name"].as_str().map(str::to_string))
            .collect()
    }

    #[tokio::test]
    async fn list_components_returns_in_process_catalog_components() {
        // Discovery DISABLED: `list_components` serves a live app's catalog
        // whenever one is registered in `~/.idealyst/apps/`, falling back to
        // the in-process catalog only when none is. This test asserts the
        // in-process path, so it must not depend on whether a `idealyst dev`
        // session happens to be running (which would otherwise make it flaky —
        // it'd serve that app's catalog, sans the test canary).
        let svc = CatalogService::with_robot_mode(false, None);
        let result = svc
            .list_components(Parameters(FilterRequest::default()))
            .await
            .expect("list_components tool call succeeds");
        let names = component_names(&result);
        assert!(
            names.contains(&"list_components_regression_canary".to_string()),
            "expected canary component in list_components output; got {:?}",
            names,
        );
    }

    #[tokio::test]
    async fn list_components_output_is_lightweight_and_filterable() {
        // Discovery disabled — see the note in
        // `list_components_returns_in_process_catalog_components`: this asserts
        // the in-process catalog, so it must not race a live dev app.
        let svc = CatalogService::with_robot_mode(false, None);

        // Lightweight: entries carry a `summary`, not full `docs`, and
        // no file/line/module_path noise.
        let all = svc
            .list_components(Parameters(FilterRequest::default()))
            .await
            .unwrap();
        let payload = all
            .content
            .iter()
            .find_map(|c| c.as_text().map(|t| t.text.clone()))
            .unwrap();
        let arr: serde_json::Value = serde_json::from_str(&payload).unwrap();
        let first = &arr.as_array().unwrap()[0];
        assert!(first.get("summary").is_some(), "list entry has a summary");
        assert!(first.get("docs").is_none(), "list entry omits full docs");
        assert!(first.get("file").is_none(), "list entry omits file");
        assert!(first.get("line").is_none(), "list entry omits line");

        // Filter that matches the canary keeps it…
        let hit = svc
            .list_components(Parameters(FilterRequest {
                filter: Some("regression_canary".into()),
                app: None,
            }))
            .await
            .unwrap();
        assert!(component_names(&hit)
            .contains(&"list_components_regression_canary".to_string()));

        // …a non-matching filter excludes it (and yields fewer rows).
        let miss = svc
            .list_components(Parameters(FilterRequest {
                filter: Some("definitely-no-such-component-xyz".into()),
                app: None,
            }))
            .await
            .unwrap();
        assert!(!component_names(&miss)
            .contains(&"list_components_regression_canary".to_string()));
    }

    /// Parse a tool's JSON-array result into a `serde_json::Value`.
    /// The JSON payload of a tool result.
    ///
    /// Catalog tools may prepend a freshness marker as its own content
    /// block, so the payload is the LAST text block — and it must still
    /// be valid JSON on its own, which is the point of keeping the marker
    /// separate.
    fn parse_array(result: &CallToolResult) -> serde_json::Value {
        let payload = result
            .content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.clone()))
            .next_back()
            .expect("tool result has a text content block");
        serde_json::from_str(&payload).expect("response is valid JSON")
    }

    /// A2 regression: a multi-word `search` query must tokenize + OR
    /// across terms instead of demanding the whole phrase as one
    /// contiguous substring (which returned `[]` for ANY multi-word
    /// query). The in-process catalog carries the framework macros /
    /// guides, so a query whose terms hit different slices returns
    /// ranked hits, with the best-matching entry (most distinct terms)
    /// ranked first.
    #[tokio::test]
    async fn search_multi_word_query_tokenizes_and_ranks() {
        let svc = CatalogService::new();

        // Single-word still works (the old behavior is a subset).
        let one = svc
            .search(Parameters(SearchRequest { query: "effect".into(), app: None }))
            .await
            .unwrap();
        let ov = parse_array(&one);
        assert!(
            ov.as_array().map(|a| !a.is_empty()).unwrap_or(false),
            "single-word search still returns hits; got {ov}",
        );

        // Multi-word: terms drawn from the `effect!` macro docs +
        // names. Pre-fix this returned `[]` because no single field
        // contains the literal phrase "effect signal reactive".
        let many = svc
            .search(Parameters(SearchRequest {
                query: "effect signal reactive".into(),
                app: None,
            }))
            .await
            .unwrap();
        let mv = parse_array(&many);
        let arr = mv.as_array().expect("array");
        assert!(!arr.is_empty(), "multi-word search must not be empty; got {mv}");

        // Results are ranked: every entry carries a numeric score and a
        // matched_terms list, and the list is sorted best-first.
        let first = &arr[0];
        assert!(first["score"].as_u64().unwrap_or(0) >= 1, "hit carries a score; {first}");
        assert!(first["matched_terms"].is_array(), "hit lists matched terms; {first}");
        let scores: Vec<u64> = arr
            .iter()
            .map(|h| h["score"].as_u64().unwrap_or(0))
            .collect();
        assert!(
            scores.windows(2).all(|w| w[0] >= w[1]),
            "search results must be ranked best-first; got scores {scores:?}",
        );
    }

    /// Regression: `search` aborted the whole MCP server ("MCP error
    /// -32000: Connection closed" client-side, since the release profile
    /// is panic=abort) whenever the ±80-byte excerpt window around a hit
    /// split a multi-byte char — the framework docs are full of '—' and
    /// '…', so this fired on ordinary queries. `docs_excerpt_around`
    /// sliced `docs[pos-80..pos+len+80]` at raw byte offsets, and also
    /// indexed `docs` with an offset found in `docs.to_lowercase()`,
    /// whose byte offsets can drift (e.g. 'İ' lowercases to 2 chars).
    #[test]
    fn regression_search_excerpt_multibyte_no_panic() {
        // Window START lands mid-'—': needle at byte 180, start = 100,
        // and 100 is not a char boundary in a run of 3-byte dashes.
        let leading = format!("{}needle", "—".repeat(60));
        assert!(docs_excerpt_around(&leading, "needle").contains("needle"));

        // Window END lands mid-'—': end = 86 falls inside a dash.
        let trailing = format!("needle {}", "—".repeat(40));
        assert!(docs_excerpt_around(&trailing, "needle").contains("needle"));

        // Lowercased offset drift: 'İ' (2 bytes) lowercases to "i̇"
        // (3 bytes), so the match offset in `lower` can exceed
        // `docs.len()` entirely — an out-of-bounds slice pre-fix.
        let drifted = format!("{}needle", "İ".repeat(100));
        let _ = docs_excerpt_around(&drifted, "needle");
    }

    /// A2 (coverage half): the search index must span more than guide
    /// prose — a query that only matches a macro name surfaces a `macro`
    /// kind, proving non-guide slices are indexed.
    /// Regression: searching for a PROP name must surface the primitives
    /// that take it. `on_end_reached` was listed on `scroll_view` and the
    /// virtualizer and still unfindable, because the primitive slice
    /// indexed only the name and the summary. That is the MCP-side half
    /// of 47d014a2 — an author who cannot find the setter reaches for a
    /// spelling that silently does nothing.
    #[tokio::test]
    async fn search_finds_a_primitive_by_one_of_its_props() {
        let svc = CatalogService::new();
        let r = svc
            .search(Parameters(SearchRequest { query: "on_end_reached".into(), app: None }))
            .await
            .unwrap();
        let v = parse_array(&r);
        let prims: Vec<&str> = v
            .as_array()
            .unwrap()
            .iter()
            .filter(|h| h["kind"] == "primitive")
            .filter_map(|h| h["name"].as_str())
            .collect();
        assert!(prims.contains(&"scroll_view"), "scroll_view takes it; got {prims:?}");
        assert!(prims.contains(&"flat_list"), "flat_list takes it; got {prims:?}");

        // And by what the prop is FOR, not only its identifier.
        let r = svc
            .search(Parameters(SearchRequest { query: "infinite paging".into(), app: None }))
            .await
            .unwrap();
        let v = parse_array(&r);
        let prims: Vec<&str> = v
            .as_array()
            .unwrap()
            .iter()
            .filter(|h| h["kind"] == "primitive")
            .filter_map(|h| h["name"].as_str())
            .collect();
        assert!(prims.contains(&"scroll_view"), "got {prims:?}");
    }

    #[tokio::test]
    async fn search_covers_non_guide_slices() {
        let svc = CatalogService::new();
        let r = svc
            .search(Parameters(SearchRequest { query: "stylesheet".into(), app: None }))
            .await
            .unwrap();
        let v = parse_array(&r);
        let kinds: Vec<&str> = v
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|h| h["kind"].as_str())
            .collect();
        assert!(
            kinds.contains(&"macro"),
            "search should index the macros slice (stylesheet! macro); kinds {kinds:?}",
        );
    }

    /// A5 regression: `list_macros` must not be empty and `describe_macro`
    /// must resolve a known macro — the field-report symptom was `[]` /
    /// "macro not found" at the SERVER-TOOL layer even though the slice
    /// existed.
    #[tokio::test]
    async fn list_and_describe_macros_are_populated() {
        let svc = CatalogService::new();
        let listed = svc
            .list_macros(Parameters(FilterRequest::default()))
            .await
            .unwrap();
        let lv = parse_array(&listed);
        let names: Vec<&str> = lv
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["name"].as_str())
            .collect();
        assert!(names.contains(&"ui"), "list_macros must include `ui`; got {names:?}");
        // Signal creation is the plain `signal(value)` function now — it
        // moved to the utilities table when the `signal!` macro was
        // removed, so the macros list must NOT carry it.
        assert!(
            !names.contains(&"signal"),
            "the removed `signal!` macro must not be listed; got {names:?}"
        );

        // describe_macro resolves bare name AND a trailing `!`.
        for needle in ["ui", "effect!"] {
            let d = svc
                .describe_macro(Parameters(NameRequest { name: needle.into(), app: None }))
                .await
                .unwrap_or_else(|e| panic!("describe_macro({needle:?}) errored: {e:?}"));
            let dv = parse_array(&d);
            assert!(dv["name"].is_string(), "describe_macro({needle:?}) returns a record; {dv}");
        }

        // The paste-ready snippet rides along for macros and utilities —
        // it's what the editor inserts, so an agent sees the same shape.
        let d = svc
            .describe_macro(Parameters(NameRequest { name: "effect".into(), app: None }))
            .await
            .unwrap();
        assert_eq!(parse_array(&d)["snippet"], "effect!({\n\t$0\n});");
        let u = svc
            .describe_utility(Parameters(NameRequest { name: "signal".into(), app: None }))
            .await
            .unwrap();
        assert_eq!(parse_array(&u)["snippet"], "let ${1:name} = signal(${2:value});");
    }

    /// A1 regression: the opt-in SDK crates are discoverable through the
    /// server tools — `list_sdks` returns the data-layer crates and
    /// `describe_sdk` hands back the dep line an agent needs to add them.
    #[tokio::test]
    async fn list_and_describe_sdks_expose_non_ui_crates() {
        let svc = CatalogService::new();
        let listed = svc
            .list_sdks(Parameters(FilterRequest::default()))
            .await
            .unwrap();
        let lv = parse_array(&listed);
        let names: Vec<&str> = lv
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|s| s["name"].as_str())
            .collect();
        for required in ["net", "storage", "credentials", "server"] {
            assert!(
                names.contains(&required),
                "list_sdks must surface `{required}`; got {names:?}",
            );
        }

        let d = svc
            .describe_sdk(Parameters(NameRequest { name: "net".into(), app: None }))
            .await
            .unwrap();
        let dv = parse_array(&d);
        assert_eq!(dv["name"], "net");
        assert!(
            dv["dep_line"].as_str().unwrap_or("").contains("net ="),
            "describe_sdk returns a copy-pasteable dep line; {dv}",
        );

        // A bad name is an actionable error, not a silent empty.
        let err = svc
            .describe_sdk(Parameters(NameRequest { name: "nope".into(), app: None }))
            .await;
        assert!(err.is_err(), "describe_sdk on an unknown crate errors");

        // Filter narrows to the data crates.
        let filtered = svc
            .list_sdks(Parameters(FilterRequest { filter: Some("data".into()), app: None }))
            .await
            .unwrap();
        let fv = parse_array(&filtered);
        assert!(
            fv.as_array().unwrap().iter().all(|s| s["category"] == "data"),
            "data filter yields only data-category SDKs; {fv}",
        );
    }

    /// Icon packs are discoverable and paginated/searchable through the
    /// server tools. Built from a synthetic catalog (icon packs
    /// self-register from `icons-lucide`, which the standalone server
    /// doesn't link) injected via `replace_catalog`.
    #[tokio::test]
    async fn list_values_filters_by_prop_type_and_spells_call_site_form() {
        let doc = serde_json::json!({
            "components": [],
            "values": [
                { "short_name": "Primary", "module_path": "idea_theme::extensible::tone",
                  "docs": "Built-in semantic tone.", "value_of": "ToneRef", "via": "idea_ui::tone" },
                { "short_name": "Soft", "module_path": "idea_theme::extensible::variant",
                  "docs": "", "value_of": "VariantRef", "via": "idea_ui::variant" },
            ],
        });
        let cat = ResolvedCatalog::build_from_json(&doc.to_string()).unwrap();
        let svc = CatalogService::new();
        svc.replace_catalog(cat).await;

        let all = parse_array(&svc.list_values(Parameters(FilterRequest::default())).await.unwrap());
        assert_eq!(all.as_array().unwrap().len(), 2);

        // The filter is the join an agent actually has: the prop's type.
        let tones = parse_array(
            &svc.list_values(Parameters(FilterRequest { filter: Some("ToneRef".into()), ..Default::default() }))
                .await
                .unwrap(),
        );
        let tones = tones.as_array().unwrap();
        assert_eq!(tones.len(), 1);
        assert_eq!(tones[0]["spelled"], "tone::Primary");
        assert_eq!(tones[0]["import"], "idea_ui::tone");
        assert_eq!(tones[0]["fqn"], "idea_theme::extensible::tone::Primary");
        assert_eq!(tones[0]["docs"], "Built-in semantic tone.");
    }

    #[tokio::test]
    async fn icon_set_tools_list_paginate_and_search() {
        let doc = serde_json::json!({
            "components": [],
            "icon_sets": [{
                "name": "icons-lucide",
                "title": "Lucide",
                "docs": "Outlined icon pack.",
                "import_path": "icons_lucide",
                "license": "ISC",
                "homepage": "https://lucide.dev",
                "icon_count": 3,
                "icons": [
                    { "name": "arrow-left", "ident": "ARROW_LEFT" },
                    { "name": "arrow-right", "ident": "ARROW_RIGHT" },
                    { "name": "search", "ident": "SEARCH" },
                ],
            }],
        });
        let cat = ResolvedCatalog::build_from_json(&doc.to_string()).unwrap();
        let svc = CatalogService::new();
        svc.replace_catalog(cat).await;

        // list_icon_sets surfaces the pack WITHOUT the per-icon names.
        let listed = svc
            .list_icon_sets(Parameters(FilterRequest::default()))
            .await
            .unwrap();
        let lv = parse_array(&listed);
        let set = &lv.as_array().unwrap()[0];
        assert_eq!(set["name"], "icons-lucide");
        assert_eq!(set["icon_count"], 3);
        assert!(set.get("icons").is_none(), "list must not dump icon names: {set}");

        // search_icons finds by keyword and returns a paste-ready import.
        let found = svc
            .search_icons(Parameters(IconSearchRequest {
                query: "arrow".into(),
                set: None,
                limit: None,
            }))
            .await
            .unwrap();
        let fv = parse_array(&found);
        let arr = fv.as_array().unwrap();
        assert_eq!(arr.len(), 2, "two arrow icons; got {fv}");
        assert_eq!(arr[0]["import"], "icons_lucide::ARROW_LEFT");

        // describe_icon_set pages: offset 1, limit 1 → just the middle icon.
        let page = svc
            .describe_icon_set(Parameters(DescribeIconSetRequest {
                name: "icons-lucide".into(),
                offset: Some(1),
                limit: Some(1),
            }))
            .await
            .unwrap();
        let pv = parse_array(&page);
        assert_eq!(pv["icon_count"], 3);
        let icons = pv["icons"].as_array().unwrap();
        assert_eq!(icons.len(), 1, "limit honored");
        assert_eq!(icons[0]["name"], "arrow-right");
        assert_eq!(icons[0]["import"], "icons_lucide::ARROW_RIGHT");

        // Unknown pack errors actionably.
        assert!(
            svc.describe_icon_set(Parameters(DescribeIconSetRequest {
                name: "nope".into(),
                offset: None,
                limit: None,
            }))
            .await
            .is_err(),
            "describe_icon_set on an unknown pack errors",
        );
    }

    fn qr_icon_catalog() -> ResolvedCatalog {
        let doc = serde_json::json!({
            "components": [],
            "icon_sets": [{
                "name": "icons-lucide",
                "title": "Lucide",
                "docs": "Outlined icon pack.",
                "import_path": "icons_lucide",
                "license": "ISC",
                "homepage": "https://lucide.dev",
                "icon_count": 4,
                "icons": [
                    { "name": "arrow-right", "ident": "ARROW_RIGHT" },
                    { "name": "qr-code", "ident": "QR_CODE" },
                    { "name": "scan-qr-code", "ident": "SCAN_QR_CODE" },
                    { "name": "square", "ident": "SQUARE" },
                ],
            }],
        });
        ResolvedCatalog::build_from_json(&doc.to_string()).unwrap()
    }

    async fn icon_search(svc: &CatalogService, query: &str, set: Option<&str>) -> Vec<String> {
        let res = svc
            .search_icons(Parameters(IconSearchRequest {
                query: query.into(),
                set: set.map(Into::into),
                limit: None,
            }))
            .await
            .unwrap();
        parse_array(&res)
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["import"].as_str().unwrap().to_string())
            .collect()
    }

    #[tokio::test]
    async fn regression_search_icons_qr_finds_qr_code() {
        // `search_icons("qr", set = "lucide")` returned `[]`: the `set`
        // filter compared against the crate name `icons-lucide`
        // byte-for-byte, so the natural spelling skipped every pack.
        let svc = CatalogService::new();
        svc.replace_catalog(qr_icon_catalog()).await;
        let both = vec![
            "icons_lucide::QR_CODE".to_string(),
            "icons_lucide::SCAN_QR_CODE".to_string(),
        ];
        for set in [None, Some("lucide"), Some("Lucide"), Some("icons-lucide"), Some("icons_lucide")] {
            assert_eq!(icon_search(&svc, "qr", set).await, both, "set = {set:?}");
        }
        // Either spelling of the name finds it: kebab, SCREAMING_SNAKE, spaced.
        for q in ["qr-code", "QR_CODE", "qr code", "  QR  "] {
            assert!(
                icon_search(&svc, q, Some("lucide")).await.contains(&both[0]),
                "query {q:?}"
            );
        }
    }

    #[tokio::test]
    async fn search_icons_unknown_set_is_an_error_not_an_empty_result() {
        let svc = CatalogService::new();
        svc.replace_catalog(qr_icon_catalog()).await;
        let err = svc
            .search_icons(Parameters(IconSearchRequest {
                query: "qr".into(),
                set: Some("feather".into()),
                limit: None,
            }))
            .await
            .expect_err("an unknown pack must not read as \"no such icon\"");
        assert!(err.message.contains("icons-lucide"), "names the real packs: {err:?}");
        // `set` is a filter, not a fuzzy search: a prefix of every pack
        // does not match them all.
        assert!(svc
            .search_icons(Parameters(IconSearchRequest {
                query: "qr".into(),
                set: Some("icons".into()),
                limit: None,
            }))
            .await
            .is_err());
        // describe_icon_set resolves the same spellings.
        let page = svc
            .describe_icon_set(Parameters(DescribeIconSetRequest {
                name: "lucide".into(),
                offset: None,
                limit: None,
            }))
            .await
            .unwrap();
        assert_eq!(parse_array(&page)["name"], "icons-lucide");
    }

    #[test]
    fn doc_summary_takes_first_nonempty_line_and_truncates() {
        assert_eq!(doc_summary(""), "");
        assert_eq!(doc_summary("\n\n  Hello there  \nsecond line"), "Hello there");
        let long = "x".repeat(200);
        let s = doc_summary(&long);
        assert_eq!(s.chars().count(), 120);
        assert!(s.ends_with('…'));
    }

    #[test]
    fn matches_filter_is_case_insensitive_substring_with_and_terms() {
        // Empty / absent matches everything.
        assert!(matches_filter(&None, &["Button"]));
        assert!(matches_filter(&Some("   ".into()), &["Button"]));
        // Case-insensitive substring against any field.
        assert!(matches_filter(&Some("btn".into()), &["IconBtn", "icon_btn"]));
        assert!(matches_filter(&Some("BUTTON".into()), &["button"]));
        // AND across whitespace-separated terms (each may hit a
        // different field).
        assert!(matches_filter(&Some("icon button".into()), &["IconButton", "layout"]));
        assert!(!matches_filter(&Some("icon missing".into()), &["IconButton"]));
        // No match.
        assert!(!matches_filter(&Some("slider".into()), &["Button", "button"]));
    }

    #[test]
    fn ensure_screenshot_saved_is_noop_when_relay_already_set_path() {
        // The dev relay decodes + saves the PNG and injects `path`; the MCP
        // must NOT re-save or touch the response in that case.
        let mut v = json!({
            "png_base64": "aGVsbG8=", // "hello"
            "width": 10,
            "height": 20,
            "path": "/already/saved.png",
        });
        ensure_screenshot_saved("myapp", &mut v);
        assert_eq!(v["path"], json!("/already/saved.png"));
    }

    #[test]
    fn ensure_screenshot_saved_writes_file_for_direct_bridge() {
        // No `path` → direct-bridge (`--local`) session. We must decode the
        // base64, write a PNG, and inject a `path` so the model never needs
        // the bytes.
        let tmp = std::env::temp_dir().join(format!(
            "idealyst-screenshot-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);

        let mut v = json!({
            "png_base64": "aGVsbG8=", // "hello"
            "width": 10,
            "height": 20,
        });
        // Target the temp dir directly — repointing HOME here raced
        // parallel tests that resolve `~/.idealyst` (dev_runner's
        // log-glob test flaked whenever it ran in this window).
        ensure_screenshot_saved_in(&tmp, "myapp", &mut v);

        let path = v["path"].as_str().expect("path was injected");
        assert!(path.contains("myapp-"));
        let bytes = std::fs::read(path).expect("file written");
        assert_eq!(bytes, b"hello");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn glob_filter_supports_prefix_suffix_and_infix_wildcards() {
        assert!(matches_filter(&Some("butt*".into()), &["button"]));
        assert!(matches_filter(&Some("*view".into()), &["scroll_view"]));
        assert!(matches_filter(&Some("*field*".into()), &["text_field_inner"]));
        assert!(!matches_filter(&Some("btn*".into()), &["button"]));
        // Raw glob_match anchoring.
        assert!(glob_match("a*c", "abc"));
        assert!(glob_match("a*c", "ac"));
        assert!(!glob_match("a*c", "abd"));
        assert!(glob_match("*", "anything"));
    }

    /// Regression for the prop-doc wire gap: a catalog arriving over the
    /// managed-wrapper / bridge flow carries prop docs only in its
    /// serialized `types` slice (the server binary has no in-process
    /// `PropsSchemaEntry` for the project's structs). `describe_component`
    /// must inline them from `cat.types()`. The pre-fix join via the
    /// global `lookup_schema` returned nothing here.
    #[tokio::test]
    async fn describe_component_surfaces_prop_docs_over_the_wire() {
        let json = r#"{
          "catalog_version": 2,
          "components": [
            {
              "name": "gauge",
              "module_path": "demo::gauge",
              "file": "src/gauge.rs",
              "line": 10,
              "docs": "A radial gauge.",
              "composes": [],
              "params": [
                { "name": "props", "type": "& GaugeProps", "type_short_name": "GaugeProps" }
              ]
            }
          ],
          "types": [
            {
              "short_name": "GaugeProps",
              "module_path": "demo::gauge",
              "docs": "Props for the gauge.",
              "shape": {
                "kind": "struct",
                "fields": [
                  { "name": "value", "type": "f64", "doc": "The gauge value, 0.0-1.0.", "constraint": "0..=1" }
                ]
              }
            }
          ]
        }"#;
        let cat = mcp_catalog::ResolvedCatalog::build_from_json(json)
            .expect("build catalog from wire JSON");
        let svc = CatalogService::new();
        svc.replace_catalog(cat).await;

        let result = svc
            .describe_component(Parameters(NameRequest {
                name: "gauge".into(),
                app: None,
            }))
            .await
            .expect("describe_component succeeds");
        let payload = result
            .content
            .iter()
            .find_map(|c| c.as_text().map(|t| t.text.clone()))
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
        let schema = &v["params"][0]["schema"];
        assert!(schema.is_array(), "param must inline its props schema; got {v}");
        assert_eq!(schema[0]["name"], "value");
        assert_eq!(schema[0]["doc"], "The gauge value, 0.0-1.0.");
        assert_eq!(schema[0]["constraint"], "0..=1");
    }

    /// Regression: two props structs with one name in different modules
    /// (CrewForge has `BlockerRowProps` in two crates) — the param joined
    /// whichever `TypeEntry` came first, documenting the component with
    /// the other struct's fields. It joins the one nearest the component.
    #[tokio::test]
    async fn describe_component_joins_the_props_struct_nearest_the_component() {
        let json = r#"{
          "catalog_version": 2,
          "components": [
            { "name": "Row", "module_path": "accounting::lifecycle", "file": "src/lifecycle.rs", "line": 1,
              "docs": "", "composes": [],
              "params": [ { "name": "props", "type": "& RowProps", "type_short_name": "RowProps" } ] }
          ],
          "types": [
            { "short_name": "RowProps", "module_path": "ui_shared::wizard", "docs": "",
              "shape": { "kind": "struct", "fields": [ { "name": "blocker", "type": "Blocker", "doc": "", "constraint": "" } ] } },
            { "short_name": "RowProps", "module_path": "accounting::lifecycle", "docs": "",
              "shape": { "kind": "struct", "fields": [ { "name": "chip", "type": "String", "doc": "", "constraint": "" } ] } }
          ]
        }"#;
        let cat = mcp_catalog::ResolvedCatalog::build_from_json(json).expect("build catalog from wire JSON");
        let svc = CatalogService::new();
        svc.replace_catalog(cat).await;
        let result = svc
            .describe_component(Parameters(NameRequest { name: "Row".into(), app: None }))
            .await
            .expect("describe_component succeeds");
        let payload = result.content.iter().find_map(|c| c.as_text().map(|t| t.text.clone())).unwrap();
        let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(v["params"][0]["schema"][0]["name"], "chip", "{v}");
    }

    /// When a component's `*Props` param has NO catalogued type (the
    /// struct lacks `#[derive(IdealystSchema)]` — e.g. idea-ui's
    /// `SelectProps`), `describe_component` must flag it as
    /// `props_documented: false` with a note (and NO `schema`), instead
    /// of silently omitting it and leaving an LLM to dead-end on
    /// `describe_type`. Regression for the quill-emr
    /// `type "SelectProps" not found` report.
    #[tokio::test]
    async fn describe_component_flags_undocumented_props() {
        let json = r#"{
          "catalog_version": 2,
          "components": [
            {
              "name": "Select",
              "module_path": "idea_ui::components::select",
              "file": "select.rs",
              "line": 1,
              "docs": "A dropdown select.",
              "composes": [],
              "params": [
                { "name": "props", "type": "& SelectProps", "type_short_name": "SelectProps" }
              ]
            }
          ],
          "types": []
        }"#;
        let cat = mcp_catalog::ResolvedCatalog::build_from_json(json)
            .expect("build catalog from wire JSON");
        let svc = CatalogService::new();
        svc.replace_catalog(cat).await;

        let result = svc
            .describe_component(Parameters(NameRequest {
                name: "Select".into(),
                app: None,
            }))
            .await
            .expect("describe_component succeeds");
        let payload = result
            .content
            .iter()
            .find_map(|c| c.as_text().map(|t| t.text.clone()))
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
        let param = &v["params"][0];
        assert!(param["schema"].is_null(), "no schema when props undocumented; got {v}");
        assert_eq!(param["props_documented"], serde_json::json!(false));
        assert!(
            param["note"].as_str().unwrap_or("").contains("IdealystSchema"),
            "note should point at the missing derive; got {param}"
        );
    }

    /// Recipes flow over the wire and surface in all three places:
    /// `list_recipes`, `describe_recipe`, and — the key one —
    /// `describe_component`'s `recipes` section (matched by primary
    /// component AND by `uses`).
    #[tokio::test]
    async fn recipes_surface_in_list_describe_and_component() {
        let json = r#"{
          "catalog_version": 2,
          "components": [
            { "name": "Select", "module_path": "idea_ui::components::select",
              "file": "select.rs", "line": 1, "docs": "A dropdown.",
              "composes": [], "params": [] }
          ],
          "recipes": [
            { "name": "select_basic", "target": "Select",
              "module_path": "demo::recipes", "file": "recipes.rs", "line": 10,
              "docs": "Basic Select usage.",
              "source": "fn select_basic() -> Element { ui!{ Select(value = v) } }",
              "uses": ["Select"] }
          ]
        }"#;
        let cat = mcp_catalog::ResolvedCatalog::build_from_json(json).unwrap();
        let svc = CatalogService::new();
        svc.replace_catalog(cat).await;

        let text = |r: &CallToolResult| {
            r.content
                .iter()
                .find_map(|c| c.as_text().map(|t| t.text.clone()))
                .unwrap()
        };

        // list_recipes
        let lr = svc
            .list_recipes(Parameters(FilterRequest::default()))
            .await
            .unwrap();
        let lv: serde_json::Value = serde_json::from_str(&text(&lr)).unwrap();
        assert_eq!(lv[0]["name"], "select_basic");
        assert_eq!(lv[0]["target"], "Select");

        // describe_recipe → full source
        let dr = svc
            .describe_recipe(Parameters(NameRequest {
                name: "select_basic".into(),
                app: None,
            }))
            .await
            .unwrap();
        let dv: serde_json::Value = serde_json::from_str(&text(&dr)).unwrap();
        assert!(dv["source"].as_str().unwrap().contains("Select(value"));
        assert_eq!(dv["uses"][0], "Select");

        // describe_component links the recipe inline
        let dc = svc
            .describe_component(Parameters(NameRequest {
                name: "Select".into(),
                app: None,
            }))
            .await
            .unwrap();
        let cv: serde_json::Value = serde_json::from_str(&text(&dc)).unwrap();
        let recipes = cv["recipes"].as_array().expect("recipes section");
        assert_eq!(recipes.len(), 1, "got {cv}");
        assert_eq!(recipes[0]["name"], "select_basic");
        assert_eq!(recipes[0]["primary"], serde_json::json!(true));
        assert!(recipes[0]["source"].as_str().unwrap().contains("Select"));
    }

    /// Scopes surface in `list_scopes`, `describe_scope` (with the
    /// components assigned by module proximity), and as the `scope` field
    /// of `describe_component`.
    #[tokio::test]
    async fn scopes_surface_in_list_describe_and_component() {
        let json = r#"{
          "catalog_version": 2,
          "components": [
            { "name": "LoginForm", "module_path": "app::auth::forms",
              "file": "forms.rs", "line": 1, "docs": "A login form.",
              "composes": [], "params": [] }
          ],
          "scopes": [
            { "slug": "auth", "title": "Authentication", "docs": "Login + sessions.",
              "module_path": "app::auth", "order": 5 }
          ]
        }"#;
        let cat = mcp_catalog::ResolvedCatalog::build_from_json(json).unwrap();
        let svc = CatalogService::new();
        svc.replace_catalog(cat).await;

        let text = |r: &CallToolResult| {
            r.content
                .iter()
                .find_map(|c| c.as_text().map(|t| t.text.clone()))
                .unwrap()
        };

        // list_scopes
        let ls = svc
            .list_scopes(Parameters(FilterRequest::default()))
            .await
            .unwrap();
        let lv: serde_json::Value = serde_json::from_str(&text(&ls)).unwrap();
        assert_eq!(lv[0]["slug"], "auth");
        assert_eq!(lv[0]["title"], "Authentication");

        // describe_scope → the component lands here by module proximity
        // (`app::auth` is an ancestor of `app::auth::forms`).
        let ds = svc
            .describe_scope(Parameters(NameRequest {
                name: "auth".into(),
                app: None,
            }))
            .await
            .unwrap();
        let dv: serde_json::Value = serde_json::from_str(&text(&ds)).unwrap();
        assert_eq!(dv["slug"], "auth");
        let comps = dv["components"].as_array().expect("components section");
        assert!(
            comps.iter().any(|c| c["name"] == "LoginForm"),
            "scope should contain LoginForm; got {dv}"
        );

        // describe_component reports its ambient scope.
        let dc = svc
            .describe_component(Parameters(NameRequest {
                name: "LoginForm".into(),
                app: None,
            }))
            .await
            .unwrap();
        let cv: serde_json::Value = serde_json::from_str(&text(&dc)).unwrap();
        assert_eq!(cv["scope"], "auth", "got {cv}");
    }

    /// `lint_project` runs the source linter and surfaces idiom-drift
    /// findings as structured JSON: a non-PascalCase `#[component]`
    /// (error) and a raw `Signal::new` (warn) in one file must both
    /// appear, and the error must flip the run's `failed` flag.
    #[tokio::test]
    async fn lint_project_reports_idiom_drift() {
        use std::io::Write;

        // A unique temp dir so concurrent test runs don't collide.
        let dir = std::env::temp_dir()
            .join(format!("idealyst-lint-tool-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("sample.rs");
        {
            let mut f = std::fs::File::create(&file).unwrap();
            // `icon_button` → component-pascal-case (error);
            // `Signal::new(0)` → prefer-signal-macro (warn).
            write!(
                f,
                "#[component]\nfn icon_button() -> Element {{\n    let s = Signal::new(0);\n    todo!()\n}}\n"
            )
            .unwrap();
        }

        let svc = CatalogService::new();
        let result = svc
            .lint_project(Parameters(LintRequest {
                path: Some(file.display().to_string()),
                ..Default::default()
            }))
            .await
            .expect("lint_project tool call succeeds");

        let text = |r: &CallToolResult| {
            r.content
                .iter()
                .find_map(|c| c.as_text().map(|t| t.text.clone()))
                .unwrap()
        };

        let v: serde_json::Value = serde_json::from_str(&text(&result)).unwrap();
        let rules: Vec<String> = v["diagnostics"]
            .as_array()
            .expect("diagnostics array")
            .iter()
            .filter_map(|d| d["rule"].as_str().map(str::to_string))
            .collect();

        assert!(
            rules.contains(&"component-pascal-case".to_string()),
            "expected component-pascal-case; got {rules:?}"
        );
        assert!(
            rules.contains(&"prefer-signal-fn".to_string()),
            "expected prefer-signal-fn; got {rules:?}"
        );
        assert_eq!(v["files_scanned"], 1);
        assert!(v["error_count"].as_u64().unwrap() >= 1, "got {v}");
        assert_eq!(v["failed"], true, "an error-level diagnostic must fail the run");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `configure_devcontainer` drives the shared `configure` engine: enabling
    /// a service through the MCP tool must scaffold a base devcontainer and the
    /// idealyst-managed compose file, and report the addition.
    #[tokio::test]
    async fn configure_devcontainer_enables_service() {
        let dir = std::env::temp_dir()
            .join(format!("idealyst-configure-tool-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let svc = CatalogService::new();
        let result = svc
            .configure_devcontainer(Parameters(ConfigureDevcontainerRequest {
                dir: Some(dir.display().to_string()),
                services: vec![ConfigureServiceArg {
                    id: "database".into(),
                    variant: Some("postgres".into()),
                    action: Some("enable".into()),
                }],
                config: None,
            }))
            .await
            .expect("configure_devcontainer tool call succeeds");

        let text = |r: &CallToolResult| {
            r.content
                .iter()
                .find_map(|c| c.as_text().map(|t| t.text.clone()))
                .unwrap()
        };
        let v: serde_json::Value = serde_json::from_str(&text(&result)).unwrap();

        assert!(
            v["added"].as_array().unwrap().iter().any(|s| s == "database"),
            "expected database added; got {v}"
        );
        assert!(dir.join(".devcontainer/devcontainer.json").exists());
        let managed =
            std::fs::read_to_string(dir.join(".devcontainer/docker-compose.idealyst.yml")).unwrap();
        assert!(managed.contains("postgres:16"), "managed compose has postgres");

        // Unknown action is a params error, not a panic.
        let bad = svc
            .configure_devcontainer(Parameters(ConfigureDevcontainerRequest {
                dir: Some(dir.display().to_string()),
                services: vec![ConfigureServiceArg {
                    id: "redis".into(),
                    variant: None,
                    action: Some("frobnicate".into()),
                }],
                config: None,
            }))
            .await;
        assert!(bad.is_err(), "unknown action should be rejected");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `configure_vscode` with no aspects enables everything: it must write the
    /// rust-analyzer overrideCommand + the extension recommendations and report
    /// both aspects added.
    #[tokio::test]
    async fn configure_vscode_enables_all_aspects() {
        let dir = std::env::temp_dir()
            .join(format!("idealyst-configure-vscode-tool-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let svc = CatalogService::new();
        let result = svc
            .configure_vscode(Parameters(ConfigureVscodeRequest {
                dir: Some(dir.display().to_string()),
                aspects: None,
                remove_all: None,
            }))
            .await
            .expect("configure_vscode tool call succeeds");

        let text = |r: &CallToolResult| {
            r.content
                .iter()
                .find_map(|c| c.as_text().map(|t| t.text.clone()))
                .unwrap()
        };
        let v: serde_json::Value = serde_json::from_str(&text(&result)).unwrap();
        let added = v["added"].as_array().unwrap();
        assert!(added.iter().any(|s| s == "lint"), "got {v}");
        assert!(added.iter().any(|s| s == "extensions"), "got {v}");

        let settings: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join(".vscode/settings.json")).unwrap(),
        )
        .unwrap();
        assert!(settings.get("rust-analyzer.check.overrideCommand").is_some());
        assert!(dir.join(".vscode/ra-check.sh").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Regression for the styling dead-end: `describe_type("StyleSource")`
    /// (or any core style-machinery type seen in a signature) returned a
    /// bare "not found", sending agents source-diving. The error must now
    /// redirect to the styling guide + the `stylesheet` macro entry.
    #[tokio::test]
    async fn regression_describe_type_redirects_core_style_types() {
        let svc = CatalogService::new();
        for name in ["StyleSource", "StyleApplication", "StyleSheet", "StyleRules", "IntoStyleSource"] {
            let err = svc
                .describe_type(Parameters(NameRequest { name: name.into(), app: None }))
                .await
                .expect_err("core style type must not resolve as a schema type");
            let msg = err.message.to_string();
            assert!(msg.contains("core style type"), "{name}: {msg}");
            assert!(msg.contains("read_guide(\"styling\")"), "{name}: {msg}");
            assert!(msg.contains("stylesheet"), "{name}: {msg}");
        }
        // Unrelated unknown types keep the generic list_types hint.
        let err = svc
            .describe_type(Parameters(NameRequest { name: "NoSuchType".into(), app: None }))
            .await
            .expect_err("unknown type still errors");
        assert!(err.message.contains("list_types"), "{}", err.message);
    }

    /// Regression for the arena debug-fix miss: the agent read the
    /// reactivity guide, named the frozen-snapshot pitfall, then wrote
    /// `for item in items.get()` — because the `keyed_list_add_remove`
    /// recipe was never surfaced. `read_guide` must now append a
    /// "Related recipes:" footer joining recipes to the guide (curated pin
    /// for the reactivity guides), and fall back to a static pointer at
    /// list_recipes/describe_recipe on the core guides when nothing joins.
    #[tokio::test]
    async fn regression_read_guide_appends_related_recipes_footer() {
        let json = r#"{
          "catalog_version": 2,
          "components": [],
          "guides": [
            { "slug": "reactivity", "title": "Reactivity", "order": 30,
              "tags": ["core", "reactivity"], "body": "signals prose" },
            { "slug": "styling", "title": "Styling", "order": 40,
              "tags": ["style", "theme"], "body": "styling prose" },
            { "slug": "media", "title": "Media", "order": 90,
              "tags": ["media"], "body": "media prose" }
          ],
          "recipes": [
            { "name": "keyed_list_add_remove", "target": "ui",
              "module_path": "runtime_core::recipes", "file": "recipes.rs", "line": 1,
              "docs": "Reactive keyed list.", "source": "fn keyed_list_add_remove() {}",
              "uses": [] },
            { "name": "stylesheet_variants", "target": "styling",
              "module_path": "demo::recipes", "file": "recipes.rs", "line": 2,
              "docs": "Stylesheet variants.", "source": "fn stylesheet_variants() {}",
              "uses": [] }
          ]
        }"#;
        let cat = mcp_catalog::ResolvedCatalog::build_from_json(json).unwrap();
        let svc = CatalogService::new();
        svc.replace_catalog(cat).await;

        let body_of = |r: &CallToolResult| {
            let payload = r
                .content
                .iter()
                .find_map(|c| c.as_text().map(|t| t.text.clone()))
                .unwrap();
            let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
            v["body"].as_str().unwrap().to_string()
        };

        // Reactivity: the curated pin surfaces keyed_list_add_remove even
        // though its target (`ui`) shares no name with the guide's tags.
        let r = svc
            .read_guide(Parameters(SlugRequest { slug: "reactivity".into() }))
            .await
            .unwrap();
        let body = body_of(&r);
        assert!(body.contains("Related recipes:"), "{body}");
        assert!(body.contains("keyed_list_add_remove"), "{body}");
        assert!(body.contains("describe_recipe"), "{body}");
        assert!(body.starts_with("signals prose"), "footer must append, not replace: {body}");

        // Styling: the fuzzy join (recipe target == guide slug) finds the
        // stylesheet recipe.
        let r = svc
            .read_guide(Parameters(SlugRequest { slug: "styling".into() }))
            .await
            .unwrap();
        let body = body_of(&r);
        assert!(body.contains("stylesheet_variants"), "{body}");

        // Non-core guide with no joining recipes: no footer at all.
        let r = svc
            .read_guide(Parameters(SlugRequest { slug: "media".into() }))
            .await
            .unwrap();
        let body = body_of(&r);
        assert!(!body.contains("Related recipes:"), "{body}");

        // Core guide + a catalog with NO recipes: static footer still names
        // the recipe tools so the pathway stays discoverable.
        let bare = r#"{
          "catalog_version": 2,
          "components": [],
          "guides": [
            { "slug": "navigation", "title": "Navigation", "order": 50,
              "tags": ["navigation"], "body": "nav prose" }
          ]
        }"#;
        svc.replace_catalog(mcp_catalog::ResolvedCatalog::build_from_json(bare).unwrap())
            .await;
        let r = svc
            .read_guide(Parameters(SlugRequest { slug: "navigation".into() }))
            .await
            .unwrap();
        let body = body_of(&r);
        assert!(body.contains("Related recipes:"), "{body}");
        assert!(body.contains("list_recipes"), "{body}");
        assert!(body.contains("describe_recipe"), "{body}");
    }

    // ---- provenance across a multi-project catalog ----

    /// Two apps plus a shared library, the shape a workspace-wide
    /// catalog actually has. `beta_app::AppRoot` and `alpha_app::AppRoot`
    /// deliberately collide: that is the case a merged catalog makes
    /// ordinary and a single-project one never produced.
    fn multi_project_catalog() -> serde_json::Value {
        let comp = |module: &str, name: &str| {
            serde_json::json!({
                "name": name,
                "module_path": module,
                "file": format!("/w/{module}/src/lib.rs"),
                "line": 1,
                "docs": format!("{name} from {module}."),
                "composes": [],
                "params": [],
            })
        };
        serde_json::json!({
            "components": [
                comp("alpha_app::screens", "AppRoot"),
                comp("alpha_app::screens", "CrewSheet"),
                comp("beta_app::kiosk", "AppRoot"),
                comp("beta_app::kiosk", "PinPad"),
                comp("shared_ui::widgets", "DateJumper"),
            ]
        })
    }

    /// Robot DISABLED on purpose: `CatalogService::new()` starts the
    /// discovery thread, which reads the real `~/.idealyst/apps/`. A
    /// developer with an app running would then change these tests'
    /// results — which is exactly how this helper was caught, when a
    /// background build registered a bridge mid-suite.
    async fn multi_project_service() -> CatalogService {
        let cat = ResolvedCatalog::build_from_json(&multi_project_catalog().to_string()).unwrap();
        let svc = CatalogService::with_robot_mode(false, None);
        svc.replace_catalog(cat).await;
        svc
    }

    #[tokio::test]
    async fn list_components_reports_the_originating_crate() {
        // Regression: this field was hardcoded `null` on the static
        // catalog path, so a merged catalog gave no way to tell which
        // app a component belonged to.
        let svc = multi_project_service().await;
        let out = svc
            .list_components(Parameters(FilterRequest::default()))
            .await
            .unwrap();
        let rows = parse_array(&out);
        let rows = rows.as_array().unwrap();
        assert_eq!(rows.len(), 5, "{rows:#?}");
        for row in rows {
            let app = row["app"].as_str().unwrap_or("");
            assert!(
                ["alpha_app", "beta_app", "shared_ui"].contains(&app),
                "every row must name its crate, got {row:#?}"
            );
        }
    }

    #[tokio::test]
    async fn list_components_app_filter_narrows_to_one_project() {
        let svc = multi_project_service().await;
        let out = svc
            .list_components(Parameters(FilterRequest {
                filter: None,
                app: Some("beta_app".into()),
            }))
            .await
            .unwrap();
        let mut names = component_names(&out);
        names.sort();
        assert_eq!(names, vec!["AppRoot".to_string(), "PinPad".to_string()]);
    }

    #[tokio::test]
    async fn app_filter_accepts_the_hyphenated_package_spelling() {
        // Users read `beta-app` in Cargo.toml but `module_path` reports
        // the underscored lib name. Requiring the underscore form would
        // make the filter fail in exactly the way that looks like "no
        // such app".
        let svc = multi_project_service().await;
        let out = svc
            .list_components(Parameters(FilterRequest {
                filter: None,
                app: Some("beta-app".into()),
            }))
            .await
            .unwrap();
        assert_eq!(component_names(&out).len(), 2);
    }

    #[tokio::test]
    async fn describe_component_reports_ambiguity_instead_of_guessing() {
        // The important one. Two apps each define `AppRoot`; picking the
        // first match would answer confidently with the wrong component's
        // props. The error must name the candidates AND how to choose.
        let svc = multi_project_service().await;
        let err = svc
            .describe_component(Parameters(NameRequest {
                name: "AppRoot".into(),
                app: None,
            }))
            .await
            .expect_err("ambiguous short name must not resolve");
        let msg = format!("{err:?}");
        assert!(msg.contains("ambiguous"), "{msg}");
        assert!(msg.contains("alpha_app::screens::AppRoot"), "{msg}");
        assert!(msg.contains("beta_app::kiosk::AppRoot"), "{msg}");
    }

    #[tokio::test]
    async fn describe_component_disambiguates_by_app_or_fqn() {
        let svc = multi_project_service().await;

        // By app.
        let out = svc
            .describe_component(Parameters(NameRequest {
                name: "AppRoot".into(),
                app: Some("beta_app".into()),
            }))
            .await
            .unwrap();
        assert!(format!("{out:?}").contains("beta_app::kiosk"), "{out:?}");

        // By fully-qualified name — unambiguous by construction, so it
        // resolves with no `app` at all.
        let out = svc
            .describe_component(Parameters(NameRequest {
                name: "alpha_app::screens::AppRoot".into(),
                app: None,
            }))
            .await
            .unwrap();
        assert!(format!("{out:?}").contains("alpha_app::screens"), "{out:?}");
    }

    #[tokio::test]
    async fn unique_short_names_still_resolve_bare() {
        // Ambiguity handling must not tax the common case: a name only
        // one component has resolves with no ceremony.
        let svc = multi_project_service().await;
        let out = svc
            .describe_component(Parameters(NameRequest {
                name: "DateJumper".into(),
                app: None,
            }))
            .await
            .unwrap();
        assert!(format!("{out:?}").contains("shared_ui::widgets"), "{out:?}");
    }

    #[test]
    fn app_matches_is_spelling_insensitive_and_open_when_unset() {
        assert!(app_matches(None, "anything"));
        assert!(app_matches(Some("beta_app"), "beta_app"));
        assert!(app_matches(Some("beta-app"), "beta_app"));
        assert!(!app_matches(Some("beta_app"), "alpha_app"));
    }

    // ---- catalog freshness ----

    use crate::catalog_status::CatalogState;

    #[tokio::test]
    async fn a_current_catalog_carries_no_marker() {
        // The happy path must stay silent: a marker on every response is
        // noise, and noise is skimmed past exactly when it matters.
        let svc = multi_project_service().await;
        assert!(svc.status().await.marker().is_none());
        let out = svc
            .list_components(Parameters(FilterRequest::default()))
            .await
            .unwrap();
        assert!(!format!("{out:?}").contains("[catalog:"), "{out:?}");
    }

    #[tokio::test]
    async fn an_in_flight_rebuild_is_announced_on_the_response() {
        let svc = multi_project_service().await;
        svc.mark_build_started().await;

        let st = svc.status().await;
        assert!(!st.is_settled_current());
        let marker = st.marker().expect("in-flight rebuild must be announced");
        assert!(marker.contains("REBUILDING"), "{marker}");

        // And it rides on the tool output itself, not only on the
        // status tool nobody thinks to call.
        let out = svc
            .list_components(Parameters(FilterRequest::default()))
            .await
            .unwrap();
        assert!(format!("{out:?}").contains("REBUILDING"), "{out:?}");
    }

    #[tokio::test]
    async fn a_failed_rebuild_keeps_the_catalog_and_marks_it_stale() {
        // The invariant this whole feature protects: a failed rebuild
        // must NOT tear down what is being served. What changes is that
        // the staleness is now visible instead of silent.
        let svc = multi_project_service().await;
        let before = component_names(
            &svc.list_components(Parameters(FilterRequest::default()))
                .await
                .unwrap(),
        );

        svc.mark_build_started().await;
        svc.mark_build_failed("error[E0432]: unresolved import `crate::widgets::Missing`")
            .await;

        let st = svc.status().await;
        assert_eq!(st.state, CatalogState::Stale);
        let marker = st.marker().unwrap();
        assert!(marker.contains("STALE"), "{marker}");
        assert!(marker.contains("E0432"), "marker must carry the reason: {marker}");

        // Still serving every component it had — and the payload is
        // still valid JSON, because the marker is its own content block
        // rather than a prefix smuggled into the body.
        let out = svc
            .list_components(Parameters(FilterRequest::default()))
            .await
            .unwrap();
        assert_eq!(out.content.len(), 2, "marker and payload are separate blocks");
        let after = component_names(&out);
        assert_eq!(before, after, "a failed rebuild must not drop the catalog");
    }

    #[tokio::test]
    async fn a_first_build_that_never_succeeded_is_unavailable_not_stale() {
        // "Stale" implies there is good older data. When the very first
        // build failed there is none, and an empty result would otherwise
        // read as "this project has no components".
        let svc = CatalogService::with_robot_mode(false, None);
        svc.mark_build_started().await;
        svc.mark_build_failed("the project does not compile").await;

        let st = svc.status().await;
        assert_eq!(st.state, CatalogState::Unavailable);
        let marker = st.marker().unwrap();
        assert!(marker.contains("UNAVAILABLE"), "{marker}");
        assert!(
            marker.contains("NOT because the project has no components"),
            "the marker must rule out the wrong reading: {marker}"
        );
    }

    #[tokio::test]
    async fn a_successful_build_clears_a_previous_failure() {
        let svc = multi_project_service().await;
        svc.mark_build_failed("transient breakage").await;
        assert_eq!(svc.status().await.state, CatalogState::Stale);

        svc.mark_build_succeeded(Some(std::time::Duration::from_secs(71)))
            .await;
        let st = svc.status().await;
        assert_eq!(st.state, CatalogState::Current);
        assert!(st.last_error.is_none(), "a good build supersedes the failure");
        assert!(st.marker().is_none());
        // The build cost is what a caller needs to judge whether waiting
        // for the next rebuild is worth it.
        assert_eq!(st.build_duration.map(|d| d.as_secs()), Some(71));
    }

    #[tokio::test]
    async fn generation_advances_only_on_a_successful_build() {
        let svc = multi_project_service().await;
        let g0 = svc.status().await.generation;
        svc.mark_build_failed("nope").await;
        assert_eq!(svc.status().await.generation, g0, "a failure is not a new catalog");
        svc.mark_build_succeeded(None).await;
        assert_eq!(svc.status().await.generation, g0 + 1);
    }

    #[tokio::test]
    async fn wait_for_current_returns_as_soon_as_a_rebuild_lands() {
        let svc = Arc::new(multi_project_service().await);
        svc.mark_build_started().await;

        let waiter = {
            let svc = svc.clone();
            tokio::spawn(async move {
                svc.wait_for_current(std::time::Duration::from_secs(30)).await
            })
        };
        // Let the waiter park before the build completes.
        tokio::task::yield_now().await;
        svc.mark_build_succeeded(Some(std::time::Duration::from_secs(2)))
            .await;

        let st = waiter.await.unwrap();
        assert!(st.is_settled_current(), "{st:?}");
    }

    #[tokio::test]
    async fn wait_for_current_times_out_into_a_status_not_an_error() {
        // A timeout is an answer — "still building after N seconds" —
        // and must not surface as a failed tool call.
        let svc = multi_project_service().await;
        svc.mark_build_started().await;
        let st = svc
            .wait_for_current(std::time::Duration::from_millis(50))
            .await;
        assert!(!st.is_settled_current());
        assert!(st.marker().unwrap().contains("REBUILDING"));
    }

    #[test]
    fn trim_error_keeps_the_diagnosis_and_drops_the_noise() {
        // Cargo failures are mostly warnings; the useful line is at the
        // top and the response should not carry hundreds of lines.
        let raw = "warning: unused import: `Foo`\n\
                   Compiling my-app v0.1.0\n\
                   error[E0432]: unresolved import `crate::Missing`\n\
                   \n\
                   help: a similar path exists\n";
        let out = trim_error(raw);
        assert!(out.contains("E0432"), "{out}");
        assert!(!out.contains("warning:"), "{out}");
        assert!(!out.contains("Compiling"), "{out}");
    }

    #[test]
    fn regression_trim_error_drops_whole_warning_blocks() {
        // Dropping only the `warning:` header let the block's continuation
        // lines through — the STALE marker then quoted a dead-code warning
        // (`fn sniff_content_type`) instead of the error further down.
        let raw = "warning: function `sniff_content_type` is never used\n  \
                   --> crates/ui-shared/src/voice.rs:62:4\n   |\n\
                   62 | fn sniff_content_type(bytes: &[u8]) -> &'static str {\n   \
                   |    ^^^^^^^^^^^^^^^^^^\n   |\n   \
                   = note: `#[warn(dead_code)]` on by default\n\n\
                   error: could not compile `ui-shared` (lib) due to 1 previous error\n\
                   Error: build the catalog wrapper (see stderr above)\n\n\
                   Caused by:\n    cargo build exited with exit status: 101\n";
        let out = trim_error(raw);
        assert!(!out.contains("sniff_content_type"), "{out}");
        assert!(!out.contains("voice.rs"), "{out}");
        assert!(out.starts_with("error: could not compile"), "{out}");
        assert!(out.contains("exit status: 101"), "the cause chain is kept: {out}");
    }

    #[test]
    fn regression_trim_error_sees_through_ansi_coloured_warnings() {
        // With CARGO_TERM_COLOR=always the header is `\x1b[33mwarning\x1b[0m:`
        // and a bare prefix check missed it.
        let raw = "\u{1b}[1m\u{1b}[33mwarning\u{1b}[0m\u{1b}[1m: function `f` is never used\u{1b}[0m\n\
                   \u{1b}[1m\u{1b}[31merror[E0425]\u{1b}[0m: cannot find value `x`\n";
        let out = trim_error(raw);
        assert_eq!(out, "error[E0425]: cannot find value `x`");
        assert!(has_error_diagnostic(raw));
        assert!(!has_error_diagnostic("\u{1b}[33mwarning\u{1b}[0m: unused"));
    }

    #[test]
    fn extractor_failure_message_leads_with_status_only_when_no_error() {
        let warn = "warning: unused import: `Foo`\n --> src/lib.rs:1:5\n";
        let msg = extractor_failure_message("signal: 9 (SIGKILL)", warn);
        assert!(trim_error(&msg).starts_with("catalog extractor exited with signal: 9"), "{msg}");
        // A real error is passed through untouched — it is the reason.
        let err = "error[E0432]: unresolved import\n";
        assert_eq!(extractor_failure_message("exit status: 101", err), err);
        assert!(extractor_failure_message("exit status: 1", "").contains("exit status: 1"));
    }

    #[test]
    fn trim_error_falls_back_when_everything_looked_like_noise() {
        // Never return an empty reason — a blank explanation is worse
        // than a noisy one.
        assert!(!trim_error("warning: only warnings here").is_empty());
    }

    #[tokio::test]
    async fn initializing_is_distinct_from_an_empty_catalog_that_built_fine() {
        // The whole point: before this, a first build in flight reported
        // state "current" over an empty catalog, which reads as "this
        // project has no components".
        let svc = CatalogService::with_robot_mode(false, None);
        svc.mark_initializing().await;

        let st = svc.status().await;
        assert_eq!(st.state, CatalogState::Initializing);
        assert!(!st.is_settled_current());
        let marker = st.marker().unwrap();
        assert!(marker.contains("INITIALIZING"), "{marker}");
        assert!(
            marker.contains("not built yet") && marker.contains("NOT"),
            "the marker must rule out the wrong reading: {marker}"
        );
    }

    #[tokio::test]
    async fn initializing_rides_on_catalog_tool_responses() {
        // A client that connects mid-build must learn this WITHOUT
        // having thought to call catalog_status.
        let svc = CatalogService::with_robot_mode(false, None);
        svc.mark_initializing().await;
        let out = svc
            .list_components(Parameters(FilterRequest::default()))
            .await
            .unwrap();
        assert!(format!("{out:?}").contains("INITIALIZING"), "{out:?}");
    }

    #[tokio::test]
    async fn the_first_successful_build_clears_initializing() {
        let svc = CatalogService::with_robot_mode(false, None);
        svc.mark_initializing().await;
        let cat =
            ResolvedCatalog::build_from_json(&multi_project_catalog().to_string()).unwrap();
        svc.replace_catalog(cat).await;

        let st = svc.status().await;
        assert_eq!(st.state, CatalogState::Current);
        assert!(st.is_settled_current());
        assert!(st.marker().is_none(), "a landed catalog is silent again");
    }

    /// The default mode is Discovery, where robot tools work by routing
    /// per call through `~/.idealyst/apps/` — but the pinned-bridge
    /// field they used to be reported from is `None` there, so every
    /// discovery-mode session was told its robot tools were disabled.
    #[test]
    fn regression_discovery_mode_does_not_report_robot_disabled() {
        let svc = CatalogService::with_robot_mode(true, None);
        let instructions = svc.get_info().instructions.unwrap();
        assert!(
            instructions.contains("Robot tools drive running apps found via `list_apps`"),
            "discovery mode is enabled: {instructions}",
        );
        assert!(!instructions.contains("disabled"), "{instructions}");
    }

    /// The robot docs topic is how an agent learns the robot surface beyond
    /// one-line descriptions, so the tools that are easy to miss must have
    /// sections there and the guide it points at must ship. (The robot
    /// paragraph of the instructions once named only the element verbs, so
    /// functions, native taps and SDK test controls went unseen.) Every tool
    /// the instructions' robot sentence names must be one the server serves.
    #[test]
    fn robot_docs_name_real_tools_and_the_testing_guide() {
        let tools: std::collections::HashSet<String> =
            CatalogService::tool_router().list_all().into_iter().map(|t| t.name.to_string()).collect();
        for tool in ["tap_native", "call_function", "list_functions", "open_link"] {
            let (topic, _) = crate::tool_docs::tool_section(tool).expect("documented");
            assert_eq!(topic.name, "robot", "{tool} is documented in the robot topic");
        }
        let robot_doc = crate::tool_docs::topic("robot").unwrap().body;
        assert!(robot_doc.contains("read_guide testing"));
        assert!(mcp_catalog::guides().any(|g| g.slug == "testing"), "the testing guide ships");

        for pinned in [None, Some("127.0.0.1:1".to_string())] {
            let svc = CatalogService::with_robot_mode(true, pinned);
            let instructions = svc.get_info().instructions.unwrap();
            let robot = &instructions[instructions.find("Robot tools").expect("a robot sentence")..];
            for name in robot
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .filter(|w| w.contains('_'))
            {
                assert!(tools.contains(name), "the robot sentence names `{name}`, which is not a tool");
            }
        }
    }

    /// A dev session whose relay could not register (a root-owned
    /// `~/.idealyst`) left `wait_for_app` blaming a build that had finished.
    /// The message now tells the two failures apart.
    #[test]
    fn regression_wait_for_app_timeout_says_what_was_missing() {
        let none = wait_for_app_timeout_message(
            "any app",
            400,
            &[],
            "/home/u/.idealyst/apps does not exist and can't be created (/home/u/.idealyst is not a writable directory), so no app can register there",
        );
        assert!(none.contains("no registration for any app"), "{none}");
        assert!(none.contains("can't be created"), "{none}");
        assert!(none.contains("could not write its registration"), "{none}");

        let silent = wait_for_app_timeout_message("any app", 60, &["App".into()], "unused");
        assert!(silent.contains("App registered but never answered"), "{silent}");
        assert!(silent.contains("local=true"), "{silent}");
        assert!(!silent.contains("unused"), "{silent}");
    }

    #[test]
    fn robot_status_names_the_pinned_bridge_and_the_off_switch() {
        let pinned = CatalogService::with_robot_mode(true, Some("127.0.0.1:1".into()));
        assert!(pinned
            .get_info()
            .instructions
            .unwrap()
            .contains("--robot-port"));

        let off = CatalogService::with_robot_mode(false, None);
        let instructions = off.get_info().instructions.unwrap();
        assert!(instructions.contains("disabled"), "{instructions}");
        assert!(instructions.contains("--no-robot"), "{instructions}");
    }

    #[tokio::test]
    async fn a_failed_first_build_becomes_unavailable_not_initializing() {
        // Still initializing would imply "wait, it's coming". It isn't.
        let svc = CatalogService::with_robot_mode(false, None);
        svc.mark_initializing().await;
        svc.mark_build_failed("the project does not compile").await;
        assert_eq!(svc.status().await.state, CatalogState::Unavailable);
    }

    #[tokio::test]
    async fn wait_for_current_covers_the_initializing_window() {
        // The affordance that makes a non-blocking start usable: one
        // waiting call instead of polling across turns.
        let svc = Arc::new(CatalogService::with_robot_mode(false, None));
        svc.mark_initializing().await;
        let waiter = {
            let svc = svc.clone();
            tokio::spawn(async move {
                svc.wait_for_current(std::time::Duration::from_secs(30)).await
            })
        };
        tokio::task::yield_now().await;
        let cat =
            ResolvedCatalog::build_from_json(&multi_project_catalog().to_string()).unwrap();
        svc.replace_catalog(cat).await;
        let st = waiter.await.unwrap();
        assert!(st.is_settled_current(), "{st:?}");
    }

    #[tokio::test]
    async fn the_app_filter_applies_to_the_live_arm_too() {
        // Regression: `app_matches` guarded only the static arm, so with
        // a live app registered the filter silently matched everything.
        // Explicit robot mode synthesizes one live app named "app" at an
        // address nothing answers on.
        let svc = CatalogService::with_robot_mode(true, Some("127.0.0.1:1".into()));
        let cat = ResolvedCatalog::build_from_json(&multi_project_catalog().to_string()).unwrap();
        svc.replace_catalog(cat).await;

        // Filtering to a DIFFERENT app must skip the live entry entirely
        // and fall through to the static catalog — which is how we can
        // tell the guard ran without needing a reachable bridge.
        let out = svc
            .list_components(Parameters(FilterRequest {
                filter: None,
                app: Some("beta_app".into()),
            }))
            .await
            .unwrap();
        let mut names = component_names(&out);
        names.sort();
        assert_eq!(
            names,
            vec!["AppRoot".to_string(), "PinPad".to_string()],
            "a non-matching live app must not contribute, and must not \
             suppress the static catalog"
        );
    }

    #[tokio::test]
    async fn every_catalog_tool_carries_the_marker_when_not_current() {
        // The marker started on 5 of ~30 catalog tools, so `list_recipes`
        // and `list_icon_sets` could return [] mid-build with nothing to
        // say why — the same defect the marker exists to prevent, just
        // hiding in the tools nobody checked. The catalog is swapped
        // WHOLESALE, so every slice can differ across a build; every tool
        // that reads it must be able to say so.
        let svc = CatalogService::with_robot_mode(false, None);
        svc.mark_initializing().await;

        macro_rules! assert_marked {
            ($label:expr, $call:expr) => {{
                let out = $call.await.expect($label);
                assert!(
                    format!("{out:?}").contains("INITIALIZING"),
                    "{} must carry the freshness marker",
                    $label
                );
            }};
        }

        assert_marked!("list_components", svc.list_components(Parameters(FilterRequest::default())));
        assert_marked!("list_recipes", svc.list_recipes(Parameters(FilterRequest::default())));
        assert_marked!("list_icon_sets", svc.list_icon_sets(Parameters(FilterRequest::default())));
        assert_marked!("list_types", svc.list_types(Parameters(FilterRequest::default())));
        assert_marked!("list_values", svc.list_values(Parameters(FilterRequest::default())));
        assert_marked!("list_primitives", svc.list_primitives(Parameters(FilterRequest::default())));
        assert_marked!("list_sdks", svc.list_sdks(Parameters(FilterRequest::default())));
        assert_marked!("list_macros", svc.list_macros(Parameters(FilterRequest::default())));
        assert_marked!("list_scopes", svc.list_scopes(Parameters(FilterRequest::default())));
        assert_marked!("list_tokens", svc.list_tokens());
        assert_marked!("list_guides", svc.list_guides());
    }

    #[tokio::test]
    async fn a_current_catalog_leaves_every_tool_silent() {
        // The other half of the contract: zero noise in the happy path,
        // or the marker becomes wallpaper and stops being read.
        let svc = multi_project_service().await;
        for (label, out) in [
            ("list_components", svc.list_components(Parameters(FilterRequest::default())).await.unwrap()),
            ("list_recipes", svc.list_recipes(Parameters(FilterRequest::default())).await.unwrap()),
            ("list_icon_sets", svc.list_icon_sets(Parameters(FilterRequest::default())).await.unwrap()),
            ("list_guides", svc.list_guides().await.unwrap()),
        ] {
            assert_eq!(out.content.len(), 1, "{label} must return payload only when current");
            assert!(!format!("{out:?}").contains("[catalog:"), "{label} must be silent");
        }
    }

    #[tokio::test]
    async fn a_miss_says_so_when_live_apps_were_not_searched() {
        // describe_component / find_uses / find_dependencies resolve
        // against the project catalog only, while list_components fans
        // out across live bridges. Reporting a flat "not found" while a
        // live app went unconsulted is a confidently wrong answer.
        let svc = CatalogService::with_robot_mode(true, Some("127.0.0.1:1".into()));
        let cat = ResolvedCatalog::build_from_json(&multi_project_catalog().to_string()).unwrap();
        svc.replace_catalog(cat).await;

        let err = svc
            .describe_component(Parameters(NameRequest {
                name: "NoSuchComponent".into(),
                app: None,
            }))
            .await
            .expect_err("a miss is still an error");
        let msg = format!("{err:?}");
        assert!(msg.contains("not found"), "{msg}");
        assert!(
            msg.contains("does NOT search them"),
            "a miss must disclose the unsearched live apps: {msg}"
        );
    }

    #[tokio::test]
    async fn a_miss_with_no_live_apps_stays_a_plain_not_found() {
        // No caveat when there is nothing unsearched — the note must not
        // become boilerplate on every miss.
        let svc = multi_project_service().await;
        let err = svc
            .describe_component(Parameters(NameRequest {
                name: "NoSuchComponent".into(),
                app: None,
            }))
            .await
            .unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("not found"), "{msg}");
        assert!(!msg.contains("live"), "no caveat when nothing was skipped: {msg}");
    }

    /// The robot verbs the MCP server didn't expose — state, scrolling,
    /// navigation, watched signals, live component instances, and any verb
    /// by name — each reach the bridge with the right verb and arguments.
    /// Before, `invoke_method` was listed but its `instance_id` could not be
    /// found over MCP (`list_components` is the catalog), and navigation
    /// and signals had no tools at all.
    #[tokio::test]
    async fn regression_robot_tools_reach_every_bridge_verb() {
        use std::sync::{Arc, Mutex};
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen: Arc<Mutex<Vec<(String, serde_json::Value)>>> = Arc::default();
        let log = seen.clone();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (r, mut w) = stream.into_split();
            let mut lines = BufReader::new(r).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let req: serde_json::Value = serde_json::from_str(&line).unwrap();
                log.lock().unwrap().push((req["cmd"].as_str().unwrap().to_string(), req["args"].clone()));
                let resp = json!({ "id": req["id"], "ok": "ok" });
                w.write_all(format!("{resp}\n").as_bytes()).await.unwrap();
            }
        });
        let svc = CatalogService::with_robot_mode(true, Some(addr.to_string()));

        svc.get_state(Parameters(RobotElementId { element_id: 4, app: None })).await.unwrap();
        svc.set_scroll(Parameters(RobotSetScroll { element_id: 4, x: 0.0, y: 120.0, app: None })).await.unwrap();
        svc.list_navigators(Parameters(RobotAppOnly { app: None })).await.unwrap();
        svc.get_navigator_state(Parameters(RobotNavId { nav_id: 2, app: None })).await.unwrap();
        svc.navigate(Parameters(RobotNavigate { nav_id: 2, action: "push".into(), path: "/a".into(), app: None }))
            .await
            .unwrap();
        svc.list_watched_signals(Parameters(RobotAppOnly { app: None })).await.unwrap();
        svc.read_signal(Parameters(RobotSignalRef { name: Some("count".into()), id: None, app: None })).await.unwrap();
        svc.write_signal(Parameters(RobotWriteSignal { name: Some("count".into()), id: None, value: json!(3), app: None }))
            .await
            .unwrap();
        svc.list_component_instances(Parameters(RobotAppOnly { app: None })).await.unwrap();
        svc.get_component_instance(Parameters(RobotInstanceId { instance_id: 9, app: None })).await.unwrap();
        svc.call_verb(Parameters(RobotRawCall { verb: "get_identity".into(), args: json!({}), app: None }))
            .await
            .unwrap();
        svc.list_functions(Parameters(RobotAppOnly { app: None })).await.unwrap();
        svc.open_link(Parameters(RobotOpenLink { url: "myapp://items/4".into(), app: None })).await.unwrap();
        svc.tap_native(Parameters(RobotElementId { element_id: 5, app: None })).await.unwrap();
        svc.drag_native(Parameters(RobotDragNative { element_id: 5, points: vec![[1.0, 2.0], [3.0, 4.0]], app: None }))
            .await
            .unwrap();
        for action in [PointerNativeAction::Hover, PointerNativeAction::Press, PointerNativeAction::Release] {
            svc.pointer_native(Parameters(RobotPointerNative { element_id: 6, action, app: None })).await.unwrap();
        }

        let seen = seen.lock().unwrap().clone();
        let expected = vec![
            ("get_state", json!({ "element_id": 4 })),
            ("set_scroll", json!({ "element_id": 4, "x": 0.0, "y": 120.0 })),
            ("list_navigators", json!({})),
            ("get_navigator_state", json!({ "nav_id": 2 })),
            ("navigate", json!({ "nav_id": 2, "action": "push", "path": "/a" })),
            ("list_watched_signals", json!({})),
            ("read_signal", json!({ "name": "count" })),
            ("write_signal", json!({ "name": "count", "value": 3 })),
            ("list_components", json!({})),
            ("get_component", json!({ "instance_id": 9 })),
            ("get_identity", json!({})),
            ("list_functions", json!({})),
            ("open_link", json!({ "url": "myapp://items/4" })),
            ("tap_native", json!({ "element_id": 5 })),
            ("drag_native", json!({ "element_id": 5, "points": [[1.0, 2.0], [3.0, 4.0]] })),
            ("hover_native", json!({ "element_id": 6 })),
            ("press_native", json!({ "element_id": 6 })),
            ("release_native", json!({ "element_id": 6 })),
        ];
        let got: Vec<(&str, serde_json::Value)> = seen.iter().map(|(c, a)| (c.as_str(), a.clone())).collect();
        assert_eq!(got, expected);
    }

    /// `call_function` waits for an async robot function: the bridge
    /// answers `pending` with a call id, and the tool polls
    /// `poll_function` until the call is done, returning its value.
    #[tokio::test]
    async fn call_function_waits_for_an_async_function() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (r, mut w) = stream.into_split();
            let mut lines = BufReader::new(r).lines();
            let mut polls = 0;
            while let Ok(Some(line)) = lines.next_line().await {
                let req: serde_json::Value = serde_json::from_str(&line).unwrap();
                let ok = match (req["cmd"].as_str().unwrap(), req["args"]["name"].as_str()) {
                    ("call_function", Some("demo.slow")) => json!({ "done": false, "call": 4 }),
                    ("call_function", _) => json!({ "done": true, "value": req["args"]["args"]["x"] }),
                    ("poll_function", _) => {
                        polls += 1;
                        if polls < 3 { json!({ "done": false }) } else { json!({ "done": true, "value": [1, 2] }) }
                    }
                    (cmd, _) => panic!("unexpected verb {cmd}"),
                };
                let resp = json!({ "id": req["id"], "ok": ok });
                w.write_all(format!("{resp}\n").as_bytes()).await.unwrap();
            }
        });
        let svc = CatalogService::with_robot_mode(true, Some(addr.to_string()));
        let text = |r: CallToolResult| format!("{:?}", r.content);

        let sync = svc
            .call_function(Parameters(RobotCallFunction { name: "demo.echo".into(), args: Some(json!({ "x": 5 })), timeout_ms: None, app: None }))
            .await
            .unwrap();
        assert_ne!(sync.is_error, Some(true));
        assert!(text(sync).contains('5'));

        let slow = svc
            .call_function(Parameters(RobotCallFunction { name: "demo.slow".into(), args: None, timeout_ms: Some(5_000), app: None }))
            .await
            .unwrap();
        assert_ne!(slow.is_error, Some(true), "{slow:?}");
        let body = text(slow);
        assert!(body.contains('1') && body.contains('2'), "polled until done: {body}");
    }
}

#[cfg(test)]
mod tool_surface_tests {
    //! The advertised tool surface is resent to the model as context for the
    //! whole session, so its size is a contract: definitions stay terse and
    //! the long form lives in `get_docs`. These tests keep both halves in
    //! step.

    use super::*;
    use crate::tool_docs::{split_sections, tool_section, TOPICS};
    use serde_json::Value;

    /// Serialized size of every tool definition. 90.9 k chars before tool
    /// docs moved behind `get_docs`; ~27 k after. The headroom is for new
    /// tools, not for regrowing descriptions.
    const SURFACE_BUDGET_CHARS: usize = 32_000;
    const DESCRIPTION_MAX_CHARS: usize = 120;
    const PARAM_DOC_MAX_CHARS: usize = 100;
    const INSTRUCTIONS_MAX_CHARS: usize = 700;

    fn service() -> CatalogService {
        CatalogService::with_robot_mode(false, None)
    }

    /// Tool definitions as the `ServerHandler` serves them. `get_tool` is
    /// generated from the same router expression as `list_tools`, so this
    /// checks what a client receives, not just what the field holds.
    fn advertised(svc: &CatalogService) -> Vec<rmcp::model::Tool> {
        CatalogService::tool_router()
            .list_all()
            .iter()
            .map(|t| ServerHandler::get_tool(svc, &t.name).expect("served"))
            .collect()
    }

    fn text(r: &CallToolResult) -> String {
        r.content.iter().filter_map(|c| c.as_text().map(|t| t.text.clone())).collect()
    }

    fn walk<'a>(v: &'a Value, path: String, out: &mut Vec<(String, &'a Value)>) {
        out.push((path.clone(), v));
        match v {
            Value::Object(m) => m.iter().for_each(|(k, c)| walk(c, format!("{path}.{k}"), out)),
            Value::Array(a) => a.iter().enumerate().for_each(|(i, c)| walk(c, format!("{path}[{i}]"), out)),
            _ => {}
        }
    }

    #[test]
    fn tool_surface_stays_within_budget() {
        let tools = advertised(&service());
        let total: usize = tools.iter().map(|t| serde_json::to_string(t).unwrap().len()).sum();
        assert!(total <= SURFACE_BUDGET_CHARS, "tool surface is {total} chars (budget {SURFACE_BUDGET_CHARS})");
        for t in &tools {
            let d = t.description.as_deref().unwrap_or("");
            assert!(!d.is_empty(), "`{}` has no description", t.name);
            assert!(
                d.chars().count() <= DESCRIPTION_MAX_CHARS,
                "`{}` description is {} chars; move detail into its docs section",
                t.name,
                d.chars().count()
            );
            if let Some(Value::Object(props)) = t.input_schema.get("properties") {
                for (name, p) in props {
                    let pd = p.get("description").and_then(Value::as_str).unwrap_or("");
                    assert!(
                        pd.chars().count() <= PARAM_DOC_MAX_CHARS,
                        "`{}.{name}` param doc is {} chars",
                        t.name,
                        pd.chars().count()
                    );
                }
            }
        }
    }

    #[test]
    fn advertised_schemas_carry_no_schemars_noise() {
        for t in advertised(&service()) {
            let schema = Value::Object((*t.input_schema).clone());
            for key in ["$schema", "title", "description"] {
                assert!(schema.get(key).is_none(), "`{}` schema root has `{key}`", t.name);
            }
            let mut nodes = Vec::new();
            walk(&schema, String::new(), &mut nodes);
            for (path, node) in nodes {
                let Value::Object(m) = node else { continue };
                if path.ends_with(".properties") || path.ends_with(".$defs") {
                    continue; // keys here are argument names, not keywords
                }
                assert!(!m.contains_key("format"), "`{}` has `format` at {path}", t.name);
                if let Some(Value::Array(ts)) = m.get("type") {
                    assert!(ts.len() > 2 || !ts.contains(&json!("null")), "`{}` has a null union at {path}", t.name);
                }
            }
        }
    }

    #[test]
    fn every_tool_has_exactly_one_docs_section() {
        for t in service().tool_router.list_all() {
            let owners: Vec<&str> = TOPICS
                .iter()
                .filter(|topic| split_sections(topic.body, 2).iter().any(|s| s.keys().contains(&t.name.as_ref())))
                .map(|topic| topic.name)
                .collect();
            assert_eq!(owners.len(), 1, "`{}` should have exactly one `## {}` docs section, found in {owners:?}", t.name, t.name);
        }
    }

    #[test]
    fn docs_sections_name_only_registered_tools() {
        // Single-word headings that are concepts rather than tools.
        const CONCEPT_SECTIONS: &[&str] = &["filter", "elements"];
        let tools = service().tool_router.list_all();
        for topic in TOPICS {
            for s in split_sections(topic.body, 2) {
                for key in s.keys() {
                    assert!(
                        CONCEPT_SECTIONS.contains(&key) || tools.iter().any(|t| t.name == key),
                        "docs/{}.md has `## {key}`, which is not a registered tool; rename or remove it",
                        topic.name
                    );
                }
            }
        }
    }

    #[test]
    fn instructions_are_short_and_point_at_get_docs() {
        let info = service().get_info();
        let instructions = info.instructions.unwrap_or_default();
        assert!(instructions.len() <= INSTRUCTIONS_MAX_CHARS, "instructions are {} chars", instructions.len());
        assert!(instructions.contains("get_docs"));
    }

    #[test]
    fn get_docs_index_lists_every_topic_and_its_tools() {
        let out = text(&service().docs_result(GetDocsRequest::default()));
        for topic in TOPICS {
            assert!(out.contains(&format!("`{}`", topic.name)), "index misses topic {}", topic.name);
        }
        assert!(out.contains("compare_native_parity"));
    }

    #[test]
    fn get_docs_tool_page_has_summary_params_and_section() {
        let svc = service();
        let r = svc.docs_result(GetDocsRequest { name: Some("tap".into()), query: None });
        assert_ne!(r.is_error, Some(true));
        let out = text(&r);
        assert!(out.starts_with("# tap\n"));
        // Parameters come from the advertised schema.
        assert!(out.contains("- `serial` (string)"), "got:\n{out}");
        // The long form comes from docs/robot.md.
        assert!(out.contains("adb shell input tap"));
        assert!(out.contains("Topic: `robot`."));
    }

    #[test]
    fn get_docs_tool_query_narrows_to_matching_subsections() {
        let out = text(&service().docs_result(GetDocsRequest {
            name: Some("compare_native_parity".into()),
            query: Some("navigator chrome".into()),
        }));
        assert!(out.contains("### Scoping"), "got:\n{out}");
        assert!(!out.contains("### Alignment"), "got:\n{out}");
    }

    #[test]
    fn get_docs_topic_and_cross_topic_search() {
        let svc = service();
        let topic = text(&svc.docs_result(GetDocsRequest { name: Some("dev".into()), query: None }));
        assert_eq!(topic, crate::tool_docs::topic("dev").unwrap().body);
        let search = text(&svc.docs_result(GetDocsRequest { name: None, query: Some("SIGKILL".into()) }));
        assert!(search.contains("## stop_dev"), "got:\n{search}");
    }

    #[test]
    fn get_docs_unknown_name_is_an_error_with_suggestions() {
        let r = service().docs_result(GetDocsRequest { name: Some("parity".into()), query: None });
        assert_eq!(r.is_error, Some(true));
        assert!(text(&r).contains("compare_native_parity"));
    }

    #[test]
    fn tool_section_resolves_multi_tool_headings() {
        let (topic, section) = tool_section("clear_logs").unwrap();
        assert_eq!(topic.name, "robot");
        assert_eq!(section.title, "get_logs, clear_logs");
    }
}
