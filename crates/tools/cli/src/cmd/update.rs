//! `idealyst update` — rebuild this CLI from the registry and replace
//! the running binary.
//!
//! The CLI is published to the framework's sparse registry like every
//! other framework crate, so updating is `cargo install` against that
//! index. This command adds the parts a bare `cargo install` gets wrong
//! or makes the user spell out:
//!
//! - **Is there anything to do?** One sparse-index file
//!   (`<index>/id/ea/idealyst-cli`) lists every published version; a
//!   no-op update costs one HTTP request instead of a rebuild.
//! - **Which binary?** It installs into the root of the binary that is
//!   running (`<root>/bin/idealyst`), not wherever cargo would default
//!   to — a devcontainer installs to `/idealyst/cli`, not `~/.cargo`.
//! - **No registry setup.** `--index` names the registry directly, so
//!   it works with no `.cargo/config.toml` and no
//!   `CARGO_REGISTRIES_IDEALYST_INDEX`. A published crate records its
//!   dependencies' registry by URL, so nothing else needs configuring.
//! - **The same architecture.** On an Apple Silicon Mac whose default
//!   rustup host is x86_64 (Rosetta), a bare `cargo install` silently
//!   produces an x86_64 binary. The target this binary was built for is
//!   baked in by build.rs and passed as `--target` when it differs.
//! - **A warm cache.** `cargo install` builds in a fresh temp dir by
//!   default; a persistent `--target-dir` makes the next update rebuild
//!   only what changed.
//!
//! Replacing the running binary is cargo's job and is safe on Unix:
//! cargo copies the new file into place only after the build succeeds,
//! and the running process keeps the old inode. Windows refuses to
//! overwrite a running `.exe` but does allow renaming it, so there the
//! old binary is moved aside first (and put back if the install fails).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

/// The package this binary is published as.
const PACKAGE: &str = "idealyst-cli";

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Only report whether a newer version is published; install nothing.
    #[arg(long)]
    check: bool,
    /// Install this exact version instead of the newest one (also allows
    /// going back to an older release).
    #[arg(long, value_name = "VERSION")]
    version: Option<String>,
    /// Reinstall even when the selected version is the one running.
    #[arg(long)]
    force: bool,
    /// Install root (the binary lands in `<ROOT>/bin`). Defaults to the
    /// root of the running binary.
    #[arg(long, value_name = "ROOT")]
    root: Option<PathBuf>,
}

pub fn run(args: Args) -> Result<()> {
    let index = registry_index();
    let current = semver::Version::parse(env!("CARGO_PKG_VERSION")).expect("CARGO_PKG_VERSION is semver");

    let published = fetch_index_file(&index)?;
    let target = match &args.version {
        Some(v) => {
            let v = semver::Version::parse(v.trim_start_matches('='))
                .with_context(|| format!("--version {v} is not a version"))?;
            if !published.iter().any(|p| p.vers == v && !p.yanked) {
                bail!("{PACKAGE} {v} is not published (or was yanked) at {index}");
            }
            v
        }
        None => newest(&published).with_context(|| format!("{PACKAGE} has no published release at {index}"))?,
    };

    match decide(&current, &target, args.version.is_some(), args.force) {
        Decision::UpToDate => {
            println!("idealyst {current} is the newest release.");
            return Ok(());
        }
        Decision::Install if args.check => {
            println!("idealyst {target} is available (running {current}). Run `idealyst update` to install it.");
            return Ok(());
        }
        Decision::Install => {}
    }

    let exe = std::env::current_exe().context("locating the running idealyst binary")?;
    let root = match args.root {
        Some(r) => r,
        None => install_root(&exe)?,
    };
    let rustc_host = rustc_host()?;
    let target_triple = (rustc_host != env!("IDEALYST_CLI_TARGET")).then_some(env!("IDEALYST_CLI_TARGET"));
    let argv = install_argv(&index, &target, &root, &cache_dir()?, target_triple);

    println!("Updating idealyst {current} -> {target} (building from source; this takes a few minutes)…");
    if let Some(t) = target_triple {
        println!("  rustc's default host is {rustc_host}; building for {t} to match this binary.");
    }
    let moved = MovedAside::new(&exe, &root)?;
    let status = Command::new("cargo")
        .args(&argv)
        .status()
        .context("running `cargo install` — is cargo on PATH?")?;
    if !status.success() {
        moved.restore()?;
        bail!("`cargo {}` failed ({status}); idealyst {current} is still installed", argv.join(" "));
    }
    moved.forget();
    println!("idealyst {target} installed to {}", root.join("bin").display());
    Ok(())
}

