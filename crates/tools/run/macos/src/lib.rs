//! macOS launcher. Builds via `build-macos`, then either spawns the
//! binary as a foreground child (one-shot `idealyst run macos`) or
//! fire-and-forgets it (dev mode, where blocking on the app's
//! lifecycle would tie up the orchestrator's other targets).

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result};
use build_ios::{capabilities, parse_manifest, FrameworkSource};

/// App Store + Developer-ID distribution for `idealyst publish macos`:
/// distribution-signed `.app` → `.pkg` upload (Mac App Store) or notarized,
/// stapled `.dmg` (direct). Reuses this crate's `.app` assembly + signing
/// helpers; only the distribution layer (entitlements, productbuild,
/// notarytool, altool) is new.
pub mod publish;

/// Which build path to spawn. `Local` mounts the user's `app()`
/// in-process via `host_appkit::run`; `Aas` connects to a dev-server
/// via `host_appkit::run_aas` and streams the sidecar's commands.
/// runtime-server produces a wrapper that does NOT depend on the user's crate
/// (the sidecar process owns it), so changes to user code don't
/// require recompiling the wrapper — only the sidecar.
#[derive(Clone, Debug)]
pub enum RunMode {
    Local,
    RuntimeServer,
}

impl RunMode {
    pub fn is_runtime_server(&self) -> bool {
        matches!(self, RunMode::RuntimeServer)
    }
}

#[derive(Clone, Debug)]
pub struct RunOptions {
    /// Compile with `--release`. Default: debug.
    pub release: bool,
    /// Selects between local-mount (default) and runtime-server-client paths.
    pub mode: RunMode,
    /// Framework-source resolution for the wrapper crate's deps.
    pub source: FrameworkSource,
    /// If true, spawn the binary detached (stdin nulled, parent's
    /// launcher thread returns immediately so other dev targets can
    /// launch). Used by `idealyst dev`. NOTE this does not decouple the
    /// app's lifetime from the CLI's — the dev orchestrator links them
    /// bidirectionally: it sets
    /// `IDEALYST_LAUNCHER_PID` so the AppKit host's watchdog exits the app
    /// when the CLI dies, and it waits on the returned child so closing
    /// the app window tears the dev session down. One-shot `idealyst run
    /// macos` leaves this false — the user there expects a foreground
    /// process they can Ctrl-C.
    pub background: bool,
    /// Cargo features to enable on the build. `idealyst dev` passes the
    /// wrapper's `dev` feature (→ `runtime-core/dev`) here so the Robot
    /// bridge auto-starts.
    pub user_features: Vec<String>,
    /// Environment variables to set on the spawned binary.
    /// `idealyst dev` uses this to plumb `IDEALYST_BRIDGE_PORT_FILE`
    /// (and optionally `IDEALYST_BRIDGE_PORT`) so the running app's
    /// Robot bridge writes its port discovery file to a project-local
    /// `.idealyst/bridge.port`.
    pub env_vars: Vec<(String, String)>,
}

#[derive(Debug)]
pub struct RunArtifact {
    /// Path to the binary that was launched.
    pub binary: PathBuf,
    /// `Some` in background mode — the still-running spawned
    /// [`Child`]. Foreground mode waits-and-drops, leaving `None`.
    /// The dev orchestrator pushes this into its shared `children` Vec
    /// so the SIGINT handler can kill the detached binary — without the
    /// handle every `idealyst dev --macos` session leaks the app process.
    pub child: Option<Child>,
}

