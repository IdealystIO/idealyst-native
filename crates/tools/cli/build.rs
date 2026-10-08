//! Bake compile-time git defaults into the `idealyst` binary.
//!
//! The CLI scaffolds wrapper Cargo.tomls that depend on the framework
//! crates. When the project lives outside the framework workspace
//! (the `cargo install idealyst-cli` case), those deps need a git
//! URL + refspec. We capture both at *CLI* compile time so a given
//! installed binary always scaffolds projects pinned to the framework
//! commit it was built against — predictable, reproducible, no
//! "default branch moved underneath me" surprises.
//!
//! Refspec preference, in order:
//! 1. `IDEALYST_FRAMEWORK_GIT_TAG` env var → `tag = "<value>"`
//! 2. `IDEALYST_FRAMEWORK_GIT_REV` env var → `rev = "<value>"`
//! 3. Git tag at HEAD (from `git describe --tags --exact-match HEAD`)
//!    → `tag = "<value>"`. Preferred over rev because tags are
//!    human-readable and stable.
//! 4. Git commit SHA → `rev = "<value>"`.
//! 5. `v<CARGO_PKG_VERSION>` as a final fallback (used in source
//!    tarballs where `.git/` isn't present).
//!
//! URL is overridable via `IDEALYST_FRAMEWORK_GIT_URL` at both build
//! and runtime; defaults to the public idealyst-native repo.

use std::path::{Path, PathBuf};
use std::process::Command;

const DEFAULT_URL: &str = "https://github.com/IdealystIO/idealyst-native";


