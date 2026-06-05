//! `idealyst mcp` — launch the framework MCP catalog server.
//!
//! Run as a long-lived stdio process — wire it into Claude Desktop /
//! claude.ai/code / any MCP client as their `command`. The server
//! exposes:
//!
//! - The **static catalog** of `#[component]` / `#[idealyst_tool]`
//!   functions, plus framework primitives / utilities / guides.
//!   Sourced live from running apps over their Robot bridge's
//!   `get_catalog` command (discovered via `~/.idealyst/apps/`
//!   registration files), or from a project's catalog binary at
//!   startup when no app is running.
//! - The **Robot tools**: `find_element`, `click`, `type_text`,
//!   `get_snapshot`, and so on. These proxy to the running app's
//!   Robot bridge over TCP, discovered via the same
//!   `~/.idealyst/apps/<name>-<pid>.json` files.
//!
//! Either side degrades gracefully — when no app is running the
//! catalog falls back to the in-process catalog (or `--project-root`
//! extracted catalog), and Robot tools return "no app running."
//!
//! With no `--project-root` / `--from-bin`, the catalog is extracted
//! from the **current directory** — Claude Code launches the server
//! with the project root as cwd via the scaffolded `.mcp.json`, so the
//! bare `idealyst mcp` invocation populates the catalog from the
//! project around it.
//!
//! ```text
//! idealyst mcp                      # catalog (from cwd) + Robot
//! idealyst mcp --no-robot           # catalog-only (e.g. CI doc-gen)
//! idealyst mcp --project-root DIR   # extract catalog from DIR's catalog binary at startup
//! idealyst mcp --check              # lint pass, exit non-zero on findings
//! ```

use anyhow::{Context, Result};
use clap::{Args as ClapArgs, Subcommand};

#[derive(ClapArgs, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Skip the Robot tools, leaving only the catalog. Pass this when
    /// you specifically want a catalog-only server (CI doc-gen, etc.).
    /// Fully disables robot control: no `~/.idealyst/apps/` discovery,
    /// no bridge contact at all, and the Robot tools error out.
    #[arg(long)]
    pub no_robot: bool,

    /// Connect the Robot tools to an explicit bridge port instead of
    /// discovering the running app via `~/.idealyst/apps/`. Use when
    /// the port is known up front (e.g. `idealyst dev` established it).
    /// Ignored when `--no-robot` is set.
    #[arg(long, value_name = "PORT")]
    pub robot_port: Option<u16>,

    /// Host for `--robot-port`. Defaults to 127.0.0.1 (the bridge and
    /// MCP server run on the same machine).
    #[arg(long, value_name = "HOST", default_value = "127.0.0.1")]
    pub robot_host: String,

    /// Lint the catalog and exit non-zero on findings instead of
    /// starting the server. Useful as a CI gate.
    #[arg(long)]
    pub check: bool,

    /// With `--check`: treat an unscoped first-party component as an
    /// error (not a warning), failing the build. Structural scope issues
    /// (cycles, dangling parents) are always errors regardless.
    #[arg(long)]
    pub strict_scopes: bool,

    /// Path to a project directory whose catalog binary should populate
    /// the server's catalog at startup. The CLI looks for
    /// `<dir>/target/debug/catalog` (then `target/release/catalog`) and
    /// invokes it with `--emit-catalog`; if neither exists yet it runs
    /// `cargo run --bin catalog --features mcp -- --emit-catalog` in the
    /// directory to build it. Either way the JSON is piped into the live
    /// catalog. Use this when running the MCP server against a project
    /// that isn't currently running — when an app IS running, the
    /// catalog flows automatically over its Robot bridge.
    ///
    /// **Defaults to the current directory** when neither this flag nor
    /// `--from-bin` is given, so the bare `idealyst mcp` the scaffolded
    /// `.mcp.json` runs populates the catalog from the project cwd.
    #[arg(long, value_name = "DIR")]
    pub project_root: Option<std::path::PathBuf>,

    /// Path to an explicit catalog binary. Bypasses the
    /// `--project-root` lookup. Same emit contract: invoked with
    /// `--emit-catalog`, stdout parsed as the catalog JSON.
    #[arg(long)]
    pub from_bin: Option<std::path::PathBuf>,

    /// Watch source directories and refresh the catalog on change.
    /// Overrides the default watch set (the project's `src/` +
    /// `Cargo.toml`). The watcher re-runs the catalog extractor on
    /// every save, so adding a component or a dependency refreshes the
    /// catalog without restarting the server. Pass once per dir.
    #[arg(long = "watch", value_name = "DIR")]
    pub watch_dirs: Vec<std::path::PathBuf>,

    /// Disable the default catalog file-watch. By default `idealyst mcp`
    /// watches the project's `src/` + `Cargo.toml` and rebuilds the
    /// catalog (via the managed wrapper) on change, so new components /
    /// dependencies appear in a running session. `--no-watch` turns that
    /// off: the catalog is loaded once at startup and a pre-built
    /// `target/{debug,release}/catalog` binary is preferred (lock-free,
    /// no `cargo run` contending with `idealyst dev`'s build lock).
    #[arg(long)]
    pub no_watch: bool,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Write a `.mcp.json` file at the current directory pointing at
    /// `idealyst mcp`. Claude Code auto-loads `.mcp.json` from the
    /// project root, so this is what hooks an existing project into
    /// the framework's MCP server. Idempotent — overwrites any
    /// existing `.mcp.json` with the canonical default contents.
    Install(InstallArgs),
}

