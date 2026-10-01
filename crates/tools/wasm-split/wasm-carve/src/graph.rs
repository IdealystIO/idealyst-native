//! Who reaches what: the partition of a module into main, one module per
//! split point, and the shared chunk.
//!
//! The same analysis as Dioxus's walrus-based `wasm-split-cli`, which this
//! crate replaced, with the same results and no IR. The edges come from two places:
//!
//! * the rustc module's relocations (`reloc.CODE` / `reloc.DATA` against
//!   the `linking` symbol table) — the only record of which DATA holds a
//!   function pointer, which is how vtable and closure targets are reached;
//! * a streaming walk of the bindgened module's `call` / `return_call` /
//!   `ref.func` operands, which covers the shims wasm-bindgen added.
//!
//! The two modules are paired by function NAME, and every same-named copy
//! inherits every copy's edges (LLVM under `opt-level=z` emits distinct
//! functions sharing a mangled name. Keyed 1:1 by name, their edges landed
//! on an arbitrary copy and the other was gutted from main while a
//! main-resident vtable still pointed at it — "function signature
//! mismatch" at boot; `tests/lazy-many-splits` is that shape).

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    ops::Range,
};

use anyhow::{Context, Result};
use wasmparser::{
    BinaryReader, ExternalKind, KnownCustom, Linking, LinkingSectionReader, Parser, Payload,
    RelocSectionReader, RelocationEntry, SymbolInfo, TypeRef,
};

use crate::module::ModuleIndex;

/// A function (by index in the BINDGENED module) or a data symbol (by index
/// in the linking symbol table, which wasm-bindgen passes through unchanged).
#[derive(Debug, PartialEq, Eq, Hash, Copy, Clone, PartialOrd, Ord)]
pub enum Node {
    Function(u32),
    DataSymbol(usize),
}

#[derive(Debug, Clone)]
pub struct DataSymbol {
    pub index: usize,
    /// Absolute file range of the symbol's bytes.
    pub range: Range<usize>,
    pub segment_offset: usize,
    pub symbol_size: usize,
    pub which_data_segment: usize,
}

#[derive(Debug, Clone)]
pub struct SplitPoint {
    pub module_name: String,
    pub import_name: String,
    /// The `…_import_…` function main calls to enter the module.
    pub import_func: u32,
    /// The `…_export_…` function that IS the module's entry.
    pub export_func: u32,
    pub export_name: String,
    pub hash_name: String,
    pub component_name: String,
    pub index: usize,
    pub reachable: HashSet<Node>,
}

pub struct Partition {
    pub split_points: Vec<SplitPoint>,
    pub main_graph: HashSet<Node>,
    /// The shared chunk: split-only symbols more than one split reaches.
    pub chunks: Vec<HashSet<Node>>,
    /// Main-resident functions (and every function import) the splits
    /// reach, in index order: each gets a slot appended to the table.
    pub shared_symbols: BTreeSet<Node>,
    pub call_graph: HashMap<Node, HashSet<Node>>,
    pub data_symbols: BTreeMap<usize, DataSymbol>,
    /// The rustc module's reference graph, for exact data liveness.
    pub original: OriginalGraph,
    pub names: NameMap,
}

impl Partition {
    pub fn compute(original: &[u8], source: &ModuleIndex<'_>) -> Result<Self> {
        let split_points = accumulate_split_points(source)?;
        let data_symbols = packaged_data_symbols(source.bytes, original)?;

        let (mut call_graph, original_graph, names) = build_call_graph(original, source)?;

        let mut split_points = split_points;
        for split in &mut split_points {
            let roots: HashSet<_> = [Node::Function(split.export_func)].into();
            split.reachable = reachable_graph(&call_graph, &roots);
        }
        let main_graph = reachable_graph(&call_graph, &main_roots(source, &split_points));

        let mut shared = HashSet::new();
        for split in &split_points {
            shared.extend(main_graph.intersection(&split.reachable).copied());
        }
        for f in 0..source.func_imports {
            shared.insert(Node::Function(f));
        }
        let shared_symbols: BTreeSet<Node> = shared.into_iter().collect();

        let chunks = vec![compute_shared_chunk(
            split_points.iter().map(|s| &s.reachable),
            &main_graph,
        )];

        // Keep the graph only for callers that want to walk it again.
        call_graph.shrink_to_fit();
        Ok(Partition {
            split_points,
            main_graph,
            chunks,
            shared_symbols,
            call_graph,
            data_symbols,
            original: original_graph,
            names,
        })
    }

