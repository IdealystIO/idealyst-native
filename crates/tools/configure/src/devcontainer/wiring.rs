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

/// Path to the targeted `devcontainer.json`: the default
/// `<dir>/.devcontainer/devcontainer.json`, or a **named config**
/// `<dir>/.devcontainer/<config>/devcontainer.json` (the devcontainer spec's
/// multi-config layout, e.g. this repo's `arena` variant).
pub fn devcontainer_json_path(dir: &Path, config: Option<&str>) -> PathBuf {
    match config {
        Some(c) => dir.join(".devcontainer").join(c).join("devcontainer.json"),
        None => dir.join(".devcontainer").join("devcontainer.json"),
    }
}

/// The `dockerComposeFile` entry for the managed file, relative to the
/// targeted config's json. Named configs live one level deeper than
/// `.devcontainer/`, where the (shared) managed file stays.
fn managed_ref(config: Option<&str>) -> String {
    match config {
        Some(_) => format!("../{MANAGED_FILE}"),
        None => MANAGED_FILE.to_string(),
    }
}

/// Path to the user-owned base compose file.
pub fn base_compose_path(dir: &Path) -> PathBuf {
    dir.join(".devcontainer").join("docker-compose.yml")
}

/// Does the project already have the targeted `devcontainer.json`?
pub fn exists(dir: &Path, config: Option<&str>) -> bool {
    devcontainer_json_path(dir, config).exists()
}

/// The compose service key of the main dev container. Read from the targeted
/// `devcontainer.json`'s `"service"`; defaults to `"dev"`.
pub fn app_service(dir: &Path, config: Option<&str>) -> String {
    read_json(dir, config)
        .ok()
        .flatten()
        .and_then(|v| v.get("service").and_then(|s| s.as_str()).map(String::from))
        .unwrap_or_else(|| "dev".to_string())
}

