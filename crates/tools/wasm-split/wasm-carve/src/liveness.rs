//! What main keeps, exactly — and therefore what `--data-prune` may zero.
//!
//! The partition in `graph` decides which lazy module OWNS each function,
//! and it may be generous: a function in the wrong module costs size, not
//! correctness, as long as main keeps whatever main can reach. Zeroing data
//! is different. A byte main still reads, zeroed, is silent corruption (the
//! walrus splitter's heuristic did exactly that to a CSS string and to
//! font registrations), so the set of data main keeps is computed here
//! from facts, not from the partition:
//!
//! * the functions main keeps — its exports, start function and table,
//!   closed over direct calls, after the split modules' table slots are
//!   emptied (see [`plan_main`]);
//! * every data symbol those functions' bodies reference, per the rustc
//!   module's relocations, closed over data→data references. LLD emits a
//!   relocation for every address it writes into code or data, so this is
//!   the complete record — [`graph::OriginalGraph::unsound`] refuses the
//!   module when any address relocation points at something that is not a
//!   defined data symbol;
//! * for the functions wasm-bindgen generated (no relocations exist for
//!   them) and for global initializers: every constant address in them.
//!
//! The same closure over a split module's own bodies is what it must put
//! back before its code runs.
//!
//! Function pointers get the matching guarantee: a function that main's
//! live data or live code holds a table index for keeps its slot in main
//! (see [`plan_main`]), so no path from main can reach an emptied slot.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::{Result, bail};
use wasmparser::ExternalKind;

use crate::{
    emit::Layout,
    graph::{OldNode, Partition},
    module::{ElemItem, ModuleIndex},
};

/// Data symbols by address, for the constant-address scan. Symbols
/// overlap (a constant nested in a larger one), so a lookup returns every
/// symbol containing the address.
pub struct AddrIndex {
    /// `(start, end, symbol)`, sorted by start.
    spans: Vec<(u32, u32, usize)>,
    /// The largest symbol: how far back a containing symbol can start.
    widest: u32,
}

impl AddrIndex {
    pub fn new(source: &ModuleIndex<'_>, partition: &Partition) -> Self {
        let mut spans = Vec::new();
        for (id, sym) in &partition.data_symbols {
            let Some(seg) = source.data.get(sym.which_data_segment) else { continue };
            let Some((_, base)) = seg.active_const else { continue };
            let start = base as u32 + sym.segment_offset as u32;
            spans.push((start, start + sym.symbol_size as u32, *id));
        }
        spans.sort_unstable();
        let widest = spans.iter().map(|(s, e, _)| e - s).max().unwrap_or(0);
        AddrIndex { spans, widest }
    }

    /// Every data symbol containing `addr`.
    pub fn symbols_at(&self, addr: u32) -> impl Iterator<Item = usize> + '_ {
        let hi = self.spans.partition_point(|(s, _, _)| *s <= addr);
        let lo = self.spans.partition_point(|(s, _, _)| *s < addr.saturating_sub(self.widest));
        self.spans[lo..hi].iter().filter(move |(_, e, _)| addr < *e).map(|(_, _, id)| *id)
    }
}

/// Every data symbol `funcs` (bindgened indices) and `extra` reference,
/// closed over data→data references.
pub fn data_closure(
    source: &ModuleIndex<'_>,
    partition: &Partition,
    addrs: &AddrIndex,
    funcs: impl IntoIterator<Item = u32>,
    extra: &[usize],
) -> Result<HashSet<usize>> {
    let graph = &partition.original.call_graph;
    let mut live: HashSet<usize> = extra.iter().copied().collect();
    let mut stack: Vec<usize> = extra.to_vec();
    let mut consts = Vec::new();
    for f in funcs {
        match partition.names.new_to_old.get(&f) {
            Some(olds) => {
                for old in olds {
                    for target in graph.get(&OldNode::Function(*old)).into_iter().flatten() {
                        if let OldNode::DataSymbol(s) = target {
                            if live.insert(*s) {
                                stack.push(*s);
                            }
                        }
                    }
                }
            }
            // No rustc counterpart: a function wasm-bindgen wrote. It has
            // no relocations, so read its constant addresses directly.
            None if f >= source.func_imports => {
                consts.clear();
                source.const_addresses(f, &mut consts)?;
                for addr in &consts {
                    for s in addrs.symbols_at(*addr) {
                        if live.insert(s) {
                            stack.push(s);
                        }
                    }
                }
            }
            None => {}
        }
    }
    while let Some(s) = stack.pop() {
        for target in graph.get(&OldNode::DataSymbol(s)).into_iter().flatten() {
            if let OldNode::DataSymbol(t) = target {
                if live.insert(*t) {
                    stack.push(*t);
                }
            }
        }
    }
    Ok(live)
}