/// The sparse index to update from: `IDEALYST_REGISTRY_INDEX` at run
/// time, else the one this binary was built against.
fn registry_index() -> String {
    std::env::var("IDEALYST_REGISTRY_INDEX")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| env!("IDEALYST_REGISTRY_INDEX_DEFAULT").to_string())
}

/// One line of a sparse-index file. Cargo writes more fields; these are
/// the ones that decide what to install.
#[derive(serde::Deserialize, Debug)]
struct IndexEntry {
    vers: semver::Version,
    #[serde(default)]
    yanked: bool,
}

/// `<index>/<prefix>/<name>`, the file a sparse registry serves for one
/// crate. The prefix rules are cargo's (`1/`, `2/`, `3/<c>/`, `ab/cd/`).
fn index_file_url(index: &str, name: &str) -> String {
    let base = index.strip_prefix("sparse+").unwrap_or(index);
    let base = base.trim_end_matches('/');
    let lower = name.to_ascii_lowercase();
    let prefix = match lower.len() {
        1 => "1".to_string(),
        2 => "2".to_string(),
        3 => format!("3/{}", &lower[..1]),
        _ => format!("{}/{}", &lower[..2], &lower[2..4]),
    };
    format!("{base}/{prefix}/{lower}")
}

fn fetch_index_file(index: &str) -> Result<Vec<IndexEntry>> {
    let url = index_file_url(index, PACKAGE);
    // `curl` rather than an HTTP client: the CLI links no TLS stack, and
    // pulling one in for a single GET would cost every install a
    // noticeably longer build. curl ships with macOS, Windows 10+ and
    // every Linux the framework targets (`run-roku` already relies on it).
    let out = Command::new("curl")
        .args(["-sSL", "--retry", "2", "-w", "\n%{http_code}", &url])
        .output()
        .context("running curl to read the registry index — is curl on PATH?")?;
    if !out.status.success() {
        bail!("could not read {url}: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let (body, code) = text.rsplit_once('\n').unwrap_or(("", &text));
    match code.trim() {
        "200" => parse_index_file(body),
        // The registry is a static bucket behind a CDN, which answers 403
        // (not 404) for a key that doesn't exist.
        "403" | "404" => bail!("{PACKAGE} is not published at {index} (no {url})"),
        code => bail!("could not read {url}: HTTP {code}"),
    }
}

fn parse_index_file(body: &str) -> Result<Vec<IndexEntry>> {
    body.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).with_context(|| format!("unreadable index line: {l}")))
        .collect()
}

/// The newest non-yanked, non-prerelease version.
fn newest(entries: &[IndexEntry]) -> Option<semver::Version> {
    entries
        .iter()
        .filter(|e| !e.yanked && e.vers.pre.is_empty())
        .map(|e| e.vers.clone())
        .max()
}

#[derive(Debug, PartialEq)]
enum Decision {
    UpToDate,
    Install,
}

/// Install when the target is newer — or, when the user named a version
/// or passed `--force`, whenever it differs or a reinstall was asked for.
/// A build from an unreleased commit can carry a version the registry has
/// not reached yet; that is up to date, not a downgrade to offer.
fn decide(current: &semver::Version, target: &semver::Version, pinned: bool, force: bool) -> Decision {
    if force || (pinned && target != current) || target > current {
        Decision::Install
    } else {
        Decision::UpToDate
    }
}

