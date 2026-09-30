//! One wasm function, several symbol names.
//!
//! # The bug this exists to prevent
//!
//! A patch resolves everything it did not recompile against the base's
//! `__indirect_function_table`, and it looks each one up **by mangled
//! symbol name**. The base's names come from the custom `name` section,
//! which holds exactly ONE name per function index. That is not the same
//! set the linker resolved against, and the difference is not rare:
//! measured on the hot-reload lab, 949 of the base's functions carry more
//! than one symbol.
//!
//! Two ways that happens, both seen in one ordinary base:
//!
//! * **Types that are the same machine type.** `usize` is `u32` on
//!   wasm32, so `<usize as Display>::fmt` and `<u32 as Display>::fmt`
//!   are one function in the precompiled `core` rlib. The name section
//!   calls it `<u32 as Display>::fmt`; a patch that formats a `usize`
//!   asks for the other name. No opt level changes this — the merge is
//!   in the shipped sysroot.
//! * **LLVM's merge-functions pass.** A dependency built at `opt-level =
//!   3` (which is what `--dev-opt optimized` gives every dependency)
//!   folds identical bodies together. `<Element as
//!   IntoElement>::into_element` and `<Element as
//!   IntoSceneElement>::into_scene_element` are both `{ self }`, so they
//!   become one function carrying the second's name.
//!
//! Looked up by name-section name alone, either one reports "the base
//! does not contain this function" — indistinguishable from the genuine
//! case of a function rustc never codegened, and it sends whoever reads
//! it after compile flags that cannot help.
//!
//! # Where the other names are
//!
//! In the `linking` custom section's symbol table, which LLD emits under
//! `--emit-relocs` and which lists every symbol with the function index
//! it resolved to. Grouping symbols by index and pairing them against
//! the name section gives `alias -> canonical` — a name-to-name map with
//! no index in it, which matters because the index is the one part that
//! does not survive: wasm-bindgen's GC renumbers every function, so by
//! the time the page is running the `linking` section's indices name
//! different functions. The map is read once, from the LINKED module,
//! before base prep drops that section and wasm-bindgen runs.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use anyhow::{Context, Result};
use wasmparser::{Parser, Payload, SymbolInfo};

/// Alias symbol name -> the name the base's `name` section gives that
/// same function.
pub type AliasMap = BTreeMap<String, String>;

/// Read the alias map out of a freshly LINKED base module.
///
/// Must be the module cargo's linker produced, before
/// [`crate::hotpatch_base::prepare_base_module`] and before
/// wasm-bindgen: both renumber functions, and the `linking` section they
/// carry forward keeps the OLD indices, so reading it later pairs
/// symbols with unrelated functions.
pub fn read_from_linked(linked: &[u8]) -> Result<AliasMap> {
    let names = function_names(linked).context("reading the linked module's name section")?;

    // Function index -> every symbol the linker recorded for it.
    let mut by_index: HashMap<u32, Vec<String>> = HashMap::new();
    for payload in Parser::new(0).parse_all(linked) {
        let Payload::CustomSection(c) = payload.context("parsing the linked module's sections")?
        else {
            continue;
        };
        if c.name() != "linking" {
            continue;
        }
        let reader = wasmparser::LinkingSectionReader::new(wasmparser::BinaryReader::new(
            c.data(),
            c.data_offset(),
        ))
        .context("parsing the linking section")?;
        for subsection in reader.subsections() {
            let wasmparser::Linking::SymbolTable(symbols) =
                subsection.context("parsing a linking subsection")?
            else {
                continue;
            };
            for symbol in symbols {
                let symbol = symbol.context("parsing a linking symbol")?;
                let SymbolInfo::Func { flags, index, name } = symbol else {
                    continue;
                };
                // An undefined symbol is a reference to somewhere else;
                // its index is an import's, and nothing in the base's
                // table answers to it.
                if flags.contains(wasmparser::SymbolFlags::UNDEFINED) {
                    continue;
                }
                let Some(name) = name else { continue };
                by_index.entry(index).or_default().push(name.to_string());
            }
        }
    }

    let mut out = AliasMap::new();
    for (index, symbols) in by_index {
        // No name-section entry means no key in the base's table index
        // either, so an alias pointing at it would resolve to nothing.
        let Some(canonical) = names.get(&index) else {
            continue;
        };
        for symbol in symbols {
            if &symbol != canonical {
                out.insert(symbol, canonical.clone());
            }
        }
    }
    Ok(out)
}