    /// Split-reached symbols main neither reaches nor exports: the table
    /// slots main hands over to the split modules.
    pub fn unused_main_symbols(&self, source: &ModuleIndex<'_>) -> HashSet<Node> {
        let exported: HashSet<u32> = source
            .exports
            .iter()
            .filter(|e| e.kind == ExternalKind::Func)
            .map(|e| e.index)
            .collect();
        self.split_points
            .iter()
            .flat_map(|split| split.reachable.iter())
            .filter(|sym| !self.main_graph.contains(sym))
            .filter(|sym| match sym {
                Node::Function(f) => !exported.contains(f),
                Node::DataSymbol(_) => true,
            })
            .copied()
            .collect()
    }

    /// Functions shared through the table, in slot order.
    pub fn shared_funcs(&self) -> Vec<u32> {
        self.shared_symbols
            .iter()
            .filter_map(|n| match n {
                Node::Function(f) => Some(*f),
                Node::DataSymbol(_) => None,
            })
            .collect()
    }
}

fn accumulate_split_points(source: &ModuleIndex<'_>) -> Result<Vec<SplitPoint>> {
    // Function imports in the order the index space assigns them.
    let mut func_imports = Vec::new();
    for import in &source.imports {
        if matches!(import.ty, TypeRef::Func(_) | TypeRef::FuncExact(_)) {
            func_imports.push(import);
        }
    }
    let mut named: Vec<(u32, &str)> = func_imports
        .iter()
        .enumerate()
        .filter(|(_, i)| i.name.starts_with("__wasm_split_00"))
        .map(|(idx, i)| (idx as u32, i.name))
        .collect();
    named.sort_by(|a, b| a.1.cmp(b.1));

    let mut points = Vec::new();
    for (index, (import_func, name)) in named.into_iter().enumerate() {
        let remain = name.trim_start_matches("__wasm_split_00___");
        let (module_name, rest) = remain.split_once("___00").context("split import name")?;
        let (hash, fn_name) = rest
            .trim_start_matches("_import_")
            .split_once('_')
            .context("split import hash")?;
        let export_name = format!("__wasm_split_00___{module_name}___00_export_{hash}_{fn_name}");
        let export_func = source
            .exports
            .iter()
            .find(|e| e.kind == ExternalKind::Func && e.name == export_name)
            .with_context(|| format!("no export {export_name}"))?
            .index;
        points.push(SplitPoint {
            module_name: module_name.to_string(),
            import_name: name.to_string(),
            import_func,
            export_func,
            export_name,
            hash_name: hash.to_string(),
            component_name: fn_name.to_string(),
            index,
            reachable: HashSet::new(),
        });
    }
    Ok(points)
}

fn main_roots(source: &ModuleIndex<'_>, splits: &[SplitPoint]) -> HashSet<Node> {
    let split_exports: HashSet<u32> = splits.iter().map(|s| s.export_func).collect();
    let mut roots: HashSet<Node> = source
        .exports
        .iter()
        .filter(|e| e.kind == ExternalKind::Func && !split_exports.contains(&e.index))
        .map(|e| Node::Function(e.index))
        .collect();
    if let Some(start) = source.start {
        roots.insert(Node::Function(start));
    }
    for f in 0..source.func_imports {
        roots.insert(Node::Function(f));
    }
    roots
}

