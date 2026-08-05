//! SSR build orchestration for `idealyst dev --ssr` (and `--static`).
//!
//! Mirror of `crates/tools/build/web`, `ios`, `android`, `macos`: the
//! user's app crate stays platform-agnostic — it exposes
//! `pub fn app() -> Element` plus a feature-gated
//! `pub fn register_ssr_scene_handlers(&mut runtime_scene::Registry<backend_ssr::SsrBackend>)`
//! that the wrapper invokes per request to install SDK payload handlers
//! (drawer navigator, code-block external, …). Everything else lives
//! in this generated wrapper.
//!
//! `build()` generates an ephemeral `bin` crate at:
//!
//! ```text
//! <workspace>/target/idealyst/<project>/ssr/wrapper/
//! ```
//!
//! whose `src/main.rs` calls `backend_ssr::serve(...)` with the user's
//! `app` + `register_ssr_extensions`. Two modes, picked at run time
//! via CLI args (so one binary serves both):
//!
//! - **default (hydrate)** — emits `<script>import init from "<bundle>"</script>`
//!   so the live web bundle (`dist/web/pkg/<lib>.js`) boots and adopts
//!   the SSR DOM. Requires `idealyst build --web` to have staged the
//!   bundle alongside.
//! - **`--static`** — no `<script>`, no hydration. Pure server-render
//!   for SEO / unfurls / static preview.
//!
//! The wrapper renders through `backend_ssr::newcore::{render_all,
//! serve}` — a fresh `World` per request — with the user's
//! `register_ssr_scene_handlers` as the registration seam.
//!
//! **Premint builds** (`BuildOptions::premint`): the wrapper compiles
//! with the same `--cfg idealyst_premint*` posture as the wasm bundle
//! (so both sides stamp identical `iy-*` classes during hydration),
//! bakes `const PREMINT: bool = true`, resolves the staged
//! `premint.css`, links it from every document, and arms the
//! minted-class guard per render. Premint builds get an isolated
//! `target-premint/` dir — the RUSTFLAGS change would otherwise
//! invalidate the project's shared native cache in both directions.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use build_ios::{parse_manifest, FrameworkSource, Manifest};

#[derive(Clone, Debug)]
pub struct BuildOptions {
    /// Build in release mode. Default: debug (fast incremental).
    pub release: bool,
    /// Where the wrapper Cargo.toml should source framework crates
    /// from. The CLI constructs this with `FrameworkSource::detect`
    /// before invoking `build()`.
    pub source: FrameworkSource,
    /// Cargo features to enable on the user crate (in addition to the
    /// always-on `ssr` feature). Typically empty; reserved for future
    /// dev-mode flags.
    pub user_features: Vec<String>,
    /// Compile the server with `--cfg idealyst_premint` — REQUIRED
    /// whenever the paired wasm bundle is a premint build, so the server
    /// stamps the same deterministic `iy-*` classes the hydrating client
    /// stamps (the `IntoStyleProp` premint fast path is cfg-gated).
    /// Mismatched postures re-introduce the hydration style divergence
    /// the old `--premint × --ssg/--ssr` bail refused.
    pub premint: bool,
    /// Also set `--cfg idealyst_premint_only` (engine + rule closures
    /// compiled out; non-premintable styles panic, same clean-app
    /// contract as the client).
    pub premint_only: bool,
    /// Also set `--cfg idealyst_premint_report` (per-attach fall-through
    /// logging; serve mode re-warns per request since each renders on a
    /// fresh thread — dev-only diagnostics).
    pub premint_report: bool,
}

#[derive(Debug)]
pub struct BuildArtifact {
    /// Path to the built SSR binary. The CLI spawns this with
    /// `--addr <addr>` and optionally `--static`.
    pub binary: PathBuf,
    /// Path to the generated wrapper crate. Useful for debugging.
    pub wrapper_dir: PathBuf,
}