fn main() {
    // Watch the reflog — it appends on every HEAD movement (commit,
    // amend, reset, checkout, merge, rebase, …). The previous list
    // — `HEAD`, `index`, `refs/tags` — never updated on a regular
    // commit (commits move `refs/heads/<branch>`, not `HEAD`
    // itself), so build.rs's baked SHA went stale until a
    // `cargo install --force` blew the cache. Watching the reflog
    // fixes that: every `git commit` mutates `logs/HEAD`, cargo
    // re-runs build.rs, the new SHA gets baked.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/logs/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/tags");
    println!("cargo:rerun-if-env-changed=IDEALYST_FRAMEWORK_GIT_URL");
    println!("cargo:rerun-if-env-changed=IDEALYST_FRAMEWORK_GIT_REV");
    println!("cargo:rerun-if-env-changed=IDEALYST_FRAMEWORK_GIT_TAG");

    let url = std::env::var("IDEALYST_FRAMEWORK_GIT_URL")
        .unwrap_or_else(|_| DEFAULT_URL.to_string());

    let (kind, value) = resolve_refspec();

    println!("cargo:rustc-env=IDEALYST_FRAMEWORK_GIT_URL_DEFAULT={}", url);
    // Two env consts so the runtime can pick the right TOML key.
    // `KIND` is one of `rev`, `tag`, `branch` (the cargo dep-table
    // key). `VALUE` is the corresponding string.
    println!("cargo:rustc-env=IDEALYST_FRAMEWORK_GIT_REF_KIND_DEFAULT={}", kind);
    println!("cargo:rustc-env=IDEALYST_FRAMEWORK_GIT_REF_VALUE_DEFAULT={}", value);
    // Legacy compat: keep the old `_REV_` constant pointing at
    // whatever value we picked, so older builds that imported it
    // don't break mid-upgrade. New code reads the KIND + VALUE pair.
    println!("cargo:rustc-env=IDEALYST_FRAMEWORK_GIT_REV_DEFAULT={}", value);

    // --- Registry defaults --------------------------------------------
    //
    // What a freshly-scaffolded project pins the framework to. Git stays
    // available for forks and for projects that predate the registry, but
    // a version-keyed dep is the default: cargo reuses the compiled
    // artifact of any crate whose version did not move, which a git pin
    // makes impossible (its source id carries the commit, so every crate
    // in the graph gets a new PackageId on every bump).
    //
    // Each framework crate is pinned to ITS OWN version (they release
    // independently: backend-web is on 2.x beside runtime-world 1.8), read
    // at run time from the framework's `[workspace.dependencies]` — see
    // `FRAMEWORK_MANIFEST` below.
    println!("cargo:rerun-if-env-changed=IDEALYST_REGISTRY_NAME");
    println!("cargo:rerun-if-env-changed=IDEALYST_REGISTRY_INDEX");

    let reg_name = std::env::var("IDEALYST_REGISTRY_NAME")
        .unwrap_or_else(|_| "idealyst".to_string());
    let reg_index = std::env::var("IDEALYST_REGISTRY_INDEX")
        .unwrap_or_else(|_| "sparse+https://crates.idealyst.io/index/".to_string());
    println!("cargo:rustc-env=IDEALYST_REGISTRY_NAME_DEFAULT={}", reg_name);
    println!("cargo:rustc-env=IDEALYST_REGISTRY_INDEX_DEFAULT={}", reg_index);

    // The triple this binary is built for. `idealyst update` passes it as
    // `--target` when rustc's default host differs, so an arm64 binary
    // never updates itself into an x86_64 one under a Rosetta toolchain.
    println!(
        "cargo:rustc-env=IDEALYST_CLI_TARGET={}",
        std::env::var("TARGET").expect("cargo sets TARGET for build scripts"),
    );

    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let assets = manifest_dir.join(PACKAGE_ASSETS);
    let workspace = workspace_root(&manifest_dir);
    let export = std::env::var_os(EXPORT_ENV).is_some_and(|v| !v.is_empty());
    println!("cargo:rerun-if-env-changed={EXPORT_ENV}");
    println!("cargo:rerun-if-changed={}", assets.display());
    if export && workspace.is_none() {
        panic!("{EXPORT_ENV} is set, but there is no workspace to export package assets from");
    }

    let welcome = match &workspace {
        Some(root) => root.join("examples/welcome"),
        None => assets.join("welcome"),
    };
    if !welcome.join("src/lib.rs").is_file() {
        panic!(
            "no welcome scaffold at {}: a packaged idealyst-cli must carry {PACKAGE_ASSETS}/welcome \
             (the release's prepackage step writes it)",
            welcome.display(),
        );
    }
    println!("cargo:rustc-env=IDEALYST_WELCOME_DIR={}", welcome.display());

    // The framework root manifest, whose `[workspace.dependencies]` give
    // every framework crate's version. `framework_source.rs` embeds it with
    // `include_str!`, so an edit to it recompiles the CLI without
    // re-running this script (and its Inspector build).
    let framework_manifest = match &workspace {
        Some(root) => root.join("Cargo.toml"),
        None => assets.join(FRAMEWORK_MANIFEST),
    };
    if !framework_manifest.is_file() {
        panic!(
            "no framework manifest at {}: a packaged idealyst-cli must carry {PACKAGE_ASSETS}/{FRAMEWORK_MANIFEST}",
            framework_manifest.display(),
        );
    }
    println!("cargo:rustc-env=IDEALYST_FRAMEWORK_MANIFEST={}", framework_manifest.display());

    let inspector = embed_inspector(workspace.as_deref(), &assets.join("inspector"), export);

    if export {
        export_package_assets(&assets, &welcome, inspector.as_deref(), &framework_manifest);
    }
}

