//! runtime-server dev-host build orchestration for `idealyst build aas`.
//!
//! runtime-server (Application-as-a-Server) runs the user's reactive runtime on
//! a dev-host process and lets browsers / native shells connect as
//! thin clients that ship primitive commands over a WebSocket.
//!
//! ## Split-process architecture
//!
//! The runtime-server dev host is **two** binaries, generated side by side under
//! `<workspace>/target/idealyst/<project>/aas/`:
//!
//! - `host/`   → `<project>-runtime-server-host`  — long-lived infra (WebSocket
//!                                       server, mDNS, file watcher).
//!                                       Statically links `dev-server`
//!                                       but NOT the user crate.
//! - `app/`    → `<project>-runtime-server-app`   — short-lived sidecar that
//!                                       statically links the user
//!                                       crate, runs `render(app())`,
//!                                       and streams wire commands /
//!                                       reads events over its
//!                                       stdout / stdin pipes.
//!
//! On file change the host rebuilds the **sidecar** (NOT itself),
//! SIGKILLs the running sidecar, and respawns. The WebSocket listener
//! stays up the entire time — connected clients (Android, iOS) never
//! disconnect, so the perceived hot-reload latency drops to roughly
//! the user-crate rebuild + sidecar startup time. This is the win
//! over the legacy single-process model where every save did a
//! self-exec and forced every client to reconnect.
//!
//! ## Layout
//!
//! ```text
//! <workspace>/target/idealyst/<project>/aas/
//! ├── host/
//! │   ├── Cargo.toml
//! │   ├── .cargo/config.toml      ← share workspace target dir
//! │   └── src/main.rs             ← spawns sidecar + serves
//! └── app/
//!     ├── Cargo.toml
//!     ├── .cargo/config.toml      ← share workspace target dir
//!     └── src/main.rs             ← runs render(app()) + frame I/O
//! ```
//!
//! Both crates are regenerated on every build. `idealyst scaffold
//! aas` (future) will materialize editable copies into the repo for
//! users who want to customize either side.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use build_ios::{parse_manifest, FrameworkSource, Manifest};

pub mod hotpatch;

#[derive(Clone, Debug)]
pub struct BuildOptions {
    /// Compile with `--release`. Default: debug. The host and sidecar
    /// both run locally — release is almost never worth the slower
    /// rebuild cycle here.
    pub release: bool,
    /// Where the generated wrapper crates should source framework
    /// deps from. The CLI constructs this via
    /// `FrameworkSource::detect(project_dir, git_defaults)` so a
    /// project using `runtime-core = { git = "…", rev = "…" }`
    /// gets a wrapper that uses the same git ref, and a project
    /// with a path-dep gets a wrapper that uses the same path — no
    /// local-checkout-alongside-the-project assumption.
    pub source: FrameworkSource,
}

#[derive(Debug)]
pub struct BuildArtifact {
    /// Path to the produced infra-host executable.
    pub host_binary: PathBuf,
    /// Path to the produced sidecar executable.
    pub sidecar_binary: PathBuf,
    /// Host wrapper crate directory.
    pub wrapper_dir: PathBuf,
    /// Sidecar wrapper crate directory.
    pub sidecar_dir: PathBuf,
}

/// Bind address the generated host uses unless overridden via its
/// first CLI arg. `0.0.0.0:0` means "OS picks a free port and listen
/// on every interface" — the chosen port is published over mDNS so
/// clients (iOS, Android, …) discover the host without anyone having
/// to coordinate a fixed number.
const DEFAULT_BIND_ADDR: &str = "0.0.0.0:0";

pub fn build(project_dir: &Path, opts: BuildOptions) -> Result<BuildArtifact> {
    let project_dir = fs::canonicalize(project_dir)
        .with_context(|| format!("resolve project dir {}", project_dir.display()))?;
    let manifest = parse_manifest(&project_dir)?;
    build_sidecar_mode(&project_dir, &manifest, &opts)
}

