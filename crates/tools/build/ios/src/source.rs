//! Where wrapper Cargo.tomls source framework crates from.
//!
//! The build crates emit ephemeral wrapper Cargo.tomls for each
//! platform (`ios/wrapper`, `android/wrapper`, …). Those wrappers
//! depend on `runtime-core`, `backend-<platform>-*`, and friends.
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
use std::process::Command;

use anyhow::{Context, Result};

/// The framework crate whose resolution decides everything. Every
/// generated wrapper depends on it, and so does the user's app crate —
/// if those two resolve differently, nothing type-checks across the
/// boundary.
const FRAMEWORK_PKG: &str = "runtime-core";

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
    /// External project — wrapper deps go through git. Kept for
    /// projects scaffolded before the registry existed, and for anyone
    /// deliberately pinning a fork by rev.
    Git { url: String, refspec: GitRef },
    /// External project resolving from the idealyst cargo registry.
    /// This is the default for anything scaffolded now: a registry dep
    /// is version-keyed, so cargo reuses the compiled artifact of any
    /// crate whose version did not move between releases. A git pin
    /// cannot — its source id carries the commit, so every crate in the
    /// graph gets a new PackageId on every bump and the whole framework
    /// rebuilds.
    Registry { registry: String, version: String },
}

/// Compile-time registry defaults baked into the CLI binary, the
/// registry counterpart of [`GitDefaults`].
#[derive(Clone, Debug)]
pub struct RegistryDefaults {
    /// Name the registry is spelled as in `.cargo/config.toml`.
    pub name: String,
    /// Sparse index URL, needed to write that `.cargo/config.toml`.
    pub index: String,
    /// Version requirement to scaffold with — a caret on major.minor.
    pub version: String,
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
    /// 3. **Ask cargo** (`cargo metadata`) where the project's
    ///    `runtime-core` actually resolves, and mirror that. This is
    ///    the most important branch in practice — it makes the user's
    ///    real dependency graph authoritative, so the generated
    ///    wrapper picks up the same `runtime-core` the app crate uses
    ///    and cargo can unify them. Without it, a CLI re-installed
    ///    against a different commit than the project was scaffolded
    ///    against would generate a wrapper pointing at a different rev
    ///    → cargo treats them as two `runtime-core` instances →
    ///    `Element` type mismatch at link.
    /// 3b. If cargo can't be run (or reports nothing useful), fall
    ///    back to parsing the project's `Cargo.toml` by hand —
    ///    including `{ workspace = true }` inheritance.
    /// 4. Fall back to git, using the supplied defaults (only used
    ///    for fresh `idealyst new` scaffolding where there isn't a
    ///    project `Cargo.toml` yet).
    ///
    /// # Why cargo is asked rather than the manifest read
    ///
    /// Step 3b is a partial reimplementation of cargo's resolver, and
    /// every dependency form it doesn't model is a silent fallback to
    /// step 4 — i.e. a wrapper pinned to a different framework than
    /// the app, i.e. the `Element` mismatch this whole function exists
    /// to prevent. That has bitten four separate ways (relative
    /// `project_dir`, the `crates/framework/core` → `crates/runtime/core`
    /// reorg, relative `path` deps, and `{ workspace = true }`
    /// inheritance), and `[patch]`, `[replace]`, `paths` overrides and
    /// vendored sources are all still unmodelled. `cargo metadata`
    /// reports the *resolved* source after all of those, so step 3
    /// is correct by construction for dep forms nobody here has
    /// thought of yet.
    pub fn detect(project_dir: &Path, git: GitDefaults, reg: RegistryDefaults) -> Result<Self> {
        let resolved = Self::detect_inner(project_dir, git, reg)?;
        eprintln!("idealyst: framework source — {}", resolved.describe());
        Ok(resolved)
    }

    fn detect_inner(
        project_dir: &Path,
        git: GitDefaults,
        reg: RegistryDefaults,
    ) -> Result<Self> {
        let _ = &git;
        if let Ok(p) = std::env::var("IDEALYST_FRAMEWORK_PATH") {
            let root = PathBuf::from(&p);
            if !is_framework_root(&root) {
                anyhow::bail!(
                    "IDEALYST_FRAMEWORK_PATH={} does not look like an idealyst-native \
                     checkout (missing crates/runtime/core/Cargo.toml)",
                    root.display(),
                );
            }
            return Ok(Self::Workspace { root });
        }
        if let Some(root) = find_framework_workspace(project_dir) {
            return Ok(Self::Workspace { root });
        }
        // A duplicate-`runtime-core` graph is a hard error, not a
        // fallback — see `framework_source_from_metadata`.
        if let Some(from_cargo) = resolve_via_cargo_metadata(project_dir)? {
            return Ok(from_cargo);
        }
        if let Some(from_project) = read_project_framework_dep(project_dir) {
            return Ok(from_project);
        }
        // Nothing told us how this project pins the framework — this is
        // fresh `idealyst new` scaffolding, with no Cargo.toml yet. Scaffold
        // against the registry: a git pin would make every framework release
        // rebuild the consumer's entire graph.
        Ok(Self::Registry { registry: reg.name, version: reg.version })
    }

    /// One-line summary for the "which framework am I building
    /// against?" log line `detect` emits.
    ///
    /// The git-defaults fallback used to be indistinguishable from a
    /// deliberate git pin, so a misdetected project looked identical
    /// to a correctly-detected one right up until the wasm build died
    /// with a type error naming `Element` twice. Printing the resolved
    /// source makes that visible in one line of `idealyst dev` output.
    pub fn describe(&self) -> String {
        match self {
            Self::Workspace { root } => format!("path {}", root.display()),
            Self::Git { url, refspec } => {
                let (key, value) = refspec.as_pair();
                format!("git {url} ({key} {value})")
            }
            Self::Registry { registry, version } => {
                format!("registry {registry} ({version})")
            }
        }
    }

    /// True if this source is an in-tree workspace.
    pub fn is_workspace(&self) -> bool {
        matches!(self, Self::Workspace { .. })
    }

    /// Workspace root, when we have one. Some commands (`build-runtime-server`,
    /// `dev`) need this because they reach into the workspace's
    /// `target/` for sidecar binaries or shared caches.
    pub fn workspace_root(&self) -> Option<&Path> {
        match self {
            Self::Workspace { root } => Some(root.as_path()),
            Self::Git { .. } | Self::Registry { .. } => None,
        }
    }

    /// Root for ephemeral wrapper crates. In-tree projects share the
    /// framework workspace's `target/idealyst/` so cargo's build
    /// cache stays warm across `examples/*` rebuilds. External
    /// projects use their own `<project>/target/idealyst/`.
    pub fn wrapper_root(&self, project_dir: &Path) -> PathBuf {
        match self {
            Self::Workspace { root } => root.join("target/idealyst"),
            Self::Git { .. } | Self::Registry { .. } => project_dir.join("target/idealyst"),
        }
    }

    /// Cargo target dir to redirect the wrapper crate's build output
    /// to via its `.cargo/config.toml`. Same in-tree-vs-external
    /// distinction as `wrapper_root`.
    pub fn cargo_target_dir(&self, project_dir: &Path) -> PathBuf {
        match self {
            Self::Workspace { root } => root.join("target"),
            Self::Git { .. } | Self::Registry { .. } => project_dir.join("target"),
        }
    }

