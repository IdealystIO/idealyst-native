use anyhow::{Context, Result};
use itertools::Itertools;
use rayon::prelude::{IntoParallelIterator, ParallelIterator};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    hash::Hash,
    ops::Range,
    sync::{Arc, RwLock},
};
use walrus::{
    ConstExpr, DataKind, ElementItems, ElementKind, ExportId, ExportItem, FunctionBuilder,
    FunctionId, FunctionKind, GlobalKind, ImportId, ImportKind, Module, ModuleConfig, RefType,
    TableId, TypeId,
    ir::{self, Visitor, dfs_in_order},
};
use wasmparser::{
    BinaryReader, Linking, LinkingSectionReader, Payload, RelocSectionReader, RelocationEntry,
    SymbolInfo,
};

pub const MAKE_LOAD_JS: &str = include_str!("./__wasm_split.js");

/// Patch a wasm-bindgen-generated module to neutralize the
/// `*.command_export` wrappers that 0.2.122 emits around its internal
/// helpers (`__wbindgen_malloc/realloc/free/exn_store/destroy_closure`,
/// `__externref_table_alloc/dealloc`).
///
/// # The bug
///
/// wasm-bindgen 0.2.122 wraps each helper export in a thin function
/// whose body is `call __wasm_call_ctors; <forward args>; call <bare>`
/// and re-exports it under the same name with a `_command_export`
/// suffix. The generated JS calls the **suffixed** export on every
/// JS↔wasm round trip (every string marshal, every closure invoke).
/// `__wasm_call_ctors` runs every module ctor again — including every
/// `inventory::submit!` — which double-submits items into
/// `inventory`'s global linked list. The list then has a cycle (or a
/// pointer into freed memory), and the next traversal traps with
/// `RuntimeError: memory access out of bounds`. The trap surfaces in
/// whichever frame is on the stack when the corrupted list is read,
/// so it looks like a different bug every time (walker OOB, signal
/// arena OOB, etc.).
///
/// The wrapper around `main` / `host_reserve` *does* legitimately need
/// to call `__wasm_call_ctors` (first-invocation init), so we leave
/// unsuffixed exports alone.
///
/// # The fix
///
/// For each export whose name ends in `_command_export`, look up the
/// bare function by its internal name (the wasm-bindgen helper's
/// real symbol from the `name` section) and rewrite the export to
/// point at it. JS keeps calling `wasm.<bare>_command_export(a, b)`
/// but the call now lands in the bare implementation directly, no
/// ctor re-run. The wrapper function becomes unreachable; `wasm-split`'s
/// reachability walker (or any downstream gc pass) will drop it.
///
/// Signature-safety: the wrapper forwards its args verbatim and
/// returns the bare function's result, so the two have identical
/// wasm function types and the remap is type-safe at the JS call
/// site — `wasm-bindgen`'s JS doesn't observe the change.
pub fn neutralize_command_export_wrappers(bindgened: &[u8]) -> Result<Vec<u8>> {
    let mut module =
        Module::from_buffer(bindgened).context("walrus: parse bindgened wasm")?;

    // --- Pass A: gut each helper wrapper's body ---
    //
    // The export remap below redirects `wasm.__wbindgen_X_command_export`
    // call sites in the generated JS to the bare helper. But the
    // wasm-bindgen externref-closure shim invokes some wrappers
    // *internally* (`call_indirect` through the function table, or a
    // direct `call <wrapper_fid>` baked into another wasm function), so
    // simply moving the export off the wrapper doesn't stop the ctor
    // re-run on those paths. Stripping the `call __wasm_call_ctors`
    // instruction from the wrapper's body neutralizes the bug for all
    // caller paths — direct, indirect, or via the export.
    //
    // We touch only wrappers whose export name ends in
    // `_command_export` (or that are unexported). The `main` and
    // `host_reserve` wrappers are exported under their bare names —
    // they're the legitimate one-time-init entrypoints called from
    // `__wbindgen_start`, and `__wasm_call_ctors` MUST run on first
    // invocation. Stripping their ctor call would mean ctors never run.

    let ctors_fid: Option<FunctionId> = module
        .funcs
        .iter()
        .find(|f| f.name.as_deref() == Some("__wasm_call_ctors"))
        .map(|f| f.id());

    // Map FunctionId → its export name (if exported). Used to spare the
    // legitimate-init wrappers (`main`, `host_reserve`).
    let exported_func_names: HashMap<FunctionId, String> = module
        .exports
        .iter()
        .filter_map(|e| match e.item {
            ExportItem::Function(fid) => Some((fid, e.name.clone())),
            _ => None,
        })
        .collect();

    // Plan: which wrapper FunctionIds to gut?
    let mut to_gut: Vec<FunctionId> = Vec::new();
    if ctors_fid.is_some() {
        for func in module.funcs.iter() {
            let Some(name) = &func.name else { continue };
            if !name.ends_with(".command_export") {
                continue;
            }
            match exported_func_names.get(&func.id()) {
                // Exported with `_command_export` suffix — a JS-side
                // helper wrapper. Gut it.
                Some(export_name) if export_name.ends_with("_command_export") => {
                    to_gut.push(func.id());
                }
                // Not exported — purely internal wrapper still reached
                // via `call_indirect`. Gut it.
                None => to_gut.push(func.id()),
                // Exported under a bare name (`main`, `host_reserve`) —
                // legitimate one-time-init. LEAVE ALONE.
                Some(_) => {}
            }
        }
    }

    if let Some(ctors_fid) = ctors_fid {
        for wrapper_fid in to_gut {
            let func = module.funcs.get_mut(wrapper_fid);
            if let FunctionKind::Local(local_func) = &mut func.kind {
                let entry = local_func.entry_block();
                let block = local_func.block_mut(entry);
                // The wrapper body is shaped as: `call __wasm_call_ctors;
                // <forward args>; call <bare>; end`. Strip the leading
                // call to ctors and leave the forward intact.
                let strip = matches!(
                    block.instrs.first(),
                    Some((ir::Instr::Call(call), _)) if call.func == ctors_fid
                );
                if strip {
                    block.instrs.remove(0);
                }
            }
        }
    }

    // --- Pass B: remap `_command_export`-suffixed EXPORTS to bare ---
    //
    // Belt-and-suspenders on top of Pass A: by pointing the helper
    // exports at the bare functions directly, JS↔wasm round trips skip
    // the (now-empty-ish) wrapper indirection entirely. Functionally
    // redundant after Pass A, but it keeps the export table honest and
    // gives downstream DCE a cleaner reachability picture.

    let by_internal_name: HashMap<String, FunctionId> = module
        .funcs
        .iter()
        .filter_map(|f| f.name.as_ref().map(|n| (n.clone(), f.id())))
        .collect();

    let mut remaps: Vec<(ExportId, FunctionId)> = Vec::new();
    for export in module.exports.iter() {
        if !matches!(export.item, ExportItem::Function(_)) {
            continue;
        }
        let Some(bare_name) = export.name.strip_suffix("_command_export") else {
            continue;
        };
        if let Some(&bare_fid) = by_internal_name.get(bare_name) {
            remaps.push((export.id(), bare_fid));
        }
    }
    for (export_id, new_fid) in remaps {
        module.exports.get_mut(export_id).item = ExportItem::Function(new_fid);
    }

    Ok(module.emit_wasm())
}

/// A parsed wasm module with additional metadata and functionality for splitting and patching.
///
/// This struct assumes that relocations will be present in incoming wasm binary.
/// Upon construction, all the required metadata will be constructed.
pub struct Splitter<'a> {
    /// The original module we use as a reference
    source_module: Module,

    // The byte sources of the pre and post wasm-bindgen .wasm files
    // We need the original around since wasm-bindgen ruins the relocation locations.
    original: &'a [u8],
    bindgened: &'a [u8],

    // Mapping of indices of source functions
    // This lets us use a much faster approach to emitting split modules simply by maintaining a mapping
    // between the original Module and the new Module. Ideally we could just index the new module
    // with old FunctionIds but the underlying IndexMap actually checks that a key belongs to a particular
    // arena.
    fns_to_ids: HashMap<FunctionId, usize>,
    _ids_to_fns: Vec<FunctionId>,

    shared_symbols: BTreeSet<Node>,
    split_points: Vec<SplitPoint>,
    chunks: Vec<HashSet<Node>>,
    data_symbols: BTreeMap<usize, DataSymbol>,
    main_graph: HashSet<Node>,
    call_graph: HashMap<Node, HashSet<Node>>,
    parent_graph: HashMap<Node, HashSet<Node>>,
}

/// The results of splitting the wasm module with some additional metadata for later use.
pub struct OutputModules {
    /// The main chunk
    pub main: SplitModule,

    /// The modules of the wasm module that were split.
    pub modules: Vec<SplitModule>,

    /// The chunks that might be imported by the main modules
    pub chunks: Vec<SplitModule>,
}

/// A wasm module that was split from the main module.
///
/// All IDs here correspond to *this* module - not the parent main module
pub struct SplitModule {
    pub module_name: String,
    pub hash_id: Option<String>,
    pub component_name: Option<String>,
    pub bytes: Vec<u8>,
    pub relies_on_chunks: HashSet<usize>,
}

impl<'a> Splitter<'a> {
    /// Create a new "splitter" instance using the original wasm and the wasm from the output of wasm-bindgen.
    ///
    /// This will use the relocation data from the original module to create a call graph that we
    /// then use with the post-bindgened module to create the split modules.
    ///
    /// It's important to compile the wasm with --emit-relocs such that the relocations are available
    /// to construct the callgraph.
    pub fn new(original: &'a [u8], bindgened: &'a [u8]) -> Result<Self> {
        let (module, ids, fns_to_ids) = parse_module_with_ids(bindgened)?;

        let split_points = accumulate_split_points(&module);

        // Note that we can't trust the normal symbols - just the data symbols - and we can't use the data offset
        // since that's not reliable after bindgening
        let raw_data = parse_bytes_to_data_segment(bindgened)?;

        let mut module = Self {
            source_module: module,
            original,
            bindgened,
            split_points,
            data_symbols: raw_data.data_symbols,
            _ids_to_fns: ids,
            fns_to_ids,
            main_graph: Default::default(),
            chunks: Default::default(),
            call_graph: Default::default(),
            parent_graph: Default::default(),
            shared_symbols: Default::default(),
        };

        module.build_call_graph()?;
        module.build_split_chunks();

        Ok(module)
    }

