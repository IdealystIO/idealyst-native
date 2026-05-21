//! Sim runtime build orchestration for `idealyst run sim`.
//!
//! The "sim" target is the wgpu-backed desktop preview runtime —
//! `native-phone` / `native-tablet` / `native-tv` driving a winit
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

/// Skin choice — which `render_wgpu::Skin` paints the platform
/// chrome (status bar, gesture indicators, system fonts) around
/// the user's tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkinChoice {
    Ios,
    Android,
}

impl SkinChoice {
    pub fn as_str(self) -> &'static str {
        match self {
            SkinChoice::Ios => "ios",
            SkinChoice::Android => "android",
        }
    }
}

#[derive(Clone, Debug)]
pub struct BuildOptions {
    /// Compile with `--release`. Default: debug. The sim is a dev
    /// preview — release is rarely worth the slower rebuild.
    pub release: bool,
    /// Window form factor (drives crate selection + window size).
    pub form: FormFactor,
    /// Skin painting the platform chrome.
    pub skin: SkinChoice,
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
    let wrapper_dir = wrapper_root.join(&manifest.name).join("sim");
    let cargo_target_dir = opts.source.cargo_target_dir(&project_dir);

    generate_wrapper(&wrapper_dir, &cargo_target_dir, &project_dir, &manifest, &opts)?;
    cargo_build(&wrapper_dir, opts.release)?;

    let profile = if opts.release { "release" } else { "debug" };
    let bin_name = binary_name(&manifest.name);
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
/// so it doesn't collide with the project crate's own bin/lib name.
fn binary_name(project_name: &str) -> String {
    format!("{project_name}-sim")
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

    // Pick the form-factor crate (window size + title) and the skin
    // crate (chrome painter). Both come from the framework source so
    // workspace + git installs work identically.
    let (form_crate, form_subpath, form_ident) = match opts.form {
        FormFactor::Phone => ("native-phone", "crates/native/phone", "native_phone"),
        FormFactor::Tablet => ("native-tablet", "crates/native/tablet", "native_tablet"),
        FormFactor::Tv => ("native-tv", "crates/native/tv", "native_tv"),
    };
    let (skin_crate, skin_subpath, skin_ident, skin_type) = match opts.skin {
        SkinChoice::Ios => ("ios-sim", "crates/skin/ios-sim", "ios_sim", "IosSim"),
        SkinChoice::Android => (
            "android-sim",
            "crates/skin/android-sim",
            "android_sim",
            "AndroidSim",
        ),
    };

    let form_dep = opts.source.dep(form_subpath, &[]);
    let skin_dep = opts.source.dep(skin_subpath, &[]);
    let user_dep = format!("{{ path = \"{}\" }}", project_dir.display());

    let cargo_toml = format!(
        r#"# GENERATED by `idealyst run sim`. Do not edit — rewritten every build.
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

    let main_rs = format!(
        r#"//! GENERATED by `idealyst run sim`. Wrapper binary for the
//! wgpu sim runtime ({form_label}, {skin_label} skin).

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