#[derive(ClapArgs, Debug)]
pub struct InstallArgs {
    /// Project directory. Defaults to the current directory.
    #[arg(default_value = ".")]
    pub dir: std::path::PathBuf,

    /// Server name in `.mcp.json`. Defaults to `idealyst`. Change
    /// only if you already have another server registered under
    /// that name in the same project.
    #[arg(long, default_value = "idealyst")]
    pub name: String,

    /// Path to the `idealyst` binary the MCP client should spawn.
    /// Defaults to bare `idealyst` (resolved from `$PATH` at launch
    /// time — works after `cargo install idealyst-cli`).
    #[arg(long, default_value = "idealyst")]
    pub command: String,

    /// Overwrite an existing `.mcp.json` without prompting.
    /// Without `--force`, the command refuses if the file already
    /// exists.
    #[arg(long)]
    pub force: bool,
}

pub fn run(args: Args) -> Result<()> {
    if let Some(cmd) = args.command {
        return match cmd {
            Command::Install(install_args) => run_install(install_args),
        };
    }
    if args.check {
        return run_check(args.strict_scopes);
    }

    let mut opts = mcp_server::ServerOptions::new();
    // Robot routing reads `~/.idealyst/apps/<name>-<pid>.json` files
    // the running app's bridge writes on bind. No explicit `--bridge`
    // flag; the CatalogService's discovery thread maintains a live
    // table from that directory and the resolver picks the unique
    // live app, or by `app` arg when multiple are running. Off when
    // `--no-robot` is set.
    if !args.no_robot {
        opts = match args.robot_port {
            Some(port) => opts.with_robot_address(format!("{}:{}", args.robot_host, port)),
            None => opts.with_robot_discovery(),
        };
    }

    // The project root the catalog source and the default watch set are
    // both derived from. With NO flags this is the cwd, because Claude
    // Code launches `idealyst mcp` with the project root as cwd via the
    // scaffolded `.mcp.json` (`{"args": ["mcp"]}`).
    let cwd = std::env::current_dir().ok();
    let project_root = args.project_root.clone().or_else(|| cwd.clone());

    // Whether we'll watch + auto-refresh. Default ON (so adding a
    // component or dependency refreshes the catalog without restarting
    // the server); `--no-watch` opts out. An explicit `--watch DIR`
    // overrides the default watch set but still implies watching.
    let explicit_watch = !args.watch_dirs.is_empty();
    let want_watch = !args.no_watch;

    // Catalog source resolution (see `resolve_catalog_source`). The
    // factory the server preloads — and re-runs on each watch event —
    // must print the project's catalog JSON to stdout.
    //
    // When watching, we force the rebuilding managed wrapper
    // (`prefer_rebuild = true`): a pre-built `catalog` binary can't
    // recompile, so re-running it on a source change would just re-serve
    // a stale snapshot. The managed wrapper rebuilds via `cargo run`, so
    // new components and dependencies actually surface. With `--no-watch`
    // we keep the lock-free pre-built fast-path.
    //
    // `managed` records whether the chosen source rebuilds, so we only
    // default-enable the watcher when refreshing it would do something.
    let mut managed = false;
    match resolve_catalog_source(
        args.from_bin.clone(),
        args.project_root.clone(),
        cwd.clone(),
        want_watch,
    ) {
        CatalogSource::Prebuilt(bin) => {
            let bin = std::sync::Arc::new(bin);
            opts = opts.with_subprocess_catalog(move || {
                let mut c = std::process::Command::new(bin.as_path());
                c.arg("--emit-catalog");
                c
            });
        }
        CatalogSource::Managed(root) => {
            // Generate the catalog wrapper crate now (cheap, idempotent)
            // — this both validates that `root` is a real project and
            // builds the initial wrapper. The subprocess factory then
            // regenerates it on every (re)load: regeneration re-reads the
            // project's `Cargo.toml`, so a dependency added mid-session
            // is force-linked into the wrapper (see `catalog_wrapper`)
            // before the rebuild. `cargo run -q` keeps progress chatter
            // off stdout so the child's stdout stays pure catalog JSON;
            // build diagnostics still go to stderr.
            match super::catalog_wrapper::generate(&root) {
                Ok(wrapper_dir) => {
                    managed = true;
                    let root = std::sync::Arc::new(root);
                    let wrapper_dir = std::sync::Arc::new(wrapper_dir);
                    opts = opts.with_subprocess_catalog(move || {
                        // Idempotent — only rewrites files when their
                        // contents change, so a steady-state reload (no
                        // dep change) doesn't churn cargo fingerprints.
                        if let Err(e) = super::catalog_wrapper::generate(&root) {
                            eprintln!("[idealyst mcp] catalog wrapper regenerate failed: {:#}", e);
                        }
                        let mut c = std::process::Command::new("cargo");
                        c.current_dir(wrapper_dir.as_path());
                        c.args(["run", "-q", "--bin", "catalog"]);
                        c
                    });
                }
                Err(e) => {
                    // Not a parseable project (e.g. cwd isn't an idealyst
                    // project). Leave the catalog to live apps / the
                    // in-process fallback rather than failing startup.
                    eprintln!(
                        "[idealyst mcp] no project catalog available from {}: {:#}",
                        root.display(),
                        e
                    );
                }
            }
        }
        CatalogSource::None => {}
    }

    // Wire the watcher. Explicit `--watch DIR` always wins. Otherwise,
    // when watching is on and the source can rebuild, default to the
    // project's `src/` + `Cargo.toml`. We deliberately do NOT watch the
    // whole project root — `target/` holds the wrapper's own build
    // output, so a recursive watch there would self-trigger forever.
    let watch_paths = if explicit_watch {
        args.watch_dirs.clone()
    } else if want_watch && managed {
        default_watch_paths(project_root.as_deref())
    } else {
        Vec::new()
    };
    if !watch_paths.is_empty() {
        opts = opts.with_watch(watch_paths);
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    rt.block_on(async move {
        mcp_server::run_stdio_with_full_options(opts).await
    })
    .map_err(|e| anyhow::anyhow!("mcp server exited: {:?}", e))
}

/// How the server should obtain the project's catalog JSON, decided
/// purely from the CLI flags + the process cwd. Kept side-effect-free
/// so the decision is unit-testable without spawning anything.
#[derive(Debug, PartialEq, Eq)]
enum CatalogSource {
    /// Execute a prebuilt binary directly: `<path> --emit-catalog`.
    /// Acquires no cargo build lock, so it never contends with a
    /// concurrent `idealyst dev` build. Covers `--from-bin` and a
    /// project that ships its own pre-built `catalog` bin.
    Prebuilt(std::path::PathBuf),
    /// Generate a catalog wrapper crate for the project rooted here and
    /// run it. The project needs no `[[bin]] catalog` and no `mcp`
    /// feature — the wrapper supplies both (see [`super::catalog_wrapper`]).
    Managed(std::path::PathBuf),
    /// No project context at all (no cwd) — leave the catalog to the
    /// live-app bridge / in-process fallback.
    None,
}

/// Decide where the catalog comes from:
///
/// 1. `--from-bin PATH` wins — run that prebuilt binary. The no-cargo
///    escape hatch (CI, doc-gen, sandboxes without a toolchain).
/// 2. Otherwise resolve a project root: `--project-root <dir>` if
///    given, else the current directory. This cwd default is what makes
///    the scaffolded `.mcp.json` (`{"args": ["mcp"]}`) populate the
///    catalog — Claude Code launches the server with the project root
///    as cwd. Within that root, the CLI generates + runs a managed
///    wrapper.
///
/// `prefer_rebuild` is set when the server will watch + auto-refresh.
/// A pre-built `target/{debug,release}/catalog` binary can't recompile,
/// so re-running it on a source change re-serves a stale snapshot —
/// when watching, we therefore skip that fast-path and always use the
/// managed wrapper (which rebuilds via `cargo run`). With watching off
/// (`--no-watch`) we keep the lock-free pre-built fast-path: it never
/// takes a cargo build lock, so it can't contend with `idealyst dev`.
fn resolve_catalog_source(
    from_bin: Option<std::path::PathBuf>,
    project_root: Option<std::path::PathBuf>,
    cwd: Option<std::path::PathBuf>,
    prefer_rebuild: bool,
) -> CatalogSource {
    if let Some(bin) = from_bin {
        return CatalogSource::Prebuilt(bin);
    }
    let Some(root) = project_root.or(cwd) else {
        return CatalogSource::None;
    };
    if !prefer_rebuild {
        if let Some(bin) = find_catalog_binary(&root) {
            return CatalogSource::Prebuilt(bin);
        }
    }
    CatalogSource::Managed(root)
}

/// The default set of paths to watch when the user didn't pass an
/// explicit `--watch`: the project's `src/` directory and its
/// `Cargo.toml`. A source edit refreshes new/changed components; a
/// `Cargo.toml` edit refreshes added/removed dependencies.
///
/// Crucially this does NOT include the project root itself: the managed
/// catalog wrapper writes its build output under `<root>/target/...`, so
/// a recursive watch on the root would observe the wrapper's own rebuild
/// and re-trigger endlessly.
fn default_watch_paths(project_root: Option<&std::path::Path>) -> Vec<std::path::PathBuf> {
    let Some(root) = project_root else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    let src = root.join("src");
    if src.is_dir() {
        paths.push(src);
    }
    let cargo = root.join("Cargo.toml");
    if cargo.is_file() {
        paths.push(cargo);
    }
    paths
}

/// Resolve a project's catalog binary by looking at the scaffolded
/// `[[bin]] name = "catalog"` output path. Prefers `target/debug` over
/// `target/release` since dev is the typical flow; release is checked
/// as a fallback in case the user pre-built for production.
fn find_catalog_binary(project_root: &std::path::Path) -> Option<std::path::PathBuf> {
    for profile in ["debug", "release"] {
        let candidate = project_root.join("target").join(profile).join("catalog");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn run_install(args: InstallArgs) -> Result<()> {
    let target = args.dir.join(".mcp.json");
    if target.exists() && !args.force {
        anyhow::bail!(
            "{} already exists. Pass --force to overwrite.",
            target.display()
        );
    }

    let body = serde_json::json!({
        "mcpServers": {
            args.name.clone(): {
                "command": args.command,
                "args": ["mcp"],
            }
        }
    });
    let pretty = serde_json::to_string_pretty(&body)? + "\n";
    std::fs::write(&target, pretty)
        .with_context(|| format!("write {}", target.display()))?;

    eprintln!("[idealyst mcp install] wrote {}", target.display());
    eprintln!(
        "[idealyst mcp install] Server name: {}, command: {}",
        args.name, args.command
    );
    eprintln!(
        "[idealyst mcp install] Run `idealyst dev` to launch the app; the bridge \
         port is written to .idealyst/bridge.port and the MCP server discovers \
         it from cwd."
    );
    Ok(())
}

fn run_check(strict_scopes: bool) -> Result<()> {
    let cat = mcp_catalog::ResolvedCatalog::build();
    let opts = mcp_server::LintOptions {
        strict_scopes,
        // `--check` runs against the project's own catalog; treat every
        // entry as first-party. (A dep-aware list could be passed here.)
        first_party_crates: Vec::new(),
    };
    let findings = mcp_server::lint_catalog_with(&cat, &opts);
    if findings.is_empty() {
        println!(
            "OK — {} components, no catalog-integrity issues",
            cat.entries().len()
        );
        return Ok(());
    }
    let mut errors = 0usize;
    for f in &findings {
        let tag = match f.severity {
            mcp_server::Severity::Warning => "warn",
            mcp_server::Severity::Error => {
                errors += 1;
                "error"
            }
        };
        println!("[{}] {} — {}", tag, f.fqn, f.message);
    }
    println!("\n{} findings ({} errors)", findings.len(), errors);
    // Warnings inform but don't fail the build (the lint is adopted
    // incrementally); only errors gate CI. `--strict-scopes` promotes
    // unscoped-component findings to errors.
    if errors > 0 {
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn tmp_dir(tag: &str) -> PathBuf {
        // No `tempfile` dep in the CLI crate; derive a unique-enough dir
        // from the test's tag plus this process's id. Tests create
        // disjoint tags, so no collision.
        let dir = std::env::temp_dir().join(format!(
            "idealyst-mcp-test-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_catalog_bin(root: &std::path::Path, profile: &str) -> PathBuf {
        let bin_dir = root.join("target").join(profile);
        fs::create_dir_all(&bin_dir).unwrap();
        let bin = bin_dir.join("catalog");
        fs::write(&bin, b"#!/bin/sh\n").unwrap();
        bin
    }

    /// Regression: `idealyst mcp` with NO flags must default to the
    /// current directory and wire a catalog subprocess. Before the fix
    /// the resolver returned `None` here, so `list_components` served
    /// the empty in-process catalog — the "no components appear" bug
    /// reported against scaffolded projects (whose `.mcp.json` invokes
    /// the server as `{"args": ["mcp"]}`, i.e. no `--project-root`).
    #[test]
    fn no_flags_defaults_to_cwd_not_none() {
        let cwd = tmp_dir("cwd-default");
        // Fresh project: no prebuilt binary yet → managed wrapper.
        let src = resolve_catalog_source(None, None, Some(cwd.clone()), false);
        assert_eq!(src, CatalogSource::Managed(cwd));

        // The pre-fix behavior we must never regress to:
        assert_ne!(
            resolve_catalog_source(None, None, Some(std::env::temp_dir()), false),
            CatalogSource::None,
            "no-flag invocation must not yield an empty catalog source \
             when a cwd is available"
        );
    }

    #[test]
    fn prebuilt_binary_preferred_over_managed_wrapper_when_not_watching() {
        // With `prefer_rebuild = false` (i.e. `--no-watch`), the
        // lock-free pre-built fast-path applies.
        let root = tmp_dir("prebuilt");
        let bin = write_catalog_bin(&root, "debug");
        // cwd default
        assert_eq!(
            resolve_catalog_source(None, None, Some(root.clone()), false),
            CatalogSource::Prebuilt(bin.clone())
        );
        // explicit --project-root
        assert_eq!(
            resolve_catalog_source(None, Some(root.clone()), None, false),
            CatalogSource::Prebuilt(bin)
        );
    }

    #[test]
    fn watching_forces_managed_wrapper_over_prebuilt_binary() {
        // Regression: with the watcher on (the default), a stale
        // pre-built `catalog` binary must NOT be preferred — re-running
        // it on a source change would re-serve the same frozen catalog,
        // defeating auto-refresh. `prefer_rebuild = true` forces the
        // managed wrapper so reloads recompile and pick up new
        // components / dependencies.
        let root = tmp_dir("watch-forces-managed");
        write_catalog_bin(&root, "debug");
        assert_eq!(
            resolve_catalog_source(None, None, Some(root.clone()), true),
            CatalogSource::Managed(root)
        );
    }

    #[test]
    fn release_binary_used_when_no_debug() {
        let root = tmp_dir("release-only");
        let bin = write_catalog_bin(&root, "release");
        assert_eq!(
            resolve_catalog_source(None, Some(root), None, false),
            CatalogSource::Prebuilt(bin)
        );
    }

    #[test]
    fn from_bin_wins_over_everything() {
        let root = tmp_dir("from-bin");
        // Even with a prebuilt under the project root and a cwd, an
        // explicit --from-bin takes precedence — and even when watching.
        write_catalog_bin(&root, "debug");
        let explicit = root.join("custom-catalog");
        assert_eq!(
            resolve_catalog_source(
                Some(explicit.clone()),
                Some(root.clone()),
                Some(root.clone()),
                false,
            ),
            CatalogSource::Prebuilt(explicit.clone())
        );
        assert_eq!(
            resolve_catalog_source(Some(explicit.clone()), Some(root.clone()), Some(root), true),
            CatalogSource::Prebuilt(explicit)
        );
    }

    #[test]
    fn project_root_with_no_binary_uses_managed_wrapper() {
        let root = tmp_dir("no-bin");
        assert_eq!(
            resolve_catalog_source(None, Some(root.clone()), None, false),
            CatalogSource::Managed(root)
        );
    }

    #[test]
    fn no_cwd_and_no_flags_yields_none() {
        // Only when there is genuinely no project context (no cwd, no
        // flags) do we leave the catalog to live apps / in-process.
        assert_eq!(
            resolve_catalog_source(None, None, None, false),
            CatalogSource::None
        );
        assert_eq!(
            resolve_catalog_source(None, None, None, true),
            CatalogSource::None
        );
    }

    #[test]
    fn default_watch_paths_are_src_and_cargo_toml_only() {
        // The watch set must be src/ + Cargo.toml — never the project
        // root, whose target/ holds the wrapper's own build output and
        // would cause the watcher to self-trigger forever.
        let root = tmp_dir("watch-paths");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("Cargo.toml"), b"[package]\n").unwrap();
        // A target/ dir exists, as it would after a build — it must not
        // be watched.
        fs::create_dir_all(root.join("target").join("idealyst")).unwrap();

        let paths = default_watch_paths(Some(root.as_path()));
        assert_eq!(paths, vec![root.join("src"), root.join("Cargo.toml")]);
        assert!(
            !paths.iter().any(|p| p.ends_with("target")),
            "target/ must never be in the default watch set: {paths:?}"
        );
    }

    #[test]
    fn default_watch_paths_skip_missing_entries() {
        // Only existing paths are watched; a project missing src/ (or
        // not yet written) shouldn't hand notify a non-existent path.
        let root = tmp_dir("watch-paths-missing");
        fs::write(root.join("Cargo.toml"), b"[package]\n").unwrap();
        let paths = default_watch_paths(Some(root.as_path()));
        assert_eq!(paths, vec![root.join("Cargo.toml")]);

        assert!(default_watch_paths(None).is_empty());
    }
}
