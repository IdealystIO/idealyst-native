//! SSR build orchestration for `idealyst dev --ssr` (and `--static`).
//!
//! Mirror of `crates/tools/build/web`, `ios`, `android`, `macos`: the
//! user's app crate stays platform-agnostic — it exposes
//! `pub fn app() -> Element` plus a feature-gated
//! `pub fn register_ssr_extensions(&mut backend_ssr::SsrBackend)` that
//! the wrapper invokes per request to install SDK chrome handlers
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
//! With [`BuildOptions::new_core`] (the CLI's `idealyst build
//! --ssr/--ssg --new-core`) the SAME wrapper surface is generated
//! against `backend_ssr::newcore::{render_all, serve}` — per-request
//! `World`s instead of the old walker — with the user's
//! `register_ssr_scene_handlers` as the registration seam. See
//! `generate_wrapper` for the dual-core dep-graph rules.

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
    /// Build the wrapper against the NEW core (idea-lite migration):
    /// renders through `backend_ssr::newcore::{render_all, serve}` on
    /// per-request `World`s instead of the old walker. Requires the
    /// dual-core app convention (the website / idea-ui-docs shape):
    /// the user crate must expose a `new-core` cargo feature (its
    /// default features are disabled — one core per build graph) and a
    /// `register_ssr_scene_handlers(&mut runtime_scene::Registry<backend_ssr::SsrBackend>)`
    /// fn (the scene-registry seam replacing the old
    /// `register_ssr_extensions(&mut SsrBackend)`). The CLI sets this
    /// from `idealyst build --new-core`.
    pub new_core: bool,
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
    generate_wrapper(&wrapper_dir, &project_dir, &opts.source, &manifest, opts.new_core)?;

    cargo_build(&wrapper_dir, opts.release, &opts.user_features)?;

    let profile = if opts.release { "release" } else { "debug" };
    let bin_name = binary_name(&manifest.name);
    // `write_shared_target_config` points the wrapper's cargo at the
    // project's shared `target/` (so common deps aren't recompiled
    // per-wrapper). The binary lands there, not inside the wrapper's
    // own `target/`.
    let binary = project_dir
        .join("target")
        .join(profile)
        .join(&bin_name);

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
/// `new_core` swaps BOTH generated files onto the new-core leg (see
/// [`BuildOptions::new_core`]): the dep graph compiles the user crate
/// with `default-features = false, features = ["ssr", "new-core"]`
/// (one core per build graph — a dual-core app's `old-core` default
/// must not unify in) and the binary renders through
/// `backend_ssr::newcore::{render_all, serve}` with the user's
/// `register_ssr_scene_handlers` as the scene-registry seam. A
/// mode-specific wrapper (rather than a cargo feature on one wrapper)
/// because the `default-features = false` line cannot be
/// feature-conditional in Cargo — and the wrapper is regenerated every
/// build anyway.
pub fn generate_wrapper(
    wrapper_dir: &Path,
    project_dir: &Path,
    source: &FrameworkSource,
    manifest: &Manifest,
    new_core: bool,
) -> Result<()> {
    fs::create_dir_all(wrapper_dir.join("src"))
        .with_context(|| format!("create {}", wrapper_dir.display()))?;

    let bin_name = binary_name(&manifest.name);
    let bssr_features: &[&str] = if new_core { &["serve", "new-core"] } else { &["serve"] };
    let bssr_dep = source.dep("crates/backend/ssr", bssr_features);

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

    // User-crate dep line per mode. New core disables default features:
    // dual-core apps default to `old-core`, and exactly one core may be
    // in a build graph (the proc-macro lowering is graph-wide).
    let user_dep = if new_core {
        format!(
            "{user_name} = {{ path = \"{user_path}\", default-features = false, \
             features = [\"ssr\", \"new-core\"] }}",
            user_name = manifest.name,
            user_path = project_dir.display(),
        )
    } else if build_ios::declares_feature(project_dir, "old-core") {
        // Dual-core apps default to new-core since the runtime-v2
        // defaults flip — the old-core build must pin single-core.
        format!(
            "{user_name} = {{ path = \"{user_path}\", default-features = false, \
             features = [\"ssr\", \"old-core\"] }}",
            user_name = manifest.name,
            user_path = project_dir.display(),
        )
    } else {
        format!(
            "{user_name} = {{ path = \"{user_path}\", features = [\"ssr\"] }}",
            user_name = manifest.name,
            user_path = project_dir.display(),
        )
    };

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
# (`register_ssr_extensions` on the old core,
# `register_ssr_scene_handlers` under `--new-core`) and enables the
# `backend-ssr` dep on the user side.
{user_dep}
{patch_block}
"#,
        bin_name = bin_name,
        bssr_dep = bssr_dep,
        user_dep = user_dep,
        patch_block = source.patch_block(),
    );

    // Per-core render entries + registration seam. Same runtime CLI
    // surface either way — only the imports and the register closure
    // differ, so the two binaries are drop-in interchangeable.
    let (entry_imports, register_arg) = if new_core {
        (
            "use backend_ssr::newcore::{render_all, serve};\n\
             use backend_ssr::{render_document, ServeConfig};",
            format!("|r| {lib}::register_ssr_scene_handlers(r)", lib = manifest.lib_name),
        )
    } else {
        (
            "use backend_ssr::{render_all, render_document, serve, ServeConfig};",
            format!("|b| {lib}::register_ssr_extensions(b)", lib = manifest.lib_name),
        )
    };

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
            let html = render_document(page, module_ref, Some(EXTRA_HEAD));
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
    );

    write_shared_target_config(wrapper_dir, project_dir)?;
    fs::write(wrapper_dir.join("Cargo.toml"), cargo_toml)?;
    fs::write(wrapper_dir.join("src/main.rs"), main_rs)?;
    Ok(())
}

