//! Building main and the split modules from byte ranges.
//!
//! Every output keeps the source module's function, type, table, memory and
//! global numbering. That is the whole trick: a kept function body is copied
//! byte for byte, because every index inside it still means what it meant.
//! A function the output does not own is either a table trampoline (it lives
//! in main, reached through the shared funcref table) or a three-byte
//! `unreachable` stub; wasm-opt, which runs on every output anyway, removes
//! the stubs and renumbers.
//!
//! In a split module the index space is kept by giving it NO function
//! imports: index `i` below the source's import count becomes a defined
//! trampoline to main's import, and every defined function stays at its
//! index. Memory, tables and globals are imported from main in their
//! original order, so their indices hold too.

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
};

use anyhow::{Context, Result, bail, ensure};
use wasm_encoder::{
    CodeSection, ConstExpr, CustomSection, DataCountSection, DataSection, ElementSection,
    Elements, EntityType, ExportKind, ExportSection, FunctionSection, GlobalType, ImportSection,
    MemoryType, Module, NameMap, NameSection, RawSection, RefType, TableSection, TableType,
    reencode::{self, Reencode},
};
use wasmparser::{ExternalKind, TypeRef};

use crate::{
    graph::{Node, Partition},
    module::{ElemItem, ModuleIndex, read_leb},
};

/// The body every stubbed function gets: no locals, `unreachable`, `end`.
/// Valid for any signature.
const STUB_ENTRY: [u8; 4] = [0x03, 0x00, 0x00, 0x0b];

/// Names the outputs agree on for what main exports and the splits import.
pub struct Layout {
    funcref_table: u32,
    /// The funcref table's limits once main has grown it.
    grown_table: wasmparser::TableType,
    /// First appended slot: split entries, then the shared functions.
    segment_start: u32,
    table_names: Vec<String>,
    memory_names: Vec<String>,
    global_names: Vec<String>,
    /// Function index → its slot in the appended range.
    shared_slot: BTreeMap<u32, u32>,
    shared_funcs: Vec<u32>,
    /// Function index → every table slot it occupies in the source.
    table_slots: BTreeMap<i64, u32>,
}

impl Layout {
    /// The table slot a split module installs its entry at, which main's
    /// split-point import forwards to.
    pub fn entry_slot(&self, split_index: usize) -> u32 {
        self.segment_start + split_index as u32
    }

    pub fn new(source: &ModuleIndex<'_>, partition: &Partition) -> Result<Self> {
        let funcref_tables: Vec<u32> = source
            .table_types
            .iter()
            .enumerate()
            .filter(|(_, t)| t.element_type == wasmparser::RefType::FUNCREF)
            .map(|(i, _)| i as u32)
            .collect();
        let funcref_table = *funcref_tables.first().context("no funcref table")?;
        ensure!(
            funcref_table >= source.table_imports,
            "the funcref table is imported; it cannot be grown"
        );

        let shared_funcs = partition.shared_funcs();
        let appended = (partition.split_points.len() + shared_funcs.len()) as u64;
        let mut grown_table = source.table_types[funcref_table as usize];
        let max = grown_table
            .maximum
            .context("the funcref table has no maximum; nothing to append after")?;
        let segment_start = u32::try_from(max).context("table maximum")?;
        grown_table.initial += appended;
        grown_table.maximum = Some(max + appended);

        let mut table_names = Vec::new();
        for idx in 0..source.table_types.len() as u32 {
            table_names.push(if idx == funcref_table {
                "__indirect_function_table".to_string()
            } else {
                format!("__imported_table_{idx}")
            });
        }
        let mut memory_names = Vec::new();
        for idx in 0..source.memory_types.len() as u32 {
            let existing = source
                .exports
                .iter()
                .find(|e| e.kind == ExternalKind::Memory && e.index == idx)
                .map(|e| e.name.to_string());
            memory_names.push(existing.unwrap_or_else(|| format!("__memory_{idx}")));
        }
        let global_names =
            (0..source.global_types.len()).map(|idx| format!("__global__{idx}")).collect();

        let shared_base = segment_start + partition.split_points.len() as u32;
        let shared_slot = shared_funcs
            .iter()
            .enumerate()
            .map(|(pos, f)| (*f, shared_base + pos as u32))
            .collect();

        let mut table_slots = BTreeMap::new();
        for elem in &source.elems {
            let Some((table, offset)) = elem.active else { continue };
            if table != funcref_table {
                continue;
            }
            for (i, item) in elem.items.iter().enumerate() {
                if let ElemItem::Func(f) = item {
                    table_slots.insert(offset as i64 + i as i64, *f);
                }
            }
        }

        Ok(Layout {
            funcref_table,
            grown_table,
            segment_start,
            table_names,
            memory_names,
            global_names,
            shared_slot,
            shared_funcs,
            table_slots,
        })
    }
}

