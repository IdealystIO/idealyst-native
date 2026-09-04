//! Web build orchestration for `idealyst build web` and the dev
//! server.
//!
//! **No wrapper crate.** Unlike `crates/build/ios/` and
//! `crates/build/android/`, which still generate an ephemeral crate to
//! carry the platform entry point, the web target builds the app crate
//! *itself*. The app owns a `src/main.rs` holding one
//! `idealyst::entry!(<lib>)` line, and the `idealyst` facade carries the
//! wasm-only deps (`backend-web`, `wasm-bindgen`,
//! `console_error_panic_hook`, `lol_alloc`) the author used to have to
//! name. See [`ensure_entry_point`], which reports a missing one in the
//! author's terms.
//!
//! That removes the failure mode the wrapper existed to create: the
//! wrapper resolved `runtime-core` independently of the app, and any
//! disagreement produced two `runtime_core` crates and "expected
//! `Element`, found `Element`" at a boundary the author never wrote.
//!
//! What's left here is packaging, which `cargo build` does not do:
//! `cargo build --target wasm32-unknown-unknown` against the app,
//! then `wasm-bindgen` over the resulting `.wasm`, then `wasm-split`
//! (unless `--no-split`, which is refused when the app has lazy
//! boundaries — see [`BuildOptions::wasm_split`]), then (release)
//! `wasm-opt`, then staging `pkg/` + `index.html` + assets into
//! `dist/web`.
//!
//! ```text
//! <workspace>/target/idealyst/<project>/web/
//! ```
//! is now a staging directory for that output, not a crate.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use build_ios::{
    font_preload_tags, inject_into_head, parse_manifest, remap_path_flags, FrameworkSource,
};

mod premint;
pub use premint::PREMINT_CSS_NAME;
use flate2::write::GzEncoder;
use flate2::Compression;

#[derive(Clone, Debug)]
pub struct BuildOptions {
    /// Build in release mode (`wasm-pack build --release`). Default:
    /// debug (`--dev`), which skips wasm-opt and keeps debug info.
    pub release: bool,
    /// Where the wrapper Cargo.toml should source framework crates
    /// from. The CLI constructs this with `FrameworkSource::detect`
    /// before invoking `build()`.
    pub source: FrameworkSource,
    /// Compile the runtime style engine OUT (`--premint-only`).
    ///
    /// Implies [`Self::premint`]. Sets `--cfg idealyst_premint_only`, which
    /// strips `attach_style`'s live-engine arms and `repeat`'s sheet-batching
    /// arm — the only two paths reaching sheet registration, the token
    /// cohort, and `StyleRules` → CSS.
    ///
    /// `--premint` alone cannot drop the engine: the `stylesheet!` builder's
    /// preminted fast path falls through to `Sheet`/`SheetDynamic` for any
    /// reactive or override-carrying application, so the engine stays named
    /// and linked even when every class preminted.
    ///
    /// A PROMISE, not something the build can verify: a style that still
    /// needs the engine panics at mount naming the offending shape.
    pub premint_only: bool,
    /// Diagnose what blocks [`Self::premint_only`] (`--premint-report`).
    ///
    /// Implies [`Self::premint`] and sets `--cfg idealyst_premint_report`.
    /// KEEPS the engine, so the app renders normally and one page load
    /// lists every style that fell through to it — instead of the boot
    /// panic `--premint-only` gives you at the first offender, which names
    /// the shape but not the source.
    ///
    /// Each distinct fall-through logs once to the console, with the
    /// sheet's premint class (or `NONE-no-build-time-css`), the
    /// runtime-valued layers that disqualified it, and a content
    /// fingerprint of the resolved rules to locate it in source.
    pub premint_report: bool,
    /// How much debug information a DEV build's wasm carries
    /// (`--debuginfo`). Ignored on release, which sets its own
    /// `debug = "limited"` for wasm-split's benefit. See [`DebugInfo`].
    pub debuginfo: DebugInfo,
    /// Optimization posture for DEV builds (`--dev-opt`). Ignored for
    /// `release`, which has its own fixed profile. See [`DevOpt`] for the
    /// measured trade-off and why `Fast` is the default.
    pub dev_opt: DevOpt,
    /// Which builtin primitives the bundle registers (`--primitives`).
    ///
    /// `None` keeps `runtime_vocabulary::AllBuiltins`, the historical
    /// behavior. `Some(list)` makes the wrapper declare a
    /// `builtin_set!` of exactly those names, so every unlisted
    /// primitive's handler is never reached from the boot entry and
    /// LLVM drops it — along with the web-sys imports and JS glue it
    /// alone reached. Measured on a `view`+`text` app: 195,255 →
    /// 126,813 bytes brotli, 236 → 163 wasm imports.
    ///
    /// Validated by [`resolve_primitive_set`] before the wrapper is
    /// written, so a typo is a CLI error rather than a macro error
    /// inside generated code.
    pub primitives: Option<Vec<String>>,
    /// Cargo features to enable on the **user crate** (e.g.
    /// `["dev-hot-reload"]` for runtime-server-mode hot reload). The wrapper's
    /// Cargo.toml grows a parallel `[features]` block that forwards
    /// each named feature to the user-crate dep, and wasm-pack runs
    /// with `-- --features <list>` so those features are active.
    /// Empty means "default features" — the common case.
    pub user_features: Vec<String>,
    /// When `Some`, after the normal in-project `pkg/` sync also stage
    /// a self-contained static-site bundle at this path. The bundle
    /// contains `index.html`, the fresh `pkg/`, and every top-level
    /// asset directory the user keeps in their project root (anything
    /// that isn't `src/`, `target/`, `tests/`, Cargo metadata, or a
    /// dotfile). When `None`, the bundle step is skipped — the build
    /// behaves exactly as before.
    pub bundle_out_dir: Option<PathBuf>,
    /// Robot relay to advertise to the browser app, injected into the
    /// staged `index.html` as `window.IDEALYST_ROBOT_RELAY_URL`.
    ///
    /// Only the STAGED path can carry this: a full-stack project's own
    /// server hands out `dist/web/index.html` as a plain file, so there
    /// is no serve-time head injection to hook (the CLI's `dev-http`
    /// static server, which a full-stack project never starts, injects
    /// it there instead). Ignored when `bundle_out_dir` is `None`.
    ///
    /// `None` for every deploy build — a staged bundle must never ship
    /// a dev machine's relay port.
    pub robot_relay_url: Option<String>,
    /// Pre-gzip every text-ish file in the staged bundle, writing
    /// gzipped bytes under the original filename. Only meaningful
    /// when `bundle_out_dir` is `Some`; ignored otherwise. The static
    /// host must send `Content-Encoding: gzip` on these responses for
    /// the browser to inflate them transparently.
    pub gzip: bool,
    /// Emit precompressed `.br` SIBLINGS (`foo.wasm` → `foo.wasm.br`,
    /// original kept) for every compressible file in the staged
    /// bundle, using brotli q11. Only meaningful when
    /// `bundle_out_dir` is `Some`. Unlike `gzip` (which rewrites
    /// bytes in place for fixed-header hosts), siblings are the
    /// standard static-host shape: nginx `brotli_static on`, Caddy
    /// `file_server precompressed`, and CDN edges serve the `.br`
    /// when the client sends `Accept-Encoding: br` and fall back to
    /// the original otherwise. Runs BEFORE the in-place gzip pass so
    /// the `.br` is always encoded from the original bytes. The CLI
    /// defaults this ON for release bundles (brotli q11 beats
    /// gzip -9 by ~20% on wasm); `--no-brotli` opts out.
    pub brotli: bool,
    /// Strip panic machinery (`-Z build-std-features=panic_immediate_abort`).
    /// Every panic becomes a bare `unreachable` trap with no message.
    /// Requires a nightly toolchain + the `rust-src` component and
    /// recompiles std from source. Only honored alongside `release`
    /// (the CLI flips `release` on when this is set). The dev loop
    /// always leaves this `false`.
    pub strip_panics: bool,
    /// Enable `backend-web/hydrate`. Compile in the in-place hydration
    /// machinery (cursor + remount bookkeeping + per-primitive
    /// `hydrate_next` paths + the divergence-diagnostic) so the bundle
    /// can adopt SSR/SSG HTML on boot instead of clearing it. SPA-only
    /// builds (`idealyst build --web` without `--ssg`/`--ssr`) leave
    /// this `false` to shave the machinery out of the wasm. Set to
    /// `true` when SSG/SSR is being built alongside web — the
    /// CLI does this automatically.
    pub hydrate: bool,
    /// Zero chunk-only data symbols `>= min_bytes` in the main bundle.
    /// `None` disables. `Some(min)` recovers significant bytes
    /// (≈400 KB gzipped on a wgpu-sim-bearing app like the website)
    /// when there's a heavy lazy chunk that pulled big static tables
    /// into the wasm. `min` matters: the heuristic call graph
    /// misclassifies small vtables as chunk-only and zeroing them
    /// triggers null-function traps at runtime. `Some(24)` is the
    /// verified-safe floor on the website example; the CLI defaults
    /// to that for release web builds.
    pub prune_dead_data_min: Option<usize>,
    /// Preminted styles: run the ephemeral native style-dump build
    /// (every `stylesheet!` in the app graph emits its full variant
    /// space as CSS into `pkg/premint.css`) and compile the wasm with
    /// `--cfg idealyst_premint`, so all-constant style applications
    /// ship as build-time class references instead of invoking the
    /// runtime style engine. Pair with `default-features = false` on
    /// backend-web (dropping `style-dynamic`) to remove the engine
    /// from the bundle entirely. Opt-in while the parity soak is
    /// running. Composes with `hydrate`: the SSR/SSG server is built
    /// with the same premint cfgs (build-ssr), so both sides stamp
    /// identical `iy-*` classes and adoption stays clean.
    pub premint: bool,
    /// Run `wasm-split` over the bindgened module. **Default: `true`**
    /// — `--no-split` is the opt-out.
    ///
    /// Off does not remove lazy boundaries; it declines to *extract*
    /// them. The bodies ship in the main module and their loaders resolve
    /// on a microtask instead of a network round trip, so a
    /// `#[component(lazy)]` still mounts (its `loading` state just barely
    /// flashes). See [`write_inline_split_loader`] for how the imports
    /// the macro emitted get answered without a chunk.
    ///
    /// It is a trade, not a free win, because outside release the
    /// splitter is ALSO the only pass that compacts the module (there is
    /// no `wasm-opt` there): it rebuilds the module and drops the
    /// `--emit-relocs` payload and unreachable code along the way.
    /// Measured on `examples/welcome`, which has no split points at all:
    ///
    /// ```text
    /// split   : 1.69s   welcome_bg.wasm = 2,249,884 B
    /// no-split: 1.51s   welcome_bg.wasm = 6,300,179 B
    /// ```
    ///
    /// So it buys packaging time and costs bundle size — and on a big app
    /// the larger module also costs the browser more to compile on every
    /// reload, which can eat the win. Which side wins depends on the app,
    /// hence a flag rather than a heuristic.
    pub wasm_split: bool,
}

/// How much debug information the wasm carries.
///
/// wasm has no split-debuginfo: DWARF cannot live in a sidecar file, so
/// every byte of it ships inside the module and every post-cargo pass
/// (wasm-bindgen, wasm-split, staging, and the browser's own compile on
/// reload) pays for it. Cargo's dev default is `debug = 2`, which makes a
/// dev bundle carry MORE debug info than the release profile's
/// `"limited"` — measured on `websites/website`, 173 MB of a 298 MB debug
/// module, and 7.7s of a single wasm-bindgen run.
///
/// Panic *messages* keep their `file:line` at every level here: those are
/// `#[track_caller]` `Location` strings in live `.rodata`, not debug info
/// (the release path's `remap_path_flags` exists precisely because
/// `wasm-opt --strip-debug` can't reach them). What the levels change is
/// what a DWARF-aware debugger can do with a *stack frame*.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DebugInfo {
    /// Line tables only — stack frames still symbolize to source lines,
    /// but locals and types are gone. The default for dev builds: it is
    /// what makes a panic trace readable, at a fraction of the bytes.
    #[default]
    LineTables,
    /// Cargo's dev default (`debug = 2`). Full DWARF — variable and type
    /// inspection in a wasm-aware debugger. `--debuginfo full`.
    Full,
    /// No debug info at all. Panic messages still name their source
    /// location; stack frames do not symbolize. `--debuginfo none`.
    None,
}

impl DebugInfo {
    /// The value cargo's `debug` profile key takes.
    fn cargo_value(self) -> &'static str {
        match self {
            DebugInfo::LineTables => "\"line-tables-only\"",
            DebugInfo::Full => "2",
            DebugInfo::None => "0",
        }
    }

    /// Parse the `--debuginfo` CLI value.
    pub fn from_cli(value: &str) -> Result<Self> {
        match value {
            "line-tables" | "line-tables-only" => Ok(DebugInfo::LineTables),
            "full" => Ok(DebugInfo::Full),
            "none" => Ok(DebugInfo::None),
            other => anyhow::bail!(
                "unknown --debuginfo `{other}` (expected `line-tables`, `full`, or `none`)"
            ),
        }
    }
}

/// Optimization posture for DEV (non-release) web builds.
///
/// [`Optimized`][Self::Optimized] is the default and is what you want
/// almost always. [`Fast`][Self::Fast] exists for one specific regime,
/// documented below, and is a net loss outside it.
///
/// # Why the obvious change isn't a win
///
/// `profile.dev.package."*"` does NOT match workspace members (cargo:
/// "for any non-workspace member"), so under `Optimized` every framework
/// crate compiles at whatever the workspace `[profile.dev]` says —
/// `opt-level = "z"` in this repo. Dropping members to `opt-level = 0`
/// looks like it should make a framework-crate edit much cheaper, and in
/// isolated `cargo build` timings it does.
///
/// It loses anyway, because cargo is not the whole build. `Fast` inflates
/// the module by ~37% (41.9 MB vs 30.5 MB on `charts-demo`), and
/// wasm-bindgen and wasm-split are both O(module size) and run on every
/// rebuild. End-to-end through the CLI, warm target dir, two rounds:
///
/// | edit | `Fast` | `Optimized` |
/// |---|---|---|
/// | framework crate | 8.66s / 6.87s | **4.52s / 5.70s** |
/// | leaf app crate | 4.90s / 5.63s | **3.37s / 3.16s** |
///
/// The cargo half is close (3.0-6.0s vs 2.9-4.1s); the post-cargo passes
/// are what decide it (~2.5s vs ~1.4s). This is also why the older
/// leaf-edit measurement recorded in [`profile_config_args`] still holds.
///
/// # When `Fast` does win
///
/// With incremental compilation OFF, the cargo half stops being close —
/// a framework-crate edit measured 7.33s under `Fast` vs 14.19s under
/// `Optimized` — and `Fast` wins comfortably. That is not the default
/// regime, but it is the regime you are in after a `cargo clean`, with
/// `CARGO_INCREMENTAL=0` exported, or on a machine where the incremental
/// cache has been disabled for disk reasons.
///
/// Note that a *poisoned* incremental cache looks like this too but is a
/// different problem with a different fix: the cache degrades when one
/// target dir is shared across build configurations that keep evicting
/// each other, which is what [`config_key`] now prevents by giving each
/// configuration its own directory. Reach for `--dev-opt fast` only after
/// confirming the target dir is healthy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DevOpt {
    /// Workspace members inherit the workspace's own `[profile.dev]`,
    /// registry dependencies at `opt-level = 3`. Default: smaller module,
    /// and the post-cargo passes that run on every rebuild are cheaper
    /// because of it.
    #[default]
    Optimized,
    /// App + framework crates at `opt-level = 0`, registry dependencies
    /// at `1`. Cheaper cargo, ~37% larger module. Wins only when
    /// incremental compilation is off — see the type docs.
    /// `--dev-opt fast`.
    Fast,
}

impl DevOpt {
    /// Stable identity for [`config_key`].
    ///
    /// MUST NOT be the enum discriminant. Hashing `self as u8` ties the
    /// target-dir key to declaration ORDER, so reordering the variants
    /// silently hands one posture the other's directory — which is a
    /// full, unexplained rebuild of the framework the next time either is
    /// used, i.e. exactly the failure `config_key` exists to prevent.
    /// (This is not hypothetical: it happened when `Optimized` was
    /// promoted to the default and moved to the front.)
    fn key_tag(self) -> &'static str {
        match self {
            DevOpt::Optimized => "opt-optimized",
            DevOpt::Fast => "opt-fast",
        }
    }

    /// Parse the `--dev-opt` CLI value.
    pub fn from_cli(value: &str) -> Result<Self> {
        match value {
            "fast" => Ok(DevOpt::Fast),
            "optimized" => Ok(DevOpt::Optimized),
            other => anyhow::bail!("unknown --dev-opt `{other}` (expected `fast` or `optimized`)"),
        }
    }
}

/// The wasm import-module string `#[wasm_split]` (what
/// `#[component(lazy)]` expands to) names for its loader, and therefore
/// the marker that tells us a module has split points at all.
const WASM_SPLIT_LOADER: &str = "./__wasm_split.js";

#[derive(Debug)]
pub struct BuildArtifact {
    /// Path to the generated `pkg/` directory inside the user project
    /// (NOT inside the wrapper). The dev server / static serve points
    /// here.
    pub pkg_dir: PathBuf,
    /// Path to the generated wrapper crate. Useful for debugging and
    /// for a future `idealyst scaffold web` command.
    pub wrapper_dir: PathBuf,
    /// Path to the staged static-site bundle, when `bundle_out_dir`
    /// was set on the build options. `None` otherwise.
    pub bundle_dir: Option<PathBuf>,
    /// File name (within the bundle's `pkg/`) of the content-hashed
    /// entry-point JS shim, e.g. `website.3f9a12bc44d0e1a7.js`. Set
    /// when a bundle was staged (staging fingerprints the pkg);
    /// `None` on the dev-loop path, whose pkg keeps plain names and
    /// is served with `Cache-Control: no-store`.
    pub entry_js: Option<String>,
}

/// The primitives `--primitives` accepts — one per method on
/// `runtime_vocabulary::BuiltinSet`.
///
/// Kept in sync with that trait (and with `builtin_set_keep!`'s arms) by
/// `primitive_names_match_the_vocabulary_trait` below. Drift is not
/// silent either way: a name here that the macro lacks fails to compile
/// in the generated wrapper.
pub const SELECTABLE_PRIMITIVES: &[&str] = &[
    "view",
    "text",
    "button",
    "pressable",
    "image",
    "icon",
    "link",
    "toggle",
    "slider",
    "activity_indicator",
    "text_input",
    "text_area",
    "scroll_view",
    "repeat",
    "lazy",
    "virtualizer",
    "graphics",
    "portal",
    "presence",
    "nav",
];

/// Presets accepted in place of an explicit list.
const PRIMITIVE_PRESETS: &[(&str, &[&str])] = &[
    // "Just the core framework": the reactive kernel, scene model, style
    // engine and backend, with nothing composable on top.
    ("core", &["view", "text"]),
    ("all", SELECTABLE_PRIMITIVES),
];