/// Build the user's project at `project_dir` as a native SSR server
/// binary. Returns the path to the produced binary; spawn it with
/// `<binary> --addr <host:port> [--static] [--static-dir <path>]
/// [--bundle <url>]`.
pub fn build(project_dir: &Path, opts: BuildOptions) -> Result<BuildArtifact> {
    let project_dir = fs::canonicalize(project_dir)
        .with_context(|| format!("resolve project dir {}", project_dir.display()))?;
    let manifest = parse_manifest(&project_dir)?;

    let wrapper_dir = opts
        .source
        .wrapper_root(&project_dir)
        .join(&manifest.name)
        .join("ssr/wrapper");
    generate_wrapper(&wrapper_dir, &project_dir, &opts.source, &manifest, opts.premint)?;

    cargo_build(&wrapper_dir, &opts)?;

    let profile = if opts.release { "release" } else { "debug" };
    let bin_name = binary_name(&manifest.name);
    // Non-premint: `write_shared_target_config` points the wrapper's
    // cargo at the project's shared `target/` (so common deps aren't
    // recompiled per-wrapper) and the binary lands there. Premint: the
    // build injects `--cfg idealyst_premint*` RUSTFLAGS, and cargo
    // fingerprints every unit on RUSTFLAGS — sharing the target dir
    // would rebuild the whole native graph AND invalidate the project's
    // normal dev/test artifacts in both directions (the premint dump
    // wrapper documents the same concern). So premint builds get their
    // own `target-premint/` under the wrapper, cached across premint
    // builds and never thrashing the shared cache.
    let binary = if opts.premint {
        wrapper_dir.join("target-premint").join(profile).join(&bin_name)
    } else {
        project_dir.join("target").join(profile).join(&bin_name)
    };

    if !binary.is_file() {
        anyhow::bail!(
            "cargo build reported success but the SSR binary was not produced at {}",
            binary.display(),
        );
    }

    Ok(BuildArtifact { binary, wrapper_dir })
}

/// Wrapper binary name. Suffixed with `-ssr` so it coexists with the
/// other per-target binaries the CLI generates (`<name>-macos`, etc.).
fn binary_name(project_name: &str) -> String {
    format!("{project_name}-ssr")
}

