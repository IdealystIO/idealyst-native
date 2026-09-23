//! Resolve a freshly linked wasm patch against the base module running
//! in the page.
//!
//! # What the runtime gives the patch
//!
//! `subsecond::apply_patch` instantiates the patch with exactly ONE
//! import namespace: `env`, built from the base instance's own exports
//! plus two synthesized globals, `__memory_base` and `__table_base`.
//! Nothing else. So every import the patch carries has to be either
//! (a) an `env` name the base actually exports, or (b) gone by the time
//! we ship it.
//!
//! A patch linked `--pie --experimental-pic` against one crate's objects
//! does not come out that way. Measured on the probe crate, it imports:
//!
//! ```text
//! env.memory                         base exports it (--export-memory)
//! env.__indirect_function_table      base exports it (--export-table)
//! env.__stack_pointer                base exports it (--export=…)
//! env.__memory_base / __table_base   synthesized by the runtime
//! env._RNvCs…_4core9panicking…       NOT exported — internal linkage
//! __wbindgen_placeholder__.__wbg_log_…   wrong namespace entirely
//! GOT.func.<sym> / GOT.mem.<sym>     wasm-ld's PIC indirection
//! ```
//!
//! [`resolve_against_base`] turns the last four kinds into something the
//! runtime can satisfy.
//!
//! # The four rewrites
//!
//! **A function the base has but does not export.** Every private Rust
//! `fn` is one of these — internal linkage is not a dynamic symbol, so
//! no linker flag exports it. But `hotpatch_base` put every function in
//! `__indirect_function_table`, so we replace the import with a local
//! function that pushes its arguments and `call_indirect`s the base's
//! slot. This also sidesteps wasm-bindgen's export-count ceiling, which
//! is the reason dioxus does it the same way (emscripten#22863).
//!
//! **A `__wbindgen_placeholder__` import.** That namespace is
//! wasm-bindgen's, and wasm-bindgen never runs on a patch — wasm-ld
//! links it directly. `hotpatch_base` preserved each intrinsic as
//! `__saved_wbg_<name>`, so the import is re-pointed at `env` under that
//! name.
//!
//! **`GOT.func.<sym>`.** wasm-ld's PIC output takes a function's address
//! through a GOT global rather than a relocation. The value it wants is
//! a table index, which is precisely what the base's table gives us, so
//! the import becomes a local constant.
//!
//! **`GOT.mem.<sym>`.** Same, for a data symbol — a `static` that lives
//! in a crate the patch did not recompile. The value is the symbol's
//! absolute address in the base's linear memory, which is its data
//! segment's offset plus its offset within it. walrus does not keep
//! those, so [`index_base`] reads the `linking` custom section by hand;
//! that section is why the base build passes `--emit-relocs`.
//!
//! # When it cannot be done
//!
//! An import that resolves to none of the above is an error, loudly.
//! The caller's answer to an error here is a full rebuild, which is
//! always correct — whereas a patch that instantiates against a
//! half-satisfied import set is a page that dispatches into the wrong
//! function with no message anywhere.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{bail, Context, Result};
use walrus::{
    ir, ConstExpr, ElementKind, FunctionBuilder, FunctionId, GlobalKind, ImportKind, Module, TableId,
};

/// Everything a patch needs to know about the base module it will be
/// instantiated alongside.
///
/// Built once per base build and reused for every patch, because
/// parsing a debug-profile base module is the expensive part of a save
/// and it does not change between patches.
#[derive(Debug, Default, Clone)]
pub struct BaseIndex {
    /// Mangled symbol name → its `__indirect_function_table` index.
    /// Populated for every function, because `hotpatch_base` put every
    /// function in the table.
    pub ifunc: HashMap<String, u32>,
    /// Names the base actually exports — an import matching one of
    /// these needs no rewriting at all.
    pub exports: HashSet<String>,
    /// Data symbol name → its absolute address in the base's memory.
    pub data: HashMap<String, u32>,
}

