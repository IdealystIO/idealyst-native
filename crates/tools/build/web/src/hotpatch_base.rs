//! Make the base wasm module one a patch can actually link against.
//!
//! # The problem this solves
//!
//! A patch is a PIC side module. Everything it does not define itself it
//! imports, and those imports have to resolve against the module already
//! running in the page. The obvious way to make a function resolvable is
//! to export it — and that does not work here, for two measured reasons:
//!
//! 1. **wasm-bindgen garbage-collects.** Measured on the probe crate:
//!    rustc linked 2993 functions with `--no-gc-sections`, and
//!    wasm-bindgen's own pass cut that to 560, keeping only what is
//!    reachable from the 12 exports and the element segment. Passing
//!    `--export-dynamic` to LLD does not help, because a function with
//!    internal linkage — every private Rust `fn`, including the
//!    `__<Name>_hot_impl` the `#[component]` split produces — is not a
//!    dynamic symbol and is not exported by it.
//! 2. **`-Clink-dead-code` is not available to us.** It is the flag that
//!    would make rustc emit every monomorphization rather than only the
//!    reachable ones, and it panics wasm-bindgen 0.2.128 outright, at
//!    `wasm-bindgen-cli-support/src/descriptor.rs:324`, "index out of
//!    bounds: the len is 0 but the index is 0". Measured with and
//!    without `--export-dynamic`; the flag alone is enough to trigger it.
//!
//! # What we do instead
//!
//! Promote every local function into `__indirect_function_table` before
//! wasm-bindgen runs. A table entry is a GC root, so wasm-bindgen keeps
//! the function; and a table index is exactly what the patch needs
//! anyway, since a wasm `fn` pointer IS a table index and the patch's
//! `GOT.func` imports are satisfied with one. This is the technique
//! dioxus's `dx` uses, and it sidesteps wasm-bindgen's export-count
//! ceiling at the same time (emscripten#22863).
//!
//! The table is also how a patch reaches wasm-bindgen's own intrinsics.
//! Giving them a second export name to import by was tried and broke the
//! page — see [`prepare_base_module`]'s note — so there is one mechanism
//! here, not two.
//!
//! # Cost, and why it is dev-only
//!
//! This inflates the table by roughly the module's function count and
//! defeats wasm-bindgen's dead-code pass wholesale. It runs only when
//! the hot-patch tier is armed, which is a dev-mode switch; a release
//! bundle never sees it.

use anyhow::{Context, Result};
use std::collections::HashSet;
use walrus::{
    ir, ElementItems, ElementKind, FunctionBuilder, FunctionId, FunctionKind, ImportKind, Module,
};

