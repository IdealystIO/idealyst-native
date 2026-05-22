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
use rmcp::{
    ErrorData as McpError,
    RoleServer, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars,
    service::RequestContext,
    tool, tool_handler, tool_router,
};

/// The MCP catalog server. Holds the resolved catalog behind an
/// `Arc<RwLock<...>>` so the watcher thread (phase 5) can swap in
/// a fresh `ResolvedCatalog` without taking the server down.
#[derive(Clone)]
pub struct CatalogService {
    catalog: Arc<RwLock<ResolvedCatalog>>,
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
            tool_router: Self::tool_router(),
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
        .map(|p| serde_json::json!({ "name": p.name, "type": p.type_str }))
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
        .with_instructions(
            "Framework MCP catalog. Tools: list_components, describe_component, \
             find_uses, find_dependencies, search. Resource: idealyst://catalog \
             returns the full denormalized catalog JSON."
                .to_string(),
        )
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