pub fn reachable_graph(deps: &HashMap<Node, HashSet<Node>>, roots: &HashSet<Node>) -> HashSet<Node> {
    let mut queue: VecDeque<Node> = roots.iter().copied().collect();
    let mut reachable = HashSet::new();
    while let Some(node) = queue.pop_front() {
        if !reachable.insert(node) {
            continue;
        }
        if let Some(children) = deps.get(&node) {
            for child in children {
                if !reachable.contains(child) {
                    queue.push_back(*child);
                }
            }
        }
    }
    reachable
}

fn compute_shared_chunk<'a>(
    split_graphs: impl Iterator<Item = &'a HashSet<Node>>,
    main_graph: &HashSet<Node>,
) -> HashSet<Node> {
    let mut used_by: HashMap<Node, usize> = HashMap::new();
    for split in split_graphs {
        for item in split {
            if !main_graph.contains(item) {
                *used_by.entry(*item).or_insert(0) += 1;
            }
        }
    }
    used_by.into_iter().filter(|(_, n)| *n > 1).map(|(node, _)| node).collect()
}

/// A node of the ORIGINAL (rustc) module: function by its own index, or
/// data symbol.
#[derive(Debug, PartialEq, Eq, Hash, Copy, Clone)]
pub enum OldNode {
    Function(u32),
    DataSymbol(usize),
}

/// Pairs each bindgened function with every same-named function of the
/// rustc module, and back.
pub struct NameMap {
    pub new_to_old: HashMap<u32, Vec<u32>>,
    pub old_to_new: HashMap<u32, Vec<u32>>,
}

fn build_call_graph(
    original: &[u8],
    source: &ModuleIndex<'_>,
) -> Result<(HashMap<Node, HashSet<Node>>, OriginalGraph, NameMap)> {
    let old = ModuleIndex::parse(original)?;
    let old_graph = OriginalGraph::build(&old)?;

    let mut old_names: HashMap<&str, Vec<u32>> = HashMap::new();
    for f in 0..old.total_funcs() {
        if let Some(name) = old.func_names.get(&f) {
            old_names.entry(name).or_default().push(f);
        }
    }
    let mut new_names: HashMap<&str, Vec<u32>> = HashMap::new();
    for f in 0..source.total_funcs() {
        if let Some(name) = source.func_names.get(&f) {
            new_names.entry(name).or_default().push(f);
        }
    }

    // Old copy → ALL same-named new copies (see the module docs).
    let mut old_to_new: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut new_call_graph: HashMap<Node, HashSet<Node>> = HashMap::new();
    for (name, new_funcs) in &new_names {
        if let Some(old_funcs) = old_names.get(name) {
            for old_func in old_funcs {
                old_to_new.entry(*old_func).or_default().extend(new_funcs.iter().copied());
            }
        } else {
            for f in new_funcs {
                new_call_graph.insert(Node::Function(*f), HashSet::new());
            }
        }
    }
    let get_new = |old: &OldNode| -> Option<Vec<Node>> {
        match old {
            OldNode::Function(f) => old_to_new
                .get(f)
                .map(|new| new.iter().map(|n| Node::Function(*n)).collect()),
            OldNode::DataSymbol(s) => Some(vec![Node::DataSymbol(*s)]),
        }
    };

    // Old functions wasm-bindgen dissolved (describe functions): every
    // descendant is attached to main below.
    let mut lost_children: HashSet<OldNode> = HashSet::new();
    let mut graph: HashMap<Node, HashSet<Node>> = HashMap::new();
    for (old_parent, children) in &old_graph.call_graph {
        let Some(new_parents) = get_new(old_parent) else {
            let mut stack: Vec<OldNode> = children.iter().copied().collect();
            while let Some(node) = stack.pop() {
                if lost_children.insert(node) {
                    if let Some(grand) = old_graph.call_graph.get(&node) {
                        stack.extend(grand.iter().copied());
                    }
                }
            }
            continue;
        };
        let mut new_children = HashSet::new();
        for child in children {
            if let Some(new) = get_new(child) {
                new_children.extend(new);
            }
        }
        for parent in new_parents {
            graph.entry(parent).or_default().extend(new_children.iter().copied());
        }
    }

    let mut recovered: HashSet<Node> = HashSet::new();
    for lost in lost_children {
        match lost {
            OldNode::Function(f) => {
                let name = old.func_names.get(&f).context("lost function has no name")?;
                if let Some(entries) = new_names.get(name) {
                    recovered.extend(entries.iter().map(|e| Node::Function(*e)));
                }
            }
            OldNode::DataSymbol(s) => {
                recovered.insert(Node::DataSymbol(s));
            }
        }
    }

    let main_fn = (0..source.total_funcs())
        .find(|f| source.func_names.get(f) == Some(&"main"))
        .context("no `main` function — built without --emit-relocs and a name section?")?;
    let main_entry = new_call_graph.entry(Node::Function(main_fn)).or_default();
    main_entry.extend(recovered);
    for (name, new_funcs) in &new_names {
        if !old_names.contains_key(name) {
            main_entry.extend(new_funcs.iter().map(|f| Node::Function(*f)));
        }
    }
    for (node, children) in new_call_graph {
        graph.entry(node).or_default().extend(children);
    }

    // Direct references in the bindgened module itself.
    let mut refs = Vec::new();
    for f in source.func_imports..source.total_funcs() {
        refs.clear();
        source.direct_refs(f, &mut refs)?;
        if refs.is_empty() {
            continue;
        }
        graph
            .entry(Node::Function(f))
            .or_default()
            .extend(refs.iter().map(|r| Node::Function(*r)));
    }
    let mut new_to_old: HashMap<u32, Vec<u32>> = HashMap::new();
    for (old_f, new_fs) in &old_to_new {
        for new_f in new_fs {
            new_to_old.entry(*new_f).or_default().push(*old_f);
        }
    }
    Ok((graph, old_graph, NameMap { new_to_old, old_to_new }))
}

