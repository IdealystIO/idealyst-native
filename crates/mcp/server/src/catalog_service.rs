//! `CatalogService` — the rmcp `ServerHandler` implementation that
//! surfaces the mcp-catalog catalog as MCP tools + a resource.
//!
//! Tools (spec §5):
//! - `list_components` — every component, sorted by FQN.
//! - `describe_component(name)` — full record (docs, file/line,
//!   params, composes).
//! - `find_uses(name)` — reverse adjacency.
//! - `find_dependencies(name)` — forward adjacency, with each edge's
//!   resolution status.
//! - `search(query)` — fulltext over names + doc comments.
//!
//! Resource: `idealyst://catalog` returns the full denormalized
//! catalog as JSON.

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
/// `Arc<RwLock<...>>` so the watcher thread (phase 5) can swap in
/// a fresh `ResolvedCatalog` without taking the server down.
#[derive(Clone)]
pub struct CatalogService {
    catalog: Arc<RwLock<ResolvedCatalog>>,
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
    // `#[tool_handler]` reads this through the trait impl, not via
    // a direct field access — the dead-code analyzer can't see it.
    #[allow(dead_code)]
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
    /// Component short-name (e.g. `card`) or fully-qualified name
    /// (e.g. `mcp_demo::components::card`). Short-name lookups are
    /// resolved via spec §6 proximity rules.
    pub name: String,
    /// Optional app filter (from `list_apps`). Surface-level today —
    /// the field exists so callers can pre-tag the request; the
    /// per-app catalog routing for describe/search/find_uses lands
    /// in a follow-up. For now the lookup is against the in-memory
    /// catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[allow(dead_code)]
    pub app: Option<String>,
}

