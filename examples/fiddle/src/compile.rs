//! Cargo / wasm-pack orchestration for the `POST /compile` handler.
//!
//! Compilation is serialized through a process-wide `Mutex` because
//! cargo writes to a single `target/` directory; concurrent
//! invocations would step on each other. The mutex is fine for a
//! single-tenant dev fiddle — for a real multi-user deployment
//! you'd swap this for a per-user worker pool with isolated target
//! dirs.
//!
//! Output cache: results are keyed by `sha256(source)`. A repeat
//! compile of the same source returns the cached hash without
//! re-invoking cargo.

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
    /// The cargo feature name passed to `wasm-pack --features`.
    fn feature(self) -> &'static str {
        match self {
            Mode::Simulator => "simulator",
            Mode::Web => "web",
        }
    }

    /// Short tag baked into the cache-dir basename so simulator vs
    /// web builds of the same source land in distinct directories.
    fn tag(self) -> &'static str {
        match self {
            Mode::Simulator => "sim",
            Mode::Web => "web",
        }
    }
}

/// Compile a user snippet by writing it to the template crate's
/// `src/snippet.rs`, invoking wasm-pack with the right feature flag,
/// and materializing the resulting bundle into `compiled/<hash>/`.
/// Returns the hash on success (whether freshly built or served
/// from cache). The hash includes the mode AND the latest mtime of
/// any "upstream" workspace crate the snippet links against, so:
///
/// - editing the snippet → new hash → fresh build (always)
/// - flipping the mode → new hash → fresh build (always)
/// - editing render-wgpu / host-web / etc. → upstream mtime bumps
///   → new hash → fresh build (no manual `rm -rf compiled/`)
pub fn compile(source: &str, mode: Mode, fiddle_root: &Path) -> Result<CompileOk> {
    let upstream_mtime = upstream_max_mtime(fiddle_root);
    let hash = source_hash(source, mode, upstream_mtime);
    let cache_dir = fiddle_root.join("compiled").join(&hash);

    // Cache hit short-circuit. `index.html` is written last, so its
    // presence is a reliable "the bundle is complete" marker — a
    // half-finished previous run that crashed mid-`wasm-pack` won't
    // be mistaken for a hit.
    if cache_dir.join("index.html").exists() {
        return Ok(CompileOk { hash });
    }

    // Serialize compiles. The template's `target/` directory is
    // shared, so two cargo invocations against it would race.
    let _guard = COMPILE_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("compile lock poisoned"))?;

    // Recheck under the lock — two concurrent requests for the same
    // source would both miss above, then the first builds and the
    // second can short-circuit on the now-present cache entry.
    if cache_dir.join("index.html").exists() {
        return Ok(CompileOk { hash });
    }

    let template_dir = fiddle_root.join("template");
    let snippet_path = template_dir.join("src/snippet.rs");
    fs::write(&snippet_path, snippet_with_prelude(source))
        .with_context(|| format!("writing snippet to {}", snippet_path.display()))?;

    // wasm-pack handles the `cargo build --target wasm32-unknown-unknown`
    // + `wasm-bindgen` dance. `--target web` produces an ES module
    // we can load via `<script type="module">`. `--dev` skips
    // wasm-opt and uses dev profile so per-compile turnaround is
    // bounded by the snippet's rebuild, not LTO.
    fs::create_dir_all(&cache_dir)
        .with_context(|| format!("creating cache dir {}", cache_dir.display()))?;
    let pkg_dir = cache_dir.join("pkg");

    // `output()` (not `status()`) so we can capture stderr and ship
    // the actual rustc / wasm-pack messages back to the editor.
    // Without this, a missing `app()` or a borrow-check error looks
    // like "exit status 1" — useless for fixing the snippet.
    let output = Command::new("wasm-pack")
        .arg("build")
        .arg(&template_dir)
        .args(["--target", "web", "--dev", "--out-name", "snippet"])
        .arg("--out-dir")
        .arg(&pkg_dir)
        // wasm-pack 0.12 has no `--features` of its own; everything
        // after `--` is forwarded to `cargo build`. The template
        // declares no default feature, so picking exactly one here
        // satisfies the `compile_error!` guard in
        // `template/src/lib.rs`.
        .arg("--")
        .args(["--no-default-features", "--features", mode.feature()])
        .output()
        .context("invoking wasm-pack — is it on PATH? (`cargo install wasm-pack`)")?;
    if !output.status.success() {
        // Wipe the cache dir so a retry (same or different source)
        // doesn't see a half-built bundle. The check above
        // (`index.html` presence) wouldn't be fooled, but leaving
        // empty dirs around is noise.
        let _ = fs::remove_dir_all(&cache_dir);

        // wasm-pack prints its own status spam to stderr and
        // forwards cargo/rustc output unchanged. Returning the
        // raw stderr is the most useful thing: the editor can
        // surface the underlying compile error verbatim.
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Some wasm-pack failures (e.g. wasm-bindgen missing)
        // print on stdout; include both so we don't lose the
        // signal regardless of which stream it lands on.
        let combined = if stderr.trim().is_empty() {
            stdout.into_owned()
        } else if stdout.trim().is_empty() {
            stderr.into_owned()
        } else {
            format!("{stderr}\n---\n{stdout}")
        };
        bail!("wasm-pack failed ({}):\n{}", output.status, combined.trim_end());
    }

    // Iframe-loader shim. The wasm-pack output exposes
    // `init(): Promise<void>` as the default export. The iframe's
    // base URL is `/compiled/<hash>/`, so the `./pkg/snippet.js`
    // import resolves cleanly.
    let mut index = fs::File::create(cache_dir.join("index.html"))
        .with_context(|| format!("writing index.html in {}", cache_dir.display()))?;
    index.write_all(IFRAME_SHELL.as_bytes())?;

    Ok(CompileOk { hash })
}

