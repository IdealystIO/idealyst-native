//! Sim runtime build orchestration for `idealyst run sim`.
//!
//! The "sim" target is the wgpu-backed desktop preview runtime —
//! `variant-phone` / `variant-tablet` / `variant-tv` driving a winit
//! window through `render-wgpu`, with one of the platform skins
//! (`ios-sim`, `android-sim`) painting the chrome around the user's
//! tree. It is **not** a native macOS / Windows / Linux backend —
//! those would use the OS's widget toolkit, not custom-drawn wgpu.
//!
//! This crate templates a tiny binary wrapper under
//! `<wrapper_root>/<project>/sim/` that depends on the chosen
//! form-factor crate + skin + the user crate, then `cargo build`s
//! it. The produced binary opens a window and renders the user's
//! `app()` tree.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use build_ios::{parse_manifest, FrameworkSource, Manifest};

/// Window-size variant. Each picks the matching `native-*` crate
/// (which fixes the logical-px window dimensions + title).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormFactor {
    Phone,
    Tablet,
    Tv,
}

impl FormFactor {
    pub fn as_str(self) -> &'static str {
        match self {
            FormFactor::Phone => "phone",
            FormFactor::Tablet => "tablet",
            FormFactor::Tv => "tv",
        }
    }
}

/// Painter choice — which `render_wgpu::Painter` paints the platform
/// chrome (status bar, gesture indicators, system fonts) around
/// the user's tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PainterChoice {
    Ios,
    Android,
}

impl PainterChoice {
    pub fn as_str(self) -> &'static str {
        match self {
            PainterChoice::Ios => "ios",
            PainterChoice::Android => "android",
        }
    }
}

/// Which wrapper to generate. `Local` builds a binary that
/// depends on the user crate and mounts `app()` in-process via
/// `native_<form>::run(skin, app)`. `RuntimeServer` builds a
/// binary that does NOT depend on the user crate — instead it
/// calls `native_<form>::run_runtime_server(skin, app_id)` which
/// connects to an idealyst dev-host over WebSocket and renders
/// the streamed wire commands. The two modes land in distinct
/// wrapper dirs (`sim/` vs `sim-runtime-server/`) so cargo's
/// build cache doesn't get confused by their different feature
/// resolutions of `host-winit` / `variant-phone`.
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
    /// Compile with `--release`. Default: debug. The sim is a dev
    /// preview — release is rarely worth the slower rebuild.
    pub release: bool,
    /// Window form factor (drives crate selection + window size).
    pub form: FormFactor,
    /// Painter painting the platform chrome.
    pub skin: PainterChoice,
    /// Which wrapper template to generate (local-mount vs
    /// runtime-server-client).
    pub mode: BuildMode,
    /// Framework-source resolution: workspace path-deps for in-tree
    /// projects, git deps for external installs.
    pub source: FrameworkSource,
}

#[derive(Debug)]
pub struct BuildArtifact {
    /// Path to the produced sim binary (ready to spawn).
    pub binary: PathBuf,
    /// Wrapper crate directory. Useful for debugging the template.
    pub wrapper_dir: PathBuf,
}

/// Build the sim wrapper for `project_dir` with `opts`.
pub fn build(project_dir: &Path, opts: BuildOptions) -> Result<BuildArtifact> {
    let project_dir = fs::canonicalize(project_dir)
        .with_context(|| format!("resolve project dir {}", project_dir.display()))?;
    let manifest = parse_manifest(&project_dir)?;

    let wrapper_root = opts.source.wrapper_root(&project_dir);
    let subdir = if opts.mode.is_runtime_server() {
        "sim-runtime-server"
    } else {
        "sim"
    };
    let wrapper_dir = wrapper_root.join(&manifest.name).join(subdir);
    let cargo_target_dir = opts.source.cargo_target_dir(&project_dir);

    generate_wrapper(&wrapper_dir, &cargo_target_dir, &project_dir, &manifest, &opts)?;
    cargo_build(&wrapper_dir, opts.release)?;

    let profile = if opts.release { "release" } else { "debug" };
    let bin_name = binary_name(&manifest.name, opts.mode);
    let binary = cargo_target_dir.join(profile).join(&bin_name);
    if !binary.is_file() {
        anyhow::bail!(
            "cargo build reported success but sim binary not at {}",
            binary.display(),
        );
    }
    Ok(BuildArtifact { binary, wrapper_dir })
}

