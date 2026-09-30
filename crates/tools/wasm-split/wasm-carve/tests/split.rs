//! wasm-carve against a hand-built relocatable module.
//!
//! The splitter's input is a rustc/LLD module with `--emit-relocs`: a
//! `linking` symbol table plus `reloc.CODE` / `reloc.DATA`. walrus cannot
//! write those, so the fixture is assembled byte by byte, with padded
//! 5-byte LEB call operands the way LLD leaves them. Its shape is the
//! smallest one that exercises every edge kind the partition depends on:
//!
//! ```text
//! f0  import  __wasm_split_load_mod_abc_body         (loader)
//! f1  import  __wasm_split_00___mod___00_import_abc_body
//! f2  main          → call f1, call f3
//! f3  helper_main                          (main AND the split reach it)
//! f4  …_export_abc_body (split entry) → call f5, call f3
//! f5  split_only    → call f6, i32.const &VT       (reloc: MEMORY_ADDR)
//! f6  split_leaf
//! f7  vtable_target  — only reachable through VT, a data word holding
//!                      its TABLE index (reloc.DATA: TABLE_INDEX_I32);
//!                      table slot 1
//! ```
//!
//! f7 is the case that has broken splitters in this repo before: a function
//! no instruction names, reached through a pointer in data.

use std::collections::HashSet;

use wasm_carve::{
    graph::{Node, Partition},
    module::ModuleIndex,
};
use wasm_encoder::{
    ConstExpr, CustomSection, DataSection, ElementSection, Elements, EntityType, ExportKind,
    ExportSection, FunctionSection, GlobalSection, GlobalType, ImportSection, LinkingSection,
    MemorySection, MemoryType, Module, NameMap, NameSection, RawSection, RefType, SymbolTable,
    TableSection, TableType, TypeSection, ValType, DataSymbolDefinition,
};

const LOAD: &str = "__wasm_split_load_mod_abc_body";
const IMPORT: &str = "__wasm_split_00___mod___00_import_abc_body";
const EXPORT: &str = "__wasm_split_00___mod___00_export_abc_body";
const NAMES: [&str; 8] =
    [LOAD, IMPORT, "main", "helper_main", EXPORT, "split_only", "split_leaf", "vtable_target"];
const VT_ADDR: u32 = DATA_BASE;

fn padded(mut v: u32) -> [u8; 5] {
    let mut out = [0u8; 5];
    for (i, b) in out.iter_mut().enumerate() {
        *b = (v & 0x7f) as u8 | if i < 4 { 0x80 } else { 0 };
        v >>= 7;
    }
    out
}

fn leb(out: &mut Vec<u8>, mut v: u32) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

