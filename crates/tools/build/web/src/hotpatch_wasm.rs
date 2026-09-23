//! The wasm half of the subsecond jump table.
//!
//! On a native target a jump-table entry pairs two ADDRESSES. On wasm
//! there are no addresses: a Rust `fn` pointer is an **index into
//! `__indirect_function_table`**, and calling through one is a
//! `call_indirect` against that table. So a wasm entry pairs two table
//! indices, and finding them means reading two things out of each
//! module:
//!
//! 1. the custom `name` section — function index → name;
//! 2. the element segments — table index → function index.
//!
//! Composing them gives name → table index, which is the value a `fn`
//! pointer actually holds.
//!
//! # Why the name section and not the exports
//!
//! `__<Name>_hot_impl` is a PRIVATE Rust function. `--export-dynamic`
//! exports a module's globals, and a private function has internal
//! linkage, so it is not among them — measured: a base module built
//! with `--export-dynamic` carries ~1700 exports and none of them is a
//! `_hot_impl`. The identifiers survive only in the name section, which
//! is also true of the shipped baseline build. That is what we read.
//!
//! # What the runtime does with the result
//!
//! `subsecond::apply_patch`'s wasm arm grows the table by
//! [`WasmJumpTable::ifunc_count`] and rebases every VALUE by the grow's
//! return (the table's prior length), because the patch module's own
//! element segment is placed at the `__table_base` global it imports.
//! So a value here is an index **within the patch's element segment**,
//! while a key is an **absolute** index in the base's table. Getting
//! that asymmetry backwards produces a table that applies cleanly and
//! dispatches into the wrong function, which is the one failure mode
//! with no error message.
//!
//! Reference: the approach (name-section pairing, element-segment
//! rebasing, growing the table by an ifunc count) follows dioxus's
//! `dx` CLI, whose `build/patch.rs` does the same job for the same
//! runtime. The code here is our own.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use wasmparser::{ElementItems, ElementKind, Parser, Payload};

/// A jump table for a wasm patch, in the shape
/// `subsecond_types::JumpTable` wants.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WasmJumpTable {
    /// base table index → patch element-segment index.
    pub map: BTreeMap<u64, u64>,
    /// How many table slots the patch needs. The runtime grows
    /// `__indirect_function_table` by exactly this before instantiating.
    pub ifunc_count: u32,
}

