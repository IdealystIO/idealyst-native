//! Linux GTK4-host build orchestration for `idealyst build --linux`.
//!
//! Same shape as `build-terminal` / `build-macos`: generate a tiny
//! binary wrapper that depends on `host-gtk` + the user crate and
//! mounts the user's `app()` in-process via `host_gtk::run_with`.
//! Cargo-build the wrapper and return the produced binary's path.
//!
//! Wrapper layout: `<wrapper_root>/<project>/linux/`.
//!
//! Only [`BuildMode::Local`] exists today — the GTK host has no
//! runtime-server (dev-host streaming) variant yet. A `RuntimeServer`
//! mode can be added here (mirroring `build-terminal`) once
//! `host_gtk::run_runtime_server` lands.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use build_ios::{parse_manifest, FrameworkSource, Manifest};

/// Which wrapper to generate. Only `Local` is implemented today
/// (mounts the user crate's `app()` in-process); the enum exists so
/// the CLI plumbing matches the other native targets and a
/// runtime-server variant can slot in later.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildMode {
    Local,
}

#[derive(Clone, Debug)]
pub struct BuildOptions {
    /// Compile with `--release`. Default: debug.
    pub release: bool,
    /// Selects the wrapper template. Only [`BuildMode::Local`] today.
    pub mode: BuildMode,
    /// Cargo features to enable on the build. `idealyst dev` passes the
    /// wrapper-local `dev` feature so the Robot bridge auto-starts.
    pub user_features: Vec<String>,
    /// Framework-source resolution for the wrapper crate's deps.
    pub source: FrameworkSource,
}

#[derive(Debug)]
pub struct BuildArtifact {
    pub binary: PathBuf,
    pub wrapper_dir: PathBuf,
}

/// Build the Linux wrapper for `project_dir` with `opts`.
pub fn build(project_dir: &Path, opts: BuildOptions) -> Result<BuildArtifact> {
    let project_dir = fs::canonicalize(project_dir)
        .with_context(|| format!("resolve project dir {}", project_dir.display()))?;
    let manifest = parse_manifest(&project_dir)?;

    let wrapper_root = opts.source.wrapper_root(&project_dir);
    let wrapper_dir = wrapper_root.join(&manifest.name).join("linux");
    // The wrapper builds into its OWN target directory, not the project's.
    //
    // It used to share the project's `target/` to avoid recompiling common
    // dependencies. That is unsound here: the wrapper is a SEPARATE workspace
    // (its own `[workspace]`, its own lockfile) that depends on the app crate
    // by path, so it resolves the app and every framework crate under its own
    // feature set and its own dependency resolution — then writes those
    // artifacts next to the project workspace's. The next plain
    // `cargo test`/`cargo build` in the project then links a MIXTURE, and
    // rustc reports "multiple different versions of crate `runtime_scene`"
    // against a graph that contains exactly one, pointing at the same source
    // file as both the expected and the found type.
    //
    // It is a nasty failure because it is delayed and misattributed: the
    // wrapper build succeeds, and the breakage lands on the NEXT unrelated
    // build of the project, which looks like the app is broken. Running a
    // parity sweep poisoned every later sweep this way, each needing a manual
    // `cargo clean -p runtime-scene -p idealyst` to recover.
    //
    // A private target dir costs a one-time recompile of the shared deps and
    // makes the two workspaces incapable of corrupting each other.
    let cargo_target_dir = wrapper_target_dir(&wrapper_dir);

    generate_wrapper(&wrapper_dir, &cargo_target_dir, &project_dir, &manifest, &opts)?;
    cargo_build(&wrapper_dir, opts.release, &opts.user_features)?;

    let profile = if opts.release { "release" } else { "debug" };
    let bin_name = binary_name(&manifest.name);
    let binary = cargo_target_dir.join(profile).join(&bin_name);
    if !binary.is_file() {
        anyhow::bail!(
            "cargo build reported success but linux binary not at {}",
            binary.display(),
        );
    }
    Ok(BuildArtifact {
        binary,
        wrapper_dir,
    })
}

