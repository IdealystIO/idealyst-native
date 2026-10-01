//! The own-bindings packaging pass: `pkg/<lib>.js` + `pkg/<lib>_bg.wasm`
//! from a linked module that uses `web-glue` instead of wasm-bindgen.
//!
//! `docs/proposals/own-web-bindings.md`. Every `idealyst build --web` /
//! `dev --web` goes through [`extract_for_build`], which reads the linked
//! module once and decides how it is packaged:
//!
//! * **Own mode** (the default for a framework app) — the module carries no
//!   wasm-bindgen metadata ([`wasm_carve::glue::WasmBindgenUse`] says what
//!   counts). The wasm-bindgen CLI is never run: [`write_own_pkg`] writes
//!   `pkg/<lib>_bg.wasm` and `pkg/<lib>.js` ([`loader_js`]) from the glue
//!   alone.
//! * **Hybrid mode** — something in the app still links wasm-bindgen (wgpu
//!   for the GPU host or canvas-vello, an app's own web-sys /
//!   `#[wasm_bindgen]` use). The glue comes out first, wasm-bindgen runs
//!   over the stripped module as it always did, and
//!   [`write_hybrid_glue_file`] supplies `pkg/__idealyst_glue.js` for the
//!   namespace wasm-bindgen passes through. `idealyst export` is always
//!   hybrid (its bridge is `#[wasm_bindgen]` classes) and uses
//!   [`hybrid_extract`].
//!
//! [`package_own`] / [`package_hybrid`] / [`build_crate`] are the phase-1
//! measurement paths, driven by `tests/own_glue_e2e.rs`.
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
//! # Constructors
//!
//! A bin is linked by LLD as a *command* module: every export is wrapped
//! in a `__wasm_call_ctors` call, so each JS → Rust call would re-run every
//! static constructor. The build links every web app the same way (the
//! mode is only known after the link, and hybrid cannot be a reactor —
//! wasm-bindgen's start calls only `main`), and own mode then unwraps every
//! export but `main` ([`wasm_carve::command_exports`]): `main`'s wrapper is
//! the one constructor run, and the loader calls `main(0, 0)` once. That is
//! exactly what a reactor link ([`link_args`], used by the phase-1 E2E)
//! gives, where the loader calls `__wasm_call_ctors()` itself. Hybrid
//! unwraps web-glue's own `__glue_*` exports only.

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

pub use wasm_carve::glue_js::{hybrid_glue_js, hybrid_glue_js_with, loader_js, loader_js_with, JsLayout};

/// The hybrid glue file's name in `pkg/` — what wasm-bindgen's output
/// imports the `./__idealyst_glue.js` namespace from.
pub const HYBRID_GLUE_FILE: &str = "__idealyst_glue.js";

/// Hybrid packaging of a module that is KNOWN to need wasm-bindgen
/// (`idealyst export`'s `#[wasm_bindgen]` bridge): extract the glue from
/// `linked` and write the stripped module to a
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

/// Hybrid pipeline, after wasm-bindgen wrote `pkg_dir`: the
/// `__idealyst_glue.js` its output imports, in `layout`
/// ([`JsLayout::for_release`]). No-op for a module without glue.
pub fn write_hybrid_glue_file(pkg_dir: &Path, glue: &Glue, lib_name: &str, layout: JsLayout) -> Result<()> {
    if !wasm_carve::glue_js::needs_glue_file(glue) {
        return Ok(());
    }
    write(&pkg_dir.join(HYBRID_GLUE_FILE), hybrid_glue_js_with(glue, &format!("{lib_name}.js"), layout))
}

/// What [`extract_for_build`] read out of the linked module.
#[derive(Debug)]
pub struct Extracted {
    /// How the build packages this module.
    pub mode: Mode,
    /// The extracted glue. `glue.wasm` is the module the rest of the
    /// pipeline runs on: glue imports renamed, the glue section gone, and
    /// the command exports unwrapped as the mode needs. In own mode it is
    /// also stripped of what only the linked module's readers need
    /// ([`served_custom_section`]).
    pub glue: Glue,
    /// How many exports were repointed past LLD's constructor wrapper.
    pub unwrapped_command_exports: usize,
}

