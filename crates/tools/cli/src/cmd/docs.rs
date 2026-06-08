//! `idealyst docs` — build + serve a catalog-driven documentation site
//! for a project.
//!
//! The docs *renderer* is the `docs-app` crate (`crates/tools/docs-app`):
//! it embeds a `catalog.json` at build time and auto-generates a browsable
//! site (sidebar by kind; per-entry detail pages with props, types,
//! composition, methods, recipes, icons). On its own it embeds the
//! **framework** catalog. This command makes it document **any** project:
//!
//! 1. **Extract the project's catalog JSON** the same way `idealyst mcp`
//!    does — generate the ephemeral `catalog` wrapper
//!    ([`catalog_wrapper::generate`]) and run its `catalog` bin, capturing
//!    the JSON it prints. That JSON carries every `#[component]` /
//!    primitive / utility / type / guide / icon set in the project *and*
//!    its component-library dependencies.
//! 2. **Generate an ephemeral docs-app project** that re-exports
//!    `docs-app`'s `app()` / `register_extensions()`. It exists so the web
//!    build has a local project crate to point at in BOTH workspace and
//!    git mode (git mode has no local `docs-app` checkout). It mirrors the
//!    `catalog_wrapper` shape (empty `[workspace]`, framework-sourced dep).
//! 3. **Build the web bundle** via [`build_web::build`], exporting
//!    `IDEALYST_DOCS_CATALOG` so `docs-app`'s build script embeds the
//!    project's extracted catalog instead of the framework's.
//! 4. **Serve** the staged bundle on a local port.
//!
//! Known v1 limitation: a *user* project's own recipes render source-only
//! and its custom icon packs render names-only (no live preview / glyph
//! grid) — those need the renderer to link the project's `pub` recipe fns
//! + icon registries, which the JSON-injection path doesn't. Framework
//! recipes/icons (which `docs-app` links) still render live.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use build_ios::FrameworkSource;
use dev_http::serve_static;

use crate::framework_source;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Project to document. Defaults to the current directory. Its own
    /// components plus every component-library dependency are included.
    #[arg(default_value = ".")]
    pub project: PathBuf,

    /// HTTP port for the docs server.
    #[arg(long, default_value_t = 8300)]
    pub port: u16,

    /// Interface to bind. `127.0.0.1` for loopback only; `0.0.0.0` to
    /// expose to the LAN (e.g. to read the docs from a phone over Wi-Fi).
    #[arg(long, default_value = "0.0.0.0")]
    pub host: String,

    /// Build the docs bundle in release mode (smaller wasm, slower build).
    /// The default debug build is faster to produce and fine for local
    /// reading.
    #[arg(long)]
    pub release: bool,

    /// Open the docs site in the default browser once it's serving.
    #[arg(long)]
    pub open: bool,
}

pub fn run(args: Args) -> Result<()> {
    // Absolute project path — the wrapper crates live elsewhere on disk
    // and reference the project by path, so a relative `.` would resolve
    // against the wrong dir.
    let project = std::fs::canonicalize(&args.project)
        .with_context(|| format!("resolve project dir {}", args.project.display()))?;
    let source = framework_source::resolve(&project)?;

    // Per-project staging root, alongside the other `target/idealyst/...`
    // wrappers. Named from the project directory (not the manifest) so it
    // works even when pointed at a workspace whose catalog we extract
    // best-effort.
    let stage_name = sanitize_pkg_name(
        project
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("project"),
    );
    let docs_root = source.wrapper_root(&project).join(&stage_name).join("docs");
    std::fs::create_dir_all(&docs_root)
        .with_context(|| format!("create {}", docs_root.display()))?;

    // 1. Extract the project's catalog → docs_root/catalog.json. Best
    //    effort: if the project is a bare workspace or extraction fails,
    //    we warn and fall back to the framework catalog (no injection).
    let catalog_json = docs_root.join("catalog.json");
    let injected = extract_catalog(&project, &catalog_json);

    // 2. Generate the ephemeral docs-app project crate.
    let app_dir = generate_docs_app(&docs_root, &source, &stage_name)?;

    // 3. Build the web bundle, embedding the extracted catalog. Setting the
    //    env var on this process propagates to the wasm-pack/cargo children
    //    `build_web` spawns, so `docs-app`'s build script reads it. We only
    //    set it when extraction succeeded; otherwise the build embeds the
    //    framework catalog (docs-app's standalone default).
    if injected {
        // Safety: single-threaded CLI startup; no other thread is reading
        // the environment concurrently.
        std::env::set_var("IDEALYST_DOCS_CATALOG", &catalog_json);
    } else {
        std::env::remove_var("IDEALYST_DOCS_CATALOG");
    }

    let bundle_out = project.join("dist/docs");
    println!(
        "[idealyst docs] building docs for {} ({} catalog)…",
        project.display(),
        if injected { "project" } else { "framework fallback" },
    );
    let artifact = build_web::build(
        &app_dir,
        build_web::BuildOptions {
            release: args.release,
            source: source.clone(),
            user_features: Vec::new(),
            bundle_out_dir: Some(bundle_out.clone()),
            gzip: false,
            strip_panics: false,
            // Pure SPA docs site — no SSR/SSG HTML to adopt, so the
            // hydration machinery can DCE out of the wasm.
            hydrate: false,
            // Debug builds skip data pruning; release docs aren't size-
            // critical enough to risk the heuristic, so leave it off.
            prune_dead_data_min: None,
        },
    )
    .context("build the docs web bundle")?;

    let serve_dir = artifact.bundle_dir.unwrap_or(bundle_out);

    // 4. Serve. Print a loopback URL even when bound to 0.0.0.0 so it's
    //    clickable.
    let click_host = if args.host == "0.0.0.0" { "127.0.0.1" } else { &args.host };
    let url = format!("http://{click_host}:{}", args.port);
    println!("[idealyst docs] serving {} at {url}", serve_dir.display());
    if args.open {
        open_browser(&url);
    }
    serve_static(&args.host, args.port, &serve_dir, None, None, None, None, None, None)
}

