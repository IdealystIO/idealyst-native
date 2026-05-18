//! Experimental dlopen-driven AAS host. See [`crate::AasMode::Dylib`]
//! for the full description and known caveats.
//!
//! ## Architecture
//!
//! The dev host is **one** long-lived binary that statically links
//! the framework and uses `libloading` to `dlopen` the user crate as
//! a Rust dylib at runtime. On every source change the host rebuilds
//! ONLY the user crate's dylib (~150 ms cargo invocation, no
//! framework relink), `dlclose`s the old version, `dlopen`s the new,
//! drops the previous framework runtime, and re-renders. No process
//! restart, no IPC, no WebSocket churn.
//!
//! ## Known issue (as of writing)
//!
//! `framework-core`'s `thread_local!` statics produce hash-suffixed
//! `_RUST_STD_INTERNAL_VAL` symbols. When the user-dylib build and
//! the host build both compile framework-core (via separate cargo
//! invocations sharing the same target dir), cargo's fingerprint
//! lands on two compilations whose internal symbol hashes differ.
//! The dlopen then fails with "symbol not found" because the
//! user-dylib was linked against one generation's hash but the
//! actual `libframework_core.dylib` on disk is from another. Fix
//! ideas (untested):
//!
//! - Run a single `cargo build` for both bins instead of two
//!   sequential invocations, so cargo produces one framework-core
//!   dylib that both bins reference by the same hash.
//! - Drop the `rlib` crate-type from framework-core entirely so
//!   there's no ambiguity — every consumer must pick the dylib.
//!   Breaks workspace consumers that today statically link
//!   framework-core; would need workspace-wide audit.
//! - Build std as a dylib via nightly `-Z build-std`; sidesteps the
//!   issue by making the entire link graph dynamic.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use build_ios::Manifest;

use crate::{BuildArtifact, BuildOptions};

const DEFAULT_BIND_ADDR: &str = "0.0.0.0:0";

pub(crate) fn build(
    project_dir: &Path,
    workspace_root: &Path,
    manifest: &Manifest,
    opts: &BuildOptions,
) -> Result<BuildArtifact> {
    // Generate a SINGLE cargo workspace containing both wrappers as
    // members. `cargo build` from the parent dir resolves features /
    // profile across both bins at once, producing exactly ONE
    // compilation of `framework-core` that both binaries link
    // against. Per-wrapper `[workspace]` sections (what the previous
    // sidecar mode uses) cause TWO independent compilations whose
    // crate disambiguators don't match — fatal here because the
    // user-dylib's `thread_local!` references resolve at dlopen-time
    // against the canonical `libframework_core.dylib`, which only
    // exists once with one hash.
    let aas_dir = workspace_root
        .join("target/idealyst")
        .join(&manifest.name)
        .join("aas");
    let wrapper_dir = aas_dir.join("host");
    let user_dylib_dir = aas_dir.join("user-dylib");
    let workspace_target = workspace_root.join("target");

    generate_dylib_workspace(&aas_dir, workspace_root)?;
    generate_user_dylib_wrapper(&user_dylib_dir, project_dir, workspace_root, manifest)?;
    generate_host_wrapper(
        &wrapper_dir,
        &user_dylib_dir,
        project_dir,
        workspace_root,
        manifest,
    )?;

    cargo_build_workspace(&aas_dir, opts.release)?;

    let profile = if opts.release { "release" } else { "debug" };
    let host_bin_name = format!("{}-aas-host", manifest.name);
    let user_dylib_name = user_dylib_filename(&manifest.name);

    let host_binary = workspace_target.join(profile).join(&host_bin_name);
    let user_dylib = workspace_target
        .join(profile)
        .join("deps")
        .join(&user_dylib_name);

    if !host_binary.is_file() {
        anyhow::bail!(
            "cargo build reported success but host binary not at {}",
            host_binary.display(),
        );
    }
    if !user_dylib.is_file() {
        anyhow::bail!(
            "cargo build reported success but user dylib not at {}",
            user_dylib.display(),
        );
    }

    Ok(BuildArtifact {
        host_binary,
        sidecar_binary: user_dylib,
        wrapper_dir,
        sidecar_dir: user_dylib_dir,
    })
}

