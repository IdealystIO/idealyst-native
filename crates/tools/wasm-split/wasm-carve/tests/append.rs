//! `append::append`: forwarders and table roots added without moving
//! anything that was already there.

use wasm_carve::{
    append::{Additions, Forwarder, append},
    module::{ElemItem, ModuleIndex},
};
use wasm_encoder::{
    CodeSection, ConstExpr, CustomSection, ElementSection, Elements, EntityType, ExportKind,
    ExportSection, Function, FunctionSection, ImportSection, Module, NameMap, NameSection,
    RefType, TableSection, TableType, TypeSection, ValType,
};
use wasmparser::Operator;

/// ```text
/// f0 import wbg.shim   (i32, i32) -> i32
/// f1 a                 () -> ()        table slot 1
/// f2 b                 () -> ()        not in the table
/// f3 c                 (i32, i32) -> i32
/// ```
/// plus `linking`, `reloc.CODE`, `.debug_info` and `producers` customs.
fn fixture() -> Vec<u8> {
    let mut m = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([ValType::I32, ValType::I32], [ValType::I32]);
    types.ty().function([], []);
    m.section(&types);
    let mut imports = ImportSection::new();
    imports.import("wbg", "shim", EntityType::Function(0));
    m.section(&imports);
    let mut funcs = FunctionSection::new();
    funcs.function(1).function(1).function(0);
    m.section(&funcs);
    let mut tables = TableSection::new();
    tables.table(TableType { element_type: RefType::FUNCREF, table64: false, minimum: 2, maximum: Some(2), shared: false });
    m.section(&tables);
    let mut exports = ExportSection::new();
    exports.export("__indirect_function_table", ExportKind::Table, 0);
    m.section(&exports);
    let mut elems = ElementSection::new();
    elems.active(None, &ConstExpr::i32_const(1), Elements::Functions([1u32].as_slice().into()));
    m.section(&elems);
    let mut code = CodeSection::new();
    let mut a = Function::new([]);
    a.instructions().call(2).end();
    code.function(&a);
    let mut b = Function::new([]);
    b.instructions().end();
    code.function(&b);
    let mut c = Function::new([]);
    c.instructions().local_get(0).local_get(1).call(0).end();
    code.function(&c);
    m.section(&code);
    let mut names = NameMap::new();
    for (i, n) in ["shim", "a", "b", "c"].iter().enumerate() {
        names.append(i as u32, n);
    }
    let mut section = NameSection::new();
    section.functions(&names);
    m.section(&section);
    for name in ["linking", "reloc.CODE", ".debug_info", "producers"] {
        m.section(&CustomSection { name: name.into(), data: [1u8, 2, 3].as_slice().into() });
    }
    m.finish()
}

fn validate(bytes: &[u8]) {
    wasmparser::Validator::new().validate_all(bytes).expect("output validates");
}

fn keep(name: &str) -> bool {
    name != "linking" && !name.starts_with("reloc.") && !name.starts_with(".debug_")
}

fn customs(bytes: &[u8]) -> Vec<String> {
    ModuleIndex::parse(bytes)
        .unwrap()
        .sections
        .iter()
        .filter_map(|s| s.custom_name.clone())
        .collect()
}

