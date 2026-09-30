//! Split a linked, wasm-bindgen'd module at its `#[wasm_split]` boundaries
//! without building an instruction IR.
//!
//! It replaced Dioxus's `wasm-split-cli`, which parsed the whole program
//! into walrus IR — ~60 bytes per byte of code — once for its analysis and
//! once more per output (3.1 GB and 9.5 s on CrewForge, against 0.34 GB and
//! 1.5 s here). This crate computes the same partition from streaming
//! parses (relocations of the rustc module, `call` operands of the
//! bindgened one) and assembles every output from byte ranges of the
//! bindgened module, keeping its index spaces so kept bodies never need
//! rewriting. See `emit` for the output shapes.

pub mod append;
pub mod data;
pub mod emit;
pub mod graph;
pub mod liveness;
pub mod module;
pub mod neutralize;
pub mod strand;

/// The wasmparser this crate's types are built on ([`module::Import::ty`]
/// is its `TypeRef`), for callers on a different version.
pub use wasmparser;

use std::collections::HashSet;

use anyhow::Result;

use crate::{
    emit::{Layout, SplitBody, emit_main, emit_split},
    graph::{Node, Partition},
    liveness::{AddrIndex, MainPlan, assign_restores, plan_main},
    module::ModuleIndex,
};

/// One emitted module.
#[derive(Debug, Clone)]
pub struct SplitModule {
    pub module_name: String,
    pub hash_id: Option<String>,
    pub component_name: Option<String>,
    pub bytes: Vec<u8>,
    pub relies_on_chunks: HashSet<usize>,
}

#[derive(Debug)]
pub struct OutputModules {
    pub main: SplitModule,
    pub modules: Vec<SplitModule>,
    pub chunks: Vec<SplitModule>,
}

/// The `makeLoad` factory the generated `__wasm_split.js` is built on:
/// fetches a split module (after the chunks it relies on), instantiates it
/// against main's exports, and wakes the Rust future through main's table.
pub const MAKE_LOAD_JS: &str = include_str!("./__wasm_split.js");

#[derive(Debug, Clone, Default)]
pub struct SplitOptions {
    /// Zero main's copy of every data symbol at least this large that
    /// main's code cannot reach (`--data-prune`); the split outputs whose
    /// code reads it put it back. Smaller symbols are kept because each
    /// restored one costs a segment header. `None` keeps main's data whole.
    pub prune_dead_data_min: Option<usize>,
}