/// Generate the *outer* workspace Cargo.toml that ties both the host
/// wrapper and the user-dylib wrapper into a single cargo build.
/// Sits at `<workspace>/target/idealyst/<project>/aas/Cargo.toml`
/// and declares its own `[workspace]` so cargo treats it as a
/// separate workspace from the main project workspace (which holds
/// the workspace deps we resolve via path).
///
/// Both members MUST omit their own `[workspace]` lines (cargo would
/// reject nested workspaces). The shared `.cargo/config.toml` lives
/// at this level too so `prefer-dynamic` + the rustlib `rpath`
/// apply to both members uniformly.
fn generate_dylib_workspace(aas_dir: &Path, workspace_root: &Path) -> Result<()> {
    fs::create_dir_all(aas_dir)
        .with_context(|| format!("create {}", aas_dir.display()))?;
    let cargo_toml = r#"# GENERATED. Outer workspace that ties the AAS dylib host and the
# user-dylib together so a single `cargo build` produces ONE
# compilation of every shared dep (framework-core etc.) — eliminates
# the symbol-hash divergence that breaks dlopen.

[workspace]
members = ["host", "user-dylib"]
resolver = "2"

# Match the speed-tuned settings the user-dylib needs. Applies to
# both members through workspace inheritance.
[profile.dev]
debug = 0
strip = "debuginfo"
"#;
    fs::write(aas_dir.join("Cargo.toml"), cargo_toml)?;
    write_dylib_cargo_config(aas_dir, workspace_root)?;
    Ok(())
}

fn user_dylib_crate_name(project_name: &str) -> String {
    format!("{project_name}_aas_user")
}

fn user_dylib_filename(project_name: &str) -> String {
    let crate_underscored = user_dylib_crate_name(project_name);
    format!("lib{crate_underscored}.dylib")
}

// ---------------------------------------------------------------------------
// User-dylib wrapper generation
// ---------------------------------------------------------------------------

fn generate_user_dylib_wrapper(
    user_dylib_dir: &Path,
    project_dir: &Path,
    workspace_root: &Path,
    manifest: &Manifest,
) -> Result<()> {
    fs::create_dir_all(user_dylib_dir.join("src"))
        .with_context(|| format!("create {}", user_dylib_dir.display()))?;

    let crate_name = user_dylib_crate_name(&manifest.name);
    let fcore = workspace_root.join("crates/framework/core");

    // Member of the outer aas workspace. Profile + .cargo/config
    // come from the parent — DO NOT declare them here, otherwise
    // cargo errors with "[workspace] section in member crate".
    let cargo_toml = format!(
        r#"# GENERATED by `idealyst build aas` (Dylib mode).
#
# Tiny re-export crate that exposes the user's `app()` function as a
# `#[no_mangle]` entry point in a Rust-ABI dylib. The host dlopens
# this at startup and on every successful rebuild.

[package]
name = "{crate_name}"
version = "0.0.1"
edition = "2021"

[lib]
crate-type = ["dylib"]

[dependencies]
framework-core = {{ path = "{fcore}" }}
{user_name} = {{ path = "{user_path}" }}
"#,
        crate_name = crate_name,
        fcore = fcore.display(),
        user_name = manifest.name,
        user_path = project_dir.display(),
    );

    let lib_rs = format!(
        r#"//! GENERATED by `idealyst build aas` (Dylib mode). Re-exports
//! the user's `{user_name}::app()` as a stable `#[no_mangle]` symbol
//! the host resolves via libloading.

use framework_core::Primitive;

#[no_mangle]
pub extern "Rust" fn idealyst_app() -> Primitive {{
    {user_name}::app()
}}
"#,
        user_name = manifest.name,
    );

    // Don't write a per-member .cargo/config — the parent
    // workspace's config applies.
    fs::write(user_dylib_dir.join("Cargo.toml"), cargo_toml)?;
    fs::write(user_dylib_dir.join("src/lib.rs"), lib_rs)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Host wrapper generation
// ---------------------------------------------------------------------------

fn generate_host_wrapper(
    wrapper_dir: &Path,
    user_dylib_dir: &Path,
    project_dir: &Path,
    workspace_root: &Path,
    manifest: &Manifest,
) -> Result<()> {
    fs::create_dir_all(wrapper_dir.join("src"))
        .with_context(|| format!("create {}", wrapper_dir.display()))?;

    let wrapper_name = format!("{}-aas-host", manifest.name);
    let fcore = workspace_root.join("crates/framework/core");
    let dev_server = workspace_root.join("crates/dev/server");

    // Member of the outer aas workspace — no per-member [workspace]
    // or [profile] sections (inherited from the parent).
    let cargo_toml = format!(
        r#"# GENERATED by `idealyst build aas` (Dylib mode).
#
# Long-lived AAS dev host that dlopens the user crate's dylib.

[package]
name = "{wrapper_name}"
version = "0.0.1"
edition = "2021"

[dependencies]
framework-core = {{ path = "{fcore}" }}
dev-server = {{ path = "{dev_server}" }}
libloading = "0.8"
serde_json = "1"
"#,
        wrapper_name = wrapper_name,
        fcore = fcore.display(),
        dev_server = dev_server.display(),
    );

    let workspace_target = workspace_root.join("target");
    let user_dylib_filename = user_dylib_filename(&manifest.name);
    let aas_workspace_dir = user_dylib_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("user_dylib_dir has no parent"))?
        .to_path_buf();
    let user_dylib_crate = user_dylib_crate_name(&manifest.name);
    let user_src = project_dir.join("src");

    let main_rs = format!(
        r#"//! GENERATED by `idealyst build aas` (Dylib mode).

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{{Arc, Mutex}};

use dev_server::{{
    spawn_rebuild_loop, RebuildCommand, RebuildConfig, WireRecordingBackend,
}};
use framework_core::{{render, Owner, Primitive}};

const DEFAULT_ADDR: &str = "{default_addr}";
const APP_ID: &str = "{app_id}";
const USER_DYLIB_PATH: &str = "{user_dylib_path}";

type IdealystAppFn = unsafe extern "Rust" fn() -> Primitive;

struct Generation {{
    _owner: Owner,
    _library: libloading::Library,
}}

fn load_and_render(
    recorder_rc: &Rc<RefCell<WireRecordingBackend>>,
) -> std::io::Result<Generation> {{
    let library = unsafe {{
        libloading::Library::new(USER_DYLIB_PATH).map_err(|e| {{
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("dlopen({{:?}}): {{}}", USER_DYLIB_PATH, e),
            )
        }})?
    }};
    let app: libloading::Symbol<IdealystAppFn> = unsafe {{
        library.get(b"idealyst_app\0").map_err(|e| {{
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("symbol `idealyst_app`: {{}}", e),
            )
        }})?
    }};
    let tree = unsafe {{ app() }};
    drop(app);
    let owner = render(recorder_rc.clone(), tree);
    Ok(Generation {{ _owner: owner, _library: library }})
}}