fn build_sidecar_mode(
    project_dir: &Path,
    manifest: &Manifest,
    opts: &BuildOptions,
) -> Result<BuildArtifact> {
    // Wrapper crates + their cargo target dir come from the
    // `FrameworkSource`. In-tree workspace mode shares the framework
    // workspace's `target/` so common deps stay warm across rebuilds;
    // external (git-deps) projects use their own `<project>/target/`.
    let wrapper_root = opts.source.wrapper_root(project_dir).join(&manifest.name);
    let wrapper_dir = wrapper_root.join("runtime-server/host");
    let sidecar_dir = wrapper_root.join("aas/app");
    let cargo_target = opts.source.cargo_target_dir(project_dir);

    generate_sidecar_wrapper(&sidecar_dir, project_dir, &opts.source, &cargo_target, manifest)?;
    generate_host_wrapper(
        &wrapper_dir,
        &sidecar_dir,
        project_dir,
        &opts.source,
        &cargo_target,
        manifest,
    )?;

    // Captures dir for the sidecar's fat build. The hot-patch
    // builder reads `<dir>/<crate>.<crate-type>.json` per crate
    // when re-emitting the user crate's .rcgu.o files.
    let captures_dir = wrapper_dir
        .parent()
        .map(|p| p.join("captures"))
        .unwrap_or_else(|| sidecar_dir.join("captures"));
    fs::create_dir_all(&captures_dir)
        .with_context(|| format!("create captures dir {}", captures_dir.display()))?;
    // Same parent layout for the per-edit patch dylibs.
    let patches_dir = wrapper_dir
        .parent()
        .map(|p| p.join("patches"))
        .unwrap_or_else(|| sidecar_dir.join("patches"));
    fs::create_dir_all(&patches_dir)
        .with_context(|| format!("create patches dir {}", patches_dir.display()))?;

    // Force the user crate to recompile through the wrapper so we
    // get a fresh capture. Otherwise — if the user crate is already
    // cached from a prior workspace build — cargo skips rustc for
    // it and our wrapper never fires for the very crate we need to
    // replay on hot-patch.
    //
    // This costs one extra ~300ms rebuild per `idealyst dev` start,
    // which is a one-time price; subsequent file-change cycles use
    // the cached capture directly.
    let user_lib = project_dir.join("src/lib.rs");
    if user_lib.exists() {
        let now = std::time::SystemTime::now();
        let _ = filetime_set(&user_lib, now);
    }

    // Build the sidecar first so the host (which spawns it on
    // startup) finds the binary present. The sidecar's build is
    // the "fat" build — RUSTC_WRAPPER captures each member's rustc
    // invocation; -Csave-temps + -Clink-dead-code keep .rcgu.o
    // files on disk + every symbol in the bin's text section so
    // the per-edit patch link can resolve them.
    cargo_build_fat(&sidecar_dir, opts.release, "sidecar", &captures_dir)?;
    cargo_build(&wrapper_dir, opts.release, "host")?;

    let profile = if opts.release { "release" } else { "debug" };
    let host_bin_name = host_binary_name(&manifest.name);
    let sidecar_bin_name = sidecar_binary_name(&manifest.name);

    // Wrapper crate's `.cargo/config.toml` directs all build output
    // to `cargo_target`; cargo writes the binaries under
    // `<cargo_target>/<profile>/<binary-name>`.
    let host_binary = cargo_target.join(profile).join(&host_bin_name);
    let sidecar_binary = cargo_target.join(profile).join(&sidecar_bin_name);

    for (label, path) in [
        ("host", &host_binary),
        ("sidecar", &sidecar_binary),
    ] {
        if !path.is_file() {
            anyhow::bail!(
                "cargo build reported success but {label} binary not at {}",
                path.display(),
            );
        }
    }

    Ok(BuildArtifact {
        host_binary,
        sidecar_binary,
        wrapper_dir,
        sidecar_dir,
    })
}

/// Update a file's mtime to `t`. Wrapper around stdlib utime
/// because we don't want to pull in `filetime` for one call.
/// Returns Err if the path doesn't exist; caller can ignore.
#[cfg(unix)]
fn filetime_set(path: &Path, t: std::time::SystemTime) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    // Easiest portable way: open + close. The OS bumps mtime on
    // metadata-write via futimens, but we'd need libc bindings.
    // Re-write the contents byte-for-byte instead — it's a hot path
    // only on dev-host startup, ~1ms.
    let data = std::fs::read(path)?;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .truncate(false)
        .custom_flags(0)
        .open(path)?;
    use std::io::Write;
    f.write_all(&data)?;
    let _ = t; // mtime gets bumped automatically by the OS on write
    Ok(())
}