/// Materialize the wrapper crate at `wrapper_dir`. Idempotent —
/// overwrites whatever was there. Public so a future
/// `idealyst scaffold ssr` command can drive the same generator.
///
/// The generated binary renders through
/// `backend_ssr::newcore::{render_all, serve}` with the user's
/// `register_ssr_scene_handlers` as the scene-registry seam.
pub fn generate_wrapper(
    wrapper_dir: &Path,
    project_dir: &Path,
    source: &FrameworkSource,
    manifest: &Manifest,
    premint: bool,
) -> Result<()> {
    fs::create_dir_all(wrapper_dir.join("src"))
        .with_context(|| format!("create {}", wrapper_dir.display()))?;

    let bin_name = binary_name(&manifest.name);
    // `new-core` compiles backend-ssr's per-request-`World` render path.
    // The feature is vacuous once backend-ssr makes its contents
    // unconditional; drop it from this list at that point.
    let bssr_dep = source.dep("crates/backend/ssr", &["serve"]);

    // Bake the favicon `<link>` snippet into the wrapper. Empty
    // string when the project has no `[icon]` block — the wrapper
    // still compiles and serves; `extra_head` injection just no-ops
    // for an empty value (see `render_document` in backend-ssr).
    // `{:?}` on a string produces a Rust-quoted literal with `"` and
    // `\` properly escaped — safe to embed inside the outer
    // `format!` without any further escaping work here.
    let extra_head_snippet = match icon_gen::load_config_from_manifest(project_dir)? {
        Some(_) => icon_gen::web_icon_link_tags(),
        None => String::new(),
    };
    let extra_head_literal = format!("{extra_head_snippet:?}");

    // User-crate dep line: the app's own defaults plus `ssr` (which is
    // what exposes the registration seam).
    let user_dep = format!(
        "{user_name} = {{ path = \"{user_path}\", features = [\"ssr\"] }}",
        user_name = manifest.name,
        user_path = project_dir.display(),
    );

    let cargo_toml = format!(
        r#"# GENERATED by `idealyst dev --ssr` / `--static`. Do not edit —
# rewritten every build. Run `idealyst scaffold ssr` to materialize an
# editable copy of this wrapper into your repo (once that command lands).

[workspace]

[package]
name = "{bin_name}"
version = "0.0.1"
edition = "2021"

[[bin]]
name = "{bin_name}"
path = "src/main.rs"

[dependencies]
# `serve` feature pulls in the `tiny_http` runtime. The CLI is the
# only caller of build-ssr today; this dep is always wanted.
backend-ssr = {bssr_dep}
# User crate compiled with the `ssr` feature flipped on, which is
# what exposes the wrapper's registration seam
# (`register_ssr_scene_handlers`) and enables the `backend-ssr` dep on
# the user side.
{user_dep}
{patch_block}
"#,
        bin_name = bin_name,
        bssr_dep = bssr_dep,
        user_dep = user_dep,
        patch_block = source.patch_block(),
    );

    // Render entries + registration seam.
    let entry_imports = "use backend_ssr::newcore::{render_all, serve};\n\
                         use backend_ssr::{render_document, ServeConfig};";
    let register_arg =
        format!("|r| {lib}::register_ssr_scene_handlers(r)", lib = manifest.lib_name);

    let main_rs = format!(
        r##"//! GENERATED by `idealyst dev --ssr` / `--static` / `idealyst build --ssg`.
//! Three modes selected at run time:
//!
//! - default — runtime HTTP server. Hydration: emits the boot
//!   `<script>` so the live web bundle adopts the server DOM.
//! - `--static` (with no `--export`) — runtime HTTP server, no
//!   `<script>`; pure server-render.
//! - `--export <dir>` — one-shot SSG. Crawls every literal route in
//!   the app's navigator hierarchy (via `backend_ssr::render_all`) and
//!   writes `<dir>/<path>/index.html` per page. Pair with `--static`
//!   to suppress the boot script for a pure-static deploy.

{entry_imports}
use std::fs;
use std::path::PathBuf;

/// Favicon `<link>` tag block baked in at wrapper-generation time from
/// `[package.metadata.idealyst.app.icon]`. Empty string when the
/// project has no icon block — the framework's `render_document`
/// no-ops on empty so an unconfigured project still serves clean
/// HTML.
const EXTRA_HEAD: &str = {extra_head_literal};

/// Baked at wrapper-generation time: whether the paired wasm bundle is a
/// premint build. When true this binary was ALSO compiled with
/// `--cfg idealyst_premint*`, so it stamps the same `iy-*` classes the
/// hydrating client stamps — and it must link `premint.css` from every
/// document and arm the minted-class guard, below.
const PREMINT: bool = {premint};

fn main() {{
    // Defaults match `idealyst dev`'s out-of-the-box ports + bundle path.
    let mut addr = "127.0.0.1:8081".to_string();
    let mut static_only = false;
    let mut bundle_module: Option<String> = Some("/pkg/{lib}.js".to_string());
    let mut bundle_overridden = false;
    let mut static_dir: Option<PathBuf> = None;
    let mut export_dir: Option<PathBuf> = None;

    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {{
        let arg = argv[i].as_str();
        match arg {{
            "--static" | "static" => {{
                static_only = true;
            }}
            "--addr" if i + 1 < argv.len() => {{
                addr = argv[i + 1].clone();
                i += 1;
            }}
            "--bundle" if i + 1 < argv.len() => {{
                bundle_module = Some(argv[i + 1].clone());
                bundle_overridden = true;
                i += 1;
            }}
            "--static-dir" if i + 1 < argv.len() => {{
                static_dir = Some(PathBuf::from(&argv[i + 1]));
                i += 1;
            }}
            "--export" if i + 1 < argv.len() => {{
                export_dir = Some(PathBuf::from(&argv[i + 1]));
                i += 1;
            }}
            other if !other.starts_with('-') && other.contains(':') => {{
                // bare `host:port` positional, matches the website
                // example's serve CLI.
                addr = other.to_string();
            }}
            _ => {{}}
        }}
        i += 1;
    }}

    // `idealyst build --web` content-addresses the staged bundle
    // (`pkg/<lib>.<hash>.js` — cache busting), so the unhashed default
    // above would 404 against a production `--static-dir`. Resolve the
    // fingerprinted entry from the served dir unless the caller pinned
    // one with `--bundle`. A dev-shaped pkg (plain names) resolves to
    // `None` and the default stands.
    if !bundle_overridden {{
        if let Some(dir) = static_dir.as_deref() {{
            if let Some(found) = backend_ssr::resolve_bundle_module(dir, "{lib}") {{
                bundle_module = Some(found);
            }}
        }}
    }}

    // Premint builds: resolve the staged `premint.css` (export mode from
    // the export dir, serve mode from `--static-dir` — both hold the
    // web build's `pkg/`), arm the process-wide minted-class guard from
    // its scanned classes, and thread the link into every document.
    let mut premint_css: Option<backend_ssr::PremintCss> = None;
    if PREMINT {{
        let root = export_dir.clone().or_else(|| static_dir.clone());
        match root.as_deref().and_then(backend_ssr::resolve_premint_css) {{
            Some(css) => {{
                backend_ssr::install_premint_classes(css.classes.clone());
                premint_css = Some(css);
            }}
            None => eprintln!(
                "[ssr] warning: premint build but no premint.css was found \
                 under the export/static dir's pkg/ — rendered pages will \
                 link no stylesheet and preminted classes will have no CSS. \
                 Run `idealyst build --web` with the same premint flags first."
            ),
        }}
    }}

    if let Some(out) = export_dir {{
        // SSG mode: one-shot crawl + write. No HTTP server.
        if static_only {{
            bundle_module = None;
        }}
        let module_ref = bundle_module.as_deref();
        let result = render_all(
            {register_arg},
            {lib}::app,
        );
        let mut written: Vec<String> = Vec::with_capacity(result.pages.len());
        for (path, page) in &result.pages {{
            // `/` → `<out>/index.html`; `/about` → `<out>/about/index.html`.
            // Per-page directory so S3 + CloudFront with
            // `index document = index.html` resolves deep links without
            // any rewrite rule.
            let rel = path.trim_start_matches('/').trim_end_matches('/');
            let file = if rel.is_empty() {{
                out.join("index.html")
            }} else {{
                out.join(rel).join("index.html")
            }};
            if let Some(parent) = file.parent() {{
                fs::create_dir_all(parent).expect("create export dir");
            }}
            let html = render_document(page, module_ref, Some(EXTRA_HEAD), premint_css.as_ref());
            fs::write(&file, html).expect("write SSG html");
            written.push(file.display().to_string());
        }}
        written.sort();
        println!(
            "SSG: wrote {{n}} page(s){{mode}}",
            n = written.len(),
            mode = if module_ref.is_some() {{ " (hydration mode)" }} else {{ " (static, no script)" }},
        );
        for w in &written {{
            println!("  {{}}", w);
        }}
        if !result.skipped_parameterized.is_empty() {{
            eprintln!(
                "SSG: skipped {{}} parameterized route(s) (need explicit param values):",
                result.skipped_parameterized.len(),
            );
            for p in &result.skipped_parameterized {{
                eprintln!("  {{}}", p);
            }}
        }}
        return;
    }}

    if static_only {{
        bundle_module = None;
        println!("SSR (static, no hydration) on http://{{addr}}");
    }} else {{
        println!("SSR + hydration on http://{{addr}}");
    }}

    serve(
        &addr,
        ServeConfig {{
            bundle_module,
            static_dir,
            extra_head: Some(EXTRA_HEAD.to_string()),
            premint_css,
        }},
        {register_arg},
        {lib}::app,
    )
    .expect("SSR server failed to start");
}}
"##,
        lib = manifest.lib_name,
        extra_head_literal = extra_head_literal,
        entry_imports = entry_imports,
        register_arg = register_arg,
        premint = premint,
    );

    write_target_config(wrapper_dir, project_dir, premint)?;
    fs::write(wrapper_dir.join("Cargo.toml"), cargo_toml)?;
    fs::write(wrapper_dir.join("src/main.rs"), main_rs)?;
    Ok(())
}

