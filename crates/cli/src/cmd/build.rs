//! `idealyst build` — produce shippable artifacts for one or more
//! platforms.
//!
//! Mirrors the flag shape of `idealyst dev`: `--web`, `--ios`,
//! `--android`, optional `--aas` (build the dev-host binary even
//! though it's not a deploy target), plus `--release` to flip every
//! platform into its production pipeline (wasm-opt for web,
//! xcodebuild Release for iOS, `assembleRelease` for Android).
//!
//! With no platform flags, the active set falls back to
//! `[package.metadata.idealyst.app].targets`. Builds run sequentially
//! — there's no point parallelizing cargo invocations against the
//! same target dir.

use std::path::PathBuf;

use anyhow::{Context, Result};
use build_ios::{parse_manifest, Target};

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Project directory.
    #[arg(default_value = ".")]
    pub dir: PathBuf,

    /// Build for the web (wasm bundle).
    #[arg(long)]
    pub web: bool,

    /// Build for iOS (staticlib + Xcode wrapper).
    #[arg(long)]
    pub ios: bool,

    /// Build for Android (cdylib + Gradle wrapper).
    #[arg(long)]
    pub android: bool,

    /// Build for Roku (package layout + manifest).
    #[arg(long)]
    pub roku: bool,

    /// Build the AAS dev-host binary. Not a deploy target — useful
    /// for inspecting the wrapper Cargo workspace or running the
    /// host outside of `idealyst dev --aas`.
    #[arg(long)]
    pub aas: bool,

    /// Release profile. Forwarded to each platform's release
    /// pipeline (wasm-opt for web, xcodebuild Release for iOS,
    /// `assembleRelease` for Android, cargo --release for AAS host).
    #[arg(long)]
    pub release: bool,

    /// iOS only: build for a physical device (`aarch64-apple-ios`)
    /// rather than the simulator's host arch.
    #[arg(long)]
    pub device: bool,
}

pub fn run(args: Args) -> Result<()> {
    let dir = std::fs::canonicalize(&args.dir)
        .with_context(|| format!("cannot resolve project dir {}", args.dir.display()))?;
    let manifest = parse_manifest(&dir)?;

    // Resolve which targets to build. Explicit flags win; otherwise
    // fall back to manifest. The `--aas` flag is separate from the
    // platform set — it's an extra build that happens alongside the
    // platforms (or alone if no platforms are selected).
    let mut targets = collect_targets(&args, &manifest.app.targets);
    if targets.is_empty() && !args.aas {
        anyhow::bail!(
            "no targets to build: pass `--web` / `--ios` / `--android` / `--roku` / `--aas`, \
             or add `targets = [...]` to `[package.metadata.idealyst.app]`"
        );
    }
    // De-dup while preserving the order the user (or manifest) gave.
    let mut seen: std::collections::HashSet<Target> = std::collections::HashSet::new();
    targets.retain(|t| seen.insert(*t));

    eprintln!(
        "[build] {} targets: {}{}",
        if args.release { "release" } else { "debug" },
        targets
            .iter()
            .map(|t| t.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        if args.aas { " (+ aas host)" } else { "" },
    );

    for target in &targets {
        build_target(*target, &dir, &args).with_context(|| format!("build {}", target))?;
    }

    if args.aas {
        build_aas_host(&dir, &args)?;
    }

    Ok(())
}

fn collect_targets(args: &Args, manifest_targets: &[Target]) -> Vec<Target> {
    let mut out = Vec::new();
    if args.web {
        out.push(Target::Web);
    }
    if args.ios {
        out.push(Target::Ios);
    }
    if args.android {
        out.push(Target::Android);
    }
    if args.roku {
        out.push(Target::Roku);
    }
    if out.is_empty() {
        out.extend(manifest_targets.iter().copied());
    }
    out
}

fn build_target(target: Target, dir: &std::path::Path, args: &Args) -> Result<()> {
    match target {
        Target::Web => build_web(dir, args),
        Target::Ios => build_ios_target(dir, args),
        Target::Android => build_android_target(dir, args),
        Target::Roku => build_roku_target(dir, args),
    }
}

fn build_web(dir: &std::path::Path, args: &Args) -> Result<()> {
    // wasm-pack is the canonical web build tool. Release adds
    // `--release` + the wasm-opt flags declared in the user's
    // Cargo.toml under `[package.metadata.wasm-pack.profile.release]`.
    eprintln!("[build web] running wasm-pack…");
    let mut cmd = std::process::Command::new("wasm-pack");
    cmd.current_dir(dir).arg("build").args(["--target", "web"]);
    if args.release {
        cmd.arg("--release");
    } else {
        cmd.arg("--dev");
    }
    let status = cmd
        .status()
        .with_context(|| "exec wasm-pack — is it on PATH? (cargo install wasm-pack)")?;
    if !status.success() {
        anyhow::bail!("wasm-pack exited with {status}");
    }
    eprintln!("[build web] success → {}", dir.join("pkg").display());
    Ok(())
}

fn build_ios_target(dir: &std::path::Path, args: &Args) -> Result<()> {
    let artifact = build_ios::build(
        dir,
        build_ios::BuildOptions {
            release: args.release,
            device: args.device,
        },
    )?;
    eprintln!(
        "[build ios] success → {} ({})",
        artifact.staticlib.display(),
        artifact.target_triple,
    );
    Ok(())
}

fn build_android_target(dir: &std::path::Path, args: &Args) -> Result<()> {
    let artifact = build_android::build(
        dir,
        build_android::BuildOptions {
            release: args.release,
            ..Default::default()
        },
    )?;
    eprintln!(
        "[build android] success → {} (abi: {})",
        artifact.dylib.display(),
        artifact.abi,
    );
    Ok(())
}

fn build_roku_target(dir: &std::path::Path, _args: &Args) -> Result<()> {
    let artifact = build_roku::build(dir, build_roku::BuildOptions::default())?;
    eprintln!(
        "[build roku] success → {} ({} #[method] fns, {} ui commands)",
        artifact.package_dir.display(),
        artifact.method_count,
        artifact.command_count,
    );
    if artifact.command_count == 0 {
        eprintln!(
            "  ⚠ no `dist/ui.json` found — the package will install but render an empty scene"
        );
    }
    Ok(())
}

fn build_aas_host(dir: &std::path::Path, args: &Args) -> Result<()> {
    let artifact = build_aas::build(
        dir,
        build_aas::BuildOptions {
            release: args.release,
        },
    )?;
    eprintln!(
        "[build aas] success → {} (wrapper at {})",
        artifact.host_binary.display(),
        artifact.wrapper_dir.display(),
    );
    Ok(())
}
