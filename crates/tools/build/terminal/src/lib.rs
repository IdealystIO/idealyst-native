//! Terminal-host build orchestration for `idealyst dev --terminal`.
//!
//! Same shape as `build-macos`: generate a tiny binary wrapper that
//! depends on `host-terminal` and either mounts the user's `app()`
//! in-process ([`BuildMode::Local`]) or runs as a thin runtime-server
//! client ([`BuildMode::RuntimeServer`], no user-crate dep — the
//! sidecar runs the user code). Cargo-build the wrapper and return
//! the produced binary's path.
//!
//! Wrapper layout: `<wrapper_root>/<project>/terminal/` or
//! `<wrapper_root>/<project>/terminal-runtime-server/`. The two
//! modes use distinct subdirs so cargo's build cache doesn't get
//! confused by their different feature resolutions of `host-terminal`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use build_ios::{parse_manifest, FrameworkSource, Manifest};

/// Which wrapper to generate. Mirrors [`build_macos::BuildMode`] —
/// `Local` depends on the user crate and mounts `app()` directly,
/// `RuntimeServer` skips the user crate and connects to a dev-host
/// over WebSocket, applying the wire commands the sidecar streams
/// in.
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
    pub release: bool,
    pub mode: BuildMode,
    pub user_features: Vec<String>,
    pub source: FrameworkSource,
}

#[derive(Debug)]
pub struct BuildArtifact {
    pub binary: PathBuf,
    pub wrapper_dir: PathBuf,
}

pub fn build(project_dir: &Path, opts: BuildOptions) -> Result<BuildArtifact> {
    let project_dir = fs::canonicalize(project_dir)
        .with_context(|| format!("resolve project dir {}", project_dir.display()))?;
    let manifest = parse_manifest(&project_dir)?;

    let wrapper_root = opts.source.wrapper_root(&project_dir);
    let subdir = if opts.mode.is_runtime_server() {
        "terminal-runtime-server"
    } else {
        "terminal"
    };
    let wrapper_dir = wrapper_root.join(&manifest.name).join(subdir);
    let cargo_target_dir = opts.source.cargo_target_dir(&project_dir);

    generate_wrapper(&wrapper_dir, &cargo_target_dir, &project_dir, &manifest, &opts)?;
    let extra_features: &[&str] = if opts.mode.is_runtime_server() {
        &["runtime-server"]
    } else {
        &[]
    };
    cargo_build(&wrapper_dir, opts.release, &opts.user_features, extra_features)?;

    let profile = if opts.release { "release" } else { "debug" };
    let bin_name = binary_name(&manifest.name, opts.mode);
    let binary = cargo_target_dir.join(profile).join(&bin_name);
    if !binary.is_file() {
        anyhow::bail!(
            "cargo build reported success but terminal binary not at {}",
            binary.display(),
        );
    }
    Ok(BuildArtifact { binary, wrapper_dir })
}

fn binary_name(project_name: &str, mode: BuildMode) -> String {
    match mode {
        BuildMode::Local => format!("{project_name}-terminal"),
        BuildMode::RuntimeServer => format!("{project_name}-terminal-runtime-server"),
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
    let host_dep = opts.source.dep("crates/host/terminal", &[]);
    let fcore_dep = opts.source.dep("crates/framework/core", &[]);
    let bundle_id = manifest
        .app
        .bundle_id
        .clone()
        .unwrap_or_else(|| format!("com.example.{}", manifest.name));

    let (deps_block, features_block, main_rs) = match opts.mode {
        BuildMode::Local => {
            let user_dep = format!("{{ path = \"{}\" }}", project_dir.display());
            let deps = format!(
                "host-terminal = {host_dep}\n\
                 runtime-core = {fcore_dep}\n\
                 {user_name} = {user_dep}\n",
                user_name = manifest.name,
            );
            let features =
                "[features]\ndev = [\"runtime-core/dev\"]\n".to_string();
            let main = local_main_rs(&manifest.lib_name, &bin_name);
            (deps, features, main)
        }
        BuildMode::RuntimeServer => {
            let deps = format!(
                "host-terminal = {host_dep}\n\
                 runtime-core = {fcore_dep}\n",
            );
            // `runtime-server` toggles host-terminal's runtime-
            // server variant; `dev` enables Robot bridge + MCP
            // catalog. Wrapper-local names so `--features
            // runtime-server,dev` works from cargo.
            let features = "[features]\n\
                runtime-server = [\"host-terminal/runtime-server\"]\n\
                dev = [\"runtime-core/dev\"]\n"
                .to_string();
            let main = runtime_server_main_rs(&bundle_id, &bin_name);
            (deps, features, main)
        }
    };

    let cargo_toml = format!(
        r#"# GENERATED by `idealyst dev --terminal` ({mode}). Do not edit — rewritten every build.
#
# Terminal wrapper. {mode_desc}
# Produces a TTY binary at `<target>/<profile>/{bin_name}`.

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
            "Connects to a dev-host and renders streamed wire commands \
             into a crossterm grid; does NOT depend on the user crate."
        } else {
            "Depends on `host-terminal` + the user crate, mounts \
             `app()` in-process into the crossterm grid."
        },
    );

    write_shared_target_config(wrapper_dir, cargo_target_dir)?;
    fs::write(wrapper_dir.join("Cargo.toml"), cargo_toml)?;
    fs::write(wrapper_dir.join("src/main.rs"), main_rs)?;
    Ok(())
}

fn local_main_rs(user_lib: &str, bin_name: &str) -> String {
    format!(
        r#"//! GENERATED by `idealyst dev --terminal` (local-mount).
//! Mounts the user's `app()` into the terminal grid.

use {user_lib}::app;

fn main() {{
    let opts = host_terminal::RunOptions::default();
    if let Err(e) = host_terminal::run(app, opts) {{
        eprintln!("[{bin_name}] runtime error: {{e}}");
        std::process::exit(1);
    }}
}}
"#,
    )
}

fn runtime_server_main_rs(bundle_id: &str, bin_name: &str) -> String {
    // No user-crate import — the sidecar owns the user app. We
    // ship the bundle id as the mDNS-discovery key the dev-server
    // matches against (same as iOS / Android / macOS / sim).
    format!(
        r#"//! GENERATED by `idealyst dev --terminal --runtime-server`.
//! Connects to an idealyst dev-host and renders streamed wire
//! commands into the terminal. Does NOT depend on the user crate.

fn main() {{
    let opts = host_terminal::RunOptions::default();
    if let Err(e) = host_terminal::run_runtime_server(
        "{bundle_id}".to_string(),
        opts,
    ) {{
        eprintln!("[{bin_name}] runtime error: {{e}}");
        std::process::exit(1);
    }}
}}
"#,
    )
}

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
        "[build-terminal] cargo build{}{} (in {})",
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
        anyhow::bail!("[build-terminal] cargo build exited with {status}");
    }
    Ok(())
}
