//! What a patch needs to know about the base module running in the page,
//! read once per base build — and the byte-level strip applied to every
//! patch before it is served.
//!
//! [`BaseIndex`] answers, for a symbol a patch imports: which table slot
//! the base keeps that function in (`hotpatch_base` put every function
//! there), what signature the function in a slot has, what the base
//! exports, and where a data symbol lives in the base's memory.
//! [`crate::hotpatch_prepare`] turns those answers into the plan the page
//! loads the patch with.
//!
//! The patch itself used to be rewritten here — every import turned into
//! a local `call_indirect` stub, every `GOT` import into a constant — by a
//! walrus pass that parsed and re-emitted the whole patch on every save
//! (0.7–1.0 s on CrewForge). The page now supplies those imports directly
//! (`backend_web::hot_patch`), so nothing is rewritten; see
//! `hotpatch_prepare` for what replaced it.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{bail, Context, Result};
use walrus::{ir, ConstExpr, ElementKind, Module};

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
    /// Table slot → the signature of the function in it, as
    /// `params>results` in one character per value type (the alphabet
    /// `hotpatch_prepare` compares a patch's import against). The page
    /// passes a base function to the patch as ITSELF, and an import of
    /// the wrong type fails the whole instantiation — so a disagreement
    /// is caught here and the import made to trap instead, as the old
    /// `call_indirect` stub's signature check did.
    pub slot_sigs: HashMap<u32, String>,
    /// Every function the base HAS, under every name the linker knew
    /// it by.
    ///
    /// Only used to explain a failure, and that distinction is the whole
    /// diagnosis: a symbol in here but not in `ifunc` was dropped by a
    /// rooting or GC pass and the fix is in `hotpatch_base`, while a
    /// symbol in neither was never codegened into the base at all and
    /// the fix is in the compile flags.
    pub all_funcs: HashSet<String>,
}

impl BaseIndex {
    /// Read a wasm-bindgen'd base module: its table, its exports, and
    /// its data symbols.
    ///
    /// `aliases` is [`crate::hotpatch_aliases`]'s name-to-name map, read
    /// from the module BEFORE walrus and wasm-bindgen renumbered it.
    /// Without it every symbol the linker knew by a second name — 4,060
    /// of them on the hot-reload lab, `<usize as Display>::fmt` among
    /// them — looks absent, and the patch is refused over functions that
    /// are right there in the table under another spelling.
    pub fn of(wasm: &[u8], aliases: &crate::hotpatch_aliases::AliasMap) -> Result<Self> {
        let module = Module::from_buffer(wasm).context("parsing the base module")?;

        let mut ifunc = HashMap::new();
        let mut slot_sigs = HashMap::new();
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
                let ty = module.types.get(module.funcs.get(*id).ty());
                slot_sigs.entry(base + i as u32).or_insert_with(|| {
                    crate::hotpatch_prepare::sig_string(&(
                        ty.params().iter().map(walrus_val_char).collect(),
                        ty.results().iter().map(walrus_val_char).collect(),
                    ))
                });
                if let Some(name) = module.funcs.get(*id).name.as_deref() {
                    // First slot wins: a function listed twice is two
                    // valid pointers to one body, and either redirects
                    // correctly.
                    ifunc.entry(name.to_string()).or_insert(base + i as u32);
                }
            }
        }

        // A cast intrinsic is an import after bindgen, under a sequence
        // name; its forwarding body carries the mangled name the patch
        // calls it by (`hotpatch_base::cast_trampoline_name`).
        let casts: Vec<(String, u32)> = ifunc
            .iter()
            .filter_map(|(name, slot)| {
                name.strip_prefix(crate::hotpatch_base::CAST_TRAMPOLINE_PREFIX)
                    .map(|symbol| (symbol.to_string(), *slot))
            })
            .collect();
        for (symbol, slot) in casts {
            ifunc.entry(symbol).or_insert(slot);
        }

        let exports = module.exports.iter().map(|e| e.name.clone()).collect();
        let data = data_symbol_addresses(wasm).context("reading the base's data symbols")?;
        let mut all_funcs: HashSet<String> =
            module.funcs.iter().filter_map(|f| f.name.clone()).collect();

        // An alias resolves to whatever slot its canonical name got.
        // `or_insert` rather than `insert`: a name that is BOTH a
        // function's own name-section name and some other function's
        // alias must keep its own slot, or a patch calling it would
        // dispatch into the aliasing function instead.
        for (alias, canonical) in aliases {
            if let Some(slot) = ifunc.get(canonical).copied() {
                ifunc.entry(alias.clone()).or_insert(slot);
            }
            if all_funcs.contains(canonical) {
                all_funcs.insert(alias.clone());
            }
        }

        Ok(Self {
            ifunc,
            exports,
            data,
            slot_sigs,
            all_funcs,
        })
    }
}