// --- Package assets --------------------------------------------------------
//
// Three things the binary embeds live OUTSIDE this crate in the workspace:
// the `idealyst new` scaffold (`examples/welcome`, which is also a runnable
// example and the scaffold's single source of truth), the Inspector front
// end (built from `examples/inspector`), and the framework root
// `Cargo.toml` (its `[workspace.dependencies]` are the per-crate versions
// a registry project is pinned to). A crate published to the registry
// carries only its own directory, so none would exist in a registry install.
//
// The fix keeps ONE source of truth: in the workspace (dev builds, `cargo
// install --git`) everything is read from the workspace exactly as
// before. The release copies them into `package-assets/` (gitignored,
// listed in `include`) just before `cargo package`, through the registry
// tool's `prepackage` hook, which runs a build with `EXPORT_ENV` set. A
// registry install has no workspace around it, so it reads `package-assets/`.
//
// The workspace always wins when it is present. Preferring a populated
// `package-assets/` would make every dev build after a release embed the
// stale copy from the last release.

const PACKAGE_ASSETS: &str = "package-assets";
const EXPORT_ENV: &str = "IDEALYST_CLI_EXPORT_PACKAGE_ASSETS";
/// The framework root `Cargo.toml`, as staged into `package-assets/`.
const FRAMEWORK_MANIFEST: &str = "framework-Cargo.toml";

/// The framework workspace this crate is building inside, if any.
///
/// A registry install unpacks to `~/.cargo/registry/src/<index>/idealyst-cli-<v>/`,
/// where nothing three levels up looks like the workspace. `cargo package`'s
/// verify build unpacks to `<target>/package/idealyst-cli-<v>/`, which DOES
/// sit three levels under the workspace when the target dir is the default.
fn workspace_root(manifest_dir: &Path) -> Option<PathBuf> {
    let root = manifest_dir.join("../../..").canonicalize().ok()?;
    let is_framework = root.join("crates/runtime/core/Cargo.toml").is_file()
        && root.join("examples/welcome/src/lib.rs").is_file();
    is_framework.then_some(root)
}

/// Copy the welcome scaffold's `src/` + `fonts/`, the built Inspector
/// bundle and the framework root manifest into `package-assets/`,
/// replacing whatever was there.
fn export_package_assets(assets: &Path, welcome: &Path, inspector: Option<&Path>, manifest: &Path) {
    let inspector = inspector.unwrap_or_else(|| {
        panic!("{EXPORT_ENV}: the Inspector front end did not build, so there is no bundle to package")
    });
    let _ = std::fs::remove_dir_all(assets);
    for sub in ["src", "fonts"] {
        copy_dir(&welcome.join(sub), &assets.join("welcome").join(sub))
            .unwrap_or_else(|e| panic!("{EXPORT_ENV}: copying welcome/{sub}: {e}"));
    }
    copy_dir(inspector, &assets.join("inspector"))
        .unwrap_or_else(|e| panic!("{EXPORT_ENV}: copying the Inspector bundle: {e}"));
    std::fs::copy(manifest, assets.join(FRAMEWORK_MANIFEST))
        .unwrap_or_else(|e| panic!("{EXPORT_ENV}: copying the framework manifest: {e}"));
    println!("cargo:warning=exported package assets to {}", assets.display());
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let path = entry?.path();
        let dest = to.join(path.file_name().expect("read_dir entries have names"));
        if path.is_dir() {
            copy_dir(&path, &dest)?;
        } else if path.file_name().is_some_and(|n| n != ".DS_Store") {
            std::fs::copy(&path, &dest)?;
        }
    }
    Ok(())
}

// --- The Inspector front end ---------------------------------------------
//
// `idealyst inspect` serves the Inspector's web build from the binary, so
// an installed CLI needs nothing else on disk. We build it HERE, with the
// same pipeline `idealyst build --web --release` runs (`build_web::build`),
// into OUT_DIR, and generate `inspector_bundle.rs`: a `FILES` table of
// `include_bytes!` entries.
//
// In the workspace the Inspector's source is beside this crate and is
// built here. Building it needs the wasm toolchain (wasm32-unknown-unknown,
// wasm-opt — and wasm-bindgen: the Inspector still uses web-sys, so it
// builds in hybrid mode). A missing tool must not make the CLI
// uninstallable: the failure becomes a build warning plus `SKIPPED`, which
// `idealyst inspect` reports when it starts. A registry install embeds the
// bundle the release prebuilt into `package-assets/inspector` instead, so
// it needs no wasm toolchain at all.
//
// `IDEALYST_CLI_SKIP_INSPECTOR=1` skips the build outright, for CLI work
// that doesn't touch the Inspector: every framework crate the front end
// depends on is watched below, so without it a runtime edit re-runs the
// (incremental) wasm build on the next CLI build.