/// Rewrite a freshly linked base module so a patch can resolve against
/// it. Returns the new module's bytes.
///
/// Must run BEFORE wasm-bindgen: the whole point is to be holding the
/// GC roots when wasm-bindgen's pass runs.
pub fn prepare_base_module(wasm: &[u8]) -> Result<Vec<u8>> {
    let mut module = Module::from_buffer(wasm).context("parsing the linked base module")?;

    let already_indirect = functions_in_the_table(&module);
    let mut promote: Vec<FunctionId> = Vec::new();
    let mut exported: HashSet<String> = HashSet::new();

    // NOTE: no `__saved_wbg_` alias exports. An earlier version added
    // one per `__wbindgen*` function so a patch could import it by that
    // name, and it broke the page: wasm-bindgen deletes the original
    // `__wbindgen_exn_store` export, then looks up "the export name for
    // this function" to generate its JS — and found OUR alias, emitting
    //
    //     wasm.__saved_wbg___wbindgen_exn_store.command_export(idx)
    //
    // which is not a function and threw on the first handled error, mid
    // boot. The table is the right place for these, exactly as it is for
    // everything else: `hotpatch_patch` resolves a patch's
    // `__wbindgen_placeholder__` import through the base's table under
    // the function's own name, with no second name for wasm-bindgen to
    // trip over.

    // And now the main event: every local function that is not already
    // reachable through the table gets a slot, so wasm-bindgen's GC
    // treats it as live and a patch can call it by index.
    //
    // `is_bindgen_internal` is excluded because those functions only
    // exist for wasm-bindgen's own descriptor interpreter, which runs
    // over them and then expects them gone. Rooting one keeps it alive
    // into the output where it has no meaning.
    let candidates: Vec<FunctionId> = module
        .funcs
        .iter()
        .filter(|f| matches!(f.kind, FunctionKind::Local(_)))
        .filter(|f| !already_indirect.contains(&f.id()))
        .filter(|f| !f.name.as_deref().is_some_and(is_bindgen_internal))
        .map(|f| f.id())
        .collect();
    promote.extend(candidates);

    if promote.is_empty() {
        return Ok(module.emit_wasm());
    }
    let added = promote.len() as u64;

    // Append to the LAST active segment rather than adding a new one.
    // A new segment would need its own offset, and the only offset that
    // is certainly free is the one just past the existing entries —
    // which is what appending gives us for nothing.
    let last_active = module
        .elements
        .iter()
        .filter(|e| matches!(e.kind, ElementKind::Active { .. }))
        .map(|e| e.id())
        .last()
        .context(
            "the base module has no active element segment to grow — \
             was it linked without `--export-table`?",
        )?;
    let segment = module.elements.get_mut(last_active);
    let (table, offset) = match &segment.kind {
        ElementKind::Active { table, offset } => (*table, const_offset(offset)),
        _ => unreachable!("filtered to active segments"),
    };
    let ElementItems::Functions(entries) = &mut segment.items else {
        anyhow::bail!("the base module's element segment is not a function table");
    };
    entries.extend(promote);
    let needed = offset + entries.len() as u64;

    // The table has to actually be big enough to hold what the segment
    // now writes into it. A `maximum` left where it was turns this into
    // an instantiation failure in the browser rather than a build error
    // — a long way from the cause.
    //
    // `initial + added` is what a normally-linked module wants, since
    // LLD already sized it for the entries it emitted. `needed` is the
    // floor the segment itself imposes. Taking the larger is correct
    // either way and does not assume LLD's arithmetic.
    let table = module.tables.get_mut(table);
    table.initial = (table.initial + added).max(needed);
    if let Some(max) = table.maximum {
        table.maximum = Some(max.max(table.initial));
    }

    Ok(module.emit_wasm())
}

/// The constant an active segment's offset folds to. A base module is
/// linked non-PIC, so this is always an `i32.const`; anything else
/// means we are looking at a module we were not handed, and zero is the
/// conservative floor (it can only make the table larger).
fn const_offset(expr: &walrus::ConstExpr) -> u64 {
    match expr {
        walrus::ConstExpr::Value(ir::Value::I32(v)) => (*v).max(0) as u64,
        walrus::ConstExpr::Value(ir::Value::I64(v)) => (*v).max(0) as u64,
        _ => 0,
    }
}


/// Give every import wasm-bindgen's generated JS will not supply a local
/// body that traps, so the module can instantiate.
///
/// Run AFTER wasm-bindgen, and only on a hot-patch build.
///
/// # What this is for
///
/// `prepare_base_module` roots every function so wasm-bindgen's dead-code
/// pass keeps it, and some of what it keeps is the descriptor machinery:
/// functions that call `__wbindgen_placeholder__.__wbindgen_describe`.
/// wasm-bindgen interprets descriptors at build time and emits no JS
/// binding for that import, so the page dies before it starts:
///
/// ```text
/// WebAssembly.instantiate(): Import #0 "__wbindgen_placeholder__":
/// module is not an object or function
/// ```
///
/// # Why a trap rather than not rooting them
///
/// Deciding which functions keep the import alive, and leaving those
/// out, was tried twice and is not decidable from the call graph. One hop
/// missed the survivors; walking transitively excluded a THIRD of the
/// module — `Closure::wrap` calls a `describe` function, so every event
/// handler reaching it went too — and ordinary patches then failed to
/// link against `<u32 as Display>::fmt`.
///
/// So root everything, and make the leftover import harmless instead. A
/// descriptor function is never called at run time: wasm-bindgen has
/// already read what it describes. If one somehow were, `unreachable`
/// traps at the call with a stack rather than returning a plausible
/// value, which is the loud failure this is allowed to have.
pub fn neutralize_unsupplied_imports(wasm: &[u8]) -> Result<Option<Vec<u8>>> {
    let mut module = Module::from_buffer(wasm).context("parsing the bindgened module")?;

    // The two namespaces wasm-bindgen uses for "I will resolve this
    // myself". Everything else in the import list is either a real JS
    // shim it generated a binding for, or the memory and table.
    const UNSUPPLIED: [&str; 2] = ["__wbindgen_placeholder__", "__wbindgen_externref_xform__"];

    let stranded: Vec<_> = module
        .imports
        .iter()
        .filter(|i| UNSUPPLIED.contains(&i.module.as_str()))
        .filter_map(|i| match i.kind {
            ImportKind::Function(func) => Some((i.id(), func, i.name.clone())),
            _ => None,
        })
        .collect();
    if stranded.is_empty() {
        return Ok(None);
    }

    for (import_id, func_id, name) in stranded {
        let ty_id = module.funcs.get(func_id).ty();
        let ty = module.types.get(ty_id);
        let params = ty.params().to_vec();
        let results = ty.results().to_vec();
        let locals: Vec<_> = params.iter().map(|t| module.locals.add(*t)).collect();

        let mut builder = FunctionBuilder::new(&mut module.types, &params, &results);
        builder
            .name(format!("__idealyst_stranded_{name}"))
            .func_body()
            .unreachable();

        module.imports.delete(import_id);
        let func = module.funcs.get_mut(func_id);
        func.kind = FunctionKind::Local(builder.local_func(locals));
        func.name = Some(format!("__idealyst_stranded_{name}"));
    }

    Ok(Some(module.emit_wasm()))
}