    /// Render a `[patch."<git-url>"]` block redirecting every
    /// framework crate to its local path. Required in the generated
    /// wrapper crates so the user's git-pinned `runtime-core`
    /// (and transitive deps) resolves to the **same physical crate**
    /// the wrapper itself uses — without it, cargo treats the
    /// wrapper's path-dep and the user's git-dep as two separate
    /// `runtime_core` instances, producing inscrutable "expected
    /// `Element` but found `Element`" type errors at the
    /// wrapper-→-user-crate boundary.
    ///
    /// Returns an empty string in `Git` mode (wrapper and user
    /// already use the same git rev — no redirect needed).
    pub fn patch_block(&self) -> String {
        let Self::Workspace { root } = self else {
            return String::new();
        };
        // Default git URL the scaffold uses. Anyone running with a
        // forked / mirrored URL via `IDEALYST_FRAMEWORK_GIT_URL`
        // would need a custom patch block; we'll wire that through
        // when someone actually hits the case.
        let url = "https://github.com/IdealystIO/idealyst-native";
        let mut out = format!("\n[patch.\"{}\"]\n", url);
        // Patch EVERY consumable framework crate (workspace members under
        // `crates/`), not a hand-picked subset. The wrapper is its own
        // workspace, so the user's git-pinned deps resolve inside it —
        // any framework crate MISSING from this block resolves from git
        // while its siblings resolve from the local checkout, and the
        // graph ends up with two `runtime_scene`/`runtime_shared`/…
        // instances ("expected `Element`, found `Element`"). A hardcoded
        // list rotted exactly that way (it predated the runtime-v2 split
        // and listed 9 crates). Unused entries only cost a cargo warning.
        let crates = Self::workspace_framework_crates(root);
        for (name, dir) in &crates {
            out.push_str(&format!("{name} = {{ path = \"{}\" }}\n", dir.display()));
        }
        // Projects scaffolded since the registry landed pin the framework by
        // version rather than by git, so the redirect has to cover that source
        // too. Both sections are emitted because one wrapper may be built for
        // either kind of project; an unused `[patch]` costs a cargo warning,
        // whereas a MISSING one costs two `runtime_scene` instances and
        // "expected `Element`, found `Element`".
        let registry = std::env::var("IDEALYST_REGISTRY_NAME")
            .unwrap_or_else(|_| REGISTRY_NAME.to_string());
        out.push_str(&format!("\n[patch.{registry}]\n"));
        for (name, dir) in &crates {
            out.push_str(&format!("{name} = {{ path = \"{}\" }}\n", dir.display()));
        }
        out
    }

    /// Enumerate the framework workspace's consumable crates —
    /// `(package name, manifest dir)` for every workspace member whose
    /// directory sits under `<root>/crates/`. Examples, websites and
    /// benchmarks are members too but are never git-dep'd by consumers,
    /// so they're skipped to keep the generated patch block readable.
    ///
    /// Uses `cargo metadata --no-deps` (fast; no network, no resolve of
    /// the full graph). If cargo can't be run — a broken member manifest
    /// mid-edit, say — fall back to a minimal static list so wrapper
    /// generation still works for the common crates.
    fn workspace_framework_crates(root: &Path) -> Vec<(String, PathBuf)> {
        let run = || -> Result<Vec<(String, PathBuf)>> {
            let out = Command::new("cargo")
                .arg("metadata")
                .arg("--no-deps")
                .arg("--format-version")
                .arg("1")
                .arg("--manifest-path")
                .arg(root.join("Cargo.toml"))
                .output()
                .context("running `cargo metadata` on the framework workspace")?;
            anyhow::ensure!(out.status.success(), "cargo metadata failed");
            let meta: serde_json::Value = serde_json::from_slice(&out.stdout)
                .context("parsing `cargo metadata` output")?;
            let crates_root = root.join("crates");
            let mut crates: Vec<(String, PathBuf)> = meta["packages"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|pkg| {
                    let name = pkg["name"].as_str()?;
                    let manifest = Path::new(pkg["manifest_path"].as_str()?);
                    let dir = manifest.parent()?;
                    dir.starts_with(&crates_root)
                        .then(|| (name.to_string(), dir.to_path_buf()))
                })
                .collect();
            crates.sort();
            Ok(crates)
        };
        match run() {
            Ok(crates) if !crates.is_empty() => crates,
            _ => [
                ("runtime-core", "crates/runtime/core"),
                ("runtime-shared", "crates/runtime/shared"),
                ("runtime-scene", "crates/runtime/scene"),
                ("runtime-vocabulary", "crates/runtime/vocabulary"),
                ("runtime-world", "crates/runtime/world"),
                ("runtime-layout", "crates/runtime/layout"),
                ("runtime-macros", "crates/runtime/macros"),
                ("dev-hot", "crates/dev/hot"),
                ("mcp-catalog", "crates/mcp/catalog"),
                ("wire", "crates/dev/wire"),
                ("dev-client", "crates/dev/client"),
                ("dev-server", "crates/dev/server"),
                ("backend-web", "crates/backend/web"),
                ("backend-ssr", "crates/backend/ssr"),
                ("css", "crates/css"),
                ("idea-ui", "crates/ui/idea-ui"),
                ("idea-theme", "crates/ui/idea-theme"),
            ]
            .into_iter()
            .map(|(n, s)| (n.to_string(), root.join(s)))
            .collect(),
        }
    }

    /// Render a single dependency line for a framework crate.
    ///
    /// `subpath` is the directory under the workspace root (e.g.
    /// `crates/framework/core`) used in workspace mode. In git mode
    /// the package name is taken from the TOML key the caller uses —
    /// cargo accepts `runtime-core = { git = "...", rev = "..." }`
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
            // `registry` is not optional. Most framework crates have short
            // names that belong to unrelated packages on crates.io — `css`,
            // `wire`, `net`, `table`, `menu`, `video`, `canvas` — so a bare
            // version requirement resolves to a stranger's crate.
            Self::Registry { registry, version } => format!(
                "{{ version = \"{}\", registry = \"{}\"{} }}",
                version, registry, features_clause,
            ),
        }
    }

    /// The registry this source resolves from, when it is a registry.
    pub fn registry_name(&self) -> Option<&str> {
        match self {
            Self::Registry { registry, .. } => Some(registry.as_str()),
            _ => None,
        }
    }
}

