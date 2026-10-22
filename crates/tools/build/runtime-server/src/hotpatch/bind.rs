//! Point a patch's statics at the running binary's (Mach-O, arm64).
//!
//! [`super::statics`] decides WHICH statics are bound; this rewrites the
//! replayed `.rcgu.o` files so the linked dylib uses the running binary's
//! storage for them. Two kinds of symbol need it:
//!
//! * a writable static, or a thread-local, the patch DEFINES and the
//!   binary already has ([`super::statics::pair`]);
//! * a static or thread-local the patch only REFERENCES, which the stub
//!   resolves to the binary's address, when the reference is one an
//!   absolute address cannot satisfy (below).
//!
//! # Why the instructions change
//!
//! Code reaches a static of its own crate PC-relatively — `adrp x8,
//! S@PAGE; add x8, x8, S@PAGEOFF` — and a thread-local through
//! `adrp x0, T@TLVPPAGE; ldr x0, [x0, T@TLVPPAGEOFF]`. Neither reaches
//! the running binary: the dylib's own load address is unknown at link
//! time, so ld64 rejects a page-relative fixup to an absolute address
//! ("ADRP out of range"), and it rejects a thread-local fixup to anything
//! that is not a thread-local it defines. A GOT load reaches anything:
//! `adrp x8, S@GOTPAGE; ldr x8, [x8, S@GOTPAGEOFF]` loads the address out
//! of a GOT slot, and ld64 fills the slot of an absolute symbol with the
//! absolute value. So:
//!
//! * `ARM64_RELOC_PAGE21` → `GOT_LOAD_PAGE21` (the `adrp` is unchanged);
//! * `ARM64_RELOC_PAGEOFF12` on an `add` → `GOT_LOAD_PAGEOFF12`, and the
//!   `add xd, xn, #lo12` becomes `ldr xd, [xn, #lo12]` — same registers,
//!   and the result is the same address, read from the slot;
//! * `TLVP_LOAD_PAGE21` / `TLVP_LOAD_PAGEOFF12` → their GOT twins (the
//!   `ldr` is already the right instruction). What the code loads is
//!   then the binary's own TLV descriptor, and calling its thunk yields
//!   the binary's slot for this thread;
//! * a pointer in data (`ARM64_RELOC_UNSIGNED`) is retargeted as is;
//! * a GOT load is retargeted as is.
//!
//! A `PAGEOFF12` folded into a load or store (`ldr w8, [x8, S@PAGEOFF]`)
//! has no GOT form without one more instruction, and an object cannot
//! grow one in place; the patch is refused and the dev loop rebuilds.
//! Debug builds (`opt-level = 0`, the dev profile) mostly materialize the
//! address with an `add`, but not always: an `inventory::submit!`
//! constructor reads its node's first field with `ldr x0, [x8,
//! __INVENTORY@PAGEOFF]`. That is the next rule's business.
//!
//! Code that cannot run is not bound. A constructor the binary already
//! ran is never called by the patch (below), so inside its body a
//! reference to a static the patch defines is left on the patch's own
//! copy, folded load or not. Every `#[component]` declares such a
//! constructor for its catalog entry, so without this rule every patch of
//! a file with one in it was refused.
//!
//! A GOT slot holds one address, so a reference with an addend
//! (`S+8@PAGE`) is pointed at a symbol of its own whose value already
//! includes it ([`super::statics::host_name`]).
//!
//! A constructor the binary already ran is pointed at [`CTOR_RAN`], an
//! empty function, in the patch's initializer list: it initialized
//! state the patch now shares, and dyld would run it again on load.
//!
//! # The object edit
//!
//! Relocation entries and the converted instructions are rewritten in
//! place, `ARM64_RELOC_ADDEND` entries folded into the new symbols are
//! dropped (the section's relocation count shrinks; the space stays), the
//! new undefined symbols are appended to the symbol table (undefined
//! symbols are the table's last group, so no existing index moves), and
//! the string table, which follows the symbol table at the end of the
//! file, is rewritten after it. Linker optimization hints describe
//! instruction pairs by address and kind, and the converted pairs are no
//! longer what they describe, so an edited object drops its hints. The
//! edited object is written next to the replay's, never over it: rustc
//! hard-links a codegen unit it reused from the incremental cache into
//! the replay's directory, and an edit in place would rewrite the cache.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use super::cache::HostBinCache;
use super::statics;

/// The symbols a patch's stub must define for its bound statics: every
/// name [`bind`] pointed a reference at, with the binary's LINK-TIME
/// address (addend included). The stub adds the ASLR slide.
#[derive(Debug, Default)]
pub struct Bindings {
    pub absolute: BTreeMap<String, u64>,
    /// Whether some constructor entry was pointed at [`CTOR_RAN`], which
    /// the stub then defines as a function that returns at once.
    pub ctor_ran: bool,
}

