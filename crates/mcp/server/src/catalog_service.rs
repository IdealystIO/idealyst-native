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
        let entry = match (app, live.len()) {
            (Some(name), _) => live
                .iter()
                .find(|a| a.name == name)
                .cloned()
                .ok_or_else(|| {
                    let known: Vec<&str> = live.iter().map(|a| a.name.as_str()).collect();
                    McpError::invalid_params(
                        format!(
                            "app {:?} not running. Live apps: {:?}. \
                             Run `idealyst dev` in the target project.",
                            name, known
                        ),
                        None,
                    )
                })?,
            (None, 1) => live.into_iter().next().unwrap(),
            (None, 0) => {
                return Err(McpError::invalid_params(
                    "no live apps discovered — run `idealyst dev` \
                     in a project first"
                        .to_string(),
                    None,
                ));
            }
            (None, _) => {
                let known: Vec<&str> = live.iter().map(|a| a.name.as_str()).collect();
                return Err(McpError::invalid_params(
                    format!(
                        "{} apps live; specify `app` (one of: {:?})",
                        live.len(),
                        known
                    ),
                    None,
                ));
            }
        };
        let mut bridges = self.bridges.lock().await;
        let bridge = bridges
            .entry(entry.name.clone())
            .or_insert_with(|| Arc::new(RobotBridge::new(entry.bridge_addr.clone())))
            .clone();
        Ok(bridge)
    }
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

    #[tool(description = "Invoke a `#[method]`-tagged method on an element's instance (Robot's component-level escape hatch).")]
    async fn invoke_method(
        &self,
        Parameters(args): Parameters<RobotInvokeMethod>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("invoke_method", app.as_deref(), body).await
    }

    #[tool(description = "Capture a PNG screenshot of the running app and return it as base64. Two capture sources, selected via the optional `source` arg: `client` snapshots the REAL rendered native surface (macOS/iOS/Android — native widgets, fonts, the live view hierarchy), working both for a `--local` native app and for a runtime-server session by asking the connected client to capture over the wire; `replay` rasterizes the current scene with the wgpu renderer server-side (always available, even with no client attached, but uses the framework's renderer not the platform's). `auto` (the default) tries the real client and falls back to replay. Optional `width`/`height` in physical pixels are honored by the replay path (default: the session's viewport size); real-client capture always returns the device's own pixel dimensions. Response JSON: { png_base64, width, height }. Requires the session to have registered the screenshot verb; returns an error otherwise.")]
    async fn screenshot(
        &self,
        Parameters(args): Parameters<RobotScreenshotArgs>,
    ) -> Result<CallToolResult, McpError> {
        let app = args.app.clone();
        let body = strip_app(serde_json::to_value(args).unwrap_or_default());
        self.robot_call("screenshot", app.as_deref(), body).await
    }

    #[tool(description = "List every running idealyst app — discovered via per-process registration files at `~/.idealyst/apps/<name>-<pid>.json` that the running app's Robot bridge writes on bind. Each entry includes name, bundle_id, project_root, bridge_addr, and pid. Entries are removed automatically when the app exits (RAII cleanup on graceful shutdown; stale ones get pruned at scan time when `kill(pid, 0)` reports the process is gone).")]
    async fn list_apps(&self) -> Result<CallToolResult, McpError> {
        let live = self.live_apps();
        let json: Vec<serde_json::Value> = live
            .iter()
            .map(|a| {
                serde_json::json!({
                    "name": a.name,
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
        if matches!(self.robot_mode, RobotMode::Disabled) {
            return Err(McpError::invalid_params(
                "Robot control is disabled — the MCP server was started with \
                 --no-robot. Restart it without --no-robot (or pass --robot-port \
                 to target a known bridge) to drive a running app."
                    .to_string(),
                None,
            ));
        }
        let bridge: Arc<RobotBridge> = if let Some(b) = &self.robot {
            // Pinned-bridge mode: `with_robot_bridge(...)` or
            // `RobotMode::Explicit`. Ignore `app` — there's exactly one.
            b.clone()
        } else {
            match self.resolver.resolve(app, &self.discovery).await {
                Ok(b) => b,
                Err(e) => return Err(e),
            }
        };
        match bridge.call(cmd, args).await {
            Ok(value) => Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()),
            )])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
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

    #[tool(description = "Fulltext search over every catalog slice — components, primitives, utilities, guides, types, methods. Returns matches as a JSON array of { kind, name, fqn, docs_excerpt } tagged with the slice the match came from.")]
    async fn search(
        &self,
        Parameters(req): Parameters<SearchRequest>,
    ) -> Result<CallToolResult, McpError> {
        let needle = req.query.to_lowercase();
        let cat = self.catalog.read().await;
        let mut hits: Vec<serde_json::Value> = Vec::new();

        for e in cat.entries() {
            if e.name.to_lowercase().contains(&needle)
                || e.docs.to_lowercase().contains(&needle)
            {
                hits.push(serde_json::json!({
                    "kind": "component",
                    "name": e.name,
                    "fqn": format!("{}::{}", e.module_path, e.name),
                    "docs_excerpt": docs_excerpt_around(e.docs, &needle),
                }));
            }
        }
        for p in cat.primitives() {
            if p.name.to_lowercase().contains(&needle)
                || p.pascal_name.to_lowercase().contains(&needle)
                || p.docs.to_lowercase().contains(&needle)
            {
                hits.push(serde_json::json!({
                    "kind": "primitive",
                    "name": p.name,
                    "fqn": p.name,
                    "docs_excerpt": docs_excerpt_around(p.docs, &needle),
                }));
            }
        }
        for u in cat.utilities() {
            if u.name.to_lowercase().contains(&needle)
                || u.docs.to_lowercase().contains(&needle)
            {
                hits.push(serde_json::json!({
                    "kind": "utility",
                    "name": u.name,
                    "fqn": format!("{}::{}", u.module_path, u.name),
                    "docs_excerpt": docs_excerpt_around(u.docs, &needle),
                }));
            }
        }
        for g in cat.guides() {
            if g.slug.to_lowercase().contains(&needle)
                || g.title.to_lowercase().contains(&needle)
                || g.body.to_lowercase().contains(&needle)
            {
                hits.push(serde_json::json!({
                    "kind": "guide",
                    "name": g.slug,
                    "fqn": g.slug,
                    "docs_excerpt": docs_excerpt_around(g.body, &needle),
                }));
            }
        }
        for t in cat.types() {
            if t.short_name.to_lowercase().contains(&needle)
                || t.docs.to_lowercase().contains(&needle)
            {
                hits.push(serde_json::json!({
                    "kind": "type",
                    "name": t.short_name,
                    "fqn": format!("{}::{}", t.module_path, t.short_name),
                    "docs_excerpt": docs_excerpt_around(t.docs, &needle),
                }));
            }
        }
        for m in cat.methods() {
            if m.name.to_lowercase().contains(&needle)
                || m.docs.to_lowercase().contains(&needle)
            {
                hits.push(serde_json::json!({
                    "kind": "method",
                    "name": m.name,
                    "fqn": format!("{}::{}.{}", m.parent_module_path, m.parent_name, m.name),
                    "docs_excerpt": docs_excerpt_around(m.docs, &needle),
                }));
            }
        }

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&hits).unwrap(),
        )]))
    }
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
                 Guides: list_guides, read_guide (bundled framework docs). \
                 Cross-slice: search. \
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
        let svc = CatalogService::new();
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
        let svc = CatalogService::new();

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
}