/// Build (or rebuild) the macOS wrapper for `project_dir` and launch
/// it. Foreground mode blocks until the app exits; background mode
/// returns once the binary has been spawned.
pub fn run(project_dir: &Path, opts: RunOptions) -> Result<RunArtifact> {
    let build_mode = match opts.mode {
        RunMode::Local => build_macos::BuildMode::Local,
        RunMode::RuntimeServer => build_macos::BuildMode::RuntimeServer,
    };
    let built = build_macos::build(
        project_dir,
        build_macos::BuildOptions {
            release: opts.release,
            mode: build_mode,
            source: opts.source,
            user_features: opts.user_features.clone(),
            universal: false, // dev/run: fast host-arch build
        },
    )?;

    // If the project has an `[icon]` block, wrap the bare binary
    // in a minimal `<App>.app` bundle so macOS picks up the icon
    // (via Info.plist's `CFBundleIconFile`) and shows it in the
    // dock + command-tab. Without an icon block we keep the
    // historical bare-binary path — the dock just shows the
    // Terminal icon. The wrapping always re-runs (cheap: a copy
    // and a few small file writes), so editing the icon and
    // re-launching always picks up the new icns.
    // Capability-derived Info.plist usage-description keys (e.g.
    // NSMicrophoneUsageDescription). Discovered from the wrapper's
    // dependency graph; their presence also forces a `.app` bundle even
    // when the project declares no icon, because a bare binary has no
    // Info.plist for the OS to read the usage string from.
    let permission_pairs = macos_permission_pairs(
        &built.wrapper_dir.join("Cargo.toml"),
        &parse_manifest(project_dir)?.app.permissions,
    );

    let links = parse_manifest(project_dir)?.app.links;
    // Resolved before the bundle is laid out: SDK bundle templates can name
    // `{{team_id}}`, which comes from the signing identity at the latest.
    let identity = resolve_macos_signing_identity();
    let team_id = if built.sdk.is_empty() {
        None
    } else {
        resolve_dev_team_id(links.apple_team_id.as_deref(), identity.as_deref())
    };
    let wrapped = maybe_wrap_in_app_bundle(
        project_dir,
        &built.binary,
        &permission_pairs,
        &links,
        &built.sdk,
        team_id.as_deref(),
    )?;
    let spawn_target = match &wrapped {
        Some(app) => app.inner_binary.clone(),
        None => built.binary.clone(),
    };

    // Code-sign the `.app` with a STABLE Apple Development identity so macOS
    // TCC grants (Screen Recording, Camera, Microphone) PERSIST across rebuilds.
    // The Rust linker auto-ad-hoc-signs with a content-hash identifier
    // (`whiteboard_demo_macos-<hash>`) that changes every build, so the OS sees
    // each rebuild as a new app and forgets the grant — the "Screen Recording
    // keeps re-prompting even though the app is enabled in Settings" bug.
    // Signing with the developer's cert pins a stable code identity (cert +
    // bundle id), exactly like the iOS path. Honors
    // `IDEALYST_DEVELOPMENT_TEAM` / `DEVELOPMENT_TEAM` to pick a team; otherwise
    // uses the first Apple Development cert. No-op (ad-hoc) if none is found.
    if let Some(app) = &wrapped {
        match &identity {
            Some(identity) => {
                // SDK-declared app entitlements; a dev signature has none
                // of its own. Restricted ones need a provisioning profile.
                let profile = std::env::var_os(DEV_PROFILE_ENV).map(PathBuf::from);
                let manifest = parse_manifest(project_dir)?;
                let executable = app.inner_binary.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                let dev = dev_entitlements_for(
                    &manifest.app,
                    executable,
                    &built.sdk.app_entitlements,
                    profile.is_some(),
                    team_id.as_deref(),
                );
                if !dev.dropped.is_empty() {
                    eprintln!(
                        "[run-macos] not signing with {} — restricted entitlements need a provisioning \
                         profile, and macOS kills an app that claims them without one. The features \
                         that need them report an error at runtime instead. Set {DEV_PROFILE_ENV} to a \
                         macOS development .provisionprofile for this bundle id to include them.",
                        dev.dropped.join(", "),
                    );
                }
                if let Some(p) = &profile {
                    let dest = app.app_dir.join("Contents").join("embedded.provisionprofile");
                    fs::copy(p, &dest).with_context(|| format!("copy {} → {}", p.display(), dest.display()))?;
                }
                let entitlements = if dev.booleans.is_empty() && dev.strings.is_empty() {
                    None
                } else {
                    let path = app.app_dir.with_extension("dev-entitlements.plist");
                    fs::write(&path, publish::entitlements_plist(false, &dev.booleans, &dev.strings, &[]))
                        .with_context(|| format!("write {}", path.display()))?;
                    Some(path)
                };
                let plan = codesign_plan(
                    &app.app_dir,
                    identity,
                    entitlements.as_deref(),
                    &app.nested,
                    SignFlags { timestamp: false, hardened_runtime: false },
                );
                run_codesign("run-macos", &plan)?;
            }
            None => eprintln!(
                "[run-macos] no Apple Development signing identity found — \
                 leaving the linker ad-hoc signature{}. macOS will re-prompt for \
                 Screen Recording / Camera permission on every rebuild. Install \
                 an Apple Development certificate (or set DEVELOPMENT_TEAM) to \
                 make grants stick.",
                if app.nested.is_empty() { "" } else { " (SDK bundles stay unsigned, and the OS will refuse to load them)" },
            ),
        }
    }

    // A custom URL scheme only opens the app once LaunchServices knows the
    // bundle declares it. A dev bundle is launched by spawning its binary
    // directly (never through `open`), which doesn't register it — so
    // register explicitly. Best effort: the app still runs without it,
    // it just isn't reachable by `myapp://` links yet.
    if !links.schemes.is_empty() {
        if let Some(app_bundle) = app_bundle_for(&spawn_target) {
            register_with_launch_services(&app_bundle);
        }
    }

    eprintln!(
        "[run-macos] launching {} (release={}, background={})",
        spawn_target.display(),
        opts.release,
        opts.background,
    );

    let child = if opts.background {
        // Detach: null stdin so the app doesn't fight the dev
        // orchestrator's terminal input, but pipe stdout/stderr
        // through to the orchestrator so runtime-server-mode connection logs +
        // any apply-time panic from the macOS binary actually
        // surface (nulled, a "nothing renders" crash is silent).
        // Leave the child unwaited
        // so we can return; the returned `Child` handle goes into
        // the dev orchestrator's `children` Vec so Ctrl-C reaches
        // it.
        let mut cmd = Command::new(&spawn_target);
        cmd.stdin(Stdio::null());
        for (k, v) in &opts.env_vars {
            cmd.env(k, v);
        }
        let child = cmd
            .spawn()
            .with_context(|| {
                format!("spawn macOS binary {}", spawn_target.display())
            })?;
        Some(child)
    } else {
        let mut cmd = Command::new(&spawn_target);
        for (k, v) in &opts.env_vars {
            cmd.env(k, v);
        }
        let status = cmd
            .status()
            .with_context(|| {
                format!("spawn macOS binary {}", spawn_target.display())
            })?;
        if !status.success() {
            anyhow::bail!("macOS binary exited with {status}");
        }
        None
    };

    Ok(RunArtifact {
        binary: spawn_target,
        child,
    })
}

/// The `.app` bundle that contains `binary` (walking up the path), if any.
fn app_bundle_for(binary: &Path) -> Option<PathBuf> {
    binary
        .ancestors()
        .find(|p| p.extension().map(|e| e == "app").unwrap_or(false))
        .map(|p| p.to_path_buf())
}

/// Resolve a stable codesigning identity (SHA-1 hash) for the local dev
/// signature — the first "Apple Development" cert, team-filtered.
fn resolve_macos_signing_identity() -> Option<String> {
    resolve_signing_identity("codesigning", &["Apple Development"])
}

/// Resolve a signing-identity SHA-1 from the login keychain whose cert name
/// contains one of `kinds` (tried in listed priority), honoring
/// `IDEALYST_DEVELOPMENT_TEAM` / `DEVELOPMENT_TEAM` to disambiguate when
/// multiple teams' certs are installed. `policy` is the `security
/// find-identity -p <policy>` value: `"codesigning"` for app-signing certs
/// (Apple Development / Apple Distribution / Developer ID Application),
/// `"basic"` for installer certs (3rd Party Mac Developer Installer), which
/// are NOT codesigning identities and won't appear under `codesigning`.
/// Returns `None` when no matching identity is installed.
pub(crate) fn resolve_signing_identity(policy: &str, kinds: &[&str]) -> Option<String> {
    let out = Command::new("security")
        .args(["find-identity", "-v", "-p", policy])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let team = std::env::var("IDEALYST_DEVELOPMENT_TEAM")
        .or_else(|_| std::env::var("DEVELOPMENT_TEAM"))
        .ok()
        .filter(|t| !t.is_empty());
    select_identity_from_listing(&text, team.as_deref(), kinds)
}