/// Parse the targeted `devcontainer.json` (JSONC-tolerant). `Ok(None)` when
/// the file doesn't exist.
fn read_json(dir: &Path, config: Option<&str>) -> Result<Option<Value>> {
    let path = devcontainer_json_path(dir, config);
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
///
/// Named configs are never scaffolded — they're deliberate, hand-authored
/// variants (a missing one is a typo, not a fresh project), so targeting an
/// absent named config errors instead.
pub fn ensure_base(dir: &Path, config: Option<&str>, project_name: &str) -> Result<Vec<PathBuf>> {
    if read_json(dir, config)?.is_some() {
        return Ok(Vec::new());
    }
    if let Some(c) = config {
        anyhow::bail!(
            "named devcontainer config {:?} not found at {} — create it first \
             (named configs are not scaffolded)",
            c,
            devcontainer_json_path(dir, config).display(),
        );
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
pub fn add_reference(dir: &Path, config: Option<&str>) -> Result<Vec<PathBuf>> {
    let Some(mut value) = read_json(dir, config)? else {
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
            devcontainer_json_path(dir, config).display(),
        );
    }
    if add_managed_reference(obj, &managed_ref(config)) {
        write_json(dir, config, &value)?;
        return Ok(vec![devcontainer_json_path(dir, config)]);
    }
    Ok(Vec::new())
}

/// Remove the managed-file reference from `dockerComposeFile` (used when the
/// last managed service is torn down). Returns files written. No-op when the
/// devcontainer is absent or already doesn't reference it.
pub fn remove_reference(dir: &Path, config: Option<&str>) -> Result<Vec<PathBuf>> {
    let Some(mut value) = read_json(dir, config)? else {
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
    let managed = managed_ref(config);
    let before = arr.len();
    arr.retain(|v| v.as_str() != Some(managed.as_str()));
    if arr.len() == before {
        return Ok(Vec::new());
    }
    // Collapse a single-element array back to a bare string for tidiness.
    if arr.len() == 1 {
        let only = arr[0].clone();
        obj.insert("dockerComposeFile".into(), only);
    }
    write_json(dir, config, &value)?;
    Ok(vec![devcontainer_json_path(dir, config)])
}

/// Add the managed file to `dockerComposeFile`, normalizing a bare string to
/// an array. Returns true if a change was made (caller should rewrite).
fn add_managed_reference(obj: &mut serde_json::Map<String, Value>, managed: &str) -> bool {
    let entry = obj.get("dockerComposeFile").cloned().unwrap_or(Value::Null);
    let mut files: Vec<Value> = match entry {
        Value::String(s) => vec![Value::String(s)],
        Value::Array(a) => a,
        _ => Vec::new(),
    };
    if files.iter().any(|v| v.as_str() == Some(managed)) {
        return false;
    }
    files.push(Value::String(managed.to_string()));
    obj.insert("dockerComposeFile".into(), Value::Array(files));
    true
}

/// The `devcontainer.json` keys idealyst added — recorded in the managed
/// compose file's `x-idealyst.devcontainer` block so a later removal only
/// ever deletes keys *we* inserted. A feature (or lifecycle entry) the user
/// already had is never idealyst-owned and never touched.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ManagedWiring {
    /// Exact `features` keys we inserted (version tag included).
    pub features: Vec<String>,
    /// `postCreateCommand` object keys we inserted.
    pub post_create: Vec<String>,
}

/// A feature key without its version tag — `ghcr.io/x/y:1.0` → `ghcr.io/x/y`.
/// Feature ids contain no other `:`, so presence checks are tag-insensitive
/// (a user pinning `claude-code:1.2` counts as "already installed").
fn feature_id(key: &str) -> &str {
    key.rsplit_once(':').map(|(id, _)| id).unwrap_or(key)
}

/// Sync the desired `features` + keyed `postCreateCommand` entries into the
/// targeted `devcontainer.json`. `managed` is the previously-recorded set of
/// idealyst-owned keys; entries in it that are no longer desired are removed,
/// entries the user already had are left alone (and stay user-owned). Returns
/// (files written, the new idealyst-owned key set to record).
pub fn sync_tooling(
    dir: &Path,
    config: Option<&str>,
    desired_features: &[(String, serde_json::Value)],
    desired_post_create: &[(String, String)],
    managed: &ManagedWiring,
) -> Result<(Vec<PathBuf>, ManagedWiring)> {
    let Some(mut value) = read_json(dir, config)? else {
        return Ok((Vec::new(), ManagedWiring::default()));
    };
    let original = value.clone();
    let obj = value
        .as_object_mut()
        .context("devcontainer.json is not a JSON object")?;
    let mut now = ManagedWiring::default();

    // --- features ---
    let features = obj.entry("features").or_insert_with(|| json!({}));
    let map = features
        .as_object_mut()
        .context("devcontainer.json `features` is not an object")?;
    for (key, opts) in desired_features {
        match map.keys().find(|k| feature_id(k) == feature_id(key)).cloned() {
            // Present under some tag: ours only if we added it (this run or a
            // previous one); otherwise the user's — never claimed.
            Some(present) => {
                let ours = managed.features.contains(&present) || now.features.contains(&present);
                if ours && !now.features.contains(&present) {
                    now.features.push(present);
                }
            }
            None => {
                map.insert(key.clone(), opts.clone());
                now.features.push(key.clone());
            }
        }
    }
    for m in &managed.features {
        let still_desired = desired_features
            .iter()
            .any(|(k, _)| feature_id(k) == feature_id(m));
        if !still_desired {
            map.remove(m);
        }
    }
    if map.is_empty() {
        obj.remove("features");
    }

    // --- postCreateCommand (object form — entries run in parallel) ---
    now.post_create = desired_post_create.iter().map(|(k, _)| k.clone()).collect();
    match obj.get("postCreateCommand").cloned() {
        Some(Value::Object(mut map)) => {
            for (k, cmd) in desired_post_create {
                map.insert(k.clone(), Value::String(cmd.clone()));
            }
            for m in &managed.post_create {
                if !desired_post_create.iter().any(|(k, _)| k == m) {
                    map.remove(m);
                }
            }
            if map.is_empty() {
                obj.remove("postCreateCommand");
            } else {
                obj.insert("postCreateCommand".into(), Value::Object(map));
            }
        }
        // A bare string/array command is the user's own; converting it to one
        // keyed object entry preserves its semantics while letting ours merge
        // beside it. When we have nothing to add, leave its shape alone.
        Some(existing) if !desired_post_create.is_empty() => {
            let mut map = serde_json::Map::new();
            map.insert("main".into(), existing);
            for (k, cmd) in desired_post_create {
                map.insert(k.clone(), Value::String(cmd.clone()));
            }
            obj.insert("postCreateCommand".into(), Value::Object(map));
        }
        Some(_) => {}
        None if !desired_post_create.is_empty() => {
            let mut map = serde_json::Map::new();
            for (k, cmd) in desired_post_create {
                map.insert(k.clone(), Value::String(cmd.clone()));
            }
            obj.insert("postCreateCommand".into(), Value::Object(map));
        }
        None => {}
    }

    if value != original {
        write_json(dir, config, &value)?;
        return Ok((vec![devcontainer_json_path(dir, config)], now));
    }
    Ok((Vec::new(), now))
}

/// Write the targeted `devcontainer.json` pretty-printed with a trailing newline.
fn write_json(dir: &Path, config: Option<&str>, value: &Value) -> Result<()> {
    let path = devcontainer_json_path(dir, config);
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
    let json_path = devcontainer_json_path(dir, None);
    write_json(dir, None, &devcontainer)?;

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
