//! Where wrapper Cargo.tomls source framework crates from.
//!
//! The build crates emit ephemeral wrapper Cargo.tomls for each
//! platform (`ios/wrapper`, `android/wrapper`, …). Those wrappers
//! depend on `framework-core`, `backend-<platform>-*`, and friends.
//!
//! Two cases:
//!
//! 1. **In-tree.** The user's project lives inside the framework
//!    workspace (e.g. `examples/hello-world/`). We emit
//!    `path = ".../crates/framework/core"` deps so cargo shares
//!    the workspace's `target/` cache and any local edits to the
//!    framework take effect immediately.
//!
//! 2. **External.** The CLI was installed (`cargo install idealyst-cli`)
//!    and is being run against a project that doesn't sit inside this
//!    repo. We emit `git = "<repo>", rev = "<sha>"` deps so cargo
//!    fetches the framework from GitHub.
//!
//! Resolution happens via [`FrameworkSource::detect`]. The git
//! defaults (URL + rev) are captured at CLI compile time and threaded
//! down — see `crates/cli/build.rs`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Where the generated wrapper Cargo.toml should source framework
/// crates from.
#[derive(Clone, Debug)]
pub enum FrameworkSource {
    /// The project lives inside the framework workspace, or
    /// `IDEALYST_FRAMEWORK_PATH` pointed us at a checkout. Wrapper
    /// deps are emitted as `path = "<root>/crates/..."`.
    Workspace { root: PathBuf },
    /// External project — wrapper deps go through git. Used as the
    /// fallback when no framework workspace is found near the user's
    /// project.
    Git { url: String, rev: String },
}

/// Compile-time git defaults baked into the CLI binary.
///
/// The CLI captures these in its own `build.rs` (so `cargo install`
/// users get a CLI pinned to the framework commit it was built
/// against) and passes them to the build crates at runtime. The build
/// crates can't reach those env consts directly because they're set
/// during the CLI's compile, not theirs.
#[derive(Clone, Debug)]
pub struct GitDefaults {
    pub url: String,
    pub rev: String,
}

impl FrameworkSource {
    /// Resolve a `FrameworkSource` for the given project.
    ///
    /// Resolution order:
    /// 1. `IDEALYST_FRAMEWORK_PATH` env var — force path mode against
    ///    the supplied checkout. Useful for contributors who want to
    ///    test the CLI against an unrelated working directory.
    /// 2. Walk up from `project_dir`; if an idealyst framework
    ///    workspace root is found, use it.
    /// 3. **Read the project's `Cargo.toml`** and reuse whatever
    ///    `framework-core = { git, rev }` (or `path`) spec it
    ///    already has. This is the most important branch in
    ///    practice — it makes the user's Cargo.toml authoritative,
    ///    so the generated wrapper picks up the same `framework-core`
    ///    revision the user crate uses and cargo can unify them.
    ///    Without this, a CLI re-installed against a different commit
    ///    than the project was scaffolded against would generate a
    ///    wrapper pointing at a different rev → cargo treats them as
    ///    two `framework-core` instances → `Primitive` type
    ///    mismatch at link.
    /// 4. Fall back to git, using the supplied defaults (only used
    ///    for fresh `idealyst new` scaffolding where there isn't a
    ///    project `Cargo.toml` yet).
    pub fn detect(project_dir: &Path, git: GitDefaults) -> Result<Self> {
        if let Ok(p) = std::env::var("IDEALYST_FRAMEWORK_PATH") {
            let root = PathBuf::from(&p);
            if !is_framework_root(&root) {
                anyhow::bail!(
                    "IDEALYST_FRAMEWORK_PATH={} does not look like an idealyst-native \
                     checkout (missing crates/framework/core/Cargo.toml)",
                    root.display(),
                );
            }
            return Ok(Self::Workspace { root });
        }
        if let Some(root) = find_framework_workspace(project_dir) {
            return Ok(Self::Workspace { root });
        }
        if let Some(from_project) = read_project_framework_dep(project_dir) {
            return Ok(from_project);
        }
        Ok(Self::Git { url: git.url, rev: git.rev })
    }

    /// True if this source is an in-tree workspace.
    pub fn is_workspace(&self) -> bool {
        matches!(self, Self::Workspace { .. })
    }

    /// Workspace root, when we have one. Some commands (`build-aas`,
    /// `dev`) need this because they reach into the workspace's
    /// `target/` for sidecar binaries or shared caches.
    pub fn workspace_root(&self) -> Option<&Path> {
        match self {
            Self::Workspace { root } => Some(root.as_path()),
            Self::Git { .. } => None,
        }
    }

    /// Root for ephemeral wrapper crates. In-tree projects share the
    /// framework workspace's `target/idealyst/` so cargo's build
    /// cache stays warm across `examples/*` rebuilds. External
    /// projects use their own `<project>/target/idealyst/`.
    pub fn wrapper_root(&self, project_dir: &Path) -> PathBuf {
        match self {
            Self::Workspace { root } => root.join("target/idealyst"),
            Self::Git { .. } => project_dir.join("target/idealyst"),
        }
    }