impl BaseIndex {
    /// Read a wasm-bindgen'd base module: its table, its exports, and
    /// its data symbols.
    pub fn of(wasm: &[u8]) -> Result<Self> {
        let module = Module::from_buffer(wasm).context("parsing the base module")?;

        let mut ifunc = HashMap::new();
        for element in module.elements.iter() {
            let ElementKind::Active { offset, .. } = &element.kind else {
                continue;
            };
            // A base is linked non-PIC, so its segment offset is a
            // constant. Anything else and the indices we would compute
            // are guesses, and a guessed index is a silent mis-dispatch.
            let Some(base) = const_u32(offset) else {
                continue;
            };
            let walrus::ElementItems::Functions(ids) = &element.items else {
                continue;
            };
            for (i, id) in ids.iter().enumerate() {
                if let Some(name) = module.funcs.get(*id).name.as_deref() {
                    // First slot wins: a function listed twice is two
                    // valid pointers to one body, and either redirects
                    // correctly.
                    ifunc.entry(name.to_string()).or_insert(base + i as u32);
                }
            }
        }

        let exports = module.exports.iter().map(|e| e.name.clone()).collect();
        let data = data_symbol_addresses(wasm).context("reading the base's data symbols")?;

        Ok(Self {
            ifunc,
            exports,
            data,
        })
    }
}

/// Rewrite `patch` so every import it carries is one
/// `subsecond::apply_patch` can satisfy from `base`. Returns the new
/// module's bytes.
pub fn resolve_against_base(patch: &[u8], base: &BaseIndex) -> Result<Vec<u8>> {
    let mut module = Module::from_buffer(patch).context("parsing the patch module")?;

    // Every rewrite that ends in a `call_indirect` needs the table to
    // call through. The patch imports the base's table, and its own
    // element segment names it.
    let table = patch_table(&module);

    let mut unresolved: Vec<String> = Vec::new();
    let imports: Vec<_> = module
        .imports
        .iter()
        .map(|i| (i.id(), i.module.clone(), i.name.clone(), i.kind.clone()))
        .collect();

    for (id, namespace, name, kind) in imports {
        match namespace.as_str() {
            // wasm-ld's PIC indirection for a function's address. The
            // value it wants is a table index.
            "GOT.func" => match (base.ifunc.get(&name), kind) {
                (Some(index), ImportKind::Global(global)) => {
                    module.imports.delete(id);
                    module.globals.get_mut(global).kind =
                        GlobalKind::Local(ConstExpr::Value(ir::Value::I32(*index as i32)));
                }
                (None, _) => unresolved.push(format!("GOT.func.{name} (no slot in the base table)")),
                (_, _) => unresolved.push(format!("GOT.func.{name} (not a global)")),
            },

            // Same, for a `static` living in a crate the patch did not
            // recompile. The value is its absolute address.
            "GOT.mem" => match (base.data.get(&name), kind) {
                (Some(address), ImportKind::Global(global)) => {
                    module.imports.delete(id);
                    module.globals.get_mut(global).kind =
                        GlobalKind::Local(ConstExpr::Value(ir::Value::I32(*address as i32)));
                }
                (None, _) => unresolved.push(format!(
                    "GOT.mem.{name} (no data symbol of that name in the base — \
                     was it linked without `--emit-relocs`?)"
                )),
                (_, _) => unresolved.push(format!("GOT.mem.{name} (not a global)")),
            },

            // wasm-bindgen's namespace, which the runtime does not
            // provide. `hotpatch_base` kept each intrinsic alive under
            // `__saved_wbg_<name>`; point at that instead.
            "__wbindgen_placeholder__" | "__wbindgen_externref_xform__" => {
                let saved = format!("__saved_wbg_{name}");
                if crate::hotpatch_base::is_bindgen_internal(&name) {
                    // A descriptor symbol. wasm-bindgen interprets these
                    // at bindgen time and deletes them, so the base has
                    // none to point at — and the patch only imports one
                    // because `--no-gc-sections` kept the machinery that
                    // references it alive. Nothing calls it at runtime.
                    //
                    // Point it at table slot 0, which is the null entry:
                    // the import is satisfied so the patch instantiates,
                    // and if one somehow IS called the page traps at the
                    // call rather than running whatever happens to live
                    // at some plausible-looking index.
                    if let ImportKind::Function(func) = kind {
                        module.imports.delete(id);
                        call_through_table(&mut module, table, func, 0, &name)?;
                    } else {
                        unresolved.push(format!(
                            "{namespace}.{name} (a descriptor symbol that is not a function)"
                        ));
                    }
                } else if base.exports.contains(&saved) {
                    let import = module.imports.get_mut(id);
                    import.module = "env".to_string();
                    import.name = saved;
                } else if let (Some(index), ImportKind::Function(func)) =
                    (base.ifunc.get(&saved), kind)
                {
                    module.imports.delete(id);
                    call_through_table(&mut module, table, func, *index, &saved)?;
                } else {
                    unresolved.push(format!(
                        "{namespace}.{name} (the base kept no __saved_wbg_ alias for it)"
                    ));
                }
            }

            // The runtime's own namespace. Anything the base exports is
            // already fine; anything else has to go through the table,
            // which is where every private function lives.
            "env" => {
                if base.exports.contains(&name) {
                    continue;
                }
                match (base.ifunc.get(&name), kind) {
                    (Some(index), ImportKind::Function(func)) => {
                        module.imports.delete(id);
                        call_through_table(&mut module, table, func, *index, &name)?;
                    }
                    // A non-function import the base does not export
                    // cannot be faked. `__memory_base` / `__table_base`
                    // are the exception: the runtime synthesizes those.
                    _ if name == "__memory_base" || name == "__table_base" => {}
                    _ => unresolved.push(format!(
                        "env.{name} (the base neither exports it nor has it in the table)"
                    )),
                }
            }

            other => unresolved.push(format!(
                "{other}.{name} (the runtime only provides an `env` namespace)"
            )),
        }
    }

    if !unresolved.is_empty() {
        unresolved.sort();
        bail!(
            "the patch imports {} thing(s) the running module cannot supply, so it would \
             instantiate against a half-satisfied import set:\n  {}",
            unresolved.len(),
            unresolved.join("\n  "),
        );
    }

    // A patch must not run anything of its own accord. Its `start` would
    // fire during instantiation, before the jump table is committed and
    // therefore before any of its functions are reachable — initializing
    // state the base already initialized, against a half-applied patch.
    module.start = None;

    // wasm-bindgen's descriptor section describes the base's bindings,
    // not the patch's, and nothing downstream reads it. It is bytes the
    // browser downloads on every save.
    let stale: Vec<_> = module
        .customs
        .iter()
        .filter(|(_, c)| c.name().contains("__wasm_bindgen"))
        .map(|(id, _)| id)
        .collect();
    for id in stale {
        module.customs.delete(id);
    }

    Ok(module.emit_wasm())
}

