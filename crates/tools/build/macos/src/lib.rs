//! Build orchestration for `idealyst build --macos`.
//!
//! Generates a tiny binary wrapper at:
//!
//! ```text
//! <workspace>/target/idealyst/<project>/macos/
//! ```
//!
//! The wrapper depends on `host-appkit` + the user's crate, with a
//! `main()` that calls `host_appkit::run(<user>::app, …)`. Builds
//! the wrapper via `cargo build`, returns the produced binary's
//! path.
//!
//! Mirrors `build-sim` for the sim runtime — same template shape,
//! same shared-target-dir trick to avoid recompiling deps per
//! wrapper.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use build_ios::{parse_manifest, FrameworkSource, Manifest};

/// Which wrapper to generate. `Local` builds a binary that depends
/// on the user crate and mounts `app()` in-process via
/// `host_appkit::run`. `Aas` builds a binary that does NOT depend on
/// the user crate — `host_appkit::run_aas` connects to a dev-server
/// over WebSocket and applies the sidecar's command stream. The two
/// modes land in distinct wrapper dirs (`macos/` vs `macos-runtime-server/`)
/// and produce distinct binary names so they coexist on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildMode {
    Local,
    RuntimeServer,
}

impl BuildMode {
    pub fn is_runtime_server(self) -> bool {
        matches!(self, BuildMode::RuntimeServer)
    }
}

#[derive(Clone, Debug)]
pub struct BuildOptions {
    /// Compile with `--release`. Default: debug. Native macOS builds
    /// are usually for dev iteration; release matters for shipping.
    pub release: bool,
    /// Which wrapper template to generate (local-mount vs runtime-server).
    pub mode: BuildMode,
    /// Cargo features to enable on the cargo invocation. Forwarded
    /// as `--features <list>`. Used by `idealyst dev` to pass `dev`
    /// (→ `runtime-core/dev` + `runtime-shared/robot`) so the Robot
    /// bridge auto-starts.
    pub user_features: Vec<String>,
    /// Framework-source resolution: workspace path-deps for in-tree
    /// projects, git deps for external installs. Same shape sim uses.
    pub source: FrameworkSource,
    /// Build a **universal** binary (arm64 + x86_64 lipo'd together) so the
    /// `.app` runs on both Apple Silicon and Intel Macs. Used by `idealyst
    /// publish macos` — the App Store rejects an arm64-only build unless the
    /// deployment target is ≥ 12.0 (error 409). The dev/run path leaves this
    /// `false` and builds the host arch only (one fast compile).
    pub universal: bool,
}

#[derive(Debug)]
pub struct BuildArtifact {
    /// Path to the produced macOS binary (ready to spawn). For now
    /// this is the cargo-emitted binary directly; a future revision
    /// will wrap it in a `.app` bundle.
    pub binary: PathBuf,
    /// Wrapper crate directory. Useful for debugging the template
    /// or for `idealyst scaffold macos` to take ownership later.
    pub wrapper_dir: PathBuf,
}

/// Build the macOS wrapper for `project_dir` with `opts`.
pub fn build(project_dir: &Path, opts: BuildOptions) -> Result<BuildArtifact> {
    let project_dir = fs::canonicalize(project_dir)
        .with_context(|| format!("resolve project dir {}", project_dir.display()))?;
    let manifest = parse_manifest(&project_dir)?;

    let wrapper_root = opts.source.wrapper_root(&project_dir);
    let subdir = if opts.mode.is_runtime_server() { "macos-runtime-server" } else { "macos" };
    let wrapper_dir = wrapper_root.join(&manifest.name).join(subdir);
    let cargo_target_dir = opts.source.cargo_target_dir(&project_dir);

    generate_wrapper(&wrapper_dir, &cargo_target_dir, &project_dir, &manifest, &opts)?;
    let extra_features: &[&str] = if opts.mode.is_runtime_server() {
        // Activate the wrapper crate's `aas` feature, which forwards
        // to `host-appkit/runtime-server` → `backend-macos/runtime-server`.
        // Without this, the wrapper's `main()` calls `run_aas` which
        // doesn't exist in the local-render build.
        &["aas"]
    } else {
        &[]
    };
    let profile = if opts.release { "release" } else { "debug" };
    let bin_name = binary_name(&manifest.name, opts.mode);

    let binary = if opts.universal {
        build_universal(
            &wrapper_dir,
            &cargo_target_dir,
            opts.release,
            &opts.user_features,
            extra_features,
            &bin_name,
            profile,
            &manifest.app.macos.min_version,
        )?
    } else {
        cargo_build(
            &wrapper_dir,
            opts.release,
            &opts.user_features,
            extra_features,
            None,
            None,
        )?;
        cargo_target_dir.join(profile).join(&bin_name)
    };
    if !binary.is_file() {
        anyhow::bail!(
            "cargo build reported success but macOS binary not at {}",
            binary.display(),
        );
    }
    Ok(BuildArtifact {
        binary,
        wrapper_dir,
    })
}