/// `--remap-path-prefix` flags that keep build-machine absolute paths out
/// of a shipped binary.
///
/// **Why this is needed at all.** Every `panic!` / `unwrap` / `expect` /
/// bounds check emits a `&'static core::panic::Location` holding
/// `file!()`, and the panic handler reads it to build its message. That
/// makes the path *live `.rodata`*, not debug info — `wasm-opt
/// --strip-debug` and `strip = "..."` provably cannot remove it (a
/// release web bundle keeps them while retaining only ~157 bytes of
/// custom sections). Without remapping, a bundle built on a laptop ships
/// the developer's home directory, username, toolchain version, and full
/// dependency inventory to every client that loads it.
///
/// **Why the framework paths are absolute in the first place.** The
/// generated wrappers live under `target/idealyst/<app>/<platform>/wrapper`
/// with their own `[workspace]`, and [`FrameworkSource::dep`] spells each
/// framework dep as an absolute `path = "…"`. Cargo hands rustc those
/// absolute paths, so `file!()` expands absolute in every framework crate.
/// Building the same crates as ordinary workspace members (e.g. the
/// `benchmark/` wasm) yields repo-relative paths instead — the wrapper
/// design is what promotes them, so the wrapper build is where this is
/// fixed.
///
/// Four prefixes are covered:
///
/// 1. the framework workspace root (`Workspace` mode only — `Git`-mode
///    checkouts land under `CARGO_HOME` and are caught by 3),
/// 2. the app being built,
/// 3. `CARGO_HOME` — crates.io registry sources *and* git checkouts,
/// 4. the rustc sysroot — `std`/`core`/`alloc` sources.
///
/// **Order is load-bearing.** rustc's `map_prefix` scans the mapping list
/// in reverse and takes the first hit, so the LAST matching entry wins.
/// The app root is pushed after the framework root precisely so an in-tree
/// app (`examples/baseline`, which sits *inside* the framework workspace)
/// maps to `/app` rather than `/idealyst` — the more specific prefix has
/// to be able to win.
///
/// Release builds only. Dev builds keep real paths so a panic in the
/// terminal or devtools stays clickable.
///
/// **Diagnostics are remapped too.** On stable rustc `--remap-path-prefix`
/// is all-or-nothing — it rewrites compiler diagnostics as well as the
/// embedded strings (scoping it to object code needs the unstable
/// `-Zremap-path-scope`). So a *failing* release build reports
/// `/app/src/lib.rs:19` rather than a clickable absolute path. Dev builds
/// are unaffected, which is where iteration happens; set
/// `IDEALYST_NO_PATH_REMAP=1` to opt out for a one-off release build you
/// need to debug.
pub fn remap_path_flags(source: &FrameworkSource, project_root: &Path) -> Vec<String> {
    if remap_disabled(std::env::var("IDEALYST_NO_PATH_REMAP").ok().as_deref()) {
        return Vec::new();
    }
    let mut prefixes: Vec<(PathBuf, &str)> = Vec::new();
    if let FrameworkSource::Workspace { root } = source {
        prefixes.push((root.clone(), "/idealyst"));
    }
    prefixes.push((project_root.to_path_buf(), "/app"));
    if let Some(cargo_home) = cargo_home() {
        prefixes.push((cargo_home, "/cargo"));
    }
    if let Some(sysroot) = rustc_sysroot() {
        prefixes.push((sysroot, "/rust"));
    }
    prefixes
        .into_iter()
        .map(|(from, to)| format!("--remap-path-prefix={}={}", from.display(), to))
        .collect()
}

/// Whether `IDEALYST_NO_PATH_REMAP` asks for path remapping to be skipped.
///
/// Taken as a parameter rather than read here so it can be tested without
/// mutating process-global env state (Rust runs tests in parallel, so an
/// env-var-setting test corrupts its neighbours).
///
/// Set-but-empty counts as unset — that is what `FOO= cmd` produces, and it
/// reads as "no opinion" rather than "opt out". `0` / `false` / `no` are
/// honored as explicit negatives so `IDEALYST_NO_PATH_REMAP=0` doesn't
/// silently do the opposite of what it says.
fn remap_disabled(value: Option<&str>) -> bool {
    match value.map(str::trim) {
        None | Some("") => false,
        Some(v) => !matches!(v.to_ascii_lowercase().as_str(), "0" | "false" | "no"),
    }
}

/// `CARGO_HOME`, or the conventional `~/.cargo` when unset.
fn cargo_home() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("CARGO_HOME") {
        if !explicit.is_empty() {
            return Some(PathBuf::from(explicit));
        }
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo"))
}

/// The active toolchain's sysroot, under which `lib/rustlib/src/rust/library`
/// holds the `std`/`core`/`alloc` sources whose paths reach panic `Location`s.
///
/// Asked of `rustc` rather than assumed to be `~/.rustup/...` so distro and
/// non-rustup toolchains are covered too. Returns `None` if `rustc` can't be
/// run — the caller simply emits one fewer remap.
fn rustc_sysroot() -> Option<PathBuf> {
    let out = std::process::Command::new("rustc")
        .arg("--print")
        .arg("sysroot")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8(out.stdout).ok()?;
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    Some(PathBuf::from(path))
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
    // A relative `start` (e.g. `.`) has no real ancestors to walk —
    // `Path::ancestors()` yields only the literal components, never the
    // actual filesystem parents — so a caller that forgot to canonicalize
    // would silently fail detection and the build would fall through to
    // git-mode. That produces TWO `runtime_core` crate instances (one from
    // the git rev, one from the local path) and every wrapper→user-crate
    // bridge fails with "expected `Element`, found `Element`" at the `mount`
    // bound. Anchor a relative path to CWD first so the walk reaches the
    // real root regardless of caller hygiene.
    let anchored;
    let start = if start.is_absolute() {
        start
    } else {
        anchored = std::env::current_dir().ok()?.join(start);
        anchored.as_path()
    };
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
    // Post-reorg the runtime crate lives at `crates/runtime/core`
    // (was `crates/framework/core`). Probe the new path to
    // re-enable workspace-mode detection for in-tree projects —
    // without this, examples like `examples/welcome` fall through
    // to git-mode and produce two distinct `runtime_core` crate
    // instances (one from the git rev, one from the local path),
    // failing every wrapper→user-crate type bridge.
    if !root.join("crates/runtime/core/Cargo.toml").is_file() {
        return false;
    }
    let content = fs::read_to_string(&cargo).unwrap_or_default();
    content.contains("[workspace]")
}

/// Ask cargo to resolve the project's graph and report where
/// `runtime-core` actually comes from.
///
/// `cargo metadata` runs the real resolver, so its answer already
/// accounts for workspace inheritance, `[patch]`, `[replace]`,
/// `.cargo/config.toml` `paths` overrides, vendored sources and
/// whatever cargo adds next — none of which the hand-rolled manifest
/// parser below models.
///
/// Returns `Ok(None)` (→ caller falls through) when cargo can't run or
/// reports nothing usable, which covers the `idealyst new` case where
/// there is no manifest yet. Returns `Err` only for a graph that is
/// *provably* broken, i.e. more than one `runtime-core`.
fn resolve_via_cargo_metadata(project_dir: &Path) -> Result<Option<FrameworkSource>> {
    let manifest = project_dir.join("Cargo.toml");
    if !manifest.is_file() {
        return Ok(None);
    }
    // `CARGO` is set when we're invoked from a cargo subcommand, and
    // points at the exact toolchain's binary — preferable to whatever
    // `cargo` resolves to on PATH under rustup shims.
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let out = Command::new(cargo)
        .arg("metadata")
        .arg("--format-version")
        .arg("1")
        .arg("--manifest-path")
        .arg(&manifest)
        .current_dir(project_dir)
        .output();
    let out = match out {
        Ok(out) => out,
        Err(e) => {
            eprintln!("idealyst: could not run `cargo metadata` ({e}); falling back to reading Cargo.toml");
            return Ok(None);
        }
    };
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let first = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        eprintln!("idealyst: `cargo metadata` failed ({first}); falling back to reading Cargo.toml");
        return Ok(None);
    }
    let meta: serde_json::Value = match serde_json::from_slice(&out.stdout) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("idealyst: could not parse `cargo metadata` output ({e}); falling back to reading Cargo.toml");
            return Ok(None);
        }
    };
    framework_source_from_metadata(&meta)
}