/// Pure core of [`resolve_signing_identity`] — pick a matching identity hash
/// from `security find-identity` output. A line looks like
/// `  1) <40-hex-sha1> "Apple Distribution: Name (TEAMID)"`. When `team` is
/// set, an identity carrying `(TEAMID)` wins; otherwise the first cert whose
/// name contains one of `kinds`. Split out so it's unit-testable without the
/// keychain.
pub(crate) fn select_identity_from_listing(
    text: &str,
    team: Option<&str>,
    kinds: &[&str],
) -> Option<String> {
    let mut first: Option<String> = None;
    for line in text.lines() {
        let Some(paren) = line.find(") ") else { continue };
        let rest = line[paren + 2..].trim();
        let mut parts = rest.splitn(2, ' ');
        let hash = parts.next().unwrap_or("");
        let name = parts.next().unwrap_or("");
        if hash.len() != 40 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        if !kinds.iter().any(|k| name.contains(k)) {
            continue;
        }
        if let Some(team) = team {
            if name.contains(&format!("({team})")) {
                return Some(hash.to_string());
            }
        }
        if first.is_none() {
            first = Some(hash.to_string());
        }
    }
    first
}

/// `lsregister -f <app>`: make LaunchServices read the bundle's
/// `CFBundleURLTypes` so its custom schemes open it. Warns (never fails the
/// run) when the tool is missing or refuses.
fn register_with_launch_services(app: &Path) {
    const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/\
        Frameworks/LaunchServices.framework/Support/lsregister";
    match Command::new(LSREGISTER).arg("-f").arg(app).status() {
        Ok(status) if status.success() => {}
        Ok(status) => eprintln!(
            "[run-macos] lsregister exited with {status}; custom URL schemes may not open {}",
            app.display()
        ),
        Err(e) => eprintln!(
            "[run-macos] could not run lsregister ({e}); custom URL schemes may not open {}",
            app.display()
        ),
    }
}

/// The team SDK bundle templates' `{{team_id}}` resolves to on a dev run:
/// `IDEALYST_DEVELOPMENT_TEAM` / `DEVELOPMENT_TEAM`, else the manifest's
/// `[..app.links] apple_team_id`, else the OU of the signing identity's
/// certificate. `None` is only an error for a template that uses it.
fn resolve_dev_team_id(manifest_team: Option<&str>, identity: Option<&str>) -> Option<String> {
    std::env::var("IDEALYST_DEVELOPMENT_TEAM")
        .or_else(|_| std::env::var("DEVELOPMENT_TEAM"))
        .ok()
        .filter(|t| !t.is_empty())
        .or_else(|| manifest_team.map(str::to_string))
        .or_else(|| identity.and_then(certificate_team_id))
}

/// Environment variable naming a macOS development `.provisionprofile` a dev
/// run embeds, so the app may carry restricted SDK entitlements.
const DEV_PROFILE_ENV: &str = "IDEALYST_MACOS_PROVISIONING_PROFILE";

/// What a dev signature carries from the SDK-declared app entitlements.
#[derive(Debug, PartialEq, Eq)]
struct DevEntitlements {
    booleans: Vec<String>,
    /// String entitlements the profile requires the signature to match.
    strings: Vec<(String, String)>,
    /// Declared but left out (restricted, no profile).
    dropped: Vec<String>,
}

/// Decide a dev signature's entitlements. `com.apple.developer.*` keys are
/// restricted: AMFI SIGKILLs an app at launch that claims one without an
/// embedded provisioning profile authorising it, so without a profile they
/// are dropped (the feature fails at runtime with an error the app can
/// show). With a profile they are kept, along with the application- and
/// team-identifier entitlements the profile is matched against.
/// The app's bundle identifier: the manifest's `bundle_id`, else
/// `com.example.<executable>`. One rule for the `.app`'s `CFBundleIdentifier`
/// and the dev signature's `application-identifier`, which must agree; and a
/// dev run never requires a `bundle_id` the bundle itself doesn't.
fn app_bundle_id(app: &build_ios::AppMetadata, executable: &str) -> String {
    app.bundle_id.clone().unwrap_or_else(|| format!("com.example.{executable}"))
}

/// [`dev_entitlements`] for the app being signed, its bundle id resolved the
/// way its Info.plist resolves it.
fn dev_entitlements_for(
    app: &build_ios::AppMetadata,
    executable: &str,
    declared: &[String],
    has_profile: bool,
    team: Option<&str>,
) -> DevEntitlements {
    dev_entitlements(declared, has_profile, team, &app_bundle_id(app, executable))
}

fn dev_entitlements(declared: &[String], has_profile: bool, team: Option<&str>, bundle_id: &str) -> DevEntitlements {
    let restricted = |e: &String| e.starts_with("com.apple.developer.");
    if !has_profile {
        let (dropped, booleans) = declared.iter().cloned().partition(restricted);
        return DevEntitlements { booleans, strings: Vec::new(), dropped };
    }
    let strings = match team {
        Some(t) => vec![
            ("com.apple.application-identifier".to_string(), format!("{t}.{bundle_id}")),
            ("com.apple.developer.team-identifier".to_string(), t.to_string()),
        ],
        None => Vec::new(),
    };
    DevEntitlements { booleans: declared.to_vec(), strings, dropped: Vec::new() }
}