/// What a patch's constructor entry is pointed at when the binary already
/// ran that constructor (an `inventory::submit!`, say). dyld runs a
/// dylib's initializers when it loads it, and the patch's copy of the
/// constructor now acts on the binary's statics: run again, it registers
/// twice — an `inventory` node pushed onto the list it already heads
/// makes the list a cycle, and the next walk never ends.
pub const CTOR_RAN: &str = "__idealyst_hotpatch_constructor_ran";

// Mach-O constants (`<mach-o/loader.h>`, `<mach-o/nlist.h>`,
// `<mach-o/arm64/reloc.h>`).
const MH_MAGIC_64: u32 = 0xfeed_facf;
const MH_OBJECT: u32 = 0x1;
const LC_SEGMENT_64: u32 = 0x19;
const LC_SYMTAB: u32 = 0x2;
const LC_DYSYMTAB: u32 = 0xb;
const LC_LINKER_OPTIMIZATION_HINT: u32 = 0x2e;
const N_STAB: u8 = 0xe0;
const N_TYPE: u8 = 0x0e;
const N_EXT: u8 = 0x01;
const N_UNDF: u8 = 0x0;
const N_SECT: u8 = 0xe;
const S_ZEROFILL: u32 = 0x1;
const S_THREAD_LOCAL_VARIABLES: u32 = 0x13;
const S_MOD_INIT_FUNC_POINTERS: u32 = 0x9;
const S_ATTR_DEBUG: u32 = 0x0200_0000;
const UNSIGNED: u32 = 0;
const SUBTRACTOR: u32 = 1;
const PAGE21: u32 = 3;
const PAGEOFF12: u32 = 4;
const GOT_LOAD_PAGE21: u32 = 5;
const GOT_LOAD_PAGEOFF12: u32 = 6;
const TLVP_LOAD_PAGE21: u32 = 8;
const TLVP_LOAD_PAGEOFF12: u32 = 9;
const ADDEND: u32 = 10;

/// Whether a section of this segment/name/flags holds state a static
/// can own: writable data, zero-fill, or thread-local descriptors. Shared
/// with [`HostBinCache::load`] so the binary and the patch classify alike.
pub fn is_state_section(segment: &str, section: &str, flags: u32) -> bool {
    let kind = flags & 0xff;
    if kind == S_THREAD_LOCAL_VARIABLES {
        return true;
    }
    segment == "__DATA" && (matches!(section, "__data" | "__bss" | "__common") || kind == S_ZEROFILL)
}

/// A symbol name the compiler made up for a label rather than an item:
/// never paired with anything.
fn is_temporary(name: &str) -> bool {
    name.starts_with('l') || name.starts_with('L')
}

/// Rewrite `tip_objs` so the patch uses the running binary's statics.
/// Returns the objects to link (edited copies under `out_dir` where
/// something changed, the originals elsewhere) and what the stub must
/// define. An `Err` is a refusal: the dev loop rebuilds.
pub fn bind(tip_objs: &[PathBuf], cache: &HostBinCache, out_dir: &Path) -> Result<(Vec<PathBuf>, Bindings)> {
    let mut parsed = Vec::with_capacity(tip_objs.len());
    for path in tip_objs {
        let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        let obj = MachO::parse(&bytes).with_context(|| format!("parse {}", path.display()))?;
        parsed.push((path, bytes, obj));
    }

    // Every writable static the patch defines, and every name it defines
    // at all (a reference in one unit is often defined by another).
    let mut patch_state: Vec<&str> = Vec::new();
    let mut defined: HashSet<&str> = HashSet::new();
    for (_, bytes, obj) in &parsed {
        for sym in &obj.symbols {
            let name = obj.name(bytes, sym);
            if sym.is_defined() {
                if sym.n_type & N_EXT != 0 {
                    defined.insert(name);
                }
                if !is_temporary(name) && obj.is_state(sym) {
                    patch_state.push(name);
                }
            }
        }
    }
    let bound = statics::pair(patch_state.iter().copied(), &cache.state_symbols)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let mut bindings = Bindings::default();
    let mut out = Vec::with_capacity(parsed.len());
    for (path, bytes, obj) in &parsed {
        // What each symbol index of this object becomes.
        let mut plan: HashMap<u32, Target> = HashMap::new();
        for (index, sym) in obj.symbols.iter().enumerate() {
            let name = obj.name(bytes, sym);
            if bound.contains(name) {
                let Some(host) = cache.symbols.get(name) else { continue };
                plan.insert(index as u32, Target::Renamed { name: name.to_string(), host: host.address });
            } else if sym.is_undefined() && !defined.contains(name) {
                // A reference the stub resolves to the binary. Only its
                // page-relative and thread-local fixups need a GOT.
                let Some(host) = cache.symbols.get(name) else { continue };
                match host.kind {
                    object::SymbolKind::Text => {}
                    object::SymbolKind::Tls => {
                        plan.insert(index as u32, Target::InPlace { name: name.to_string(), host: host.address, tls: true });
                    }
                    _ => {
                        plan.insert(index as u32, Target::InPlace { name: name.to_string(), host: host.address, tls: false });
                    }
                }
            }
        }
        let has_ctors = obj.sections.iter().any(|s| s.flags & 0xff == S_MOD_INIT_FUNC_POINTERS);
        if plan.is_empty() && !has_ctors {
            out.push((*path).clone());
            continue;
        }
        let ran = |name: &str| cache.symbols.get(name).is_some_and(|s| s.kind == object::SymbolKind::Text);
        match rewrite(bytes, obj, &plan, &ran, &mut bindings).with_context(|| format!("binding statics in {}", path.display()))? {
            None => out.push((*path).clone()),
            Some(edited) => {
                std::fs::create_dir_all(out_dir).with_context(|| format!("create {}", out_dir.display()))?;
                let file = out_dir.join(path.file_name().context("object path has no file name")?);
                std::fs::write(&file, edited).with_context(|| format!("write {}", file.display()))?;
                out.push(file);
            }
        }
    }
    Ok((out, bindings))
}

