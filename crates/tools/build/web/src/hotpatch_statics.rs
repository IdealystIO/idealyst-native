//! Point a wasm patch's statics at the running module's.
//!
//! A patch re-emits whole crates, so it defines each of their statics
//! again — `thread_local!`s included, which on `wasm32-unknown-unknown`
//! are ordinary statics. Left alone, every one gets a copy in the
//! patch's own memory, and the base code still running reads the base's
//! copy while the patched code writes the patch's.
//! `build_runtime_server::hotpatch::statics` decides which statics are
//! the base's; this rewrites the patch's objects, before the link, so
//! they use the base's storage:
//!
//! * the patch's definition of a bound static becomes an UNDEFINED data
//!   symbol. A reference wasm-ld resolves through the GOT — code that
//!   loads `GOT.mem.<name>`, a pointer in data — then imports
//!   `GOT.mem.<name>`, which the import plan already fills with the
//!   base's address (`hotpatch_prepare`).
//! * code that addressed the static relative to the patch's own memory —
//!   `i32.const S@MBREL` (`R_WASM_MEMORY_ADDR_REL_SLEB`), later added to
//!   `__memory_base` — becomes `global.get` of a new imported global
//!   `idealyst.static.<name>$idealyst_host[+addend]`, which the page fills
//!   with "the base's address minus `__memory_base`" ([`STATIC_MODULE`]).
//!   The `__memory_base` addition that follows is untouched and lands on
//!   the base's static. Both instructions are an opcode and a 5-byte
//!   padded LEB, so the swap is one byte and nothing moves; the address
//!   of the patch's own memory is only known to the page, which is why
//!   the value is supplied there rather than computed here.
//!
//! What is bound: a static in a writable data segment (`.data.*` other
//! than `.data.rel.ro.*`, or `.bss.*`). An immutable static keeps a
//! fresh copy per patch — `stylesheet!`'s cache key relies on that (see
//! the pairing module). A thread-local segment of a threaded build would
//! need a `__tls_base`-relative rewrite, so its references refuse the
//! patch; the web build has no threads.
//!
//! The edited objects are written to `out_dir`; the replay's objects are
//! never changed, because the replay cache reuses them for later patches.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use build_runtime_server::hotpatch::statics::{self, ProgramStatics};

/// The import module of a bound static's memory-base-relative address.
/// The field is [`statics::host_name`] of the static; `hotpatch_prepare`
/// resolves it to the base's address plus the addend, and the page
/// subtracts the patch's `__memory_base`.
pub const STATIC_MODULE: &str = "idealyst.static";

/// The static a [`STATIC_MODULE`] import names, and its addend.
pub fn parse_static_import(field: &str) -> Option<(&str, i64)> {
    let at = field.rfind(statics::HOST_SUFFIX)?;
    let name = &field[..at];
    let rest = &field[at + statics::HOST_SUFFIX.len()..];
    let addend = if rest.is_empty() { 0 } else { rest.parse().ok()? };
    Some((name, addend))
}

// Wasm object format constants (`llvm/BinaryFormat/Wasm.h`).
const SECTION_CUSTOM: u8 = 0;
const SECTION_IMPORT: u8 = 2;
const SECTION_GLOBAL: u8 = 6;
const SECTION_CODE: u8 = 10;
const SUBSECTION_SYMBOL_TABLE: u8 = 8;
const SUBSECTION_SEGMENT_INFO: u8 = 5;
const SYMTAB_FUNCTION: u8 = 0;
const SYMTAB_DATA: u8 = 1;
const SYMTAB_GLOBAL: u8 = 2;
const SYMTAB_SECTION: u8 = 3;
const SYMTAB_TAG: u8 = 4;
const SYMTAB_TABLE: u8 = 5;
const SYM_BINDING_LOCAL: u32 = 0x2;
const SYM_VISIBILITY_HIDDEN: u32 = 0x4;
const SYM_UNDEFINED: u32 = 0x10;
const SYM_EXPORTED: u32 = 0x20;
const SYM_EXPLICIT_NAME: u32 = 0x40;
const R_MEMORY_ADDR_I32: u8 = 5;
const R_TYPE_INDEX_LEB: u8 = 6;
const R_GLOBAL_INDEX_LEB: u8 = 7;
const R_MEMORY_ADDR_REL_SLEB: u8 = 11;
const OP_I32_CONST: u8 = 0x41;
const OP_GLOBAL_GET: u8 = 0x23;