/// Produced-binary name. Suffixed with `-linux` so it coexists with
/// the user crate's lib/bin and the other targets' wrappers.
fn binary_name(project_name: &str) -> String {
    format!("{project_name}-linux")
}

fn generate_wrapper(
    wrapper_dir: &Path,
    cargo_target_dir: &Path,
    project_dir: &Path,
    manifest: &Manifest,
    _opts: &BuildOptions,
) -> Result<()> {
    fs::create_dir_all(wrapper_dir.join("src"))
        .with_context(|| format!("create {}", wrapper_dir.display()))?;

    let bin_name = binary_name(&manifest.name);
    let host_dep = _opts.source.dep("crates/host/gtk", &[]);
    // `runtime-core` as a direct dep so `idealyst dev` can activate the
    // wrapper-local `dev` feature (→ `runtime-core/dev`) and reach
    // `__mcp::catalog_json()` for `idealyst mcp`.
    let fcore_dep = _opts.source.dep("crates/runtime/core", &[]);
    // `runtime-shared` as a DIRECT dep too: it carries the Robot bridge
    // TRANSPORT (`robot::bridge`), which the identity stamp below calls.
    // `runtime-core` only re-exports the author surface, so reaching the
    // bridge through it does not resolve. Mirrors the macOS wrapper.
    let fshared_dep = _opts.source.dep("crates/runtime/shared", &[]);
    let user_dep = format!("{{ path = \"{}\" }}", project_dir.display());
    let bundle_id = manifest
        .app
        .bundle_id
        .clone()
        .unwrap_or_else(|| format!("com.example.{}", manifest.name));

    let deps_block = format!(
        "host-gtk = {host_dep}\n\
         runtime-core = {fcore_dep}\n\
         runtime-shared = {fshared_dep}\n\
         {user_name} = {user_dep}\n",
        user_name = manifest.name,
    );
    // `dev` turns on the robot REGISTRY (`runtime-core/dev`) AND the bridge
    // TRANSPORT (`runtime-shared/robot`) — the latter is what makes
    // `robot::bridge::set_app_identity` below resolve.
    // `dev` turns on THREE things, and all three are load-bearing:
    //   runtime-core/dev      the robot element REGISTRY + catalog emission
    //   runtime-shared/robot  the bridge TRANSPORT (`robot::bridge` resolves)
    //   host-gtk/robot        the boot-time `install_robot_env`, which points
    //                         the bridge at the registry this core fills
    // Without the third the bridge binds, answers, and reports an EMPTY app.
    let features_block = "[features]\ndev = [\"runtime-core/dev\",          \"runtime-shared/robot\", \"host-gtk/robot\"]\n"
        .to_string();
    let main_rs = local_main_rs(
        &manifest.lib_name,
        &manifest.name,
        &bundle_id,
        &project_dir.display().to_string(),
    );

    let cargo_toml = format!(
        r#"# GENERATED by `idealyst build --linux` (local-mount). Do not edit — rewritten every build.
#
# GTK4 wrapper. Depends on `host-gtk` + the user crate, mounts `app()`
# in-process into a `gtk::ApplicationWindow`. Produces a desktop binary
# at `<target>/<profile>/{bin_name}`.

[workspace]

[package]
name = "{bin_name}"
version = "0.0.1"
edition = "2021"

[dependencies]
{deps_block}
{features_block}"#,
    );

    write_shared_target_config(wrapper_dir, cargo_target_dir)?;
    fs::write(wrapper_dir.join("Cargo.toml"), cargo_toml)?;
    fs::write(wrapper_dir.join("src/main.rs"), main_rs)?;
    refresh_wrapper_lockfile(wrapper_dir)?;
    Ok(())
}

