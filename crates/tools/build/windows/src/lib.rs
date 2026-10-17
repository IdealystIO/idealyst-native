//! Build orchestration for `idealyst build --windows`.
//!
//! Generates a tiny binary wrapper at:
//!
//! ```text
//! <workspace>/target/idealyst/<project>/windows/
//! ```
//!
//! The wrapper depends on `host-win32` + the user's crate, with a
//! `main()` that calls `host_win32::run_with(opts, register_scene_extensions,
//! app)`. Builds the wrapper via `cargo build`, returns the produced
//! `.exe`'s path.
//!
//! Mirrors `build-macos` but simpler: Windows has no `.app`-bundle /
//! codesign step, and no universal-binary lipo — a native `.exe`
//! launches directly. The generated binary uses the console subsystem
//! so framework logs surface in the terminal `idealyst run` was
//! launched from.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use build_ios::{parse_manifest, FrameworkSource, Manifest};

#[derive(Clone, Debug)]
pub struct BuildOptions {
    /// Compile with `--release`. Default: debug.
    pub release: bool,
    /// Cargo features to enable (e.g. `runtime-core/dev` from `idealyst
    /// dev` so the Robot bridge auto-starts). Forwarded as `--features`.
    pub user_features: Vec<String>,
    /// Framework-source resolution: workspace path-deps in-tree, git
    /// deps for external installs.
    pub source: FrameworkSource,
    /// CPU architecture to build for. `None` = this host's default (x64
    /// when cross-building).
    pub arch: Option<Arch>,
}

/// A Windows CPU architecture to build for.
///
/// An app should match the machine it runs on: an x64 build runs on an
/// Arm64 PC under emulation, but in-process components the OS loads into
/// its own native processes (a virtual camera's media source, shell
/// extensions) must then exist for two architectures at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch {
    X64,
    Arm64,
}

impl Arch {
    /// Parse a `--arch` value.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "x64" | "x86_64" | "amd64" => Ok(Arch::X64),
            "arm64" | "aarch64" => Ok(Arch::Arm64),
            other => anyhow::bail!("unknown Windows architecture `{other}` (use `x64` or `arm64`)"),
        }
    }
}

/// The Arm64 cross triple: LLVM's MinGW (`llvm-mingw`), whose
/// `aarch64-w64-mingw32-clang` is rustc's default linker for it.
pub const ARM64_CROSS_TRIPLE: &str = "aarch64-pc-windows-gnullvm";

/// The `--target` triple for `arch` on this host (`None` = the host's own).
pub fn triple_for(arch: Option<Arch>) -> Option<&'static str> {
    triple_for_host(cfg!(target_os = "windows"), arch)
}

fn triple_for_host(host_is_windows: bool, arch: Option<Arch>) -> Option<&'static str> {
    match (host_is_windows, arch) {
        (_, None) => cross_triple_for(host_is_windows),
        (true, Some(Arch::X64)) => Some("x86_64-pc-windows-msvc"),
        (true, Some(Arch::Arm64)) => Some("aarch64-pc-windows-msvc"),
        (false, Some(Arch::X64)) => Some(CROSS_TRIPLE),
        (false, Some(Arch::Arm64)) => Some(ARM64_CROSS_TRIPLE),
    }
}

/// Fail early, with what to install, when the Arm64 cross linker is absent
/// (otherwise the failure is a link error deep in a long build).
fn check_cross_linker(triple: Option<&str>) -> Result<()> {
    if triple != Some(ARM64_CROSS_TRIPLE) {
        return Ok(());
    }
    let linker = "aarch64-w64-mingw32-clang";
    let found = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(linker).is_file()))
        .unwrap_or(false);
    if !found {
        anyhow::bail!(
            "building Windows Arm64 needs `{linker}` on PATH: install llvm-mingw \
             (https://github.com/mstorsjo/llvm-mingw/releases) and add its `bin` directory to PATH, \
             and `rustup target add {ARM64_CROSS_TRIPLE}`"
        );
    }
    Ok(())
}

mod runtime_files;

#[derive(Debug)]
pub struct BuildArtifact {
    /// Path to the produced Windows `.exe` (ready to spawn).
    pub binary: PathBuf,
    /// Files SDKs need beside the `.exe` at runtime — declared runtime files
    /// (`WebView2Loader.dll`) and the DLLs of declared artifacts — already in
    /// its directory; see `runtime_files`. Whatever
    /// moves the `.exe` elsewhere must move these with it.
    pub runtime_files: Vec<PathBuf>,
    /// Wrapper crate directory (useful for debugging the template).
    pub wrapper_dir: PathBuf,
}

