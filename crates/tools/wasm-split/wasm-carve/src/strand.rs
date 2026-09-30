//! Turn function imports nobody will supply into defined functions that
//! trap — without an IR.
//!
//! A hot-reload base roots every function before wasm-bindgen runs, which
//! keeps its descriptor machinery alive; that machinery calls
//! `__wbindgen_placeholder__` imports wasm-bindgen emits no JS binding
//! for (see `build_web::hotpatch_base`). The walrus pass that fixed this
//! parsed and re-emitted the whole module to change a handful of imports:
//! 5.4–5.7 s of every warm CrewForge rebuild on a Mac, 19–25 s in the
//! container.
//!
//! Only the function index space moves. An import cannot become a defined
//! function in place — imports come first in that space — so the stranded
//! imports are dropped, every function after one shifts down, and the
//! stand-ins are appended at the end. Every function reference is
//! renumbered: call / `return_call` / `ref.func` operands, element
//! segments, global initializers, exports, the start function and the
//! `name` section. Nothing else — types, tables, memories, globals, data,
//! other custom sections — changes, so it is copied as bytes. The code
//! section is the one part proportional to program size; it is re-encoded
//! on every core, as `emit` does for main.

use anyhow::{Result, anyhow};
use wasm_encoder::{
    ElementSection, ExportSection, FunctionSection, GlobalSection, ImportSection, IndirectNameMap,
    Module, NameMap, NameSection, RawSection,
    reencode::{self, Reencode},
};
use wasmparser::{KnownCustom, Name, Parser, Payload, TypeRef};

/// The body every stand-in gets: no locals, `unreachable`, `end`. Valid
/// for any signature.
const TRAP_BODY: [u8; 3] = [0x00, 0x00, 0x0b];

/// Replace every function import from one of `namespaces` with a defined
/// function whose body is `unreachable`, named `{prefix}{import name}`.
/// `None` when there is none — the module is not touched.
pub fn trap_imports(wasm: &[u8], namespaces: &[&str], prefix: &str) -> Result<Option<Vec<u8>>> {
    // Pass 1: the function imports, and which of them are stranded.
    let mut stranded: Vec<(u32, u32, &str)> = Vec::new(); // (old index, type, name)
    let mut func_imports = 0u32;
    let mut defined = 0u32;
    for payload in Parser::new(0).parse_all(wasm) {
        match payload? {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import?;
                    if let TypeRef::Func(ty) | TypeRef::FuncExact(ty) = import.ty {
                        if namespaces.contains(&import.module) {
                            stranded.push((func_imports, ty, import.name));
                        }
                        func_imports += 1;
                    }
                }
            }
            Payload::FunctionSection(reader) => defined = reader.count(),
            _ => {}
        }
    }
    if stranded.is_empty() {
        return Ok(None);
    }

    // Old index → new: kept imports and every defined function close up
    // over the removed imports; the stand-ins go last, in import order.
    let total = func_imports + defined;
    let kept_total = total - stranded.len() as u32;
    let mut new_index = vec![0u32; total as usize];
    let mut next = 0u32;
    let mut s = stranded.iter().peekable();
    for old in 0..total {
        if s.peek().is_some_and(|(i, _, _)| *i == old) {
            s.next();
            continue;
        }
        new_index[old as usize] = next;
        next += 1;
    }
    for (pos, (old, _, _)) in stranded.iter().enumerate() {
        new_index[*old as usize] = kept_total + pos as u32;
    }
    let is_stranded = |import: &wasmparser::Import<'_>| {
        matches!(import.ty, TypeRef::Func(_) | TypeRef::FuncExact(_))
            && namespaces.contains(&import.module)
    };

    // Pass 2: emit, section by section, in source order.
    let mut remap = Remap { new_index: &new_index };
    let mut module = Module::new();
    let mut code_seen = false;
    for payload in Parser::new(0).parse_all(wasm) {
        let payload = payload?;
        match &payload {
            Payload::ImportSection(reader) => {
                let mut imports = ImportSection::new();
                for import in reader.clone().into_imports() {
                    let import = import?;
                    if is_stranded(&import) {
                        continue;
                    }
                    let ty = remap.entity_type(import.ty).map_err(err)?;
                    imports.import(import.module, import.name, ty);
                }
                module.section(&imports);
            }
            Payload::FunctionSection(reader) => {
                let mut funcs = FunctionSection::new();
                for ty in reader.clone() {
                    funcs.function(ty?);
                }
                for (_, ty, _) in &stranded {
                    funcs.function(*ty);
                }
                module.section(&funcs);
            }
            Payload::GlobalSection(reader) => {
                let mut globals = GlobalSection::new();
                remap.parse_global_section(&mut globals, reader.clone()).map_err(err)?;
                module.section(&globals);
            }
            Payload::ExportSection(reader) => {
                let mut exports = ExportSection::new();
                for export in reader.clone() {
                    remap.parse_export(&mut exports, export?).map_err(err)?;
                }
                module.section(&exports);
            }
            Payload::StartSection { func, .. } => {
                module.section(&wasm_encoder::StartSection { function_index: new_index[*func as usize] });
            }
            Payload::ElementSection(reader) => {
                let mut elems = ElementSection::new();
                remap.parse_element_section(&mut elems, reader.clone()).map_err(err)?;
                module.section(&elems);
            }
            Payload::CodeSectionStart { range, .. } => {
                code_seen = true;
                let code = encode_code(wasm, range.clone(), &new_index, stranded.len())?;
                module.section(&RawSection { id: 10, data: &code });
            }
            Payload::CustomSection(reader) => match reader.as_known() {
                KnownCustom::Name(names) => {
                    module.section(&renumber_names(names, &new_index, &stranded, prefix)?);
                }
                _ => {
                    module.section(&wasm_encoder::CustomSection {
                        name: reader.name().into(),
                        data: reader.data().into(),
                    });
                }
            },
            Payload::Version { .. } | Payload::End(_) | Payload::CodeSectionEntry(_) => {}
            other => {
                // Every other section holds no function index: copy it.
                let Some((id, range)) = other.as_section() else {
                    return Err(anyhow!("unexpected payload {other:?}"));
                };
                module.section(&RawSection { id, data: &wasm[range] });
            }
        }
    }
    // A base always defines functions; a module that did not would need a
    // function and a code section invented at the right place in section
    // order, for a case that does not occur.
    anyhow::ensure!(code_seen, "a module with no code section");
    Ok(Some(module.finish()))
}

