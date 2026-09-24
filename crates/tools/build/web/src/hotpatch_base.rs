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

/// What one run of [`prepare_base_module`] did, in the four numbers
/// that decide whether a patch can link.
///
/// A patch resolves every call it does not define itself against the
/// base's table, so "how many functions ended up in the table" is the
/// whole story — and when a patch fails on an unresolved import, the
/// first question is always whether that function was dropped here or
/// never codegened at all. Reporting the census rather than one total
/// is what separates those two.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BasePrep {
    /// Local (non-imported) functions in the module we were handed.
    pub locals: usize,
    /// Of those, already reachable through an active element segment.
    pub already_indirect: usize,
    /// Of those, skipped because they exist only for wasm-bindgen's
    /// descriptor interpreter.
    pub bindgen_internal: usize,
    /// Of those, appended to the element segment by this pass.
    pub promoted: usize,
    /// Forwarding bodies added for wasm-bindgen JS-shim imports (and
    /// rooted on top of `promoted`), so a patch can call a shim.
    pub shim_trampolines: usize,
    /// Forwarding bodies added for wasm-bindgen's cast intrinsics
    /// (`wbg_cast::breaks_if_inlined<…>`), rooted on top of `promoted`.
    /// See [`cast_trampoline_name`].
    pub cast_trampolines: usize,
    /// Slots in the emitted module's element segments. Lower than
    /// `already_indirect + promoted` means walrus's emit-time GC
    /// dropped something we rooted.
    pub slots_emitted: usize,
    /// Local functions in the emitted module. Lower than `locals` is
    /// the same GC, seen from the other side.
    pub locals_emitted: usize,
}

impl BasePrep {
    /// The one-line census for the build log.
    pub fn summary(&self) -> String {
        format!(
            "{} local fns = {} already in the table + {} bindgen-internal + {} promoted, \
             + {} JS-shim + {} cast trampolines → emitted {} slots over {} local fns",
            self.locals,
            self.already_indirect,
            self.bindgen_internal,
            self.promoted,
            self.shim_trampolines,
            self.cast_trampolines,
            self.slots_emitted,
            self.locals_emitted,
        )
    }
}