/// The `--target` a non-Windows host cross-compiles the wrapper to.
pub const CROSS_TRIPLE: &str = "x86_64-pc-windows-gnu";

/// The `--target` triple for a build on THIS host: `None` on Windows (a
/// native host build), [`CROSS_TRIPLE`] everywhere else.
pub fn host_cross_triple() -> Option<&'static str> {
    cross_triple_for(cfg!(target_os = "windows"))
}

fn cross_triple_for(host_is_windows: bool) -> Option<&'static str> {
    if host_is_windows { None } else { Some(CROSS_TRIPLE) }
}

/// Where cargo puts the `.exe` for a build into `target_dir`: the host
/// profile dir for a native build, `<target>/<triple>/<profile>` for a
/// cross build.
fn binary_dir(target_dir: &Path, triple: Option<&str>, release: bool) -> PathBuf {
    let profile = if release { "release" } else { "debug" };
    match triple {
        Some(t) => target_dir.join(t).join(profile),
        None => target_dir.join(profile),
    }
}

/// Build the Windows wrapper for `project_dir` with `opts`.
pub fn build(project_dir: &Path, opts: BuildOptions) -> Result<BuildArtifact> {
    // Absolutize WITHOUT any volume access: both `fs::canonicalize` and
    // `std::path::absolute` issue a volume/final-path query that fails
    // with "the volume does not contain a recognized file system" (os
    // error 1005) on this setup's `Z:` VM share (a virtio-fs/9p mount
    // where `GetFinalPathNameByHandleW` isn't supported). `current_dir`
    // + lexical `join` never opens the volume — all we need for a
    // stable path-dep in the generated wrapper.
    let project_dir = if project_dir.is_absolute() {
        project_dir.to_path_buf()
    } else {
        std::env::current_dir()
            .with_context(|| "read current dir to absolutize the project path")?
            .join(project_dir)
    };
    let manifest = parse_manifest(&project_dir)?;

    let wrapper_root = opts.source.wrapper_root(&project_dir);
    let wrapper_dir = wrapper_root.join(&manifest.name).join("windows");
    let cargo_target_dir = windows_target_dir(&wrapper_dir);

    generate_wrapper(&wrapper_dir, &cargo_target_dir, &project_dir, &manifest, &opts)?;

    let bin_name = binary_name(&manifest.name);
    let triple = triple_for(opts.arch);
    check_cross_linker(triple)?;
    cargo_build(&wrapper_dir, &cargo_target_dir, triple, opts.release, &opts.user_features)?;

    // cargo emits `<bin_name>.exe` on the windows target.
    let binary = binary_dir(&cargo_target_dir, triple, opts.release).join(format!("{bin_name}.exe"));
    if !binary.is_file() {
        anyhow::bail!(
            "cargo build reported success but Windows binary not at {}",
            binary.display(),
        );
    }
    ship_assets(&project_dir, &binary)?;
    let metadata = runtime_files::metadata(&wrapper_dir.join("Cargo.toml"))?;
    let mut declared = runtime_files::resolve(&metadata, runtime_files::arch_dir(triple))?;
    for plan in runtime_files::resolve_artifacts(&metadata)? {
        declared.push(build_artifact(&wrapper_dir, &cargo_target_dir, &project_dir, &plan, triple, opts.release)?);
    }
    let exe_dir = binary.parent().expect("the binary sits in a directory");
    let runtime_files = runtime_files::ship(&declared, exe_dir)?;
    Ok(BuildArtifact { binary, runtime_files, wrapper_dir })
}

/// Build one SDK-declared artifact (`runtime_files::ArtifactPlan`) for the
/// app's target and profile, into the wrapper's target dir, and return the
/// DLL. A cross build puts it in the `.exe`'s own directory.
fn build_artifact(
    wrapper_dir: &Path,
    target_dir: &Path,
    project_dir: &Path,
    plan: &runtime_files::ArtifactPlan,
    triple: Option<&str>,
    release: bool,
) -> Result<PathBuf> {
    let b = build_ios::sdk_products::ProductBuild {
        label: "build-windows",
        wrapper_dir,
        target_dir,
        project_dir,
        // The Windows wrapper carries no `[patch]` tables.
        patch_block: "",
        package: &plan.package,
        manifest_dir: &plan.manifest_dir,
        target: build_ios::sdk_products::ProductTarget::Lib,
        features: &plan.features,
        env: &[],
        triple,
        release,
    };
    // `write_replacing`, like every generated file here: the shim lives on
    // the same VM share as the wrapper.
    let write = |path: &Path, contents: &str| {
        let comment = if path.extension().is_some_and(|e| e == "rs") { "//" } else { "#" };
        write_replacing(path, contents, comment)
    };
    let profile = build_ios::sdk_products::build_product(&b, &write)?;
    let dll = profile.join(plan.dll_name());
    if !dll.is_file() {
        anyhow::bail!(
            "{} declares a windows artifact from `{}`, but its build produced no {}",
            plan.declarer,
            plan.package,
            dll.display()
        );
    }
    Ok(dll)
}