/// What the references to one symbol index become.
enum Target {
    /// A static the patch defines and the binary has: references move to
    /// a new undefined symbol bound to the binary's address.
    Renamed { name: String, host: u64 },
    /// A symbol the patch references and the stub defines as the
    /// binary's address: references keep the symbol and only change
    /// fixup kind (or, with an addend, move to a symbol of their own).
    InPlace { name: String, host: u64, tls: bool },
}

/// The body of a function the patch never calls, as an address range of
/// one section.
struct Dormant {
    section: usize,
    start: u64,
    end: u64,
}

impl Dormant {
    fn covers(&self, section: usize, address: u64) -> bool {
        self.section == section && (self.start..self.end).contains(&address)
    }
}

/// The constructors in this object that [`rewrite`] points at
/// [`CTOR_RAN`]: the ones the binary already ran. Each runs from its
/// symbol to the next named symbol of its section, or the section's end;
/// a label the compiler made up (`ltmp…`) can sit inside a body, so only
/// a named symbol ends one.
fn dormant_code(bytes: &[u8], obj: &MachO, ran: &dyn Fn(&str) -> bool) -> Vec<Dormant> {
    let mut out = Vec::new();
    for section in &obj.sections {
        if section.flags & 0xff != S_MOD_INIT_FUNC_POINTERS {
            continue;
        }
        for i in 0..section.nreloc as usize {
            let info = read_u32(bytes, section.reloff as usize + i * 8 + 4);
            if info & (1 << 27) == 0 || info >> 28 != UNSIGNED {
                continue;
            }
            let Some(ctor) = obj.symbols.get((info & 0x00ff_ffff) as usize) else { continue };
            if !ctor.is_defined() || !ran(obj.name(bytes, ctor)) {
                continue;
            }
            let Some(home) = (ctor.n_sect as usize).checked_sub(1).and_then(|i| obj.sections.get(i)) else {
                continue;
            };
            let end = obj
                .symbols
                .iter()
                .filter(|s| s.is_defined() && s.n_sect == ctor.n_sect && s.value > ctor.value)
                .filter(|s| !is_temporary(obj.name(bytes, s)))
                .map(|s| s.value)
                .min()
                .unwrap_or(home.addr + home.size);
            out.push(Dormant { section: ctor.n_sect as usize, start: ctor.value, end });
        }
    }
    out
}