/// Step 1 of every web build: extract the glue from the linked module and
/// decide own vs hybrid (see the module docs). Nothing is written; the
/// caller decides where the module goes (wasm-bindgen's input in hybrid
/// mode, `pkg/` in own mode).
pub fn extract_for_build(linked: &Path) -> Result<Extracted> {
    let bytes = fs::read(linked).with_context(|| format!("read {}", linked.display()))?;
    extract_bytes(&bytes)
}

/// [`extract_for_build`] on bytes already in memory.
pub fn extract_bytes(bytes: &[u8]) -> Result<Extracted> {
    let mut glue = glue::extract(bytes).context("extract web-glue")?;
    let mode = if glue.wasm_bindgen.is_some() { Mode::Hybrid } else { Mode::Own };
    let unwrapped = match mode {
        // Every export but `main`: what a reactor link would have given.
        Mode::Own => wasm_carve::command_exports::unwrap_command_exports(&glue.wasm),
        Mode::Hybrid => wasm_carve::command_exports::unwrap_command_exports_where(&glue.wasm, |name| {
            name.starts_with(GLUE_EXPORT_PREFIX)
        }),
    }
    .context("unwrap the command exports")?;
    let mut moved = 0;
    if let Some((module, n)) = unwrapped {
        glue.wasm = module;
        moved = n;
    }
    if mode == Mode::Own {
        if let Some(stripped) = strip_custom_sections(&glue.wasm, served_custom_section)? {
            glue.wasm = stripped;
        }
    }
    Ok(Extracted { mode, glue, unwrapped_command_exports: moved })
}

/// Which custom sections the module the page loads keeps: all but
/// `linking`, every `reloc.*`, and DWARF (`.debug_*`).
///
/// In hybrid mode wasm-bindgen drops these (it is not run with
/// `--keep-debug`, and it never re-emits `linking` / `reloc.*`); own mode
/// has to do it itself. `--emit-relocs` and the dev profile's line tables
/// are for the readers of the LINKED module — the splitter, the hot-patch
/// alias map — which read cargo's artifact, never this one. The `name`
/// section stays: it is what a stack trace and the hot-patch base index
/// read.
pub fn served_custom_section(name: &str) -> bool {
    name != "linking" && !name.starts_with("reloc.") && !name.starts_with(".debug_")
}

/// Drop every custom section `keep` refuses, copying every other byte.
/// `None` when nothing was dropped.
///
/// Walks section headers only (id + size), never a section's contents,
/// so it costs a scan of the header chain plus one copy.
pub fn strip_custom_sections(wasm: &[u8], keep: impl Fn(&str) -> bool) -> Result<Option<Vec<u8>>> {
    ensure!(wasm.len() >= 8 && &wasm[..4] == b"\0asm", "not a wasm module");
    let mut out: Option<Vec<u8>> = None;
    let mut at = 8usize;
    let mut copied_until = 0usize;
    while at < wasm.len() {
        let start = at;
        let id = wasm[at];
        at += 1;
        let (size, after) = read_u32_leb(wasm, at)?;
        let end = after
            .checked_add(size as usize)
            .filter(|&e| e <= wasm.len())
            .with_context(|| format!("section at byte {start} runs past the end of the module"))?;
        if id == 0 {
            let (name_len, name_at) = read_u32_leb(wasm, after)?;
            let name = wasm
                .get(name_at..name_at + name_len as usize)
                .and_then(|n| std::str::from_utf8(n).ok())
                .with_context(|| format!("custom section at byte {start} has a bad name"))?;
            if !keep(name) {
                let buf = out.get_or_insert_with(|| Vec::with_capacity(wasm.len()));
                buf.extend_from_slice(&wasm[copied_until..start]);
                copied_until = end;
            }
        }
        at = end;
    }
    Ok(out.map(|mut buf| {
        buf.extend_from_slice(&wasm[copied_until..]);
        buf
    }))
}

fn read_u32_leb(bytes: &[u8], mut at: usize) -> Result<(u32, usize)> {
    let mut value = 0u32;
    for shift in (0..35).step_by(7) {
        let b = *bytes.get(at).context("truncated LEB128")?;
        at += 1;
        value |= u32::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Ok((value, at));
        }
    }
    bail!("LEB128 longer than a u32")
}