fn main() -> std::io::Result<()> {{
    let addr = if let Some(a) = std::env::args().nth(1) {{
        a
    }} else if let Ok(p) = std::env::var("IDEALYST_AAS_BIND_PORT") {{
        format!("0.0.0.0:{{}}", p)
    }} else {{
        DEFAULT_ADDR.to_string()
    }};

    let recorder = WireRecordingBackend::new();
    let recorder_rc = Rc::new(RefCell::new(recorder.clone()));

    let generation: Rc<RefCell<Option<Generation>>> = Rc::new(RefCell::new(None));
    match load_and_render(&recorder_rc) {{
        Ok(g) => {{
            *generation.borrow_mut() = Some(g);
            eprintln!("[aas-host] initial dylib loaded + rendered");
        }}
        Err(e) => {{
            eprintln!("[aas-host] initial dlopen failed: {{e}} — host running empty");
        }}
    }}

    let reload_signal: Arc<Mutex<Option<()>>> = Arc::new(Mutex::new(None));
    let reload_for_watcher = reload_signal.clone();
    let aas_workspace_dir = PathBuf::from("{aas_workspace_dir}");
    let user_src = PathBuf::from("{user_src}");
    spawn_rebuild_loop(RebuildConfig {{
        // Drive the watcher's cargo from inside the OUTER aas
        // workspace directory. Cargo's config-file discovery walks
        // upward from cwd; without `cwd` set here, cargo would miss
        // `aas/.cargo/config.toml` (prefer-dynamic + rpath) AND end
        // up using a different target-dir than the initial build —
        // either of which forces a full from-scratch recompile every
        // edit. `-p {user_dylib_crate}` narrows the build to just
        // the user-dylib member; the host bin stays up.
        command: RebuildCommand {{
            program: "cargo".into(),
            args: vec![
                "build".into(),
                "-p".into(),
                "{user_dylib_crate}".into(),
            ],
            cwd: Some(aas_workspace_dir),
        }},
        watch_paths: vec![user_src],
        debounce: std::time::Duration::from_millis(100),
        before_exec: None,
        on_success: Some(Box::new(move || {{
            if let Ok(mut g) = reload_for_watcher.lock() {{
                *g = Some(());
            }}
        }})),
    }});

    let port_mirror: Arc<Mutex<Option<u16>>> = Arc::new(Mutex::new(None));
    if let Ok(path) = std::env::var("IDEALYST_AAS_PORT_FILE") {{
        let port_for_file = port_mirror.clone();
        std::thread::spawn(move || {{
            for _ in 0..200 {{
                if let Ok(g) = port_for_file.lock() {{
                    if let Some(p) = *g {{
                        let _ = std::fs::write(&path, p.to_string());
                        return;
                    }}
                }}
                std::thread::sleep(std::time::Duration::from_millis(50));
            }}
        }});
    }}

    eprintln!("[aas-host] starting (advertising app_id={{}} via mDNS)", APP_ID);

    let generation_for_tick = generation.clone();
    let recorder_for_tick = recorder_rc.clone();
    dev_server::transport::serve_with_tick_and_port(
        addr,
        recorder,
        APP_ID,
        move || {{
            let pending = if let Ok(mut g) = reload_signal.lock() {{
                g.take().is_some()
            }} else {{
                false
            }};
            if !pending {{
                return;
            }}
            eprintln!("[aas-host] reload triggered — swapping dylib");
            {{
                let mut gen_slot = generation_for_tick.borrow_mut();
                *gen_slot = None;
            }}
            match load_and_render(&recorder_for_tick) {{
                Ok(g) => {{
                    *generation_for_tick.borrow_mut() = Some(g);
                    eprintln!("[aas-host] reload OK");
                }}
                Err(e) => {{
                    eprintln!("[aas-host] reload dlopen failed: {{e}}");
                }}
            }}
        }},
        Some(port_mirror),
        None,
    )
}}
"#,
        default_addr = DEFAULT_BIND_ADDR,
        app_id = manifest.app.require_bundle_id()?,
        user_dylib_path = workspace_target.join("debug/deps").join(&user_dylib_filename).display(),
        aas_workspace_dir = aas_workspace_dir.display(),
        user_dylib_crate = user_dylib_crate,
        user_src = user_src.display(),
    );

    // No per-member .cargo/config — parent workspace's applies.
    fs::write(wrapper_dir.join("Cargo.toml"), cargo_toml)?;
    fs::write(wrapper_dir.join("src/main.rs"), main_rs)?;
    Ok(())
}