/// Every function already reachable through an active element segment.
/// Promoting one of these again would give it two slots — two valid
/// pointers to one body — and inflate the table for nothing.
fn functions_in_the_table(module: &Module) -> HashSet<FunctionId> {
    let mut out = HashSet::new();
    for element in module.elements.iter() {
        if !matches!(element.kind, ElementKind::Active { .. }) {
            continue;
        }
        if let ElementItems::Functions(ids) = &element.items {
            out.extend(ids.iter().copied());
        }
    }
    out
}

/// Build a local function with the same type as `target` that forwards
/// its arguments and calls it. Used to give an import a second, local
/// identity that wasm-bindgen's deletion pass will not match.
fn call_through(module: &mut Module, target: FunctionId, name: &str) -> FunctionId {
    let ty_id = module.funcs.get(target).ty();
    let ty = module.types.get(ty_id);
    let params = ty.params().to_vec();
    let results = ty.results().to_vec();

    let locals: Vec<_> = params.iter().map(|t| module.locals.add(*t)).collect();
    let mut builder = FunctionBuilder::new(&mut module.types, &params, &results);
    let mut body = builder.name(name.to_string()).func_body();
    for local in &locals {
        body.local_get(*local);
    }
    body.instr(ir::Instr::Call(ir::Call { func: target }));

    module.funcs.add_local(builder.local_func(locals))
}

/// A wasm-bindgen JS-call intrinsic — the thing an `extern "wbg"` block
/// becomes. Excludes the descriptor machinery, which must not survive.
fn is_wbg_intrinsic(name: &str) -> bool {
    (name.starts_with("__wbindgen") || name.starts_with("__wbg_")) && !is_bindgen_internal(name)
}