/// Ship the app crate's `assets/` to `<exe dir>/assets/`, where the
/// Win32 host reads `asset!` files at runtime. Incremental and symlink-safe;
/// see [`build_ios::app_assets`].
fn ship_assets(project_dir: &Path, binary: &Path) -> Result<()> {
    build_ios::app_assets::stage_app_assets_beside(project_dir, binary).map(|_| ())
}

/// Produced-binary name, suffixed `-windows` so it can't collide with
/// the user crate's lib/bin name or the other platforms' wrappers.
fn binary_name(project_name: &str) -> String {
    format!("{project_name}-windows")
}

/// Where the Windows wrapper builds: a target dir PRIVATE to it
/// (`build_ios::wrapper_target_dir` — `<wrapper>/target`).
///
/// Two reasons, the second inherited from the per-OS bucket this replaced
/// (`<shared target>/win32`):
///
/// - Every wrapper seeds its lock from the app's and prunes its target dir
///   after each build, and both require that nothing else builds there.
/// - The project's `target/` is also written by builds from other
///   operating systems when the repo lives on a shared filesystem (this
///   project's dev setup: Linux host + Windows VM over a `Z:` share).
///   Cargo does not segregate host-triple artifacts or fingerprints by
///   triple — a Linux build of the same crate replaces
///   `deps/lib<crate>*.rlib` and re-stamps `.fingerprint/`, after which
///   the next Windows build either fails with E0461 ("couldn't find crate
///   `<name>` with expected target triple x86_64-pc-windows-msvc") or,
///   worse, silently links artifacts cargo wrongly believes fresh. Only
///   the Windows builder generates the `windows` wrapper, so no other OS
///   ever builds into its dir.
fn windows_target_dir(wrapper_dir: &Path) -> PathBuf {
    build_ios::wrapper_target_dir(wrapper_dir)
}

fn generate_wrapper(
    wrapper_dir: &Path,
    cargo_target_dir: &Path,
    project_dir: &Path,
    manifest: &Manifest,
    opts: &BuildOptions,
) -> Result<()> {
    fs::create_dir_all(wrapper_dir.join("src"))
        .with_context(|| format!("create {}", wrapper_dir.display()))?;

    let bin_name = binary_name(&manifest.name);
    let host_dep = opts.source.dep("crates/host/win32", &[]);
    // `runtime-core` as a direct dep so `idealyst dev` can pass
    // `--features runtime-core/dev` (via the wrapper's `dev` feature).
    let fcore_dep = opts.source.dep("crates/runtime/core", &[]);
    // `runtime-shared` for the Robot bridge the `dev` main starts.
    let fshared_dep = opts.source.dep("crates/runtime/shared", &[]);
    let user_dep = format!("{{ path = \"{}\" }}", project_dir.display());

    let bundle_id = manifest
        .app
        .bundle_id
        .clone()
        .unwrap_or_else(|| format!("com.example.{}", manifest.name));

    let deps_block = format!(
        "host-win32 = {host_dep}\n\
         runtime-core = {fcore_dep}\n\
         runtime-shared = {fshared_dep}\n\
         {user_name} = {user_dep}\n",
        user_name = manifest.name,
    );
    // Path deps carry absolute Windows paths (`z:\…`). TOML basic
    // strings treat `\` as an escape, so a raw Windows path is invalid
    // TOML — normalize every separator to `/` (Cargo accepts forward
    // slashes in `path = "…"` on Windows). The block is only path deps
    // + crate names (no legitimate backslashes), so a blanket replace
    // is safe.
    let deps_block = deps_block.replace('\\', "/");
    // Workspace mode: redirect the app crate's registry / git framework deps
    // onto the same checkout the wrapper's path deps use — without it the
    // graph holds two of every runtime crate ("expected `Element`, found
    // `Element`"). Empty in git mode. Same `/` normalization as above.
    let patch_block = opts.source.patch_block().replace('\\', "/");

    let cargo_toml = format!(
        r#"# GENERATED by `idealyst build --windows`. Do not edit — rewritten every build.
#
# Win32 wrapper. Depends on `host-win32` + the user crate, mounts
# `app()` in-process. Produces `<target>/<profile>/{bin_name}.exe`.

[workspace]

[package]
name = "{bin_name}"
version = "0.0.1"
edition = "2021"

[dependencies]
{deps_block}
[features]
# `runtime-shared/robot` = the bridge transport; `host-win32/robot` =
# `backend-windows/robot` (element registry + native introspection).
# Same set as the GTK wrapper's `dev`.
dev = ["runtime-core/dev", "runtime-shared/robot", "host-win32/robot"]
{patch_block}"#,
    );

    let main_rs = main_rs(
        &manifest.lib_name,
        &manifest.app.name,
        &bundle_id,
        &project_dir.to_string_lossy(),
    );

    write_target_config(wrapper_dir, cargo_target_dir, triple_for(opts.arch))?;
    write_replacing(&wrapper_dir.join("Cargo.toml"), &cargo_toml, "#")?;
    write_replacing(&wrapper_dir.join("src/main.rs"), &main_rs, "//")?;
    // Build exactly the versions the app locked (re-seeded whenever the
    // app's lock changes) instead of re-resolving against the registry on
    // every run. Safe because the target dir is private. See
    // `build_ios::seed_wrapper_lockfile`.
    build_ios::seed_wrapper_lockfile(wrapper_dir, project_dir)?;
    Ok(())
}