/// Normalize `--primitives` into the concrete keep-list the wrapper
/// declares, or `None` for "every builtin" (the flag's absence).
///
/// Accepts a preset (`core`, `all`) or an explicit comma-separated list.
/// An unknown name is rejected here, with the valid set in the message —
/// the alternative is a `no rules expected this token` error pointing
/// into a generated file the user never wrote.
pub fn resolve_primitive_set(spec: Option<&[String]>) -> Result<Option<Vec<String>>> {
    let Some(spec) = spec else { return Ok(None) };

    let requested: Vec<String> = spec
        .iter()
        .flat_map(|s| s.split(','))
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    if requested.is_empty() {
        anyhow::bail!(
            "--primitives was given no names. Pass a preset (`core`, `all`) or a \
             comma-separated list, e.g. `--primitives view,text,button`. Omit the \
             flag entirely to register every builtin."
        );
    }

    if requested.len() == 1 {
        if let Some((_, preset)) = PRIMITIVE_PRESETS
            .iter()
            .find(|(name, _)| *name == requested[0])
        {
            return Ok(Some(preset.iter().map(|s| s.to_string()).collect()));
        }
    }

    let mut keep: Vec<String> = Vec::new();
    for name in requested {
        if !SELECTABLE_PRIMITIVES.contains(&name.as_str()) {
            anyhow::bail!(
                "--primitives: `{name}` is not a builtin primitive.\n\n\
                 Valid names: {}\n\
                 Presets: {}\n\n\
                 Note `overlay` / `anchored_overlay` are compositions over \
                 `portal`, and `flat_list` is `virtualizer` — select those \
                 spellings instead.",
                SELECTABLE_PRIMITIVES.join(", "),
                PRIMITIVE_PRESETS
                    .iter()
                    .map(|(n, _)| *n)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        if !keep.contains(&name) {
            keep.push(name);
        }
    }
    Ok(Some(keep))
}

/// Build the user's project at `project_dir` for the web target.
///
/// Builds the app crate's own binary for `wasm32-unknown-unknown`, runs
/// `wasm-bindgen` (plus `wasm-split` + `wasm-opt` on release) over the
/// result, and stages the `pkg/` bundle into `project_dir/pkg/` and
/// `dist/web`. No wrapper crate is involved — see the module docs.
pub fn build(project_dir: &Path, opts: BuildOptions) -> Result<BuildArtifact> {
    let project_dir = fs::canonicalize(project_dir)
        .with_context(|| format!("resolve project dir {}", project_dir.display()))?;
    let manifest = parse_manifest(&project_dir)?;
    // `--primitives` used to generate a `builtin_set!` into the wrapper.
    // There is no wrapper to generate into: the set is now declared in
    // the manifest, where `idealyst::entry!` reads it. Fail loudly
    // rather than silently drop the trim — it was worth ~35% of the
    // bundle on a small app.
    if let Some(list) = opts.primitives.as_deref() {
        anyhow::bail!(
            "`--primitives` is no longer a build flag — the primitive set is \
             part of the app's configuration now. Move it into the app's \
             Cargo.toml:\n\n    \
             [package.metadata.idealyst.app]\n    \
             primitives = [{}]\n\n\
             `idealyst::entry!` reads it and declares the `builtin_set!` in \
             your own crate.",
            list.iter()
                .map(|p| format!("\"{p}\""))
                .collect::<Vec<_>>()
                .join(", "),
        );
    }

    // The app crate is the artifact. `bin_name` is the *binary* target's
    // name, which keeps hyphens (unlike `lib_name`, which cargo
    // underscores) because that's what rustc writes to disk.
    let bin_name = manifest.name.clone();
    ensure_entry_point(&project_dir, &bin_name)?;

    // Per-app staging: `pkg/` output and the premint dump wrapper. No
    // longer holds a generated crate.
    let build_dir = opts
        .source
        .wrapper_root(&project_dir)
        .join(&manifest.name)
        .join("web");
    // The wasm build gets its own target dir, KEYED BY the build config.
    // Not isolation for its own sake: it carries RUSTFLAGS the app's
    // native builds don't (`--emit-relocs`, `+simd128`, the premint
    // cfgs), so sharing a target dir with `cargo check` would invalidate
    // one fingerprint every time the other ran. Shared across apps under
    // one framework source so sibling apps still reuse dependency builds.
    //
    // The config key is what stops the far nastier version of the same
    // problem — see [`config_key`].
    let target_dir = opts
        .source
        .cargo_target_dir(&project_dir)
        .join(format!("idealyst-web-{}", config_key(&opts)));
    eprintln!(
        "[build-web] target dir: {} ({})",
        target_dir.display(),
        config_summary(&opts),
    );

    // Direct pipeline (no wasm-pack), so we can hit the flag matrix
    // wasm-split-cli needs to actually extract chunks:
    //
    //   1. `cargo build` with `-C link-args=--emit-relocs` (passed via
    //      `CARGO_ENCODED_RUSTFLAGS`) so the rustc-emitted wasm has the
    //      relocation info wasm-split needs to rewrite indirect calls.
    //   2. `wasm-bindgen --keep-lld-exports` so wasm-bindgen preserves
    //      the LLD-emitted exports wasm-split's reachability walker
    //      uses to identify chunk-only code.
    //   3. `wasm-split-cli split` rewrites the bindgened wasm into a
    //      lean base + per-chunk wasms + a `__wasm_split.js` loader.
    //      Skippable with `--no-split`. Not skipped by default even
    //      when there is nothing to extract: outside release this is
    //      also the only pass that compacts the module, so skipping it
    //      trades packaging time for a much larger served wasm.
    //   4. `wasm-opt -Oz` runs LAST, per-file, on the base + every
    //      chunk. wasm-pack ran it BEFORE wasm-bindgen which mangled
    //      symbols wasm-split needed — that's why my earlier
    //      website measurements showed 0 KB chunks even when the
    //      lazy body was clearly extractable.
    let wrapper_pkg = build_dir.join("pkg");
    let original_wasm = target_dir
        .join("wasm32-unknown-unknown")
        .join(if opts.release { "release" } else { "debug" })
        .join(format!("{bin_name}.wasm"));
    // premint × hydrate COMPOSES now: the SSR/SSG server binary is built
    // with the same `--cfg idealyst_premint*` posture (build-ssr injects
    // the cfgs and the wrapper links premint.css + arms the minted-class
    // guard), so server and client stamp identical deterministic `iy-*`
    // classes and hydration's `classList.add` re-stamp is a no-op. The
    // historical bail here refused the combination back when the server
    // could only emit live-minted classes.
    let mut timings = BuildTimings::default();
    timings.time("cargo", || {
        cargo_build_wasm(
            &project_dir,
            &bin_name,
            &target_dir,
            opts.release,
            opts.wasm_split,
            opts.debuginfo,
            opts.dev_opt,
            opts.strip_panics,
            opts.premint,
            opts.premint_only,
            opts.premint_report,
            opts.hydrate,
            &opts.user_features,
            &opts.source,
            &project_dir,
        )
    })?;
    timings.time("wasm-bindgen", || {
        wasm_bindgen_build(&original_wasm, &wrapper_pkg, &manifest.lib_name)
    })
    .with_context(|| "wasm-bindgen")?;
    timings.time("command-export-neutralize", || {
        neutralize_command_export_wrappers(&wrapper_pkg, &manifest.lib_name)
    })
    .with_context(|| "wasm-bindgen command_export neutralize")?;
    if opts.wasm_split {
        timings.time("wasm-split", || {
            run_wasm_split(
                &original_wasm,
                &wrapper_pkg,
                &manifest.lib_name,
                opts.prune_dead_data_min,
            )
        })
        .with_context(|| "wasm-split-cli post-build")?;
    } else {
        // `--no-split` means "bundle it anyway", not "drop it": the module
        // still declares whatever imports `#[wasm_split]` emitted, and an
        // unsatisfied import is a module the browser refuses to
        // instantiate. So read them back off the module and answer them
        // locally.
        let bindgened = wrapper_pkg.join(format!("{}_bg.wasm", manifest.lib_name));
        let imports = wasm_split_imports(
            &fs::read(&bindgened).with_context(|| format!("read {}", bindgened.display()))?,
        )
        .with_context(|| "wasm-split: scan for split-loader imports")?;
        // Stale chunks + loader from an earlier splitting build go FIRST —
        // `fingerprint_pkg` digests every file under pkg/ and `stage_bundle`
        // copies them, so an orphan would ship. This also clears the path
        // the inline loader is about to be written to.
        clear_wasm_split_artifacts(&wrapper_pkg)
            .with_context(|| "wasm-split: clear stale chunk artifacts")?;
        if imports.is_empty() {
            eprintln!(
                "[build-web] wasm-split: skipped (--no-split); no lazy boundaries, \
                 {}_bg.wasm keeps its relocs and is correspondingly larger",
                manifest.lib_name,
            );
        } else {
            let inlined =
                write_inline_split_loader(&wrapper_pkg, &manifest.lib_name, &imports)?;
            eprintln!(
                "[build-web] wasm-split: skipped (--no-split); {inlined} lazy \
                 boundary(ies) stay in {}_bg.wasm and resolve immediately",
                manifest.lib_name,
            );
        }
    }
    if opts.release {
        timings
            .time("wasm-opt", || wasm_opt_pkg(&wrapper_pkg))
            .with_context(|| "wasm-opt post-split")?;
    }

    if opts.premint {
        // Native dump build → `pkg/premint.css`. Written into the
        // wrapper pkg BEFORE staging/sync so both the staged bundle and
        // the in-project `pkg/` carry it, and before `fingerprint_pkg`
        // so it gets content-addressed with the rest of the bundle.
        let css = timings
            .time("premint-dump", || {
                premint::generate_and_run_dump(
                    &build_dir,
                    &project_dir,
                    &opts.source,
                    &manifest,
                )
            })
            .with_context(|| "premint style dump")?;
        fs::write(wrapper_pkg.join(premint::PREMINT_CSS_NAME), &css).with_context(|| {
            format!("write {}", wrapper_pkg.join(premint::PREMINT_CSS_NAME).display())
        })?;
        eprintln!(
            "[build-web] premint: {} bytes of preminted CSS → pkg/{}",
            css.len(),
            premint::PREMINT_CSS_NAME,
        );
    }

    let stage_start = std::time::Instant::now();
    let (pkg_dir, bundle_dir, entry_js) = if let Some(out) = opts.bundle_out_dir.as_ref() {
        let default_index = default_index_html(&manifest.app.name, &manifest.lib_name);
        let staged = stage_bundle(
            &project_dir,
            out,
            Some(&default_index),
            &manifest.app.web.assets,
        )
        .with_context(|| format!("stage static bundle at {}", out.display()))?;
        let staged_pkg = staged.join("pkg");
        sync_pkg_dir(&wrapper_pkg, &staged_pkg).with_context(|| {
            format!("sync {} → {}", wrapper_pkg.display(), staged_pkg.display())
        })?;
        strip_wasm_pack_metadata(&staged_pkg);
        // Content-address the bundle: every pkg file is renamed to
        // carry the build digest and index.html is pointed at the
        // hashed entry, so a redeploy can never be half-served from a
        // stale HTTP cache. Runs after metadata stripping (so junk
        // files don't feed the digest) and before gzip (which rewrites
        // bytes in place under the final names).
        let fp = fingerprint_pkg(&staged_pkg, &manifest.lib_name)
            .with_context(|| format!("fingerprint {}", staged_pkg.display()))?;
        rewrite_index_bundle_ref(&staged.join("index.html"), &manifest.lib_name, &fp.entry_js)?;
        // Link the preminted stylesheet (content-addressed by the
        // fingerprint pass above). A plain <link> — the browser needs
        // the rules before first paint anyway, and pkg/ is immutable
        // so it caches like the wasm.
        if let Some(css_name) = &fp.premint_css {
            let css = fs::read_to_string(staged_pkg.join(css_name))
                .with_context(|| format!("read staged {css_name}"))?;
            inject_premint_css_link(
                &staged.join("index.html"),
                css_name,
                &premint_font_families(&css),
            )?;
        }
        // Rewrite the staged `index.html` to preload the project's
        // declared fonts. Has to run BEFORE `gzip_bundle` (which
        // overwrites `index.html` with gzipped bytes) so the gzipped
        // copy carries the preload tags. No-op when the user hasn't
        // declared `[package.metadata.idealyst.app.web].preload_fonts`.
        inject_font_preloads_into_staged_index(
            &staged.join("index.html"),
            &manifest.app.web.preload_fonts,
        )?;
        // Robot-on-web for a full-stack project. The app reads this
        // global at boot (`idealyst::boot::web`) and dials the relay,
        // which is what puts a web app in front of the MCP server /
        // inspector / evaluators. Same `</head>` splice and the same
        // pre-gzip ordering as the injections above.
        stage_robot_relay_url(&staged.join("index.html"), opts.robot_relay_url.as_deref())?;
        // Stage any EXTERNAL dirs the app links in (e.g. a component
        // library's `fonts/`), copied under their final path component so
        // `../whiteboard/fonts` → `<bundle>/fonts/`. Lets a library own the
        // font files (native `include_bytes!`) while the app serves them on
        // web — no per-app copy or symlink.
        stage_external_dirs(&project_dir, &staged, &manifest.app.web.font_dirs)?;
        // Generate the favicon set into the staged bundle and inject
        // the corresponding `<link>` tags into `index.html`. Driven
        // by `[package.metadata.idealyst.app.icon].source`; no-op
        // when the icon block is absent. Has to run AFTER fonts
        // (independent concerns, but both must finish before gzip)
        // and BEFORE gzip for the same reason.
        sync_and_inject_web_icons(&project_dir, &staged)?;
        if opts.brotli {
            brotli_precompress_bundle(&staged)
                .with_context(|| format!("brotli-precompress bundle at {}", staged.display()))?;
        }
        if opts.gzip {
            gzip_bundle(&staged).with_context(|| format!("gzip bundle at {}", staged.display()))?;
        }
        (staged_pkg, Some(staged), Some(fp.entry_js))
    } else {
        let project_pkg = project_dir.join("pkg");
        sync_pkg_dir(&wrapper_pkg, &project_pkg).with_context(|| {
            format!("sync {} → {}", wrapper_pkg.display(), project_pkg.display())
        })?;
        if opts.premint {
            // No staging → no index rewriting; the project's own
            // index.html must link the sheet itself.
            eprintln!(
                "[build-web] premint: add <link rel=\"stylesheet\" \
                 href=\"pkg/{}\"> to your index.html <head>",
                premint::PREMINT_CSS_NAME,
            );
        }
        (project_pkg, None, None)
    };

        timings
        .phases
        .push(("stage+fingerprint", stage_start.elapsed()));
    timings.report();

    Ok(BuildArtifact {
        pkg_dir,
        // No longer a generated crate — the per-app staging dir that
        // holds `pkg/` and the premint dump. Field name kept so the
        // display lines in `cmd/build.rs` don't churn.
        wrapper_dir: build_dir,
        bundle_dir,
        entry_js,
    })
}

/// Stage a deployable static-site bundle at `out_dir`. `pkg/` is
/// populated separately by the caller, straight from the wasm-pack
/// output dir — that way the project root never has to carry a `pkg/`
/// for the bundle's sake. `out_dir` is fully cleared first so stale
/// files from a prior bundle (renamed wasm, removed assets) never
/// linger.
///
/// # What ships
///
/// Staging is **safe by default** — internal docs, configs, and source
/// must never end up served at the public site root:
///
/// - **`assets` non-empty (allowlist, explicit-is-safe)**: ONLY the
///   declared top-level entries are copied, plus `index.html`. `pkg/`
///   and the icon set are emitted by the build later, so they don't
///   need to be listed. Anything not named is skipped — a leak is
///   impossible regardless of what sits in the project root.
/// - **`assets` empty (tightened denylist fallback)**: top-level entries
///   auto-ship EXCEPT source trees, build outputs, VCS/IDE metadata,
///   and — critically — docs (`*.md`, `README*`, `LICENSE*`,
///   `FEEDBACK*`), configs (`*.toml`, `*.lock`, `*.log`), and the
///   `design-files/` folder. Real web assets (`assets/`, `public/`,
///   `fonts/`, `robots.txt`, images, css) still ship. See
///   [`is_excluded_from_bundle`].
///
/// When the project supplies no `index.html`, `fallback_index` decides
/// what happens: `Some(html)` writes that HTML into the *staged* bundle
/// as `index.html` (the project source tree is never touched), so a
/// project doesn't have to hand-author boilerplate just to be served;
/// `None` errors (there's nothing to serve). Production builds pass the
/// generated default (see [`default_index_html`]); a project's own
/// `index.html`, when present, always wins.
///
/// Returns the canonicalized bundle path.
pub fn stage_bundle(
    project_dir: &Path,
    out_dir: &Path,
    fallback_index: Option<&str>,
    assets: &[String],
) -> Result<PathBuf> {
    let index = project_dir.join("index.html");
    let synth_index = if index.is_file() {
        None
    } else {
        match fallback_index {
            Some(html) => Some(html),
            None => anyhow::bail!(
                "cannot stage web bundle: {} missing (a web bundle needs an index.html at the \
                 project root that loads ./pkg/<lib>.js)",
                index.display(),
            ),
        }
    };
    if out_dir.exists() {
        fs::remove_dir_all(out_dir)
            .with_context(|| format!("clear stale bundle {}", out_dir.display()))?;
    }
    fs::create_dir_all(out_dir)
        .with_context(|| format!("create bundle dir {}", out_dir.display()))?;

    if assets.is_empty() {
        // No explicit allowlist: auto-ship the project root through the
        // tightened denylist. `is_excluded_from_bundle` keeps source,
        // build outputs, docs, configs, and VCS/IDE metadata out so
        // nothing internal leaks to the public site root.
        for entry in fs::read_dir(project_dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if is_excluded_from_bundle(&name_str) {
                continue;
            }
            let from = entry.path();
            let to = out_dir.join(&name);
            if from.is_dir() {
                copy_dir(&from, &to)
                    .with_context(|| format!("copy dir {} → {}", from.display(), to.display()))?;
            } else if from.is_file() {
                fs::copy(&from, &to)
                    .with_context(|| format!("copy file {} → {}", from.display(), to.display()))?;
            }
        }
    } else {
        // Explicit allowlist: stage ONLY the declared entries. A
        // declared entry that doesn't exist is silently skipped (e.g.
        // `pkg/` may be listed for clarity but is emitted later by the
        // caller). `index.html` is always staged (handled below /
        // copied here if it exists), never gated by the allowlist —
        // the bundle is unservable without it.
        let mut wanted: Vec<&str> = assets.iter().map(|s| s.as_str()).collect();
        if !wanted.iter().any(|s| *s == "index.html") {
            wanted.push("index.html");
        }
        for entry in wanted {
            // Defend against `..`/absolute escapes — only single
            // top-level names are valid allowlist entries. Anything
            // with a path separator or parent ref is rejected.
            if entry.is_empty()
                || entry.contains("..")
                || entry.contains('/')
                || entry.contains('\\')
            {
                anyhow::bail!(
                    "invalid web `assets` entry {:?}: must be a single project-root file or \
                     folder name (no path separators or `..`)",
                    entry,
                );
            }
            let from = project_dir.join(entry);
            let to = out_dir.join(entry);
            if from.is_dir() {
                copy_dir(&from, &to)
                    .with_context(|| format!("copy dir {} → {}", from.display(), to.display()))?;
            } else if from.is_file() {
                fs::copy(&from, &to)
                    .with_context(|| format!("copy file {} → {}", from.display(), to.display()))?;
            }
        }
    }

    // Synthesize a default index.html into the staged bundle when the
    // project supplied none. Written here (the staged out_dir), never
    // into the project source tree.
    if let Some(html) = synth_index {
        fs::write(out_dir.join("index.html"), html)
            .with_context(|| format!("write default index.html into {}", out_dir.display()))?;
    }

    fs::canonicalize(out_dir).with_context(|| format!("canonicalize {}", out_dir.display()))
}

/// Stage EXTERNAL directories (declared via
/// `[package.metadata.idealyst.app.web].font_dirs`) into the bundle. Each entry
/// is resolved relative to the app crate (and MAY contain `..`, unlike the
/// in-crate `assets` allowlist) and copied to `<bundle>/<final-component>/`, so
/// `../whiteboard/fonts` lands at `<bundle>/fonts/`. The motivating case: a
/// component library owns its typeface's font files (native `include_bytes!`)
/// and a consuming app serves the same files on web without a per-app copy or
/// symlink.
fn stage_external_dirs(project_dir: &Path, staged: &Path, dirs: &[String]) -> Result<()> {
    for d in dirs {
        let src = project_dir.join(d);
        if !src.is_dir() {
            anyhow::bail!(
                "[package.metadata.idealyst.app.web].font_dirs entry {:?} is not a directory \
                 (resolved to {})",
                d,
                src.display(),
            );
        }
        let name = src.file_name().ok_or_else(|| {
            anyhow::anyhow!("font_dirs entry {:?} has no final path component", d)
        })?;
        let dest = staged.join(name);
        copy_dir(&src, &dest)
            .with_context(|| format!("stage {} → {}", src.display(), dest.display()))?;
    }
    Ok(())
}

/// The default `index.html` a web bundle is served with when the project
/// doesn't ship its own. Mounts into `#app` and boots the wasm via
/// `/pkg/<lib_name>.js` — identical in shape to what `idealyst scaffold`
/// writes, so an index-less project behaves the same as a scaffolded one.
/// `lib_name` is the package name with `-` → `_` (matches the emitted
/// `pkg/<lib_name>.js`).
pub fn default_index_html(title: &str, lib_name: &str) -> String {
    format!(
        r##"<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1, user-scalable=no" />
    <base href="/" />
    <title>{title}</title>
    <style>
      html, body, #app {{ height: 100%; margin: 0; }}
      body {{ background: #f7f8fb; }}
      /* Mount is a flex column so the app's root view fills the viewport
         height; without it the root sizes to content and short screens
         stop short of full height on tall windows. */
      #app {{ display: flex; flex-direction: column; }}
      #app > * {{ flex: 1 1 auto; min-height: 0; }}
    </style>
  </head>
  <body>
    <div id="app"></div>
    <script type="module">
      import init from "/pkg/{lib_name}.js";
      init();
    </script>
  </body>
</html>
"##
    )
}

/// Read `index_path`, splice `<link rel="preload">` tags for every
/// font in `paths` right before `</head>`, write it back. No-op when
/// `paths` is empty — most projects don't declare preloads.
///
/// Mirrors the dev-http path: both call the same `font_preload_tags`
/// + `inject_into_head` helpers so the dev loop and the deployed
/// bundle preload the same set from the same TOML list.
/// Splice the preminted stylesheet `<link>` into the staged
/// `index.html` head. Same read-modify-write shape as the font-preload
/// injector below; runs before gzip for the same reason.
///
/// `font_families` (comma-joined) rides a `data-iy-font-families`
/// attribute on the link: the web backend's runtime `register_typeface`
/// reads it to skip families whose `@font-face` already ships in this
/// stylesheet. The attribute — not the parsed stylesheet or the
/// document's `FontFaceSet` — because those race the `<link>`'s async
/// load against wasm boot (observed: 4 fonts double-fetched); a DOM
/// attribute is readable the instant the tag is parsed.
fn inject_premint_css_link(index_path: &Path, css_name: &str, font_families: &str) -> Result<()> {
    let html = fs::read_to_string(index_path)
        .with_context(|| format!("read {}", index_path.display()))?;
    let snippet = format!("\n    {}", premint_css_link_tag(css_name, font_families));
    let out = inject_into_head(html, &snippet);
    fs::write(index_path, out).with_context(|| format!("write {}", index_path.display()))?;
    Ok(())
}

/// The premint stylesheet `<link>` tag itself — shared between the
/// staged-bundle injector above and the dev server's head injection
/// (`idealyst dev --web --local --premint`), so the served-HTML tag and
/// the deployed one carry the same shape (incl. the
/// `data-iy-font-families` dedup attribute; see
/// [`inject_premint_css_link`] for why it's an attribute).
pub fn premint_css_link_tag(css_name: &str, font_families: &str) -> String {
    let families_attr = if font_families.is_empty() {
        String::new()
    } else {
        format!(" data-iy-font-families=\"{font_families}\"")
    };
    format!("<link rel=\"stylesheet\" href=\"pkg/{css_name}\"{families_attr}>")
}

/// The unique `@font-face` family names in an emitted premint
/// stylesheet, comma-joined in appearance order. Scans for the exact
/// prefix `css::font_face_css` emits — the dump and this parser share
/// that format by construction.
pub fn premint_font_families(css: &str) -> String {
    let mut seen: Vec<&str> = Vec::new();
    for chunk in css.split("@font-face{font-family:\"").skip(1) {
        if let Some(end) = chunk.find('"') {
            let fam = &chunk[..end];
            if !seen.contains(&fam) {
                seen.push(fam);
            }
        }
    }
    seen.join(",")
}

fn inject_font_preloads_into_staged_index(index_path: &Path, paths: &[String]) -> Result<()> {
    let snippet = font_preload_tags(paths);
    if snippet.is_empty() {
        return Ok(());
    }
    let html = fs::read_to_string(index_path)
        .with_context(|| format!("read {}", index_path.display()))?;
    let rewritten = inject_into_head(html, &snippet);
    fs::write(index_path, rewritten)
        .with_context(|| format!("write {}", index_path.display()))?;
    Ok(())
}

/// The staged-index relay step, gate included: `None` writes nothing at
/// all.
///
/// The gate lives here rather than at the call site so it is reachable
/// from a test. What it protects is not cosmetic — a deploy bundle that
/// picked up a dev machine's relay port would ship an app that dials a
/// developer's laptop, and every `idealyst build` path relies on this
/// staying a no-op.
fn stage_robot_relay_url(index_path: &Path, url: Option<&str>) -> Result<()> {
    let Some(url) = url else { return Ok(()) };
    inject_robot_relay_url_into_staged_index(index_path, url)
}

/// Splice `window.IDEALYST_ROBOT_RELAY_URL` into the staged
/// `index.html` head, so a browser-hosted app dials the dev session's
/// robot relay. Same read-modify-write shape as the injectors above,
/// and it must run BEFORE gzip for the same reason.
fn inject_robot_relay_url_into_staged_index(index_path: &Path, url: &str) -> Result<()> {
    let html = fs::read_to_string(index_path)
        .with_context(|| format!("read {}", index_path.display()))?;
    let rewritten = inject_into_head(html, &robot_relay_script_tag(url));
    fs::write(index_path, rewritten)
        .with_context(|| format!("write {}", index_path.display()))?;
    Ok(())
}

/// The relay `<script>` tag itself — shared with the dev server's
/// serve-time head injection (`cmd/dev.rs`), so the file a full-stack
/// server hands out and the HTML `dev-http` synthesizes advertise the
/// relay identically.
///
/// The URL is escaped rather than interpolated: it reaches us from
/// `IDEALYST_ROBOT_RELAY_URL`, which a user can set to anything, and an
/// unescaped `"` there would close the string literal and leave the rest
/// of the page's `<head>` as executable script.
pub fn robot_relay_script_tag(url: &str) -> String {
    format!(
        "\n    <script>window.IDEALYST_ROBOT_RELAY_URL=\"{}\";</script>",
        escape_js_string(url),
    )
}

/// Escape `s` for embedding in a double-quoted JS string literal that
/// lives inside an inline `<script>`.
///
/// `<` becomes `\u003C` — not for the string literal's sake but for the
/// HTML parser's: a literal `</script` anywhere in the body ends the
/// block, whatever it is nested in, so a URL containing one would spill
/// the rest of the tag into the document as markup.
fn escape_js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '<' => out.push_str("\\u003C"),
            c => out.push(c),
        }
    }
    out
}

/// Rasterize the project's master icon into the staged bundle and
/// splice `<link>` tags for the generated files into `index.html`.
/// No-op when `[package.metadata.idealyst.app.icon]` is absent —
/// nothing is written, nothing is injected, the user's existing
/// icon-handling (or lack of it) survives untouched.
///
/// Files land at the bundle root (`/favicon.ico`,
/// `/favicon-{192,512}.png`, `/apple-touch-icon.png`) so the
/// injected `<link>` tags can reference them with absolute paths,
/// matching how the font-preload pipeline emits `/fonts/...`. The
/// 16/32/48 sizes are bundled into `favicon.ico`; the PNGs cover
/// web-app-manifest and Apple home-screen pinning.
fn sync_and_inject_web_icons(project_dir: &Path, staged: &Path) -> Result<()> {
    let Some(config) = icon_gen::load_config_from_manifest(project_dir)? else {
        return Ok(());
    };
    let block = config.resolved_for(icon_gen::Target::Web);
    icon_gen::sync_web_icons(Some(&block), staged)
        .with_context(|| format!("generate web icons into {}", staged.display()))?;

    let index_path = staged.join("index.html");
    let html = fs::read_to_string(&index_path)
        .with_context(|| format!("read {}", index_path.display()))?;
    let snippet = icon_gen::web_icon_link_tags();
    let rewritten = inject_into_head(html, &snippet);
    fs::write(&index_path, rewritten)
        .with_context(|| format!("write {}", index_path.display()))?;
    Ok(())
}

/// Drop wasm-pack housekeeping files from a staged `pkg/`. They're
/// build artifacts that have no place in a deployed bundle:
/// `package.json` makes some CDNs mis-guess directory MIME types,
/// and `.d.ts` files just bloat the wire for browsers that don't
/// touch them.
fn strip_wasm_pack_metadata(staged_pkg: &Path) {
    for stem in ["package.json", ".gitignore", "README.md"] {
        let _ = fs::remove_file(staged_pkg.join(stem));
    }
    if let Ok(read) = fs::read_dir(staged_pkg) {
        for entry in read.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("ts") {
                let _ = fs::remove_file(&path);
            }
        }
    }
}

