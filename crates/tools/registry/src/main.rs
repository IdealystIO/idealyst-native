//! `registry` — release tooling for the idealyst cargo registry.
//!
//!   registry migrate            one-time: give every publishable crate its own version
//!   registry plan               what would be released, and at what version
//!   registry build --out DIR    package + lay out a complete registry locally
//!   registry publish            build, upload to S3, invalidate CloudFront
//!
//! `plan`, `build` and `publish` take `--force <CRATE>` to release a crate
//! whose source is unchanged but whose PUBLISHED manifest is wrong. Nothing
//! else can reach that case: the plan is driven by "did this directory
//! change?", and correcting recorded metadata never changes a directory.
//!
//! The plan also runs an API check (`api.rs`) on every crate that changed:
//! it parses the crate's source at its last release and at HEAD, and a
//! breaking change — a field added to an all-pub struct, a variant added to
//! an exhaustive enum, a removed `pub` item — raises the crate to a major
//! whatever its commit subjects said. `--skip-api-check <CRATE>` waives it
//! for a false positive.
//!
//! The default host is set by `--bucket` / `IDEALYST_REGISTRY_BUCKET` and
//! `--distribution-id` / `IDEALYST_REGISTRY_DISTRIBUTION`.

mod api;
mod deploy;
mod index;
mod manifest;
mod orphans;
mod version;
mod workspace;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

use version::{Bump, ReleaseState, Released};
use workspace::Workspace;

#[derive(Parser)]
#[command(name = "registry", about = "Release the idealyst crates to the sparse registry")]
struct Cli {
    /// Workspace root. Defaults to the current directory.
    #[arg(long, default_value = ".", global = true)]
    dir: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Pin per-crate versions and give internal deps version requirements.
    Migrate {
        /// Show what would change without writing anything.
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        remote: RemoteArgs,
    },
    /// Report the release each publishable crate has earned.
    Plan(RemoteArgs),
    /// Package every crate that needs releasing and lay out a registry tree.
    Build {
        #[command(flatten)]
        remote: RemoteArgs,
        #[command(flatten)]
        build: BuildArgs,
    },
    /// Build, then upload and invalidate.
    Publish {
        #[command(flatten)]
        remote: RemoteArgs,
        #[command(flatten)]
        build: BuildArgs,
        /// Actually write to S3. Without it the command stops after staging,
        /// having touched nothing remote.
        #[arg(long)]
        execute: bool,
    },
    /// Mark a published version yanked (or, with `--undo`, un-yank it).
    ///
    /// Cargo never picks a yanked version for a new resolution, but a lockfile
    /// that already names it keeps working. The tarball stays up. For a
    /// release that should not be resolved by default, such as a patch that
    /// pulled in a dependency's new major.
    Yank {
        /// The crate, as published.
        #[arg(value_name = "CRATE")]
        name: String,
        /// The exact version to yank.
        #[arg(value_name = "VERSION")]
        version: String,
        /// Clear the yank instead of setting it.
        #[arg(long)]
        undo: bool,
        #[command(flatten)]
        remote: RemoteArgs,
        /// Actually write to S3. Without it the command prints the index line
        /// it would write and stops.
        #[arg(long)]
        execute: bool,
    },
}

#[derive(clap::Args, Clone)]
struct BuildArgs {
    #[arg(long, default_value = "target/registry")]
    out: PathBuf,
    /// Let `cargo package` compile each crate before accepting it. Slow
    /// across 165 crates, but the only check that a published tarball is
    /// actually self-contained.
    #[arg(long)]
    verify: bool,
    /// Restrict the release to these crates. Their dependencies are NOT
    /// pulled in — use it to re-cut a single crate, or to rehearse the
    /// pipeline on one leaf. Other crates' history is not read at all, so a
    /// crate whose recorded release commit is missing from this checkout
    /// does not block releasing the named ones.
    #[arg(long = "only", value_name = "CRATE")]
    only: Vec<String>,
    /// Stage a release from an uncommitted tree. The recorded commit will
    /// not describe the published bytes, so this is for rehearsals only —
    /// never for a real publish.
    #[arg(long)]
    allow_dirty: bool,
}

#[derive(clap::Args, Clone)]
struct RemoteArgs {
    #[arg(long, env = "IDEALYST_REGISTRY_BUCKET")]
    bucket: Option<String>,
    #[arg(long, env = "IDEALYST_REGISTRY_DISTRIBUTION")]
    distribution_id: Option<String>,
    /// Public base URL the registry is served from.
    #[arg(
        long,
        env = "IDEALYST_REGISTRY_URL",
        default_value = "https://crates.idealyst.io"
    )]
    url: String,
    /// Treat every publishable crate as unreleased. Used for the first
    /// publish, and to rebuild a registry from scratch.
    #[arg(long)]
    from_scratch: bool,
    /// Release these crates at a patch bump even though nothing in their
    /// directory changed. For correcting a published manifest — the source is
    /// fine, the recorded metadata is not — which no source diff will ever
    /// trigger on its own. Combine with `--bump CRATE=LEVEL` to re-cut it at
    /// a higher level.
    #[arg(long = "force", value_name = "CRATE")]
    force: Vec<String>,
    /// Raise a crate's bump level: `--bump runtime-shared=minor`. For work
    /// whose commit subjects under-classify it — additive public API landing
    /// under a free-form subject reads as a patch, because the level comes
    /// from the subject. Raise-only: asking for less than the crate earned is
    /// an error, not a silent downgrade.
    #[arg(long = "bump", value_name = "CRATE=LEVEL")]
    bump: Vec<String>,
    /// Report this crate's API-check findings without raising it to a
    /// major. For a false positive only — a `pub` item the crate documents
    /// as internal, a sealed trait gaining a method. The findings still
    /// print, so the waiver is visible in the plan.
    #[arg(long = "skip-api-check", value_name = "CRATE")]
    skip_api_check: Vec<String>,
    /// Leave this crate out of the release without reading its history.
    /// For a crate whose recorded release commit is not in this clone (it
    /// was released from a machine that has not pushed): its change
    /// detection cannot run here, and releasing it from here would publish
    /// a version that lacks the unpushed work. If the plan needs to
    /// republish it anyway (a dependency of it goes major), the plan fails
    /// instead of leaving it on the old major.
    #[arg(long = "skip", value_name = "CRATE")]
    skip: Vec<String>,
    /// The registry's name as consumers spell it in `.cargo/config.toml`.
    #[arg(long, env = "IDEALYST_REGISTRY_NAME", default_value = "idealyst")]
    registry_name: String,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let ws = Workspace::load(&cli.dir)?;
    match cli.cmd {
        Cmd::Migrate { dry_run, remote } => migrate(&ws, dry_run, &remote),
        Cmd::Plan(r) => {
            let (plan, _) = plan(&ws, &r, &[])?;
            report(&ws, &plan);
            Ok(())
        }
        Cmd::Build { remote, build: b } => {
            let (plan, ahead) = plan(&ws, &remote, &b.only)?;
            let plan = scoped(plan, &b.only);
            report(&ws, &plan);
            build(&ws, &plan, &ahead, &remote, &b, false)?;
            println!("\nstaged in {}", b.out.display());
            Ok(())
        }
        Cmd::Publish { remote, build: b, execute } => {
            let (plan, ahead) = plan(&ws, &remote, &b.only)?;
            let plan = scoped(plan, &b.only);
            report(&ws, &plan);
            let state = build(&ws, &plan, &ahead, &remote, &b, execute)?;
            if !execute {
                println!("\nstaged in {} — re-run with --execute to upload", b.out.display());
                return Ok(());
            }
            let target = target(&remote)?;
            // The per-crate uploads already happened inside the loop; this
            // just publishes the metadata and clears the edge caches.
            deploy::put_metadata(&b.out, &target, deploy::Metadata::WithReleaseState)?;
            deploy::invalidate(&target)?;
            println!("\npublished {} crates to {}", state.len(), remote.url);
            Ok(())
        }
        Cmd::Yank { name, version, undo, remote, execute } => {
            yank(&name, &version, !undo, &remote, execute)
        }
    }
}

/// `registry yank`: rewrite one line of a crate's index file and upload it.
fn yank(name: &str, version: &str, yanked: bool, r: &RemoteArgs, execute: bool) -> Result<()> {
    let target = target(r)?;
    let ipath = index::index_path(name);
    let body = deploy::fetch_index_file(&target, &ipath)?
        .with_context(|| format!("{name} has no index file — never published?"))?;
    let (new_body, changed) = index::set_yanked(&body, version, yanked)?;
    let verb = if yanked { "yank" } else { "un-yank" };
    if !changed {
        println!("{name} {version} is already {}", if yanked { "yanked" } else { "not yanked" });
        return Ok(());
    }
    if !execute {
        println!("would {verb} {name} {version} (index/{ipath}) — re-run with --execute");
        return Ok(());
    }
    deploy::put_index(&target, name, &ipath, &new_body)?;
    deploy::invalidate(&target)?;
    println!("{verb}ed {name} {version}");
    Ok(())
}

/// Narrow a plan to an explicit crate list, for a rehearsal or a re-cut.
fn scoped(
    plan: BTreeMap<String, Release>,
    only: &[String],
) -> BTreeMap<String, Release> {
    if only.is_empty() {
        return plan;
    }
    plan.into_iter().filter(|(n, _)| only.contains(n)).collect()
}