    /// Split the module into multiple modules at the boundaries of split points.
    ///
    /// Note that the binaries might still be "large" at the end of this process. In practice, you
    /// need to push these binaries through wasm-bindgen and wasm-opt to take advantage of the
    /// optimizations and splitting. We perform a few steps like zero-ing out the data segments
    /// that will only be removed by the memory-packing step of wasm-opt.
    ///
    /// This returns the list of chunks, an import map, and some javascript to link everything together.
    pub fn emit(self) -> Result<OutputModules> {
        tracing::info!("Emitting split modules.");

        let chunks = (0..self.chunks.len())
            .into_par_iter()
            .map(|idx| self.emit_split_chunk(idx))
            .collect::<Result<Vec<SplitModule>>>()?;

        let modules = (0..self.split_points.len())
            .into_par_iter()
            .map(|idx| self.emit_split_module(idx))
            .collect::<Result<Vec<SplitModule>>>()?;

        // Emit the main module, consuming self since we're going to
        let main = self.emit_main_module()?;

        Ok(OutputModules {
            modules,
            chunks,
            main,
        })
    }

    /// Emit the main module.
    ///
    /// This will analyze the call graph and then perform some transformations on the module.
    /// - Clear out active segments that the split modules will initialize
    /// - Wipe away unused functions and data symbols
    /// - Re-export the memories, globals, and other items that the split modules will need
    /// - Convert the split module import functions to real functions that call the indirect function
    ///
    /// Once this is done, all the split module functions will have been removed, making the main module smaller.
    ///
    /// Emitting the main module is conceptually pretty simple. Emitting the split modules is more
    /// complex.
    fn emit_main_module(mut self) -> Result<SplitModule> {
        tracing::info!("Emitting main bundle split module");

        // Perform some analysis of the module before we start messing with it
        let unused_symbols = self.unused_main_symbols();

        // Diagnostic (opt-in): report how much of main's data section is
        // chunk-only — i.e. data that some split module also carries and
        // re-initialises, so it's the theoretical ceiling for a future
        // "carve main's .rodata" size fix. Gated so it's silent normally.
        if std::env::var("IDEALYST_WASM_SPLIT_STATS").is_ok() {
            let mut chunk_only_data_bytes = 0usize;
            let mut chunk_only_data_syms = 0usize;
            let mut chunk_only_funcs = 0usize;
            for sym in &unused_symbols {
                match sym {
                    Node::DataSymbol(id) => {
                        if let Some(ds) = self.data_symbols.get(id) {
                            chunk_only_data_bytes += ds.symbol_size;
                            chunk_only_data_syms += 1;
                        }
                    }
                    Node::Function(_) => chunk_only_funcs += 1,
                }
            }
            let total_data: usize = self.data_symbols.values().map(|d| d.symbol_size).sum();
            eprintln!(
                "[wasm-split stats] chunk-only (removable-from-main ceiling): \
                 {chunk_only_data_bytes} data bytes across {chunk_only_data_syms} symbols \
                 ({} of {total_data} total data-symbol bytes); {chunk_only_funcs} chunk-only funcs",
                if total_data > 0 {
                    format!("{:.1}%", 100.0 * chunk_only_data_bytes as f64 / total_data as f64)
                } else {
                    "0%".to_string()
                },
            );
        }

        // Use the original module that contains all the right ids
        let mut out = std::mem::take(&mut self.source_module);

        // 1. Clear out the active segments that try to initialize functions for modules we just split off.
        //    When the side modules load, they will initialize functions into the table where the "holes" are.
        self.replace_segments_with_holes(&mut out, &unused_symbols);

        // 2. Wipe away the unused functions and data symbols
        self.prune_main_symbols(&mut out, &unused_symbols)?;

        // 3. Change the functions called from split modules to be local functions that call the indirect function
        self.create_ifunc_table(&mut out);

        // 4. Re-export the memories, globals, and other stuff
        self.re_export_items(&mut out);

        // 6. Remove the reloc and linking custom sections
        self.remove_custom_sections(&mut out);

        // 7. Run the garbage collector to remove unused functions
        walrus::passes::gc::run(&mut out);

        Ok(SplitModule {
            module_name: "main".to_string(),
            component_name: None,
            bytes: out.emit_wasm(),
            relies_on_chunks: Default::default(),
            hash_id: None,
        })
    }

    /// Write the contents of the split modules to the output
    fn emit_split_module(&self, split_idx: usize) -> Result<SplitModule> {
        let split = self.split_points[split_idx].clone();

        // These are the symbols that will only exist in this module and not in the main module.
        let mut unique_symbols = split
            .reachable_graph
            .difference(&self.main_graph)
            .cloned()
            .collect::<HashSet<_>>();

        // The functions we'll need to import
        let mut symbols_to_import: HashSet<_> = split
            .reachable_graph
            .intersection(&self.main_graph)
            .cloned()
            .collect();

        // Identify the functions we'll delete
        let symbols_to_delete: HashSet<_> = self
            .main_graph
            .difference(&split.reachable_graph)
            .cloned()
            .collect();

        // Convert split chunk functions to imports
        let mut relies_on_chunks = HashSet::new();
        for (idx, chunk) in self.chunks.iter().enumerate() {
            let nodes_to_extract = unique_symbols
                .intersection(chunk)
                .cloned()
                .collect::<Vec<_>>();
            for node in nodes_to_extract {
                if !self.main_graph.contains(&node) {
                    unique_symbols.remove(&node);
                    symbols_to_import.insert(node);
                    relies_on_chunks.insert(idx);
                }
            }
        }

        tracing::info!(
            "Emitting module {}/{} {}: {:?}",
            split_idx,
            self.split_points.len(),
            split.module_name,
            relies_on_chunks
        );

        let (mut out, ids_to_fns, _fns_to_ids) = parse_module_with_ids(self.bindgened)?;

        // Remap the graph to our module's IDs
        let shared_funcs = self
            .shared_symbols
            .iter()
            .map(|f| self.remap_id(&ids_to_fns, f))
            .collect::<Vec<_>>();

        let unique_symbols = self.remap_ids(&unique_symbols, &ids_to_fns);
        let symbols_to_delete = self.remap_ids(&symbols_to_delete, &ids_to_fns);
        let symbols_to_import = self.remap_ids(&symbols_to_import, &ids_to_fns);
        let split_export_func = ids_to_fns[self.fns_to_ids[&split.export_func]];

        // Do some basic cleanup of the module to make it smaller
        // This removes exports, imports, and the start function
        self.prune_split_module(&mut out);

        // Clear away the data segments
        self.clear_data_segments(&mut out, &unique_symbols);

        // Clear out the element segments and then add in the initializers for the shared imports
        self.create_ifunc_initializers(&mut out, &unique_symbols);

        // Convert our split module's functions to real functions that call the indirect function
        self.add_split_imports(
            &mut out,
            split.index,
            split_export_func,
            split.export_name,
            &symbols_to_import,
            &shared_funcs,
        );

        // Delete all the functions that are not reachable from the main module
        self.delete_main_funcs_from_split(&mut out, &symbols_to_delete);

        // Remove the reloc and linking custom sections
        self.remove_custom_sections(&mut out);

        // Run the gc to remove unused functions - also validates the module to ensure we can emit it properly
        // todo(jon): prefer to delete the items as we go so we don't need to run a gc pass. it/it's quite slow
        walrus::passes::gc::run(&mut out);

        Ok(SplitModule {
            bytes: out.emit_wasm(),
            module_name: split.module_name.clone(),
            component_name: Some(split.component_name.clone()),
            relies_on_chunks,
            hash_id: Some(split.hash_name.clone()),
        })
    }

    /// Write a split chunk - this is a chunk with no special functions, just exports + initializers
    fn emit_split_chunk(&self, idx: usize) -> Result<SplitModule> {
        tracing::info!("emitting chunk {}", idx);

        let unique_symbols = &self.chunks[idx];

        // The functions we'll need to import
        let symbols_to_import: HashSet<_> = unique_symbols
            .intersection(&self.main_graph)
            .cloned()
            .collect();

        // Delete everything except the symbols that are reachable from this module
        let symbols_to_delete: HashSet<_> = self
            .main_graph
            .difference(unique_symbols)
            .cloned()
            .collect();

        // Make sure to remap any ids from the main module to this module
        let (mut out, ids_to_fns, _fns_to_ids) = parse_module_with_ids(self.bindgened)?;

        // Remap the graph to our module's IDs
        let shared_funcs = self
            .shared_symbols
            .iter()
            .map(|f| self.remap_id(&ids_to_fns, f))
            .collect::<Vec<_>>();

        let unique_symbols = self.remap_ids(unique_symbols, &ids_to_fns);
        let symbols_to_import = self.remap_ids(&symbols_to_import, &ids_to_fns);
        let symbols_to_delete = self.remap_ids(&symbols_to_delete, &ids_to_fns);

        self.prune_split_module(&mut out);

        // Clear away the data segments
        self.clear_data_segments(&mut out, &unique_symbols);

        // Clear out the element segments and then add in the initializers for the shared imports
        self.create_ifunc_initializers(&mut out, &unique_symbols);

        // We have to make sure our table matches that of the other tables even though we don't call them.
        let ifunc_table_id = self.load_funcref_table(&mut out);
        let segment_start = self
            .expand_ifunc_table_max(
                &mut out,
                ifunc_table_id,
                self.split_points.len() + shared_funcs.len(),
            )
            .unwrap();

        self.convert_shared_to_imports(&mut out, segment_start, &shared_funcs, &symbols_to_import);

        // Make sure we haven't deleted anything important....
        self.delete_main_funcs_from_split(&mut out, &symbols_to_delete);

        // Remove the reloc and linking custom sections
        self.remove_custom_sections(&mut out);

        // Run the gc to remove unused functions - also validates the module to ensure we can emit it properly
        walrus::passes::gc::run(&mut out);

        Ok(SplitModule {
            bytes: out.emit_wasm(),
            module_name: "split".to_string(),
            component_name: None,
            relies_on_chunks: Default::default(),
            hash_id: None,
        })
    }