/// Outcome of [`fingerprint_pkg`]'s content-hash pass over a staged
/// `pkg/`.
#[derive(Debug, Clone)]
pub struct PkgFingerprint {
    /// 16 lowercase hex chars of the SHA-256 digest over every file in
    /// the pkg dir (path + bytes, path-sorted). Stable across
    /// byte-identical rebuilds, different for any change.
    pub hash: String,
    /// File name (within `pkg/`) of the hashed entry-point JS shim,
    /// e.g. `website.3f9a12bc44d0e1a7.js`. Pages boot the app via
    /// `/pkg/<entry_js>`.
    pub entry_js: String,
    /// Content-hashed name of `premint.css` when the bundle carries a
    /// preminted stylesheet (`--premint` builds), e.g.
    /// `premint.3f9a12bc44d0e1a7.css`. `None` when no premint sheet
    /// was in the pkg.
    pub premint_css: Option<String>,
}

/// Content-address a staged `pkg/`: rename every top-level `.js` /
/// `.wasm` from `<stem>.<ext>` to `<stem>.<hash>.<ext>` and rewrite
/// the references between them (ES import specifiers, the shim's
/// `new URL('<lib>_bg.wasm')` boot path, the split loader's chunk
/// fetch URLs). The point: a redeploy changes every filename, so an
/// HTTP cache can never mix old and new bundle halves — the classic
/// "index.html cached yesterday's <lib>.js which fetches today's
/// wasm" failure is structurally impossible, and hosts can serve
/// `pkg/` with `Cache-Control: immutable`. `index.html` itself is
/// rewritten separately ([`rewrite_index_bundle_ref`]) because it
/// lives in the bundle root, not in `pkg/`.
///
/// # Why one build-wide hash, not per-file content hashes
///
/// `<lib>.js` and `__wasm_split.js` import each other (the shim's
/// `import * as importN from "./__wasm_split.js"` vs the loader's
/// `import { initSync } from "./<lib>.js"`), so per-file hashing is
/// circular: each file's final bytes depend on the other's final
/// name. One digest over all pre-rewrite bytes breaks the cycle and
/// loses nothing real — any Rust change perturbs the main wasm,
/// whose function-table layout is baked into every chunk, so
/// "only one chunk changed, keep the others cached" essentially
/// never happens for split wasm output anyway.
///
/// # The import-object-key trap (why `.js` renames only rewrite
/// `from "…"` specifiers)
///
/// The main wasm imports its chunk loaders from the literal module
/// string `./__wasm_split.js` (`#[link(wasm_import_module = …)]` in
/// wasm-split-macro), and that string appears in the shim twice: as
/// an ES import specifier (a real URL — MUST follow the rename) and
/// as the wasm import-object key (`"./__wasm_split.js": import1` —
/// MUST stay byte-identical to the string inside the wasm binary,
/// which we can't rewrite without corrupting LEB128 section
/// lengths). So `.js` references are rewritten only in
/// `from "…"` / `from '…'` position, never as bare strings; `.wasm`
/// names are only ever fetch URLs and are replaced anywhere.
pub fn fingerprint_pkg(pkg_dir: &Path, lib_name: &str) -> Result<PkgFingerprint> {
    use sha2::{Digest, Sha256};

    // Digest every file under pkg/ (recursive, so snippet dirs count),
    // path-sorted so the hash is deterministic across filesystems.
    fn collect(dir: &Path, root: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                collect(&path, root, out)?;
            } else {
                let rel = path
                    .strip_prefix(root)
                    .expect("walk stays under root")
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, path));
            }
        }
        Ok(())
    }
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    collect(pkg_dir, pkg_dir, &mut files)
        .with_context(|| format!("walk {}", pkg_dir.display()))?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = Sha256::new();
    for (rel, path) in &files {
        hasher.update(rel.as_bytes());
        hasher.update([0u8]);
        hasher.update(fs::read(path).with_context(|| format!("read {}", path.display()))?);
    }
    let digest = hasher.finalize();
    let hash: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();

    // Rename plan: top-level `.js` / `.wasm` only. `snippets/…` (from
    // `#[wasm_bindgen(inline_js)]`) stays put — wasm-bindgen already
    // names those dirs by a per-crate identifier AND their bytes feed
    // the digest above, so a snippet change still rotates the shim
    // that imports it.
    let mut renames: Vec<(String, String)> = Vec::new();
    for (rel, _) in &files {
        if rel.contains('/') {
            continue;
        }
        let Some((stem, ext)) = rel.rsplit_once('.') else {
            continue;
        };
        if ext != "js" && ext != "wasm" && ext != "css" {
            continue;
        }
        // The preminted stylesheet is a LEAF: nothing inside pkg/ links to
        // it (index.html is rewritten separately) and it links to nothing.
        // So it gets its OWN content hash rather than the build-wide one.
        //
        // Sharing the build hash made the CSS URL rotate on every deploy
        // that touched a single line of Rust, evicting a byte-identical
        // stylesheet from every browser cache. That defeats the point of
        // shipping styles as a separate asset: app code changes far more
        // often than styles, and the whole reason premint moves rules out
        // of the wasm is so they can be cached — and re-used — on their
        // own schedule.
        let file_hash = if ext == "css" {
            let bytes = fs::read(pkg_dir.join(rel))
                .with_context(|| format!("read {rel} for content hash"))?;
            let d = Sha256::digest(&bytes);
            d[..8].iter().map(|b| format!("{b:02x}")).collect::<String>()
        } else {
            hash.clone()
        };
        renames.push((rel.clone(), format!("{stem}.{file_hash}.{ext}")));
    }

    // Rewrite references inside each top-level JS file, writing the
    // result under its new name.
    for (old, new) in &renames {
        if !old.ends_with(".js") {
            continue;
        }
        let src = pkg_dir.join(old);
        let mut content =
            fs::read_to_string(&src).with_context(|| format!("read {}", src.display()))?;
        for (o, n) in &renames {
            if o.ends_with(".wasm") {
                content = content.replace(o, n);
            } else {
                content = content.replace(&format!("from \"./{o}\""), &format!("from \"./{n}\""));
                content = content.replace(&format!("from './{o}'"), &format!("from './{n}'"));
            }
        }
        fs::write(pkg_dir.join(new), content)
            .with_context(|| format!("write {}", pkg_dir.join(new).display()))?;
        fs::remove_file(&src).with_context(|| format!("remove {}", src.display()))?;
    }
    for (old, new) in &renames {
        // `.wasm` and `.css` are plain fetch targets — no intra-pkg
        // references to rewrite, so a straight rename suffices.
        if old.ends_with(".wasm") || old.ends_with(".css") {
            fs::rename(pkg_dir.join(old), pkg_dir.join(new))
                .with_context(|| format!("rename {old} → {new}"))?;
        }
    }

    let entry_old = format!("{lib_name}.js");
    let entry_js = renames
        .iter()
        .find(|(o, _)| *o == entry_old)
        .map(|(_, n)| n.clone())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "fingerprint: expected {entry_old} in {} (wasm-bindgen output shape changed?)",
                pkg_dir.display(),
            )
        })?;
    let premint_css = renames
        .iter()
        .find(|(o, _)| *o == premint::PREMINT_CSS_NAME)
        .map(|(_, n)| n.clone());
    eprintln!(
        "[build-web] fingerprint {hash}: {} pkg file(s) content-addressed (entry {entry_js}{})",
        renames.len(),
        premint_css
            .as_deref()
            .map(|c| format!(", stylesheet {c} — hashed on its own bytes"))
            .unwrap_or_default(),
    );
    Ok(PkgFingerprint { hash, entry_js, premint_css })
}

/// Point the staged `index.html` at the fingerprinted entry shim:
/// `pkg/<lib>.js` → `pkg/<entry_js>`. Covers `/pkg/…`, `./pkg/…`, and
/// bare `pkg/…` spellings (the replacement is on the common
/// substring). A user-authored index.html that doesn't reference the
/// bundle by the conventional name gets a loud warning instead of a
/// silent stale-cache footgun.
fn rewrite_index_bundle_ref(index_path: &Path, lib_name: &str, entry_js: &str) -> Result<()> {
    let html = fs::read_to_string(index_path)
        .with_context(|| format!("read {}", index_path.display()))?;
    let old = format!("pkg/{lib_name}.js");
    let new = format!("pkg/{entry_js}");
    if !html.contains(&old) {
        eprintln!(
            "[build-web] ⚠ {} doesn't reference {old}; the fingerprinted entry is /{new} — \
             point your script tag at it (or at {old}, which the build rewrites) or browsers \
             may cache a stale bundle",
            index_path.display(),
        );
        return Ok(());
    }
    fs::write(index_path, html.replace(&old, &new))
        .with_context(|| format!("write {}", index_path.display()))?;
    Ok(())
}

/// Locate the fingerprinted entry shim (`<lib>.<16 hex>.js`) in a
/// previously staged `pkg/`, for callers that need the entry name
/// without re-running the build — e.g. `idealyst build --ssg` against
/// a `dist/web` staged by an earlier `--web` run. Returns the bare
/// file name. `backend_ssr::resolve_bundle_module` is the runtime
/// twin of this for the generated SSR wrapper; keep their matching
/// rules in sync.
pub fn find_hashed_entry(pkg_dir: &Path, lib_name: &str) -> Option<String> {
    let prefix = format!("{lib_name}.");
    for entry in fs::read_dir(pkg_dir).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(mid) = name
            .strip_prefix(&prefix)
            .and_then(|rest| rest.strip_suffix(".js"))
        else {
            continue;
        };
        if mid.len() == 16 && mid.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Some(name);
        }
    }
    None
}

/// Top-level entries that never belong in a deployable bundle when no
/// explicit `assets` allowlist is declared. Source trees, build
/// outputs, VCS/IDE metadata, package-manager caches, dotfiles — AND,
/// critically, internal docs/configs that would otherwise leak to the
/// public site root (`FEEDBACK.md`, `dev.toml`, `design-files/`, …).
///
/// Anything not excluded ships, so the "drop a folder in your project
/// root and it auto-deploys" convenience still works for real web
/// assets — `fonts/`, `assets/`, `public/`, `images/`, `robots.txt`,
/// css, etc. — without forcing an allowlist. Projects that want a hard
/// guarantee declare `[package.metadata.idealyst.app.web].assets` and
/// switch to the allowlist path entirely.
///
/// SECURITY: this is the fallback denylist. It must stay tight enough
/// that no docs/config/source escapes; when in doubt prefer the
/// `assets` allowlist. The matcher is case-insensitive for the
/// extension/prefix checks so `README.MD` / `Feedback.txt` don't slip
/// through on case-sensitive filesystems.
fn is_excluded_from_bundle(name: &str) -> bool {
    if name.starts_with('.') {
        return true;
    }
    let lower = name.to_ascii_lowercase();
    // Exact-name folders/files: source trees, build outputs, caches.
    if matches!(
        lower.as_str(),
        "src"
            | "target"
            | "tests"
            | "benches"
            | "examples"
            | "node_modules"
            | "dist"
            | "pkg"
            | "cargo.toml"
            | "cargo.lock"
            | "design-files"
    ) {
        return true;
    }
    // Doc / license / internal-report prefixes (any extension).
    if lower.starts_with("readme")
        || lower.starts_with("license")
        || lower.starts_with("licence")
        || lower.starts_with("feedback")
        || lower.starts_with("changelog")
        || lower.starts_with("contributing")
    {
        return true;
    }
    // Source / doc / config / log extensions. ALL `*.toml` and
    // `*.lock` (not just the two Cargo names) so a stray `dev.toml`,
    // `app.toml`, or sibling lockfile never ships.
    [
        ".rs", ".md", ".toml", ".lock", ".log", ".markdown", ".mdx",
    ]
    .iter()
    .any(|ext| lower.ends_with(ext))
}

fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let ft = entry.file_type()?;
        if ft.is_dir() {
            copy_dir(&from, &to)?;
        } else if ft.is_file() {
            fs::copy(&from, &to)?;
        }
        // Symlinks intentionally ignored — bundles are meant to be
        // self-contained and portable to remote object storage.
    }
    Ok(())
}