/// Apply `plan` to one object. `None` when no relocation needed a change.
fn rewrite(
    bytes: &[u8],
    obj: &MachO,
    plan: &HashMap<u32, Target>,
    ran: &dyn Fn(&str) -> bool,
    bindings: &mut Bindings,
) -> Result<Option<Vec<u8>>> {
    let mut data = bytes.to_vec();
    let mut new_syms: Vec<String> = Vec::new();
    let mut new_index: HashMap<String, u32> = HashMap::new();
    let mut changed = false;
    let first_new = obj.symbols.len() as u32;
    // Sections holding a static this object defines and the plan binds.
    let dormant = dormant_code(bytes, obj, ran);
    let bound_sections: HashSet<usize> = plan
        .iter()
        .filter(|(_, t)| matches!(t, Target::Renamed { .. }))
        .filter_map(|(&i, _)| obj.symbols.get(i as usize))
        .filter(|sym| sym.is_defined())
        .map(|sym| sym.n_sect as usize)
        .collect();

    for section in &obj.sections {
        if section.nreloc == 0 {
            continue;
        }
        let debug = section.flags & S_ATTR_DEBUG != 0;
        let ctors = section.flags & 0xff == S_MOD_INIT_FUNC_POINTERS;
        let mut relocs: Vec<(i32, u32)> = (0..section.nreloc as usize)
            .map(|i| {
                let at = section.reloff as usize + i * 8;
                (read_i32(bytes, at), read_u32(bytes, at + 4))
            })
            .collect();
        let mut keep = vec![true; relocs.len()];
        let mut touched = false;
        for i in 0..relocs.len() {
            let (address, info) = relocs[i];
            let symbolnum = info & 0x00ff_ffff;
            let is_extern = info & (1 << 27) != 0;
            let kind = info >> 28;
            if !is_extern {
                // Section-relative: arm64 emits these only in debug
                // sections, which may keep pointing at the patch's copy.
                if !debug && kind != ADDEND && bound_sections.contains(&(symbolnum as usize)) {
                    bail!(
                        "a section-relative relocation points into section {}, which holds a static \
                         bound to the running binary",
                        obj.sections[symbolnum as usize - 1].name
                    );
                }
                continue;
            }
            if ctors && kind == UNSIGNED {
                let Some(sym) = obj.symbols.get(symbolnum as usize) else { continue };
                if ran(obj.name(bytes, sym)) {
                    let symbol = *new_index.entry(CTOR_RAN.to_string()).or_insert_with(|| {
                        new_syms.push(CTOR_RAN.to_string());
                        first_new + new_syms.len() as u32 - 1
                    });
                    relocs[i].1 = (info & 0xff00_0000) | symbol;
                    bindings.ctor_ran = true;
                    touched = true;
                }
                continue;
            }
            let Some(target) = plan.get(&symbolnum) else { continue };
            if matches!(target, Target::Renamed { .. })
                && dormant.iter().any(|d| d.covers(section.index, section.addr + address as u64))
            {
                // Never runs; the patch's own copy satisfies the linker.
                continue;
            }
            let (name, host, renamed, tls) = match target {
                Target::Renamed { name, host } => (name, *host, true, false),
                Target::InPlace { name, host, tls } => (name, *host, false, *tls),
            };
            let addend = if i > 0 && relocs[i - 1].1 >> 28 == ADDEND && keep[i - 1] {
                sign_extend_24(relocs[i - 1].1 & 0x00ff_ffff)
            } else {
                0
            };
            let had_addend_entry = i > 0 && relocs[i - 1].1 >> 28 == ADDEND;
            let insn_at = section.offset as usize + address as usize;
            let new_kind = match kind {
                UNSIGNED => {
                    if i > 0 && relocs[i - 1].1 >> 28 == SUBTRACTOR {
                        bail!("`{name}` is the minuend of a SUBTRACTOR pair, which cannot be bound");
                    }
                    if !renamed {
                        continue; // the stub's absolute value satisfies it
                    }
                    UNSIGNED
                }
                PAGE21 | TLVP_LOAD_PAGE21 => GOT_LOAD_PAGE21,
                PAGEOFF12 => {
                    let insn = read_u32(bytes, insn_at);
                    // ADD (immediate), 64-bit, unshifted.
                    if insn & 0xffc0_0000 != 0x9100_0000 {
                        bail!(
                            "a load or store addresses `{name}` directly (instruction {insn:#010x}); a \
                             patch can only reach the running binary's copy through an address it \
                             materializes"
                        );
                    }
                    let rd = insn & 0x1f;
                    let rn = (insn >> 5) & 0x1f;
                    if rd == 31 {
                        bail!("`{name}`'s address is added into sp");
                    }
                    // LDR (immediate, unsigned offset), 64-bit; the fixup
                    // fills the offset.
                    write_u32(&mut data, insn_at, 0xf940_0000 | (rn << 5) | rd);
                    GOT_LOAD_PAGEOFF12
                }
                TLVP_LOAD_PAGEOFF12 => {
                    let insn = read_u32(bytes, insn_at);
                    if insn & 0xffc0_0000 != 0xf940_0000 {
                        bail!("thread-local `{name}` is loaded by an unexpected instruction {insn:#010x}");
                    }
                    GOT_LOAD_PAGEOFF12
                }
                GOT_LOAD_PAGE21 | GOT_LOAD_PAGEOFF12 => {
                    if !renamed && addend == 0 {
                        continue;
                    }
                    kind
                }
                other => bail!("`{name}` is the target of relocation type {other}, which cannot be bound"),
            };
            if tls && !matches!(kind, TLVP_LOAD_PAGE21 | TLVP_LOAD_PAGEOFF12) && kind != UNSIGNED {
                bail!("thread-local `{name}` is referenced by relocation type {kind}");
            }
            // The symbol the fixup now names.
            let symbol = if renamed || addend != 0 {
                let new_name = statics::host_name(name, addend);
                bindings.absolute.insert(new_name.clone(), (host as i64 + addend) as u64);
                *new_index.entry(new_name.clone()).or_insert_with(|| {
                    new_syms.push(new_name);
                    first_new + new_syms.len() as u32 - 1
                })
            } else {
                symbolnum
            };
            if had_addend_entry {
                keep[i - 1] = false;
            }
            relocs[i].1 = (new_kind << 28) | (info & 0x0f00_0000) | symbol;
            touched = true;
        }
        if touched {
            changed = true;
            let kept: Vec<(i32, u32)> = relocs.iter().zip(&keep).filter(|(_, k)| **k).map(|(r, _)| *r).collect();
            for (i, (address, info)) in kept.iter().enumerate() {
                let at = section.reloff as usize + i * 8;
                write_u32(&mut data, at, *address as u32);
                write_u32(&mut data, at + 4, *info);
            }
            write_u32(&mut data, section.header_at + 60, kept.len() as u32); // nreloc
        }
    }
    if !changed {
        return Ok(None);
    }

    // Append the new undefined symbols and their names. The symbol table
    // and then the string table must close the file, which is how LLVM
    // lays out an object.
    let symtab_end = obj.symoff as usize + obj.symbols.len() * 16;
    if obj.stroff as usize != symtab_end || (obj.stroff + obj.strsize) as usize != bytes.len() {
        bail!("the object's symbol and string tables are not the last thing in it");
    }
    let mut strtab = bytes[obj.stroff as usize..].to_vec();
    let mut tail = bytes[obj.symoff as usize..symtab_end].to_vec();
    for name in &new_syms {
        let strx = strtab.len() as u32;
        strtab.extend_from_slice(name.as_bytes());
        strtab.push(0);
        tail.extend_from_slice(&strx.to_le_bytes());
        tail.push(N_UNDF | N_EXT);
        tail.push(0); // n_sect
        tail.extend_from_slice(&0u16.to_le_bytes()); // n_desc
        tail.extend_from_slice(&0u64.to_le_bytes()); // n_value
    }
    while strtab.len() % 8 != 0 {
        strtab.push(0);
    }
    let new_stroff = obj.symoff as usize + tail.len();
    data.truncate(obj.symoff as usize);
    data.extend_from_slice(&tail);
    data.extend_from_slice(&strtab);
    write_u32(&mut data, obj.symtab_cmd + 12, (obj.symbols.len() + new_syms.len()) as u32);
    write_u32(&mut data, obj.symtab_cmd + 16, new_stroff as u32);
    write_u32(&mut data, obj.symtab_cmd + 20, strtab.len() as u32);
    if let Some(dysymtab) = obj.dysymtab_cmd {
        let nundef = read_u32(&data, dysymtab + 28);
        write_u32(&mut data, dysymtab + 28, nundef + new_syms.len() as u32);
    }
    if let Some(loh) = obj.loh_cmd {
        write_u32(&mut data, loh + 12, 0); // datasize
    }
    Ok(Some(data))
}