#[cfg(not(unix))]
fn filetime_set(path: &Path, _t: std::time::SystemTime) -> std::io::Result<()> {
    let data = std::fs::read(path)?;
    std::fs::write(path, data)
}

fn host_binary_name(project_name: &str) -> String {
    format!("{project_name}-runtime-server-host")
}

fn sidecar_binary_name(project_name: &str) -> String {
    format!("{project_name}-runtime-server-app")
}

// ---------------------------------------------------------------------------
// Sidecar wrapper generation
// ---------------------------------------------------------------------------

fn generate_sidecar_wrapper(
    sidecar_dir: &Path,
    project_dir: &Path,
    source: &FrameworkSource,
    cargo_target: &Path,
    manifest: &Manifest,
) -> Result<()> {
    fs::create_dir_all(sidecar_dir.join("src"))
        .with_context(|| format!("create {}", sidecar_dir.display()))?;

    let sidecar_name = sidecar_binary_name(&manifest.name);
    // `hot-reload` flips the `#[component]` macro into its split form
    // (`__<Name>_hot_impl` + outer dispatch via `dev_hot::call`).
    // Without it, subsecond's jump table is never consulted, so the
    // user crate has to keep the feature on regardless of how thin the
    // wrapper gets.
    let fcore_dep = source.dep("crates/framework/core", &["hot-reload"]);
    // `runtime-server` is dev-server's opt-in for both `host::run` and
    // `sidecar::run`. It pulls `dev-hot`, `subsecond-types`,
    // `libc`, and `anyhow` into the wrapper transitively — we no
    // longer name those deps from this Cargo.toml.
    let dev_server_dep = source.dep("crates/dev/server", &["runtime-server"]);

    let cargo_toml = format!(
        r#"# GENERATED by `idealyst build aas`. Do not edit — rewritten
# every build.
#
# Sidecar binary: statically links the user's crate and calls
# `dev_server::sidecar::run` to host the runtime-server frame loop. Pre-refactor
# this crate carried dev-hot, wire, subsecond-types, libc, and
# serde_json directly — every one of those moved into dev-server
# behind its `sidecar-runtime` feature, so a framework API tweak no
# longer requires regenerating + recompiling the wrapper.

[workspace]

[package]
name = "{sidecar_name}"
version = "0.0.1"
edition = "2021"

[dependencies]
runtime-core = {fcore_dep}
dev-server = {dev_server_dep}
{user_name} = {{ path = "{user_path}" }}

# Sidecar is short-lived dev infra — strip everything that costs
# link time. debug = 0 cuts ~half the link work; the patch dylib's
# stub generator doesn't care about debug info.
[profile.dev]
debug = 0
strip = "debuginfo"
{patch_block}
"#,
        sidecar_name = sidecar_name,
        fcore_dep = fcore_dep,
        dev_server_dep = dev_server_dep,
        user_name = manifest.name,
        user_path = project_dir.display(),
        patch_block = source.patch_block(),
    );


    let main_rs = format!(
        r#"//! GENERATED by `idealyst build aas`. Sidecar binary for the
//! split-process runtime-server dev host. Delegates the entire frame loop to
//! `dev_server::sidecar::run` — anything beyond pointing at the
//! user crate's `app()` belongs in that library function, not
//! here.

fn main() -> std::io::Result<()> {{
    dev_server::sidecar::run({lib}::app)
}}
"#,
        lib = manifest.lib_name,
    );

    write_shared_target_config(sidecar_dir, cargo_target)?;
    fs::write(sidecar_dir.join("Cargo.toml"), cargo_toml)?;
    fs::write(sidecar_dir.join("src/main.rs"), main_rs)?;
    Ok(())
}


// ---------------------------------------------------------------------------
// Host wrapper generation
// ---------------------------------------------------------------------------

