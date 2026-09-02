//! Ephemeral "catalog wrapper" crate generator.
//!
//! The MCP server lists a project's `#[component]`s by running a binary
//! that (a) links the project's library so its `inventory::submit!`
//! registrations are present, and (b) is built with `runtime-core/catalog`
//! on so the `#[component]` macro actually *emits* those registrations.
//!
//! Rather than make every project carry that binary + an `catalog` feature
//! (the old scaffold did), the CLI generates a throwaway wrapper crate
//! under `target/idealyst/<project(s)>/catalog/` — the same place and
//! shape as the per-platform `{web,ios,android}` wrappers. The wrapper
//! path-deps the project and turns on `runtime-core/catalog`; Cargo's
//! feature unification then builds `runtime-macros` with emission on for
//! the *entire* graph, so the project's components register even though
//! the project declares no `catalog` feature itself.
//!
//! This is the mechanism that lets `idealyst mcp` work against any
//! project — old or new — with zero per-project boilerplate.
//!
//! ## Several projects, one catalog
//!
//! A wrapper can link MORE than one project. `inventory` registration is
//! additive at link time, so N project libs in one extractor produce one
//! catalog spanning all of them, and each component still reports the
//! crate it came from via `module_path`.
//!
//! That is what makes `idealyst mcp` useful in a monorepo: pointed at a
//! cargo workspace root, [`resolve_project_roots`] discovers every member
//! carrying `[package.metadata.idealyst]` and they are wrapped together.
//! Before this, a workspace root simply failed to parse as a project and
//! the server served an EMPTY catalog with only a line on stderr to say
//! why — indistinguishable, to a client, from a project with no
//! components.
//!
//! ## Force-linking dependency component crates
//!
//! `use {lib} as _;` only pins the project's OWN `inventory::submit!`
//! registrations. A component library the project depends on (e.g.
//! `idea-ui`) registers its components via the same mechanism, but
//! `inventory`'s linker-section ctors only survive linking if the
//! linker actually pulls that crate's object code in — `inventory`'s
//! known cross-rlib caveat (framework-mcp-spec §9.3). Merely depending
//! on a component library doesn't guarantee that.
//!
//! So the wrapper walks the project's `cargo metadata`, finds every
//! direct dependency that itself depends on `runtime-core`
//! (the signal for "this crate may declare
//! `#[component]`s" — runtime v2 crates reach the author surface through
//! the facade alias, so the facade is the marker there), declares each as
//! a direct wrapper dependency, and emits a `use <dep> as _;` for it. That
//! forces their object code — and thus their `inventory::submit!`
//! registrations — into the catalog binary, so a freshly-added
//! component-library dependency surfaces in the catalog even before the
//! project references any of its components.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use build_ios::FrameworkSource;
use serde_json::Value;

use crate::framework_source;

/// Generate (or refresh) the catalog wrapper crate for `project_root`
/// and return its directory. The returned crate exposes a `catalog`
/// bin that prints the project's MCP catalog JSON to stdout — run it
/// with `cargo run -q --bin catalog` from the returned directory.
///
/// Idempotent: files are only rewritten when their contents change, so
/// repeated calls don't invalidate cargo's fingerprints and trigger
/// needless rebuilds.
pub fn generate(project_root: &Path) -> Result<PathBuf> {
    generate_for_roots(std::slice::from_ref(&project_root.to_path_buf()))
}

/// Generate one catalog wrapper linking **every** project in
/// `project_roots`, so a single extractor emits a catalog covering all of
/// them. Each component keeps its originating crate in `module_path`, so
/// provenance survives the merge.
///
/// This is what `idealyst mcp` uses: pair it with [`resolve_project_roots`]
/// to turn a workspace root into the set of projects to wrap.
/// Path to an already-built extractor for `project_roots`, if one is on
/// disk.
///
/// Running it directly costs ~70ms against ~470ms for `cargo run` plus
/// ~450ms of `cargo metadata` — and, more importantly, takes NO cargo
/// build lock, so it cannot contend with a concurrent build.
///
/// The wrapper's `main` dumps the catalog unconditionally, so the binary
/// needs no argument; `--emit-catalog` is passed anyway to match the
/// `--from-bin` contract, and is ignored.
///
/// This deliberately does NOT check whether the binary is older than the
/// project's sources. It is only used under `--no-watch`, whose contract
/// is already "loaded once at startup, never refreshed" — a caller that
/// opted out of freshness gets the fast path it asked for. With watching
/// on, the managed `cargo run` path stays in charge.
pub fn prebuilt_catalog_bin(project_roots: &[PathBuf]) -> Option<PathBuf> {
    let anchor = project_roots
        .iter()
        .filter_map(|r| {
            let root = fs::canonicalize(r).ok()?;
            let manifest = build_ios::parse_manifest(&root).ok()?;
            Some((root, manifest.name))
        })
        .min_by(|a, b| a.1.cmp(&b.1))?;
    let source = framework_source::resolve(&anchor.0).ok()?;
    let target = sidecar_target_dir(&source, &anchor.0);
    // Release first: if someone built one, it is the faster binary.
    ["release", "debug"]
        .iter()
        .map(|profile| target.join(profile).join("catalog"))
        .find(|p| p.is_file())
}

pub fn generate_for_roots(project_roots: &[PathBuf]) -> Result<PathBuf> {
    generate_with(project_roots, "catalog", "catalog", "dump_catalog_json")
}

/// Directory name for a wrapper covering `projects`.
///
/// One project keeps its own name, so a single-project wrapper lands at
/// exactly the path it always did. Several projects are joined by `+` in
/// sorted order — stable across runs, readable in a build log, and
/// distinct per project set, so changing the set correctly lands in a
/// fresh wrapper rather than silently reusing the old one. Long sets are
/// truncated to keep the path manageable.
fn wrapper_name(names: &[&str]) -> String {
    const MAX: usize = 64;
    let joined = names.join("+");
    if joined.len() <= MAX {
        return joined;
    }
    format!("{}+{}-more", names[0], names.len() - 1)
}

/// Cargo package name for the wrapper crate.
///
/// [`wrapper_name`] joins projects with `+`, which is fine in a path but
/// is NOT a legal cargo package name — cargo requires Unicode XID
/// characters plus `-` and `_`, and rejects the manifest outright:
///
/// ```text
/// error: invalid character `+` in package name
/// ```
///
/// A single-project wrapper has no `+`, so its package name is unchanged
/// by this — existing wrappers keep their identity and their warm build
/// cache across the upgrade.
fn wrapper_package_name(names: &[&str]) -> String {
    wrapper_name(names).replace('+', "-")
}