/// The rustc module's own graph, from its relocations: every function or
/// data symbol each function body and each data symbol refers to.
pub struct OriginalGraph {
    pub call_graph: HashMap<OldNode, HashSet<OldNode>>,
    /// Function pointers taken in CODE (`TABLE_INDEX_*` relocations): the
    /// functions whose table slot a function's body can produce. Kept apart
    /// from `call_graph`, which mixes them with direct calls.
    pub code_fn_ptrs: HashMap<u32, Vec<u32>>,
    /// Why the relocations cannot be trusted as a complete record of data
    /// references, if they cannot — a memory-address relocation against
    /// anything but a defined data symbol. Pruning is refused then.
    pub unsound: Option<String>,
}

impl OriginalGraph {
    fn build(old: &ModuleIndex<'_>) -> Result<Self> {
        let raw = parse_data_symbols(old.bytes)?;
        let code_relocs = relocations(old, "reloc.CODE")?;
        let data_relocs = relocations(old, "reloc.DATA")?;
        let dep = |index: u32| -> Option<OldNode> {
            match raw.symbols.get(index as usize)? {
                SymbolInfo::Data { .. } => Some(OldNode::DataSymbol(index as usize)),
                SymbolInfo::Func { index: func, .. } => {
                    (*func < old.total_funcs()).then_some(OldNode::Function(*func))
                }
                _ => None,
            }
        };

        let mut call_graph: HashMap<OldNode, HashSet<OldNode>> = HashMap::new();
        let mut code_fn_ptrs: HashMap<u32, Vec<u32>> = HashMap::new();
        let mut unsound: Option<String> = None;
        let mut check = |entry: &RelocationEntry| {
            if unsound.is_none() && format!("{:?}", entry.ty).starts_with("MemoryAddr") {
                match raw.symbols.get(entry.index as usize) {
                    Some(SymbolInfo::Data { symbol: Some(_), .. }) => {}
                    other => {
                        unsound = Some(format!("{:?} relocation against {other:?}", entry.ty));
                    }
                }
            }
        };

        // Code: each relocation lands in exactly one body, walked in order.
        let mut relocs = code_relocs.iter().peekable();
        for (pos, (_, body)) in old.bodies.iter().enumerate() {
            let range = walrus_range(old, body);
            let func = OldNode::Function(old.func_imports + pos as u32);
            while let Some(entry) = relocs.next_if(|e| e.relocation_range().start < range.end) {
                let r = entry.relocation_range();
                anyhow::ensure!(r.start >= range.start && r.end <= range.end, "reloc outside its body");
                check(entry);
                if let Some(target) = dep(entry.index) {
                    if let (OldNode::Function(f), true) =
                        (target, format!("{:?}", entry.ty).starts_with("TableIndex"))
                    {
                        code_fn_ptrs.entry(old.func_imports + pos as u32).or_default().push(f);
                    }
                    call_graph.entry(func).or_default().insert(target);
                }
            }
        }
        anyhow::ensure!(relocs.next().is_none(), "reloc.CODE entries past the last body");

        // Data: each relocation lands in one data symbol.
        let mut relocs = data_relocs.iter().peekable();
        let mut sorted: Vec<&DataSymbol> = raw.data_symbols.values().collect();
        sorted.sort_by_key(|s| s.range.start);
        for symbol in sorted {
            let start = symbol.range.start - raw.data_range.start;
            let end = symbol.range.end - raw.data_range.start;
            while let Some(entry) = relocs.next_if(|e| e.relocation_range().start < end) {
                let r = entry.relocation_range();
                anyhow::ensure!(r.start >= start && r.end <= end, "reloc outside its data symbol");
                check(entry);
                if let Some(target) = dep(entry.index) {
                    call_graph.entry(OldNode::DataSymbol(symbol.index)).or_default().insert(target);
                }
            }
        }
        anyhow::ensure!(relocs.next().is_none(), "reloc.DATA entries past the last symbol");
        Ok(OriginalGraph { call_graph, code_fn_ptrs, unsound })
    }
}