/// Produced-binary name. Suffixed with `-macos` (local-mount) or
/// `-macos-runtime-server` (runtime-server-client) so the two coexist on disk without
/// colliding with each other or the user crate's lib/bin name.
fn binary_name(project_name: &str, mode: BuildMode) -> String {
    match mode {
        BuildMode::Local => format!("{project_name}-macos"),
        BuildMode::RuntimeServer => format!("{project_name}-macos-runtime-server"),
    }
}

fn generate_wrapper(
    wrapper_dir: &Path,
    cargo_target_dir: &Path,
    project_dir: &Path,
    manifest: &Manifest,
    opts: &BuildOptions,
) -> Result<()> {
    fs::create_dir_all(wrapper_dir.join("src"))
        .with_context(|| format!("create {}", wrapper_dir.display()))?;

    let bin_name = binary_name(&manifest.name, opts.mode);

    // `host-appkit` is the only required dep in both modes. runtime-server mode
    // additionally needs the `runtime-server` feature forwarded; we
    // declare a wrapper-local `aas` feature that turns it on.
    let host_dep = opts.source.dep("crates/gpu-backend/host/appkit", &[]);
    // `runtime-core` + `runtime-shared` as DIRECT deps: cargo only
    // resolves a `<dep>/<feat>` spec for a direct dependency of the
    // package being built, and the wrapper's own `dev` feature maps onto
    // them. The facade carries the catalog anchor + emission gate;
    // runtime-shared carries the bridge transport and the substrate
    // names this wrapper spells directly (`robot::bridge`).
    let runtime_core_dep = opts.source.dep("crates/runtime/core", &[]);
    let shared_dep = opts.source.dep("crates/runtime/shared", &[]);

    let bundle_id = manifest
        .app
        .bundle_id
        .clone()
        .unwrap_or_else(|| format!("com.example.{}", manifest.name));

    let (deps_block, features_block, main_rs) = match opts.mode {
        BuildMode::Local => {
            // Plain path dep on the user crate: the app's own defaults
            // select its prim families / feature set, and there is only
            // one core to select.
            let user_dep = format!("{{ path = \"{}\" }}", project_dir.display());
            // `host-appkit/new-core` compiles the boot path
            // (windowed `run_with`) + backend-macos's
            // Host/caps impls and flush driver. The feature is vacuous
            // once backend-macos makes its contents unconditional; drop
            // it from this list at that point.
            let host_dep = opts
                .source
                .dep("crates/gpu-backend/host/appkit", &[]);
            let deps = format!(
                "host-appkit = {host_dep}\n\
                 runtime-core = {runtime_core_dep}\n\
                 runtime-shared = {shared_dep}\n\
                 {user_name} = {user_dep}\n",
                host_dep = host_dep,
                runtime_core_dep = runtime_core_dep,
                shared_dep = shared_dep,
                user_name = manifest.name,
                user_dep = user_dep,
            );
            // `dev` = the catalog + automation surface
            // (`runtime-core/dev` = robot registry + catalog emission
            // gate) plus the bridge TRANSPORT
            // (`runtime-shared/robot`), which is what makes
            // `robot::bridge::set_app_identity` below resolve.
            let features = "[features]\n\
                dev = [\"runtime-core/dev\", \"runtime-shared/robot\"]\n"
                .to_string();
            let main = local_main_rs(
                &manifest.lib_name,
                &manifest.name,
                &bundle_id,
                &bin_name,
            );
            (deps, features, main)
        }
        BuildMode::RuntimeServer => {
            // No dep on the user crate — the sidecar owns it. The
            // wrapper just connects to the dev-server URL (set by
            // the CLI via `IDEALYST_DEV_ENDPOINT`) and applies
            // whatever stream arrives.
            let shell_dep = opts
                .source
                .dep("crates/dev/runtime-server-shell", &["runtime-server"]);
            let deps = format!(
                "host-appkit = {host_dep}\n\
                 runtime-core = {runtime_core_dep}\n\
                 runtime-shared = {shared_dep}\n\
                 runtime-server-shell-native = {shell_dep}\n",
                host_dep = host_dep,
                runtime_core_dep = runtime_core_dep,
                shared_dep = shared_dep,
                shell_dep = shell_dep,
            );
            // `aas` toggles the host-appkit runtime-server variant; `dev`
            // additionally enables Robot bridge + MCP catalog.
            let features = "[features]\n\
                aas = [\"host-appkit/runtime-server\"]\n\
                dev = [\"runtime-core/dev\", \"runtime-shared/robot\"]\n"
                .to_string();
            let main = aas_main_rs(&bundle_id, &manifest.name, &bin_name);
            (deps, features, main)
        }
    };

    let cargo_toml = format!(
        r#"# GENERATED by `idealyst build --macos` ({mode}). Do not edit — rewritten every build.
#
# AppKit wrapper. {mode_desc}
# Produces a desktop binary at `<target>/<profile>/{bin_name}`.

[workspace]

[package]
name = "{bin_name}"
version = "0.0.1"
edition = "2021"

[dependencies]
{deps_block}
{features_block}"#,
        mode = if opts.mode.is_runtime_server() { "runtime-server" } else { "local" },
        mode_desc = if opts.mode.is_runtime_server() {
            "Connects to the dev-server and renders commands from the sidecar; \
             does NOT depend on the user crate."
        } else {
            "Depends on `host-appkit` + the user crate, mounts `app()` in-process."
        },
        bin_name = bin_name,
        deps_block = deps_block,
        features_block = features_block,
    );

    write_shared_target_config(wrapper_dir, cargo_target_dir)?;
    fs::write(wrapper_dir.join("Cargo.toml"), cargo_toml)?;
    fs::write(wrapper_dir.join("src/main.rs"), main_rs)?;
    Ok(())
}