pub fn split(original: &[u8], bindgened: &[u8], options: &SplitOptions) -> Result<OutputModules> {
    let source = ModuleIndex::parse(bindgened)?;
    let partition = Partition::compute(original, &source)?;
    let layout = Layout::new(&source, &partition)?;
    let addrs = AddrIndex::new(&source, &partition);
    let plan = plan_main(&source, &partition, &layout, &addrs, options.prune_dead_data_min)?;
    report(&plan, options);

    let main = emit_main(&source, &partition, &layout, &plan)?;

    // Per split module: its own bodies (everything it reaches outside
    // main, taken before chunk extraction — an extracted chunk function
    // that is not in `shared_symbols` keeps its own copy — a duplication
    // inherited from the walrus splitter), and the functions it installs
    // into the table (after extraction).
    let plans: Vec<(HashSet<Node>, HashSet<Node>, HashSet<usize>)> = partition
        .split_points
        .iter()
        .map(|split| {
            let bodies: HashSet<Node> =
                split.reachable.difference(&partition.main_graph).copied().collect();
            let mut unique = bodies.clone();
            let mut relies = HashSet::new();
            for (idx, chunk) in partition.chunks.iter().enumerate() {
                let extracted: Vec<Node> = unique.intersection(chunk).copied().collect();
                for node in extracted {
                    unique.remove(&node);
                    relies.insert(idx);
                }
            }
            (bodies, unique, relies)
        })
        .collect();

    // Who puts back what `--data-prune` zeroed in main: outputs are the
    // modules in order, then the chunk.
    let chunk_output = plans.len();
    let restores: Vec<(Vec<usize>, bool)> = if plan.pruned.is_empty() {
        vec![(Vec::new(), false); plans.len() + partition.chunks.len()]
    } else {
        let funcs = |set: &HashSet<Node>| -> Vec<u32> {
            let mut v: Vec<u32> = set
                .iter()
                .filter_map(|n| match n {
                    Node::Function(f) => Some(*f),
                    Node::DataSymbol(_) => None,
                })
                .collect();
            v.sort_unstable();
            v
        };
        let mut outputs: Vec<Vec<u32>> = plans.iter().map(|(bodies, _, _)| funcs(bodies)).collect();
        outputs.extend(partition.chunks.iter().map(funcs));
        assign_restores(&source, &partition, &addrs, &outputs, chunk_output, &plan.pruned)?
    };

    let modules = std::thread::scope(|scope| -> Result<Vec<SplitModule>> {
        let handles: Vec<_> = partition
            .split_points
            .iter()
            .zip(&plans)
            .zip(&restores)
            .map(|((split, (bodies, unique, relies)), (restore, needs_chunk))| {
                let (source, partition, layout, zeroed) = (&source, &partition, &layout, &plan.zeroed);
                scope.spawn(move || -> Result<SplitModule> {
                    let out = emit_split(
                        source,
                        partition,
                        layout,
                        SplitBody {
                            bodies,
                            unique,
                            entry: Some((&split.export_name, split.export_func, split.index)),
                            restore,
                            zeroed,
                        },
                    )?;
                    let mut relies_on_chunks = relies.clone();
                    if *needs_chunk {
                        relies_on_chunks.insert(0);
                    }
                    Ok(SplitModule {
                        module_name: split.module_name.clone(),
                        hash_id: Some(split.hash_name.clone()),
                        component_name: Some(split.component_name.clone()),
                        bytes: out.bytes,
                        relies_on_chunks,
                    })
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("emit thread panicked")).collect()
    })?;

    let mut chunks = Vec::new();
    for (i, chunk) in partition.chunks.iter().enumerate() {
        let out = emit_split(
            &source,
            &partition,
            &layout,
            SplitBody {
                bodies: chunk,
                unique: chunk,
                entry: None,
                restore: &restores[chunk_output + i].0,
                zeroed: &plan.zeroed,
            },
        )?;
        chunks.push(SplitModule {
            module_name: "split".to_string(),
            hash_id: None,
            component_name: None,
            bytes: out.bytes,
            relies_on_chunks: HashSet::new(),
        });
    }

    Ok(OutputModules {
        main: SplitModule {
            module_name: "main".to_string(),
            hash_id: None,
            component_name: None,
            bytes: main.bytes,
            relies_on_chunks: HashSet::new(),
        },
        modules,
        chunks,
    })
}

/// The build-log lines: printed, not traced — the CLI installs no tracing
/// subscriber.
fn report(plan: &MainPlan, options: &SplitOptions) {
    if !plan.reclaimed.is_empty() {
        eprintln!(
            "[wasm-split] main holds pointers to {} function(s) the partition gave to split modules; main keeps their table slots",
            plan.reclaimed.len(),
        );
    }
    let Some(min) = options.prune_dead_data_min else { return };
    let s = &plan.selection;
    match &s.refused {
        Some(why) => eprintln!("[wasm-split prune-data] not pruning: {why}; main keeps all its data"),
        None => eprintln!(
            "[wasm-split prune-data] zeroed {} bytes of {} data symbol(s) main's code cannot reach \
             ({} under {min} bytes kept, {} without a fixed address kept)",
            plan.zeroed.values().flatten().map(|r| r.len()).sum::<usize>(),
            plan.pruned.len(),
            s.skipped_small,
            s.skipped_unrestorable,
        ),
    }
}