/// Find-by criteria for `find_element` / `find_all_elements` /
/// `count_elements`. Every field is optional; an empty struct matches
/// nothing on the bridge side. `serde(skip_serializing_if = "Option::is_none")`
/// keeps the wire payload tight so the bridge sees only the criteria
/// the caller actually set.
/// Common preamble on every Robot args struct: the optional app
/// selector. Use a flat field so MCP clients see a uniform "app"
/// arg in tool schemas. When the registry has exactly one app the
/// arg is optional (inferred); when multiple apps are registered
/// it's required.
///
/// Kept as a top-level helper here so each args struct stays
/// independent (rmcp's `#[derive(JsonSchema)]` doesn't flatten
/// nested-struct shapes the way humans expect).
const APP_ARG_DOC: &str = "Target app name (from `list_apps`). Optional when only one app is registered; required otherwise.";

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotFindArgs {
    /// Target app name (from `list_apps`). Optional when only one
    /// app is registered; required otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// Find by test ID (exact match).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_id: Option<String>,
    /// Find by label (exact match).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Find by label substring.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label_contains: Option<String>,
    /// Find by element kind: `View`, `Text`, `Button`, `Pressable`,
    /// `TextInput`, `Toggle`, `Slider`, etc.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotElementId {
    /// Element ID returned by `find_element` / `find_all_elements`.
    pub element_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotParityArgs {
    /// First app selector (the `selector` field from `list_apps` — `name`, or
    /// `name:platform` when one app runs on two platforms, e.g. `todo:web`).
    pub app_a: String,
    /// Second app selector. Compared against `app_a`.
    pub app_b: String,
    /// Per-channel color tolerance, `0.0..1.0`. Default `0.02` (absorbs
    /// color-space round-trip drift).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_tolerance: Option<f32>,
    /// Length/number tolerance in px (corner radius, font size, …). Default
    /// `0.5` (sub-pixel rounding).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length_tolerance: Option<f32>,
    /// Disable cross-platform normalization — report every representation
    /// difference (inherited fonts on containers, transparent-vs-absent
    /// colors, system-font name spelling). Default `false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<bool>,
    /// Scope the comparison to the subtree rooted at this `test_id` — pass a
    /// content anchor to EXCLUDE the navigator chrome (sidebars/headers are
    /// built per-platform with different native structure and legitimately
    /// don't align). Omit to compare the whole tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotTypeText {
    pub element_id: u64,
    pub text: String,
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
    /// Target app name (from `list_apps`). Optional when only one app
    /// is registered; required otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// Tap the center of this element (id from `find_element` /
    /// `find_all_elements`). Takes precedence over `test_id` and raw
    /// `x`/`y`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element_id: Option<u64>,
    /// Tap the center of the element with this `test_id` — resolved over
    /// the bridge via `find_element`. Used when `element_id` is absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_id: Option<String>,
    /// Raw tap point in **physical device pixels** (screen top-left
    /// origin). Used when neither `element_id` nor `test_id` is given;
    /// both `x` and `y` are required together.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<i32>,
    /// adb device serial (from `adb devices`). Optional when exactly one
    /// device/emulator is attached; required to disambiguate otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotInvokeMethod {
    pub instance_id: u64,
    pub method: String,
    /// Method args keyed by parameter name. Omit / pass `{}` for
    /// no-arg methods.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotAppOnly {
    /// Target app name (from `list_apps`). Optional when only one
    /// app is registered; required otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotScreenshotArgs {
    /// Target app name (from `list_apps`). Optional when only one app
    /// is registered; required otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// Capture width in physical pixels. Optional — defaults to the
    /// session's configured viewport size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// Capture height in physical pixels. Optional — see `width`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Capture source. One of `"auto"` (default — try the live client's
    /// native surface, fall back to the wgpu replay renderer on error),
    /// `"client"` (force the real client surface), or `"replay"` (force
    /// the server-side scene re-render). The sidecar reads this at
    /// `sidecar.rs`; without it declared here the field is dropped at the
    /// typed MCP boundary and `auto` can never be overridden.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Args for `run_dev` — launch an `idealyst dev` session detached and
/// tracked. Mirrors the subset of `idealyst dev` CLI flags worth
/// driving from MCP: target platforms, the `--local` mode toggle, and
/// the robot knobs.
#[derive(Debug, Default, serde::Deserialize, schemars::JsonSchema)]
pub struct RunDevArgs {
    /// Project directory to run. Optional — defaults to the MCP server's
    /// working directory (the project it was launched in).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    /// Target platforms — any of `web`, `ios`, `android`, `macos`,
    /// `terminal`. Omit (and leave `all` false) to use the project
    /// manifest's declared `targets`.
    #[serde(default)]
    pub platforms: Vec<String>,
    /// Run every platform the host can build for (`--all`): web +
    /// android everywhere, plus ios + macos on macOS hosts.
    #[serde(default)]
    pub all: bool,
    /// `--local`: build the app natively per platform with a file-watch
    /// rebuild loop, bypassing the runtime-server wire. State does not
    /// survive saves. Default false (runtime-server hot-reload).
    #[serde(default)]
    pub local: bool,
    /// `--no-robot`: disable the Robot bridge/relay for this session.
    /// Default false — the bridge is on so the MCP Robot tools can drive
    /// the running app.
    #[serde(default)]
    pub no_robot: bool,
    /// `--bridge-port`: pin the Robot bridge to a fixed port instead of
    /// an ephemeral one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge_port: Option<u16>,
    /// `--screenshot-dir`: where the relay saves screenshot PNGs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot_dir: Option<String>,
    /// `--no-build` (web only): skip the initial build and just serve.
    #[serde(default)]
    pub no_build: bool,
}

/// Args for `stop_dev` — stop one tracked dev session by id, or all of
/// them.
#[derive(Debug, Default, serde::Deserialize, schemars::JsonSchema)]
pub struct StopDevArgs {
    /// The session id to stop (from `run_dev` / `list_dev_sessions`).
    /// Omit and set `all: true` to stop every tracked session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<u64>,
    /// Stop every tracked dev session. Ignored when `session_id` is set.
    #[serde(default)]
    pub all: bool,
}

/// Args for `read_dev_log` — tail a tracked dev session's log file.
#[derive(Debug, Default, serde::Deserialize, schemars::JsonSchema)]
pub struct ReadDevLogArgs {
    /// The dev session id (from `run_dev` / `list_dev_sessions`).
    pub session_id: u64,
    /// How many lines from the end to return. Default 100, capped at 2000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<usize>,
    /// Case-insensitive substring filter applied BEFORE the tail, so
    /// `"error"` returns the last N matching lines. Omit for the raw tail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
}

/// Args for `wait_for_app` — block until a running app registers.
#[derive(Debug, Default, serde::Deserialize, schemars::JsonSchema)]
pub struct WaitForAppArgs {
    /// App name to wait for (matches the registration `name`). Omit to
    /// wait for ANY app to come up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Max seconds to wait before giving up. Default 60, capped at 600.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

#[allow(dead_code)]
fn _doc_marker() -> &'static str {
    APP_ARG_DOC
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
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let dir = std::path::Path::new(&home).join(".idealyst").join("screenshots");
    if std::fs::create_dir_all(&dir).is_err() {
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
    /// Free-form query. Matched case-insensitively against component
    /// names + doc comments.
    pub query: String,
    /// Optional app filter. Forward-compatible — see `NameRequest::app`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[allow(dead_code)]
    pub app: Option<String>,
}

/// Shared optional filter for the `list_*` catalog tools. Lets the LLM
/// narrow a large catalog (a project pulling in idea-ui surfaces 40+
/// components) down to the handful it cares about before drilling into
/// one with the matching `describe_*` tool — the intended top-down flow.
#[derive(Debug, Default, serde::Deserialize, schemars::JsonSchema)]
pub struct FilterRequest {
    /// Case-insensitive filter. Every whitespace-separated term must
    /// match (AND); a term matches if it is a substring of the item's
    /// name or fully-qualified name (and, where applicable, its
    /// category). `*` is a glob wildcard: `butt*`, `*view`, `*field*`.
    /// Omit / empty to list everything.
    #[serde(default)]
    pub filter: Option<String>,
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
    /// Icon pack crate name (e.g. `icons-lucide`).
    pub name: String,
    /// Pagination offset into the icon list. Default 0.
    #[serde(default)]
    pub offset: Option<usize>,
    /// Page size. Default 100, capped at 500.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Args for `search_icons` — a name keyword, an optional pack restriction,
/// and a result cap.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct IconSearchRequest {
    /// Case-insensitive substring matched against icon names.
    pub query: String,
    /// Optional pack name to restrict the search to one icon set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set: Option<String>,
    /// Max results. Default 50, capped at 200.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Args for `lint_project` — run the idealyst source linter over a
/// project's un-expanded Rust source.
#[derive(Debug, Default, serde::Deserialize, schemars::JsonSchema)]
pub struct LintRequest {
    /// File or directory to lint (walked recursively, skipping
    /// `target/`, `.git/`, `node_modules/`). Relative paths resolve
    /// against the server's working directory. Omit to lint the live
    /// app's project root when exactly one app is running, else the
    /// current directory.
    #[serde(default)]
    pub path: Option<String>,
    /// Treat warnings as failures: when true, the result's `failed`
    /// flag is set if any warning fires (CI strict mode), not only on
    /// errors / parse failures. Default false.
    #[serde(default)]
    pub deny_warnings: Option<bool>,
    /// Max diagnostics to return in the `diagnostics` array. The summary
    /// counts (`error_count`, `warn_count`, `total_diagnostics`) always
    /// reflect the full run. Default 200, capped at 1000.
    #[serde(default)]
    pub limit: Option<usize>,
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
        Self {
            catalog: Arc::new(RwLock::new(ResolvedCatalog::build())),
            robot_mode,
            robot,
            resolver: Arc::new(RobotResolver::default()),
            catalog_cache: Arc::new(CatalogCache::default()),
            discovery,
            dev_runner: Arc::new(crate::dev_runner::DevRunner::new()),
            tool_router: Self::tool_router(),
        }
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
    /// thread (phase 5). After replacing, the service should send
    /// `notifications/resources/list_changed` via the rmcp peer.
    pub async fn replace_catalog(&self, new_cat: ResolvedCatalog) {
        let mut guard = self.catalog.write().await;
        *guard = new_cat;
    }

    /// Re-extract the catalog from the *current process's* inventory
    /// slice. Useful when the consumer has refreshed inventory state
    /// some other way; in the typical phase-5 deployment a separate
    /// subprocess extracts and JSON-pipes the catalog, which the
    /// watcher feeds into [`replace_catalog`] instead.
    pub async fn reload_from_inventory(&self) {
        self.replace_catalog(ResolvedCatalog::build()).await;
    }

    #[tool(description = "List components — the top-down entry point. Returns a lightweight JSON array of { app, name, fqn, summary } (summary = first doc line) so you can scan a large catalog cheaply; pass `filter` to narrow (case-insensitive, glob `*`, matches name/fqn), then call `describe_component` for one item's props + full docs + composes. Source: live apps over their Robot bridge, else the in-process / project catalog.")]
    async fn list_components(
        &self,
        Parameters(req): Parameters<FilterRequest>,
    ) -> Result<CallToolResult, McpError> {
        let mut json: Vec<serde_json::Value> = Vec::new();

        let live = self.live_apps();

        for app in &live {
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
                json.push(serde_json::json!({
                    "app": null,
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

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json).unwrap(),
        )]))
    }

    #[tool(description = "Get the full record for one component: docs, params, file/line, and its composes edges with resolution status. Accepts a short-name or a fully-qualified name.")]
    async fn describe_component(
        &self,
        Parameters(req): Parameters<NameRequest>,
    ) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let entry = find_by_name(&cat, &req.name)
            .ok_or_else(|| McpError::invalid_params(format!("component {:?} not found", req.name), None))?;
        let edges = cat.dependencies(&EntryRef::of(entry));
        let json = entry_to_json(&cat, entry, edges);
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json).unwrap(),
        )]))
    }

    #[tool(description = "Find every component that composes the given target. Returns reverse adjacency as a JSON array of FQN strings.")]
    async fn find_uses(
        &self,
        Parameters(req): Parameters<NameRequest>,
    ) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let entry = find_by_name(&cat, &req.name)
            .ok_or_else(|| McpError::invalid_params(format!("component {:?} not found", req.name), None))?;
        let users = cat.uses(&EntryRef::of(entry));
        let json: Vec<String> = users.iter().map(|r| r.fqn()).collect();
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json).unwrap(),
        )]))
    }

    #[tool(description = "Find the components a given host composes. Returns forward adjacency as a JSON array of { raw_name, resolved_fqn, status } per edge.")]
    async fn find_dependencies(
        &self,
        Parameters(req): Parameters<NameRequest>,
    ) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let entry = find_by_name(&cat, &req.name)
            .ok_or_else(|| McpError::invalid_params(format!("component {:?} not found", req.name), None))?;
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
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json).unwrap(),
        )]))
    }

    #[tool(description = "List `#[idealyst_tool]`-registered functions. Lightweight { name, fqn, return_type, summary }; pass `filter` (case-insensitive, glob `*`, matches name/fqn) to narrow, then `describe_tool` for params + full docs.")]
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

    #[tool(description = "Get the full record for a tool — docs, parameters, return type. Accepts short-name or FQN.")]
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

    #[tool(description = "Find a UI element in the running app by test_id, label, label_contains, or kind. Returns the first match. With multiple apps registered, pass `app` to select. Requires the app to be running with `--features robot`.")]
    async fn find_element(
        &self,
        Parameters(args): Parameters<RobotFindArgs>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("find_element", app.as_deref(), body).await
    }

    #[tool(description = "Find all UI elements in the running app matching the given criteria.")]
    async fn find_all_elements(
        &self,
        Parameters(args): Parameters<RobotFindArgs>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("find_all_elements", app.as_deref(), body).await
    }

    #[tool(description = "Click/press a Button or Pressable element in the running app. `element_id` comes from a prior find_element call.")]
    async fn click(
        &self,
        Parameters(args): Parameters<RobotElementId>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("click", app.as_deref(), body).await
    }

    #[tool(description = "Type text into a TextInput (replaces current value).")]
    async fn type_text(
        &self,
        Parameters(args): Parameters<RobotTypeText>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("type_text", app.as_deref(), body).await
    }

    #[tool(description = "Set a Toggle's value (true/false).")]
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

    #[tool(description = "Focus an element (TextInput typically).")]
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

    #[tool(description = "Get a snapshot of the running app's whole element tree.")]
    async fn get_snapshot(
        &self,
        Parameters(args): Parameters<RobotAppOnly>,
    ) -> Result<CallToolResult, McpError> {
        self.robot_call("get_snapshot", args.app.as_deref(), json!({})).await
    }

    #[tool(description = "Get child elements of a node.")]
    async fn get_children(
        &self,
        Parameters(args): Parameters<RobotElementId>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("get_children", app.as_deref(), body).await
    }

    #[tool(description = "Get an element's parent in the running app.")]
    async fn get_parent(
        &self,
        Parameters(args): Parameters<RobotElementId>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("get_parent", app.as_deref(), body).await
    }

    #[tool(description = "Count elements matching the criteria.")]
    async fn count_elements(
        &self,
        Parameters(args): Parameters<RobotFindArgs>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("count_elements", app.as_deref(), body).await
    }

    #[tool(description = "Get the running app's captured logs.")]
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

    #[tool(description = "Get an element's layout frame relative to its parent.")]
    async fn get_frame(
        &self,
        Parameters(args): Parameters<RobotElementId>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("get_frame", app.as_deref(), body).await
    }

    #[tool(description = "Get an element's absolute layout frame (window coords).")]
    async fn get_absolute_frame(
        &self,
        Parameters(args): Parameters<RobotElementId>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("get_absolute_frame", app.as_deref(), body).await
    }

    #[tool(description = "Read an element's PLATFORM-NATIVE render state — the resolved geometry and visual properties as the backend itself reports them (a macOS `CALayer`'s backgroundColor/cornerRadius/borderWidth + the resolved NSFont; web's `getComputedStyle` + `getBoundingClientRect`), NOT the styles the author wrote. Returns a `NativeNode` tree: { class (the platform class/tag, e.g. `NSView`/`div`), role, frame (logical px, window-relative), props (canonical keys: background_color/opacity/corner_radius/border_width/border_color/text_color/font_family/font_size/font_weight/text/shadow_radius/shadow_color/hidden — each a typed `{type,value}`), children (the native sub-objects composing THIS primitive, pruned at child framework elements) }. A key is ABSENT when the platform doesn't expose it (meaningful: missing ≠ 0). Use this to compare how the same element renders across platforms — pair with `compare_native_parity`, or read both sides and reason about the diffs yourself. `null` means the element isn't laid out yet or the backend doesn't implement the read (web + macOS do today; iOS/Android/terminal stub).")]
    async fn introspect_native(
        &self,
        Parameters(args): Parameters<RobotElementId>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("introspect_native", app.as_deref(), body).await
    }

    #[tool(description = "Compare the PLATFORM-NATIVE render state of two running apps — the automated cross-platform PARITY check. Aligns the two element trees STRUCTURALLY by a cross-platform signature (test_id where present, else framework kind + label — never raw positional index, which collapses when one platform nests an extra wrapper), reads each aligned element's `introspect_native`, and diffs the canonical props with cross-platform NORMALIZATION (font/text props compared only on text-bearing elements, since web reports inherited fonts everywhere and macOS doesn't; a transparent color treated as equivalent to an absent one; `font_family` matched by family class so `-apple-system`≡`.AppleSystemUIFont`=system while `ui-monospace` stays distinct). Returns { app_a, app_b, aligned (matched element count), structural: { only_a, only_b, items:[{path,kind,only_in}] } (elements one platform renders and the other doesn't — a structural divergence), prop_divergences, mismatches:[{path,key,kind,detail}] } where kind ∈ value_differs/prop_missing. Empty mismatches + empty structural = full parity. Pass `raw:true` to disable normalization and see every representation difference. Pass `root` (a test_id) to SCOPE the comparison to a content anchor and exclude the navigator chrome (sidebars/headers are built per-platform with different native structure and legitimately don't align — scoping removes that structural noise). IMPORTANT for a meaningful result: both apps must show the SAME route/screen at the SAME viewport size — otherwise responsive layout makes the trees legitimately diverge. `app_a`/`app_b` are `list_apps` selectors (`name`, or `name:platform` like `todo:web` vs `todo:macos`). This finds WHERE platforms diverge; reason about WHY (expected platform difference vs real bug) and, if asked, propose the upstream fix.")]
    async fn compare_native_parity(
        &self,
        Parameters(args): Parameters<RobotParityArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.do_compare_native_parity(args).await
    }

    #[tool(description = "Inject a REAL OS-level touch on an Android emulator/device via `adb shell input tap`. Unlike `click` — which calls the element's handler closure directly and bypasses the platform event system — this dispatches a genuine touch through the full Android input stack (InputManager → window → view hit-test), so it exercises hit-testing, overlays, z-order, and disabled-state the way a real finger would. Target the tap one of three ways: `element_id` (from `find_element`), `test_id` (resolved over the bridge), or raw `x`/`y` in physical device pixels. Element-based taps read the element's on-screen pixel rect via `get_device_frame` and tap its center — no host-side density math, since the device reports physical pixels directly. Requires Android platform-tools `adb` on PATH and a device/emulator attached (pass `serial` to disambiguate when more than one is). Android-only: other backends don't implement `device_frame` yet.")]
    async fn tap(
        &self,
        Parameters(args): Parameters<RobotTap>,
    ) -> Result<CallToolResult, McpError> {
        let msg = self.do_tap(args).await?;
        Ok(CallToolResult::success(vec![Content::text(msg)]))
    }

    #[tool(description = "Invoke a `#[method]`-tagged method on an element's instance (Robot's component-level escape hatch).")]
    async fn invoke_method(
        &self,
        Parameters(args): Parameters<RobotInvokeMethod>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("invoke_method", app.as_deref(), body).await
    }

    #[tool(description = "Capture a PNG screenshot of the running app and save it to disk, returning the file PATH (not the image bytes). Two capture sources, selected via the optional `source` arg: `client` snapshots the REAL rendered surface (macOS/iOS/Android native widgets/fonts/view-hierarchy; web rasterizes the live DOM to PNG), working both for a `--local` app and for a runtime-server session by asking the connected client to capture over the wire; `replay` rasterizes the current scene with the wgpu renderer server-side (always available, even with no client attached, but uses the framework's renderer not the platform's). `auto` (the default) tries the real client and falls back to replay. Optional `width`/`height` in physical pixels are honored by the replay path (default: the session's viewport size); real-client capture always returns the device's own pixel dimensions. Response JSON: { path, width, height } — `path` is the absolute path to the saved PNG (`~/.idealyst/screenshots/<app>-<timestamp>.png` by default). The base64 is intentionally NOT returned: read the file at `path` to view the image. Requires the session to have registered the screenshot verb; returns an error otherwise.")]
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

    #[tool(description = "List every running idealyst app — discovered via per-process registration files at `~/.idealyst/apps/<name>-<pid>.json` that the running app's Robot bridge writes on bind. Each entry includes name, bundle_id, project_root, bridge_addr, and pid. Entries are removed automatically when the app exits (RAII cleanup on graceful shutdown; stale ones get pruned at scan time when `kill(pid, 0)` reports the process is gone).")]
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

    #[tool(description = "Launch an `idealyst dev` session for a project — the MCP-friendly equivalent of running `idealyst dev` in a terminal. Spawns the CLI detached (stdout/stderr go to `~/.idealyst/dev-logs/<name>-<id>.log`, returned as `log_path`) so it doesn't block the MCP connection, and TRACKS the process so `stop_dev` can tear the whole session down later. One call can run several platforms at once (they run in parallel inside the single dev process). Args: `platforms` (any of `web`/`ios`/`android`/`macos`/`terminal`; omit to use the project's manifest `targets`), `all` (every host-buildable platform), `local` (build natively per platform with file-watch rebuilds, no runtime-server wire — state doesn't survive saves; default is runtime-server hot-reload), `no_robot` (disable the Robot bridge/relay), `bridge_port`, `screenshot_dir`, `no_build` (web only), `dir` (project dir; defaults to the server's cwd). Returns the new session's { id, pid, dir, platforms, local, no_robot, log_path, status }. The dev process keeps running until you call `stop_dev`; read `log_path` to follow build progress or failures. Robot tools (find_element/click/screenshot/…) reach the running app once it registers — poll `list_apps`.")]
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
            bridge_port: args.bridge_port,
            screenshot_dir: args.screenshot_dir.map(std::path::PathBuf::from),
            no_build: args.no_build,
        };
        match self.dev_runner.start(launch) {
            Ok(info) => Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string_pretty(&info).unwrap(),
            )])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(description = "List the `idealyst dev` sessions this MCP server launched via `run_dev`. Returns a JSON array of { id, pid, dir, platforms, local, no_robot, log_path, uptime_secs, status } where `status` is `running` or `exited(<code>)`. A session that has exited is reported once (with its exit status) and then dropped from the list on the next call. This tracks ONLY sessions started through this server — apps you launched in a terminal show up in `list_apps`, not here.")]
    async fn list_dev_sessions(&self) -> Result<CallToolResult, McpError> {
        let sessions = self.dev_runner.list();
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&sessions).unwrap(),
        )]))
    }

    #[tool(description = "Stop a tracked `idealyst dev` session (or all of them). Pass `session_id` to stop one, or `all: true` to stop every session this server launched. On unix the whole dev process GROUP is signalled — the `idealyst dev` process and its children (cargo builds, web servers, simulators) — with a graceful SIGINT first (so simulators/servers tear down cleanly) escalating to SIGKILL after a grace window. Returns the stopped session(s)' final info. Stopping a session removes it from `list_dev_sessions`.")]
    async fn stop_dev(
        &self,
        Parameters(args): Parameters<StopDevArgs>,
    ) -> Result<CallToolResult, McpError> {
        if args.all && args.session_id.is_none() {
            let stopped = self.dev_runner.stop_all();
            return Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string_pretty(&stopped).unwrap(),
            )]));
        }
        match args.session_id {
            Some(id) => match self.dev_runner.stop(id) {
                Ok(info) => Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&info).unwrap(),
                )])),
                Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
            },
            None => Ok(CallToolResult::error(vec![Content::text(
                "stop_dev needs either `session_id` or `all: true`".to_string(),
            )])),
        }
    }

    #[tool(description = "Read the tail of a `run_dev` session's log — the way to follow a dev launch's build progress and see compile errors without filesystem access. Pass `session_id` (from `run_dev` / `list_dev_sessions`); optionally `lines` (from the end, default 100, max 2000) and `filter` (case-insensitive substring applied BEFORE the tail, so `filter:\"error\"` returns the last N matching lines — great for jumping straight to a compile failure). Logs stay readable even after the session exits and is pruned from `list_dev_sessions` (the file is found by globbing `~/.idealyst/dev-logs/`). Returns `{ session_id, log_path, lines, content }`.")]
    async fn read_dev_log(
        &self,
        Parameters(args): Parameters<ReadDevLogArgs>,
    ) -> Result<CallToolResult, McpError> {
        let Some(path) = self.dev_runner.log_path(args.session_id) else {
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "no dev session log for id {} — it was never started here, or its \
                 log file is gone. Check `list_dev_sessions`.",
                args.session_id
            ))]));
        };
        let lines = args.lines.unwrap_or(100).min(2000);
        match crate::dev_runner::tail_log(&path, lines, args.filter.as_deref()) {
            Ok(content) => {
                let out = json!({
                    "session_id": args.session_id,
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

    #[tool(description = "Block until a running idealyst app registers its Robot bridge, then return it — the bridge between `run_dev` (which returns as soon as the build is kicked off) and the Robot tools (which need the app actually up). Polls `~/.idealyst/apps/` discovery. Pass `name` to wait for a specific app (matches the registration name) or omit to wait for ANY app; `timeout_secs` defaults to 60 (max 600). On success returns the matched app(s) in `list_apps` shape (with the `selector` to pass as `app`). On timeout returns an error — if you launched via `run_dev`, check `read_dev_log` for a build still in progress or a compile failure. Errors immediately if robot control is disabled (`--no-robot`).")]
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
            let matched: Vec<_> = live
                .iter()
                .filter(|a| args.name.as_deref().map(|n| a.name == n).unwrap_or(true))
                .cloned()
                .collect();
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
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "timed out after {}s waiting for {target} to register. If you \
                     launched it with run_dev, the build may still be running or \
                     failing — check read_dev_log.",
                    timeout.as_secs()
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
            },
            normalize: !args.raw.unwrap_or(false),
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

    #[tool(description = "List framework primitives — the leaf nodes of `ui!` (view, text, button, scroll_view, …). Lightweight { name, pascal_name, category, summary }; pass `filter` (case-insensitive, glob `*`, matches name/pascal_name/category) to narrow, then `describe_primitive` for every prop + backend support + full docs.")]
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
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json).unwrap(),
        )]))
    }

    #[tool(description = "Get the full record for one framework primitive: docs, every prop (name/type/doc/constraint), backend support, category. Accepts snake_case (`scroll_view`) or PascalCase (`ScrollView`).")]
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
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json).unwrap(),
        )]))
    }

    #[tool(description = "List framework utility functions — free helpers authors call outside `ui!` (`platform()`, `parse_color()`, `now_micros()`). Lightweight { name, fqn, category, return_type, summary }; pass `filter` (case-insensitive, glob `*`, matches name/fqn/category) to narrow, then `describe_utility` for params + full docs.")]
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
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json).unwrap(),
        )]))
    }

    #[tool(description = "Get the full record for one framework utility: docs, params, return type. Inlines the return type's `TypeEntry` (variants for enums, fields for structs) when known.")]
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
        if let Some(ty) = return_type_inline {
            obj.insert("return_type_entry".into(), ty);
        }
        // Cross-kind recipes (phase 3): usage examples that target or use
        // this utility, surfaced the same way `describe_component` does.
        obj.insert(
            "recipes".into(),
            serde_json::json!(recipes_json_for(&cat, entry.name)),
        );
        if let Some(s) = cat.scope_for(entry.module_path) {
            obj.insert("scope".into(), s.slug.into());
        }
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&serde_json::Value::Object(obj)).unwrap(),
        )]))
    }

    #[tool(description = "List the framework authoring macros — the verbs of writing an app (`signal!`, `effect!`, `ui!`, `#[component]`, `stylesheet!`, `animated!`). Lightweight { name, invocation, kind, summary }; pass `filter` (case-insensitive, glob `*`, matches name/invocation/kind) to narrow, then `describe_macro` for full docs + what it expands to.")]
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
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json).unwrap(),
        )]))
    }

    #[tool(description = "Get the full record for one authoring macro: what it does, the crate it's exported from, the canonical invocation syntax, and a one-line sketch of what it expands to (so you see the primitive underneath — e.g. `effect!` → `Effect::scoped(move || …)`). Accepts the bare name or a trailing `!` (`effect` and `effect!` both resolve).")]
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
        });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json).unwrap(),
        )]))
    }

    #[tool(description = "List the four framework interaction states (`hovered`, `pressed`, `focused`, `disabled`) — the valid names for `state foo(theme) { … }` arms in `stylesheet!`. Returns { name, docs, backends }.")]
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
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json).unwrap(),
        )]))
    }

    #[tool(description = "List bundled framework usage guides — markdown documents covering getting started, concepts, reactivity, styling, navigation, backends. Returns { slug, title, order, tags } sorted by order.")]
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
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json).unwrap(),
        )]))
    }

    #[tool(description = "Read the full markdown body of one framework guide by slug. Cross-references in the body use the `[[name]]` convention — resolve them via `describe_component`/`describe_primitive`/`describe_utility`/`describe_type`.")]
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
        let json = serde_json::json!({
            "slug": entry.slug,
            "title": entry.title,
            "order": entry.order,
            "tags": entry.tags,
            "body": entry.body,
        });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json).unwrap(),
        )]))
    }

    #[tool(description = "List every imperative method declared via `methods! { fn foo(&self, …) { … } }` blocks. Returns one entry per method, tagged with its parent component. Filter by passing the parent component's name.")]
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
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json).unwrap(),
        )]))
    }

    #[tool(description = "List every `AnimatedValue` declared inside a `#[component]` body (`let x = animated!(…)`). Filter by parent component name. Each entry is { parent_fqn, binding, initial, line }.")]
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
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json).unwrap(),
        )]))
    }

    #[tool(description = "List types registered via `#[derive(IdealystSchema)]` (structs and enums). Returns { short_name, fqn, kind }; pass `filter` (case-insensitive, glob `*`, matches name/fqn/kind) to narrow, then `describe_type` for fields/variants.")]
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
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json).unwrap(),
        )]))
    }

    #[tool(description = "Describe one type — struct fields (name, type, doc, constraint) or enum variants (name, docs, payload). Accepts a short-name or fully-qualified name.")]
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
                let hint = if req.name.ends_with("Props") {
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
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&type_entry_json(entry)).unwrap(),
        )]))
    }

    #[tool(description = "List recipes — compile-checked usage examples for any catalog entity (component, utility, function, or type). Lightweight { name, target, fqn, summary }; pass `filter` (case-insensitive, glob `*`, matches name/target/fqn) to narrow, then `describe_recipe` for the full example source. Recipes are how you learn the canonical, type-verified way to use something.")]
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
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json).unwrap(),
        )]))
    }

    #[tool(description = "Get one recipe in full: its target entity, docs, the compile-verified source code, and the entities it uses. Accepts the recipe's short-name or fully-qualified name. The `source` is a working, copy-pasteable example proven to type-check against the current API.")]
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
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json).unwrap(),
        )]))
    }

    #[tool(description = "List documentation scopes — flat labels (declared with `doc_scope!`) that group components/utilities by feature area. Lightweight { slug, title, order, summary }; pass `filter` (case-insensitive, glob `*`, matches slug/title) to narrow, then `describe_scope` for a scope's docs + the entities it contains.")]
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
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json).unwrap(),
        )]))
    }

    #[tool(description = "Describe one documentation scope by its slug: title, docs, and the entities (components, utilities) assigned to it by module proximity. A component declared inside a nearer scope belongs to that nearer scope, not this one — so this lists what lives *directly* in the scope. The way to see what a feature area contains.")]
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
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json).unwrap(),
        )]))
    }

    #[tool(description = "List the opt-in SDK crates — peripheral capabilities that ship OUTSIDE runtime-core (networking, persistence, camera, the component library, …) and are invisible to list_components/list_primitives/list_utilities because they expose plain functions/types or `Element::External` primitives. THIS is how you discover which crate makes a network request (`net`), persists data (`storage`/`credentials`), or renders a map (`maps`). Lightweight { name, category, kind, dep_line, summary }; pass `filter` (case-insensitive, glob `*`, matches name/category/kind) to narrow, then `describe_sdk` for the full record. Prose home: the `sdks` guide (read_guide).")]
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
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json).unwrap(),
        )]))
    }

    #[tool(description = "Get the full record for one opt-in SDK crate: summary, the `Cargo.toml` dependency line to add, capability category, whether its surface is plain API or a `ui!` `Element::External` primitive, and the guide that documents it. Accepts the crate name (`net`, `storage`, `idea-ui`).")]
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
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json).unwrap(),
        )]))
    }

    #[tool(description = "List the icon packs available — crates like `icons-lucide` that expose named icon `const`s for the `icon(...)` primitive. Lightweight { name, title, icon_count, import_path, license, homepage }; the per-icon names are deliberately NOT included (a pack has ~1600). To find an icon, use `search_icons`; to browse one pack, page through `describe_icon_set`. Pass `filter` (case-insensitive, glob `*`, matches name/title) to narrow.")]
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
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json).unwrap(),
        )]))
    }

    #[tool(description = "Page through the icons in one pack. Returns { name, title, docs, import_path, license, icon_count, offset, limit, icons: [{ name, ident, import }] } where `import` is the paste-ready `use` path (`icons_lucide::ARROW_RIGHT`). Icons are name-sorted; page with `offset`/`limit` (limit default 100, max 500). To find a specific icon by keyword instead of paging, prefer `search_icons`.")]
    async fn describe_icon_set(
        &self,
        Parameters(req): Parameters<DescribeIconSetRequest>,
    ) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let set = cat
            .icon_sets()
            .iter()
            .find(|s| s.name == req.name)
            .ok_or_else(|| {
                McpError::invalid_params(
                    format!(
                        "icon set {:?} not found — use list_icon_sets to see what's available",
                        req.name
                    ),
                    None,
                )
            })?;
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
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json).unwrap(),
        )]))
    }

    #[tool(description = "Search icons across every pack by name keyword (case-insensitive substring) — THE way to find an icon without dumping ~1600 names. Returns up to `limit` (default 50, max 200) matches as { set, name, ident, import }, where `import` is the paste-ready `use icons_lucide::ARROW_RIGHT;` path you drop into the `icon(...)` primitive. Pass `set` to restrict to one pack.")]
    async fn search_icons(
        &self,
        Parameters(req): Parameters<IconSearchRequest>,
    ) -> Result<CallToolResult, McpError> {
        let q = req.query.trim().to_lowercase();
        if q.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text("[]".to_string())]));
        }
        let limit = req.limit.unwrap_or(50).min(200);
        let cat = self.catalog.read().await;
        let mut out: Vec<serde_json::Value> = Vec::new();
        'sets: for set in cat.icon_sets() {
            if let Some(want) = &req.set {
                if set.name != *want {
                    continue;
                }
            }
            for i in set.icons {
                if i.name.to_lowercase().contains(&q) {
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
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&out).unwrap(),
        )]))
    }

    #[tool(description = "Fulltext search over EVERY catalog slice — components, primitives, utilities, macros, guides, types, methods, recipes, scopes, tools, states, animations, icon packs. Multi-word queries are tokenized on whitespace and OR'd: an entry matches if ANY term appears in its name or docs, and results are ranked by how many distinct query terms each entry hit (best first). So `search('http fetch network request')` surfaces the networking guide/utility even though no single field contains the whole phrase. Returns a JSON array of { kind, name, fqn, score, matched_terms, docs_excerpt } tagged with the slice the match came from. (For individual ICONS, use `search_icons` — this searches pack-level metadata only.)")]
    async fn search(
        &self,
        Parameters(req): Parameters<SearchRequest>,
    ) -> Result<CallToolResult, McpError> {
        // Tokenize on whitespace and lowercase each term. A whole-phrase
        // query with no spaces degrades to a single-term search (the old
        // behavior), so single-word queries are unaffected. Multi-word
        // queries OR across terms and rank by distinct-term hit count —
        // the gap the old contiguous-substring `contains(&full_query)`
        // left, which returned `[]` for any multi-word query.
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
            consider(
                "primitive",
                p.name.to_string(),
                p.name.to_string(),
                &[p.name, p.pascal_name],
                p.docs,
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

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json).unwrap(),
        )]))
    }

    #[tool(description = "Run the idealyst source linter over a project and return its findings as JSON. Same engine and rules as the `idealyst lint` CLI: it flags idiom drift in un-expanded Rust source — raw reactive primitives that should be macros (`Signal::new`→`signal!`, `Effect::new`→`effect!`, `memo(…)`→`memo!`), hand-built elements instead of `ui!`/`jsx!` (`builder::…`, `BuildElement::build`, `Element::Variant {…}`), and non-PascalCase `#[component]` functions. Respects `idealyst-lint.toml` (rule levels) and inline `// idealyst-lint-disable` directives discovered from the target path. Pass `path` to lint a specific file/dir; omit it to lint the single live app's project root, else the server's working directory. Returns { path, files_scanned, error_count, warn_count, total_diagnostics, failed, truncated, parse_errors, diagnostics[] } where each diagnostic is { rule, severity, message, help, file, line, column, end_line, end_column, source_line }.")]
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

