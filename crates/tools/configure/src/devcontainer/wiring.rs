//! `devcontainer.json` wiring + fresh-container scaffold.
//!
//! Ownership split: the user owns `docker-compose.yml`; idealyst owns
//! `docker-compose.idealyst.yml`. The only thing we ever change in the user's
//! `devcontainer.json` is to make sure its `dockerComposeFile` array *lists*
//! our managed file — a one-time, idempotent edit. When there's no
//! devcontainer at all we scaffold a minimal compose-based one.
//!
//! `devcontainer.json` is JSONC (comments allowed), which `serde_json` won't
//! parse, so we strip comments before reading. We rewrite the file only when
//! the managed reference is actually missing, so the (comment-losing) rewrite
//! happens at most once on a simple, freshly-relevant file.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use super::compose::MANAGED_FILE;

/// Path to `<dir>/.devcontainer/devcontainer.json`.
pub fn devcontainer_json_path(dir: &Path) -> PathBuf {
    dir.join(".devcontainer").join("devcontainer.json")
}

/// Path to the user-owned base compose file.
pub fn base_compose_path(dir: &Path) -> PathBuf {
    dir.join(".devcontainer").join("docker-compose.yml")
}

/// Does the project already have a `devcontainer.json`?
pub fn exists(dir: &Path) -> bool {
    devcontainer_json_path(dir).exists()
}

/// The compose service key of the main dev container. Read from
/// `devcontainer.json`'s `"service"`; defaults to `"dev"`.
pub fn app_service(dir: &Path) -> String {
    read_json(dir)
        .ok()
        .flatten()
        .and_then(|v| v.get("service").and_then(|s| s.as_str()).map(String::from))
        .unwrap_or_else(|| "dev".to_string())
}

/// Parse the existing `devcontainer.json` (JSONC-tolerant). `Ok(None)` when
/// the file doesn't exist.
fn read_json(dir: &Path) -> Result<Option<Value>> {
    let path = devcontainer_json_path(dir);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    let stripped = crate::jsonc::strip(&text);
    let value: Value = serde_json::from_str(&stripped)
        .with_context(|| format!("parse {}", path.display()))?;
    Ok(Some(value))
}

/// Ensure a compose-based devcontainer *exists*, scaffolding a base one when
/// none is present. Does NOT add the managed reference (see [`add_reference`])
/// so an empty service set still yields a working base container. Returns
/// files written (empty when a devcontainer already exists).
pub fn ensure_base(dir: &Path, project_name: &str) -> Result<Vec<PathBuf>> {
    if read_json(dir)?.is_some() {
        return Ok(Vec::new());
    }
    scaffold_base(dir, project_name)
}

/// Ensure the existing devcontainer references the managed compose file,
/// normalizing a bare `dockerComposeFile` string to an array. Returns files
/// written (empty when already referenced).
///
/// Errors if `devcontainer.json` is not compose-based (Dockerfile/image
/// devcontainers can't merge our sidecar services) — rather than silently
/// mangle it, we tell the user what's incompatible.
pub fn add_reference(dir: &Path) -> Result<Vec<PathBuf>> {
    let Some(mut value) = read_json(dir)? else {
        return Ok(Vec::new());
    };
    let obj = value
        .as_object_mut()
        .context("devcontainer.json is not a JSON object")?;
    if !obj.contains_key("dockerComposeFile") {
        anyhow::bail!(
            "{} is not a docker-compose devcontainer (no `dockerComposeFile`). \
             idealyst-managed services attach via docker-compose; convert the \
             devcontainer to compose, or remove `.devcontainer/` to let \
             `idealyst configure devcontainer` scaffold a compose-based one.",
            devcontainer_json_path(dir).display(),
        );
    }
    if add_managed_reference(obj) {
        write_json(dir, &value)?;
        return Ok(vec![devcontainer_json_path(dir)]);
    }
    Ok(Vec::new())
}

