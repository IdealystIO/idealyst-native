//! The own-bindings packaging pass: `pkg/<lib>.js` + `pkg/<lib>_bg.wasm`
//! from a linked module that uses `web-glue` instead of wasm-bindgen.
//!
//! `docs/proposals/own-web-bindings.md`. Two uses:
//!
//! * **Every web build, hybrid mode** — [`hybrid_extract`] before
//!   hot-patch base prep and wasm-bindgen, [`write_hybrid_glue_file`]
//!   after (called from [`crate::build`]; `idealyst export` does the same).
//!   backend-web's bindings are web-glue while the SDKs and wgpu are still
//!   on wasm-bindgen (phase 2a), so wasm-bindgen still runs; this pass
//!   only takes the glue out first and supplies it back as
//!   `pkg/__idealyst_glue.js`. A module with no glue passes through.
//! * **Own mode** — [`package_own`] / [`build_crate`], no wasm-bindgen:
//!   what a framework-only app builds with once phases 3–4 remove the
//!   remaining web-sys users. Reached today only from the E2E and
//!   measurements (`tests/own_glue_e2e.rs`). [`package_hybrid`] is the
//!   phase-1 hybrid measurement path; unlike the build's hybrid steps it
//!   also unwraps command exports (`wasm_carve::command_exports`).
//!
//! # Output contract
//!
//! The same one wasm-bindgen's `--target web` output has, because
//! `default_index_html`, `fingerprint_pkg`, SSR's `render_document` and the
//! `__wasm_split.js` loaders are all written against it:
//!
//! * `pkg/<lib>.js` — an ES module whose default export `init(input?)`
//!   fetches `<lib>_bg.wasm` next to it (or takes a URL / Response /
//!   bytes / module / `{ module_or_path }`), instantiates, runs the entry,
//!   and resolves to the raw exports;
//! * a named `initSync(module?)`, which instantiates synchronously from
//!   bytes / a `WebAssembly.Module` / `{ module }`, and — once
//!   instantiated — returns the same raw exports on every later call
//!   (the split loaders call `initSync(undefined, undefined)` to reach
//!   main's table);
//! * any import module that is neither glue nor `env` (e.g.
//!   `./__wasm_split.js`) is passed through as a static ES import, as
//!   wasm-bindgen does.
//!
//! The entry is: `G.attach(exports)`, then `__wasm_call_ctors()` exactly
//! once, then `main(0, 0)` if the module has one (a bin target). That is
//! what wasm-bindgen's `__wbindgen_start` does for a bin. It depends on
//! the module being linked as a reactor ([`link_args`]); see `web_glue`'s
//! crate docs for the command-export trap that flag avoids.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{bail, ensure, Context, Result};
use wasm_carve::glue::{self, Glue};

/// Extra rustc link args an own-mode module needs. Returned as
/// `-C link-arg=…` pairs ready for `CARGO_ENCODED_RUSTFLAGS`.
///
/// `--export=__wasm_call_ctors` makes LLD emit a reactor: without it a
/// bin is a *command*, and LLD wraps every export in a
/// `__wasm_call_ctors` call, re-running static constructors on each JS →
/// Rust call (`inventory::submit!` would double-submit). Hybrid mode does
/// NOT take it — wasm-bindgen owns the entry there.
pub fn link_args() -> Vec<String> {
    vec!["-C".into(), "link-arg=--export=__wasm_call_ctors".into()]
}

/// What a packaging run did.
#[derive(Debug, Clone)]
pub struct PackageReport {
    pub glue_imports: usize,
    pub glue_modules: usize,
    pub glue_section_bytes: usize,
    pub wasm_bytes: usize,
    pub js_bytes: usize,
    /// Time spent in the glue pass itself (extract + write), excluding
    /// wasm-bindgen in hybrid mode.
    pub pass_time: Duration,
    /// Hybrid mode only: time spent in the wasm-bindgen CLI.
    pub bindgen_time: Option<Duration>,
    /// Hybrid mode only: exports repointed past LLD's command-export
    /// constructor wrappers (see `wasm_carve::command_exports`).
    pub unwrapped_command_exports: usize,
}

pub use wasm_carve::glue_js::{hybrid_glue_js, loader_js};

/// The hybrid glue file's name in `pkg/` — what wasm-bindgen's output
/// imports the `./__idealyst_glue.js` namespace from.
pub const HYBRID_GLUE_FILE: &str = "__idealyst_glue.js";

