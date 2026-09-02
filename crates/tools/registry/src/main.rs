//! `registry` — release tooling for the idealyst cargo registry.
//!
//!   registry migrate            one-time: give every publishable crate its own version
//!   registry plan               what would be released, and at what version
//!   registry build --out DIR    package + lay out a complete registry locally
//!   registry publish            build, upload to S3, invalidate CloudFront
//!
//! The default host is set by `--bucket` / `IDEALYST_REGISTRY_BUCKET` and
//! `--distribution-id` / `IDEALYST_REGISTRY_DISTRIBUTION`.

mod deploy;
mod index;
mod manifest;
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
    /// pipeline on one leaf.
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
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let ws = Workspace::load(&cli.dir)?;
    match cli.cmd {
        Cmd::Migrate { dry_run } => migrate(&ws, dry_run),
        Cmd::Plan(r) => {
            let plan = plan(&ws, &r)?;
            report(&ws, &plan);
            Ok(())
        }
        Cmd::Build { remote, build: b } => {
            let plan = scoped(plan(&ws, &remote)?, &b.only);
            report(&ws, &plan);
            build(&ws, &plan, &remote, &b)?;
            println!("\nstaged in {}", b.out.display());
            Ok(())
        }
        Cmd::Publish { remote, build: b, execute } => {
            let plan = scoped(plan(&ws, &remote)?, &b.only);
            report(&ws, &plan);
            let state = build(&ws, &plan, &remote, &b)?;
            if !execute {
                println!("\nstaged in {} — re-run with --execute to upload", b.out.display());
                return Ok(());
            }
            let target = target(&remote)?;
            deploy::sync(&b.out, &target)?;
            deploy::invalidate(&target)?;
            println!("\npublished {} crates to {}", state.len(), remote.url);
            Ok(())
        }
    }
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
fn migrate(ws: &Workspace, dry_run: bool) -> Result<()> {
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
        let touched = manifest::version_literal_path_deps(&mut doc, &lookup)?;
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
        if manifest::set_workspace_dep_version(&mut root, &p.name, &p.version, &rel)? {
            added += 1;
        }
    }
    if !dry_run {
        manifest::write(&root_manifest, &root)?;
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
/// Write the registry's `config.json`.
///
/// Cargo fetches `<index-url>/config.json`, so this belongs at the root of the
/// INDEX, not of the bucket. Putting it at the bucket root yields a 404 that
/// cargo reports as "no matching package named X found" for whichever crate it
/// happened to be resolving — an error message that points nowhere near the
/// actual fault, which is why this has its own regression test.
/// Write the planned versions into the manifests before packaging.
///
/// `cargo package` names its tarball after the version in the manifest, so the
/// bump has to land on disk first — computing a version and not writing it
/// produces a `.crate` under the OLD name and a confusing "expected …" error.
/// CI commits the result back, which is what makes the next release's
/// "changed since" comparison meaningful.
fn apply_plan(ws: &Workspace, plan: &BTreeMap<String, Release>) -> Result<()> {
    for (name, rel) in plan {
        let Some(p) = ws.packages.get(name) else { continue };
        let mut doc = manifest::read(&p.manifest_path)?;
        manifest::set_package_version(&mut doc, &rel.to)?;
        manifest::write(&p.manifest_path, &doc)?;
    }

    // Dependents resolve through `[workspace.dependencies]`, so the caret
    // requirement there has to follow a major bump. Minor and patch bumps
    // leave it alone on purpose: `1.5` already admits 1.5.3, and rewriting it
    // would republish dependents that have no reason to change.
    let root_manifest = ws.root.join("Cargo.toml");
    let mut root = manifest::read(&root_manifest)?;
    let mut moved = 0;
    for (name, rel) in plan {
        let bumped_major = rel.from.as_ref().is_some_and(|f| f.major != rel.to.major);
        if !bumped_major && !rel.initial {
            continue;
        }
        let Some(p) = ws.packages.get(name) else { continue };
        let dir = pathdiff(&p.manifest_path, &ws.root);
        if manifest::set_workspace_dep_version(&mut root, name, &rel.to, &dir)? {
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
    /// True when nothing in this crate changed, but a dependency took a major
    /// bump so its requirement had to be rewritten.
    forced: bool,
}

fn plan(ws: &Workspace, r: &RemoteArgs) -> Result<BTreeMap<String, Release>> {
    let state = if r.from_scratch {
        ReleaseState::default()
    } else {
        let target = target(r)?;
        match deploy::fetch_release_state(&target)? {
            Some(s) => serde_json::from_str(&s).context("parsing releases.json")?,
            None => {
                println!("registry has no releases.json yet — treating this as a first publish");
                ReleaseState::default()
            }
        }
    };

    let mut plan: BTreeMap<String, Release> = BTreeMap::new();
    for p in ws.publishable() {
        let Some(prev) = state.crates.get(&p.name) else {
            // Never published: ship the manifest version as-is.
            plan.insert(
                p.name.clone(),
                Release {
                    bump: Bump::None,
                    initial: true,
                    to: p.version.clone(),
                    from: None,
                    forced: false,
                },
            );
            continue;
        };
        let bump = version::bump_for(&ws.root, &p.rel_dir, &prev.commit)?;
        if bump == Bump::None {
            continue;
        }
        let from = semver::Version::parse(&prev.version).context("parsing a recorded version")?;
        plan.insert(
            p.name.clone(),
            Release { bump, initial: false, to: bump.apply(&from), from: Some(from), forced: false },
        );
    }

    // A major bump changes the requirement dependents carry, so they must be
    // republished too. Minor and patch bumps do not: the caret requirement
    // already admits them, which is precisely the reuse this migration buys.
    let majors: Vec<String> = plan
        .iter()
        .filter(|(_, r)| r.bump == Bump::Major)
        .map(|(n, _)| n.clone())
        .collect();
    if !majors.is_empty() {
        for name in ws.dependents_of(majors.iter().map(String::as_str)) {
            if plan.contains_key(&name) {
                continue;
            }
            let Some(p) = ws.packages.get(&name) else { continue };
            let from = state
                .crates
                .get(&name)
                .map(|r| semver::Version::parse(&r.version))
                .transpose()?;
            let base = from.clone().unwrap_or_else(|| p.version.clone());
            plan.insert(
                name,
                Release {
                    bump: Bump::Patch,
                    initial: false,
                    to: Bump::Patch.apply(&base),
                    from,
                    forced: true,
                },
            );
        }
    }
    Ok(plan)
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
        let why = if r.forced { "  (dependency major bump)" } else { "" };
        let label = if r.initial { "initial" } else { r.bump.label() };
        println!("  {:<28} {:>7}  {} -> {}{}", name, label, from, r.to, why);
    }
    println!("\n{} crates unchanged and NOT republished — consumers keep their cached builds",
             total - plan.len());
}

// ---------------------------------------------------------------------------
// build

fn build(
    ws: &Workspace,
    plan: &BTreeMap<String, Release>,
    r: &RemoteArgs,
    b: &BuildArgs,
) -> Result<Vec<String>> {
    let (out, verify) = (b.out.as_path(), b.verify);
    if plan.is_empty() {
        return Ok(vec![]);
    }
    if !version::is_clean(&ws.root)? && !b.allow_dirty {
        bail!("working tree is dirty — a release must record the commit it was cut from (--allow-dirty to rehearse)");
    }
    let head = version::head_commit(&ws.root)?;
    apply_plan(ws, plan)?;

    std::fs::create_dir_all(out.join("index"))?;
    std::fs::create_dir_all(out.join("crates"))?;
    write_config(out, &r.url)?;

    let mut released = Vec::new();
    let mut state = ReleaseState::default();
    for p in ws.publish_order()? {
        let Some(rel) = plan.get(&p.name) else { continue };

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
        if lines.lines().any(|l| l.contains(&format!("\"vers\":\"{}\"", rel.to))) {
            bail!("{} {} is already published — versions are immutable", p.name, rel.to);
        }
        if !lines.is_empty() && !lines.ends_with('\n') {
            lines.push('\n');
        }
        lines.push_str(&serde_json::to_string(&entry)?);
        lines.push('\n');

        let dest = out.join("index").join(&ipath);
        std::fs::create_dir_all(dest.parent().unwrap())?;
        std::fs::write(&dest, lines)?;

        let tarball = out
            .join("crates")
            .join(&p.name)
            .join(rel.to.to_string());
        std::fs::create_dir_all(&tarball)?;
        std::fs::copy(&crate_file, tarball.join("download"))?;

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
            let prev: ReleaseState = serde_json::from_str(&prev)?;
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

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("registry-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
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