/// Parameterized core of [`generate`]. Builds an ephemeral wrapper that
/// links the project (+ force-links its component-library deps) with
/// `runtime-core/catalog` on, exposing a `<bin_name>` binary whose
/// `main()` calls `runtime_core::__mcp::<dump_call>()`.
///
/// `subdir` names the staging dir under `target/idealyst/<project>/`, so
/// distinct extractors (the MCP catalog vs. the external-export manifest)
/// don't clobber each other's build fingerprints.
/// Directory name of the catalog wrapper's dedicated build output,
/// under the project/workspace cargo target root.
///
/// Peer of the web bundle's `idealyst-web-<config_key>` and the dev
/// server's `idealyst-dev-server`: every CLI-managed build gets its own
/// target dir so it never competes for the bare `target/` build lock
/// that the user's own `cargo` invocations (and rust-analyzer) hold.
pub const SIDECAR_TARGET_DIR: &str = "idealyst-mcp";

/// Where the generated catalog wrapper writes its build output.
///
/// Rooted at [`FrameworkSource::cargo_target_dir`], so sibling projects
/// under one framework source share a warm dependency cache — the same
/// rooting the web and dev-server target dirs use.
pub fn sidecar_target_dir(source: &FrameworkSource, project_root: &Path) -> PathBuf {
    source.cargo_target_dir(project_root).join(SIDECAR_TARGET_DIR)
}

/// Expand a user-supplied root into the set of idealyst projects to wrap.
///
/// A project directory (one whose `Cargo.toml` has a `[package]`) yields
/// itself. A **workspace root** yields every member carrying
/// `[package.metadata.idealyst]` — the framework's own marker for "this
/// crate is an idealyst project", the same key [`build_ios::parse_manifest`]
/// reads. Plain library members are deliberately not wrapped directly:
/// their components still reach the catalog through the apps that depend
/// on them, and wrapping every lib in a large workspace would build far
/// more than the catalog needs.
///
/// This is what makes a bare `idealyst mcp` work at a workspace root.
/// Previously that errored and the server degraded to an empty catalog.
pub fn resolve_project_roots(root: &Path) -> Result<Vec<PathBuf>> {
    let root = fs::canonicalize(root)
        .with_context(|| format!("canonicalize {}", root.display()))?;
    // A real project: wrap exactly it.
    if build_ios::parse_manifest(&root).is_ok() {
        return Ok(vec![root]);
    }
    let found = discover_workspace_projects(&root)?;
    if found.is_empty() {
        anyhow::bail!(
            "{} is a cargo workspace with no idealyst projects in it. An \
             idealyst project is a member crate whose Cargo.toml has a \
             [package.metadata.idealyst] section; add one, or point \
             --project-root at a project directory",
            root.join("Cargo.toml").display(),
        );
    }
    Ok(found)
}