/// Remove the managed-file reference from `dockerComposeFile` (used when the
/// last managed service is torn down). Returns files written. No-op when the
/// devcontainer is absent or already doesn't reference it.
pub fn remove_reference(dir: &Path) -> Result<Vec<PathBuf>> {
    let Some(mut value) = read_json(dir)? else {
        return Ok(Vec::new());
    };
    let Some(obj) = value.as_object_mut() else {
        return Ok(Vec::new());
    };
    let Some(entry) = obj.get_mut("dockerComposeFile") else {
        return Ok(Vec::new());
    };
    let Some(arr) = entry.as_array_mut() else {
        return Ok(Vec::new());
    };
    let before = arr.len();
    arr.retain(|v| v.as_str() != Some(MANAGED_FILE));
    if arr.len() == before {
        return Ok(Vec::new());
    }
    // Collapse a single-element array back to a bare string for tidiness.
    if arr.len() == 1 {
        let only = arr[0].clone();
        obj.insert("dockerComposeFile".into(), only);
    }
    write_json(dir, &value)?;
    Ok(vec![devcontainer_json_path(dir)])
}

/// Add the managed file to `dockerComposeFile`, normalizing a bare string to
/// an array. Returns true if a change was made (caller should rewrite).
fn add_managed_reference(obj: &mut serde_json::Map<String, Value>) -> bool {
    let entry = obj.get("dockerComposeFile").cloned().unwrap_or(Value::Null);
    let mut files: Vec<Value> = match entry {
        Value::String(s) => vec![Value::String(s)],
        Value::Array(a) => a,
        _ => Vec::new(),
    };
    if files.iter().any(|v| v.as_str() == Some(MANAGED_FILE)) {
        return false;
    }
    files.push(Value::String(MANAGED_FILE.to_string()));
    obj.insert("dockerComposeFile".into(), Value::Array(files));
    true
}

/// Write `devcontainer.json` pretty-printed with a trailing newline.
fn write_json(dir: &Path, value: &Value) -> Result<()> {
    let path = devcontainer_json_path(dir);
    let mut text = serde_json::to_string_pretty(value).context("serialize devcontainer.json")?;
    text.push('\n');
    std::fs::write(&path, text).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Scaffold a minimal compose-based devcontainer: `.devcontainer/` +
/// `devcontainer.json` + a base `docker-compose.yml` with a `dev` service.
/// Returns the files written.
fn scaffold_base(dir: &Path, project_name: &str) -> Result<Vec<PathBuf>> {
    let dc_dir = dir.join(".devcontainer");
    std::fs::create_dir_all(&dc_dir)
        .with_context(|| format!("create {}", dc_dir.display()))?;

    // Reference only the base compose here; the managed file is added by
    // `add_reference` when services are actually enabled, so an empty
    // configuration never leaves a dangling reference.
    let devcontainer = json!({
        "name": project_name,
        "dockerComposeFile": "docker-compose.yml",
        "service": "dev",
        "workspaceFolder": format!("/workspaces/{project_name}"),
        "customizations": {
            "vscode": {
                "extensions": ["rust-lang.rust-analyzer"]
            }
        }
    });
    let json_path = devcontainer_json_path(dir);
    write_json(dir, &devcontainer)?;

    // Base compose: a single Rust dev container. `..` (the project root, one
    // level up from `.devcontainer/`) is mounted at the workspace folder.
    let base = format!(
        "# Base devcontainer service — YOURS to edit. Add your own services here;\n\
         # idealyst-managed sidecars live in `{MANAGED_FILE}`.\n\
         services:\n\
        \x20 dev:\n\
        \x20   image: mcr.microsoft.com/devcontainers/rust:1\n\
        \x20   volumes:\n\
        \x20     - ..:/workspaces/{project_name}:cached\n\
        \x20   command: sleep infinity\n"
    );
    let base_path = base_compose_path(dir);
    std::fs::write(&base_path, base)
        .with_context(|| format!("write {}", base_path.display()))?;

    Ok(vec![json_path, base_path])
}