/// Whether a relocation type carries an addend.
fn has_addend(ty: u8) -> bool {
    matches!(ty, 3 | 4 | 5 | 8 | 9 | 11 | 14 | 15 | 16 | 17 | 21 | 22 | 23 | 25)
}

/// Whether a data segment of this name holds a static's state. A
/// thread-local one (`.tdata`/`.tbss`, threaded builds only) counts, and
/// its `__tls_base`-relative references then refuse the patch.
pub fn is_state_segment(name: &str) -> bool {
    (name.starts_with(".data") && !name.starts_with(".data.rel.ro"))
        || name.starts_with(".bss")
        || name.starts_with(".tdata")
        || name.starts_with(".tbss")
}

/// Rewrite `objects` so the patch uses the base's statics for every
/// static `program` (the base's data symbols) already has. Returns the
/// objects to link — an edited copy under `out_dir` where something
/// changed, the original elsewhere. An `Err` is a refusal: the dev loop
/// rebuilds.
pub fn bind(objects: &[PathBuf], program: &ProgramStatics, out_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut parsed = Vec::with_capacity(objects.len());
    for path in objects {
        let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        let object = Object::parse(&bytes).with_context(|| format!("parse {}", path.display()))?;
        parsed.push((path, bytes, object));
    }

    // Pair every named static the patch defines — read-only ones too:
    // the base's names cannot be told apart by segment (its link merged
    // them into `.data`/`.rodata`/`.bss`), so both sides are compared
    // whole, and only the writable ones are then bound.
    let mut named: Vec<&str> = Vec::new();
    let mut state: HashSet<&str> = HashSet::new();
    for (_, _, object) in &parsed {
        for sym in &object.symbols {
            let Some(def) = sym.data_def() else { continue };
            if rustc_demangle_ok(&sym.name) {
                named.push(&sym.name);
                let segment = object.segments.get(def.segment as usize).map(String::as_str).unwrap_or("");
                if is_state_segment(segment) {
                    state.insert(&sym.name);
                }
            }
        }
    }
    let paired = statics::pair(named.iter().copied(), program).map_err(|e| anyhow::anyhow!("{e}"))?;
    let bound: HashSet<&str> = paired.iter().map(String::as_str).filter(|n| state.contains(n)).collect();
    if bound.is_empty() {
        return Ok(objects.to_vec());
    }

    let mut out = Vec::with_capacity(parsed.len());
    for (path, bytes, object) in &parsed {
        match rewrite(bytes, object, &bound).with_context(|| format!("binding statics in {}", path.display()))? {
            None => out.push((*path).clone()),
            Some(edited) => {
                std::fs::create_dir_all(out_dir).with_context(|| format!("create {}", out_dir.display()))?;
                let file = out_dir.join(path.file_name().context("object path has no file name")?);
                std::fs::write(&file, edited).with_context(|| format!("write {}", file.display()))?;
                out.push(file);
            }
        }
    }
    Ok(out)
}

fn rustc_demangle_ok(name: &str) -> bool {
    statics::static_key(name) != name
}

