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

    /// Build for native macOS (AppKit `.app` via `host-appkit` +
    /// `backend-macos`). Different from `--sim` — that's the wgpu
    /// phone-shaped preview; `--macos` is the desktop-native target.
    #[arg(long)]
    pub macos: bool,

    /// Build for the terminal (TTY binary via `host-terminal` +
    /// `backend-terminal`).
    #[arg(long)]
    pub terminal: bool,

    /// Build the runtime-server dev-host binary on its own. Not a
    /// deploy target — useful for running the host outside of
    /// `idealyst dev --runtime-server`. `--aas` accepted as a
    /// deprecated alias for one release.
    #[arg(long, alias = "aas")]
    pub runtime_server: bool,

    /// Build with the release profile.
    #[arg(long)]
    pub release: bool,

    /// iOS only: build for a physical device rather than the
    /// simulator.
    #[arg(long)]
    pub device: bool,

    /// Web only: pre-gzip every text-ish file in the staged bundle.
    /// Filenames stay the same — the bytes are gzipped. The host
    /// must send `Content-Encoding: gzip` on those responses for the
    /// browser to inflate. Skip this if a CDN in front of the bucket
    /// already does on-the-fly compression. Has no effect on non-web
    /// targets.
    #[arg(long)]
    pub gzip: bool,

    /// Web only: override where the bundle is written. Default is
    /// `<project>/dist`. Has no effect on non-web targets.
    #[arg(long, value_name = "PATH")]
    pub out_dir: Option<PathBuf>,
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
    if targets.is_empty() && !args.runtime_server {
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
        if args.runtime_server { " (+ aas host)" } else { "" },
    );

    for target in &targets {
        build_target(*target, &dir, &args).with_context(|| format!("build {}", target))?;
    }

    if args.runtime_server {
        build_runtime_server_host(&dir, &args)?;
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
    if args.macos {
        out.push(Target::Macos);
    }
    if args.terminal {
        out.push(Target::Terminal);
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
        Target::Macos => build_macos_target(dir, args),
        Target::Terminal => build_terminal_target(dir, args),
    }
}

fn build_terminal_target(dir: &std::path::Path, args: &Args) -> Result<()> {
    let source = crate::framework_source::resolve(dir)?;
    let artifact = build_terminal::build(
        dir,
        build_terminal::BuildOptions {
            release: args.release,
            mode: build_terminal::BuildMode::Local,
            source,
            user_features: Vec::new(),
        },
    )?;
    eprintln!(
        "[build terminal] success → {} ({})",
        artifact.binary.display(),
        artifact.wrapper_dir.display(),
    );
    Ok(())
}

fn build_web(dir: &std::path::Path, args: &Args) -> Result<()> {
    // Web builds go through a generated wrapper crate, same shape as
    // iOS / Android: the user's app crate stays platform-agnostic
    // (no `web.rs`, no `[lib] crate-type = ["cdylib"]`, no
    // `wasm-bindgen` dep) and the wrapper carries the
    // `#[wasm_bindgen(start)]` entry point + cdylib output. The
    // wrapper is regenerated on every build; wasm-pack runs against
    // it, and the resulting `pkg/` is copied into the user project
    // so existing `index.html` references keep working.
    let source = crate::framework_source::resolve(dir)?;

    // `idealyst build --web` always stages a self-contained bundle at
    // `<project>/dist` (override with `--out-dir`). The bundle is what
    // gets deployed; nothing lands in the project root anymore. The
    // older "pkg/ in project dir" path is still used by the dev loop
    // (`idealyst dev --web`, which calls `build_web::build` with
    // `bundle_out_dir: None`) so the dev HTTP server can serve from
    // the project tree.
    let bundle_out_dir = Some(args.out_dir.clone().unwrap_or_else(|| dir.join("dist")));

    let artifact = build_web::build(
        dir,
        build_web::BuildOptions {
            release: args.release,
            source,
            user_features: Vec::new(),
            bundle_out_dir,
            gzip: args.gzip,
        },
    )?;
    let bundle = artifact
        .bundle_dir
        .as_deref()
        .expect("CLI always sets bundle_out_dir for --web; this Option is for the dev-loop path");
    eprintln!(
        "[build web] bundle{} → {}",
        if args.gzip { " (gzipped)" } else { "" },
        bundle.display(),
    );
    if args.gzip {
        eprintln!(
            "[build web] serve with `Content-Encoding: gzip` on every response (the bundle's \
             filenames are unchanged but their bytes are gzipped). See \
             examples/website/scripts/export-static.sh for a reference S3 upload."
        );
    }
    Ok(())
}

fn build_ios_target(dir: &std::path::Path, args: &Args) -> Result<()> {
    let source = crate::framework_source::resolve(dir)?;
    let artifact = build_ios::build(
        dir,
        build_ios::BuildOptions {
            release: args.release,
            device: args.device,
            source,
            user_features: Vec::new(),
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
    let source = crate::framework_source::resolve(dir)?;
    let artifact = build_android::build(
        dir,
        build_android::BuildOptions {
            release: args.release,
            api_level: 21,
            mode: build_android::BuildMode::Local,
            source,
            user_features: Vec::new(),
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
    let source = crate::framework_source::resolve(dir)?;
    let artifact = build_roku::build(
        dir,
        build_roku::BuildOptions {
            output_dir: None,
            ui_json: None,
            title: None,
            source,
        },
    )?;
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

fn build_macos_target(dir: &std::path::Path, args: &Args) -> Result<()> {
    let source = crate::framework_source::resolve(dir)?;
    let artifact = build_macos::build(
        dir,
        build_macos::BuildOptions {
            release: args.release,
            // `idealyst build --macos` always produces the local-mount
            // wrapper. The runtime-server variant is dev-only (no shipping use
            // case for a binary that requires a dev-server at runtime).
            mode: build_macos::BuildMode::Local,
            source,
            user_features: Vec::new(),
        },
    )?;
    eprintln!(
        "[build macos] success → {}",
        artifact.binary.display(),
    );
    Ok(())
}

fn build_runtime_server_host(dir: &std::path::Path, args: &Args) -> Result<()> {
    let source = crate::framework_source::resolve(dir)?;
    let artifact = build_runtime_server::build(
        dir,
        build_runtime_server::BuildOptions {
            release: args.release,
            source,
        },
    )?;
    eprintln!(
        "[build aas] success → {} (wrapper at {})",
        artifact.host_binary.display(),
        artifact.wrapper_dir.display(),
    );
    Ok(())
}