/// Hybrid pipeline, step 1 (before hot-patch base prep and wasm-bindgen):
/// extract the glue from `linked` and write the stripped module to a
/// SIBLING file (`<bin>.glue.wasm`) — never over cargo's own artifact,
/// whose mtime cargo's freshness check trusts. Returns the stripped path
/// and what was extracted. A module without glue is returned as-is.
///
/// LLD's command-export wrappers (`wasm_carve::command_exports`) are
/// unwrapped for web-glue's OWN exports only ([`GLUE_EXPORT_PREFIX`]):
/// they are the entry for every listener dispatch, JS → Rust string and
/// executor drain, and a wrapped one re-runs every static constructor per
/// event — which today's wasm-bindgen call paths do not (measured on
/// `examples/nav-showcase` under the pre-port pipeline: 40 clicks, 0
/// extra constructor runs). Every other export keeps exactly the
/// behaviour it has today; whether to unwrap THOSE is its own decision
/// (docs/proposals/own-web-bindings.md, "Open questions").
pub fn hybrid_extract(linked: &Path) -> Result<(PathBuf, Glue)> {
    let bytes = fs::read(linked).with_context(|| format!("read {}", linked.display()))?;
    let mut glue = glue::extract(&bytes).context("extract web-glue")?;
    if !wasm_carve::glue_js::needs_glue_file(&glue) {
        return Ok((linked.to_path_buf(), glue));
    }
    if let Some((unwrapped, _)) = wasm_carve::command_exports::unwrap_command_exports_where(
        &glue.wasm,
        |name| name.starts_with(GLUE_EXPORT_PREFIX),
    )
    .context("unwrap web-glue's command exports")?
    {
        glue.wasm = unwrapped;
    }
    let stripped = linked.with_extension("glue.wasm");
    write(&stripped, &glue.wasm)?;
    Ok((stripped, glue))
}

/// web-glue's exports (`__glue_invoke`, `__glue_alloc`, `__glue_release`,
/// `__glue_microtask`, `__glue_err_slot`) — called only by its runtime.
pub const GLUE_EXPORT_PREFIX: &str = "__glue_";

/// Hybrid pipeline, step 2 (after wasm-bindgen wrote `pkg_dir`): the
/// `__idealyst_glue.js` its output imports. No-op for a module without
/// glue.
pub fn write_hybrid_glue_file(pkg_dir: &Path, glue: &Glue, lib_name: &str) -> Result<()> {
    if !wasm_carve::glue_js::needs_glue_file(glue) {
        return Ok(());
    }
    write(&pkg_dir.join(HYBRID_GLUE_FILE), hybrid_glue_js(glue, &format!("{lib_name}.js")))
}

fn write(path: &Path, bytes: impl AsRef<[u8]>) -> Result<()> {
    fs::write(path, bytes).with_context(|| format!("write {}", path.display()))
}

/// Own mode: linked wasm → `out_dir/<lib>_bg.wasm` + `out_dir/<lib>.js`.
/// No wasm-bindgen anywhere.
pub fn package_own(linked: &Path, out_dir: &Path, lib_name: &str) -> Result<PackageReport> {
    let start = Instant::now();
    let bytes = fs::read(linked).with_context(|| format!("read {}", linked.display()))?;
    let glue = glue::extract(&bytes).context("extract web-glue")?;
    ensure!(
        glue.runtime.is_none() || has_export(&glue.wasm, "__wasm_call_ctors")?,
        "{} was not linked as a reactor: link with `{}` (own_glue::link_args) — \
         otherwise LLD re-runs static constructors on every JS → Rust call",
        linked.display(),
        link_args().join(" "),
    );
    fs::create_dir_all(out_dir).with_context(|| format!("create {}", out_dir.display()))?;
    let js = loader_js(&glue, lib_name);
    write(&out_dir.join(format!("{lib_name}_bg.wasm")), &glue.wasm)?;
    write(&out_dir.join(format!("{lib_name}.js")), &js)?;
    Ok(PackageReport {
        unwrapped_command_exports: 0,
        glue_imports: glue.imports.len(),
        glue_modules: glue.modules.len(),
        glue_section_bytes: glue.section_bytes,
        wasm_bytes: glue.wasm.len(),
        js_bytes: js.len(),
        pass_time: start.elapsed(),
        bindgen_time: None,
    })
}