fn err<E: std::fmt::Debug>(e: reencode::Error<E>) -> anyhow::Error {
    anyhow!("re-encode: {e:?}")
}

/// Every defined body re-encoded with its function references renumbered,
/// in parallel, then the stand-ins.
fn encode_code(
    wasm: &[u8],
    range: std::ops::Range<usize>,
    new_index: &[u32],
    stand_ins: usize,
) -> Result<Vec<u8>> {
    let reader = wasmparser::CodeSectionReader::new(wasmparser::BinaryReader::new(
        &wasm[range.clone()],
        range.start,
    ))?;
    let bodies: Vec<wasmparser::FunctionBody<'_>> = reader.into_iter().collect::<Result<_, _>>()?;
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let per = bodies.len().div_ceil(threads).max(1);
    let parts: Vec<Vec<u8>> = std::thread::scope(|scope| {
        let handles: Vec<_> = bodies
            .chunks(per)
            .map(|part| {
                scope.spawn(move || -> Result<Vec<u8>> {
                    let mut out = Vec::new();
                    let mut remap = Remap { new_index };
                    for body in part {
                        let mut func = remap
                            .new_function_with_parsed_locals(body)
                            .map_err(|e| anyhow!("locals: {e:?}"))?;
                        let mut ops = body.get_operators_reader()?;
                        while !ops.eof() {
                            let ins = remap
                                .parse_instruction(&mut ops)
                                .map_err(|e| anyhow!("re-encode: {e:?}"))?;
                            func.instruction(&ins);
                        }
                        wasm_encoder::Encode::encode(&func, &mut out);
                    }
                    Ok(out)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("encode thread panicked")).collect::<Result<_>>()
    })?;
    let mut code = Vec::with_capacity(parts.iter().map(Vec::len).sum::<usize>() + 16 + 4 * stand_ins);
    wasm_encoder::Encode::encode(&((bodies.len() + stand_ins) as u32), &mut code);
    for part in &parts {
        code.extend_from_slice(part);
    }
    for _ in 0..stand_ins {
        code.push(TRAP_BODY.len() as u8);
        code.extend_from_slice(&TRAP_BODY);
    }
    Ok(code)
}

/// The `name` section with function-keyed maps renumbered and re-sorted
/// (a name map must list indices in increasing order, and the stand-ins
/// moved from the front to the back). A stand-in is named `{prefix}{import
/// name}`, so a trap in one names the import it stands in for.
fn renumber_names(
    names: wasmparser::NameSectionReader<'_>,
    new_index: &[u32],
    stranded: &[(u32, u32, &str)],
    prefix: &str,
) -> Result<NameSection> {
    let mut out = NameSection::new();
    let mut other = Remap { new_index };
    for sub in names {
        match sub? {
            Name::Function(map) => {
                let mut entries: Vec<(u32, String)> = Vec::new();
                for naming in map {
                    let naming = naming?;
                    if stranded.iter().any(|(old, _, _)| *old == naming.index) {
                        continue;
                    }
                    entries.push((new_index[naming.index as usize], naming.name.to_string()));
                }
                for (old, _, name) in stranded {
                    entries.push((new_index[*old as usize], format!("{prefix}{name}")));
                }
                entries.sort_by_key(|(i, _)| *i);
                let mut map = NameMap::new();
                for (i, name) in &entries {
                    map.append(*i, name);
                }
                out.functions(&map);
            }
            Name::Local(map) => out.locals(&renumber_indirect(map, new_index)?),
            Name::Label(map) => out.labels(&renumber_indirect(map, new_index)?),
            sub => other.parse_custom_name_subsection(&mut out, sub).map_err(err)?,
        }
    }
    Ok(out)
}

fn renumber_indirect(map: wasmparser::IndirectNameMap<'_>, new_index: &[u32]) -> Result<IndirectNameMap> {
    let mut entries: Vec<(u32, NameMap)> = Vec::new();
    for naming in map {
        let naming = naming?;
        let mut inner = NameMap::new();
        for n in naming.names {
            let n = n?;
            inner.append(n.index, n.name);
        }
        entries.push((new_index[naming.index as usize], inner));
    }
    entries.sort_by_key(|(i, _)| *i);
    let mut out = IndirectNameMap::new();
    for (i, inner) in &entries {
        out.append(*i, inner);
    }
    Ok(out)
}

struct Remap<'a> {
    new_index: &'a [u32],
}

impl Reencode for Remap<'_> {
    type Error = std::convert::Infallible;

    fn function_index(&mut self, func: u32) -> Result<u32, reencode::Error<Self::Error>> {
        Ok(self.new_index[func as usize])
    }
}