/// Byte size every generated file is padded to a multiple of.
const PAD_BLOCK: usize = 4096;

/// Pad `contents` with `comment`-prefixed filler lines to the next
/// `PAD_BLOCK` multiple (with ≥128 bytes of slack). See
/// [`write_replacing`] for why: it makes every regeneration of a file
/// the same byte length, and makes any stale region a cache could
/// resurface consist of old PADDING — inert comment bytes — never a
/// dangling token.
fn pad_to_block(contents: &str, comment: &str) -> String {
    let mut s = String::from(contents);
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s.push_str(comment);
    s.push_str(
        " ---- padding: keeps every regeneration of this file byte-stable so a \
         stale-page cache on a VM-share filesystem can never resurface a \
         non-comment tail (see build-windows::write_replacing). ----\n",
    );
    let target = ((s.len() + 128) / PAD_BLOCK + 1) * PAD_BLOCK;
    while s.len() < target {
        let remaining = target - s.len();
        if remaining <= comment.len() + 1 {
            // Too small for another comment line: finish the previous
            // one exactly (replace its trailing newline with filler).
            s.pop();
            while s.len() < target - 1 {
                s.push('~');
            }
            s.push('\n');
            break;
        }
        let line = (remaining - 1).min(78).max(comment.len());
        s.push_str(comment);
        for _ in 0..(line - comment.len()) {
            s.push('~');
        }
        s.push('\n');
    }
    s
}

/// Durable replace for a generated file on a hostile filesystem.
///
/// This project's `Z:` VM share (virtio-fs/9p; server = the Linux
/// host) nondeterministically resurfaces stale tail bytes of a
/// PREVIOUS same-path file after a rewrite — observed repeatedly as a
/// regenerated wrapper Cargo.toml re-growing an old trailing `]`
/// ("missing table open, expected `[`"). It survived plain
/// `fs::write`, `remove_file` + write, AND write-to-unique-temp +
/// rename — the resurrection is on the PATH's cached pages, not the
/// write strategy. So this layers three defenses:
///
/// 1. **Skip identical writes** — the wrapper regenerates identical
///    bytes on almost every build; not rewriting means no rewrite to
///    corrupt (and no mtime churn for cargo to chase).
/// 2. **Fixed-size comment padding** (`pad_to_block`) — every version
///    has the same padded length, so there is no "beyond new EOF"
///    region; and if lengths ever differ across a block boundary, the
///    resurrected region is the old version's padding: comments.
/// 3. **Read-back verify + retry** — writes go through a unique temp
///    name + rename, then are read back and compared; mismatch retries
///    (fresh temp name), then fails loudly instead of letting cargo
///    parse garbage.
fn write_replacing(path: &Path, contents: &str, comment: &str) -> Result<()> {
    let padded = pad_to_block(contents, comment);
    if let Ok(existing) = fs::read_to_string(path) {
        if existing == padded {
            return Ok(());
        }
    }
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    for attempt in 0u32..3 {
        let tmp = path.with_file_name(format!(
            "{file_name}.tmp-{}-{attempt}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0),
        ));
        fs::write(&tmp, &padded).with_context(|| format!("write {}", tmp.display()))?;
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(e).with_context(|| format!("remove stale {}", path.display()))
            }
        }
        fs::rename(&tmp, path)
            .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
        match fs::read_to_string(path) {
            Ok(back) if back == padded => return Ok(()),
            _ => {
                eprintln!(
                    "[build-windows] {} read back corrupted after write (attempt {}) — retrying",
                    path.display(),
                    attempt + 1,
                );
            }
        }
    }
    anyhow::bail!(
        "generated file {} keeps reading back corrupted — filesystem cache issue; \
         delete the wrapper directory and rebuild",
        path.display(),
    )
}

