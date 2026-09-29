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
const VT_ADDR: u32 = 16;

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

/// The fixture: `(module bytes)`. Used as both the rustc and the
/// bindgened module — the splitter pairs them by name.
fn fixture() -> Vec<u8> {
    // Symbols: 0 helper_main, 1 split_only, 2 split_leaf, 3 the split
    // import (undefined), 4 VT (data), 5 vtable_target.
    enum Op {
        Call(u32, u32), // function index, symbol
        Addr(u32),      // symbol
    }
    let bodies: [Vec<Op>; 6] = [
        vec![Op::Call(1, 3), Op::Call(3, 0)], // main
        vec![],                               // helper_main
        vec![Op::Call(5, 1), Op::Call(3, 0)], // split entry
        vec![Op::Call(6, 2), Op::Addr(4)],    // split_only
        vec![],                               // split_leaf
        vec![],                               // vtable_target
    ];

    let mut code = Vec::new();
    let mut code_relocs: Vec<(u8, u32, u32, bool)> = Vec::new(); // (type, offset, symbol, addend?)
    leb(&mut code, bodies.len() as u32);
    for ops in &bodies {
        let mut body = vec![0u8]; // no locals
        let mut sites = Vec::new();
        for op in ops {
            match op {
                Op::Call(f, sym) => {
                    body.push(0x10);
                    sites.push((0u8, body.len(), *sym, false));
                    body.extend_from_slice(&padded(*f));
                }
                Op::Addr(sym) => {
                    body.push(0x41);
                    sites.push((4u8, body.len(), *sym, true));
                    body.extend_from_slice(&padded(VT_ADDR));
                    body.push(0x1a); // drop
                }
            }
        }
        body.push(0x0b);
        assert!(body.len() < 128);
        code.push(body.len() as u8);
        let body_start = code.len();
        code.extend_from_slice(&body);
        for (ty, at, sym, addend) in sites {
            code_relocs.push((ty, (body_start + at) as u32, sym, addend));
        }
    }

    let mut m = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], []);
    m.section(&types);
    let mut imports = ImportSection::new();
    imports.import("./__wasm_split.js", LOAD, EntityType::Function(0));
    imports.import("./__wasm_split.js", IMPORT, EntityType::Function(0));
    m.section(&imports);
    let mut funcs = FunctionSection::new();
    for _ in 0..6 {
        funcs.function(0);
    }
    m.section(&funcs);
    let mut tables = TableSection::new();
    tables.table(TableType {
        element_type: RefType::FUNCREF,
        table64: false,
        minimum: 2,
        maximum: Some(2),
        shared: false,
    });
    m.section(&tables);
    let mut memories = MemorySection::new();
    memories.memory(MemoryType { minimum: 1, maximum: None, memory64: false, shared: false, page_size_log2: None });
    m.section(&memories);
    let mut globals = GlobalSection::new();
    globals.global(GlobalType { val_type: ValType::I32, mutable: true, shared: false }, &ConstExpr::i32_const(1024));
    m.section(&globals);
    let mut exports = ExportSection::new();
    exports.export("main", ExportKind::Func, 2);
    exports.export(EXPORT, ExportKind::Func, 4);
    exports.export("memory", ExportKind::Memory, 0);
    m.section(&exports);
    let mut elems = ElementSection::new();
    elems.active(None, &ConstExpr::i32_const(1), Elements::Functions([7u32][..].into()));
    m.section(&elems);
    m.section(&RawSection { id: 10, data: &code });
    let mut data = DataSection::new();
    data.active(0, &ConstExpr::i32_const(VT_ADDR as i32), 1u32.to_le_bytes());
    m.section(&data);

    let mut names = NameMap::new();
    for (i, n) in NAMES.iter().enumerate() {
        names.append(i as u32, n);
    }
    let mut name_section = NameSection::new();
    name_section.functions(&names);
    m.section(&name_section);

    let mut symbols = SymbolTable::new();
    symbols.function(0, 3, Some("helper_main"));
    symbols.function(0, 5, Some("split_only"));
    symbols.function(0, 6, Some("split_leaf"));
    symbols.function(SymbolTable::WASM_SYM_UNDEFINED, 1, None);
    symbols.data(0, "VT", Some(DataSymbolDefinition { index: 0, offset: 0, size: 4 }));
    symbols.function(0, 7, Some("vtable_target"));
    let mut linking = LinkingSection::new();
    linking.symbol_table(&symbols);
    m.section(&linking);

    // Sections in order: type 0, import 1, function 2, table 3, memory 4,
    // global 5, export 6, elem 7, code 8, data 9.
    let mut reloc_code = Vec::new();
    leb(&mut reloc_code, 8);
    leb(&mut reloc_code, code_relocs.len() as u32);
    for (ty, offset, sym, addend) in code_relocs {
        reloc_code.push(ty);
        leb(&mut reloc_code, offset);
        leb(&mut reloc_code, sym);
        if addend {
            reloc_code.push(0);
        }
    }
    m.section(&CustomSection { name: "reloc.CODE".into(), data: reloc_code.into() });
    // The data section payload: count, flags, `i32.const 16 end`, size —
    // six bytes before VT's word, which holds f7's table index.
    let mut reloc_data = Vec::new();
    leb(&mut reloc_data, 9);
    leb(&mut reloc_data, 1);
    reloc_data.push(2); // R_WASM_TABLE_INDEX_I32
    leb(&mut reloc_data, 6);
    leb(&mut reloc_data, 5);
    m.section(&CustomSection { name: "reloc.DATA".into(), data: reloc_data.into() });
    m.finish()
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
    assert!(split.reachable.contains(&Node::DataSymbol(4)));
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
    let unique: HashSet<Node> = [Node::DataSymbol(10), Node::DataSymbol(11)].into();
    let segments = wasm_carve::data::rematerialize(&source, &symbols, &unique);
    let got: Vec<(i32, Vec<u8>)> = segments.iter().map(|(_, a, b)| (*a, b.to_vec())).collect();
    assert_eq!(
        got,
        vec![(1024 + 8, (8u8..40).collect()), (4096 + 16, (116u8..148).collect())],
        "both the .rodata and the .data symbol are re-materialized at their addresses"
    );
}