    /// Convert functions coming in from outside the module to indirect calls to the ifunc table created in the main module
    fn convert_shared_to_imports(
        &self,
        out: &mut Module,
        segment_start: usize,
        ifuncs: &Vec<Node>,
        symbols_to_import: &HashSet<Node>,
    ) {
        let ifunc_table_id = self.load_funcref_table(out);

        let mut idx = self.split_points.len();
        for node in ifuncs {
            if let Node::Function(ifunc) = node {
                if symbols_to_import.contains(node) {
                    let ty_id = out.funcs.get(*ifunc).ty();
                    let stub = (idx + segment_start) as _;
                    out.funcs.get_mut(*ifunc).kind =
                        self.make_stub_funcs(out, ifunc_table_id, ty_id, stub);
                }

                idx += 1;
            }
        }
    }

    /// Convert split import functions to local functions that call an indirect function that will
    /// be filled in from the loaded split module.
    ///
    /// This is because these imports are going to be delayed until the split module is loaded
    /// and loading in the main module these as imports won't be possible since the imports won't
    /// be resolved until the split module is loaded.
    fn create_ifunc_table(&self, out: &mut Module) {
        let ifunc_table = self.load_funcref_table(out);
        let dummy_func = self.make_dummy_func(out);

        out.exports.add("__indirect_function_table", ifunc_table);

        // Expand the ifunc table to accommodate the new ifuncs
        let segment_start = self
            .expand_ifunc_table_max(
                out,
                ifunc_table,
                self.split_points.len() + self.shared_symbols.len(),
            )
            .expect("failed to expand ifunc table");

        // Delete the split import functions and replace them with local functions
        //
        // Start by pushing all the shared imports into the list
        // These don't require an additional stub function
        let mut ifuncs = vec![];

        // Push the split import functions into the list - after we've pushed in the shared imports
        for idx in 0..self.split_points.len() {
            // this is okay since we're in the main module
            let import_func = self.split_points[idx].import_func;
            let import_id = self.split_points[idx].import_id;
            let ty_id = out.funcs.get(import_func).ty();
            let stub_idx = segment_start + ifuncs.len();

            // Replace the import function with a local function that calls the indirect function
            out.funcs.get_mut(import_func).kind =
                self.make_stub_funcs(out, ifunc_table, ty_id, stub_idx as _);

            // And remove the corresponding import
            out.imports.delete(import_id);

            // Push into the list the properly typed dummy func so the entry is populated
            // unclear if the typing is important here
            ifuncs.push(dummy_func);
        }

        // Add the stub functions to the ifunc table
        // The callers of these functions will call the stub instead of the import
        let mut _idx = 0;
        // Track every export name already in use so the shared-function
        // exports added below stay unique (see the dedup note there).
        let mut used_export_names: HashSet<String> =
            out.exports.iter().map(|e| e.name.clone()).collect();
        // Monotonic counter for the synthetic short names given to the
        // DCE-root exports below. See the note there for why these don't
        // need to carry the function's real (mangled) name.
        let mut shared_export_idx: usize = 0;
        for func in self.shared_symbols.iter() {
            if let Node::Function(id) = func {
                ifuncs.push(*id);
                _idx += 1;

                // PATCHED for idealyst (bug #5): also export each
                // shared function from main, so post-split wasm-opt
                // (run by build-web's `wasm_opt_pkg`) doesn't DCE
                // them. The funcref-table element segment alone
                // isn't a strong enough liveness signal for
                // binaryen's `remove-unused-module-elements` pass —
                // it removes the segment entries AND the function
                // bodies when no main-module call_indirect targets
                // the matching table index. Chunks call the
                // function via call_indirect through the *shared*
                // table, which lives across modules, so binaryen
                // optimizing main in isolation can't see the
                // cross-module liveness.
                //
                // Exporting makes the function externally
                // observable, which binaryen's DCE treats as a
                // root.
                //
                // Symptom this fixes: a chunk's call into main
                // (e.g. a stdlib generic like `Zip::new` shared
                // between main and the lazy chunk) traps with
                // `RuntimeError: unreachable` because main's body
                // was stripped to a single `unreachable`
                // instruction by wasm-opt.
                if out.exports.get_exported_func(*id).is_none() {
                    // These exports are ONLY DCE roots — chunks reach the
                    // function through the shared call_indirect table, not
                    // by name, and wasm-opt keeps an exported function
                    // regardless of its export name. So we emit a short
                    // synthetic name (`s{n}`) rather than the function's
                    // full mangled symbol.
                    //
                    // This matters at scale: idealyst-sized bundles produce
                    // ~2800 such exports, and mangled names made main's
                    // Export section ~300 KB (e.g. a 1 KB `lol_alloc` fn
                    // carried a ~150-byte export name). Short names cut that
                    // to a few KB. Functions that ALREADY have a real export
                    // are skipped by the guard above, so wasm-bindgen's JS
                    // shim still resolves them by their original name.
                    let export_name =
                        next_synthetic_export_name(&mut shared_export_idx, &mut used_export_names);
                    out.exports.add(&export_name, *id);
                }
            }
        }

        // Now add segments to the ifunc table
        out.tables
            .get_mut(ifunc_table)
            .elem_segments
            .insert(out.elements.add(
                ElementKind::Active {
                    table: ifunc_table,
                    offset: ConstExpr::Value(ir::Value::I32(segment_start as _)),
                },
                ElementItems::Functions(ifuncs),
            ));
    }

    /// Re-export the memories, globals, and other items from the main module to the side modules
    fn re_export_items(&self, out: &mut Module) {
        // Re-export memories
        for (idx, memory) in out.memories.iter().enumerate() {
            let name = memory
                .name
                .clone()
                .unwrap_or_else(|| format!("__memory_{}", idx));
            out.exports.add(&name, memory.id());
        }

        // Re-export globals
        for (idx, global) in out.globals.iter().enumerate() {
            let global_name = format!("__global__{idx}");
            out.exports.add(&global_name, global.id());
        }

        // Export any tables
        for (idx, table) in out.tables.iter().enumerate() {
            if table.element_ty != RefType::Funcref {
                let table_name = format!("__imported_table_{}", idx);
                out.exports.add(&table_name, table.id());
            }
        }
    }

    fn prune_main_symbols(&self, out: &mut Module, unused_symbols: &HashSet<Node>) -> Result<()> {
        // Wipe the split point exports
        for split in self.split_points.iter() {
            // it's okay that we're not re-mapping IDs since this is just used by the main module
            out.exports.delete(split.export_id);
        }

        // And then any actual symbols from the callgraph
        for symbol in unused_symbols.iter().cloned() {
            match symbol {
                // PATCHED for idealyst (bug #2 + #4): leave function
                // bodies untouched in main. Two prior approaches
                // failed:
                //
                //   * `funcs.delete(id)` (upstream) — panics walrus
                //     during emit/GC when element segments, exports,
                //     or indirect-call sites still reference the
                //     deleted function. Triggers reliably on
                //     idealyst-scale wasms with heavy trait dispatch.
                //
                //   * Replace body with `unreachable` — keeps walrus
                //     happy, but main's data segments hold function
                //     indices for trait-object vtables. When main
                //     code dispatches through those (call_indirect
                //     against an index into main's own data),
                //     wasm-split's static call-graph analysis misses
                //     the path (it can't trace through data) and
                //     classifies the function as chunk-only.
                //     Stubbing breaks runtime dispatch silently —
                //     symptom was a wgpu Simulator inside a lazy
                //     chunk that loaded, ran on_ready, called
                //     host_web::mount, then trapped on a vtable
                //     method that had been overwritten with
                //     `unreachable` in main.
                //
                // Correctness-preserving alternative: leave the
                // function intact. wasm-opt's DCE pass (runs after
                // wasm-split) removes anything truly unreferenced;
                // functions reachable through vtable data stay.
                // Trade-off is a slightly larger main bundle, but
                // these functions are real reachable code from
                // main's perspective even if main's static call
                // graph doesn't see them.
                Node::Function(_id) => {
                    // intentionally do nothing — see comment above.
                }

                // PATCHED for idealyst: leave "unused" data intact — do
                // NOT zero it. Same reasoning as the function arm above.
                // Main's static call graph over-approximates and can't
                // reliably tell which data is reachable from main (data is
                // reached through pointers embedded in the data section —
                // vtables, &'static tables — and the original→bindgened
                // remap collapses duplicate mangled names that release
                // codegen emits once `--emit-relocs` disables ICF). Zeroing
                // misclassified-live data corrupted main: the release crash
                // was a zeroed CSS string → `WebBackend::insert_rule` fed
                // the browser bad CSS → its `.expect()` panicked while a
                // `backend.borrow_mut()` was held, and with `panic="abort"`
                // every later microtask hit "RefCell already borrowed".
                //
                // Trade-off: chunk-only data stays in main, so the main
                // bundle is larger than it could be. Shrinking it safely
                // needs the POST-`gc` surviving-function set as the seed
                // for an address-based reachability closure (see notes in
                // the size investigation) — the pre-`gc` `main_graph` here
                // is far too broad to drive zeroing.
                Node::DataSymbol(_id) => {
                    // intentionally do nothing — see comment above.
                }
            }
        }

        // EXPERIMENTAL: zero the bytes of chunk-only data symbols in
        // the main module. Gated behind an env var because the
        // analysis uses the pre-GC main graph which is known to
        // over-approximate (see comment block above). When safe it
        // shrinks the gzipped main bundle by megabyte-scale on apps
        // with a heavy lazy chunk (e.g. wgpu sim in main vs lazy).
        //
        // Approach: iterate every "unused" symbol the call graph
        // classified as not-reachable-from-main, and overwrite its
        // bytes with zeros. Segment shape is preserved (offsets +
        // sizes unchanged) so live symbols stay at the same memory
        // addresses; only the dead bytes go to zero, which gzip
        // collapses to ~nothing. If a misclassified symbol gets
        // zeroed, main will crash at runtime — that's the existing
        // risk this gate exposes for experimentation.
        if std::env::var("IDEALYST_WASM_SPLIT_PRUNE_DATA").is_ok() {
            // Optional size threshold so the experiment can start by
            // only pruning OBVIOUSLY-chunk-only symbols (huge tables
            // shipped by wgpu/naga/cosmic-text/icu, which can't
            // possibly be reached from main on a no-GPU SPA page).
            // Small dead symbols are riskier — they're often
            // panic / format strings that the symbol-level call
            // graph occasionally misclassifies as chunk-only.
            let min = std::env::var("IDEALYST_WASM_SPLIT_PRUNE_DATA_MIN")
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(0);
            self.zero_dead_data_in_main(out, unused_symbols, min)?;
        }
        if std::env::var("IDEALYST_WASM_SPLIT_PRUNE_DATA_STATS").is_ok() {
            self.report_dead_data_size_histogram(unused_symbols);
        }

        Ok(())
    }