/// The bindgened functions whose table slots `funcs` and `data` hold
/// indices of: pointers in code (`TABLE_INDEX_*` in `reloc.CODE`) and in
/// data (vtables, closure tables in `reloc.DATA`).
fn pointer_targets(partition: &Partition, funcs: &[u32], data: &HashSet<usize>) -> HashSet<u32> {
    let graph = &partition.original.call_graph;
    let mut olds: HashSet<u32> = HashSet::new();
    for f in funcs {
        for old in partition.names.new_to_old.get(f).into_iter().flatten() {
            olds.extend(partition.original.code_fn_ptrs.get(old).into_iter().flatten().copied());
        }
    }
    for s in data {
        for target in graph.get(&OldNode::DataSymbol(*s)).into_iter().flatten() {
            if let OldNode::Function(f) = target {
                olds.insert(*f);
            }
        }
    }
    olds.iter()
        .flat_map(|old| partition.names.old_to_new.get(old).into_iter().flatten().copied())
        .collect()
}

/// Why symbols were or were not pruned, for the build log.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PruneSelection {
    pub pruned_bytes: usize,
    pub skipped_small: usize,
    pub skipped_unrestorable: usize,
    /// Set when pruning was asked for and refused.
    pub refused: Option<String>,
}

pub struct MainPlan {
    /// Per function index: main keeps it.
    pub live: Vec<bool>,
    /// Functions whose table slots main hands over to the split outputs.
    pub holes: HashSet<u32>,
    /// Functions the partition gave away that main's live code or data
    /// holds a pointer to, so main keeps their slots after all.
    pub reclaimed: Vec<u32>,
    /// Data symbols main's kept code can reach.
    pub live_data: HashSet<usize>,
    /// Data symbols main's code cannot reach (`--data-prune`): the ones the
    /// split outputs put back.
    pub pruned: BTreeSet<usize>,
    /// The bytes actually zeroed: `pruned`'s minus any a live symbol also
    /// covers. See `data::zeroed_ranges`.
    pub zeroed: BTreeMap<usize, Vec<std::ops::Range<usize>>>,
    pub selection: PruneSelection,
}

/// What main keeps, and what it may zero.
///
/// Starts from the partition's hand-over — every split-reached function
/// main's call graph does not reach gives up its table slot — and repeats
/// until main holds no pointer to a slot it gave up: each round keeps the
/// slot of every such function and recomputes what main reaches.
pub fn plan_main(
    source: &ModuleIndex<'_>,
    partition: &Partition,
    layout: &Layout,
    addrs: &AddrIndex,
    prune_min: Option<usize>,
) -> Result<MainPlan> {
    let mut holes: HashSet<u32> = partition
        .unused_main_symbols(source)
        .into_iter()
        .filter_map(|n| match n {
            crate::graph::Node::Function(f) => Some(f),
            crate::graph::Node::DataSymbol(_) => None,
        })
        .collect();
    // Globals are initialized from constants; one that holds an address
    // (a stack or heap pointer, a static) keeps what it points at.
    let from_globals: Vec<usize> =
        source.global_init_consts.iter().flat_map(|c| addrs.symbols_at(*c as u32)).collect();
    let mut reclaimed = Vec::new();
    let (live, live_data) = loop {
        let live = main_live(source, partition, layout, &holes)?;
        let live_funcs: Vec<u32> =
            (0..source.total_funcs()).filter(|f| live[*f as usize]).collect();
        let live_data =
            data_closure(source, partition, addrs, live_funcs.iter().copied(), &from_globals)?;
        let mut back: Vec<u32> = pointer_targets(partition, &live_funcs, &live_data)
            .into_iter()
            .filter(|f| holes.contains(f))
            .collect();
        if back.is_empty() {
            break (live, live_data);
        }
        back.sort_unstable();
        for f in &back {
            holes.remove(f);
        }
        reclaimed.extend(back);
    };

    let (pruned, selection) = match (prune_min, &partition.original.unsound) {
        (None, _) => (BTreeSet::new(), PruneSelection::default()),
        (Some(_), Some(why)) => {
            (BTreeSet::new(), PruneSelection { refused: Some(why.clone()), ..Default::default() })
        }
        (Some(min), None) => select_prunable(source, &partition.data_symbols, &live_data, min),
    };
    let zeroed = crate::data::zeroed_ranges(&partition.data_symbols, &pruned, &live_data);
    Ok(MainPlan { live, holes, reclaimed, live_data, pruned, zeroed, selection })
}

