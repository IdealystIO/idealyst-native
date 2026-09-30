//! Extract `web-glue`'s JS from a linked module, without an IR.
//!
//! `web-glue` (crates/runtime/web-glue) ships every binding's JS INSIDE the
//! linked wasm, in two carriers:
//!
//! * **Import names.** Every import from [`IMPORT_MODULE`] is named
//!   `<key>\n<flags>\n<js>`: a unique key (`module_path::fn(types) -> ret`),
//!   comma-separated flags (only `catch` today), and a JS expression that
//!   evaluates to the import's function.
//! * **The [`SECTION`] custom section.** A concatenation — in LLD's link
//!   order, which is arbitrary — of self-delimiting records (see
//!   `web_glue::record`): the JS runtime (`kind 0`, exactly one) and any
//!   crate's `js_module!`s (`kind 1`).
//!
//! [`extract`] reads both and returns the module with every glue import
//! renamed to a short id (`g0`, `g1`, …) and every glue section dropped.
//! Only the import section is re-encoded; every other section — the code
//! section included — is copied as bytes, so the pass costs a memcpy of
//! the module plus a walk of its import list. It never looks at a
//! function body, which is the whole point: wasm-bindgen's CLI parses and
//! re-emits every body (6–10 s and 4.7 GB per rebuild on CrewForge's
//! hot-reload base) to service a boundary 94% of those bodies never touch.
//!
//! The function index space does not move (imports are renamed, never
//! added or removed), so the `name` section and anything else that
//! indexes functions stays valid untouched.

use std::collections::BTreeMap;

use anyhow::{Context, Result, anyhow, bail, ensure};
use wasm_encoder::{ImportSection, Module, RawSection, reencode::Reencode};
use wasmparser::{Parser, Payload};

/// The wasm import module glue imports are declared under
/// (`web_glue::IMPORT_MODULE`).
pub const IMPORT_MODULE: &str = "./__idealyst_glue.js";
/// The custom section carrying records (`web_glue::SECTION`).
pub const SECTION: &str = "__idealyst_glue";

/// Record framing (`web_glue::record`) — duplicated deliberately: this
/// crate must not depend on the runtime crate, and the framing is the
/// entire contract between them. The version byte is what catches drift.
const MAGIC: &[u8; 4] = b"IGLU";
const VERSION: u8 = 1;
const HEADER_LEN: usize = 12;
pub const KIND_RUNTIME: u8 = 0;
pub const KIND_MODULE: u8 = 1;

/// One glue import, renamed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlueImport {
    /// The short name it now has in the module (`g0`, …).
    pub short: String,
    /// `module_path::fn(types) -> ret` — diagnostics only.
    pub key: String,
    /// Wrap the snippet in `G.catching`.
    pub catch: bool,
    /// The JS expression.
    pub js: String,
}

/// One record from the glue section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlueRecord {
    pub kind: u8,
    pub name: String,
    pub source: String,
}

#[derive(Debug)]
pub struct Glue {
    /// The module with glue imports renamed and glue sections removed.
    pub wasm: Vec<u8>,
    pub imports: Vec<GlueImport>,
    /// The runtime record's source. `None` for a module that links no
    /// web-glue at all.
    pub runtime: Option<String>,
    /// `js_module!` records, deduplicated by name, sorted by name so the
    /// generated JS is deterministic whatever order LLD linked them in.
    pub modules: Vec<GlueRecord>,
    /// Import modules that are neither glue nor `env` — e.g.
    /// `./__wasm_split.js` — which the generated loader must pass through
    /// as ES imports, as wasm-bindgen does.
    pub foreign_import_modules: Vec<String>,
    /// Bytes of glue section content that were stripped.
    pub section_bytes: usize,
}

/// Parse `<key>\n<flags>\n<js>`.
pub fn parse_import_name(name: &str) -> Result<(String, bool, String)> {
    let (key, rest) = name
        .split_once('\n')
        .ok_or_else(|| anyhow!("glue import {name:?} has no key line — not declared with web_glue::import!"))?;
    let (flags, js) = rest
        .split_once('\n')
        .ok_or_else(|| anyhow!("glue import `{key}` has no flags line"))?;
    let mut catch = false;
    for flag in flags.split(',').filter(|f| !f.is_empty()) {
        match flag {
            "catch" => catch = true,
            other => bail!("glue import `{key}` has unknown flag {other:?} (newer web-glue than this build tool?)"),
        }
    }
    ensure!(!js.trim().is_empty(), "glue import `{key}` has an empty JS snippet");
    Ok((key.to_string(), catch, js.to_string()))
}