/// Replace every compressible file in `bundle_dir` with its gzipped
/// bytes (keeps the original filename). Skips formats that are already
/// compressed — re-gzipping wastes bytes and CPU and would force the
/// host to advertise the wrong Content-Type.
fn gzip_bundle(bundle_dir: &Path) -> Result<()> {
    fn walk(dir: &Path, on_file: &mut dyn FnMut(&Path) -> Result<()>) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let ft = entry.file_type()?;
            if ft.is_dir() {
                walk(&path, on_file)?;
            } else if ft.is_file() {
                on_file(&path)?;
            }
        }
        Ok(())
    }
    walk(bundle_dir, &mut |path| {
        if is_already_compressed(path) {
            return Ok(());
        }
        let bytes = fs::read(path).with_context(|| format!("read {} for gzip", path.display()))?;
        let mut enc = GzEncoder::new(Vec::with_capacity(bytes.len()), Compression::best());
        enc.write_all(&bytes)
            .with_context(|| format!("gzip {}", path.display()))?;
        let gz = enc
            .finish()
            .with_context(|| format!("finalize gzip {}", path.display()))?;
        fs::write(path, gz).with_context(|| format!("write gzipped {}", path.display()))?;
        Ok(())
    })
}

/// Emit a brotli-compressed `.br` sibling for every compressible file in
/// the staged bundle (originals untouched). q11/window-22 — the encode is
/// a one-time build cost, so max quality is free at serve time. A sibling
/// is only kept when it's actually smaller than the original; hosts fall
/// back to the uncompressed file otherwise (tiny files where the brotli
/// framing would win nothing get no sibling).
fn brotli_precompress_bundle(bundle_dir: &Path) -> Result<()> {
    fn walk(dir: &Path, on_file: &mut dyn FnMut(&Path) -> Result<()>) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let ft = entry.file_type()?;
            if ft.is_dir() {
                walk(&path, on_file)?;
            } else if ft.is_file() {
                on_file(&path)?;
            }
        }
        Ok(())
    }
    let mut emitted = 0usize;
    let mut saved: u64 = 0;
    walk(bundle_dir, &mut |path| {
        // Skip already-compressed formats AND any `.br` from a previous
        // run (re-staging syncs over the old bundle dir).
        if is_already_compressed(path)
            || path.extension().and_then(|s| s.to_str()) == Some("br")
        {
            return Ok(());
        }
        let bytes =
            fs::read(path).with_context(|| format!("read {} for brotli", path.display()))?;
        let mut out = Vec::with_capacity(bytes.len() / 2);
        {
            // buffer_size 4096, quality 11, lg_window_size 22 — the encoder
            // defaults brotli's own CLI uses at `-q 11`.
            let mut enc = brotli::CompressorWriter::new(&mut out, 4096, 11, 22);
            enc.write_all(&bytes)
                .with_context(|| format!("brotli {}", path.display()))?;
        }
        if out.len() >= bytes.len() {
            return Ok(());
        }
        let mut sibling = path.as_os_str().to_owned();
        sibling.push(".br");
        let sibling = PathBuf::from(sibling);
        fs::write(&sibling, &out)
            .with_context(|| format!("write {}", sibling.display()))?;
        emitted += 1;
        saved += (bytes.len() - out.len()) as u64;
        Ok(())
    })?;
    if emitted > 0 {
        println!(
            "[build-web] brotli: {emitted} precompressed .br sibling(s) emitted ({} KB smaller than the originals); hosts with `brotli_static` / `precompressed` serve them automatically",
            saved / 1024
        );
    }
    Ok(())
}

fn is_already_compressed(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "avif"
            | "ico"
            | "woff"
            | "woff2"
            | "mp4"
            | "mov"
            | "webm"
            | "mp3"
            | "ogg"
            | "m4a"
            | "zip"
            | "gz"
            | "br"
    )
}


/// Pick the nightly toolchain name to use for the `--strip-panics`
/// (`-Z build-std`) build.
///
/// We derive `nightly-<host-triple>` from the **active** rustc rather
/// than passing a bare `+nightly`: rustup expands `+nightly` against
/// its *default host triple*, which can differ from the active
/// toolchain's arch (e.g. an x86_64 rustup install driving an
/// Apple-Silicon default toolchain). That mismatch resolves to a
/// wrong-arch nightly that usually lacks the `rust-src` component, so
/// the build fails confusingly. Matching the active host triple avoids
/// that. Falls back to a bare `nightly` if `rustc -vV` can't be parsed.
fn default_nightly_toolchain() -> String {
    let host = Command::new("rustc")
        .arg("-vV")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("host: ").map(str::to_string))
        });
    match host {
        Some(triple) => format!("nightly-{triple}"),
        None => "nightly".to_string(),
    }
}

/// Features the app forwards to the framework rather than owning.
///
/// These were wrapper features that gated `backend-web`; with the
/// wrapper gone they resolve against the `idealyst` facade the app
/// depends on. Anything NOT listed here is one of the app's own
/// features and is passed through untouched.
const FRAMEWORK_FEATURES: &[&str] = &["runtime-server", "robot", "hydrate"];

/// Map a CLI-supplied feature name to the spec cargo needs on the app
/// crate: `robot` → `idealyst/robot`, `my-thing` → `my-thing`.
///
/// `aas` is the deprecated alias for `runtime-server` and normalizes to
/// it, which is what the wrapper's `[features]` block did.
fn feature_spec(name: &str) -> String {
    match name {
        "aas" => "idealyst/runtime-server".to_string(),
        f if FRAMEWORK_FEATURES.contains(&f) => format!("idealyst/{f}"),
        other => other.to_string(),
    }
}

/// Profile settings wasm-split depends on, passed as `--config` rather
/// than written into a manifest.
///
/// The generated wrapper used to carry these in its own
/// `[profile.release]`. Building the app crate directly means the
/// profile comes from the *app's* workspace, which the framework
/// doesn't own and shouldn't edit — so they're injected per-invocation.
/// Each is load-bearing:
///
/// * `lto = "off"` — `"fat"` LTO inlines `#[wasm_split]`-annotated
///   functions back into their callers, putting the body in the main
///   bundle and leaving the chunk a stub. wasm-opt's per-chunk pass
///   recovers most of the size win anyway.
/// * `codegen-units = 1` — fewer cross-unit indirections gives
///   wasm-split's reachability walker more precision (it is pessimistic
///   across CU boundaries).
/// * `strip = "none"` / `debug = "limited"` — symbol names and line
///   tables stay alive for wasm-split's call-graph matching; wasm-opt
///   strips both as a final step.
///
/// `[profile.dev.package."*"] opt-level = 3` optimizes DEPENDENCIES
/// only (the glob excludes the app crate, so app iteration stays fast to
/// compile). Without it, compute-heavy dependency crates — a CPU
/// rasterizer's un-inlined inner loops — run 10-40x slower in dev.
///
/// The two dev-only settings beside it are pure iteration cost, and both
/// override whatever the app's own workspace declares (that is what
/// `--config` does — same as the opt-level line above):
///
/// * `debug` — cargo defaults dev to `2`, which on wasm means a dev
///   bundle carries MORE debug info than release's `"limited"`, with no
///   sidecar to put it in. See [`DebugInfo`]; `--debuginfo full` restores
///   cargo's default for a debugging session.
/// * `lto = "off"` — `false` (cargo's dev default) still runs thin-LOCAL
///   LTO across the crate's own codegen units. `-Ztime-passes` on the
///   website's app crate attributes 0.63s per rebuild to it, buying
///   nothing a dev build wants.
///
/// Note the app crate's own `opt-level` is deliberately NOT set: dropping
/// it to 0 measured *slower* end-to-end (10.4s vs 9.1s on the website)
/// and 30% larger, because unoptimized IR costs more to emit and link
/// than the `-Oz` passes cost to run.
/// Wall-clock attribution for the build pipeline.
///
/// The web build is a chain of passes with wildly different cost
/// profiles — cargo, wasm-bindgen, the command-export neutralize,
/// wasm-split, premint's second native build, staging, fingerprinting,
/// compression — and until this existed none of them reported a
/// duration. "Why did that rebuild take two minutes" could only be
/// answered by re-running the pieces by hand, so in practice it was
/// answered by guessing, and the guesses were wrong: the packaging
/// passes are a near-constant few seconds and effectively all variance
/// is cargo recompiling upstream crates.
///
/// Mirrors the runtime's `PhaseTimer` convention (see the profiling
/// section of the repo's CLAUDE.md): stable, specific phase names,
/// because they are aggregation keys. Unlike that one this is always on
/// — a handful of `Instant::now()` calls per build is unmeasurable
/// against passes that take seconds, and a profiler you have to enable
/// is a profiler nobody has enabled when they need it.
#[derive(Default)]
struct BuildTimings {
    phases: Vec<(&'static str, std::time::Duration)>,
}

impl BuildTimings {
    /// Run `f`, record how long it took under `phase`, return its value.
    /// Generic over the return type so it wraps fallible passes without
    /// forcing a `?` inside the closure.
    fn time<T>(&mut self, phase: &'static str, f: impl FnOnce() -> T) -> T {
        let start = std::time::Instant::now();
        let out = f();
        self.phases.push((phase, start.elapsed()));
        out
    }

    /// One summary line, longest phase first, plus the total. Printed at
    /// the end of every build.
    fn report(&self) {
        if self.phases.is_empty() {
            return;
        }
        let total: std::time::Duration = self.phases.iter().map(|(_, d)| *d).sum();
        let mut sorted: Vec<_> = self.phases.iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1));
        let body = sorted
            .iter()
            .map(|(name, d)| format!("{name} {:.2}s", d.as_secs_f64()))
            .collect::<Vec<_>>()
            .join(" | ");
        eprintln!(
            "[build-web] timing: total {:.2}s — {body}",
            total.as_secs_f64(),
        );
    }
}

/// Short stable key for every option that invalidates the WHOLE cargo
/// build graph, used to give each build configuration its own target dir.
///
/// The flags below do not invalidate one crate — they invalidate all of
/// them, because they ride in `CARGO_ENCODED_RUSTFLAGS` (the `--cfg`s,
/// `--export-table`, the panic strategy) or change feature unification
/// (`hydrate`, `user_features`) or the profile (`debuginfo`). Sharing one
/// target dir across them means every toggle is a full cold rebuild of
/// the framework, and — worse — that two *different commands* silently
/// evict each other's cache. `idealyst dev --web` builds with
/// `idealyst/hydrate` on and `idealyst build --web` builds it off, so
/// alternating them used to recompile everything each way; you could see
/// both fingerprints sitting in one dir as two `libidea_ui-*.rlib`s.
///
/// Keying the directory turns "recompile the world" into "switch
/// directories". The cost is disk: each configuration you actually use
/// keeps its own dependency build. Directories are created lazily, so an
/// unused combination costs nothing.
fn config_key(opts: &BuildOptions) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    // Every field here MUST be one that invalidates the whole graph. Adding
    // a field that doesn't just fragments the cache for no benefit; leaving
    // one out reintroduces the thrash this exists to prevent.
    h.update([
        opts.release as u8,
        opts.premint as u8,
        opts.premint_only as u8,
        opts.premint_report as u8,
        opts.wasm_split as u8,
        opts.strip_panics as u8,
        opts.hydrate as u8,
    ]);
    h.update(opts.debuginfo.cargo_value().as_bytes());
    h.update(b"\x1f");
    h.update(opts.dev_opt.key_tag().as_bytes());
    // Feature order reaches cargo sorted+deduped; mirror that so a caller
    // passing the same set in a different order lands in the same dir.
    let mut features: Vec<&str> = opts.user_features.iter().map(String::as_str).collect();
    features.sort_unstable();
    features.dedup();
    for f in features {
        h.update(b"\x1f");
        h.update(f.as_bytes());
    }
    let digest = h.finalize();
    digest[..4].iter().map(|b| format!("{b:02x}")).collect()
}

/// Human-readable rendering of the same options [`config_key`] hashes.
/// Logged next to the resolved target dir so a directory full of opaque
/// `idealyst-web-<hex>` names is still debuggable.
fn config_summary(opts: &BuildOptions) -> String {
    let mut parts = vec![if opts.release { "release" } else { "dev" }.to_string()];
    if opts.premint_only {
        parts.push("premint-only".into());
    } else if opts.premint {
        parts.push("premint".into());
    }
    if opts.premint_report {
        parts.push("premint-report".into());
    }
    if !opts.wasm_split {
        parts.push("no-split".into());
    }
    if opts.strip_panics {
        parts.push("strip-panics".into());
    }
    if opts.hydrate {
        parts.push("hydrate".into());
    }
    parts.push(format!("debug={}", opts.debuginfo.cargo_value().trim_matches('"')));
    if !opts.release {
        parts.push(format!("opt={:?}", opts.dev_opt).to_lowercase());
    }
    for f in &opts.user_features {
        parts.push(format!("+{f}"));
    }
    parts.join(", ")
}

fn profile_config_args(release: bool, debuginfo: DebugInfo, dev_opt: DevOpt) -> Vec<String> {
    let settings: Vec<String> = if release {
        [
            "profile.release.opt-level=\"z\"",
            "profile.release.codegen-units=1",
            "profile.release.lto=\"off\"",
            "profile.release.panic=\"abort\"",
            "profile.release.strip=\"none\"",
            "profile.release.debug=\"limited\"",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect()
    } else {
        let debug = debuginfo.cargo_value();
        let mut v = vec![
            format!("profile.dev.debug={debug}"),
            format!("profile.dev.package.\"*\".debug={debug}"),
            "profile.dev.lto=\"off\"".to_string(),
        ];
        match dev_opt {
            DevOpt::Fast => {
                // `profile.dev.opt-level` is the half that does anything
                // here: `package."*"` skips workspace members, so without
                // it a framework-crate edit still recompiles every member
                // at the workspace's `opt-level`. Deps drop to 1 rather
                // than 0 because they rebuild rarely and their codegen
                // quality still shows up in dev runtime.
                v.push("profile.dev.opt-level=0".to_string());
                v.push("profile.dev.package.\"*\".opt-level=1".to_string());
            }
            DevOpt::Optimized => {
                v.push("profile.dev.package.\"*\".opt-level=3".to_string());
            }
        }
        v
    };
    settings
        .iter()
        .flat_map(|s| ["--config".to_string(), s.clone()])
        .collect()
}

/// Fail early, and in the app author's terms, when the crate has no
/// binary target.
///
/// The app crate is the artifact now, so it needs a `main`. Without this
/// the failure is cargo's `no bin target named ...`, which says nothing
/// about the one line that fixes it.
fn ensure_entry_point(project_dir: &Path, bin_name: &str) -> Result<()> {
    if project_dir.join("src/main.rs").is_file() {
        return Ok(());
    }
    // An explicit `[[bin]]` can put the entry anywhere, so a missing
    // `src/main.rs` is only a problem if the manifest doesn't declare
    // one either.
    //
    // Matched per line with comments stripped, NOT as a substring of the
    // whole file: the scaffold's own Cargo.toml mentions `[[bin]]` in a
    // prose comment, and a raw `contains` therefore reported every
    // freshly-scaffolded project as having an entry point — swapping the
    // one actionable message below for cargo's bare `no bin target
    // named <app>`.
    let manifest = fs::read_to_string(project_dir.join("Cargo.toml")).unwrap_or_default();
    let declares_bin = manifest
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .any(|line| line == "[[bin]]");
    if declares_bin {
        return Ok(());
    }
    anyhow::bail!(
        "{} has no binary target, so there is nothing to build.\n\n\
         An idealyst app IS its binary now — there is no generated wrapper \
         crate supplying the entry point. Add `src/main.rs`:\n\n    \
         idealyst::entry!({});\n\n\
         and depend on the facade in Cargo.toml:\n\n    \
         [dependencies]\n    idealyst = \"1.2\"",
        project_dir.display(),
        bin_name.replace('-', "_"),
    )
}

/// Run `cargo build --target wasm32-unknown-unknown` against the
/// app crate. `-C link-args=--emit-relocs` is set so the rustc-emitted
/// wasm carries the relocation info wasm-split-cli needs to identify
/// indirect-call targets per chunk. Cost is a few KB of metadata
/// pre-bindgen; stripped from the final bundle by wasm-opt.
///
/// Flags reach cargo through `CARGO_ENCODED_RUSTFLAGS` (`\x1f`-separated)
/// rather than the space-separated `RUSTFLAGS`, because release builds add
/// `--remap-path-prefix` flags that embed filesystem paths — a space in one
/// would otherwise split it into two garbage arguments. Any user-supplied
/// `RUSTFLAGS` is folded in, since cargo ignores it once the encoded form
/// is set.
fn cargo_build_wasm(
    project_dir: &Path,
    bin_name: &str,
    target_dir: &Path,
    release: bool,
    wasm_split: bool,
    debuginfo: DebugInfo,
    dev_opt: DevOpt,
    strip_panics: bool,
    premint: bool,
    premint_only: bool,
    premint_report: bool,
    hydrate: bool,
    user_features: &[String],
    source: &FrameworkSource,
    project_root: &Path,
) -> Result<()> {
    let mut cmd = Command::new("cargo");
    // `panic_immediate_abort` lives in std/core, so stripping panics
    // means recompiling std from source with `-Z build-std` — both of
    // which are nightly-only. We select nightly via the rustup `+`
    // shim rather than touching the user's default toolchain. Override
    // the toolchain name with `IDEALYST_NIGHTLY` if the default isn't
    // right on a given machine (e.g. to pin a nightly date).
    if strip_panics {
        let toolchain =
            std::env::var("IDEALYST_NIGHTLY").unwrap_or_else(|_| default_nightly_toolchain());
        cmd.arg(format!("+{toolchain}"));
    }
    cmd.current_dir(project_dir)
        .arg("build")
        .args(["--target", "wasm32-unknown-unknown"])
        // The binary target IS the app entry point. Named explicitly so
        // a crate that also has a lib (the normal shape — components in
        // `lib.rs`, `entry!` in `main.rs`) doesn't build both.
        .args(["--bin", bin_name])
        .arg("--target-dir")
        .arg(target_dir)
        .args(profile_config_args(release, debuginfo, dev_opt));
    if release {
        cmd.arg("--release");
    }
    if strip_panics {
        // Rebuild std so the panic-strip applies to it (not just the user
        // crates). `panic_abort` must be listed explicitly alongside `std`.
        //
        // NOTE: `panic_immediate_abort` was promoted from a std/core *feature*
        // to a real panic strategy in recent nightlies (rustc ~1.98): the old
        // `-Z build-std-features=panic_immediate_abort` now `compile_error!`s
        // in `core::panicking`. The replacement is the `immediate-abort` panic
        // strategy, selected via `-Zunstable-options -Cpanic=immediate-abort`
        // in RUSTFLAGS below (still requires `-Z build-std` to recompile core).
        cmd.args(["-Z", "build-std=std,panic_abort"]);
    }
    // One `--features` list against the app crate. Framework features
    // are spelled `idealyst/<f>`, which cargo accepts because `idealyst`
    // is a direct dependency of the app. The wrapper's parallel
    // `[features]` forwarding block is gone entirely.
    let mut features: Vec<String> = user_features.iter().map(|f| feature_spec(f)).collect();
    if hydrate {
        features.push("idealyst/hydrate".to_string());
    }
    features.sort();
    features.dedup();
    if !features.is_empty() {
        cmd.arg("--features").arg(features.join(","));
    }
    // Flags are assembled as a LIST and handed to cargo via
    // `CARGO_ENCODED_RUSTFLAGS` (`\x1f`-separated) rather than the
    // space-separated `RUSTFLAGS`. The remap flags below embed filesystem
    // paths, and a space anywhere in them (`/Users/Jane Smith/...`) would
    // split one flag into two garbage arguments under `RUSTFLAGS`.
    //
    // Setting the encoded form makes cargo ignore `RUSTFLAGS` entirely, so
    // whatever the user supplied is folded in here to preserve today's
    // behavior. Cargo prefers `CARGO_ENCODED_RUSTFLAGS` over `RUSTFLAGS`
    // when both are set; mirror that precedence rather than clobbering an
    // already-encoded value (its `\x1f` fields are taken verbatim, so
    // fields containing spaces survive).
    let mut flags: Vec<String> = match std::env::var("CARGO_ENCODED_RUSTFLAGS") {
        Ok(encoded) if !encoded.is_empty() => {
            encoded.split('\x1f').map(str::to_string).collect()
        }
        _ => std::env::var("RUSTFLAGS")
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_string)
            .collect(),
    };
    // `+simd128`: enable wasm SIMD so SIMD-centric deps (e.g. `vello_cpu`/
    // `fearless_simd`, behind `hayro`'s PDF rasterization) take their vectorized
    // path instead of the scalar fallback — a large speedup for CPU rasterization.
    // No `SharedArrayBuffer` / cross-origin isolation involved (that's wasm
    // *threads*); this is pure codegen and orthogonal to wasm-split. Supported by
    // all evergreen browsers (Chrome/Firefox 2021+, Safari 16.4+).
    flags.push("-C".into());
    flags.push("target-feature=+simd128".into());
    flags.push("-C".into());
    flags.push("link-args=--emit-relocs".into());
    if !wasm_split {
        // `--no-split`'s inline loader wakes the Rust future through the
        // main module's function table, which LLD only exports when asked.
        // The splitter adds that export itself, so this rides the no-split
        // path alone rather than being set unconditionally — an exported
        // table roots every entry, which would cost release builds DCE.
        flags.push("-C".into());
        flags.push("link-args=--export-table".into());
    }
    if premint {
        // Flip the `stylesheet!`-generated builders to their preminted
        // fast path (`StyleSource::Preminted` for all-constant
        // applications). Paired with the native dump build that emits
        // the matching `pkg/premint.css` — see the `premint` module.
        flags.push("--cfg".into());
        flags.push("idealyst_premint".into());
    }
    if premint_report {
        // Diagnostic only — the engine STAYS, so the app renders while it
        // reports. See `style_attach::report`.
        flags.push("--cfg".into());
        flags.push("idealyst_premint_report".into());
    }
    if strip_panics {
        // Select the `immediate-abort` panic strategy (the modern replacement
        // for the removed `panic_immediate_abort` build-std feature). Applies to
        // every crate incl. the `-Z build-std` core/std rebuild above, so panic
        // paths + their `#[track_caller]` location strings are dropped bundle-wide.
        flags.push("-Zunstable-options".into());
        flags.push("-Cpanic=immediate-abort".into());
    }
    if premint_only {
        // Strip the runtime style engine. Paired with `--cfg
        // idealyst_premint` (set above) — without that this would remove the
        // engine AND leave nothing to render styles with.
        flags.push("--cfg".into());
        flags.push("idealyst_premint_only".into());
    }
    if release {
        // Keep the build machine's absolute paths out of the shipped wasm.
        // These are NOT debug info — they are panic `Location` strings in live
        // `.rodata` that `wasm-opt --strip-debug` cannot reach — so without
        // this a deployed bundle discloses the builder's home directory,
        // username, toolchain version, and dependency inventory. See
        // `build_ios::remap_path_flags` for the mechanism and the ordering
        // rule. Release only: dev builds keep real paths so panics stay
        // clickable in the terminal and devtools.
        flags.extend(remap_path_flags(source, project_root));
    }
    cmd.env("CARGO_ENCODED_RUSTFLAGS", flags.join("\x1f"));

    eprintln!(
        "[build-web] cargo build --target wasm32-unknown-unknown{}{} (in {})",
        if release { " --release" } else { "" },
        if strip_panics {
            " -Z build-std (panic_immediate_abort)"
        } else {
            ""
        },
        project_dir.display(),
    );
    let status = cmd.status().with_context(|| "exec cargo")?;
    if !status.success() {
        if strip_panics {
            anyhow::bail!(
                "cargo exited with {status}\n\n\
                 `--strip-panics` recompiles std on nightly via `-Z build-std`, which needs:\n  \
                 * a *recent* nightly (an old rolling `nightly` may fail to parse the workspace manifest — `rustup update nightly`)\n  \
                 * the `rust-src` component for that nightly (`rustup component add rust-src --toolchain <nightly>`)\n\
                 Pin a specific known-good nightly with `IDEALYST_NIGHTLY=nightly-YYYY-MM-DD`.\n\
                 Or drop `--strip-panics` to build on the default stable toolchain."
            );
        }
        anyhow::bail!("cargo exited with {status}");
    }
    Ok(())
}