/// Own mode, the last step: `out_dir/<lib>_bg.wasm` (= `module`, the
/// extracted — and, for a hot-patch base, prepared — module) and
/// `out_dir/<lib>.js` ([`loader_js`]). Also removes what an earlier HYBRID
/// build of the same app left in `out_dir` (the glue file, wasm-bindgen's
/// `.d.ts` files and `snippets/`): `fingerprint_pkg` digests and stages
/// every file under `pkg/`, so a leftover would ship. `layout` is
/// [`JsLayout::for_release`]. Returns the bytes of JS written.
pub fn write_own_pkg(
    module: &[u8],
    glue: &Glue,
    out_dir: &Path,
    lib_name: &str,
    layout: JsLayout,
) -> Result<usize> {
    fs::create_dir_all(out_dir).with_context(|| format!("create {}", out_dir.display()))?;
    for stale in [
        HYBRID_GLUE_FILE.to_string(),
        format!("{lib_name}.d.ts"),
        format!("{lib_name}_bg.wasm.d.ts"),
    ] {
        let path = out_dir.join(stale);
        if path.is_file() {
            fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
        }
    }
    let snippets = out_dir.join("snippets");
    if snippets.is_dir() {
        fs::remove_dir_all(&snippets).with_context(|| format!("remove {}", snippets.display()))?;
    }
    let js = loader_js_with(glue, lib_name, layout);
    write(&out_dir.join(format!("{lib_name}_bg.wasm")), module)?;
    write(&out_dir.join(format!("{lib_name}.js")), &js)?;
    Ok(js.len())
}

fn write(path: &Path, bytes: impl AsRef<[u8]>) -> Result<()> {
    fs::write(path, bytes).with_context(|| format!("write {}", path.display()))
}