/// Local wrapper `main`: mounts through `host_appkit::newcore::run_with`,
/// which owns the world + scene registry + flush driver + viewport
/// source.
fn local_main_rs(
    user_lib: &str,
    app_name: &str,
    bundle_id: &str,
    bin_name: &str,
) -> String {
    format!(
        r#"//! GENERATED by `idealyst build --macos` (local-mount).
//! Wrapper binary for the AppKit-backed native macOS runtime.

fn main() {{
    // `--emit-catalog`: dump the MCP catalog JSON to stdout and exit
    // without launching the AppKit host. This is what `idealyst mcp`
    // (with `--from-bin <this-binary>`) spawns to extract the project's
    // catalog. Only available in `dev` builds — the `catalog` feature on
    // `runtime-core` (transitively on via `dev`) is what makes
    // `__mcp::catalog_json()` reachable.
    #[cfg(feature = "dev")]
    {{
        if std::env::args().any(|a| a == "--emit-catalog") {{
            let json = ::runtime_core::__mcp::catalog_json();
            println!("{{}}", ::runtime_core::__serde_json::to_string_pretty(&json).unwrap());
            return;
        }}
    }}

    // Register the project's identity for the Robot bridge's per-process
    // registration file (`~/.idealyst/apps/<name>-<pid>.json`). Tells the
    // MCP server which project this app belongs to without any network
    // discovery. No-op when the `dev` feature is off (bridge not built).
    #[cfg(feature = "dev")]
    {{
        ::runtime_shared::robot::bridge::set_app_identity(
            ::runtime_shared::robot::bridge::AppIdentity {{
                name: "{app_name}".to_string(),
                bundle_id: Some("{bundle_id}".to_string()),
                project_root: ::std::option::Option::None,
            }},
        );
    }}

    let opts = host_appkit::RunOptions {{
        title: "{app_name}".to_string(),
        width: 1024.0,
        height: 768.0,
    }};
    // `run_with` (not `run`) so the user crate's
    // `register_scene_extensions` seam runs after `register_builtins` —
    // this is how SDK payload handlers (canvas, codeblock, table, …)
    // register their per-backend implementations. An unregistered payload
    // panics at realize (the scene contract), so the seam is load-bearing.
    if let Err(e) = host_appkit::run_with(
        || {user_lib}::app(),
        opts,
        {user_lib}::register_scene_extensions,
    ) {{
        eprintln!("[{bin_name}] runtime error: {{e}}");
        std::process::exit(1);
    }}
}}
"#,
        user_lib = user_lib,
        app_name = app_name,
        bundle_id = bundle_id,
        bin_name = bin_name,
    )
}