/// Point the wrapper crate's build output somewhere deliberate — and
/// always WRITE the file, since the wrapper dir persists across builds
/// and a stale config from the other mode must not survive a
/// premint/non-premint flip.
///
/// - Non-premint: the project's shared `target/`, so common
///   dependencies aren't recompiled per wrapper invocation.
/// - Premint: a wrapper-local `target-premint/`. The premint build
///   injects `--cfg idealyst_premint*` RUSTFLAGS and cargo fingerprints
///   every unit on RUSTFLAGS — sharing the project target would rebuild
///   the entire native graph and invalidate the project's normal
///   dev/test artifacts in both directions (the premint dump wrapper
///   documents the same hazard). Isolated, the premint graph caches
///   across premint builds and never thrashes the shared cache.
fn write_target_config(dir: &Path, project_dir: &Path, premint: bool) -> Result<()> {
    let config = if premint {
        "# GENERATED. Premint builds carry --cfg RUSTFLAGS that would\n\
         # invalidate the project's shared target cache, so they build\n\
         # into their own dir (cached across premint builds).\n\
         \n\
         [build]\n\
         target-dir = \"target-premint\"\n"
            .to_string()
    } else {
        let target_dir = project_dir.join("target");
        format!(
            "# GENERATED. Share the project's `target/` so common\n\
             # dependencies aren't recompiled per-wrapper.\n\
             \n\
             [build]\n\
             target-dir = \"{}\"\n",
            target_dir.display(),
        )
    };
    fs::create_dir_all(dir.join(".cargo"))?;
    fs::write(dir.join(".cargo/config.toml"), config)?;
    Ok(())
}