/// Apply the binding to one object. `None` when nothing in it names a
/// bound static.
fn rewrite(bytes: &[u8], object: &Object, bound: &HashSet<&str>) -> Result<Option<Vec<u8>>> {
    let targets: HashMap<u32, &str> = object
        .symbols
        .iter()
        .enumerate()
        .filter(|(_, s)| s.kind == SYMTAB_DATA && bound.contains(s.name.as_str()))
        .map(|(i, s)| (i as u32, s.name.as_str()))
        .collect();
    if targets.is_empty() {
        return Ok(None);
    }
    let code = object.code.clone().map(|r| &bytes[r]);

    // New global imports, one per (static, addend) an `i32.const` used.
    let mut new_imports: Vec<String> = Vec::new();
    let mut new_symbol: HashMap<String, u32> = HashMap::new();
    let first_symbol = object.symbols.len() as u32;
    let mut patched_code: Vec<(usize, u8)> = Vec::new();
    let mut new_relocs: BTreeMap<usize, Vec<u8>> = BTreeMap::new();

    for (&section_index, payload) in &object.reloc_sections {
        let reloc = parse_relocs(&bytes[payload.clone()])?;
        let target_section = object.sections.get(reloc.target as usize);
        let in_code = target_section.is_some_and(|s| s.id == SECTION_CODE);
        let mut changed = false;
        let mut entries = Vec::with_capacity(reloc.entries.len());
        for entry in &reloc.entries {
            // `R_WASM_TYPE_INDEX_LEB`'s index is a type, not a symbol.
            let Some(name) = targets.get(&entry.index).filter(|_| entry.ty != R_TYPE_INDEX_LEB) else {
                entries.push(entry.clone());
                continue;
            };
            match entry.ty {
                // Loads the GOT entry, which now imports the base's address.
                R_GLOBAL_INDEX_LEB => entries.push(entry.clone()),
                // A pointer in data: wasm-ld turns it into a GOT-relative
                // store in `__wasm_apply_data_relocs`.
                R_MEMORY_ADDR_I32 if !in_code => entries.push(entry.clone()),
                R_MEMORY_ADDR_REL_SLEB if in_code => {
                    let code = code.context("a code relocation with no code section")?;
                    let opcode_at = (entry.offset as usize)
                        .checked_sub(1)
                        .filter(|&at| code.get(at) == Some(&OP_I32_CONST))
                        .with_context(|| {
                            format!("`{name}`'s memory-relative address is not an `i32.const` immediate")
                        })?;
                    patched_code.push((opcode_at, OP_GLOBAL_GET));
                    let field = statics::host_name(name, entry.addend);
                    let symbol = *new_symbol.entry(field.clone()).or_insert_with(|| {
                        new_imports.push(field);
                        first_symbol + new_imports.len() as u32 - 1
                    });
                    entries.push(Reloc { ty: R_GLOBAL_INDEX_LEB, offset: entry.offset, index: symbol, addend: 0 });
                    changed = true;
                }
                // Debug info may keep pointing wherever wasm-ld leaves it.
                _ if target_section.is_some_and(|s| s.custom_name.as_deref().is_some_and(|n| n.starts_with(".debug"))) => {
                    entries.push(entry.clone())
                }
                other => bail!("`{name}` is the target of relocation type {other}, which cannot be bound"),
            }
        }
        if changed {
            new_relocs.insert(section_index, encode_reloc_section(reloc.target, &entries));
        }
    }

    // The definitions go: each bound static is now undefined here.
    let mut symtab = Vec::new();
    write_uleb(&mut symtab, (object.symbols.len() + new_imports.len()) as u64);
    for (i, sym) in object.symbols.iter().enumerate() {
        if targets.contains_key(&(i as u32)) && sym.data_def().is_some() {
            let flags = (sym.flags & !(SYM_BINDING_LOCAL | SYM_VISIBILITY_HIDDEN | SYM_EXPORTED)) | SYM_UNDEFINED;
            symtab.push(SYMTAB_DATA);
            write_uleb(&mut symtab, flags as u64);
            write_name(&mut symtab, &sym.name);
        } else {
            symtab.extend_from_slice(&sym.raw);
        }
    }
    let global_imports = object.global_imports;
    if object.defined_globals > 0 && !new_imports.is_empty() {
        bail!("the object defines globals of its own, which a new global import would renumber");
    }
    for (k, _) in new_imports.iter().enumerate() {
        symtab.push(SYMTAB_GLOBAL);
        write_uleb(&mut symtab, SYM_UNDEFINED as u64);
        write_uleb(&mut symtab, (global_imports + k as u32) as u64);
    }

    // Reassemble: every section verbatim but the ones edited.
    let mut out = bytes[..8].to_vec();
    for (index, section) in object.sections.iter().enumerate() {
        let payload: Vec<u8> = if section.id == SECTION_IMPORT && !new_imports.is_empty() {
            let mut p = Vec::new();
            write_uleb(&mut p, (object.import_count + new_imports.len() as u32) as u64);
            p.extend_from_slice(&bytes[section.payload.clone()][object.import_entries_at..]);
            for field in &new_imports {
                write_name(&mut p, STATIC_MODULE);
                write_name(&mut p, field);
                p.push(0x03); // global
                p.push(0x7f); // i32
                p.push(0x00); // immutable
            }
            p
        } else if section.id == SECTION_CODE && !patched_code.is_empty() {
            let mut p = bytes[section.payload.clone()].to_vec();
            for &(at, op) in &patched_code {
                p[at] = op;
            }
            p
        } else if section.custom_name.as_deref() == Some("linking") {
            let mut p = Vec::new();
            write_name(&mut p, "linking");
            write_uleb(&mut p, object.linking_version as u64);
            for (ty, raw) in &object.linking_subsections {
                p.push(*ty);
                let body = if *ty == SUBSECTION_SYMBOL_TABLE { &symtab } else { raw };
                write_uleb(&mut p, body.len() as u64);
                p.extend_from_slice(body);
            }
            p
        } else if let Some(reloc) = new_relocs.get(&index) {
            let mut p = Vec::new();
            write_name(&mut p, section.custom_name.as_deref().unwrap_or(""));
            p.extend_from_slice(reloc);
            p
        } else {
            out.push(section.id);
            write_uleb(&mut out, section.raw.len() as u64);
            out.extend_from_slice(&bytes[section.raw.clone()]);
            continue;
        };
        out.push(section.id);
        write_uleb(&mut out, payload.len() as u64);
        out.extend_from_slice(&payload);
    }
    Ok(Some(out))
}