fn aas_main_rs(_bundle_id: &str, app_name: &str, bin_name: &str) -> String {
    // runtime-server wrapper. No user-crate dep — the sidecar runs `app()`
    // remotely and ships commands over WebSocket. The dev-server URL
    // is resolved from the `IDEALYST_DEV_ENDPOINT` env var that
    // `idealyst dev` sets on the spawned macOS child process.
    format!(
        r#"//! GENERATED by `idealyst build --macos --aas` (runtime-server-client).
//! Wrapper binary that runs as a thin client of an runtime-server dev-server;
//! does NOT depend on the user crate.

fn main() {{
    let url = runtime_server_shell_native::endpoint_or_panic();
    let opts = host_appkit::RunOptions {{
        title: "{app_name}".to_string(),
        width: 1024.0,
        height: 768.0,
    }};
    if let Err(e) = host_appkit::run_aas(&url, opts) {{
        eprintln!("[{bin_name}] runtime error: {{e}}");
        std::process::exit(1);
    }}
}}
"#,
        app_name = app_name,
        bin_name = bin_name,
    )
}

/// Redirect the wrapper crate's build output back into the project's
/// (or framework workspace's) shared `target/` so common dependencies
/// aren't recompiled per wrapper.
fn write_shared_target_config(dir: &Path, target_dir: &Path) -> Result<()> {
    let config = format!(
        "# GENERATED. Share the project's `target/` so common\n\
         # dependencies aren't recompiled per-wrapper.\n\
         \n\
         [build]\n\
         target-dir = \"{}\"\n",
        target_dir.display(),
    );
    fs::create_dir_all(dir.join(".cargo"))?;
    fs::write(dir.join(".cargo/config.toml"), config)?;
    Ok(())
}

fn cargo_build(
    wrapper_dir: &Path,
    release: bool,
    user_features: &[String],
    extra_features: &[&str],
    target: Option<&str>,
    deployment_target: Option<&str>,
) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.args(["build"]).current_dir(wrapper_dir);
    if release {
        cmd.arg("--release");
    }
    if let Some(t) = target {
        cmd.args(["--target", t]);
    }
    // Pin the deployment target so BOTH arches report the same minOS as the
    // bundle's `LSMinimumSystemVersion` (x86_64 otherwise defaults to a much
    // older floor than aarch64's 11.0).
    if let Some(dt) = deployment_target {
        cmd.env("MACOSX_DEPLOYMENT_TARGET", dt);
    }
    let mut combined: Vec<String> = user_features.to_vec();
    combined.extend(extra_features.iter().map(|s| (*s).to_string()));
    if !combined.is_empty() {
        cmd.arg("--features").arg(combined.join(","));
    }
    eprintln!(
        "[build-macos] cargo build{}{}{} (in {})",
        if release { " --release" } else { "" },
        target.map(|t| format!(" --target {t}")).unwrap_or_default(),
        if combined.is_empty() {
            String::new()
        } else {
            format!(" --features {}", combined.join(","))
        },
        wrapper_dir.display(),
    );
    let status = cmd
        .status()
        .with_context(|| "spawn `cargo` — is it on your PATH?")?;
    if !status.success() {
        anyhow::bail!("[build-macos] cargo build exited with {status}");
    }
    Ok(())
}