    /// Histogram of chunk-only data symbol sizes — helps pick a
    /// "only zero big symbols" heuristic if the unconditional zeroing
    /// is too aggressive (over-approximation can misclassify
    /// main-live data, particularly small panic / format strings).
    fn report_dead_data_size_histogram(&self, unused_symbols: &HashSet<Node>) {
        let buckets: &[(usize, usize)] = &[
            (0, 16), (16, 64), (64, 256), (256, 1024),
            (1024, 4096), (4096, 16384), (16384, usize::MAX),
        ];
        let mut counts = vec![(0usize, 0usize); buckets.len()];
        for sym in unused_symbols {
            let Node::DataSymbol(id) = sym else { continue };
            let Some(symbol) = self.data_symbols.get(id) else { continue };
            for (i, (lo, hi)) in buckets.iter().enumerate() {
                if symbol.symbol_size >= *lo && symbol.symbol_size < *hi {
                    counts[i].0 += 1;
                    counts[i].1 += symbol.symbol_size;
                    break;
                }
            }
        }
        eprintln!("[wasm-split prune-data histogram] chunk-only symbol size buckets:");
        for (i, (lo, hi)) in buckets.iter().enumerate() {
            let hi_label = if *hi == usize::MAX { "∞".to_string() } else { hi.to_string() };
            eprintln!(
                "  [{lo:>6}, {hi_label:>6}): {} symbols, {} bytes",
                counts[i].0, counts[i].1,
            );
        }
    }

    /// Walk each main data segment and zero the byte ranges that
    /// correspond to chunk-only data symbols. See the call site
    /// above for the risk profile.
    ///
    /// `min_size` filters out small symbols — a tunable safety knob
    /// during experimentation, since the over-approximation
    /// in the symbol-level call graph misclassifies small
    /// strings (panic / format messages) more often than huge
    /// per-chunk tables.
    fn zero_dead_data_in_main(
        &self,
        out: &mut Module,
        unused_symbols: &HashSet<Node>,
        min_size: usize,
    ) -> Result<()> {
        // Collect dead-symbol ranges, indexed by their data-segment
        // index. The data-symbol parse stores `which_data_segment`
        // already.
        let mut dead_per_segment: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
        let mut dead_bytes_total: usize = 0;
        let mut skipped_small: usize = 0;
        for sym in unused_symbols {
            let Node::DataSymbol(id) = sym else { continue };
            let Some(symbol) = self.data_symbols.get(id) else { continue };
            if symbol.symbol_size < min_size {
                skipped_small += 1;
                continue;
            }
            let range = symbol.segment_offset..symbol.segment_offset + symbol.symbol_size;
            dead_bytes_total += symbol.symbol_size;
            dead_per_segment
                .entry(symbol.which_data_segment)
                .or_default()
                .push((range.start, range.end));
        }
        if skipped_small > 0 {
            eprintln!(
                "[wasm-split prune-data] skipped {skipped_small} chunk-only symbols smaller \
                 than {min_size} bytes (safety threshold)",
            );
        }

        // Iterate main's data segments in declaration order and zero
        // every dead range. Walrus indexes segments via `out.data.iter()`;
        // the parser's `which_data_segment` index matches that order.
        let data_ids: Vec<_> = out.data.iter().map(|d| d.id()).collect();
        let mut zeroed_bytes: usize = 0;
        for (idx, data_id) in data_ids.into_iter().enumerate() {
            let Some(dead_ranges) = dead_per_segment.get(&idx) else {
                continue;
            };
            let data = out.data.get_mut(data_id);
            for (lo, hi) in dead_ranges {
                let lo = *lo;
                let hi = (*hi).min(data.value.len());
                if hi <= lo {
                    continue;
                }
                for b in &mut data.value[lo..hi] {
                    *b = 0;
                }
                zeroed_bytes += hi - lo;
            }
        }

        eprintln!(
            "[wasm-split prune-data] zeroed {zeroed_bytes} of {dead_bytes_total} chunk-only data bytes",
        );
        Ok(())
    }

    // 2.1 Create a dummy func that will be overridden later as modules pop in
    // 2.2 swap the segment entries with the dummy func, leaving hole in its placed that will be filled in later
    fn replace_segments_with_holes(&self, out: &mut Module, unused_symbols: &HashSet<Node>) {
        let dummy_func = self.make_dummy_func(out);
        for element in out.elements.iter_mut() {
            match &mut element.items {
                ElementItems::Functions(vec) => {
                    for item in vec.iter_mut() {
                        if unused_symbols.contains(&Node::Function(*item)) {
                            *item = dummy_func;
                        }
                    }
                }
                ElementItems::Expressions(_ref_type, const_exprs) => {
                    for item in const_exprs.iter_mut() {
                        if let &mut ConstExpr::RefFunc(id) = item {
                            if unused_symbols.contains(&Node::Function(id)) {
                                *item = ConstExpr::RefFunc(dummy_func);
                            }
                        }
                    }
                }
            }
        }
    }

    /// Creates the jump points
    fn create_ifunc_initializers(&self, out: &mut Module, unique_symbols: &HashSet<Node>) {
        let ifunc_table = self.load_funcref_table(out);

        let mut initializers = HashMap::new();
        for segment in out.elements.iter_mut() {
            let ElementKind::Active { offset, .. } = &mut segment.kind else {
                continue;
            };

            let ConstExpr::Value(ir::Value::I32(offset)) = offset else {
                continue;
            };

            match &segment.items {
                ElementItems::Functions(vec) => {
                    for (idx, id) in vec.iter().enumerate() {
                        if unique_symbols.contains(&Node::Function(*id)) {
                            initializers
                                .insert(*offset + idx as i32, ElementItems::Functions(vec![*id]));
                        }
                    }
                }

                ElementItems::Expressions(ref_type, const_exprs) => {
                    for (idx, expr) in const_exprs.iter().enumerate() {
                        if let ConstExpr::RefFunc(id) = expr {
                            if unique_symbols.contains(&Node::Function(*id)) {
                                initializers.insert(
                                    *offset + idx as i32,
                                    ElementItems::Expressions(
                                        *ref_type,
                                        vec![ConstExpr::RefFunc(*id)],
                                    ),
                                );
                            }
                        }
                    }
                }
            }
        }

        // Wipe away references to these segments
        for table in out.tables.iter_mut() {
            table.elem_segments.clear();
        }

        // Wipe away the element segments themselves
        let segments_to_delete: Vec<_> = out.elements.iter().map(|e| e.id()).collect();
        for id in segments_to_delete {
            out.elements.delete(id);
        }

        // Add in our new segments
        let ifunc_table_ = out.tables.get_mut(ifunc_table);
        for (offset, items) in initializers {
            let kind = ElementKind::Active {
                table: ifunc_table,
                offset: ConstExpr::Value(ir::Value::I32(offset)),
            };

            ifunc_table_
                .elem_segments
                .insert(out.elements.add(kind, items));
        }
    }

    fn add_split_imports(
        &self,
        out: &mut Module,
        split_idx: usize,
        split_export_func: FunctionId,
        split_export_name: String,
        symbols_to_import: &HashSet<Node>,
        ifuncs: &Vec<Node>,
    ) {
        let ifunc_table_id = self.load_funcref_table(out);
        let segment_start = self
            .expand_ifunc_table_max(out, ifunc_table_id, self.split_points.len() + ifuncs.len())
            .unwrap();

        // Make sure to re-export the split func
        out.exports.add(&split_export_name, split_export_func);

        // Add the elements back to the table
        out.tables
            .get_mut(ifunc_table_id)
            .elem_segments
            .insert(out.elements.add(
                ElementKind::Active {
                    table: ifunc_table_id,
                    offset: ConstExpr::Value(ir::Value::I32((segment_start + split_idx) as i32)),
                },
                ElementItems::Functions(vec![split_export_func]),
            ));

        self.convert_shared_to_imports(out, segment_start, ifuncs, symbols_to_import);
    }

    fn delete_main_funcs_from_split(&self, out: &mut Module, symbols_to_delete: &HashSet<Node>) {
        for node in symbols_to_delete {
            if let Node::Function(id) = *node {
                // if out.exports.get_exported_func(id).is_none() {
                out.funcs.delete(id);
                // }
            }
        }
    }