fn encode_reloc_section(target: u32, entries: &[Reloc]) -> Vec<u8> {
    let mut p = Vec::new();
    write_uleb(&mut p, target as u64);
    write_uleb(&mut p, entries.len() as u64);
    for e in entries {
        p.push(e.ty);
        write_uleb(&mut p, e.offset as u64);
        write_uleb(&mut p, e.index as u64);
        if has_addend(e.ty) {
            write_sleb(&mut p, e.addend);
        }
    }
    p
}

// ── reading ─────────────────────────────────────────────────────────────

struct Section {
    id: u8,
    /// The custom section's name, for a custom section.
    custom_name: Option<String>,
    /// Where the payload after a custom section's name (all of it
    /// otherwise) sits in the file.
    payload: Range<usize>,
    /// Where the whole payload, name included, sits in the file.
    raw: Range<usize>,
}

#[derive(Clone)]
struct Reloc {
    ty: u8,
    offset: u32,
    index: u32,
    addend: i64,
}

struct RelocSection {
    /// Index of the section the relocations apply to.
    target: u32,
    entries: Vec<Reloc>,
}

struct Symbol {
    kind: u8,
    flags: u32,
    name: String,
    /// `(segment, offset, size)` of a defined data symbol.
    data: Option<DataDef>,
    /// The entry's bytes in the symbol table, for verbatim re-emission.
    raw: Vec<u8>,
}

#[derive(Clone, Copy)]
struct DataDef {
    segment: u32,
}

impl Symbol {
    fn data_def(&self) -> Option<DataDef> {
        if self.kind == SYMTAB_DATA { self.data } else { None }
    }
}

struct Object {
    sections: Vec<Section>,
    /// The code section's payload in the file.
    code: Option<Range<usize>>,
    import_count: u32,
    /// Offset in the import section's payload where the entries start.
    import_entries_at: usize,
    global_imports: u32,
    defined_globals: u32,
    linking_version: u32,
    linking_subsections: Vec<(u8, Vec<u8>)>,
    symbols: Vec<Symbol>,
    segments: Vec<String>,
    /// File section index → where its relocation payload sits; parsed
    /// only for an object that names a bound static.
    reloc_sections: BTreeMap<usize, Range<usize>>,
}