impl WasmJumpTable {
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// Pair every patchable function the two modules share.
///
/// "Patchable" means the name is one the `#[component]` split produced
/// (`__*_hot_impl`) — the only functions reached through
/// `dev_hot::call`'s indirect dispatch, and therefore the only ones a
/// table entry can redirect. Pairing anything else is at best dead
/// weight and at worst a same-name-different-signature helper routed
/// through a wrong-arity trampoline.
pub fn build_jump_table(base: &[u8], patch: &[u8]) -> Result<WasmJumpTable> {
    let base_slots = table_index_by_name(base).context("reading the base module")?;
    let patch_slots = table_index_by_name(patch).context("reading the patch module")?;

    let mut map = BTreeMap::new();
    for (name, patch_index) in &patch_slots {
        if !is_patchable_symbol(name) {
            continue;
        }
        if let Some(base_index) = base_slots.get(name) {
            map.insert(*base_index as u64, *patch_index as u64);
        }
    }

    Ok(WasmJumpTable {
        map,
        ifunc_count: table_slot_count(patch).context("counting the patch's table slots")?,
    })
}

/// True for a symbol a jump-table entry can usefully redirect.
///
/// Mirrors the native builder's rule, and for the same reason: only
/// `__*_hot_impl` is reached through an indirect call that consults the
/// table. A mangled name embeds the ident, so a substring test holds for
/// both legacy and v0 mangling.
pub fn is_patchable_symbol(name: &str) -> bool {
    name.contains("_hot_impl")
}

/// name → the index that function occupies in
/// `__indirect_function_table`.
///
/// Only functions that are actually IN the table appear: a function
/// whose address is never taken has no slot, and no `fn` pointer can
/// ever hold its index, so there is nothing to redirect.
fn table_index_by_name(wasm: &[u8]) -> Result<BTreeMap<String, u32>> {
    let names = function_names(wasm)?;
    let mut out = BTreeMap::new();
    for (table_index, func_index) in element_slots(wasm)? {
        if let Some(name) = names.get(&func_index) {
            // First slot wins. A function listed twice would be two
            // valid pointers to one body; redirecting the first is
            // correct and redirecting both would be redundant.
            out.entry(name.clone()).or_insert(table_index);
        }
    }
    Ok(out)
}

/// function index → name, from the custom `name` section's function
/// subsection (id 1).
fn function_names(wasm: &[u8]) -> Result<BTreeMap<u32, String>> {
    let mut out = BTreeMap::new();
    for payload in Parser::new(0).parse_all(wasm) {
        let payload = payload.context("parsing wasm sections")?;
        if let Payload::CustomSection(c) = payload {
            if c.name() != "name" {
                continue;
            }
            let reader = wasmparser::NameSectionReader::new(wasmparser::BinaryReader::new(
                c.data(),
                c.data_offset(),
            ));
            for subsection in reader {
                let subsection = match subsection {
                    Ok(s) => s,
                    // A malformed name subsection is not fatal: we may
                    // still have read the function names, and a patch
                    // with fewer pairings degrades to a rebuild rather
                    // than failing the build.
                    Err(_) => break,
                };
                if let wasmparser::Name::Function(map) = subsection {
                    for naming in map {
                        let naming = naming.context("parsing a function name")?;
                        out.insert(naming.index, naming.name.to_string());
                    }
                }
            }
        }
    }
    Ok(out)
}

/// (table index, function index) for every active element-segment entry.
///
/// The segment's offset is folded two ways, and the difference is the
/// whole reason a jump-table key and value are not symmetric:
///
/// * an `i32.const` offset — what a normally-linked base module has —
///   gives ABSOLUTE indices in the running table;
/// * a `global.get` offset — what a `--pie --experimental-pic` patch
///   has, where the global is the imported `__table_base` — is counted
///   from ZERO, giving indices RELATIVE to the patch's own segment.
///
/// Relative is right for the patch because `apply_patch` grows
/// `__indirect_function_table` and rebases the patch's entries by the
/// grow's return value, which is what `__table_base` resolves to. So
/// the value must not already carry an offset.
///
/// Folding a global to zero rather than skipping the segment matters:
/// skipping it yields an empty map for every patch, and therefore a
/// jump table that applies cleanly and redirects nothing.
///
/// Passive and declared segments are not in the table at instantiation,
/// so nothing can hold a pointer into one; those are skipped.
fn element_slots(wasm: &[u8]) -> Result<Vec<(u32, u32)>> {
    let mut out = Vec::new();
    for payload in Parser::new(0).parse_all(wasm) {
        let payload = payload.context("parsing wasm sections")?;
        let Payload::ElementSection(reader) = payload else {
            continue;
        };
        for element in reader {
            let element = element.context("parsing an element segment")?;
            let ElementKind::Active { offset_expr, .. } = element.kind else {
                continue;
            };
            // See the doc comment: a non-constant offset is the patch's
            // imported `__table_base`, and its entries are relative.
            let base = const_i32(&offset_expr).unwrap_or(0);
            if let ElementItems::Functions(funcs) = element.items {
                for (i, func) in funcs.into_iter().enumerate() {
                    let func = func.context("parsing an element entry")?;
                    out.push((base.saturating_add(i as u32), func));
                }
            }
        }
    }
    Ok(out)
}

/// How many table slots a module's element segments occupy.
///
/// For a patch this is what the runtime grows
/// `__indirect_function_table` by; for a base it is how many functions
/// `hotpatch_base` left reachable, which is worth printing because it is
/// the number that decides whether a patch can call anything.
///
/// Counts entries rather than reading the segment's declared offset:
/// the patch is linked `--shared`, so its offset is the imported
/// `__table_base` global and is not a constant we could read here.
pub fn table_slot_count(wasm: &[u8]) -> Result<u32> {
    let mut count = 0u32;
    for payload in Parser::new(0).parse_all(wasm) {
        let payload = payload.context("parsing wasm sections")?;
        let Payload::ElementSection(reader) = payload else {
            continue;
        };
        for element in reader {
            let element = element.context("parsing an element segment")?;
            if let ElementItems::Functions(funcs) = element.items {
                count = count.saturating_add(funcs.count());
            }
        }
    }
    Ok(count)
}

/// Constant-fold an `i32.const` offset expression. `None` for anything
/// else — in practice the `global.get __table_base` a PIC patch
/// carries, which the caller folds to zero.
fn const_i32(expr: &wasmparser::ConstExpr) -> Option<u32> {
    let mut reader = expr.get_operators_reader();
    let first = reader.read().ok()?;
    match first {
        wasmparser::Operator::I32Const { value } => Some(value as u32),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-assemble a module with a name section and one active
    /// element segment, so the pairing logic is tested against real
    /// bytes rather than a mock.
    ///
    /// `funcs` is `(function index, name)`; `table_base` is the element
    /// segment's constant offset; the segment lists the function indices
    /// in order.
    fn module(table_base: u32, funcs: &[(u32, &str)]) -> Vec<u8> {
        let mut out = b"\0asm\x01\0\0\0".to_vec();

        // --- element section (id 9) ---
        let mut seg = Vec::new();
        seg.push(0x00); // active, table 0, expr offset
        seg.push(0x41); // i32.const
        leb_i32(&mut seg, table_base as i32);
        seg.push(0x0b); // end
        leb_u32(&mut seg, funcs.len() as u32);
        for (idx, _) in funcs {
            leb_u32(&mut seg, *idx);
        }
        let mut elem = Vec::new();
        leb_u32(&mut elem, 1); // one segment
        elem.extend_from_slice(&seg);
        section(&mut out, 9, &elem);

        // --- custom "name" section, function subsection ---
        let mut namemap = Vec::new();
        leb_u32(&mut namemap, funcs.len() as u32);
        for (idx, name) in funcs {
            leb_u32(&mut namemap, *idx);
            leb_u32(&mut namemap, name.len() as u32);
            namemap.extend_from_slice(name.as_bytes());
        }
        let mut sub = Vec::new();
        sub.push(1); // function-names subsection
        leb_u32(&mut sub, namemap.len() as u32);
        sub.extend_from_slice(&namemap);

        let mut custom = Vec::new();
        leb_u32(&mut custom, 4);
        custom.extend_from_slice(b"name");
        custom.extend_from_slice(&sub);
        section(&mut out, 0, &custom);

        out
    }

    /// Like [`module`], but the element segment's offset is
    /// `global.get 0` — how wasm-ld emits a `--pie
    /// --experimental-pic` patch, where the global is the imported
    /// `__table_base`.
    fn pic_module(funcs: &[(u32, &str)]) -> Vec<u8> {
        let plain = module(0, funcs);
        // Swap the `i32.const 0` (0x41 0x00) offset for
        // `global.get 0` (0x23 0x00). Both are two bytes, so the
        // section lengths stay valid.
        let from = [0x41u8, 0x00, 0x0b];
        let to = [0x23u8, 0x00, 0x0b];
        let at = plain
            .windows(3)
            .position(|w| w == from)
            .expect("the const offset is in there");
        let mut out = plain;
        out[at..at + 3].copy_from_slice(&to);
        out
    }

    /// Regression: a PIC patch's element segment is offset by the
    /// imported `__table_base` global, not by a constant. Treating a
    /// non-constant offset as "skip this segment" produced an empty map
    /// for EVERY real patch — a jump table that applies cleanly and
    /// redirects nothing, with no error anywhere. It must fold to zero
    /// instead, because `apply_patch` rebases the values itself.
    #[test]
    fn regression_a_pic_patch_offset_by_a_global_still_pairs() {
        let base = module(1, &[(10, "other"), (11, "__Counter_hot_impl")]);
        let patch = pic_module(&[(4, "unrelated"), (5, "__Counter_hot_impl")]);

        let jt = build_jump_table(&base, &patch).unwrap();
        assert_eq!(jt.map.get(&2), Some(&1), "map was {:?}", jt.map);
        assert_eq!(jt.ifunc_count, 2);
    }

    fn section(out: &mut Vec<u8>, id: u8, body: &[u8]) {
        out.push(id);
        leb_u32(out, body.len() as u32);
        out.extend_from_slice(body);
    }

    fn leb_u32(out: &mut Vec<u8>, mut v: u32) {
        loop {
            let mut b = (v & 0x7f) as u8;
            v >>= 7;
            if v != 0 {
                b |= 0x80;
            }
            out.push(b);
            if v == 0 {
                break;
            }
        }
    }

    fn leb_i32(out: &mut Vec<u8>, v: i32) {
        let mut v = v;
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            let done = (v == 0 && byte & 0x40 == 0) || (v == -1 && byte & 0x40 != 0);
            out.push(if done { byte } else { byte | 0x80 });
            if done {
                break;
            }
        }
    }

    /// The whole point: a key is the BASE's absolute table index, a
    /// value is the PATCH's index within its own element segment. The
    /// runtime rebases values by the table's prior length, because the
    /// patch's segment lands at the `__table_base` it imports.
    ///
    /// Getting the asymmetry backwards yields a table that applies
    /// cleanly and dispatches into the wrong function — the one failure
    /// mode with no error message.
    #[test]
    fn a_key_is_absolute_in_the_base_and_a_value_is_relative_in_the_patch() {
        // Base: table starts at 1 (LLD's default `--table-base`), so
        // `__Counter_hot_impl` sits at table index 2.
        let base = module(1, &[(10, "other"), (11, "__Counter_hot_impl")]);
        // Patch: its own segment, so the same function is at offset 1.
        let patch = module(0, &[(4, "unrelated"), (5, "__Counter_hot_impl")]);

        let jt = build_jump_table(&base, &patch).unwrap();
        assert_eq!(jt.map.len(), 1, "one patchable symbol in common");
        assert_eq!(jt.map.get(&2), Some(&1), "base table idx 2 -> patch elem idx 1");
        assert_eq!(jt.ifunc_count, 2, "the patch needs both of its slots");
    }

    /// Only `__*_hot_impl` is reached through an indirect call that
    /// consults the table. Pairing anything else is dead weight at best
    /// and a wrong-arity trampoline at worst.
    #[test]
    fn only_hot_impl_symbols_are_paired() {
        let base = module(0, &[(1, "core::ptr::drop_in_place"), (2, "__A_hot_impl")]);
        let patch = module(0, &[(1, "core::ptr::drop_in_place"), (2, "__A_hot_impl")]);
        let jt = build_jump_table(&base, &patch).unwrap();
        assert_eq!(jt.map.len(), 1);
        assert!(jt.map.contains_key(&1), "only the hot_impl: {:?}", jt.map);
    }

    /// A function the base does not have cannot be redirected — there
    /// is no pointer in the running module to redirect. Skipped, not
    /// guessed at.
    #[test]
    fn a_symbol_absent_from_the_base_is_skipped() {
        let base = module(0, &[(1, "__A_hot_impl")]);
        let patch = module(0, &[(1, "__A_hot_impl"), (2, "__BrandNew_hot_impl")]);
        let jt = build_jump_table(&base, &patch).unwrap();
        assert_eq!(jt.map.len(), 1);
        assert!(!jt.map.values().any(|v| *v == 1));
    }

    /// A function whose address is never taken has no table slot, so no
    /// `fn` pointer can hold its index and there is nothing to redirect.
    #[test]
    fn a_function_not_in_the_table_has_no_entry() {
        // Named, but absent from the element segment.
        let base = module(0, &[(1, "__A_hot_impl")]);
        let mut patch = module(0, &[(1, "__A_hot_impl")]);
        // Strip the element section from the patch entirely.
        patch = module_without_elements(&patch);
        let jt = build_jump_table(&base, &patch).unwrap();
        assert!(jt.is_empty(), "{:?}", jt.map);
        assert_eq!(jt.ifunc_count, 0);
    }

    fn module_without_elements(wasm: &[u8]) -> Vec<u8> {
        // Rebuild keeping everything but section id 9.
        let mut out = wasm[..8].to_vec();
        let mut i = 8usize;
        while i < wasm.len() {
            let id = wasm[i];
            let (len, next) = read_leb(wasm, i + 1);
            let end = next + len as usize;
            if id != 9 {
                out.extend_from_slice(&wasm[i..end]);
            }
            i = end;
        }
        out
    }

    fn read_leb(d: &[u8], mut i: usize) -> (u32, usize) {
        let (mut r, mut s) = (0u32, 0u32);
        loop {
            let b = d[i];
            i += 1;
            r |= ((b & 0x7f) as u32) << s;
            if b & 0x80 == 0 {
                return (r, i);
            }
            s += 7;
        }
    }

    /// `ifunc_count` is what the runtime grows the table by, so it
    /// counts the patch's ENTRIES. It cannot be read off the segment's
    /// offset: a `--shared` patch's offset is the imported
    /// `__table_base` global, not a constant.
    #[test]
    fn ifunc_count_counts_entries_not_offsets() {
        let patch = module(0, &[(1, "a"), (2, "b"), (3, "c")]);
        assert_eq!(table_slot_count(&patch).unwrap(), 3);
    }

    /// An empty or nameless module is answered with an empty table
    /// rather than an error: the caller's fallback is a rebuild, and a
    /// build should not fail because a module had nothing to pair.
    #[test]
    fn a_module_with_no_names_yields_an_empty_table() {
        let bare = b"\0asm\x01\0\0\0".to_vec();
        let jt = build_jump_table(&bare, &bare).unwrap();
        assert!(jt.is_empty());
    }
}