    /// Remove un-needed stuff and then hoist
    fn prune_split_module(&self, out: &mut Module) {
        // Clear the module's start/main
        if let Some(start) = out.start.take() {
            if let Some(export) = out.exports.get_exported_func(start) {
                out.exports.delete(export.id());
            }
        }

        // We're going to import the funcref table, so wipe it altogether
        for table in out.tables.iter_mut() {
            table.elem_segments.clear();
        }

        // Wipe all our imports - we're going to use a different set of imports
        let all_imports: HashSet<_> = out.imports.iter().map(|i| i.id()).collect();
        for import_id in all_imports {
            out.imports.delete(import_id);
        }

        // Wipe away memories
        let all_memories: Vec<_> = out.memories.iter().map(|m| m.id()).collect();
        for memory_id in all_memories {
            out.memories.get_mut(memory_id).data_segments.clear();
        }

        // Add exports that call the corresponding import
        let exports = out.exports.iter().map(|e| e.id()).collect::<Vec<_>>();
        for export_id in exports {
            out.exports.delete(export_id);
        }

        // Convert the tables to imports.
        // Should be as simple as adding a new import and then writing the `.import` field
        for (idx, table) in out.tables.iter_mut().enumerate() {
            let name = table.name.clone().unwrap_or_else(|| {
                if table.element_ty == RefType::Funcref {
                    "__indirect_function_table".to_string()
                } else {
                    format!("__imported_table_{}", idx)
                }
            });
            let import = out.imports.add("__wasm_split", &name, table.id());
            table.import = Some(import);
        }

        // Convert the memories to imports
        // Should be as simple as adding a new import and then writing the `.import` field
        for (idx, memory) in out.memories.iter_mut().enumerate() {
            let name = memory
                .name
                .clone()
                .unwrap_or_else(|| format!("__memory_{}", idx));
            let import = out.imports.add("__wasm_split", &name, memory.id());
            memory.import = Some(import);
        }

        // Convert the globals to imports
        // We might not use the global, so if we don't, we can just get
        let global_ids: Vec<_> = out.globals.iter().map(|t| t.id()).collect();
        for (idx, global_id) in global_ids.into_iter().enumerate() {
            let global = out.globals.get_mut(global_id);
            let global_name = format!("__global__{idx}");
            let import = out.imports.add("__wasm_split", &global_name, global.id());
            global.kind = GlobalKind::Import(import);
        }
    }

    fn make_dummy_func(&self, out: &mut Module) -> FunctionId {
        let mut b = FunctionBuilder::new(&mut out.types, &[], &[]);
        b.name("dummy".into()).func_body().unreachable();
        b.finish(vec![], &mut out.funcs)
    }

    fn clear_data_segments(&self, out: &mut Module, unique_symbols: &HashSet<Node>) {
        // Preserve the data symbols for this module and then clear them away
        let data_ids: Vec<_> = out.data.iter().map(|t| t.id()).collect();
        for (idx, data_id) in data_ids.into_iter().enumerate() {
            let data = out.data.get_mut(data_id);

            // Take the data out of the vec - zeroing it out unless we patch it in manually
            let contents = data.value.split_off(0);

            // Zero out the non-primary data segments
            if idx != 0 {
                continue;
            }

            let DataKind::Active { memory, offset } = data.kind else {
                continue;
            };

            let ConstExpr::Value(ir::Value::I32(data_offset)) = offset else {
                continue;
            };

            // And then assign chunks of the data to new data entries that will override the individual slots
            for unique in unique_symbols {
                if let Node::DataSymbol(id) = unique {
                    if let Some(symbol) = self.data_symbols.get(id) {
                        if symbol.which_data_segment == idx {
                            let range =
                                symbol.segment_offset..symbol.segment_offset + symbol.symbol_size;
                            let offset = ConstExpr::Value(ir::Value::I32(
                                data_offset + symbol.segment_offset as i32,
                            ));
                            out.data.add(
                                DataKind::Active { memory, offset },
                                contents[range].to_vec(),
                            );
                        }
                    }
                }
            }
        }
    }

    /// Load the funcref table from the main module. This *should* exist for all modules created by
    /// Rustc or Wasm-Bindgen, but we create it if it doesn't exist.
    fn load_funcref_table(&self, out: &mut Module) -> TableId {
        let ifunc_table = out
            .tables
            .iter()
            .find(|t| t.element_ty == RefType::Funcref)
            .map(|t| t.id());

        if let Some(table) = ifunc_table {
            table
        } else {
            out.tables.add_local(false, 0, None, RefType::Funcref)
        }
    }

    /// Convert the imported function to a local function that calls an indirect function from the table
    ///
    /// This will enable the main module (and split modules) to call functions from outside their own module.
    /// The functions might not exist when the main module is loaded, so we'll register some elements
    /// that fill those in eventually.
    fn make_stub_funcs(
        &self,
        out: &mut Module,
        table: TableId,
        ty_id: TypeId,
        table_idx: i32,
    ) -> FunctionKind {
        // Convert the import function to a local function that calls the indirect function from the table
        let ty = out.types.get(ty_id);

        let params = ty.params().to_vec();
        let results = ty.results().to_vec();
        let args: Vec<_> = params.iter().map(|ty| out.locals.add(*ty)).collect();

        // New function that calls the indirect function
        let mut builder = FunctionBuilder::new(&mut out.types, &params, &results);
        let mut body = builder.name("stub".into()).func_body();

        // Push the params onto the stack
        for arg in args.iter() {
            body.local_get(*arg);
        }

        // And then the address of the indirect function
        body.instr(ir::Instr::Const(ir::Const {
            value: ir::Value::I32(table_idx),
        }));

        // And call it
        body.instr(ir::Instr::CallIndirect(ir::CallIndirect {
            ty: ty_id,
            table,
        }));

        FunctionKind::Local(builder.local_func(args))
    }

    /// Expand the ifunc table to accommodate the new ifuncs
    ///
    /// returns the old maximum
    fn expand_ifunc_table_max(
        &self,
        out: &mut Module,
        table: TableId,
        num_ifuncs: usize,
    ) -> Option<usize> {
        let ifunc_table_ = out.tables.get_mut(table);

        if let Some(max) = ifunc_table_.maximum {
            ifunc_table_.maximum = Some(max + num_ifuncs as u64);
            ifunc_table_.initial += num_ifuncs as u64;
            return Some(max as usize);
        }

        None
    }

    // only keep the target-features and names section so wasm-opt can use it to optimize the output
    fn remove_custom_sections(&self, out: &mut Module) {
        let sections_to_delete = out
            .customs
            .iter()
            .filter_map(|(id, section)| {
                if section.name() == "target_features" {
                    None
                } else {
                    Some(id)
                }
            })
            .collect::<Vec<_>>();

        for id in sections_to_delete {
            out.customs.delete(id);
        }
    }

    /// Accumulate any shared funcs between multiple chunks into a single residual chunk.
    /// This prevents duplicates from being downloaded.
    /// Eventually we need to group the chunks into smarter "communities" - ie the Louvain algorithm
    ///
    /// Todo: we could chunk up the main module itself! Not going to now but it would enable parallel downloads of the main chunk
    fn build_split_chunks(&mut self) {
        // create a single chunk that contains all functions used by multiple modules
        let mut funcs_used_by_chunks: HashMap<Node, HashSet<usize>> = HashMap::new();
        for split in self.split_points.iter() {
            for item in split.reachable_graph.iter() {
                if self.main_graph.contains(item) {
                    continue;
                }
            }
        }

        // Only consider funcs that are used by multiple modules - otherwise they can just stay in their respective module
        funcs_used_by_chunks.retain(|_, v| v.len() > 1);

        // todo: break down this chunk if it exceeds a certain size (100kb?) by identifying different groups

        self.chunks
            .push(funcs_used_by_chunks.keys().cloned().collect());
    }

    fn unused_main_symbols(&self) -> HashSet<Node> {
        self.split_points
            .iter()
            .flat_map(|split| split.reachable_graph.iter())
            .filter(|sym| {
                // Make sure the symbol isn't in the main graph
                if self.main_graph.contains(sym) {
                    return false;
                }

                // And ensure we aren't also exporting it
                match sym {
                    Node::Function(u) => self.source_module.exports.get_exported_func(*u).is_none(),
                    _ => true,
                }
            })
            .cloned()
            .collect()
    }