/// Run `wasm-bindgen --target web --keep-lld-exports` to turn the
/// rustc-emitted wasm into the JS-callable wasm-bindgen output.
///
/// `--keep-lld-exports` is the critical flag: without it,
/// wasm-bindgen strips the LLD-emitted exports that wasm-split-cli
/// uses to identify per-chunk reachable code. With them stripped,
/// wasm-split conservatively keeps everything in the main bundle —
/// which is exactly what was happening to the website's bundle in
/// the wasm-pack pipeline.
///
/// We also pass `--keep-debug` so wasm-split has the symbol info it
/// needs to match function references across the relocations. The
/// final wasm-opt pass strips debug info, so this doesn't bloat the
/// shipped bundle.
fn wasm_bindgen_build(original_wasm: &Path, out_dir: &Path, lib_name: &str) -> Result<()> {
    if out_dir.exists() {
        fs::remove_dir_all(out_dir).with_context(|| format!("clear {}", out_dir.display()))?;
    }
    fs::create_dir_all(out_dir).with_context(|| format!("create {}", out_dir.display()))?;
    eprintln!(
        "[build-web] wasm-bindgen --target web --keep-lld-exports --keep-debug → {}",
        out_dir.display(),
    );
    let status = Command::new("wasm-bindgen")
        .args(["--target", "web"])
        .arg("--keep-lld-exports")
        .arg("--keep-debug")
        // CRITICAL: --no-demangle. wasm-bindgen demangles Rust
        // symbol names by default. wasm-split-cli matches reloc
        // records (which carry MANGLED names from rustc) against
        // the bindgened wasm's symbol table — demangled names
        // there mean nothing matches, so wasm-split conservatively
        // keeps everything in main and emits empty chunks. Without
        // this flag the website's lazy hero-simulator chunk
        // measured 469 bytes; with it, the wgpu/welcome/sim stack
        // actually moves out of main.
        .arg("--no-demangle")
        .args(["--out-name", lib_name])
        .args(["--out-dir"])
        .arg(out_dir)
        .arg(original_wasm)
        .status()
        .with_context(|| {
            "exec wasm-bindgen — is it on PATH? (cargo install wasm-bindgen-cli --version <matching>)"
        })?;
    if !status.success() {
        anyhow::bail!("wasm-bindgen exited with {status}");
    }
    Ok(())
}

/// Run `wasm-opt -Oz` on every .wasm in `pkg_dir` (the base + each
/// chunk). Runs LAST in the pipeline — after wasm-split — so the
/// optimizer doesn't strip the symbols / reloc info wasm-split
/// needed. Per-chunk optimization keeps chunks lean independently.
fn wasm_opt_pkg(pkg_dir: &Path) -> Result<()> {
    for entry in fs::read_dir(pkg_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("wasm") {
            continue;
        }
        let tmp = path.with_extension("wasm.opt");
        let status = Command::new("wasm-opt")
            .arg("-Oz")
            .arg("--strip-debug")
            .arg("--strip-producers")
            .arg("--enable-bulk-memory")
            .arg("--enable-nontrapping-float-to-int")
            .arg("-o")
            .arg(&tmp)
            .arg(&path)
            .status()
            .with_context(|| {
                "exec wasm-opt — is binaryen installed? (`brew install binaryen` / apt etc.)"
            })?;
        if !status.success() {
            anyhow::bail!("wasm-opt failed on {}: {status}", path.display());
        }
        fs::rename(&tmp, &path)?;
        eprintln!("[build-web] wasm-opt → {}", path.display());
    }
    Ok(())
}

/// Every `./__wasm_split.js` import the bindgened module declares.
///
/// The `#[wasm_split]` macro emits exactly two imports per split point:
///
/// * `__wasm_split_load_<module>_<hash>_<name>(callback, data)` — the
///   loader the Rust future awaits, and
/// * `__wasm_split_00___<module>___00_import_<hash>_<name>` — the body
///   itself, which the splitter would have moved into a chunk.
///
/// Both must be satisfied by whatever `__wasm_split.js` the build
/// writes, so `--no-split` needs their exact names. Scans the import
/// section only — `walrus`-parsing a multi-hundred-MB debug module is
/// most of the cost `--no-split` exists to avoid.
fn wasm_split_imports(wasm: &[u8]) -> Result<Vec<String>> {
    use wasmparser::{Parser, Payload};

    let mut names = Vec::new();
    for payload in Parser::new(0).parse_all(wasm) {
        if let Payload::ImportSection(reader) = payload.context("wasm: parse sections")? {
            for import in reader {
                let import = import.context("wasm: parse import")?;
                if import.module == WASM_SPLIT_LOADER {
                    names.push(import.name.to_string());
                }
            }
        }
    }
    Ok(names)
}

/// Write the `__wasm_split.js` a NON-split build needs.
///
/// Nothing was extracted, so every `#[wasm_split]` body is still in the
/// main module — under its `…_export_…` name, which the macro emits as a
/// `#[no_mangle]` export next to the `…_import_…` declaration. This
/// loader is what makes "don't split" mean "bundle it anyway" instead of
/// "ship a module with unresolved imports":
///
/// * each `__wasm_split_load_*` resolves immediately — there is nothing
///   to fetch — by waking the Rust future through the main module's
///   function table, exactly as [`wasm_split_cli::MAKE_LOAD_JS`] does;
/// * each `…_import_…` forwards straight to its `…_export_…` twin.
///
/// The lazy boundary therefore still *works* — `loading` shows for one
/// microtask instead of one network round trip — while the body ships in
/// the main bundle.
///
/// Reaching the table requires the wasm to export it, which is why the
/// no-split cargo invocation adds `--export-table` (see
/// [`cargo_build_wasm`]); the splitter adds that export itself on the
/// split path, which is why the flag is not set unconditionally.
fn write_inline_split_loader(pkg_dir: &Path, lib_name: &str, imports: &[String]) -> Result<usize> {
    use std::fmt::Write as _;

    let mut js = format!(
        "// Generated by `idealyst build --web --no-split`.\n\
         //\n\
         // Nothing was split out, so every `#[wasm_split]` body is still in\n\
         // {lib_name}_bg.wasm. These exports satisfy the imports the macro\n\
         // emitted: the loaders resolve immediately, and each `_import_`\n\
         // forwards to the `_export_` twin already present in the main module.\n\
         import {{ initSync }} from \"./{lib_name}.js\";\n\
         \n\
         let mainExports;\n\
         function main() {{\n\
         \x20 return (mainExports ??= initSync(undefined, undefined));\n\
         }}\n\
         \n\
         // Mirrors the real loader's signalling path: the callback arrives as\n\
         // an index into the main module's function table. Deferred to a\n\
         // microtask so it never fires inside `SplitLoaderFuture::poll`, which\n\
         // is still mid-way through setting its waker when it calls us.\n\
         function loadedAlready(callbackIndex, callbackData) {{\n\
         \x20 queueMicrotask(() => {{\n\
         \x20   if (callbackIndex === undefined) return;\n\
         \x20   main().__indirect_function_table.get(callbackIndex)(callbackData, true);\n\
         \x20 }});\n\
         }}\n\
         \n",
    );

    let mut loaders = 0usize;
    for name in imports {
        if name.starts_with("__wasm_split_load_") {
            writeln!(js, "export const {name} = loadedAlready;")?;
            loaders += 1;
        } else {
            // `…___00_import_…` ⇄ `…___00_export_…`: the macro derives both
            // from one hash, so the twin's name is this one with the single
            // `_import_` segment swapped. Replacing only that segment (not
            // every occurrence) keeps a user function literally named
            // `_import_something` from being mangled.
            let export = name.replacen("___00_import_", "___00_export_", 1);
            writeln!(
                js,
                "export function {name}(...args) {{ return main()[\"{export}\"](...args); }}",
            )?;
        }
    }

    let path = pkg_dir.join("__wasm_split.js");
    fs::write(&path, js).with_context(|| format!("write {}", path.display()))?;
    Ok(loaders)
}

/// Remove the loader + chunk wasms a PREVIOUS split build left in
/// `pkg_dir`.
///
/// pkg/ is incremental — wasm-bindgen overwrites its own outputs and
/// leaves everything else alone. Without this, turning splitting off
/// (or removing the app's last lazy component) would leave orphaned
/// `chunk_*.wasm` / `module_*.wasm` files that `fingerprint_pkg`
/// digests and `stage_bundle` ships.
fn clear_wasm_split_artifacts(pkg_dir: &Path) -> Result<()> {
    let loader = pkg_dir.join("__wasm_split.js");
    if loader.is_file() {
        fs::remove_file(&loader).with_context(|| format!("remove {}", loader.display()))?;
    }
    for entry in fs::read_dir(pkg_dir).with_context(|| format!("read {}", pkg_dir.display()))? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if (name.starts_with("chunk_") || name.starts_with("module_")) && name.ends_with(".wasm") {
            fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
        }
    }
    Ok(())
}

/// Run `wasm-split-cli split` against the wasm-pack output to extract
/// `#[wasm_split]`-annotated functions into separate chunk wasms.
///
/// Inputs:
/// - `original_wasm`: the rustc-emitted wasm (in the wrapper's
///   `target/wasm32-unknown-unknown/<profile>/<lib>.wasm`). Carries
///   the `linking` / `reloc.*` sections wasm-split-cli needs.
/// - `pkg_dir`: the wasm-bindgen output directory. Contains
///   `<lib>_bg.wasm` (the bindgened binary) and `<lib>.js` (the JS
///   shim). After this fn returns, `<lib>_bg.wasm` is REPLACED by
///   wasm-split's `main.wasm` and chunk wasms + a `__wasm_split.js`
///   shim are added alongside.
///
/// The emitted `__wasm_split.js` uses some default placeholder URLs
/// for the chunk wasm files (`/harness/split/...`); we rewrite those
/// to relative paths that resolve against wherever the bundle is
/// served. Same for its `import { initSync } from "./main.js"` —
/// rewritten to `./<lib>.js` so it lands on the wasm-bindgen shim.
///
/// Skips silently when the wasm has no `#[wasm_split]` annotations
/// (wasm-split-cli will emit just `main.wasm` with no chunks; we
/// detect that and leave the pkg dir alone).
/// Strip wasm-bindgen 0.2.122's `*.command_export` wrappers from the
/// bindgened wasm in place — without this, every JS↔wasm round trip
/// (string marshal, closure invoke) re-runs `__wasm_call_ctors`, which
/// re-executes every `inventory::submit!`, double-submitting items into
/// `inventory`'s global linked list and eventually trapping with
/// `RuntimeError: memory access out of bounds` somewhere in the next
/// list traversal. See [`wasm_split_cli::neutralize_command_export_wrappers`]
/// for the underlying patch (and regression tests). Runs between
/// `wasm-bindgen` and `wasm-split`; `wasm-split`'s reachability walker
/// drops the now-orphaned wrapper functions for free.
fn neutralize_command_export_wrappers(pkg_dir: &Path, lib_name: &str) -> Result<()> {
    let bindgened_path = pkg_dir.join(format!("{lib_name}_bg.wasm"));
    let bindgened = fs::read(&bindgened_path)
        .with_context(|| format!("read {}", bindgened_path.display()))?;
    let before_len = bindgened.len();
    let patched = wasm_split_cli::neutralize_command_export_wrappers(&bindgened)
        .with_context(|| "walrus: rewrite *.command_export exports → bare helpers")?;
    fs::write(&bindgened_path, &patched)
        .with_context(|| format!("write {}", bindgened_path.display()))?;
    eprintln!(
        "[build-web] command_export neutralized ({} → {} bytes) in {}",
        before_len,
        patched.len(),
        bindgened_path.display(),
    );
    Ok(())
}