/// The team id (subject OU) of the keychain certificate whose SHA-1 is
/// `identity`. The `(…)` in an "Apple Development: Name (XXXXXXXXXX)"
/// identity name is a per-person id, not the team, so it is never used.
fn certificate_team_id(identity: &str) -> Option<String> {
    let listing = Command::new("security").args(["find-certificate", "-a", "-Z", "-p"]).output().ok()?;
    let pem = pem_for_sha1(&String::from_utf8_lossy(&listing.stdout), identity)?;
    let mut child = Command::new("openssl")
        .args(["x509", "-noout", "-subject"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .ok()?;
    {
        use std::io::Write;
        child.stdin.take()?.write_all(pem.as_bytes()).ok()?;
    }
    let out = child.wait_with_output().ok()?;
    team_from_subject(&String::from_utf8_lossy(&out.stdout))
}

/// The PEM block following `SHA-1 hash: <sha1>` in `security
/// find-certificate -a -Z -p` output.
fn pem_for_sha1(listing: &str, sha1: &str) -> Option<String> {
    let mut lines = listing.lines().skip_while(|l| {
        !l.trim().strip_prefix("SHA-1 hash:").is_some_and(|h| h.trim().eq_ignore_ascii_case(sha1))
    });
    lines.next()?;
    let mut pem = String::new();
    for line in lines.skip_while(|l| !l.starts_with("-----BEGIN CERTIFICATE-----")) {
        pem.push_str(line);
        pem.push('\n');
        if line.starts_with("-----END CERTIFICATE-----") {
            return Some(pem);
        }
    }
    None
}

/// The `OU=` of an `openssl x509 -subject` line, in OpenSSL's
/// (`subject=UID=…, CN=…, OU=TEAM, …`) or LibreSSL's (`subject= /UID=…/OU=TEAM/…`)
/// format.
fn team_from_subject(subject: &str) -> Option<String> {
    let body = subject.trim().strip_prefix("subject=").unwrap_or(subject).trim();
    body.split(['/', ','])
        .filter_map(|part| part.trim().strip_prefix("OU="))
        .map(|v| v.trim().to_string())
        .find(|v| !v.is_empty())
}

/// How `codesign` is invoked for one signing pass.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SignFlags {
    /// `--timestamp` (a secure timestamp; notarization requires it).
    pub timestamp: bool,
    /// `--options runtime` on the app (nested SDK bundles always get it).
    pub hardened_runtime: bool,
}

/// The `codesign` invocations (arguments after `codesign`) that sign `app`,
/// in order. Nested SDK bundles come first, each with its own entitlements
/// and the hardened runtime; the app's signature then seals them.
///
/// The app is signed `--deep` only when it holds no SDK bundle: `--deep`
/// re-signs the nested code it finds with the APP's `--entitlements` and
/// `--options`, replacing each bundle's own (measured with codesign on
/// macOS 26: a bundle under `Contents/PlugIns` signed with sandbox +
/// app-group entitlements and the hardened runtime came out with only the
/// app's entitlement and no runtime flag) — and an extension without its
/// own entitlements is refused by the OS.
pub(crate) fn codesign_plan(
    app: &Path,
    identity: &str,
    app_entitlements: Option<&Path>,
    nested: &[build_macos::sdk_bundles::AssembledBundle],
    flags: SignFlags,
) -> Vec<Vec<OsString>> {
    let mut plan = Vec::with_capacity(nested.len() + 1);
    for bundle in nested {
        let mut args: Vec<OsString> = vec!["--force".into()];
        if flags.timestamp {
            args.push("--timestamp".into());
        }
        args.extend(["--options".into(), "runtime".into()]);
        if let Some(e) = &bundle.entitlements {
            args.extend(["--entitlements".into(), e.into()]);
        }
        args.extend(["--sign".into(), identity.into(), bundle.path.clone().into()]);
        plan.push(args);
    }
    let mut args: Vec<OsString> = vec!["--force".into()];
    if nested.is_empty() {
        args.push("--deep".into());
    }
    if flags.timestamp {
        args.push("--timestamp".into());
    }
    if flags.hardened_runtime {
        args.extend(["--options".into(), "runtime".into()]);
    }
    if let Some(e) = app_entitlements {
        args.extend(["--entitlements".into(), e.into()]);
    }
    args.extend(["--sign".into(), identity.into(), app.into()]);
    plan.push(args);
    plan
}

/// Run each `codesign` invocation of `plan` in order. The identity (a
/// SHA-1) is shortened in the log line.
pub(crate) fn run_codesign(label: &str, plan: &[Vec<OsString>]) -> Result<()> {
    for args in plan {
        let shown: Vec<String> = args
            .iter()
            .map(|a| {
                let a = a.to_string_lossy();
                if a.len() == 40 && a.chars().all(|c| c.is_ascii_hexdigit()) { format!("{}…", &a[..10]) } else { a.into_owned() }
            })
            .collect();
        eprintln!("[{label}] codesign {}", shown.join(" "));
        let status = Command::new("codesign").args(args).status().context("invoke codesign")?;
        if !status.success() {
            anyhow::bail!("codesign {} failed (status {status})", shown.join(" "));
        }
    }
    Ok(())
}

/// Wrap `binary` in a `<App>.app/Contents/{MacOS,Resources}` bundle
/// with Info.plist + AppIcon.icns. Returns the path to the spawnable
/// binary INSIDE the bundle (which is what gets launched so macOS
/// reads the parent `.app`'s metadata for dock chrome).
///
/// Returns `Ok(None)` when the project has no `[icon]` block, no
/// capability-derived permission keys, no custom URL schemes, no `assets/`
/// and no SDK bundle or app entitlement — the caller falls back to the
/// bare-binary launch path. A project with permissions but no icon is
/// still wrapped, because the usage-description strings have to live in an
/// `Info.plist` for the OS to show them. Errors out on genuinely broken
/// icon configs (missing source file, invalid gradient stops, etc.)
/// because users typing them want loud feedback, not a silently iconless
/// app.
fn maybe_wrap_in_app_bundle(
    project_dir: &Path,
    binary: &Path,
    permissions: &[(String, String)],
    links: &build_ios::links::LinksMetadata,
    sdk: &build_macos::sdk_bundles::SdkBundles,
    team_id: Option<&str>,
) -> Result<Option<AssembledApp>> {
    let config = icon_gen::load_config_from_manifest(project_dir)?;
    // URL schemes live in the bundle's Info.plist too: without a bundle
    // the OS has nothing to route `myapp://` links to. An `assets/` dir
    // forces the bundle as well, so dev runs read `asset!` files from
    // `Contents/Resources/assets/` exactly as a shipped app does. SDK
    // bundles need an app to nest in, and the entitlements SDKs declare
    // are applied by signing the bundle.
    if config.is_none()
        && permissions.is_empty()
        && links.schemes.is_empty()
        && build_ios::app_assets::app_assets_dir(project_dir).is_none()
        && sdk.is_empty()
    {
        return Ok(None);
    }
    Ok(Some(assemble_app_bundle(project_dir, binary, permissions, sdk, team_id)?))
}

/// A laid-out `.app` bundle.
pub(crate) struct AssembledApp {
    /// The `<App>.app` directory (what gets signed / packaged).
    pub app_dir: PathBuf,
    /// The executable inside `Contents/MacOS/` (what gets launched).
    pub inner_binary: PathBuf,
    /// The SDK bundles nested in it, to be signed before it
    /// ([`codesign_plan`]).
    pub nested: Vec<build_macos::sdk_bundles::AssembledBundle>,
}

/// Assemble `<App>.app/Contents/{MacOS,Resources}` around `binary` with an
/// Info.plist + (when declared) AppIcon.icns. ALWAYS wraps — used by the
/// publish path, which needs a guaranteed bundle. The dev/run path goes
/// through [`maybe_wrap_in_app_bundle`], which skips wrapping when nothing
/// needs a bundle. SDK bundles are laid out at their declared places, their
/// templates rendered with `team_id` (see `build_macos::sdk_bundles`); they
/// still need signing before the app ([`codesign_plan`]). Errors out on a broken icon
/// config (missing source, invalid gradient) rather than shipping a
/// silently iconless app.
pub(crate) fn assemble_app_bundle(
    project_dir: &Path,
    binary: &Path,
    permissions: &[(String, String)],
    sdk: &build_macos::sdk_bundles::SdkBundles,
    team_id: Option<&str>,
) -> Result<AssembledApp> {
    let config = icon_gen::load_config_from_manifest(project_dir)?;
    let manifest = parse_manifest(project_dir)?;

    // Bundle sits next to the cargo-emitted binary so a `cargo
    // clean` reaches it the same way it reaches everything else.
    let bin_dir = binary
        .parent()
        .ok_or_else(|| anyhow::anyhow!("binary path has no parent: {}", binary.display()))?;
    let bin_name = binary
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("binary path has no filename: {}", binary.display()))?
        .to_string();
    let display_name = manifest.app.name.clone();
    let app_dir = bin_dir.join(format!("{display_name}.app"));
    let contents = app_dir.join("Contents");
    let macos_dir = contents.join("MacOS");
    let resources = contents.join("Resources");
    fs::create_dir_all(&macos_dir).with_context(|| format!("create {}", macos_dir.display()))?;
    fs::create_dir_all(&resources).with_context(|| format!("create {}", resources.display()))?;

    // Generate `AppIcon.icns` straight into Resources/ — ALWAYS, so the
    // bundle has a valid icon (the Mac App Store rejects an iconless app, and
    // requires the 512pt@2x slot — error 409). Uses the project's declared
    // icon when present, else a generated placeholder (icon-gen owns the
    // fallback). icon-gen's cache skips work when a declared source hasn't
    // changed. `CFBundleIconFile` references the stem ("AppIcon").
    let block = config
        .as_ref()
        .map(|c| c.resolved_for(icon_gen::Target::Macos));
    let icns_outs = icon_gen::sync_macos_icns(block.as_ref(), &resources)?;
    let icon_file_stem = icns_outs
        .icns
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::to_string);

    // Copy the binary INTO MacOS/. Using `fs::copy` (not a hard
    // link / symlink) so the .app is self-contained and survives
    // moving outside `target/` for distribution. `set_permissions`
    // preserves the executable bit fs::copy already carried over,
    // but we re-assert it defensively in case future cargo output
    // strips it.
    let dest_binary = macos_dir.join(&bin_name);
    fs::copy(binary, &dest_binary)
        .with_context(|| format!("copy {} → {}", binary.display(), dest_binary.display()))?;
    set_executable(&dest_binary)?;

    // Info.plist. Minimal; carries icon ref + identity. Future
    // additions (NSPrincipalClass, NSHighResolutionCapable) layer
    // on top.
    let plist = render_info_plist(
        &display_name,
        &manifest.app,
        &bin_name,
        icon_file_stem.as_deref(),
        permissions,
    );
    fs::write(contents.join("Info.plist"), plist)
        .with_context(|| format!("write {}/Info.plist", contents.display()))?;
    // `PkgInfo` is historical but still required by some tooling
    // (xattr-based icon caches, older LaunchServices).
    fs::write(contents.join("PkgInfo"), b"APPL????")?;

    // The app's `assets/` → `Contents/Resources/assets/`, where the AppKit
    // backend resolves `asset!` paths (`NSBundle.mainBundle.resourcePath`).
    // The bundle persists across runs, so the sync only rewrites changed
    // files. Written before signing — the publish path codesigns the
    // finished bundle, and a resource added after would break the seal.
    ship_bundle_assets(project_dir, &resources)?;

    // SDK bundles (plug-ins, system extensions) at their declared places,
    // their entitlements rendered beside the `.app` (outside its seal).
    let mut nested = Vec::with_capacity(sdk.bundles.len());
    if !sdk.bundles.is_empty() {
        let app = build_macos::sdk_bundles::AppIdentity {
            bundle_id: manifest.app.require_bundle_id()?,
            name: &display_name,
            version: &manifest.app.version,
            team_id,
        };
        let entitlements_dir = bin_dir.join(format!("{display_name}.sdk-entitlements"));
        for built in &sdk.bundles {
            nested.push(build_macos::sdk_bundles::assemble(&app_dir, built, &app, &entitlements_dir)?);
        }
    }

    Ok(AssembledApp {
        app_dir,
        inner_binary: dest_binary,
        nested,
    })
}