/// Wrap the user's snippet with a `use` prelude + the entry-point
/// contract the template's `lib.rs` expects (`pub fn app() ->
/// framework_core::Primitive`). Users write the body of `app()`
/// and any helper functions / types they need; the prelude makes
/// the common imports ambient so tiny snippets stay tiny.
///
/// The framework crate names are re-exported under
/// `fiddle_template::__rt` so the user never has to think about
/// which crate any given symbol lives in. That's set up in
/// `template/src/lib.rs`.
fn snippet_with_prelude(source: &str) -> String {
    format!(
        "//! Auto-generated per /compile request. Do not edit by hand —\n\
         //! it's overwritten on every build.\n\
         \n\
         #![allow(unused_imports)]\n\
         #![allow(dead_code)]\n\
         \n\
         use crate::__rt::*;\n\
         \n\
         {source}\n"
    )
}

fn source_hash(source: &str, mode: Mode, upstream_mtime: u64) -> String {
    let mut h = Sha256::new();
    h.update(source.as_bytes());
    // Mix the upstream mtime in so a render-wgpu / host-web /
    // framework-core edit invalidates every cached snippet. Mode
    // and tag are appended OUTSIDE the digest so the dir name
    // stays human-skimmable (`<sha>-sim` / `<sha>-web`).
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

/// Latest mtime (UNIX seconds) across the workspace crates whose
/// source affects the snippet wasm. Cheap — a few hundred `stat`s
/// per compile request. Catches the "I edited render-wgpu but
/// forgot to `rm -rf compiled/`" case automatically.
///
/// We don't watch `Cargo.toml` files explicitly because any change
/// to them bumps `Cargo.lock`'s mtime too.
fn upstream_max_mtime(fiddle_root: &Path) -> u64 {
    let Some(workspace_root) = workspace_root_from(fiddle_root) else {
        return 0;
    };
    let watched: &[&str] = &[
        // The framework + UI + backend bits the snippet always
        // links against, regardless of mode.
        "crates/framework/core/src",
        "crates/framework/theme/src",
        "crates/ui/idea-ui/src",
        "crates/backend/web/src",
        // Simulator-mode-only deps. Cheap to walk on every compile
        // even when web mode is the one in play.
        "crates/host/web/src",
        "crates/render/wgpu/src",
        "crates/render/api/src",
        "crates/skin/ios-sim/src",
        // Anything that bumps when a workspace dep version moves.
        "Cargo.lock",
        // The template wrapper itself — editing `lib.rs` (e.g. to
        // add a new prelude symbol) needs to invalidate too.
        "examples/fiddle/template/src",
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

/// `examples/fiddle/` → `examples/fiddle/../..` = workspace root.
fn workspace_root_from(fiddle_root: &Path) -> Option<PathBuf> {
    fiddle_root.parent()?.parent().map(|p| p.to_path_buf())
}

/// Locate the fiddle root (`examples/fiddle/`) relative to the
/// current working directory. The server is normally launched as
/// `cargo run -p fiddle` from anywhere in the workspace; cargo
/// puts the crate's manifest dir into `CARGO_MANIFEST_DIR` at
/// build time, so we bake it in.
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