fn sign_extend_24(v: u32) -> i64 {
    ((v << 8) as i32 >> 8) as i64
}

fn read_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes"))
}

fn read_i32(b: &[u8], at: usize) -> i32 {
    read_u32(b, at) as i32
}

fn read_u64(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().expect("8 bytes"))
}

fn write_u32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn fixed_name(b: &[u8]) -> &str {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    std::str::from_utf8(&b[..end]).unwrap_or("")
}

/// The parts of a 64-bit Mach-O relocatable object the rewrite reads.
struct MachO {
    sections: Vec<Section>,
    symbols: Vec<Nlist>,
    symoff: u32,
    stroff: u32,
    strsize: u32,
    /// File offsets of the load commands the rewrite edits.
    symtab_cmd: usize,
    dysymtab_cmd: Option<usize>,
    loh_cmd: Option<usize>,
}

struct Section {
    /// This section's number in `n_sect` terms (1-based).
    index: usize,
    /// File offset of this `section_64` header.
    header_at: usize,
    segment: String,
    name: String,
    addr: u64,
    size: u64,
    offset: u32,
    reloff: u32,
    nreloc: u32,
    flags: u32,
}

struct Nlist {
    strx: u32,
    n_type: u8,
    n_sect: u8,
    value: u64,
}

impl Nlist {
    fn is_defined(&self) -> bool {
        self.n_type & N_STAB == 0 && self.n_type & N_TYPE == N_SECT
    }
    fn is_undefined(&self) -> bool {
        self.n_type & N_STAB == 0 && self.n_type & N_TYPE == N_UNDF
    }
}