/// Run `cargo metadata` at a workspace root and return the member
/// directories that are idealyst projects. Cargo resolves `members`
/// globs for us, which hand-globbing would get wrong.
fn discover_workspace_projects(workspace_root: &Path) -> Result<Vec<PathBuf>> {
    let manifest_path = workspace_root.join("Cargo.toml");
    let output = std::process::Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .arg("--manifest-path")
        .arg(&manifest_path)
        .output()
        .with_context(|| format!("run cargo metadata for {}", manifest_path.display()))?;
    if !output.status.success() {
        anyhow::bail!(
            "cargo metadata failed for {}: {}",
            manifest_path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let json: Value = serde_json::from_slice(&output.stdout)
        .with_context(|| format!("parse cargo metadata for {}", manifest_path.display()))?;
    Ok(collect_workspace_projects(&json))
}

/// Pure core of [`discover_workspace_projects`], unit-testable against a
/// synthetic `cargo metadata` document.
///
/// Returns member directories sorted by path, so the generated wrapper is
/// byte-stable across runs — an unstable order would rewrite the wrapper
/// on every call and defeat the idempotence that keeps cargo from
/// rebuilding.
fn collect_workspace_projects(meta: &Value) -> Vec<PathBuf> {
    let members: Vec<&str> = meta
        .get("workspace_members")
        .and_then(|m| m.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    let Some(packages) = meta.get("packages").and_then(|p| p.as_array()) else {
        return Vec::new();
    };
    let mut roots: Vec<PathBuf> = packages
        .iter()
        .filter(|pkg| {
            pkg.get("id")
                .and_then(|i| i.as_str())
                .is_some_and(|id| members.contains(&id))
        })
        // The framework's own definition of an idealyst project.
        .filter(|pkg| {
            pkg.get("metadata")
                .and_then(|m| m.get("idealyst"))
                .is_some()
        })
        .filter_map(|pkg| {
            let manifest = pkg.get("manifest_path")?.as_str()?;
            Path::new(manifest).parent().map(Path::to_path_buf)
        })
        .collect();
    roots.sort();
    roots.dedup();
    roots
}

pub fn generate_with(
    project_roots: &[PathBuf],
    subdir: &str,
    bin_name: &str,
    dump_call: &str,
) -> Result<PathBuf> {
    if project_roots.is_empty() {
        anyhow::bail!("no project roots to build a catalog wrapper for");
    }

    // Absolute project paths — the wrapper lives elsewhere on disk and
    // references each project by path, so a relative `.` / cwd would
    // resolve against the wrapper dir, not here.
    //
    // Sorted so the emitted wrapper is byte-stable regardless of the
    // order roots arrived in: an unstable order would rewrite the files
    // on every call and defeat the idempotence cargo's fingerprints rely
    // on.
    let mut projects: Vec<(PathBuf, build_ios::Manifest)> = Vec::new();
    for root in project_roots {
        let root = fs::canonicalize(root)
            .with_context(|| format!("canonicalize project dir {}", root.display()))?;
        let manifest = build_ios::parse_manifest(&root)
            .context("read the project's Cargo.toml to build a catalog wrapper")?;
        projects.push((root, manifest));
    }
    projects.sort_by(|a, b| a.1.name.cmp(&b.1.name));
    projects.dedup_by(|a, b| a.0 == b.0);

    // The anchor project roots the wrapper on disk and picks the
    // framework source. Every project in one workspace resolves the
    // framework the same way, so the anchor's answer holds for all.
    let (anchor_root, _) = &projects[0];
    let source = framework_source::resolve(anchor_root)?;

    let project_names: Vec<&str> = projects.iter().map(|(_, m)| m.name.as_str()).collect();

    let wrapper_dir = source
        .wrapper_root(anchor_root)
        .join(wrapper_name(&project_names))
        .join(subdir);
    fs::create_dir_all(wrapper_dir.join("src"))
        .with_context(|| format!("create {}", wrapper_dir.join("src").display()))?;

    // `runtime-core` with `catalog` on is the lever: enabling it anywhere
    // in the graph flips the `#[component]` emission gate for every crate,
    // including the project lib.
    // `runtime-core/catalog` turns on BOTH halves the emission needs:
    // the vocabulary's `glue::__mcp` anchor (where the retargeted
    // `#[component]` registrations land) and the
    // `runtime-macros/catalog` emission gate. One `mcp-catalog`
    // instance, so the wrapper's `runtime_core::__mcp` dump sees every
    // registration.
    let fcore_dep = source.dep("crates/runtime/core", &["catalog"]);
    // Redirect any git-pinned framework crates the project uses to the
    // same physical paths the wrapper uses, so the two halves share ONE
    // `runtime_core` instance (otherwise feature unification can't merge
    // the `catalog` feature, and the wrapper→project type bridge fails).
    // Empty in git mode — there both sides already use the same rev.
    let patch_block = source.patch_block();

    // Direct dependencies that themselves depend on `runtime-core` —
    // i.e. crates that may declare `#[component]`s. We force-link each
    // so its catalog registrations survive linking even before the
    // project references any of its components (see module docs).
    // Non-fatal: a metadata failure just means we link the project lib
    // only, the same behaviour as before this was added.
    // Union of every project's component-library deps. Two apps in one
    // workspace normally share most of these (the component library, the
    // icon pack), so dedup by package name — declaring the same dep twice
    // is a manifest error, and the first spelling wins.
    //
    // A project is never force-linked as another's dep: it is already an
    // explicit `[dependencies]` entry below, and re-declaring it would
    // collide.
    let mut forced: Vec<ForcedDep> = Vec::new();
    for (root, manifest) in &projects {
        for dep in discover_forced_deps(root, &source, &manifest.name) {
            if project_names.contains(&dep.pkg_name.as_str()) {
                continue;
            }
            if !forced.iter().any(|d| d.pkg_name == dep.pkg_name) {
                forced.push(dep);
            }
        }
    }

    let forced_dep_lines = forced
        .iter()
        .map(|d| format!("{} = {}\n", d.pkg_name, d.dep_line))
        .collect::<String>();
    let forced_use_lines = forced
        .iter()
        .map(|d| format!("use {} as _;\n", d.lib_ident))
        .collect::<String>();

    // One `[dependencies]` entry per wrapped project. Several projects
    // in one wrapper is what lets a single catalog cover every app in a
    // workspace: `inventory` registration is additive at link time, so
    // linking N project libs collects N projects' components into one
    // catalog, each still carrying its own crate in `module_path`.
    let project_dep_lines = projects
        .iter()
        .map(|(root, m)| format!("{} = {{ path = \"{}\" }}\n", m.name, root.display()))
        .collect::<String>();
    let project_use_lines = projects
        .iter()
        .map(|(_, m)| format!("use {} as _;\n", m.lib_name))
        .collect::<String>();

    let cargo_toml = format!(
        r#"# GENERATED by `idealyst mcp`. Do not edit — rewritten on demand.
#
# Ephemeral catalog-extraction wrapper. Links each project's library and
# turns on `runtime-core/catalog` so every `#[component]` in those projects
# (and their component-library deps) registers in the MCP catalog. A
# project itself needs no `[[bin]] catalog` and no `catalog` feature.
#
# More than one project appears here when the wrapper was generated for a
# workspace: one catalog covering every app, with each component's origin
# still recoverable from its `module_path`.
#
# Empty `[workspace]` declares this wrapper standalone even though it
# lives under the framework workspace's `target/idealyst/...`; without it
# cargo would try to claim it as a member of the parent workspace.
[workspace]

[package]
name = "{name}-{subdir}-wrapper"
version = "0.0.1"
edition = "2021"
publish = false

[[bin]]
name = "{bin_name}"
path = "src/main.rs"

[dependencies]
runtime-core = {fcore_dep}
{project_dep_lines}{forced_dep_lines}{patch_block}"#,
        name = wrapper_package_name(&project_names),
        subdir = subdir,
        bin_name = bin_name,
        fcore_dep = fcore_dep,
        project_dep_lines = project_dep_lines,
        forced_dep_lines = forced_dep_lines,
        patch_block = patch_block,
    );

    let main_rs = format!(
        r#"//! GENERATED by `idealyst mcp` — ephemeral catalog extractor.
//!
//! Each `use <project> as _;` links a project's library so its
//! `inventory::submit!` component registrations are present; the wrapper
//! is built with `runtime-core/catalog`, so those registrations were
//! emitted. Registration is additive at link time, which is why several
//! projects can share one extractor. Each `use <dep> as _;` below
//! force-links a component-library dependency so its registrations
//! survive linking too (see the wrapper generator's module docs).
//! `dump_catalog_json` serializes the collected catalog to stdout.

{project_use_lines}{forced_use_lines}
fn main() {{
    runtime_core::__mcp::{dump_call}();
}}
"#,
        project_use_lines = project_use_lines,
        forced_use_lines = forced_use_lines,
        dump_call = dump_call,
    );

    // The wrapper builds into its OWN sidecar target dir under the
    // project/workspace target root — never the bare target dir. This is
    // the same isolation the web bundle (`idealyst-web-<config_key>`) and
    // the dev server (`idealyst-dev-server`) already take, for the same
    // reason: this build carries a different feature union than the
    // project's own (`runtime-core/catalog` and each component library's
    // `catalog`), and under `--watch` it reruns on every save.
    //
    // Sharing the bare target dir made every catalog refresh contend for
    // the one cargo build lock that `cargo check`, `cargo test` and
    // rust-analyzer also take — so a watch-triggered extraction would
    // stall the editor. The lost cache sharing is smaller than it looks:
    // the project's own dev builds live in the sibling `idealyst-web-*` /
    // `idealyst-dev-server` dirs already, so the bare dir held little the
    // wrapper could reuse.
    let cargo_config = format!(
        "# GENERATED. Redirect this wrapper's build output to a dedicated\n\
         # sidecar target dir, so catalog extraction never takes the build\n\
         # lock the project's own cargo commands use.\n\
         \n\
         [build]\n\
         target-dir = \"{}\"\n",
        sidecar_target_dir(&source, anchor_root).display(),
    );

    write_if_changed(&wrapper_dir.join("Cargo.toml"), &cargo_toml)?;
    write_if_changed(&wrapper_dir.join("src/main.rs"), &main_rs)?;
    fs::create_dir_all(wrapper_dir.join(".cargo"))
        .with_context(|| format!("create {}", wrapper_dir.join(".cargo").display()))?;
    write_if_changed(&wrapper_dir.join(".cargo/config.toml"), &cargo_config)?;

    Ok(wrapper_dir)
}

/// A dependency crate the wrapper force-links so its component
/// registrations survive linking.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ForcedDep {
    /// Cargo package name — the `[dependencies]` table key.
    pkg_name: String,
    /// Lib crate identifier for `use <ident> as _;` (the dep's lib
    /// target name, e.g. `idea_ui`).
    lib_ident: String,
    /// The right-hand side of the `[dependencies]` entry — a cargo dep
    /// table sourced to match how the project resolves the same crate
    /// (path in workspace mode, git in git mode), so cargo unifies them
    /// into a single instance rather than a parallel `runtime_core`.
    /// Carries `features = ["catalog"]` when the crate declares a `catalog`
    /// feature (the MCP self-registration gate — see [`dep_line_for`]).
    dep_line: String,
}

/// Run `cargo metadata` for `project_root` and collect the component-
/// library dependencies to force-link. Non-fatal: on any failure we log
/// to stderr and return an empty list — the wrapper still links the
/// project's own library, so the project's own components appear.
fn discover_forced_deps(
    project_root: &Path,
    source: &FrameworkSource,
    project_pkg_name: &str,
) -> Vec<ForcedDep> {
    let manifest_path = project_root.join("Cargo.toml");
    let output = std::process::Command::new("cargo")
        .args(["metadata", "--format-version", "1"])
        .arg("--manifest-path")
        .arg(&manifest_path)
        .output();
    let output = match output {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            eprintln!(
                "[idealyst mcp] cargo metadata failed; dependency components may not \
                 appear in the catalog: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
            return Vec::new();
        }
        Err(e) => {
            eprintln!(
                "[idealyst mcp] could not run cargo metadata ({e}); dependency \
                 components may not appear in the catalog"
            );
            return Vec::new();
        }
    };
    let json: Value = match serde_json::from_slice(&output.stdout) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[idealyst mcp] cargo metadata produced invalid JSON: {e}");
            return Vec::new();
        }
    };
    collect_forced_deps(&json, source, &manifest_path, project_pkg_name)
}