/// A body's range the way walrus reports `original_range` (and the way the
/// relocation offsets were matched against it): relative to the code
/// payload, starting where a MINIMAL size LEB would start.
fn walrus_range(m: &ModuleIndex<'_>, body: &Range<usize>) -> Range<usize> {
    let size = body.end - body.start;
    let prefix = (usize::BITS - size.leading_zeros() - 1) as usize / 7 + 1;
    body.start - m.code_payload_start - prefix..body.end - m.code_payload_start
}

fn relocations(m: &ModuleIndex<'_>, name: &str) -> Result<Vec<RelocationEntry>> {
    let payload = m
        .custom_payload(name)
        .with_context(|| format!("module has no {name} section (built without --emit-relocs?)"))?;
    let reader = RelocSectionReader::new(BinaryReader::new(payload, 0))?;
    Ok(reader.entries().into_iter().collect::<Result<_, _>>()?)
}

pub struct RawData<'a> {
    pub data_range: Range<usize>,
    pub symbols: Vec<SymbolInfo<'a>>,
    pub data_symbols: BTreeMap<usize, DataSymbol>,
}

/// The packaged module's data symbols: addresses from ITS data section, the
/// symbol table from its `linking` section — or, when packaging dropped
/// that section, from the rustc module's.
///
/// wasm-bindgen carries `linking` through to its output; the own-mode glue
/// pass strips it (with `reloc.*` and DWARF) because nothing reads it from
/// the served module. Without the fallback an own-mode split saw no data
/// symbols at all, so `--data-prune` zeroed nothing (lazy-payload-split's
/// main stayed 1409 KiB where it must shed the chunk's 512 KiB). The two
/// tables are the same table: packaging renames imports and repoints exports
/// but never renumbers symbols or moves data, so the rustc module's
/// `(segment, offset, size)` entries describe the packaged data section.
pub fn packaged_data_symbols(packaged: &[u8], original: &[u8]) -> Result<BTreeMap<usize, DataSymbol>> {
    let raw = parse_data_symbols(packaged)?;
    if !raw.symbols.is_empty() {
        return Ok(raw.data_symbols);
    }
    let symbols = parse_data_symbols(original)?.symbols;
    let segments = data_segments(packaged)?;
    data_symbols_of(&symbols, &segments)
}