fn target(r: &RemoteArgs) -> Result<deploy::Target> {
    Ok(deploy::Target {
        bucket: r
            .bucket
            .clone()
            .context("no --bucket (or IDEALYST_REGISTRY_BUCKET) given")?,
        distribution_id: r.distribution_id.clone(),
    })
}

// ---------------------------------------------------------------------------
// migrate

/// Turn a lockstep workspace into one where each crate carries its own version.
fn migrate(ws: &Workspace, dry_run: bool, r: &RemoteArgs) -> Result<()> {
    let versions: BTreeMap<String, semver::Version> = ws
        .publishable()
        .map(|p| (p.name.clone(), p.version.clone()))
        .collect();

    // Directories, so a member can spell a sibling as `../client`.
    let dirs: BTreeMap<String, PathBuf> = ws
        .packages
        .values()
        .filter_map(|p| p.manifest_path.parent().map(|d| (p.name.clone(), d.to_path_buf())))
        .collect();
    let ws_extras = workspace_dep_extras(&ws.root)?;

    let mut pinned = 0;
    let mut delinked = 0;
    for p in ws.publishable() {
        let mut doc = manifest::read(&p.manifest_path)?;
        manifest::set_package_version(&mut doc, &p.version)?;
        let lookup = |n: &str| versions.get(n).cloned();
        let touched = manifest::version_literal_path_deps(&mut doc, &lookup, &r.registry_name)?;
        if !touched.is_empty() {
            println!("  {} — versioned literal path deps: {}", p.name, touched.join(", "));
        }

        let here = p.manifest_path.parent().unwrap_or(&ws.root).to_path_buf();
        let dev_lookup = |n: &str| -> Option<manifest::WsDep> {
            let target = dirs.get(n)?;
            Some(manifest::WsDep {
                rel_path: relpath(&here, target),
                extras: ws_extras.get(n).cloned().unwrap_or_default(),
            })
        };
        let dev = manifest::delink_internal_dev_deps(&mut doc, &dev_lookup)?;
        delinked += dev.len();

        if !dry_run {
            manifest::write(&p.manifest_path, &doc)?;
        }
        pinned += 1;
    }

    // Internal deps flow through [workspace.dependencies] for 727 of the 740
    // declarations in this workspace, so putting the requirement there means
    // a release edits one line per crate instead of one line per dependent.
    let root_manifest = ws.root.join("Cargo.toml");
    let mut root = manifest::read(&root_manifest)?;
    let mut added = 0;
    for p in ws.publishable() {
        let rel = pathdiff(&p.manifest_path, &ws.root);
        if manifest::set_workspace_dep_version(
            &mut root,
            &p.name,
            &p.version,
            &rel,
            &r.registry_name,
        )? {
            added += 1;
        }
    }
    if !dry_run {
        manifest::write(&root_manifest, &root)?;
        write_cargo_config(&ws.root, r)?;
    }

    println!(
        "\n{}pinned {pinned} crate versions; set {added} requirements in \
         [workspace.dependencies]; de-linked {delinked} internal dev-deps",
        if dry_run { "(dry run) " } else { "" }
    );
    Ok(())
}

/// Everything `[workspace.dependencies]` says about each internal crate apart
/// from `path` and `version`, so a de-linked dev-dep keeps its features.
fn workspace_dep_extras(root: &Path) -> Result<BTreeMap<String, Vec<(String, toml_edit::Value)>>> {
    let doc = manifest::read(&root.join("Cargo.toml"))?;
    let mut out = BTreeMap::new();
    let Some(deps) = doc
        .get("workspace")
        .and_then(|w| w.get("dependencies"))
        .and_then(|d| d.as_table_like())
    else {
        return Ok(out);
    };
    for (name, item) in deps.iter() {
        let Some(t) = item.as_table_like() else { continue };
        if t.get("path").is_none() {
            continue;
        }
        let extras: Vec<(String, toml_edit::Value)> = t
            .iter()
            .filter(|(k, _)| *k != "path" && *k != "version")
            .filter_map(|(k, v)| v.as_value().map(|v| (k.to_string(), v.clone())))
            .collect();
        out.insert(name.to_string(), extras);
    }
    Ok(out)
}

/// Relative path from one directory to another, in the `../sibling` form
/// cargo manifests use.
/// How many crates the registry already holds, for the landing page.
fn published_count(r: &RemoteArgs) -> Result<usize> {
    if r.from_scratch {
        return Ok(0);
    }
    let Some(raw) = deploy::fetch_release_state(&target(r)?)? else {
        return Ok(0);
    };
    let state: ReleaseState = serde_json::from_str(&raw).context("parsing releases.json")?;
    Ok(state.crates.len())
}

/// A human landing page at the bucket root.
///
/// A sparse registry has no browsable root — cargo only ever fetches
/// `index/…` and `crates/…` — so `https://crates.idealyst.io/` otherwise
/// answers 403 (the bucket deliberately grants no listing). Anyone who pastes
/// the registry URL into a browser, which is everyone at least once, deserves
/// the setup instructions rather than an access error.
fn write_landing_page(out: &Path, url: &str, registry: &str, crates: usize) -> Result<()> {
    let url = url.trim_end_matches('/');
    let html = format!(
        r#"<!doctype html>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>idealyst crate registry</title>
<style>
  :root {{ color-scheme: light dark; }}
  body {{ font: 15px/1.6 ui-sans-serif, system-ui, sans-serif; max-width: 44rem;
         margin: 4rem auto; padding: 0 1.5rem; }}
  code, pre {{ font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 13px; }}
  pre {{ background: color-mix(in srgb, currentColor 7%, transparent);
        padding: 1rem; border-radius: 6px; overflow-x: auto; }}
  .muted {{ opacity: .65; }}
</style>
<h1>idealyst crate registry</h1>
<p>A <a href="https://doc.rust-lang.org/cargo/reference/registry-index.html#sparse-protocol">sparse</a>
cargo registry hosting the idealyst framework crates.
Currently <strong>{crates}</strong> crates.</p>

<h2>Use it</h2>
<p>Add the registry to <code>.cargo/config.toml</code>:</p>
<pre>[registries.{registry}]
index = "sparse+{url}/index/"</pre>
<p>Then depend on crates by version:</p>
<pre>[dependencies]
idealyst = {{ version = "1.5.2", registry = "{registry}" }}
idea-ui  = {{ version = "1.8.0", registry = "{registry}" }}</pre>
<p><strong>Always include <code>registry = "{registry}"</code>.</strong> Many of these
crates have short names that belong to unrelated packages on crates.io; without
it cargo resolves the wrong one.</p>

<h2 class="muted">Browsing</h2>
<p class="muted">There is no index to browse — a sparse registry is just static
files. A crate's metadata lives at <code>{url}/index/&lt;path&gt;</code> using
cargo's index layout, e.g.
<a href="{url}/index/id/ea/idea-ui"><code>/index/id/ea/idea-ui</code></a>.</p>
"#
    );
    std::fs::write(out.join("index.html"), html)?;
    Ok(())
}

/// The checksum a version already carries in the index, if it is there.
fn published_checksum(index_lines: &str, version: &str) -> Result<Option<String>> {
    for line in index_lines.lines().filter(|l| !l.trim().is_empty()) {
        let v: serde_json::Value =
            serde_json::from_str(line).context("parsing an existing index line")?;
        if v.get("vers").and_then(|x| x.as_str()) == Some(version) {
            return Ok(v
                .get("cksum")
                .and_then(|x| x.as_str())
                .map(str::to_owned));
        }
    }
    Ok(None)
}

/// Teach this workspace where the registry lives.
///
/// `[workspace.dependencies]` now names `registry = "<name>"` on every internal
/// crate, and cargo refuses to build until that name resolves to an index. It
/// belongs in the repo rather than in each developer's `~/.cargo/config.toml`
/// because a fresh clone has to build without setup.
fn write_cargo_config(root: &Path, r: &RemoteArgs) -> Result<()> {
    let dir = root.join(".cargo");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("config.toml");
    let entry = format!(
        "[registries.{}]\nindex = \"sparse+{}/index/\"\n",
        r.registry_name,
        r.url.trim_end_matches('/')
    );
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if existing.contains(&format!("[registries.{}]", r.registry_name)) {
        return Ok(());
    }
    let header = concat!(
        "# Where the idealyst framework crates resolve from. Internal deps in\n",
        "# [workspace.dependencies] name this registry explicitly, because most\n",
        "# of their bare names are taken by unrelated crates on crates.io.\n",
    );
    let out = if existing.is_empty() {
        format!("{header}{entry}")
    } else {
        format!("{}\n{header}{entry}", existing.trim_end())
    };
    std::fs::write(&path, out)?;
    println!("  wrote .cargo/config.toml");
    Ok(())
}

/// Write the registry's `config.json`.
///
/// Cargo fetches `<index-url>/config.json`, so this belongs at the root of the
/// INDEX, not of the bucket. Putting it at the bucket root yields a 404 that
/// cargo reports as "no matching package named X found" for whichever crate it
/// happened to be resolving — an error message that points nowhere near the
/// actual fault, which is why this has its own regression test.
/// Run a crate's `[package.metadata.registry] prepackage` command.
///
/// It exists for a crate that embeds files from OUTSIDE its own directory —
/// the CLI embeds `examples/welcome` and the Inspector's web build. `cargo
/// package` carries only the crate's directory, so the command stages those
/// files into a gitignored folder the crate lists in `include`. It runs from
/// the workspace root, and a failure stops the release: packaging anyway
/// would publish a crate that cannot build outside this workspace.
fn run_prepackage(root: &Path, name: &str, argv: &[String]) -> Result<()> {
    println!("  {name} — prepackage: {}", argv.join(" "));
    let (program, args) = argv.split_first().context("empty prepackage command")?;
    let st = Command::new(program)
        .args(args)
        .current_dir(root)
        .status()
        .with_context(|| format!("running {name}'s prepackage command"))?;
    if !st.success() {
        bail!("{name}'s prepackage command failed ({st}): {}", argv.join(" "));
    }
    Ok(())
}

