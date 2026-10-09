//! `idealyst catalog-json` — print the project's full catalog JSON to
//! stdout.
//!
//! The stable machine-facing entry point for editor tooling (the VS Code
//! completion extension shells out to this), CI, and anything else that
//! wants the catalog without speaking MCP. Reuses the exact pipeline
//! `idealyst mcp` uses internally: generate the ephemeral catalog
//! wrapper crate (idempotent — no rewrite when nothing changed), then
//! `cargo run -q --bin catalog`, whose stdout IS the catalog JSON
//! (`mcp_catalog::catalog_json()` — components, props schemas,
//! primitives, macros, utilities, types, guides).
//!
//! `DIR` may be a project directory or a cargo **workspace root**: the
//! root is expanded through the same [`resolve_project_roots`] the MCP
//! server uses, so every member with `[package.metadata.idealyst]` is
//! wrapped into one catalog (each component keeps its crate in
//! `module_path`). Editor tooling that only knows the workspace folder
//! can therefore hand it over verbatim — before this it failed to parse
//! the root as a project and produced nothing, which the VS Code
//! extension surfaced as "no completion at all" in monorepos.
//!
//! `--deps-only` builds the other wrapper flavour: no workspace member is
//! linked, only the non-member crates the projects depend on (see
//! [`generate_deps_only`]). That is the half editor tooling wants from
//! a compile — the members it reads from source with `catalog-scan`.
//!
//! First run compiles the wrapper (the project graph with the `catalog`
//! feature on) — minutes cold, seconds warm; cargo caches everything.
//! The wrapper and its build live under the project's cargo workspace
//! target dir (`target/idealyst/…`, `target/idealyst-mcp/`), shared by
//! every member crate, and each run trims superseded incremental caches
//! (see [`catalog_target_root`] / [`prune_incremental`]).
//! Build chatter goes to stderr; stdout stays pure JSON.
//!
//! `--scan` produces the same document without compiling the workspace:
//! the `--deps-only` extractor supplies everything outside it, and the
//! members are read from source by running the catalog macros' own
//! expansion over them (`catalog-scan`; planned by
//! [`scan_plan`](super::scan_plan)). The extractor only rebuilds when the
//! dependency graph changes, so a refresh after an edit costs a
//! `cargo metadata`, a no-op `cargo build`, and a parse — not a build of
//! the app. This is what `idealyst mcp` refreshes with. When the scan
//! cannot reproduce what the macros would register (a catalog type
//! registered by hand, a `macro_rules!` it cannot expand), it says why on
//! stderr and compiles the full catalog instead; a source file that does
//! not parse is skipped with a warning, since a build could not compile
//! it either.
//!
//! [`generate_deps_only`]: super::catalog_wrapper::generate_deps_only
//!
//! [`resolve_project_roots`]: super::catalog_wrapper::resolve_project_roots
//! [`catalog_target_root`]: super::catalog_wrapper::catalog_target_root
//! [`prune_incremental`]: super::catalog_wrapper::prune_incremental

use std::path::PathBuf;
use std::process::Command;

use anyhow::{bail, Context, Result};

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Project directories (or one workspace root).
    #[arg(default_value = ".")]
    pub dirs: Vec<PathBuf>,
    /// Link only the project's dependencies (idea-ui, icon packs, SDKs),
    /// never a workspace member: the catalog of what the project USES.
    /// Pair with `catalog-scan` for what it writes. A compile error in
    /// the project can't break this build, and it only re-runs when the
    /// dependency graph changes.
    #[arg(long, conflicts_with = "scan")]
    pub deps_only: bool,
    /// Read the workspace members from source instead of compiling them;
    /// only the dependencies are compiled. Same document, without
    /// building the app.
    #[arg(long)]
    pub scan: bool,
}

pub fn run(args: Args) -> Result<()> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for dir in &args.dirs {
        let dir = std::fs::canonicalize(dir).with_context(|| format!("cannot resolve project dir {}", dir.display()))?;
        for root in super::catalog_wrapper::resolve_project_roots(&dir).context("resolve the idealyst project(s) to catalog")? {
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
    }
    let json = if args.scan {
        scanned_catalog(&roots)?
    } else if args.deps_only {
        compiled_catalog(&super::catalog_wrapper::generate_deps_only(&roots).context("generate the catalog wrapper crate")?)?
    } else {
        compiled_catalog(&super::catalog_wrapper::generate_for_roots(&roots).context("generate the catalog wrapper crate")?)?
    };
    println!("{json}");
    Ok(())
}

