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
}

#[derive(Debug)]
pub struct BuildArtifact {
    /// Path to the produced Windows `.exe` (ready to spawn).
    pub binary: PathBuf,
    /// Wrapper crate directory (useful for debugging the template).
    pub wrapper_dir: PathBuf,
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

    let profile = if opts.release { "release" } else { "debug" };
    let bin_name = binary_name(&manifest.name);
    cargo_build(&wrapper_dir, &cargo_target_dir, opts.release, &opts.user_features)?;

    // cargo emits `<bin_name>.exe` on the windows target.
    let binary = cargo_target_dir.join(profile).join(format!("{bin_name}.exe"));
    if !binary.is_file() {
        anyhow::bail!(
            "cargo build reported success but Windows binary not at {}",
            binary.display(),
        );
    }
    Ok(BuildArtifact { binary, wrapper_dir })
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
    let user_dep = format!("{{ path = \"{}\" }}", project_dir.display());

    let bundle_id = manifest
        .app
        .bundle_id
        .clone()
        .unwrap_or_else(|| format!("com.example.{}", manifest.name));

    let deps_block = format!(
        "host-win32 = {host_dep}\n\
         runtime-core = {fcore_dep}\n\
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
dev = ["runtime-core/dev"]
"#,
    );

    let main_rs = main_rs(&manifest.lib_name, &manifest.app.name, &bundle_id, &bin_name);

    write_target_config(wrapper_dir, cargo_target_dir)?;
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

fn main_rs(user_lib: &str, app_name: &str, bundle_id: &str, bin_name: &str) -> String {
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
        ::runtime_core::robot::bridge::set_app_identity(
            ::runtime_core::robot::bridge::AppIdentity {{
                name: "{app_name}".to_string(),
                bundle_id: Some("{bundle_id}".to_string()),
                project_root: ::std::option::Option::None,
            }},
        );
    }}

    let opts = host_win32::RunOptions {{
        title: "{app_name}".to_string(),
        width: 1024,
        height: 768,
    }};
    // `run_with` (not `run`) so the user crate's
    // `pub fn register_scene_extensions(&mut Registry<_>)` seam runs after
    // `register_builtins` — how SDK payload handlers get into the scene
    // registry. Same seam as the GTK / macOS wrappers; `register_extensions`
    // was the pre-v2 name and took the backend. Returns the exit code.
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
fn write_target_config(dir: &Path, target_dir: &Path) -> Result<()> {
    let config = build_ios::private_target_config("idealyst build --windows", target_dir);
    fs::create_dir_all(dir.join(".cargo"))?;
    write_replacing(&dir.join(".cargo/config.toml"), &config, "#")?;
    Ok(())
}

/// Build the wrapper into `target_dir`, then prune that dir to the units
/// its recorded variants use (`build_ios::build_wrapper`).
fn cargo_build(wrapper_dir: &Path, target_dir: &Path, release: bool, user_features: &[String]) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.args(["build"]);
    if release {
        cmd.arg("--release");
    }
    if !user_features.is_empty() {
        cmd.arg("--features").arg(user_features.join(","));
    }
    eprintln!(
        "[build-windows] cargo build{}{} (in {})",
        if release { " --release" } else { "" },
        if user_features.is_empty() {
            String::new()
        } else {
            format!(" --features {}", user_features.join(","))
        },
        wrapper_dir.display(),
    );
    let spec = build_ios::target_gc::BuildSpec::wrapper(target_dir, None, release, user_features);
    build_ios::build_wrapper("build-windows", wrapper_dir, &mut cmd, &spec)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Regression (CrewForge, 2026-10-08, the iOS wrapper's twin): the
    /// Windows wrapper deleted its `Cargo.lock` on every generation, so
    /// each run resolved the registry's newest versions instead of the
    /// app's locked ones — a different graph from the app's and a new
    /// generation of the tree per framework release.
    #[test]
    fn regression_windows_wrapper_lock_reresolved_every_run() {
        let world = build_ios::test_support::LockWorld::new(&[("crates/host/win32", "host-win32", &[])], &[]);
        let wrapper = world.wrapper_dir("windows");
        let manifest = build_ios::parse_manifest(&world.app).expect("parse manifest");
        let opts = BuildOptions { release: false, user_features: Vec::new(), source: world.source() };
        let generate = || {
            generate_wrapper(&wrapper, &windows_target_dir(&wrapper), &world.app, &manifest, &opts)
                .expect("generate wrapper")
        };
        world.assert_wrapper_follows_app_lock(&wrapper, generate);
        world.assert_private_target_dir(&wrapper, &world.source().cargo_target_dir(&world.app));
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
        write_target_config(&dir, &bucket).unwrap();
        let cfg = fs::read_to_string(dir.join(".cargo/config.toml")).unwrap();
        assert!(cfg.contains("target-dir = \"Z:/proj/target/idealyst/demo/windows/target\""), "got:\n{cfg}");
        let _ = fs::remove_dir_all(&dir);
    }
}