pub struct Emitted {
    pub bytes: Vec<u8>,
}

/// Main: what the source module reaches from its roots, with the splits'
/// table slots emptied, the split entries and every shared function
/// appended to the table, main's memory, tables and globals exported for
/// the splits, the split exports gone, and each split-point import turned
/// into a trampoline to its module's entry slot.
pub fn emit_main(source: &ModuleIndex<'_>, partition: &Partition, layout: &Layout) -> Result<Emitted> {
    let unused: HashSet<u32> = partition
        .unused_main_symbols(source)
        .into_iter()
        .filter_map(|n| match n {
            Node::Function(f) => Some(f),
            Node::DataSymbol(_) => None,
        })
        .collect();
    let split_exports: HashSet<&str> =
        partition.split_points.iter().map(|s| s.export_name.as_str()).collect();

    // Table slots after the splits' are emptied: `None` is a hole the
    // dummy fills.
    let elem_items: Vec<Vec<Option<u32>>> = source
        .elems
        .iter()
        .map(|elem| {
            elem.items
                .iter()
                .map(|item| match item {
                    ElemItem::Func(f) if unused.contains(f) => Ok(None),
                    ElemItem::Func(f) => Ok(Some(*f)),
                    ElemItem::Null => bail!("ref.null in an element segment"),
                })
                .collect::<Result<_>>()
        })
        .collect::<Result<_>>()?;

    // What main keeps: everything reachable from its exports (the split
    // exports removed, every shared function added), its start function
    // and its table — the same roots walrus's GC used. Bodies are walked
    // for direct references only; nothing reached through data needs
    // one, because such a function is in the table.
    let mut live = vec![false; source.total_funcs() as usize];
    let mut stack: Vec<u32> = Vec::new();
    let root = |f: u32, live: &mut Vec<bool>, stack: &mut Vec<u32>| {
        if !std::mem::replace(&mut live[f as usize], true) {
            stack.push(f);
        }
    };
    for f in 0..source.func_imports {
        root(f, &mut live, &mut stack);
    }
    for e in &source.exports {
        if e.kind == ExternalKind::Func && !split_exports.contains(e.name) {
            root(e.index, &mut live, &mut stack);
        }
    }
    for f in &layout.shared_funcs {
        root(*f, &mut live, &mut stack);
    }
    if let Some(start) = source.start {
        root(start, &mut live, &mut stack);
    }
    for f in elem_items.iter().flatten().flatten() {
        root(*f, &mut live, &mut stack);
    }
    let mut refs = Vec::new();
    while let Some(f) = stack.pop() {
        if f < source.func_imports {
            continue;
        }
        refs.clear();
        source.direct_refs(f, &mut refs)?;
        for r in &refs {
            root(*r, &mut live, &mut stack);
        }
    }

    // Renumber. Imports first, minus the split-point imports: each of
    // those becomes a defined trampoline to the table slot its module
    // installs its entry at. Then the live defined functions in source
    // order, then the dummy that fills emptied slots — its body traps, so
    // a slot a split never filled fails loudly.
    let split_entries: BTreeMap<u32, u32> = partition
        .split_points
        .iter()
        .map(|s| (s.import_func, layout.entry_slot(s.index)))
        .collect();
    let mut new_index: HashMap<u32, u32> = HashMap::new();
    let mut kept_imports: Vec<usize> = Vec::new();
    let mut func_import = 0u32;
    for (pos, import) in source.imports.iter().enumerate() {
        if !matches!(import.ty, TypeRef::Func(_) | TypeRef::FuncExact(_)) {
            kept_imports.push(pos);
            continue;
        }
        if !split_entries.contains_key(&func_import) {
            new_index.insert(func_import, new_index.len() as u32);
            kept_imports.push(pos);
        }
        func_import += 1;
    }
    for f in split_entries.keys() {
        new_index.insert(*f, new_index.len() as u32);
    }
    let mut live_defined: Vec<u32> = Vec::new();
    for f in source.func_imports..source.total_funcs() {
        if live[f as usize] {
            new_index.insert(f, new_index.len() as u32);
            live_defined.push(f);
        }
    }
    let dummy = new_index.len() as u32;
    let dummy_type = 0u32;
    let ni = |f: u32| new_index[&f];

    let mut module = Module::new();
    for section in &source.sections {
        match section.id {
            2 => {
                let mut imports = ImportSection::new();
                for pos in &kept_imports {
                    let import = &source.imports[*pos];
                    imports.import(import.module, import.name, entity_type(&import.ty)?);
                }
                module.section(&imports);
            }
            3 => {
                let mut funcs = FunctionSection::new();
                for f in split_entries.keys() {
                    funcs.function(source.func_types[*f as usize]);
                }
                for f in &live_defined {
                    funcs.function(source.func_types[*f as usize]);
                }
                funcs.function(dummy_type);
                module.section(&funcs);
            }
            4 => {
                let mut tables = TableSection::new();
                for idx in source.table_imports..source.table_types.len() as u32 {
                    let ty = if idx == layout.funcref_table {
                        layout.grown_table
                    } else {
                        source.table_types[idx as usize]
                    };
                    tables.table(table_type(&ty)?);
                }
                module.section(&tables);
            }
            7 => {
                let mut exports = ExportSection::new();
                let mut used: HashSet<String> = HashSet::new();
                let mut exported_funcs: HashSet<u32> = HashSet::new();
                for e in &source.exports {
                    if e.kind == ExternalKind::Func && split_exports.contains(e.name) {
                        continue;
                    }
                    let index = if e.kind == ExternalKind::Func {
                        exported_funcs.insert(e.index);
                        ni(e.index)
                    } else {
                        e.index
                    };
                    exports.export(e.name, export_kind(e.kind)?, index);
                    used.insert(e.name.to_string());
                }
                for (idx, name) in layout.table_names.iter().enumerate() {
                    let already = source
                        .exports
                        .iter()
                        .any(|e| e.kind == ExternalKind::Table && e.index == idx as u32 && e.name == name);
                    if !already && used.insert(name.clone()) {
                        exports.export(name, ExportKind::Table, idx as u32);
                    }
                }
                for (idx, name) in layout.memory_names.iter().enumerate() {
                    if used.insert(name.clone()) {
                        exports.export(name, ExportKind::Memory, idx as u32);
                    }
                }
                for (idx, name) in layout.global_names.iter().enumerate() {
                    if used.insert(name.clone()) {
                        exports.export(name, ExportKind::Global, idx as u32);
                    }
                }
                // Keep main's copy of every shared function alive through
                // wasm-opt: the splits reach it through the table, which
                // binaryen cannot see across modules. Short names — see
                // `wasm-split-cli`'s `next_synthetic_export_name`.
                let mut next = 0usize;
                for f in &layout.shared_funcs {
                    if exported_funcs.contains(f) {
                        continue;
                    }
                    let name = loop {
                        let name = format!("s{next}");
                        next += 1;
                        if used.insert(name.clone()) {
                            break name;
                        }
                    };
                    exports.export(&name, ExportKind::Func, ni(*f));
                }
                module.section(&exports);
            }
            8 => {
                let start = source.start.context("start section without a start")?;
                module.section(&wasm_encoder::StartSection { function_index: ni(start) });
            }
            9 => {
                let mut elems = ElementSection::new();
                for (elem, items) in source.elems.iter().zip(&elem_items) {
                    let items: Vec<u32> = items.iter().map(|f| f.map_or(dummy, ni)).collect();
                    let Some((table, offset)) = elem.active else {
                        bail!("non-constant or passive element segment");
                    };
                    push_active_funcs(&mut elems, table, offset, elem.expr_ref_type, &items);
                }
                let mut ifuncs = vec![dummy; partition.split_points.len()];
                ifuncs.extend(layout.shared_funcs.iter().map(|f| ni(*f)));
                push_active_funcs(
                    &mut elems,
                    layout.funcref_table,
                    layout.segment_start as i32,
                    None,
                    &ifuncs,
                );
                module.section(&elems);
            }
            10 => {
                let mut trampolines = Vec::new();
                for (f, slot) in &split_entries {
                    let t = trampoline(source, *f, *slot, layout.funcref_table)?;
                    wasm_encoder::Encode::encode(&t, &mut trampolines);
                }
                let code = encode_bodies(
                    source,
                    &trampolines,
                    split_entries.len() as u32,
                    &live_defined,
                    &new_index,
                )?;
                module.section(&RawSection { id: 10, data: &code });
            }
            0 => match section.custom_name.as_deref() {
                Some("name") => {
                    let mut names = NameMap::new();
                    for f in 0..source.total_funcs() {
                        if let (Some(new), Some(name)) = (new_index.get(&f), source.func_names.get(&f)) {
                            names.append(*new, name);
                        }
                    }
                    let mut section = NameSection::new();
                    section.functions(&names);
                    module.section(&section);
                }
                Some("target_features") => {
                    module.section(&RawSection { id: 0, data: payload(source, section)? });
                }
                _ => {}
            },
            id => {
                module.section(&RawSection { id, data: payload(source, section)? });
            }
        }
    }
    Ok(Emitted { bytes: module.finish() })
}

