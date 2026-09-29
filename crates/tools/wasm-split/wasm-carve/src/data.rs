//! Data: what a split output re-materializes, and what `--data-prune`
//! zeroes in main.
//!
//! All outputs share main's memory, and main initializes every data
//! segment. So a split output carries no data of its own — unless
//! `--data-prune` zeroed main's copy of its data symbols, in which case it
//! re-initializes them at their addresses when it loads. That pairing is
//! what makes pruning safe: a symbol is only zeroed in main when a split
//! output will put it back.

use std::collections::{BTreeMap, HashMap, HashSet};

use wasm_encoder::{ConstExpr, DataSection};

use crate::{
    graph::{DataSymbol, Node},
    module::ModuleIndex,
};

/// Active segments with a constant offset: the only ones a split output
/// can re-materialize a symbol into, since it needs the address.
fn rematerializable(source: &ModuleIndex<'_>) -> HashSet<usize> {
    source
        .data
        .iter()
        .enumerate()
        .filter(|(_, seg)| seg.active_const.is_some())
        .map(|(idx, _)| idx)
        .collect()
}

/// The `(memory, address, bytes)` of every data symbol in `unique`, in
/// symbol order — one active segment each in the split output. Walks every
/// re-materializable segment: `.rodata` and `.data`/`.bss` alike (see
/// `regression_rematerializes_non_rodata_segments`).
pub fn rematerialize<'a>(
    source: &ModuleIndex<'a>,
    symbols: &BTreeMap<usize, DataSymbol>,
    unique: &HashSet<Node>,
) -> Vec<(u32, i32, &'a [u8])> {
    let mut ids: Vec<usize> = unique
        .iter()
        .filter_map(|n| match n {
            Node::DataSymbol(s) => Some(*s),
            Node::Function(_) => None,
        })
        .collect();
    ids.sort_unstable();
    let mut out = Vec::new();
    for (seg_idx, seg) in source.data.iter().enumerate() {
        let Some((memory, base)) = seg.active_const else { continue };
        let bytes = &source.bytes[seg.data.clone()];
        for id in &ids {
            let Some(symbol) = symbols.get(id) else { continue };
            if symbol.which_data_segment != seg_idx {
                continue;
            }
            let range = symbol.segment_offset..symbol.segment_offset + symbol.symbol_size;
            if range.end > bytes.len() {
                continue;
            }
            out.push((memory, base + symbol.segment_offset as i32, &bytes[range]));
        }
    }
    out
}

/// Counters for the build log.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PruneStats {
    pub zeroed_bytes: usize,
    pub dead_bytes_total: usize,
    pub skipped_small: usize,
    pub skipped_unrematerializable: usize,
}

/// Main's data section with the bytes of every split-only data symbol of at
/// least `min_size` zeroed. Segment shapes are unchanged, so live symbols
/// keep their addresses; the zeros compress to nothing.
///
/// Two gates, both from the walrus implementation this replaces:
/// * `min_size` — the symbol-level graph misclassifies small vtables
///   (4-byte function-index slots), and zeroing one is a null-function trap;
///   24 is the verified floor.
/// * re-materializability — a symbol in a passive or non-constant segment
///   has no address a split output could restore it at, so it stays.
pub fn prune_main_data(
    source: &ModuleIndex<'_>,
    symbols: &BTreeMap<usize, DataSymbol>,
    unused: &HashSet<Node>,
    min_size: usize,
) -> (DataSection, PruneStats) {
    let mut stats = PruneStats::default();
    let restorable = rematerializable(source);
    let mut dead: HashMap<usize, Vec<std::ops::Range<usize>>> = HashMap::new();
    for node in unused {
        let Node::DataSymbol(id) = node else { continue };
        let Some(symbol) = symbols.get(id) else { continue };
        if symbol.symbol_size < min_size {
            stats.skipped_small += 1;
            continue;
        }
        if !restorable.contains(&symbol.which_data_segment) {
            stats.skipped_unrematerializable += 1;
            continue;
        }
        stats.dead_bytes_total += symbol.symbol_size;
        dead.entry(symbol.which_data_segment)
            .or_default()
            .push(symbol.segment_offset..symbol.segment_offset + symbol.symbol_size);
    }

    let mut section = DataSection::new();
    for (idx, seg) in source.data.iter().enumerate() {
        match (dead.get(&idx), seg.active_const) {
            (Some(ranges), Some((memory, offset))) => {
                let mut bytes = source.bytes[seg.data.clone()].to_vec();
                for range in ranges {
                    let end = range.end.min(bytes.len());
                    if end > range.start {
                        bytes[range.start..end].fill(0);
                        stats.zeroed_bytes += end - range.start;
                    }
                }
                section.active(memory, &ConstExpr::i32_const(offset), bytes);
            }
            _ => {
                section.raw(&source.bytes[seg.entry.clone()]);
            }
        }
    }
    (section, stats)
}
