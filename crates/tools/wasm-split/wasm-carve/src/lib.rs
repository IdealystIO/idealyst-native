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

pub mod data;
pub mod emit;
pub mod graph;
pub mod module;
pub mod neutralize;

use std::collections::HashSet;

use anyhow::Result;

use crate::{
    emit::{Layout, SplitBody, emit_main, emit_split},
    graph::{Node, Partition},
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
    /// Zero main's copy of split-only data symbols at least this large
    /// (`--data-prune`; 24 is the verified floor). `None` keeps main's data
    /// whole.
    pub prune_dead_data_min: Option<usize>,
}

pub fn split(original: &[u8], bindgened: &[u8], options: &SplitOptions) -> Result<OutputModules> {
    let source = ModuleIndex::parse(bindgened)?;
    let partition = Partition::compute(original, &source)?;
    let layout = Layout::new(&source, &partition)?;

    let main = emit_main(&source, &partition, &layout, options.prune_dead_data_min)?;

    // Per split module: its own bodies (everything it reaches outside
    // main, taken before chunk extraction — an extracted chunk function
    // that is not in `shared_symbols` keeps its own copy — a duplication
    // inherited from the walrus splitter, kept so outputs stayed
    // comparable), and what it installs (after extraction).
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

    let modules = std::thread::scope(|scope| -> Result<Vec<SplitModule>> {
        let handles: Vec<_> = partition
            .split_points
            .iter()
            .zip(&plans)
            .map(|(split, (bodies, unique, relies))| {
                let (source, partition, layout) = (&source, &partition, &layout);
                scope.spawn(move || -> Result<SplitModule> {
                    let out = emit_split(
                        source,
                        partition,
                        layout,
                        SplitBody {
                            bodies,
                            unique,
                            entry: Some((&split.export_name, split.export_func, split.index)),
                        },
                    )?;
                    Ok(SplitModule {
                        module_name: split.module_name.clone(),
                        hash_id: Some(split.hash_name.clone()),
                        component_name: Some(split.component_name.clone()),
                        bytes: out.bytes,
                        relies_on_chunks: relies.clone(),
                    })
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("emit thread panicked")).collect()
    })?;

    let mut chunks = Vec::new();
    for chunk in &partition.chunks {
        let out = emit_split(
            &source,
            &partition,
            &layout,
            SplitBody { bodies: chunk, unique: chunk, entry: None },
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