/// Delete the generated wrapper's `Cargo.lock` so the next build re-resolves.
///
/// # The trap this closes
///
/// The wrapper declares its own `[workspace]`, so it keeps its own lockfile —
/// and that lockfile goes stale whenever the USER crate's dependencies change,
/// which never touches the wrapper's own `Cargo.toml` and so triggers no
/// regeneration. Cargo then resolves the framework path crates TWICE and the
/// build fails with:
///
/// ```text
/// error[E0271]: expected `app` to return `Element`, but it returns `Element`
/// note: there are multiple different versions of crate `runtime_scene`
/// ```
///
/// Two identically-named types and no mention of a lockfile — an hour of
/// confusion for a one-line cause. Hit live: adding an `anyhow`
/// dev-dependency to `websites/idea-ui-docs` broke `idealyst dev --linux` until
/// this file was removed by hand.
///
/// # Why delete rather than copy the workspace's lock
///
/// Seeding the wrapper from `<workspace>/Cargo.lock` looks tidier — same
/// versions on both sides — and was tried first. It made things WORSE: the
/// wrapper shares the workspace's `target/` (see `write_shared_target_config`),
/// and once both resolutions are identical the two builds' units differ only in
/// whether the path deps are spelled relatively (workspace build, cwd = repo
/// root) or absolutely (wrapper build, cwd = wrapper dir). Cargo then mixed the
/// two, and the WORKSPACE build started failing with the same duplicate-crate
/// error inside `idealyst::entry!`. Letting the wrapper resolve independently
/// keeps its units distinct.
///
/// The wrapper is regenerated on every build, so its lock has no reproducibility
/// role of its own to protect; the workspace's lock still governs the versions
/// of everything either build actually shares.
fn refresh_wrapper_lockfile(wrapper_dir: &Path) -> Result<()> {
    let lock = wrapper_dir.join("Cargo.lock");
    if lock.exists() {
        fs::remove_file(&lock)
            .with_context(|| format!("removing stale {}", lock.display()))?;
    }
    Ok(())
}

fn local_main_rs(
    user_lib: &str,
    app_name: &str,
    bundle_id: &str,
    project_root: &str,
) -> String {
    format!(
        r#"//! GENERATED by `idealyst build --linux` (local-mount). Wrapper
//! binary for the GTK4-backed native Linux runtime.

use {user_lib}::app;

fn main() {{
    // `--emit-catalog`: dump the MCP catalog JSON and exit without
    // opening a window. `idealyst mcp` (via `--from-bin`) spawns this
    // to extract the project's catalog. Only in `dev` builds (the
    // `mcp` surface on `runtime-core` is pulled by `dev`).
    #[cfg(feature = "dev")]
    {{
        if std::env::args().any(|a| a == "--emit-catalog") {{
            let json = ::runtime_core::__mcp::catalog_json();
            println!("{{}}", ::runtime_core::__serde_json::to_string_pretty(&json).unwrap());
            return;
        }}
    }}

    // Register the project's identity for the Robot bridge's per-
    // process registration file, then START the bridge. No-op when `dev`
    // is off.
    //
    // Stamping the identity is not enough on its own — that only decides
    // what the registration file WILL say. Every other platform gets its
    // listener from the dev-server sidecar, which calls
    // `start_auto_polling`; Linux has no runtime-server variant (dev
    // --linux is local-mount only), so without this the app came up with
    // the robot registry compiled in, no listener bound, and no
    // registration file — so `list_apps` never saw it, the MCP robot tools
    // could not reach it, and `idealyst test --parity web,linux` sat
    // waiting for a registration that was never coming.
    //
    // Deferred to `on_main_loop_start` because the bridge's poll is
    // scheduler-driven (`schedule_periodic_poll` bails when no scheduler
    // is installed) and `run_with` installs the GTK scheduler on the way
    // into the loop. Calling it here directly would bind the socket and
    // then never poll it.
    #[cfg(feature = "dev")]
    {{
        ::runtime_shared::robot::bridge::set_app_identity(
            ::runtime_shared::robot::bridge::AppIdentity {{
                name: "{app_name}".to_string(),
                bundle_id: Some("{bundle_id}".to_string()),
                // The absolute project root, baked in at generation time.
                // Leaving this `None` wrote a registration with a null root,
                // and every PROJECT-SCOPED discovery filters on it — so
                // `idealyst test --parity` could not attribute the launch it
                // had just made and waited out its timeout, and `idealyst mcp`
                // in a project directory could not find the app either.
                project_root: Some("{project_root}".to_string()),
            }},
        );
        host_gtk::on_main_loop_start(|| {{
            // A relay URL means "dial out to a host-side relay" (container
            // / CI, where nothing can reach into this process); otherwise
            // bind a local listener. Mirrors the sidecar's own choice.
            match ::runtime_shared::robot::bridge::relay_url_from_env() {{
                Some(url) => ::runtime_shared::robot::bridge::start_relay_client(url),
                None => ::runtime_shared::robot::bridge::start_auto_polling(
                    ::runtime_shared::robot::bridge::DEFAULT_PORT,
                ),
            }}
        }});
    }}

    let opts = host_gtk::RunOptions {{
        title: "{app_name}".to_string(),
        width: 1024,
        height: 768,
    }};
    // `run_with` (not `run`) so the user crate's
    // `pub fn register_scene_extensions(&mut Registry<_>)` seam runs after
    // `register_builtins` — this is how SDK payload handlers (codeblock,
    // table, svg, …) get into the scene registry. Mirrors the macOS
    // template; `register_extensions` was the pre-v2 name and took the
    // backend, which no longer has an External table to register into.
    let code = host_gtk::run_with(opts, {user_lib}::register_scene_extensions, app);
    std::process::exit(code);
}}
"#,
    )
}

