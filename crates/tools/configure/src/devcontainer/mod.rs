//! Devcontainer configuration — the shared, non-interactive core.
//!
//! Both front-ends (the `idealyst configure devcontainer` CLI wizard and the
//! MCP `configure_devcontainer` tool) build a [`ConfigureRequest`] and call
//! [`apply`]. [`read_state`] reports what's currently configured so a re-run
//! can preselect it.
//!
//! ## Ownership
//!
//! idealyst-managed sidecar services live *only* in
//! `.devcontainer/docker-compose.idealyst.yml` (owned by us, regenerated
//! wholesale). The user's `docker-compose.yml` is never parsed or rewritten.
//! `devcontainer.json` is touched at most once, to list the managed file in
//! its `dockerComposeFile` array. This is what guarantees we never disturb a
//! user's own `minio`/`database` service added under a different key — it
//! simply isn't in our file.

pub mod compose;
pub mod service;
pub mod services;
pub mod wiring;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub use service::{registry, DevService, ServiceVariant};

/// One idealyst-managed service that is currently enabled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnabledService {
    /// Canonical service id (`"database"`, `"redis"`, `"minio"`).
    pub id: String,
    /// Chosen variant id, if the service has variants.
    pub variant: Option<String>,
}

/// What to do with a service in a [`ConfigureRequest`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Turn the service on. If already configured, it's a safe no-op (the
    /// existing variant is kept) and a warning is reported.
    Enable,
    /// Reset the service to the given/default variant (a "blank" config),
    /// adding it if it wasn't present.
    Reconfigure,
    /// Remove the service.
    Remove,
}

/// A single service instruction.
#[derive(Clone, Debug)]
pub struct ServiceRequest {
    pub id: String,
    /// Variant to use for `Enable`/`Reconfigure`. `None` = the service's
    /// default variant.
    pub variant: Option<String>,
    pub action: Action,
}

/// A batch of service instructions to apply to a project's devcontainer.
#[derive(Clone, Debug, Default)]
pub struct ConfigureRequest {
    pub services: Vec<ServiceRequest>,
}

/// Snapshot of the current devcontainer configuration.
#[derive(Clone, Debug, Default)]
pub struct DevcontainerState {
    /// Whether the project has a `devcontainer.json` at all.
    pub exists: bool,
    /// Currently-enabled idealyst-managed services (for preselect).
    pub enabled: Vec<EnabledService>,
}

/// What `apply` changed — printed by the CLI, JSON-serialized by the MCP tool.
#[derive(Clone, Debug, Default)]
pub struct ConfigureReport {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub reconfigured: Vec<String>,
    pub unchanged: Vec<String>,
    /// Non-fatal notices (e.g. "minio already configured — no-op").
    pub warnings: Vec<String>,
    /// Files created or modified.
    pub wrote: Vec<PathBuf>,
}

/// Read the current devcontainer configuration for the project at `dir`.
pub fn read_state(dir: &Path) -> Result<DevcontainerState> {
    Ok(DevcontainerState {
        exists: wiring::exists(dir),
        enabled: compose::read_enabled(dir)?,
    })
}

/// Apply a batch of service instructions, (re)generating the managed compose
/// file and wiring the devcontainer. Always ensures a base devcontainer
/// exists (this is how a project gets "initialized").
pub fn apply(dir: &Path, req: &ConfigureRequest) -> Result<ConfigureReport> {
    let mut report = ConfigureReport::default();

    let before = compose::read_enabled(dir)?;
    let (after, warnings) = resolve(&before, &req.services)?;
    report.warnings = warnings;
    diff(&before, &after, &mut report);

    let project_name = project_name(dir);

    // Always initialize a base devcontainer (no-op if one exists).
    report.wrote.extend(wiring::ensure_base(dir, &project_name)?);

    // The app service key must be read AFTER a potential scaffold (which
    // creates it as `dev`).
    let app_service = wiring::app_service(dir);

    if after.is_empty() {
        // Tear down: drop the managed file + its reference, keep the base.
        let managed = compose::managed_path(dir);
        if managed.exists() {
            std::fs::remove_file(&managed)
                .with_context(|| format!("remove {}", managed.display()))?;
            report.wrote.push(managed);
        }
        report.wrote.extend(wiring::remove_reference(dir)?);
    } else {
        report.wrote.extend(wiring::add_reference(dir)?);
        let text = compose::render(&after, &app_service);
        let managed = compose::managed_path(dir);
        std::fs::write(&managed, text)
            .with_context(|| format!("write {}", managed.display()))?;
        report.wrote.push(managed);
    }

    dedup(&mut report.wrote);
    Ok(report)
}

