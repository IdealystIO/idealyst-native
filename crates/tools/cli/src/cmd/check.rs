//! `idealyst check` — fan `cargo check` out across the project's
//! configured target triples.
//!
//! A type-check sweep for every platform the app declares (or the
//! subset passed via `--platform`). The app crate is platform-agnostic,
//! so a per-triple `cargo check` is what surfaces breakage hiding
//! behind `#[cfg(target_os = ...)]` / `#[cfg(target_arch = "wasm32")]`
//! arms that a single host check never compiles.
//!
//! Crucially this command exits NON-ZERO when any triple fails to
//! check — a `check` that always exits 0 is a CI footgun (the very
//! thing this used to be while stubbed out).
//!
//! `cargo check` doesn't link, so the cross targets don't need the
//! platform SDK/linker (no Xcode, no Android NDK) — only the rustc
//! target std installed (`rustup target add <triple>`). A missing
//! target std is reported per-triple and counted as a failure rather
//! than aborting the whole sweep.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};
use build_ios::{parse_manifest, Target};

use crate::Platform;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Project directory.
    #[arg(default_value = ".")]
    pub dir: PathBuf,

    /// Platforms to type-check, comma-separated (e.g.
    /// `--platform web,ios`). May also be repeated (`--platform web
    /// --platform ios`). Empty means all platforms configured in
    /// `[package.metadata.idealyst.app].targets`.
    ///
    /// Takes exactly one (comma-delimited) value per occurrence rather
    /// than a greedy space-separated list — a variadic `num_args` here
    /// would swallow the trailing `[DIR]` positional
    /// (`idealyst check --platform web examples/foo` would read
    /// `examples/foo` as a platform).
    #[arg(long, value_enum, value_delimiter = ',')]
    pub platform: Vec<Platform>,

    /// Check with the release profile (`cargo check --release`).
    #[arg(long)]
    pub release: bool,

    /// iOS only: check the physical-device triple
    /// (`aarch64-apple-ios`) instead of the simulator triple.
    #[arg(long)]
    pub device: bool,
}

pub fn run(args: Args) -> Result<()> {
    let dir = std::fs::canonicalize(&args.dir)
        .with_context(|| format!("cannot resolve project dir {}", args.dir.display()))?;
    let manifest = parse_manifest(&dir)?;

    // Resolve which targets to check. Explicit `--platform` wins;
    // otherwise fall back to the manifest's declared targets — the same
    // precedence `idealyst build` / `dev` use.
    let targets = resolve_targets(&args.platform, &manifest.app.targets)?;

    // Map each target to its rustc triple. Several targets share a triple
    // (or the host), so de-dup: checking `x86_64-apple-darwin` twice
    // because both `macos` and `terminal` were configured is wasted work.
    // `BTreeSet` keeps the sweep order stable for reproducible CI logs.
    let mut triples: BTreeSet<CheckTriple> = BTreeSet::new();
    for t in &targets {
        triples.insert(triple_for(*t, args.device));
    }

    eprintln!(
        "[check] {} profile, {} triple(s): {}",
        if args.release { "release" } else { "debug" },
        triples.len(),
        triples
            .iter()
            .map(|t| t.label())
            .collect::<Vec<_>>()
            .join(", "),
    );

    let mut failures: Vec<String> = Vec::new();
    for triple in &triples {
        eprintln!("[check] cargo check {}", triple.label());
        match check_one(&dir, triple, args.release) {
            Ok(()) => eprintln!("[check] {} OK", triple.label()),
            Err(e) => {
                eprintln!("[check] {} FAILED: {e}", triple.label());
                failures.push(triple.label().to_string());
            }
        }
    }

    if !failures.is_empty() {
        anyhow::bail!(
            "`idealyst check` failed for {} of {} triple(s): {}",
            failures.len(),
            triples.len(),
            failures.join(", "),
        );
    }

    eprintln!("[check] all {} triple(s) passed", triples.len());
    Ok(())
}

/// The rustc target a [`Target`] checks against. `Host` means "the
/// machine's native triple" — passed to cargo as *no* `--target`, so
/// the check runs against the default toolchain. Used for targets whose
/// platform code is host-shaped (terminal, native macOS, roku) where
/// there's no distinct cross triple to type-check.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum CheckTriple {
    /// A concrete cross triple, e.g. `wasm32-unknown-unknown`.
    Cross(&'static str),
    /// The host's native triple (no `--target` flag).
    Host,
}

impl CheckTriple {
    fn label(&self) -> &'static str {
        match self {
            CheckTriple::Cross(t) => t,
            CheckTriple::Host => "<host>",
        }
    }
}

/// Map a configured platform target to the rustc triple `cargo check`
/// should compile it for.
fn triple_for(target: Target, ios_device: bool) -> CheckTriple {
    match target {
        Target::Web => CheckTriple::Cross("wasm32-unknown-unknown"),
        Target::Ios => CheckTriple::Cross(build_ios::pick_target(ios_device)),
        // The Android backend builds for arm64 devices/emulators. `cargo
        // check` only needs the rustc std for the triple — no NDK linker.
        Target::Android => CheckTriple::Cross("aarch64-linux-android"),
        // Native macOS / terminal / roku are host-shaped for type-check
        // purposes: their platform code compiles for the host triple, so
        // checking against `<host>` exercises it. (A real ship build still
        // goes through the per-target wrapper crates.)
        Target::Macos | Target::Terminal | Target::Roku => CheckTriple::Host,
    }
}

