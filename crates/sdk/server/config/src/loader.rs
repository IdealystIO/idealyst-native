//! Loading + merging config files.
//!
//! Every file — `idealyst.toml` (the base) and the per-tool `jobs.toml` /
//! `pubsub.toml` / `email.toml` (aka `mail.toml`) / `cache.toml` —
//! deserializes into the same
//! [`Config`]. The loader merges them (base first, then each per-tool file),
//! resolves `extends` inheritance, and finally overlays environment variables.

use crate::error::ConfigError;
use crate::schema::Config;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The base config file name.
const BASE_FILE: &str = "idealyst.toml";

/// Per-tool config files, merged after the base (in this order). `mail.toml`
/// is an accepted alias for `email.toml`.
const TOOL_FILES: &[&str] = &["jobs.toml", "pubsub.toml", "email.toml", "mail.toml", "cache.toml"];

/// Load and merge configuration from the current working directory.
pub fn load() -> Result<Config, ConfigError> {
    let dir = std::env::current_dir().map_err(|e| ConfigError::Io {
        path: ".".into(),
        source: e,
    })?;
    load_from(&dir)
}

/// Load and merge configuration rooted at `dir`: the base `idealyst.toml`, then
/// each present per-tool file, then environment-variable overrides. Missing
/// files are simply skipped (an app with no config gets the defaults +
/// whatever env provides).
pub fn load_from(dir: &Path) -> Result<Config, ConfigError> {
    let mut config = Config::default();

    let base = dir.join(BASE_FILE);
    if base.is_file() {
        config.merge(read_with_extends(&base, &mut HashSet::new())?);
    }
    for name in TOOL_FILES {
        let path = dir.join(name);
        if path.is_file() {
            config.merge(read_with_extends(&path, &mut HashSet::new())?);
        }
    }

    apply_env_overrides(&mut config);
    Ok(config)
}

/// Parse `path` as a [`Config`], resolving its `extends` parents first (each
/// relative to `path`'s directory) so a file inherits its parents' connections
/// and sections, then overlaying `path`'s own values on top. `visiting` guards
/// against `extends` cycles.
fn read_with_extends(path: &Path, visiting: &mut HashSet<PathBuf>) -> Result<Config, ConfigError> {
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if !visiting.insert(canon.clone()) {
        return Err(ConfigError::ExtendsCycle(path.display().to_string()));
    }

    let raw = std::fs::read_to_string(path).map_err(|e| ConfigError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    let this: Config = toml::from_str(&raw).map_err(|e| ConfigError::Parse {
        path: path.display().to_string(),
        source: e,
    })?;

    // Parents merge first (in listed order); this file overlays them.
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut acc = Config::default();
    for parent in &this.extends {
        let parent_path = dir.join(parent);
        acc.merge(read_with_extends(&parent_path, visiting)?);
    }
    acc.merge(this);
    acc.extends.clear();

    visiting.remove(&canon);
    Ok(acc)
}

/// Overlay environment variables onto the merged config, so env stays a valid
/// OVERRIDE layer (secret injection in prod) even though the file is the
/// primary surface. Only the long-standing flat vars are honored; a set var
/// wins over the file.
fn apply_env_overrides(config: &mut Config) {
    use crate::schema::{CacheSection, EmailSection, JobsSection, PubsubSection};

    if let Ok(v) = std::env::var("IDEALYST_JOBS_BACKEND") {
        config.jobs.get_or_insert_with(JobsSection::default).backend = Some(v);
    }
    if let Ok(v) = std::env::var("IDEALYST_JOBS_URL") {
        config.jobs.get_or_insert_with(JobsSection::default).url = Some(v);
    }
    if let Ok(v) = std::env::var("IDEALYST_PUBSUB_BACKEND") {
        config.pubsub.get_or_insert_with(PubsubSection::default).backend = Some(v);
    }
    if let Ok(v) = std::env::var("IDEALYST_PUBSUB_URL") {
        config.pubsub.get_or_insert_with(PubsubSection::default).url = Some(v);
    }
    if let Ok(v) = std::env::var("IDEALYST_CACHE_BACKEND") {
        config.cache.get_or_insert_with(CacheSection::default).backend = Some(v);
    }
    if let Ok(v) = std::env::var("IDEALYST_CACHE_URL") {
        config.cache.get_or_insert_with(CacheSection::default).url = Some(v);
    }
    if let Ok(v) = std::env::var("IDEALYST_EMAIL_PROVIDER") {
        config.email.get_or_insert_with(EmailSection::default).provider = Some(v);
    }
    if let Ok(v) = std::env::var("IDEALYST_EMAIL_FROM") {
        config.email.get_or_insert_with(EmailSection::default).from = Some(v);
    }
}