    /// Accumulate the relocations from the original module, create a relocation map, and then convert
    /// that to our *new* module's symbols.
    fn build_call_graph(&mut self) -> Result<()> {
        let original = ModuleWithRelocations::new(self.original)?;

        let old_names: HashMap<String, FunctionId> = original
            .module
            .funcs
            .iter()
            .flat_map(|f| Some((f.name.clone()?, f.id())))
            .collect();

        let new_names: HashMap<String, FunctionId> = self
            .source_module
            .funcs
            .iter()
            .flat_map(|f| Some((f.name.clone()?, f.id())))
            .collect();

        let mut old_to_new = HashMap::new();
        let mut new_call_graph: HashMap<Node, HashSet<Node>> = HashMap::new();

        for (new_name, new_func) in new_names.iter() {
            if let Some(old_func) = old_names.get(new_name) {
                old_to_new.insert(*old_func, new_func);
            } else {
                new_call_graph.insert(Node::Function(*new_func), HashSet::new());
            }
        }

        let get_old = |old: &Node| -> Option<Node> {
            match old {
                Node::Function(id) => old_to_new.get(id).map(|new_id| Node::Function(**new_id)),
                Node::DataSymbol(id) => Some(Node::DataSymbol(*id)),
            }
        };

        // the symbols that we can't find in the original module touch functions that unfortunately
        // we can't figure out where should exist in the call graph
        //
        // we're going to walk and find every child we possibly can and then add it to the call graph
        // at the root
        //
        // wasm-bindgen will dissolve describe functions into the shim functions, but we don't have a
        // sense of lining up old to new, so we just assume everything ends up in the main chunk.
        let mut lost_children = HashSet::new();
        self.call_graph = original
            .call_graph
            .iter()
            .flat_map(|(old, children)| {
                // If the old function isn't in the new module, we need to move all its descendents into the main chunk
                let Some(new) = get_old(old) else {
                    for child in children {
                        fn descend(
                            lost_children: &mut HashSet<Node>,
                            old_graph: &HashMap<Node, HashSet<Node>>,
                            node: Node,
                        ) {
                            if !lost_children.insert(node) {
                                return;
                            }

                            if let Some(children) = old_graph.get(&node) {
                                for child in children {
                                    descend(lost_children, old_graph, *child);
                                }
                            }
                        }

                        descend(&mut lost_children, &original.call_graph, *child);
                    }
                    return None;
                };

                let mut new_children = HashSet::new();
                for child in children {
                    if let Some(new) = get_old(child) {
                        new_children.insert(new);
                    }
                }

                Some((new, new_children))
            })
            .collect();

        let mut recovered_children = HashSet::new();
        for lost in lost_children {
            match lost {
                // Functions need to be found - the wasm describe functions are usually completely dissolved
                Node::Function(id) => {
                    let func = original.module.funcs.get(id);
                    let name = func.name.as_ref().unwrap();
                    if let Some(entry) = new_names.get(name) {
                        recovered_children.insert(Node::Function(*entry));
                    }
                }

                // Data symbols are unchanged and fine to remap
                Node::DataSymbol(id) => {
                    recovered_children.insert(Node::DataSymbol(id));
                }
            }
        }

        // We're going to attach the recovered children to the main function
        let main_fn = self.source_module.funcs.by_name("main").context("Failed to find `main` function - was this built with LTO, --emit-relocs, and debug symbols?")?;
        let main_fn_entry = new_call_graph.entry(Node::Function(main_fn)).or_default();
        main_fn_entry.extend(recovered_children);

        // Also attach any truly new symbols to the main function. Usually these are the shim functions
        for (name, new) in new_names.iter() {
            if !old_names.contains_key(name) {
                main_fn_entry.insert(Node::Function(*new));
            }
        }

        // Walk the functions and try to disconnect any holes manually
        // This will attempt to resolve any of the new symbols like the shim functions
        for func in self.source_module.funcs.iter() {
            struct CallGrapher<'a> {
                cur: FunctionId,
                call_graph: &'a mut HashMap<Node, HashSet<Node>>,
            }
            impl<'a> Visitor<'a> for CallGrapher<'a> {
                fn visit_function_id(&mut self, function: &walrus::FunctionId) {
                    self.call_graph
                        .entry(Node::Function(self.cur))
                        .or_default()
                        .insert(Node::Function(*function));
                }
            }
            if let FunctionKind::Local(local) = &func.kind {
                let mut call_grapher = CallGrapher {
                    cur: func.id(),
                    call_graph: &mut self.call_graph,
                };
                dfs_in_order(&mut call_grapher, local, local.entry_block());
            }
        }

        // Fill in the parent graph
        for (parnet, children) in self.call_graph.iter() {
            for child in children {
                self.parent_graph.entry(*child).or_default().insert(*parnet);
            }
        }

        // Now go fill in the reachability graph for each of the split points
        // We want to be as narrow as possible since we've reparented any new symbols to the main module
        self.split_points.iter_mut().for_each(|split| {
            let roots: HashSet<_> = [Node::Function(split.export_func)].into();
            split.reachable_graph = reachable_graph(&self.call_graph, &roots);
        });

        // And then the reachability graph for main
        self.main_graph = reachable_graph(&self.call_graph, &self.main_roots());

        // And then the symbols shared between all
        self.shared_symbols = {
            let mut shared_funcs = HashSet::new();

            // Add all the symbols shared between the various modules
            for split in self.split_points.iter() {
                shared_funcs.extend(self.main_graph.intersection(&split.reachable_graph));
            }

            // And then all our imports will be callabale via the ifunc table too
            for import in self.source_module.imports.iter() {
                if let ImportKind::Function(id) = import.kind {
                    shared_funcs.insert(Node::Function(id));
                }
            }

            // Make sure to make this *ordered*
            shared_funcs.into_iter().collect()
        };

        Ok(())
    }

    fn main_roots(&self) -> HashSet<Node> {
        // Accumulate all the split entrypoints
        // This will include wasm_bindgen functions too
        let exported_splits = self
            .split_points
            .iter()
            .map(|f| f.export_func)
            .collect::<HashSet<_>>();

        // And only return the functions that are reachable from the main module's start function
        let mut roots = self
            .source_module
            .exports
            .iter()
            .filter_map(|e| match e.item {
                ExportItem::Function(id) if !exported_splits.contains(&id) => {
                    Some(Node::Function(id))
                }
                _ => None,
            })
            .chain(self.source_module.start.map(Node::Function))
            .collect::<HashSet<Node>>();

        // Also add "imports" to the roots
        for import in self.source_module.imports.iter() {
            if let ImportKind::Function(id) = import.kind {
                roots.insert(Node::Function(id));
            }
        }

        roots
    }

    /// Convert this set of nodes to reference the new module
    fn remap_ids(&self, set: &HashSet<Node>, ids_to_fns: &[FunctionId]) -> HashSet<Node> {
        let mut out = HashSet::with_capacity(set.len());
        for node in set {
            out.insert(self.remap_id(ids_to_fns, node));
        }
        out
    }

    fn remap_id(&self, ids_to_fns: &[id_arena::Id<walrus::Function>], node: &Node) -> Node {
        match node {
            // Remap the function IDs
            Node::Function(id) => Node::Function(ids_to_fns[self.fns_to_ids[id]]),
            // data symbols don't need remapping
            Node::DataSymbol(id) => Node::DataSymbol(*id),
        }
    }
}

/// Pick the next unused short synthetic export name (`s0`, `s1`, …) for a
/// shared-function DCE-root export, advancing `next_idx` past any name
/// already in `used` and recording the choice in `used`.
///
/// These exports exist only to keep main's copy of a function alive for
/// wasm-opt (chunks call it through the shared call_indirect table, not by
/// name), so the name is arbitrary — short names keep main's Export section
/// from ballooning. Wasm requires export names be unique within a module;
/// `s{n}` could collide with a pre-existing export (or, under opt-level=z,
/// LLVM emits distinct functions sharing one mangled name), so we bump until
/// free.
fn next_synthetic_export_name(next_idx: &mut usize, used: &mut HashSet<String>) -> String {
    loop {
        let name = format!("s{}", *next_idx);
        *next_idx += 1;
        if used.insert(name.clone()) {
            return name;
        }
    }
}

/// Parse a module and return the mapping of index to FunctionID.
/// We'll use this mapping to remap ModuleIDs
fn parse_module_with_ids(
    bindgened: &[u8],
) -> Result<(Module, Vec<FunctionId>, HashMap<FunctionId, usize>)> {
    let ids = Arc::new(RwLock::new(Vec::new()));
    let ids_ = ids.clone();
    let module = Module::from_buffer_with_config(
        bindgened,
        ModuleConfig::new().on_parse(move |_m, our_ids| {
            let mut ids = ids_.write().expect("No shared writers");
            let mut idx = 0;
            while let Ok(entry) = our_ids.get_func(idx) {
                ids.push(entry);
                idx += 1;
            }

            Ok(())
        }),
    )?;
    let mut ids_ = ids.write().expect("No shared writers");
    let mut ids = vec![];
    std::mem::swap(&mut ids, &mut *ids_);

    let mut fns_to_ids = HashMap::new();
    for (idx, id) in ids.iter().enumerate() {
        fns_to_ids.insert(*id, idx);
    }

    Ok((module, ids, fns_to_ids))
}

struct ModuleWithRelocations<'a> {
    module: Module,
    symbols: Vec<SymbolInfo<'a>>,
    names_to_funcs: HashMap<String, FunctionId>,
    call_graph: HashMap<Node, HashSet<Node>>,
    parents: HashMap<Node, HashSet<Node>>,
    relocation_map: HashMap<Node, Vec<RelocationEntry>>,
    data_symbols: BTreeMap<usize, DataSymbol>,
    data_section_range: Range<usize>,
}

impl<'a> ModuleWithRelocations<'a> {
    fn new(bytes: &'a [u8]) -> Result<Self> {
        let module = Module::from_buffer(bytes)?;
        let raw_data = parse_bytes_to_data_segment(bytes)?;
        let names_to_funcs = module
            .funcs
            .iter()
            .flat_map(|f| Some((f.name.clone()?, f.id())))
            .collect();

        let mut module = Self {
            module,
            data_symbols: raw_data.data_symbols,
            data_section_range: raw_data.data_range,
            symbols: raw_data.symbols,
            names_to_funcs,
            call_graph: Default::default(),
            relocation_map: Default::default(),
            parents: Default::default(),
        };

        module.build_code_call_graph()?;
        module.build_data_call_graph()?;

        for (func, children) in module.call_graph.iter() {
            for child in children {
                module.parents.entry(*child).or_default().insert(*func);
            }
        }

        Ok(module)
    }

    fn build_code_call_graph(&mut self) -> Result<()> {
        let codes_relocations = self.collect_relocations_from_section("reloc.CODE")?;
        let mut relocations = codes_relocations.iter().peekable();

        for (func_id, local) in self.module.funcs.iter_local() {
            let range = local
                .original_range
                .clone()
                .context("local function has no range")?;

            // Walk with relocation
            while let Some(entry) =
                relocations.next_if(|entry| entry.relocation_range().start < range.end)
            {
                let reloc_range = entry.relocation_range();
                assert!(reloc_range.start >= range.start);
                assert!(reloc_range.end <= range.end);

                if let Some(target) = self.get_symbol_dep_node(entry.index as usize)? {
                    let us = Node::Function(func_id);
                    self.call_graph.entry(us).or_default().insert(target);
                    self.relocation_map.entry(us).or_default().push(*entry);
                }
            }
        }

        assert!(relocations.next().is_none());

        Ok(())
    }