/// The table a `call_indirect` should go through: the one the patch's
/// own element segment writes into, which is the base's table imported
/// as `env.__indirect_function_table`.
fn patch_table(module: &Module) -> Option<TableId> {
    module.elements.iter().find_map(|e| match e.kind {
        ElementKind::Active { table, .. } => Some(table),
        _ => None,
    })
}

/// Replace an imported function with a local one of the same type that
/// forwards its arguments and `call_indirect`s the base's table slot.
///
/// This is how a private function in the base — which no linker flag
/// will export — becomes callable from a patch.
fn call_through_table(
    module: &mut Module,
    table: Option<TableId>,
    func: FunctionId,
    index: u32,
    name: &str,
) -> Result<()> {
    let table = table.context(
        "the patch has no element segment, so there is no table to call the base through — \
         it was linked without `--experimental-pic`",
    )?;

    let ty_id = module.funcs.get(func).ty();
    let ty = module.types.get(ty_id);
    let params = ty.params().to_vec();
    let results = ty.results().to_vec();

    let locals: Vec<_> = params.iter().map(|t| module.locals.add(*t)).collect();
    let mut builder = FunctionBuilder::new(&mut module.types, &params, &results);
    let mut body = builder.name(name.to_string()).func_body();
    for local in &locals {
        body.local_get(*local);
    }
    // The callee index goes on the stack last — `call_indirect` pops it
    // first, then the arguments beneath it.
    body.instr(ir::Instr::Const(ir::Const {
        value: ir::Value::I32(index as i32),
    }));
    body.instr(ir::Instr::CallIndirect(ir::CallIndirect { ty: ty_id, table }));

    let func = module.funcs.get_mut(func);
    func.kind = walrus::FunctionKind::Local(builder.local_func(locals));
    // Carry the symbol's name onto the trampoline. An imported function
    // in a freshly linked module has no name-section entry, and without
    // one a stack trace through the patch shows an anonymous frame
    // exactly where the interesting hop is.
    func.name = Some(name.to_string());
    Ok(())
}