/// The `--cfg` RUSTFLAGS a premint server build injects — a pure fn so
/// the template tests can pin it. Mirrors `build-web`'s wasm-side
/// injection (same cfgs, same premint ⊇ premint_only/report implication
/// handled by the caller setting `premint` for any of the three).
fn premint_rustflags(premint: bool, premint_only: bool, premint_report: bool) -> Vec<String> {
    let mut flags = Vec::new();
    if premint {
        flags.push("--cfg".to_string());
        flags.push("idealyst_premint".to_string());
    }
    if premint_report {
        flags.push("--cfg".to_string());
        flags.push("idealyst_premint_report".to_string());
    }
    if premint_only {
        flags.push("--cfg".to_string());
        flags.push("idealyst_premint_only".to_string());
    }
    flags
}

fn cargo_build(wrapper_dir: &Path, opts: &BuildOptions) -> Result<()> {
    let release = opts.release;
    let user_features = &opts.user_features;
    let mut cmd = Command::new("cargo");
    cmd.args(["build"]).current_dir(wrapper_dir);
    if release {
        cmd.arg("--release");
    }
    // Premint cfgs ride CARGO_ENCODED_RUSTFLAGS exactly as the wasm
    // build's do (build-web `cargo_build_wasm`), seeded from any
    // caller-provided flags so we extend rather than clobber.
    let cfgs = premint_rustflags(opts.premint, opts.premint_only, opts.premint_report);
    if !cfgs.is_empty() {
        let mut flags: Vec<String> = match std::env::var("CARGO_ENCODED_RUSTFLAGS") {
            Ok(v) if !v.is_empty() => v.split('\x1f').map(str::to_string).collect(),
            _ => match std::env::var("RUSTFLAGS") {
                Ok(v) if !v.is_empty() => v.split_whitespace().map(str::to_string).collect(),
                _ => Vec::new(),
            },
        };
        flags.extend(cfgs);
        cmd.env("CARGO_ENCODED_RUSTFLAGS", flags.join("\x1f"));
    }
    // User features are forwarded to the wrapper's user-crate dep, NOT
    // applied at the wrapper-crate level (the wrapper itself has no
    // [features]). cargo's `--features` looks them up on the root
    // package by default — for now we treat the wrapper as the root
    // and rely on the always-on `ssr` feature in the dep declaration.
    // If user_features are passed they're appended for the cargo build,
    // which works for transitive feature paths like
    // `user-crate/some-feat`.
    if !user_features.is_empty() {
        cmd.arg("--features").arg(user_features.join(","));
    }
    eprintln!(
        "[build-ssr] cargo build{} (in {})",
        if release { " --release" } else { "" },
        wrapper_dir.display(),
    );
    let status = cmd
        .status()
        .with_context(|| "spawn `cargo` — is it on your PATH?")?;
    if !status.success() {
        anyhow::bail!("cargo build failed for the SSR wrapper at {}", wrapper_dir.display());
    }
    Ok(())
}

