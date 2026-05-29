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

/// The MCP catalog server. Holds the resolved catalog behind an
/// `Arc<RwLock<...>>` so the watcher thread (phase 5) can swap in
/// a fresh `ResolvedCatalog` without taking the server down.
#[derive(Clone)]
pub struct CatalogService {
    catalog: Arc<RwLock<ResolvedCatalog>>,
    /// Legacy single-bridge mode — used when the server was
    /// constructed with `with_robot_bridge(...)`. The registry-aware
    /// mode (default) ignores this and looks up bridges per-call.
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
    mdns: crate::mdns_discovery::DiscoveryTable,
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
        mdns: &crate::mdns_discovery::DiscoveryTable,
    ) -> Result<Arc<RobotBridge>, McpError> {
        let live = mdns.snapshot();
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

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SlugRequest {
    /// Guide slug (e.g. `getting-started`).
    pub slug: String,
}

#[tool_router]
impl CatalogService {
    pub fn new() -> Self {
        Self {
            catalog: Arc::new(RwLock::new(ResolvedCatalog::build())),
            robot: None,
            resolver: Arc::new(RobotResolver::default()),
            catalog_cache: Arc::new(CatalogCache::default()),
            mdns: crate::mdns_discovery::start(),
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

    #[tool(description = "List every component across live apps. Each app's catalog is fetched over its Robot bridge's `get_catalog` command. If no apps are live, falls back to the in-process catalog (the catalog the MCP server itself was built with — useful when running offline with `--project-root`). Returns a JSON array of { app, name, module_path, fqn, file, line } sorted by (app, fqn).")]
    async fn list_components(&self) -> Result<CallToolResult, McpError> {
        let mut json: Vec<serde_json::Value> = Vec::new();

        let live = self.mdns.snapshot();

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
                json.push(serde_json::json!({
                    "app": app.name,
                    "name": e.name,
                    "module_path": e.module_path,
                    "fqn": format!("{}::{}::{}", app.name, e.module_path, e.name),
                    "file": e.file,
                    "line": e.line,
                }));
            }
        }

        if json.is_empty() {
            // No live apps — serve the in-process catalog (set up at
            // server startup by `--project-root` extractor spawn, or
            // empty if the server was started with no project).
            let cat = self.catalog.read().await;
            let mut sorted: Vec<&ComponentEntry> = cat.entries().to_vec();
            sorted.sort_by_key(|e| (e.module_path, e.name));
            for e in sorted {
                json.push(serde_json::json!({
                    "app": null,
                    "name": e.name,
                    "module_path": e.module_path,
                    "fqn": format!("{}::{}", e.module_path, e.name),
                    "file": e.file,
                    "line": e.line,
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
        let json = entry_to_json(entry, edges);
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

    #[tool(description = "List every `#[idealyst_tool]`-registered function. Returns a JSON array of { name, module_path, fqn, file, line, return_type }.")]
    async fn list_tools(&self) -> Result<CallToolResult, McpError> {
        let mut sorted: Vec<&mcp_catalog::ToolEntry> = mcp_catalog::tools().collect();
        sorted.sort_by_key(|t| (t.module_path, t.name));
        let json: Vec<serde_json::Value> = sorted
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "module_path": t.module_path,
                    "fqn": format!("{}::{}", t.module_path, t.name),
                    "file": t.file,
                    "line": t.line,
                    "return_type": t.return_type,
                })
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

    #[tool(description = "List every running idealyst app — discovered via per-process registration files at `~/.idealyst/apps/<name>-<pid>.json` that the running app's Robot bridge writes on bind. Each entry includes name, bundle_id, project_root, bridge_addr, and pid. Entries are removed automatically when the app exits (RAII cleanup on graceful shutdown; stale ones get pruned at scan time when `kill(pid, 0)` reports the process is gone).")]
    async fn list_apps(&self) -> Result<CallToolResult, McpError> {
        let live = self.mdns.snapshot();
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
        let bridge: Arc<RobotBridge> = if let Some(b) = &self.robot {
            // Legacy pinned-bridge mode: ignore `app`. Kept for
            // back-compat with `with_robot_bridge(...)` callers.
            b.clone()
        } else {
            match self.resolver.resolve(app, &self.mdns).await {
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

    #[tool(description = "List every framework primitive — the leaf nodes of `ui!` (View, Text, Button, ScrollView, …). Returns a JSON array of { name, pascal_name, category, backends, docs } sorted by name.")]
    async fn list_primitives(&self) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let json: Vec<serde_json::Value> = cat
            .primitives()
            .iter()
            .map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "pascal_name": p.pascal_name,
                    "category": p.category.as_str(),
                    "backends": p.backends,
                    "docs": p.docs,
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

    #[tool(description = "List every framework utility function — free helpers authors call from their own code (not inside `ui!`). Examples: `platform()`, `parse_color()`, `now_micros()`. Returns { name, fqn, category, return_type, docs } sorted by name.")]
    async fn list_utilities(&self) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let json: Vec<serde_json::Value> = cat
            .utilities()
            .iter()
            .map(|u| {
                serde_json::json!({
                    "name": u.name,
                    "module_path": u.module_path,
                    "fqn": format!("{}::{}", u.module_path, u.name),
                    "category": u.category.as_str(),
                    "return_type": u.return_type,
                    "docs": u.docs,
                })
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
                    "docs": m.docs,
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

    #[tool(description = "List every type registered via `#[derive(IdealystSchema)]` (structs and enums). Returns { short_name, fqn, kind } sorted by FQN.")]
    async fn list_types(&self) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let json: Vec<serde_json::Value> = cat
            .types()
            .iter()
            .map(|t| {
                let kind = match &t.shape {
                    mcp_catalog::TypeShape::Struct { .. } => "struct",
                    mcp_catalog::TypeShape::Enum { .. } => "enum",
                };
                serde_json::json!({
                    "short_name": t.short_name,
                    "module_path": t.module_path,
                    "fqn": format!("{}::{}", t.module_path, t.short_name),
                    "kind": kind,
                })
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
                McpError::invalid_params(format!("type {:?} not found", req.name), None)
            })?;
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&type_entry_json(entry)).unwrap(),
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

fn entry_to_json(entry: &ComponentEntry, edges: &[mcp_catalog::ResolvedEdge]) -> serde_json::Value {
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
            // Inline `IdealystSchema`-derived fields when the
            // parameter's type matches a registered props struct.
            if !p.type_short_name.is_empty() {
                if let Some(schema) = mcp_catalog::lookup_schema(p.type_short_name) {
                    let fields: Vec<serde_json::Value> = schema
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
                        .collect();
                    obj.insert("schema".into(), serde_json::json!(fields));
                }
            }
            serde_json::Value::Object(obj)
        })
        .collect();
    serde_json::json!({
        "name": entry.name,
        "module_path": entry.module_path,
        "fqn": format!("{}::{}", entry.module_path, entry.name),
        "file": entry.file,
        "line": entry.line,
        "docs": entry.docs,
        "params": params,
        "composes": composes,
    })
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
                        entry_to_json(e, edges)
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

    #[tokio::test]
    async fn list_components_returns_in_process_catalog_components() {
        let svc = CatalogService::new();

        let result = svc
            .list_components()
            .await
            .expect("list_components tool call succeeds");

        // `CallToolResult::success(vec![Content::text(json)])` —
        // the JSON-pretty-printed component array is the first
        // content block's text. Pull it out, parse, and verify
        // our canary's there.
        let payload = result
            .content
            .iter()
            .find_map(|c| c.as_text().map(|t| t.text.clone()))
            .expect("tool result has a text content block");

        let entries: serde_json::Value =
            serde_json::from_str(&payload).expect("response is valid JSON");
        let arr = entries.as_array().expect("response is a JSON array");

        let names: Vec<&str> = arr
            .iter()
            .filter_map(|e| e["name"].as_str())
            .collect();
        assert!(
            names.contains(&"list_components_regression_canary"),
            "expected canary component in list_components output; got {:?}",
            names,
        );
    }
}