/// Build (and prune the sidecar of) the extractor in `wrapper_dir` and
/// return what it prints.
fn compiled_catalog(wrapper_dir: &std::path::Path) -> Result<String> {
    let exe = super::catalog_wrapper::build_extractor(wrapper_dir, "catalog")
        .context("build the catalog wrapper (see stderr above)")?;
    let out = Command::new(&exe)
        .stderr(std::process::Stdio::inherit())
        .output()
        .with_context(|| format!("run the catalog extractor {}", exe.display()))?;
    if !out.status.success() {
        bail!("catalog extractor failed (see stderr above)");
    }
    Ok(String::from_utf8(out.stdout).context("the catalog extractor printed non-UTF-8")?.trim_end().to_string())
}

/// The catalog for `roots` with the workspace members read from source —
/// `--scan` (see the module docs).
pub fn scanned_catalog(roots: &[PathBuf]) -> Result<String> {
    let plan = super::scan_plan::plan(roots).context("plan the source scan")?;
    let scanned = catalog_scan::scan(&plan.crates, &plan.macro_deps);
    for skipped in &scanned.skipped {
        eprintln!("[idealyst] catalog scan: {skipped}");
    }
    // A refused member is compiled with the dependencies instead.
    let mut refused: Vec<String> = Vec::new();
    for r in &scanned.refused {
        eprintln!("[idealyst] catalog scan: compiling `{}` instead of reading it ({})", plan.packages[r.krate], r.error);
        refused.push(plan.packages[r.krate].clone());
    }
    let deps = dependency_catalog(roots, &plan, &refused)?;
    let deps: serde_json::Value = serde_json::from_str(&deps).context("parse the dependency catalog")?;
    let mut parts = mcp_catalog::CatalogParts::from_json(&deps).context("read the dependency catalog")?;
    // A member the scan read may also sit in the extractor, compiled as a
    // refused member's dependency; the scan's copy wins.
    parts.extend_replacing(scanned.parts);
    Ok(serde_json::to_string_pretty(&parts.to_json())?)
}

/// The compiled half of `--scan`: the `--deps-only` extractor's catalog
/// (plus the refused members), from a cache when that cannot have
/// changed.
///
/// Even with nothing to rebuild, producing it costs a handful of
/// `cargo metadata` calls and a no-op `cargo build` — seconds on a large
/// workspace, on every refresh. When every dependency outside the
/// workspace comes from a registry or git (so its source is fixed by the
/// lockfile) and no member is compiled with it, the document is a
/// function of the files [`ScanPlan::deps_inputs`] lists, so it is kept
/// at `<target>/idealyst-mcp/catalog-deps-cache.json` keyed by their
/// contents (and this CLI's version) and reused while they match.
///
/// [`ScanPlan::deps_inputs`]: super::scan_plan::ScanPlan::deps_inputs
fn dependency_catalog(roots: &[PathBuf], plan: &super::scan_plan::ScanPlan, refused: &[String]) -> Result<String> {
    let build = || -> Result<String> {
        compiled_catalog(
            &super::catalog_wrapper::generate_deps_and(roots, refused).context("generate the dependency catalog wrapper")?,
        )
    };
    let (Some(inputs), true) = (&plan.deps_inputs, refused.is_empty()) else {
        return build();
    };
    let key = deps_cache_key(inputs, roots);
    let cache = plan.target_dir.join(super::catalog_wrapper::SIDECAR_TARGET_DIR).join("catalog-deps-cache.json");
    if let Ok(text) = std::fs::read_to_string(&cache) {
        if let Some(rest) = text.strip_prefix(&format!("{key}\n")) {
            return Ok(rest.to_string());
        }
    }
    let json = build()?;
    // Best effort: a cache that cannot be written costs the next refresh
    // a rebuild, nothing else. Written whole, then renamed into place, so
    // a concurrent reader never sees half a document.
    if let Some(dir) = cache.parent() {
        let tmp = dir.join(format!("catalog-deps-cache.{}.tmp", std::process::id()));
        if std::fs::create_dir_all(dir).is_ok() && std::fs::write(&tmp, format!("{key}\n{json}")).is_ok() {
            let _ = std::fs::rename(&tmp, &cache);
        }
    }
    Ok(json)
}

/// The cache key over `inputs`' contents (absent files count as absent),
/// the projects, and this CLI's version.
fn deps_cache_key(inputs: &[PathBuf], roots: &[PathBuf]) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    env!("CARGO_PKG_VERSION").hash(&mut h);
    roots.hash(&mut h);
    for f in inputs {
        f.hash(&mut h);
        std::fs::read(f).ok().hash(&mut h);
    }
    format!("catalog-deps {:016x}", h.finish())
}