/// Pure core of [`discover_forced_deps`], split out so it's unit-testable
/// against a synthetic `cargo metadata` document without invoking cargo.
///
/// A dependency qualifies when it is (a) a direct, normal (non dev/build)
/// dependency of the project, (b) itself depends on `runtime-core` — the
/// signal that it may declare `#[component]`s — and (c) exposes a normal
/// `lib`/`rlib` target (not a proc-macro). In git mode we additionally
/// require the dependency to originate from the framework's own git repo:
/// a third-party crate resolved from a different source can't be
/// re-declared by the wrapper without risking a duplicate crate instance,
/// so we skip it rather than corrupt the build.
fn collect_forced_deps(
    metadata: &Value,
    source: &FrameworkSource,
    project_manifest_path: &Path,
    project_pkg_name: &str,
) -> Vec<ForcedDep> {
    let packages = match metadata.get("packages").and_then(|p| p.as_array()) {
        Some(p) => p,
        None => return Vec::new(),
    };

    // Resolve the root package id. Prefer `resolve.root`; fall back to
    // the package whose manifest_path matches the project (cargo leaves
    // `resolve.root` null for a virtual-workspace manifest).
    let root_id = metadata
        .pointer("/resolve/root")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            packages.iter().find_map(|p| {
                let mp = p.get("manifest_path").and_then(|m| m.as_str())?;
                (Path::new(mp) == project_manifest_path)
                    .then(|| p.get("id").and_then(|i| i.as_str()).map(String::from))
                    .flatten()
            })
        });
    let Some(root_id) = root_id else {
        return Vec::new();
    };

    // Direct, normal dependency package ids of the root, from the
    // resolve graph (which already applied platform/feature resolution).
    let direct_ids: Vec<String> = metadata
        .pointer("/resolve/nodes")
        .and_then(|n| n.as_array())
        .and_then(|nodes| nodes.iter().find(|n| n.get("id").and_then(|i| i.as_str()) == Some(root_id.as_str())))
        .and_then(|root_node| root_node.get("deps").and_then(|d| d.as_array()))
        .map(|deps| {
            deps.iter()
                .filter(|dep| dep_has_normal_kind(dep))
                .filter_map(|dep| dep.get("pkg").and_then(|p| p.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let mut out: Vec<ForcedDep> = Vec::new();
    for id in &direct_ids {
        let Some(pkg) = packages
            .iter()
            .find(|p| p.get("id").and_then(|i| i.as_str()) == Some(id.as_str()))
        else {
            continue;
        };
        let name = pkg.get("name").and_then(|n| n.as_str()).unwrap_or_default();
        // Never re-declare the core crates (both are already wrapper
        // deps) or the project itself (already linked via
        // `use {lib} as _;`).
        if name.is_empty()
            || name == "runtime-core"
            || name == "runtime-core"
            || name == project_pkg_name
        {
            continue;
        }
        // Only crates that depend on the framework core can host components.
        if !pkg_depends_on_core(pkg) {
            continue;
        }
        let Some(lib_ident) = pkg_lib_target_name(pkg) else {
            continue;
        };
        let Some(manifest) = pkg.get("manifest_path").and_then(|m| m.as_str()) else {
            continue;
        };
        let Some(dir) = Path::new(manifest).parent() else {
            continue;
        };
        let pkg_source = pkg.get("source").and_then(|s| s.as_str());
        // Some component-library crates gate their MCP self-registration
        // behind their OWN `catalog` feature (a separate inventory slice
        // from `runtime-core/catalog`'s `#[component]` emission) — e.g.
        // `icons-lucide`'s `IconSetEntry` and the recipe system. Enabling
        // `runtime-core/catalog` does NOT transitively turn those on, so we
        // must enable each crate's `catalog` feature explicitly or its
        // registrations compile out and never reach the catalog.
        let features: &[&str] = if pkg_has_catalog_feature(pkg) {
            &["catalog"]
        } else {
            &[]
        };
        let Some(dep_line) = dep_line_for(source, dir, pkg_source, features) else {
            // git mode + third-party source: skip (see fn docs).
            eprintln!(
                "[idealyst mcp] skipping force-link of `{name}` — its source isn't the \
                 framework git repo, so it can't be safely re-declared in git mode. Its \
                 components will appear once your code references the crate."
            );
            continue;
        };
        out.push(ForcedDep {
            pkg_name: name.to_string(),
            lib_ident,
            dep_line,
        });
    }

    // Deterministic order so `write_if_changed` stays idempotent.
    out.sort_by(|a, b| a.pkg_name.cmp(&b.pkg_name));
    out.dedup_by(|a, b| a.pkg_name == b.pkg_name);
    out
}

/// True if a `resolve.nodes[].deps[]` entry includes a normal (non
/// dev/build) dependency kind. Cargo encodes the normal kind as a
/// `null` `kind`; older metadata without `dep_kinds` is treated as
/// normal too.
fn dep_has_normal_kind(dep: &Value) -> bool {
    match dep.get("dep_kinds").and_then(|k| k.as_array()) {
        None => true,
        Some(kinds) => kinds
            .iter()
            .any(|k| k.get("kind").map(|v| v.is_null()).unwrap_or(true)),
    }
}

/// True if a package's manifest declares a dependency named
/// `runtime-core` (any kind) — the marker that it may host components.
///
/// This matched TWO spellings during the runtime-v2 migration, when the
/// author surface briefly lived in a `runtime-facade` package: matching
/// only `runtime-core` silently stopped force-linking every migrated
/// component library (idea-ui, idea-theme, icons-lucide, …), so
/// `idealyst mcp` / `catalog-json` / `export` saw an empty dependency
/// slice — the components compiled but their `inventory::submit!` ctors
/// were never linked in. One spelling again; the regression test below
/// still pins the behavior.
fn pkg_depends_on_core(pkg: &Value) -> bool {
    pkg.get("dependencies")
        .and_then(|d| d.as_array())
        .map(|deps| {
            deps.iter().any(|d| {
                d.get("name").and_then(|n| n.as_str()) == Some("runtime-core")
            })
        })
        .unwrap_or(false)
}

/// True if a package declares a `catalog` Cargo feature — the framework's
/// convention for gating `mcp_catalog::inventory::submit!` self-registration
/// that `#[component]` emission doesn't cover (icon packs' `IconSetEntry`,
/// the recipe system, …). `cargo metadata` exposes the feature map on every
/// package, so this is a pure lookup.
fn pkg_has_catalog_feature(pkg: &Value) -> bool {
    pkg.get("features")
        .and_then(|f| f.as_object())
        .map(|m| m.contains_key("catalog"))
        .unwrap_or(false)
}

/// The package's importable lib target name (`idea_ui`), or `None` if it
/// has no normal library target (e.g. a binary-only or proc-macro crate,
/// which can't be `use`d as a linked library).
fn pkg_lib_target_name(pkg: &Value) -> Option<String> {
    let targets = pkg.get("targets").and_then(|t| t.as_array())?;
    targets.iter().find_map(|t| {
        let kinds = t.get("kind").and_then(|k| k.as_array())?;
        let is_lib = kinds.iter().any(|k| {
            matches!(k.as_str(), Some("lib") | Some("rlib") | Some("dylib"))
        });
        let is_proc_macro = kinds.iter().any(|k| k.as_str() == Some("proc-macro"));
        (is_lib && !is_proc_macro)
            .then(|| t.get("name").and_then(|n| n.as_str()).map(String::from))
            .flatten()
    })
}

/// Build the `[dependencies]` RHS for a force-linked crate, sourced to
/// match how the project resolves it so cargo unifies them:
///
/// - **Workspace mode**: a `path` dep to the resolved manifest directory.
///   The project's own dep (whether `{ workspace = true }` or `path`)
///   resolves to the same directory, so cargo sees one package instance.
/// - **Git mode**: a `git` dep pinned to the same url + refspec as the
///   framework. Returns `None` for a dependency whose source isn't that
///   git repo — re-declaring a foreign source would fork the crate graph.
///
/// `features` are appended verbatim (e.g. `["catalog"]`) so a crate whose
/// MCP registration is feature-gated is built with that gate on. Feature
/// unification then enables it on the single shared crate instance.
fn dep_line_for(
    source: &FrameworkSource,
    manifest_dir: &Path,
    pkg_source: Option<&str>,
    features: &[&str],
) -> Option<String> {
    // `, features = ["a", "b"]` or empty.
    let feat = if features.is_empty() {
        String::new()
    } else {
        let list = features
            .iter()
            .map(|f| format!("\"{}\"", f))
            .collect::<Vec<_>>()
            .join(", ");
        format!(", features = [{}]", list)
    };
    match source {
        FrameworkSource::Workspace { .. } => {
            Some(format!("{{ path = \"{}\"{} }}", manifest_dir.display(), feat))
        }
        FrameworkSource::Git { url, refspec } => {
            // Only force-link crates that come from the framework's git
            // repo; their git source already matches the project's, so
            // cargo merges them into one instance.
            let src = pkg_source?;
            if !src.contains(url.as_str()) {
                return None;
            }
            let (key, value) = refspec.as_pair();
            Some(format!("{{ git = \"{}\", {} = \"{}\"{} }}", url, key, value, feat))
        }
    }
}

/// Write `contents` to `path` only if it differs from what's already
/// there. Avoids bumping mtimes (and thus cargo fingerprints) on
/// no-op regenerations.
fn write_if_changed(path: &Path, contents: &str) -> Result<()> {
    if let Ok(existing) = fs::read_to_string(path) {
        if existing == contents {
            return Ok(());
        }
    }
    fs::write(path, contents).with_context(|| format!("write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal project: `[package]` + a hyphenated name (so we exercise
    /// the `name → lib_name` `-`→`_` conversion) and a `runtime-core`
    /// dep. No `[[bin]] catalog`, no `catalog` feature — the whole point is
    /// that the wrapper supplies both.
    ///
    /// `runtime-core` is pinned to a non-existent local path so the
    /// `cargo metadata` call inside `generate` (forced-dep discovery)
    /// fails fast and OFFLINE — these tests assert file emission, not
    /// dependency resolution, and must not touch the network. Discovery
    /// degrades gracefully to "no forced deps", which is what we want
    /// here. The forced-dep selection itself is covered by the pure
    /// `collect_forced_deps` tests below.
    fn fake_project(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "idealyst-catwrap-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"my-app\"\nversion = \"0.0.1\"\nedition = \"2021\"\n\
             [dependencies]\nruntime-core = { path = \"./does-not-exist/crates/runtime/core\" }\n",
        )
        .unwrap();
        dir
    }

    /// A project with a chosen package name, so multi-project tests can
    /// tell the two apart in the emitted wrapper.
    fn fake_named_project(tag: &str, pkg: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "idealyst-catwrap-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{pkg}\"\nversion = \"0.0.1\"\nedition = \"2021\"\n\
                 [dependencies]\nruntime-core = {{ path = \"./does-not-exist/crates/runtime/core\" }}\n"
            ),
        )
        .unwrap();
        dir
    }

    #[test]
    fn generate_links_every_project_into_one_wrapper() {
        // The point of the merge: ONE extractor binary linking N project
        // libs. `inventory` registration is additive at link time, so the
        // emitted catalog spans every app.
        let a = fake_named_project("multi-a", "alpha-app");
        let b = fake_named_project("multi-b", "beta-app");
        let wrapper = generate_for_roots(&[b.clone(), a.clone()]).expect("generate");

        let cargo = fs::read_to_string(wrapper.join("Cargo.toml")).unwrap();
        assert!(cargo.contains("alpha-app = { path ="), "cargo: {cargo}");
        assert!(cargo.contains("beta-app = { path ="), "cargo: {cargo}");

        let main_rs = fs::read_to_string(wrapper.join("src/main.rs")).unwrap();
        assert!(main_rs.contains("use alpha_app as _;"), "main: {main_rs}");
        assert!(main_rs.contains("use beta_app as _;"), "main: {main_rs}");

        // Named for the project set, in sorted order — so the roots
        // arriving as [b, a] above still land in the same wrapper as
        // [a, b] would.
        assert!(
            wrapper.ends_with("alpha-app+beta-app/catalog"),
            "{wrapper:?}"
        );

        let _ = fs::remove_dir_all(&a);
        let _ = fs::remove_dir_all(&b);
    }

    #[test]
    fn multi_project_generate_is_order_independent_and_idempotent() {
        // Root order must not churn cargo fingerprints: a watch-triggered
        // regenerate that rewrote files would force a rebuild every time.
        let a = fake_named_project("ord-a", "alpha-app");
        let b = fake_named_project("ord-b", "beta-app");
        let first = generate_for_roots(&[a.clone(), b.clone()]).expect("first");
        let main_path = first.join("src/main.rs");
        let mtime1 = fs::metadata(&main_path).unwrap().modified().unwrap();

        let second = generate_for_roots(&[b.clone(), a.clone()]).expect("second");
        assert_eq!(first, second, "root order must not move the wrapper");
        let mtime2 = fs::metadata(&main_path).unwrap().modified().unwrap();
        assert_eq!(mtime1, mtime2, "reordered roots must not rewrite files");

        let _ = fs::remove_dir_all(&a);
        let _ = fs::remove_dir_all(&b);
    }

    #[test]
    fn collect_workspace_projects_takes_only_members_marked_idealyst() {
        // The framework's own marker for "this crate is a project" is
        // `[package.metadata.idealyst]` — the key `parse_manifest` reads.
        // Plain library members are NOT wrapped directly: their
        // components still arrive via the apps that depend on them, and
        // wrapping every lib in a big workspace would build far more than
        // the catalog needs.
        let meta = json!({
            "workspace_members": ["app-a 0.1.0 (path+file:///w/a)", "lib-b 0.1.0 (path+file:///w/b)"],
            "packages": [
                {
                    "id": "app-a 0.1.0 (path+file:///w/a)",
                    "name": "app-a",
                    "manifest_path": "/w/a/Cargo.toml",
                    "metadata": {"idealyst": {"app_name": "A"}}
                },
                {
                    "id": "lib-b 0.1.0 (path+file:///w/b)",
                    "name": "lib-b",
                    "manifest_path": "/w/b/Cargo.toml",
                    "metadata": null
                },
                {
                    // A non-member (a path dependency outside the
                    // workspace) is never wrapped, marker or not.
                    "id": "vendor-c 0.1.0 (path+file:///elsewhere)",
                    "name": "vendor-c",
                    "manifest_path": "/elsewhere/Cargo.toml",
                    "metadata": {"idealyst": {}}
                }
            ]
        });
        assert_eq!(
            collect_workspace_projects(&meta),
            vec![PathBuf::from("/w/a")]
        );
    }

    #[test]
    fn collect_workspace_projects_is_sorted_and_deduped() {
        // Byte-stability of the generated wrapper depends on this order.
        let meta = json!({
            "workspace_members": ["z 0.1.0 (p)", "a 0.1.0 (q)"],
            "packages": [
                {"id": "z 0.1.0 (p)", "name": "z", "manifest_path": "/w/z/Cargo.toml",
                 "metadata": {"idealyst": {}}},
                {"id": "a 0.1.0 (q)", "name": "a", "manifest_path": "/w/a/Cargo.toml",
                 "metadata": {"idealyst": {}}}
            ]
        });
        assert_eq!(
            collect_workspace_projects(&meta),
            vec![PathBuf::from("/w/a"), PathBuf::from("/w/z")]
        );
    }

    #[test]
    fn collect_workspace_projects_empty_when_nothing_is_marked() {
        // A workspace of plain crates yields nothing, and the caller
        // turns that into an explanatory error rather than an empty
        // catalog that looks like "this project has no components".
        let meta = json!({
            "workspace_members": ["lib 0.1.0 (p)"],
            "packages": [
                {"id": "lib 0.1.0 (p)", "name": "lib",
                 "manifest_path": "/w/lib/Cargo.toml", "metadata": null}
            ]
        });
        assert!(collect_workspace_projects(&meta).is_empty());
    }

    #[test]
    fn wrapper_name_is_the_sole_project_name_when_there_is_one() {
        // Single-project wrappers must land at exactly the path they
        // always did — this is what keeps existing on-disk wrappers and
        // their warm build caches valid across the upgrade.
        assert_eq!(wrapper_name(&["solo-app"]), "solo-app");
    }

    #[test]
    fn wrapper_package_name_is_a_legal_cargo_package_name() {
        // Regression: the `+` that separates projects in the wrapper's
        // DIRECTORY name is illegal in a cargo PACKAGE name. Emitting it
        // made cargo reject the generated manifest, so the extractor
        // exited 101 and the catalog silently stayed empty.
        let pkg = wrapper_package_name(&["alpha-app", "beta-app"]);
        assert!(!pkg.contains('+'), "{pkg}");
        assert!(
            pkg.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_'),
            "package name must be cargo-legal: {pkg}"
        );
        // Single-project wrappers keep the name they always had.
        assert_eq!(wrapper_package_name(&["solo-app"]), "solo-app");
    }

    #[test]
    fn wrapper_name_truncates_a_large_project_set() {
        // A monorepo with many apps must not produce an unusable path.
        let owned: Vec<String> = (0..30)
            .map(|i| format!("some-fairly-long-app-name-{i}"))
            .collect();
        let many: Vec<&str> = owned.iter().map(String::as_str).collect();
        let name = wrapper_name(&many);
        assert!(name.ends_with("+29-more"), "{name}");
        assert!(name.len() < 80, "{name}");
    }

    #[test]
    fn generate_emits_a_runnable_catalog_wrapper() {
        let project = fake_project("emit");
        let wrapper = generate(&project).expect("generate wrapper");

        // Lives under the project's `target/idealyst/<name>/catalog`.
        assert!(wrapper.ends_with("target/idealyst/my-app/catalog"), "{:?}", wrapper);

        let cargo = fs::read_to_string(wrapper.join("Cargo.toml")).unwrap();
        // A `catalog` bin the MCP server can run.
        assert!(cargo.contains("[[bin]]"));
        assert!(cargo.contains("name = \"catalog\""));
        // runtime-core with the `catalog` feature on — the emission lever.
        assert!(cargo.contains("runtime-core ="));
        assert!(cargo.contains("features = [\"catalog\"]"), "cargo: {cargo}");
        // Path-deps the project under its package name.
        assert!(cargo.contains("my-app = { path ="), "cargo: {cargo}");
        // Standalone so the parent workspace doesn't claim it.
        assert!(cargo.contains("[workspace]"));

        let main_rs = fs::read_to_string(wrapper.join("src/main.rs")).unwrap();
        // Imports by LIB name (hyphen → underscore) and dumps the catalog.
        assert!(main_rs.contains("use my_app as _;"), "main: {main_rs}");
        assert!(main_rs.contains("dump_catalog_json"));

        // Build output goes to the dedicated sidecar dir, NOT the bare
        // target dir — otherwise every catalog extraction takes the same
        // cargo build lock as the user's `cargo check` and rust-analyzer.
        let cfg = fs::read_to_string(wrapper.join(".cargo/config.toml")).unwrap();
        let target_line = cfg
            .lines()
            .find(|l| l.starts_with("target-dir"))
            .unwrap_or_else(|| panic!("no target-dir in config: {cfg}"));
        assert!(
            target_line.contains(SIDECAR_TARGET_DIR),
            "wrapper must build into the {SIDECAR_TARGET_DIR} sidecar: {target_line}"
        );
        assert!(
            !target_line.trim_end().ends_with("target\""),
            "wrapper must not build into the bare target dir: {target_line}"
        );

        let _ = fs::remove_dir_all(&project);
    }

    #[test]
    fn sidecar_target_dir_is_a_child_of_the_cargo_target_root() {
        // Peer of `idealyst-web-*` / `idealyst-dev-server`: rooted at the
        // same cargo target dir so sibling projects under one framework
        // source still share a warm dependency cache, but namespaced so
        // the build lock is ours alone.
        let src = FrameworkSource::Workspace {
            root: PathBuf::from("/fw"),
        };
        let dir = sidecar_target_dir(&src, Path::new("/proj"));
        assert_eq!(dir, PathBuf::from("/fw/target").join(SIDECAR_TARGET_DIR));
        assert_eq!(dir.parent().unwrap(), src.cargo_target_dir(Path::new("/proj")));
    }

    #[test]
    fn generate_is_idempotent() {
        let project = fake_project("idem");
        let wrapper = generate(&project).expect("first generate");
        let main_path = wrapper.join("src/main.rs");
        let mtime1 = fs::metadata(&main_path).unwrap().modified().unwrap();
        // Second call with identical inputs must not rewrite the files
        // (would bump mtime and invalidate cargo's fingerprint).
        let wrapper2 = generate(&project).expect("second generate");
        assert_eq!(wrapper, wrapper2);
        let mtime2 = fs::metadata(&main_path).unwrap().modified().unwrap();
        assert_eq!(mtime1, mtime2, "idempotent regenerate must not rewrite files");

        let _ = fs::remove_dir_all(&project);
    }

    use build_ios::{FrameworkSource, GitRef};
    use serde_json::json;

    /// A `cargo metadata`-shaped document: the project `my-app` directly
    /// depends on `idea-ui` (a component lib — depends on runtime-core,
    /// has a lib target), `serde` (no runtime-core dep), a dev-dep
    /// `dev-tool` (component-ish but dev kind), `proc-mac` (proc-macro
    /// only) and `runtime-core` itself.
    fn sample_metadata() -> Value {
        json!({
            "packages": [
                {
                    "id": "app",
                    "name": "my-app",
                    "manifest_path": "/proj/Cargo.toml",
                    "source": null,
                    "dependencies": [
                        {"name": "runtime-core", "kind": null},
                        {"name": "idea-ui", "kind": null},
                        {"name": "serde", "kind": null}
                    ],
                    "targets": [{"name": "my_app", "kind": ["lib"]}]
                },
                {
                    "id": "ui",
                    "name": "idea-ui",
                    "manifest_path": "/ws/crates/ui/idea-ui/Cargo.toml",
                    "source": null,
                    "dependencies": [{"name": "runtime-core", "kind": null}],
                    "targets": [{"name": "idea_ui", "kind": ["lib"]}]
                },
                {
                    "id": "serde",
                    "name": "serde",
                    "manifest_path": "/reg/serde/Cargo.toml",
                    "source": "registry+https://github.com/rust-lang/crates.io-index",
                    "dependencies": [],
                    "targets": [{"name": "serde", "kind": ["lib"]}]
                },
                {
                    "id": "devtool",
                    "name": "dev-tool",
                    "manifest_path": "/ws/crates/dev-tool/Cargo.toml",
                    "source": null,
                    "dependencies": [{"name": "runtime-core", "kind": null}],
                    "targets": [{"name": "dev_tool", "kind": ["lib"]}]
                },
                {
                    "id": "pm",
                    "name": "proc-mac",
                    "manifest_path": "/ws/crates/proc-mac/Cargo.toml",
                    "source": null,
                    "dependencies": [{"name": "runtime-core", "kind": null}],
                    "targets": [{"name": "proc_mac", "kind": ["proc-macro"]}]
                },
                {
                    "id": "rc",
                    "name": "runtime-core",
                    "manifest_path": "/ws/crates/runtime/core/Cargo.toml",
                    "source": null,
                    "dependencies": [],
                    "targets": [{"name": "runtime_core", "kind": ["lib"]}]
                }
            ],
            "resolve": {
                "root": "app",
                "nodes": [
                    {
                        "id": "app",
                        "deps": [
                            {"pkg": "ui", "dep_kinds": [{"kind": null}]},
                            {"pkg": "serde", "dep_kinds": [{"kind": null}]},
                            {"pkg": "devtool", "dep_kinds": [{"kind": "dev"}]},
                            {"pkg": "pm", "dep_kinds": [{"kind": null}]},
                            {"pkg": "rc", "dep_kinds": [{"kind": null}]}
                        ]
                    }
                ]
            }
        })
    }

    #[test]
    fn collect_force_links_only_component_library_deps_in_workspace_mode() {
        let src = FrameworkSource::Workspace { root: PathBuf::from("/ws") };
        let deps = collect_forced_deps(
            &sample_metadata(),
            &src,
            Path::new("/proj/Cargo.toml"),
            "my-app",
        );
        // Only idea-ui qualifies: serde lacks a runtime-core dep,
        // dev-tool is a dev-dependency, proc-mac is proc-macro-only,
        // runtime-core and the project itself are excluded by name.
        assert_eq!(deps.len(), 1, "got: {deps:?}");
        let d = &deps[0];
        assert_eq!(d.pkg_name, "idea-ui");
        assert_eq!(d.lib_ident, "idea_ui");
        // Workspace mode → path dep to the resolved manifest dir, which
        // is exactly what the project's own dep resolves to (unifies).
        assert_eq!(d.dep_line, "{ path = \"/ws/crates/ui/idea-ui\" }");
    }

    /// Regression: a component library is force-linked because it
    /// declares a `runtime-core` dependency. When the author surface
    /// briefly lived in a separate `runtime-facade` package, this marker
    /// matched only the old spelling, so `idealyst mcp` / `catalog-json`
    /// / `export` stopped force-linking idea-ui, idea-theme,
    /// icons-lucide — their `inventory::submit!` ctors never reached the
    /// extractor binary and the catalog lost every dependency component.
    ///
    /// `runtime-core` itself must still be excluded by name: the
    /// wrapper already declares it, and re-declaring would duplicate the
    /// dep table key.
    #[test]
    fn collect_force_links_runtime_core_deps() {
        let mut meta = sample_metadata();
        meta["packages"][1]["dependencies"] = json!([{"name": "runtime-core", "kind": null}]);
        meta["packages"].as_array_mut().unwrap().push(json!({
            "id": "fc",
            "name": "runtime-core",
            "manifest_path": "/ws/crates/runtime/core/Cargo.toml",
            "source": null,
            "dependencies": [],
            "targets": [{"name": "runtime_core", "kind": ["lib"]}]
        }));
        meta["resolve"]["nodes"][0]["deps"]
            .as_array_mut()
            .unwrap()
            .push(json!({"pkg": "fc", "dep_kinds": [{"kind": null}]}));

        let src = FrameworkSource::Workspace { root: PathBuf::from("/ws") };
        let deps = collect_forced_deps(&meta, &src, Path::new("/proj/Cargo.toml"), "my-app");

        assert_eq!(deps.len(), 1, "got: {deps:?}");
        assert_eq!(deps[0].pkg_name, "idea-ui");
        assert!(
            !deps.iter().any(|d| d.pkg_name == "runtime-core"),
            "runtime-core is already a wrapper dep — must not be re-declared: {deps:?}"
        );
    }

    #[test]
    fn collect_enables_catalog_feature_on_deps_that_declare_one() {
        // A crate that gates its MCP self-registration behind a `catalog`
        // feature (like `icons-lucide`'s IconSetEntry) must be force-linked
        // WITH that feature on — otherwise the submission compiles out and
        // its slice is empty. Crates without a `catalog` feature (idea-ui)
        // must stay bare so we don't enable a feature that doesn't exist.
        let mut meta = sample_metadata();
        // Add an icons-lucide-shaped package with a `catalog` feature.
        meta["packages"].as_array_mut().unwrap().push(json!({
            "id": "icons",
            "name": "icons-lucide",
            "manifest_path": "/ws/crates/ui/icons-lucide/Cargo.toml",
            "source": null,
            "dependencies": [{"name": "runtime-core", "kind": null}],
            "targets": [{"name": "icons_lucide", "kind": ["lib"]}],
            "features": {"registry": [], "catalog": ["dep:mcp-catalog"]}
        }));
        meta["resolve"]["nodes"][0]["deps"]
            .as_array_mut()
            .unwrap()
            .push(json!({"pkg": "icons", "dep_kinds": [{"kind": null}]}));

        let src = FrameworkSource::Workspace { root: PathBuf::from("/ws") };
        let deps = collect_forced_deps(&meta, &src, Path::new("/proj/Cargo.toml"), "my-app");
        assert_eq!(deps.len(), 2, "got: {deps:?}");

        let icons = deps.iter().find(|d| d.pkg_name == "icons-lucide").expect("icons-lucide forced");
        assert_eq!(
            icons.dep_line,
            "{ path = \"/ws/crates/ui/icons-lucide\", features = [\"catalog\"] }",
            "icons-lucide must be force-linked with its catalog feature on",
        );
        // idea-ui declares no `catalog` feature → stays bare (regression
        // guard: we don't enable a non-existent feature).
        let ui = deps.iter().find(|d| d.pkg_name == "idea-ui").expect("idea-ui forced");
        assert_eq!(ui.dep_line, "{ path = \"/ws/crates/ui/idea-ui\" }");
    }

    #[test]
    fn collect_enables_catalog_feature_in_git_mode_too() {
        let url = "https://github.com/IdealystIO/idealyst-native";
        let src = FrameworkSource::Git {
            url: url.to_string(),
            refspec: GitRef::Rev("abc123".to_string()),
        };
        let mut meta = sample_metadata();
        meta["packages"].as_array_mut().unwrap().push(json!({
            "id": "icons",
            "name": "icons-lucide",
            "manifest_path": "/ws/crates/ui/icons-lucide/Cargo.toml",
            "source": format!("git+{url}?rev=abc123#abc123"),
            "dependencies": [{"name": "runtime-core", "kind": null}],
            "targets": [{"name": "icons_lucide", "kind": ["lib"]}],
            "features": {"catalog": ["dep:mcp-catalog"]}
        }));
        // idea-ui must also be sourced from the framework git repo to be kept.
        meta["packages"][1]["source"] = json!(format!("git+{url}?rev=abc123#abc123"));
        meta["resolve"]["nodes"][0]["deps"]
            .as_array_mut()
            .unwrap()
            .push(json!({"pkg": "icons", "dep_kinds": [{"kind": null}]}));

        let deps = collect_forced_deps(&meta, &src, Path::new("/proj/Cargo.toml"), "my-app");
        let icons = deps.iter().find(|d| d.pkg_name == "icons-lucide").expect("icons-lucide forced");
        assert_eq!(
            icons.dep_line,
            format!("{{ git = \"{url}\", rev = \"abc123\", features = [\"catalog\"] }}"),
        );
    }

    #[test]
    fn collect_emits_git_deps_for_framework_crates_and_skips_foreign_in_git_mode() {
        let url = "https://github.com/IdealystIO/idealyst-native";
        let src = FrameworkSource::Git {
            url: url.to_string(),
            refspec: GitRef::Tag("v0.1.0".to_string()),
        };
        // idea-ui resolved from the framework git repo → force-linked
        // via a matching git dep.
        let mut meta = sample_metadata();
        meta["packages"][1]["source"] =
            json!(format!("git+{url}?tag=v0.1.0#abc123"));
        // A third-party component lib resolved from crates.io: depends on
        // runtime-core but its source isn't the framework repo, so we
        // can't re-declare it safely in git mode — must be skipped.
        meta["packages"][2]["dependencies"] = json!([{"name": "runtime-core", "kind": null}]);

        let deps = collect_forced_deps(&meta, &src, Path::new("/proj/Cargo.toml"), "my-app");
        assert_eq!(deps.len(), 1, "got: {deps:?}");
        assert_eq!(deps[0].pkg_name, "idea-ui");
        assert_eq!(
            deps[0].dep_line,
            format!("{{ git = \"{url}\", tag = \"v0.1.0\" }}")
        );
    }

    #[test]
    fn collect_falls_back_to_manifest_path_when_resolve_root_missing() {
        // A virtual-workspace manifest leaves resolve.root null; we must
        // still find the root by matching the project manifest path.
        let mut meta = sample_metadata();
        meta["resolve"]["root"] = Value::Null;
        let src = FrameworkSource::Workspace { root: PathBuf::from("/ws") };
        let deps = collect_forced_deps(&meta, &src, Path::new("/proj/Cargo.toml"), "my-app");
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].pkg_name, "idea-ui");
    }

    #[test]
    fn collect_returns_empty_on_malformed_metadata() {
        let src = FrameworkSource::Workspace { root: PathBuf::from("/ws") };
        assert!(collect_forced_deps(&json!({}), &src, Path::new("/proj/Cargo.toml"), "my-app").is_empty());
        assert!(collect_forced_deps(&json!({"packages": []}), &src, Path::new("/proj/Cargo.toml"), "my-app").is_empty());
    }

    #[test]
    fn generate_rejects_a_workspace_root() {
        // A bare `[workspace]` with no `[package]` is not a project; the
        // caller turns this Err into a graceful "no catalog" warning.
        let dir = std::env::temp_dir().join(format!(
            "idealyst-catwrap-ws-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
        assert!(generate(&dir).is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