/// `project_root` is the HOST path of the app crate — what the parity runner
/// and the MCP server compare a registration against. Baked at generation
/// time rather than read at run time because under Wine the process sees the
/// project as `Z:\…`, which no host-side tool would match.
fn main_rs(user_lib: &str, app_name: &str, bundle_id: &str, project_root: &str) -> String {
    // A Rust string literal, quotes included — the path is spliced into
    // generated source and may contain `"` or `\`.
    let project_root = format!("{project_root:?}");
    format!(
        r#"//! GENERATED by `idealyst build --windows`. Wrapper binary for
//! the Win32-backed native Windows runtime.

use {user_lib}::app;

fn main() {{
    // `--emit-catalog`: dump the MCP catalog JSON and exit without
    // opening a window. `idealyst mcp --from-bin <this-exe>` spawns
    // this to extract the project's catalog. Only in `dev` builds.
    #[cfg(feature = "dev")]
    {{
        if std::env::args().any(|a| a == "--emit-catalog") {{
            let json = ::runtime_core::__mcp::catalog_json();
            println!("{{}}", ::runtime_core::__serde_json::to_string_pretty(&json).unwrap());
            return;
        }}
    }}

    // Register the project's identity for the Robot bridge's registration
    // file, then START the bridge. Stamping the identity alone binds
    // nothing: other platforms get their listener from the dev-server
    // sidecar, and `dev --windows` is local-mount only, so without the
    // start the app had no listener and no registration — unreachable by
    // the MCP robot tools, and a timeout for `idealyst test --parity`.
    // Deferred to `on_main_loop_start` because the bridge's poll is
    // scheduler-driven and `run_with` installs the scheduler on the way
    // into the loop. Same shape as the GTK wrapper.
    #[cfg(feature = "dev")]
    {{
        ::runtime_shared::robot::bridge::set_app_identity(
            ::runtime_shared::robot::bridge::AppIdentity {{
                name: "{app_name}".to_string(),
                bundle_id: Some("{bundle_id}".to_string()),
                project_root: ::std::option::Option::Some({project_root}.to_string()),
            }},
        );
        host_win32::on_main_loop_start(|| {{
            match ::runtime_shared::robot::bridge::relay_url_from_env() {{
                Some(url) => ::runtime_shared::robot::bridge::start_relay_client(url),
                None => ::runtime_shared::robot::bridge::start_auto_polling(
                    ::runtime_shared::robot::bridge::DEFAULT_PORT,
                ),
            }}
        }});
    }}

    // The app's identity to Windows (AppUserModelID + display name): toast
    // notifications are only shown for an id Windows knows, and the taskbar
    // groups the app's windows under it.
    host_win32::set_app_identity("{bundle_id}", "{app_name}");

    let opts = host_win32::RunOptions {{
        title: "{app_name}".to_string(),
        width: 1024,
        height: 768,
    }};
    // `run_with` (not `run`) so the user crate's
    // `pub fn register_scene_extensions(&mut Registry<_>)` seam runs after
    // `register_builtins` — how SDK payload handlers get into the scene
    // registry. Same seam as the GTK / macOS wrappers. Returns the exit code.
    std::process::exit(host_win32::run_with(opts, {user_lib}::register_scene_extensions, app));
}}
"#,
    )
}

/// Redirect the wrapper crate's build output into the project's shared
/// `target/` so common dependencies aren't recompiled per wrapper.
/// The wrapper's `.cargo/config.toml`: its private target dir + the
/// framework registry (`build_ios::private_target_config`, which writes
/// the path with forward slashes — a backslash is an invalid TOML escape).
fn write_target_config(dir: &Path, target_dir: &Path, triple: Option<&str>) -> Result<()> {
    let config = target_config(target_dir, triple);
    fs::create_dir_all(dir.join(".cargo"))?;
    write_replacing(&dir.join(".cargo/config.toml"), &config, "#")?;
    Ok(())
}