impl BaseIndex {
    /// Why a symbol could not be resolved, in the terms that pick the
    /// next move.
    ///
    /// The two cases look identical from the link error and have
    /// opposite fixes, so the message names which one it is rather than
    /// leaving whoever reads the log to go and dump a name section.
    pub fn diagnose(&self, name: &str) -> String {
        if self.all_funcs.contains(name) {
            "the base HAS this function but it is not in the table: a rooting \
             or GC pass dropped it"
                .to_string()
        } else {
            "the base does not contain this function at all: rustc never codegened \
             it into the base, so no rooting pass could have kept it"
                .to_string()
        }
    }
}

/// Drop the `name` section and every DWARF (`.debug_*`) custom section
/// from a module, leaving every other byte where it was.
///
/// Done on the bytes, not through walrus: the sections to drop are
/// self-contained, so removing them is a copy of the rest, where a second
/// walrus pass would parse and re-emit the whole module (0.3 + 0.4 s on a
/// CrewForge patch) to the same result.
pub fn strip_debug_sections(wasm: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(wasm.len());
    let mut kept_until = 0usize;
    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        let payload = payload.context("scanning the patch's sections")?;
        let wasmparser::Payload::CustomSection(c) = payload else {
            continue;
        };
        let name = c.name();
        if name != "name" && !name.starts_with(".debug_") {
            continue;
        }
        // The custom section's full extent: its id byte and size LEB
        // precede `c.range()`, which covers the name and the data.
        let body = c.range();
        let start = section_start(wasm, body.start, body.len())?;
        out.extend_from_slice(&wasm[kept_until..start]);
        kept_until = body.end;
    }
    out.extend_from_slice(&wasm[kept_until..]);
    Ok(out)
}

/// Where the custom section whose contents (`len` bytes) begin at
/// `contents` starts: its id byte (0) and a size LEB that decodes to
/// exactly `len` precede them.
fn section_start(wasm: &[u8], contents: usize, len: usize) -> Result<usize> {
    for leb_len in 1..=5usize {
        let Some(id_at) = contents.checked_sub(leb_len + 1) else { break };
        if wasm[id_at] != 0 {
            continue;
        }
        let leb = &wasm[id_at + 1..contents];
        let (mut value, mut shift, mut ok) = (0u64, 0u32, true);
        for (i, b) in leb.iter().enumerate() {
            value |= u64::from(b & 0x7f) << shift;
            shift += 7;
            let last = i == leb.len() - 1;
            if (b & 0x80 == 0) != last {
                ok = false;
                break;
            }
        }
        if ok && value == len as u64 {
            return Ok(id_at);
        }
    }
    bail!("could not find the start of the custom section at byte {contents}")
}