/// Write the planned versions into the manifests before packaging.
///
/// `cargo package` names its tarball after the version in the manifest, so the
/// bump has to land on disk first — computing a version and not writing it
/// produces a `.crate` under the OLD name and a confusing "expected …" error.
/// CI commits the result back, which is what makes the next release's
/// "changed since" comparison meaningful.
fn apply_plan(ws: &Workspace, plan: &BTreeMap<String, Release>, ahead: &Ahead, r: &RemoteArgs) -> Result<()> {
    // A dep resolves to the version it will HAVE after this release, not the
    // one on disk: `gesture` packaged in the same run as runtime-shared 1.7.1
    // has to record 1.7.1, not the 1.7.0 its manifest still said when the run
    // started.
    let after = |n: &str| -> Option<semver::Version> {
        let p = ws.packages.get(n)?;
        if !p.publish {
            // Not publishable, so it has no registry version to name. Leaving
            // it path-only is what makes `cargo package` drop it.
            return None;
        }
        Some(
            plan.get(n)
                .map(|r| r.to.clone())
                .or_else(|| ahead.get(n).cloned())
                .unwrap_or_else(|| p.version.clone()),
        )
    };
    // Each crate's version once this release is cut: the planned one, or,
    // for a crate whose manifest is behind the registry ([`Ahead`]), the
    // version the registry already serves. Without the second, a dependent
    // packaged here requires the older version (`server 4.0.1` shipped
    // requiring `server-macros ^1.5.2` beside the adopted 2.0.0 this way).
    let mut moving: BTreeMap<&String, &semver::Version> = ahead.iter().collect();
    moving.extend(plan.iter().map(|(n, r)| (n, &r.to)));

    for (name, to) in &moving {
        let Some(p) = ws.packages.get(*name) else { continue };
        let mut doc = manifest::read(&p.manifest_path)?;
        manifest::set_package_version(&mut doc, to)?;
        // Most internal deps go through `[workspace.dependencies]`, but a
        // handful — `crates/idealyst` among them — spell the path directly,
        // and those requirements need the same floor.
        let touched = manifest::version_literal_path_deps(&mut doc, &after, &r.registry_name)?;
        if !touched.is_empty() {
            println!("  {name} — literal path deps re-floored: {}", touched.join(", "));
        }
        manifest::write(&p.manifest_path, &doc)?;
    }

    // Dependents resolve through `[workspace.dependencies]`, so the floor
    // there follows EVERY bump, patch included.
    //
    // Rewriting the floor does not republish anyone — the root manifest is
    // not published, and `plan` forces dependents only on a *major* — it
    // decides what a dependent records the next time it is published. And
    // the floor is what a dependent packaged in THIS release carries: skip
    // it on a minor/patch and the dependent ships a requirement below the
    // sibling it was compiled against (`gesture 1.5.3` recorded
    // `runtime-shared = "1.5"` while calling an API from 1.7.1). See the
    // regression test on `manifest::set_workspace_dep_version`.
    //
    // Order matters: this runs before `cargo package`, so a dependent in the
    // same plan picks up the sibling's new floor.
    let root_manifest = ws.root.join("Cargo.toml");
    let mut root = manifest::read(&root_manifest)?;
    let mut moved = 0;
    for (name, to) in &moving {
        let Some(p) = ws.packages.get(*name) else { continue };
        let dir = pathdiff(&p.manifest_path, &ws.root);
        if manifest::set_workspace_dep_version(&mut root, name, to, &dir, &r.registry_name)? {
            moved += 1;
        }
    }
    if moved > 0 {
        manifest::write(&root_manifest, &root)?;
        println!("  updated {moved} requirement(s) in [workspace.dependencies]");
    }
    Ok(())
}

fn write_config(out: &Path, url: &str) -> Result<()> {
    let dir = out.join("index");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("config.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "dl": format!("{}/crates/{{crate}}/{{version}}/download", url.trim_end_matches('/')),
        }))? + "\n",
    )?;
    Ok(())
}

fn relpath(from: &Path, to: &Path) -> String {
    let common = from
        .components()
        .zip(to.components())
        .take_while(|(a, b)| a == b)
        .count();
    let ups = from.components().count() - common;
    let rest: Vec<String> = to
        .components()
        .skip(common)
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let mut parts = vec![".."; ups]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    parts.extend(rest);
    if parts.is_empty() { ".".into() } else { parts.join("/") }
}

fn pathdiff(manifest_path: &Path, root: &Path) -> String {
    manifest_path
        .parent()
        .and_then(|d| d.strip_prefix(root).ok())
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// plan

struct Release {
    bump: Bump,
    /// First time this crate has ever been published.
    initial: bool,
    from: Option<semver::Version>,
    to: semver::Version,
    /// Why this crate is in the plan despite nothing in it having changed.
    forced: Forced,
    /// `--bump` raised this level past what the commit subjects earned.
    overridden: bool,
    /// Breaking public-API changes the API check found since the last
    /// release (see `api.rs`). Non-empty raises the crate to a major unless
    /// `api_waived`.
    api: Vec<String>,
    /// `--skip-api-check <CRATE>`: the findings are reported, not acted on.
    api_waived: bool,
}

/// A crate can be dragged into a release without a diff of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Forced {
    /// It earned its own release — something in its directory changed.
    No,
    /// A dependency took a major bump, so the requirement this crate carries
    /// had to be rewritten.
    DependencyMajor,
    /// `--force <CRATE>`: the published manifest is wrong even though the
    /// source is not. The case this exists for is an under-declared internal
    /// requirement — `gesture 1.5.3` shipped `runtime-shared = "1.5"` while
    /// calling an API added in 1.7.1, and no amount of waiting for the source
    /// to change would ever correct the published bytes.
    Requested,
}

/// `only` (from `--only`), when non-empty, skips change detection for every
/// other crate. The result is narrowed to `only` by [`scoped`] anyway, and
/// reading a skipped crate's history can fail outright: its recorded release
/// commit may be one this checkout never received (published from another
/// clone, never pushed).
/// Also returns the crates whose manifest names a version below the one the
/// registry records ([`Ahead`]), for [`apply_plan`] to bring up.
fn plan(ws: &Workspace, r: &RemoteArgs, only: &[String]) -> Result<(BTreeMap<String, Release>, Ahead)> {
    let state = if r.from_scratch {
        ReleaseState::default()
    } else {
        let target = target(r)?;
        match deploy::fetch_release_state(&target)? {
            Some(s) => {
                let mut state = serde_json::from_str(&s).context("parsing releases.json")?;
                for o in adopt_orphans(ws, r, &mut state, &|_| true)? {
                    println!(
                        "adopted {} {} @ {} — published but never recorded (was {})",
                        o.name,
                        o.version,
                        &o.commit[..o.commit.len().min(9)],
                        o.recorded.as_deref().unwrap_or("unpublished"),
                    );
                }
                state
            }
            None => {
                println!("registry has no releases.json yet — treating this as a first publish");
                ReleaseState::default()
            }
        }
    };
    let ahead = registry_ahead(ws, &state)?;
    Ok((plan_from_state(ws, r, &state, only)?, ahead))
}

/// Crates whose manifest `version` is below the version the registry
/// records, with that version. A publish that stopped partway never
/// committed its manifests, so once its uploads are adopted (see
/// [`orphans`]) the manifests still name the versions before them, and a
/// dependent packaged against them would require the older version.
type Ahead = BTreeMap<String, semver::Version>;

fn registry_ahead(ws: &Workspace, state: &ReleaseState) -> Result<Ahead> {
    let mut ahead = Ahead::new();
    for p in ws.publishable() {
        let Some(rec) = state.crates.get(&p.name) else { continue };
        let rec = semver::Version::parse(&rec.version).context("parsing a recorded version")?;
        if rec > p.version {
            ahead.insert(p.name.clone(), rec);
        }
    }
    Ok(ahead)
}

