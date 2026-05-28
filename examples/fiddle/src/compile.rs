//! Cargo / wasm-pack orchestration for the `POST /compile` handler.
//!
//! Compilation is serialized through a process-wide `Mutex` because
//! cargo writes to a single `target/` directory; concurrent
//! invocations would step on each other. The mutex is fine for a
//! single-tenant dev fiddle — for a real multi-user deployment
//! you'd swap this for a per-user worker pool with isolated target
//! dirs.
//!
//! Project model (v2): the request carries a `files` map of
//! `<path-under-src/>` → contents. Each path is written into
//! `template/src/snippet/<path>` — the snippet sub-module the
//! template wraps. The user's entry file is conventionally
//! `lib.rs`; the server quietly renames it to `mod.rs` on disk so
//! Rust's submodule resolution picks it up under `mod snippet;`.
//!
//! Output cache: results are keyed by sha256 over the canonical
//! sorted (path, contents) list. A repeat compile of the same
//! project tree returns the cached hash without re-invoking cargo.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Single, global compile lock. Held for the duration of a build.
static COMPILE_LOCK: Mutex<()> = Mutex::new(());

/// Outcome of a compile request, JSON-encoded into `POST /compile`'s
/// response body. `hash` is the cache-dir basename — the browser
/// loads `/compiled/<hash>/index.html` in the simulator iframe.
pub struct CompileOk {
    pub hash: String,
}

/// Output mode the user picked in the editor. Picks which cargo
/// feature the template builds against — see
/// `template/src/lib.rs` for what each mode does.
#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// wgpu canvas + host-web + iOS skin. The default; matches what
    /// the docs Simulator embeds.
    Simulator,
    /// Plain DOM mount via backend-web. Native `<div>` / `<button>` /
    /// `<input>` rendering — the snippet as a real web app.
    Web,
}

impl Default for Mode {
    fn default() -> Self {
        Self::Simulator
    }
}

impl Mode {
    fn feature(self) -> &'static str {
        match self {
            Mode::Simulator => "simulator",
            Mode::Web => "web",
        }
    }
    fn tag(self) -> &'static str {
        match self {
            Mode::Simulator => "sim",
            Mode::Web => "web",
        }
    }
}

/// Conventional entry path the editor uses for the project's root
/// file. Mapped to `snippet/mod.rs` on disk so Rust's `mod snippet;`
/// declaration picks it up; the user never has to care about the
/// `mod.rs` convention.
const ENTRY_PATH: &str = "lib.rs";

/// Compile a multi-file project by writing each file into the
/// template's `src/snippet/` directory (replacing any previous
/// snippet tree), invoking wasm-pack with the right feature flag,
/// and materializing the resulting bundle into `compiled/<hash>/`.
///
/// `files` keys are paths relative to the user's logical `src/`
/// directory. Each must be a relative `.rs` path with no `..`
/// components; non-`.rs` files are rejected. The map must contain
/// the entry file at [`ENTRY_PATH`] (`"lib.rs"`), or the snippet
/// has no `app()` to call.
///
/// The hash includes the mode AND the latest mtime of any
/// "upstream" workspace crate the snippet links against, so editing
/// upstream framework code automatically invalidates the cache.
pub fn compile(
    files: &BTreeMap<String, String>,
    mode: Mode,
    fiddle_root: &Path,
) -> Result<CompileOk> {
    validate_files(files)?;

    let upstream_mtime = upstream_max_mtime(fiddle_root);
    let hash = files_hash(files, mode, upstream_mtime);
    let cache_dir = fiddle_root.join("compiled").join(&hash);

    if cache_dir.join("index.html").exists() {
        return Ok(CompileOk { hash });
    }

    let _guard = COMPILE_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("compile lock poisoned"))?;

    if cache_dir.join("index.html").exists() {
        return Ok(CompileOk { hash });
    }

    let template_dir = fiddle_root.join("template");
    let snippet_dir = template_dir.join("src/snippet");

    // Wipe the previous snippet tree before writing the new one.
    // Files the user deleted between runs would otherwise linger
    // and either silently break orphan `mod foo;` declarations or
    // be picked up by their old contents. `remove_dir_all` is a
    // no-op when the dir doesn't exist yet (first compile).
    if snippet_dir.exists() {
        fs::remove_dir_all(&snippet_dir)
            .with_context(|| format!("clearing {}", snippet_dir.display()))?;
    }
    fs::create_dir_all(&snippet_dir)
        .with_context(|| format!("creating {}", snippet_dir.display()))?;

    for (rel_path, contents) in files {
        let on_disk = disk_path_for(&snippet_dir, rel_path);
        if let Some(parent) = on_disk.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("creating parent dir {}", parent.display())
            })?;
        }
        fs::write(&on_disk, wrap_with_prelude(contents))
            .with_context(|| format!("writing {}", on_disk.display()))?;
    }

    // Snippet.rs (the legacy single-file path) is a stale leftover
    // from the v1 compile worker. Delete it if it's there — `mod
    // snippet;` resolves to the directory we just wrote.
    let legacy = template_dir.join("src/snippet.rs");
    if legacy.exists() {
        let _ = fs::remove_file(&legacy);
    }

    fs::create_dir_all(&cache_dir)
        .with_context(|| format!("creating cache dir {}", cache_dir.display()))?;
    let pkg_dir = cache_dir.join("pkg");

    let output = Command::new("wasm-pack")
        .arg("build")
        .arg(&template_dir)
        .args(["--target", "web", "--dev", "--out-name", "snippet"])
        .arg("--out-dir")
        .arg(&pkg_dir)
        .arg("--")
        .args(["--no-default-features", "--features", mode.feature()])
        .output()
        .context("invoking wasm-pack — is it on PATH? (`cargo install wasm-pack`)")?;
    if !output.status.success() {
        let _ = fs::remove_dir_all(&cache_dir);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let combined = if stderr.trim().is_empty() {
            stdout.into_owned()
        } else if stdout.trim().is_empty() {
            stderr.into_owned()
        } else {
            format!("{stderr}\n---\n{stdout}")
        };
        bail!("wasm-pack failed ({}):\n{}", output.status, combined.trim_end());
    }

    let mut index = fs::File::create(cache_dir.join("index.html"))
        .with_context(|| format!("writing index.html in {}", cache_dir.display()))?;
    index.write_all(IFRAME_SHELL.as_bytes())?;

    Ok(CompileOk { hash })
}