/// Generate `inspector_bundle.rs` and return the bundle directory it
/// embeds, or `None` when the Inspector was skipped.
fn embed_inspector(workspace: Option<&Path>, prebuilt: &Path, export: bool) -> Option<PathBuf> {
    println!("cargo:rerun-if-env-changed=IDEALYST_CLI_SKIP_INSPECTOR");
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let skip = std::env::var_os("IDEALYST_CLI_SKIP_INSPECTOR").is_some_and(|v| !v.is_empty());
    let result = match workspace {
        _ if skip && export => panic!("{EXPORT_ENV} and IDEALYST_CLI_SKIP_INSPECTOR are both set"),
        _ if skip => Err("IDEALYST_CLI_SKIP_INSPECTOR is set".to_string()),
        Some(root) => build_inspector(root, &out_dir),
        None if prebuilt.join("index.html").is_file() => Ok(prebuilt.to_path_buf()),
        None => Err(format!("this package carries no prebuilt bundle at {}", prebuilt.display())),
    };
    let bundle = match result {
        Ok(dir) => {
            let mut files = Vec::new();
            match collect(&dir, &dir, &mut files) {
                Ok(()) if files.iter().any(|(rel, _)| rel == "index.html") => {
                    files.sort();
                    Ok((dir, files))
                }
                Ok(()) => Err(format!("{} has no index.html", dir.display())),
                Err(e) => Err(format!("reading {}: {e}", dir.display())),
            }
        }
        Err(why) => Err(why),
    };
    let mut gen = String::from("pub static FILES: &[(&str, &[u8])] = &[\n");
    let (skipped, dir) = match bundle {
        Ok((dir, files)) => {
            for (rel, abs) in &files {
                gen.push_str(&format!("    ({rel:?}, include_bytes!({:?})),\n", abs.display().to_string()));
            }
            ("None".to_string(), Some(dir))
        }
        Err(why) => {
            println!("cargo:warning=`idealyst inspect` will serve no page: the Inspector front end was not built ({why})");
            (format!("Some({why:?})"), None)
        }
    };
    gen.push_str("];\n");
    gen.push_str(&format!("pub static SKIPPED: Option<&str> = {skipped};\n"));
    std::fs::write(out_dir.join("inspector_bundle.rs"), gen).expect("write inspector_bundle.rs");
    dir
}

fn build_inspector(root: &Path, out_dir: &Path) -> Result<PathBuf, String> {
    let root = root.to_path_buf();
    let project = root.join("examples/inspector");
    if !project.join("Cargo.toml").is_file() {
        return Err(format!("no Inspector source at {}", project.display()));
    }
    watch_local_deps(&project)?;

    // The build script's environment describes THIS (host) build; the
    // nested wasm build must not inherit it. Cargo hands build scripts
    // the host's encoded RUSTFLAGS (which build-web would fold into the
    // wasm flags), and `cargo clippy` a workspace wrapper.
    std::env::remove_var("CARGO_ENCODED_RUSTFLAGS");
    std::env::remove_var("RUSTC_WORKSPACE_WRAPPER");

    let bundle = out_dir.join("inspector-web");
    let _ = std::fs::remove_dir_all(&bundle);
    build_web::build(
        &project,
        build_web::BuildOptions {
            release: true,
            source: build_ios::FrameworkSource::Workspace { root },
            premint_only: false,
            premint_report: false,
            debuginfo: build_web::DebugInfo::default(),
            dev_opt: build_web::DevOpt::default(),
            primitives: None,
            user_features: Vec::new(),
            bundle_out_dir: Some(bundle.clone()),
            robot_relay_url: None,
            head_script: None,
            runtime_server_url: None,
            hot_patch: false,
            gzip: false,
            // The server serves originals; `.br` siblings would only
            // double the embedded bytes.
            brotli: false,
            strip_panics: false,
            hydrate: false,
            prune_dead_data_min: None,
            premint: false,
            // One module: nothing in the Inspector is lazy, and it keeps
            // the embedded file set small.
            wasm_split: false,
            reporter: dev_events::Reporter::plain_stderr(),
        },
    )
    .map_err(|e| format!("{e:#}"))?;

    Ok(bundle)
}