#[derive(Clone)]
enum Op {
    /// `call f` (FUNCTION_INDEX_LEB relocation).
    Call(u32),
    /// `i32.const &sym; drop` (MEMORY_ADDR_SLEB relocation).
    Addr(&'static str),
    /// `i32.const &sym; drop` with NO relocation — what a function
    /// wasm-bindgen wrote looks like.
    RawAddr(&'static str),
    /// `i32.const 0; drop` with a MEMORY_ADDR relocation against the first
    /// function import — an address relocation the analysis cannot account
    /// for.
    UntrackedAddr,
    /// `throw 0` — needs `Spec::jstag`.
    Throw,
}

/// What a data symbol's first word holds, with its `reloc.DATA` entry.
#[derive(Clone)]
enum Word {
    /// The table slot of a function (TABLE_INDEX_I32) — a vtable entry.
    FnPtr(u32),
    /// The address of another data symbol (MEMORY_ADDR_I32).
    DataPtr(&'static str),
    Plain,
}

#[derive(Clone)]
struct Spec {
    /// Function imports, from `./__wasm_split.js`.
    imports: Vec<&'static str>,
    /// Defined functions, numbered after the imports.
    funcs: Vec<(&'static str, Vec<Op>)>,
    exports: Vec<(&'static str, u32)>,
    /// Table slots from 1.
    elem: Vec<u32>,
    /// Data symbols, laid out back to back from `DATA_BASE`, 8 bytes each.
    data: Vec<(&'static str, Word)>,
    /// Functions with no symbol and no relocations — what wasm-bindgen
    /// adds. wasm-bindgen passes the rustc module's symbol table through
    /// untouched, so these must not shift anyone's symbol index.
    unlinked: Vec<&'static str>,
    /// Import tag 0 the way wasm-bindgen ≥ 0.2.128 imports
    /// `WebAssembly.JSTag`.
    jstag: bool,
}

const DATA_BASE: u32 = 16;
const DATA_SIZE: u32 = 8;

struct Built {
    bytes: Vec<u8>,
    sym: std::collections::HashMap<&'static str, u32>,
}

impl Spec {
    fn addr(&self, name: &str) -> u32 {
        DATA_BASE + DATA_SIZE * self.data.iter().position(|(n, _)| *n == name).unwrap() as u32
    }

    fn build(&self) -> Built {
        use std::collections::HashMap;
        let nimports = self.imports.len() as u32;
        // Symbols: imports (undefined), defined functions, data.
        let mut sym: HashMap<&'static str, u32> = HashMap::new();
        let mut symbols = SymbolTable::new();
        let mut next = 0u32;
        for (i, _) in self.imports.iter().enumerate() {
            symbols.function(SymbolTable::WASM_SYM_UNDEFINED, i as u32, None);
            next += 1;
        }
        for (i, (name, _)) in self.funcs.iter().enumerate() {
            if self.unlinked.contains(name) {
                continue;
            }
            symbols.function(0, nimports + i as u32, Some(name));
            sym.insert(name, next);
            next += 1;
        }
        for (i, (name, _)) in self.data.iter().enumerate() {
            symbols.data(0, name, Some(DataSymbolDefinition { index: 0, offset: i as u32 * DATA_SIZE, size: DATA_SIZE }));
            sym.insert(name, next);
            next += 1;
        }
        let func_sym = |f: u32| -> u32 {
            if f < nimports { f } else { sym[self.funcs[(f - nimports) as usize].0] }
        };

        let mut code = Vec::new();
        let mut code_relocs: Vec<(u8, u32, u32, bool)> = Vec::new();
        leb(&mut code, self.funcs.len() as u32);
        for (_, ops) in &self.funcs {
            let mut body = vec![0u8];
            let mut sites = Vec::new();
            for op in ops {
                match op {
                    Op::Call(f) => {
                        body.push(0x10);
                        sites.push((0u8, body.len(), func_sym(*f), false));
                        body.extend_from_slice(&padded(*f));
                    }
                    Op::Addr(name) => {
                        body.push(0x41);
                        sites.push((4u8, body.len(), sym[name], true));
                        body.extend_from_slice(&padded(self.addr(name)));
                        body.push(0x1a);
                    }
                    Op::RawAddr(name) => {
                        body.push(0x41);
                        body.extend_from_slice(&padded(self.addr(name)));
                        body.push(0x1a);
                    }
                    Op::UntrackedAddr => {
                        body.push(0x41);
                        sites.push((4u8, body.len(), 0, true));
                        body.extend_from_slice(&padded(0));
                        body.push(0x1a);
                    }
                    Op::Throw => body.extend_from_slice(&[0x08, 0x00]),
                }
            }
            body.push(0x0b);
            assert!(body.len() < 128);
            code.push(body.len() as u8);
            let body_start = code.len();
            code.extend_from_slice(&body);
            for (ty, at, s, addend) in sites {
                code_relocs.push((ty, (body_start + at) as u32, s, addend));
            }
        }

        let mut m = Module::new();
        let mut types = TypeSection::new();
        types.ty().function([], []);
        m.section(&types);
        let mut imports = ImportSection::new();
        for name in &self.imports {
            imports.import("./__wasm_split.js", name, EntityType::Function(0));
        }
        if self.jstag {
            imports.import(
                "__wbindgen_placeholder__",
                "__wbindgen_jstag",
                EntityType::Tag(wasm_encoder::TagType { kind: wasm_encoder::TagKind::Exception, func_type_idx: 0 }),
            );
        }
        m.section(&imports);
        let mut funcs = FunctionSection::new();
        for _ in &self.funcs {
            funcs.function(0);
        }
        m.section(&funcs);
        let slots = 1 + self.elem.len() as u64;
        let mut tables = TableSection::new();
        tables.table(TableType { element_type: RefType::FUNCREF, table64: false, minimum: slots, maximum: Some(slots), shared: false });
        m.section(&tables);
        let mut memories = MemorySection::new();
        memories.memory(MemoryType { minimum: 1, maximum: None, memory64: false, shared: false, page_size_log2: None });
        m.section(&memories);
        let mut globals = GlobalSection::new();
        globals.global(GlobalType { val_type: ValType::I32, mutable: true, shared: false }, &ConstExpr::i32_const(1024));
        m.section(&globals);
        let mut exports = ExportSection::new();
        for (name, f) in &self.exports {
            exports.export(name, ExportKind::Func, *f);
        }
        exports.export("memory", ExportKind::Memory, 0);
        m.section(&exports);
        let mut elems = ElementSection::new();
        elems.active(None, &ConstExpr::i32_const(1), Elements::Functions(self.elem.clone().into()));
        m.section(&elems);
        m.section(&RawSection { id: 10, data: &code });

        // One segment; each symbol's first word per `Word`, the rest zero.
        let mut bytes = Vec::new();
        let mut data_relocs: Vec<(u8, u32, u32)> = Vec::new();
        // Payload prefix: count, flags, `i32.const DATA_BASE end`, size LEB.
        let payload_prefix = 1 + 1 + 3 + 1;
        for (i, (_, word)) in self.data.iter().enumerate() {
            let at = payload_prefix + i as u32 * DATA_SIZE;
            let value = match word {
                Word::FnPtr(f) => {
                    data_relocs.push((2, at, func_sym(*f)));
                    1 + self.elem.iter().position(|e| e == f).unwrap() as u32
                }
                Word::DataPtr(name) => {
                    data_relocs.push((5, at, sym[name]));
                    self.addr(name)
                }
                Word::Plain => 0xAB00 + i as u32,
            };
            bytes.extend_from_slice(&value.to_le_bytes());
            bytes.extend_from_slice(&[0xCD; 4]);
        }
        assert!(bytes.len() < 128 && DATA_BASE < 64);
        let mut data = DataSection::new();
        data.active(0, &ConstExpr::i32_const(DATA_BASE as i32), bytes);
        m.section(&data);

        let mut names = NameMap::new();
        for (i, n) in self.imports.iter().enumerate() {
            names.append(i as u32, n);
        }
        for (i, (n, _)) in self.funcs.iter().enumerate() {
            names.append(nimports + i as u32, n);
        }
        let mut name_section = NameSection::new();
        name_section.functions(&names);
        m.section(&name_section);
        let mut linking = LinkingSection::new();
        linking.symbol_table(&symbols);
        m.section(&linking);

        // Sections: type 0, import 1, function 2, table 3, memory 4,
        // global 5, export 6, elem 7, code 8, data 9.
        let mut reloc_code = Vec::new();
        leb(&mut reloc_code, 8);
        leb(&mut reloc_code, code_relocs.len() as u32);
        for (ty, offset, s, addend) in code_relocs {
            reloc_code.push(ty);
            leb(&mut reloc_code, offset);
            leb(&mut reloc_code, s);
            if addend {
                reloc_code.push(0);
            }
        }
        m.section(&CustomSection { name: "reloc.CODE".into(), data: reloc_code.into() });
        let mut reloc_data = Vec::new();
        leb(&mut reloc_data, 9);
        leb(&mut reloc_data, data_relocs.len() as u32);
        for (ty, offset, s) in data_relocs {
            reloc_data.push(ty);
            leb(&mut reloc_data, offset);
            leb(&mut reloc_data, s);
            if ty == 5 {
                reloc_data.push(0);
            }
        }
        m.section(&CustomSection { name: "reloc.DATA".into(), data: reloc_data.into() });
        Built { bytes: m.finish(), sym }
    }
}

/// The fixture the module docs draw.
fn spec() -> Spec {
    Spec {
        imports: vec![LOAD, IMPORT],
        funcs: vec![
            ("main", vec![Op::Call(1), Op::Call(3)]),
            ("helper_main", vec![]),
            (EXPORT, vec![Op::Call(5), Op::Call(3)]),
            ("split_only", vec![Op::Call(6), Op::Addr("VT")]),
            ("split_leaf", vec![]),
            ("vtable_target", vec![]),
        ],
        exports: vec![("main", 2), (EXPORT, 4)],
        elem: vec![7],
        data: vec![("VT", Word::FnPtr(7))],
        unlinked: vec![],
        jstag: false,
    }
}

fn fixture() -> Vec<u8> {
    spec().build().bytes
}

/// VT's bytes: table slot 1, then filler.
fn vt_word() -> Vec<u8> {
    let mut w = 1u32.to_le_bytes().to_vec();
    w.extend_from_slice(&[0xCD; 4]);
    w
}

fn f(i: u32) -> Node {
    Node::Function(i)
}

fn validate(bytes: &[u8]) {
    wasmparser::Validator::new().validate_all(bytes).expect("output validates");
}

fn names(bytes: &[u8]) -> HashSet<String> {
    ModuleIndex::parse(bytes).unwrap().func_names.values().map(|s| s.to_string()).collect()
}

#[test]
fn the_fixture_is_a_valid_module() {
    validate(&fixture());
}

#[test]
fn partition_follows_calls_and_pointers_in_data() {
    let bytes = fixture();
    let source = ModuleIndex::parse(&bytes).unwrap();
    let p = Partition::compute(&bytes, &source).unwrap();
    assert_eq!(p.split_points.len(), 1);
    let split = &p.split_points[0];
    assert_eq!((split.import_func, split.export_func), (1, 4));
    for n in [f(4), f(5), f(6), f(7), f(3)] {
        assert!(split.reachable.contains(&n), "{n:?} reachable from the split");
    }
    // vtable_target is reached only through VT's relocation.
    let vt = spec().build().sym["VT"] as usize;
    assert!(split.reachable.contains(&Node::DataSymbol(vt)));
    for n in [f(2), f(3)] {
        assert!(p.main_graph.contains(&n), "{n:?} in main");
    }
    for n in [f(4), f(5), f(6), f(7)] {
        assert!(!p.main_graph.contains(&n), "{n:?} not in main");
    }
    assert_eq!(p.shared_funcs(), vec![0, 1, 3], "imports and the main code the split calls");
}

#[test]
fn every_output_validates() {
    let bytes = fixture();
    let out = wasm_carve::split(&bytes, &bytes, &Default::default()).unwrap();
    validate(&out.main.bytes);
    for m in out.modules.iter().chain(&out.chunks) {
        validate(&m.bytes);
    }
}

#[test]
fn main_turns_the_split_import_into_a_trampoline_and_hands_over_its_slots() {
    let bytes = fixture();
    let out = wasm_carve::split(&bytes, &bytes, &Default::default()).unwrap();
    let main = ModuleIndex::parse(&out.main.bytes).unwrap();
    assert!(main.imports.iter().any(|i| i.name == LOAD), "the loader stays an import");
    assert!(!main.imports.iter().any(|i| i.name == IMPORT), "the split import is a trampoline now");
    assert!(!main.exports.iter().any(|e| e.name == EXPORT), "the split entry leaves main");
    let kept = names(&out.main.bytes);
    for gone in ["split_only", "split_leaf", "vtable_target", EXPORT] {
        assert!(!kept.contains(gone), "{gone} is split code, not main's");
    }
    for stays in ["main", "helper_main"] {
        assert!(kept.contains(stays), "{stays} stays in main");
    }
    // Slot 1 held vtable_target; main now points it at the trapping dummy,
    // and the split module fills it when it loads.
    let slot1 = main.elems.iter().find(|e| e.active == Some((0, 1))).unwrap();
    let wasm_carve::module::ElemItem::Func(dummy) = slot1.items[0] else { panic!() };
    assert!(main.func_names.get(&dummy).is_none(), "slot 1 is the unnamed dummy");
    // The table grew past its old maximum by the split entry + 3 shared.
    assert_eq!(main.table_types[0].maximum, Some(2 + 1 + 3));
}

#[test]
fn the_module_owns_its_code_and_installs_it_into_the_table() {
    let bytes = fixture();
    let out = wasm_carve::split(&bytes, &bytes, &Default::default()).unwrap();
    let module = &out.modules[0];
    let m = ModuleIndex::parse(&module.bytes).unwrap();
    assert_eq!(m.func_imports, 0, "a split module imports no functions");
    let kept = names(&module.bytes);
    for owned in [EXPORT, "split_only", "split_leaf", "vtable_target"] {
        assert!(kept.contains(owned), "{owned} is the module's");
    }
    assert!(!kept.contains("main"), "main's code is not copied");
    // Installs: vtable_target at its source slot 1, the entry at the
    // first appended slot (the old maximum, 2).
    let installs: Vec<(i32, String)> = m
        .elems
        .iter()
        .map(|e| {
            let wasm_carve::module::ElemItem::Func(func) = e.items[0] else { panic!() };
            (e.active.unwrap().1, m.func_names[&func].to_string())
        })
        .collect();
    assert_eq!(installs, vec![(1, "vtable_target".to_string()), (2, EXPORT.to_string())]);
    assert!(m.exports.iter().any(|e| e.name == EXPORT));
}

#[test]
fn splitting_is_deterministic() {
    let bytes = fixture();
    let a = wasm_carve::split(&bytes, &bytes, &Default::default()).unwrap();
    let b = wasm_carve::split(&bytes, &bytes, &Default::default()).unwrap();
    assert_eq!(a.main.bytes, b.main.bytes);
    for (x, y) in a.modules.iter().zip(&b.modules) {
        assert_eq!(x.bytes, y.bytes);
    }
}

// --- carried over from the walrus-based splitter ----------------------

/// Two active segments (rustc's `.rodata` at 1024 and `.data` at 4096) and
/// a passive one, with one 32-byte symbol in each.
fn two_segments() -> (Vec<u8>, std::collections::BTreeMap<usize, wasm_carve::graph::DataSymbol>) {
    let mut m = Module::new();
    let mut memories = MemorySection::new();
    memories.memory(MemoryType { minimum: 1, maximum: None, memory64: false, shared: false, page_size_log2: None });
    m.section(&memories);
    let mut data = DataSection::new();
    data.active(0, &ConstExpr::i32_const(1024), 0u8..64);
    data.active(0, &ConstExpr::i32_const(4096), 100u8..164);
    data.passive(vec![7u8; 64]);
    m.section(&data);
    let symbol = |index: usize, segment: usize, offset: usize| wasm_carve::graph::DataSymbol {
        index,
        range: 0..0,
        segment_offset: offset,
        symbol_size: 32,
        which_data_segment: segment,
    };
    let symbols = [(10, symbol(10, 0, 8)), (11, symbol(11, 1, 16)), (12, symbol(12, 2, 0))].into();
    (m.finish(), symbols)
}

/// Regression for the `--data-prune` chunk corruption: a split-only
/// mutable static in `.data` (segment 1) was zeroed in main but never
/// re-added by the split output, which only walked segment 0 — the lazy
/// component read zeros and silently rendered nothing.
#[test]
fn regression_rematerializes_non_rodata_segments() {
    let (bytes, symbols) = two_segments();
    let source = ModuleIndex::parse(&bytes).unwrap();
    let segments = wasm_carve::data::rematerialize(&source, &symbols, &[10, 11], &zeroed(&symbols, &[10, 11], &[]));
    let got: Vec<(i32, Vec<u8>)> = segments.into_iter().map(|(_, a, b)| (a, b)).collect();
    assert_eq!(
        got,
        vec![(1024 + 8, (8u8..40).collect()), (4096 + 16, (116u8..148).collect())],
        "both the .rodata and the .data symbol are re-materialized at their addresses"
    );
}

fn zeroed(
    symbols: &std::collections::BTreeMap<usize, wasm_carve::graph::DataSymbol>,
    pruned: &[usize],
    live: &[usize],
) -> std::collections::BTreeMap<usize, Vec<std::ops::Range<usize>>> {
    wasm_carve::data::zeroed_ranges(
        symbols,
        &pruned.iter().copied().collect(),
        &live.iter().copied().collect(),
    )
}

/// Regression: symbols overlap (a constant nested inside a larger one), and
/// zeroing a dead symbol whole zeroed the live one inside it — on CrewForge
/// 317 KB of live bytes, on the website the `@font-face` URL table, so
/// every custom font failed to load. Only bytes no live symbol covers are
/// zeroed, and a split output restores only those — never a live byte main
/// may already have written.
#[test]
fn regression_pruning_never_zeroes_a_live_symbol_nested_in_a_dead_one() {
    let (bytes, _) = two_segments();
    let source = ModuleIndex::parse(&bytes).unwrap();
    let sym = |index: usize, offset: usize, size: usize| wasm_carve::graph::DataSymbol {
        index,
        range: 0..0,
        segment_offset: offset,
        symbol_size: size,
        which_data_segment: 0,
    };
    // DEAD spans bytes 0..32; LIVE is bytes 8..16 inside it.
    let symbols: std::collections::BTreeMap<_, _> = [(1, sym(1, 0, 32)), (2, sym(2, 8, 8))].into();
    let z = zeroed(&symbols, &[1], &[2]);
    assert_eq!(z[&0], vec![0..8, 16..32]);
    let segs = data_bytes(&wasm_carve::data::prune_main_data(&source, &z));
    assert!(segs[0][..8].iter().all(|b| *b == 0));
    assert_eq!(segs[0][8..16], (8u8..16).collect::<Vec<_>>()[..], "the live symbol survives");
    assert!(segs[0][16..32].iter().all(|b| *b == 0));
    let restored: Vec<(i32, Vec<u8>)> = wasm_carve::data::rematerialize(&source, &symbols, &[1], &z)
        .into_iter()
        .map(|(_, a, b)| (a, b))
        .collect();
    assert_eq!(
        restored,
        vec![(1024, (0u8..8).collect()), (1024 + 16, (16u8..32).collect())],
        "only the zeroed bytes come back"
    );
}

fn symbols_every(stride: usize, count: usize) -> std::collections::BTreeMap<usize, wasm_carve::graph::DataSymbol> {
    (0..count)
        .map(|i| {
            (i, wasm_carve::graph::DataSymbol {
                index: i,
                range: 0..0,
                segment_offset: i * stride,
                symbol_size: 4,
                which_data_segment: 0,
            })
        })
        .collect()
}

/// Regression: re-materialized segments followed hash order, so two builds
/// of one input shipped different bytes. They come out in address order
/// whatever order they are asked for in.
#[test]
fn regression_rematerialized_segments_follow_address_order() {
    let (bytes, _) = two_segments();
    let source = ModuleIndex::parse(&bytes).unwrap();
    let symbols = symbols_every(8, 8);
    let ids: Vec<usize> = vec![5, 1, 7, 3, 0];
    let addrs: Vec<i32> = wasm_carve::data::rematerialize(&source, &symbols, &ids, &zeroed(&symbols, &ids, &[]))
        .iter()
        .map(|s| s.1)
        .collect();
    assert_eq!(addrs, vec![1024, 1024 + 8, 1024 + 24, 1024 + 40, 1024 + 56]);
}

/// Symbols that sit back to back become one segment: a segment header per
/// symbol would cost more than many small symbols are worth.
#[test]
fn adjacent_symbols_are_restored_as_one_segment() {
    let (bytes, _) = two_segments();
    let source = ModuleIndex::parse(&bytes).unwrap();
    let symbols = symbols_every(4, 16);
    let ids: Vec<usize> = (0..16).collect();
    let segments = wasm_carve::data::rematerialize(&source, &symbols, &ids, &zeroed(&symbols, &ids, &[]));
    assert_eq!(segments.len(), 1);
    assert_eq!((segments[0].1, segments[0].2.clone()), (1024, (0u8..64).collect::<Vec<u8>>()));
}

fn data_bytes(section: &DataSection) -> Vec<Vec<u8>> {
    let mut m = Module::new();
    let mut memories = MemorySection::new();
    memories.memory(MemoryType { minimum: 1, maximum: None, memory64: false, shared: false, page_size_log2: None });
    m.section(&memories);
    m.section(section);
    let bytes = m.finish();
    let index = ModuleIndex::parse(&bytes).unwrap();
    index.data.iter().map(|d| bytes[d.data.clone()].to_vec()).collect()
}

/// Only symbols with a fixed address a split output could restore them at
/// are selected; a passive segment's are kept however dead.
#[test]
fn regression_prune_skips_unrematerializable_segments() {
    let (bytes, symbols) = two_segments();
    let source = ModuleIndex::parse(&bytes).unwrap();
    let (pruned, sel) =
        wasm_carve::liveness::select_prunable(&source, &symbols, &Default::default(), 24);
    assert_eq!(pruned.iter().copied().collect::<Vec<_>>(), vec![10, 11]);
    assert_eq!((sel.pruned_bytes, sel.skipped_unrestorable, sel.skipped_small), (64, 1, 0));
    let segs = data_bytes(&wasm_carve::data::prune_main_data(
        &source,
        &wasm_carve::data::zeroed_ranges(&symbols, &pruned, &Default::default()),
    ));
    assert!(segs[0][8..40].iter().all(|b| *b == 0));
    assert_eq!(segs[0][..8], (0u8..8).collect::<Vec<_>>()[..]);
    assert!(segs[1][16..48].iter().all(|b| *b == 0));
    assert_eq!(segs[1][..16], (100u8..116).collect::<Vec<_>>()[..]);
    assert!(segs[2].iter().all(|b| *b == 7), "the passive segment is untouched");
}

#[test]
fn prune_min_size_threshold_still_applies() {
    let (bytes, symbols) = two_segments();
    let source = ModuleIndex::parse(&bytes).unwrap();
    let (pruned, sel) =
        wasm_carve::liveness::select_prunable(&source, &symbols, &Default::default(), 64);
    assert!(pruned.is_empty());
    assert_eq!(sel.skipped_small, 2);
}

/// Live data is never selected, whatever its size.
#[test]
fn prune_never_selects_live_data() {
    let (bytes, symbols) = two_segments();
    let source = ModuleIndex::parse(&bytes).unwrap();
    let live: HashSet<usize> = [10].into();
    let (pruned, _) = wasm_carve::liveness::select_prunable(&source, &symbols, &live, 1);
    assert_eq!(pruned.iter().copied().collect::<Vec<_>>(), vec![11]);
}

/// With pruning on, main loses the split's data and the module restores it.
#[test]
fn data_prune_moves_split_only_data_out_of_main() {
    let bytes = fixture();
    let options = wasm_carve::SplitOptions { prune_dead_data_min: Some(4) };
    let out = wasm_carve::split(&bytes, &bytes, &options).unwrap();
    validate(&out.main.bytes);
    let main = ModuleIndex::parse(&out.main.bytes).unwrap();
    let vt = &out.main.bytes[main.data[0].data.clone()];
    assert_eq!(vt, [0; 8], "VT is only the split's, so main's copy is zeroed");
    let module = ModuleIndex::parse(&out.modules[0].bytes).unwrap();
    let restored: Vec<(i32, Vec<u8>)> = module
        .data
        .iter()
        .filter_map(|d| d.active_const.map(|(_, o)| (o, out.modules[0].bytes[d.data.clone()].to_vec())))
        .filter(|(_, b)| !b.is_empty())
        .collect();
    assert_eq!(restored, vec![(VT_ADDR as i32, vt_word())]);
}

/// Regression: the DCE-root exports use short `s{n}` names, never the
/// mangled symbol (~300 KB of export names on the website), and never
/// collide with an existing export.
#[test]
fn synthetic_export_names_are_short_and_dodge_collisions() {
    let mut used: HashSet<String> =
        ["main", "memory", "__wbindgen_malloc", "s1"].into_iter().map(String::from).collect();
    let mut next = 0;
    let names: Vec<String> =
        (0..3).map(|_| wasm_carve::emit::next_synthetic_export_name(&mut next, &mut used)).collect();
    assert_eq!(names, ["s0", "s2", "s3"]);
}

/// Regression: a failed split-module fetch must wake the Rust future with
/// `false`. The old glue logged and returned, so `load()` awaited forever.
#[test]
fn make_load_glue_signals_callback_on_failure() {
    let glue = wasm_carve::MAKE_LOAD_JS;
    let after = glue.split_once("Failed to load wasm-split module").expect("failure log").1;
    let before_signal = after.split_once("signal(false)").expect("failure signals false").0;
    assert!(!before_signal.contains("return;"), "no early return before signalling");
    assert!(glue.contains("signal(true)"));
}

/// wasm-bindgen 0.2.122's helper shape: `__wbindgen_malloc.command_export`
/// calls `__wasm_call_ctors`, then forwards to the bare helper, and is
/// exported under `export_name`.
fn wrapper_fixture(export_name: &str) -> Vec<u8> {
    let mut m = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], []);
    types.ty().function([ValType::I32, ValType::I32], [ValType::I32]);
    m.section(&types);
    let mut funcs = FunctionSection::new();
    funcs.function(0).function(1).function(1);
    m.section(&funcs);
    let mut exports = ExportSection::new();
    exports.export(export_name, ExportKind::Func, 2);
    m.section(&exports);
    let mut code = wasm_encoder::CodeSection::new();
    let mut ctors = wasm_encoder::Function::new([]);
    ctors.instructions().end();
    code.function(&ctors);
    let mut bare = wasm_encoder::Function::new([]);
    bare.instructions().i32_const(0).end();
    code.function(&bare);
    let mut wrapper = wasm_encoder::Function::new([]);
    wrapper.instructions().call(0).local_get(0).local_get(1).call(1).end();
    code.function(&wrapper);
    m.section(&code);
    let mut names = NameMap::new();
    names.append(0, "__wasm_call_ctors");
    names.append(1, "__wbindgen_malloc");
    names.append(2, "__wbindgen_malloc.command_export");
    let mut section = NameSection::new();
    section.functions(&names);
    m.section(&section);
    m.finish()
}

fn export_target(bytes: &[u8], export: &str) -> String {
    let m = ModuleIndex::parse(bytes).unwrap();
    let e = m.exports.iter().find(|e| e.name == export).unwrap();
    m.func_names[&e.index].to_string()
}

fn calls_ctors_first(bytes: &[u8], func: &str) -> bool {
    let m = ModuleIndex::parse(bytes).unwrap();
    let f = *m.func_names.iter().find(|(_, n)| **n == func).unwrap().0;
    let mut refs = Vec::new();
    m.direct_refs(f, &mut refs).unwrap();
    refs.first() == Some(&0)
}

/// Regression (wasm-bindgen 0.2.122): the suffixed export re-ran
/// `__wasm_call_ctors` on every JS↔wasm round trip, double-submitting
/// `inventory` items until a list traversal trapped. The export must land
/// on the bare helper, and the wrapper must stop calling the ctors (the
/// externref closure shim reaches it without the export).
#[test]
fn neutralize_repoints_the_export_and_strips_the_ctor_call() {
    let pre = wrapper_fixture("__wbindgen_malloc_command_export");
    assert_eq!(export_target(&pre, "__wbindgen_malloc_command_export"), "__wbindgen_malloc.command_export");
    assert!(calls_ctors_first(&pre, "__wbindgen_malloc.command_export"));
    let post = wasm_carve::neutralize::neutralize_command_export_wrappers(&pre).unwrap();
    validate(&post);
    assert_eq!(export_target(&post, "__wbindgen_malloc_command_export"), "__wbindgen_malloc");
    assert!(!calls_ctors_first(&post, "__wbindgen_malloc.command_export"));
}

/// A wrapper exported under a bare name (`main`) is the legitimate
/// one-time init and keeps its ctor call — and with nothing else to do the
/// pass hands the input back untouched.
#[test]
fn neutralize_leaves_the_main_wrapper_alone() {
    let pre = wrapper_fixture("main");
    let post = wasm_carve::neutralize::neutralize_command_export_wrappers(&pre).unwrap();
    assert!(matches!(post, std::borrow::Cow::Borrowed(_)));
    assert!(calls_ctors_first(&post, "__wbindgen_malloc.command_export"));
}

fn data_payload_bytes(bytes: &[u8]) -> usize {
    let m = ModuleIndex::parse(bytes).unwrap();
    m.data.iter().map(|d| d.data.len()).sum()
}

/// Regression: every split output re-wrote its own data symbols on load
/// even with `--data-prune` off, when main had already shipped and
/// initialized those bytes — 2 MB of CrewForge's split modules were a
/// second copy of main's data. Without pruning they carry none; main keeps
/// all of it.
#[test]
fn regression_split_outputs_do_not_duplicate_mains_data() {
    let bytes = fixture();
    let out = wasm_carve::split(&bytes, &bytes, &Default::default()).unwrap();
    for m in out.modules.iter().chain(&out.chunks) {
        validate(&m.bytes);
        assert_eq!(data_payload_bytes(&m.bytes), 0, "{} carries data main already has", m.module_name);
    }
    let main = ModuleIndex::parse(&out.main.bytes).unwrap();
    assert_eq!(out.main.bytes[main.data[0].data.clone()], vt_word()[..], "main still initializes VT");
}

// --- exact data liveness ---------------------------------------------

fn prune(min: usize) -> wasm_carve::SplitOptions {
    wasm_carve::SplitOptions { prune_dead_data_min: Some(min) }
}

fn main_data_at(out: &wasm_carve::OutputModules, addr: u32) -> Vec<u8> {
    let main = ModuleIndex::parse(&out.main.bytes).unwrap();
    let seg = &main.data[0];
    let base = seg.active_const.unwrap().1 as u32;
    let bytes = &out.main.bytes[seg.data.clone()];
    let at = (addr - base) as usize;
    bytes[at..at + DATA_SIZE as usize].to_vec()
}

fn restored(m: &wasm_carve::SplitModule) -> Vec<(i32, Vec<u8>)> {
    let idx = ModuleIndex::parse(&m.bytes).unwrap();
    idx.data
        .iter()
        .filter_map(|d| d.active_const.map(|(_, o)| (o, m.bytes[d.data.clone()].to_vec())))
        .filter(|(_, b)| !b.is_empty())
        .collect()
}

/// Data main reaches only through another symbol's pointer is live: main
/// reads PTR, PTR holds &VT. Zeroing VT would corrupt what main loads.
#[test]
fn main_keeps_data_it_reaches_only_through_data() {
    let mut s = spec();
    s.data = vec![("PTR", Word::DataPtr("VT")), ("VT", Word::FnPtr(7))];
    s.funcs[0].1.push(Op::Addr("PTR"));
    let b = s.build().bytes;
    let out = wasm_carve::split(&b, &b, &prune(1)).unwrap();
    validate(&out.main.bytes);
    assert_ne!(main_data_at(&out, s.addr("VT")), vec![0; 8], "VT is live through PTR");
    assert_ne!(main_data_at(&out, s.addr("PTR")), vec![0; 8]);
}

/// Regression guard for the corruption this analysis exists to prevent. A
/// function wasm-bindgen wrote has no relocations, so the partition's graph
/// never sees what it references: here a shim main exports holds &PTR,
/// PTR holds &VT, VT holds vtable_target's table slot — and the partition
/// hands that slot to the split module. Main must keep PTR and VT AND take
/// the slot back, or calling through it before the module loads traps.
#[test]
fn a_pointer_main_holds_keeps_its_data_and_takes_the_function_back() {
    let mut s = spec();
    s.data = vec![("PTR", Word::DataPtr("VT")), ("VT", Word::FnPtr(7))];
    let original = s.build().bytes;
    let mut bindgened_spec = s.clone();
    bindgened_spec.funcs.push(("__wbg_shim", vec![Op::RawAddr("PTR")]));
    bindgened_spec.unlinked.push("__wbg_shim");
    bindgened_spec.exports.push(("shim", 2 + bindgened_spec.funcs.len() as u32 - 1));
    let bindgened = bindgened_spec.build().bytes;

    // Without the shim's reference, the partition gives the slot away.
    let plain = wasm_carve::split(&original, &original, &prune(1)).unwrap();
    assert!(!names(&plain.main.bytes).contains("vtable_target"));

    let out = wasm_carve::split(&original, &bindgened, &prune(1)).unwrap();
    validate(&out.main.bytes);
    assert_ne!(main_data_at(&out, s.addr("VT")), vec![0; 8], "the shim reaches VT through PTR");
    assert_ne!(main_data_at(&out, s.addr("PTR")), vec![0; 8]);
    let main = ModuleIndex::parse(&out.main.bytes).unwrap();
    let slot1 = main.elems.iter().find(|e| e.active == Some((0, 1))).unwrap();
    let wasm_carve::module::ElemItem::Func(f) = slot1.items[0] else { panic!() };
    assert_eq!(main.func_names.get(&f).copied(), Some("vtable_target"), "main keeps the slot");
}

const LOAD_B: &str = "__wasm_split_load_modb_bbb_body";
const IMPORT_B: &str = "__wasm_split_00___modb___00_import_bbb_body";
const EXPORT_B: &str = "__wasm_split_00___modb___00_export_bbb_body";

/// Two split points reading one pruned symbol: it is restored ONCE, by the
/// shared chunk — a second restore when the other module loads would reset
/// a static the first had written — and both modules load the chunk first.
/// A symbol only one module reads is that module's to restore.
#[test]
fn shared_data_is_restored_once_by_the_chunk() {
    let s = Spec {
        imports: vec![LOAD, IMPORT, LOAD_B, IMPORT_B],
        funcs: vec![
            ("main", vec![Op::Call(1), Op::Call(3)]),                 // 4
            (EXPORT, vec![Op::Call(7), Op::Call(8)]),                 // 5
            (EXPORT_B, vec![Op::Call(7)]),                            // 6
            ("common", vec![Op::Addr("SHARED")]),                     // 7
            ("a_only", vec![Op::Addr("A_ONLY")]),                     // 8
        ],
        exports: vec![("main", 4), (EXPORT, 5), (EXPORT_B, 6)],
        elem: vec![],
        data: vec![("SHARED", Word::Plain), ("A_ONLY", Word::Plain)],
        unlinked: vec![],
        jstag: false,
    };
    let b = s.build().bytes;
    let out = wasm_carve::split(&b, &b, &prune(1)).unwrap();
    for m in std::iter::once(&out.main).chain(&out.modules).chain(&out.chunks) {
        validate(&m.bytes);
    }
    assert_eq!(main_data_at(&out, s.addr("SHARED")), vec![0; 8]);
    assert_eq!(main_data_at(&out, s.addr("A_ONLY")), vec![0; 8]);
    let addr = |n: &str| s.addr(n) as i32;
    assert_eq!(restored(&out.chunks[0]).iter().map(|r| r.0).collect::<Vec<_>>(), vec![addr("SHARED")]);
    let by_entry = |export: &str| {
        out.modules.iter().find(|m| ModuleIndex::parse(&m.bytes).unwrap().exports.iter().any(|e| e.name == export)).unwrap()
    };
    let (a, bmod) = (by_entry(EXPORT), by_entry(EXPORT_B));
    assert_eq!(restored(a).iter().map(|r| r.0).collect::<Vec<_>>(), vec![addr("A_ONLY")]);
    assert!(restored(bmod).is_empty());
    assert!(a.relies_on_chunks.contains(&0) && bmod.relies_on_chunks.contains(&0));
}

/// A memory-address relocation against something that is not a defined
/// data symbol means the relocations are not the whole story: prune
/// nothing rather than guess.
#[test]
fn pruning_is_refused_when_an_address_relocation_is_unaccounted_for() {
    let mut s = spec();
    s.funcs[1].1.push(Op::UntrackedAddr);
    let b = s.build().bytes;
    let out = wasm_carve::split(&b, &b, &prune(1)).unwrap();
    assert_eq!(main_data_at(&out, s.addr("VT")), vt_word(), "main keeps all its data");
    assert!(out.modules.iter().all(|m| restored(m).is_empty()));
}


/// A split whose code throws against wasm-bindgen's imported JSTag.
fn jstag_fixture() -> Vec<u8> {
    let mut s = spec();
    s.jstag = true;
    s.funcs[4].1.push(Op::Throw); // split_leaf
    s.build().bytes
}

/// Regression (wasm-bindgen 0.2.128): its hot-reload and release bases
/// import `WebAssembly.JSTag` as an exception tag, and wasm-carve bailed
/// with "tag imports are not supported" — both in the `command_export`
/// neutralize pass every web build runs and in the splitter, so
/// `idealyst dev --web` could not produce a first build at all.
#[test]
fn regression_a_jstag_import_neutralizes_and_splits() {
    let bytes = jstag_fixture();
    validate(&bytes);
    let m = ModuleIndex::parse(&bytes).unwrap();
    assert_eq!((m.tag_imports, m.tag_types.len()), (1, 1));
    let post = wasm_carve::neutralize::neutralize_command_export_wrappers(&bytes).unwrap();
    assert!(matches!(post, std::borrow::Cow::Borrowed(_)), "nothing to neutralize");

    let out = wasm_carve::split(&bytes, &bytes, &Default::default()).unwrap();
    validate(&out.main.bytes);
    let main = ModuleIndex::parse(&out.main.bytes).unwrap();
    assert!(
        main.imports.iter().any(|i| i.name == "__wbindgen_jstag"),
        "main keeps importing the tag from JS"
    );
    let export = main
        .exports
        .iter()
        .find(|e| e.kind == wasmparser::ExternalKind::Tag)
        .expect("main exports the tag for the splits");
    assert_eq!(export.index, 0);

    // The split that throws imports MAIN's tag, under the name main
    // exported it as, so a throw in one module is caught in the other.
    let split = &out.modules[0];
    validate(&split.bytes);
    let sm = ModuleIndex::parse(&split.bytes).unwrap();
    assert!(names(&split.bytes).contains("split_leaf"));
    let tag_imports: Vec<_> = sm
        .imports
        .iter()
        .filter(|i| matches!(i.ty, wasmparser::TypeRef::Tag(_)))
        .map(|i| (i.module, i.name))
        .collect();
    assert_eq!(tag_imports, vec![("__wasm_split", export.name)]);
}