/// The wrapper's `.cargo/config.toml` body: the private target dir and
/// registry, plus — for the GNU cross target — the DLL export fix below.
///
/// An app crate is usually `crate-type = ["cdylib", "rlib"]` (the cdylib is
/// its web build), and cargo links every listed crate type of a dependency,
/// so the cross build also links the app as a Windows DLL. That DLL exports
/// no C symbols, and GNU ld's documented fallback for a DLL with no explicit
/// exports is to export EVERY global symbol — tens of thousands for a debug
/// build of the framework. Past 65 535 the link fails outright ("export
/// ordinal too large: 96800" on the docs app); below it, it silently
/// produced a pointless DLL. `--exclude-all-symbols` turns the fallback off;
/// anything rustc lists in its `.def` file is still exported. The MSVC
/// linker (native Windows hosts) never auto-exports, so it needs nothing.
fn target_config(target_dir: &Path, cross: Option<&str>) -> String {
    let mut config = build_ios::private_target_config("idealyst build --windows", target_dir);
    match cross {
        // llvm-mingw's lld honours the same flag. `+crt-static` links the
        // unwinder statically: by default the gnullvm target imports
        // `libunwind.dll`, which no Windows machine has.
        Some(triple) if triple.ends_with("-gnullvm") => config.push_str(&format!(
            "\n[target.{triple}]\nrustflags = [\"-C\", \"link-arg=-Wl,--exclude-all-symbols\", \"-C\", \"target-feature=+crt-static\"]\n"
        )),
        Some(triple) if triple.ends_with("-gnu") => config.push_str(&format!(
            "\n[target.{triple}]\nrustflags = [\"-C\", \"link-arg=-Wl,--exclude-all-symbols\"]\n"
        )),
        _ => {}
    }
    config
}