/// Serialize the map next to the base module.
///
/// Tab-separated, one pair per line. A Rust mangled symbol contains
/// neither a tab nor a newline, so the format needs no quoting — and
/// being greppable is worth more here than being compact, since the
/// first question about a failed patch is always "is this name in the
/// map".
pub fn write(path: &Path, aliases: &AliasMap) -> Result<()> {
    let mut out = String::with_capacity(aliases.len() * 96);
    for (alias, canonical) in aliases {
        out.push_str(alias);
        out.push('\t');
        out.push_str(canonical);
        out.push('\n');
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(path, out).with_context(|| format!("write {}", path.display()))
}

/// Read a map [`write`] produced. A missing file is an empty map, not an
/// error: a base built before this existed still patches, just without
/// the aliased symbols.
pub fn read(path: &Path) -> Result<AliasMap> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(AliasMap::new()),
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    let mut out = AliasMap::new();
    for line in text.lines() {
        if let Some((alias, canonical)) = line.split_once('\t') {
            out.insert(alias.to_string(), canonical.to_string());
        }
    }
    Ok(out)
}

/// Data symbol name -> its absolute address in the base's memory.
pub type DataSymbols = BTreeMap<String, u32>;

/// Read every DEFINED data symbol's address out of a freshly LINKED base
/// module: its `linking` symbol table (segment index + offset) against
/// its data section (each segment's constant offset).
///
/// A patch imports the base's statics as `GOT.mem` globals, and this is
/// the only place their addresses are written down. Read here, alongside
/// the alias map, so the base the page loads need not carry `linking` at
/// all: with the `reloc.*` sections that ride along with it that is
/// ~150 MB of CrewForge's ~300 MB base, which wasm-bindgen, every later
/// pass and every page reload otherwise paid for, and which nothing reads
/// once the functions have been renumbered.
pub fn read_data_symbols_from_linked(linked: &[u8]) -> Result<DataSymbols> {
    // Segment index -> the segment's own offset in linear memory.
    let mut segment_offsets: BTreeMap<u32, u32> = BTreeMap::new();
    let mut out = DataSymbols::new();
    for payload in Parser::new(0).parse_all(linked) {
        match payload.context("parsing the linked module's sections")? {
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
                    let wasmparser::Linking::SymbolTable(symbols) =
                        subsection.context("parsing a linking subsection")?
                    else {
                        continue;
                    };
                    for symbol in symbols {
                        // Only a DEFINED data symbol has a location. An
                        // undefined one is a reference to somewhere else
                        // and has no address to hand out.
                        if let SymbolInfo::Data { name, symbol: Some(def), .. } =
                            symbol.context("parsing a linking symbol")?
                        {
                            // The data section precedes the custom
                            // sections, so every offset is known here.
                            let at = segment_offsets.get(&def.index).copied().unwrap_or(0);
                            out.insert(name.to_string(), def.offset.saturating_add(at));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Serialize the data symbols next to the base module: `name<TAB>address`
/// per line, like [`write`].
pub fn write_data_symbols(path: &Path, data: &DataSymbols) -> Result<()> {
    let mut out = String::with_capacity(data.len() * 64);
    for (name, addr) in data {
        out.push_str(name);
        out.push('\t');
        out.push_str(&addr.to_string());
        out.push('\n');
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(path, out).with_context(|| format!("write {}", path.display()))
}

/// Read what [`write_data_symbols`] wrote. A missing file is an empty
/// map: every `GOT.mem` import then goes unresolved and the patch falls
/// back to a rebuild naming the symbol, which is the loud failure.
pub fn read_data_symbols(path: &Path) -> Result<DataSymbols> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(DataSymbols::new()),
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    let mut out = DataSymbols::new();
    for line in text.lines() {
        let Some((name, addr)) = line.split_once('\t') else { continue };
        let addr = addr
            .parse()
            .with_context(|| format!("{}: bad address in `{line}`", path.display()))?;
        out.insert(name.to_string(), addr);
    }
    Ok(out)
}

/// function index -> name, from the custom `name` section's function
/// subsection.
fn function_names(wasm: &[u8]) -> Result<BTreeMap<u32, String>> {
    let mut out = BTreeMap::new();
    for payload in Parser::new(0).parse_all(wasm) {
        let Payload::CustomSection(c) = payload.context("parsing wasm sections")? else {
            continue;
        };
        if c.name() != "name" {
            continue;
        }
        let reader = wasmparser::NameSectionReader::new(wasmparser::BinaryReader::new(
            c.data(),
            c.data_offset(),
        ));
        for subsection in reader {
            // A malformed subsection is not fatal: the function names may
            // already be read, and fewer aliases degrades to a rebuild
            // rather than a failed build.
            let Ok(subsection) = subsection else { break };
            if let wasmparser::Name::Function(map) = subsection {
                for naming in map {
                    let naming = naming.context("parsing a function name")?;
                    out.insert(naming.index, naming.name.to_string());
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_round_trip_through_the_file_keeps_every_pair() {
        let mut map = AliasMap::new();
        map.insert("_RNvXsi_alias".into(), "_RNvXs8_canonical".into());
        map.insert("__second".into(), "__first".into());
        let dir = std::env::temp_dir().join("idealyst-alias-roundtrip");
        let path = dir.join("aliases.tsv");
        write(&path, &map).unwrap();
        assert_eq!(read(&path).unwrap(), map);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A base built before the map existed, or one whose build skipped
    /// it, must still patch — just without the aliased symbols.
    #[test]
    fn a_missing_file_reads_as_an_empty_map() {
        let path = std::env::temp_dir().join("idealyst-alias-does-not-exist.tsv");
        let _ = std::fs::remove_file(&path);
        assert!(read(&path).unwrap().is_empty());
    }

    /// The canonical name must never map to itself: `BaseIndex` inserts
    /// aliases over a map with thousands of entries, and a self-pair is
    /// a wasted one.
    #[test]
    fn the_name_sections_own_name_is_not_an_alias_of_itself() {
        let wasm = linked_fixture(&[("f", &["f", "g"])]);
        let map = read_from_linked(&wasm).unwrap();
        assert_eq!(map.get("g").map(String::as_str), Some("f"));
        assert!(!map.contains_key("f"), "{map:?}");
    }

    /// The regression this module exists for. Two symbols on one
    /// function index, the name section keeping only one of them — the
    /// shape `<usize as Display>::fmt` / `<u32 as Display>::fmt` has in
    /// every wasm32 base, since `usize` IS `u32` there.
    #[test]
    fn regression_a_second_symbol_on_one_function_becomes_an_alias() {
        let usize_fmt = "_RNvXsi_NtNtNtCs0_4core3fmt3num3impjNtB9_7Display3fmt";
        let u32_fmt = "_RNvXs8_NtNtNtCs0_4core3fmt3num3impmNtB9_7Display3fmt";
        let wasm = linked_fixture(&[(u32_fmt, &[u32_fmt, usize_fmt])]);
        let map = read_from_linked(&wasm).unwrap();
        assert_eq!(
            map.get(usize_fmt).map(String::as_str),
            Some(u32_fmt),
            "the usize spelling has to resolve to the u32 one's slot: {map:?}"
        );
    }

    /// An undefined symbol's index is an import's. Treating it as a
    /// definition would alias a name onto whatever local function
    /// happens to share that number.
    #[test]
    fn an_undefined_symbol_is_not_a_definition() {
        let wasm = linked_fixture_with_undefined("f", "somewhere_else");
        let map = read_from_linked(&wasm).unwrap();
        assert!(map.is_empty(), "{map:?}");
    }

    /// A base linked without `--emit-relocs` has no `linking` section.
    /// That is a degradation (aliased symbols stop resolving), not a
    /// build failure.
    #[test]
    fn a_module_without_a_linking_section_yields_an_empty_map() {
        let mut module = walrus::Module::default();
        let mut b = walrus::FunctionBuilder::new(&mut module.types, &[], &[]);
        b.name("f".to_string()).func_body();
        module.funcs.add_local(b.local_func(vec![]));
        assert!(read_from_linked(&module.emit_wasm()).unwrap().is_empty());
    }

    /// A patch's `GOT.mem` import is answered with this address, so it
    /// must be the segment's own offset PLUS the symbol's offset in it —
    /// and only for DEFINED symbols.
    #[test]
    fn a_data_symbol_resolves_to_its_segment_offset_plus_its_own() {
        let wasm = data_fixture();
        let data = read_data_symbols_from_linked(&wasm).unwrap();
        assert_eq!(data.get("A").copied(), Some(1024 + 8));
        assert_eq!(data.get("B").copied(), Some(4096 + 16));
        assert!(!data.contains_key("C"), "an undefined symbol has no address: {data:?}");
    }

    #[test]
    fn data_symbols_round_trip_through_their_file() {
        let data = read_data_symbols_from_linked(&data_fixture()).unwrap();
        let path = std::env::temp_dir().join("idealyst-datasyms-roundtrip").join("datasyms.tsv");
        write_data_symbols(&path, &data).unwrap();
        assert_eq!(read_data_symbols(&path).unwrap(), data);
        let missing = std::env::temp_dir().join("idealyst-datasyms-does-not-exist.tsv");
        let _ = std::fs::remove_file(&missing);
        assert!(read_data_symbols(&missing).unwrap().is_empty());
    }

    /// Two segments (at 1024 and 4096) and a symbol table with a defined
    /// symbol in each and one undefined.
    fn data_fixture() -> Vec<u8> {
        let mut module = walrus::Module::default();
        let memory = module.memories.add_local(false, false, 1, None, None);
        for at in [1024, 4096] {
            module.data.add(
                walrus::DataKind::Active {
                    memory,
                    offset: walrus::ConstExpr::Value(walrus::ir::Value::I32(at)),
                },
                vec![0; 32],
            );
        }
        let mut wasm = module.emit_wasm();

        fn leb(out: &mut Vec<u8>, mut v: u32) {
            loop {
                let byte = (v & 0x7f) as u8;
                v >>= 7;
                if v == 0 {
                    out.push(byte);
                    return;
                }
                out.push(byte | 0x80);
            }
        }
        // Data symbol: kind 1, flags, name; a defined one then carries
        // (segment, offset, size).
        let mut table = Vec::new();
        leb(&mut table, 3);
        for (name, def) in [("A", Some((0, 8))), ("B", Some((1, 16))), ("C", None)] {
            table.push(1);
            leb(&mut table, if def.is_some() { 0 } else { 0x10 });
            leb(&mut table, name.len() as u32);
            table.extend_from_slice(name.as_bytes());
            if let Some((segment, offset)) = def {
                leb(&mut table, segment);
                leb(&mut table, offset);
                leb(&mut table, 4);
            }
        }
        let mut body = Vec::new();
        leb(&mut body, 2);
        body.push(8);
        leb(&mut body, table.len() as u32);
        body.extend_from_slice(&table);
        let mut payload = Vec::new();
        leb(&mut payload, "linking".len() as u32);
        payload.extend_from_slice(b"linking");
        payload.extend_from_slice(&body);
        wasm.push(0);
        leb(&mut wasm, payload.len() as u32);
        wasm.extend_from_slice(&payload);
        wasm
    }

    /// Build a module whose name section names one function and whose
    /// `linking` symbol table gives it several symbols.
    ///
    /// Hand-assembled because walrus does not model the `linking`
    /// section — it round-trips it as opaque bytes — and the encoding is
    /// part of what is being tested.
    fn linked_fixture(funcs: &[(&str, &[&str])]) -> Vec<u8> {
        let mut module = walrus::Module::default();
        for (name, _) in funcs {
            let mut b = walrus::FunctionBuilder::new(&mut module.types, &[], &[]);
            b.name((*name).to_string()).func_body();
            module.funcs.add_local(b.local_func(vec![]));
        }
        let mut symbols: Vec<(u32, &str, bool)> = Vec::new();
        for (index, (_, names)) in funcs.iter().enumerate() {
            for name in *names {
                symbols.push((index as u32, name, false));
            }
        }
        with_linking(module.emit_wasm(), &symbols)
    }

    fn linked_fixture_with_undefined(defined: &str, undefined: &str) -> Vec<u8> {
        let mut module = walrus::Module::default();
        let mut b = walrus::FunctionBuilder::new(&mut module.types, &[], &[]);
        b.name(defined.to_string()).func_body();
        module.funcs.add_local(b.local_func(vec![]));
        with_linking(module.emit_wasm(), &[(0, undefined, true)])
    }

    /// Append a `linking` custom section carrying a symbol table.
    fn with_linking(mut wasm: Vec<u8>, symbols: &[(u32, &str, bool)]) -> Vec<u8> {
        fn leb(out: &mut Vec<u8>, mut v: u32) {
            loop {
                let byte = (v & 0x7f) as u8;
                v >>= 7;
                if v == 0 {
                    out.push(byte);
                    return;
                }
                out.push(byte | 0x80);
            }
        }

        // WASM_SYMBOL_TABLE = subsection id 8; each entry is
        // (kind, flags, index, name) with kind 0 = function. An
        // undefined symbol carries no name unless EXPLICIT_NAME is set,
        // which is what makes it skippable.
        let mut table = Vec::new();
        leb(&mut table, symbols.len() as u32);
        for (index, name, undefined) in symbols {
            table.push(0);
            leb(&mut table, if *undefined { 0x10 } else { 0 });
            leb(&mut table, *index);
            if !*undefined {
                leb(&mut table, name.len() as u32);
                table.extend_from_slice(name.as_bytes());
            }
        }

        let mut body = Vec::new();
        leb(&mut body, 2); // linking section version
        body.push(8); // WASM_SYMBOL_TABLE
        leb(&mut body, table.len() as u32);
        body.extend_from_slice(&table);

        let mut section = Vec::new();
        leb(&mut section, "linking".len() as u32);
        section.extend_from_slice(b"linking");
        section.extend_from_slice(&body);

        wasm.push(0); // custom section id
        leb(&mut wasm, section.len() as u32);
        wasm.extend_from_slice(&section);
        wasm
    }
}
