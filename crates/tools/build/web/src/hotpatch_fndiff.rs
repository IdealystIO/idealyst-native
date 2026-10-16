//! Which functions of a patch differ from the base, and which of them
//! the jump table has to redirect.
//!
//! A patch re-emits whole crates. Unfiltered, every function of theirs
//! whose address the patch takes gets a jump-table entry: 32,195 for a
//! one-line edit in a crate ~30 others depend on (CrewForge), where a
//! leaf crate's edit redirected 2,170. Linking, resolving and stripping
//! all of that, and installing the table on the page, cost ~1.6 s of a
//! 3.6 s save. Almost none of it changed.
//!
//! # What "unchanged" can be compared against
//!
//! Not the base module: the base is compiled for a fixed address space
//! and a patch as position-independent code, so the same source compiles
//! to different instructions. What CAN be compared is a patch replay of
//! the crate against a replay of its BASE sources, with the same flags —
//! which is exactly what the background seed (`hotpatch_build`'s
//! `SeedQueue`) produces after every base build. Its objects are digested
//! per function (a [`CrateFns`]) and kept as the crate's baseline.
//!
//! A function's digest covers its type and its body bytes, with every
//! relocation site replaced by what it refers to: a function, global or
//! table by symbol name, a call_indirect type by its signature, and a
//! data symbol by its name, its bytes and the relocations inside them
//! (string literals are anonymous symbols named by position, so a name
//! alone would equate `"a"` with `"b"`). Two functions with one digest
//! compile to the same code against the same symbols.
//!
//! # Why an unchanged function may still need redirecting
//!
//! The table only redirects calls made THROUGH it. A direct call in base
//! code goes to the base's copy. So an unchanged function that calls a
//! changed one directly — or calls something that does — must be
//! redirected too, or its base copy keeps calling the old body. The set
//! to redirect is the changed functions plus every function that reaches
//! one through direct calls ([`plan`]). Calls through the table or a
//! vtable reach the redirected entry anyway and add nothing.
//!
//! Anything this cannot account for turns the filter off for that patch
//! (every function redirected): a crate with no baseline, an
//! object it cannot parse, a relocation kind it does not know.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use anyhow::{bail, Context, Result};
use sha2::{Digest as _, Sha256};
use wasmparser::{Parser, Payload, RelocationType, SymbolFlags, SymbolInfo};

/// One defined function of a crate's objects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnFacts {
    /// See the module docs.
    pub digest: [u8; 16],
    /// Functions it calls directly, by symbol name.
    pub calls: Vec<String>,
    /// Functions whose address (table slot) it takes, by symbol name.
    pub addr_of: Vec<String>,
}

/// Every defined function of one crate's objects, by symbol name, and
/// the data symbols that take a function's address (a vtable, a `static`
/// holding a `fn` pointer): `(data symbol, function)`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CrateFns {
    pub fns: HashMap<String, FnFacts>,
    pub data_addr_of: Vec<(String, String)>,
    /// What each object file defines and needs, for [`keep_objects`].
    pub objects: Vec<ObjectFacts>,
}

/// One object file of a crate's replay, as the link sees it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectFacts {
    pub path: std::path::PathBuf,
    /// Every function and data symbol it defines, local ones included.
    pub defines: Vec<String>,
    /// Every function and data symbol it references but does not define:
    /// what the link resolves against the other objects (or, failing
    /// them, imports from the base).
    pub refs: Vec<String>,
    /// Whether it declares constructors, which the page runs when it
    /// loads the patch.
    pub init: bool,
}

impl CrateFns {
    /// Digest every object of a crate's replay.
    pub fn of_objects(objects: &[impl AsRef<Path>]) -> Result<Self> {
        let mut out = CrateFns::default();
        for path in objects {
            let path = path.as_ref();
            let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
            let mut one = analyze_object(&bytes).with_context(|| format!("digesting {}", path.display()))?;
            for o in &mut one.objects {
                o.path = path.to_path_buf();
            }
            out.merge(one);
        }
        Ok(out)
    }