/// Resolve the target set to check. `--platform` filter wins; otherwise
/// fall back to the manifest's `targets`. Mirrors the precedence in
/// `dev::resolve_targets` / `build::collect_targets` so all three
/// commands agree on "which platforms does this project mean".
fn resolve_targets(platforms: &[Platform], manifest_targets: &[Target]) -> Result<Vec<Target>> {
    if !platforms.is_empty() {
        let mut out = Vec::new();
        for p in platforms {
            match platform_to_target(*p) {
                Some(t) => out.push(t),
                None => anyhow::bail!(
                    "platform `{p}` has no type-check target (it's a dev-host / preview, \
                     not a build target). Pass a buildable platform: web, ios, android, \
                     macos, terminal, roku."
                ),
            }
        }
        return Ok(dedup_preserve_order(out));
    }
    if !manifest_targets.is_empty() {
        return Ok(dedup_preserve_order(manifest_targets.to_vec()));
    }
    anyhow::bail!(
        "no targets to check: pass `--platform web,ios,...`, or add \
         `targets = [\"web\", ...]` to `[package.metadata.idealyst.app]`"
    )
}

/// Map the CLI `Platform` value-enum onto a buildable `Target`. The
/// dev-only platforms (`sim`, `runtime-server`) have no standalone
/// type-check triple, so they map to `None` and the caller errors.
fn platform_to_target(p: Platform) -> Option<Target> {
    match p {
        Platform::Web => Some(Target::Web),
        Platform::Ios => Some(Target::Ios),
        Platform::Android => Some(Target::Android),
        Platform::Macos => Some(Target::Macos),
        Platform::Terminal => Some(Target::Terminal),
        Platform::Roku => Some(Target::Roku),
        Platform::Sim | Platform::RuntimeServer => None,
    }
}

fn dedup_preserve_order(xs: Vec<Target>) -> Vec<Target> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for x in xs {
        if seen.insert(x) {
            out.push(x);
        }
    }
    out
}

/// Run a single `cargo check` for one triple in the project dir.
fn check_one(dir: &std::path::Path, triple: &CheckTriple, release: bool) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(dir).arg("check");
    if release {
        cmd.arg("--release");
    }
    if let CheckTriple::Cross(t) = triple {
        cmd.arg("--target").arg(t);
    }

    let status = cmd
        .status()
        .with_context(|| format!("spawn `cargo check` for {}", triple.label()))?;

    if !status.success() {
        // A common cause is a missing rustc std for the cross triple.
        // Surface the actionable fix rather than just the exit code.
        if let CheckTriple::Cross(t) = triple {
            anyhow::bail!(
                "cargo check exited {status}. If the error mentions a missing target, run \
                 `rustup target add {t}` and retry."
            );
        }
        anyhow::bail!("cargo check exited {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_checks_wasm_triple() {
        assert!(matches!(
            triple_for(Target::Web, false),
            CheckTriple::Cross("wasm32-unknown-unknown")
        ));
    }

    #[test]
    fn ios_device_flag_picks_device_triple() {
        assert!(matches!(
            triple_for(Target::Ios, true),
            CheckTriple::Cross("aarch64-apple-ios")
        ));
    }

    #[test]
    fn android_checks_arm64_triple() {
        assert!(matches!(
            triple_for(Target::Android, false),
            CheckTriple::Cross("aarch64-linux-android")
        ));
    }

    #[test]
    fn host_shaped_targets_use_host_triple() {
        assert!(matches!(triple_for(Target::Macos, false), CheckTriple::Host));
        assert!(matches!(triple_for(Target::Terminal, false), CheckTriple::Host));
        assert!(matches!(triple_for(Target::Roku, false), CheckTriple::Host));
    }

    #[test]
    fn triples_dedup_across_host_shaped_targets() {
        // macos + terminal both collapse to the host triple → one check.
        let mut set: BTreeSet<CheckTriple> = BTreeSet::new();
        set.insert(triple_for(Target::Macos, false));
        set.insert(triple_for(Target::Terminal, false));
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn empty_platforms_falls_back_to_manifest() {
        let got = resolve_targets(&[], &[Target::Web, Target::Ios]).unwrap();
        assert_eq!(got, vec![Target::Web, Target::Ios]);
    }

    #[test]
    fn platform_filter_overrides_manifest() {
        let got =
            resolve_targets(&[Platform::Web], &[Target::Web, Target::Ios, Target::Android]).unwrap();
        assert_eq!(got, vec![Target::Web]);
    }

    #[test]
    fn dev_only_platform_is_rejected() {
        let err = resolve_targets(&[Platform::Sim], &[Target::Web]).unwrap_err();
        assert!(err.to_string().contains("no type-check target"), "got: {err}");
    }

    #[test]
    fn no_targets_anywhere_errors() {
        let err = resolve_targets(&[], &[]).unwrap_err();
        assert!(err.to_string().contains("no targets to check"), "got: {err}");
    }

    #[test]
    fn manifest_targets_dedup() {
        let got = resolve_targets(&[], &[Target::Web, Target::Web, Target::Ios]).unwrap();
        assert_eq!(got, vec![Target::Web, Target::Ios]);
    }
}