/// Produced-binary name for a given project. Suffixed with `-sim`
/// (local-mount) or `-sim-runtime-server` (runtime-server-client)
/// so the two modes coexist on disk without colliding with the
/// project crate's own bin/lib name.
fn binary_name(project_name: &str, mode: BuildMode) -> String {
    match mode {
        BuildMode::Local => format!("{project_name}-sim"),
        BuildMode::RuntimeServer => format!("{project_name}-sim-runtime-server"),
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

    // Pick the form-factor crate (window size + title) and the skin
    // crate (chrome painter). Both come from the framework source so
    // workspace + git installs work identically.
    let (form_crate, form_subpath, form_ident) = match opts.form {
        FormFactor::Phone => ("variant-phone", "crates/gpu-backend/variant/phone", "variant_phone"),
        FormFactor::Tablet => ("variant-tablet", "crates/gpu-backend/variant/tablet", "variant_tablet"),
        FormFactor::Tv => ("variant-tv", "crates/gpu-backend/variant/tv", "variant_tv"),
    };
    let (skin_crate, skin_subpath, skin_ident, skin_type) = match opts.skin {
        PainterChoice::Ios => (
            "ios-sim",
            "crates/gpu-backend/painter/ios-sim",
            "ios_sim",
            "IosSim",
        ),
        PainterChoice::Android => (
            "android-sim",
            "crates/gpu-backend/painter/android-sim",
            "android_sim",
            "AndroidSim",
        ),
    };

    // Runtime-server mode opts the form-factor crate into its
    // `runtime-server` feature so `run_runtime_server` is in scope.
    // Local mode keeps zero transport overhead.
    let form_features: &[&str] = if opts.mode.is_runtime_server() {
        &["runtime-server"]
    } else {
        &[]
    };
    let form_dep = opts.source.dep(form_subpath, form_features);
    let skin_dep = opts.source.dep(skin_subpath, &[]);
    // Sanity check; the value isn't otherwise plumbed.
    let _bundle_id = manifest
        .app
        .bundle_id
        .clone()
        .unwrap_or_else(|| format!("com.example.{}", manifest.name));
    // Sim runtime-server wrapper needs the shell crate so its `main()`
    // can resolve the dev-server URL via `endpoint_or_panic()`.
    let shell_dep = opts
        .source
        .dep("crates/dev/runtime-server-shell", &["runtime-server"]);

    let (cargo_toml, main_rs) = match opts.mode {
        BuildMode::Local => {
            let user_dep = format!("{{ path = \"{}\" }}", project_dir.display());
            let cargo = format!(
                r#"# GENERATED by `idealyst run sim` (local-mount). Do not edit — rewritten every build.
#
# Wgpu sim wrapper: depends on the form-factor crate + skin + the
# user crate, exposes a `main()` that calls
# `native_<form>::run(skin, app)`. Produces a desktop binary at
# `<target>/<profile>/{bin_name}`.

[workspace]

[package]
name = "{bin_name}"
version = "0.0.1"
edition = "2021"

[dependencies]
{form_crate} = {form_dep}
{skin_crate} = {skin_dep}
{user_name} = {user_dep}
"#,
                form_crate = form_crate,
                skin_crate = skin_crate,
                bin_name = bin_name,
                form_dep = form_dep,
                skin_dep = skin_dep,
                user_name = manifest.name,
                user_dep = user_dep,
            );
            let main = format!(
                r#"//! GENERATED by `idealyst run sim` (local-mount). Wrapper binary
//! for the wgpu sim runtime ({form_label}, {skin_label} skin).

use std::rc::Rc;
use {user_lib}::app;

fn main() {{
    let skin = Rc::new({skin_ident}::{skin_type}::new());
    if let Err(e) = {form_ident}::run(skin, app) {{
        eprintln!("[{bin_name}] runtime error: {{e:?}}");
        std::process::exit(1);
    }}
}}
"#,
                user_lib = manifest.lib_name,
                form_label = opts.form.as_str(),
                skin_label = opts.skin.as_str(),
                skin_ident = skin_ident,
                skin_type = skin_type,
                form_ident = form_ident,
                bin_name = bin_name,
            );
            (cargo, main)
        }
        BuildMode::RuntimeServer => {
            // No user-crate dep — the sidecar owns the user's
            // app and ships wire commands over WebSocket.
            let cargo = format!(
                r#"# GENERATED by `idealyst run sim --runtime-server`. Do not edit — rewritten every build.
#
# Wgpu sim runtime-server wrapper: depends on the form-factor
# crate (with the `runtime-server` feature on, which pulls in
# `host-winit/runtime-server` → `runtime-server-shell-native`) +
# the skin. Does NOT depend on the user crate — the sidecar
# runs it and streams wire commands over WebSocket. Produces a
# desktop binary at `<target>/<profile>/{bin_name}`.

[workspace]

[package]
name = "{bin_name}"
version = "0.0.1"
edition = "2021"

[dependencies]
{form_crate} = {form_dep}
{skin_crate} = {skin_dep}
runtime-server-shell-native = {shell_dep}
"#,
                form_crate = form_crate,
                skin_crate = skin_crate,
                bin_name = bin_name,
                form_dep = form_dep,
                skin_dep = skin_dep,
                shell_dep = shell_dep,
            );
            let main = format!(
                r#"//! GENERATED by `idealyst run sim --runtime-server`. Wrapper
//! binary that connects to an idealyst dev-host as a thin client;
//! does NOT depend on the user crate.

use std::rc::Rc;

fn main() {{
    let url = runtime_server_shell_native::endpoint_or_panic();
    let skin = Rc::new({skin_ident}::{skin_type}::new());
    if let Err(e) = {form_ident}::run_runtime_server(skin, url) {{
        eprintln!("[{bin_name}] runtime error: {{e:?}}");
        std::process::exit(1);
    }}
}}
"#,
                skin_ident = skin_ident,
                skin_type = skin_type,
                form_ident = form_ident,
                bin_name = bin_name,
            );
            (cargo, main)
        }
    };

    write_shared_target_config(wrapper_dir, cargo_target_dir)?;
    fs::write(wrapper_dir.join("Cargo.toml"), cargo_toml)?;
    fs::write(wrapper_dir.join("src/main.rs"), main_rs)?;
    Ok(())
}

/// Redirect the wrapper crate's build output back into the
/// project's (or framework workspace's) shared `target/` so common
/// dependencies aren't recompiled per wrapper.
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

fn cargo_build(wrapper_dir: &Path, release: bool) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.args(["build"]).current_dir(wrapper_dir);
    if release {
        cmd.arg("--release");
    }
    eprintln!(
        "[build-sim] cargo build{} (in {})",
        if release { " --release" } else { "" },
        wrapper_dir.display(),
    );
    let status = cmd
        .status()
        .with_context(|| "spawn `cargo` — is it on your PATH?")?;
    if !status.success() {
        anyhow::bail!("[build-sim] cargo build exited with {status}");
    }
    Ok(())
}

