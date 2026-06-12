//! Web build orchestration for `idealyst build web` and the dev
//! server.
//!
//! Mirror of `crates/build/ios/` and `crates/build/android/`: the
//! user's app crate is intentionally platform-agnostic — it exposes
//! `pub fn app() -> Primitive` and nothing else. The web target has
//! historically required the user to also write a `web.rs` with a
//! `#[wasm_bindgen(start)]` function plus a `[lib] crate-type =
//! ["cdylib", "rlib"]` and a handful of wasm-only deps
//! (`wasm-bindgen`, `console_error_panic_hook`, `lol_alloc`). That's
//! the same per-platform plumbing the iOS / Android wrapper crates
//! exist to absorb, and now web absorbs it the same way.
//!
//! `build()` generates an ephemeral `cdylib` wrapper at:
//!
//! ```text
//! <workspace>/target/idealyst/<project>/web/wrapper/
//! ```
//!
//! whose `src/lib.rs` is the wasm-bindgen entry point boilerplate
//! identical for every project — only the `<project>::app()` call
//! site changes. wasm-pack runs against the wrapper, producing the
//! `pkg/` bundle. We then copy that `pkg/` over to the user
//! project's root so the user's `index.html` (which references
//! `./pkg/<lib>.js`) keeps working without changes.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use build_ios::{font_preload_tags, inject_into_head, parse_manifest, FrameworkSource, Manifest};
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
    /// Pre-gzip every text-ish file in the staged bundle, writing
    /// gzipped bytes under the original filename. Only meaningful
    /// when `bundle_out_dir` is `Some`; ignored otherwise. The static
    /// host must send `Content-Encoding: gzip` on these responses for
    /// the browser to inflate them transparently.
    pub gzip: bool,
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
}

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
}