fn collect(base: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect(base, &path, out)?;
        } else {
            let rel = path.strip_prefix(base).expect("under base").to_string_lossy().replace('\\', "/");
            out.push((rel, path));
        }
    }
    Ok(())
}

/// Re-run when any workspace crate the Inspector compiles against
/// changes: its local dependency closure for wasm32, from `cargo
/// metadata`, each watched as a directory.
fn watch_local_deps(project: &Path) -> Result<(), String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let out = Command::new(cargo)
        .current_dir(project)
        .args(["metadata", "--format-version", "1", "--filter-platform", "wasm32-unknown-unknown"])
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!("cargo metadata: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let meta: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata output: {e}"))?;
    let local: std::collections::HashMap<&str, PathBuf> = meta["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["source"].is_null())
        .filter_map(|p| {
            let dir = Path::new(p["manifest_path"].as_str()?).parent()?.to_path_buf();
            Some((p["id"].as_str()?, dir))
        })
        .collect();
    let nodes: std::collections::HashMap<&str, Vec<&str>> = meta["resolve"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| {
            let deps = n["dependencies"].as_array()?.iter().filter_map(|d| d.as_str()).collect();
            Some((n["id"].as_str()?, deps))
        })
        .collect();
    let start = local
        .iter()
        .find(|(_, dir)| dir.as_path() == project)
        .map(|(id, _)| *id)
        .ok_or("cargo metadata doesn't list the Inspector")?;
    let mut seen = std::collections::HashSet::new();
    let mut stack = vec![start];
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        if let Some(dir) = local.get(id) {
            println!("cargo:rerun-if-changed={}", dir.join("src").display());
            println!("cargo:rerun-if-changed={}", dir.join("Cargo.toml").display());
            for dep in nodes.get(id).into_iter().flatten() {
                if local.contains_key(dep) {
                    stack.push(dep);
                }
            }
        }
    }
    Ok(())
}

/// Returns `(refspec_kind, refspec_value)` where `refspec_kind` is
/// `"rev"`, `"tag"`, or `"branch"`.
fn resolve_refspec() -> (&'static str, String) {
    if let Ok(tag) = std::env::var("IDEALYST_FRAMEWORK_GIT_TAG") {
        if !tag.is_empty() {
            return ("tag", tag);
        }
    }
    if let Ok(rev) = std::env::var("IDEALYST_FRAMEWORK_GIT_REV") {
        if !rev.is_empty() {
            return ("rev", rev);
        }
    }
    if let Some(tag) = git_head_tag() {
        return ("tag", tag);
    }
    if let Some(sha) = git_head_sha() {
        return ("rev", sha);
    }
    (
        "tag",
        format!(
            "v{}",
            std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.1".into()),
        ),
    )
}

/// Tag at exact HEAD, if any. `git describe --tags --exact-match HEAD`
/// errors when HEAD isn't tagged; we treat that as "no tag" and
/// return None.
fn git_head_tag() -> Option<String> {
    let out = Command::new("git")
        .args(["describe", "--tags", "--exact-match", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let tag = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if tag.is_empty() { None } else { Some(tag) }
}

fn git_head_sha() -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if sha.is_empty() { None } else { Some(sha) }
}