/// Map an editor-side `<rel_path>` (relative to the user's logical
/// `src/`) to the on-disk path under `template/src/snippet/`. The
/// only rewrite is the entry file: `lib.rs` becomes `mod.rs` so the
/// template's `mod snippet;` declaration finds it.
fn disk_path_for(snippet_dir: &Path, rel_path: &str) -> PathBuf {
    if rel_path == ENTRY_PATH {
        snippet_dir.join("mod.rs")
    } else {
        snippet_dir.join(rel_path)
    }
}

/// Reject paths that could escape `snippet/` or aren't Rust source.
/// Cheap whitelist — anything outside `.rs` files at relative paths
/// without `..` components gets a 400 before we touch the disk.
fn validate_files(files: &BTreeMap<String, String>) -> Result<()> {
    if !files.contains_key(ENTRY_PATH) {
        bail!(
            "project missing entry file `{ENTRY_PATH}` — every snippet \
             needs a top-level `lib.rs` that defines `pub fn app() -> Element`."
        );
    }
    for rel in files.keys() {
        if rel.is_empty() {
            bail!("empty path in files map");
        }
        if rel.starts_with('/') || rel.contains('\\') {
            bail!("invalid path {rel:?} — paths must be relative POSIX-style");
        }
        for seg in rel.split('/') {
            if seg.is_empty() || seg == "." || seg == ".." {
                bail!("invalid path {rel:?} — `..` / empty segments not allowed");
            }
        }
        if !rel.ends_with(".rs") {
            bail!("invalid path {rel:?} — only `.rs` files are supported");
        }
    }
    Ok(())
}

/// Inject the snippet runtime prelude at the top of each user
/// file. Same prelude as v1 (`use crate::__rt::*;`); applying it
/// uniformly across every `.rs` means sibling files can use the
/// re-exported framework types without their own boilerplate.
fn wrap_with_prelude(contents: &str) -> String {
    format!(
        "//! Auto-generated per /compile request — overwritten on every build.\n\
         \n\
         #![allow(unused_imports)]\n\
         #![allow(dead_code)]\n\
         \n\
         use crate::__rt::*;\n\
         \n\
         {contents}\n"
    )
}

/// Canonical hash over the project tree. We sort by path inside the
/// `BTreeMap` iterator order (already sorted by key), then digest
/// each (path, contents) pair with a delimiter so collisions
/// across "ab|cd" vs. "a|bcd" are impossible.
fn files_hash(
    files: &BTreeMap<String, String>,
    mode: Mode,
    upstream_mtime: u64,
) -> String {
    let mut h = Sha256::new();
    for (path, contents) in files {
        h.update(path.as_bytes());
        h.update(b"\0");
        h.update(contents.as_bytes());
        h.update(b"\x1e"); // ASCII record separator
    }
    h.update(b"\0");
    h.update(upstream_mtime.to_le_bytes());
    let digest = h.finalize();
    let mut s = String::with_capacity(20);
    for byte in &digest[..8] {
        s.push_str(&format!("{byte:02x}"));
    }
    s.push('-');
    s.push_str(mode.tag());
    s
}

fn upstream_max_mtime(fiddle_root: &Path) -> u64 {
    let Some(workspace_root) = workspace_root_from(fiddle_root) else {
        return 0;
    };
    let watched: &[&str] = &[
        "crates/framework/core/src",
        "crates/framework/theme/src",
        "crates/ui/idea-ui/src",
        "crates/backend/web/src",
        "crates/host/web/src",
        "crates/render/wgpu/src",
        "crates/render/api/src",
        "crates/skin/ios-sim/src",
        "Cargo.lock",
        "examples/fiddle/template/src/lib.rs",
        "examples/fiddle/template/Cargo.toml",
    ];
    let mut max = 0u64;
    for rel in watched {
        scan_max_mtime(&workspace_root.join(rel), &mut max);
    }
    max
}

fn scan_max_mtime(path: &Path, max: &mut u64) {
    let Ok(meta) = std::fs::metadata(path) else { return };
    if let Ok(mt) = meta.modified() {
        if let Ok(d) = mt.duration_since(std::time::UNIX_EPOCH) {
            *max = (*max).max(d.as_secs());
        }
    }
    if meta.is_dir() {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                scan_max_mtime(&entry.path(), max);
            }
        }
    }
}

fn workspace_root_from(fiddle_root: &Path) -> Option<PathBuf> {
    fiddle_root.parent()?.parent().map(|p| p.to_path_buf())
}

pub fn fiddle_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

const IFRAME_SHELL: &str = r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Idealyst snippet</title>
    <style>
      html, body, #app { height: 100%; margin: 0; }
      body { background: transparent; }
    </style>
  </head>
  <body>
    <div id="app"></div>
    <script type="module">
      import init from "./pkg/snippet.js";
      init();
    </script>
  </body>
</html>
"#;