/// Build + run the ephemeral `catalog` bin for `project` and write its
/// JSON stdout to `dest`. Returns `true` on success. Best effort: any
/// failure logs to stderr and returns `false` so the caller falls back to
/// the framework catalog rather than aborting.
fn extract_catalog(project: &Path, dest: &Path) -> bool {
    let wrapper_dir = match crate::cmd::catalog_wrapper::generate(project) {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!(
                "[idealyst docs] could not prepare catalog extraction for {} ({e:#}); \
                 documenting the framework catalog instead",
                project.display(),
            );
            return false;
        }
    };

    // Same command `idealyst mcp` runs to reload a project's catalog.
    let output = match Command::new("cargo")
        .current_dir(&wrapper_dir)
        .args(["run", "-q", "--bin", "catalog"])
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            eprintln!("[idealyst docs] failed to launch the catalog extractor ({e}); \
                 documenting the framework catalog instead");
            return false;
        }
    };
    if !output.status.success() {
        eprintln!(
            "[idealyst docs] catalog extraction failed; documenting the framework catalog \
             instead:\n{}",
            String::from_utf8_lossy(&output.stderr).trim(),
        );
        return false;
    }

    // Validate it's the catalog JSON shape before embedding — a parse or
    // missing-`components` failure means we'd bake garbage, so fall back.
    let json = String::from_utf8_lossy(&output.stdout);
    match serde_json::from_str::<serde_json::Value>(&json) {
        Ok(v) if v.get("components").and_then(|c| c.as_array()).is_some() => {}
        _ => {
            eprintln!(
                "[idealyst docs] catalog extractor produced unexpected output; documenting \
                 the framework catalog instead",
            );
            return false;
        }
    }

    if let Err(e) = std::fs::write(dest, json.as_bytes()) {
        eprintln!("[idealyst docs] could not write {} ({e}); documenting the framework \
             catalog instead", dest.display());
        return false;
    }
    true
}

/// Materialize the ephemeral docs-app project crate at `docs_root/app`,
/// returning its directory. It re-exports `docs-app`'s entry points so
/// `build_web` can treat it as an ordinary project (`<lib>::app()` +
/// `<lib>::register_extensions()`). Idempotent — files are only rewritten
/// when their contents change, so repeated `idealyst docs` runs don't
/// invalidate cargo fingerprints.
fn generate_docs_app(docs_root: &Path, source: &FrameworkSource, stage_name: &str) -> Result<PathBuf> {
    let app_dir = docs_root.join("app");
    std::fs::create_dir_all(app_dir.join("src"))
        .with_context(|| format!("create {}", app_dir.join("src").display()))?;

    // `docs-app` sourced the same way the framework resolves (path in
    // workspace mode, git in git mode) so its `runtime_core` / `backend_web`
    // unify with `build_web`'s generated web wrapper.
    let docs_app_dep = source.dep("crates/tools/docs-app", &[]);
    let pkg_name = format!("{stage_name}-docs");

    let cargo_toml = format!(
        r#"# GENERATED by `idealyst docs`. Do not edit — rewritten on demand.
#
# Ephemeral docs-site project. Re-exports the `docs-app` renderer's entry
# points so `idealyst`'s web build can treat it as an ordinary project.
# The catalog it renders is injected at build time via the
# `IDEALYST_DOCS_CATALOG` env var (read by docs-app's build script).
#
# Empty `[workspace]` declares this standalone even though it lives under
# the framework workspace's `target/idealyst/...`.
[workspace]

[package]
name = "{pkg_name}"
version = "0.0.1"
edition = "2021"
publish = false

[dependencies]
docs-app = {docs_app_dep}

[package.metadata.idealyst.app]
name      = "Idealyst Docs"
bundle_id = "io.idealyst.docs_site"
version   = "0.0.1"
targets   = ["web"]
"#,
    );

    let lib_rs = "//! GENERATED by `idealyst docs` — re-exports the docs-app renderer so the\n\
         //! web build mounts it as the project's `app()`. Do not edit.\n\
         pub use docs_app::{app, register_extensions};\n";

    write_if_changed(&app_dir.join("Cargo.toml"), &cargo_toml)?;
    write_if_changed(&app_dir.join("src/lib.rs"), lib_rs)?;
    Ok(app_dir)
}