fn generate_host_wrapper(
    wrapper_dir: &Path,
    sidecar_dir: &Path,
    project_dir: &Path,
    source: &FrameworkSource,
    cargo_target: &Path,
    manifest: &Manifest,
) -> Result<()> {
    fs::create_dir_all(wrapper_dir.join("src"))
        .with_context(|| format!("create {}", wrapper_dir.display()))?;

    let wrapper_name = host_binary_name(&manifest.name);
    let dev_server_dep = source.dep("crates/dev/server", &["runtime-server"]);
    let build_runtime_server_dep = source.dep("crates/build/runtime-server", &[]);

    let cargo_toml = format!(
        r#"# GENERATED by `idealyst build aas`. Do not edit — rewritten
# every build.
#
# Infra-only runtime-server host: WebSocket server, mDNS, file watcher, and
# the hot-patch builder adapter. Does NOT link the user's crate —
# that lives in the sibling `aas/app` sidecar, which the host spawns
# and either subsecond-patches in place or SIGKILL-respawns. The
# loop body is `dev_server::host::run` (see dev-server's
# `runtime-server` feature); this wrapper exists only to construct a
# `HostConfig` and wire `build_runtime_server::hotpatch::HotPatchBuilder` to
# the `HotPatchAdapter` trait.

[workspace]

[package]
name = "{wrapper_name}"
version = "0.0.1"
edition = "2021"

[dependencies]
dev-server = {dev_server_dep}
# Owns the hot-patch builder: captured-rustc replay, stub-object
# synthesis, dylib link, jump-table construction. The host wrapper
# implements `dev_server::host::HotPatchAdapter` over it; that's
# the only build-tools surface the host depends on.
build-runtime-server = {build_runtime_server_dep}
# `HotPatchAdapter::build`'s return type is `anyhow::Result` — same
# error type the underlying `HotPatchBuilder` returns, so the wrapper
# can `?` through without conversion. Tiny dep; not worth re-exporting
# through dev-server.
anyhow = "1"
{patch_block}
"#,
        wrapper_name = wrapper_name,
        dev_server_dep = dev_server_dep,
        build_runtime_server_dep = build_runtime_server_dep,
        patch_block = source.patch_block(),
    );

    let profile_dir = "debug"; // Mirror what the sidecar lands in. The
                               // host doesn't currently support
                               // release mode through this template.
    let sidecar_bin = cargo_target
        .join(profile_dir)
        .join(sidecar_binary_name(&manifest.name));
    let sidecar_manifest = sidecar_dir.join("Cargo.toml");
    let user_src = project_dir.join("src");
    let captures_dir = sidecar_dir
        .parent()
        .map(|p| p.join("captures"))
        .unwrap_or_else(|| sidecar_dir.join("captures"));
    let patch_target_dir = sidecar_dir
        .parent()
        .map(|p| p.join("patches"))
        .unwrap_or_else(|| sidecar_dir.join("patches"));

    let main_rs = format!(
        r#"//! GENERATED by `idealyst build aas`. Thin runtime-server host shim:
//! builds a `HostConfig`, wraps `HotPatchBuilder` in a
//! `HotPatchAdapter`, hands both to `dev_server::host::run`. The
//! actual host loop (WebSocket listener, mDNS, file watcher,
//! sidecar lifecycle, hot-patch dispatch, port sentinel) lives in
//! `dev-server` under its `runtime-server` feature — only the
//! project-specific paths are inlined here.

use std::path::PathBuf;
use build_runtime_server::hotpatch::{{HotPatchArtifact, HotPatchBuilder}};
use dev_server::host::{{HostConfig, HotPatchAdapter, JumpTable, run}};

const DEFAULT_ADDR: &str = "{default_addr}";

struct BuilderAdapter(HotPatchBuilder);

impl HotPatchAdapter for BuilderAdapter {{
    fn build(
        &self,
        user_crate: &str,
        aslr_reference: u64,
    ) -> anyhow::Result<JumpTable> {{
        let HotPatchArtifact {{ table, .. }} = self.0.build(user_crate, aslr_reference)?;
        Ok(table)
    }}
}}

fn main() -> std::io::Result<()> {{
    let bind_addr = if let Some(a) = std::env::args().nth(1) {{
        a
    }} else if let Ok(p) = std::env::var("IDEALYST_RUNTIME_SERVER_BIND_PORT") {{
        format!("0.0.0.0:{{}}", p)
    }} else {{
        DEFAULT_ADDR.to_string()
    }};

    let sidecar_path = PathBuf::from("{sidecar_bin}");
    let captures_dir = PathBuf::from("{captures_dir}");
    let patch_target_dir = PathBuf::from("{patch_target_dir}");

    let hot_patch: Option<Box<dyn HotPatchAdapter>> =
        match HotPatchBuilder::new(captures_dir, &sidecar_path, patch_target_dir) {{
            Ok(b) => Some(Box::new(BuilderAdapter(b))),
            Err(e) => {{
                eprintln!(
                    "[runtime-server-host] hot-patch builder init failed: {{e:#}} — \
                     falling back to respawn on every change"
                );
                None
            }}
        }};

    let cfg = HostConfig {{
        bind_addr,
        app_id: "{app_id}".to_string(),
        sidecar_path,
        sidecar_manifest: PathBuf::from("{sidecar_manifest}"),
        cargo_target: PathBuf::from("{cargo_target}"),
        user_src: PathBuf::from("{user_src}"),
        user_crate: "{user_crate}".to_string(),
    }};

    run(cfg, hot_patch)
}}
"#,
        default_addr = DEFAULT_BIND_ADDR,
        app_id = manifest.app.require_bundle_id()?,
        sidecar_bin = sidecar_bin.display(),
        sidecar_manifest = sidecar_manifest.display(),
        user_src = user_src.display(),
        cargo_target = cargo_target.display(),
        captures_dir = captures_dir.display(),
        user_crate = manifest.name,
        patch_target_dir = patch_target_dir.display(),
    );

    write_shared_target_config(wrapper_dir, cargo_target)?;
    fs::write(wrapper_dir.join("Cargo.toml"), cargo_toml)?;
    fs::write(wrapper_dir.join("src/main.rs"), main_rs)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Write a `.cargo/config.toml` that redirects the wrapper crate's
/// builds into `target_dir`. For in-tree (workspace) projects this is
/// the framework workspace's shared `target/` so common deps stay
/// warm; for external (git-deps) projects it's the user project's
/// own `target/` so runtime-server output lives alongside the rest of the
/// project's build artifacts.
fn write_shared_target_config(dir: &Path, target_dir: &Path) -> Result<()> {
    let config = format!(
        "# GENERATED. Redirect this wrapper's build output to the\n\
         # shared target dir so subsequent builds reuse the cache and\n\
         # the resulting binary lives at a predictable path.\n\
         \n\
         [build]\n\
         target-dir = \"{}\"\n",
        target_dir.display(),
    );
    fs::create_dir_all(dir.join(".cargo"))?;
    fs::write(dir.join(".cargo/config.toml"), config)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Cargo invocation
// ---------------------------------------------------------------------------

fn cargo_build(wrapper_dir: &Path, release: bool, label: &str) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.args(["build"]).current_dir(wrapper_dir);
    if release {
        cmd.arg("--release");
    }

    eprintln!(
        "[build-runtime-server:{label}] cargo build{} (in {})",
        if release { " --release" } else { "" },
        wrapper_dir.display(),
    );
    let status = cmd
        .status()
        .with_context(|| "spawn `cargo` — is it on your PATH?")?;
    if !status.success() {
        anyhow::bail!("[build-runtime-server:{label}] cargo build exited with {status}");
    }
    Ok(())
}

/// "Fat" build for the sidecar: cargo with `RUSTC_WORKSPACE_WRAPPER`
/// pointing at the running idealyst CLI binary (which dispatches to
/// the `rustc-capture` subcommand via the env-var discriminator),
/// plus `RUSTFLAGS` augmented with `-Csave-temps=true
/// -Clink-dead-code`. Both flags are what dx ships for its
/// equivalent fat build — save-temps keeps the .rcgu.o files on disk
/// past link, and link-dead-code stops the linker from dropping
/// symbols the patch may want to resolve.
fn cargo_build_fat(
    wrapper_dir: &Path,
    release: bool,
    label: &str,
    captures_dir: &Path,
) -> Result<()> {
    let idealyst_bin = std::env::current_exe()
        .context("locate idealyst CLI binary for RUSTC_WORKSPACE_WRAPPER")?;
    let env = hotpatch::fat_build_env(&idealyst_bin, captures_dir);

    let mut cmd = Command::new("cargo");
    cmd.args(["build"]).current_dir(wrapper_dir);
    if release {
        cmd.arg("--release");
    }
    for (k, v) in &env {
        cmd.env(k, v);
    }

    eprintln!(
        "[build-runtime-server:{label}] cargo build (fat){} (in {}; captures → {})",
        if release { " --release" } else { "" },
        wrapper_dir.display(),
        captures_dir.display(),
    );
    let status = cmd
        .status()
        .with_context(|| "spawn `cargo` — is it on your PATH?")?;
    if !status.success() {
        anyhow::bail!("[build-runtime-server:{label}] cargo build exited with {status}");
    }
    Ok(())
}
