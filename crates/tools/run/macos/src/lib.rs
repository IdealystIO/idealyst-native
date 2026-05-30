//! macOS launcher. Builds via `build-macos`, then either spawns the
//! binary as a foreground child (one-shot `idealyst run macos`) or
//! fire-and-forgets it (dev mode, where blocking on the app's
//! lifecycle would tie up the orchestrator's other targets).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result};
use build_ios::{parse_manifest, FrameworkSource};

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
    let spawn_target = match maybe_wrap_in_app_bundle(project_dir, &built.binary)? {
        Some(path) => path,
        None => built.binary.clone(),
    };

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

/// Wrap `binary` in a `<App>.app/Contents/{MacOS,Resources}` bundle
/// with Info.plist + AppIcon.icns. Returns the path to the spawnable
/// binary INSIDE the bundle (which is what gets launched so macOS
/// reads the parent `.app`'s metadata for dock chrome).
///
/// Returns `Ok(None)` when the project has no `[icon]` block — the
/// caller falls back to the bare-binary launch path. Errors out on
/// genuinely broken icon configs (missing source file, invalid
/// gradient stops, etc.) because users typing them want loud
/// feedback, not a silently iconless app.
fn maybe_wrap_in_app_bundle(
    project_dir: &Path,
    binary: &Path,
) -> Result<Option<PathBuf>> {
    let Some(config) = icon_gen::load_config_from_manifest(project_dir)? else {
        return Ok(None);
    };
    let block = config.resolved_for(icon_gen::Target::Macos);
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

    // Generate `.icns` straight into Resources/. icon-gen's cache
    // skips work when the icon source hasn't changed, so this is
    // near-free on subsequent runs.
    let icns_outs = icon_gen::sync_macos_icns(Some(&block), &resources)?;
    let icon_file_stem = match icns_outs {
        Some(outs) => outs
            .icns
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::to_string),
        None => None,
    };

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
    let plist = render_info_plist(&display_name, &manifest.app, &bin_name, icon_file_stem.as_deref());
    fs::write(contents.join("Info.plist"), plist)
        .with_context(|| format!("write {}/Info.plist", contents.display()))?;
    // `PkgInfo` is historical but still required by some tooling
    // (xattr-based icon caches, older LaunchServices).
    fs::write(contents.join("PkgInfo"), b"APPL????")?;

    Ok(Some(dest_binary))
}

fn render_info_plist(
    display_name: &str,
    app: &build_ios::AppMetadata,
    executable: &str,
    icon_stem: Option<&str>,
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
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n\
         {icon_entry}\
             <key>CFBundleExecutable</key>\n    <string>{executable}</string>\n\
             <key>CFBundleIdentifier</key>\n    <string>{bundle_id}</string>\n\
             <key>CFBundleName</key>\n    <string>{display_name}</string>\n\
             <key>CFBundleDisplayName</key>\n    <string>{display_name}</string>\n\
             <key>CFBundleShortVersionString</key>\n    <string>{version}</string>\n\
             <key>CFBundleVersion</key>\n    <string>1</string>\n\
             <key>CFBundlePackageType</key>\n    <string>APPL</string>\n\
             <key>NSHighResolutionCapable</key>\n    <true/>\n\
         </dict>\n</plist>\n",
        version = app.version,
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