/// Lower-case + replace any non-alphanumeric run with a single `-`, so a
/// project directory name becomes a valid cargo package-name stem.
fn sanitize_pkg_name(name: &str) -> String {
    let mut s = String::new();
    let mut prev_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            s.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            s.push('-');
            prev_dash = true;
        }
    }
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "project".to_string()
    } else {
        s
    }
}

/// Write `contents` to `path` only if it differs — avoids bumping mtimes
/// (and thus cargo fingerprints) on no-op regenerations. Mirrors the
/// helper in `catalog_wrapper`.
fn write_if_changed(path: &Path, contents: &str) -> Result<()> {
    if let Ok(existing) = std::fs::read_to_string(path) {
        if existing == contents {
            return Ok(());
        }
    }
    std::fs::write(path, contents).with_context(|| format!("write {}", path.display()))
}

/// Best-effort open `url` in the default browser. Never fails the command.
fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let (cmd, args): (&str, Vec<&str>) = ("open", vec![url]);
    #[cfg(all(unix, not(target_os = "macos")))]
    let (cmd, args): (&str, Vec<&str>) = ("xdg-open", vec![url]);
    #[cfg(windows)]
    let (cmd, args): (&str, Vec<&str>) = ("cmd", vec!["/C", "start", "", url]);
    let _ = Command::new(cmd).args(args).status();
}

#[cfg(test)]
mod tests {
    use super::*;
    use build_ios::FrameworkSource;
    use clap::Parser;

    #[derive(Parser)]
    struct TestCli {
        #[command(flatten)]
        args: Args,
    }

    #[test]
    fn defaults_to_cwd_and_standard_port() {
        let cli = TestCli::parse_from(["docs"]);
        assert_eq!(cli.args.project, PathBuf::from("."));
        assert_eq!(cli.args.port, 8300);
        assert_eq!(cli.args.host, "0.0.0.0");
        assert!(!cli.args.release);
        assert!(!cli.args.open);
    }

    #[test]
    fn sanitize_pkg_name_is_cargo_safe() {
        assert_eq!(sanitize_pkg_name("whiteboard-demo"), "whiteboard-demo");
        assert_eq!(sanitize_pkg_name("My App 2"), "my-app-2");
        assert_eq!(sanitize_pkg_name("...weird..."), "weird");
        assert_eq!(sanitize_pkg_name(""), "project");
    }

    #[test]
    fn generate_docs_app_re_exports_and_is_idempotent() {
        let tmp = std::env::temp_dir().join(format!("idealyst-docs-gen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let source = FrameworkSource::Workspace { root: PathBuf::from("/ws") };

        let app_dir = generate_docs_app(&tmp, &source, "demo").expect("generate");
        assert!(app_dir.ends_with("app"));

        let cargo = std::fs::read_to_string(app_dir.join("Cargo.toml")).unwrap();
        // Deps the renderer crate via the workspace path, carries the
        // idealyst.app metadata, and is standalone.
        assert!(cargo.contains("docs-app = { path = \"/ws/crates/tools/docs-app\" }"), "cargo: {cargo}");
        assert!(cargo.contains("[package.metadata.idealyst.app]"), "cargo: {cargo}");
        assert!(cargo.contains("name = \"demo-docs\""), "cargo: {cargo}");
        assert!(cargo.contains("[workspace]"));

        let lib = std::fs::read_to_string(app_dir.join("src/lib.rs")).unwrap();
        // Re-exports both entry points `build_web`'s wrapper calls.
        assert!(lib.contains("pub use docs_app::{app, register_extensions};"), "lib: {lib}");

        // Idempotent: a second identical generate must not rewrite files.
        let mtime1 = std::fs::metadata(app_dir.join("src/lib.rs")).unwrap().modified().unwrap();
        generate_docs_app(&tmp, &source, "demo").expect("regenerate");
        let mtime2 = std::fs::metadata(app_dir.join("src/lib.rs")).unwrap().modified().unwrap();
        assert_eq!(mtime1, mtime2, "idempotent regenerate must not rewrite files");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
