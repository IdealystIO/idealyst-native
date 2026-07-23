//! Build orchestration for `idealyst build --windows`.
//!
//! Generates a tiny binary wrapper at:
//!
//! ```text
//! <workspace>/target/idealyst/<project>/windows/
//! ```
//!
//! The wrapper depends on `host-win32` + the user's crate, with a
//! `main()` that calls `host_win32::run_with(opts, register_extensions,
//! app)`. Builds the wrapper via `cargo build`, returns the produced
//! `.exe`'s path.
//!
//! Mirrors `build-macos` but simpler: Windows has no `.app`-bundle /
//! codesign step, and no universal-binary lipo — a native `.exe`
//! launches directly. The generated binary uses the console subsystem
//! so framework logs surface in the terminal `idealyst run` was
//! launched from.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use build_ios::{parse_manifest, FrameworkSource, Manifest};

#[derive(Clone, Debug)]
pub struct BuildOptions {
    /// Compile with `--release`. Default: debug.
    pub release: bool,
    /// Cargo features to enable (e.g. `runtime-core/dev` from `idealyst
    /// dev` so the Robot bridge auto-starts). Forwarded as `--features`.
    pub user_features: Vec<String>,
    /// Framework-source resolution: workspace path-deps in-tree, git
    /// deps for external installs.
    pub source: FrameworkSource,
}

#[derive(Debug)]
pub struct BuildArtifact {
    /// Path to the produced Windows `.exe` (ready to spawn).
    pub binary: PathBuf,
    /// Wrapper crate directory (useful for debugging the template).
    pub wrapper_dir: PathBuf,
}

/// Build the Windows wrapper for `project_dir` with `opts`.
pub fn build(project_dir: &Path, opts: BuildOptions) -> Result<BuildArtifact> {
    // Absolutize WITHOUT any volume access: both `fs::canonicalize` and
    // `std::path::absolute` issue a volume/final-path query that fails
    // with "the volume does not contain a recognized file system" (os
    // error 1005) on this setup's `Z:` VM share (a virtio-fs/9p mount
    // where `GetFinalPathNameByHandleW` isn't supported). `current_dir`
    // + lexical `join` never opens the volume — all we need for a
    // stable path-dep in the generated wrapper.
    let project_dir = if project_dir.is_absolute() {
        project_dir.to_path_buf()
    } else {
        std::env::current_dir()
            .with_context(|| "read current dir to absolutize the project path")?
            .join(project_dir)
    };
    let manifest = parse_manifest(&project_dir)?;

    let wrapper_root = opts.source.wrapper_root(&project_dir);
    let wrapper_dir = wrapper_root.join(&manifest.name).join("windows");
    let cargo_target_dir = opts.source.cargo_target_dir(&project_dir);

    generate_wrapper(&wrapper_dir, &cargo_target_dir, &project_dir, &manifest, &opts)?;

    let profile = if opts.release { "release" } else { "debug" };
    let bin_name = binary_name(&manifest.name);
    cargo_build(&wrapper_dir, opts.release, &opts.user_features)?;

    // cargo emits `<bin_name>.exe` on the windows target.
    let binary = cargo_target_dir.join(profile).join(format!("{bin_name}.exe"));
    if !binary.is_file() {
        anyhow::bail!(
            "cargo build reported success but Windows binary not at {}",
            binary.display(),
        );
    }
    Ok(BuildArtifact { binary, wrapper_dir })
}