/// Mirror the app crate's `assets/` into a bundle's `Contents/Resources/`
/// (see [`build_ios::app_assets`]).
fn ship_bundle_assets(project_dir: &Path, resources: &Path) -> Result<()> {
    build_ios::app_assets::stage_app_assets(
        project_dir,
        &resources.join(build_ios::app_assets::ASSETS_DIR),
    )
    .map(|_| ())
}

/// Escape text for inclusion in a plist XML `<string>`. Reason strings
/// are author-supplied, so an unescaped `&` or `<` would corrupt the
/// plist; this keeps the generated file well-formed.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Discover the app's declared capabilities and resolve them to macOS
/// `Info.plist` usage-description `(key, reason)` pairs. Prints warnings +
/// a per-permission report; a discovery error degrades to no entries with
/// a warning rather than failing the run.
fn macos_permission_pairs(
    wrapper_manifest: &Path,
    app_reasons: &std::collections::BTreeMap<String, String>,
) -> Vec<(String, String)> {
    let discovered = match capabilities::discover(wrapper_manifest) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("warning: could not discover app capabilities: {e}");
            return Vec::new();
        }
    };
    if discovered.is_empty() {
        return Vec::new();
    }
    let resolved = capabilities::resolve(&discovered, app_reasons);
    for w in &resolved.warnings {
        eprintln!("warning: {w}");
    }
    for r in &resolved.report {
        println!("  macOS permission: {r}");
    }
    resolved.macos_plist
}