/// Split concatenated glue-section bytes into records.
pub fn parse_records(mut data: &[u8]) -> Result<Vec<GlueRecord>> {
    let mut out = Vec::new();
    while !data.is_empty() {
        ensure!(data.len() >= HEADER_LEN, "truncated glue record header");
        ensure!(&data[..4] == MAGIC, "glue section is not a sequence of IGLU records");
        ensure!(
            data[4] == VERSION,
            "glue record version {} — this build tool reads version {VERSION}; \
             rebuild with a matching web-glue / build tool",
            data[4]
        );
        let kind = data[5];
        let name_len = u16::from_le_bytes([data[6], data[7]]) as usize;
        let src_len = u32::from_le_bytes([data[8], data[9], data[10], data[11]]) as usize;
        let end = HEADER_LEN + name_len + src_len;
        ensure!(data.len() >= end, "truncated glue record body");
        let name = std::str::from_utf8(&data[HEADER_LEN..HEADER_LEN + name_len])
            .context("glue record name is not UTF-8")?;
        let source = std::str::from_utf8(&data[HEADER_LEN + name_len..end])
            .context("glue record source is not UTF-8")?;
        ensure!(kind == KIND_RUNTIME || kind == KIND_MODULE, "glue record `{name}` has unknown kind {kind}");
        out.push(GlueRecord { kind, name: name.to_string(), source: source.to_string() });
        data = &data[end..];
    }
    Ok(out)
}