/// Hybrid mode: the module ALSO uses wasm-bindgen (an app on wgpu, or
/// any crate on web-sys). Extract + strip the glue, run the normal
/// wasm-bindgen step over the stripped module, then write
/// `__idealyst_glue.js` for the namespace wasm-bindgen passes through.
pub fn package_hybrid(
    reporter: &dev_events::Reporter,
    linked: &Path,
    out_dir: &Path,
    lib_name: &str,
) -> Result<PackageReport> {
    let start = Instant::now();
    let bytes = fs::read(linked).with_context(|| format!("read {}", linked.display()))?;
    let glue = glue::extract(&bytes).context("extract web-glue")?;
    // A bin is a command module: LLD wrapped every export (web-glue's
    // `__glue_*` and wasm-bindgen's own) in a `__wasm_call_ctors` call, and
    // wasm-bindgen's start only calls `main`, so the module cannot be
    // linked as a reactor here. Keep `main`'s wrapper (the one-time ctor
    // run) and point every other export past its wrapper.
    let (module, unwrapped) =
        match wasm_carve::command_exports::unwrap_command_exports(&glue.wasm)
            .context("unwrap command exports")?
        {
            Some((m, n)) => (m, n),
            None => (glue.wasm.clone(), 0),
        };
    let stripped = linked.with_extension("glue-stripped.wasm");
    write(&stripped, &module)?;
    let pass = start.elapsed();

    let bindgen_start = Instant::now();
    crate::wasm_bindgen_build(reporter, &stripped, out_dir, lib_name, false, false)?;
    let bindgen_time = bindgen_start.elapsed();

    let js_start = Instant::now();
    let js = hybrid_glue_js(&glue, &format!("{lib_name}.js"));
    write(&out_dir.join(HYBRID_GLUE_FILE), &js)?;
    let wasm_bytes = fs::metadata(out_dir.join(format!("{lib_name}_bg.wasm")))?.len() as usize;
    Ok(PackageReport {
        unwrapped_command_exports: unwrapped,
        glue_imports: glue.imports.len(),
        glue_modules: glue.modules.len(),
        glue_section_bytes: glue.section_bytes,
        wasm_bytes,
        js_bytes: js.len(),
        pass_time: pass + js_start.elapsed(),
        bindgen_time: Some(bindgen_time),
    })
}

fn has_export(wasm: &[u8], name: &str) -> Result<bool> {
    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        if let wasmparser::Payload::ExportSection(reader) = payload? {
            for e in reader {
                if e?.name == name {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

/// Which packaging [`build_crate`] runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// web-glue only — no wasm-bindgen invoked.
    Own,
    /// web-glue + wasm-bindgen in one module.
    Hybrid,
}

/// Build one bin crate for wasm32 and package it.
#[derive(Clone, Debug)]
pub struct CrateBuild {
    /// A directory inside the workspace the crate belongs to (cargo runs
    /// there).
    pub manifest_dir: PathBuf,
    pub package: String,
    pub bin: String,
    pub release: bool,
    pub mode: Mode,
    /// Own mode changes RUSTFLAGS, which invalidates every dependency's
    /// fingerprint; give it a target dir of its own.
    pub target_dir: PathBuf,
    pub out_dir: PathBuf,
    /// Passed to `cargo build` verbatim (`--no-default-features`, …).
    pub extra_cargo_args: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CrateBuildReport {
    pub cargo_time: Duration,
    pub package: PackageReport,
    pub linked_wasm: PathBuf,
}

pub fn build_crate(reporter: &dev_events::Reporter, b: &CrateBuild) -> Result<CrateBuildReport> {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(&b.manifest_dir)
        .args(["build", "--target", "wasm32-unknown-unknown", "-p", &b.package, "--bin", &b.bin])
        .arg("--target-dir")
        .arg(&b.target_dir);
    if b.release {
        cmd.arg("--release");
    }
    cmd.args(&b.extra_cargo_args);
    // Replace, don't inherit, so an ambient RUSTFLAGS cannot silently
    // drop the reactor flag.
    cmd.env_remove("RUSTFLAGS");
    let flags = match b.mode {
        Mode::Own => link_args(),
        Mode::Hybrid => Vec::new(),
    };
    cmd.env("CARGO_ENCODED_RUSTFLAGS", flags.join("\x1f"));
    let start = Instant::now();
    let status = cmd.status().context("spawn cargo")?;
    if !status.success() {
        bail!("cargo build of {} failed: {status}", b.package);
    }
    let cargo_time = start.elapsed();
    let linked = b
        .target_dir
        .join("wasm32-unknown-unknown")
        .join(if b.release { "release" } else { "debug" })
        .join(format!("{}.wasm", b.bin));
    let lib_name = b.bin.replace('-', "_");
    let package = match b.mode {
        Mode::Own => package_own(&linked, &b.out_dir, &lib_name)?,
        Mode::Hybrid => package_hybrid(reporter, &linked, &b.out_dir, &lib_name)?,
    };
    Ok(CrateBuildReport { cargo_time, package, linked_wasm: linked })
}