/// Rewrite a freshly linked base module so a patch can resolve against
/// it. Returns the new module's bytes and the census of what it did.
///
/// Must run BEFORE wasm-bindgen: the whole point is to be holding the
/// GC roots when wasm-bindgen's pass runs.
pub fn prepare_base_module(wasm: &[u8]) -> Result<(Vec<u8>, BasePrep)> {
    let mut module = Module::from_buffer(wasm).context("parsing the linked base module")?;
    let mut report = BasePrep::default();

    let already_indirect = functions_in_the_table(&module);
    let mut promote: Vec<FunctionId> = Vec::new();

    // A JS shim — what an `extern "C"` block under `#[wasm_bindgen]`
    // becomes — is an IMPORT in the base, not a function, so it has no
    // table slot, and a patch that calls one has nothing to resolve to.
    // So is a `#[component(lazy)]` loader in a no-split build: an import
    // from `./__wasm_split.js` that the stub module answers at once. On
    // CrewForge the first patch that got past the shims stopped on 17 of
    // those. Every function import gets a trampoline, except wasm-bindgen's
    // own descriptor and externref-transform machinery, which it rewrites
    // or deletes and which no user code calls.
    // Give each a local forwarding body under a name wasm-bindgen does
    // not recognise, and root that. The shim's own import stays exactly
    // where wasm-bindgen expects it; the trampoline just calls it.
    //
    // NOT exported. The first version of this exported the trampoline as
    // `__saved_wbg_<name>`, and wasm-bindgen then generated its JS
    // accessors against OUR export name (see below). A table slot is a
    // GC root without being a name wasm-bindgen can find.
    let shims: Vec<(FunctionId, String)> = module
        .imports
        .iter()
        .filter(|i| {
            i.module != "env"
                && i.module != "__wbindgen_externref_xform__"
                && !is_bindgen_internal(&i.name)
        })
        .filter_map(|i| match i.kind {
            ImportKind::Function(f) => Some((f, i.name.clone())),
            _ => None,
        })
        .collect();
    let mut trampolines: HashSet<FunctionId> = HashSet::new();
    for (import, name) in shims {
        let trampoline = call_through(&mut module, import, &shim_trampoline_name(&name));
        promote.push(trampoline);
        trampolines.insert(trampoline);
        report.shim_trampolines += 1;
    }

    // wasm-bindgen's CAST intrinsics — `wbg_cast::breaks_if_inlined<F, T>`,
    // one instantiation per closure or value type crossing into JS — are
    // local functions here, and gone by name after bindgen: wasm-bindgen
    // reads each one's descriptor and replaces the function with an
    // import named after its sequence number (`__wbindgen_cast_000…8`),
    // whose name-section entry is the descriptor's debug text. A patch
    // compiled from a crate that builds a `Closure` itself (CrewForge's
    // ui-shared: WebSocket and audio handlers) still calls the
    // instantiation by its MANGLED name, and nothing in the served module
    // answers to it — the first ui-shared patch was refused over six of
    // them. So each gets a forwarding body under a name that keeps the
    // mangled one, rooted like every other. wasm-bindgen repoints the
    // body's call at the import it generates (it replaces the callee,
    // not the call sites), and `BaseIndex` resolves the mangled name to
    // the forwarder's slot.
    let casts: Vec<(FunctionId, String)> = module
        .funcs
        .iter()
        .filter(|f| matches!(f.kind, FunctionKind::Local(_)))
        .filter_map(|f| f.name.as_deref().filter(|n| is_bindgen_cast(n)).map(|n| (f.id(), n.to_string())))
        .collect();
    for (cast, name) in casts {
        let trampoline = call_through(&mut module, cast, &cast_trampoline_name(&name));
        promote.push(trampoline);
        trampolines.insert(trampoline);
        report.cast_trampolines += 1;
    }

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
    let mut candidates: Vec<FunctionId> = Vec::new();
    for f in module.funcs.iter() {
        if !matches!(f.kind, FunctionKind::Local(_)) {
            continue;
        }
        // Already queued above.
        if trampolines.contains(&f.id()) {
            continue;
        }
        report.locals += 1;
        if already_indirect.contains(&f.id()) {
            report.already_indirect += 1;
            continue;
        }
        if f.name.as_deref().is_some_and(is_bindgen_internal) {
            report.bindgen_internal += 1;
            continue;
        }
        candidates.push(f.id());
    }
    report.promoted = candidates.len();
    promote.extend(candidates);

    if promote.is_empty() {
        let out = module.emit_wasm();
        drop(module);
        let report = census(&out, report);
        return Ok((out, report));
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

    let out = module.emit_wasm();
    drop(module);
    let report = census(&out, report);
    Ok((out, report))
}

/// Fill in the two after-the-fact numbers by re-reading what we emitted.
///
/// walrus runs a GC on `emit_wasm`, and its roots are the exports, the
/// start function and the element segments of IMPORTED tables — a
/// locally-defined table's segments are reached only through the table,
/// and only if the table itself is rooted (walrus 0.26
/// `passes/used.rs`). A base linked without `--export-table` therefore
/// loses every slot this pass just added, silently. Counting the output
/// is how that shows up as a number instead of as an unresolved import
/// three minutes later.
///
/// Streams the sections with wasmparser rather than parsing a second
/// walrus module: on CrewForge the emitted module is 223 MB, and a
/// second walrus parse of it pushed the CLI over its 4 GB memory cap.
fn census(out: &[u8], mut report: BasePrep) -> BasePrep {
    use wasmparser::{Parser, Payload};
    for payload in Parser::new(0).parse_all(out) {
        match payload {
            Ok(Payload::FunctionSection(reader)) => {
                report.locals_emitted = reader.count() as usize;
            }
            Ok(Payload::ElementSection(reader)) => {
                for element in reader.into_iter().flatten() {
                    if let wasmparser::ElementItems::Functions(funcs) = element.items {
                        report.slots_emitted += funcs.count() as usize;
                    }
                }
            }
            Ok(_) => {}
            Err(_) => return report,
        }
    }
    report
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

/// The name the base's forwarding body for JS-shim import `import` goes
/// by. `hotpatch_patch` resolves a patch's `__wbindgen_placeholder__`
/// import through the table slot of this name, so the two sides share
/// this one function rather than two spellings of it.
pub fn shim_trampoline_name(import: &str) -> String {
    format!("__idealyst_shim_{import}")
}

/// The prefix of a cast intrinsic's forwarding body. See the note on
/// casts in [`prepare_base_module`].
pub const CAST_TRAMPOLINE_PREFIX: &str = "__idealyst_cast_";

/// The name the base's forwarding body for cast intrinsic `symbol` (its
/// mangled name) goes by. `BaseIndex` strips the prefix to resolve a
/// patch's call to `symbol` through this body's slot.
pub fn cast_trampoline_name(symbol: &str) -> String {
    format!("{CAST_TRAMPOLINE_PREFIX}{symbol}")
}

/// wasm-bindgen's cast intrinsic, `wasm_bindgen::__rt::wbg_cast::
/// breaks_if_inlined<From, To>`, in either mangling.
pub(crate) fn is_bindgen_cast(name: &str) -> bool {
    name.contains("wbg_cast") && name.contains("breaks_if_inlined")
}

/// Build a local function with the same type as `target` that forwards
/// its arguments and calls it.
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
        let table = module
            .tables
            .add_local(false, 0, Some(64), walrus::RefType::FUNCREF);

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
        let (out, census) = prepare_base_module(&module.emit_wasm()).unwrap();
        // The census is what a failed patch's investigation starts from,
        // so its arithmetic has to hold: nothing dropped, nothing
        // double-counted.
        assert_eq!(
            (census.locals, census.already_indirect, census.promoted),
            (5, 2, 3),
            "{census:?}"
        );
        assert_eq!((census.slots_emitted, census.locals_emitted), (5, 5), "{census:?}");
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
        let (out, _) = prepare_base_module(&module.emit_wasm()).unwrap();
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
        let (out, _) = prepare_base_module(&module.emit_wasm()).unwrap();
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

        let (out, _) = prepare_base_module(&module.emit_wasm()).unwrap();
        let module = Module::from_buffer(&out).unwrap();
        assert!(
            !module
                .exports
                .iter()
                .any(|e| e.name.starts_with("__saved_wbg_")),
            "an alias export makes wasm-bindgen generate the wrong accessor: {:?}",
            module.exports.iter().map(|e| &e.name).collect::<Vec<_>>()
        );
        assert!(
            table_entries(&out).contains(&"__wbindgen_exn_store".to_string()),
            "it still has to be reachable through the table: {:?}",
            table_entries(&out)
        );
    }

    /// Regression: a patch calling a JS shim — `log` from an `extern
    /// "C"` block under `#[wasm_bindgen]` — was refused, because the shim
    /// is an IMPORT in the base and imports have no table slot. The
    /// roundtrip test caught it once the `__saved_wbg_` alias exports
    /// (which had been covering it) were removed for breaking the page.
    ///
    /// Each shim import now gets a forwarding body, rooted in the table
    /// and NOT exported: an export is a name wasm-bindgen can mistake for
    /// the shim's own.
    #[test]
    fn regression_a_js_shim_import_gets_a_rooted_unexported_trampoline() {
        let mut module = Module::default();
        let table = module
            .tables
            .add_local(false, 0, Some(64), walrus::RefType::FUNCREF);
        let ty = module.types.add(&[ValType::I32], &[]);
        let (shim, _) = module.add_import_func("__wbindgen_placeholder__", "__wbg_log_abc", ty);
        let (_describe, _) =
            module.add_import_func("__wbindgen_placeholder__", "__wbindgen_describe", ty);
        let mut b = FunctionBuilder::new(&mut module.types, &[], &[]);
        b.name("caller".to_string())
            .func_body()
            .i32_const(1)
            .call(shim);
        let caller = module.funcs.add_local(b.local_func(vec![]));
        module.exports.add("caller", caller);
        module.elements.add(
            ElementKind::Active {
                table,
                offset: ConstExpr::Value(ir::Value::I32(1)),
            },
            ElementItems::Functions(vec![]),
        );

        let (out, census) = prepare_base_module(&module.emit_wasm()).unwrap();
        assert_eq!(census.shim_trampolines, 1, "{census:?}");
        let entries = table_entries(&out);
        assert!(
            entries.contains(&shim_trampoline_name("__wbg_log_abc")),
            "{entries:?}"
        );
        assert!(
            !entries.iter().any(|e| e.contains("__wbindgen_describe")),
            "a descriptor import must not get a live body: {entries:?}"
        );
        let module = Module::from_buffer(&out).unwrap();
        assert!(
            !module.exports.iter().any(|e| e.name.contains("shim")),
            "the trampoline must not be exported"
        );
    }

    /// Regression: the first patch of CrewForge's ui-shared was refused
    /// over six `wbg_cast::breaks_if_inlined<…>` instantiations — "the
    /// base does not contain this function at all" — because wasm-bindgen
    /// replaces each with an import under a SEQUENCE name. Each gets a
    /// rooted, unexported forwarder that keeps the mangled name.
    #[test]
    fn regression_a_cast_intrinsic_gets_a_rooted_forwarder_under_its_mangled_name() {
        let cast_name = "_RINvNvNtCs8e_12wasm_bindgen4___rt8wbg_cast17breaks_if_inlinedReNtB6_7JsValueEB6_";
        let mut module = Module::default();
        let table = module.tables.add_local(false, 0, Some(64), walrus::RefType::FUNCREF);
        let mut b = FunctionBuilder::new(&mut module.types, &[ValType::I32], &[ValType::I32]);
        let arg = module.locals.add(ValType::I32);
        b.name(cast_name.to_string()).func_body().local_get(arg);
        let cast = module.funcs.add_local(b.local_func(vec![arg]));
        let mut b = FunctionBuilder::new(&mut module.types, &[], &[ValType::I32]);
        b.name("caller".to_string()).func_body().i32_const(7).call(cast);
        let caller = module.funcs.add_local(b.local_func(vec![]));
        module.exports.add("caller", caller);
        module.elements.add(
            ElementKind::Active { table, offset: ConstExpr::Value(ir::Value::I32(1)) },
            ElementItems::Functions(vec![]),
        );

        let (out, census) = prepare_base_module(&module.emit_wasm()).unwrap();
        assert_eq!(census.cast_trampolines, 1, "{census:?}");
        let entries = table_entries(&out);
        assert!(entries.contains(&cast_trampoline_name(cast_name)), "{entries:?}");
        let module = Module::from_buffer(&out).unwrap();
        assert!(!module.exports.iter().any(|e| e.name.contains(CAST_TRAMPOLINE_PREFIX)));
        // The forwarder calls the cast itself — the call wasm-bindgen
        // repoints at the import it generates.
        let fwd = module
            .funcs
            .iter()
            .find(|f| f.name.as_deref() == Some(cast_trampoline_name(cast_name).as_str()))
            .unwrap();
        let walrus::FunctionKind::Local(local) = &fwd.kind else { panic!("not local") };
        let calls_cast = local.block(local.entry_block()).instrs.iter().any(|(i, _)| {
            matches!(i, ir::Instr::Call(c) if module.funcs.get(c.func).name.as_deref() == Some(cast_name))
        });
        assert!(calls_cast, "the forwarder must call the cast intrinsic");
    }

    /// Regression: on CrewForge, a patch that got past every JS shim
    /// stopped on 17 imports from `./__wasm_split.js` — the
    /// `#[component(lazy)]` loaders a no-split build answers from a stub
    /// module. They are imports in the base exactly like a JS shim, and
    /// need the same trampoline.
    #[test]
    fn regression_a_lazy_loader_import_gets_a_trampoline_too() {
        let mut module = Module::default();
        let table = module
            .tables
            .add_local(false, 0, Some(64), walrus::RefType::FUNCREF);
        let ty = module.types.add(&[ValType::I32], &[]);
        module.add_import_func("./__wasm_split.js", "__wasm_split_00_lazy_body", ty);
        module.add_import_func("__wbindgen_externref_xform__", "__wbindgen_externref_table_grow", ty);
        module.elements.add(
            ElementKind::Active {
                table,
                offset: ConstExpr::Value(ir::Value::I32(1)),
            },
            ElementItems::Functions(vec![]),
        );

        let (out, census) = prepare_base_module(&module.emit_wasm()).unwrap();
        let entries = table_entries(&out);
        assert!(
            entries.contains(&shim_trampoline_name("__wasm_split_00_lazy_body")),
            "{entries:?}"
        );
        assert_eq!(
            census.shim_trampolines, 1,
            "wasm-bindgen's externref transform is its own business: {entries:?}"
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

        let (out, _) = prepare_base_module(&module.emit_wasm()).unwrap();
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