fn data_segments(bytes: &[u8]) -> Result<Vec<wasmparser::Data<'_>>> {
    for payload in Parser::new(0).parse_all(bytes) {
        if let Payload::DataSection(section) = payload? {
            return Ok(section.into_iter().collect::<Result<Vec<_>, _>>()?);
        }
    }
    Ok(Vec::new())
}

fn data_symbols_of(symbols: &[SymbolInfo<'_>], segments: &[wasmparser::Data<'_>]) -> Result<BTreeMap<usize, DataSymbol>> {
    let mut data_symbols = BTreeMap::new();
    for (index, symbol) in symbols.iter().enumerate() {
        let SymbolInfo::Data { symbol: Some(def), .. } = symbol else { continue };
        if def.size == 0 {
            continue;
        }
        let segment = segments.get(def.index as usize).context("data symbol's segment")?;
        let offset = segment.range.end - segment.data.len() + def.offset as usize;
        data_symbols.insert(
            index,
            DataSymbol {
                index,
                range: offset..offset + def.size as usize,
                segment_offset: def.offset as usize,
                symbol_size: def.size as usize,
                which_data_segment: def.index as usize,
            },
        );
    }
    Ok(data_symbols)
}

pub fn parse_data_symbols(bytes: &[u8]) -> Result<RawData<'_>> {
    let mut segments = Vec::new();
    let mut data_range = 0..0;
    let mut symbols = Vec::new();
    for payload in Parser::new(0).parse_all(bytes) {
        match payload? {
            Payload::DataSection(section) => {
                data_range = section.range();
                segments = section.into_iter().collect::<Result<Vec<_>, _>>()?;
            }
            Payload::CustomSection(section) => {
                if let KnownCustom::Linking(reader) = section.as_known() {
                    for sub in reader.subsections() {
                        if let Linking::SymbolTable(map) = sub? {
                            symbols = map.into_iter().collect::<Result<Vec<_>, _>>()?;
                        }
                    }
                } else if section.name() == "linking" {
                    let reader = LinkingSectionReader::new(BinaryReader::new(section.data(), 0))?;
                    for sub in reader.subsections() {
                        if let Linking::SymbolTable(map) = sub? {
                            symbols = map.into_iter().collect::<Result<Vec<_>, _>>()?;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let data_symbols = data_symbols_of(&symbols, &segments)?;
    Ok(RawData { data_range, symbols, data_symbols })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(ids: &[u32]) -> HashSet<Node> {
        ids.iter().map(|i| Node::Function(*i)).collect()
    }

    /// Regression: code more than one split reaches, and main does not,
    /// goes to the shared chunk. The original tally loop was empty, so the
    /// chunk was always empty and every module embedded the whole engine.
    #[test]
    fn shared_chunk_holds_only_multi_module_split_symbols() {
        let main = set(&[0, 1]);
        let (a, b) = (set(&[1, 2, 3]), set(&[1, 3, 4]));
        assert_eq!(compute_shared_chunk([&a, &b].into_iter(), &main), set(&[3]));
    }

    #[test]
    fn shared_chunk_is_empty_with_a_single_split_point() {
        let a = set(&[1, 2, 3]);
        assert!(compute_shared_chunk([&a].into_iter(), &set(&[0])).is_empty());
    }

    #[test]
    fn shared_chunk_excludes_symbols_promoted_to_main() {
        let main = set(&[7]);
        let (a, b) = (set(&[7, 8]), set(&[7, 8]));
        assert_eq!(compute_shared_chunk([&a, &b].into_iter(), &main), set(&[8]));
    }
}
