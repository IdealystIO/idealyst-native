//! The VS Code configuration extension point.
//!
//! Each configurable concern (recommend the editor extensions, wire the
//! idealyst linter into rust-analyzer, …) is a [`VscodeAspect`] that declares
//! its contribution: `settings.json` keys to set, arrays to union into,
//! `extensions.json` recommendations, and files to drop under `.vscode/`.
//!
//! Detection, apply, and remove are all generic over [`AspectContribution`],
//! so an aspect is purely declarative — adding one is a new file under
//! `aspects/` plus a line in [`registry`], exactly like the devcontainer
//! [`DevService`] registry.
//!
//! [`DevService`]: crate::devcontainer::DevService

use std::path::PathBuf;

use serde_json::Value;

/// A file this aspect drops under the project (path is relative to the project
/// dir, typically `.vscode/…`).
#[derive(Clone, Debug)]
pub struct ManagedFile {
    pub rel: PathBuf,
    pub contents: String,
    /// Mark executable (0o755) on unix — used for `ra-check.sh`.
    pub executable: bool,
}

/// Everything an aspect contributes. All fields are optional; the generic
/// apply/remove/detect logic in `files.rs` interprets them.
#[derive(Clone, Debug, Default)]
pub struct AspectContribution {
    /// `settings.json` keys set to exactly these values (object overwrite).
    pub set_keys: Vec<(String, Value)>,
    /// `settings.json` array-valued keys to union these entries into (e.g.
    /// `rust-analyzer.diagnostics.disabled`).
    pub union_arrays: Vec<(String, Vec<Value>)>,
    /// `extensions.json` `recommendations` entries to union in.
    pub recommendations: Vec<String>,
    /// Files to write on enable / delete on remove.
    pub files: Vec<ManagedFile>,
}

/// A configurable VS Code concern.
pub trait VscodeAspect: Send + Sync {
    /// Canonical id — the CLI flag / wizard / MCP key. Stable + unique.
    fn id(&self) -> &'static str;
    /// Human label for the wizard.
    fn label(&self) -> &'static str;
    /// One-line description for the wizard / MCP schema.
    fn description(&self) -> &'static str;
    /// The declarative contribution. `project_dir` is passed so an aspect can
    /// reference project-relative paths if needed.
    fn contribution(&self, project_dir: &std::path::Path) -> AspectContribution;
}

/// The aspect registry — the single list the feature is driven by.
///
/// **To add an aspect:** implement [`VscodeAspect`] in a new `aspects/<id>.rs`
/// and push it here. The CLI flags, wizard entries, and MCP schema all derive
/// from this list.
pub fn registry() -> Vec<Box<dyn VscodeAspect>> {
    vec![
        Box::new(super::aspects::extensions::Extensions),
        Box::new(super::aspects::lint::Lint),
    ]
}

/// Look up an aspect by id.
pub fn find(id: &str) -> Option<Box<dyn VscodeAspect>> {
    registry().into_iter().find(|a| a.id() == id)
}
