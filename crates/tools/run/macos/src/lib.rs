//! macOS launcher. Builds via `build-macos`, then either spawns the
//! binary as a foreground child (one-shot `idealyst run macos`) or
//! fire-and-forgets it (dev mode, where blocking on the app's
//! lifecycle would tie up the orchestrator's other targets).

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
    /// If true, spawn the binary detached (stdio nulled, parent
    /// returns immediately). Used by `idealyst dev` so the macOS
    /// app's lifetime is decoupled from the CLI's. One-shot
    /// `idealyst run macos` leaves this false — the user there
    /// expects a foreground process they can Ctrl-C.
    pub background: bool,
    /// Cargo features to enable on the build. `idealyst dev` passes
    /// `runtime-core/dev` here so the Robot bridge auto-starts.
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
    /// Pre-fix the caller never got a handle to the detached binary
    /// and the dev orchestrator's Ctrl-C handler couldn't kill it,
    /// so every `idealyst dev --macos` session leaked one
    /// `nicho-portfolio-macos[-aas]` process per invocation. The
    /// caller (`cli/cmd/dev.rs::launch_macos`) now pushes this into
    /// the shared `children` Vec so the SIGINT handler reaches it.
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

    let spawn_target = match maybe_wrap_in_app_bundle(project_dir, &built.binary, &permission_pairs)? {
        Some(path) => path,
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
    if let Some(app_bundle) = app_bundle_for(&spawn_target) {
        match resolve_macos_signing_identity() {
            Some(identity) => codesign_bundle(&app_bundle, &identity)?,
            None => eprintln!(
                "[run-macos] no Apple Development signing identity found — \
                 leaving the linker ad-hoc signature. macOS will re-prompt for \
                 Screen Recording / Camera permission on every rebuild. Install \
                 an Apple Development certificate (or set DEVELOPMENT_TEAM) to \
                 make grants stick."
            ),
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
        // surface (pre-fix both were `Stdio::null()`, which made
        // "nothing renders" debugging impossible — the binary
        // would crash or log silently). Leave the child unwaited
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

/// `codesign --force --deep --sign <identity> <app>`. Deep-signs the bundle and
/// its nested binary so the launched executable carries the stable cert
/// identity TCC keys grants on. No hardened runtime / entitlements — this is a
/// local dev signature, not a distribution one.
fn codesign_bundle(app: &Path, identity: &str) -> Result<()> {
    eprintln!(
        "[run-macos] codesign --force --deep --sign {} {}",
        &identity[..identity.len().min(10)],
        app.display()
    );
    let status = Command::new("codesign")
        .args(["--force", "--deep", "--sign", identity])
        .arg(app)
        .status()
        .context("invoke codesign")?;
    if !status.success() {
        anyhow::bail!("codesign failed for {} (status {status})", app.display());
    }
    Ok(())
}

/// Wrap `binary` in a `<App>.app/Contents/{MacOS,Resources}` bundle
/// with Info.plist + AppIcon.icns. Returns the path to the spawnable
/// binary INSIDE the bundle (which is what gets launched so macOS
/// reads the parent `.app`'s metadata for dock chrome).
///
/// Returns `Ok(None)` when the project has neither an `[icon]` block nor
/// any capability-derived permission keys — the caller falls back to the
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
) -> Result<Option<PathBuf>> {
    let config = icon_gen::load_config_from_manifest(project_dir)?;
    if config.is_none() && permissions.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        assemble_app_bundle(project_dir, binary, permissions)?.inner_binary,
    ))
}

/// A laid-out `.app` bundle.
pub(crate) struct AssembledApp {
    /// The `<App>.app` directory (what gets signed / packaged).
    pub app_dir: PathBuf,
    /// The executable inside `Contents/MacOS/` (what gets launched).
    pub inner_binary: PathBuf,
}

/// Assemble `<App>.app/Contents/{MacOS,Resources}` around `binary` with an
/// Info.plist + (when declared) AppIcon.icns. ALWAYS wraps — used by the
/// publish path, which needs a guaranteed bundle. The dev/run path goes
/// through [`maybe_wrap_in_app_bundle`], which skips wrapping when there's
/// neither an icon nor a permission to carry. Errors out on a broken icon
/// config (missing source, invalid gradient) rather than shipping a
/// silently iconless app.
pub(crate) fn assemble_app_bundle(
    project_dir: &Path,
    binary: &Path,
    permissions: &[(String, String)],
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

    Ok(AssembledApp {
        app_dir,
        inner_binary: dest_binary,
    })
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
    let bundle_id = app
        .bundle_id
        .clone()
        .unwrap_or_else(|| format!("com.example.{}", executable));
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
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n\
         {icon_entry}\
         {permission_entries}\
         {category_entry}\
         {copyright_entry}\
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
            web: Default::default(),
            macos,
            permissions: Default::default(),
        }
    }

    /// The Info.plist must carry the macOS distribution keys from
    /// `[..app.macos]` plus `CFBundleVersion` from `build_number` (it used to
    /// be hardcoded `1`) — these gate Mac App Store acceptance.
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
    #[test]
    fn info_plist_omits_unset_optional_macos_keys() {
        let app = app_metadata(MacosMetadata::default());
        let plist = render_info_plist("Demo", &app, "demo-macos", None, &[]);
        assert!(!plist.contains("LSApplicationCategoryType"));
        assert!(!plist.contains("NSHumanReadableCopyright"));
        // min_version always present (defaults to 11.0).
        assert!(plist.contains("<key>LSMinimumSystemVersion</key>\n    <string>11.0</string>"));
    }
}