#[cfg(test)]
mod regression_tests {
    //! Wrapper-shape regression for `build-sim`.
    //!
    //! The sim wrapper is a thin shim that picks a form-factor
    //! crate (`variant-phone` / `tablet` / `tv`) and a skin crate
    //! (`ios-sim` / `android-sim`). It doesn't carry `runtime-core`
    //! directly — the form-factor crate pulls it in transitively.
    //!
    //! What the wrapper IS responsible for: forwarding the
    //! `runtime-server` feature onto the form-factor crate in
    //! runtime-server mode. The variant crate (`variant-phone`,
    //! etc.) gates its `run_runtime_server` entry point on that
    //! feature; if the wrapper drops it, the generated main.rs
    //! references a function that doesn't exist at link time, and
    //! `idealyst run sim --runtime-server` fails to compile. The
    //! reverse is just as bad: turning the feature on in local
    //! mode pulls in the WebSocket / WireBackend transport stack
    //! for no reason.

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
            form: FormFactor::Phone,
            skin: PainterChoice::Ios,
            mode,
            source: FrameworkSource::Workspace { root: workspace_root },
        };
        generate_wrapper(&wrapper_dir, &cargo_target, &project_dir, &manifest, &opts)
            .expect("generate wrapper");
        (wrapper_dir, tmp)
    }

    fn variant_phone_features(toml_text: &str) -> Vec<String> {
        let parsed: toml::Value = toml::from_str(toml_text).expect("valid TOML");
        let phone = parsed
            .get("dependencies")
            .and_then(|d| d.get("variant-phone"))
            .expect("sim wrapper deps variant-phone");
        // Two shapes: `{ path = "...", features = [...] }` or
        // `{ path = "..." }`. The Workspace-mode `dep()` always
        // produces inline tables, so a table lookup is safe.
        phone
            .get("features")
            .and_then(|f| f.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn local_mode_does_not_enable_runtime_server_feature() {
        let (wrapper_dir, _tmp) = run_generator(BuildMode::Local);
        let cargo = std::fs::read_to_string(wrapper_dir.join("Cargo.toml")).unwrap();
        let feats = variant_phone_features(&cargo);
        assert!(
            !feats.iter().any(|f| f == "runtime-server"),
            "local sim wrapper must NOT enable variant-phone's `runtime-server` \
             feature; got {:?}",
            feats,
        );
    }

    #[test]
    fn runtime_server_mode_enables_runtime_server_feature() {
        let (wrapper_dir, _tmp) = run_generator(BuildMode::RuntimeServer);
        let cargo = std::fs::read_to_string(wrapper_dir.join("Cargo.toml")).unwrap();
        let feats = variant_phone_features(&cargo);
        assert!(
            feats.iter().any(|f| f == "runtime-server"),
            "runtime-server sim wrapper must enable variant-phone's `runtime-server` \
             feature so `run_runtime_server` is in scope. Got {:?}",
            feats,
        );
    }
}