/// Build the user's project at `project_dir` for the web target.
/// Generates the wrapper, runs `wasm-pack build --target web`, and
/// copies the resulting `pkg/` over to `project_dir/pkg/`.
pub fn build(project_dir: &Path, opts: BuildOptions) -> Result<BuildArtifact> {
    let project_dir = fs::canonicalize(project_dir)
        .with_context(|| format!("resolve project dir {}", project_dir.display()))?;
    let manifest = parse_manifest(&project_dir)?;

    let wrapper_dir = opts
        .source
        .wrapper_root(&project_dir)
        .join(&manifest.name)
        .join("web/wrapper");
    generate_wrapper(
        &wrapper_dir,
        &project_dir,
        &opts.source,
        &manifest,
        &opts.user_features,
        opts.hydrate,
    )?;

    // Direct pipeline (no wasm-pack), so we can hit the flag matrix
    // wasm-split-cli needs to actually extract chunks:
    //
    //   1. `cargo build` with `RUSTFLAGS="-C link-args=--emit-relocs"`
    //      so the rustc-emitted wasm has the relocation info wasm-split
    //      needs to rewrite indirect calls.
    //   2. `wasm-bindgen --keep-lld-exports` so wasm-bindgen preserves
    //      the LLD-emitted exports wasm-split's reachability walker
    //      uses to identify chunk-only code.
    //   3. `wasm-split-cli split` rewrites the bindgened wasm into a
    //      lean base + per-chunk wasms + a `__wasm_split.js` loader.
    //   4. `wasm-opt -Oz` runs LAST, per-file, on the base + every
    //      chunk. wasm-pack ran it BEFORE wasm-bindgen which mangled
    //      symbols wasm-split needed — that's why my earlier
    //      website measurements showed 0 KB chunks even when the
    //      lazy! body was clearly extractable.
    let wrapper_pkg = wrapper_dir.join("pkg");
    let original_wasm = wrapper_dir
        .join("target/wasm32-unknown-unknown")
        .join(if opts.release { "release" } else { "debug" })
        .join(format!("{}.wasm", manifest.lib_name));
    cargo_build_wasm(
        &wrapper_dir,
        opts.release,
        opts.strip_panics,
        &opts.user_features,
    )?;
    wasm_bindgen_build(&original_wasm, &wrapper_pkg, &manifest.lib_name)
        .with_context(|| "wasm-bindgen")?;
    neutralize_command_export_wrappers(&wrapper_pkg, &manifest.lib_name)
        .with_context(|| "wasm-bindgen command_export neutralize")?;
    run_wasm_split(
        &original_wasm,
        &wrapper_pkg,
        &manifest.lib_name,
        opts.prune_dead_data_min,
    )
    .with_context(|| "wasm-split-cli post-build")?;
    if opts.release {
        wasm_opt_pkg(&wrapper_pkg).with_context(|| "wasm-opt post-split")?;
    }

    let (pkg_dir, bundle_dir) = if let Some(out) = opts.bundle_out_dir.as_ref() {
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
        // Rewrite the staged `index.html` to preload the project's
        // declared fonts. Has to run BEFORE `gzip_bundle` (which
        // overwrites `index.html` with gzipped bytes) so the gzipped
        // copy carries the preload tags. No-op when the user hasn't
        // declared `[package.metadata.idealyst.app.web].preload_fonts`.
        inject_font_preloads_into_staged_index(
            &staged.join("index.html"),
            &manifest.app.web.preload_fonts,
        )?;
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
        if opts.gzip {
            gzip_bundle(&staged).with_context(|| format!("gzip bundle at {}", staged.display()))?;
        }
        (staged_pkg, Some(staged))
    } else {
        let project_pkg = project_dir.join("pkg");
        sync_pkg_dir(&wrapper_pkg, &project_pkg).with_context(|| {
            format!("sync {} → {}", wrapper_pkg.display(), project_pkg.display())
        })?;
        (project_pkg, None)
    };

    Ok(BuildArtifact {
        pkg_dir,
        wrapper_dir,
        bundle_dir,
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

/// Materialize the wrapper crate at `wrapper_dir`. Idempotent —
/// overwrites whatever was there. Public so a future
/// `idealyst scaffold web` command can drive the same generator.
///
/// `user_features` names cargo features that should be forwarded to
/// the user-crate dep — the wrapper grows a `[features]` block that
/// re-exports each one (`<feat> = ["<user>/<feat>"]`) so a
/// `wasm-pack build -- --features <feat>` invocation against the
/// wrapper turns on the matching feature on the user crate. This is
/// the path runtime-server-mode hot reload uses to enable `dev-hot-reload` on
/// the user crate without forcing every user crate to carry that
/// feature in its default set.
pub fn generate_wrapper(
    wrapper_dir: &Path,
    project_dir: &Path,
    source: &FrameworkSource,
    manifest: &Manifest,
    user_features: &[String],
    hydrate: bool,
) -> Result<()> {
    fs::create_dir_all(wrapper_dir.join("src"))
        .with_context(|| format!("create {}", wrapper_dir.display()))?;

    let wrapper_name = format!("{}-web-wrapper", manifest.name);
    // `wasm-pack` uses `package.name` (`-` not preserved) to derive
    // the emitted JS filename: e.g. `<name>.js`, `<name>_bg.wasm`.
    // The user's `index.html` references `./pkg/<lib>.js`, where
    // `<lib>` is `manifest.lib_name` (= package name with `-` → `_`).
    // Our wrapper's package name is `<lib>-web-wrapper`; wasm-pack
    // would produce `<lib>_web_wrapper.js`. We force the wasm-pack
    // output to use the original lib name by setting `[lib].name`
    // on the wrapper to `manifest.lib_name`, which wasm-pack
    // prefers over the package name when present.
    let fcore_dep = source.dep("crates/runtime/core", &[]);
    // The wrapper always installs `backend_web::install_async_executor()`
    // so `runtime_core::driver::spawn_async` works inside any
    // wasm app — required by `resource()`, `mutation()`, and the
    // server-fn batch flusher.
    //
    // `hydrate` is the in-place hydration machinery (cursor + remount
    // + per-primitive `hydrate_next` paths + the divergence-diagnostic).
    // Enabled when the bundle is paired with SSG/SSR HTML it needs to
    // adopt; suppressed for pure SPA builds so the machinery DCE's out
    // of the wasm. We construct this dep line by hand (not via
    // `source.dep`) so we can spell `default-features = false` and pick
    // an exact feature set — `backend-web`'s default set includes
    // `hydrate`, which is the desired standalone-check default but the
    // wrong default for a CLI-built wrapper that knows what it needs.
    let bweb_base = source.dep("crates/backend/web", &[]);
    // `source.dep` returns `{ path = "..." }` (or git equivalent). Splice
    // in the explicit feature set so the wrapper opts out of `hydrate`
    // when SSG/SSR isn't paired with this build.
    let bweb_inner = bweb_base.trim().trim_start_matches('{').trim_end_matches('}').trim();
    let bweb_features: Vec<&str> = if hydrate {
        vec!["async-driver", "hydrate"]
    } else {
        vec!["async-driver"]
    };
    let bweb_features_clause = format!(
        "features = [{}]",
        bweb_features.iter().map(|f| format!("\"{}\"", f)).collect::<Vec<_>>().join(", "),
    );
    let bweb_dep = format!(
        "{{ {}, default-features = false, {} }}",
        bweb_inner, bweb_features_clause,
    );
    // `dev-client` is only needed in runtime-server mode. Declared as an
    // optional dep so plain wasm builds don't drag the `WireBackend`
    // replay engine into their bundle. We strip the outer braces from
    // `source.dep` so we can splice in `optional = true` alongside the
    // git/path fields.
    let dev_client_raw = source.dep("crates/dev/client", &[]);
    let dev_client_inner = dev_client_raw
        .trim()
        .trim_start_matches('{')
        .trim_end_matches('}')
        .trim();

    let cargo_toml = format!(
        r#"# GENERATED by `idealyst build web`. Do not edit — rewritten
# every build. Run `idealyst scaffold web` to materialize an editable
# copy of this wrapper into your repo (once that command lands).

# Empty `[workspace]` declares this wrapper as a standalone project
# even though it physically lives under the main workspace's
# `target/idealyst/...`. Without it, cargo refuses to build because
# the parent Cargo.toml has `[workspace]` and would normally claim
# this directory as a member.
[workspace]

[package]
name = "{wrapper_name}"
version = "0.0.1"
edition = "2021"

# Forcing `[lib].name = "{lib_name}"` so wasm-pack emits
# `pkg/{lib_name}.js` / `pkg/{lib_name}_bg.wasm` regardless of the
# wrapper's package name — matches what the user's `index.html`
# expects (`import init from "./pkg/{lib_name}.js"`).
[lib]
name = "{lib_name}"
crate-type = ["cdylib"]

[dependencies]
runtime-core = {fcore_dep}
backend-web = {bweb_dep}
# runtime-server-mode dep. Optional + gated by the `aas` feature so plain wasm
# builds don't pull the `WireBackend` replay engine into their bundle.
dev-client = {{ {dev_client_inner}, optional = true }}
{user_name} = {{ path = "{user_path}" }}

wasm-bindgen = "0.2"
console_error_panic_hook = "0.1"
# runtime-server-mode `start()` calls `js_sys::Reflect::get` to read
# `window.IDEALYST_RUNTIME_SERVER_URL` and `web_sys::console` for log lines.
# Both are already in the dep graph via backend-web; declaring them
# here lets the wrapper template reference them directly without
# leaking a transitive-import requirement on backend-web.
js-sys = "0.3"
web-sys = {{ version = "0.3", features = ["Window", "Navigator"] }}
# Smaller WASM allocator — slightly higher per-alloc cost in exchange
# for a few KB shaved off the bundle.
lol_alloc = "0.4"

[features]
# runtime-server-mode hot reload. Activated by `idealyst dev --aas --web`. When
# on, the generated `start()` reads `window.IDEALYST_RUNTIME_SERVER_URL` (the dev
# HTTP server injects it on every served page) and connects the
# `WebBackend` to the runtime-server host over WebSocket via
# `backend_web::connect_web`. When off (plain `idealyst build --web`
# or `idealyst dev --web` without `--aas`), `start()` mounts the
# user's `app()` locally in the browser as before.
# Two flips together: pull in the optional `dev-client` (WireBackend
# replay engine) AND turn on `backend-web/runtime-server`, which is what
# gates the `connect_web` + `WebClientHandle` exports we use below.
runtime-server = ["dep:dev-client", "backend-web/runtime-server"]
# Deprecated alias for the old "AAS" name — kept so any tooling that
# still enables `aas` resolves. The canonical name is `runtime-server`
# (matches the generated code's `#[cfg(feature = "runtime-server")]`
# gates and `idealyst dev`'s `--features runtime-server`).
aas = ["runtime-server"]
{user_feature_forwards}
{patch_block}
# Wrapper-level release profile. wasm-pack is no longer in the
# pipeline (build-web invokes cargo + wasm-bindgen + wasm-split +
# wasm-opt directly), so the wrapper's [profile.release] is what
# governs release builds.
#
# Key choices, all in service of letting wasm-split-cli actually do
# its job:
#   * `lto = "off"` — `"fat"` LTO inlines `#[wasm_split]` annotated
#     functions back into their callers, which puts their body code
#     in main and shrinks the chunk to a stub. "off" preserves the
#     function boundary; wasm-opt's per-chunk pass recovers most of
#     the LTO size win anyway.
#   * `codegen-units = 1` — fewer cross-unit indirections in the
#     emitted wasm gives wasm-split's reachability walker more
#     precision (it's pessimistic across CU boundaries).
#   * `strip = "none"` — symbol names alive for wasm-split's
#     call-graph matching; wasm-opt strips them as a final step.
#   * `debug = "limited"` — line tables stay so wasm-split can match
#     calls to their reloc records; stripped by wasm-opt.
[profile.release]
opt-level = "z"
codegen-units = 1
lto = "off"
panic = "abort"
strip = "none"
debug = "limited"

# Dev builds default to `opt-level = 0`, which is catastrophic for compute-heavy
# DEPENDENCY crates (e.g. a CPU rasterizer like `vello_cpu`/`hayro`): un-inlined
# tiny-function inner loops + bounds checks run 10-40x slower, turning a sub-second
# render into 10s+. Optimize all *dependencies* (the `"*"` glob — excludes this
# wrapper + the app crate, so app iteration stays fast to compile) without
# touching the size-tuned release profile above. SAFE for `#[wasm_split]` lazy
# loading: split points are `#[no_mangle] extern "C"` FFI export/import edges the
# optimizer can't inline across, and `lto` stays unset (not "fat"), so no bodies
# relocate into `main`.
[profile.dev.package."*"]
opt-level = 3
"#,
        wrapper_name = wrapper_name,
        lib_name = manifest.lib_name,
        user_name = manifest.name,
        user_path = project_dir.display(),
        fcore_dep = fcore_dep,
        bweb_dep = bweb_dep,
        dev_client_inner = dev_client_inner,
        user_feature_forwards = user_feature_forwards(&manifest.name, user_features),
        patch_block = source.patch_block(),
    );

    let lib_rs = format!(
        r##"//! GENERATED by `idealyst build web`. Two start paths, picked by
//! the `aas` cargo feature:
//!
//! - **Default (no feature):** mounts `{lib}::app()` locally on the
//!   DOM element `#app`. The browser runs the framework runtime
//!   directly. This is what `idealyst build --web` produces and what
//!   `idealyst dev --web` (without `--aas`) serves.
//!
//! - **`aas` feature on:** reads `window.IDEALYST_RUNTIME_SERVER_URL` (the dev
//!   HTTP server injects it into every served page) and connects a
//!   `WireBackend<WebBackend>` to the runtime-server host over WebSocket via
//!   `backend_web::connect_web`. The browser becomes a thin replayer;
//!   the framework runtime lives in the runtime-server sidecar. This is what
//!   `idealyst dev --aas --web` produces. Without the feature the
//!   browser would render locally and never connect to runtime-server — the
//!   sidecar would sit idle reporting `0 session(s)` on every
//!   hot-patch.

#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;

use backend_web::WebBackend;
use wasm_bindgen::prelude::*;

// Smaller WASM allocator — trades a few cycles per allocation for a
// few KB shaved off the bundle.
#[global_allocator]
static ALLOCATOR: lol_alloc::AssumeSingleThreaded<lol_alloc::FreeListAllocator> =
    unsafe {{ lol_alloc::AssumeSingleThreaded::new(lol_alloc::FreeListAllocator::new()) }};

thread_local! {{
    /// Local-mode: `mount` returns an `Owner` that must outlive the
    /// page. Stash it in a thread-local so it survives `start()`.
    static OWNER: RefCell<Option<runtime_core::Owner>> =
        const {{ RefCell::new(None) }};
    /// runtime-server-mode: the `WebClientHandle` owns the WebSocket + event
    /// closures + raf pump. Drop tears down the connection, so keep it
    /// alive for the page's lifetime.
    #[cfg(feature = "runtime-server")]
    static AAS_HANDLE: RefCell<Option<backend_web::WebClientHandle>> =
        const {{ RefCell::new(None) }};
    /// runtime-server-mode: the `WireBackend` lives behind an `Rc<RefCell<…>>`
    /// because both the `connect_web` raf pump and the on-disconnect
    /// reconnect closure want to retarget it.
    #[cfg(feature = "runtime-server")]
    static AAS_WIRE: RefCell<Option<Rc<RefCell<dev_client::WireBackend<WebBackend>>>>> =
        const {{ RefCell::new(None) }};
}}

// Named `main` (not `start`) because `wasm-split-cli` looks for a
// function called `main` as the entry point of the call graph it
// walks to decide what's reachable from the base bundle vs. only
// from a lazy chunk. The `#[wasm_bindgen(start)]` attribute is what
// actually marks it as the JS-init entry — the function name is
// arbitrary as far as wasm-bindgen is concerned.
#[wasm_bindgen(start)]
pub fn main() {{
    // This same wasm is also imported + initialized inside Web Workers (by the
    // `offload` SDK / `wasmworker`) purely to make `#[offload::job]`-exported
    // functions callable there. A Worker has no `Window` and no DOM, so the start
    // function must NOT install the UI backend or mount — it just returns, leaving
    // the module instantiated and the job exports reachable. Detect the Worker
    // context (no `window`) and bail before any main-thread-only setup.
    if web_sys::window().is_none() {{
        return;
    }}

    console_error_panic_hook::set_once();

    // Scheduler + time source + async executor + render loop -- every
    // code path needs them. The async executor is what makes
    // `runtime_core::driver::spawn_async` work on wasm; without it any
    // async work (resource fetchers, server-fn calls, mutation
    // triggers) panics at first poll with "no AsyncExecutor
    // installed". The render-loop driver is what makes
    // `runtime_core::driver::render_loop` tick frames; without it,
    // host-web's per-frame paint closure (the wgpu Simulator preview,
    // every future host-driven animation surface) gets a `NoopHandle`
    // and never paints -- the canvas mounts but stays blank.
    backend_web::install_scheduler();
    backend_web::install_time_source();
    // Route runtime-core `log_*` through the browser `console.*`. Without
    // this, `log_info!`/`log_error!` hit the wasm stderr no-op sink and
    // vanish — Rust-side logs (incl. an in-app E2E suite's `[E2E-RESULT]`
    // summary) never reach devtools. JS-side shim logs are unaffected; this
    // is specifically the Rust logging channel.
    backend_web::install_logger();
    backend_web::install_async_executor();
    backend_web::install_render_loop();
    // NOTE: the viewport observer is installed INSIDE the start fns, not
    // here — its timing differs for hydration (after mount, so the first
    // render uses the SSR-assumed viewport) vs a fresh boot (before mount,
    // so the first render sees the real viewport).

    #[cfg(feature = "runtime-server")]
    {{
        start_aas();
    }}
    #[cfg(not(feature = "runtime-server"))]
    {{
        start_local();
    }}
}}

/// Local mode: framework runtime lives in this browser. Same flow as
/// `idealyst build --web` (no `--aas`).
///
/// HYDRATION: if `#app` was server-rendered (has children), ADOPT that
/// DOM instead of mounting fresh — `WebBackend::hydrate` reuses the
/// server's nodes and just wires handlers/reactivity. To keep the first
/// (hydration) render matching the server's tree, we seed the
/// SSR-assumed viewport (`#app[data-ssr-viewport]`) before the build,
/// then install the viewport observer AFTER mount so the real viewport
/// drives a reactive reconcile. A fresh boot (empty `#app`) reads the
/// real viewport up front, as before.
#[cfg(not(feature = "runtime-server"))]
fn start_local() {{
    let selector = "#app";
    let prerendered = backend_web::page_is_prerendered(selector);

    let mut web = if prerendered {{
        WebBackend::hydrate(selector)
    }} else {{
        WebBackend::new(selector)
    }};
    // Hand the bare backend to the user crate so it can install
    // navigator-SDK / external-primitive handlers before mount. The
    // user crate must expose `pub fn register_extensions(&mut WebBackend)`;
    // an empty body is fine when the crate has no SDK deps.
    {lib}::register_extensions(&mut web);
    let backend = Rc::new(RefCell::new(web));
    backend_web::install_global_self(&backend);

    // Fresh boot: read the real viewport BEFORE the first render.
    if !prerendered {{
        backend_web::install_viewport_observer();
    }}

    // Seed the SSR-assumed viewport INSIDE the mount scope so the
    // hydration render matches the server's tree (clean DOM adoption).
    let seed = if prerendered {{ backend_web::ssr_viewport(selector) }} else {{ None }};
    let owner = runtime_core::mount(backend, move || {{
        if let Some((w, h)) = seed {{
            runtime_core::set_viewport_size(runtime_core::ViewportSize::new(w, h));
        }}
        {lib}::app()
    }});
    OWNER.with(|slot| *slot.borrow_mut() = Some(owner));

    // Hydration done: switch to the REAL viewport; reactivity reconciles
    // any viewport-dependent content AFTER adoption.
    if prerendered {{
        backend_web::install_viewport_observer();
    }}
}}

/// runtime-server mode: framework runtime lives in the runtime-server sidecar on the dev
/// host. The browser is a thin client that replays wire commands and
/// forwards events back.
#[cfg(feature = "runtime-server")]
fn start_aas() {{
    // runtime-server mode doesn't hydrate (the host renders); read the
    // real viewport up front, as the old `main()` did.
    backend_web::install_viewport_observer();
    let url = match read_aas_url() {{
        Some(u) => u,
        None => {{
            web_sys::console::error_1(
                &"[dev-client] runtime-server mode enabled but window.IDEALYST_RUNTIME_SERVER_URL is missing — \
                  did the dev HTTP server fail to inject it? Falling back to local mount.".into(),
            );
            // Defensive fallback so the page doesn't go blank.
            let mut web = WebBackend::new("#app");
            {lib}::register_extensions(&mut web);
            let backend = Rc::new(RefCell::new(web));
            backend_web::install_global_self(&backend);
            let owner = runtime_core::mount(backend, {lib}::app);
            OWNER.with(|slot| *slot.borrow_mut() = Some(owner));
            return;
        }}
    }};

    web_sys::console::log_1(
        &format!("[dev-client] runtime-server mode: connecting to {{}}", url).into(),
    );

    let mut backend = WebBackend::new("#app");
    // Register SDK extensions on the REAL backend BEFORE wrapping it in
    // the WireBackend. The wire client replays navigator commands by
    // driving this backend's `create_navigator`, which dispatches to the
    // SDK handler registered here — that handler builds the real native
    // chrome (e.g. the web drawer's `ui-nav-drawer-*` CSS structure).
    // `register` also installs each SDK's wire presentation-factory so
    // `dev-client` can rebuild the presentation from wire config. Without
    // this the client falls back to a structural sidebar (no chrome).
    {lib}::register_extensions(&mut backend);
    let outbound = dev_client::OutboundSender::new();
    let wire = Rc::new(RefCell::new(dev_client::WireBackend::new(backend, outbound)));
    AAS_WIRE.with(|slot| *slot.borrow_mut() = Some(wire.clone()));

    let wire_for_reconnect = wire.clone();
    let url_for_reconnect = url.clone();
    let on_disconnect: Rc<dyn Fn()> = Rc::new(move || {{
        // The dev server is likely restarting the sidecar (hot-patch
        // fallback). Try to reconnect; if it fails we'll drop the
        // handle and the page will be inert until next reload.
        let wire = wire_for_reconnect.clone();
        let url = url_for_reconnect.clone();
        let nested_url = url.clone();
        let nested_wire = wire.clone();
        let on_disconnect_again: Rc<dyn Fn()> = Rc::new(move || {{
            web_sys::console::warn_1(&format!(
                "[dev-client] reconnect to {{}} failed; will retry on next disconnect",
                nested_url
            ).into());
            let _ = nested_wire;
        }});
        match backend_web::connect_web(&url, wire, on_disconnect_again) {{
            Ok(h) => {{
                AAS_HANDLE.with(|slot| *slot.borrow_mut() = Some(h));
            }}
            Err(e) => web_sys::console::error_2(
                &"[dev-client] reconnect failed:".into(),
                &e,
            ),
        }}
    }});

    match backend_web::connect_web(&url, wire, on_disconnect) {{
        Ok(h) => {{
            AAS_HANDLE.with(|slot| *slot.borrow_mut() = Some(h));
        }}
        Err(e) => web_sys::console::error_2(
            &"[dev-client] initial runtime-server connect failed:".into(),
            &e,
        ),
    }}
}}

/// Read `window.IDEALYST_RUNTIME_SERVER_URL`, the URL the dev HTTP layer injects
/// into the page via `<script>window.IDEALYST_RUNTIME_SERVER_URL = "..."</script>`.
#[cfg(feature = "runtime-server")]
fn read_aas_url() -> Option<String> {{
    let win = web_sys::window()?;
    js_sys::Reflect::get(&win, &"IDEALYST_RUNTIME_SERVER_URL".into())
        .ok()?
        .as_string()
}}
"##,
        lib = manifest.lib_name,
    );

    fs::write(wrapper_dir.join("Cargo.toml"), cargo_toml)?;
    fs::write(wrapper_dir.join("src/lib.rs"), lib_rs)?;
    Ok(())
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

/// Run `cargo build --target wasm32-unknown-unknown` against the
/// wrapper crate. `RUSTFLAGS="-C link-args=--emit-relocs"` is set
/// (composing with any user-supplied RUSTFLAGS) so the rustc-emitted
/// wasm carries the relocation info wasm-split-cli needs to identify
/// indirect-call targets per chunk. Cost is a few KB of metadata
/// pre-bindgen; stripped from the final bundle by wasm-opt.
fn cargo_build_wasm(
    wrapper_dir: &Path,
    release: bool,
    strip_panics: bool,
    user_features: &[String],
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
    cmd.current_dir(wrapper_dir)
        .arg("build")
        .args(["--target", "wasm32-unknown-unknown"]);
    if release {
        cmd.arg("--release");
    }
    if strip_panics {
        // Rebuild std so the panic-strip feature actually applies to it
        // (not just the user crates). `panic_abort` must be listed
        // explicitly alongside `std` for `panic = "abort"` builds.
        cmd.args([
            "-Z",
            "build-std=std,panic_abort",
            "-Z",
            "build-std-features=panic_immediate_abort",
        ]);
    }
    if !user_features.is_empty() {
        cmd.arg("--features").arg(user_features.join(","));
    }
    let existing_rustflags = std::env::var("RUSTFLAGS").unwrap_or_default();
    // `+simd128`: enable wasm SIMD so SIMD-centric deps (e.g. `vello_cpu`/
    // `fearless_simd`, behind `hayro`'s PDF rasterization) take their vectorized
    // path instead of the scalar fallback — a large speedup for CPU rasterization.
    // No `SharedArrayBuffer` / cross-origin isolation involved (that's wasm
    // *threads*); this is pure codegen and orthogonal to wasm-split. Supported by
    // all evergreen browsers (Chrome/Firefox 2021+, Safari 16.4+).
    let base_flags = "-C target-feature=+simd128 -C link-args=--emit-relocs";
    let combined = if existing_rustflags.is_empty() {
        base_flags.to_string()
    } else {
        format!("{existing_rustflags} {base_flags}")
    };
    cmd.env("RUSTFLAGS", combined);

    eprintln!(
        "[build-web] cargo build --target wasm32-unknown-unknown{}{} (in {})",
        if release { " --release" } else { "" },
        if strip_panics {
            " -Z build-std (panic_immediate_abort)"
        } else {
            ""
        },
        wrapper_dir.display(),
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
        // this flag the website's `lazy! { Simulator(…) }` chunk
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

/// Render the wrapper's `[features]` block. Each *wrapper-local*
/// feature entry becomes `<feat> = ["<user>/<feat>"]` so a
/// `wasm-pack build -- --features <feat>` against the wrapper turns
/// that feature on in the user crate.
///
/// Cross-crate feature activations of the form `<dep>/<feat>` (e.g.
/// `runtime-core/dev`) are **skipped** here — those are valid
/// cargo command-line arguments to `--features`, but they aren't
/// valid feature *names*, and trying to emit them as keys produces
/// invalid TOML. The build command passes them through to cargo as
/// `--features <dep>/<feat>` directly and cargo activates the
/// underlying feature on the named dep.
///
/// Returns the empty string when no wrapper-local features remain
/// so the resulting Cargo.toml doesn't gain an empty `[features]`
/// block.
/// Render user-feature pass-throughs that sit inside the wrapper's
/// single `[features]` block (the wrapper already declares `aas =
/// ["dep:dev-client"]`; we append the forwards to it). Each
/// wrapper-local feature `<f>` becomes `<f> = ["<user>/<f>"]` so a
/// `wasm-pack build -- --features <f>` invocation against the wrapper
/// flips the matching feature on the user crate.
///
/// Two filters:
/// - **Cross-crate (`<dep>/<feat>`) features are skipped.** Those are
///   already valid cargo `--features` values; no aliasing needed.
/// - **`aas` is skipped** because the wrapper defines it itself
///   (gates `dev-client` + the WireBackend `start()` branch). Without
///   this skip, the forward would emit `aas = ["<user>/aas"]` which
///   collides with the wrapper-local definition and fails cargo
///   resolution.
fn user_feature_forwards(user_name: &str, user_features: &[String]) -> String {
    let local: Vec<&String> = user_features
        .iter()
        .filter(|f| {
            !f.is_empty()
                // `dep/feat` activations are passed straight to cargo,
                // not forwarded through the wrapper's feature table.
                && !f.contains('/')
                // Wrapper-LOCAL features the template already declares.
                // Forwarding these to the user crate (e.g.
                // `runtime-server = ["<user>/runtime-server"]`) would
                // require every app to declare an unused feature and
                // breaks `idealyst dev --web` on a fresh scaffold,
                // which is exactly the bug this guards against.
                && f.as_str() != "aas"
                && f.as_str() != "runtime-server"
        })
        .collect();
    if local.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for feat in local {
        out.push_str(&format!(
            "{feat} = [\"{user_name}/{feat}\"]\n",
            feat = feat,
            user_name = user_name,
        ));
    }
    out
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
    //! Wrapper-shape regression for `build-web`.
    //!
    //! The web wrapper has both a `runtime-core` direct dep (so the
    //! launcher's `--features runtime-core/dev` resolves) AND an
    //! `aas` feature that flips on `backend-web/runtime-server`
    //! (the WebSocket / WireBackend boot path). Dropping either
    //! breaks dev mode silently:
    //!  - no runtime-core dep → `--features runtime-core/dev` errors
    //!    at cargo time, MCP catalog ends up empty.
    //!  - no `aas` feature on the wrapper → `idealyst dev --web
    //!    --runtime-server` builds a wasm bundle that mounts
    //!    `app()` locally in the browser instead of connecting to
    //!    the dev-host, and saves visibly do nothing.

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
                build_number: "1".to_string(),
                splash: SplashConfig {
                    background: "#000000".to_string(),
                    title: "Demo".to_string(),
                    title_color: "#ffffff".to_string(),
                    duration_ms: 0,
                },
                targets: Vec::new(),
                server_bin: None,
                server_manifest: None,
                server_port: 3000,
                web: Default::default(),
                macos: Default::default(),
                permissions: Default::default(),
            },
        }
    }

    fn run_generator() -> (std::path::PathBuf, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project_dir = tmp.path().join("project");
        let wrapper_dir = tmp.path().join("wrapper");
        let workspace_root = tmp.path().join("workspace");
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::create_dir_all(&workspace_root).unwrap();
        let manifest = fake_manifest();
        let source = FrameworkSource::Workspace {
            root: workspace_root,
        };
        generate_wrapper(&wrapper_dir, &project_dir, &source, &manifest, &[], false)
            .expect("generate wrapper");
        (wrapper_dir, tmp)
    }

    #[test]
    fn wrapper_has_runtime_core_dep() {
        let (wrapper_dir, _tmp) = run_generator();
        let cargo = std::fs::read_to_string(wrapper_dir.join("Cargo.toml")).unwrap();
        let parsed: toml::Value = toml::from_str(&cargo).expect("valid TOML");
        assert!(
            parsed
                .get("dependencies")
                .and_then(|d| d.get("runtime-core"))
                .is_some(),
            "web wrapper missing runtime-core dep — launcher's \
             `--features runtime-core/dev` will fail. Got:\n{cargo}",
        );
    }

    /// Production-bundle guard: the generated web wrapper must NOT enable
    /// the catalog (`catalog` / its `mcp` alias / the `dev` umbrella) on
    /// runtime-core. Those features pull `mcp-catalog` and make every
    /// `#[component]` / `#[derive(IdealystSchema)]` bake its doc strings +
    /// prop schema into the wasm as `inventory` statics — pure bundle
    /// bloat in a shipped app. The catalog is a dev/tooling concern only
    /// (`idealyst dev` opts in via `--features runtime-core/dev`); the
    /// release bundle carries none of it. If this test ever fails,
    /// documentation/catalog data is about to ship to end users.
    #[test]
    fn wrapper_does_not_enable_catalog_in_production() {
        let (wrapper_dir, _tmp) = run_generator();
        let cargo = std::fs::read_to_string(wrapper_dir.join("Cargo.toml")).unwrap();
        for forbidden in ["runtime-core/catalog", "runtime-core/mcp", "runtime-core/dev"] {
            assert!(
                !cargo.contains(forbidden),
                "production web wrapper must not enable {forbidden} — it pulls \
                 mcp-catalog and bloats the bundle with doc/catalog data. Got:\n{cargo}",
            );
        }
        // And the runtime-core dep line itself must carry no catalog-ish
        // feature, however it's spelled.
        let parsed: toml::Value = toml::from_str(&cargo).expect("valid TOML");
        if let Some(feats) = parsed
            .get("dependencies")
            .and_then(|d| d.get("runtime-core"))
            .and_then(|rc| rc.get("features"))
            .and_then(|f| f.as_array())
        {
            for f in feats {
                let f = f.as_str().unwrap_or("");
                assert!(
                    !matches!(f, "catalog" | "mcp" | "dev"),
                    "runtime-core dep enables catalog feature {f:?} in the production wrapper",
                );
            }
        }
    }

    #[test]
    fn wrapper_runtime_server_feature_pulls_backend_web_runtime_server() {
        let (wrapper_dir, _tmp) = run_generator();
        let cargo = std::fs::read_to_string(wrapper_dir.join("Cargo.toml")).unwrap();
        let parsed: toml::Value = toml::from_str(&cargo).expect("valid TOML");
        let rs = parsed
            .get("features")
            .and_then(|f| f.get("runtime-server"))
            .and_then(|a| a.as_array())
            .expect("web wrapper declares the `runtime-server` feature");
        let entries: Vec<&str> = rs.iter().filter_map(|v| v.as_str()).collect();
        assert!(
            entries.iter().any(|e| *e == "backend-web/runtime-server"),
            "web wrapper `runtime-server` feature must enable backend-web/runtime-server; \
             without it, `idealyst dev --web` produces a local-mount bundle that \
             won't connect to the dev-host. Got {:?}",
            entries,
        );
        // Back-compat: the deprecated `aas` alias still resolves to it.
        let aas = parsed
            .get("features")
            .and_then(|f| f.get("aas"))
            .and_then(|a| a.as_array())
            .expect("web wrapper keeps the deprecated `aas` alias");
        assert!(
            aas.iter().filter_map(|v| v.as_str()).any(|e| e == "runtime-server"),
            "`aas` must alias `runtime-server`",
        );
    }

    /// Regression for the fresh-scaffold `idealyst dev --web` failure:
    /// the wrapper's wrapper-LOCAL features (`runtime-server`, `aas`)
    /// must NEVER be forwarded to the user crate as
    /// `<user>/<feature>`. The dev launcher passes `runtime-server`
    /// (+ `runtime-core/hot-reload`) as `user_features`; before the
    /// fix `user_feature_forwards` emitted
    /// `runtime-server = ["demo/runtime-server"]`, and since a
    /// scaffolded app declares no such feature, cargo failed with
    /// "package ... depends on demo with feature runtime-server but
    /// demo does not have that feature."
    #[test]
    fn runtime_server_feature_is_not_forwarded_to_user_crate() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project_dir = tmp.path().join("project");
        let wrapper_dir = tmp.path().join("wrapper");
        let workspace_root = tmp.path().join("workspace");
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::create_dir_all(&workspace_root).unwrap();
        let manifest = fake_manifest(); // name = "demo"
        let source = FrameworkSource::Workspace {
            root: workspace_root,
        };
        // Exactly what `idealyst dev --web` (runtime-server mode) passes.
        let user_features = vec![
            "runtime-server".to_string(),
            "runtime-core/hot-reload".to_string(),
        ];
        generate_wrapper(&wrapper_dir, &project_dir, &source, &manifest, &user_features, false)
            .expect("generate wrapper");
        let cargo = std::fs::read_to_string(wrapper_dir.join("Cargo.toml")).unwrap();

        assert!(
            !cargo.contains("demo/runtime-server"),
            "wrapper must NOT forward `runtime-server` to the user crate \
             (`demo/runtime-server`) — a fresh scaffold declares no such \
             feature and the build would fail. Got:\n{cargo}",
        );
        // And it must still be a valid, resolvable feature locally.
        let parsed: toml::Value = toml::from_str(&cargo).expect("valid TOML");
        assert!(
            parsed
                .get("features")
                .and_then(|f| f.get("runtime-server"))
                .is_some(),
            "wrapper must declare `runtime-server` locally",
        );
    }
}

#[cfg(test)]
mod bundle_tests {
    //! Coverage for `idealyst build --web --gzip --out-dir`. These
    //! tests don't run wasm-pack — they drive `stage_bundle` /
    //! `gzip_bundle` against a synthetic project layout, so they
    //! stay fast (<10ms) and don't need a wasm toolchain on CI.

    use super::*;
    use std::io::Read as _;

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
