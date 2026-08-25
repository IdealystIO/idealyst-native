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

    /// Build for native Linux (GTK4 binary via `host-gtk` +
    /// `backend-linux`).
    #[arg(long)]
    pub linux: bool,

    /// Build for native Windows (Win32 `.exe` via `host-win32` +
    /// `backend-windows`).
    #[arg(long)]
    pub windows: bool,

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

    /// Build the app's `#[server]` fns as an AWS Lambda `bootstrap`
    /// (`provided.al2023` custom runtime) via `cargo lambda build`.
    /// Generates an ephemeral wrapper that runs `server::router()` under
    /// the Lambda runtime (`server_aws::run()`), then stages the
    /// `bootstrap` + an RIE Dockerfile under `dist/serverless-lambda/` for
    /// image-fidelity local testing and `cargo lambda deploy`. Requires
    /// `cargo lambda` (`cargo install cargo-lambda`). Server-side target —
    /// not part of the client `targets` set.
    #[arg(long)]
    pub serverless_lambda: bool,

    /// serverless-lambda only: target CPU architecture (`arm64` default,
    /// or `x86_64`). No effect on other targets.
    #[arg(long, value_name = "arm64|x86_64")]
    pub arch: Option<String>,

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

    /// Web only: skip emitting precompressed `.br` siblings in the
    /// staged bundle. By default a release build writes `<file>.br`
    /// next to every compressible bundle file (brotli q11, ~20%
    /// smaller than gzip -9 on wasm) so hosts with nginx
    /// `brotli_static` / Caddy `precompressed` / a CDN edge serve
    /// max-quality brotli at zero per-request cost. Originals are
    /// kept — hosts without brotli support are unaffected.
    #[arg(long)]
    pub no_brotli: bool,

    /// Web only: override where the bundle is written. Default is
    /// `<project>/dist/web`. Has no effect on non-web targets.
    #[arg(long, value_name = "PATH")]
    pub out_dir: Option<PathBuf>,

    /// **EXPERIMENTAL, off by default.** Web + release only: opt IN to
    /// chunk-only data pruning in the main wasm bundle. When enabled, release
    /// builds zero data symbols (≥ 24 bytes) that wasm-split-cli classifies as
    /// reachable only from lazy chunks — recovering ~25-50% of the gzipped
    /// main bundle on apps with a heavy lazy chunk.
    ///
    /// Every pruned symbol is re-materialized by the chunk that owns it, from
    /// any active const-offset data segment (`.rodata`, `.data`, `.bss`), and
    /// symbols in segments a chunk can't restore are never pruned — so a
    /// chunk-only symbol always has exactly one shipper.
    ///
    /// This is still OFF by default because the classification
    /// **under-approximates what `main` reaches**: it can't trace data reached
    /// via data→data pointers, `call_indirect` / the function table, or the
    /// deferred handler-registration queue (removed with the old core;
    /// see docs/proposals/lazy-primitive.md). Data that `main`
    /// actually reads BEFORE the owning chunk loads gets misclassified
    /// "chunk-only" and zeroed, silently corrupting `main.wasm` (no wasm
    /// trap): fonts fail to register, a `#[component(lazy)]` route renders
    /// nothing. Only enable it after verifying your app renders correctly
    /// with it, and re-verify when your static data changes.
    #[arg(long)]
    pub data_prune: bool,

    /// Deprecated no-op: data pruning is now off by default (see
    /// `--data-prune`). Accepted so existing invocations keep working; if both
    /// are passed, pruning stays off.
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

    /// Web only: premint static styles at build time. Runs an ephemeral
    /// native dump build that emits every `stylesheet!`'s full variant
    /// space into a content-addressed `pkg/premint.<hash>.css` (linked
    /// from index.html), and compiles the wasm with
    /// `--cfg idealyst_premint` so all-constant style applications ship
    /// as class references instead of invoking the runtime style
    /// engine. The full size win additionally needs the app to disable
    /// the `style-dynamic` feature on `backend-web` so the engine drops
    /// out of the bundle — without that edit the classes premint but the
    /// engine still ships. Composes with `--ssg`/`--ssr`: the server
    /// binary is built with the same premint cfgs, stamps the same
    /// `iy-*` classes the hydrating client stamps, links `premint.css`
    /// from every page, and arms the same minted-class guard.
    #[arg(long)]
    pub premint: bool,

    /// Web only, debug builds only: how much debug info the wasm carries.
    /// `line-tables` (default) keeps stack frames symbolizing to source
    /// lines while dropping the DWARF for locals and types — on a large
    /// app that is most of the module's bytes, and wasm has no sidecar to
    /// move them to. `full` restores cargo's `debug = 2` for a session
    /// with a wasm-aware debugger; `none` drops it entirely. Panic
    /// messages name their `file:line` at every level (that is `.rodata`,
    /// not debug info). Release builds set their own posture and ignore
    /// this.
    #[arg(long, default_value = "line-tables")]
    pub debuginfo: String,

    /// Web only, debug builds only: how much CPU each rebuild spends
    /// optimizing. `optimized` (default) keeps workspace members at the
    /// workspace's own `[profile.dev]` and dependencies at `opt-level =
    /// 3`; `fast` drops the app and every framework crate to `0` and
    /// dependencies to `1`.
    ///
    /// `fast` cuts the cargo half but produces a ~37% larger module, and
    /// wasm-bindgen + wasm-split are O(module size) and run on every
    /// rebuild — so with incremental compilation on (cargo's default) it
    /// is a net LOSS end-to-end, on framework-crate and leaf edits alike.
    /// Reach for it when incremental is off (`CARGO_INCREMENTAL=0`, or
    /// right after a `cargo clean`), where a framework-crate edit
    /// measured 7.3s under `fast` vs 14.2s under `optimized`. Release
    /// builds set their own posture and ignore this.
    #[arg(long, default_value = "optimized")]
    pub dev_opt: String,

    /// Web only: skip the `wasm-split` pass. `#[component(lazy)]`
    /// boundaries still work — their bodies ship in the main bundle and
    /// resolve immediately instead of over the network. Trades bundle
    /// size for packaging time: outside release, splitting is also the
    /// only pass that compacts the module, so skipping it leaves the
    /// relocs in and the served wasm several times larger (measured on
    /// the `welcome` example: 2.2 MB split, 6.3 MB skipped, for 0.2s
    /// saved). On a large app the browser then pays more to compile the
    /// bigger module on every reload, which can outweigh the saving.
    #[arg(long)]
    pub no_split: bool,

    /// Web only: enable the Robot bridge in the bundle (`robot` feature →
    /// `backend-web/robot`). A browser app can't host
    /// the bridge itself, so it dials a `robot-relay` whose URL it reads from
    /// `window.IDEALYST_ROBOT_RELAY_URL`; the relay exposes the ordinary TCP
    /// bridge to the MCP server / an evaluator. Off by default; the MCP Arena
    /// and `idealyst dev --web --local --robot` pass it. No effect on non-web
    /// targets.
    #[arg(long)]
    pub robot: bool,

    /// Accepted no-op alias: runtime v2 is the only runtime, so every
    /// build already runs on it. Kept so existing invocations, scripts,
    /// and CI files don't break (see `crate::core_mode`).
    #[arg(long)]
    pub new_core: bool,

    /// REMOVED: the pre-runtime-v2 walker no longer exists. Passing
    /// this fails with the migration pointer rather than silently
    /// building runtime v2 with old-core semantics expected
    /// (`crate::core_mode`, `docs/migrating-to-runtime-v2.md`).
    #[arg(long)]
    pub old_core: bool,


    /// Web + release only: additionally compile the runtime style engine OUT
    /// of the bundle. Implies `--premint`.
    ///
    /// `--premint` mints build-time CSS but CANNOT remove the engine: the
    /// `stylesheet!` builder's preminted fast path falls through to the live
    /// engine for any reactive or override-carrying style, so the engine
    /// stays linked even when every class preminted. This strips those
    /// fallthrough paths.
    ///
    /// A promise the build cannot verify. Styles that still need the engine
    /// — a reactive input, a runtime slot override, `signal_class`, a raw
    /// `StyleRules` closure, or passing the raw sheet (`style =
    /// card_style()`) instead of the builder (`style = Card()`) — panic at
    /// mount with a message naming the shape. Use `--premint` alone if
    /// unsure; it is always safe.
    #[arg(long)]
    pub premint_only: bool,

    /// Web only: report every style that still needs the runtime style
    /// engine. The diagnostic for "why can't this app use
    /// `--premint-only`?". Implies `--premint`.
    ///
    /// KEEPS the engine, so the app renders normally and one page load
    /// lists everything that fell through — rather than the boot panic
    /// `--premint-only` gives you at the first offender, which names the
    /// shape but not the source.
    ///
    /// Each distinct fall-through logs once to the browser console as
    /// `[premint-report] #N <shape> css=… overrides=… computed=… axes=…
    /// rules=…`. `css=NONE-no-build-time-css` means the sheet never
    /// preminted at all (give it an identity); a `computed=` key means a
    /// `with_computed` layer, whose rules are produced at runtime under a
    /// key the build cannot enumerate — that one needs the layer turned
    /// into a bounded variant axis, or the app keeps the engine.
    ///
    /// Not a size flag: a `--premint-report` build is a debugging build.
    #[arg(long)]
    pub premint_report: bool,

    /// MOVED to the app's manifest — passing this is an error that tells
    /// you what to write instead. The primitive set is a property of the
    /// app, not of one invocation of a build tool, and it has to be a
    /// *type* at the boot call site for the dead-code elimination to fire
    /// at all, so it now lives where `idealyst::entry!` can read it:
    ///
    ///     [package.metadata.idealyst.app]
    ///     primitives = ["view", "text", "button"]
    ///
    /// Omit it to register every builtin (the default). `entry!` also
    /// accepts the `core` (`view` + `text`) and `all` presets.
    ///
    /// Unlisted primitives are never named at the boot seam, so their
    /// handlers, the backend code behind them, and the web-sys imports and
    /// JS glue they alone reached are all dropped by LLVM. Measured on a
    /// `view`+`text` app: 195,255 → 126,813 bytes brotli (-35%) and 236 →
    /// 163 wasm imports.
    ///
    /// Rendering a primitive the set omits panics at mount — deliberately
    /// loud, the same failure a missing third-party payload gets. Note a
    /// component library counts: `idea_ui::Button` needs `button`.
    ///
    /// Kept as an argument rather than deleted so the migration message
    /// reaches anyone with it in a script; see `build_web::build`.
    #[arg(long, value_delimiter = ',')]
    pub primitives: Option<Vec<String>>,
}