/// The data symbols main may zero: every one its code cannot reach, at
/// least `min` bytes, in a segment with a fixed address a split output can
/// restore it at (active, constant offset).
pub fn select_prunable(
    source: &ModuleIndex<'_>,
    symbols: &BTreeMap<usize, crate::graph::DataSymbol>,
    live_data: &HashSet<usize>,
    min: usize,
) -> (BTreeSet<usize>, PruneSelection) {
    let mut selection = PruneSelection::default();
    let mut pruned = BTreeSet::new();
    for (id, sym) in symbols {
        if live_data.contains(id) {
            continue;
        }
        let restorable = source
            .data
            .get(sym.which_data_segment)
            .is_some_and(|seg| seg.active_const.is_some());
        if !restorable {
            selection.skipped_unrestorable += 1;
        } else if sym.symbol_size < min {
            selection.skipped_small += 1;
        } else {
            selection.pruned_bytes += sym.symbol_size;
            pruned.insert(*id);
        }
    }
    (pruned, selection)
}

/// Main's kept functions: everything reachable from its exports (the
/// split exports removed, every shared function added), its start function
/// and its table after `holes` give up their slots — the roots walrus's GC
/// used. A function reached only through data needs no root of its own: it
/// is in the table.
fn main_live(
    source: &ModuleIndex<'_>,
    partition: &Partition,
    layout: &Layout,
    holes: &HashSet<u32>,
) -> Result<Vec<bool>> {
    let split_exports: HashSet<&str> =
        partition.split_points.iter().map(|s| s.export_name.as_str()).collect();
    let mut live = vec![false; source.total_funcs() as usize];
    let mut stack: Vec<u32> = Vec::new();
    let root = |f: u32, live: &mut Vec<bool>, stack: &mut Vec<u32>| {
        if !std::mem::replace(&mut live[f as usize], true) {
            stack.push(f);
        }
    };
    for f in 0..source.func_imports {
        root(f, &mut live, &mut stack);
    }
    for e in &source.exports {
        if e.kind == ExternalKind::Func && !split_exports.contains(e.name) {
            root(e.index, &mut live, &mut stack);
        }
    }
    for f in layout.shared_funcs() {
        root(*f, &mut live, &mut stack);
    }
    if let Some(start) = source.start {
        root(start, &mut live, &mut stack);
    }
    for elem in &source.elems {
        for item in &elem.items {
            match item {
                ElemItem::Func(f) if !holes.contains(f) => root(*f, &mut live, &mut stack),
                ElemItem::Func(_) => {}
                ElemItem::Null => bail!("ref.null in an element segment"),
            }
        }
    }
    let mut refs = Vec::new();
    while let Some(f) = stack.pop() {
        if f < source.func_imports {
            continue;
        }
        refs.clear();
        source.direct_refs(f, &mut refs)?;
        for r in &refs {
            root(*r, &mut live, &mut stack);
        }
    }
    Ok(live)
}

/// Which output puts each pruned data symbol back, from what each output's
/// own bodies reference (`outputs[i]` = its kept functions). A symbol one
/// output needs is restored there. A symbol several need is restored ONCE,
/// by the shared chunk (`chunk`) — restoring it again when a second module
/// loads would reset a static the first had already written — and every
/// module that needs it then depends on the chunk.
///
/// Returns, per output, the symbols it restores and whether it needs the
/// chunk loaded first.
pub fn assign_restores(
    source: &ModuleIndex<'_>,
    partition: &Partition,
    addrs: &AddrIndex,
    outputs: &[Vec<u32>],
    chunk: usize,
    pruned: &BTreeSet<usize>,
) -> Result<Vec<(Vec<usize>, bool)>> {
    let mut users: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, funcs) in outputs.iter().enumerate() {
        let closure = data_closure(source, partition, addrs, funcs.iter().copied(), &[])?;
        for s in closure.into_iter().filter(|s| pruned.contains(s)) {
            users.entry(s).or_default().push(i);
        }
    }
    let mut out: Vec<(Vec<usize>, bool)> = vec![(Vec::new(), false); outputs.len()];
    for (s, who) in users {
        if who.len() == 1 {
            out[who[0]].0.push(s);
        } else {
            out[chunk].0.push(s);
            for i in who {
                if i != chunk {
                    out[i].1 = true;
                }
            }
        }
    }
    for (list, _) in &mut out {
        list.sort_unstable();
    }
    Ok(out)
}
