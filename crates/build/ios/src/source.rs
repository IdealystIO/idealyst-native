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

/// Which git refspec a git-sourced framework dep should pin to.
/// Cargo lets us choose between three forms; we surface all three so
/// the CLI and the user's `Cargo.toml` can agree on a stable name.
#[derive(Clone, Debug)]
pub enum GitRef {
    /// `rev = "<sha>"` — exact commit. Maximum precision, but the
    /// hash needs bumping every time the framework changes. The CLI
    /// falls back to this when no tag covers HEAD.
    Rev(String),
    /// `tag = "<name>"` — annotated git tag (typically `v0.1.0`).
    /// Stable, human-readable, immutable in practice. Preferred for
    /// release-pinned consumers; the CLI uses this when `build.rs`
    /// detected a tag at HEAD.
    Tag(String),
    /// `branch = "<name>"` — tracks a branch. Useful for "latest on
    /// main" workflows but moves under you, so we don't scaffold
    /// with it by default.
    Branch(String),
}

impl GitRef {
    /// `(key, value)` pair for emitting into a Cargo.toml dep table.
    pub fn as_pair(&self) -> (&'static str, &str) {
        match self {
            Self::Rev(s) => ("rev", s.as_str()),
            Self::Tag(s) => ("tag", s.as_str()),
            Self::Branch(s) => ("branch", s.as_str()),
        }
    }
}

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
    Git { url: String, refspec: GitRef },
}

/// Compile-time git defaults baked into the CLI binary.
///
/// The CLI captures these in its own `build.rs` (so `cargo install`
/// users get a CLI pinned to the framework commit it was built
/// against) and passes them to the build crates at runtime. The build
/// crates can't reach those env consts directly because they're set
/// during the CLI's compile, not theirs.
///
/// Prefer the most-recent annotated tag at HEAD over the raw commit
/// — `tag = "v0.1.0"` reads better in scaffolded Cargo.tomls and is
/// what release-tracking users actually want. `build.rs` does the
/// detection; this struct just transports the result.
#[derive(Clone, Debug)]
pub struct GitDefaults {
    pub url: String,
    pub refspec: GitRef,
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
        Ok(Self::Git { url: git.url, refspec: git.refspec })
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
            Self::Git { url, refspec } => {
                let (key, value) = refspec.as_pair();
                format!(
                    "{{ git = \"{}\", {} = \"{}\"{} }}",
                    url, key, value, features_clause,
                )
            }
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
    // Preserve the user's choice of refspec — emitting `rev = "v0.1.0"`
    // when the project specifies `tag = "v0.1.0"` would round-trip as
    // an invalid commit hash. Order matches cargo's: rev > tag > branch.
    let refspec = if let Some(s) = table.get("rev").and_then(|v| v.as_str()) {
        GitRef::Rev(s.to_string())
    } else if let Some(s) = table.get("tag").and_then(|v| v.as_str()) {
        GitRef::Tag(s.to_string())
    } else if let Some(s) = table.get("branch").and_then(|v| v.as_str()) {
        GitRef::Branch(s.to_string())
    } else {
        return None;
    };
    Some(FrameworkSource::Git { url, refspec })
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

#[cfg(test)]
mod tests {
    //! Regression coverage for the out-of-tree AAS / build path.
    //!
    //! The original failure mode: a user CLI installed via
    //! `cargo install idealyst-cli` was running `idealyst dev --aas`
    //! against a project that lived nowhere near an `idealyst-native/`
    //! checkout. The legacy wrappers walked up looking for a framework
    //! workspace and bailed with "could not find the idealyst framework
    //! workspace…". The current behavior is that `FrameworkSource::detect`
    //! falls back to reading the project's own `framework-core` git dep,
    //! and finally to compile-time git defaults — never to a workspace
    //! requirement.
    //!
    //! These tests pin that flow so the regression can't slip back in.
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Tempdir under `std::env::temp_dir()` that cleans itself up on
    /// drop. Avoids adding a `tempfile` dev-dependency for two tests.
    struct TempProject {
        path: PathBuf,
    }

    impl TempProject {
        fn new(label: &str) -> Self {
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let seq = SEQ.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("idealyst-source-test-{label}-{nanos}-{seq}"));
            fs::create_dir_all(&path).expect("create tempdir");
            // Canonicalize so the path returned matches what `detect`
            // sees after its own canonicalize calls — macOS routes
            // `/var/folders/...` through `/private/var/folders/...`.
            let canon = fs::canonicalize(&path).expect("canonicalize tempdir");
            Self { path: canon }
        }