impl Object {
    fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 8 || &bytes[..4] != b"\0asm" {
            bail!("not a wasm module");
        }
        let mut r = Reader { b: bytes, at: 8 };
        let mut sections = Vec::new();
        while r.at < bytes.len() {
            let id = r.u8()?;
            let len = r.uleb()? as usize;
            let start = r.at;
            let raw = r.take(len)?;
            let (custom_name, payload) = if id == SECTION_CUSTOM {
                let mut pr = Reader { b: raw, at: 0 };
                let name = pr.name()?;
                (Some(name), start + pr.at..start + len)
            } else {
                (None, start..start + len)
            };
            sections.push(Section { id, custom_name, payload, raw: start..start + len });
        }

        let mut object = Object {
            sections: Vec::new(),
            code: None,
            import_count: 0,
            import_entries_at: 0,
            global_imports: 0,
            defined_globals: 0,
            linking_version: 0,
            linking_subsections: Vec::new(),
            symbols: Vec::new(),
            segments: Vec::new(),
            reloc_sections: BTreeMap::new(),
        };
        for (index, s) in sections.iter().enumerate() {
            let payload = &bytes[s.payload.clone()];
            match (s.id, s.custom_name.as_deref()) {
                (SECTION_IMPORT, _) => {
                    let mut r = Reader { b: payload, at: 0 };
                    object.import_count = r.uleb()? as u32;
                    object.import_entries_at = r.at;
                    for _ in 0..object.import_count {
                        r.name()?;
                        r.name()?;
                        match r.u8()? {
                            0x00 => {
                                r.uleb()?;
                            }
                            0x01 => {
                                r.u8()?;
                                r.limits()?;
                            }
                            0x02 => r.limits()?,
                            0x03 => {
                                r.u8()?;
                                r.u8()?;
                                object.global_imports += 1;
                            }
                            0x04 => {
                                r.u8()?;
                                r.uleb()?;
                            }
                            other => bail!("unknown import kind {other}"),
                        }
                    }
                }
                (SECTION_GLOBAL, _) => {
                    object.defined_globals = Reader { b: payload, at: 0 }.uleb()? as u32;
                }
                (SECTION_CODE, _) => object.code = Some(s.payload.clone()),
                (SECTION_CUSTOM, Some("linking")) => {
                    let mut r = Reader { b: payload, at: 0 };
                    object.linking_version = r.uleb()? as u32;
                    while r.at < payload.len() {
                        let ty = r.u8()?;
                        let len = r.uleb()? as usize;
                        let body = r.take(len)?.to_vec();
                        match ty {
                            SUBSECTION_SYMBOL_TABLE => object.symbols = parse_symbols(&body)?,
                            SUBSECTION_SEGMENT_INFO => {
                                let mut sr = Reader { b: &body, at: 0 };
                                let n = sr.uleb()?;
                                for _ in 0..n {
                                    object.segments.push(sr.name()?);
                                    sr.uleb()?; // alignment
                                    sr.uleb()?; // flags
                                }
                            }
                            _ => {}
                        }
                        object.linking_subsections.push((ty, body));
                    }
                }
                (SECTION_CUSTOM, Some(name)) if name.starts_with("reloc.") => {
                    object.reloc_sections.insert(index, s.payload.clone());
                }
                _ => {}
            }
        }
        object.sections = sections;
        Ok(object)
    }
}

fn parse_relocs(payload: &[u8]) -> Result<RelocSection> {
    let mut r = Reader { b: payload, at: 0 };
    let target = r.uleb()? as u32;
    let n = r.uleb()?;
    let mut entries = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let ty = r.u8()?;
        let offset = r.uleb()? as u32;
        let index = r.uleb()? as u32;
        let addend = if has_addend(ty) { r.sleb()? } else { 0 };
        entries.push(Reloc { ty, offset, index, addend });
    }
    Ok(RelocSection { target, entries })
}