/// The `--root` that puts the new binary where the running one is.
///
/// `cargo install --root R` writes `R/bin/<name>`, so the running binary
/// must sit in a directory called `bin`. Anything else (a `target/`
/// build, a copied binary) has no install root to reuse; guessing one
/// would install a second `idealyst` the shell may never find.
fn install_root(exe: &Path) -> Result<PathBuf> {
    let exe = exe.canonicalize().unwrap_or_else(|_| exe.to_path_buf());
    let bin = exe.parent().context("the idealyst binary has no parent directory")?;
    if bin.file_name().is_some_and(|n| n == "bin") {
        if let Some(root) = bin.parent() {
            return Ok(root.to_path_buf());
        }
    }
    bail!(
        "{} is not inside a `bin/` directory, so there is no install root to update in place. \
         Pass --root <dir> to install to <dir>/bin.",
        exe.display(),
    )
}

/// Arguments to `cargo` for the install.
fn install_argv(
    index: &str,
    version: &semver::Version,
    root: &Path,
    target_dir: &Path,
    target: Option<&str>,
) -> Vec<String> {
    let mut argv: Vec<String> = vec![
        "install".into(),
        PACKAGE.into(),
        "--index".into(),
        index.into(),
        "--version".into(),
        format!("={version}"),
        // The lockfile published with the release: the exact graph that
        // release was built and checked against.
        "--locked".into(),
        // `--force` because cargo refuses to replace an installed binary
        // from a different source (a `--git` install) without it.
        "--force".into(),
        "--root".into(),
        root.display().to_string(),
        "--target-dir".into(),
        target_dir.display().to_string(),
    ];
    if let Some(t) = target {
        argv.extend(["--target".into(), t.into()]);
    }
    argv
}

/// `~/.idealyst/cli-target`: the build cache kept between updates.
fn cache_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .context("neither HOME nor USERPROFILE is set; cannot place the build cache")?;
    Ok(PathBuf::from(home).join(".idealyst/cli-target"))
}