fn run_wasm_split(
    original_wasm: &Path,
    pkg_dir: &Path,
    lib_name: &str,
    prune_dead_data_min: Option<usize>,
) -> Result<()> {
    let bindgened_wasm = pkg_dir.join(format!("{lib_name}_bg.wasm"));
    if !bindgened_wasm.is_file() {
        anyhow::bail!(
            "wasm-split: wasm-bindgen output not found at {}",
            bindgened_wasm.display(),
        );
    }
    if !original_wasm.is_file() {
        anyhow::bail!(
            "wasm-split: rustc-emitted wasm not found at {} \
             (--emit-relocs may not have been applied — RUSTFLAGS issue?)",
            original_wasm.display(),
        );
    }

    let original =
        fs::read(original_wasm).with_context(|| format!("read {}", original_wasm.display()))?;
    let bindgened =
        fs::read(&bindgened_wasm).with_context(|| format!("read {}", bindgened_wasm.display()))?;

    // Library API — calls into our vendored wasm-split-cli, so
    // patches we apply land automatically without users needing a
    // separate `cargo install`.
    let splitter = wasm_split_cli::Splitter::new(&original, &bindgened)
        .context("wasm-split: parse module")?
        .with_data_pruning(prune_dead_data_min);
    let output = splitter.emit().context("wasm-split: emit chunks")?;

    // Replace the bindgened wasm with the split-extracted main.
    fs::write(&bindgened_wasm, &output.main.bytes)
        .with_context(|| format!("write split main to {}", bindgened_wasm.display()))?;

    // Drop each chunk + module wasm into pkg_dir alongside the main.
    // Naming mirrors what the CLI binary used to emit, so the
    // generated JS shim's URLs still match.
    let mut chunk_count = 0;
    for (idx, chunk) in output.chunks.iter().enumerate() {
        let name = format!("chunk_{idx}_{}.wasm", chunk.module_name);
        fs::write(pkg_dir.join(&name), &chunk.bytes)
            .with_context(|| format!("write chunk {name}"))?;
        chunk_count += 1;
    }
    for (idx, module) in output.modules.iter().enumerate() {
        let cname = module
            .component_name
            .as_deref()
            .unwrap_or(module.module_name.as_str());
        let name = format!("module_{idx}_{cname}.wasm");
        fs::write(pkg_dir.join(&name), &module.bytes)
            .with_context(|| format!("write module {name}"))?;
        chunk_count += 1;
    }

    // JS loader shim. wasm-split-cli's MAKE_LOAD_JS is just the
    // `makeLoad` factory; the per-chunk `export const
    // __wasm_split_load_…` declarations are appended at runtime by
    // the CLI binary. We replicate that here (build-web equivalent
    // of wasm-split-cli's `emit_js`).
    use std::fmt::Write as _;
    let mut shim = format!(
        "import {{ initSync }} from \"./{lib_name}.js\";\n{}",
        wasm_split_cli::MAKE_LOAD_JS,
    );
    for (idx, chunk) in output.chunks.iter().enumerate() {
        writeln!(
            shim,
            "export const __wasm_split_load_chunk_{idx} = \
             makeLoad(\"./chunk_{idx}_{name}.wasm\", [], fusedImports, initSync);",
            name = chunk.module_name,
        )?;
    }
    for (idx, module) in output.modules.iter().enumerate() {
        let cname = module
            .component_name
            .as_deref()
            .unwrap_or(module.module_name.as_str());
        let hash_id = module.hash_id.as_deref().unwrap_or("");
        let deps = module
            .relies_on_chunks
            .iter()
            .map(|i| format!("__wasm_split_load_chunk_{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            shim,
            "export const __wasm_split_load_{mname}_{hash_id}_{cname} = \
             makeLoad(\"./module_{idx}_{cname}.wasm\", [{deps}], fusedImports, initSync);",
            mname = module.module_name,
        )?;
    }
    // Wrap `fetch(url)` to resolve module-relative — without this
    // the chunk URLs (rewritten to `./`) resolve against the page
    // URL, not against __wasm_split.js's own location.
    let shim = shim.replace(
        "const response = await fetch(url);",
        "const response = await fetch(new URL(url, import.meta.url));",
    );
    fs::write(pkg_dir.join("__wasm_split.js"), shim)?;

    eprintln!(
        "[build-web] wasm-split: {} chunk wasm(s) emitted alongside {}_bg.wasm",
        chunk_count, lib_name,
    );
    Ok(())
}


/// Mirror `wrapper_pkg/` → `project_pkg/`. We don't trust an OS-level
/// symlink for this — the dev server's static-file logic uses
/// `is_file` checks that would follow the link but cache filenames,
/// and on Windows symlinks need admin. Plain copy is robust and
/// `pkg/` is small (a few hundred KB).
fn sync_pkg_dir(src: &Path, dst: &Path) -> Result<()> {
    if !src.is_dir() {
        anyhow::bail!(
            "wasm-pack reported success but {} doesn't exist",
            src.display()
        );
    }
    // Clean slate — wasm-pack sometimes leaves stale files behind
    // (e.g. renaming the lib renames the .js but leaves the old one).
    if dst.exists() {
        fs::remove_dir_all(dst).with_context(|| format!("remove stale {}", dst.display()))?;
    }
    fs::create_dir_all(dst).with_context(|| format!("create {}", dst.display()))?;
    // Recurse so wasm-pack subdirs (notably `snippets/<crate>-<hash>/`
    // for `#[wasm_bindgen(inline_js = ...)]` blocks) come along.
    // Missing snippets/ shows up at runtime as a 404 for
    // `pkg/snippets/.../inline*.js` which the main shim's `import`
    // tries to resolve. `pkg/` is small (a few hundred KB) so
    // straight copy stays cheap.
    copy_dir(src, dst)?;
    Ok(())
}

#[cfg(test)]
mod regression_tests {
    //! Regression coverage for how the app crate is invoked.
    //!
    //! This module used to assert on the *text* of a generated wrapper
    //! Cargo.toml and lib.rs — that the wrapper carried a direct
    //! `runtime-core` dep, that both boot paths named the same builtin
    //! set, that the runtime-server feature reached `backend-web`. All
    //! of that compensated for generated code being unchecked until
    //! someone ran a build.
    //!
    //! Those went with the wrapper. The boot sequence is ordinary
    //! compiled code in `idealyst::boot` now, so the compiler enforces
    //! what string-matching approximated. What's left here is what the
    //! compiler still can't see: the shape of the cargo invocation.

    use super::*;
    use std::io::Read;

    /// Framework features resolve against the facade; the app's own
    /// features pass through untouched. Getting this backwards fails at
    /// cargo time with a message that names neither the app nor the
    /// framework.
    #[test]
    fn framework_features_are_namespaced_app_features_are_not() {
        assert_eq!(feature_spec("robot"), "idealyst/robot");
        assert_eq!(feature_spec("runtime-server"), "idealyst/runtime-server");
        assert_eq!(feature_spec("hydrate"), "idealyst/hydrate");
        assert_eq!(feature_spec("my-feature"), "my-feature");
        assert_eq!(feature_spec("dev-hot-reload"), "dev-hot-reload");
        // An already-qualified spec is left alone.
        assert_eq!(feature_spec("runtime-core/dev"), "runtime-core/dev");
    }

    /// `aas` is the deprecated alias and must normalize. `idealyst/aas`
    /// would fail resolution; leaving it bare would build a bundle that
    /// mounts locally instead of connecting to the dev host, and saves
    /// would visibly do nothing.
    #[test]
    fn aas_alias_normalizes_to_runtime_server() {
        assert_eq!(feature_spec("aas"), "idealyst/runtime-server");
    }

    /// wasm-split needs `lto = "off"` (fat LTO inlines `#[wasm_split]`
    /// bodies back into their callers, leaving stub chunks) plus live
    /// symbols and line tables for call-graph matching. The wrapper
    /// carried these in its own `[profile.release]`; they now have to be
    /// injected per-invocation rather than written into a manifest the
    /// framework doesn't own.
    #[test]
    fn release_profile_config_preserves_what_wasm_split_needs() {
        let args = profile_config_args(true, DebugInfo::default(), DevOpt::default());
        let joined = args.join(" ");
        assert!(joined.contains("profile.release.lto=\"off\""), "{joined}");
        assert!(joined.contains("profile.release.strip=\"none\""), "{joined}");
        assert!(joined.contains("profile.release.debug=\"limited\""), "{joined}");
        assert!(joined.contains("profile.release.codegen-units=1"), "{joined}");
        assert_eq!(
            args.iter().filter(|a| *a == "--config").count(),
            args.len() / 2,
            "each setting needs its own --config flag: {args:?}",
        );
    }

    /// Build a `BuildOptions` whose every graph-invalidating field is at
    /// its default, for the `config_key` tests to perturb one at a time.
    fn key_opts() -> BuildOptions {
        BuildOptions {
            release: false,
            source: FrameworkSource::Workspace {
                root: PathBuf::from("/ws"),
            },
            premint: false,
            premint_only: false,
            premint_report: false,
            primitives: None,
            hydrate: false,
            strip_panics: false,
            gzip: false,
            brotli: false,
            wasm_split: true,
            debuginfo: DebugInfo::default(),
            dev_opt: DevOpt::default(),
            user_features: Vec::new(),
            // Not graph-invalidating: it rewrites the staged
            // `index.html`, never the cargo build.
            robot_relay_url: None,
            bundle_out_dir: None,
            prune_dead_data_min: None,
        }
    }

    /// Regression guard for the thrash that made a dev rebuild look like
    /// a cold build: `idealyst dev --web` builds with `idealyst/hydrate`
    /// ON and `idealyst build --web` builds it OFF, and both used one
    /// `target/idealyst-web`. Different feature unification, same
    /// directory — so alternating the two commands recompiled the entire
    /// framework each way, and you could find both fingerprints sitting
    /// in the dir as two `libidea_ui-*.rlib`s.
    ///
    /// Keying the target dir by config is what turns that into a
    /// directory switch instead of a rebuild.
    #[test]
    fn regression_hydrate_toggle_does_not_share_a_target_dir() {
        let mut dev = key_opts();
        dev.hydrate = true;
        let build = key_opts();
        assert_ne!(
            config_key(&dev),
            config_key(&build),
            "dev (hydrate on) and build (hydrate off) must not evict each other",
        );
    }

    /// Every flag that reaches cargo through `CARGO_ENCODED_RUSTFLAGS`,
    /// the feature list, or the profile invalidates the WHOLE graph, so
    /// each must key a distinct target dir. Missing one silently
    /// reintroduces the full-rebuild-on-toggle behavior.
    #[test]
    fn config_key_separates_every_graph_invalidating_flag() {
        let base = config_key(&key_opts());
        let mut seen = vec![base.clone()];
        let mutate: Vec<(&str, fn(&mut BuildOptions))> = vec![
            ("release", |o| o.release = true),
            ("premint", |o| o.premint = true),
            ("premint_only", |o| o.premint_only = true),
            ("premint_report", |o| o.premint_report = true),
            ("wasm_split", |o| o.wasm_split = false),
            ("strip_panics", |o| o.strip_panics = true),
            ("hydrate", |o| o.hydrate = true),
            ("debuginfo", |o| o.debuginfo = DebugInfo::Full),
            ("dev_opt", |o| o.dev_opt = DevOpt::Fast),
            ("user_features", |o| o.user_features = vec!["vello".into()]),
        ];
        for (name, f) in mutate {
            let mut o = key_opts();
            f(&mut o);
            let k = config_key(&o);
            assert_ne!(k, base, "{name} must change the target dir key");
            assert!(!seen.contains(&k), "{name} collided with an earlier key");
            seen.push(k);
        }
    }

    /// Fields that do NOT change what cargo compiles must not fragment
    /// the cache. `gzip`/`brotli` rewrite staged output bytes and
    /// `bundle_out_dir` only says where to copy — keying on them would
    /// mean a second full dependency build for nothing.
    #[test]
    fn config_key_ignores_post_cargo_only_options() {
        let base = config_key(&key_opts());
        for f in [
            (|o: &mut BuildOptions| o.gzip = true) as fn(&mut BuildOptions),
            |o: &mut BuildOptions| o.brotli = true,
            |o: &mut BuildOptions| o.bundle_out_dir = Some(PathBuf::from("/out")),
            |o: &mut BuildOptions| o.prune_dead_data_min = Some(64),
        ] {
            let mut o = key_opts();
            f(&mut o);
            assert_eq!(config_key(&o), base, "post-cargo options must not fragment");
        }
    }

    /// Regression guard: the target-dir key must not depend on enum
    /// DECLARATION ORDER.
    ///
    /// `config_key` originally hashed `dev_opt as u8`. When `Optimized`
    /// was promoted to the default and moved to the front of the enum,
    /// its discriminant became `0` — the value `Fast` used to have — so
    /// the new default silently inherited the directory full of `Fast`
    /// artifacts and the next build recompiled the entire framework for
    /// no visible reason. Keys must come from stable tags.
    #[test]
    fn regression_config_key_does_not_depend_on_enum_discriminants() {
        assert_eq!(DevOpt::Optimized.key_tag(), "opt-optimized");
        assert_eq!(DevOpt::Fast.key_tag(), "opt-fast");
        assert_ne!(DevOpt::Optimized.key_tag(), DevOpt::Fast.key_tag());
        // And the key itself must actually consume the tag.
        let mut a = key_opts();
        a.dev_opt = DevOpt::Optimized;
        let mut b = key_opts();
        b.dev_opt = DevOpt::Fast;
        assert_ne!(config_key(&a), config_key(&b));
    }

    /// Feature order is a caller detail — cargo receives the list sorted
    /// and deduped, so two spellings of one set must land in one dir.
    #[test]
    fn config_key_is_stable_across_feature_ordering() {
        let mut a = key_opts();
        a.user_features = vec!["vello".into(), "robot".into()];
        let mut b = key_opts();
        b.user_features = vec!["robot".into(), "vello".into(), "robot".into()];
        assert_eq!(config_key(&a), config_key(&b));
    }

    /// `--dev-opt optimized` is the historical posture: optimize
    /// DEPENDENCIES only — the `"*"` glob excludes workspace members, so
    /// the app crate stays fast to compile while compute-heavy deps don't
    /// run 10-40x slower.
    #[test]
    fn dev_opt_optimized_optimizes_dependencies_only() {
        let joined = profile_config_args(false, DebugInfo::default(), DevOpt::Optimized).join(" ");
        assert!(
            joined.contains("profile.dev.package.\"*\".opt-level=3"),
            "{joined}"
        );
        assert!(
            !joined.contains("profile.dev.opt-level="),
            "workspace members must inherit the workspace profile here: {joined}"
        );
    }

    /// `--dev-opt fast` drops workspace members to `opt-level = 0`.
    ///
    /// That half is the one that does anything: `profile.dev.package."*"`
    /// does NOT match workspace members (cargo: "for any non-workspace
    /// member"), so setting only the glob leaves every framework crate at
    /// the workspace's `opt-level` — which is the whole cost of a
    /// framework-crate edit.
    #[test]
    fn dev_opt_fast_drops_workspace_members_to_zero() {
        let joined = profile_config_args(false, DebugInfo::default(), DevOpt::Fast).join(" ");
        assert!(joined.contains("profile.dev.opt-level=0"), "{joined}");
        assert!(
            joined.contains("profile.dev.package.\"*\".opt-level=1"),
            "deps stay lightly optimized — they rebuild rarely: {joined}"
        );
        assert!(
            !joined.contains("opt-level=3"),
            "the 3 is what `Fast` exists to avoid: {joined}"
        );
    }

    /// `Optimized` is the default.
    ///
    /// `Fast` cuts the cargo half but inflates the module ~37%, and
    /// wasm-bindgen + wasm-split are O(module size) and run on EVERY
    /// rebuild — so end-to-end it loses in the default (incremental-on)
    /// regime, on both framework-crate and leaf edits. Pinned here
    /// because the cargo-only numbers point the other way, and someone
    /// reading only those would flip this back.
    #[test]
    fn dev_opt_defaults_to_optimized() {
        assert_eq!(DevOpt::default(), DevOpt::Optimized);
        assert_eq!(
            profile_config_args(false, DebugInfo::default(), DevOpt::default()),
            profile_config_args(false, DebugInfo::default(), DevOpt::Optimized),
        );
    }

    /// The posture is a DEV-profile concern. Release has its own fixed
    /// profile (`opt-level = "z"`, `codegen-units = 1`, …) that
    /// `--dev-opt` must not perturb — a shipped bundle's size posture
    /// cannot depend on an iteration-speed flag.
    #[test]
    fn dev_opt_never_touches_the_release_profile() {
        let fast = profile_config_args(true, DebugInfo::default(), DevOpt::Fast);
        let optimized = profile_config_args(true, DebugInfo::default(), DevOpt::Optimized);
        assert_eq!(fast, optimized);
        assert!(!fast.join(" ").contains("profile.dev"));
    }

    /// Unknown values are rejected rather than silently falling back —
    /// a typo'd `--dev-opt fst` that quietly built the other posture
    /// would produce timings nobody could explain.
    #[test]
    fn dev_opt_rejects_unknown_values() {
        assert_eq!(DevOpt::from_cli("fast").unwrap(), DevOpt::Fast);
        assert_eq!(DevOpt::from_cli("optimized").unwrap(), DevOpt::Optimized);
        assert!(DevOpt::from_cli("fst").is_err());
    }

    /// The dev default trims debuginfo on BOTH halves of the graph. Only
    /// setting `profile.dev.debug` would leave every dependency at
    /// cargo's `2`, which is where most of the bytes are — the app crate
    /// is one crate out of ~70.
    #[test]
    fn dev_profile_config_trims_debuginfo_for_deps_and_app_alike() {
        let joined = profile_config_args(false, DebugInfo::default(), DevOpt::default()).join(" ");
        assert!(joined.contains("profile.dev.debug=\"line-tables-only\""), "{joined}");
        assert!(
            joined.contains("profile.dev.package.\"*\".debug=\"line-tables-only\""),
            "{joined}"
        );
    }

    /// `lto = false` (cargo's dev default) still runs thin-local LTO;
    /// only `"off"` disables it.
    #[test]
    fn dev_profile_config_disables_thin_local_lto() {
        let joined = profile_config_args(false, DebugInfo::default(), DevOpt::default()).join(" ");
        assert!(joined.contains("profile.dev.lto=\"off\""), "{joined}");
    }

    /// `--debuginfo full` has to reach cargo as `2`, not as the string
    /// `"full"` — cargo rejects that and the build dies before compiling.
    #[test]
    fn debuginfo_levels_map_onto_cargos_own_spelling() {
        let full = profile_config_args(false, DebugInfo::Full, DevOpt::default()).join(" ");
        assert!(full.contains("profile.dev.debug=2"), "{full}");
        let none = profile_config_args(false, DebugInfo::None, DevOpt::default()).join(" ");
        assert!(none.contains("profile.dev.debug=0"), "{none}");
    }

    /// Release owns its own debug posture (`"limited"`, which wasm-split's
    /// call-graph matching needs); `--debuginfo` must not reach it.
    #[test]
    fn debuginfo_flag_does_not_touch_the_release_profile() {
        for level in [DebugInfo::LineTables, DebugInfo::Full, DebugInfo::None] {
            let joined = profile_config_args(true, level, DevOpt::default()).join(" ");
            assert!(joined.contains("profile.release.debug=\"limited\""), "{joined}");
            assert!(!joined.contains("profile.dev."), "{joined}");
        }
    }

    #[test]
    fn debuginfo_cli_values_parse_and_reject_typos() {
        assert_eq!(DebugInfo::from_cli("line-tables").unwrap(), DebugInfo::LineTables);
        assert_eq!(DebugInfo::from_cli("full").unwrap(), DebugInfo::Full);
        assert_eq!(DebugInfo::from_cli("none").unwrap(), DebugInfo::None);
        let err = DebugInfo::from_cli("lines").unwrap_err().to_string();
        assert!(err.contains("line-tables"), "{err}");
    }

    /// A crate with no binary target can't be an app any more. The error
    /// has to name the fix — cargo's own "no bin target named …" says
    /// nothing about `entry!`.
    #[test]
    fn missing_entry_point_is_reported_in_the_authors_terms() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("demo");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::write(project.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
        std::fs::write(project.join("src/lib.rs"), "pub fn app() {}").unwrap();

        let err = ensure_entry_point(&project, "demo").expect_err("no bin target");
        let msg = format!("{err:#}");
        assert!(msg.contains("idealyst::entry!"), "names the fix: {msg}");
        assert!(msg.contains("src/main.rs"), "names where it goes: {msg}");
    }

    /// `src/main.rs` is the conventional entry, but an explicit
    /// `[[bin]]` can put it anywhere — don't reject those.
    #[test]
    fn explicit_bin_target_satisfies_the_entry_point_check() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("demo");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::write(
            project.join("Cargo.toml"),
            "[package]\nname = \"demo\"\n\n[[bin]]\nname = \"demo\"\npath = \"src/entry.rs\"\n",
        )
        .unwrap();
        ensure_entry_point(&project, "demo").expect("explicit [[bin]] is an entry point");

        let plain = tmp.path().join("plain");
        std::fs::create_dir_all(plain.join("src")).unwrap();
        std::fs::write(plain.join("Cargo.toml"), "[package]\nname = \"plain\"\n").unwrap();
        std::fs::write(plain.join("src/main.rs"), "fn main() {}").unwrap();
        ensure_entry_point(&plain, "plain").expect("src/main.rs is an entry point");
    }

    /// A `[[bin]]` mentioned in PROSE is not a bin target.
    ///
    /// `idealyst new`'s generated Cargo.toml carries the sentence "No
    /// `catalog` feature or `[[bin]] catalog` is needed…", and the check
    /// used to substring-match the whole file — so every freshly
    /// scaffolded project looked like it had an entry point and the
    /// author got cargo's bare `no bin target named <app>` instead of
    /// the message above.
    #[test]
    fn regression_bin_target_in_a_comment_is_not_an_entry_point() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("demo");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::write(
            project.join("Cargo.toml"),
            "[package]\nname = \"demo\"\n\n\
             # No `catalog` feature or `[[bin]] catalog` is needed for the MCP server.\n\
             [dependencies]\n",
        )
        .unwrap();
        std::fs::write(project.join("src/lib.rs"), "pub fn app() {}").unwrap();

        let err = ensure_entry_point(&project, "demo")
            .expect_err("a commented-out [[bin]] must not count as a bin target");
        assert!(format!("{err:#}").contains("idealyst::entry!"));
    }

    /// A trailing comment on the real table header must still count.
    #[test]
    fn bin_target_with_a_trailing_comment_is_an_entry_point() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("demo");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::write(
            project.join("Cargo.toml"),
            "[package]\nname = \"demo\"\n\n[[bin]] # the app entry\nname = \"demo\"\n",
        )
        .unwrap();
        ensure_entry_point(&project, "demo").expect("real [[bin]] header, comment and all");
    }

    #[test]
    fn primitive_set_presets_and_lists_resolve() {
        assert_eq!(resolve_primitive_set(None).unwrap(), None);

        let core = resolve_primitive_set(Some(&["core".to_string()])).unwrap();
        assert_eq!(core, Some(vec!["view".to_string(), "text".to_string()]));

        let all = resolve_primitive_set(Some(&["all".to_string()])).unwrap();
        assert_eq!(all.unwrap().len(), SELECTABLE_PRIMITIVES.len());

        // Explicit list: order preserved, duplicates collapsed, case and
        // whitespace normalized.
        let list = resolve_primitive_set(Some(&[
            "View".to_string(),
            " text ".to_string(),
            "view".to_string(),
        ]))
        .unwrap();
        assert_eq!(list, Some(vec!["view".to_string(), "text".to_string()]));
    }

    /// A typo must fail at the CLI with the valid names, not as a
    /// `no rules expected this token` deep inside a generated file.
    #[test]
    fn unknown_primitive_is_rejected_with_the_valid_set() {
        let err = resolve_primitive_set(Some(&["flat_list".to_string()]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("`flat_list` is not a builtin primitive"), "{err}");
        assert!(err.contains("virtualizer"), "message must list valid names: {err}");

        let empty = resolve_primitive_set(Some(&["".to_string()]))
            .unwrap_err()
            .to_string();
        assert!(empty.contains("no names"), "{empty}");
    }

    fn fake_project(tmp: &Path) -> PathBuf {
        let project = tmp.join("proj");
        fs::create_dir_all(project.join("src")).unwrap();
        fs::create_dir_all(project.join("target/debug")).unwrap();
        // A stale project-root pkg/ left over from a previous build —
        // bundling must NOT pick it up (the freshly built pkg comes
        // straight from the wasm-pack output dir).
        fs::create_dir_all(project.join("pkg")).unwrap();
        fs::create_dir_all(project.join("fonts")).unwrap();
        fs::create_dir_all(project.join("assets/images")).unwrap();
        fs::create_dir_all(project.join(".git")).unwrap();
        fs::write(project.join("Cargo.toml"), b"[package]\nname = 'demo'\n").unwrap();
        fs::write(project.join("Cargo.lock"), b"").unwrap();
        fs::write(project.join("index.html"), b"<html><body>hi</body></html>").unwrap();
        fs::write(project.join("src/lib.rs"), b"pub fn app() {}").unwrap();
        fs::write(project.join("target/debug/junk"), b"big-binary").unwrap();
        fs::write(project.join("pkg/STALE_FROM_OLD_BUILD.wasm"), b"old-bytes").unwrap();
        fs::write(project.join("fonts/Inter.ttf"), b"font-bytes").unwrap();
        fs::write(project.join("assets/images/logo.png"), b"png-bytes").unwrap();
        // Internal docs / configs that historically leaked into the
        // served bundle (Field report 3.3). They must NOT ship.
        fs::create_dir_all(project.join("design-files")).unwrap();
        fs::write(project.join("design-files/mock.fig"), b"figma").unwrap();
        fs::write(project.join("FEEDBACK.md"), b"# internal notes").unwrap();
        fs::write(project.join("README.md"), b"# readme").unwrap();
        fs::write(project.join("LICENSE"), b"MIT").unwrap();
        fs::write(project.join("dev.toml"), b"secret = 'value'").unwrap();
        // A real web asset that MUST still auto-ship.
        fs::write(project.join("robots.txt"), b"User-agent: *").unwrap();
        fs::create_dir_all(project.join("public")).unwrap();
        fs::write(project.join("public/manifest.json"), b"{}").unwrap();
        project
    }

    fn read_gzipped(path: &Path) -> Vec<u8> {
        let raw = fs::read(path).expect("read gz");
        let mut dec = flate2::read::GzDecoder::new(&raw[..]);
        let mut out = Vec::new();
        dec.read_to_end(&mut out).expect("decode gz");
        out
    }

    #[test]
    fn stage_bundle_keeps_assets_skips_sources_and_pkg() {
        let tmp = tempfile::tempdir().unwrap();
        let project = fake_project(tmp.path());
        let out = tmp.path().join("dist");

        stage_bundle(&project, &out, None, &[]).expect("stage");

        assert!(
            out.join("index.html").is_file(),
            "index.html must be copied"
        );
        assert!(
            out.join("fonts/Inter.ttf").is_file(),
            "top-level asset dir (fonts/) must auto-ship",
        );
        assert!(
            out.join("assets/images/logo.png").is_file(),
            "nested asset paths must auto-ship",
        );
        assert!(
            out.join("public/manifest.json").is_file(),
            "public/ must auto-ship",
        );
        assert!(
            out.join("robots.txt").is_file(),
            "robots.txt must auto-ship",
        );
        assert!(!out.join("src").exists(), "src/ must be skipped");
        assert!(!out.join("target").exists(), "target/ must be skipped");
        assert!(!out.join(".git").exists(), "dotdirs must be skipped");
        assert!(
            !out.join("Cargo.toml").exists(),
            "Cargo.toml must be skipped"
        );
        // Field report 3.3 (SECURITY): internal docs/configs and the
        // design-files/ folder must NEVER be staged into the served
        // bundle — they previously leaked to the public site root.
        assert!(
            !out.join("FEEDBACK.md").exists(),
            "FEEDBACK.md (internal doc) must NOT ship",
        );
        assert!(
            !out.join("README.md").exists(),
            "README.md must NOT ship",
        );
        assert!(!out.join("LICENSE").exists(), "LICENSE must NOT ship");
        assert!(
            !out.join("dev.toml").exists(),
            "dev.toml (arbitrary config) must NOT ship — all *.toml is excluded",
        );
        assert!(
            !out.join("design-files").exists(),
            "design-files/ folder must NOT ship",
        );
        assert!(
            !out.join("Cargo.lock").exists(),
            "Cargo.lock (and all *.lock) must NOT ship",
        );
        // Bundling owns pkg/ — it gets populated from wasm-pack output
        // by the caller, NOT scraped out of the project root. A stale
        // project-root pkg/ from a previous build must not leak in,
        // or deployments would ship outdated wasm.
        assert!(
            !out.join("pkg").exists(),
            "stage_bundle must not copy project/pkg/ — the caller copies wrapper_pkg straight in",
        );
    }

    #[test]
    fn stage_bundle_allowlist_ships_only_declared_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let project = fake_project(tmp.path());
        let out = tmp.path().join("dist");

        // Declare an explicit allowlist: only these top-level entries
        // (plus the always-needed index.html) may ship.
        let assets = vec!["assets".to_string(), "robots.txt".to_string()];
        stage_bundle(&project, &out, None, &assets).expect("stage");

        // index.html is always staged, even when not listed.
        assert!(out.join("index.html").is_file(), "index.html always ships");
        // Declared entries ship.
        assert!(
            out.join("assets/images/logo.png").is_file(),
            "declared `assets` dir must ship (recursively)",
        );
        assert!(
            out.join("robots.txt").is_file(),
            "declared robots.txt must ship",
        );
        // Everything NOT declared is skipped — including otherwise-safe
        // assets like fonts/ and public/. Explicit means explicit.
        assert!(
            !out.join("fonts").exists(),
            "fonts/ was not in the allowlist, so it must NOT ship",
        );
        assert!(
            !out.join("public").exists(),
            "public/ was not in the allowlist, so it must NOT ship",
        );
        // And of course no internal docs/config can leak.
        assert!(!out.join("FEEDBACK.md").exists(), "FEEDBACK.md must NOT ship");
        assert!(!out.join("dev.toml").exists(), "dev.toml must NOT ship");
        assert!(
            !out.join("design-files").exists(),
            "design-files/ must NOT ship",
        );
    }

    #[test]
    fn stage_bundle_allowlist_rejects_path_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let project = fake_project(tmp.path());
        let out = tmp.path().join("dist");

        let assets = vec!["../secret".to_string()];
        let err = stage_bundle(&project, &out, None, &assets).unwrap_err();
        assert!(
            err.to_string().contains("invalid web `assets` entry"),
            "allowlist must reject path-escaping entries, got: {err}",
        );
    }

    #[test]
    fn stage_bundle_errors_without_index_html_when_no_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        fs::create_dir_all(&project).unwrap();
        let err = stage_bundle(&project, &tmp.path().join("dist"), None, &[]).unwrap_err();
        assert!(
            err.to_string().contains("index.html"),
            "missing-index error should mention index.html, got: {err}",
        );
    }

    #[test]
    fn stage_bundle_synthesizes_default_index_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        // A project with NO index.html (and some other asset to copy).
        let project = tmp.path().join("proj");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("robots.txt"), b"User-agent: *").unwrap();
        let out = tmp.path().join("dist");

        let html = default_index_html("My App", "my_app");
        stage_bundle(&project, &out, Some(&html), &[]).expect("stage with fallback");

        // The default index is written into the STAGED dir...
        let staged_index = out.join("index.html");
        assert!(staged_index.is_file(), "default index.html must be staged");
        let contents = fs::read_to_string(&staged_index).unwrap();
        assert!(
            contents.contains("/pkg/my_app.js"),
            "default index must boot the lib's wasm, got:\n{contents}",
        );
        // ...and NOT back into the project source tree.
        assert!(
            !project.join("index.html").exists(),
            "synthesizing a default must never touch the project source tree",
        );
        // Other assets still copy.
        assert!(out.join("robots.txt").is_file(), "non-source assets still copy");
    }

    #[test]
    fn stage_bundle_prefers_project_index_over_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let project = fake_project(tmp.path()); // writes its own index.html
        let out = tmp.path().join("dist");

        let fallback = default_index_html("Fallback", "fallback_lib");
        stage_bundle(&project, &out, Some(&fallback), &[]).expect("stage");

        let contents = fs::read_to_string(out.join("index.html")).unwrap();
        assert!(
            !contents.contains("fallback_lib"),
            "a project's own index.html must win over the fallback, got:\n{contents}",
        );
    }

    #[test]
    fn stage_bundle_replaces_prior_output() {
        let tmp = tempfile::tempdir().unwrap();
        let project = fake_project(tmp.path());
        let out = tmp.path().join("dist");

        // Pretend a previous build left a stale artifact behind.
        fs::create_dir_all(&out).unwrap();
        fs::write(out.join("ghost.wasm"), b"old").unwrap();

        stage_bundle(&project, &out, None, &[]).expect("stage");
        assert!(
            !out.join("ghost.wasm").exists(),
            "stale files from a prior bundle must be cleared so renamed/removed assets don't leak",
        );
    }

    #[test]
    fn strip_wasm_pack_metadata_drops_housekeeping_only() {
        let tmp = tempfile::tempdir().unwrap();
        let pkg = tmp.path().join("pkg");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(pkg.join("demo_bg.wasm"), b"wasm").unwrap();
        fs::write(pkg.join("demo.js"), b"js").unwrap();
        fs::write(pkg.join("demo.d.ts"), b"types").unwrap();
        fs::write(pkg.join("demo_bg.wasm.d.ts"), b"types").unwrap();
        fs::write(pkg.join("package.json"), b"{}").unwrap();
        fs::write(pkg.join("README.md"), b"# pkg").unwrap();

        strip_wasm_pack_metadata(&pkg);

        assert!(pkg.join("demo_bg.wasm").is_file(), ".wasm must stay");
        assert!(pkg.join("demo.js").is_file(), ".js must stay");
        assert!(!pkg.join("demo.d.ts").exists(), ".d.ts must be stripped");
        assert!(
            !pkg.join("demo_bg.wasm.d.ts").exists(),
            ".d.ts must be stripped"
        );
        assert!(
            !pkg.join("package.json").exists(),
            "package.json must be stripped"
        );
        assert!(
            !pkg.join("README.md").exists(),
            "README.md must be stripped"
        );
    }

    #[test]
    fn brotli_precompress_emits_siblings_and_skips_binaries() {
        let tmp = tempfile::tempdir().unwrap();
        let project = fake_project(tmp.path());
        let out = tmp.path().join("dist");
        stage_bundle(&project, &out, None, &[]).expect("stage");

        let pkg = out.join("pkg");
        fs::create_dir_all(&pkg).unwrap();
        // Long-and-repetitive so compression clearly wins and the
        // smaller-than-original keep-rule can't flake.
        let wasm_raw = "abcdefgh".repeat(2000).into_bytes();
        fs::write(pkg.join("demo_bg.wasm"), &wasm_raw).unwrap();
        let png_raw = fs::read(out.join("assets/images/logo.png")).unwrap();

        brotli_precompress_bundle(&out).expect("brotli");

        // SIBLING model: the original must be untouched (hosts without
        // brotli support serve it as-is)...
        assert_eq!(
            fs::read(pkg.join("demo_bg.wasm")).unwrap(),
            wasm_raw,
            "original wasm must be left byte-identical — .br is a sibling, not a rewrite",
        );
        // ...and the .br must exist, be smaller, and round-trip.
        let br = fs::read(pkg.join("demo_bg.wasm.br")).expect(".br sibling must be emitted");
        assert!(
            br.len() < wasm_raw.len(),
            "brotli must shrink the wasm (was {}, .br is {})",
            wasm_raw.len(),
            br.len(),
        );
        let mut decoded = Vec::new();
        brotli::Decompressor::new(&br[..], 4096)
            .read_to_end(&mut decoded)
            .expect("decode .br");
        assert_eq!(decoded, wasm_raw, ".br must round-trip to the original bytes");

        // Already-compressed formats get no sibling.
        assert!(
            !out.join("assets/images/logo.png.br").exists(),
            ".png must not get a .br sibling",
        );
        assert_eq!(
            fs::read(out.join("assets/images/logo.png")).unwrap(),
            png_raw,
            ".png bytes untouched",
        );

        // Idempotence: a second pass must not produce `.br.br`.
        brotli_precompress_bundle(&out).expect("brotli again");
        assert!(
            !pkg.join("demo_bg.wasm.br.br").exists(),
            "re-running must skip existing .br siblings",
        );
    }

    #[test]
    fn gzip_bundle_compresses_text_skips_binaries() {
        let tmp = tempfile::tempdir().unwrap();
        let project = fake_project(tmp.path());
        let out = tmp.path().join("dist");
        stage_bundle(&project, &out, None, &[]).expect("stage");

        // Drop in a synthetic pkg/ the way `build()` would after
        // copying from `wrapper_pkg`. Wasm body is intentionally
        // long-and-repetitive so gzip noticeably shrinks it; without
        // that the test could flake on tiny inputs where the gzip
        // header outweighs the savings.
        let pkg = out.join("pkg");
        fs::create_dir_all(&pkg).unwrap();
        let wasm_raw = "abcdefgh".repeat(2000).into_bytes();
        fs::write(pkg.join("demo_bg.wasm"), &wasm_raw).unwrap();
        fs::write(pkg.join("demo.js"), b"export default function init() {}").unwrap();
        let png_raw = fs::read(out.join("assets/images/logo.png")).unwrap();

        gzip_bundle(&out).expect("gzip");

        // Compressible: wasm replaced by gzip bytes; filename unchanged
        // so the same `Content-Encoding: gzip` response can serve them.
        let wasm_after = fs::read(out.join("pkg/demo_bg.wasm")).unwrap();
        assert_ne!(
            wasm_raw, wasm_after,
            "wasm must be replaced by gzipped bytes (filename preserved)",
        );
        assert!(
            wasm_after.len() < wasm_raw.len(),
            "gzip must shrink the wasm (was {}, now {})",
            wasm_raw.len(),
            wasm_after.len(),
        );
        assert_eq!(
            read_gzipped(&out.join("pkg/demo_bg.wasm")),
            wasm_raw,
            "gzipped wasm must round-trip back to the original bytes",
        );

        // Pre-compressed formats must be left alone — re-gzipping
        // wastes bytes and would confuse the host's Content-Type
        // routing.
        assert_eq!(
            fs::read(out.join("assets/images/logo.png")).unwrap(),
            png_raw,
            ".png must not be re-compressed",
        );
    }

    #[test]
    fn sync_and_inject_web_icons_is_noop_without_block() {
        let tmp = tempfile::tempdir().unwrap();
        let project = fake_project(tmp.path());
        let out = tmp.path().join("dist");
        stage_bundle(&project, &out, None, &[]).unwrap();
        let html_before = fs::read_to_string(out.join("index.html")).unwrap();

        // `fake_project`'s Cargo.toml has no icon block, so the
        // helper must leave the bundle untouched — no extra files,
        // no HTML rewrite.
        sync_and_inject_web_icons(&project, &out).unwrap();

        assert!(!out.join("favicon.ico").exists());
        assert!(!out.join("favicon-192.png").exists());
        assert!(!out.join("favicon-512.png").exists());
        assert!(!out.join("apple-touch-icon.png").exists());
        assert_eq!(
            fs::read_to_string(out.join("index.html")).unwrap(),
            html_before,
            "no icon block → index.html must be byte-identical",
        );
    }

    #[test]
    fn sync_and_inject_web_icons_emits_files_and_link_tags() {
        let tmp = tempfile::tempdir().unwrap();
        let project = fake_project(tmp.path());
        // Append an icon block + drop an SVG next to Cargo.toml. Both
        // are stripped from the bundle (Cargo.toml is excluded; the
        // SVG isn't a top-level asset directory) so this only affects
        // the icon-gen pipeline.
        fs::write(
            project.join("Cargo.toml"),
            b"[package]\nname = 'demo'\n\n\
              [package.metadata.idealyst.app.icon]\n\
              source = 'icon.svg'\n",
        )
        .unwrap();
        fs::write(
            project.join("icon.svg"),
            br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" width="64" height="64">
  <rect width="64" height="64" fill="#ff7a00"/>
</svg>"##,
        )
        .unwrap();

        let out = tmp.path().join("dist");
        stage_bundle(&project, &out, None, &[]).unwrap();
        sync_and_inject_web_icons(&project, &out).unwrap();

        for name in [
            "favicon.ico",
            "favicon-192.png",
            "favicon-512.png",
            "apple-touch-icon.png",
        ] {
            assert!(
                out.join(name).is_file(),
                "{name} must be written into the bundle root",
            );
        }

        let html = fs::read_to_string(out.join("index.html")).unwrap();
        // Tag presence + the specific hrefs we emit. Using the
        // attribute strings keeps the test sensitive to accidental
        // path changes (e.g. someone dropping the leading slash).
        for fragment in [
            r#"rel="icon" type="image/x-icon" href="/favicon.ico""#,
            r#"href="/favicon-192.png""#,
            r#"href="/favicon-512.png""#,
            r#"rel="apple-touch-icon" href="/apple-touch-icon.png""#,
        ] {
            assert!(
                html.contains(fragment),
                "index.html must contain `{fragment}`, got:\n{html}",
            );
        }
    }

    #[test]
    fn is_already_compressed_covers_common_web_assets() {
        // Sanity: the skip-list keys off lowercase extension. A
        // capital-letter extension (.PNG from a careless author) must
        // still skip.
        assert!(is_already_compressed(Path::new("a.png")));
        assert!(is_already_compressed(Path::new("a.PNG")));
        assert!(is_already_compressed(Path::new("a.woff2")));
        assert!(is_already_compressed(Path::new("a.mp4")));
        assert!(!is_already_compressed(Path::new("a.wasm")));
        assert!(!is_already_compressed(Path::new("a.js")));
        assert!(!is_already_compressed(Path::new("a.html")));
        assert!(!is_already_compressed(Path::new("a.ttf")));
    }
}