fn const_u32(expr: &ConstExpr) -> Option<u32> {
    match expr {
        ConstExpr::Value(ir::Value::I32(v)) => Some((*v).max(0) as u32),
        ConstExpr::Value(ir::Value::I64(v)) => Some((*v).max(0) as u32),
        _ => None,
    }
}

/// Data symbol name → absolute address in the base's linear memory.
///
/// Read straight from the bytes rather than through walrus, which keeps
/// a data segment's contents but not the offsets the `linking` section's
/// symbol table indexes them by. An address is a segment's own offset
/// plus the symbol's offset inside it.
///
/// This is the only consumer of `--emit-relocs` on the base build: a
/// module linked without it has no `linking` section, every `GOT.mem`
/// import goes unresolved, and the caller falls back to a rebuild with
/// that named as the reason.
fn data_symbol_addresses(wasm: &[u8]) -> Result<HashMap<String, u32>> {
    use wasmparser::{Payload, SymbolInfo};

    // Segment index → the segment's own offset in linear memory.
    let mut segment_offsets: BTreeMap<u32, u32> = BTreeMap::new();
    let mut out = HashMap::new();

    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        match payload.context("parsing the base module's sections")? {
            Payload::DataSection(reader) => {
                for (index, data) in reader.into_iter().enumerate() {
                    let data = data.context("parsing a data segment")?;
                    if let wasmparser::DataKind::Active { offset_expr, .. } = data.kind {
                        let mut ops = offset_expr.get_operators_reader();
                        if let Ok(wasmparser::Operator::I32Const { value }) = ops.read() {
                            segment_offsets.insert(index as u32, value.max(0) as u32);
                        }
                    }
                }
            }
            Payload::CustomSection(c) if c.name() == "linking" => {
                let reader = wasmparser::LinkingSectionReader::new(wasmparser::BinaryReader::new(
                    c.data(),
                    c.data_offset(),
                ))
                .context("parsing the linking section")?;
                for subsection in reader.subsections() {
                    let subsection = subsection.context("parsing a linking subsection")?;
                    let wasmparser::Linking::SymbolTable(symbols) = subsection else {
                        continue;
                    };
                    for symbol in symbols {
                        let symbol = symbol.context("parsing a linking symbol")?;
                        // Only a DEFINED data symbol has a location. An
                        // undefined one is a reference to somewhere
                        // else and has no address to hand out.
                        if let SymbolInfo::Data {
                            name,
                            symbol: Some(definition),
                            ..
                        } = symbol
                        {
                            out.insert(
                                name.to_string(),
                                definition
                                    .offset
                                    .saturating_add(segment_offsets.get(&definition.index).copied().unwrap_or(0)),
                            );
                        }
                    }
                }
            }
            _ => {}
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use walrus::{ElementItems, RefType, ValType};

    /// A patch-shaped module: a PIC element segment offset by an
    /// imported `__table_base`, plus whatever imports the test wants.
    fn patch_module(imports: &[(&str, &str)]) -> Vec<u8> {
        let mut module = Module::default();
        let (table, _) = module.add_import_table(
            "env",
            "__indirect_function_table",
            false,
            0,
            None,
            RefType::Funcref,
        );
        let table_base = module.add_import_global("env", "__table_base", ValType::I32, false, false);

        let ty = module.types.add(&[], &[]);
        let mut called = Vec::new();
        for (namespace, name) in imports {
            if namespace.starts_with("GOT.") {
                module.add_import_global(namespace, name, ValType::I32, false, false);
            } else {
                let (func, _) = module.add_import_func(namespace, name, ty);
                called.push(func);
            }
        }

        // The patch's own function, which is what a jump table would
        // point at. It CALLS each imported function, because walrus
        // garbage-collects on emit and an import nothing calls would not
        // survive to be rewritten — which is not the shape of a real
        // patch, where every import exists because something calls it.
        let mut builder = FunctionBuilder::new(&mut module.types, &[ValType::I32], &[ValType::I32]);
        let arg = module.locals.add(ValType::I32);
        {
            let mut body = builder.name("__Probe_hot_impl".to_string()).func_body();
            for func in called {
                body.call(func);
            }
            body.local_get(arg);
        }
        let own = module.funcs.add_local(builder.local_func(vec![arg]));

        module.elements.add(
            ElementKind::Active {
                table,
                offset: ConstExpr::Global(table_base.0),
            },
            ElementItems::Functions(vec![own]),
        );
        module.emit_wasm()
    }

    fn base_with(ifunc: &[(&str, u32)], exports: &[&str], data: &[(&str, u32)]) -> BaseIndex {
        // Every real base exports these three — `--export-memory`,
        // `--export-table` and `--export=__stack_pointer` — and every
        // patch imports them, so a fixture without them tests a module
        // that could not exist.
        let standard = ["memory", "__indirect_function_table", "__stack_pointer"];
        BaseIndex {
            ifunc: ifunc.iter().map(|(n, i)| (n.to_string(), *i)).collect(),
            exports: standard
                .iter()
                .chain(exports.iter())
                .map(|s| s.to_string())
                .collect(),
            data: data.iter().map(|(n, a)| (n.to_string(), *a)).collect(),
        }
    }

    fn imports_of(wasm: &[u8]) -> Vec<String> {
        let module = Module::from_buffer(wasm).unwrap();
        let mut out: Vec<_> = module
            .imports
            .iter()
            .map(|i| format!("{}.{}", i.module, i.name))
            .collect();
        out.sort();
        out
    }

    /// The core rewrite. Every private Rust `fn` has internal linkage,
    /// so no linker flag exports it — but `hotpatch_base` put it in the
    /// table, and a `call_indirect` through the base's slot reaches it.
    #[test]
    fn a_function_the_base_does_not_export_is_called_through_the_table() {
        let patch = patch_module(&[("env", "_RNvCs_4core9panicking5panic")]);
        let base = base_with(&[("_RNvCs_4core9panicking5panic", 412)], &[], &[]);

        let out = resolve_against_base(&patch, &base).unwrap();
        assert!(
            !imports_of(&out)
                .iter()
                .any(|i| i.contains("panicking5panic")),
            "still imported: {:?}",
            imports_of(&out)
        );

        // …and it reaches the base through slot 412, not some other one.
        let module = Module::from_buffer(&out).unwrap();
        let body = module
            .funcs
            .iter()
            .find(|f| f.name.as_deref() == Some("_RNvCs_4core9panicking5panic"))
            .expect("the trampoline");
        let walrus::FunctionKind::Local(local) = &body.kind else {
            panic!("the import should have become a local function");
        };
        let instrs = &local.block(local.entry_block()).instrs;
        assert!(
            instrs.iter().any(|(i, _)| matches!(
                i,
                ir::Instr::Const(ir::Const {
                    value: ir::Value::I32(412)
                })
            )),
            "the trampoline does not push slot 412"
        );
        assert!(
            instrs
                .iter()
                .any(|(i, _)| matches!(i, ir::Instr::CallIndirect(_))),
            "the trampoline does not call_indirect"
        );
    }

    /// If the base exports it, the import already resolves. Rewriting
    /// would add a trampoline and a table hop for nothing.
    #[test]
    fn an_import_the_base_exports_is_left_alone() {
        let patch = patch_module(&[("env", "memory_intrinsic")]);
        let base = base_with(&[], &["memory_intrinsic"], &[]);
        let out = resolve_against_base(&patch, &base).unwrap();
        assert!(
            imports_of(&out).contains(&"env.memory_intrinsic".to_string()),
            "{:?}",
            imports_of(&out)
        );
    }

    /// wasm-bindgen never runs on a patch, so its namespace has no
    /// provider at instantiation. The base kept each intrinsic alive
    /// under `__saved_wbg_<name>` precisely so this can point at it.
    #[test]
    fn a_wasm_bindgen_placeholder_is_repointed_at_the_saved_alias() {
        let patch = patch_module(&[("__wbindgen_placeholder__", "__wbg_log_51db52")]);
        let base = base_with(&[], &["__saved_wbg___wbg_log_51db52"], &[]);
        let out = resolve_against_base(&patch, &base).unwrap();
        assert_eq!(
            imports_of(&out)
                .iter()
                .filter(|i| i.contains("wbg_log"))
                .collect::<Vec<_>>(),
            vec!["env.__saved_wbg___wbg_log_51db52"],
        );
    }

    /// wasm-ld takes a function's address through a GOT global under
    /// PIC. The value it wants is a table index.
    #[test]
    fn a_got_func_global_becomes_the_base_table_index() {
        let patch = patch_module(&[("GOT.func", "_RNvCs_3app7handler")]);
        let base = base_with(&[("_RNvCs_3app7handler", 77)], &[], &[]);
        let out = resolve_against_base(&patch, &base).unwrap();
        assert!(!imports_of(&out).iter().any(|i| i.starts_with("GOT.")));

        let module = Module::from_buffer(&out).unwrap();
        assert!(
            module.globals.iter().any(|g| matches!(
                &g.kind,
                GlobalKind::Local(ConstExpr::Value(ir::Value::I32(77)))
            )),
            "the GOT global did not become the constant 77"
        );
    }

    /// A `static` in a crate the patch did not recompile. The value is
    /// its absolute address: segment offset plus offset within.
    #[test]
    fn a_got_mem_global_becomes_the_base_data_address() {
        let patch = patch_module(&[("GOT.mem", "_RNvCs_3app8GREETING")]);
        let base = base_with(&[], &[], &[("_RNvCs_3app8GREETING", 1_048_612)]);
        let out = resolve_against_base(&patch, &base).unwrap();

        let module = Module::from_buffer(&out).unwrap();
        assert!(
            module.globals.iter().any(|g| matches!(
                &g.kind,
                GlobalKind::Local(ConstExpr::Value(ir::Value::I32(1_048_612)))
            )),
            "the GOT.mem global did not become the base's address"
        );
    }

    /// The failure that must never be silent. A patch instantiated
    /// against a half-satisfied import set throws at load — or worse,
    /// dispatches somewhere arbitrary. The caller's answer is a full
    /// rebuild, and it needs to know which symbol forced it.
    #[test]
    fn an_import_nothing_can_supply_is_an_error_naming_it() {
        let patch = patch_module(&[("env", "_RNvCs_3app12never_existed")]);
        let err = resolve_against_base(&patch, &base_with(&[], &[], &[])).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("never_existed"), "{message}");
        assert!(
            message.contains("neither exports it nor has it in the table"),
            "{message}"
        );
    }

    /// A patch's `start` would fire during instantiation — before the
    /// jump table is committed, so before any of its functions are
    /// reachable — re-initializing state the base already owns.
    #[test]
    fn the_patch_never_keeps_a_start_function() {
        let mut module = Module::from_buffer(&patch_module(&[])).unwrap();
        let mut builder = FunctionBuilder::new(&mut module.types, &[], &[]);
        builder.name("ctor".to_string()).func_body();
        module.start = Some(module.funcs.add_local(builder.local_func(vec![])));

        let out = resolve_against_base(&module.emit_wasm(), &base_with(&[], &[], &[])).unwrap();
        assert!(Module::from_buffer(&out).unwrap().start.is_none());
    }
}