/// What a hot-reload base needs: a forwarder to an import and one to a
/// defined function, both rooted along with an unrooted original, and
/// every original function untouched at its index.
#[test]
fn forwarders_and_roots_are_appended_and_nothing_moves() {
    let before = fixture();
    validate(&before);
    let src = ModuleIndex::parse(&before).unwrap();
    let forwarders = [
        Forwarder { name: "__fwd_shim".into(), target: 0 },
        Forwarder { name: "__fwd_c".into(), target: 3 },
    ];
    let out = append(&src, &Additions { forwarders: &forwarders, root: &[4, 5, 2], keep_custom: &keep }).unwrap();
    validate(&out);
    let m = ModuleIndex::parse(&out).unwrap();

    // Originals keep their index, name and exact body bytes.
    for f in 1..4 {
        assert_eq!(m.func_names[&f], src.func_names[&f]);
        assert_eq!(m.body_entry(f), src.body_entry(f), "body of {} re-encoded", src.func_names[&f]);
    }
    // Forwarders come after, typed as their target, forwarding every
    // parameter.
    assert_eq!((m.func_names[&4], m.func_names[&5]), ("__fwd_shim", "__fwd_c"));
    assert_eq!((m.func_types[4], m.func_types[5]), (0, 0));
    let mut refs = Vec::new();
    m.direct_refs(4, &mut refs).unwrap();
    assert_eq!(refs, vec![0]);
    let (_, body) = &m.bodies[(4 - m.func_imports) as usize];
    let fb = wasmparser::FunctionBody::new(wasmparser::BinaryReader::new(&out[body.clone()], 0));
    let ops: Vec<String> = fb
        .get_operators_reader()
        .unwrap()
        .into_iter()
        .map(|o| format!("{:?}", o.unwrap()))
        .collect();
    assert_eq!(ops.len(), 4, "local.get 0; local.get 1; call; end — got {ops:?}");
    assert!(matches!(
        fb.get_operators_reader().unwrap().read().unwrap(),
        Operator::LocalGet { local_index: 0 }
    ));

    // The segment grew by the roots, in order, and the table with it.
    let slots: Vec<u32> = m.elems[0]
        .items
        .iter()
        .map(|i| match i {
            ElemItem::Func(f) => *f,
            ElemItem::Null => panic!("null"),
        })
        .collect();
    assert_eq!(slots, vec![1, 4, 5, 2]);
    assert_eq!(m.table_types[0].initial, 5, "offset 1 + 4 entries");
    assert_eq!(m.table_types[0].maximum, Some(5));

    // Linker metadata and DWARF gone; everything else kept.
    assert_eq!(customs(&out), vec!["name", "producers"]);
}

/// A module with only imports has no function or code section; the
/// forwarders need both, in section order, and a name to be found by.
#[test]
fn a_module_with_no_defined_functions_gets_the_sections_its_forwarders_need() {
    let mut m = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([ValType::I32], []);
    m.section(&types);
    let mut imports = ImportSection::new();
    imports.import("./__wasm_split.js", "lazy", EntityType::Function(0));
    m.section(&imports);
    let mut tables = TableSection::new();
    tables.table(TableType { element_type: RefType::FUNCREF, table64: false, minimum: 1, maximum: Some(1), shared: false });
    m.section(&tables);
    let mut elems = ElementSection::new();
    elems.active(None, &ConstExpr::i32_const(1), Elements::Functions([].as_slice().into()));
    m.section(&elems);
    let before = m.finish();
    validate(&before);
    let src = ModuleIndex::parse(&before).unwrap();
    let forwarders = [Forwarder { name: "__fwd_lazy".into(), target: 0 }];
    let out = append(&src, &Additions { forwarders: &forwarders, root: &[1], keep_custom: &keep }).unwrap();
    validate(&out);
    let m = ModuleIndex::parse(&out).unwrap();
    assert_eq!(m.func_names[&1], "__fwd_lazy");
    assert_eq!(m.elems[0].items, vec![ElemItem::Func(1)]);
}

/// A base linked without `--export-table` has no active segment to grow,
/// and inventing an offset is a silent mis-dispatch; say so.
#[test]
fn no_active_segment_is_refused() {
    let mut m = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], []);
    m.section(&types);
    let mut funcs = FunctionSection::new();
    funcs.function(0);
    m.section(&funcs);
    let mut code = CodeSection::new();
    let mut f = Function::new([]);
    f.instructions().end();
    code.function(&f);
    m.section(&code);
    let bytes = m.finish();
    let src = ModuleIndex::parse(&bytes).unwrap();
    let err = append(&src, &Additions { forwarders: &[], root: &[0], keep_custom: &keep }).unwrap_err();
    assert!(format!("{err:#}").contains("element segment"), "{err:#}");
}
