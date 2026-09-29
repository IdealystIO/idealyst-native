//! wasm-bindgen 0.2.122's `*.command_export` wrappers, neutralized without
//! an IR. Same two edits as `wasm_split_cli::neutralize_command_export_wrappers`
//! (which documents the inventory double-submit they cause):
//!
//! * a wrapper whose body starts with `call __wasm_call_ctors` loses that
//!   call — unless it is exported under a bare name (`main`), which is the
//!   legitimate one-time init;
//! * an export `X_command_export` is pointed at the function named `X`.
//!
//! wasm-bindgen 0.2.128 emits no such wrappers, so on a current toolchain
//! this finds nothing and hands the input back — where the walrus version
//! parsed and re-emitted the whole module (1.4 GB on CrewForge) to change
//! nothing.

use std::{borrow::Cow, collections::HashMap};

use anyhow::Result;
use wasm_encoder::{ExportKind, ExportSection, Module, RawSection};
use wasmparser::{ExternalKind, Operator};

use crate::module::{ModuleIndex, read_leb};

pub fn neutralize_command_export_wrappers(bindgened: &[u8]) -> Result<Cow<'_, [u8]>> {
    let m = ModuleIndex::parse(bindgened)?;
    let by_name: HashMap<&str, u32> = m.func_names.iter().map(|(f, n)| (*n, *f)).collect();
    let ctors = by_name.get("__wasm_call_ctors").copied();
    let export_of: HashMap<u32, &str> = m
        .exports
        .iter()
        .filter(|e| e.kind == ExternalKind::Func)
        .map(|e| (e.index, e.name))
        .collect();

    // Pass A: wrapper bodies to strip the ctor call from.
    let mut new_bodies: HashMap<u32, Vec<u8>> = HashMap::new();
    if let Some(ctors) = ctors {
        for (f, name) in &m.func_names {
            if !name.ends_with(".command_export") || *f < m.func_imports {
                continue;
            }
            let gut = match export_of.get(f) {
                Some(export) => export.ends_with("_command_export"),
                None => true,
            };
            if gut {
                if let Some(body) = strip_leading_call(&m, *f, ctors)? {
                    new_bodies.insert(*f, body);
                }
            }
        }
    }

    // Pass B: exports to repoint.
    let mut remaps: HashMap<usize, u32> = HashMap::new();
    for (idx, e) in m.exports.iter().enumerate() {
        if e.kind != ExternalKind::Func {
            continue;
        }
        if let Some(bare) = e.name.strip_suffix("_command_export") {
            if let Some(f) = by_name.get(bare) {
                remaps.insert(idx, *f);
            }
        }
    }

    if new_bodies.is_empty() && remaps.is_empty() {
        return Ok(Cow::Borrowed(bindgened));
    }

    let mut module = Module::new();
    for section in &m.sections {
        let (_, payload_start) = read_leb(bindgened, section.range.start + 1)?;
        let payload = &bindgened[payload_start..section.range.end];
        match section.id {
            7 if !remaps.is_empty() => {
                let mut exports = ExportSection::new();
                for (idx, e) in m.exports.iter().enumerate() {
                    let kind = match e.kind {
                        ExternalKind::Func | ExternalKind::FuncExact => ExportKind::Func,
                        ExternalKind::Table => ExportKind::Table,
                        ExternalKind::Memory => ExportKind::Memory,
                        ExternalKind::Global => ExportKind::Global,
                        ExternalKind::Tag => ExportKind::Tag,
                    };
                    exports.export(e.name, kind, remaps.get(&idx).copied().unwrap_or(e.index));
                }
                module.section(&exports);
            }
            10 if !new_bodies.is_empty() => {
                let mut code = Vec::with_capacity(payload.len());
                wasm_encoder::Encode::encode(&m.defined_funcs(), &mut code);
                for f in m.func_imports..m.total_funcs() {
                    match new_bodies.get(&f) {
                        Some(body) => {
                            wasm_encoder::Encode::encode(&(body.len() as u32), &mut code);
                            code.extend_from_slice(body);
                        }
                        None => code.extend_from_slice(m.body_entry(f)),
                    }
                }
                module.section(&RawSection { id: 10, data: &code });
            }
            id => {
                module.section(&RawSection { id, data: payload });
            }
        }
    }
    Ok(Cow::Owned(module.finish()))
}

/// The body of `f` without its first instruction, when that instruction is
/// `call target`.
fn strip_leading_call(m: &ModuleIndex<'_>, f: u32, target: u32) -> Result<Option<Vec<u8>>> {
    let (_, body) = &m.bodies[(f - m.func_imports) as usize];
    let bytes = &m.bytes[body.clone()];
    let fb = wasmparser::FunctionBody::new(wasmparser::BinaryReader::new(bytes, 0));
    let mut locals = fb.get_locals_reader()?;
    for _ in 0..locals.get_count() {
        locals.read()?;
    }
    let ops_start = locals.original_position();
    let mut ops = fb.get_operators_reader()?;
    let Operator::Call { function_index } = ops.read()? else { return Ok(None) };
    if function_index != target {
        return Ok(None);
    }
    let after_call = ops.original_position();
    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&bytes[..ops_start]);
    out.extend_from_slice(&bytes[after_call..]);
    Ok(Some(out))
}