/// `hotpatch_prepare`'s value-type alphabet, for walrus's types.
fn walrus_val_char(v: &walrus::ValType) -> char {
    match v {
        walrus::ValType::I32 => 'i',
        walrus::ValType::I64 => 'I',
        walrus::ValType::F32 => 'f',
        walrus::ValType::F64 => 'F',
        walrus::ValType::V128 => 'v',
        walrus::ValType::Ref(r) if *r == walrus::RefType::EXTERNREF => 'x',
        walrus::ValType::Ref(r) if *r == walrus::RefType::FUNCREF => 'r',
        walrus::ValType::Ref(_) => '?',
    }
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
                                definition.offset.saturating_add(
                                    segment_offsets.get(&definition.index).copied().unwrap_or(0),
                                ),
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
    use walrus::{ir, ElementItems, FunctionBuilder, RefType, ValType};

    const CAST: &str = "_RINvNvNtCs8e_12wasm_bindgen4___rt8wbg_cast17breaks_if_inlinedReNtB6_7JsValueEB6_";

    /// A served base with one function, `canonical`, in its table at slot
    /// 1 — the shape `hotpatch_base` leaves behind — taking an `i32` and
    /// returning an `i64`.
    fn served_base_with(canonical: &str) -> Vec<u8> {
        let mut module = Module::default();
        let table = module.tables.add_local(false, 2, Some(2), RefType::FUNCREF);
        module.exports.add("__indirect_function_table", table);
        let mut b = FunctionBuilder::new(&mut module.types, &[ValType::I32], &[ValType::I64]);
        b.name(canonical.to_string()).func_body().i64_const(0);
        let arg = module.locals.add(ValType::I32);
        let f = module.funcs.add_local(b.local_func(vec![arg]));
        module.elements.add(
            ElementKind::Active { table, offset: ConstExpr::Value(ir::Value::I32(1)) },
            ElementItems::Functions(vec![f]),
        );
        module.emit_wasm()
    }

    /// A PIC patch importing `imports` (functions of type `[] -> []`,
    /// `GOT.*` globals), for the tests that go through `prepare`.
    fn patch_importing(imports: &[(&str, &str)]) -> Vec<u8> {
        let mut module = Module::default();
        let (table, _) = module.add_import_table("env", "__indirect_function_table", false, 0, None, RefType::FUNCREF);
        let table_base = module.add_import_global("env", "__table_base", ValType::I32, false, false);
        let ty = module.types.add(&[], &[]);
        let mut called = Vec::new();
        for (namespace, name) in imports {
            if namespace.starts_with("GOT.") {
                module.add_import_global(namespace, name, ValType::I32, true, false);
            } else {
                called.push(module.add_import_func(namespace, name, ty).0);
            }
        }
        let mut builder = FunctionBuilder::new(&mut module.types, &[], &[]);
        {
            let mut body = builder.name("__Probe_hot_impl".to_string()).func_body();
            for func in called {
                body.call(func);
            }
        }
        let own = module.funcs.add_local(builder.local_func(vec![]));
        module.elements.add(
            ElementKind::Active { table, offset: ConstExpr::Global(table_base.0) },
            ElementItems::Functions(vec![own]),
        );
        module.emit_wasm()
    }

    fn base_with(ifunc: &[(&str, u32)]) -> BaseIndex {
        BaseIndex {
            ifunc: ifunc.iter().map(|(n, i)| (n.to_string(), *i)).collect(),
            exports: Default::default(),
            data: Default::default(),
            slot_sigs: Default::default(),
            all_funcs: ifunc.iter().map(|(n, _)| n.to_string()).collect(),
        }
    }

    /// Regression (CrewForge ui-shared): a patch calls a cast intrinsic by
    /// its mangled name; after bindgen only the forwarder carries that
    /// name, prefixed. The index resolves the bare name to the forwarder's
    /// slot.
    #[test]
    fn regression_a_cast_intrinsic_resolves_to_its_forwarders_slot() {
        let served = served_base_with(&crate::hotpatch_base::cast_trampoline_name(CAST));
        let base = BaseIndex::of(&served, &Default::default()).unwrap();
        let slot = base.ifunc[&crate::hotpatch_base::cast_trampoline_name(CAST)];
        assert_eq!(base.ifunc.get(CAST), Some(&slot));
    }

    /// Each slot's signature is recorded in `hotpatch_prepare`'s alphabet:
    /// it is what a patch's import is checked against before the page
    /// passes the base's function as that import.
    #[test]
    fn the_index_records_each_slots_signature() {
        let base = BaseIndex::of(&served_base_with("f"), &Default::default()).unwrap();
        assert_eq!(base.slot_sigs.get(&1).map(String::as_str), Some("i>I"));
    }

    /// Regression: the first wasm patch on a real app was refused over
    /// `<usize as Display>::fmt` and `<Element as IntoElement>::into_element`,
    /// both of which the base HAD — as second symbols on functions the
    /// name section calls `<u32 as Display>::fmt` and
    /// `<Element as IntoSceneElement>::into_scene_element`. Indexed by
    /// name-section name alone they looked absent, and the diagnosis said
    /// "never codegened", which pointed at compile flags that cannot help.
    /// With the alias map, the import resolves to the canonical slot.
    #[test]
    fn regression_an_aliased_symbol_resolves_through_its_canonical_slot() {
        use crate::hotpatch_prepare::{prepare, ImportSource};
        let canonical = "_RNvXs8_core3fmt3num3impmNtB9_7Display3fmt";
        let alias = "_RNvXsi_core3fmt3num3impjNtB9_7Display3fmt";
        let served = served_base_with(canonical);
        let patch = patch_importing(&[("env", alias), ("GOT.func", alias)]);

        // Without the map: refused, and NOT mislabelled as a rooting bug.
        let blind = BaseIndex::of(&served, &Default::default()).unwrap();
        let message = format!("{:#}", prepare(&patch, &blind).unwrap_err());
        assert!(message.contains(alias), "{message}");

        let mut aliases = crate::hotpatch_aliases::AliasMap::new();
        aliases.insert(alias.to_string(), canonical.to_string());
        let base = BaseIndex::of(&served, &aliases).unwrap();
        assert_eq!(base.ifunc.get(alias), Some(&1));
        assert_eq!(base.ifunc.get(canonical), Some(&1));
        let prepared = prepare(&patch, &base).unwrap_or_else(|e| panic!("the alias should resolve: {e:#}"));
        // The function import meets a slot whose signature (`i>I`)
        // disagrees with this fixture's `[] -> []`, so it traps rather than
        // failing the load; the GOT entry is the slot either way.
        assert!(prepared.plan.imports.iter().any(|s| matches!(s, ImportSource::Global { value: 1, .. })));
        assert!(prepared.plan.imports.iter().any(|s| matches!(s, ImportSource::Trap(_))));
    }

    /// A name that is some function's OWN name must keep its own slot even
    /// if the alias map also lists it as another function's alias —
    /// otherwise a call to it would dispatch into the other function.
    #[test]
    fn an_alias_never_displaces_a_functions_own_name() {
        let served = served_base_with("own");
        let mut aliases = crate::hotpatch_aliases::AliasMap::new();
        aliases.insert("own".to_string(), "somebody_else".to_string());
        let base = BaseIndex::of(&served, &aliases).unwrap();
        assert_eq!(base.ifunc.get("own"), Some(&1));
    }

    /// The diagnosis names which of the two opposite fixes applies.
    #[test]
    fn a_function_the_base_has_but_did_not_table_is_diagnosed_as_such() {
        let mut base = base_with(&[]);
        base.all_funcs.insert("kept_but_untabled".to_string());
        assert!(base.diagnose("kept_but_untabled").contains("HAS this function"));
        assert!(base.diagnose("nowhere").contains("does not contain"));
    }

    /// The served patch drops its `name` and DWARF sections and keeps
    /// every other byte: the result must still be a valid module with the
    /// same functions, imports, exports and table segment.
    #[test]
    fn stripping_debug_sections_keeps_the_module_intact() {
        let raw = patch_importing(&[("env", "_RNvCs_3app5other")]);
        let mut with_debug = raw.clone();
        for (name, len) in [(".debug_info", 300usize), (".debug_str", 2)] {
            let mut body = vec![name.len() as u8];
            body.extend_from_slice(name.as_bytes());
            body.extend(std::iter::repeat_n(7u8, len));
            with_debug.push(0);
            let mut size = body.len();
            loop {
                let b = (size & 0x7f) as u8;
                size >>= 7;
                if size == 0 {
                    with_debug.push(b);
                    break;
                }
                with_debug.push(b | 0x80);
            }
            with_debug.extend_from_slice(&body);
        }
        let stripped = strip_debug_sections(&with_debug).unwrap();
        let customs: Vec<String> = wasmparser::Parser::new(0)
            .parse_all(&stripped)
            .filter_map(|p| match p.unwrap() {
                wasmparser::Payload::CustomSection(c) => Some(c.name().to_string()),
                _ => None,
            })
            .collect();
        assert!(!customs.iter().any(|c| c == "name" || c.starts_with(".debug")), "{customs:?}");
        wasmparser::Validator::new().validate_all(&stripped).expect("still a valid module");
        let imports = |w: &[u8]| {
            let m = Module::from_buffer(w).unwrap();
            let mut v: Vec<_> = m.imports.iter().map(|i| format!("{}.{}", i.module, i.name)).collect();
            v.sort();
            v
        };
        let (a, b) = (Module::from_buffer(&raw).unwrap(), Module::from_buffer(&stripped).unwrap());
        assert_eq!(a.funcs.iter().count(), b.funcs.iter().count());
        assert_eq!(imports(&raw), imports(&stripped));
        assert_eq!(a.elements.iter().count(), b.elements.iter().count());
    }
}