/// The code section payload for `live` (in order) plus the dummy: each
/// body re-encoded with its calls renumbered, on every core — main keeps
/// most of the program, and re-encoding is the one step here that is
/// proportional to code volume.
fn encode_bodies(
    source: &ModuleIndex<'_>,
    leading: &[u8],
    leading_count: u32,
    live: &[u32],
    new_index: &HashMap<u32, u32>,
) -> Result<Vec<u8>> {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let per = live.len().div_ceil(threads).max(1);
    let parts: Vec<Vec<u8>> = std::thread::scope(|scope| {
        let handles: Vec<_> = live
            .chunks(per)
            .map(|part| {
                scope.spawn(move || -> Result<Vec<u8>> {
                    let mut out = Vec::new();
                    let mut remap = Renumber { new_index };
                    for f in part {
                        let (_, body) = &source.bodies[(*f - source.func_imports) as usize];
                        let fb = wasmparser::FunctionBody::new(wasmparser::BinaryReader::new(
                            &source.bytes[body.clone()],
                            body.start,
                        ));
                        let mut func = remap
                            .new_function_with_parsed_locals(&fb)
                            .map_err(|e| anyhow::anyhow!("locals of {f}: {e:?}"))?;
                        let mut ops = fb.get_operators_reader()?;
                        while !ops.eof() {
                            let ins = remap
                                .parse_instruction(&mut ops)
                                .map_err(|e| anyhow::anyhow!("re-encode {f}: {e:?}"))?;
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
    let mut code = Vec::with_capacity(parts.iter().map(Vec::len).sum::<usize>() + 16);
    wasm_encoder::Encode::encode(&(leading_count + live.len() as u32 + 1), &mut code);
    code.extend_from_slice(leading);
    for part in &parts {
        code.extend_from_slice(part);
    }
    code.extend_from_slice(&STUB_ENTRY);
    Ok(code)
}

fn payload<'a>(source: &ModuleIndex<'a>, section: &crate::module::Section) -> Result<&'a [u8]> {
    let (_, start) = read_leb(source.bytes, section.range.start + 1)?;
    Ok(&source.bytes[start..section.range.end])
}

/// What one split output owns.
pub struct SplitBody<'p> {
    /// Functions whose bodies it keeps.
    pub bodies: &'p HashSet<Node>,
    /// Symbols it installs: its functions' table slots and its data.
    pub unique: &'p HashSet<Node>,
    /// A split module's entry: `(export name, function, split index)`.
    pub entry: Option<(&'p str, u32, usize)>,
}

pub fn emit_split(
    source: &ModuleIndex<'_>,
    partition: &Partition,
    layout: &Layout,
    split: SplitBody<'_>,
) -> Result<Emitted> {
    let mut module = Module::new();

    // Types, verbatim.
    if let Some(types) = source.section(1) {
        module.section(&RawSection { id: 1, data: payload(source, types)? });
    }

    // Imports: main's tables, memories and globals, in index order.
    let mut imports = ImportSection::new();
    for (idx, name) in layout.table_names.iter().enumerate() {
        let ty = if idx as u32 == layout.funcref_table {
            layout.grown_table
        } else {
            source.table_types[idx]
        };
        imports.import("__wasm_split", name, EntityType::Table(table_type(&ty)?));
    }
    for (idx, name) in layout.memory_names.iter().enumerate() {
        imports.import("__wasm_split", name, EntityType::Memory(memory_type(&source.memory_types[idx])));
    }
    for (idx, name) in layout.global_names.iter().enumerate() {
        imports.import("__wasm_split", name, EntityType::Global(global_type(&source.global_types[idx])?));
    }
    module.section(&imports);

    // The functions it keeps, and everything they reference directly:
    // renumbered densely, so wasm-opt parses what the output uses rather
    // than a stub for every function of the program.
    let kept: Vec<u32> = {
        let mut kept: Vec<u32> = split
            .bodies
            .iter()
            .filter_map(|n| match n {
                Node::Function(f) if *f >= source.func_imports => Some(*f),
                _ => None,
            })
            .collect();
        kept.sort_unstable();
        kept
    };
    let kept_set: HashSet<u32> = kept.iter().copied().collect();
    let mut installs: BTreeMap<i64, u32> = BTreeMap::new();
    for (slot, f) in &layout.table_slots {
        if split.unique.contains(&Node::Function(*f)) {
            installs.insert(*slot, *f);
        }
    }
    if let Some((_, func, split_idx)) = split.entry {
        installs.insert(layout.segment_start as i64 + split_idx as i64, func);
    }
    let mut needed: BTreeSet<u32> = kept.iter().copied().collect();
    needed.extend(installs.values().copied());
    let mut refs = Vec::new();
    for f in &kept {
        refs.clear();
        source.direct_refs(*f, &mut refs)?;
        needed.extend(refs.iter().copied());
    }
    let order: Vec<u32> = needed.into_iter().collect();
    let new_index: HashMap<u32, u32> =
        order.iter().enumerate().map(|(i, f)| (*f, i as u32)).collect();

    let mut funcs = FunctionSection::new();
    for f in &order {
        funcs.function(source.func_types[*f as usize]);
    }
    module.section(&funcs);

    if let Some((name, func, _)) = split.entry {
        let mut exports = ExportSection::new();
        exports.export(name, ExportKind::Func, new_index[&func]);
        module.section(&exports);
    }

    // Install this output's functions at their source table slots, and a
    // split module's entry at its appended slot.
    if !installs.is_empty() {
        let mut elems = ElementSection::new();
        for (slot, f) in &installs {
            push_active_funcs(&mut elems, layout.funcref_table, *slot as i32, None, &[new_index[f]]);
        }
        module.section(&elems);
    }

    // Data: every source segment emptied (their indices hold), then this
    // output's own data symbols re-materialized at their addresses.
    let mut data_segments: Vec<(u32, i32, &[u8])> = Vec::new();
    let mut unique_data: Vec<usize> = split
        .unique
        .iter()
        .filter_map(|n| match n {
            Node::DataSymbol(s) => Some(*s),
            Node::Function(_) => None,
        })
        .collect();
    unique_data.sort_unstable();
    for (seg_idx, seg) in source.data.iter().enumerate() {
        let Some((memory, base)) = seg.active_const else { continue };
        let bytes = &source.bytes[seg.data.clone()];
        for id in &unique_data {
            let Some(symbol) = partition.data_symbols.get(id) else { continue };
            if symbol.which_data_segment != seg_idx {
                continue;
            }
            let range = symbol.segment_offset..symbol.segment_offset + symbol.symbol_size;
            if range.end > bytes.len() {
                continue;
            }
            data_segments.push((memory, base + symbol.segment_offset as i32, &bytes[range]));
        }
    }
    if source.data_count.is_some() {
        module.section(&DataCountSection { count: (source.data.len() + data_segments.len()) as u32 });
    }

    // Code: kept bodies re-encoded with their calls renumbered, main's
    // functions as table trampolines. A reference to anything else would
    // be a call into code this output does not have; it gets a trapping
    // stub, and is reported.
    let mut code = CodeSection::new();
    let mut remap = Renumber { new_index: &new_index };
    let mut dangling = 0usize;
    for f in &order {
        if kept_set.contains(f) {
            let (_, body) = &source.bodies[(*f - source.func_imports) as usize];
            let fb = wasmparser::FunctionBody::new(wasmparser::BinaryReader::new(
                &source.bytes[body.clone()],
                body.start,
            ));
            remap
                .parse_function_body(&mut code, fb)
                .map_err(|e| anyhow::anyhow!("re-encode function {f}: {e:?}"))?;
        } else if let Some(slot) = layout.shared_slot.get(f) {
            code.function(&trampoline(source, *f, *slot, layout.funcref_table)?);
        } else {
            dangling += 1;
            code.raw(&STUB_ENTRY[1..]);
        }
    }
    module.section(&code);
    if dangling > 0 {
        eprintln!("[wasm-carve] {dangling} referenced function(s) neither kept nor shared; stubbed");
    }

    let mut data = DataSection::new();
    for seg in &source.data {
        match seg.active_const {
            Some((memory, offset)) => {
                data.active(memory, &ConstExpr::i32_const(offset), []);
            }
            None if seg.passive => {
                data.passive([]);
            }
            None => bail!("data segment with a non-constant offset"),
        }
    }
    for (memory, offset, bytes) in &data_segments {
        data.active(*memory, &ConstExpr::i32_const(*offset), bytes.iter().copied());
    }
    if !data.is_empty() {
        module.section(&data);
    }

    // Names of the bodies it kept, for debugging and for wasm-opt's maps.
    // Kept bodies and trampolines alike carry their source name, so a
    // stack trace through a trampoline still says which function it is.
    let mut names = NameMap::new();
    for f in &order {
        if let Some(name) = source.func_names.get(f) {
            names.append(new_index[f], name);
        }
    }
    let mut name_section = NameSection::new();
    name_section.functions(&names);
    module.section(&name_section);
    if let Some(payload) = source.custom_payload("target_features") {
        module.section(&CustomSection { name: Cow::Borrowed("target_features"), data: Cow::Borrowed(payload) });
    }
    Ok(Emitted { bytes: module.finish() })
}

/// Re-encodes a body with its function references renumbered; every other
/// index (types, globals, tables, memories, data) is unchanged.
struct Renumber<'m> {
    new_index: &'m HashMap<u32, u32>,
}

impl Reencode for Renumber<'_> {
    type Error = std::convert::Infallible;

    fn function_index(&mut self, func: u32) -> Result<u32, reencode::Error<Self::Error>> {
        Ok(self.new_index[&func])
    }
}

/// `local.get 0..n; i32.const slot; call_indirect (type f) table`.
fn trampoline(source: &ModuleIndex<'_>, f: u32, slot: u32, table: u32) -> Result<wasm_encoder::Function> {
    let ty = source.func_types[f as usize];
    let params = *source.type_params.get(ty as usize).context("type index out of range")?;
    let mut func = wasm_encoder::Function::new([]);
    for p in 0..params {
        func.instructions().local_get(p);
    }
    func.instructions().i32_const(slot as i32);
    func.instructions().call_indirect(table, ty);
    func.instructions().end();
    Ok(func)
}

fn push_active_funcs(
    elems: &mut ElementSection,
    table: u32,
    offset: i32,
    expr_ref_type: Option<wasmparser::RefType>,
    items: &[u32],
) {
    let offset = ConstExpr::i32_const(offset);
    match expr_ref_type {
        None => {
            let table = (table != 0).then_some(table);
            elems.active(table, &offset, Elements::Functions(Cow::Borrowed(items)));
        }
        Some(_) => {
            let exprs: Vec<ConstExpr> = items.iter().map(|f| ConstExpr::ref_func(*f)).collect();
            elems.active(Some(table), &offset, Elements::Expressions(RefType::FUNCREF, Cow::Owned(exprs)));
        }
    }
}

fn entity_type(ty: &TypeRef) -> Result<EntityType> {
    Ok(match ty {
        TypeRef::Func(t) | TypeRef::FuncExact(t) => EntityType::Function(*t),
        TypeRef::Table(t) => EntityType::Table(table_type(t)?),
        TypeRef::Memory(m) => EntityType::Memory(memory_type(m)),
        TypeRef::Global(g) => EntityType::Global(global_type(g)?),
        TypeRef::Tag(_) => bail!("tag imports are not supported"),
    })
}

fn table_type(t: &wasmparser::TableType) -> Result<TableType> {
    Ok(TableType {
        element_type: ref_type(t.element_type)?,
        table64: t.table64,
        minimum: t.initial,
        maximum: t.maximum,
        shared: t.shared,
    })
}

fn ref_type(r: wasmparser::RefType) -> Result<RefType> {
    if r == wasmparser::RefType::FUNCREF {
        Ok(RefType::FUNCREF)
    } else if r == wasmparser::RefType::EXTERNREF {
        Ok(RefType::EXTERNREF)
    } else {
        bail!("unsupported table element type {r:?}")
    }
}

fn memory_type(m: &wasmparser::MemoryType) -> MemoryType {
    MemoryType {
        minimum: m.initial,
        maximum: m.maximum,
        memory64: m.memory64,
        shared: m.shared,
        page_size_log2: m.page_size_log2,
    }
}

fn global_type(g: &wasmparser::GlobalType) -> Result<GlobalType> {
    let val_type = match g.content_type {
        wasmparser::ValType::I32 => wasm_encoder::ValType::I32,
        wasmparser::ValType::I64 => wasm_encoder::ValType::I64,
        wasmparser::ValType::F32 => wasm_encoder::ValType::F32,
        wasmparser::ValType::F64 => wasm_encoder::ValType::F64,
        wasmparser::ValType::V128 => wasm_encoder::ValType::V128,
        wasmparser::ValType::Ref(r) => wasm_encoder::ValType::Ref(ref_type(r)?),
    };
    Ok(GlobalType { val_type, mutable: g.mutable, shared: g.shared })
}

fn export_kind(k: ExternalKind) -> Result<ExportKind> {
    Ok(match k {
        ExternalKind::Func | ExternalKind::FuncExact => ExportKind::Func,
        ExternalKind::Table => ExportKind::Table,
        ExternalKind::Memory => ExportKind::Memory,
        ExternalKind::Global => ExportKind::Global,
        ExternalKind::Tag => ExportKind::Tag,
    })
}