/// Fold the requests onto the current set, producing the resulting enabled
/// set + any warnings. Errors on an unknown service id or invalid variant.
fn resolve(
    before: &[EnabledService],
    requests: &[ServiceRequest],
) -> Result<(Vec<EnabledService>, Vec<String>)> {
    let mut set = before.to_vec();
    let mut warnings = Vec::new();

    for r in requests {
        let svc = service::find(&r.id).with_context(|| {
            format!(
                "unknown service {:?}; known services: {}",
                r.id,
                registry()
                    .iter()
                    .map(|s| s.id())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
        if !svc.accepts_variant(r.variant.as_deref()) {
            anyhow::bail!(
                "service {:?} has no variant {:?}; valid variants: {}",
                r.id,
                r.variant,
                svc.variants()
                    .iter()
                    .map(|v| v.id)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        let pos = set.iter().position(|e| e.id == r.id);
        match r.action {
            Action::Enable => match pos {
                Some(_) => warnings.push(format!(
                    "{} already configured — no-op (use reconfigure to reset, remove to drop)",
                    r.id
                )),
                None => set.push(EnabledService {
                    id: r.id.clone(),
                    variant: normalize_variant(&*svc, r.variant.as_deref()),
                }),
            },
            Action::Reconfigure => {
                let variant = normalize_variant(&*svc, r.variant.as_deref());
                match pos {
                    Some(i) => set[i].variant = variant,
                    None => set.push(EnabledService { id: r.id.clone(), variant }),
                }
            }
            Action::Remove => match pos {
                Some(i) => {
                    set.remove(i);
                }
                None => warnings.push(format!("{} is not configured — nothing to remove", r.id)),
            },
        }
    }
    Ok((set, warnings))
}

/// Resolve a requested variant to a stored variant: an explicit choice is
/// kept; `None` stores the default explicitly for variant-bearing services
/// (so the file records what was chosen), or `None` for variantless ones.
fn normalize_variant(svc: &dyn DevService, requested: Option<&str>) -> Option<String> {
    match requested {
        Some(v) => Some(v.to_string()),
        None => svc.default_variant().map(String::from),
    }
}

/// Populate the report's diff buckets by comparing before/after sets.
fn diff(before: &[EnabledService], after: &[EnabledService], report: &mut ConfigureReport) {
    for a in after {
        match before.iter().find(|b| b.id == a.id) {
            None => report.added.push(a.id.clone()),
            Some(b) if b.variant != a.variant => report.reconfigured.push(a.id.clone()),
            Some(_) => report.unchanged.push(a.id.clone()),
        }
    }
    for b in before {
        if !after.iter().any(|a| a.id == b.id) {
            report.removed.push(b.id.clone());
        }
    }
}

/// Best-effort project name from the directory basename, for the scaffolded
/// devcontainer's `name` / workspace folder. Falls back to `"app"`.
fn project_name(dir: &Path) -> String {
    let canon = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    canon
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
        .unwrap_or("app")
        .to_string()
}

fn dedup(paths: &mut Vec<PathBuf>) {
    let mut seen = std::collections::HashSet::new();
    paths.retain(|p| seen.insert(p.clone()));
}