/// Regression: a split module's re-materialized segments followed hash
/// order, so two builds of one input shipped different bytes.
#[test]
fn regression_rematerialized_segments_follow_symbol_order() {
    let (bytes, _) = two_segments();
    let source = ModuleIndex::parse(&bytes).unwrap();
    let symbols: std::collections::BTreeMap<usize, wasm_carve::graph::DataSymbol> = (0..16)
        .map(|i| {
            (i, wasm_carve::graph::DataSymbol {
                index: i,
                range: 0..0,
                segment_offset: i * 4,
                symbol_size: 4,
                which_data_segment: 0,
            })
        })
        .collect();
    let unique: HashSet<Node> = (0..16).map(Node::DataSymbol).collect();
    let addrs: Vec<i32> =
        wasm_carve::data::rematerialize(&source, &symbols, &unique).iter().map(|s| s.1).collect();
    assert_eq!(addrs, (0..16).map(|i| 1024 + i * 4).collect::<Vec<i32>>());
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

/// Pruning zeroes split-only symbols in segments a split output can
/// restore, and never one in a passive segment (nobody could put it back).
#[test]
fn regression_prune_skips_unrematerializable_segments() {
    let (bytes, symbols) = two_segments();
    let source = ModuleIndex::parse(&bytes).unwrap();
    let unused: HashSet<Node> = [10, 11, 12].into_iter().map(Node::DataSymbol).collect();
    let (section, stats) = wasm_carve::data::prune_main_data(&source, &symbols, &unused, 24);
    assert_eq!(stats.zeroed_bytes, 64);
    assert_eq!(stats.skipped_unrematerializable, 1);
    assert_eq!(stats.skipped_small, 0);
    let segs = data_bytes(&section);
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
    let unused: HashSet<Node> = [10, 11].into_iter().map(Node::DataSymbol).collect();
    let (_, stats) = wasm_carve::data::prune_main_data(&source, &symbols, &unused, 64);
    assert_eq!(stats.zeroed_bytes, 0);
    assert_eq!(stats.skipped_small, 2);
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
    assert_eq!(vt, [0, 0, 0, 0], "VT is only the split's, so main's copy is zeroed");
    let module = ModuleIndex::parse(&out.modules[0].bytes).unwrap();
    let restored: Vec<(i32, Vec<u8>)> = module
        .data
        .iter()
        .filter_map(|d| d.active_const.map(|(_, o)| (o, out.modules[0].bytes[d.data.clone()].to_vec())))
        .filter(|(_, b)| !b.is_empty())
        .collect();
    assert_eq!(restored, vec![(VT_ADDR as i32, 1u32.to_le_bytes().to_vec())]);
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