/// The two macOS arches a universal binary spans.
const UNIVERSAL_TARGETS: [&str; 2] = ["aarch64-apple-darwin", "x86_64-apple-darwin"];

/// Build each arch in [`UNIVERSAL_TARGETS`] and `lipo` them into one fat
/// binary, returned. Each arch lands in `target/<triple>/<profile>/`; the
/// merged binary is written to `target/<profile>/<bin>` (the same path a
/// non-universal build would produce, so downstream bundle assembly is
/// unchanged).
#[allow(clippy::too_many_arguments)]
fn build_universal(
    wrapper_dir: &Path,
    cargo_target_dir: &Path,
    release: bool,
    user_features: &[String],
    extra_features: &[&str],
    bin_name: &str,
    profile: &str,
    deployment_target: &str,
) -> Result<PathBuf> {
    let mut arch_binaries = Vec::with_capacity(UNIVERSAL_TARGETS.len());
    for target in UNIVERSAL_TARGETS {
        ensure_rust_target(target);
        cargo_build(
            wrapper_dir,
            release,
            user_features,
            extra_features,
            Some(target),
            Some(deployment_target),
        )?;
        let bin = cargo_target_dir.join(target).join(profile).join(bin_name);
        if !bin.is_file() {
            anyhow::bail!(
                "universal build: {target} slice not found at {}",
                bin.display(),
            );
        }
        arch_binaries.push(bin);
    }

    let universal = cargo_target_dir.join(profile).join(bin_name);
    lipo_create(&arch_binaries, &universal)?;
    Ok(universal)
}

/// Best-effort `rustup target add <target>` so the x86_64 slice can build on
/// an Apple-Silicon host (no-op if already installed, or if `rustup` isn't
/// the toolchain manager — the subsequent `cargo build --target` then
/// surfaces a clear "can't find crate for `std`" if the target is missing).
fn ensure_rust_target(target: &str) {
    let _ = Command::new("rustup")
        .args(["target", "add", target])
        .status();
}

/// `lipo -create <slices…> -output <universal>`.
fn lipo_create(slices: &[PathBuf], output: &Path) -> Result<()> {
    eprintln!(
        "[build-macos] lipo -create → {} (universal arm64 + x86_64)",
        output.display(),
    );
    let mut cmd = Command::new("lipo");
    cmd.arg("-create");
    for s in slices {
        cmd.arg(s);
    }
    cmd.arg("-output").arg(output);
    let status = cmd
        .status()
        .with_context(|| "spawn `lipo` — part of the Xcode command-line tools")?;
    if !status.success() {
        anyhow::bail!("[build-macos] lipo failed (exit {status})");
    }
    Ok(())
}

#[cfg(test)]
mod regression_tests {
    //! Wrapper-shape regressions for `build-macos`.
    //!
    //! macOS uses a wrapper-local `dev` feature: the cargo
    //! `--features dev` invocation from
    //! `cli/cmd/dev.rs::dev_user_features_macos` activates it, and it in
    //! turn pulls in `runtime-core/dev` (catalog + robot registry) and
    //! `runtime-shared/robot` (the bridge transport). If either half of
    //! that mapping is dropped, `idealyst mcp` against a running macOS
    //! dev session returns an empty catalog — same end-user symptom the
    //! runtime-server sidecar bug had.
    //!
    //! These tests don't fire `cargo build`; they only run the
    //! wrapper-generation step (sub-millisecond) and assert on the
    //! produced Cargo.toml.

    use super::*;
    use build_ios::{AppMetadata, Manifest, SplashConfig};

    fn fake_manifest() -> Manifest {
        Manifest {
            name: "demo".to_string(),
            lib_name: "demo".to_string(),
            app: AppMetadata {
                name: "Demo".to_string(),
                bundle_id: Some("ai.example.demo".to_string()),
                version: "0.0.1".to_string(),
                build_number: "1".to_string(),
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
                macos: Default::default(),
                permissions: Default::default(),
            },
        }
    }