#[cfg(test)]
mod wrapper_template_tests {
    //! Shape regression for the generated SSR wrapper. The wrapper is
    //! ephemeral generated source, so drift is invisible until a user
    //! runs it — these tests pin the load-bearing lines.

    use super::*;

    fn generated_wrapper_with(premint: bool) -> (String, String, String) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project_dir = tmp.path().join("project");
        let wrapper_dir = tmp.path().join("wrapper");
        fs::create_dir_all(&project_dir).unwrap();
        fs::write(
            project_dir.join("Cargo.toml"),
            "[package]\nname = \"demo-app\"\nversion = \"0.0.1\"\n",
        )
        .unwrap();
        let manifest = parse_manifest(&project_dir).expect("parse manifest");
        let source = FrameworkSource::Workspace {
            root: tmp.path().join("workspace"),
        };
        generate_wrapper(&wrapper_dir, &project_dir, &source, &manifest, premint)
            .expect("generate wrapper");
        (
            fs::read_to_string(wrapper_dir.join("src/main.rs")).unwrap(),
            fs::read_to_string(wrapper_dir.join("Cargo.toml")).unwrap(),
            fs::read_to_string(wrapper_dir.join(".cargo/config.toml")).unwrap(),
        )
    }

    fn generated_wrapper() -> (String, String) {
        let (main_rs, cargo_toml, _) = generated_wrapper_with(false);
        (main_rs, cargo_toml)
    }

    fn generated_main_rs() -> String {
        generated_wrapper().0
    }

    /// A production bundle staged by `idealyst build --web` has a
    /// content-hashed entry (`pkg/<lib>.<hash>.js`); the wrapper must
    /// resolve it from `--static-dir` instead of hardcoding the
    /// unhashed default (which would 404 → silent no-hydration).
    #[test]
    fn wrapper_resolves_fingerprinted_bundle_from_static_dir() {
        let main_rs = generated_main_rs();
        assert!(
            main_rs.contains("backend_ssr::resolve_bundle_module(dir, \"demo_app\")"),
            "wrapper must resolve the hashed entry for the project's lib_name:\n{main_rs}",
        );
        assert!(
            main_rs.contains("if !bundle_overridden {"),
            "an explicit --bundle must win over auto-resolution:\n{main_rs}",
        );
        assert!(
            main_rs.contains("bundle_overridden = true;"),
            "--bundle parsing must record the override:\n{main_rs}",
        );
    }

    /// The wrapper renders through `backend_ssr::newcore::{render_all,
    /// serve}` with the scene-registry seam, and takes a plain
    /// `features = ["ssr"]` dep on the user crate — no core pin, since
    /// there is one core.
    #[test]
    fn wrapper_uses_newcore_entries_and_scene_seam() {
        let (main_rs, cargo_toml) = generated_wrapper();
        assert!(
            main_rs.contains("use backend_ssr::newcore::{render_all, serve};"),
            "wrapper imports the newcore entries:\n{main_rs}",
        );
        assert!(
            main_rs.contains("|r| demo_app::register_ssr_scene_handlers(r)"),
            "wrapper registers through the scene-registry seam:\n{main_rs}",
        );
        assert!(
            !main_rs.contains("register_ssr_extensions"),
            "wrapper must not reference the deleted old registration seam:\n{main_rs}",
        );
        assert!(
            cargo_toml.contains("features = [\"ssr\"] }"),
            "user crate keeps its own defaults plus `ssr`:\n{cargo_toml}",
        );
        assert!(
            !cargo_toml.contains("default-features = false"),
            "wrapper must not pin a core on the user crate:\n{cargo_toml}",
        );
        assert!(
            !cargo_toml.contains("old-core"),
            "wrapper must not mention old-core anywhere:\n{cargo_toml}",
        );
    }

    /// WIRING half of the premint×SSR contract (the mechanism half —
    /// stamping, guard veto, link ordering — lives in backend-ssr's unit
    /// tests). A premint wrapper must: bake `PREMINT = true`, resolve
    /// `premint.css` from the export/static dir, arm the process guard,
    /// and thread the link into both render modes. A non-premint wrapper
    /// must bake `false` and change nothing else.
    #[test]
    fn premint_wrapper_bakes_flag_resolves_css_and_arms_guard() {
        let (main_rs, _cargo, config) = generated_wrapper_with(true);
        assert!(
            main_rs.contains("const PREMINT: bool = true;"),
            "premint wrapper bakes the flag:\n{main_rs}"
        );
        assert!(
            main_rs.contains("backend_ssr::resolve_premint_css"),
            "wrapper resolves the staged premint.css:\n{main_rs}"
        );
        assert!(
            main_rs.contains("backend_ssr::install_premint_classes"),
            "wrapper arms the process-wide minted-class guard:\n{main_rs}"
        );
        assert!(
            main_rs.contains("render_document(page, module_ref, Some(EXTRA_HEAD), premint_css.as_ref())"),
            "export mode threads the link:\n{main_rs}"
        );
        assert!(
            main_rs.contains("premint_css,"),
            "serve mode threads the link through ServeConfig:\n{main_rs}"
        );
        // Premint builds must NOT share the project target dir — the
        // --cfg RUSTFLAGS would invalidate the whole shared cache.
        assert!(
            config.contains("target-dir = \"target-premint\""),
            "premint wrapper isolates its target dir:\n{config}"
        );

        let (main_rs, _cargo, config) = generated_wrapper_with(false);
        assert!(
            main_rs.contains("const PREMINT: bool = false;"),
            "non-premint wrapper bakes false:\n{main_rs}"
        );
        assert!(
            !config.contains("target-premint"),
            "non-premint wrapper keeps the shared target dir:\n{config}"
        );
    }

    /// The premint server build must inject the SAME cfgs the wasm build
    /// injects, or the two sides of hydration disagree about premint
    /// posture — the exact divergence the old bail refused.
    #[test]
    fn premint_build_sets_cfgs_and_isolates_target_dir() {
        assert_eq!(
            premint_rustflags(true, false, false),
            vec!["--cfg", "idealyst_premint"],
        );
        assert_eq!(
            premint_rustflags(true, true, false),
            vec!["--cfg", "idealyst_premint", "--cfg", "idealyst_premint_only"],
        );
        assert_eq!(
            premint_rustflags(true, false, true),
            vec!["--cfg", "idealyst_premint", "--cfg", "idealyst_premint_report"],
        );
        assert!(premint_rustflags(false, false, false).is_empty());
    }
}