pub fn run(args: Args) -> Result<()> {
    // Removed-flag rejection runs FIRST, before the project is even
    // resolved: these errors are about the invocation, not the project,
    // and an operator porting a CI line should read "this flag is gone"
    // rather than an unrelated manifest error from whatever directory
    // the command happened to run in.
    //
    // One core: `--new-core` is a no-op, `--old-core` is a hard error.
    crate::core_mode::validate_flags(args.new_core, args.old_core)?;

    let dir = crate::framework_source::abs_project_dir(&args.dir)?;
    let manifest = parse_manifest(&dir)?;

    // Resolve which targets to build. Explicit flags win; otherwise
    // fall back to manifest. The `--aas` flag is separate from the
    // platform set — it's an extra build that happens alongside the
    // platforms (or alone if no platforms are selected).
    // `--serverless-lambda` is a server-side artifact; on its own it should
    // NOT drag in the manifest's client `targets` (a lambda build shouldn't
    // also emit a web bundle). Combining explicitly still works
    // (`--web --serverless-lambda`). ssr/ssg/runtime-server keep their existing
    // manifest-fallback so hydration can rely on the web bundle being built.
    let explicit_client = args.web
        || args.ios
        || args.android
        || args.roku
        || args.macos
        || args.terminal
        || args.linux
        || args.windows;
    let mut targets = if args.serverless_lambda && !explicit_client {
        Vec::new()
    } else {
        collect_targets(&args, &manifest.app.targets)
    };
    if targets.is_empty()
        && !args.runtime_server
        && !args.ssr
        && !args.ssg
        && !args.serverless_lambda
    {
        anyhow::bail!(
            "no targets to build: pass `--web` / `--ios` / `--android` / `--roku` / `--aas` / \
             `--ssr` / `--ssg` / `--serverless-lambda`, or add `targets = [...]` to \
             `[package.metadata.idealyst.app]`"
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
    if args.serverless_lambda {
        extras.push("serverless-lambda");
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

    // The web target's staged bundle is content-hashed; its entry shim
    // name (`<lib>.<hash>.js`) flows into the SSG export so emitted
    // pages hydrate via the fingerprinted URL.
    let mut web_entry: Option<String> = None;
    for target in &targets {
        if let Some(entry) =
            build_target(*target, &dir, &args).with_context(|| format!("build {}", target))?
        {
            web_entry = Some(entry);
        }
    }

    if args.runtime_server {
        build_runtime_server_host(&dir, &args)?;
    }

    if args.ssr {
        build_ssr_binary(&dir, &args, targets.contains(&Target::Web))?;
    }

    if args.ssg {
        build_ssg_export(
            &dir,
            &args,
            targets.contains(&Target::Web),
            web_entry.as_deref(),
        )?;
    }

    if args.serverless_lambda {
        build_serverless_lambda_target(&dir, &args)?;
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
    if args.linux {
        out.push(Target::Linux);
    }
    if args.windows {
        out.push(Target::Windows);
    }
    if out.is_empty() {
        out.extend(manifest_targets.iter().copied());
    }
    out
}

/// Returns the web bundle's content-hashed entry shim name for the
/// web target (`Some("<lib>.<hash>.js")`); `None` for every other
/// target.
fn build_target(target: Target, dir: &std::path::Path, args: &Args) -> Result<Option<String>> {
    match target {
        Target::Web => return build_web(dir, args),
        Target::Ios => build_ios_target(dir, args)?,
        Target::Android => build_android_target(dir, args)?,
        Target::Roku => build_roku_target(dir, args)?,
        Target::Macos => build_macos_target(dir, args)?,
        Target::Terminal => build_terminal_target(dir, args)?,
        Target::Linux => build_linux_target(dir, args)?,
        Target::Windows => build_windows_target(dir, args)?,
    }
    Ok(None)
}

fn build_windows_target(dir: &std::path::Path, args: &Args) -> Result<()> {
    let source = crate::framework_source::resolve(dir)?;
    let artifact = build_windows::build(
        dir,
        build_windows::BuildOptions {
            release: args.release,
            source,
            user_features: Vec::new(),
        },
    )?;
    eprintln!("[build windows] success → {}", artifact.binary.display());
    Ok(())
}

fn build_linux_target(dir: &std::path::Path, args: &Args) -> Result<()> {
    let source = crate::framework_source::resolve(dir)?;
    let artifact = build_linux::build(
        dir,
        build_linux::BuildOptions {
            release: args.release,
            mode: build_linux::BuildMode::Local,
            source,
            user_features: Vec::new(),
        },
    )?;
    eprintln!("[build linux] success → {}", artifact.binary.display());
    Ok(())
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

fn build_web(dir: &std::path::Path, args: &Args) -> Result<Option<String>> {
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
            primitives: args.primitives.clone(),
            premint_only: args.premint_only,
            premint_report: args.premint_report,
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
            // `.br` siblings ride release builds only — the deploy
            // artifact. Debug bundles skip the q11 encode (seconds of
            // build tail for a bundle nobody ships).
            brotli: !args.no_brotli && (args.release || args.strip_panics),
            strip_panics: args.strip_panics,
            // Compile in hydration when SSG/SSR is also being built —
            // the emitted HTML expects the wasm to adopt it on boot.
            // Pure SPA builds drop the machinery for a smaller wasm.
            hydrate: args.ssg || args.ssr,
            // Chunk-only data pruning is OFF by default and opt-in via
            // `--data-prune` — the classification under-approximates main's
            // reachability and silently corrupts main.wasm otherwise. See
            // `resolve_prune_data_min`.
            prune_dead_data_min: resolve_prune_data_min(
                args.release,
                args.data_prune,
                args.no_data_prune,
            ),
            wasm_split: !args.no_split,
            debuginfo: build_web::DebugInfo::from_cli(&args.debuginfo)?,
            dev_opt: build_web::DevOpt::from_cli(&args.dev_opt)?,
            // `--premint-only` strips the engine, so it MUST also premint —
            // otherwise the bundle has neither build-time classes nor a
            // runtime to mint them, and every styled node panics.
            premint: args.premint || args.premint_only || args.premint_report,
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
    eprintln!(
        "[build web] pkg/ filenames carry the build hash — safe to serve with \
         `Cache-Control: public, max-age=31536000, immutable`; keep `*.html` at \
         `no-cache` so redeploys are picked up."
    );

    if args.gzip {
        eprintln!(
            "[build web] serve with `Content-Encoding: gzip` on every response (the bundle's \
             filenames are unchanged but their bytes are gzipped). See \
             websites/website/scripts/export-static.sh for a reference S3 upload."
        );
    }
    Ok(artifact.entry_js)
}

/// Resolve the chunk-only data-prune threshold for a web build. Returns
/// `Some(min_bytes)` to enable pruning, `None` to disable it.
///
/// Pruning is **off by default** and opt-in via `--data-prune`. The
/// `wasm-split-cli` chunk-only classification under-approximates what `main`
/// reaches: it walks the symbol-level call graph but can't trace data reached
/// through data→data pointers, `call_indirect` / the function table, or the
/// deferred handler-registration queue (removed with the old core).
/// Data `main` reads through
/// those edges gets misclassified "chunk-only" and zeroed — silently corrupting
/// `main.wasm` with no wasm trap (observed: fonts fail to register via
/// `typeface!`, and a `#[component(lazy)]` route mounts nothing, not even its
/// `loading` placeholder). The 24-byte floor only guards small vtables; larger
/// misclassified statics slip through. So the safe default is to not prune;
/// `--data-prune` is an explicit, per-app opt-in for those who verify it.
///
/// `--no-data-prune` is now redundant (kept as a no-op); if both flags are
/// passed, pruning stays off.
fn resolve_prune_data_min(release: bool, data_prune: bool, no_data_prune: bool) -> Option<usize> {
    if release && data_prune && !no_data_prune {
        Some(24)
    } else {
        None
    }
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
            // The server must share the wasm bundle's premint posture:
            // both sides stamp classes during hydration, and only
            // matching cfgs make them stamp the SAME ones.
            premint: args.premint || args.premint_only || args.premint_report,
            premint_only: args.premint_only,
            premint_report: args.premint_report,
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

fn build_ssg_export(
    dir: &std::path::Path,
    args: &Args,
    web_built: bool,
    web_entry: Option<&str>,
) -> Result<()> {
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
            // The server must share the wasm bundle's premint posture:
            // both sides stamp classes during hydration, and only
            // matching cfgs make them stamp the SAME ones.
            premint: args.premint || args.premint_only || args.premint_report,
            premint_only: args.premint_only,
            premint_report: args.premint_report,
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
    } else {
        // Point the emitted pages' hydration script at the
        // content-hashed entry shim. Entry from this run's web build
        // when `--web` was included; otherwise from the pkg/ a prior
        // `idealyst build --web` staged into the same out dir. Without
        // a hashed bundle (nothing staged yet), the wrapper's unhashed
        // default stands and the warning below fires.
        let entry = web_entry.map(str::to_string).or_else(|| {
            build_web::find_hashed_entry(&out_dir.join("pkg"), &parse_manifest(dir).ok()?.lib_name)
        });
        if let Some(entry) = entry {
            cmd.arg("--bundle").arg(format!("/pkg/{entry}"));
        }
    }
    let status = cmd
        .status()
        .with_context(|| format!("spawn SSG wrapper {}", artifact.binary.display()))?;
    if !status.success() {
        anyhow::bail!("SSG export failed (wrapper exit {})", status);
    }
    if !web_built && !args.ssg_static {
        eprintln!(
            "  ⚠ no `--web` in this build — emitted pages reference the wasm bundle for \
             hydration but it isn't staged in this run. Run `idealyst build --web{}` to stage \
             it, or re-run with `--ssg-static` for a no-hydration pure-static export.",
            if args.release { " --release" } else { "" },
        );
    }
    Ok(())
}

fn build_serverless_lambda_target(dir: &std::path::Path, args: &Args) -> Result<()> {
    let source = crate::framework_source::resolve(dir)?;
    let arch = build_serverless_lambda::Arch::parse(args.arch.as_deref())?;
    let artifact = build_serverless_lambda::build(
        dir,
        build_serverless_lambda::BuildOptions {
            release: args.release,
            arch,
            source,
            user_features: Vec::new(),
        },
    )?;
    eprintln!(
        "[build serverless-lambda] success ({}) → {}",
        arch.as_str(),
        artifact.bootstrap.display(),
    );
    eprintln!(
        "  staged deploy/test context → {}",
        artifact.deploy_dir.display(),
    );
    eprintln!(
        "  local (real image via RIE): docker build --platform {plat} -t {name} {dir} && \
         docker run --rm --platform {plat} -p 9000:8080 {name}",
        plat = if matches!(arch, build_serverless_lambda::Arch::Arm64) {
            "linux/arm64"
        } else {
            "linux/amd64"
        },
        name = "idealyst-lambda",
        dir = artifact.deploy_dir.display(),
    );
    eprintln!("  deploy: cargo lambda deploy (see docs/serverless.md)");
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

#[cfg(test)]
mod tests {
    use super::resolve_prune_data_min;

    /// Regression for the release `data-prune` corruption: the unsound
    /// chunk-only classification must NOT run by default. It corrupted
    /// `main.wasm` (zeroed main-reachable fonts / lazy-dispatch data) because
    /// its reachability walk misses data→data / call_indirect / deferred-
    /// registration edges. Off unless the app explicitly opts in.
    ///
    /// A tighter end-to-end test would need to build a wasm fixture with
    /// indirect main→data reachability and diff the pruned bytes — not
    /// reachable from a CLI unit test — so this pins the default/opt-in gate,
    /// the layer the fix actually changed.
    #[test]
    fn data_prune_is_off_by_default_and_opt_in() {
        // release, no flags → OFF (the fix: was Some(24), which corrupted main)
        assert_eq!(resolve_prune_data_min(true, false, false), None);
        // explicit opt-in → ON
        assert_eq!(resolve_prune_data_min(true, true, false), Some(24));
        // opt-in but also --no-data-prune → OFF (no-prune wins)
        assert_eq!(resolve_prune_data_min(true, true, true), None);
        // debug never prunes, even with the opt-in
        assert_eq!(resolve_prune_data_min(false, true, false), None);
        // the deprecated --no-data-prune alone is a harmless no-op (already off)
        assert_eq!(resolve_prune_data_min(true, false, true), None);
    }
}