#[cfg(test)]
mod wasm_split_gate_tests {
    //! Coverage for `--no-split`, whose contract is "bundle the lazy body
    //! anyway" — NOT "drop the boundary". The module keeps importing what
    //! `#[wasm_split]` declared, so the build has to answer those imports
    //! itself; getting that wrong is a module the browser refuses to
    //! instantiate.

    use super::*;

    /// The two imports the macro emits per split point, verbatim from the
    /// website's pre-split module (`#[component(lazy)]` on
    /// `HeroSimulator`). Hard-coded rather than paraphrased: the
    /// `_import_` ⇄ `_export_` twin rule is a naming contract with
    /// `wasm-split-macro`, and a paraphrase would still pass while the
    /// real names drifted.
    const LOAD_IMPORT: &str =
        "__wasm_split_load___idealyst_lazy_HeroSimulator_a3558c2587326b3cb37c2057039bbac0___lazy_body";
    const BODY_IMPORT: &str = "__wasm_split_00_____idealyst_lazy_HeroSimulator___00_import_\
                               a3558c2587326b3cb37c2057039bbac0___lazy_body";
    const BODY_EXPORT: &str = "__wasm_split_00_____idealyst_lazy_HeroSimulator___00_export_\
                               a3558c2587326b3cb37c2057039bbac0___lazy_body";

