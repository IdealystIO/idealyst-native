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
use build_ios::{Target, parse_manifest};

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

    /// Build the native SSR server binary. Renders `app()` per request
    /// and (in hydration mode) emits the boot `<script>` so the live
    /// web bundle adopts the server DOM. The produced binary takes
    /// `--addr <host:port>` / `--static` / `--static-dir <path>` /
    /// `--bundle <url>` at run time. For hydration to work, the wasm
    /// bundle must be staged alongside — pair with `--web` (or run
    /// `idealyst build --web` separately) so `dist/web` contains the
    /// `pkg/` directory the binary serves.
    #[arg(long)]
    pub ssr: bool,

    /// Static-site generation: crawl every literal route in the app's
    /// navigator hierarchy and write `<out>/<path>.html` per page (root
    /// becomes `index.html`). Drops cleanly into S3 / CloudFront /
    /// nginx with no runtime SSR server. Builds the SSR wrapper binary
    /// under the hood and invokes it in `--export` mode against
    /// `dist/web/`. Parameterized routes (`:placeholder` segments) are
    /// skipped with a warning. Pair with `--web` so the emitted pages
    /// can hydrate via the wasm bundle; pair with `--ssg-static` to
    /// suppress the boot script for a pure-static deploy.
    #[arg(long)]
    pub ssg: bool,

    /// SSG only: suppress the hydration boot `<script>`. The exported
    /// HTML is pure server-render — useful for SEO/marketing pages
    /// where no client takeover is wanted. No effect outside `--ssg`.
    #[arg(long)]
    pub ssg_static: bool,

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
    /// `<project>/dist/web`. Has no effect on non-web targets.
    #[arg(long, value_name = "PATH")]
    pub out_dir: Option<PathBuf>,

    /// Web + release only: opt out of chunk-only data pruning in the
    /// main wasm bundle. By default release web builds zero data
    /// symbols (≥ 24 bytes) that wasm-split-cli classifies as
    /// reachable only from `lazy!` chunks — recovers ~25-50% of the
    /// gzipped main bundle on apps with a heavy lazy chunk (a wgpu
    /// simulator, an editor, …). The 24-byte threshold is the
    /// verified-safe floor below which the symbol-level call graph
    /// misclassifies small vtables and the runtime hits null-function
    /// traps. Pass this flag if a custom app trips on the analysis
    /// — file a repro so we can tighten the heuristic.
    #[arg(long)]
    pub no_data_prune: bool,

    /// Web + release only: strip panic machinery from the wasm bundle
    /// via `-Z build-std-features=panic_immediate_abort`. Every panic
    /// (incl. `unwrap`/`expect`) becomes a bare `unreachable` trap with
    /// NO message — only enable this for production builds where you've
    /// accepted losing crash diagnostics. REQUIRES a nightly toolchain
    /// with the `rust-src` component (`rustup component add rust-src
    /// --toolchain nightly`) and recompiles std from source, so the
    /// first build is slow. Implies `--release`. Modest size win
    /// (~30 KB gzip on a large app); has no effect on non-web targets.
    #[arg(long)]
    pub strip_panics: bool,

    /// Web only: enable the Robot bridge in the bundle (`robot` feature →
    /// `backend-web/robot` → `runtime-core/robot`). A browser app can't host
    /// the bridge itself, so it dials a `robot-relay` whose URL it reads from
    /// `window.IDEALYST_ROBOT_RELAY_URL`; the relay exposes the ordinary TCP
    /// bridge to the MCP server / an evaluator. Off by default; the MCP Arena
    /// and `idealyst dev --web --local --robot` pass it. No effect on non-web
    /// targets.
    #[arg(long)]
    pub robot: bool,
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
    if targets.is_empty() && !args.runtime_server && !args.ssr && !args.ssg {
        anyhow::bail!(
            "no targets to build: pass `--web` / `--ios` / `--android` / `--roku` / `--aas` / \
             `--ssr` / `--ssg`, or add `targets = [...]` to `[package.metadata.idealyst.app]`"
        );
    }
    // De-dup while preserving the order the user (or manifest) gave.
    let mut seen: std::collections::HashSet<Target> = std::collections::HashSet::new();
    targets.retain(|t| seen.insert(*t));

    let mut extras: Vec<&str> = Vec::new();
    if args.runtime_server {
        extras.push("aas host");
    }
    if args.ssr {
        extras.push("ssr binary");
    }
    if args.ssg {
        extras.push("ssg export");
    }
    eprintln!(
        "[build] {} targets: {}{}",
        if args.release { "release" } else { "debug" },
        targets
            .iter()
            .map(|t| t.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        if extras.is_empty() {
            String::new()
        } else {
            format!(" (+ {})", extras.join(", "))
        },
    );

    for target in &targets {
        build_target(*target, &dir, &args).with_context(|| format!("build {}", target))?;
    }

    if args.runtime_server {
        build_runtime_server_host(&dir, &args)?;
    }

    if args.ssr {
        build_ssr_binary(&dir, &args, targets.contains(&Target::Web))?;
    }

    if args.ssg {
        build_ssg_export(&dir, &args, targets.contains(&Target::Web))?;
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
    // `<project>/dist/web` (override with `--out-dir`). Each target gets
    // its own `dist/<target>` subdir so building several platforms into
    // the same project root doesn't clobber siblings, and `idealyst
    // serve` can default to `dist/web`. The bundle is what gets
    // deployed; nothing lands in the project root anymore. The older
    // "pkg/ in project dir" path is still used by the dev loop
    // (`idealyst dev --web`, which calls `build_web::build` with
    // `bundle_out_dir: None`) so the dev HTTP server can serve from
    // the project tree.
    let bundle_out_dir = Some(
        args.out_dir
            .clone()
            .unwrap_or_else(|| dir.join("dist").join(Target::Web.as_str())),
    );

    let artifact = build_web::build(
        dir,
        build_web::BuildOptions {
            // `--strip-panics` is a release-only transform, so it implies
            // `--release` (panic_immediate_abort in a debug build would
            // just slow the build for no benefit).
            release: args.release || args.strip_panics,
            source: source.clone(),
            // `robot` is a wrapper-local feature → `backend-web/robot`; the
            // build/web feature filter skips forwarding it to the user crate.
            user_features: if args.robot {
                vec!["robot".to_string()]
            } else {
                Vec::new()
            },
            bundle_out_dir: bundle_out_dir.clone(),
            gzip: args.gzip,
            strip_panics: args.strip_panics,
            // Compile in hydration when SSG/SSR is also being built —
            // the emitted HTML expects the wasm to adopt it on boot.
            // Pure SPA builds drop the machinery for a smaller wasm.
            hydrate: args.ssg || args.ssr,
            // Release web builds prune chunk-only data ≥ 24 bytes from
            // the main bundle (the verified-safe floor below which the
            // heuristic call graph misclassifies small vtables and
            // null-function-traps at runtime). Recovers up to ~50% of
            // gzipped bytes on apps with a heavy lazy chunk. Debug
            // builds skip pruning to keep the build cycle fast.
            // `--no-data-prune` opts out.
            prune_dead_data_min: if args.release && !args.no_data_prune {
                Some(24)
            } else {
                None
            },
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

    // TODO(lazy-primitive): post-cargo wasm-split-cli step here.
    // Read the wasm-pack output, run wasm-split-cli to extract
    // chunks, emit them into `<bundle>/pkg/`. Coming up next.

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
            // `build --macos` is a host-arch dev artifact; the universal
            // (Intel + Apple Silicon) build is `publish macos`'s job.
            universal: false,
        },
    )?;
    eprintln!("[build macos] success → {}", artifact.binary.display(),);
    Ok(())
}

fn build_ssr_binary(dir: &std::path::Path, args: &Args, web_built: bool) -> Result<()> {
    let source = crate::framework_source::resolve(dir)?;
    let artifact = build_ssr::build(
        dir,
        build_ssr::BuildOptions {
            release: args.release,
            source,
            user_features: Vec::new(),
        },
    )?;
    eprintln!(
        "[build ssr] success → {} (wrapper at {})",
        artifact.binary.display(),
        artifact.wrapper_dir.display(),
    );
    eprintln!(
        "  run: {} --addr 0.0.0.0:8081 --static-dir {} [--static]",
        artifact.binary.display(),
        dir.join("dist").join("web").display(),
    );
    if !web_built {
        eprintln!(
            "  ⚠ no `--web` in this build — hydration mode needs the wasm bundle at \
             `dist/web/pkg/`. Run `idealyst build --web{}` to stage it, or pass `--static` \
             to the SSR binary for the no-hydration variant.",
            if args.release { " --release" } else { "" },
        );
    }
    Ok(())
}

fn build_ssg_export(dir: &std::path::Path, args: &Args, web_built: bool) -> Result<()> {
    // SSG reuses the SSR wrapper binary — same generator, same dep
    // graph; the wrapper's `--export <dir>` mode calls `render_all` and
    // writes per-path `index.html` files into the bundle dir.
    let source = crate::framework_source::resolve(dir)?;
    let artifact = build_ssr::build(
        dir,
        build_ssr::BuildOptions {
            release: args.release,
            source,
            user_features: Vec::new(),
        },
    )?;
    let out_dir = args
        .out_dir
        .clone()
        .unwrap_or_else(|| dir.join("dist").join(Target::Web.as_str()));
    std::fs::create_dir_all(&out_dir)
        .with_context(|| format!("create SSG output dir {}", out_dir.display()))?;
    eprintln!("[build ssg] crawling navigator hierarchy → {}", out_dir.display());
    let mut cmd = std::process::Command::new(&artifact.binary);
    cmd.arg("--export").arg(&out_dir);
    if args.ssg_static {
        cmd.arg("--static");
    }
    let status = cmd
        .status()
        .with_context(|| format!("spawn SSG wrapper {}", artifact.binary.display()))?;
    if !status.success() {
        anyhow::bail!("SSG export failed (wrapper exit {})", status);
    }
    if !web_built && !args.ssg_static {
        eprintln!(
            "  ⚠ no `--web` in this build — emitted pages reference `/pkg/{}.js` for \
             hydration but the bundle isn't staged. Run `idealyst build --web{}` to stage \
             it, or re-run with `--ssg-static` for a no-hydration pure-static export.",
            parse_manifest(dir)?.lib_name,
            if args.release { " --release" } else { "" },
        );
    }
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
