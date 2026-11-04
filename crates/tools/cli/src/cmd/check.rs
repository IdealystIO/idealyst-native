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
//! check — a `check` that always exits 0 is a CI footgun.
//!
//! `cargo check` doesn't link, so the cross targets don't need the
//! platform SDK/linker (no Xcode, no Android NDK) — only the rustc
//! target std installed (`rustup target add <triple>`). A missing
//! target std is reported per-triple and counted as a failure rather
//! than aborting the whole sweep.
//!
//! A full-stack project (`server_bin` / `server_manifest`) gets one more
//! check: its SERVER half, on the host, with the `server` feature. That
//! half is the one no per-triple client check ever compiles — the
//! `#[server]` bodies, the db layer behind them — so this is the one
//! command that checks such a project whole, without guessing feature
//! sets or target dirs.
//!
//! `--session` is a different command in the same place: it asks the running
//! `idealyst dev` session of the directory to run its own build of what is on
//! disk and apply nothing, in the session's target dirs, so the compile is the
//! one the next real rebuild would do and is not repeated by it. A plain
//! `idealyst check` keeps its separate `cargo check` cache. See "Checking
//! through a session" in docs/hot-reload.md.

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

    /// Also check the project's server half (host, `--features server`)
    /// when `--platform` narrows the sweep. Without `--platform` the
    /// server is checked whenever the manifest declares one.
    #[arg(long)]
    pub server: bool,

    /// Skip the server half even though the manifest declares one.
    #[arg(long, conflicts_with = "server")]
    pub no_server: bool,

    /// Build through the running `idealyst dev` session of this directory,
    /// applying nothing: the session's own builds, in its target dirs, with
    /// its flags, features and environment, queued with its other work. No
    /// server restart, no page reload, and saves a `--no-watch` session holds
    /// stay held. Exits non-zero when a target fails. The next real rebuild
    /// finds the crates compiled here fresh and only links, restarts or
    /// reloads.
    #[arg(long, conflicts_with_all = ["platform", "release", "device", "server", "no_server"])]
    pub session: bool,

    /// With `--session -p <crate>`: compile that crate's test modules
    /// (`cargo test --no-run`) in the session's server target dir, with
    /// features resolved together with the server's package so dependencies
    /// are reused. Does not run them.
    #[arg(long, requires = "session", requires = "package")]
    pub tests: bool,

    /// The crate `--tests` compiles. Only with `--tests`: a build of one crate
    /// without its test modules is not available.
    #[arg(short = 'p', long, value_name = "CRATE", requires = "tests")]
    pub package: Option<String>,
}

const SERVER_LABEL: &str = "<host, server>";

/// How long `--session` waits for the session's builds: a cold first check
/// of a large project is many minutes, and nothing is lost by waiting.
const SESSION_CHECK_WAIT: std::time::Duration = std::time::Duration::from_secs(4 * 60 * 60);

/// `idealyst check --session`: see the module docs.
fn run_through_session(dir: &std::path::Path, tests: Option<&str>) -> Result<()> {
    let registry = dev_events::sessions::registry_dir().context("this user has no session registry directory")?;
    let sessions = mcp_server::live_dev_sessions_in(&registry);
    let Some(entry) = dev_events::sessions::in_dir(&sessions, dir) else {
        anyhow::bail!(
            "no `idealyst dev` session is running in {}. Start one, or drop `--session` for a \
             `cargo check` sweep in its own cache.",
            dir.display()
        );
    };
    let answer = dev_events::sessions::run_check(entry, tests, SESSION_CHECK_WAIT).map_err(anyhow::Error::msg)?;
    let state = match answer {
        dev_events::sessions::CheckAnswer::Done(state) => state,
        dev_events::sessions::CheckAnswer::Running(state) => anyhow::bail!(
            "check {} is still running after {} hours",
            state["request"],
            SESSION_CHECK_WAIT.as_secs() / 3600
        ),
    };
    let (ok, lines) = session_report(&state, tests);
    for line in lines {
        eprintln!("{line}");
    }
    anyhow::ensure!(ok, "`idealyst check --session` failed");
    Ok(())
}

