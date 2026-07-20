//! Generic `.vscode/` read/merge/remove over [`AspectContribution`].
//!
//! We never own `settings.json` / `extensions.json` outright (users keep their
//! own keys there), so we merge *surgically*: set only our keys, union only our
//! array entries + recommendations, and on remove pull exactly those back out.
//! Files we generate whole (`ra-check.sh`) are written on enable and deleted on
//! remove. Both JSON files are JSONC (comments allowed) so we strip comments to
//! read; we only rewrite a file when something actually changed.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Map, Value};

use super::aspect::AspectContribution;

pub fn vscode_dir(dir: &Path) -> PathBuf {
    dir.join(".vscode")
}
pub fn settings_path(dir: &Path) -> PathBuf {
    vscode_dir(dir).join("settings.json")
}
pub fn extensions_path(dir: &Path) -> PathBuf {
    vscode_dir(dir).join("extensions.json")
}

/// In-memory view of the project's `.vscode/` JSON files, mutated by
/// aspect apply/remove and flushed by [`Workspace::finish`].
pub struct Workspace {
    dir: PathBuf,
    settings: Map<String, Value>,
    settings_existed: bool,
    settings_dirty: bool,
    extensions: Map<String, Value>,
    extensions_existed: bool,
    extensions_dirty: bool,
    wrote: Vec<PathBuf>,
}

impl Workspace {
    /// Read the current `.vscode/settings.json` + `extensions.json` (JSONC
    /// tolerant). Missing files start as empty objects.
    pub fn load(dir: &Path) -> Result<Self> {
        let (settings, settings_existed) = read_object(&settings_path(dir))?;
        let (extensions, extensions_existed) = read_object(&extensions_path(dir))?;
        Ok(Self {
            dir: dir.to_path_buf(),
            settings,
            settings_existed,
            settings_dirty: false,
            extensions,
            extensions_existed,
            extensions_dirty: false,
            wrote: Vec::new(),
        })
    }

    /// Is this contribution fully applied already? (Used for wizard preselect
    /// and to report unchanged/no-op.)
    pub fn is_present(&self, c: &AspectContribution) -> bool {
        let recs = self.recommendations();
        c.set_keys.iter().all(|(k, _)| self.settings.contains_key(k))
            && c.union_arrays.iter().all(|(k, vals)| {
                self.settings
                    .get(k)
                    .and_then(|v| v.as_array())
                    .map(|arr| vals.iter().all(|want| arr.contains(want)))
                    .unwrap_or(false)
            })
            && c.recommendations.iter().all(|id| recs.contains(&id.as_str()))
            && c.files.iter().all(|f| self.dir.join(&f.rel).exists())
    }

    /// Apply the contribution: set keys, union arrays + recommendations, write
    /// files.
    pub fn apply(&mut self, c: &AspectContribution) -> Result<()> {
        for (k, v) in &c.set_keys {
            if self.settings.get(k) != Some(v) {
                self.settings.insert(k.clone(), v.clone());
                self.settings_dirty = true;
            }
        }
        for (k, vals) in &c.union_arrays {
            let arr = self
                .settings
                .entry(k.clone())
                .or_insert_with(|| Value::Array(Vec::new()));
            if let Some(arr) = arr.as_array_mut() {
                for want in vals {
                    if !arr.contains(want) {
                        arr.push(want.clone());
                        self.settings_dirty = true;
                    }
                }
            }
        }
        if !c.recommendations.is_empty() {
            let recs = self
                .extensions
                .entry("recommendations".to_string())
                .or_insert_with(|| Value::Array(Vec::new()));
            if let Some(recs) = recs.as_array_mut() {
                for id in &c.recommendations {
                    let want = Value::String(id.clone());
                    if !recs.contains(&want) {
                        recs.push(want);
                        self.extensions_dirty = true;
                    }
                }
            }
        }
        for f in &c.files {
            self.write_file(f)?;
        }
        Ok(())
    }