fn parse_symbols(body: &[u8]) -> Result<Vec<Symbol>> {
    let mut r = Reader { b: body, at: 0 };
    let n = r.uleb()?;
    let mut out = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let start = r.at;
        let kind = r.u8()?;
        let flags = r.uleb()? as u32;
        let undefined = flags & SYM_UNDEFINED != 0;
        let mut name = String::new();
        let mut data = None;
        match kind {
            SYMTAB_FUNCTION | SYMTAB_GLOBAL | SYMTAB_TAG | SYMTAB_TABLE => {
                r.uleb()?;
                if !undefined || flags & SYM_EXPLICIT_NAME != 0 {
                    name = r.name()?;
                }
            }
            SYMTAB_DATA => {
                name = r.name()?;
                if !undefined {
                    let segment = r.uleb()? as u32;
                    r.uleb()?; // offset
                    r.uleb()?; // size
                    data = Some(DataDef { segment });
                }
            }
            SYMTAB_SECTION => {
                r.uleb()?;
            }
            other => bail!("unknown symbol kind {other}"),
        }
        out.push(Symbol { kind, flags, name, data, raw: body[start..r.at].to_vec() });
    }
    Ok(out)
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn u8(&mut self) -> Result<u8> {
        let v = *self.b.get(self.at).context("truncated")?;
        self.at += 1;
        Ok(v)
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let s = self.b.get(self.at..self.at + n).context("truncated")?;
        self.at += n;
        Ok(s)
    }
    fn uleb(&mut self) -> Result<u64> {
        let mut v = 0u64;
        let mut shift = 0;
        loop {
            let byte = self.u8()?;
            v |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(v);
            }
            shift += 7;
            if shift > 63 {
                bail!("LEB too long");
            }
        }
    }
    fn sleb(&mut self) -> Result<i64> {
        let mut v = 0i64;
        let mut shift = 0;
        loop {
            let byte = self.u8()?;
            v |= i64::from(byte & 0x7f) << shift;
            shift += 7;
            if byte & 0x80 == 0 {
                if shift < 64 && byte & 0x40 != 0 {
                    v |= -1i64 << shift;
                }
                return Ok(v);
            }
            if shift > 63 {
                bail!("LEB too long");
            }
        }
    }
    fn name(&mut self) -> Result<String> {
        let n = self.uleb()? as usize;
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
    fn limits(&mut self) -> Result<()> {
        let flags = self.u8()?;
        self.uleb()?;
        if flags & 0x1 != 0 {
            self.uleb()?;
        }
        Ok(())
    }
}

fn write_uleb(out: &mut Vec<u8>, mut v: u64) {
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

fn write_sleb(out: &mut Vec<u8>, mut v: i64) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        let done = (v == 0 && byte & 0x40 == 0) || (v == -1 && byte & 0x40 != 0);
        out.push(if done { byte } else { byte | 0x80 });
        if done {
            return;
        }
    }
}

fn write_name(out: &mut Vec<u8>, name: &str) {
    write_uleb(out, name.len() as u64);
    out.extend_from_slice(name.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writable_segments_are_state_and_read_only_ones_are_not() {
        assert!(is_state_segment(".data._RNvCs_5probe7COUNTER"));
        assert!(is_state_segment(".bss._RNvCs_5probe7SESSION"));
        // Immutable statics keep a fresh copy per patch; `stylesheet!`'s
        // cache key is one.
        assert!(!is_state_segment(".rodata._RNvCs_5probe3KEY"));
        assert!(!is_state_segment(".data.rel.ro._RNvCs_5probe5TABLE"));
    }

    #[test]
    fn a_static_import_names_its_static_and_addend() {
        assert_eq!(parse_static_import("_RNv5probe1S$idealyst_host"), Some(("_RNv5probe1S", 0)));
        assert_eq!(parse_static_import("_RNv5probe1S$idealyst_host+8"), Some(("_RNv5probe1S", 8)));
        assert_eq!(parse_static_import("_RNv5probe1S$idealyst_host-4"), Some(("_RNv5probe1S", -4)));
        assert_eq!(parse_static_import("_RNv5probe1S"), None);
    }

    #[test]
    fn leb_round_trips() {
        for v in [0i64, 1, 63, 64, -1, -64, -65, 1 << 20, -(1 << 20)] {
            let mut b = Vec::new();
            write_sleb(&mut b, v);
            assert_eq!(Reader { b: &b, at: 0 }.sleb().unwrap(), v);
        }
        for v in [0u64, 127, 128, 1 << 30] {
            let mut b = Vec::new();
            write_uleb(&mut b, v);
            assert_eq!(Reader { b: &b, at: 0 }.uleb().unwrap(), v);
        }
    }
}