    fn tmpdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("idealyst-split-gate-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A minimal valid wasm module that imports two functions from
    /// `./__wasm_split.js` (plus one from elsewhere, which must be
    /// ignored). Hand-encoded so the test needs no wasm toolchain.
    fn wasm_importing_split_loader() -> Vec<u8> {
        fn leb(mut n: u32, out: &mut Vec<u8>) {
            loop {
                let mut b = (n & 0x7f) as u8;
                n >>= 7;
                if n != 0 {
                    b |= 0x80;
                }
                out.push(b);
                if n == 0 {
                    break;
                }
            }
        }
        fn name(s: &str, out: &mut Vec<u8>) {
            leb(s.len() as u32, out);
            out.extend_from_slice(s.as_bytes());
        }

        // Type section: one `() -> ()`.
        let mut types = vec![0x01, 0x60, 0x00, 0x00];
        // Import section: three function imports of that type.
        let mut imports = Vec::new();
        leb(3, &mut imports);
        for (module, field) in [
            (WASM_SPLIT_LOADER, LOAD_IMPORT),
            (WASM_SPLIT_LOADER, BODY_IMPORT),
            ("./other.js", "unrelated"),
        ] {
            name(module, &mut imports);
            name(field, &mut imports);
            imports.push(0x00); // kind: func
            leb(0, &mut imports); // type index
        }

        let mut wasm = b"\0asm\x01\0\0\0".to_vec();
        for (id, body) in [(1u8, &mut types), (2u8, &mut imports)] {
            wasm.push(id);
            leb(body.len() as u32, &mut wasm);
            wasm.append(body);
        }
        wasm
    }

    #[test]
    fn import_scan_finds_split_imports_and_ignores_the_rest() {
        let found = wasm_split_imports(&wasm_importing_split_loader()).unwrap();
        assert_eq!(found, vec![LOAD_IMPORT.to_string(), BODY_IMPORT.to_string()]);
    }

    /// An app with no lazy boundary imports nothing from the loader, which
    /// is what lets `--no-split` skip writing a loader at all there.
    #[test]
    fn import_scan_is_empty_for_a_module_without_split_points() {
        let wasm = b"\0asm\x01\0\0\0".to_vec();
        assert!(wasm_split_imports(&wasm).unwrap().is_empty());
    }

    /// The generated loader must answer BOTH imports: the load function
    /// (resolve now — the body never left) and the body itself (forward to
    /// the `_export_` twin the main module still carries).
    #[test]
    fn inline_loader_answers_every_import_the_module_declares() {
        let tmp = tmpdir("inline");
        let pkg = tmp.join("pkg");
        fs::create_dir_all(&pkg).unwrap();

        let inlined = write_inline_split_loader(
            &pkg,
            "demo",
            &[LOAD_IMPORT.to_string(), BODY_IMPORT.to_string()],
        )
        .unwrap();
        assert_eq!(inlined, 1, "one lazy boundary → one loader");

        let js = fs::read_to_string(pkg.join("__wasm_split.js")).unwrap();
        assert!(js.contains(&format!("export const {LOAD_IMPORT} = loadedAlready;")), "{js}");
        assert!(js.contains(&format!("export function {BODY_IMPORT}(")), "{js}");
        // The forwarder must target the EXPORT twin, not re-enter itself.
        assert!(js.contains(&format!("main()[\"{BODY_EXPORT}\"]")), "{js}");
        assert!(js.contains("import { initSync } from \"./demo.js\";"), "{js}");
        fs::remove_dir_all(&tmp).ok();
    }

    /// The callback arrives as a function-table index and MUST be invoked —
    /// `SplitLoaderFuture` stays `Pending` forever otherwise, so the lazy
    /// component would show its `loading` state until the tab closed. The
    /// microtask defer keeps it from firing inside `poll`, which is still
    /// installing its waker when it calls the loader.
    #[test]
    fn inline_loader_wakes_the_future_through_the_function_table() {
        let tmp = tmpdir("wake");
        let pkg = tmp.join("pkg");
        fs::create_dir_all(&pkg).unwrap();
        write_inline_split_loader(&pkg, "demo", &[LOAD_IMPORT.to_string()]).unwrap();

        let js = fs::read_to_string(pkg.join("__wasm_split.js")).unwrap();
        assert!(js.contains("queueMicrotask"), "{js}");
        assert!(
            js.contains("__indirect_function_table.get(callbackIndex)(callbackData, true)"),
            "{js}",
        );
        fs::remove_dir_all(&tmp).ok();
    }

    /// Skipping has to leave pkg/ in the state a never-split build would
    /// have produced. A leftover chunk from an earlier splitting run is
    /// digested by `fingerprint_pkg` and copied by `stage_bundle`, so an
    /// orphan would ship AND would rotate the build hash when it later
    /// disappeared.
    #[test]
    fn clearing_removes_the_loader_and_every_chunk_but_keeps_the_bundle() {
        let tmp = tmpdir("clear");
        let pkg = tmp.join("pkg");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(pkg.join("demo.js"), "export { initSync };").unwrap();
        fs::write(pkg.join("__wasm_split.js"), "makeLoad(...)").unwrap();
        fs::write(pkg.join("chunk_0_split.wasm"), b"\0asm").unwrap();
        fs::write(pkg.join("module_0___idealyst_lazy_body_abc.wasm"), b"\0asm").unwrap();
        fs::write(pkg.join("demo_bg.wasm"), b"\0asm").unwrap();

        clear_wasm_split_artifacts(&pkg).unwrap();

        assert!(!pkg.join("__wasm_split.js").exists());
        assert!(!pkg.join("chunk_0_split.wasm").exists());
        assert!(!pkg.join("module_0___idealyst_lazy_body_abc.wasm").exists());
        assert!(pkg.join("demo_bg.wasm").exists(), "the bundle itself must survive");
        assert!(pkg.join("demo.js").exists(), "the entry shim must survive");
        fs::remove_dir_all(&tmp).ok();
    }

    /// Clearing an already-clean pkg/ is a no-op, not an error — it runs on
    /// every `--no-split` build, including the first one.
    #[test]
    fn clearing_is_idempotent() {
        let tmp = tmpdir("idem");
        let pkg = tmp.join("pkg");
        fs::create_dir_all(&pkg).unwrap();
        clear_wasm_split_artifacts(&pkg).unwrap();
        clear_wasm_split_artifacts(&pkg).unwrap();
        fs::remove_dir_all(&tmp).ok();
    }
}

#[cfg(test)]
mod fingerprint_tests {
    //! Coverage for the content-hash cache-busting pass. Synthetic
    //! pkg dirs shaped like real wasm-bindgen + wasm-split output —
    //! no wasm toolchain needed. The shim/loader fixtures mirror the
    //! exact reference shapes observed in a real build (see the
    //! `fingerprint_pkg` doc comment for why each shape matters).

    use super::*;

    /// A pkg/ shaped like real `wasm-bindgen --target web` +
    /// wasm-split output for a lib named `demo`. The shim carries the
    /// three reference kinds the rewrite has to treat differently:
    /// an ES import specifier, a wasm import-object key (same string,
    /// MUST NOT change), and the `new URL` wasm boot path.
    fn fake_pkg(tmp: &Path) -> PathBuf {
        let pkg = tmp.join("pkg");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(
            pkg.join("demo.js"),
            concat!(
                "import * as import1 from \"./__wasm_split.js\"\n",
                "function __wbg_get_imports() {\n",
                "    const imports = {\n",
                "        \"./__wasm_split.js\": import1,\n",
                "    };\n",
                "    return imports;\n",
                "}\n",
                "module_or_path = new URL('demo_bg.wasm', import.meta.url);\n",
                "export { initSync };\n",
            ),
        )
        .unwrap();
        fs::write(
            pkg.join("__wasm_split.js"),
            concat!(
                "import { initSync } from \"./demo.js\";\n",
                "export const __wasm_split_load_chunk_0 = ",
                "makeLoad(\"./chunk_0_split.wasm\", [], fusedImports, initSync);\n",
                "export const __wasm_split_load_lazy = ",
                "makeLoad(\"./module_0_lazy.wasm\", [__wasm_split_load_chunk_0], fusedImports, initSync);\n",
            ),
        )
        .unwrap();
        fs::write(pkg.join("demo_bg.wasm"), b"\0asm-main").unwrap();
        fs::write(pkg.join("chunk_0_split.wasm"), b"\0asm-chunk").unwrap();
        fs::write(pkg.join("module_0_lazy.wasm"), b"\0asm-module").unwrap();
        // Snippet dirs must feed the digest but keep their paths.
        fs::create_dir_all(pkg.join("snippets/demo-abc123")).unwrap();
        fs::write(pkg.join("snippets/demo-abc123/inline0.js"), b"export {};").unwrap();
        pkg
    }

    fn hashed(name: &str, hash: &str) -> String {
        let (stem, ext) = name.rsplit_once('.').unwrap();
        format!("{stem}.{hash}.{ext}")
    }

    /// The preminted stylesheet must keep its URL across a code-only
    /// deploy.
    ///
    /// It used to share the build-wide digest, so editing one line of Rust
    /// rotated `premint.<hash>.css` even though the CSS bytes were
    /// identical — every browser re-downloaded a stylesheet it already
    /// had. That defeats the reason premint moves rules out of the wasm in
    /// the first place: styles change on a different (much slower)
    /// schedule than app code, and shipping them separately is only worth
    /// anything if they can be cached separately.
    #[test]
    fn stylesheet_url_survives_a_code_only_change() {
        let tmp = tempfile::tempdir().unwrap();
        let pkg = fake_pkg(tmp.path());
        fs::write(pkg.join("premint.css"), b".iy-abc { color: red }").unwrap();
        let first = fingerprint_pkg(&pkg, "demo").unwrap();
        let css_first = first.premint_css.clone().expect("stylesheet fingerprinted");

        // Rebuild with CHANGED wasm but byte-identical CSS.
        let pkg2 = fake_pkg(tmp.path().join("second").as_path());
        fs::write(pkg2.join("demo_bg.wasm"), b"\0asm-main-CHANGED").unwrap();
        fs::write(pkg2.join("premint.css"), b".iy-abc { color: red }").unwrap();
        let second = fingerprint_pkg(&pkg2, "demo").unwrap();
        let css_second = second.premint_css.clone().expect("stylesheet fingerprinted");

        assert_ne!(
            first.hash, second.hash,
            "the code change must rotate the build hash, or this test proves nothing"
        );
        assert_eq!(
            css_first, css_second,
            "byte-identical CSS must keep its URL when only code changed \
             ({css_first} → {css_second})"
        );
        assert_ne!(
            first.entry_js, second.entry_js,
            "the entry shim must still rotate — it references the new wasm"
        );
    }

    /// The flip side: a CSS edit MUST rotate the stylesheet URL, or a
    /// restyle silently serves stale cached rules.
    #[test]
    fn stylesheet_url_rotates_when_the_css_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let pkg = fake_pkg(tmp.path());
        fs::write(pkg.join("premint.css"), b".iy-abc { color: red }").unwrap();
        let first = fingerprint_pkg(&pkg, "demo").unwrap();

        let pkg2 = fake_pkg(tmp.path().join("second").as_path());
        fs::write(pkg2.join("premint.css"), b".iy-abc { color: blue }").unwrap();
        let second = fingerprint_pkg(&pkg2, "demo").unwrap();

        assert_ne!(
            first.premint_css.unwrap(),
            second.premint_css.unwrap(),
            "a restyle must produce a new stylesheet URL"
        );
    }

    #[test]
    fn fingerprint_renames_every_bundle_file_and_rewrites_references() {
        let tmp = tempfile::tempdir().unwrap();
        let pkg = fake_pkg(tmp.path());
        let fp = fingerprint_pkg(&pkg, "demo").unwrap();

        assert_eq!(fp.hash.len(), 16, "16 hex chars, got {:?}", fp.hash);
        assert_eq!(fp.entry_js, hashed("demo.js", &fp.hash));

        for old in [
            "demo.js",
            "demo_bg.wasm",
            "__wasm_split.js",
            "chunk_0_split.wasm",
            "module_0_lazy.wasm",
        ] {
            assert!(!pkg.join(old).exists(), "{old} must be renamed away");
            assert!(
                pkg.join(hashed(old, &fp.hash)).is_file(),
                "{} must exist",
                hashed(old, &fp.hash),
            );
        }
        // Snippets stay path-stable (wasm-bindgen names those dirs).
        assert!(pkg.join("snippets/demo-abc123/inline0.js").is_file());

        let shim = fs::read_to_string(pkg.join(&fp.entry_js)).unwrap();
        let split_new = hashed("__wasm_split.js", &fp.hash);
        assert!(
            shim.contains(&format!("from \"./{split_new}\"")),
            "ES import specifier must follow the loader rename:\n{shim}",
        );
        assert!(
            shim.contains("\"./__wasm_split.js\": import1"),
            "wasm import-object key must stay byte-identical to the string \
             baked into the wasm binary:\n{shim}",
        );
        assert!(
            shim.contains(&format!("new URL('{}'", hashed("demo_bg.wasm", &fp.hash))),
            "wasm boot URL must point at the hashed main wasm:\n{shim}",
        );

        let loader = fs::read_to_string(pkg.join(&split_new)).unwrap();
        assert!(
            loader.contains(&format!("from \"./{}\"", fp.entry_js)),
            "loader's initSync import must follow the shim rename:\n{loader}",
        );
        assert!(
            loader.contains(&format!(
                "makeLoad(\"./{}\"",
                hashed("chunk_0_split.wasm", &fp.hash),
            )),
            "chunk fetch URL must be hashed:\n{loader}",
        );
        assert!(
            loader.contains(&format!(
                "makeLoad(\"./{}\"",
                hashed("module_0_lazy.wasm", &fp.hash),
            )),
            "module fetch URL must be hashed:\n{loader}",
        );
    }

    #[test]
    fn fingerprint_is_deterministic_and_change_sensitive() {
        let tmp_a = tempfile::tempdir().unwrap();
        let tmp_b = tempfile::tempdir().unwrap();
        let a = fingerprint_pkg(&fake_pkg(tmp_a.path()), "demo").unwrap();
        let b = fingerprint_pkg(&fake_pkg(tmp_b.path()), "demo").unwrap();
        assert_eq!(
            a.hash, b.hash,
            "byte-identical pkgs must fingerprint identically (redeploy of an \
             unchanged app keeps caches valid)",
        );

        let tmp_c = tempfile::tempdir().unwrap();
        let pkg_c = fake_pkg(tmp_c.path());
        fs::write(pkg_c.join("chunk_0_split.wasm"), b"\0asm-chunk-CHANGED").unwrap();
        let c = fingerprint_pkg(&pkg_c, "demo").unwrap();
        assert_ne!(
            a.hash, c.hash,
            "any byte change anywhere in pkg/ must rotate every filename",
        );

        // A snippet-only change must rotate names too — snippet paths
        // stay stable, so the cache-bust has to come from the digest.
        let tmp_d = tempfile::tempdir().unwrap();
        let pkg_d = fake_pkg(tmp_d.path());
        fs::write(
            pkg_d.join("snippets/demo-abc123/inline0.js"),
            b"export {}; // changed",
        )
        .unwrap();
        let d = fingerprint_pkg(&pkg_d, "demo").unwrap();
        assert_ne!(a.hash, d.hash, "snippet bytes must feed the digest");
    }

    #[test]
    fn index_rewrite_points_every_spelling_at_hashed_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let index = tmp.path().join("index.html");
        fs::write(
            &index,
            "<script type=\"module\">import init from \"/pkg/demo.js\"; init();</script>\n\
             <link rel=\"modulepreload\" href=\"./pkg/demo.js\" />",
        )
        .unwrap();
        rewrite_index_bundle_ref(&index, "demo", "demo.0123456789abcdef.js").unwrap();
        let html = fs::read_to_string(&index).unwrap();
        assert!(html.contains("/pkg/demo.0123456789abcdef.js"), "{html}");
        assert!(html.contains("./pkg/demo.0123456789abcdef.js"), "{html}");
        assert!(!html.contains("pkg/demo.js"), "{html}");
    }

    #[test]
    fn index_without_conventional_ref_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let index = tmp.path().join("index.html");
        let original = "<script src=\"/custom/boot.js\"></script>";
        fs::write(&index, original).unwrap();
        rewrite_index_bundle_ref(&index, "demo", "demo.0123456789abcdef.js").unwrap();
        assert_eq!(fs::read_to_string(&index).unwrap(), original);
    }

    #[test]
    fn find_hashed_entry_matches_only_the_fingerprinted_shim() {
        // Decoys only — nothing matching `<lib>.<16 lower hex>.js`.
        // The uppercase twin lives in its own dir because on a
        // case-insensitive filesystem (macOS default) it would share
        // a directory entry with the real lowercase name.
        let decoys = tempfile::tempdir().unwrap();
        let pkg = decoys.path();
        fs::write(pkg.join("demo.js"), b"").unwrap();
        fs::write(pkg.join("demo.notahash.js"), b"").unwrap();
        fs::write(pkg.join("demo.0123456789ABCDEF.js"), b"").unwrap(); // uppercase = not ours
        fs::write(pkg.join("demo_bg.0123456789abcdef.wasm"), b"").unwrap();
        assert_eq!(find_hashed_entry(pkg, "demo"), None);

        let real = tempfile::tempdir().unwrap();
        let pkg = real.path();
        fs::write(pkg.join("demo.notahash.js"), b"").unwrap();
        fs::write(pkg.join("demo_bg.0123456789abcdef.wasm"), b"").unwrap();
        fs::write(pkg.join("demo.0123456789abcdef.js"), b"").unwrap();
        assert_eq!(
            find_hashed_entry(pkg, "demo").as_deref(),
            Some("demo.0123456789abcdef.js"),
        );
    }
}

#[cfg(test)]
mod robot_relay_tests {
    use super::*;

    /// A full-stack project's own server hands out the STAGED
    /// `index.html` as a plain file, so the relay URL has to be written
    /// into that file. It used never to be written anywhere, and the
    /// browser app — with nothing on `window` to read — never dialed
    /// the relay.
    ///
    /// This is the closest reachable test: the end-to-end chain
    /// (bundle → browser → relay → MCP verb) needs a cargo build, a
    /// server and a real headless browser, which is what
    /// `robot-relay/tests/browser_handshake.rs` covers under `#[ignore]`.
    /// What is unit-testable — and what actually broke — is that the
    /// served HTML carries the global at all.
    #[test]
    fn regression_staged_index_advertises_the_relay() {
        let tmp = tempfile::tempdir().unwrap();
        let index = tmp.path().join("index.html");
        fs::write(&index, "<html><head><title>App</title></head><body></body></html>").unwrap();

        inject_robot_relay_url_into_staged_index(&index, "ws://127.0.0.1:44885").unwrap();

        let html = fs::read_to_string(&index).unwrap();
        assert!(
            html.contains(r#"window.IDEALYST_ROBOT_RELAY_URL="ws://127.0.0.1:44885";"#),
            "staged index must advertise the relay, got:\n{html}",
        );
        // Before `</head>`, like every other staged-index injection —
        // the app reads the global at boot, so it must be set before
        // the bundle's own script runs.
        let script = html.find("IDEALYST_ROBOT_RELAY_URL").unwrap();
        assert!(script < html.find("</head>").unwrap());
    }

    /// The URL arrives from an environment variable, so it is not
    /// trusted input. A `"` would close the string literal and a
    /// `</script>` would close the block, either one turning the rest
    /// of the head into markup the browser executes.
    #[test]
    fn relay_url_cannot_break_out_of_the_script_tag() {
        let tag = robot_relay_script_tag(r#"ws://x"</script><script>alert(1)"#);

        // Exactly one script element, and it is ours — the payload's
        // `</script><script>` must not have survived as markup.
        assert_eq!(tag.matches("<script>").count(), 1, "escaped: {tag}");
        assert_eq!(tag.matches("</script>").count(), 1, "escaped: {tag}");

        // Assert on the ESCAPES themselves, not on the presence of
        // characters our own `<script>` wrapper contributes anyway:
        // `tag.contains("<")` is true of every tag ever produced and so
        // tests nothing.
        assert!(tag.contains(r#"\u003C/script"#), "`<` must become \\u003C: {tag}");
        assert!(tag.contains(r#"ws://x\""#), "the quote must be backslash-escaped: {tag}");

        // And nothing from the URL reaches the document as raw markup.
        let body = tag.strip_prefix("\n    <script>").unwrap().strip_suffix("</script>").unwrap();
        assert!(!body.contains('<'), "no raw `<` may survive into the script body: {body}");
    }

    /// A deploy bundle must never carry a dev machine's relay port —
    /// the injection is opt-in via `robot_relay_url`, and every
    /// `idealyst build` path leaves it `None`.
    #[test]
    fn plain_staged_index_carries_no_relay_global() {
        let tmp = tempfile::tempdir().unwrap();
        let index = tmp.path().join("index.html");
        let original = "<html><head><title>App</title></head><body></body></html>";
        fs::write(&index, original).unwrap();

        // Drive the REAL gate `build()` calls. Asserting on a file we
        // just wrote and never handed to production code would pass just
        // as happily against an unconditional injection.
        stage_robot_relay_url(&index, None).unwrap();

        let html = fs::read_to_string(&index).unwrap();
        assert_eq!(html, original, "a None relay must leave the staged index byte-identical");
        assert!(!html.contains("IDEALYST_ROBOT_RELAY_URL"));

        // And the same gate with Some(..) does inject — otherwise this
        // test would also pass against a function that never writes.
        stage_robot_relay_url(&index, Some("ws://127.0.0.1:44885")).unwrap();
        assert!(fs::read_to_string(&index).unwrap().contains("IDEALYST_ROBOT_RELAY_URL"));
    }
}