/// Produced-binary name, suffixed `-windows` so it can't collide with
/// the user crate's lib/bin name or the other platforms' wrappers.
fn binary_name(project_name: &str) -> String {
    format!("{project_name}-windows")
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

    let bin_name = binary_name(&manifest.name);
    let host_dep = opts.source.dep("crates/host/win32", &[]);
    // `runtime-core` as a direct dep so `idealyst dev` can pass
    // `--features runtime-core/dev` (via the wrapper's `dev` feature).
    let fcore_dep = opts.source.dep("crates/runtime/core", &[]);
    let user_dep = format!("{{ path = \"{}\" }}", project_dir.display());

    let bundle_id = manifest
        .app
        .bundle_id
        .clone()
        .unwrap_or_else(|| format!("com.example.{}", manifest.name));

    let deps_block = format!(
        "host-win32 = {host_dep}\n\
         runtime-core = {fcore_dep}\n\
         {user_name} = {user_dep}\n",
        user_name = manifest.name,
    );
    // Path deps carry absolute Windows paths (`z:\…`). TOML basic
    // strings treat `\` as an escape, so a raw Windows path is invalid
    // TOML — normalize every separator to `/` (Cargo accepts forward
    // slashes in `path = "…"` on Windows). The block is only path deps
    // + crate names (no legitimate backslashes), so a blanket replace
    // is safe.
    let deps_block = deps_block.replace('\\', "/");

    let cargo_toml = format!(
        r#"# GENERATED by `idealyst build --windows`. Do not edit — rewritten every build.
#
# Win32 wrapper. Depends on `host-win32` + the user crate, mounts
# `app()` in-process. Produces `<target>/<profile>/{bin_name}.exe`.

[workspace]

[package]
name = "{bin_name}"
version = "0.0.1"
edition = "2021"

[dependencies]
{deps_block}
[features]
dev = ["runtime-core/dev"]
"#,
    );

    let main_rs = main_rs(&manifest.lib_name, &manifest.app.name, &bundle_id, &bin_name);

    write_shared_target_config(wrapper_dir, cargo_target_dir)?;
    fs::write(wrapper_dir.join("Cargo.toml"), cargo_toml)?;
    fs::write(wrapper_dir.join("src/main.rs"), main_rs)?;
    Ok(())
}

fn main_rs(user_lib: &str, app_name: &str, bundle_id: &str, bin_name: &str) -> String {
    format!(
        r#"//! GENERATED by `idealyst build --windows`. Wrapper binary for
//! the Win32-backed native Windows runtime.

use {user_lib}::app;

fn main() {{
    // `--emit-catalog`: dump the MCP catalog JSON and exit without
    // opening a window. `idealyst mcp --from-bin <this-exe>` spawns
    // this to extract the project's catalog. Only in `dev` builds.
    #[cfg(feature = "dev")]
    {{
        if std::env::args().any(|a| a == "--emit-catalog") {{
            let json = ::runtime_core::__mcp::catalog_json();
            println!("{{}}", ::runtime_core::__serde_json::to_string_pretty(&json).unwrap());
            return;
        }}
        ::runtime_core::robot::bridge::set_app_identity(
            ::runtime_core::robot::bridge::AppIdentity {{
                name: "{app_name}".to_string(),
                bundle_id: Some("{bundle_id}".to_string()),
                project_root: ::std::option::Option::None,
            }},
        );
    }}

    let opts = host_win32::RunOptions {{
        title: "{app_name}".to_string(),
        width: 1024,
        height: 768,
    }};
    // `run_with` (not `run`) so the user crate's `register_extensions`
    // runs first — that's how SDK `Element::External` handlers register
    // per-backend. `run_with` returns the process exit code.
    std::process::exit(host_win32::run_with(opts, {user_lib}::register_extensions, app));
}}
"#,
    )
}

/// Redirect the wrapper crate's build output into the project's shared
/// `target/` so common dependencies aren't recompiled per wrapper.
fn write_shared_target_config(dir: &Path, target_dir: &Path) -> Result<()> {
    let config = format!(
        "# GENERATED. Share the project's `target/` so common\n\
         # dependencies aren't recompiled per-wrapper.\n\
         \n\
         [build]\n\
         target-dir = \"{}\"\n",
        // Cargo config paths use forward slashes even on Windows;
        // escape backslashes so a Windows path stays valid TOML.
        target_dir.display().to_string().replace('\\', "/"),
    );
    fs::create_dir_all(dir.join(".cargo"))?;
    fs::write(dir.join(".cargo/config.toml"), config)?;
    Ok(())
}

fn cargo_build(wrapper_dir: &Path, release: bool, user_features: &[String]) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.args(["build"]).current_dir(wrapper_dir);
    if release {
        cmd.arg("--release");
    }
    if !user_features.is_empty() {
        cmd.arg("--features").arg(user_features.join(","));
    }
    eprintln!(
        "[build-windows] cargo build{}{} (in {})",
        if release { " --release" } else { "" },
        if user_features.is_empty() {
            String::new()
        } else {
            format!(" --features {}", user_features.join(","))
        },
        wrapper_dir.display(),
    );
    let status = cmd
        .status()
        .with_context(|| "spawn `cargo` — is it on your PATH?")?;
    if !status.success() {
        anyhow::bail!("[build-windows] cargo build exited with {status}");
    }
    Ok(())
}