    /// Remove the contribution: drop our keys, our array entries, our
    /// recommendations, and delete our files.
    pub fn remove(&mut self, c: &AspectContribution) -> Result<()> {
        for (k, _) in &c.set_keys {
            if self.settings.remove(k).is_some() {
                self.settings_dirty = true;
            }
        }
        for (k, vals) in &c.union_arrays {
            if let Some(arr) = self.settings.get_mut(k).and_then(|v| v.as_array_mut()) {
                let before = arr.len();
                arr.retain(|entry| !vals.contains(entry));
                if arr.len() != before {
                    self.settings_dirty = true;
                }
                if arr.is_empty() {
                    self.settings.remove(k);
                }
            }
        }
        if !c.recommendations.is_empty() {
            if let Some(recs) = self
                .extensions
                .get_mut("recommendations")
                .and_then(|v| v.as_array_mut())
            {
                let before = recs.len();
                recs.retain(|entry| entry.as_str().map(|s| !c.recommendations.iter().any(|r| r == s)).unwrap_or(true));
                if recs.len() != before {
                    self.extensions_dirty = true;
                }
                if recs.is_empty() {
                    self.extensions.remove("recommendations");
                }
            }
        }
        for f in &c.files {
            let path = self.dir.join(&f.rel);
            if path.exists() {
                std::fs::remove_file(&path)
                    .with_context(|| format!("remove {}", path.display()))?;
                self.wrote.push(path);
            }
        }
        Ok(())
    }

    /// Flush any changes to disk. Returns the files written/removed.
    pub fn finish(mut self) -> Result<Vec<PathBuf>> {
        if self.settings_dirty {
            // Skip writing an empty object that never existed.
            if !self.settings.is_empty() || self.settings_existed {
                write_object(&settings_path(&self.dir), &self.settings)?;
                self.wrote.push(settings_path(&self.dir));
            }
        }
        if self.extensions_dirty && (!self.extensions.is_empty() || self.extensions_existed) {
            write_object(&extensions_path(&self.dir), &self.extensions)?;
            self.wrote.push(extensions_path(&self.dir));
        }
        Ok(self.wrote)
    }

    fn recommendations(&self) -> Vec<&str> {
        self.extensions
            .get("recommendations")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default()
    }

    fn write_file(&mut self, f: &super::aspect::ManagedFile) -> Result<()> {
        let path = self.dir.join(&f.rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        let already = std::fs::read_to_string(&path).ok().as_deref() == Some(f.contents.as_str());
        if !already {
            std::fs::write(&path, &f.contents)
                .with_context(|| format!("write {}", path.display()))?;
            self.wrote.push(path.clone());
        }
        #[cfg(unix)]
        if f.executable {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path)?.permissions();
            if perms.mode() & 0o111 == 0 {
                perms.set_mode(0o755);
                std::fs::set_permissions(&path, perms)
                    .with_context(|| format!("chmod +x {}", path.display()))?;
            }
        }
        Ok(())
    }
}

/// Read a JSONC object file into a map. Returns `(map, existed)`; a missing
/// file yields an empty map. Errors if the file isn't a JSON object.
fn read_object(path: &Path) -> Result<(Map<String, Value>, bool)> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok((Map::new(), false));
    };
    let stripped = crate::jsonc::strip(&text);
    // An empty/whitespace file is a valid "empty object" start.
    if stripped.trim().is_empty() {
        return Ok((Map::new(), true));
    }
    let value: Value = serde_json::from_str(&stripped)
        .with_context(|| format!("parse {}", path.display()))?;
    let map = value
        .as_object()
        .cloned()
        .with_context(|| format!("{} is not a JSON object", path.display()))?;
    Ok((map, true))
}

fn write_object(path: &Path, map: &Map<String, Value>) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;
    }
    let mut text = serde_json::to_string_pretty(&Value::Object(map.clone()))
        .context("serialize .vscode json")?;
    text.push('\n');
    std::fs::write(path, text).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}