    fn merge(&mut self, other: CrateFns) {
        for (name, facts) in other.fns {
            match self.fns.get_mut(&name) {
                // One symbol defined in two objects (a COMDAT copy). Equal
                // copies are one function; unequal ones get a digest of
                // both, so the pair matches only the same pair again.
                Some(existing) if existing.digest != facts.digest => {
                    let (a, b) = if existing.digest < facts.digest {
                        (existing.digest, facts.digest)
                    } else {
                        (facts.digest, existing.digest)
                    };
                    let mut h = Sha256::new();
                    h.update(a);
                    h.update(b);
                    existing.digest = truncate(h.finalize().into());
                    existing.calls.extend(facts.calls);
                    existing.addr_of.extend(facts.addr_of);
                }
                Some(_) => {}
                None => {
                    self.fns.insert(name, facts);
                }
            }
        }
        self.data_addr_of.extend(other.data_addr_of);
        self.objects.extend(other.objects);
    }
}

/// The objects a trimmed patch links: those defining a function the plan
/// redirects through a slot or something that takes its address
/// ([`RedirectPlan::roots`]), those declaring constructors, and every
/// object defining a symbol a kept one references — transitively. Each
/// kept object is linked whole, as before trimming existed.
///
/// Whole objects rather than a garbage-collected link, for two reasons
/// that both showed on a real app: a GC root has to be EXPORTED, and the
/// linker only exports global symbols, so a local address-taker (an
/// anonymous vtable, a private fn) was dropped and took its target's
/// table slot with it; and dropped code would drop constructors the page
/// runs. A reference across objects is always by a global name, so
/// keeping every definer of every referenced name loses nothing a kept
/// object needs.
pub fn keep_objects(plan: &RedirectPlan, crates: &[&CrateFns]) -> Vec<std::path::PathBuf> {
    let all: Vec<&ObjectFacts> = crates.iter().flat_map(|c| c.objects.iter()).collect();
    let mut definers: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, o) in all.iter().enumerate() {
        for d in &o.defines {
            definers.entry(d.as_str()).or_default().push(i);
        }
    }
    let mut kept = vec![false; all.len()];
    let mut stack: Vec<usize> = all
        .iter()
        .enumerate()
        .filter(|(_, o)| o.init || o.defines.iter().any(|d| plan.roots.contains(d)))
        .map(|(i, _)| i)
        .collect();
    while let Some(i) = stack.pop() {
        if std::mem::replace(&mut kept[i], true) {
            continue;
        }
        for r in &all[i].refs {
            stack.extend(definers.get(r.as_str()).into_iter().flatten().copied().filter(|j| !kept[*j]));
        }
    }
    all.iter().zip(kept).filter(|(_, k)| *k).map(|(o, _)| o.path.clone()).collect()
}

/// What the jump table redirects, and what the patch link keeps.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RedirectPlan {
    /// Functions that differ from the base, or reach one by direct calls.
    pub redirect: BTreeSet<String>,
    /// [`Self::slotted`], plus whatever takes the address of one of them
    /// — a function, or a data symbol: the objects defining these are the
    /// ones a trimmed link starts from ([`keep_objects`]), so each of them
    /// keeps its table slot.
    pub roots: BTreeSet<String>,
    /// The members of `redirect` whose address something in the patch's
    /// objects takes: the ones the patch gives a table slot, so the only
    /// ones a jump table can redirect. (The base's table holds every
    /// function; a patch's only those it points at.) After a trimmed link
    /// each of these must still have its slot.
    pub slotted: BTreeSet<String>,
    /// How many functions differ from the base.
    pub changed: usize,
}

/// The plan for a patch of these crates: each crate's facts now, beside
/// its baseline. `None` when any crate has no baseline — nothing can be
/// proven unchanged there, so every function is redirected.
pub fn plan(crates: &[(&CrateFns, Option<&CrateFns>)]) -> Option<RedirectPlan> {
    let mut changed: HashSet<&str> = HashSet::new();
    for (now, baseline) in crates {
        let baseline = (*baseline)?;
        for (name, facts) in &now.fns {
            if baseline.fns.get(name).map(|b| b.digest) != Some(facts.digest) {
                changed.insert(name.as_str());
            }
        }
    }

    // Reverse direct-call edges across every crate of the patch: a
    // dependent's function calls a library's directly, by name.
    let mut callers: HashMap<&str, Vec<&str>> = HashMap::new();
    for (now, _) in crates {
        for (name, facts) in &now.fns {
            for callee in &facts.calls {
                callers.entry(callee.as_str()).or_default().push(name.as_str());
            }
        }
    }
    let mut redirect: BTreeSet<String> = BTreeSet::new();
    let mut stack: Vec<&str> = changed.iter().copied().collect();
    while let Some(f) = stack.pop() {
        if redirect.insert(f.to_string()) {
            stack.extend(callers.get(f).into_iter().flatten().copied());
        }
    }

    let mut slotted = BTreeSet::new();
    for (now, _) in crates {
        for facts in now.fns.values() {
            slotted.extend(facts.addr_of.iter().filter(|t| redirect.contains(*t)).cloned());
        }
        slotted.extend(now.data_addr_of.iter().filter(|(_, f)| redirect.contains(f)).map(|(_, f)| f.clone()));
    }
    // Only a slotted function can be redirected; the rest of `redirect`
    // is in the patch only as something a slotted one calls, which the
    // link keeps by itself.
    let mut roots = slotted.clone();
    for (now, _) in crates {
        for (name, facts) in &now.fns {
            if facts.addr_of.iter().any(|t| redirect.contains(t)) {
                roots.insert(name.clone());
            }
        }
        for (data, f) in &now.data_addr_of {
            if redirect.contains(f) {
                roots.insert(data.clone());
            }
        }
    }
    Some(RedirectPlan { redirect, roots, slotted, changed: changed.len() })
}