        fn write_cargo(&self, body: &str) {
            fs::write(self.path.join("Cargo.toml"), body).expect("write Cargo.toml");
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn git_defaults() -> GitDefaults {
        GitDefaults {
            url: "https://example.invalid/framework.git".to_string(),
            refspec: GitRef::Tag("v0.0.1".to_string()),
        }
    }

    /// Out-of-tree project pinning the framework with `git = ".." rev = ".."`
    /// must resolve to `Git`, NOT bail with the workspace error. This is
    /// the exact scenario `idealyst new` scaffolds and that AAS dev
    /// previously broke on.
    #[test]
    fn detect_out_of_tree_git_rev_yields_git_source() {
        let proj = TempProject::new("git-rev");
        proj.write_cargo(
            r#"
[package]
name = "demo"
version = "0.0.1"
edition = "2021"

[dependencies]
framework-core = { git = "https://github.com/IdealystIO/idealyst-native", rev = "deadbeef" }
"#,
        );

        let src = FrameworkSource::detect(&proj.path, git_defaults())
            .expect("detect must succeed on out-of-tree projects");

        match &src {
            FrameworkSource::Git { url, refspec } => {
                assert_eq!(url, "https://github.com/IdealystIO/idealyst-native");
                assert!(matches!(refspec, GitRef::Rev(s) if s == "deadbeef"));
            }
            FrameworkSource::Workspace { root } => panic!(
                "expected Git source, got Workspace {{ root: {} }} — \
                 the out-of-tree path is regressing back to workspace-required",
                root.display()
            ),
        }
        assert!(src.workspace_root().is_none());
    }

    /// Tag-pinned scaffolds (what `idealyst new` emits when HEAD has a
    /// release tag) must round-trip as `Tag`, not be re-emitted as `rev`.
    #[test]
    fn detect_out_of_tree_git_tag_preserves_tag_refspec() {
        let proj = TempProject::new("git-tag");
        proj.write_cargo(
            r#"
[package]
name = "demo"
version = "0.0.1"
edition = "2021"

[dependencies]
framework-core = { git = "https://github.com/IdealystIO/idealyst-native", tag = "v0.1.0" }
"#,
        );

        let src = FrameworkSource::detect(&proj.path, git_defaults()).expect("detect");
        match src {
            FrameworkSource::Git { refspec: GitRef::Tag(t), .. } => assert_eq!(t, "v0.1.0"),
            other => panic!("expected Git/Tag, got {other:?}"),
        }
    }

    /// Project with no `framework-core` dep at all → fall back to the
    /// CLI's compile-time git defaults. Covers the very-first
    /// `idealyst new` step before the scaffold's Cargo.toml is written.
    #[test]
    fn detect_falls_back_to_git_defaults_when_project_has_no_framework_dep() {
        let proj = TempProject::new("nodep");
        proj.write_cargo(
            r#"
[package]
name = "demo"
version = "0.0.1"
edition = "2021"

[dependencies]
"#,
        );

        let src = FrameworkSource::detect(&proj.path, git_defaults()).expect("detect");
        match src {
            FrameworkSource::Git { url, refspec: GitRef::Tag(t) } => {
                assert_eq!(url, "https://example.invalid/framework.git");
                assert_eq!(t, "v0.0.1");
            }
            other => panic!("expected Git defaults fallback, got {other:?}"),
        }
    }

    /// In Git mode the wrapper and target dirs must be project-local —
    /// AAS wrappers under `<project>/target/idealyst/...` is what makes
    /// the out-of-tree CLI work; any path leaking back to a workspace
    /// root re-introduces the in-tree requirement.
    #[test]
    fn git_mode_wrapper_and_target_dirs_are_project_local() {
        let proj = TempProject::new("paths");
        let src = FrameworkSource::Git {
            url: "https://example.invalid/framework.git".into(),
            refspec: GitRef::Rev("abc".into()),
        };
        assert_eq!(src.wrapper_root(&proj.path), proj.path.join("target/idealyst"));
        assert_eq!(src.cargo_target_dir(&proj.path), proj.path.join("target"));
        assert!(src.workspace_root().is_none());
    }

    /// `FrameworkSource::dep` for Git mode must emit a usable cargo
    /// dep table. The wrappers paste this directly into the generated
    /// `Cargo.toml`, so a malformed string would surface as a cargo
    /// parse error at first build.
    #[test]
    fn git_mode_dep_emits_cargo_table_with_refspec_and_features() {
        let src = FrameworkSource::Git {
            url: "https://example.invalid/framework.git".into(),
            refspec: GitRef::Rev("c77425a".into()),
        };
        let line = src.dep("crates/framework/core", &["hot-reload"]);
        assert!(line.contains("git = \"https://example.invalid/framework.git\""));
        assert!(line.contains("rev = \"c77425a\""));
        assert!(line.contains("features = [\"hot-reload\"]"));
        assert!(!line.contains("path ="));
    }

    /// `require_workspace_root` is the legacy fail-clear helper. The
    /// CLI's only remaining caller wraps it in `unwrap_or_else`; if a
    /// new caller adds it back as a hard requirement we want them to
    /// have to acknowledge this test's existence.
    #[test]
    fn require_workspace_root_errors_with_actionable_message_when_out_of_tree() {
        let proj = TempProject::new("require");
        let err = require_workspace_root(&proj.path)
            .expect_err("out-of-tree project must not resolve a framework workspace");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("idealyst framework workspace"),
            "error message should explain what was missing: {msg}"
        );
    }
}