    /// Cargo target dir to redirect the wrapper crate's build output
    /// to via its `.cargo/config.toml`. Same in-tree-vs-external
    /// distinction as `wrapper_root`.
    pub fn cargo_target_dir(&self, project_dir: &Path) -> PathBuf {
        match self {
            Self::Workspace { root } => root.join("target"),
            Self::Git { .. } => project_dir.join("target"),
        }
    }

    /// Render a single dependency line for a framework crate.
    ///
    /// `subpath` is the directory under the workspace root (e.g.
    /// `crates/framework/core`) used in workspace mode. In git mode
    /// the package name is taken from the TOML key the caller uses —
    /// cargo accepts `framework-core = { git = "...", rev = "..." }`
    /// and resolves the matching package in the monorepo.
    pub fn dep(&self, subpath: &str, features: &[&str]) -> String {
        let features_clause = if features.is_empty() {
            String::new()
        } else {
            let list = features
                .iter()
                .map(|f| format!("\"{}\"", f))
                .collect::<Vec<_>>()
                .join(", ");
            format!(", features = [{}]", list)
        };
        match self {
            Self::Workspace { root } => format!(
                "{{ path = \"{}\"{} }}",
                root.join(subpath).display(),
                features_clause,
            ),
            Self::Git { url, rev } => format!(
                "{{ git = \"{}\", rev = \"{}\"{} }}",
                url, rev, features_clause,
            ),
        }
    }
}

/// Walk up from `start` looking for the idealyst framework workspace
/// root.
///
/// We require both `[workspace]` AND `crates/framework/core/Cargo.toml`
/// to exist at the same directory before we'll claim it as the
/// framework workspace. A consumer's project that happens to live
/// inside its *own* unrelated Cargo workspace would otherwise be
/// mistaken for an in-tree idealyst checkout.
fn find_framework_workspace(start: &Path) -> Option<PathBuf> {
    for ancestor in start.ancestors() {
        if is_framework_root(ancestor) {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

fn is_framework_root(root: &Path) -> bool {
    let cargo = root.join("Cargo.toml");
    if !cargo.is_file() {
        return false;
    }
    if !root.join("crates/framework/core/Cargo.toml").is_file() {
        return false;
    }
    let content = fs::read_to_string(&cargo).unwrap_or_default();
    content.contains("[workspace]")
}

/// Parse `<project>/Cargo.toml` and extract the `framework-core` dep
/// as a `FrameworkSource`. Supports the three common forms:
///
/// - `framework-core = { git = "<url>", rev = "<sha>" }` → `Git`.
/// - `framework-core = { git = "<url>", branch = "<b>" }` → `Git`
///   with rev set to the branch name (cargo accepts branches).
/// - `framework-core = { path = "/p/to/framework/core" }` → strip
///   `/crates/framework/core` to get the workspace root and emit
///   `Workspace`. (Falls through to git defaults if the path
///   doesn't end with the expected suffix.)
///
/// Returns `None` if the project has no `framework-core` dep, or
/// the dep is in a form we can't interpret (e.g. plain version
/// string, custom registries). Callers fall back to the git
/// defaults in those cases.
fn read_project_framework_dep(project_dir: &Path) -> Option<FrameworkSource> {
    let raw = fs::read_to_string(project_dir.join("Cargo.toml")).ok()?;
    let parsed: toml::Value = toml::from_str(&raw).ok()?;
    let dep = parsed.get("dependencies")?.get("framework-core")?;
    let table = dep.as_table()?;

    if let Some(path_str) = table.get("path").and_then(|v| v.as_str()) {
        let core_path = PathBuf::from(path_str);
        // Expect the path to end in `crates/framework/core`. Strip
        // those segments to recover the workspace root.
        let trimmed = core_path
            .ancestors()
            .nth(3)
            .map(|p| p.to_path_buf());
        if let Some(root) = trimmed {
            if is_framework_root(&root) {
                return Some(FrameworkSource::Workspace { root });
            }
        }
        return None;
    }

    let url = table.get("git").and_then(|v| v.as_str())?.to_string();
    let rev = table
        .get("rev")
        .or_else(|| table.get("branch"))
        .or_else(|| table.get("tag"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())?;
    Some(FrameworkSource::Git { url, rev })
}

/// Back-compat thin wrapper around the legacy `find_workspace_root`
/// semantics — kept so call sites that genuinely need an in-tree
/// workspace (AAS mode, `dev` server) can fail clearly when run
/// outside the framework checkout.
pub fn require_workspace_root(start: &Path) -> Result<PathBuf> {
    find_framework_workspace(start).with_context(|| {
        format!(
            "could not find the idealyst framework workspace walking up from {}. \
             This command requires an in-tree checkout — install the framework \
             alongside your project or run it from inside `idealyst-native/`.",
            start.display(),
        )
    })
}