/// Own mode: linked wasm → `out_dir/<lib>_bg.wasm` + `out_dir/<lib>.js`.
/// No wasm-bindgen anywhere. Refuses a module that uses wasm-bindgen.
/// Accepts a reactor ([`link_args`]) or a command module (unwrapped as the
/// build does — see the module docs).
pub fn package_own(linked: &Path, out_dir: &Path, lib_name: &str, layout: JsLayout) -> Result<PackageReport> {
    let start = Instant::now();
    let bytes = fs::read(linked).with_context(|| format!("read {}", linked.display()))?;
    let Extracted { mode, glue, unwrapped_command_exports } = extract_bytes(&bytes)?;
    ensure!(
        mode == Mode::Own,
        "{} uses wasm-bindgen ({}) — it can only be packaged in hybrid mode",
        linked.display(),
        glue.wasm_bindgen.as_ref().map(ToString::to_string).unwrap_or_default(),
    );
    // A reactor (`link_args`) needs nothing; a command module must have had
    // its wrappers taken off every export but `main`, or each JS → Rust call
    // re-runs every static constructor.
    ensure!(
        glue.runtime.is_none()
            || has_export(&glue.wasm, "__wasm_call_ctors")?
            || unwrapped_command_exports > 0
            || !has_export(&glue.wasm, "main")?,
        "{} is a command module whose exports could not be unwrapped — link it as a reactor \
         (`{}`, own_glue::link_args)",
        linked.display(),
        link_args().join(" "),
    );
    let js_bytes = write_own_pkg(&glue.wasm, &glue, out_dir, lib_name, layout)?;
    Ok(PackageReport {
        unwrapped_command_exports,
        glue_imports: glue.imports.len(),
        glue_modules: glue.modules.len(),
        glue_section_bytes: glue.section_bytes,
        wasm_bytes: glue.wasm.len(),
        js_bytes,
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
    layout: JsLayout,
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
    let js = hybrid_glue_js_with(&glue, &format!("{lib_name}.js"), layout);
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
    /// Own mode only: link a reactor ([`link_args`]) instead of the command
    /// module `idealyst build` links (whose exports [`extract_bytes`]
    /// unwraps). The reactor flag changes RUSTFLAGS, which invalidates every
    /// dependency's fingerprint; give it a target dir of its own.
    pub reactor: bool,
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
        Mode::Own if b.reactor => link_args(),
        _ => Vec::new(),
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
        Mode::Own => package_own(&linked, &b.out_dir, &lib_name, JsLayout::for_release(b.release))?,
        Mode::Hybrid => {
            package_hybrid(reporter, &linked, &b.out_dir, &lib_name, JsLayout::for_release(b.release))?
        }
    };
    Ok(CrateBuildReport { cargo_time, package, linked_wasm: linked })
}

#[cfg(test)]
mod tests {
    use super::*;
    use walrus::{FunctionBuilder, Module, RawCustomSection, ValType};

    /// A bin as LLD links it (a command module): a constructor that bumps
    /// a global, `main` and two more exports each in LLD's wrapper shape
    /// (`call ctors; local.get…; call inner`) — web-glue's `__glue_invoke`
    /// and an app export — one web-glue import and its runtime record, and
    /// the linker metadata `--emit-relocs` / debuginfo leave behind.
    /// `bindgen_import` adds an import from wasm-bindgen's namespace.
    fn command_module(bindgen_import: bool) -> Vec<u8> {
        let mut m = Module::default();
        let glue_ty = m.types.add(&[ValType::I32], &[ValType::I32]);
        m.add_import_func(wasm_carve::glue::IMPORT_MODULE, "demo::f(u32) -> u32\n\n(x) => x", glue_ty);
        if bindgen_import {
            m.add_import_func("__wbindgen_placeholder__", "__wbindgen_describe", glue_ty);
        }
        let counter = m.globals.add_local(
            ValType::I32,
            true,
            false,
            walrus::ConstExpr::Value(walrus::ir::Value::I32(0)),
        );
        let mut ctors = FunctionBuilder::new(&mut m.types, &[], &[]);
        ctors
            .name("__wasm_call_ctors".into())
            .func_body()
            .global_get(counter)
            .i32_const(1)
            .binop(walrus::ir::BinaryOp::I32Add)
            .global_set(counter);
        let ctors = ctors.finish(vec![], &mut m.funcs);
        let wrapped = |m: &mut Module, name: &str, export: &str| {
            let mut inner = FunctionBuilder::new(&mut m.types, &[], &[]);
            inner.name(format!("{name}_inner")).func_body();
            let inner = inner.finish(vec![], &mut m.funcs);
            let mut w = FunctionBuilder::new(&mut m.types, &[], &[]);
            w.name(format!("{name}.command_export")).func_body().call(ctors).call(inner);
            let w = w.finish(vec![], &mut m.funcs);
            m.exports.add(export, w);
        };
        wrapped(&mut m, "main", "main");
        wrapped(&mut m, "glue_invoke", "__glue_invoke");
        wrapped(&mut m, "app", "app_export");
        let mut runtime = b"IGLU\x01\x00".to_vec();
        runtime.extend(7u16.to_le_bytes());
        runtime.extend(1u32.to_le_bytes());
        runtime.extend(b"runtimeR");
        m.customs.add(RawCustomSection { name: wasm_carve::glue::SECTION.into(), data: runtime });
        for name in ["linking", "reloc.CODE", ".debug_info", "producers"] {
            m.customs.add(RawCustomSection { name: name.into(), data: vec![0] });
        }
        m.emit_wasm()
    }

    fn exports(wasm: &[u8]) -> Vec<(String, u32)> {
        let mut out = Vec::new();
        for p in wasmparser::Parser::new(0).parse_all(wasm) {
            if let wasmparser::Payload::ExportSection(r) = p.unwrap() {
                for e in r {
                    let e = e.unwrap();
                    out.push((e.name.to_string(), e.index));
                }
            }
        }
        out
    }

    fn customs(wasm: &[u8]) -> Vec<String> {
        wasmparser::Parser::new(0)
            .parse_all(wasm)
            .filter_map(|p| match p.unwrap() {
                wasmparser::Payload::CustomSection(c) => Some(c.name().to_string()),
                _ => None,
            })
            .collect()
    }

    /// Own mode: no wasm-bindgen metadata → every export but `main` points
    /// past its constructor wrapper (what a reactor link would give), and
    /// the linker metadata the served module has no reader for is gone —
    /// wasm-bindgen used to drop it, and nothing runs wasm-bindgen now.
    #[test]
    fn a_module_without_wasm_bindgen_is_packaged_own_and_unwrapped_like_a_reactor() {
        let linked = command_module(false);
        let before = exports(&linked);
        let x = extract_bytes(&linked).unwrap();
        assert_eq!(x.mode, Mode::Own);
        assert_eq!(x.unwrapped_command_exports, 2, "__glue_invoke and app_export, not main");
        wasmparser::Validator::new().validate_all(&x.glue.wasm).unwrap();
        let after = exports(&x.glue.wasm);
        let index = |v: &[(String, u32)], n: &str| v.iter().find(|(e, _)| e == n).unwrap().1;
        assert_eq!(index(&before, "main"), index(&after, "main"), "main keeps its one ctor run");
        for e in ["__glue_invoke", "app_export"] {
            assert_ne!(index(&before, e), index(&after, e), "{e} still re-runs constructors");
        }
        let names = customs(&x.glue.wasm);
        assert!(names.contains(&"name".to_string()), "{names:?}");
        assert!(names.contains(&"producers".to_string()), "{names:?}");
        for gone in ["linking", "reloc.CODE", ".debug_info", wasm_carve::glue::SECTION] {
            assert!(!names.iter().any(|n| n == gone), "{gone} survived: {names:?}");
        }
    }

    /// Hybrid mode: an import from wasm-bindgen's namespace → only
    /// web-glue's own exports are unwrapped, and everything else is left
    /// for wasm-bindgen exactly as before.
    #[test]
    fn a_module_with_wasm_bindgen_is_packaged_hybrid_and_keeps_its_other_wrappers() {
        let linked = command_module(true);
        let before = exports(&linked);
        let x = extract_bytes(&linked).unwrap();
        assert_eq!(x.mode, Mode::Hybrid);
        assert_eq!(x.unwrapped_command_exports, 1);
        let after = exports(&x.glue.wasm);
        let index = |v: &[(String, u32)], n: &str| v.iter().find(|(e, _)| e == n).unwrap().1;
        assert_ne!(index(&before, "__glue_invoke"), index(&after, "__glue_invoke"));
        assert_eq!(index(&before, "app_export"), index(&after, "app_export"));
        let names = customs(&x.glue.wasm);
        assert!(names.contains(&"linking".to_string()), "hybrid strips nothing itself: {names:?}");
    }

    #[test]
    fn package_own_refuses_a_module_that_uses_wasm_bindgen() {
        let tmp = tempfile::tempdir().unwrap();
        let linked = tmp.path().join("app.wasm");
        fs::write(&linked, command_module(true)).unwrap();
        let err = package_own(&linked, &tmp.path().join("pkg"), "app", JsLayout::Readable).unwrap_err().to_string();
        assert!(err.contains("hybrid"), "{err}");
    }

    /// Switching an app from hybrid to own (its last web-sys use removed)
    /// must not leave the hybrid build's files in `pkg/`: fingerprinting
    /// digests and staging copies every file there, so a stale glue file
    /// or `.d.ts` would ship.
    #[test]
    fn regression_own_packaging_clears_what_a_hybrid_build_left() {
        let tmp = tempfile::tempdir().unwrap();
        let pkg = tmp.path().join("pkg");
        fs::create_dir_all(pkg.join("snippets/x-123")).unwrap();
        for f in [HYBRID_GLUE_FILE, "app.d.ts", "app_bg.wasm.d.ts", "snippets/x-123/inline0.js"] {
            fs::write(pkg.join(f), "stale").unwrap();
        }
        fs::write(pkg.join("premint.css"), "keep").unwrap();
        let x = extract_bytes(&command_module(false)).unwrap();
        write_own_pkg(&x.glue.wasm, &x.glue, &pkg, "app", JsLayout::Readable).unwrap();
        let mut left: Vec<String> = fs::read_dir(&pkg)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["app.js", "app_bg.wasm", "premint.css"]);
        let js = fs::read_to_string(pkg.join("app.js")).unwrap();
        assert!(js.contains("export function initSync(module)"), "{js}");
        assert_eq!(fs::read(pkg.join("app_bg.wasm")).unwrap(), x.glue.wasm);
    }

    #[test]
    fn stripping_custom_sections_leaves_every_other_byte_and_a_valid_module() {
        let linked = command_module(false);
        assert!(strip_custom_sections(&linked, |_| true).unwrap().is_none(), "nothing refused: no copy");
        let out = strip_custom_sections(&linked, |n| n != "producers" && n != "linking").unwrap().unwrap();
        wasmparser::Validator::new().validate_all(&out).unwrap();
        let names = customs(&out);
        assert!(!names.iter().any(|n| n == "producers" || n == "linking"), "{names:?}");
        assert_eq!(names.len(), customs(&linked).len() - 2);
        assert!(strip_custom_sections(b"nope", |_| true).is_err());
    }
}