impl MachO {
    fn parse(b: &[u8]) -> Result<Self> {
        if b.len() < 32 || read_u32(b, 0) != MH_MAGIC_64 {
            bail!("not a 64-bit little-endian Mach-O object");
        }
        if read_u32(b, 12) != MH_OBJECT {
            bail!("not a relocatable object");
        }
        let ncmds = read_u32(b, 16);
        let mut at = 32usize;
        let mut sections = Vec::new();
        let (mut symtab_cmd, mut dysymtab_cmd, mut loh_cmd) = (None, None, None);
        for _ in 0..ncmds {
            let cmd = read_u32(b, at);
            let size = read_u32(b, at + 4) as usize;
            match cmd {
                LC_SEGMENT_64 => {
                    let nsects = read_u32(b, at + 64) as usize;
                    for s in 0..nsects {
                        let h = at + 72 + s * 80;
                        sections.push(Section {
                            index: sections.len() + 1,
                            header_at: h,
                            name: fixed_name(&b[h..h + 16]).to_string(),
                            segment: fixed_name(&b[h + 16..h + 32]).to_string(),
                            addr: read_u64(b, h + 32),
                            size: read_u64(b, h + 40),
                            offset: read_u32(b, h + 48),
                            reloff: read_u32(b, h + 56),
                            nreloc: read_u32(b, h + 60),
                            flags: read_u32(b, h + 64),
                        });
                    }
                }
                LC_SYMTAB => symtab_cmd = Some(at),
                LC_DYSYMTAB => dysymtab_cmd = Some(at),
                LC_LINKER_OPTIMIZATION_HINT => loh_cmd = Some(at),
                _ => {}
            }
            at += size;
        }
        let symtab_cmd = symtab_cmd.context("no LC_SYMTAB")?;
        let symoff = read_u32(b, symtab_cmd + 8);
        let nsyms = read_u32(b, symtab_cmd + 12) as usize;
        let stroff = read_u32(b, symtab_cmd + 16);
        let strsize = read_u32(b, symtab_cmd + 20);
        let symbols = (0..nsyms)
            .map(|i| {
                let e = symoff as usize + i * 16;
                Nlist { strx: read_u32(b, e), n_type: b[e + 4], n_sect: b[e + 5], value: read_u64(b, e + 8) }
            })
            .collect();
        Ok(Self { sections, symbols, symoff, stroff, strsize, symtab_cmd, dysymtab_cmd, loh_cmd })
    }

    fn name<'a>(&self, b: &'a [u8], sym: &Nlist) -> &'a str {
        let start = self.stroff as usize + sym.strx as usize;
        let end = (self.stroff + self.strsize) as usize;
        if start >= end {
            return "";
        }
        fixed_name(&b[start..end])
    }

    /// Whether a defined symbol is a static that holds state.
    fn is_state(&self, sym: &Nlist) -> bool {
        let Some(section) = (sym.n_sect as usize).checked_sub(1).and_then(|i| self.sections.get(i)) else {
            return false;
        };
        // A label at a section's very end belongs to nothing.
        if sym.value >= section.addr + section.size && section.size > 0 {
            return false;
        }
        is_state_section(&section.segment, &section.name, section.flags)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_sections_are_writable_data_zero_fill_and_tlv_descriptors() {
        assert!(is_state_section("__DATA", "__data", 0));
        assert!(is_state_section("__DATA", "__bss", S_ZEROFILL));
        assert!(is_state_section("__DATA", "__common", S_ZEROFILL));
        assert!(is_state_section("__DATA", "__thread_vars", S_THREAD_LOCAL_VARIABLES));
        // Immutable statics: content, not state. `stylesheet!`'s cache
        // key is one of these and must keep a fresh address per patch.
        assert!(!is_state_section("__TEXT", "__const", 0));
        assert!(!is_state_section("__DATA", "__const", 0));
        // A TLV's initial value is not the variable.
        assert!(!is_state_section("__DATA", "__thread_data", 0x11));
    }

    #[test]
    fn an_addend_entry_is_sign_extended() {
        assert_eq!(sign_extend_24(8), 8);
        assert_eq!(sign_extend_24(0x00ff_fff8), -8);
    }
}

/// The rewrite against objects the system assembler produced, linked by
/// the real `ld` the way a patch is: what ld64 accepts is the contract
/// here, and only ld64 can say.
#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
mod link_tests {
    use super::*;
    use crate::hotpatch::cache::{CachedSymbol, HostBinCache};
    use object::{Object, ObjectSection, ObjectSymbol, SymbolKind};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("idealyst-bind-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn assemble(dir: &Path, source: &str) -> PathBuf {
        let s = dir.join("tip.s");
        let o = dir.join("tip.o");
        std::fs::write(&s, source).unwrap();
        let status = std::process::Command::new("cc").args(["-c", "-arch", "arm64"]).arg(&s).arg("-o").arg(&o).status().unwrap();
        assert!(status.success(), "assembling the fixture failed");
        o
    }