    fn run_generator(mode: BuildMode) -> (std::path::PathBuf, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project_dir = tmp.path().join("project");
        let wrapper_dir = tmp.path().join("wrapper");
        let cargo_target = tmp.path().join("target");
        let workspace_root = tmp.path().join("workspace");
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::create_dir_all(&workspace_root).unwrap();
        let manifest = fake_manifest();
        let opts = BuildOptions {
            release: false,
            mode,
            source: FrameworkSource::Workspace { root: workspace_root },
            user_features: Vec::new(),
            universal: false,
        };
        generate_wrapper(&wrapper_dir, &cargo_target, &project_dir, &manifest, &opts)
            .expect("generate wrapper");
        (wrapper_dir, tmp)
    }

    /// The local wrapper boots through `host_appkit::run_with`,
    /// enables `host-appkit/new-core`, takes a plain path dep on the user
    /// crate (no core pin — there is one core), and keeps the dev
    /// preamble (`--emit-catalog` + Robot identity).
    #[test]
    fn local_wrapper_boots_newcore_run_with_plain_user_dep() {
        let (wrapper_dir, _tmp) = run_generator(BuildMode::Local);
        let cargo = std::fs::read_to_string(wrapper_dir.join("Cargo.toml")).unwrap();
        let main_rs = std::fs::read_to_string(wrapper_dir.join("src/main.rs")).unwrap();
        assert!(
            !cargo.contains("old-core"),
            "wrapper must not pin any core feature on the user crate:\n{cargo}",
        );
        assert!(
            !cargo.contains("default-features = false"),
            "user-crate dep must be a plain path dep so the app's own \
             defaults apply:\n{cargo}",
        );
        assert!(
            cargo.contains("host-appkit"),
            "wrapper must depend on host-appkit:\n{cargo}",
        );
        assert!(
            main_rs.contains("host_appkit::run_with("),
            "main must boot through host_appkit::run_with:\n{main_rs}",
        );
        assert!(
            main_rs.contains("register_scene_extensions"),
            "main must register through the scene seam:\n{main_rs}",
        );
        assert!(dev_feature_pulls_dev_surface(&cargo));
        assert!(main_rs.contains("--emit-catalog"));
    }

    fn dev_feature_pulls_dev_surface(toml_text: &str) -> bool {
        let parsed: toml::Value = toml::from_str(toml_text).expect("valid TOML");
        let features = match parsed.get("features").and_then(|f| f.as_table()) {
            Some(t) => t,
            None => return false,
        };
        let dev = match features.get("dev").and_then(|d| d.as_array()) {
            Some(a) => a,
            None => return false,
        };
        let names: Vec<&str> = dev.iter().filter_map(|v| v.as_str()).collect();
        names.contains(&"runtime-core/dev") && names.contains(&"runtime-shared/robot")
    }

    #[test]
    fn local_wrapper_dev_feature_pulls_dev_surface() {
        let (wrapper_dir, _tmp) = run_generator(BuildMode::Local);
        let cargo = std::fs::read_to_string(wrapper_dir.join("Cargo.toml"))
            .expect("read Cargo.toml");
        assert!(
            dev_feature_pulls_dev_surface(&cargo),
            "local macOS wrapper missing `[features] dev = [\"runtime-core/dev\", \"runtime-shared/robot\"]`. \
             MCP catalog will be empty in `idealyst dev --macos`. Got:\n{cargo}",
        );
    }

    #[test]
    fn runtime_server_wrapper_dev_feature_pulls_dev_surface() {
        let (wrapper_dir, _tmp) = run_generator(BuildMode::RuntimeServer);
        let cargo = std::fs::read_to_string(wrapper_dir.join("Cargo.toml"))
            .expect("read Cargo.toml");
        assert!(
            dev_feature_pulls_dev_surface(&cargo),
            "runtime-server macOS wrapper missing `[features] dev = [\"runtime-core/dev\", \"runtime-shared/robot\"]`. \
             MCP catalog will be empty in `idealyst dev --macos --runtime-server`. Got:\n{cargo}",
        );
    }
}