/// Where the wrapper's own build artifacts go.
///
/// Private to the wrapper by construction — see the rationale in [`build`].
/// Kept as a function so the invariant ("never the project's target dir") is
/// testable without running a real cargo build.
fn wrapper_target_dir(wrapper_dir: &Path) -> PathBuf {
    wrapper_dir.join("target")
}

fn write_shared_target_config(dir: &Path, target_dir: &Path) -> Result<()> {
    let config = format!(
        "# GENERATED. A target dir PRIVATE to this wrapper.\n\
         # The wrapper is a separate workspace with its own lockfile and\n\
         # feature resolution; sharing the project's `target/` lets the two\n\
         # write incompatible copies of the same crates into one directory,\n\
         # and the project's next build links a mixture (\"multiple different\n\
         # versions of crate `runtime_scene`\"). Do not point this back at\n\
         # the project's target dir.\n\
         \n\
         [build]\n\
         target-dir = \"{}\"\n",
        target_dir.display(),
    );
    // The wrapper Cargo.toml carries a `[patch.<registry>]` section, and an
    // undefined registry name there is a hard error. Define it here rather
    // than relying on an ancestor config having done so.
    let config = config + &build_ios::registry_config_block();
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
    if !user_features.is_empty() {
        cmd.arg("--features").arg(user_features.join(","));
    }
    eprintln!(
        "[build-linux] cargo build{}{} (in {})",
        if release { " --release" } else { "" },
        if user_features.is_empty() {
            String::new()
        } else {
            format!(" --features {}", user_features.join(","))
        },
        wrapper_dir.display(),
    );
    let status = cmd
        .status()
        .with_context(|| "spawn `cargo` — is it on your PATH?")?;
    if !status.success() {
        anyhow::bail!("[build-linux] cargo build exited with {status}");
    }
    Ok(())
}