fn render_info_plist(
    display_name: &str,
    app: &build_ios::AppMetadata,
    executable: &str,
    icon_stem: Option<&str>,
    permissions: &[(String, String)],
) -> String {
    let bundle_id = app_bundle_id(app, executable);
    let icon_entry = match icon_stem {
        Some(stem) => format!(
            "    <key>CFBundleIconFile</key>\n    <string>{}</string>\n",
            stem,
        ),
        None => String::new(),
    };
    // Capability usage-description keys, spliced into the dict alongside
    // the icon entry. XML-escaped because reason strings are author text.
    let permission_entries = permissions
        .iter()
        .map(|(k, v)| {
            format!(
                "    <key>{}</key>\n    <string>{}</string>\n",
                xml_escape(k),
                xml_escape(v)
            )
        })
        .collect::<String>();
    // macOS distribution keys. `LSApplicationCategoryType` is required by
    // the Mac App Store (publish errors earlier if it's unset for that
    // path); `LSMinimumSystemVersion` + `NSHumanReadableCopyright` refine
    // the bundle. All read from `[..app.macos]`.
    let category_entry = match &app.macos.category {
        Some(c) => format!(
            "    <key>LSApplicationCategoryType</key>\n    <string>{}</string>\n",
            xml_escape(c),
        ),
        None => String::new(),
    };
    let copyright_entry = match &app.macos.copyright {
        Some(c) => format!(
            "    <key>NSHumanReadableCopyright</key>\n    <string>{}</string>\n",
            xml_escape(c),
        ),
        None => String::new(),
    };
    // Custom URL schemes from `[package.metadata.idealyst.app.links]`.
    let url_types = match app.links.plist_url_types(&bundle_id) {
        t if t.is_empty() => t,
        t => format!("    {t}\n"),
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n\
         {icon_entry}\
         {permission_entries}\
         {category_entry}\
         {copyright_entry}\
         {url_types}\
             <key>CFBundleExecutable</key>\n    <string>{executable}</string>\n\
             <key>CFBundleIdentifier</key>\n    <string>{bundle_id}</string>\n\
             <key>CFBundleName</key>\n    <string>{display_name}</string>\n\
             <key>CFBundleDisplayName</key>\n    <string>{display_name}</string>\n\
             <key>CFBundleShortVersionString</key>\n    <string>{version}</string>\n\
             <key>CFBundleVersion</key>\n    <string>{build_number}</string>\n\
             <key>CFBundlePackageType</key>\n    <string>APPL</string>\n\
             <key>LSMinimumSystemVersion</key>\n    <string>{min_version}</string>\n\
             <key>NSHighResolutionCapable</key>\n    <true/>\n\
         </dict>\n</plist>\n",
        version = app.version,
        build_number = xml_escape(&app.build_number),
        min_version = xml_escape(&app.macos.min_version),
    )
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms)
        .with_context(|| format!("chmod +x {}", path.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    // No-op on non-Unix hosts; .app bundles only matter on macOS,
    // and cross-compiling to macOS from elsewhere isn't in scope.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use build_ios::{AppMetadata, MacosMetadata, SplashConfig};

    fn app_metadata(macos: MacosMetadata) -> AppMetadata {
        AppMetadata {
            name: "Demo".to_string(),
            bundle_id: Some("ai.example.demo".to_string()),
            version: "1.2.3".to_string(),
            build_number: "7".to_string(),
            splash: SplashConfig {
                background: "#000000".to_string(),
                title: "Demo".to_string(),
                title_color: "#ffffff".to_string(),
                duration_ms: 0,
            },
            targets: Vec::new(),
            server_bin: None,
            server_manifest: None,
            server_port: 3000,
            worker_bin: None,
            worker_manifest: None,
            web: Default::default(),
            macos,
            permissions: Default::default(),
            links: Default::default(),
            android: Default::default(),
        }
    }

    /// The Info.plist must carry the macOS distribution keys from
    /// `[..app.macos]` plus `CFBundleVersion` from `build_number` — these
    /// gate Mac App Store acceptance.
    #[test]
    fn info_plist_emits_macos_distribution_keys() {
        let app = app_metadata(MacosMetadata {
            category: Some("public.app-category.productivity".to_string()),
            min_version: "13.0".to_string(),
            copyright: Some("© 2026 Acme".to_string()),
        });
        let plist = render_info_plist("Demo", &app, "demo-macos", None, &[]);
        assert!(plist.contains(
            "<key>LSApplicationCategoryType</key>\n    <string>public.app-category.productivity</string>"
        ));
        assert!(plist.contains("<key>LSMinimumSystemVersion</key>\n    <string>13.0</string>"));
        assert!(plist.contains(
            "<key>NSHumanReadableCopyright</key>\n    <string>© 2026 Acme</string>"
        ));
        assert!(plist.contains("<key>CFBundleVersion</key>\n    <string>7</string>"));
    }

    /// Category/copyright are optional — omit the keys when unset rather than
    /// emitting empty strings.
    /// macOS routes `myapp://` (the kAEGetURL Apple Event) only to a bundle
    /// whose Info.plist declares the scheme.
    #[test]
    fn info_plist_declares_custom_url_schemes() {
        let mut app = app_metadata(MacosMetadata::default());
        assert!(!render_info_plist("Demo", &app, "demo-macos", None, &[]).contains("CFBundleURLTypes"));
        app.links.schemes = vec!["demo".to_string()];
        let plist = render_info_plist("Demo", &app, "demo-macos", None, &[]);
        assert!(plist.contains("<key>CFBundleURLTypes</key>"), "{plist}");
        assert!(plist.contains("<string>demo</string>"));
        assert!(plist.trim_end().ends_with("</plist>"));
    }

    #[test]
    fn info_plist_omits_unset_optional_macos_keys() {
        let app = app_metadata(MacosMetadata::default());
        let plist = render_info_plist("Demo", &app, "demo-macos", None, &[]);
        assert!(!plist.contains("LSApplicationCategoryType"));
        assert!(!plist.contains("NSHumanReadableCopyright"));
        // min_version always present (defaults to 11.0).
        assert!(plist.contains("<key>LSMinimumSystemVersion</key>\n    <string>11.0</string>"));
    }

    /// `asset!("images/logo.png")` on macOS reads
    /// `<App>.app/Contents/Resources/assets/images/logo.png`; the dev path
    /// must produce a bundle (not a bare binary) once `assets/` exists.
    #[test]
    fn macos_app_bundle_ships_assets_in_resources() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("demo");
        fs::create_dir_all(project.join("src")).unwrap();
        fs::write(project.join("src/lib.rs"), "").unwrap();
        fs::write(
            project.join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n\n\
             [package.metadata.idealyst.app]\nname = \"Demo\"\nbundle_id = \"ai.example.demo\"\n",
        )
        .unwrap();
        fs::create_dir_all(project.join("assets/images")).unwrap();
        fs::write(project.join("assets/images/logo.png"), b"png").unwrap();
        let binary = tmp.path().join("target/debug/demo-macos");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, b"bin").unwrap();

        let no_links = build_ios::links::LinksMetadata::default();
        let inner = maybe_wrap_in_app_bundle(&project, &binary, &[], &no_links, &Default::default(), None)
            .unwrap()
            .expect("an assets/ dir must force the .app bundle")
            .inner_binary;
        let app = app_bundle_for(&inner).unwrap();
        assert!(app.join("Contents/Resources/assets/images/logo.png").is_file());
    }

    fn nested(path: &str, entitlements: Option<&str>) -> build_macos::sdk_bundles::AssembledBundle {
        build_macos::sdk_bundles::AssembledBundle {
            path: PathBuf::from(path),
            entitlements: entitlements.map(PathBuf::from),
        }
    }

    fn strings(plan: &[Vec<OsString>]) -> Vec<Vec<String>> {
        plan.iter().map(|a| a.iter().map(|s| s.to_string_lossy().into_owned()).collect()).collect()
    }

    /// An app carrying SDK bundles was signed `--deep`, which re-signs the
    /// nested code with the APP's entitlements and options, dropping each
    /// bundle's own (sandbox + app group, hardened runtime) — the OS then
    /// refuses the extension. Nested bundles must be signed first with their
    /// own entitlements, and the app after them without `--deep`. (The
    /// codesign behaviour itself needs a keychain identity, so this pins the
    /// invocations instead; see `codesign_plan`.)
    #[test]
    fn regression_deep_app_signature_replaced_nested_bundle_entitlements() {
        let plan = codesign_plan(
            Path::new("/b/Demo.app"),
            "ID",
            Some(Path::new("/b/app.entitlements")),
            &[
                nested("/b/Demo.app/Contents/Resources/Demo Audio.driver", None),
                nested("/b/Demo.app/Contents/Library/SystemExtensions/x.systemextension", Some("/b/x.entitlements")),
            ],
            SignFlags { timestamp: false, hardened_runtime: false },
        );
        assert_eq!(
            strings(&plan),
            [
                vec!["--force", "--options", "runtime", "--sign", "ID", "/b/Demo.app/Contents/Resources/Demo Audio.driver"],
                vec![
                    "--force", "--options", "runtime", "--entitlements", "/b/x.entitlements", "--sign", "ID",
                    "/b/Demo.app/Contents/Library/SystemExtensions/x.systemextension",
                ],
                vec!["--force", "--entitlements", "/b/app.entitlements", "--sign", "ID", "/b/Demo.app"],
            ]
        );
    }

    /// Without SDK bundles the dev signature stays the deep one it has
    /// always been.
    #[test]
    fn an_app_without_sdk_bundles_is_signed_deep() {
        let plan = codesign_plan(Path::new("/b/Demo.app"), "ID", None, &[], SignFlags { timestamp: false, hardened_runtime: false });
        assert_eq!(strings(&plan), [vec!["--force", "--deep", "--sign", "ID", "/b/Demo.app"]]);
    }

    /// Distribution signing: a secure timestamp on every signature, the
    /// hardened runtime on the app when asked (Developer ID).
    #[test]
    fn distribution_signing_timestamps_every_signature() {
        let plan = codesign_plan(
            Path::new("/b/Demo.app"),
            "ID",
            Some(Path::new("/b/e.plist")),
            &[nested("/b/Demo.app/Contents/Resources/A.driver", None)],
            SignFlags { timestamp: true, hardened_runtime: true },
        );
        let plan = strings(&plan);
        assert_eq!(plan[0], ["--force", "--timestamp", "--options", "runtime", "--sign", "ID", "/b/Demo.app/Contents/Resources/A.driver"]);
        assert_eq!(
            plan[1],
            ["--force", "--timestamp", "--options", "runtime", "--entitlements", "/b/e.plist", "--sign", "ID", "/b/Demo.app"]
        );
    }

    #[test]
    fn team_id_is_read_from_the_certificate_subject_ou() {
        assert_eq!(
            team_from_subject("subject=UID=T9Z7VF8XNP, CN=Apple Development: Jane (UT9F68R76C), OU=BMVWGH34FS, O=Acme, C=US\n").as_deref(),
            Some("BMVWGH34FS")
        );
        assert_eq!(
            team_from_subject("subject= /UID=T9Z7VF8XNP/CN=Apple Development: Jane (UT9F68R76C)/OU=BMVWGH34FS/O=Acme/C=US").as_deref(),
            Some("BMVWGH34FS")
        );
        assert_eq!(team_from_subject("subject=CN=x"), None);
    }

    #[test]
    fn pem_for_sha1_picks_the_matching_certificate() {
        let listing = "SHA-256 hash: 11\nSHA-1 hash: AAAA\n-----BEGIN CERTIFICATE-----\nfirst\n-----END CERTIFICATE-----\n\
                       SHA-256 hash: 22\nSHA-1 hash: BBBB\n-----BEGIN CERTIFICATE-----\nsecond\n-----END CERTIFICATE-----\n";
        assert_eq!(
            pem_for_sha1(listing, "bbbb").as_deref(),
            Some("-----BEGIN CERTIFICATE-----\nsecond\n-----END CERTIFICATE-----\n")
        );
        assert_eq!(pem_for_sha1(listing, "CCCC"), None);
    }

    /// A project with neither icon, permissions, schemes nor assets — the
    /// bare-binary case — still gets an `.app` once an SDK declares a bundle
    /// (it has nowhere else to live), with the bundle laid out inside.
    #[test]
    fn an_sdk_bundle_forces_the_app_bundle() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("demo");
        fs::create_dir_all(project.join("src")).unwrap();
        fs::write(project.join("src/lib.rs"), "").unwrap();
        fs::write(
            project.join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n\n\
             [package.metadata.idealyst.app]\nname = \"Demo\"\nbundle_id = \"ai.example.demo\"\n",
        )
        .unwrap();
        let binary = tmp.path().join("target/debug/demo-macos");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, b"bin").unwrap();
        let no_links = build_ios::links::LinksMetadata::default();
        assert!(
            maybe_wrap_in_app_bundle(&project, &binary, &[], &no_links, &Default::default(), None).unwrap().is_none(),
            "nothing else here forces a bundle"
        );

        let pkg = tmp.path().join("plugin");
        fs::create_dir_all(pkg.join("bundle")).unwrap();
        fs::write(pkg.join("bundle/Info.plist"), "{{bundle_id}} {{executable}} {{uuid:factory}}").unwrap();
        let built_lib = tmp.path().join("target/debug/libplugin.dylib");
        fs::write(&built_lib, b"dylib").unwrap();
        let sdk = build_macos::sdk_bundles::SdkBundles {
            app_entitlements: Vec::new(),
            bundles: vec![build_macos::sdk_bundles::BuiltBundle {
                decl: build_macos::sdk_bundles::BundleDecl {
                    declarer: "sdk".into(),
                    package: "plugin".into(),
                    manifest_dir: pkg.clone(),
                    kind: build_macos::sdk_bundles::BundleKind::Cdylib { lib_name: "plugin".into() },
                    features: Vec::new(),
                    bundle: "{{app_name}} Audio.driver".into(),
                    place: PathBuf::from("Contents/Resources"),
                    info_plist: pkg.join("bundle/Info.plist"),
                    entitlements: None,
                    bundle_id: "{{bundle_id}}.audio".into(),
                    env: Vec::new(),
                },
                dir_name: "Demo Audio.driver".into(),
                bundle_id: "ai.example.demo.audio".into(),
                executable: built_lib,
            }],
        };
        let app = maybe_wrap_in_app_bundle(&project, &binary, &[], &no_links, &sdk, None)
            .unwrap()
            .expect("an SDK bundle must force the .app bundle");
        let driver = app.app_dir.join("Contents/Resources/Demo Audio.driver");
        assert_eq!(app.nested, [build_macos::sdk_bundles::AssembledBundle { path: driver.clone(), entitlements: None }]);
        assert_eq!(fs::read(driver.join("Contents/MacOS/libplugin.dylib")).unwrap(), b"dylib");
        assert_eq!(
            fs::read_to_string(driver.join("Contents/Info.plist")).unwrap(),
            format!(
                "ai.example.demo.audio libplugin.dylib {}",
                build_macos::sdk_bundles::stable_uuid("ai.example.demo.audio", "factory")
            )
        );

        // Declared app entitlements alone force it too: the dev path applies
        // them by signing the bundle.
        let entitlements_only = build_macos::sdk_bundles::SdkBundles {
            app_entitlements: vec!["com.apple.developer.system-extension.install".into()],
            bundles: Vec::new(),
        };
        assert!(maybe_wrap_in_app_bundle(&project, &binary, &[], &no_links, &entitlements_only, None).unwrap().is_some());
    }

    /// A dev run of an app using an SDK that declares
    /// `com.apple.developer.system-extension.install` was SIGKILLed at
    /// launch: the signature claimed a restricted entitlement with no
    /// provisioning profile behind it.
    #[test]
    fn regression_dev_signature_drops_restricted_entitlements_without_a_profile() {
        let declared = vec![
            "com.apple.developer.system-extension.install".to_string(),
            "com.apple.security.device.audio-input".to_string(),
        ];
        let d = dev_entitlements(&declared, false, Some("TEAM123456"), "ai.example.demo");
        assert_eq!(d.booleans, ["com.apple.security.device.audio-input"]);
        assert_eq!(d.dropped, ["com.apple.developer.system-extension.install"]);
        assert!(d.strings.is_empty());
    }

    /// A dev run signs every wrapped `.app` when a signing identity exists,
    /// and resolving the signature's bundle id used to `require_bundle_id()`
    /// — so an app with no `bundle_id` (which its Info.plist handles with a
    /// `com.example.<exe>` fallback) failed `idealyst run --macos` outright
    /// on any Mac with an Apple Development certificate. Signing resolves
    /// the id exactly as the Info.plist does, and the two agree.
    #[test]
    fn regression_dev_signature_needs_no_bundle_id() {
        let mut app = app_metadata(MacosMetadata::default());
        app.bundle_id = None;
        let declared = vec!["com.apple.developer.system-extension.install".to_string()];
        let d = dev_entitlements_for(&app, "demo-macos", &declared, true, Some("TEAM123456"));
        assert_eq!(
            d.strings[0],
            ("com.apple.application-identifier".to_string(), "TEAM123456.com.example.demo-macos".to_string())
        );
        let plist = render_info_plist("Demo", &app, "demo-macos", None, &[]);
        assert!(
            plist.contains("<key>CFBundleIdentifier</key>\n    <string>com.example.demo-macos</string>"),
            "the signature's id is the bundle's id: {plist}"
        );
    }

    #[test]
    fn dev_signature_with_a_profile_keeps_restricted_entitlements_and_ids() {
        let declared = vec!["com.apple.developer.system-extension.install".to_string()];
        let d = dev_entitlements(&declared, true, Some("TEAM123456"), "ai.example.demo");
        assert_eq!(d.booleans, declared);
        assert!(d.dropped.is_empty());
        assert_eq!(
            d.strings,
            [
                ("com.apple.application-identifier".to_string(), "TEAM123456.ai.example.demo".to_string()),
                ("com.apple.developer.team-identifier".to_string(), "TEAM123456".to_string()),
            ]
        );
    }
}
