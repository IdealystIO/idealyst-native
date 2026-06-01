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
    /// as `--features <list>`. Used by `idealyst dev` to pass
    /// `runtime-core/dev` so the Robot bridge auto-starts.
    pub user_features: Vec<String>,
    /// Framework-source resolution: workspace path-deps for in-tree
    /// projects, git deps for external installs. Same shape sim uses.
    pub source: FrameworkSource,
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
    cargo_build(&wrapper_dir, opts.release, &opts.user_features, extra_features)?;

    let profile = if opts.release { "release" } else { "debug" };
    let bin_name = binary_name(&manifest.name, opts.mode);
    let binary = cargo_target_dir.join(profile).join(&bin_name);
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
    // `runtime-core` as a direct dep of the wrapper so the dev
    // command can pass `--features runtime-core/dev` from cargo
    // without needing a [features] section. Without this, cargo
    // rejects the spec because runtime-core is only reachable
    // transitively through host-appkit / the user crate.
    let fcore_dep = opts.source.dep("crates/runtime/core", &[]);

    let bundle_id = manifest
        .app
        .bundle_id
        .clone()
        .unwrap_or_else(|| format!("com.example.{}", manifest.name));

    let (deps_block, features_block, main_rs) = match opts.mode {
        BuildMode::Local => {
            let user_dep = format!("{{ path = \"{}\" }}", project_dir.display());
            let deps = format!(
                "host-appkit = {host_dep}\n\
                 runtime-core = {fcore_dep}\n\
                 {user_name} = {user_dep}\n",
                host_dep = host_dep,
                fcore_dep = fcore_dep,
                user_name = manifest.name,
                user_dep = user_dep,
            );
            let features =
                "[features]\ndev = [\"runtime-core/dev\"]\n".to_string();
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
                 runtime-core = {fcore_dep}\n\
                 runtime-server-shell-native = {shell_dep}\n",
                host_dep = host_dep,
                fcore_dep = fcore_dep,
                shell_dep = shell_dep,
            );
            // `aas` toggles the host-appkit runtime-server variant; `dev`
            // additionally enables Robot bridge + MCP catalog.
            let features = "[features]\n\
                aas = [\"host-appkit/runtime-server\"]\n\
                dev = [\"runtime-core/dev\"]\n"
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

fn local_main_rs(
    user_lib: &str,
    app_name: &str,
    bundle_id: &str,
    bin_name: &str,
) -> String {
    format!(
        r#"//! GENERATED by `idealyst build --macos` (local-mount). Wrapper
//! binary for the AppKit-backed native macOS runtime.

use {user_lib}::app;

fn main() {{
    // `--emit-catalog`: dump the MCP catalog JSON to stdout and exit
    // without launching the AppKit host. This is what `idealyst mcp`
    // (with `--from-bin <this-binary>`) spawns to extract the
    // project's catalog. Only available in `dev` builds — the
    // `mcp` feature on `runtime-core` (transitively on via `dev`)
    // is what makes `__mcp::catalog_json()` reachable.
    #[cfg(feature = "dev")]
    {{
        if std::env::args().any(|a| a == "--emit-catalog") {{
            let json = ::runtime_core::__mcp::catalog_json();
            println!("{{}}", ::runtime_core::__serde_json::to_string_pretty(&json).unwrap());
            return;
        }}
    }}

    // Register the project's identity for the Robot bridge's per-
    // process registration file (`~/.idealyst/apps/<name>-<pid>.json`).
    // Tells the MCP server which project this app belongs to without
    // any network discovery. No-op when the `dev` feature is off
    // (bridge not built).
    #[cfg(feature = "dev")]
    {{
        ::runtime_core::robot::bridge::set_app_identity(
            ::runtime_core::robot::bridge::AppIdentity {{
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
    if let Err(e) = host_appkit::run(app, opts) {{
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
) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.args(["build"]).current_dir(wrapper_dir);
    if release {
        cmd.arg("--release");
    }
    let mut combined: Vec<String> = user_features.to_vec();
    combined.extend(extra_features.iter().map(|s| (*s).to_string()));
    if !combined.is_empty() {
        cmd.arg("--features").arg(combined.join(","));
    }
    eprintln!(
        "[build-macos] cargo build{}{} (in {})",
        if release { " --release" } else { "" },
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

#[cfg(test)]
mod regression_tests {
    //! Wrapper-shape regressions for `build-macos`.
    //!
    //! macOS uses a wrapper-local `dev` feature instead of letting
    //! the launcher pass `--features runtime-core/dev` directly —
    //! the cargo `--features dev` invocation from
    //! `cli/cmd/dev.rs::dev_user_features_macos` activates the
    //! wrapper's `dev` feature, which in turn pulls in
    //! `runtime-core/dev`. If the wrapper-side `dev` mapping is
    //! ever dropped, `idealyst mcp` against a running macOS dev
    //! session returns an empty catalog — same end-user symptom
    //! the runtime-server sidecar bug had.
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
                splash: SplashConfig {
                    background: "#000000".to_string(),
                    title: "Demo".to_string(),
                    title_color: "#ffffff".to_string(),
                    duration_ms: 0,
                },
                targets: Vec::new(),
                server_bin: None,
                web: Default::default(),
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
        };
        generate_wrapper(&wrapper_dir, &cargo_target, &project_dir, &manifest, &opts)
            .expect("generate wrapper");
        (wrapper_dir, tmp)
    }

    fn dev_feature_pulls_runtime_core_dev(toml_text: &str) -> bool {
        let parsed: toml::Value = toml::from_str(toml_text).expect("valid TOML");
        let features = match parsed.get("features").and_then(|f| f.as_table()) {
            Some(t) => t,
            None => return false,
        };
        let dev = match features.get("dev").and_then(|d| d.as_array()) {
            Some(a) => a,
            None => return false,
        };
        dev.iter()
            .filter_map(|v| v.as_str())
            .any(|s| s == "runtime-core/dev")
    }

    #[test]
    fn local_wrapper_dev_feature_pulls_runtime_core_dev() {
        let (wrapper_dir, _tmp) = run_generator(BuildMode::Local);
        let cargo = std::fs::read_to_string(wrapper_dir.join("Cargo.toml"))
            .expect("read Cargo.toml");
        assert!(
            dev_feature_pulls_runtime_core_dev(&cargo),
            "local macOS wrapper missing `[features] dev = [..., \"runtime-core/dev\"]`. \
             MCP catalog will be empty in `idealyst dev --macos`. Got:\n{cargo}",
        );
    }

    #[test]
    fn runtime_server_wrapper_dev_feature_pulls_runtime_core_dev() {
        let (wrapper_dir, _tmp) = run_generator(BuildMode::RuntimeServer);
        let cargo = std::fs::read_to_string(wrapper_dir.join("Cargo.toml"))
            .expect("read Cargo.toml");
        assert!(
            dev_feature_pulls_runtime_core_dev(&cargo),
            "runtime-server macOS wrapper missing `[features] dev = [..., \"runtime-core/dev\"]`. \
             MCP catalog will be empty in `idealyst dev --macos --runtime-server`. Got:\n{cargo}",
        );
    }
}