    fn cache(entries: &[(&str, u64, SymbolKind)], state: &[&str]) -> HostBinCache {
        let mut c = HostBinCache::default();
        c.main_addr = 0x1_0000_0000;
        c.symbols.insert("_main".into(), CachedSymbol { address: c.main_addr, kind: SymbolKind::Text, size: 0, is_weak: false });
        for (name, address, kind) in entries {
            c.symbols.insert((*name).into(), CachedSymbol { address: *address, kind: *kind, size: 0, is_weak: false });
        }
        c.state_symbols = statics::ProgramStatics::new(state.iter().copied());
        c
    }

    /// Relocation types, by the instruction offset they apply to.
    fn relocs(path: &Path) -> Vec<(u64, u32, String)> {
        let data = std::fs::read(path).unwrap();
        let file = object::File::parse(&*data).unwrap();
        let text = file.section_by_name("__text").unwrap();
        let mut out: Vec<(u64, u32, String)> = text
            .relocations()
            .map(|(at, r)| {
                let ty = match r.flags() {
                    object::RelocationFlags::MachO { r_type, .. } => r_type as u32,
                    _ => unreachable!(),
                };
                let name = match r.target() {
                    object::RelocationTarget::Symbol(i) => file.symbol_by_index(i).unwrap().name().unwrap().to_string(),
                    _ => String::new(),
                };
                (at, ty, name)
            })
            .collect();
        out.sort();
        out
    }

    fn link(dir: &Path, objs: &[PathBuf], cache: &HostBinCache, bindings: &Bindings) {
        let stub = dir.join("stub.o");
        crate::hotpatch::stub::synthesize(objs, cache, bindings, 0x1_0000_0000, &dir.join("stub.s"), &stub).unwrap();
        crate::hotpatch::link::link_dylib(objs, &stub, &dir.join("patch.dylib")).expect("ld accepts the rewritten object");
    }