#[cfg(test)]
mod regression_tests {
    //! Wrapper-shape regression tests for `build-linux`. Generates the
    //! wrapper for a synthetic manifest and asserts on the produced
    //! files — no cargo build fires.

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
                worker_bin: None,
                worker_manifest: None,
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
        let cargo_target = tmp.path().join("target");
        let workspace_root = tmp.path().join("workspace");
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::create_dir_all(&workspace_root).unwrap();
        let manifest = fake_manifest();
        let opts = BuildOptions {
            release: false,
            mode: BuildMode::Local,
            user_features: Vec::new(),
            source: FrameworkSource::Workspace {
                root: workspace_root,
            },
        };
        generate_wrapper(&wrapper_dir, &cargo_target, &project_dir, &manifest, &opts)
            .expect("generate wrapper");
        (wrapper_dir, tmp)
    }

    /// The generated `main.rs` must call `host_gtk::run_with` with the
    /// user crate's `register_scene_extensions` so SDK payload handlers
    /// reach the scene registry (mirrors the macOS wrapper).
    ///
    /// Regression: this emitted the pre-v2 `register_extensions`, which
    /// takes `&mut LinuxBackend`. Apps ported to v2 only expose the
    /// registry-generic seam, so `idealyst dev --linux` failed to compile
    /// the generated wrapper — rustc even suggested the right name.
    #[test]
    fn wrapper_calls_run_with_and_register_scene_extensions() {
        let (wrapper_dir, _tmp) = run_generator();
        let main_rs =
            std::fs::read_to_string(wrapper_dir.join("src/main.rs")).expect("read main.rs");
        assert!(
            main_rs.contains("host_gtk::run_with(opts, demo::register_scene_extensions, app)"),
            "wrapper must mount via run_with + register_scene_extensions; got:\n{main_rs}",
        );
    }

    /// The wrapper's `dev` feature must forward to `runtime-core/dev`,
    /// or `idealyst mcp` against a `dev --linux` session returns an
    /// empty catalog (same bug class the macOS/terminal wrappers guard).
    #[test]
    fn dev_feature_pulls_runtime_core_dev() {
        let (wrapper_dir, _tmp) = run_generator();
        let cargo =
            std::fs::read_to_string(wrapper_dir.join("Cargo.toml")).expect("read Cargo.toml");
        assert!(
            cargo.contains("runtime-core/dev") && cargo.contains("runtime-shared/robot"),
            "linux wrapper's `dev` must enable BOTH the robot registry \
             (runtime-core/dev) and the bridge transport (runtime-shared/robot) — \
             the identity stamp in main.rs calls `runtime_shared::robot::bridge`; \
             got:\n{cargo}",
        );
        // The third piece: without it the bridge binds and then answers every
        // verb from the OLD, empty registry — `find_element` → null and
        // `get_snapshot` → [] on a healthy app, which blinds a driver silently
        // instead of failing.
        assert!(
            cargo.contains("host-gtk/robot"),
            "linux wrapper's `dev` must also enable `host-gtk/robot`, which \
             compiles the boot-time `install_robot_env` that points the bridge \
             at the registry this core actually fills; got:\n{cargo}",
        );
    }

    /// The wrapper's stale `Cargo.lock` must be REMOVED on regeneration.
    ///
    /// It goes stale whenever the user crate's dependencies change — which never
    /// touches the wrapper's own manifest — and cargo then resolves the framework
    /// path crates twice, failing with "expected `Element`, found `Element` /
    /// multiple different versions of crate `runtime_scene`": a message that
    /// names neither the lockfile nor the real cause.
    #[test]
    fn a_stale_wrapper_lockfile_is_removed_on_regeneration() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project_dir = tmp.path().join("project");
        let wrapper_dir = tmp.path().join("wrapper");
        let workspace_root = tmp.path().join("workspace");
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::create_dir_all(&workspace_root).unwrap();
        std::fs::create_dir_all(&wrapper_dir).unwrap();
        std::fs::write(wrapper_dir.join("Cargo.lock"), "stale").unwrap();

        let opts = BuildOptions {
            release: false,
            mode: BuildMode::Local,
            user_features: Vec::new(),
            source: FrameworkSource::Workspace { root: workspace_root },
        };
        generate_wrapper(
            &wrapper_dir,
            &tmp.path().join("target"),
            &project_dir,
            &fake_manifest(),
            &opts,
        )
        .expect("generate wrapper");

        assert!(
            !wrapper_dir.join("Cargo.lock").exists(),
            "a stale wrapper lock must be deleted so the next build re-resolves",
        );
    }

    /// The wrapper must START the bridge, not merely stamp the identity.
    ///
    /// Every other platform's listener comes from the dev-server sidecar
    /// (`start_auto_polling`); Linux has no runtime-server variant, so a wrapper
    /// that only calls `set_app_identity` produces an app with the robot
    /// registry compiled in, nothing listening, and no `~/.idealyst/apps/`
    /// registration — invisible to `list_apps`, unreachable by the MCP robot
    /// tools, and a hang for `idealyst test --parity web,linux`, which waits for
    /// that registration.
    #[test]
    fn dev_builds_start_the_robot_bridge() {
        let (wrapper_dir, _tmp) = run_generator();
        let main_rs =
            std::fs::read_to_string(wrapper_dir.join("src/main.rs")).expect("read main.rs");
        assert!(
            main_rs.contains("start_auto_polling"),
            "the wrapper must bind a bridge listener, not just stamp an \
             identity; got:\n{main_rs}",
        );
        assert!(
            main_rs.contains("start_relay_client"),
            "a relay URL must be honored too, for hosts that cannot be reached \
             into (container / CI); got:\n{main_rs}",
        );
        assert!(
            main_rs.contains("project_root: Some("),
            "the registration must carry the project root — every project-scoped \
             discovery filters on it, so a null root makes the app invisible to \
             `idealyst test --parity` and to a project-anchored MCP server; \
             got:\n{main_rs}",
        );
        assert!(
            main_rs.contains("on_main_loop_start"),
            "the bridge start must be deferred until the GTK scheduler exists — \
             `schedule_periodic_poll` bails without one, so an immediate call \
             binds a socket that is never polled; got:\n{main_rs}",
        );
    }

    /// The wrapper must never build into the project's `target/`.
    ///
    /// Regression: it did, to reuse compiled dependencies. But the wrapper is
    /// a separate workspace with its own lockfile and feature resolution, so
    /// the two wrote incompatible copies of the same crates into one
    /// directory. The wrapper build still succeeded; the NEXT plain
    /// `cargo build`/`cargo test` in the project failed with "multiple
    /// different versions of crate `runtime_scene`" naming one source file as
    /// both the expected and the found type. Delayed and misattributed —
    /// it read as the app being broken, and each recovery needed a manual
    /// `cargo clean`. A parity sweep poisoned every sweep after it.
    #[test]
    fn regression_wrapper_does_not_build_into_the_projects_target_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project_dir = tmp.path().join("project");
        let workspace_root = tmp.path().join("workspace");
        let wrapper_dir = tmp.path().join("wrapper");
        let source = FrameworkSource::Workspace {
            root: workspace_root,
        };

        let chosen = wrapper_target_dir(&wrapper_dir);
        assert!(
            chosen.starts_with(&wrapper_dir),
            "the wrapper's target dir must live under the wrapper ({}), got {}",
            wrapper_dir.display(),
            chosen.display(),
        );
        assert_ne!(
            chosen,
            source.cargo_target_dir(&project_dir),
            "the wrapper must not share the project's target dir — two workspaces \
             writing one target dir is what produced the duplicate-crate failure"
        );

        // And the generated cargo config must actually point there, since that
        // file is what cargo obeys.
        std::fs::create_dir_all(&wrapper_dir).unwrap();
        write_shared_target_config(&wrapper_dir, &chosen).expect("write config");
        let cfg = std::fs::read_to_string(wrapper_dir.join(".cargo/config.toml")).unwrap();
        assert!(
            cfg.contains(&format!("target-dir = \"{}\"", chosen.display())),
            "generated cargo config does not point at the private target dir:\n{cfg}",
        );
    }

}