/// Redirect the wrapper crate's build output back into the project's
/// shared `target/` so common dependencies aren't recompiled per
/// wrapper invocation.
fn write_shared_target_config(dir: &Path, project_dir: &Path) -> Result<()> {
    let target_dir = project_dir.join("target");
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

fn cargo_build(wrapper_dir: &Path, release: bool, user_features: &[String]) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.args(["build"]).current_dir(wrapper_dir);
    if release {
        cmd.arg("--release");
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

    fn generated_wrapper(new_core: bool) -> (String, String) {
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
        generate_wrapper(&wrapper_dir, &project_dir, &source, &manifest, new_core)
            .expect("generate wrapper");
        (
            fs::read_to_string(wrapper_dir.join("src/main.rs")).unwrap(),
            fs::read_to_string(wrapper_dir.join("Cargo.toml")).unwrap(),
        )
    }

    fn generated_main_rs() -> String {
        generated_wrapper(false).0
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

    /// The old-core wrapper keeps its shipped shape: old-core entries,
    /// `register_ssr_extensions`, default features on the user crate.
    #[test]
    fn old_core_wrapper_uses_old_entries() {
        let (main_rs, cargo_toml) = generated_wrapper(false);
        assert!(
            main_rs.contains("use backend_ssr::{render_all, render_document, serve, ServeConfig};"),
            "old wrapper imports the old-core entries:\n{main_rs}",
        );
        assert!(
            main_rs.contains("|b| demo_app::register_ssr_extensions(b)"),
            "old wrapper registers through the SsrBackend seam:\n{main_rs}",
        );
        assert!(
            cargo_toml.contains("features = [\"ssr\"] }"),
            "old wrapper keeps the user crate's default features:\n{cargo_toml}",
        );
        assert!(
            !cargo_toml.contains("\"new-core\""),
            "old wrapper must not enable a new-core feature anywhere:\n{cargo_toml}",
        );
    }

    /// `--new-core` swaps the wrapper onto `backend_ssr::newcore::{render_all,
    /// serve}` with the scene-registry seam, and compiles the user crate
    /// single-core (`default-features = false` + `new-core`) — a dual-core
    /// app's `old-core` default must not unify into the graph.
    #[test]
    fn new_core_wrapper_uses_newcore_entries_and_single_core_dep() {
        let (main_rs, cargo_toml) = generated_wrapper(true);
        assert!(
            main_rs.contains("use backend_ssr::newcore::{render_all, serve};"),
            "new-core wrapper imports the newcore entries:\n{main_rs}",
        );
        assert!(
            main_rs.contains("|r| demo_app::register_ssr_scene_handlers(r)"),
            "new-core wrapper registers through the scene-registry seam:\n{main_rs}",
        );
        assert!(
            !main_rs.contains("register_ssr_extensions"),
            "new-core wrapper must not touch the old registration seam:\n{main_rs}",
        );
        assert!(
            cargo_toml.contains(
                "default-features = false, features = [\"ssr\", \"new-core\"] }"
            ),
            "new-core wrapper compiles the user crate single-core:\n{cargo_toml}",
        );
        assert!(
            cargo_toml.contains("\"new-core\""),
            "backend-ssr dep must carry the new-core feature:\n{cargo_toml}",
        );
    }
}