/// Build the wrapper into `target_dir`, then prune that dir to the units
/// its recorded variants use (`build_ios::build_wrapper`).
fn cargo_build(
    wrapper_dir: &Path,
    target_dir: &Path,
    triple: Option<&str>,
    release: bool,
    user_features: &[String],
) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.args(["build"]);
    if let Some(t) = triple {
        cmd.args(["--target", t]);
    }
    if release {
        cmd.arg("--release");
    }
    if !user_features.is_empty() {
        cmd.arg("--features").arg(user_features.join(","));
    }
    eprintln!(
        "[build-windows] cargo build{}{}{} (in {})",
        triple.map(|t| format!(" --target {t}")).unwrap_or_default(),
        if release { " --release" } else { "" },
        if user_features.is_empty() {
            String::new()
        } else {
            format!(" --features {}", user_features.join(","))
        },
        wrapper_dir.display(),
    );
    let spec = build_ios::target_gc::BuildSpec::wrapper(target_dir, triple, release, user_features);
    build_ios::build_wrapper("build-windows", wrapper_dir, &mut cmd, &spec)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Mac/Linux host must cross-compile: a plain host `cargo build` of the
    /// wrapper there produced a Mach-O/ELF binary, then failed looking for a
    /// `.exe` that was never going to exist, so `dev --windows` (and a
    /// `--parity …,windows` run) could not start off a Windows box at all.
    #[test]
    fn regression_non_windows_host_cross_compiles_to_gnu() {
        assert_eq!(cross_triple_for(false), Some("x86_64-pc-windows-gnu"));
        assert_eq!(cross_triple_for(true), None);
        let t = Path::new("/w/target");
        assert_eq!(
            binary_dir(t, Some(CROSS_TRIPLE), false),
            PathBuf::from("/w/target/x86_64-pc-windows-gnu/debug")
        );
        assert_eq!(binary_dir(t, None, true), PathBuf::from("/w/target/release"));
    }

    /// The wrapper registered `project_root: None`, so project-scoped
    /// discovery (`discover_all(Some(dir), …)` in the parity runner) skipped
    /// the Windows app and `--parity web,windows` waited out its 300 s timeout
    /// on an app that was up the whole time.
    #[test]
    fn regression_windows_wrapper_registers_its_project_root() {
        let src = main_rs("docs", "Docs", "com.x.docs", r"Z:\proj\docs");
        assert!(
            src.contains(r#"project_root: ::std::option::Option::Some("Z:\\proj\\docs".to_string())"#),
            "project root must be a valid, escaped string literal; got:\n{src}"
        );
        assert!(!src.contains("project_root: ::std::option::Option::None"));
    }

    /// The wrapper named `::runtime_core::robot::bridge`, a path that no
    /// longer exists — every `dev --windows` build failed with E0433 — and
    /// even before that it only stamped an identity, never binding a
    /// listener, so the app was invisible to `list_apps` and to the parity
    /// runner.
    #[test]
    fn regression_windows_dev_wrapper_starts_the_robot_bridge() {
        let src = main_rs("docs", "Docs", "com.x.docs", "/p/docs");
        assert!(!src.contains("runtime_core::robot::bridge"), "stale path; got:\n{src}");
        assert!(src.contains("::runtime_shared::robot::bridge::set_app_identity"));
        assert!(src.contains("start_auto_polling") && src.contains("start_relay_client"));
        assert!(
            src.contains("host_win32::on_main_loop_start"),
            "the start must wait for the scheduler; got:\n{src}"
        );
    }

    /// The wrapper never identified the app to Windows, so an unpackaged
    /// `.exe` had no AppUserModelID and toast notifications were never shown.
    #[test]
    fn regression_windows_wrapper_sets_the_app_identity() {
        let src = main_rs("docs", "Docs", "com.x.docs", "/p/docs");
        let set = src.find(r#"host_win32::set_app_identity("com.x.docs", "Docs");"#).expect(&src);
        assert!(set < src.find("host_win32::run_with").unwrap(), "before the app runs");
    }

    /// The cross build linked the app's cdylib as a DLL with GNU ld's
    /// export-everything fallback and failed past 65 535 exports ("export
    /// ordinal too large: 96800" on the docs app).
    #[test]
    fn regression_gnu_cross_build_disables_dll_auto_export() {
        let dir = Path::new("/w/target/idealyst/app/windows/target");
        let cross: toml::Value = target_config(dir, Some(CROSS_TRIPLE)).parse().expect("valid TOML");
        let flags = cross["target"][CROSS_TRIPLE]["rustflags"].as_array().expect("rustflags");
        assert_eq!(
            flags.iter().map(|v| v.as_str().unwrap()).collect::<Vec<_>>(),
            ["-C", "link-arg=-Wl,--exclude-all-symbols"]
        );
        // A native (MSVC) host build gets no GNU linker flag.
        let native: toml::Value = target_config(dir, None).parse().expect("valid TOML");
        assert!(native.get("target").is_none());
    }

    /// Windows wrappers must NOT build into the bare shared `target/`:
    /// on a cross-OS shared checkout another OS's builds of the same
    /// crates replace the rlibs / fingerprints in place and the next
    /// Windows link fails with E0461 ("couldn't find crate `website`
    /// with expected target triple x86_64-pc-windows-msvc") — or worse,
    /// silently reuses stale artifacts. See `windows_target_dir`: the dir
    /// is under the `windows` wrapper, which only the Windows builder
    /// generates.
    #[test]
    fn regression_cross_os_shared_target_gets_win32_bucket() {
        let wrapper = PathBuf::from("Z:/proj/target/idealyst/demo/windows");
        let d = windows_target_dir(&wrapper);
        assert!(d.starts_with(&wrapper), "windows builds must land under the windows wrapper, got {}", d.display());
        assert_ne!(d, PathBuf::from("Z:/proj/target"));
    }

    /// Regression (the iOS wrapper's twin): the
    /// Windows wrapper deleted its `Cargo.lock` on every generation, so
    /// each run resolved the registry's newest versions instead of the
    /// app's locked ones — a different graph from the app's and a new
    /// generation of the tree per framework release.
    #[test]
    fn regression_windows_wrapper_lock_reresolved_every_run() {
        let world = build_ios::test_support::LockWorld::new(&[("crates/host/win32", "host-win32", &["robot"])], &[]);
        let wrapper = world.wrapper_dir("windows");
        let manifest = build_ios::parse_manifest(&world.app).expect("parse manifest");
        let opts = BuildOptions { release: false, user_features: Vec::new(), source: world.source(), arch: None };
        let generate = || {
            generate_wrapper(&wrapper, &windows_target_dir(&wrapper), &world.app, &manifest, &opts)
                .expect("generate wrapper")
        };
        world.assert_wrapper_follows_app_lock(&wrapper, generate);
        world.assert_private_target_dir(&wrapper, &world.source().cargo_target_dir(&world.app));
    }

    /// An x64 build run on an Arm64 PC failed to start a virtual camera
    /// ("%1 is not a valid Win32 application"): the media source loads in
    /// the emulated x64 app AND in the native Arm64 camera service, so one
    /// registered DLL could never satisfy both. `--arch arm64` builds the
    /// app native, so app and service load the same Arm64 source.
    #[test]
    fn regression_arm64_builds_target_gnullvm_with_a_static_unwinder() {
        assert_eq!(triple_for_host(false, Some(Arch::Arm64)), Some(ARM64_CROSS_TRIPLE));
        assert_eq!(triple_for_host(false, Some(Arch::X64)), Some(CROSS_TRIPLE));
        assert_eq!(triple_for_host(false, None), Some(CROSS_TRIPLE));
        assert_eq!(triple_for_host(true, None), None);
        assert_eq!(triple_for_host(true, Some(Arch::Arm64)), Some("aarch64-pc-windows-msvc"));
        let cfg: toml::Value = target_config(Path::new("/t"), Some(ARM64_CROSS_TRIPLE)).parse().expect("valid TOML");
        let flags = cfg["target"][ARM64_CROSS_TRIPLE]["rustflags"].as_array().expect("rustflags");
        assert!(flags.iter().any(|f| f.as_str() == Some("target-feature=+crt-static")), "{flags:?}");
        assert_eq!(Arch::parse("aarch64").unwrap(), Arch::Arm64);
        assert!(Arch::parse("mips").is_err());
    }

    /// An out-of-tree app built against a local framework checkout (the
    /// CLI run from that checkout) failed to compile for Windows with
    /// "expected `Element`, found `Element`": the wrapper took the
    /// framework by path while the app crate's own deps stayed on the
    /// registry, because this wrapper never emitted the patch block the
    /// macOS / iOS wrappers do.
    #[test]
    fn regression_windows_wrapper_patches_framework_deps_onto_workspace_checkout() {
        let world = build_ios::test_support::LockWorld::new(&[("crates/host/win32", "host-win32", &["robot"])], &[]);
        let wrapper = world.wrapper_dir("windows");
        let manifest = build_ios::parse_manifest(&world.app).expect("parse manifest");
        let opts = BuildOptions { release: false, user_features: Vec::new(), source: world.source(), arch: None };
        generate_wrapper(&wrapper, &windows_target_dir(&wrapper), &world.app, &manifest, &opts).expect("generate");
        let cargo = fs::read_to_string(wrapper.join("Cargo.toml")).expect("read Cargo.toml");
        let parsed: toml::Value = toml::from_str(&cargo).expect("valid TOML");
        let patch = parsed.get("patch").unwrap_or_else(|| panic!("no [patch] in Workspace mode:\n{cargo}"));
        let entry = patch
            .get("idealyst")
            .and_then(|r| r.get("host-win32"))
            .unwrap_or_else(|| panic!("registry framework deps not redirected:\n{cargo}"));
        assert!(entry.get("path").is_some(), "host-win32 must patch to a path; got {entry:?}");
    }

    /// A regenerated (shorter) wrapper file must parse as EXACTLY the
    /// new content — no stale tail of a previous version. The broken
    /// share behavior itself can't be reproduced in a unit test; this
    /// pins the contract that survives it: content round-trips, every
    /// version is padded to the same `PAD_BLOCK` multiple (so there is
    /// no beyond-EOF region to resurrect), and the padding is pure
    /// comment lines (so a resurrected padding region is inert).
    #[test]
    fn regression_shorter_rewrite_leaves_no_stale_tail() {
        let dir = std::env::temp_dir().join("idealyst-build-windows-test-rewrite");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let f = dir.join("Cargo.toml");
        write_replacing(&f, "a much longer first version of the file\n", "#").unwrap();
        let first_len = fs::read_to_string(&f).unwrap().len();
        write_replacing(&f, "short\n", "#").unwrap();
        let back = fs::read_to_string(&f).unwrap();
        assert!(back.starts_with("short\n#"), "content then comment padding: {back:?}");
        assert_eq!(back.len() % PAD_BLOCK, 0, "padded to a block multiple");
        assert_eq!(back.len(), first_len, "same-block versions are byte-stable in length");
        for line in back.lines().skip(1) {
            assert!(line.starts_with('#'), "padding must be pure comments: {line:?}");
        }
        // Unchanged content skips the rewrite (mtime stays put).
        let before = fs::metadata(&f).unwrap().modified().unwrap();
        write_replacing(&f, "short\n", "#").unwrap();
        assert_eq!(fs::metadata(&f).unwrap().modified().unwrap(), before, "identical write skipped");
        let _ = fs::remove_dir_all(&dir);
    }

    /// The target path must survive the forward-slash TOML rewrite in
    /// `write_target_config` (backslashes are invalid TOML escapes — the
    /// wrapper Cargo config gotcha).
    #[test]
    fn win32_bucket_config_is_valid_toml_path() {
        let dir = std::env::temp_dir().join("idealyst-build-windows-test-cfg");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let bucket = PathBuf::from(r"Z:\proj\target\idealyst\demo\windows\target");
        write_target_config(&dir, &bucket, host_cross_triple()).unwrap();
        let cfg = fs::read_to_string(dir.join(".cargo/config.toml")).unwrap();
        assert!(cfg.contains("target-dir = \"Z:/proj/target/idealyst/demo/windows/target\""), "got:\n{cfg}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// `asset!("images/logo.png")` reads `<exe dir>/assets/images/logo.png`.
    #[test]
    fn windows_binary_ships_assets_beside_exe() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        std::fs::create_dir_all(project.join("assets/images")).unwrap();
        std::fs::write(project.join("assets/images/logo.png"), b"png").unwrap();
        let binary = tmp.path().join("target/debug/app-windows.exe");
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, b"bin").unwrap();
        super::ship_assets(&project, &binary).unwrap();
        assert!(tmp.path().join("target/debug/assets/images/logo.png").is_file());
    }
}