/// Adopt into `state` each version the index serves that `releases.json`
/// never recorded (see [`orphans`]), for every publishable crate that is not
/// `--skip`ped and passes `consider`.
fn adopt_orphans(
    ws: &Workspace,
    r: &RemoteArgs,
    state: &mut ReleaseState,
    consider: &dyn Fn(&str) -> bool,
) -> Result<Vec<orphans::Orphan>> {
    let target = target(r)?;
    let scratch = std::env::temp_dir().join(format!("idealyst-registry-orphans-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let index_dir = scratch.join("index");
    deploy::fetch_index_tree(&target, &index_dir)?;
    let mut latest = BTreeMap::new();
    for p in ws.publishable() {
        if r.skip.contains(&p.name) || !consider(&p.name) {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(index_dir.join(index::index_path(&p.name))) else {
            continue;
        };
        if let Some(v) = orphans::latest_version(&body)? {
            latest.insert(p.name.clone(), v);
        }
    }
    let adopted = orphans::adopt(
        state,
        &latest,
        |name, v| {
            let file = scratch.join(format!("{name}-{v}.crate"));
            deploy::fetch_crate(&target, name, &v.to_string(), &file)?;
            orphans::vcs_commit(&file)
        },
        |c| version::has_commit(&ws.root, c),
    );
    let _ = std::fs::remove_dir_all(&scratch);
    adopted
}

/// [`plan`] against an already-fetched registry state.
fn plan_from_state(
    ws: &Workspace,
    r: &RemoteArgs,
    state: &ReleaseState,
    only: &[String],
) -> Result<BTreeMap<String, Release>> {
    for name in &r.skip {
        if !ws.packages.contains_key(name) {
            bail!("--skip {name}: no such workspace member");
        }
        if only.contains(name) {
            bail!("--skip {name} and --only {name} contradict each other");
        }
    }
    if !r.skip.is_empty() {
        println!("skipped, history not read: {}", r.skip.join(", "));
    }
    let mut plan: BTreeMap<String, Release> = BTreeMap::new();
    for p in ws.publishable() {
        if !only.is_empty() && !only.contains(&p.name) {
            continue;
        }
        if r.skip.contains(&p.name) {
            continue;
        }
        let Some(prev) = state.crates.get(&p.name) else {
            // Never published: ship the manifest version as-is.
            plan.insert(
                p.name.clone(),
                Release {
                    bump: Bump::None,
                    initial: true,
                    to: p.version.clone(),
                    from: None,
                    forced: Forced::No,
                    overridden: false,
                    api: Vec::new(),
                    api_waived: false,
                },
            );
            continue;
        };
        let bump = version::bump_for(&ws.root, &p.rel_dir, &p.also_watch, &p.nested, &prev.commit)?;
        if bump == Bump::None {
            continue;
        }
        let from = semver::Version::parse(&prev.version).context("parsing a recorded version")?;
        plan.insert(
            p.name.clone(),
            Release {
                bump,
                initial: false,
                to: bump.apply(&from),
                from: Some(from),
                forced: Forced::No,
                overridden: false,
                api: Vec::new(),
                api_waived: false,
            },
        );
    }

    // The API check runs on crates that earned a release of their own; a
    // forced or dragged-in crate has no source change to inspect. Before
    // `--bump`, so the override sees the level the API demands.
    for name in &r.skip_api_check {
        if !ws.packages.contains_key(name) {
            bail!("--skip-api-check {name}: no such workspace member");
        }
    }
    let mut breaks = BTreeMap::new();
    for (name, rel) in plan.iter() {
        if rel.initial || rel.forced != Forced::No {
            continue;
        }
        let p = &ws.packages[name];
        let Some(lib) = &p.lib_path else { continue };
        let Some(prev) = state.crates.get(name) else { continue };
        let found = api::check(&ws.root, &p.rel_dir, lib, &prev.commit)
            .with_context(|| format!("API check for {name} (waive with --skip-api-check {name})"))?;
        if !found.is_empty() {
            breaks.insert(name.clone(), found);
        }
    }
    apply_api_breaks(&mut plan, breaks, &r.skip_api_check);

    // `--force` first, so `--bump X=major` can raise a forced crate: an
    // unchanged crate whose published release was the wrong LEVEL (idea-ui-nav
    // 1.5.9 should have been 2.0.0) is re-cut with `--force X --bump X=major`.
    force_into_plan(ws, &state, &mut plan, &r.force)?;
    // Before the majors sweep below, so `--bump X=major` correctly drags X's
    // dependents in with it.
    apply_bump_overrides(&mut plan, &r.bump)?;

    drag_in_dependents_of_majors(ws, &state, &mut plan)?;
    refuse_skipped(&plan, &r.skip)?;
    Ok(plan)
}

/// Fail when the plan has to publish a crate the release was told to skip:
/// a `--force` or `--bump` naming it, or a dependency of it going major.
/// Leaving a dragged-in dependent out would keep it on the old major, and a
/// consumer would build two copies of the dependency.
fn refuse_skipped(plan: &BTreeMap<String, Release>, skip: &[String]) -> Result<()> {
    let needed: Vec<&str> = skip.iter().filter(|s| plan.contains_key(*s)).map(String::as_str).collect();
    if needed.is_empty() {
        return Ok(());
    }
    bail!(
        "the plan must republish {} (named with --force/--bump, or a dependency goes major), \
         but it is skipped. Release it from a clone that has its recorded commit, or leave \
         the crates that drag it in out of this release with --only.",
        needed.join(", "),
    )
}

/// Bring the dependents of every major-bumped crate into the plan.
///
/// A major bump changes the requirement dependents carry, so they must be
/// republished. Minor and patch bumps do not: the caret requirement already
/// admits them, which is precisely the reuse this migration buys.
///
/// **A library dependent goes major too**, and so (transitively) do the
/// libraries that depend on it. Its new release requires the new major of
/// the dependency, and the dependency's types are in its public surface
/// (idea-ui-nav renders idea-ui components and shares its theme), so a
/// consumer on the old line who lets cargo pick up the new release ends up
/// with two copies of the dependency: separate theme state, and types that
/// don't unify (a patch-bumped idea-ui-nav put idea-ui 5 next to an app's
/// idea-ui 4). A library that earned a smaller bump of its own in the same
/// release is raised to major for the same reason.
///
/// A dependent with no library target (the CLI, dev tools, doc sites built
/// as binaries) cannot leak anything into a consumer, so it is republished
/// at a patch.
fn drag_in_dependents_of_majors(
    ws: &Workspace,
    state: &ReleaseState,
    plan: &mut BTreeMap<String, Release>,
) -> Result<()> {
    let majors: Vec<String> = plan
        .iter()
        .filter(|(_, r)| r.bump == Bump::Major)
        .map(|(n, _)| n.clone())
        .collect();
    if majors.is_empty() {
        return Ok(());
    }
    // `dependents_of` is transitive. Every path from a major to a dependent
    // runs through crates that depend on it, and those are libraries (only a
    // library can be depended on) — so each library reached here goes major,
    // and the closure needs no fixpoint.
    for name in ws.dependents_of(majors.iter().map(String::as_str)) {
        let Some(p) = ws.packages.get(&name) else { continue };
        let want = if p.has_lib { Bump::Major } else { Bump::Patch };
        if let Some(rel) = plan.get_mut(&name) {
            // Never published: it ships its manifest version as-is.
            if rel.initial || rel.bump >= want {
                continue;
            }
            if let Some(from) = &rel.from {
                rel.to = want.apply(from);
            }
            rel.bump = want;
            continue;
        }
        let from = state
            .crates
            .get(&name)
            .map(|r| semver::Version::parse(&r.version))
            .transpose()?;
        let base = from.clone().unwrap_or_else(|| p.version.clone());
        plan.insert(
            name,
            Release {
                bump: want,
                initial: false,
                to: want.apply(&base),
                from,
                forced: Forced::DependencyMajor,
                overridden: false,
                api: Vec::new(),
                api_waived: false,
            },
        );
    }
    Ok(())
}

/// Raise every crate with a breaking public-API change to a major.
///
/// The commit subject is a sentence; the source is the contract. A field
/// added to an all-pub struct under `feat:` (`table` 3.1.0's
/// `TableCellProps::column`) reads as a minor, and a minor is exactly what
/// `cargo update` pulls under a dependent's caret requirement — so the
/// dependent's struct literal stops compiling in a build nobody changed.
/// `api.rs` says what counts. A crate in `waived` keeps the level its
/// subjects earned and carries the findings for the report only.
fn apply_api_breaks(
    plan: &mut BTreeMap<String, Release>,
    breaks: BTreeMap<String, Vec<String>>,
    waived: &[String],
) {
    for (name, found) in breaks {
        let Some(rel) = plan.get_mut(&name) else { continue };
        rel.api = found;
        if waived.contains(&name) {
            rel.api_waived = true;
            continue;
        }
        if rel.bump < Bump::Major {
            if let Some(from) = &rel.from {
                rel.bump = Bump::Major;
                rel.to = Bump::Major.apply(from);
            }
        }
    }
}

/// Raise the bump level of crates named by `--bump CRATE=LEVEL`.
///
/// The level a crate earns is classified from its commits' SUBJECTS, and a
/// subject is a sentence someone wrote, not a contract. Additive public API
/// lands under a free-form subject and reads as a patch — `EndReach` and the
/// `on_end_reached` setters arrived that way, under "scroll_view can say when
/// the reader has reached the end". The version is the only thing a consumer
/// sees, so it should say API arrived.
///
/// **Raise-only.** Asking for less than the crate earned is refused rather
/// than applied: a subject that says `feat:` while the override says `patch`
/// is a disagreement worth stopping on, and silently under-publishing API is
/// the failure this flag exists to prevent. A crate that is not already in the
/// plan is refused too — nothing changed in it, so there is no level to raise;
/// `--force` is the flag for republishing an unchanged crate, and it runs
/// first, so `--force X --bump X=major` re-cuts an unchanged crate as a major.
fn apply_bump_overrides(plan: &mut BTreeMap<String, Release>, specs: &[String]) -> Result<()> {
    for spec in specs {
        let (name, level) = spec
            .split_once('=')
            .with_context(|| format!("--bump {spec}: expected CRATE=LEVEL, e.g. idea-ui=minor"))?;
        let want = match level.trim().to_ascii_lowercase().as_str() {
            "patch" => Bump::Patch,
            "minor" => Bump::Minor,
            "major" => Bump::Major,
            other => bail!("--bump {spec}: unknown level {other:?} (patch, minor or major)"),
        };
        let rel = plan.get_mut(name).with_context(|| {
            format!(
                "--bump {spec}: {name} is not in the plan — nothing in it changed. \
                 Use --force to republish an unchanged crate."
            )
        })?;
        if want < rel.bump {
            bail!(
                "--bump {spec}: {name} earned a {} from its commit subjects or the API \
                 check; this flag raises a level, it does not lower one",
                rel.bump.label()
            );
        }
        if want == rel.bump {
            continue;
        }
        let Some(from) = rel.from.clone() else {
            bail!("--bump {spec}: {name} is unpublished, so its manifest version ships as-is");
        };
        rel.bump = want;
        rel.to = want.apply(&from);
        rel.overridden = true;
    }
    Ok(())
}

/// Add the `--force` crates to a plan that did not earn them.
///
/// The diff-driven path answers "did this crate's source change?", and for a
/// wrong *published manifest* the answer is no and always will be — the source
/// is correct, the recorded metadata is not. `gesture 1.5.3` recorded
/// `runtime-shared = "^1.5"` while calling `Recognizer::drive`, which landed
/// in 1.7.1; no future edit to `gesture` corrects the bytes already published
/// under 1.5.3. So the crate has to be named.
///
/// A crate already in the plan keeps the bump it earned — forcing never
/// downgrades a minor to a patch.
fn force_into_plan(
    ws: &Workspace,
    state: &ReleaseState,
    plan: &mut BTreeMap<String, Release>,
    force: &[String],
) -> Result<()> {
    for name in force {
        if plan.contains_key(name) {
            continue;
        }
        let p = ws
            .packages
            .get(name)
            .with_context(|| format!("--force {name}: no such workspace member"))?;
        if !p.publish {
            bail!("--force {name}: that crate is not publishable");
        }
        let from = state
            .crates
            .get(name)
            .map(|rel| semver::Version::parse(&rel.version))
            .transpose()?;
        let base = from.clone().unwrap_or_else(|| p.version.clone());
        plan.insert(
            name.clone(),
            Release {
                bump: Bump::Patch,
                initial: from.is_none(),
                to: Bump::Patch.apply(&base),
                from,
                forced: Forced::Requested,
                overridden: false,
                api: Vec::new(),
                api_waived: false,
            },
        );
    }
    Ok(())
}

fn report(ws: &Workspace, plan: &BTreeMap<String, Release>) {
    let total = ws.publishable().count();
    if plan.is_empty() {
        println!("nothing to release — no publishable crate changed ({total} up to date)");
        return;
    }
    println!("releasing {} of {total} publishable crates:\n", plan.len());
    for (name, r) in plan {
        let from = r.from.as_ref().map(|v| v.to_string()).unwrap_or_else(|| "new".into());
        let why = match r.forced {
            Forced::No => "",
            Forced::DependencyMajor => "  (dependency major bump)",
            Forced::Requested => "  (--force)",
        };
        let label = if r.initial { "initial" } else { r.bump.label() };
        let why = if r.overridden { "  (--bump)" } else { why };
        let why = match (r.api.is_empty(), r.api_waived) {
            (true, _) => why,
            (false, false) => "  (API break)",
            (false, true) => "  (API check waived)",
        };
        println!("  {:<28} {:>7}  {} -> {}{}", name, label, from, r.to, why);
        for finding in &r.api {
            println!("  {:<28}          - {finding}", "");
        }
    }
    println!("\n{} crates unchanged and NOT republished — consumers keep their cached builds",
             total - plan.len());
}

// ---------------------------------------------------------------------------
// build

/// Delete a `releases.json` an earlier run left in the staging dir. It
/// describes the registry as of that run; the final upload must only ever
/// send the one this run writes, which carries forward the live copy.
fn clear_stale_release_state(out: &Path) -> Result<()> {
    match std::fs::remove_file(out.join("releases.json")) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(e).context("removing a stale releases.json from the staging dir")
        }
        _ => Ok(()),
    }
}

fn build(
    ws: &Workspace,
    plan: &BTreeMap<String, Release>,
    ahead: &Ahead,
    r: &RemoteArgs,
    b: &BuildArgs,
    execute: bool,
) -> Result<Vec<String>> {
    let (out, verify) = (b.out.as_path(), b.verify);
    clear_stale_release_state(out)?;
    if plan.is_empty() {
        // Still refresh the metadata. config.json and the landing page are
        // properties of the registry, not of any release, and a run that
        // releases nothing is the normal way to correct them.
        write_config(out, &r.url)?;
        write_landing_page(out, &r.url, &r.registry_name, published_count(r)?)?;
        if execute {
            deploy::put_metadata(out, &target(r)?, deploy::Metadata::Config)?;
        }
        return Ok(vec![]);
    }
    if !version::is_clean(&ws.root)? && !b.allow_dirty {
        bail!("working tree is dirty — a release must record the commit it was cut from (--allow-dirty to rehearse)");
    }
    let head = version::head_commit(&ws.root)?;
    apply_plan(ws, plan, ahead, r)?;

    std::fs::create_dir_all(out.join("index"))?;
    std::fs::create_dir_all(out.join("crates"))?;
    write_config(out, &r.url)?;
    // The landing page advertises the registry's total, not this run's slice.
    let total = published_count(r)?.max(plan.len());
    write_landing_page(out, &r.url, &r.registry_name, total)?;
    // config.json has to be live BEFORE the first crate is packaged, not with
    // the metadata at the end. It is the first thing cargo fetches when it
    // touches the registry, and the registry gets touched as soon as a crate
    // with internal deps is packaged — which is the third crate. Uploading it
    // last means a 403 on `<index>/config.json` mid-run.
    if execute {
        deploy::put_metadata(out, &target(r)?, deploy::Metadata::Config)?;
    }

    let mut released = Vec::new();
    let mut state = ReleaseState::default();
    for p in ws.publish_order(&|n| plan.contains_key(n))? {
        let Some(rel) = plan.get(&p.name) else { continue };

        if let Some(argv) = &p.prepackage {
            run_prepackage(&ws.root, &p.name, argv)?;
        }

        let mut cmd = Command::new("cargo");
        cmd.args(["package", "-p", &p.name, "--allow-dirty"]);
        if !verify {
            // Compiling all 165 crates to validate their own tarballs is
            // hours of work, and `cargo check --workspace` is not green in
            // this repo anyway — so verification is opt-in and belongs in a
            // scheduled job, not in every release.
            cmd.arg("--no-verify");
        }
        let st = cmd.current_dir(&ws.root).status().context("running cargo package")?;
        if !st.success() {
            bail!("`cargo package -p {}` failed", p.name);
        }

        let crate_file = ws
            .root
            .join("target/package")
            .join(format!("{}-{}.crate", p.name, rel.to));
        if !crate_file.exists() {
            bail!(
                "expected {} — did the manifest version get bumped before packaging?",
                crate_file.display()
            );
        }

        let packaged = index::packaged_manifest(&crate_file)?;
        let (features, features2) = index::split_features(&packaged);
        let entry = index::IndexEntry {
            name: p.name.clone(),
            vers: rel.to.to_string(),
            deps: index::deps_from_manifest(&packaged, &|n| ws.is_publishable(n)),
            cksum: index::checksum(&crate_file)?,
            features,
            yanked: false,
            links: None,
            v: 2,
            features2,
        };

        // Append to whatever the registry already has for this crate, so old
        // versions stay resolvable. A consumer that has not upgraded must keep
        // building.
        let ipath = index::index_path(&p.name);
        let mut lines = if r.from_scratch {
            String::new()
        } else {
            deploy::fetch_index_file(&target(r)?, &ipath)?.unwrap_or_default()
        };
        // A 133-crate publish that dies halfway must be restartable, so an
        // already-present version is a skip rather than a failure — PROVIDED
        // the bytes match. `cargo package` output is byte-reproducible (it
        // normalizes mtimes), so a checksum comparison is exact. A MISMATCH
        // is the real fault: the source moved without the version moving, and
        // silently leaving the old tarball up would ship a registry that
        // disagrees with this commit.
        if let Some(published) = published_checksum(&lines, &rel.to.to_string())? {
            if published == entry.cksum {
                println!("  skipped {} {} (already published, identical)", p.name, rel.to);
                state.crates.insert(
                    p.name.clone(),
                    Released { version: rel.to.to_string(), commit: head.clone() },
                );
                continue;
            }
            bail!(
                "{} {} is already published with different content\n                   published: {}\n  local:     {}\nA published version is immutable — bump instead.",
                p.name,
                rel.to,
                published,
                entry.cksum
            );
        }
        if !lines.is_empty() && !lines.ends_with('\n') {
            lines.push('\n');
        }
        lines.push_str(&serde_json::to_string(&entry)?);
        lines.push('\n');

        let dest = out.join("index").join(&ipath);
        std::fs::create_dir_all(dest.parent().unwrap())?;
        std::fs::write(&dest, &lines)?;

        let tarball = out
            .join("crates")
            .join(&p.name)
            .join(rel.to.to_string());
        std::fs::create_dir_all(&tarball)?;
        std::fs::copy(&crate_file, tarball.join("download"))?;

        // Upload before moving on. The next crate in topological order may
        // depend on this one, and `cargo package` resolves against the real
        // registry — so it has to be there already, not queued for a bulk
        // sync at the end.
        if execute {
            deploy::put_release(
                &target(r)?,
                &p.name,
                &rel.to.to_string(),
                &crate_file,
                &ipath,
                &lines,
            )?;
        }

        state.crates.insert(
            p.name.clone(),
            Released { version: rel.to.to_string(), commit: head.clone() },
        );
        released.push(p.name.clone());
        println!("  packaged {} {}", p.name, rel.to);
    }

    // Carry forward everything that did not move this round, so the state file
    // stays a complete picture of the registry.
    if !r.from_scratch {
        if let Some(prev) = deploy::fetch_release_state(&target(r)?)? {
            let mut prev: ReleaseState = serde_json::from_str(&prev)?;
            // The planner adopted orphans the raw state file lacks; record
            // them, or this file would name their old versions again. The
            // crates released this round are already in `state`.
            adopt_orphans(ws, r, &mut prev, &|n| !state.crates.contains_key(n))?;
            for (k, v) in prev.crates {
                state.crates.entry(k).or_insert(v);
            }
        }
    }
    std::fs::write(
        out.join("releases.json"),
        serde_json::to_string_pretty(&state)? + "\n",
    )?;
    Ok(released)
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Package;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("registry-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Regression: the upload that has to precede the first crate (for
    /// `config.json`) also sent the staging dir's `releases.json` — one an
    /// earlier run left behind. That overwrote the live state, and the end
    /// of the run carried forward from the stale copy: a 14-crate publish
    /// dropped the previous release's 26 entries. The early upload must not
    /// carry `releases.json`, and a build must start without a leftover one.
    #[test]
    fn regression_stale_release_state_is_never_uploaded() {
        let out = scratch("stale-state");
        std::fs::create_dir_all(out.join("index")).unwrap();
        std::fs::write(out.join("index/config.json"), "{}").unwrap();
        std::fs::write(out.join("releases.json"), r#"{"crates":{}}"#).unwrap();

        let early: Vec<&str> = deploy::metadata_files(&out, deploy::Metadata::Config)
            .into_iter()
            .map(|(_, remote, _)| remote)
            .collect();
        assert_eq!(early, ["index/config.json"]);

        clear_stale_release_state(&out).unwrap();
        let last: Vec<&str> = deploy::metadata_files(&out, deploy::Metadata::WithReleaseState)
            .into_iter()
            .map(|(_, remote, _)| remote)
            .collect();
        assert_eq!(last, ["index/config.json"], "the leftover state file is gone");

        // A missing file is not an error: most builds start without one.
        clear_stale_release_state(&out).unwrap();
    }

    /// Regression: `config.json` used to be written to the staging root, so
    /// cargo's `GET <index>/config.json` 404'd and every resolve failed with a
    /// misleading "no matching package" error.
    #[test]
    fn regression_config_json_lands_at_the_index_root() {
        let d = scratch("config");
        write_config(&d, "https://crates.idealyst.io").unwrap();
        assert!(
            d.join("index/config.json").exists(),
            "config.json must sit inside index/, where cargo asks for it"
        );
        assert!(!d.join("config.json").exists());
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(d.join("index/config.json")).unwrap())
                .unwrap();
        assert_eq!(
            v["dl"],
            "https://crates.idealyst.io/crates/{crate}/{version}/download"
        );
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn config_url_tolerates_a_trailing_slash() {
        let d = scratch("slash");
        write_config(&d, "https://crates.idealyst.io/").unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(d.join("index/config.json")).unwrap())
                .unwrap();
        assert_eq!(
            v["dl"],
            "https://crates.idealyst.io/crates/{crate}/{version}/download"
        );
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// Regression: a 133-crate publish that dies partway must be restartable.
    /// Treating an already-present version as a hard error stranded the run —
    /// but only a matching checksum makes the skip safe, since a differing one
    /// means the source moved without the version moving.
    #[test]
    fn regression_resume_skips_identical_but_catches_changed_content() {
        let lines = concat!(
            r#"{"name":"wire","vers":"1.5.1","cksum":"aaa","deps":[]}"#,
            "\n",
            r#"{"name":"wire","vers":"1.5.2","cksum":"bbb","deps":[]}"#,
            "\n",
        );
        assert_eq!(published_checksum(lines, "1.5.2").unwrap().as_deref(), Some("bbb"));
        assert_eq!(published_checksum(lines, "1.5.1").unwrap().as_deref(), Some("aaa"));
        // Not published yet -> nothing to compare against, so it publishes.
        assert_eq!(published_checksum(lines, "1.5.3").unwrap(), None);
        // Blank lines and a trailing newline must not derail the scan.
        assert_eq!(published_checksum("", "1.5.2").unwrap(), None);
    }

    /// Regression, `gesture 1.5.3`: `apply_plan` rewrote the
    /// `[workspace.dependencies]` floor only for a MAJOR bump, so
    /// runtime-shared's 1.7.0 -> 1.7.1 patch left the floor at `1.5`. Every
    /// crate packaged in that same release — `gesture` among them, freshly
    /// calling `Recognizer::drive`, which 1.7.1 had just added — shipped a
    /// requirement below the sibling it was compiled against. A consumer
    /// holding runtime-shared 1.7.0 kept it and the build broke inside
    /// `gesture`. The floor must follow a patch bump too, and it must land
    /// before the dependent is packaged.
    /// REGRESSION: a publish that stopped partway left `server-macros
    /// 2.0.0` in the index while its manifest (never committed) said 1.5.2.
    /// The planner adopted 2.0.0, but `apply_plan` left the floor alone, so
    /// `server 4.0.1` shipped requiring `server-macros ^1.5.2`. A manifest
    /// behind the registry moves, with its floor, like a release does.
    #[test]
    fn regression_an_adopted_orphan_lifts_its_floor_and_manifest() {
        let d = scratch("adopted-floor");
        let macros = d.join("crates/api/server-macros");
        let server = d.join("crates/api/server");
        std::fs::create_dir_all(&macros).unwrap();
        std::fs::create_dir_all(&server).unwrap();
        std::fs::write(
            d.join("Cargo.toml"),
            "[workspace.dependencies]\nserver-macros = { path = \"crates/api/server-macros\", version = \"1.5.2\", registry = \"idealyst\" }\nserver = { path = \"crates/api/server\", version = \"4.0.0\", registry = \"idealyst\" }\n",
        )
        .unwrap();
        std::fs::write(macros.join("Cargo.toml"), "[package]\nname = \"server-macros\"\nversion = \"1.5.2\"\n").unwrap();
        std::fs::write(
            server.join("Cargo.toml"),
            "[package]\nname = \"server\"\nversion = \"4.0.0\"\n\n[dependencies]\nserver-macros = { workspace = true }\n",
        )
        .unwrap();
        let mut ws = bare_ws(&[("server-macros", "1.5.2"), ("server", "4.0.0")]);
        ws.root = d.clone();
        for (n, dir) in [("server-macros", "crates/api/server-macros"), ("server", "crates/api/server")] {
            let p = ws.packages.get_mut(n).unwrap();
            p.manifest_path = d.join(dir).join("Cargo.toml");
            p.rel_dir = dir.into();
        }
        let plan = BTreeMap::from([("server".to_string(), planned(Bump::Patch, (4, 0, 0)))]);
        let state = released(&[("server-macros", "2.0.0"), ("server", "4.0.0")]);
        let ahead = registry_ahead(&ws, &state).unwrap();
        assert_eq!(ahead.len(), 1, "only the crate behind the registry: {ahead:?}");
        let r = RemoteArgs {
            bucket: None,
            distribution_id: None,
            url: "https://crates.idealyst.io".into(),
            from_scratch: false,
            force: vec![],
            bump: vec![],
            skip_api_check: vec![],
            skip: vec![],
            registry_name: "idealyst".into(),
        };

        apply_plan(&ws, &plan, &ahead, &r).unwrap();

        let root = std::fs::read_to_string(d.join("Cargo.toml")).unwrap();
        let floor = root.lines().find(|l| l.starts_with("server-macros")).unwrap();
        assert!(floor.contains(r#"version = "2.0.0""#), "floor still below the adopted version: {floor}");
        let m = std::fs::read_to_string(macros.join("Cargo.toml")).unwrap();
        assert!(m.contains(r#"version = "2.0.0""#), "manifest still names the old version:\n{m}");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn regression_patch_bump_lifts_the_floor_a_dependent_packages_with() {
        let d = scratch("floor");
        let shared = d.join("crates/runtime/shared");
        let gesture = d.join("crates/sdk/client/gesture");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::create_dir_all(&gesture).unwrap();
        std::fs::write(
            d.join("Cargo.toml"),
            "[workspace.dependencies]\n             runtime-shared = { path = \"crates/runtime/shared\", version = \"1.5\", registry = \"idealyst\" }\n",
        )
        .unwrap();
        std::fs::write(
            shared.join("Cargo.toml"),
            "[package]\nname = \"runtime-shared\"\nversion = \"1.7.0\"\n",
        )
        .unwrap();
        std::fs::write(
            gesture.join("Cargo.toml"),
            "[package]\nname = \"gesture\"\nversion = \"1.5.2\"\n\n             [dependencies]\nruntime-shared = { workspace = true }\n",
        )
        .unwrap();

        let pkg = |name: &str, dir: &str, v: (u64, u64, u64)| Package {
            name: name.into(),
            version: semver::Version::new(v.0, v.1, v.2),
            manifest_path: d.join(dir).join("Cargo.toml"),
            rel_dir: dir.into(),
            publish: true,
            nested: vec![],
            deps: Default::default(),
            dev_deps: Default::default(),
            has_lib: true,
            lib_path: None,
            also_watch: Vec::new(),
            prepackage: None,
        };
        let ws = Workspace {
            root: d.clone(),
            packages: BTreeMap::from([
                ("runtime-shared".to_string(), pkg("runtime-shared", "crates/runtime/shared", (1, 7, 0))),
                ("gesture".to_string(), pkg("gesture", "crates/sdk/client/gesture", (1, 5, 2))),
            ]),
        };
        let patch = |from: (u64, u64, u64), to: (u64, u64, u64)| Release {
            bump: Bump::Patch,
            initial: false,
            from: Some(semver::Version::new(from.0, from.1, from.2)),
            to: semver::Version::new(to.0, to.1, to.2),
            forced: Forced::No,
            overridden: false,
            api: Vec::new(),
            api_waived: false,
        };
        let plan = BTreeMap::from([
            ("runtime-shared".to_string(), patch((1, 7, 0), (1, 7, 1))),
            ("gesture".to_string(), patch((1, 5, 2), (1, 5, 3))),
        ]);
        let r = RemoteArgs {
            bucket: None,
            distribution_id: None,
            url: "https://crates.idealyst.io".into(),
            from_scratch: false,
            force: vec![],
            bump: vec![],
            skip_api_check: vec![],
            skip: vec![],
            registry_name: "idealyst".into(),
        };

        apply_plan(&ws, &plan, &Ahead::new(), &r).unwrap();

        let root = std::fs::read_to_string(d.join("Cargo.toml")).unwrap();
        assert!(
            root.contains(r#"version = "1.7.1""#),
            "a patch bump must lift the floor, got:\n{root}"
        );
        assert!(!root.contains(r#"version = "1.5""#), "stale floor survived:\n{root}");
        std::fs::remove_dir_all(&d).unwrap();
    }

    fn bare_ws(names: &[(&str, &str)]) -> Workspace {
        Workspace {
            root: PathBuf::from("/w"),
            packages: names
                .iter()
                .map(|(n, v)| {
                    (
                        n.to_string(),
                        Package {
                            name: n.to_string(),
                            version: semver::Version::parse(v).unwrap(),
                            manifest_path: PathBuf::from(format!("/w/crates/{n}/Cargo.toml")),
                            rel_dir: format!("crates/{n}"),
                            publish: true,
                            nested: vec![],
                            deps: Default::default(),
                            dev_deps: Default::default(),
                            has_lib: true,
                            lib_path: None,
                            also_watch: Vec::new(),
                            prepackage: None,
                        },
                    )
                })
                .collect(),
        }
    }

    fn released(pairs: &[(&str, &str)]) -> ReleaseState {
        ReleaseState {
            crates: pairs
                .iter()
                .map(|(n, v)| {
                    (n.to_string(), Released { version: v.to_string(), commit: "deadbeef".into() })
                })
                .collect(),
        }
    }

    fn planned(bump: Bump, from: (u64, u64, u64)) -> Release {
        let from = semver::Version::new(from.0, from.1, from.2);
        Release {
            bump,
            initial: false,
            to: bump.apply(&from),
            from: Some(from),
            forced: Forced::No,
            overridden: false,
            api: Vec::new(),
            api_waived: false,
        }
    }

    /// `idea-ui-nav` and `idea-theme-editor` depend on `idea-ui`; `dev-tui`
    /// depends on `idea-theme-editor` and has no library target;
    /// `idea-ui-mail` depends on `idea-ui` and earned a minor of its own.
    fn idea_ui_family() -> (Workspace, ReleaseState) {
        let mut ws = bare_ws(&[
            ("idea-ui", "4.1.0"),
            ("idea-ui-nav", "1.5.8"),
            ("idea-theme-editor", "1.5.7"),
            ("idea-ui-mail", "1.5.3"),
            ("dev-tui", "1.5.5"),
        ]);
        let deps = |names: &[&str]| names.iter().map(|n| n.to_string()).collect();
        ws.packages.get_mut("idea-ui-nav").unwrap().deps = deps(&["idea-ui"]);
        ws.packages.get_mut("idea-theme-editor").unwrap().deps = deps(&["idea-ui"]);
        ws.packages.get_mut("idea-ui-mail").unwrap().deps = deps(&["idea-ui"]);
        let tui = ws.packages.get_mut("dev-tui").unwrap();
        tui.deps = deps(&["idea-theme-editor"]);
        tui.has_lib = false;
        let state = released(&[
            ("idea-ui", "4.1.0"),
            ("idea-ui-nav", "1.5.8"),
            ("idea-theme-editor", "1.5.7"),
            ("idea-ui-mail", "1.5.3"),
            ("dev-tui", "1.5.5"),
        ]);
        (ws, state)
    }

    /// Regression: idea-ui 5.0.0 dragged idea-ui-nav in at 1.5.9, so a
    /// `cargo update` within 1.5 pulled idea-ui 5 in next to an app's idea-ui
    /// 4 — two copies, separate theme state, mismatched types (CrewForge,
    /// 2026-10-14). A library dependent of a major must go major itself.
    #[test]
    fn regression_major_dependency_bumps_library_dependents_major() {
        let (ws, state) = idea_ui_family();
        let mut plan = BTreeMap::from([
            ("idea-ui".to_string(), planned(Bump::Major, (4, 1, 0))),
            ("idea-ui-mail".to_string(), planned(Bump::Minor, (1, 5, 3))),
        ]);
        drag_in_dependents_of_majors(&ws, &state, &mut plan).unwrap();

        let nav = &plan["idea-ui-nav"];
        assert_eq!(nav.bump, Bump::Major);
        assert_eq!(nav.to, semver::Version::new(2, 0, 0), "pre-fix: 1.5.9");
        assert_eq!(nav.forced, Forced::DependencyMajor);
        assert_eq!(plan["idea-theme-editor"].to, semver::Version::new(2, 0, 0));

        let mail = &plan["idea-ui-mail"];
        assert_eq!(mail.bump, Bump::Major, "an earned minor is raised to major");
        assert_eq!(mail.to, semver::Version::new(2, 0, 0));
        assert_eq!(mail.forced, Forced::No, "it still earned its own release");

        let tui = &plan["dev-tui"];
        assert_eq!(tui.bump, Bump::Patch, "no library target: nothing leaks to a consumer");
        assert_eq!(tui.to, semver::Version::new(1, 5, 6));
    }

    /// No major in the plan: nobody is dragged in, and earned levels stay.
    /// Regression: `table` 3.1.0 added `pub column` to the all-pub
    /// `TableCellProps` under a `feat:` subject and shipped as a minor. The
    /// API finding must raise it to a major — and, being a major, drag its
    /// library dependents along.
    #[test]
    fn regression_api_break_raises_a_minor_to_a_major_and_drags_dependents() {
        let (ws, state) = idea_ui_family();
        let mut plan = BTreeMap::from([("idea-ui".to_string(), planned(Bump::Minor, (4, 1, 0)))]);
        let breaks = BTreeMap::from([(
            "idea-ui".to_string(),
            vec!["added field `column` to `TableCellProps`".to_string()],
        )]);
        apply_api_breaks(&mut plan, breaks, &[]);
        assert_eq!(plan["idea-ui"].bump, Bump::Major);
        assert_eq!(plan["idea-ui"].to, semver::Version::new(5, 0, 0));
        assert_eq!(plan["idea-ui"].api.len(), 1);
        drag_in_dependents_of_majors(&ws, &state, &mut plan).unwrap();
        assert_eq!(plan["idea-ui-nav"].bump, Bump::Major);
        // A `--bump` below the API's level is refused, not applied.
        assert!(apply_bump_overrides(&mut plan, &["idea-ui=minor".to_string()]).is_err());
    }

    #[test]
    fn a_waived_api_break_keeps_the_subject_level_but_is_reported() {
        let mut plan = BTreeMap::from([("idea-ui".to_string(), planned(Bump::Minor, (4, 1, 0)))]);
        let breaks = BTreeMap::from([("idea-ui".to_string(), vec!["removed fn `f`".to_string()])]);
        apply_api_breaks(&mut plan, breaks, &["idea-ui".to_string()]);
        assert_eq!(plan["idea-ui"].bump, Bump::Minor);
        assert_eq!(plan["idea-ui"].to, semver::Version::new(4, 2, 0));
        assert!(plan["idea-ui"].api_waived);
        assert_eq!(plan["idea-ui"].api.len(), 1);
    }

    #[test]
    fn no_api_break_leaves_the_plan_alone() {
        let mut plan = BTreeMap::from([("idea-ui".to_string(), planned(Bump::Patch, (4, 1, 0)))]);
        apply_api_breaks(&mut plan, BTreeMap::new(), &[]);
        assert_eq!(plan["idea-ui"].bump, Bump::Patch);
    }

    #[test]
    fn minor_dependency_drags_nothing_in() {
        let (ws, state) = idea_ui_family();
        let mut plan = BTreeMap::from([("idea-ui".to_string(), planned(Bump::Minor, (4, 1, 0)))]);
        drag_in_dependents_of_majors(&ws, &state, &mut plan).unwrap();
        assert_eq!(plan.len(), 1);
    }

    /// The case the flag exists for: additive public API landed under a
    /// free-form subject, so the planner read it as a patch. `EndReach` and
    /// the `on_end_reached` setters arrived exactly that way.
    #[test]
    fn bump_raises_a_patch_to_a_minor() {
        let mut plan =
            BTreeMap::from([("runtime-shared".to_string(), planned(Bump::Patch, (1, 8, 0)))]);
        apply_bump_overrides(&mut plan, &["runtime-shared=minor".to_string()]).unwrap();
        let rel = &plan["runtime-shared"];
        assert_eq!(rel.bump, Bump::Minor);
        assert_eq!(rel.to, semver::Version::new(1, 9, 0), "recomputed from `from`, not from `to`");
        assert!(rel.overridden);
    }

    /// Raising must recompute from the PUBLISHED version. Applying a minor to
    /// the already-patched `to` would ship 1.8.1 -> 1.9.0 as though the patch
    /// had happened, skipping a version and lying about the base.
    #[test]
    fn bump_recomputes_from_the_published_version() {
        let mut plan = BTreeMap::from([("css".to_string(), planned(Bump::Patch, (1, 5, 4)))]);
        assert_eq!(plan["css"].to, semver::Version::new(1, 5, 5));
        apply_bump_overrides(&mut plan, &["css=major".to_string()]).unwrap();
        assert_eq!(plan["css"].to, semver::Version::new(2, 0, 0));
    }

    /// Lowering is refused, not applied. A subject that says `feat:` against
    /// an override that says `patch` is a disagreement worth stopping on —
    /// silently under-publishing API is the failure this flag prevents.
    #[test]
    fn bump_refuses_to_lower_a_level() {
        let mut plan = BTreeMap::from([("idea-ui".to_string(), planned(Bump::Minor, (1, 8, 1)))]);
        let err = apply_bump_overrides(&mut plan, &["idea-ui=patch".to_string()]).unwrap_err();
        assert!(err.to_string().contains("raises a level"), "{err}");
        assert_eq!(plan["idea-ui"].bump, Bump::Minor, "left untouched");
    }

    /// A crate with no changes has no level to raise. `--force` is the flag
    /// for republishing one, and the error should say so rather than leaving
    /// the user to guess why nothing happened.
    #[test]
    fn bump_refuses_a_crate_that_is_not_in_the_plan() {
        let mut plan = BTreeMap::new();
        let err = apply_bump_overrides(&mut plan, &["table=minor".to_string()]).unwrap_err();
        assert!(err.to_string().contains("not in the plan"), "{err}");
        assert!(err.to_string().contains("--force"), "points at the right flag: {err}");
    }

    #[test]
    fn bump_rejects_a_malformed_spec() {
        let mut plan = BTreeMap::from([("css".to_string(), planned(Bump::Patch, (1, 5, 4)))]);
        assert!(apply_bump_overrides(&mut plan, &["css".to_string()]).is_err(), "no level");
        assert!(apply_bump_overrides(&mut plan, &["css=huge".to_string()]).is_err(), "bad level");
        assert_eq!(plan["css"].bump, Bump::Patch, "a rejected spec changes nothing");
    }

    /// `backend-linux 3.0.2` was published from a clone that never pushed, so
    /// its recorded commit is missing here and reading its history fails. A
    /// `--only` release of an unrelated crate must not read it at all.
    #[test]
    fn regression_only_skips_a_crate_released_from_an_unpushed_commit() {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let head = String::from_utf8(
            std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&repo)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        let mut ws = bare_ws(&[("unpushed", "3.0.2"), ("fixed", "4.0.0")]);
        ws.root = repo;
        let state = ReleaseState {
            crates: BTreeMap::from([
                (
                    "unpushed".to_string(),
                    Released { version: "3.0.2".into(), commit: "0".repeat(40) },
                ),
                (
                    "fixed".to_string(),
                    Released { version: "4.0.0".into(), commit: head.trim().into() },
                ),
            ]),
        };
        let r = RemoteArgs {
            bucket: None,
            distribution_id: None,
            url: "https://crates.idealyst.io".into(),
            from_scratch: false,
            force: vec![],
            bump: vec![],
            skip_api_check: vec![],
            skip: vec![],
            registry_name: "idealyst".into(),
        };

        let Err(err) = plan_from_state(&ws, &r, &state, &[]) else {
            panic!("a full plan reads the unpushed crate's history and must fail");
        };
        assert!(err.to_string().contains("still in this history"), "{err}");
        if let Err(e) = plan_from_state(&ws, &r, &state, &["fixed".to_string()]) {
            panic!("--only must not read the unpushed crate's history: {e}");
        }
        // A full plan with the crate skipped reads every other crate's history
        // and leaves it out.
        let skipping = RemoteArgs { skip: vec!["unpushed".into()], ..r };
        let plan = plan_from_state(&ws, &skipping, &state, &[])
            .unwrap_or_else(|e| panic!("--skip must not read the skipped crate's history: {e}"));
        assert!(!plan.contains_key("unpushed"), "{:?}", plan.keys().collect::<Vec<_>>());
        let Err(err) = plan_from_state(&ws, &skipping, &state, &["unpushed".to_string()]) else {
            panic!("--skip and --only on one crate must be refused");
        };
        assert!(err.to_string().contains("contradict"), "{err}");
    }

    /// A skipped crate that a major would drag in cannot be left out
    /// quietly: it would stay on the old major beside the new one.
    #[test]
    fn a_skipped_crate_the_plan_must_republish_fails_the_plan() {
        let (ws, state) = idea_ui_family();
        let mut plan = BTreeMap::from([("idea-ui".to_string(), planned(Bump::Major, (4, 1, 0)))]);
        drag_in_dependents_of_majors(&ws, &state, &mut plan).unwrap();
        let err = refuse_skipped(&plan, &["idea-ui-nav".to_string()]).unwrap_err();
        assert!(err.to_string().contains("idea-ui-nav"), "{err}");
        assert!(refuse_skipped(&plan, &["css".to_string()]).is_ok(), "not in the plan: fine");
    }

    /// `--force` exists for a published manifest that is wrong while the
    /// source is right — `gesture 1.5.3` recording `runtime-shared = "^1.5"`
    /// against an API added in 1.7.1. No source diff will ever put that crate
    /// in the plan, so naming it must, and from the version the registry last
    /// recorded rather than whatever the manifest happens to say.
    #[test]
    fn force_adds_an_unchanged_crate_at_a_patch_bump() {
        let ws = bare_ws(&[("gesture", "1.5.3")]);
        let state = released(&[("gesture", "1.5.3")]);
        let mut plan = BTreeMap::new();
        force_into_plan(&ws, &state, &mut plan, &["gesture".to_string()]).unwrap();
        let rel = &plan["gesture"];
        assert_eq!(rel.to, semver::Version::new(1, 5, 4));
        assert_eq!(rel.forced, Forced::Requested);
        assert!(!rel.initial);
    }

    /// Forcing must never downgrade a crate that earned a bigger bump on its
    /// own — the plan entry it already has wins.
    #[test]
    fn force_does_not_override_an_earned_bump() {
        let ws = bare_ws(&[("idea-ui", "1.8.0")]);
        let state = released(&[("idea-ui", "1.8.0")]);
        let mut plan = BTreeMap::from([(
            "idea-ui".to_string(),
            Release {
                bump: Bump::Minor,
                initial: false,
                from: Some(semver::Version::new(1, 8, 0)),
                to: semver::Version::new(1, 9, 0),
                forced: Forced::No,
                overridden: false,
                api: Vec::new(),
                api_waived: false,
            },
        )]);
        force_into_plan(&ws, &state, &mut plan, &["idea-ui".to_string()]).unwrap();
        assert_eq!(plan["idea-ui"].to, semver::Version::new(1, 9, 0));
        assert_eq!(plan["idea-ui"].forced, Forced::No);
    }

    /// The idea-ui-nav repair: 1.5.9 shipped at the wrong LEVEL with nothing
    /// changed since, so it is re-cut with `--force idea-ui-nav --bump
    /// idea-ui-nav=major`. `plan` forces before applying overrides, so the
    /// override finds the crate in the plan and raises it to 2.0.0.
    #[test]
    fn bump_raises_a_forced_crate_to_a_major() {
        let ws = bare_ws(&[("idea-ui-nav", "1.5.9")]);
        let state = released(&[("idea-ui-nav", "1.5.9")]);
        let mut plan = BTreeMap::new();
        force_into_plan(&ws, &state, &mut plan, &["idea-ui-nav".to_string()]).unwrap();
        apply_bump_overrides(&mut plan, &["idea-ui-nav=major".to_string()]).unwrap();
        let rel = &plan["idea-ui-nav"];
        assert_eq!(rel.bump, Bump::Major);
        assert_eq!(rel.to, semver::Version::new(2, 0, 0));
        assert_eq!(rel.forced, Forced::Requested);
    }

    #[test]
    fn force_rejects_a_name_that_is_not_a_member() {
        let ws = bare_ws(&[("gesture", "1.5.3")]);
        let err = force_into_plan(
            &ws,
            &ReleaseState::default(),
            &mut BTreeMap::new(),
            &["gestrue".to_string()],
        )
        .unwrap_err();
        assert!(err.to_string().contains("no such workspace member"), "{err}");
    }

    #[test]
    fn relpath_walks_up_to_a_sibling() {
        assert_eq!(
            relpath(Path::new("/w/crates/dev/wire"), Path::new("/w/crates/dev/client")),
            "../client"
        );
        assert_eq!(
            relpath(
                Path::new("/w/crates/dev/wire"),
                Path::new("/w/crates/runtime/shared")
            ),
            "../../runtime/shared"
        );
    }
}