/// Locate an entry by exact short-name match first, then by FQN.
/// Short-name ambiguity (two entries with the same `name`) returns
/// the first hit — consumers wanting strict disambiguation should
/// pass the FQN. The MCP `--check` lint (phase 6) is what surfaces
/// short-name conflicts at the project level.
fn find_by_name<'a>(
    cat: &'a ResolvedCatalog,
    needle: &str,
) -> Option<&'a ComponentEntry> {
    for e in cat.entries() {
        if e.name == needle {
            return Some(*e);
        }
    }
    for e in cat.entries() {
        let fqn = format!("{}::{}", e.module_path, e.name);
        if fqn == needle {
            return Some(*e);
        }
    }
    None
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
                match prop_fields_for(cat, p.type_short_name) {
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
/// `ButtonProps`).
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
) -> Option<Vec<serde_json::Value>> {
    // Wire path: the props struct as a documented `TypeEntry`.
    for t in cat.types() {
        if t.short_name != short_name {
            continue;
        }
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
    let schema = mcp_catalog::lookup_schema(short_name)?;
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
    let start = pos.saturating_sub(80);
    let end = (pos + needle.len() + 80).min(docs.len());
    docs[start..end].to_string()
}

#[tool_handler]
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
            let robot_status = if self.robot.is_some() {
                " Robot tools enabled (find_element, click, type_text, get_snapshot, get_logs, etc.) — they proxy to the running app's bridge."
            } else {
                " Robot tools present but disabled — start the server with a bridge address to enable them."
            };
            format!(
                "Idealyst framework MCP catalog (schema v2). \
                 Component tools: list_components, describe_component, find_uses, \
                 find_dependencies, list_methods, list_animations. \
                 Framework tools: list_primitives, describe_primitive, \
                 list_utilities, describe_utility, list_states. \
                 Types: list_types, describe_type. \
                 Tools (#[idealyst_tool]): list_tools, describe_tool. \
                 SDK crates (net, storage, credentials, server, …): list_sdks, describe_sdk. \
                 Guides: list_guides, read_guide (bundled framework docs). \
                 Cross-slice: search (multi-word tokenized + ranked across every slice). \
                 Resource: idealyst://catalog returns the full denormalized \
                 catalog JSON (every slice).{}",
                robot_status
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

#[cfg(test)]
mod tests {
    //! Tools-level regression: a `#[component]` linked into this
    //! test binary must appear in the `list_components` tool's
    //! response.
    //!
    //! Pre-fix the sidecar wrapper omitted `runtime-core/dev`, so
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
        ::runtime_core::view(::std::vec::Vec::new())
    }

    /// Parse a `list_*` tool result into `(name, has_summary, keys)`
    /// triples so tests can assert on shape + contents.
    fn component_names(result: &CallToolResult) -> Vec<String> {
        let payload = result
            .content
            .iter()
            .find_map(|c| c.as_text().map(|t| t.text.clone()))
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
        // none of the file/line/module_path noise the old shape had.
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
            }))
            .await
            .unwrap();
        assert!(component_names(&hit)
            .contains(&"list_components_regression_canary".to_string()));

        // …a non-matching filter excludes it (and yields fewer rows).
        let miss = svc
            .list_components(Parameters(FilterRequest {
                filter: Some("definitely-no-such-component-xyz".into()),
            }))
            .await
            .unwrap();
        assert!(!component_names(&miss)
            .contains(&"list_components_regression_canary".to_string()));
    }

    /// Parse a tool's JSON-array result into a `serde_json::Value`.
    fn parse_array(result: &CallToolResult) -> serde_json::Value {
        let payload = result
            .content
            .iter()
            .find_map(|c| c.as_text().map(|t| t.text.clone()))
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

    /// A2 (coverage half): the search index must span more than guide
    /// prose — a query that only matches a macro name surfaces a `macro`
    /// kind, proving non-guide slices are indexed.
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
        assert!(names.contains(&"signal"), "list_macros must include `signal`; got {names:?}");

        // describe_macro resolves bare name AND a trailing `!`.
        for needle in ["ui", "signal!"] {
            let d = svc
                .describe_macro(Parameters(NameRequest { name: needle.into(), app: None }))
                .await
                .unwrap_or_else(|e| panic!("describe_macro({needle:?}) errored: {e:?}"));
            let dv = parse_array(&d);
            assert!(dv["name"].is_string(), "describe_macro({needle:?}) returns a record; {dv}");
        }
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
            .list_sdks(Parameters(FilterRequest { filter: Some("data".into()) }))
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
        // Point HOME at the temp dir so the default screenshots dir lands there.
        // Safe in this single-threaded unit test.
        let prev_home = std::env::var_os("HOME");
        unsafe { std::env::set_var("HOME", &tmp); }

        let mut v = json!({
            "png_base64": "aGVsbG8=", // "hello"
            "width": 10,
            "height": 20,
        });
        ensure_screenshot_saved("myapp", &mut v);

        let path = v["path"].as_str().expect("path was injected");
        assert!(path.contains("myapp-"));
        let bytes = std::fs::read(path).expect("file written");
        assert_eq!(bytes, b"hello");

        // Restore HOME and clean up.
        match prev_home {
            Some(h) => unsafe { std::env::set_var("HOME", h) },
            None => unsafe { std::env::remove_var("HOME") },
        }
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
            rules.contains(&"prefer-signal-macro".to_string()),
            "expected prefer-signal-macro; got {rules:?}"
        );
        assert_eq!(v["files_scanned"], 1);
        assert!(v["error_count"].as_u64().unwrap() >= 1, "got {v}");
        assert_eq!(v["failed"], true, "an error-level diagnostic must fail the run");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