fn truncate(d: [u8; 32]) -> [u8; 16] {
    let mut out = [0u8; 16];
    out.copy_from_slice(&d[..16]);
    out
}

/// A symbol of the object's `linking` table, owned.
#[derive(Debug, Clone)]
enum Sym {
    Func { name: Option<String>, index: u32, undefined: bool },
    Data { name: String, def: Option<(u32, u32, u32)> },
    Global { name: Option<String>, index: u32 },
    /// A section, event or table symbol: identified by its debug text.
    #[allow(dead_code)]
    Other(String),
}

/// One object's facts. See the module docs for what a digest covers.
pub fn analyze_object(bytes: &[u8]) -> Result<CrateFns> {
    let mut types: Vec<String> = Vec::new();
    let mut func_imports: Vec<String> = Vec::new();
    let mut global_imports: Vec<String> = Vec::new();
    let mut func_types: Vec<u32> = Vec::new();
    let mut code_start: Option<usize> = None;
    let mut bodies: Vec<std::ops::Range<usize>> = Vec::new();
    let mut data_start: Option<usize> = None;
    // Absolute byte range of each data segment's contents.
    let mut segments: Vec<std::ops::Range<usize>> = Vec::new();
    let mut symbols: Vec<Sym> = Vec::new();
    let mut relocs: Vec<(u32, Vec<wasmparser::RelocationEntry>)> = Vec::new();
    let mut section_index = 0u32;
    let mut code_section = None;
    let mut data_section = None;
    let mut has_init = false;

    for payload in Parser::new(0).parse_all(bytes) {
        let payload = payload?;
        let is_section = !matches!(
            payload,
            Payload::Version { .. } | Payload::End(_) | Payload::CodeSectionEntry(_)
        );
        match payload {
            Payload::TypeSection(r) => {
                for group in r {
                    for ty in group?.into_types() {
                        types.push(format!("{:?}", ty.composite_type));
                    }
                }
            }
            Payload::ImportSection(r) => {
                for import in r {
                    let import = import?;
                    match import.ty {
                        wasmparser::TypeRef::Func(_) => func_imports.push(import.name.to_string()),
                        wasmparser::TypeRef::Global(_) => global_imports.push(import.name.to_string()),
                        _ => {}
                    }
                }
            }
            Payload::FunctionSection(r) => {
                for t in r {
                    func_types.push(t?);
                }
            }
            Payload::CodeSectionStart { range, .. } => {
                code_start = Some(range.start);
                code_section = Some(section_index);
            }
            Payload::CodeSectionEntry(body) => bodies.push(body.range()),
            Payload::DataSection(r) => {
                data_start = Some(r.range().start);
                data_section = Some(section_index);
                for d in r {
                    let d = d?;
                    segments.push(d.range.end - d.data.len()..d.range.end);
                }
            }
            Payload::CustomSection(c) if c.name() == "linking" => {
                let reader = wasmparser::LinkingSectionReader::new(wasmparser::BinaryReader::new(
                    c.data(),
                    c.data_offset(),
                ))?;
                for sub in reader.subsections() {
                    let table = match sub? {
                        wasmparser::Linking::SymbolTable(table) => table,
                        wasmparser::Linking::InitFuncs(funcs) => {
                            has_init |= funcs.into_iter().next().is_some();
                            continue;
                        }
                        _ => continue,
                    };
                    for s in table {
                        symbols.push(match s? {
                            SymbolInfo::Func { flags, index, name } => Sym::Func {
                                name: name.map(str::to_string),
                                index,
                                undefined: flags.contains(SymbolFlags::UNDEFINED),
                            },
                            SymbolInfo::Data { name, symbol, .. } => Sym::Data {
                                name: name.to_string(),
                                def: symbol.map(|d| (d.index, d.offset, d.size)),
                            },
                            SymbolInfo::Global { index, name, .. } => {
                                Sym::Global { name: name.map(str::to_string), index }
                            }
                            other => Sym::Other(format!("{other:?}")),
                        });
                    }
                }
            }
            Payload::CustomSection(c) if c.name().starts_with("reloc.") => {
                let reader = wasmparser::RelocSectionReader::new(wasmparser::BinaryReader::new(
                    c.data(),
                    c.data_offset(),
                ))?;
                let entries: Vec<_> = reader.entries().into_iter().collect::<Result<_, _>>()?;
                relocs.push((reader.section_index(), entries));
            }
            _ => {}
        }
        if is_section {
            section_index += 1;
        }
    }
    if symbols.is_empty() && !bodies.is_empty() {
        bail!("no `linking` symbol table: not a relocatable object");
    }

    let func_name = |sym: &Sym| -> Option<String> {
        match sym {
            Sym::Func { name: Some(n), .. } => Some(n.clone()),
            Sym::Func { name: None, index, .. } => func_imports.get(*index as usize).cloned(),
            _ => None,
        }
    };
    let code_relocs: Vec<wasmparser::RelocationEntry> = relocs
        .iter()
        .filter(|(s, _)| Some(*s) == code_section)
        .flat_map(|(_, e)| e.iter().copied())
        .collect();
    let data_relocs: Vec<wasmparser::RelocationEntry> = relocs
        .iter()
        .filter(|(s, _)| Some(*s) == data_section)
        .flat_map(|(_, e)| e.iter().copied())
        .collect();

    // A data symbol's identity: its name, bytes and the relocations in
    // them. Memoized; a cycle (two statics pointing at each other) falls
    // back to the name, which still differs when the name does.
    let mut data_ids: HashMap<u32, Vec<u8>> = HashMap::new();
    fn data_identity(
        index: u32,
        symbols: &[Sym],
        segments: &[std::ops::Range<usize>],
        data_start: Option<usize>,
        data_relocs: &[wasmparser::RelocationEntry],
        bytes: &[u8],
        memo: &mut HashMap<u32, Vec<u8>>,
        visiting: &mut HashSet<u32>,
    ) -> Vec<u8> {
        if let Some(id) = memo.get(&index) {
            return id.clone();
        }
        let Some(Sym::Data { name, def }) = symbols.get(index as usize) else {
            return b"?data".to_vec();
        };
        let mut h = Sha256::new();
        h.update(name.as_bytes());
        if let (Some((seg, off, size)), Some(start)) = (def, data_start) {
            if visiting.insert(index) {
                if let Some(range) = segments.get(*seg as usize) {
                    let lo = range.start + *off as usize;
                    let hi = (lo + *size as usize).min(range.end);
                    let mut cursor = lo;
                    let mut inside: Vec<_> = data_relocs
                        .iter()
                        .filter(|r| {
                            let at = start + r.offset as usize;
                            at >= lo && at < hi
                        })
                        .collect();
                    inside.sort_by_key(|r| r.offset);
                    for r in inside {
                        let at = start + r.offset as usize;
                        h.update(&bytes[cursor..at]);
                        h.update([0xff, r.ty as u8]);
                        let target = match symbols.get(r.index as usize) {
                            Some(Sym::Data { .. }) => data_identity(
                                r.index, symbols, segments, data_start, data_relocs, bytes, memo, visiting,
                            ),
                            Some(Sym::Func { name, .. }) => name.clone().unwrap_or_default().into_bytes(),
                            Some(other) => format!("{other:?}").into_bytes(),
                            None => b"?".to_vec(),
                        };
                        h.update(&target);
                        h.update(r.addend.to_le_bytes());
                        cursor = (at + r.ty.extent()).min(hi);
                    }
                    h.update(&bytes[cursor.min(hi)..hi]);
                }
                visiting.remove(&index);
            }
        }
        let id = h.finalize()[..16].to_vec();
        memo.insert(index, id.clone());
        id
    }

    let mut out = CrateFns::default();
    let mut facts = ObjectFacts { init: has_init, ..ObjectFacts::default() };
    for s in &symbols {
        match s {
            Sym::Func { name: Some(n), undefined: false, .. } => facts.defines.push(n.clone()),
            Sym::Func { undefined: true, .. } => {
                if let Some(n) = func_name(s) {
                    facts.refs.push(n);
                }
            }
            Sym::Data { name, def: Some(_) } => facts.defines.push(name.clone()),
            Sym::Data { name, def: None } => facts.refs.push(name.clone()),
            _ => {}
        }
    }
    out.objects.push(facts);
    let Some(code_start) = code_start else { return Ok(out) };
    let mut sorted = code_relocs.clone();
    sorted.sort_by_key(|r| r.offset);
    let mut next = 0usize;
    let mut visiting = HashSet::new();

    // Defined function index -> its symbol name.
    let mut defined: HashMap<u32, String> = HashMap::new();
    for s in &symbols {
        if let Sym::Func { name: Some(n), index, undefined: false } = s {
            defined.entry(*index).or_insert_with(|| n.clone());
        }
    }

    for (i, body) in bodies.iter().enumerate() {
        let index = (func_imports.len() + i) as u32;
        let ty = func_types.get(i).and_then(|t| types.get(*t as usize)).cloned().unwrap_or_default();
        let mut h = Sha256::new();
        h.update(ty.as_bytes());
        h.update([0]);
        let mut calls = Vec::new();
        let mut addr_of = Vec::new();
        let mut cursor = body.start;
        while next < sorted.len() && code_start + (sorted[next].offset as usize) < body.end {
            let r = sorted[next];
            next += 1;
            let at = code_start + r.offset as usize;
            if at < body.start {
                continue;
            }
            h.update(&bytes[cursor..at]);
            h.update([0xff, r.ty as u8]);
            let sym = symbols.get(r.index as usize);
            use RelocationType::*;
            match r.ty {
                TypeIndexLeb => {
                    h.update(types.get(r.index as usize).map(String::as_bytes).unwrap_or(b"?type"));
                }
                FunctionIndexLeb | FunctionIndexI32 => {
                    let name = sym.and_then(func_name).unwrap_or_default();
                    h.update(name.as_bytes());
                    calls.push(name);
                }
                TableIndexSleb | TableIndexI32 | TableIndexRelSleb | TableIndexSleb64
                | TableIndexI64 | TableIndexRelSleb64 => {
                    let name = sym.and_then(func_name).unwrap_or_default();
                    h.update(name.as_bytes());
                    addr_of.push(name);
                }
                GlobalIndexLeb | GlobalIndexI32 => match sym {
                    // A GOT entry for a function: its address.
                    Some(s @ Sym::Func { .. }) => {
                        let name = func_name(s).unwrap_or_default();
                        h.update(b"got.func");
                        h.update(name.as_bytes());
                        addr_of.push(name);
                    }
                    Some(Sym::Data { .. }) => {
                        h.update(b"got.mem");
                        let id = data_identity(
                            r.index, &symbols, &segments, data_start, &data_relocs, bytes, &mut data_ids,
                            &mut visiting,
                        );
                        h.update(&id);
                    }
                    Some(Sym::Global { name, index }) => {
                        let n = name.clone().or_else(|| global_imports.get(*index as usize).cloned());
                        h.update(n.unwrap_or_default().as_bytes());
                    }
                    other => h.update(format!("{other:?}").as_bytes()),
                },
                MemoryAddrLeb | MemoryAddrSleb | MemoryAddrI32 | MemoryAddrRelSleb | MemoryAddrLeb64
                | MemoryAddrSleb64 | MemoryAddrI64 | MemoryAddrRelSleb64 | MemoryAddrTlsSleb
                | MemoryAddrTlsSleb64 | MemoryAddrLocrelI32 => {
                    let id = data_identity(
                        r.index, &symbols, &segments, data_start, &data_relocs, bytes, &mut data_ids,
                        &mut visiting,
                    );
                    h.update(&id);
                }
                TableNumberLeb => h.update(format!("{sym:?}").as_bytes()),
                FunctionOffsetI32 | FunctionOffsetI64 | SectionOffsetI32 | EventIndexLeb => {
                    bail!("relocation {:?} in code: not something a patch digest accounts for", r.ty)
                }
            }
            h.update(r.addend.to_le_bytes());
            cursor = (at + r.ty.extent()).min(body.end);
        }
        h.update(&bytes[cursor.min(body.end)..body.end]);
        let Some(name) = defined.get(&index).cloned() else {
            // A function no symbol names cannot be called or redirected
            // by name; it is part of whichever function references it.
            continue;
        };
        let facts = FnFacts { digest: truncate(h.finalize().into()), calls, addr_of };
        out.merge(CrateFns { fns: HashMap::from([(name, facts)]), ..CrateFns::default() });
    }

    // Data symbols that hold a function's address.
    if let Some(start) = data_start {
        for r in &data_relocs {
            use RelocationType::*;
            if !matches!(r.ty, TableIndexI32 | TableIndexI64 | TableIndexSleb | TableIndexSleb64) {
                continue;
            }
            let Some(target) = symbols.get(r.index as usize).and_then(func_name) else { continue };
            let at = start + r.offset as usize;
            let holder = symbols.iter().find_map(|s| match s {
                Sym::Data { name, def: Some((seg, off, size)) } => {
                    let range = segments.get(*seg as usize)?;
                    let lo = range.start + *off as usize;
                    (at >= lo && at < lo + *size as usize).then(|| name.clone())
                }
                _ => None,
            });
            if let Some(holder) = holder {
                out.data_addr_of.push((holder, target));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(digest: u8, calls: &[&str], addr_of: &[&str]) -> FnFacts {
        FnFacts {
            digest: [digest; 16],
            calls: calls.iter().map(|s| s.to_string()).collect(),
            addr_of: addr_of.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn krate(fns: &[(&str, FnFacts)]) -> CrateFns {
        CrateFns { fns: fns.iter().map(|(n, f)| (n.to_string(), f.clone())).collect(), ..CrateFns::default() }
    }

    /// The changed function, everything that reaches it by direct calls
    /// (across crates), and the address-takers that keep its slot —
    /// nothing else.
    #[test]
    fn the_plan_redirects_the_changed_and_their_direct_callers() {
        let lib_base = krate(&[("lib::g", facts(1, &[], &[])), ("lib::h", facts(2, &[], &[]))]);
        let lib_now = krate(&[("lib::g", facts(9, &[], &[])), ("lib::h", facts(2, &[], &[]))]);
        let app = krate(&[
            ("app::calls_g", facts(3, &["lib::g"], &[])),
            ("app::calls_caller", facts(4, &["app::calls_g"], &[])),
            ("app::calls_h", facts(5, &["lib::h"], &[])),
            ("app::points_at_g", facts(6, &[], &["lib::g"])),
            ("app::unrelated", facts(7, &[], &[])),
        ]);
        let mut app_with_data = app.clone();
        app_with_data.data_addr_of.push(("app::VTABLE".into(), "app::calls_g".into()));
        let p = plan(&[(&lib_now, Some(&lib_base)), (&app_with_data, Some(&app))]).unwrap();
        assert_eq!(p.changed, 1);
        assert_eq!(
            p.redirect.iter().map(String::as_str).collect::<Vec<_>>(),
            vec!["app::calls_caller", "app::calls_g", "lib::g"]
        );
        assert!(p.roots.contains("app::points_at_g") && p.roots.contains("app::VTABLE"));
        assert!(!p.roots.contains("app::calls_caller"), "redirected only through its callee's slot: not a root");
        assert_eq!(
            p.slotted.iter().map(String::as_str).collect::<Vec<_>>(),
            vec!["app::calls_g", "lib::g"],
            "only what something points at gets a slot"
        );
        assert!(!p.roots.contains("app::unrelated") && !p.roots.contains("app::calls_h"));
    }

    /// `src` compiled the way a patch replay compiles a crate: a wasm32
    /// relocatable object, position-independent, many codegen units.
    fn compile(src: &str) -> CrateFns {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("lib.rs"), src).unwrap();
        let out = std::process::Command::new("rustc")
            .args([
                "--edition=2021", "--crate-type=lib", "--crate-name=fixture", "--target=wasm32-unknown-unknown",
                "--emit=obj", "-Crelocation-model=pic", "-Ccodegen-units=16", "-Copt-level=1",
                "-Cmetadata=fixed", "lib.rs",
            ])
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let objs: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "o"))
            .collect();
        assert!(!objs.is_empty());
        CrateFns::of_objects(&objs).unwrap()
    }

    const SRC: &str = r#"
use core::hint::black_box;
#[inline(never)] pub fn label(i: usize) -> u8 { black_box("first").as_bytes()[i % 5] }
#[inline(never)] pub fn uses_label(i: usize) -> u32 { black_box(label(i)) as u32 + 1 }
#[inline(never)] pub fn far_caller(i: usize) -> u32 { black_box(uses_label(i)) * 2 }
#[inline(never)] pub fn independent(x: u32) -> u32 { x.wrapping_mul(31) ^ 7 }
#[inline(never)] pub fn pointer() -> fn(usize) -> u32 { far_caller }
"#;

    fn name<'a>(c: &'a CrateFns, part: &str) -> &'a str {
        c.fns.keys().find(|k| k.contains(part)).unwrap_or_else(|| panic!("no {part} in {:?}", c.fns.keys()))
    }

    /// Against real rustc output. The same source digests the same; a
    /// string literal's edit (an anonymous data symbol, named by
    /// position) changes its function; the plan redirects it and every
    /// direct caller up the chain, and keeps the function that takes one
    /// of their addresses as a root — but not the unrelated one.
    #[test]
    fn real_objects_find_a_literal_edit_and_its_callers() {
        let base = compile(SRC);
        assert_eq!(compile(SRC).fns, base.fns, "two compiles of one source must digest the same");
        let now = compile(&SRC.replace("\"first\"", "\"later\""));
        let p = plan(&[(&now, Some(&base))]).unwrap();
        let (label, uses, far, indep, ptr) = (
            name(&now, "5label"),
            name(&now, "10uses_label"),
            name(&now, "10far_caller"),
            name(&now, "11independent"),
            name(&now, "7pointer"),
        );
        assert!(p.redirect.contains(label), "{p:?}");
        assert!(p.redirect.contains(uses) && p.redirect.contains(far), "callers up the chain: {p:?}");
        assert!(!p.redirect.contains(indep) && !p.roots.contains(indep), "{p:?}");
        assert!(p.roots.contains(ptr) && !p.redirect.contains(ptr), "the address-taker is only a root: {p:?}");
        assert!(p.slotted.contains(far), "{p:?}");

        // A different arithmetic constant changes the bytes themselves.
        let now = compile(&SRC.replace("31", "37"));
        let p = plan(&[(&now, Some(&base))]).unwrap();
        assert_eq!(p.redirect.iter().map(String::as_str).collect::<Vec<_>>(), vec![name(&now, "11independent")]);
    }

    fn object(path: &str, defines: &[&str], refs: &[&str], init: bool) -> ObjectFacts {
        ObjectFacts {
            path: path.into(),
            defines: defines.iter().map(|s| s.to_string()).collect(),
            refs: refs.iter().map(|s| s.to_string()).collect(),
            init,
        }
    }

    /// An object holding a root is linked, with everything it needs from
    /// other objects, and so is one with constructors; the rest is not.
    #[test]
    fn keep_objects_follows_references_from_roots_and_keeps_constructors() {
        let plan = RedirectPlan { roots: BTreeSet::from(["root".to_string()]), ..RedirectPlan::default() };
        let c = CrateFns {
            objects: vec![
                object("a.o", &["root", ".Lanon.local.0"], &["helper", "from_base"], false),
                object("b.o", &["helper"], &["deep"], false),
                object("c.o", &["deep"], &[], false),
                object("d.o", &["unrelated"], &["helper"], false),
                object("e.o", &["registers"], &[], true),
            ],
            ..CrateFns::default()
        };
        let mut kept: Vec<String> =
            keep_objects(&plan, &[&c]).into_iter().map(|p| p.display().to_string()).collect();
        kept.sort();
        assert_eq!(kept, vec!["a.o", "b.o", "c.o", "e.o"]);
    }

    #[test]
    fn a_new_function_is_changed_and_a_missing_baseline_turns_the_filter_off() {
        let base = krate(&[("a", facts(1, &[], &[]))]);
        let now = krate(&[("a", facts(1, &[], &[])), ("b", facts(2, &[], &[]))]);
        assert_eq!(plan(&[(&now, Some(&base))]).unwrap().redirect, BTreeSet::from(["b".to_string()]));
        assert!(plan(&[(&now, Some(&base)), (&now, None)]).is_none());
    }
}
