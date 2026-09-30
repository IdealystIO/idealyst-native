//! `strand::trap_imports` against a module that names functions through
//! every channel a function index can appear in.

use wasm_carve::module::ModuleIndex;
use wasm_encoder::{
    CodeSection, ConstExpr, ElementSection, Elements, EntityType, ExportKind, ExportSection,
    Function, FunctionSection, GlobalSection, GlobalType, ImportSection, IndirectNameMap, Module,
    NameMap, NameSection, RefType, StartSection, TableSection, TableType, TypeSection, ValType,
};
use wasmparser::{Operator, TypeRef};

const PLACEHOLDER: &str = "__wbindgen_placeholder__";
const PREFIX: &str = "__idealyst_stranded_";

/// ```text
/// f0 import wbg.a                                  () -> ()
/// f1 import __wbindgen_placeholder__.describe      (i32) -> ()   stranded
/// f2 import wbg.b                                  () -> ()
/// f3 caller: i32.const 7; call f1; call f2         exported, table slot 0
/// f4 init:   ref.func f3; drop; call f0            start, table slot 1,
///                                                  a funcref global's init
/// ```
fn fixture() -> Vec<u8> {
    let mut m = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([ValType::I32], []);
    types.ty().function([], []);
    m.section(&types);
    let mut imports = ImportSection::new();
    imports.import("wbg", "a", EntityType::Function(1));
    imports.import(PLACEHOLDER, "__wbindgen_describe", EntityType::Function(0));
    imports.import("wbg", "b", EntityType::Function(1));
    m.section(&imports);
    let mut funcs = FunctionSection::new();
    funcs.function(1).function(1);
    m.section(&funcs);
    let mut tables = TableSection::new();
    tables.table(TableType { element_type: RefType::FUNCREF, table64: false, minimum: 2, maximum: Some(2), shared: false });
    m.section(&tables);
    let mut globals = GlobalSection::new();
    globals.global(
        GlobalType { val_type: ValType::Ref(RefType::FUNCREF), mutable: false, shared: false },
        &ConstExpr::ref_func(4),
    );
    m.section(&globals);
    let mut exports = ExportSection::new();
    exports.export("caller", ExportKind::Func, 3);
    m.section(&exports);
    m.section(&StartSection { function_index: 4 });
    let mut elems = ElementSection::new();
    elems.active(None, &ConstExpr::i32_const(0), Elements::Functions([3u32, 4].as_slice().into()));
    m.section(&elems);
    let mut code = CodeSection::new();
    let mut caller = Function::new([(1, ValType::I32)]);
    caller.instructions().i32_const(7).call(1).call(2).end();
    code.function(&caller);
    let mut init = Function::new([]);
    init.instructions().ref_func(3).drop().call(0).end();
    code.function(&init);
    m.section(&code);
    let mut names = NameMap::new();
    for (i, n) in ["a", "__wbindgen_describe", "b", "caller", "init"].iter().enumerate() {
        names.append(i as u32, n);
    }
    let mut locals = NameMap::new();
    locals.append(0, "tmp");
    let mut local_names = IndirectNameMap::new();
    local_names.append(3, &locals);
    let mut section = NameSection::new();
    section.functions(&names);
    section.locals(&local_names);
    m.section(&section);
    m.finish()
}

fn validate(bytes: &[u8]) {
    wasmparser::Validator::new().validate_all(bytes).expect("output validates");
}

fn name_of(m: &ModuleIndex<'_>, f: u32) -> String {
    m.func_names.get(&f).map(|s| s.to_string()).unwrap_or_else(|| format!("<unnamed {f}>"))
}

fn index_of(m: &ModuleIndex<'_>, name: &str) -> u32 {
    *m.func_names.iter().find(|(_, n)| **n == name).unwrap_or_else(|| panic!("no {name}")).0
}

fn calls(m: &ModuleIndex<'_>, name: &str) -> Vec<String> {
    let mut refs = Vec::new();
    m.direct_refs(index_of(m, name), &mut refs).unwrap();
    refs.into_iter().map(|f| name_of(m, f)).collect()
}