    /// A thread-local the patch only references (an `#[inline]` accessor
    /// of another crate's `thread_local!`) used to get a FRESH TLV
    /// descriptor in the stub, so the patch read a slot of its own. Now
    /// the load goes through a GOT slot holding the binary's descriptor.
    #[test]
    fn regression_a_referenced_thread_local_loads_the_binarys_descriptor() {
        let dir = scratch("tlv");
        let tip = assemble(
            &dir,
            ".text\n.globl _f\n.p2align 2\n_f:\n  adrp x0, _ext_tlv@TLVPPAGE\n  ldr x0, [x0, _ext_tlv@TLVPPAGEOFF]\n  ldr x8, [x0]\n  blr x8\n  ret\n",
        );
        let cache = cache(&[("_ext_tlv", 0x1_0000_8000, SymbolKind::Tls)], &["_ext_tlv"]);
        let (objs, bindings) = bind(&[tip], &cache, &dir.join("bound")).unwrap();
        let types: Vec<u32> = relocs(&objs[0]).iter().map(|r| r.1).collect();
        assert_eq!(types, vec![GOT_LOAD_PAGE21, GOT_LOAD_PAGEOFF12]);
        link(&dir, &objs, &cache, &bindings);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A static the patch defines and the binary has, addressed with an
    /// addend: the `add` becomes a GOT load of a symbol whose value
    /// includes the addend, and the ADDEND entries are gone.
    #[test]
    fn a_defined_static_with_an_addend_is_bound_through_the_got() {
        let dir = scratch("addend");
        let tip = assemble(
            &dir,
            ".text\n.globl _f\n.p2align 2\n_f:\n  adrp x8, _S@PAGE\n  add x8, x8, _S@PAGEOFF\n  adrp x9, _S+8@PAGE\n  add x9, x9, _S+8@PAGEOFF\n  ret\n\
             .data\n.globl _S\n.p2align 3\n_S:\n  .quad 1\n  .quad 2\n",
        );
        let cache = cache(&[("_S", 0x1_0000_4000, SymbolKind::Data)], &["_S"]);
        let (objs, bindings) = bind(&[tip], &cache, &dir.join("bound")).unwrap();
        assert_eq!(bindings.absolute.get("_S$idealyst_host"), Some(&0x1_0000_4000));
        assert_eq!(bindings.absolute.get("_S$idealyst_host+8"), Some(&0x1_0000_4008));
        let rewritten = relocs(&objs[0]);
        assert!(rewritten.iter().all(|r| r.1 == GOT_LOAD_PAGE21 || r.1 == GOT_LOAD_PAGEOFF12), "{rewritten:?}");
        assert!(rewritten.iter().all(|r| r.2.starts_with("_S$idealyst_host")), "{rewritten:?}");
        // `add x8, x8, #0` is now `ldr x8, [x8]`.
        let data = std::fs::read(&objs[0]).unwrap();
        let file = object::File::parse(&*data).unwrap();
        let text = file.section_by_name("__text").unwrap().data().unwrap();
        assert_eq!(read_u32(text, 4), 0xf940_0108);
        link(&dir, &objs, &cache, &bindings);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A load folded into the page offset has no GOT form in place; the
    /// patch is refused (and the dev loop rebuilds) rather than reading
    /// the patch's own copy.
    #[test]
    fn a_load_folded_into_the_page_offset_refuses() {
        let dir = scratch("folded");
        let tip = assemble(
            &dir,
            ".text\n.globl _f\n.p2align 2\n_f:\n  adrp x8, _S@PAGE\n  ldr w0, [x8, _S@PAGEOFF]\n  ret\n\
             .data\n.globl _S\n.p2align 2\n_S:\n  .long 5\n",
        );
        let cache = cache(&[("_S", 0x1_0000_4000, SymbolKind::Data)], &["_S"]);
        let err = bind(&[tip], &cache, &dir.join("bound")).unwrap_err();
        assert!(format!("{err:#}").contains("a load or store addresses `_S` directly"), "{err:#}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The constructor `inventory::submit!` declares for every
    /// `#[component]` reads its node's first field with a load folded into
    /// the page offset, so every patch of a file with a component in it was
    /// refused (wire mode respawned instead). The binary already ran that
    /// constructor and the patch never calls it, so its references stay on
    /// the patch's own copy; the same load in code that does run still
    /// refuses.
    #[test]
    fn regression_a_constructor_the_binary_ran_may_load_its_node_directly() {
        let dir = scratch("ctor");
        let source = |caller: &str| {
            format!(
                ".text\n.p2align 2\n_ctor:\n  adrp x8, _NODE@PAGE\n  adrp x1, _NODE@PAGE\n  add x1, x1, _NODE@PAGEOFF\n  ldr x0, [x8, _NODE@PAGEOFF]\n  ret\n\
                 .globl _f\n.p2align 2\n_f:\n  adrp x8, _NODE@PAGE\n  {caller}\n  ret\n\
                 .data\n.globl _NODE\n.p2align 3\n_NODE:\n  .quad 0\n  .quad 0\n\
                 .section __DATA,__mod_init_func,mod_init_funcs\n.p2align 3\n  .quad _ctor\n"
            )
        };
        let cache = cache(
            &[("_NODE", 0x1_0000_4000, SymbolKind::Data), ("_ctor", 0x1_0000_1000, SymbolKind::Text)],
            &["_NODE"],
        );

        let tip = assemble(&dir, &source("add x8, x8, _NODE@PAGEOFF"));
        let (objs, bindings) = bind(&[tip], &cache, &dir.join("bound")).unwrap();
        assert!(bindings.ctor_ran);
        let rewritten = relocs(&objs[0]);
        let (ctor, live): (Vec<_>, Vec<_>) = rewritten.iter().partition(|r| r.0 < 16);
        assert!(ctor.iter().all(|r| r.2 == "_NODE" && (r.1 == PAGE21 || r.1 == PAGEOFF12)), "{ctor:?}");
        assert!(live.iter().all(|r| r.2 == "_NODE$idealyst_host"), "the running code is still bound: {live:?}");
        link(&dir, &objs, &cache, &bindings);

        let tip = assemble(&dir, &source("ldr x0, [x8, _NODE@PAGEOFF]"));
        let err = bind(&[tip], &cache, &dir.join("bound")).unwrap_err();
        assert!(format!("{err:#}").contains("a load or store addresses `_NODE` directly"), "{err:#}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A static the binary does not have is left alone: the patch keeps
    /// its own storage for it, and the object is linked as replayed.
    #[test]
    fn a_new_static_keeps_its_own_storage() {
        let dir = scratch("new");
        let tip = assemble(
            &dir,
            ".text\n.globl _f\n.p2align 2\n_f:\n  adrp x8, _NEW@PAGE\n  add x8, x8, _NEW@PAGEOFF\n  ret\n\
             .data\n.globl _NEW\n.p2align 2\n_NEW:\n  .long 5\n",
        );
        let cache = cache(&[], &[]);
        let (objs, bindings) = bind(&[tip.clone()], &cache, &dir.join("bound")).unwrap();
        assert_eq!(objs, vec![tip]);
        assert!(bindings.absolute.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
