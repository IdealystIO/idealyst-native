//! `CatalogService` — the rmcp `ServerHandler` implementation that
//! surfaces the framework-mcp catalog as MCP tools + a resource.
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

use std::sync::Arc;
use tokio::sync::RwLock;

use framework_mcp::{ComponentEntry, EdgeStatus, EntryRef, ResolvedCatalog};

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
    /// Optional handle to the running app's Robot bridge. When
    /// present, the Robot tools below are usable; when `None` they
    /// return "robot tools disabled — start the server with a
    /// bridge address." The bridge connection is lazy — `None` means
    /// "Robot disabled," not "not yet connected."
    robot: Option<Arc<RobotBridge>>,
    // `#[tool_handler]` reads this through the trait impl, not via
    // a direct field access — the dead-code analyzer can't see it.
    #[allow(dead_code)]
    tool_router: ToolRouter<CatalogService>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct NameRequest {
    /// Component short-name (e.g. `card`) or fully-qualified name
    /// (e.g. `mcp_demo::components::card`). Short-name lookups are
    /// resolved via spec §6 proximity rules.
    pub name: String,
}

/// Find-by criteria for `find_element` / `find_all_elements` /
/// `count_elements`. Every field is optional; an empty struct matches
/// nothing on the bridge side. `serde(skip_serializing_if = "Option::is_none")`
/// keeps the wire payload tight so the bridge sees only the criteria
/// the caller actually set.
#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotFindArgs {
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
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotTypeText {
    pub element_id: u64,
    pub text: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotSetToggle {
    pub element_id: u64,
    pub value: bool,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotSetSlider {
    pub element_id: u64,
    pub value: f64,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RobotInvokeMethod {
    pub instance_id: u64,
    pub method: String,
    /// Method args keyed by parameter name. Omit / pass `{}` for
    /// no-arg methods.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<serde_json::Value>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SearchRequest {
    /// Free-form query. Matched case-insensitively against component
    /// names + doc comments.
    pub query: String,
}

#[tool_router]
impl CatalogService {
    pub fn new() -> Self {
        Self {
            catalog: Arc::new(RwLock::new(ResolvedCatalog::build())),
            robot: None,
            tool_router: Self::tool_router(),
        }
    }

    /// Enable the Robot tools by attaching a bridge handle. The
    /// bridge is contacted lazily — passing an address that's not
    /// yet listening is fine; the first Robot tool call connects
    /// (or returns a "is the app running?" error and the catalog
    /// tools keep working).
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

    #[tool(description = "List every component in the catalog. Returns a JSON array of { name, module_path, fqn, file, line } sorted by FQN.")]
    async fn list_components(&self) -> Result<CallToolResult, McpError> {
        let cat = self.catalog.read().await;
        let mut sorted: Vec<&ComponentEntry> = cat.entries().to_vec();
        sorted.sort_by_key(|e| (e.module_path, e.name));
        let json: Vec<serde_json::Value> = sorted
            .iter()
            .map(|e| {
                serde_json::json!({
                    "name": e.name,
                    "module_path": e.module_path,
                    "fqn": format!("{}::{}", e.module_path, e.name),
                    "file": e.file,
                    "line": e.line,
                })
            })
            .collect();
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
        let mut sorted: Vec<&framework_mcp::ToolEntry> = framework_mcp::tools().collect();
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
        let entry = framework_mcp::tools()
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

    #[tool(description = "Find a UI element in the running app by test_id, label, label_contains, or kind. Returns the first match. Requires the app to be running with `--features robot`.")]
    async fn find_element(
        &self,
        Parameters(args): Parameters<RobotFindArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.robot_call("find_element", serde_json::to_value(args).unwrap_or_default()).await
    }

    #[tool(description = "Find all UI elements in the running app matching the given criteria.")]
    async fn find_all_elements(
        &self,
        Parameters(args): Parameters<RobotFindArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.robot_call("find_all_elements", serde_json::to_value(args).unwrap_or_default()).await
    }

    #[tool(description = "Click/press a Button or Pressable element in the running app. `element_id` comes from a prior find_element call.")]
    async fn click(
        &self,
        Parameters(args): Parameters<RobotElementId>,
    ) -> Result<CallToolResult, McpError> {
        self.robot_call("click", serde_json::to_value(args).unwrap_or_default()).await
    }

    #[tool(description = "Type text into a TextInput (replaces current value).")]
    async fn type_text(
        &self,
        Parameters(args): Parameters<RobotTypeText>,
    ) -> Result<CallToolResult, McpError> {
        self.robot_call("type_text", serde_json::to_value(args).unwrap_or_default()).await
    }

    #[tool(description = "Set a Toggle's value (true/false).")]
    async fn set_toggle(
        &self,
        Parameters(args): Parameters<RobotSetToggle>,
    ) -> Result<CallToolResult, McpError> {
        self.robot_call("set_toggle", serde_json::to_value(args).unwrap_or_default()).await
    }

    #[tool(description = "Set a Slider's value.")]
    async fn set_slider(
        &self,
        Parameters(args): Parameters<RobotSetSlider>,
    ) -> Result<CallToolResult, McpError> {
        self.robot_call("set_slider", serde_json::to_value(args).unwrap_or_default()).await
    }

    #[tool(description = "Focus an element (TextInput typically).")]
    async fn focus(
        &self,
        Parameters(args): Parameters<RobotElementId>,
    ) -> Result<CallToolResult, McpError> {
        self.robot_call("focus", serde_json::to_value(args).unwrap_or_default()).await
    }

    #[tool(description = "Blur an element.")]
    async fn blur(
        &self,
        Parameters(args): Parameters<RobotElementId>,
    ) -> Result<CallToolResult, McpError> {
        self.robot_call("blur", serde_json::to_value(args).unwrap_or_default()).await
    }

    #[tool(description = "Get a snapshot of the running app's whole element tree.")]
    async fn get_snapshot(&self) -> Result<CallToolResult, McpError> {
        self.robot_call("get_snapshot", json!({})).await
    }

    #[tool(description = "Get child elements of a node.")]
    async fn get_children(
        &self,
        Parameters(args): Parameters<RobotElementId>,
    ) -> Result<CallToolResult, McpError> {
        self.robot_call("get_children", serde_json::to_value(args).unwrap_or_default()).await
    }

    #[tool(description = "Get an element's parent in the running app.")]
    async fn get_parent(
        &self,
        Parameters(args): Parameters<RobotElementId>,
    ) -> Result<CallToolResult, McpError> {
        self.robot_call("get_parent", serde_json::to_value(args).unwrap_or_default()).await
    }

    #[tool(description = "Count elements matching the criteria.")]
    async fn count_elements(
        &self,
        Parameters(args): Parameters<RobotFindArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.robot_call("count_elements", serde_json::to_value(args).unwrap_or_default()).await
    }

    #[tool(description = "Get the running app's captured logs.")]
    async fn get_logs(&self) -> Result<CallToolResult, McpError> {
        self.robot_call("get_logs", json!({})).await
    }

    #[tool(description = "Clear the running app's captured logs.")]
    async fn clear_logs(&self) -> Result<CallToolResult, McpError> {
        self.robot_call("clear_logs", json!({})).await
    }

    #[tool(description = "Get an element's layout frame relative to its parent.")]
    async fn get_frame(
        &self,
        Parameters(args): Parameters<RobotElementId>,
    ) -> Result<CallToolResult, McpError> {
        self.robot_call("get_frame", serde_json::to_value(args).unwrap_or_default()).await
    }

    #[tool(description = "Get an element's absolute layout frame (window coords).")]
    async fn get_absolute_frame(
        &self,
        Parameters(args): Parameters<RobotElementId>,
    ) -> Result<CallToolResult, McpError> {
        self.robot_call("get_absolute_frame", serde_json::to_value(args).unwrap_or_default()).await
    }

    #[tool(description = "Invoke a `#[method]`-tagged method on an element's instance (Robot's component-level escape hatch).")]
    async fn invoke_method(
        &self,
        Parameters(args): Parameters<RobotInvokeMethod>,
    ) -> Result<CallToolResult, McpError> {
        self.robot_call("invoke_method", serde_json::to_value(args).unwrap_or_default()).await
    }

    /// Shared helper for every Robot tool. Dispatches to the bridge
    /// (or returns the "disabled" error when no bridge was attached).
    async fn robot_call(
        &self,
        cmd: &str,
        args: serde_json::Value,
    ) -> Result<CallToolResult, McpError> {
        let Some(bridge) = &self.robot else {
            return Ok(CallToolResult::error(vec![Content::text(
                "robot tools disabled — start the server with `--bridge <addr>` or `idealyst mcp --robot`",
            )]));
        };
        match bridge.call(cmd, args).await {
            Ok(value) => Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()),
            )])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    #[tool(description = "Fulltext search over component names and doc comments. Returns matches as a JSON array of { fqn, name, docs_excerpt }.")]
    async fn search(
        &self,
        Parameters(req): Parameters<SearchRequest>,
    ) -> Result<CallToolResult, McpError> {
        let needle = req.query.to_lowercase();
        let cat = self.catalog.read().await;
        let hits: Vec<serde_json::Value> = cat
            .entries()
            .iter()
            .filter(|e| {
                e.name.to_lowercase().contains(&needle)
                    || e.docs.to_lowercase().contains(&needle)
            })
            .map(|e| {
                let excerpt = docs_excerpt_around(e.docs, &needle);
                serde_json::json!({
                    "fqn": format!("{}::{}", e.module_path, e.name),
                    "name": e.name,
                    "docs_excerpt": excerpt,
                })
            })
            .collect();
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

fn entry_to_json(entry: &ComponentEntry, edges: &[framework_mcp::ResolvedEdge]) -> serde_json::Value {
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
                if let Some(schema) = framework_mcp::lookup_schema(p.type_short_name) {
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
                "Framework MCP catalog. Catalog tools: list_components, \
                 describe_component, find_uses, find_dependencies, list_tools, \
                 describe_tool, search. Resource: idealyst://catalog returns the \
                 full denormalized catalog JSON.{}",
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
                // Serve from the in-memory catalog (which the watcher
                // may have swapped) rather than calling
                // `framework_mcp::catalog_json()` (which reads inventory
                // every call). This way both the static and live-reload
                // server paths share the same source of truth.
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
                let body = serde_json::json!({
                    "catalog_version": 1,
                    "components": components,
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