/// The walrus pass this replaces took 5.4–5.7 s of every warm CrewForge
/// rebuild; its behavior is the contract: the stranded import is gone,
/// a defined function that traps stands in for it under
/// `{PREFIX}{name}`, and everything that named any function still names
/// the same one.
#[test]
fn a_stranded_import_becomes_a_trapping_function_and_every_reference_follows() {
    let before = fixture();
    validate(&before);
    let after = wasm_carve::strand::trap_imports(&before, &[PLACEHOLDER], PREFIX)
        .unwrap()
        .expect("a stranded import to replace");
    validate(&after);
    let m = ModuleIndex::parse(&after).unwrap();

    let imports: Vec<_> = m.imports.iter().map(|i| (i.module, i.name)).collect();
    assert_eq!(imports, vec![("wbg", "a"), ("wbg", "b")]);
    assert_eq!(m.func_imports, 2);

    let stand_in = format!("{PREFIX}__wbindgen_describe");
    let s = index_of(&m, &stand_in);
    assert_eq!(s, m.total_funcs() - 1, "the stand-in is appended");
    assert_eq!(m.func_types[s as usize], 0, "it keeps the import's signature");
    let (_, body) = &m.bodies[(s - m.func_imports) as usize];
    let fb = wasmparser::FunctionBody::new(wasmparser::BinaryReader::new(&after[body.clone()], 0));
    let first = fb.get_operators_reader().unwrap().read().unwrap();
    assert!(
        matches!(first, Operator::Unreachable),
        "a descriptor function is never called at run time; if one is it must trap, got {first:?}"
    );

    assert_eq!(calls(&m, "caller"), vec![stand_in.clone(), "b".to_string()]);
    assert_eq!(calls(&m, "init"), vec!["caller".to_string(), "a".to_string()]);
    let export = m.exports.iter().find(|e| e.name == "caller").unwrap();
    assert_eq!(name_of(&m, export.index), "caller");
    assert_eq!(name_of(&m, m.start.unwrap()), "init");
    let slots: Vec<String> = m.elems[0]
        .items
        .iter()
        .map(|i| match i {
            wasm_carve::module::ElemItem::Func(f) => name_of(&m, *f),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(slots, vec!["caller", "init"], "table slots — what a hot patch dispatches by — hold");

    // The global's `ref.func` and the local names follow too.
    for payload in wasmparser::Parser::new(0).parse_all(&after) {
        match payload.unwrap() {
            wasmparser::Payload::GlobalSection(r) => {
                let g = r.into_iter().next().unwrap().unwrap();
                let op = g.init_expr.get_operators_reader().read().unwrap();
                let Operator::RefFunc { function_index } = op else { panic!("{op:?}") };
                assert_eq!(name_of(&m, function_index), "init");
            }
            wasmparser::Payload::CustomSection(r) => {
                if let wasmparser::KnownCustom::Name(names) = r.as_known() {
                    for sub in names {
                        if let wasmparser::Name::Local(map) = sub.unwrap() {
                            let naming = map.into_iter().next().unwrap().unwrap();
                            assert_eq!(name_of(&m, naming.index), "caller");
                        }
                    }
                }
            }
            _ => {}
        }
    }
    // Name maps list indices in increasing order.
    let mut idx: Vec<u32> = m.func_names.keys().copied().collect();
    idx.sort_unstable();
    assert_eq!(idx, (0..m.total_funcs()).collect::<Vec<_>>());
}

/// A module wasm-bindgen fully satisfied is not touched — a normal build
/// must not pay for this at all.
#[test]
fn a_module_with_no_stranded_imports_is_left_alone() {
    let bytes = fixture();
    assert!(wasm_carve::strand::trap_imports(&bytes, &["__wbindgen_externref_xform__"], PREFIX)
        .unwrap()
        .is_none());
}

/// Non-function imports from a stranded namespace are not functions to
/// stand in for, and the other index spaces do not move.
#[test]
fn only_function_imports_are_replaced() {
    let mut m = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], []);
    m.section(&types);
    let mut imports = ImportSection::new();
    imports.import(PLACEHOLDER, "g", EntityType::Global(GlobalType { val_type: ValType::I32, mutable: false, shared: false }));
    imports.import(PLACEHOLDER, "f", EntityType::Function(0));
    m.section(&imports);
    let mut funcs = FunctionSection::new();
    funcs.function(0);
    m.section(&funcs);
    let mut code = CodeSection::new();
    let mut f = Function::new([]);
    f.instructions().global_get(0).drop().call(0).end();
    code.function(&f);
    m.section(&code);
    let bytes = m.finish();
    validate(&bytes);
    let after = wasm_carve::strand::trap_imports(&bytes, &[PLACEHOLDER], PREFIX).unwrap().unwrap();
    validate(&after);
    let idx = ModuleIndex::parse(&after).unwrap();
    assert_eq!(idx.imports.len(), 1);
    assert!(matches!(idx.imports[0].ty, TypeRef::Global(_)));
    assert_eq!(idx.total_funcs(), 2);
}
