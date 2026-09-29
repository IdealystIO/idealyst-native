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
    let out = wasm_carve::split(&bytes, &bytes).unwrap();
    validate(&out.main.bytes);
    for m in out.modules.iter().chain(&out.chunks) {
        validate(&m.bytes);
    }
}

#[test]
fn main_turns_the_split_import_into_a_trampoline_and_hands_over_its_slots() {
    let bytes = fixture();
    let out = wasm_carve::split(&bytes, &bytes).unwrap();
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
    let out = wasm_carve::split(&bytes, &bytes).unwrap();
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
    let a = wasm_carve::split(&bytes, &bytes).unwrap();
    let b = wasm_carve::split(&bytes, &bytes).unwrap();
    assert_eq!(a.main.bytes, b.main.bytes);
    for (x, y) in a.modules.iter().zip(&b.modules) {
        assert_eq!(x.bytes, y.bytes);
    }
}

/// Differential: the module keeps the same functions wasm-split-cli's
/// does. (walrus names its trampolines `stub`; those are left out.)
#[test]
fn the_module_keeps_what_wasm_split_cli_keeps() {
    let bytes = fixture();
    let ours = wasm_carve::split(&bytes, &bytes).unwrap();
    let theirs = wasm_split_cli::Splitter::new(&bytes, &bytes).unwrap().emit().unwrap();
    assert_eq!(ours.modules.len(), theirs.modules.len());
    let named = |b: &[u8]| -> HashSet<String> {
        names(b).into_iter().filter(|n| n != "stub" && n != "dummy").collect()
    };
    assert_eq!(named(&ours.modules[0].bytes), named(&theirs.modules[0].bytes));
}