/// Pull the glue out of a linked module. See the module docs.
pub fn extract(wasm: &[u8]) -> Result<Glue> {
    let mut module = Module::new();
    let mut imports = Vec::new();
    let mut records = Vec::new();
    let mut foreign = Vec::<String>::new();
    let mut section_bytes = 0usize;

    for payload in Parser::new(0).parse_all(wasm) {
        let payload = payload.context("parse wasm")?;
        match &payload {
            Payload::ImportSection(reader) => {
                let mut section = ImportSection::new();
                for import in reader.clone().into_imports() {
                    let import = import.context("parse import")?;
                    let ty = wasm_encoder::reencode::RoundtripReencoder
                        .entity_type(import.ty)
                        .map_err(|e| anyhow!("re-encode import type: {e:?}"))?;
                    if import.module == IMPORT_MODULE {
                        let (key, catch, js) = parse_import_name(import.name)?;
                        let short = format!("g{}", imports.len());
                        section.import(IMPORT_MODULE, &short, ty);
                        imports.push(GlueImport { short, key, catch, js });
                    } else {
                        if import.module != "env" && !foreign.iter().any(|m| m == import.module) {
                            foreign.push(import.module.to_string());
                        }
                        section.import(import.module, import.name, ty);
                    }
                }
                module.section(&section);
            }
            Payload::CustomSection(c) if c.name() == SECTION => {
                section_bytes += c.data().len();
                records.extend(parse_records(c.data())?);
            }
            other => {
                if let Some((id, range)) = other.as_section() {
                    module.section(&RawSection { id, data: &wasm[range] });
                }
            }
        }
    }

    let mut runtime = None;
    let mut modules = BTreeMap::<String, String>::new();
    for r in records {
        if r.kind == KIND_RUNTIME {
            match &runtime {
                None => runtime = Some(r.source),
                // Two web-glue versions in one graph with different
                // runtimes would share one slab under two layouts.
                Some(existing) if *existing == r.source => {}
                Some(_) => bail!("two different web-glue runtimes are linked into this module (two web-glue versions?)"),
            }
        } else {
            match modules.get(&r.name) {
                None => {
                    modules.insert(r.name, r.source);
                }
                Some(existing) if *existing == r.source => {}
                Some(_) => bail!("two different JS modules are both named `{}`", r.name),
            }
        }
    }
    ensure!(
        imports.is_empty() || runtime.is_some(),
        "the module declares {} web-glue imports but carries no web-glue runtime record \
         (was the `__idealyst_glue` custom section stripped before this pass?)",
        imports.len()
    );

    Ok(Glue {
        wasm: module.finish(),
        imports,
        runtime,
        modules: modules
            .into_iter()
            .map(|(name, source)| GlueRecord { kind: KIND_MODULE, name, source })
            .collect(),
        foreign_import_modules: foreign,
        section_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_encoder::{
        CodeSection, CustomSection, EntityType, ExportKind, ExportSection, Function,
        FunctionSection, Instruction, TypeSection, ValType,
    };

    fn record(kind: u8, name: &str, src: &str) -> Vec<u8> {
        let mut r = MAGIC.to_vec();
        r.push(VERSION);
        r.push(kind);
        r.extend((name.len() as u16).to_le_bytes());
        r.extend((src.len() as u32).to_le_bytes());
        r.extend(name.as_bytes());
        r.extend(src.as_bytes());
        r
    }

    /// A module shaped like a web-glue link: two glue imports, one foreign
    /// import, one defined function calling the first import, and the glue
    /// section split across two custom sections (as when objects are not
    /// merged) in "wrong" order.
    fn fixture() -> Vec<u8> {
        let mut m = Module::new();
        let mut types = TypeSection::new();
        types.ty().function([ValType::I32], [ValType::I32]);
        m.section(&types);
        let mut imports = ImportSection::new();
        imports.import(IMPORT_MODULE, "demo::a(u32) -> u32\n\n(x) => x + 1", EntityType::Function(0));
        imports.import("./__wasm_split.js", "__wasm_split_load_x", EntityType::Function(0));
        imports.import(IMPORT_MODULE, "demo::b(u32) -> u32\ncatch\n(x) => { throw x; }", EntityType::Function(0));
        m.section(&imports);
        let mut funcs = FunctionSection::new();
        funcs.function(0);
        m.section(&funcs);
        let mut exports = ExportSection::new();
        exports.export("f", ExportKind::Func, 3);
        m.section(&exports);
        let mut code = CodeSection::new();
        let mut f = Function::new([]);
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::Call(0));
        f.instruction(&Instruction::End);
        code.function(&f);
        m.section(&code);
        m.section(&CustomSection {
            name: SECTION.into(),
            data: record(KIND_MODULE, "zeta", "return 1;").into(),
        });
        let mut both = record(KIND_RUNTIME, "runtime", "return {};");
        both.extend(record(KIND_MODULE, "alpha", "return 2;"));
        both.extend(record(KIND_MODULE, "zeta", "return 1;"));
        m.section(&CustomSection { name: SECTION.into(), data: both.into() });
        m.section(&CustomSection { name: "producers".into(), data: b"\0".as_slice().into() });
        m.finish()
    }

    #[test]
    fn glue_imports_are_renamed_and_sections_stripped_leaving_a_valid_module() {
        let out = extract(&fixture()).unwrap();
        wasmparser::Validator::new().validate_all(&out.wasm).expect("output validates");
        assert_eq!(
            out.imports,
            [
                GlueImport { short: "g0".into(), key: "demo::a(u32) -> u32".into(), catch: false, js: "(x) => x + 1".into() },
                GlueImport { short: "g1".into(), key: "demo::b(u32) -> u32".into(), catch: true, js: "(x) => { throw x; }".into() },
            ]
        );
        assert_eq!(out.runtime.as_deref(), Some("return {};"));
        let names: Vec<_> = out.modules.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["alpha", "zeta"], "deduplicated and sorted, whatever the link order");
        assert_eq!(out.foreign_import_modules, ["./__wasm_split.js"]);

        let mut seen_imports = Vec::new();
        let mut customs = Vec::new();
        for p in Parser::new(0).parse_all(&out.wasm) {
            match p.unwrap() {
                Payload::ImportSection(r) => {
                    for i in r.into_imports() {
                        let i = i.unwrap();
                        seen_imports.push(format!("{}/{}", i.module, i.name));
                    }
                }
                Payload::CustomSection(c) => customs.push(c.name().to_string()),
                _ => {}
            }
        }
        assert_eq!(
            seen_imports,
            [
                format!("{IMPORT_MODULE}/g0"),
                "./__wasm_split.js/__wasm_split_load_x".into(),
                format!("{IMPORT_MODULE}/g1")
            ],
            "import ORDER is kept, so no function index moves"
        );
        assert_eq!(customs, ["producers"], "other custom sections survive");
    }

    #[test]
    fn a_record_from_a_newer_framing_is_refused_loudly() {
        let mut r = record(KIND_MODULE, "m", "x");
        r[4] = VERSION + 1;
        let err = parse_records(&r).unwrap_err().to_string();
        assert!(err.contains("version"), "{err}");
    }

    #[test]
    fn two_different_modules_under_one_name_are_an_error() {
        let mut data = record(KIND_RUNTIME, "runtime", "r");
        data.extend(record(KIND_MODULE, "m", "one"));
        data.extend(record(KIND_MODULE, "m", "two"));
        let mut m = Module::new();
        m.section(&CustomSection { name: SECTION.into(), data: data.into() });
        let err = extract(&m.finish()).unwrap_err().to_string();
        assert!(err.contains("both named `m`"), "{err}");
    }

    #[test]
    fn an_import_not_declared_with_the_macro_is_named_in_the_error() {
        assert!(parse_import_name("bare").unwrap_err().to_string().contains("\"bare\""));
        assert!(parse_import_name("k\nwat\nx").unwrap_err().to_string().contains("unknown flag"));
    }

    #[test]
    fn a_module_without_glue_passes_through_byte_identical_sections() {
        let mut m = Module::new();
        m.section(&CustomSection { name: "name".into(), data: b"\0\x01\0".as_slice().into() });
        let bytes = m.finish();
        let out = extract(&bytes).unwrap();
        assert_eq!(out.wasm, bytes);
        assert!(out.runtime.is_none() && out.imports.is_empty());
    }
}