/// The host triple rustc builds for by default (`rustc -vV`'s `host:`).
fn rustc_host() -> Result<String> {
    let out = Command::new("rustc")
        .arg("-vV")
        .output()
        .context("running `rustc -vV` — is a Rust toolchain installed?")?;
    if !out.status.success() {
        bail!("`rustc -vV` failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    parse_rustc_host(&String::from_utf8_lossy(&out.stdout)).context("`rustc -vV` printed no host line")
}

fn parse_rustc_host(verbose: &str) -> Option<String> {
    verbose
        .lines()
        .find_map(|l| l.strip_prefix("host: "))
        .map(|h| h.trim().to_string())
}

/// The running binary, renamed out of the way on Windows so `cargo
/// install` can write the new one at its path. A no-op elsewhere.
struct MovedAside {
    #[cfg_attr(not(windows), allow(dead_code))]
    original: PathBuf,
    aside: Option<PathBuf>,
}

impl MovedAside {
    fn new(exe: &Path, root: &Path) -> Result<Self> {
        let original = root.join("bin").join(exe.file_name().context("binary has no file name")?);
        #[cfg(windows)]
        {
            if original.is_file() {
                let aside = original.with_extension("exe.old");
                // A previous update's leftover; nothing runs it any more.
                let _ = std::fs::remove_file(&aside);
                std::fs::rename(&original, &aside)
                    .with_context(|| format!("moving {} aside", original.display()))?;
                return Ok(Self { original, aside: Some(aside) });
            }
        }
        Ok(Self { original, aside: None })
    }

    fn restore(self) -> Result<()> {
        if let Some(aside) = &self.aside {
            std::fs::rename(aside, &self.original)
                .with_context(|| format!("restoring {}", self.original.display()))?;
        }
        Ok(())
    }

    /// The install succeeded. The moved-aside file can't be deleted while
    /// this process is still running from it; the next update does it.
    fn forget(self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> semver::Version {
        semver::Version::parse(s).unwrap()
    }

    #[test]
    fn index_paths_follow_cargos_prefix_rules() {
        let idx = "sparse+https://crates.idealyst.io/index/";
        assert_eq!(index_file_url(idx, "idealyst-cli"), "https://crates.idealyst.io/index/id/ea/idealyst-cli");
        assert_eq!(index_file_url(idx, "a"), "https://crates.idealyst.io/index/1/a");
        assert_eq!(index_file_url(idx, "ab"), "https://crates.idealyst.io/index/2/ab");
        assert_eq!(index_file_url(idx, "css"), "https://crates.idealyst.io/index/3/c/css");
        assert_eq!(index_file_url("https://x/index", "Wire"), "https://x/index/wi/re/wire");
    }

    #[test]
    fn newest_skips_yanked_and_prerelease_versions() {
        let body = r#"{"name":"idealyst-cli","vers":"1.6.0","deps":[],"cksum":"x","features":{},"yanked":false}
{"name":"idealyst-cli","vers":"1.8.0","yanked":true}
{"name":"idealyst-cli","vers":"1.7.1"}
{"name":"idealyst-cli","vers":"2.0.0-beta.1","yanked":false}
"#;
        let entries = parse_index_file(body).unwrap();
        assert_eq!(entries.len(), 4);
        assert_eq!(newest(&entries), Some(v("1.7.1")));
    }

    #[test]
    fn a_newer_release_installs_and_the_same_one_does_not() {
        assert_eq!(decide(&v("1.6.0"), &v("1.7.0"), false, false), Decision::Install);
        assert_eq!(decide(&v("1.6.0"), &v("1.6.0"), false, false), Decision::UpToDate);
        assert_eq!(decide(&v("1.6.0"), &v("1.6.0"), false, true), Decision::Install);
    }

    /// A binary built from an unreleased commit can be AHEAD of the
    /// registry. Plain `update` must not "update" it backwards; an
    /// explicit `--version` may.
    #[test]
    fn only_an_explicit_version_goes_backwards() {
        assert_eq!(decide(&v("1.7.0"), &v("1.6.0"), false, false), Decision::UpToDate);
        assert_eq!(decide(&v("1.7.0"), &v("1.6.0"), true, false), Decision::Install);
    }

    #[test]
    fn the_install_root_is_the_running_binarys() {
        assert_eq!(
            install_root(Path::new("/idealyst/cli/bin/idealyst")).unwrap(),
            PathBuf::from("/idealyst/cli")
        );
        let err = install_root(Path::new("/work/target/release/idealyst")).unwrap_err();
        assert!(err.to_string().contains("--root"), "{err}");
    }

    #[test]
    fn install_pins_the_version_registry_and_root() {
        let argv = install_argv(
            "sparse+https://crates.idealyst.io/index/",
            &v("1.7.0"),
            Path::new("/r"),
            Path::new("/c"),
            None,
        );
        let s = argv.join(" ");
        assert!(s.starts_with("install idealyst-cli --index sparse+https://crates.idealyst.io/index/"));
        for part in ["--version =1.7.0", "--locked", "--force", "--root /r", "--target-dir /c"] {
            assert!(s.contains(part), "missing `{part}` in `{s}`");
        }
        assert!(!s.contains("--target "), "no --target when the host already matches");
    }

    /// The Rosetta trap: rustup's default host is x86_64 on an arm64 Mac,
    /// so a bare install produces an x86_64 binary. The binary's own
    /// target goes on the command line when they differ.
    #[test]
    fn a_mismatched_rustc_host_builds_for_this_binarys_target() {
        let verbose = "rustc 1.97.1 (abc 2026-06-30)\nbinary: rustc\nhost: x86_64-apple-darwin\nrelease: 1.97.1\n";
        assert_eq!(parse_rustc_host(verbose).as_deref(), Some("x86_64-apple-darwin"));
        let argv = install_argv("i", &v("1.7.0"), Path::new("/r"), Path::new("/c"), Some("aarch64-apple-darwin"));
        assert!(argv.join(" ").ends_with("--target aarch64-apple-darwin"));
    }
}