/// Pull the framework source out of a parsed `cargo metadata` document.
///
/// Split from [`resolve_via_cargo_metadata`] so the interesting logic is
/// testable without shelling out to cargo.
fn framework_source_from_metadata(meta: &serde_json::Value) -> Result<Option<FrameworkSource>> {
    let Some(packages) = meta.get("packages").and_then(|p| p.as_array()) else {
        return Ok(None);
    };
    let hits: Vec<&serde_json::Value> = packages
        .iter()
        .filter(|p| p.get("name").and_then(|n| n.as_str()) == Some(FRAMEWORK_PKG))
        .collect();

    match hits.as_slice() {
        [] => Ok(None),
        [only] => Ok(package_to_source(only)),
        many => {
            // Two `runtime-core`s in one graph can never work: cargo
            // compiles both, their types are nominally distinct, and
            // the app↔wrapper boundary fails to unify. Today that
            // surfaces as "expected `Element`, found `Element`" from
            // deep inside generated code. Say it here instead, while
            // we still know the two sources by name.
            let mut listed: Vec<String> = many.iter().map(|p| describe_metadata_pkg(p)).collect();
            listed.sort();
            listed.dedup();
            anyhow::bail!(
                "this project resolves {} different `{FRAMEWORK_PKG}` crates:\n  {}\n\n\
                 cargo treats those as unrelated crates, so every type crossing the \
                 generated wrapper → app boundary will fail to unify (the classic \
                 \"expected `Element`, found `Element`\"). Point every crate in the \
                 workspace at ONE framework source, or add a `[patch]` unifying them.",
                many.len(),
                listed.join("\n  "),
            )
        }
    }
}

/// `"<source> (<manifest path>)"` for the duplicate-crate error.
fn describe_metadata_pkg(pkg: &serde_json::Value) -> String {
    let source = pkg
        .get("source")
        .and_then(|s| s.as_str())
        .unwrap_or("local path");
    let manifest = pkg
        .get("manifest_path")
        .and_then(|s| s.as_str())
        .unwrap_or("<unknown manifest>");
    format!("{source} ({manifest})")
}

/// Map one resolved `cargo metadata` package to a `FrameworkSource`.
///
/// `None` means "cargo resolved it to something a wrapper dep can't
/// spell" (a registry release, say) — the caller falls through to the
/// git defaults exactly as it did before this branch existed.
fn package_to_source(pkg: &serde_json::Value) -> Option<FrameworkSource> {
    match pkg.get("source").and_then(|s| s.as_str()) {
        // `source: null` is cargo's encoding for a path dependency —
        // either a workspace member or a `path = "..."` dep. The
        // manifest path is absolute and already normalized, which is
        // what the deeper wrapper Cargo.toml needs.
        None => {
            let manifest = pkg.get("manifest_path").and_then(|s| s.as_str())?;
            let core_dir = Path::new(manifest).parent()?;
            // Strip `crates/runtime/core` to recover the workspace
            // root. `is_framework_root` is the authoritative check —
            // if the strip lands somewhere that isn't the framework
            // workspace, fall through rather than emit a bad path.
            let root = core_dir.ancestors().nth(3)?;
            is_framework_root(root).then(|| FrameworkSource::Workspace {
                root: root.to_path_buf(),
            })
        }
        Some(s) if s.starts_with("git+") => parse_git_source(s),
        // `registry+sparse+https://…` — a published `runtime-core`. Mirror
        // the app's own pin so the wrapper resolves the identical crate;
        // the version comes from the resolved package, narrowed to
        // major.minor so a patch release does not split the graph.
        Some(s) if s.contains("sparse+") || s.starts_with("registry+") => {
            let version = pkg.get("version").and_then(|v| v.as_str())?;
            let (major, minor) = {
                let mut it = version.split('.');
                (it.next()?, it.next()?)
            };
            Some(FrameworkSource::Registry {
                registry: REGISTRY_NAME.to_string(),
                version: format!("{major}.{minor}"),
            })
        }
        Some(_) => None,
    }
}

/// The registry name a generated Cargo.toml should spell.
///
/// Cargo's metadata reports the index URL, never the local alias, and offers
/// no reverse mapping — the alias lives only in the consumer's
/// `.cargo/config.toml`. The framework's registry is the sole alternative
/// registry these projects use, and the CLI writes that config itself, so the
/// name is ours to fix.
pub const REGISTRY_NAME: &str = "idealyst";

/// Sparse index the framework registry is served from.
const REGISTRY_INDEX: &str = "sparse+https://crates.idealyst.io/index/";

/// The `[registries.…]` stanza a generated wrapper needs in its own
/// `.cargo/config.toml`.
///
/// Cargo merges config files up the directory tree, so a wrapper under a
/// project that already defines the registry would inherit it — but a wrapper
/// generated for a project that pins the framework by git would not, and
/// [`FrameworkSource::patch_block`] emits a `[patch.idealyst]` section
/// unconditionally. An undefined registry name there is a hard error
/// ("registry index was not found"), so every wrapper defines it itself
/// rather than hoping an ancestor did.
pub fn registry_config_block() -> String {
    let index = std::env::var("IDEALYST_REGISTRY_INDEX")
        .unwrap_or_else(|_| REGISTRY_INDEX.to_string());
    let name = std::env::var("IDEALYST_REGISTRY_NAME")
        .unwrap_or_else(|_| REGISTRY_NAME.to_string());
    format!("\n[registries.{name}]\nindex = \"{index}\"\n")
}

/// Parse a cargo git source id: `git+<url>[?<ref>]#<sha>`.
///
/// The refspec **form** matters as much as its value: cargo keys crate
/// identity on the whole source id, so a wrapper pinning `rev = <sha>`
/// against an app pinning `tag = v1.2.5` yields two crates even though
/// both name the same commit. The user's form is preserved verbatim.
///
/// A ref-less `git+<url>#<sha>` (the app wrote a bare `git = "..."`,
/// tracking the default branch) returns `None`: [`GitRef`] has no way
/// to spell "no refspec", and guessing one would produce exactly the
/// mismatch this function exists to avoid. Falling through to the git
/// defaults is the pre-existing behavior for that case.
fn parse_git_source(source: &str) -> Option<FrameworkSource> {
    let rest = source.strip_prefix("git+")?;
    let base = rest.split_once('#').map(|(b, _sha)| b).unwrap_or(rest);
    let (url, query) = base.split_once('?')?;
    // Query is `key=value[&key=value...]`; cargo emits one ref key,
    // but scan them all rather than assume position.
    let refspec = query.split('&').find_map(|part| {
        let (key, value) = part.split_once('=')?;
        let value = percent_decode(value);
        match key {
            "tag" => Some(GitRef::Tag(value)),
            "branch" => Some(GitRef::Branch(value)),
            "rev" => Some(GitRef::Rev(value)),
            _ => None,
        }
    })?;
    Some(FrameworkSource::Git { url: url.to_string(), refspec })
}