/// A symbol that exists only for wasm-bindgen's own descriptor pass.
///
/// These are interpreted at bindgen time and are expected to be gone
/// afterwards; keeping one alive puts a function in the output whose
/// body wasm-bindgen has already consumed. The list mirrors the
/// heuristics in wasm-bindgen's own `cli-support/src/wit/mod.rs`, which
/// is where the authority for it lives.
pub(crate) fn is_bindgen_internal(name: &str) -> bool {
    name.contains("__wbindgen_describe")
        || name.contains("__wbindgen_externref")
        || name.contains("wasm_bindgen8describe6inform")
        || name.contains("wasm_bindgen..describe..WasmDescribe")
        || name.contains("wasm_bindgen..closure..WasmClosure$GT$8describe")
        || name.contains("wasm_bindgen7closure16Closure$LT$T$GT$4wrap8describe")
        || name.contains("wasm_bindgen4__rt8wbg_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use walrus::{ConstExpr, ValType};

    /// A module with `n` local functions, `in_table` of which start out
    /// in an active element segment.
    fn module_with(n: usize, in_table: usize) -> (Module, Vec<FunctionId>) {
        let mut module = Module::default();
        let table = module.tables.add_local(false, 0, Some(64), walrus::RefType::FUNCREF);

        let mut ids = Vec::new();
        for i in 0..n {
            let mut builder = FunctionBuilder::new(&mut module.types, &[], &[ValType::I32]);
            builder
                .name(format!("__F{i}_hot_impl"))
                .func_body()
                .i32_const(i as i32);
            ids.push(module.funcs.add_local(builder.local_func(vec![])));
        }

        module.elements.add(
            ElementKind::Active {
                table,
                offset: ConstExpr::Value(ir::Value::I32(1)),
            },
            ElementItems::Functions(ids[..in_table].to_vec()),
        );
        (module, ids)
    }

    fn table_entries(wasm: &[u8]) -> Vec<String> {
        let module = Module::from_buffer(wasm).unwrap();
        let mut out = Vec::new();
        for element in module.elements.iter() {
            if let ElementItems::Functions(ids) = &element.items {
                for id in ids {
                    out.push(module.funcs.get(*id).name.clone().unwrap_or_default());
                }
            }
        }
        out
    }

    /// The core claim. wasm-bindgen's GC keeps what the element segment
    /// roots and drops the rest — measured, 2993 functions in, 560 out —
    /// so every function a patch might call has to be in the table
    /// before wasm-bindgen runs.
    #[test]
    fn every_local_function_ends_up_in_the_table() {
        let (mut module, _) = module_with(5, 2);
        let out = prepare_base_module(&module.emit_wasm()).unwrap();
        let entries = table_entries(&out);
        assert_eq!(entries.len(), 5, "all five rooted, got {entries:?}");
        for i in 0..5 {
            assert!(
                entries.contains(&format!("__F{i}_hot_impl")),
                "missing __F{i}_hot_impl in {entries:?}"
            );
        }
    }

    /// A function already reachable through the table must not get a
    /// second slot: two pointers to one body, and a table inflated by
    /// the duplicates on a module with thousands of functions.
    #[test]
    fn a_function_already_in_the_table_is_not_added_twice() {
        let (mut module, _) = module_with(4, 3);
        let out = prepare_base_module(&module.emit_wasm()).unwrap();
        let entries = table_entries(&out);
        assert_eq!(entries.len(), 4);
        let mut sorted = entries.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 4, "duplicate slots in {entries:?}");
    }

    /// The table's declared size has to cover the entries it now holds.
    /// A `maximum` left where it was turns this into an instantiation
    /// failure in the browser — far from the cause.
    #[test]
    fn the_table_grows_to_fit_what_was_added() {
        let (mut module, _) = module_with(6, 1);
        let out = prepare_base_module(&module.emit_wasm()).unwrap();
        let module = Module::from_buffer(&out).unwrap();
        let table = module.tables.iter().next().unwrap();
        // One entry was in the segment at offset 1; five were added.
        assert!(table.initial >= 6, "initial was {}", table.initial);
        assert!(
            table.maximum.is_none_or(|m| m >= table.initial),
            "maximum {:?} below initial {}",
            table.maximum,
            table.initial
        );
    }

    /// Regression: NO alias export. An earlier version added
    /// `__saved_wbg_<name>` per `__wbindgen*` function so a patch could
    /// import it by that name. wasm-bindgen deletes the original
    /// `__wbindgen_exn_store` export, then looks up "the export name for
    /// this function" to generate its JS — and found the alias, emitting
    /// `wasm.__saved_wbg___wbindgen_exn_store.command_export(idx)`,
    /// which is not a function. The page threw on its first handled
    /// error, mid boot.
    ///
    /// The table is where these belong, like everything else.
    #[test]
    fn regression_a_wbindgen_intrinsic_gets_no_alias_export() {
        let mut module = Module::default();
        let table = module
            .tables
            .add_local(false, 0, Some(64), walrus::RefType::FUNCREF);
        let mut builder = FunctionBuilder::new(&mut module.types, &[], &[]);
        builder.name("__wbindgen_exn_store".to_string()).func_body();
        let id = module.funcs.add_local(builder.local_func(vec![]));
        module.exports.add("__wbindgen_exn_store", id);
        module.elements.add(
            ElementKind::Active {
                table,
                offset: ConstExpr::Value(ir::Value::I32(1)),
            },
            ElementItems::Functions(vec![]),
        );

        let out = prepare_base_module(&module.emit_wasm()).unwrap();
        let module = Module::from_buffer(&out).unwrap();
        assert!(
            !module.exports.iter().any(|e| e.name.starts_with("__saved_wbg_")),
            "an alias export makes wasm-bindgen generate the wrong accessor: {:?}",
            module.exports.iter().map(|e| &e.name).collect::<Vec<_>>()
        );
        assert!(
            table_entries(&out).contains(&"__wbindgen_exn_store".to_string()),
            "it still has to be reachable through the table: {:?}",
            table_entries(&out)
        );
    }

    /// Descriptor functions are consumed by wasm-bindgen and expected to
    /// be gone. Rooting one keeps a husk in the output.
    #[test]
    fn a_bindgen_descriptor_function_is_not_rooted() {
        let mut module = Module::default();
        let table = module
            .tables
            .add_local(false, 0, Some(64), walrus::RefType::FUNCREF);
        let mut builder = FunctionBuilder::new(&mut module.types, &[], &[]);
        builder
            .name("__wbindgen_describe_foo".to_string())
            .func_body();
        module.funcs.add_local(builder.local_func(vec![]));
        module.elements.add(
            ElementKind::Active {
                table,
                offset: ConstExpr::Value(ir::Value::I32(1)),
            },
            ElementItems::Functions(vec![]),
        );

        let out = prepare_base_module(&module.emit_wasm()).unwrap();
        assert!(
            table_entries(&out).is_empty(),
            "descriptor fn was rooted: {:?}",
            table_entries(&out)
        );
    }

    /// Regression: an import wasm-bindgen will not supply is given a
    /// trapping body rather than left to fail instantiation.
    ///
    /// Rooting every function keeps the descriptor machinery alive, and
    /// wasm-bindgen emits no JS binding for
    /// `__wbindgen_placeholder__.__wbindgen_describe` — so the page died
    /// before it started with "Import #0 __wbindgen_placeholder__: module
    /// is not an object or function".
    ///
    /// Deciding which functions keep it alive and leaving those out was
    /// tried twice and does not work: one hop missed the survivors, and
    /// walking transitively excluded a third of the module because
    /// `Closure::wrap` calls a `describe` function — ordinary patches
    /// then failed to link against `<u32 as Display>::fmt`. Rooting
    /// everything and making the leftover harmless is what holds.
    #[test]
    fn regression_a_stranded_placeholder_import_becomes_a_trap() {
        let mut module = Module::default();
        let ty = module.types.add(&[ValType::I32], &[]);
        let (describe, _) =
            module.add_import_func("__wbindgen_placeholder__", "__wbindgen_describe", ty);
        let mut builder = FunctionBuilder::new(&mut module.types, &[], &[]);
        builder
            .name("caller".to_string())
            .func_body()
            .i32_const(7)
            .call(describe);
        module.funcs.add_local(builder.local_func(vec![]));

        let out = neutralize_unsupplied_imports(&module.emit_wasm())
            .unwrap()
            .expect("a stranded import to neutralize");
        let module = Module::from_buffer(&out).unwrap();
        assert!(
            !module
                .imports
                .iter()
                .any(|i| i.module == "__wbindgen_placeholder__"),
            "still imported: {:?}",
            module.imports.iter().map(|i| &i.module).collect::<Vec<_>>()
        );
        let stranded = module
            .funcs
            .iter()
            .find(|f| f.name.as_deref() == Some("__idealyst_stranded___wbindgen_describe"))
            .expect("the trapping stand-in");
        let FunctionKind::Local(local) = &stranded.kind else {
            panic!("the import should have become a local function");
        };
        assert!(
            local
                .block(local.entry_block())
                .instrs
                .iter()
                .any(|(i, _)| matches!(i, ir::Instr::Unreachable(_))),
            "a descriptor function is never called at run time; if one is, it must trap \
             rather than return a plausible value"
        );
    }

    /// A module wasm-bindgen fully satisfied is handed back untouched —
    /// a normal build must not pay for this at all.
    #[test]
    fn a_module_with_no_stranded_imports_is_left_alone() {
        let mut module = Module::default();
        let ty = module.types.add(&[], &[]);
        module.add_import_func("./app_bg.js", "__wbg_log", ty);
        assert!(neutralize_unsupplied_imports(&module.emit_wasm())
            .unwrap()
            .is_none());
    }

    /// A module linked without `--export-table` has no segment to grow,
    /// and there is no safe offset to invent one at. Say so rather than
    /// producing a module that instantiates and dispatches nowhere.
    #[test]
    fn a_module_with_no_element_segment_is_refused_loudly() {
        let mut module = Module::default();
        let mut builder = FunctionBuilder::new(&mut module.types, &[], &[]);
        builder.name("__A_hot_impl".to_string()).func_body();
        module.funcs.add_local(builder.local_func(vec![]));

        let err = prepare_base_module(&module.emit_wasm()).unwrap_err();
        assert!(
            format!("{err:#}").contains("element segment"),
            "unhelpful error: {err:#}"
        );
    }
}