/// What a finished `GET /check/<n>` says, in lines, and whether it passed.
fn session_report(state: &serde_json::Value, tests: Option<&str>) -> (bool, Vec<String>) {
    let mut lines = Vec::new();
    for (target, report) in state["reports"].as_object().into_iter().flatten() {
        match serde_json::from_value::<dev_events::CheckReport>(report.clone()) {
            Ok(report) => {
                lines.push(dev_events::check::summary(target, &report));
                // The point of the shared cache is that only the harness compiles.
                if let Some(krate) = tests {
                    let others = report.compiled.iter().filter(|u| !u.starts_with(&format!("{krate} ("))).count();
                    if report.ok && others > 0 {
                        lines.push(format!(
                            "[check {target}] {others} other unit(s) compiled: a dev-dependency of the test build \
                             turned on a feature the session's build leaves off, or those crates had edits"
                        ));
                    }
                }
            }
            Err(e) => lines.push(format!("[check {target}] unreadable report: {e}")),
        }
    }
    for target in state["unchecked"].as_array().into_iter().flatten().filter_map(|t| t.as_str()) {
        lines.push(format!("[check] not checked: {target}"));
    }
    (state["ok"] == true, lines)
}

pub fn run(args: Args) -> Result<()> {
    let dir = crate::framework_source::abs_project_dir(&args.dir)?;
    if args.session {
        return run_through_session(&dir, args.package.as_deref().filter(|_| args.tests));
    }
    let manifest = parse_manifest(&dir)?;
    // The project's configured features reach the app's check, as they reach
    // its dev builds. The other settings choose how a build is cached, and a
    // `cargo check` has its own cache.
    let features = crate::dev_config::ProjectBuildConfig::load(&dir)?
        .and_then(|c| c.features)
        .unwrap_or_default();

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
        match check_one(&dir, triple, args.release, &features) {
            Ok(()) => eprintln!("[check] {} OK", triple.label()),
            Err(e) => {
                eprintln!("[check] {} FAILED: {e}", triple.label());
                failures.push(triple.label().to_string());
            }
        }
    }

    // The server half, when there is one. Skipped under `--platform`
    // unless the caller named it: a request for "just web" should not
    // silently grow a host compile of the db layer.
    let mut total = triples.len();
    let server_declared =
        manifest.app.server_bin.is_some() || manifest.app.server_manifest.is_some();
    let want_server = args.platform.is_empty() || args.server;
    if server_declared && want_server && !args.no_server {
        total += 1;
        eprintln!("[check] cargo check {SERVER_LABEL}");
        match check_server(&dir, &manifest, args.release) {
            Ok(()) => eprintln!("[check] {SERVER_LABEL} OK"),
            Err(e) => {
                eprintln!("[check] {SERVER_LABEL} FAILED: {e}");
                failures.push(SERVER_LABEL.to_string());
            }
        }
    } else if args.server && !server_declared {
        anyhow::bail!(
            "--server given, but the manifest declares no server \
             (set `server_bin` or `server_manifest` under \
             [package.metadata.idealyst.app])"
        );
    }

    if !failures.is_empty() {
        anyhow::bail!(
            "`idealyst check` failed for {} of {} check(s): {}",
            failures.len(),
            total,
            failures.join(", "),
        );
    }

    eprintln!("[check] all {total} check(s) passed");
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
        Target::Macos | Target::Terminal | Target::Roku | Target::Linux | Target::Windows => {
            CheckTriple::Host
        }
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
                     macos, terminal, linux, roku."
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
        Platform::Linux => Some(Target::Linux),
        Platform::Windows => Some(Target::Windows),
        Platform::Roku => Some(Target::Roku),
        Platform::Sim | Platform::RuntimeServer | Platform::Server => None,
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
fn check_one(dir: &std::path::Path, triple: &CheckTriple, release: bool, features: &[String]) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(dir).arg("check");
    if release {
        cmd.arg("--release");
    }
    if !features.is_empty() {
        cmd.arg("--features").arg(features.join(","));
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

/// Type-check the server half: the same package/manifest/feature/bin
/// selection the dev loop builds (`dev::server_build_args`), under
/// `cargo check` on the host. Same selection on purpose — a check that
/// picked a different feature set from the build would be green about a
/// binary nobody runs.
///
/// No `--target-dir` override: this lands in the workspace's own
/// `target/`, beside `cargo test`, which is where a project's tests
/// already compile the same artifact.
fn check_server(dir: &std::path::Path, manifest: &build_ios::Manifest, release: bool) -> Result<()> {
    let (_watched, args) = super::dev::server_build_args(dir, manifest)?;
    let mut cmd = Command::new("cargo");
    cmd.current_dir(dir).arg("check").args(&args);
    if release {
        cmd.arg("--release");
    }
    let status = cmd
        .status()
        .with_context(|| format!("spawn `cargo check` for {SERVER_LABEL}"))?;
    if !status.success() {
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