fn write_dylib_cargo_config(dir: &Path, workspace_root: &Path) -> Result<()> {
    let target_dir = workspace_root.join("target");
    let sysroot_lib = rustc_sysroot_lib()?;
    let config = format!(
        "# GENERATED (Dylib mode). Share workspace target dir + prefer\n\
         # dynamic linking so host + user-dylib resolve to ONE\n\
         # `libframework_core.dylib` at runtime. `-rpath` points at\n\
         # the rustup toolchain's host-target `lib/` so libstd/libcore\n\
         # resolve (they ride along when prefer-dynamic is on).\n\
         \n\
         [build]\n\
         target-dir = \"{target}\"\n\
         rustflags = [\"-C\", \"prefer-dynamic\", \"-C\", \"link-arg=-Wl,-rpath,{sysroot_lib}\"]\n",
        target = target_dir.display(),
        sysroot_lib = sysroot_lib.display(),
    );
    fs::create_dir_all(dir.join(".cargo"))?;
    fs::write(dir.join(".cargo/config.toml"), config)?;
    Ok(())
}

fn rustc_sysroot_lib() -> Result<PathBuf> {
    let sysroot_out = Command::new("rustc")
        .args(["--print", "sysroot"])
        .output()
        .context("ask rustc for sysroot")?;
    if !sysroot_out.status.success() {
        anyhow::bail!("rustc --print sysroot failed: {}", sysroot_out.status);
    }
    let sysroot = String::from_utf8(sysroot_out.stdout)
        .context("rustc sysroot output not utf-8")?;

    let triple_out = Command::new("rustc")
        .args(["-vV"])
        .output()
        .context("ask rustc for host triple")?;
    if !triple_out.status.success() {
        anyhow::bail!("rustc -vV failed: {}", triple_out.status);
    }
    let triple_info = String::from_utf8(triple_out.stdout)
        .context("rustc -vV output not utf-8")?;
    let triple = triple_info
        .lines()
        .find_map(|l| l.strip_prefix("host: "))
        .ok_or_else(|| anyhow::anyhow!("rustc -vV missing `host:` line"))?
        .trim();

    Ok(PathBuf::from(sysroot.trim())
        .join("lib/rustlib")
        .join(triple)
        .join("lib"))
}

/// Single `cargo build` from the outer aas workspace. Builds both
/// the host bin and the user-dylib lib in one invocation so cargo's
/// resolver produces a unified compilation graph — only one
/// `framework-core` compilation, only one symbol-hash generation.
fn cargo_build_workspace(aas_dir: &Path, release: bool) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.args(["build"]).current_dir(aas_dir);
    if release {
        cmd.arg("--release");
    }
    eprintln!(
        "[build-aas:dylib] cargo build{} (in {})",
        if release { " --release" } else { "" },
        aas_dir.display(),
    );
    let status = cmd
        .status()
        .with_context(|| "spawn `cargo` — is it on your PATH?")?;
    if !status.success() {
        anyhow::bail!("[build-aas:dylib] cargo build exited with {status}");
    }
    Ok(())
}