/// Minimal `%XX` decoding for git source query values.
///
/// Branch names legitimately contain `/` (`release/1.2`), which cargo
/// percent-encodes into the source id. Emitting the encoded form back
/// into a Cargo.toml would fail to check out. Malformed escapes are
/// passed through untouched rather than dropped.
fn percent_decode(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(byte) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// Parse `<project>/Cargo.toml` and extract the `runtime-core` dep
/// as a `FrameworkSource`. Fallback for when `cargo metadata` can't
/// run; supports the common forms:
///
/// - `runtime-core = { git = "<url>", rev = "<sha>" }` → `Git`.
/// - `runtime-core = { git = "<url>", branch = "<b>" }` → `Git`
///   with rev set to the branch name (cargo accepts branches).
/// - `runtime-core = { path = "/p/to/framework/core" }` → strip
///   `/crates/runtime/core` to get the workspace root and emit
///   `Workspace`. (Falls through to git defaults if the path
///   doesn't end with the expected suffix.)
/// - `runtime-core = { workspace = true }` → re-read the real spec
///   from the workspace root's `[workspace.dependencies]`.
///
/// Returns `None` if the project has no `runtime-core` dep, or
/// the dep is in a form we can't interpret (e.g. plain version
/// string, custom registries). Callers fall back to the git
/// defaults in those cases.
fn read_project_framework_dep(project_dir: &Path) -> Option<FrameworkSource> {
    let raw = fs::read_to_string(project_dir.join("Cargo.toml")).ok()?;
    let parsed: toml::Value = toml::from_str(&raw).ok()?;
    let table = parsed
        .get("dependencies")?
        .get(FRAMEWORK_PKG)?
        .as_table()?;

    // `{ workspace = true }` carries no spec of its own — the real one
    // lives in the workspace root's `[workspace.dependencies]`, and any
    // relative `path` in it is relative to THAT manifest, not this one.
    if table.get("workspace").and_then(|v| v.as_bool()) == Some(true) {
        let root_dir = find_cargo_workspace_root(project_dir)?;
        let root_raw = fs::read_to_string(root_dir.join("Cargo.toml")).ok()?;
        let root_parsed: toml::Value = toml::from_str(&root_raw).ok()?;
        let inherited = root_parsed
            .get("workspace")?
            .get("dependencies")?
            .get(FRAMEWORK_PKG)?
            .as_table()?;
        return framework_dep_from_table(inherited, &root_dir);
    }

    framework_dep_from_table(table, project_dir)
}

/// Interpret one `runtime-core` dep table. `base_dir` is the directory
/// holding the manifest the table was written in — a relative `path`
/// resolves against it, per cargo's rules.
fn registry_dep_from_table(dep: &toml::Table) -> Option<FrameworkSource> {
    let registry = dep.get("registry")?.as_str()?;
    let version = dep.get("version")?.as_str()?;
    // Narrow to major.minor: the wrapper must not pin a patch the app has
    // not, or cargo resolves two `runtime-core`s and nothing type-checks
    // across the wrapper boundary.
    let mut it = version.trim_start_matches(['^', '~', '=']).split('.');
    let (major, minor) = (it.next()?, it.next().unwrap_or("0"));
    Some(FrameworkSource::Registry {
        registry: registry.to_string(),
        version: format!("{major}.{minor}"),
    })
}

fn framework_dep_from_table(table: &toml::Table, base_dir: &Path) -> Option<FrameworkSource> {
    if let Some(path_str) = table.get("path").and_then(|v| v.as_str()) {
        // Resolve against `base_dir` and canonicalize so the recovered
        // workspace root is ABSOLUTE. This matters because the
        // generated wrapper Cargo.toml lives deeper
        // (`<root>/target/idealyst/<proj>/<platform>/wrapper`) and cargo
        // resolves a wrapper `path = ...` dep relative to the wrapper
        // file — a relative root copied verbatim (e.g. `../idealyst-native`)
        // would resolve to the wrong place and the build fails with
        // "failed to load manifest for dependency `backend-web`".
        let raw_core = PathBuf::from(path_str);
        let core_path = if raw_core.is_absolute() {
            raw_core
        } else {
            base_dir.join(&raw_core)
        };
        // canonicalize() also collapses `..` segments; fall back to the
        // un-canonicalized join if the path doesn't exist yet.
        let core_path = core_path.canonicalize().unwrap_or(core_path);
        // Strip `crates/runtime/core` (3 ancestors up) to recover
        // the workspace root. `is_framework_root` is the
        // authoritative check — if the strip lands somewhere that
        // doesn't look like the framework workspace, fall through.
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

    // A registry dep before a git one: `{ version, registry }` carries no
    // `git` key, so this only fires when the project pins the registry.
    if let Some(reg) = registry_dep_from_table(table) {
        return Some(reg);
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

/// Nearest ancestor manifest declaring `[workspace]` — the root a
/// `{ workspace = true }` dep inherits from.
///
/// Unlike [`find_framework_workspace`] this does NOT require the
/// directory to be a framework checkout; it's looking for the *user's*
/// workspace. Starts at `start` itself, since a workspace root can also
/// be a package (the `[workspace]` + `[package]` combo).
fn find_cargo_workspace_root(start: &Path) -> Option<PathBuf> {
    let anchored;
    let start = if start.is_absolute() {
        start
    } else {
        anchored = std::env::current_dir().ok()?.join(start);
        anchored.as_path()
    };
    for ancestor in start.ancestors() {
        let manifest = ancestor.join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let Ok(raw) = fs::read_to_string(&manifest) else {
            continue;
        };
        let Ok(parsed) = toml::from_str::<toml::Value>(&raw) else {
            continue;
        };
        if parsed.get("workspace").is_some() {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

/// Back-compat thin wrapper around the legacy `find_workspace_root`
/// semantics — kept so call sites that genuinely need an in-tree
/// workspace (runtime-server mode, `dev` server) can fail clearly when run
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
    //! Regression coverage for the out-of-tree runtime-server / build path.
    //!
    //! The original failure mode: a user CLI installed via
    //! `cargo install idealyst-cli` was running `idealyst dev --aas`
    //! against a project that lived nowhere near an `idealyst-native/`
    //! checkout. The legacy wrappers walked up looking for a framework
    //! workspace and bailed with "could not find the idealyst framework
    //! workspace…". The current behavior is that `FrameworkSource::detect`
    //! falls back to reading the project's own `runtime-core` git dep,
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

    fn registry_defaults() -> RegistryDefaults {
        RegistryDefaults {
            name: "idealyst".into(),
            index: "sparse+https://crates.idealyst.io/index/".into(),
            version: "1.5".into(),
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
    /// the exact scenario `idealyst new` scaffolds and that runtime-server dev
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
runtime-core = { git = "https://github.com/IdealystIO/idealyst-native", rev = "deadbeef" }
"#,
        );

        let src = FrameworkSource::detect(&proj.path, git_defaults(), registry_defaults())
            .expect("detect must succeed on out-of-tree projects");

        match &src {
            FrameworkSource::Git { url, refspec } => {
                assert_eq!(url, "https://github.com/IdealystIO/idealyst-native");
                assert!(matches!(refspec, GitRef::Rev(s) if s == "deadbeef"));
            }
            FrameworkSource::Registry { registry, version } => panic!(
                "expected the project's git pin to win, got registry {registry} {version}"
            ),
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
runtime-core = { git = "https://github.com/IdealystIO/idealyst-native", tag = "v0.1.0" }
"#,
        );

        let src = FrameworkSource::detect(&proj.path, git_defaults(), registry_defaults()).expect("detect");
        match src {
            FrameworkSource::Git { refspec: GitRef::Tag(t), .. } => assert_eq!(t, "v0.1.0"),
            other => panic!("expected Git/Tag, got {other:?}"),
        }
    }

    /// Project with no `runtime-core` dep at all → fall back to the
    /// CLI's compile-time git defaults. Covers the very-first
    /// `idealyst new` step before the scaffold's Cargo.toml is written.
    #[test]
    fn detect_falls_back_to_the_registry_when_project_has_no_framework_dep() {
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

        let src = FrameworkSource::detect(&proj.path, git_defaults(), registry_defaults())
            .expect("detect");
        match src {
            FrameworkSource::Registry { registry, version } => {
                assert_eq!(registry, "idealyst");
                assert_eq!(version, "1.5");
            }
            other => panic!("expected the registry fallback, got {other:?}"),
        }
    }

    /// A project that already pins the framework by version + registry keeps
    /// that pin, so the generated wrapper resolves the identical crate. The
    /// version is narrowed to major.minor — pinning a patch the app has not
    /// would give cargo two `runtime-core`s and nothing would type-check
    /// across the wrapper boundary.
    #[test]
    fn detect_preserves_a_projects_registry_pin() {
        let proj = TempProject::new("regdep");
        proj.write_cargo(
            r#"
[package]
name = "demo"
version = "0.0.1"
edition = "2021"

[dependencies]
runtime-core = { version = "1.5.2", registry = "idealyst" }
"#,
        );

        let src = FrameworkSource::detect(&proj.path, git_defaults(), registry_defaults())
            .expect("detect");
        match src {
            FrameworkSource::Registry { registry, version } => {
                assert_eq!(registry, "idealyst");
                assert_eq!(version, "1.5", "patch must be dropped from the requirement");
            }
            other => panic!("expected the project's registry pin, got {other:?}"),
        }
    }

    /// The emitted dep table must always name the registry. Most framework
    /// crates share a name with an unrelated crates.io package, so a bare
    /// version requirement silently resolves to a stranger's crate.
    #[test]
    fn registry_dep_names_the_registry_and_carries_features() {
        let src = FrameworkSource::Registry {
            registry: "idealyst".into(),
            version: "1.5".into(),
        };
        assert_eq!(
            src.dep("crates/runtime/core", &["async-driver"]),
            r#"{ version = "1.5", registry = "idealyst", features = ["async-driver"] }"#
        );
        assert_eq!(
            src.dep("crates/css", &[]),
            r#"{ version = "1.5", registry = "idealyst" }"#
        );
    }

    /// In Git mode the wrapper and target dirs must be project-local —
    /// runtime-server wrappers under `<project>/target/idealyst/...` is what makes
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

    /// A project that path-deps the framework with a RELATIVE path must
    /// resolve to a `Workspace` whose root is ABSOLUTE. The generated
    /// wrapper Cargo.toml lives deeper than the project, and cargo
    /// resolves its `path = ...` deps relative to the wrapper file — a
    /// relative root copied verbatim resolved to a non-existent
    /// directory and failed the web/ios wrapper build with "failed to
    /// load manifest for dependency `backend-web`".
    #[test]
    fn relative_path_dep_resolves_to_absolute_workspace_root() {
        // Lay out a fake framework checkout and a sibling project under a
        // shared temp parent, so the project's dep can be `../fw/...`.
        let parent = TempProject::new("relpath-parent");
        let fw = parent.path.join("fw");
        fs::create_dir_all(fw.join("crates/runtime/core")).expect("mk fw tree");
        fs::write(fw.join("Cargo.toml"), "[workspace]\n").expect("fw root Cargo.toml");
        fs::write(
            fw.join("crates/runtime/core/Cargo.toml"),
            "[package]\nname = \"runtime-core\"\nversion = \"0.0.1\"\n",
        )
        .expect("fw core Cargo.toml");

        let proj = parent.path.join("proj");
        fs::create_dir_all(&proj).expect("mk proj");
        fs::write(
            proj.join("Cargo.toml"),
            r#"
[package]
name = "demo"
version = "0.0.1"
edition = "2021"

[dependencies]
runtime-core = { path = "../fw/crates/runtime/core" }
"#,
        )
        .expect("write proj Cargo.toml");

        let src = FrameworkSource::detect(&proj, git_defaults(), registry_defaults()).expect("detect");
        match src {
            FrameworkSource::Workspace { root } => {
                assert!(
                    root.is_absolute(),
                    "workspace root must be absolute so the deeper wrapper \
                     Cargo.toml resolves it correctly; got {}",
                    root.display()
                );
                assert_eq!(
                    root,
                    fs::canonicalize(&fw).expect("canon fw"),
                    "root should be the framework checkout, canonicalized"
                );
            }
            FrameworkSource::Git { .. } => {
                panic!("relative path dep must resolve to Workspace, not Git")
            }
            FrameworkSource::Registry { registry, version } => panic!(
                "expected a workspace path, got registry {registry} {version}"
            ),
        }
    }

    // ---------------------------------------------------------------
    // Workspace-inherited framework deps (`{ workspace = true }`).
    //
    // The regression: a multi-crate app writes `runtime-core =
    // { workspace = true }` in the member and the real spec in the root's
    // `[workspace.dependencies]`. The manifest parser only looked at the
    // member, found neither `path` nor `git`, and silently fell through to
    // the CLI's compile-time git default — so the wrapper built against a
    // released tag while the app built against the local checkout. Two
    // `runtime_core` instances, and `mount` fails with "expected
    // `Element`, found `Element`".
    // ---------------------------------------------------------------

    /// Lay out a directory that `is_framework_root` will accept.
    fn fake_framework(at: &Path) -> PathBuf {
        fs::create_dir_all(at.join("crates/runtime/core")).expect("mk fw tree");
        fs::write(at.join("Cargo.toml"), "[workspace]\n").expect("fw root Cargo.toml");
        fs::write(
            at.join("crates/runtime/core/Cargo.toml"),
            "[package]\nname = \"runtime-core\"\nversion = \"0.0.1\"\n",
        )
        .expect("fw core Cargo.toml");
        fs::canonicalize(at).expect("canon fw")
    }

    /// A workspace MEMBER inheriting `runtime-core` from the root must
    /// resolve the root's spec — and resolve its relative `path`
    /// against the ROOT's directory, not the member's.
    ///
    /// Resolving against the member would land at
    /// `<root>/crates/idealyst-native/...`, fail `is_framework_root`,
    /// and fall through to git defaults — the same silent mismatch with
    /// a fix nominally in place.
    #[test]
    fn workspace_member_inherits_framework_path_dep_from_root() {
        let parent = TempProject::new("ws-inherit");
        let fw = fake_framework(&parent.path.join("fw"));

        let root = parent.path.join("app");
        let member = root.join("crates/app-main");
        fs::create_dir_all(&member).expect("mk member");
        fs::write(
            root.join("Cargo.toml"),
            r#"
[workspace]
members = ["crates/app-main"]

[workspace.dependencies]
runtime-core = { path = "../fw/crates/runtime/core" }
"#,
        )
        .expect("write root Cargo.toml");
        fs::write(
            member.join("Cargo.toml"),
            r#"
[package]
name = "app-main"
version = "0.0.1"
edition = "2021"

[dependencies]
runtime-core = { workspace = true }
"#,
        )
        .expect("write member Cargo.toml");

        match read_project_framework_dep(&member) {
            Some(FrameworkSource::Workspace { root: found }) => assert_eq!(
                found, fw,
                "the member's relative path must resolve against the workspace root",
            ),
            other => panic!(
                "workspace-inherited path dep must resolve to the framework \
                 checkout, got {other:?} — this is the silent git-default \
                 fallback that produces two `runtime_core` instances",
            ),
        }
    }

    /// Same inheritance path, git flavour: the root's refspec form must
    /// survive so the wrapper and the app land on one source id.
    #[test]
    fn workspace_member_inherits_framework_git_dep_from_root() {
        let parent = TempProject::new("ws-inherit-git");
        let root = parent.path.join("app");
        let member = root.join("crates/app-main");
        fs::create_dir_all(&member).expect("mk member");
        fs::write(
            root.join("Cargo.toml"),
            r#"
[workspace]
members = ["crates/app-main"]

[workspace.dependencies]
runtime-core = { git = "https://github.com/IdealystIO/idealyst-native", tag = "1.2.5" }
"#,
        )
        .expect("write root Cargo.toml");
        fs::write(
            member.join("Cargo.toml"),
            "[package]\nname = \"app-main\"\nversion = \"0.0.1\"\n\n\
             [dependencies]\nruntime-core = { workspace = true }\n",
        )
        .expect("write member Cargo.toml");

        match read_project_framework_dep(&member) {
            Some(FrameworkSource::Git { url, refspec: GitRef::Tag(t) }) => {
                assert_eq!(url, "https://github.com/IdealystIO/idealyst-native");
                assert_eq!(t, "1.2.5");
            }
            other => panic!("expected the root's Git/Tag spec, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // `cargo metadata` — the authoritative branch. Exercised against
    // synthetic documents so the tests don't shell out or need network.
    // ---------------------------------------------------------------

    fn metadata_with(packages: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "packages": packages })
    }

    /// A path-resolved `runtime-core` (`source: null`) becomes a
    /// `Workspace` rooted three levels above `crates/runtime/core`.
    #[test]
    fn metadata_path_package_yields_workspace_root() {
        let parent = TempProject::new("meta-path");
        let fw = fake_framework(&parent.path.join("fw"));
        let meta = metadata_with(serde_json::json!([
            { "name": "some-other-crate", "source": serde_json::Value::Null,
              "manifest_path": "/elsewhere/Cargo.toml" },
            { "name": "runtime-core", "source": serde_json::Value::Null,
              "manifest_path": fw.join("crates/runtime/core/Cargo.toml").to_str().unwrap() },
        ]));

        match framework_source_from_metadata(&meta).expect("single hit is not an error") {
            Some(FrameworkSource::Workspace { root }) => assert_eq!(root, fw),
            other => panic!("expected Workspace, got {other:?}"),
        }
    }

    /// Two `runtime-core`s in one graph is the failure this whole
    /// module exists to prevent. Report it here — naming both sources —
    /// rather than letting it surface as a type error inside generated
    /// code that names `Element` twice.
    #[test]
    fn metadata_duplicate_framework_crates_is_a_hard_error() {
        let meta = metadata_with(serde_json::json!([
            { "name": "runtime-core", "source": serde_json::Value::Null,
              "manifest_path": "/local/fw/crates/runtime/core/Cargo.toml" },
            { "name": "runtime-core",
              "source": "git+https://github.com/IdealystIO/idealyst-native?tag=1.2.5#abc123",
              "manifest_path": "/cargo/git/checkouts/idealyst/abc123/crates/runtime/core/Cargo.toml" },
        ]));

        let err = framework_source_from_metadata(&meta)
            .expect_err("a two-instance graph must not resolve silently");
        let msg = format!("{err:#}");
        assert!(msg.contains("/local/fw"), "names the path source: {msg}");
        assert!(msg.contains("tag=1.2.5"), "names the git source: {msg}");
        assert!(msg.contains("Element"), "explains the symptom: {msg}");
    }

    /// No `runtime-core` at all → fall through, don't error. Covers
    /// `idealyst new` scaffolding and non-idealyst projects.
    #[test]
    fn metadata_without_framework_package_falls_through() {
        let meta = metadata_with(serde_json::json!([
            { "name": "serde", "source": "registry+https://github.com/rust-lang/crates.io-index",
              "manifest_path": "/cargo/registry/serde/Cargo.toml" },
        ]));
        assert!(framework_source_from_metadata(&meta).expect("no hit is not an error").is_none());
    }

    /// Cargo keys crate identity on the whole source id, so the refspec
    /// FORM has to round-trip: a wrapper pinning `rev = <sha>` against
    /// an app pinning `tag = 1.2.5` is two crates even though both name
    /// the same commit.
    #[test]
    fn git_source_ids_round_trip_their_refspec_form() {
        let cases = [
            ("git+https://example.invalid/fw?tag=1.2.5#deadbeef", ("tag", "1.2.5")),
            ("git+https://example.invalid/fw?branch=main#deadbeef", ("branch", "main")),
            ("git+https://example.invalid/fw?rev=deadbeef#deadbeef", ("rev", "deadbeef")),
            // Branch names contain `/`, which cargo percent-encodes.
            ("git+https://example.invalid/fw?branch=release%2F1.2#deadbeef", ("branch", "release/1.2")),
        ];
        for (source, (want_key, want_value)) in cases {
            match parse_git_source(source) {
                Some(FrameworkSource::Git { url, refspec }) => {
                    assert_eq!(url, "https://example.invalid/fw", "url from {source}");
                    assert_eq!(refspec.as_pair(), (want_key, want_value), "refspec from {source}");
                }
                other => panic!("expected Git from {source}, got {other:?}"),
            }
        }
    }

    /// A bare `git = "..."` (default branch) has no refspec `GitRef`
    /// can spell. Inventing one would produce a source id that differs
    /// from the app's — the exact mismatch being avoided — so fall
    /// through to the git defaults instead.
    #[test]
    fn git_source_without_refspec_falls_through() {
        assert!(parse_git_source("git+https://example.invalid/fw#deadbeef").is_none());
    }

    /// A published `runtime-core` isn't spellable as a wrapper dep
    /// (wrappers know path and git only), so leave it to the fallback.
    #[test]
    fn metadata_registry_package_falls_through() {
        let meta = metadata_with(serde_json::json!([
            { "name": "runtime-core", "source": "registry+https://github.com/rust-lang/crates.io-index",
              "manifest_path": "/cargo/registry/runtime-core-1.2.5/Cargo.toml" },
        ]));
        assert!(framework_source_from_metadata(&meta).expect("registry is not an error").is_none());
    }

    /// A path-resolved package whose manifest ISN'T inside a framework
    /// checkout must fall through rather than emit a bogus root — an
    /// unrelated crate that happens to be named `runtime-core`.
    #[test]
    fn metadata_path_package_outside_a_checkout_falls_through() {
        let meta = metadata_with(serde_json::json!([
            { "name": "runtime-core", "source": serde_json::Value::Null,
              "manifest_path": "/nowhere/near/a/checkout/Cargo.toml" },
        ]));
        assert!(framework_source_from_metadata(&meta).expect("not an error").is_none());
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

    /// Regression: a RELATIVE `project_dir` (e.g. `.`) must still resolve the
    /// in-tree framework workspace. A caller that passed `.` without
    /// canonicalizing used to fall through to git-mode, producing two
    /// `runtime_core` instances and a `mount` "expected `Element`, found
    /// `Element`" failure (hit via `idealyst publish ios` run from a project
    /// dir). `cargo test` runs with CWD inside this crate — which lives under
    /// the framework workspace — so a relative `.` here must find the root.
    #[test]
    fn relative_project_dir_still_finds_in_tree_workspace() {
        let from_relative = find_framework_workspace(Path::new("."))
            .expect("a relative `.` inside the framework tree must resolve the workspace root");
        let from_absolute = find_framework_workspace(
            &std::env::current_dir().expect("cwd"),
        )
        .expect("absolute CWD must also resolve");
        assert_eq!(
            from_relative, from_absolute,
            "relative and absolute project dirs must resolve the same workspace root",
        );
        assert!(
            is_framework_root(&from_relative),
            "resolved root must actually be a framework workspace",
        );
    }

    // ---------------------------------------------------------------
    // remap_path_flags — keeping build-machine paths out of shipped
    // binaries (panic `Location` strings live in `.rodata`, so no
    // strip pass can remove them after the fact).
    // ---------------------------------------------------------------

    fn remap_targets(flags: &[String]) -> Vec<&str> {
        flags
            .iter()
            .filter_map(|f| f.strip_prefix("--remap-path-prefix="))
            .filter_map(|kv| kv.rsplit_once('=').map(|(_, to)| to))
            .collect()
    }

    #[test]
    fn remap_flags_cover_framework_app_cargo_and_sysroot() {
        let source = FrameworkSource::Workspace {
            root: PathBuf::from("/some/idealyst-native"),
        };
        let flags = remap_path_flags(&source, Path::new("/elsewhere/my-app"));
        let targets = remap_targets(&flags);

        assert!(targets.contains(&"/idealyst"), "framework root: {flags:?}");
        assert!(targets.contains(&"/app"), "project root: {flags:?}");
        // `cargo_home` falls back to `$HOME/.cargo` and `rustc_sysroot`
        // shells out; both are present in any environment that can build
        // this crate at all.
        assert!(targets.contains(&"/cargo"), "cargo home: {flags:?}");
        assert!(targets.contains(&"/rust"), "sysroot: {flags:?}");

        assert!(
            flags
                .iter()
                .all(|f| f.starts_with("--remap-path-prefix=")),
            "every emitted flag must be a remap: {flags:?}",
        );
    }

    /// rustc's `map_prefix` scans the mapping list in REVERSE and takes the
    /// first hit, so the last matching entry wins. An in-tree app (e.g.
    /// `examples/baseline`) sits *inside* the framework workspace, so both
    /// prefixes match its files — the app root must be emitted afterwards or
    /// the more specific mapping could never win.
    ///
    /// Verified against real rustc behavior: compiling one file with
    /// `--remap-path-prefix=<outer>=/OUTER --remap-path-prefix=<inner>=/INNER`
    /// yields `/INNER/main.rs`, and swapping the order yields
    /// `/OUTER/inner/main.rs`.
    #[test]
    fn in_tree_app_root_is_emitted_after_framework_root() {
        let root = PathBuf::from("/some/idealyst-native");
        let source = FrameworkSource::Workspace { root: root.clone() };
        let flags = remap_path_flags(&source, &root.join("examples/baseline"));

        let framework_at = flags
            .iter()
            .position(|f| f.ends_with("=/idealyst"))
            .expect("framework remap present");
        let app_at = flags
            .iter()
            .position(|f| f.ends_with("=/app"))
            .expect("app remap present");
        assert!(
            app_at > framework_at,
            "app root must come after framework root so it wins: {flags:?}",
        );
    }

    /// Git-mode framework sources are fetched into `CARGO_HOME`, so there is
    /// no separate workspace root to remap — the cargo-home entry covers them.
    #[test]
    fn git_mode_emits_no_framework_root_remap() {
        let source = FrameworkSource::Git {
            url: "https://github.com/IdealystIO/idealyst-native".into(),
            refspec: GitRef::Tag("v1.0.0".into()),
        };
        let flags = remap_path_flags(&source, Path::new("/elsewhere/my-app"));
        let targets = remap_targets(&flags);

        assert!(!targets.contains(&"/idealyst"), "{flags:?}");
        assert!(targets.contains(&"/app"), "{flags:?}");
        assert!(targets.contains(&"/cargo"), "{flags:?}");
    }

    /// `IDEALYST_NO_PATH_REMAP` is the escape hatch for the one real cost of
    /// remapping: on stable rustc it rewrites compiler *diagnostics* too, so
    /// a failing release build loses clickable paths.
    #[test]
    fn remap_opt_out_reads_only_explicit_values() {
        // Unset, or set-but-empty (`FOO= cmd`), means "no opinion".
        assert!(!remap_disabled(None));
        assert!(!remap_disabled(Some("")));
        assert!(!remap_disabled(Some("   ")));
        // Explicit negatives must not silently opt OUT.
        for no in ["0", "false", "no", "FALSE", "No"] {
            assert!(!remap_disabled(Some(no)), "{no:?} must not disable");
        }
        // Anything else is an opt-out.
        for yes in ["1", "true", "yes", "on"] {
            assert!(remap_disabled(Some(yes)), "{yes:?} must disable");
        }
    }

    /// The flags are joined with `\x1f` into `CARGO_ENCODED_RUSTFLAGS`
    /// precisely so a space in a path can't split one flag into two. Pin
    /// that a spacey path survives as a single argument.
    #[test]
    fn path_with_spaces_stays_one_flag() {
        let source = FrameworkSource::Workspace {
            root: PathBuf::from("/Users/Jane Smith/idealyst-native"),
        };
        let flags = remap_path_flags(&source, Path::new("/Users/Jane Smith/my app"));

        let encoded = flags.join("\x1f");
        assert_eq!(
            encoded.split('\x1f').count(),
            flags.len(),
            "encoding must preserve one field per flag: {flags:?}",
        );
        assert!(
            flags
                .iter()
                .any(|f| f == "--remap-path-prefix=/Users/Jane Smith/my app=/app"),
            "spacey project root must survive intact: {flags:?}",
        );
    }
}
