//! The own-bindings packaging pass: `pkg/<lib>.js` + `pkg/<lib>_bg.wasm`
//! from a linked module that uses `web-glue` instead of wasm-bindgen.
//!
//! Phase 1 of `docs/proposals/own-web-bindings.md`. Additive and opt-in:
//! nothing in [`crate::build`] calls it. It is reached only through
//! [`build_crate`] / [`package_own`] / [`package_hybrid`], which the E2E
//! (`tests/own_glue_e2e.rs`) and the phase-1 measurements drive directly.
//! Wiring a `BuildOptions` switch into `build()` belongs to phase 2: until
//! backend-web is ported, every idealyst app links wasm-bindgen through
//! backend-web, so the only mode `build()` could offer it is hybrid, whose
//! pipeline is today's plus one extraction pass.
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

use std::fmt::Write as _;
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

fn indent_block(src: &str) -> String {
    let mut out = String::with_capacity(src.len() + src.len() / 16);
    for line in src.lines() {
        if !line.is_empty() {
            out.push_str("  ");
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// The runtime + module prelude shared by both modes: defines `G`.
fn prelude_js(glue: &Glue) -> String {
    let mut js = String::new();
    match &glue.runtime {
        Some(runtime) => {
            js.push_str("const G = (function () {\n");
            js.push_str(&indent_block(runtime));
            js.push_str("})();\n");
        }
        None => js.push_str("const G = null;\n"),
    }
    for m in &glue.modules {
        let _ = writeln!(js, "G.module({}, function (G) {{", json_string(&m.name));
        js.push_str(&indent_block(&m.source));
        js.push_str("});\n");
    }
    js
}

/// One `name: (snippet),` entry per glue import.
fn snippet_entries(glue: &Glue, prefix: &str, sep: &str, suffix: &str) -> String {
    let mut js = String::new();
    for imp in &glue.imports {
        let _ = writeln!(js, "// {}", imp.key.replace('\n', " "));
        let body = if imp.catch {
            format!("G.catching((\n{}\n))", imp.js.trim())
        } else {
            format!("(\n{}\n)", imp.js.trim())
        };
        let _ = writeln!(js, "{prefix}{}{sep}{body}{suffix}", imp.short);
    }
    js
}

/// A JS string literal.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `pkg/<lib>.js` for own mode.
pub fn loader_js(glue: &Glue, lib_name: &str) -> String {
    let mut js = String::from(
        "// Generated by build-web's own-glue pass from the web-glue records and\n\
         // import snippets carried inside the wasm. Do not edit.\n",
    );
    for (i, m) in glue.foreign_import_modules.iter().enumerate() {
        let _ = writeln!(js, "import * as __foreign{i} from {};", json_string(m));
    }
    js.push_str(&prelude_js(glue));
    js.push_str("const __glue = {\n");
    js.push_str(&snippet_entries(glue, "  ", ": ", ","));
    js.push_str("};\n");
    let _ = write!(js, "const __imports = {{ {}: __glue", json_string(glue::IMPORT_MODULE));
    for (i, m) in glue.foreign_import_modules.iter().enumerate() {
        let _ = write!(js, ", {}: __foreign{i}", json_string(m));
    }
    js.push_str(" };\n");
    let _ = write!(
        js,
        r#"
let wasm;
let __pending;

function __finalize(instance) {{
  if (wasm !== undefined) return wasm;
  wasm = instance.exports;
  if (G !== null) G.attach(wasm);
  // Reactor module: constructors run once, here, never per export call.
  if (typeof wasm.__wasm_call_ctors === "function") wasm.__wasm_call_ctors();
  if (typeof wasm.main === "function") wasm.main(0, 0);
  return wasm;
}}

export function initSync(module) {{
  if (wasm !== undefined) return wasm;
  if (module !== undefined && module !== null && Object.getPrototypeOf(module) === Object.prototype) {{
    module = module.module;
  }}
  if (!(module instanceof WebAssembly.Module)) module = new WebAssembly.Module(module);
  return __finalize(new WebAssembly.Instance(module, __imports));
}}

async function __instantiate(input) {{
  if (input !== undefined && input !== null && Object.getPrototypeOf(input) === Object.prototype) {{
    input = input.module_or_path;
  }}
  if (input === undefined) input = new URL({wasm_name}, import.meta.url);
  if (typeof input === "string" || input instanceof URL ||
      (typeof Request === "function" && input instanceof Request)) {{
    input = fetch(input);
  }}
  input = await input;
  if (input instanceof WebAssembly.Module) {{
    return new WebAssembly.Instance(input, __imports);
  }}
  if (typeof Response === "function" && input instanceof Response) {{
    if (typeof WebAssembly.instantiateStreaming === "function" &&
        input.headers.get("Content-Type") === "application/wasm") {{
      return (await WebAssembly.instantiateStreaming(input, __imports)).instance;
    }}
    input = await input.arrayBuffer();
  }}
  return (await WebAssembly.instantiate(input, __imports)).instance;
}}

export default function init(input) {{
  if (wasm !== undefined) return Promise.resolve(wasm);
  return (__pending ??= __instantiate(input).then(__finalize));
}}
"#,
        wasm_name = json_string(&format!("{lib_name}_bg.wasm")),
    );
    js
}

/// `pkg/__idealyst_glue.js` for hybrid mode: the glue namespace as an ES
/// module wasm-bindgen's generated JS imports
/// (`import * as … from "./__idealyst_glue.js"`). wasm-bindgen owns
/// instantiation, so `G` attaches lazily through `initSync()`, which
/// returns the raw exports once the instance exists. The import is
/// circular (`<lib>.js` ⇄ this file); that is safe because `initSync` is
/// a hoisted function declaration and is only called at glue-call time.
pub fn hybrid_glue_js(glue: &Glue, lib_name: &str) -> String {
    let mut js = String::from(
        "// Generated by build-web's own-glue pass (hybrid mode). Do not edit.\n",
    );
    let _ = writeln!(js, "import {{ initSync }} from {};", json_string(&format!("./{lib_name}.js")));
    js.push_str(&prelude_js(glue));
    js.push_str("if (G !== null) G.lazyAttach(() => initSync(undefined));\n");
    js.push_str(&snippet_entries(glue, "export const ", " = ", ";"));
    js
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
    let js = hybrid_glue_js(&glue, lib_name);
    write(&out_dir.join("__idealyst_glue.js"), &js)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_carve::glue::{GlueImport, GlueRecord};

    fn glue() -> Glue {
        Glue {
            wasm: Vec::new(),
            imports: vec![
                GlueImport { short: "g0".into(), key: "a::f(u32)".into(), catch: false, js: "(x) => x".into() },
                GlueImport { short: "g1".into(), key: "a::g()".into(), catch: true, js: "() => { throw 1; }".into() },
            ],
            runtime: Some("return { module() {}, attach() {}, catching(f) { return f; } };".into()),
            modules: vec![GlueRecord { kind: 1, name: "m\"x".into(), source: "return 1;".into() }],
            foreign_import_modules: vec!["./__wasm_split.js".into()],
            section_bytes: 0,
        }
    }

    #[test]
    fn the_own_loader_keeps_wasm_bindgens_entry_contract() {
        let js = loader_js(&glue(), "my_app");
        assert!(js.contains("export default function init(input)"));
        assert!(js.contains("export function initSync(module)"));
        assert!(js.contains("if (wasm !== undefined) return wasm;"), "idempotent re-init returns the raw exports");
        assert!(js.contains("new URL(\"my_app_bg.wasm\", import.meta.url)"));
        assert!(js.contains("import * as __foreign0 from \"./__wasm_split.js\";"));
        assert!(js.contains("\"./__wasm_split.js\": __foreign0"));
        assert!(js.contains("G.module(\"m\\\"x\", function (G) {"), "module names are escaped");
        assert!(js.contains("g0: (\n(x) => x\n),"));
        assert!(js.contains("g1: G.catching(("), "catch flag wraps the snippet");
        // Constructors before main, both after attach.
        let attach = js.find("G.attach(wasm)").unwrap();
        let ctors = js.find("wasm.__wasm_call_ctors()").unwrap();
        let main = js.find("wasm.main(0, 0)").unwrap();
        assert!(attach < ctors && ctors < main);
    }

    #[test]
    fn the_hybrid_glue_module_exports_every_snippet_and_attaches_lazily() {
        let js = hybrid_glue_js(&glue(), "my_app");
        assert!(js.contains("import { initSync } from \"./my_app.js\";"));
        assert!(js.contains("G.lazyAttach(() => initSync(undefined));"));
        assert!(js.contains("export const g0 = (\n(x) => x\n);"));
        assert!(js.contains("export const g1 = G.catching(("));
    }

    #[test]
    fn generated_js_parses_under_node_when_available() {
        // Syntax check of both generated files with `node --check`, which
        // catches an unbalanced brace in the templates. Skipped (not
        // failed) without node: the browser E2E covers the real run.
        let Ok(out) = Command::new("node").arg("--version").output() else { return };
        if !out.status.success() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("own-glue-syntax-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        for (name, js) in [("a.mjs", loader_js(&glue(), "x")), ("b.mjs", hybrid_glue_js(&glue(), "x"))] {
            let p = dir.join(name);
            fs::write(&p, js).unwrap();
            let st = Command::new("node").arg("--check").arg(&p).output().unwrap();
            assert!(st.status.success(), "{name}: {}", String::from_utf8_lossy(&st.stderr));
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