    fn build_data_call_graph(&mut self) -> Result<()> {
        let data_relocations = self.collect_relocations_from_section("reloc.DATA")?;
        let mut relocations = data_relocations.iter().peekable();

        let symbols_sorted = self
            .data_symbols
            .values()
            .sorted_by(|a, b| a.range.start.cmp(&b.range.start));

        for symbol in symbols_sorted {
            let start = symbol.range.start - self.data_section_range.start;
            let end = symbol.range.end - self.data_section_range.start;
            let range = start..end;

            while let Some(entry) =
                relocations.next_if(|entry| entry.relocation_range().start < range.end)
            {
                let reloc_range = entry.relocation_range();
                assert!(reloc_range.start >= range.start);
                assert!(reloc_range.end <= range.end);

                if let Some(target) = self.get_symbol_dep_node(entry.index as usize)? {
                    let dep = Node::DataSymbol(symbol.index);
                    self.call_graph.entry(dep).or_default().insert(target);
                    self.relocation_map.entry(dep).or_default().push(*entry);
                }
            }
        }

        assert!(relocations.next().is_none());

        Ok(())
    }

    /// Accumulate all relocations from a section.
    ///
    /// Parses the section using the RelocSectionReader and returns a vector of relocation entries.
    fn collect_relocations_from_section(&self, name: &str) -> Result<Vec<RelocationEntry>> {
        let (_reloc_id, code_reloc) = self
            .module
            .customs
            .iter()
            .find(|(_, c)| c.name() == name)
            .context("Module does not contain the reloc section")?;

        let code_reloc_data = code_reloc.data(&Default::default());
        let reader = BinaryReader::new(&code_reloc_data, 0);
        let relocations = RelocSectionReader::new(reader)
            .context("failed to parse reloc section")?
            .entries()
            .into_iter()
            .flatten()
            .collect();

        Ok(relocations)
    }

    /// Get the symbol's corresponding entry in the call graph
    ///
    /// This might panic if the source module isn't built properly. Make sure to enable LTO and `--emit-relocs`
    /// when building the source module.
    fn get_symbol_dep_node(&self, index: usize) -> Result<Option<Node>> {
        let res = match self.symbols[index] {
            SymbolInfo::Data { .. } => Some(Node::DataSymbol(index)),
            SymbolInfo::Func { name, .. } => Some(Node::Function({
                let name = name.context(
                    "Function symbol has no name - did you forget to enable debug symbols",
                )?;

                let func_id = self.names_to_funcs.get(name);

                // wbindgen will synthesize some functions that don't exist in the original module (eg describe functions)
                // Previously this was a hard error, but now we just ignore it. It used to mean that the user
                let Some(res) = func_id else {
                    if !name.contains("__wbindgen_") {
                        tracing::error!(
                            "Could not find function symbol {name:?} in module - was this built with LTO, --emit-relocs, and debug symbols? Ignoring."
                        );
                    }
                    return Ok(None);
                };

                *res
            })),

            _ => None,
        };

        Ok(res)
    }
}

#[derive(Debug, Clone)]
pub struct SplitPoint {
    module_name: String,
    import_id: ImportId,
    export_id: ExportId,
    import_func: FunctionId,
    export_func: FunctionId,
    component_name: String,
    index: usize,
    reachable_graph: HashSet<Node>,
    hash_name: String,

    #[allow(unused)]
    import_name: String,

    #[allow(unused)]
    export_name: String,
}

/// Search the module's imports and exports for functions marked as split points.
///
/// These will be in the form of:
///
/// `__wasm_split_00<module>00_<import|export>_<hash>_<function>`
///
/// For a function named `SomeRoute2` in the module `add_body_element`, the pairings would be:
///
/// `__wasm_split_00add_body_element00_import_abef5ee3ebe66ff17677c56ee392b4c2_SomeRoute2`
/// `__wasm_split_00add_body_element00_export_abef5ee3ebe66ff17677c56ee392b4c2_SomeRoute2`
///
fn accumulate_split_points(module: &Module) -> Vec<SplitPoint> {
    let mut index = 0;

    module
        .imports
        .iter()
        .sorted_by(|a, b| a.name.cmp(&b.name))
        .flat_map(|import| {
            if !import.name.starts_with("__wasm_split_00") {
                return None;
            }

            let ImportKind::Function(import_func) = import.kind else {
                return None;
            };

            // Parse the import name to get the module name, the hash, and the function name
            let remain = import.name.trim_start_matches("__wasm_split_00___");
            let (module_name, rest) = remain.split_once("___00").unwrap();
            let (hash, fn_name) = rest.trim_start_matches("_import_").split_once("_").unwrap();

            // Look for the export with the same name
            let export_name =
                format!("__wasm_split_00___{module_name}___00_export_{hash}_{fn_name}");
            let export_func = module
                .exports
                .get_func(&export_name)
                .expect("Could not find export");
            let export = module.exports.get_exported_func(export_func).unwrap();

            let our_index = index;
            index += 1;

            Some(SplitPoint {
                export_id: export.id(),
                import_id: import.id(),
                module_name: module_name.to_string(),
                import_name: import.name.clone(),
                import_func,
                export_func,
                export_name,
                hash_name: hash.to_string(),
                component_name: fn_name.to_string(),
                index: our_index,
                reachable_graph: Default::default(),
            })
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq, Hash, Copy, PartialOrd, Ord, Clone)]
pub enum Node {
    Function(FunctionId),
    DataSymbol(usize),
}

fn reachable_graph(deps: &HashMap<Node, HashSet<Node>>, roots: &HashSet<Node>) -> HashSet<Node> {
    let mut queue: VecDeque<Node> = roots.iter().copied().collect();
    let mut reachable = HashSet::<Node>::new();
    let mut parents = HashMap::<Node, Node>::new();

    while let Some(node) = queue.pop_front() {
        reachable.insert(node);
        let Some(children) = deps.get(&node) else {
            continue;
        };
        for child in children {
            if reachable.contains(child) {
                continue;
            }
            parents.entry(*child).or_insert(node);
            queue.push_back(*child);
        }
    }

    reachable
}

struct RawDataSection<'a> {
    data_range: Range<usize>,
    symbols: Vec<SymbolInfo<'a>>,
    data_symbols: BTreeMap<usize, DataSymbol>,
}

#[derive(Debug)]
struct DataSymbol {
    index: usize,
    range: Range<usize>,
    segment_offset: usize,
    symbol_size: usize,
    which_data_segment: usize,
}

