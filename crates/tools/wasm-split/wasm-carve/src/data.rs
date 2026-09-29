//! Data: what a split output re-materializes, and what `--data-prune`
//! zeroes in main.
//!
//! All outputs share main's memory, and main initializes every data
//! segment. So a split output carries no data of its own — unless
//! `--data-prune` zeroed main's copy of symbols its code reads, in which
//! case it puts them back at their addresses when it loads. What is zeroed
//! and who restores it is decided in `liveness`; this module only moves
//! the bytes.

use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    ops::Range,
};

use wasm_encoder::{ConstExpr, DataSection};

use crate::{graph::DataSymbol, module::ModuleIndex};

/// Per data segment, the disjoint, sorted byte ranges `--data-prune` zeroes
/// in main: the bytes of the pruned symbols MINUS every byte a live symbol
/// also covers. Symbols overlap — a constant nested in a larger one, or
/// two names for one merged constant — so zeroing a dead symbol whole can
/// zero a live one's bytes (on CrewForge, 317 KB of them). This is the
/// ground truth both [`prune_main_data`] and [`rematerialize`] work from.
pub fn zeroed_ranges(
    symbols: &BTreeMap<usize, DataSymbol>,
    pruned: &BTreeSet<usize>,
    live: &HashSet<usize>,
) -> BTreeMap<usize, Vec<Range<usize>>> {
    let ranges_of = |ids: &mut dyn Iterator<Item = &usize>| -> BTreeMap<usize, Vec<Range<usize>>> {
        let mut by_seg: BTreeMap<usize, Vec<Range<usize>>> = BTreeMap::new();
        for id in ids {
            if let Some(s) = symbols.get(id) {
                by_seg
                    .entry(s.which_data_segment)
                    .or_default()
                    .push(s.segment_offset..s.segment_offset + s.symbol_size);
            }
        }
        for ranges in by_seg.values_mut() {
            *ranges = union(std::mem::take(ranges));
        }
        by_seg
    };
    let dead = ranges_of(&mut pruned.iter());
    let keep = ranges_of(&mut live.iter());
    dead.into_iter()
        .map(|(seg, ranges)| {
            let kept = keep.get(&seg).map(Vec::as_slice).unwrap_or(&[]);
            (seg, subtract(&ranges, kept))
        })
        .filter(|(_, r)| !r.is_empty())
        .collect()
}

/// Sorted, merged ranges.
fn union(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.sort_by_key(|r| r.start);
    let mut out: Vec<Range<usize>> = Vec::new();
    for r in ranges {
        match out.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

/// `a` minus `b`; both sorted and disjoint.
fn subtract(a: &[Range<usize>], b: &[Range<usize>]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut j = 0;
    for r in a {
        let mut start = r.start;
        while j < b.len() && b[j].end <= start {
            j += 1;
        }
        let mut k = j;
        while k < b.len() && b[k].start < r.end {
            if b[k].start > start {
                out.push(start..b[k].start);
            }
            start = start.max(b[k].end);
            k += 1;
        }
        if start < r.end {
            out.push(start..r.end);
        }
    }
    out
}

/// The `(memory, address, bytes)` segments that put `ids` back: each
/// symbol's bytes that main actually zeroed (`zeroed`, from
/// [`zeroed_ranges`]) — never a live byte main kept, which main may
/// already have written to — in address order, with pieces that sit back
/// to back merged (each segment costs a header; tens of thousands of small
/// symbols would otherwise pay one each).
pub fn rematerialize(
    source: &ModuleIndex<'_>,
    symbols: &BTreeMap<usize, DataSymbol>,
    ids: &[usize],
    zeroed: &BTreeMap<usize, Vec<Range<usize>>>,
) -> Vec<(u32, i32, Vec<u8>)> {
    let mut wanted: BTreeMap<usize, Vec<Range<usize>>> = BTreeMap::new();
    for id in ids {
        let Some(symbol) = symbols.get(id) else { continue };
        let range = symbol.segment_offset..symbol.segment_offset + symbol.symbol_size;
        for z in zeroed.get(&symbol.which_data_segment).into_iter().flatten() {
            let (start, end) = (range.start.max(z.start), range.end.min(z.end));
            if start < end {
                wanted.entry(symbol.which_data_segment).or_default().push(start..end);
            }
        }
    }
    let mut out: Vec<(u32, i32, Vec<u8>)> = Vec::new();
    for (seg_idx, ranges) in wanted {
        let Some(seg) = source.data.get(seg_idx) else { continue };
        let Some((memory, base)) = seg.active_const else { continue };
        let bytes = &source.bytes[seg.data.clone()];
        for r in union(ranges) {
            if r.end > bytes.len() {
                continue;
            }
            out.push((memory, base + r.start as i32, bytes[r].to_vec()));
        }
    }
    out.sort_by_key(|(memory, addr, _)| (*memory, *addr));
    out
}

/// Main's data section with every byte in `zeroed` (from
/// [`zeroed_ranges`]) set to zero. Segment shapes are unchanged, so every
/// other byte keeps its address; the zeros compress to nothing.
pub fn prune_main_data(
    source: &ModuleIndex<'_>,
    zeroed: &BTreeMap<usize, Vec<Range<usize>>>,
) -> DataSection {
    let mut section = DataSection::new();
    for (idx, seg) in source.data.iter().enumerate() {
        match (zeroed.get(&idx), seg.active_const) {
            (Some(ranges), Some((memory, offset))) => {
                let mut bytes = source.bytes[seg.data.clone()].to_vec();
                for range in ranges {
                    let end = range.end.min(bytes.len());
                    if end > range.start {
                        bytes[range.start..end].fill(0);
                    }
                }
                section.active(memory, &ConstExpr::i32_const(offset), bytes);
            }
            _ => {
                section.raw(&source.bytes[seg.entry.clone()]);
            }
        }
    }
    section
}