/// Manually parse the data section from a wasm module
///
/// We need to do this for data symbols because walrus doesn't provide the right range and offset
/// information for data segments. Fortunately, it provides it for code sections, so we only need to
/// do a small amount extra of parsing here.
fn parse_bytes_to_data_segment(bytes: &[u8]) -> Result<RawDataSection<'_>> {
    let parser = wasmparser::Parser::new(0);
    let mut parser = parser.parse_all(bytes);
    let mut segments = vec![];
    let mut data_range = 0..0;
    let mut symbols = vec![];

    // Process the payloads in the raw wasm file so we can extract the specific sections we need
    while let Some(Ok(payload)) = parser.next() {
        match payload {
            Payload::DataSection(section) => {
                data_range = section.range();
                segments = section.into_iter().collect::<Result<Vec<_>, _>>()?
            }
            Payload::CustomSection(section) if section.name() == "linking" => {
                let reader = BinaryReader::new(section.data(), 0);
                let reader = LinkingSectionReader::new(reader)?;
                for subsection in reader.subsections() {
                    if let Linking::SymbolTable(map) = subsection? {
                        symbols = map.into_iter().collect::<Result<Vec<_>, _>>()?;
                    }
                }
            }
            _ => {}
        }
    }

    // Accumulate the data symbols into a btreemap for later use
    let mut data_symbols = BTreeMap::new();
    for (index, symbol) in symbols.iter().enumerate() {
        let SymbolInfo::Data {
            symbol: Some(symbol),
            ..
        } = symbol
        else {
            continue;
        };

        if symbol.size == 0 {
            continue;
        }

        let data_segment = segments
            .get(symbol.index as usize)
            .context("Failed to find data segment")?;
        let offset: usize =
            data_segment.range.end - data_segment.data.len() + (symbol.offset as usize);
        let range = offset..(offset + symbol.size as usize);

        data_symbols.insert(
            index,
            DataSymbol {
                index,
                range,
                segment_offset: symbol.offset as usize,
                symbol_size: symbol.size as usize,
                which_data_segment: symbol.index as usize,
            },
        );
    }

    Ok(RawDataSection {
        data_range,
        symbols,
        data_symbols,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Regression: the shared-function DCE-root exports must use short
    // synthetic `s{n}` names, NOT the function's full mangled symbol.
    // Mangled names ballooned main's Export section to ~300 KB on the
    // website (~2800 exports). The names are arbitrary (chunks reach
    // these funcs via the shared call_indirect table, not by name), so
    // the only requirements are: short, and unique within the module
    // (must not collide with a pre-existing real export, and distinct
    // funcs sharing one mangled name must still each get a unique name).
    #[test]
    fn synthetic_export_names_are_short_and_dodge_collisions() {
        // Pre-existing real exports the wasm-bindgen shim depends on,
        // plus an adversarial "s1" that our scheme would otherwise reuse.
        let mut used: HashSet<String> = ["main", "memory", "__wbindgen_malloc", "s1"]
            .into_iter()
            .map(String::from)
            .collect();
        let mut idx = 0usize;

        let a = next_synthetic_export_name(&mut idx, &mut used);
        let b = next_synthetic_export_name(&mut idx, &mut used);
        let c = next_synthetic_export_name(&mut idx, &mut used);

        // Sequential, but skips the pre-existing "s1".
        assert_eq!(a, "s0");
        assert_eq!(b, "s2");
        assert_eq!(c, "s3");

        // All names are short (the whole point of the change).
        for n in [&a, &b, &c] {
            assert!(n.len() <= 4, "synthetic export name {n:?} is not short");
        }

        // Pre-existing real exports are left intact for the JS shim.
        for real in ["main", "memory", "__wbindgen_malloc"] {
            assert!(used.contains(real), "clobbered real export {real:?}");
        }
    }

    // A clash-heavy run (every synthetic slot pre-taken up to a point)
    // must still terminate with a unique name and never reuse one.
    #[test]
    fn synthetic_export_names_never_duplicate() {
        let mut used: HashSet<String> = (0..50).map(|i| format!("s{i}")).collect::<HashSet<_>>();
        let mut idx = 0usize;
        let mut produced = Vec::new();
        for _ in 0..10 {
            let n = next_synthetic_export_name(&mut idx, &mut used);
            assert!(!produced.contains(&n), "duplicate synthetic export {n:?}");
            produced.push(n);
        }
        // First free slot after s0..s49 is s50.
        assert_eq!(produced[0], "s50");
    }

    // ---- Regression tests for `neutralize_command_export_wrappers` ----
    //
    // The bug surfaces only end-to-end (browser, real wasm-bindgen
    // JS shim, an inventory linked list to corrupt), so we can't
    // exercise it cheaply here. The closest reachable check: build a
    // walrus module that mirrors the wasm-bindgen-0.2.122 wrapper
    // shape (a bare helper, a `*.command_export` wrapper that calls
    // `__wasm_call_ctors` then forwards, and a suffixed export
    // pointing at the wrapper), then run the patch and assert the
    // export now points at the bare helper instead.

    use walrus::ValType;

    /// Construct a minimal module shaped like a wasm-bindgen 0.2.122
    /// wrapper for `__wbindgen_malloc`. Returns the emitted bytes.
    fn build_wrapper_fixture(export_name: &str) -> Vec<u8> {
        let mut module = Module::with_config(ModuleConfig::new());

        // `__wasm_call_ctors`: () -> () — empty body. The wrapper calls
        // this; that call is what re-runs every module ctor and
        // corrupts inventory on every JS↔wasm round trip.
        let mut ctors_builder = FunctionBuilder::new(&mut module.types, &[], &[]);
        ctors_builder.func_body();
        let ctors_fid = ctors_builder.finish(vec![], &mut module.funcs);
        module.funcs.get_mut(ctors_fid).name = Some("__wasm_call_ctors".to_string());

        // Bare `__wbindgen_malloc`: (i32, i32) -> i32 — returns 0.
        let mut bare_builder = FunctionBuilder::new(
            &mut module.types,
            &[ValType::I32, ValType::I32],
            &[ValType::I32],
        );
        bare_builder.func_body().i32_const(0);
        let bare_arg0 = module.locals.add(ValType::I32);
        let bare_arg1 = module.locals.add(ValType::I32);
        let bare_fid =
            bare_builder.finish(vec![bare_arg0, bare_arg1], &mut module.funcs);
        module.funcs.get_mut(bare_fid).name = Some("__wbindgen_malloc".to_string());

        // Wrapper `__wbindgen_malloc.command_export`: calls ctors
        // first, then forwards args to the bare helper.
        let mut wrapper_builder = FunctionBuilder::new(
            &mut module.types,
            &[ValType::I32, ValType::I32],
            &[ValType::I32],
        );
        let wrap_arg0 = module.locals.add(ValType::I32);
        let wrap_arg1 = module.locals.add(ValType::I32);
        wrapper_builder
            .func_body()
            .call(ctors_fid)
            .local_get(wrap_arg0)
            .local_get(wrap_arg1)
            .call(bare_fid);
        let wrapper_fid =
            wrapper_builder.finish(vec![wrap_arg0, wrap_arg1], &mut module.funcs);
        module.funcs.get_mut(wrapper_fid).name =
            Some("__wbindgen_malloc.command_export".to_string());

        // Export the wrapper under the caller-chosen name.
        module.exports.add(export_name, wrapper_fid);

        module.emit_wasm()
    }

    /// Read back the function `name` that a given export resolves to.
    fn export_target_name(bytes: &[u8], export_name: &str) -> Option<String> {
        let module = Module::from_buffer(bytes).expect("re-parse wasm");
        let export = module.exports.iter().find(|e| e.name == export_name)?;
        let ExportItem::Function(fid) = export.item else {
            return None;
        };
        module.funcs.get(fid).name.clone()
    }

    /// Pre-fix, the suffixed export points at the wrapper (which would
    /// re-run `__wasm_call_ctors` on every call — the bug). Post-fix,
    /// the export points at the bare helper directly. This pair of
    /// assertions IS the regression check — without the patch, the
    /// post-fix assertion fails.
    #[test]
    fn neutralize_command_export_remaps_suffixed_export_to_bare() {
        let pre = build_wrapper_fixture("__wbindgen_malloc_command_export");

        // Pre-fix: the export points at the ctor-calling wrapper.
        // This is the *buggy* state — we assert it explicitly to prove
        // the fixture genuinely reproduces the wasm-bindgen 0.2.122
        // shape (otherwise the post-fix assertion would be vacuous).
        assert_eq!(
            export_target_name(&pre, "__wbindgen_malloc_command_export"),
            Some("__wbindgen_malloc.command_export".to_string()),
            "fixture mis-set: the suffixed export must initially point \
             at the ctor-calling wrapper, otherwise the post-fix check \
             below is vacuous"
        );

        // Post-fix: the export points at the bare helper.
        let post = neutralize_command_export_wrappers(&pre)
            .expect("neutralize_command_export_wrappers");
        assert_eq!(
            export_target_name(&post, "__wbindgen_malloc_command_export"),
            Some("__wbindgen_malloc".to_string()),
            "regression: the suffixed export must be remapped to the bare \
             helper after the pass — otherwise JS↔wasm round trips will \
             re-run `__wasm_call_ctors` and corrupt inventory state"
        );
    }

    /// The `main` and `host_reserve` wrappers are exported under their
    /// bare names (no `_command_export` suffix) and legitimately need
    /// the ctor call on first invocation. The pass must NOT touch them.
    #[test]
    fn neutralize_command_export_leaves_unsuffixed_exports_alone() {
        // Same wrapper-shape, but the export is `main` (no suffix).
        let bytes = build_wrapper_fixture("main");

        let patched = neutralize_command_export_wrappers(&bytes)
            .expect("neutralize_command_export_wrappers");

        // The export must still point at the wrapper. Unsuffixed exports
        // are the legitimate one-time-init entry points (`main`,
        // `host_reserve`) — touching them would skip the ctors.
        assert_eq!(
            export_target_name(&patched, "main"),
            Some("__wbindgen_malloc.command_export".to_string()),
            "the pass must leave unsuffixed exports alone (they're the \
             legitimate one-time-init entry points like `main`)"
        );
    }

    /// Count `call <fn-with-name>` instructions in a function's body.
    /// Walks the entry InstrSeq's instrs (the wasm-bindgen wrapper body
    /// has no nested blocks, so a single walk is enough).
    fn count_calls_to(bytes: &[u8], fn_name_with_dots: &str, target_name: &str) -> usize {
        let module = Module::from_buffer(bytes).expect("re-parse wasm");
        let fid = module
            .funcs
            .iter()
            .find(|f| f.name.as_deref() == Some(fn_name_with_dots))
            .map(|f| f.id())
            .unwrap_or_else(|| panic!("function {fn_name_with_dots} not in re-parsed wasm"));
        let func = module.funcs.get(fid);
        let FunctionKind::Local(lf) = &func.kind else {
            panic!("expected Local function for {fn_name_with_dots}");
        };
        let entry = lf.entry_block();
        let block = lf.block(entry);
        block
            .instrs
            .iter()
            .filter(|(instr, _)| match instr {
                ir::Instr::Call(call) => {
                    module.funcs.get(call.func).name.as_deref() == Some(target_name)
                }
                _ => false,
            })
            .count()
    }

    /// REGRESSION: Pass A must strip the `call __wasm_call_ctors` from
    /// every helper-wrapper body. Without it, the wasm-bindgen externref
    /// closure shim still reaches the wrapper through internal
    /// call-references (not the export table), and ctors re-run on every
    /// closure invoke → inventory linked-list corruption → OOB.
    ///
    /// Pre-fix the wrapper body has exactly one `call __wasm_call_ctors`.
    /// Post-fix it must have zero. The bare-name-exported wrappers (the
    /// `main`-and-`host_reserve` fixture below) must KEEP their call —
    /// they're the legitimate one-time-init entry points.
    #[test]
    fn neutralize_strips_ctor_call_from_suffixed_wrapper_body() {
        let pre = build_wrapper_fixture("__wbindgen_malloc_command_export");
        assert_eq!(
            count_calls_to(&pre, "__wbindgen_malloc.command_export", "__wasm_call_ctors"),
            1,
            "fixture mis-set: wrapper body must initially have one `call __wasm_call_ctors`"
        );

        let post = neutralize_command_export_wrappers(&pre)
            .expect("neutralize_command_export_wrappers");

        // The wrapper function should still exist (Pass A strips the
        // body's first instr; it does NOT remove the function).
        assert_eq!(
            count_calls_to(&post, "__wbindgen_malloc.command_export", "__wasm_call_ctors"),
            0,
            "Pass A regression: the `call __wasm_call_ctors` must be stripped \
             from suffixed-wrapper bodies. If this fails, the wasm-bindgen \
             externref closure shim will keep re-running ctors on every \
             closure invoke and corrupt inventory state."
        );
    }

    #[test]
    fn neutralize_keeps_ctor_call_in_main_wrapper_body() {
        // The `main` wrapper IS exported as `main` (no `_command_export`
        // suffix) — Pass A must leave it alone so __wbindgen_start →
        // main.command_export → __wasm_call_ctors runs once at init.
        let pre = build_wrapper_fixture("main");
        let post = neutralize_command_export_wrappers(&pre)
            .expect("neutralize_command_export_wrappers");

        assert_eq!(
            count_calls_to(&post, "__wbindgen_malloc.command_export", "__wasm_call_ctors"),
            1,
            "Pass A must leave the bare-name-exported wrapper alone — \
             it's the legitimate one-time-init entry point"
        );
    }
}
