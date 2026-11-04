//! Point a patch's statics at the running binary's (ELF, arm64): the ELF
//! twin of [`super::bind`], whose module docs give the reasoning this file
//! shares. Only what differs is spelled out here.
//!
//! # Statics
//!
//! A static of the patch's own crate is reached page-relatively —
//! `adrp x8, :pg_hi21:S; add x8, x8, :lo12:S` (`ADR_PREL_PG_HI21`,
//! `ADD_ABS_LO12_NC`) — and often not through its own symbol: LLVM names
//! the SECTION it sits in (`.bss.<symbol>` + offset). A static of another
//! crate is already a GOT load (`ADR_GOT_PAGE`, `LD64_GOT_LO12_NC`). The
//! patch is a shared object loaded far from the binary (more than 4 GiB
//! away on Linux arm64), so a page-relative reference to the binary's
//! address cannot be linked. Every reference becomes a load of the
//! address from a CELL: an 8-byte word the stub defines, holding the
//! binary's run-time address of the static. The `adrp` keeps its
//! encoding against the cell (`ADR_PREL_PG_HI21`), and the `add xd, xn,
//! #lo12` becomes `ldr xd, [xn, #lo12]` (`LDST64_ABS_LO12_NC`) — a GOT
//! load's instructions, against a slot the patch owns. Not the linker's
//! GOT with an absolute symbol, as on Mach-O: GNU ld turns the GOT slot
//! of an absolute symbol in a shared object into an `R_AARCH64_RELATIVE`
//! relocation, and the loader then adds the patch's own load address to
//! the binary's. A pointer to a static in data (`ABS64`) gets the
//! address written in place and its relocation dropped, for the same
//! reason. Both need the binary's load address, which [`bind`] takes.
//!
//! # Thread-locals
//!
//! A crate compiled for an executable still reaches its thread-locals
//! through TLS descriptors (`adrp x0; ldr x1, [x0]; add x0; blr x1`); the
//! linker relaxes that to local-exec when it links the executable, and the
//! running binary's thread-locals sit at a fixed offset from the thread
//! pointer. A shared object cannot use local-exec, and its descriptors
//! would name thread-locals the loader only knows if the binary exported
//! them. So the patch relaxes its references itself, exactly as the
//! linker did for the binary: the four instructions become `movz x0,
//! #tprel_g1, lsl #16; movk x0, #tprel_g0; nop; nop`, the code after them
//! adds `tpidr_el0` as before, and the relocations become `R_AARCH64_NONE`.
//! The offset is the binary's: variant-1 TLS puts the executable's block
//! right after the 16-byte TCB, aligned to the TLS segment
//! ([`tls_block_offset`]), so a thread-local at `st_value` lives at
//! `tp + align_up(16, p_align) + st_value` on every thread. Initial-exec
//! (`adrp; ldr` of a GOT slot) and local-exec (`add` / `movz` immediates,
//! what the bin crate itself is compiled to) relax the same way.
//!
//! A thread-local the binary does not have keeps its descriptor: it is
//! the patch's own, and the loader gives the patch a TLS block for it.
//!
//! # The object edit
//!
//! Relocation entries and instructions are rewritten in place. New
//! undefined symbols are appended to the symbol table (globals close an
//! ELF symbol table, so no existing index moves), and the symbol and
//! string tables (and `SHT_SYMTAB_SHNDX`, in a file with one) are written
//! again at the end of the file with their section headers pointed at the
//! copies. The edited object is written next to the replay's, never over it
//! (see [`super::bind`]).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use object::elf;

use super::bind::{Bindings, CTOR_RAN};
use super::cache::HostBinCache;
use super::statics;

/// The size of the arm64 thread control block that precedes the
/// executable's TLS block (variant 1: the thread pointer points at it).
const AARCH64_TCB_SIZE: u64 = 16;

/// `SHF_GNU_RETAIN`: the linker never garbage-collects the section. What
/// [`retain_roots`] sets on a redirected function's section, as Mach-O
/// marks its symbol `N_NO_DEAD_STRIP`.
pub const SHF_GNU_RETAIN: u64 = 0x20_0000;

/// `EI_OSABI` value for GNU. `SHF_GNU_RETAIN` sits in the OS-specific
/// flag range, and GNU ld reads it only in an object that declares the
/// GNU ABI; rustc's objects declare none (`ELFOSABI_NONE`), and ld then
/// garbage-collected every redirected function the patch itself never
/// called.
const ELFOSABI_GNU: u8 = 3;
const EI_OSABI: usize = 7;

/// Where the executable's TLS block starts, relative to the thread
/// pointer, given its `PT_TLS` alignment.
pub fn tls_block_offset(tls_align: u64) -> u64 {
    let align = tls_align.max(1);
    AARCH64_TCB_SIZE.div_ceil(align) * align
}

/// Whether an ELF section of this name holds state a static can own:
/// writable data, zero-fill, or thread-local storage. Relocated read-only
/// data (`.data.rel.ro`) is written once by the loader and is constant
/// after, so a patch keeps its own copy, as on Mach-O.
pub fn is_state_section(name: &str) -> bool {
    let is = |base: &str| name == base || name.strip_prefix(base).is_some_and(|rest| rest.starts_with('.'));
    if is(".data.rel.ro") {
        return false;
    }
    is(".data") || is(".bss") || is(".tdata") || is(".tbss")
}

// ── the object ───────────────────────────────────────────────────────

const EHDR_SIZE: usize = 64;
const SHDR_SIZE: usize = 64;
const SYM_SIZE: usize = 24;
const RELA_SIZE: usize = 24;

/// What a symbol's section index holds when the real index is in
/// `SHT_SYMTAB_SHNDX`.
const SHN_XINDEX: u16 = 0xffff;
/// `Sym::shndx` of a symbol in no section of the file: `SHN_ABS`, `SHN_COMMON`.
const SHN_RESERVED: u32 = u32::MAX;

struct Section {
    index: usize,
    header_at: usize,
    name: String,
    sh_type: u32,
    flags: u64,
    offset: u64,
    size: u64,
    link: u32,
    info: u32,
}

#[derive(Clone)]
struct Sym {
    name: String,
    info: u8,
    /// The section's real index: read through `SHT_SYMTAB_SHNDX` when the
    /// 16-bit field cannot hold it, [`SHN_RESERVED`] for no section.
    shndx: u32,
    value: u64,
    size: u64,
}

impl Sym {
    fn kind(&self) -> u8 {
        self.info & 0xf
    }
    fn is_undefined(&self) -> bool {
        self.shndx == u32::from(elf::SHN_UNDEF)
    }
    fn is_defined(&self) -> bool {
        !self.is_undefined() && self.shndx != SHN_RESERVED
    }
    fn is_global(&self) -> bool {
        self.info >> 4 != elf::STB_LOCAL
    }
}

struct Elf {
    sections: Vec<Section>,
    symtab: usize,
    /// The `SHT_SYMTAB_SHNDX` section of a file with more sections than the
    /// 16-bit fields can name.
    symtab_shndx: Option<usize>,
    syms: Vec<Sym>,
    /// Per section, its item symbols by address: what a reference to the
    /// section's own symbol resolves through ([`Self::referent`]). A large
    /// codegen unit has a million relocations and tens of thousands of
    /// sections, and a scan of the symbol table per relocation took 20 s on one.
    items: HashMap<u32, Vec<u32>>,
}

fn rd16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(b[at..at + 2].try_into().expect("2 bytes"))
}
fn rd32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes"))
}
fn rd64(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().expect("8 bytes"))
}
fn wr32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn wr64(b: &mut [u8], at: usize, v: u64) {
    b[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

fn cstr(b: &[u8], at: usize) -> String {
    let tail = b.get(at..).unwrap_or(&[]);
    let end = tail.iter().position(|&c| c == 0).unwrap_or(tail.len());
    String::from_utf8_lossy(&tail[..end]).into_owned()
}

impl Elf {
    fn parse(b: &[u8]) -> Result<Self> {
        if b.len() < EHDR_SIZE || &b[..4] != b"\x7fELF" {
            bail!("not an ELF file");
        }
        if b[4] != elf::ELFCLASS64 || b[5] != elf::ELFDATA2LSB {
            bail!("not a 64-bit little-endian ELF object");
        }
        if rd16(b, 16) != elf::ET_REL {
            bail!("not a relocatable object");
        }
        if rd16(b, 18) != elf::EM_AARCH64 {
            bail!("not an arm64 object");
        }
        let shoff = rd64(b, 0x28) as usize;
        anyhow::ensure!(rd16(b, 0x3a) as usize == SHDR_SIZE, "unexpected section header size");
        let header = |i: usize| shoff + i * SHDR_SIZE;
        // ELF's extended numbering: a file with 0xff00 sections or more (each
        // function has a section of its own and most have a relocation section,
        // so a codegen unit of 33,000 functions gets there) has 0 in `e_shnum`,
        // and the count in section 0's `sh_size`. Likewise `e_shstrndx`, whose
        // value then lives in section 0's `sh_link`.
        let shnum = match rd16(b, 0x3c) as usize {
            0 if shoff != 0 => rd64(b, header(0) + 32) as usize,
            n => n,
        };
        let shstrndx = match rd16(b, 0x3e) {
            SHN_XINDEX => rd32(b, header(0) + 40) as usize,
            n => n as usize,
        };
        let names_at = rd64(b, header(shstrndx) + 24) as usize;
        let mut sections = Vec::with_capacity(shnum);
        for i in 0..shnum {
            let h = header(i);
            sections.push(Section {
                index: i,
                header_at: h,
                name: cstr(b, names_at + rd32(b, h) as usize),
                sh_type: rd32(b, h + 4),
                flags: rd64(b, h + 8),
                offset: rd64(b, h + 24),
                size: rd64(b, h + 32),
                link: rd32(b, h + 40),
                info: rd32(b, h + 44),
            });
        }
        let symtab = sections.iter().position(|s| s.sh_type == elf::SHT_SYMTAB).context("no symbol table")?;
        let symtab_shndx =
            sections.iter().position(|s| s.sh_type == elf::SHT_SYMTAB_SHNDX && s.link as usize == symtab);
        let strtab = &sections[sections[symtab].link as usize];
        let strings = strtab.offset as usize;
        let count = sections[symtab].size as usize / SYM_SIZE;
        let base = sections[symtab].offset as usize;
        let syms = (0..count)
            .map(|i| {
                let e = base + i * SYM_SIZE;
                let shndx = match rd16(b, e + 6) {
                    SHN_XINDEX => {
                        let table = symtab_shndx.context("a symbol's section index is in a table the file lacks")?;
                        rd32(b, sections[table].offset as usize + i * 4)
                    }
                    n if n >= elf::SHN_LORESERVE => SHN_RESERVED,
                    n => u32::from(n),
                };
                Ok(Sym {
                    name: cstr(b, strings + rd32(b, e) as usize),
                    info: b[e + 4],
                    shndx,
                    value: rd64(b, e + 8),
                    size: rd64(b, e + 16),
                })
            })
            .collect::<Result<Vec<Sym>>>()?;
        let mut items: HashMap<u32, Vec<u32>> = HashMap::new();
        for (i, n) in syms.iter().enumerate() {
            if is_item(&n.name) && matches!(n.kind(), elf::STT_OBJECT | elf::STT_TLS | elf::STT_FUNC | elf::STT_NOTYPE) {
                items.entry(n.shndx).or_default().push(i as u32);
            }
        }
        for members in items.values_mut() {
            members.sort_by_key(|&i| syms[i as usize].value);
        }
        Ok(Self { sections, symtab, symtab_shndx, syms, items })
    }

    fn section(&self, index: u32) -> Option<&Section> {
        self.sections.get(index as usize)
    }

    /// Whether a defined, named symbol is a static that holds state.
    fn is_state(&self, sym: &Sym) -> bool {
        sym.is_defined()
            && is_shared_static(&sym.name)
            && matches!(sym.kind(), elf::STT_OBJECT | elf::STT_TLS | elf::STT_NOTYPE)
            && self.section(sym.shndx).is_some_and(|s| is_state_section(&s.name))
    }

    /// The named symbol a reference to `sym` + `addend` lands in, with the
    /// offset into it: `sym` itself, or for a section symbol the named
    /// symbol of that section covering the address.
    fn referent(&self, sym: usize, addend: i64) -> Option<(&Sym, i64)> {
        let s = self.syms.get(sym)?;
        if s.kind() != elf::STT_SECTION {
            return Some((s, addend));
        }
        let at = u64::try_from(addend).ok()?;
        let members = self.items.get(&s.shndx)?;
        let upto = members.partition_point(|&i| self.syms[i as usize].value <= at);
        members[..upto]
            .iter()
            .rev()
            .map(|&i| &self.syms[i as usize])
            .find(|n| at < n.value + n.size.max(1))
            .map(|n| (n, (at - n.value) as i64))
    }
}

/// Whether a symbol names an item rather than marking the code: arm64
/// ELF objects carry mapping symbols (`$x` before code, `$d` before data)
/// in every section, and the binary has them too — paired by name, a
/// `$d` of the patch bound to an unrelated `$d` of the binary.
pub fn is_item(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('$')
}

/// Whether a symbol in a state section is a static the patch shares with
/// the binary. Not the unwinder's `DW.ref.<personality>` words: each
/// object's copy is a constant pointer to the personality routine, kept in
/// writable data only so the loader can relocate it, and an exception
/// table's `PREL32` to it cannot reach the binary anyway.
fn is_shared_static(name: &str) -> bool {
    is_item(name) && !name.starts_with("DW.ref.")
}

// ── binding ──────────────────────────────────────────────────────────

/// What references to one named symbol become.
#[derive(Clone)]
enum Target {
    /// A static the patch defines and the binary has: references move to
    /// a new symbol bound to the binary's address.
    Renamed { host: u64 },
    /// A static the patch references and the stub defines at the binary's
    /// address: references keep the name and only change form (or, with
    /// an offset, move to a symbol of their own).
    InPlace { host: u64 },
    /// A thread-local the binary has: references are relaxed to its
    /// thread-pointer offset.
    Tls { host: u64 },
}

/// [`super::bind::bind`] for ELF objects, for a binary loaded at `slide`.
/// [`Bindings::absolute`] names the cells the stub must define.
pub fn bind(
    tip_objs: &[PathBuf],
    cache: &HostBinCache,
    out_dir: &Path,
    slide: u64,
    base: &dyn Fn(&str) -> Option<HashSet<String>>,
) -> Result<(Vec<PathBuf>, Bindings)> {
    let mut parsed = Vec::with_capacity(tip_objs.len());
    for path in tip_objs {
        let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        let obj = Elf::parse(&bytes).with_context(|| format!("parse {}", path.display()))?;
        parsed.push((path, bytes, obj));
    }
    let mut patch_state: Vec<&str> = Vec::new();
    let mut defined: HashSet<&str> = HashSet::new();
    for (_, _, obj) in &parsed {
        for sym in &obj.syms {
            if sym.is_defined() && sym.is_global() && !sym.name.is_empty() {
                defined.insert(&sym.name);
            }
            if obj.is_state(sym) {
                patch_state.push(&sym.name);
            }
        }
    }
    let bound = statics::pair_vouched(patch_state.iter().copied(), &cache.state_symbols, base).map_err(|e| anyhow::anyhow!("{e}"))?;

    let mut bindings = Bindings::default();
    let mut out = Vec::with_capacity(parsed.len());
    for (path, bytes, obj) in &parsed {
        let target = |sym: &Sym| -> Option<Target> {
            let host = cache.symbols.get(&sym.name)?;
            let tls = host.kind == object::SymbolKind::Tls;
            if bound.contains(sym.name.as_str()) {
                return Some(if tls { Target::Tls { host: host.address } } else { Target::Renamed { host: host.address } });
            }
            if sym.is_undefined() && !defined.contains(sym.name.as_str()) && host.kind != object::SymbolKind::Text {
                return Some(if tls { Target::Tls { host: host.address } } else { Target::InPlace { host: host.address } });
            }
            None
        };
        let ran = |name: &str| cache.symbols.get(name).is_some_and(|s| s.kind == object::SymbolKind::Text);
        let tp = cache.tls_block_offset;
        match rewrite(bytes, obj, &target, &ran, tp, slide, &mut bindings)
            .with_context(|| format!("binding statics in {}", path.display()))?
        {
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

/// Function bodies the patch never runs: constructors the binary already
/// ran, which [`rewrite`] points at [`CTOR_RAN`]. `(section, start, end)`.
fn dormant_code(bytes: &[u8], obj: &Elf, ran: &dyn Fn(&str) -> bool) -> Vec<(u32, u64, u64)> {
    let mut out = Vec::new();
    for rela in obj.sections.iter().filter(|s| s.sh_type == elf::SHT_RELA) {
        let Some(home) = obj.sections.get(rela.info as usize) else { continue };
        if home.sh_type != elf::SHT_INIT_ARRAY {
            continue;
        }
        for i in 0..(rela.size as usize / RELA_SIZE) {
            let at = rela.offset as usize + i * RELA_SIZE;
            let info = rd64(bytes, at + 8);
            if info as u32 != elf::R_AARCH64_ABS64 {
                continue;
            }
            let Some((ctor, inner)) = obj.referent((info >> 32) as usize, rd64(bytes, at + 16) as i64) else { continue };
            if inner != 0 || !ctor.is_defined() || !ran(&ctor.name) {
                continue;
            }
            out.push((ctor.shndx, ctor.value, ctor.value + ctor.size.max(1)));
        }
    }
    out
}

/// The instruction a TLS relocation's word becomes, relaxed to the
/// thread-pointer offset `tprel`. `None` for a form that cannot be.
pub fn relax_tls(r_type: u32, insn: u32, tprel: u64) -> Result<u32> {
    anyhow::ensure!(tprel < 1 << 32, "thread-local offset {tprel:#x} does not fit two 16-bit immediates");
    let g1 = ((tprel >> 16) & 0xffff) as u32;
    let g0 = (tprel & 0xffff) as u32;
    let movz_lsl16 = |rd: u32| 0xd2a0_0000 | (g1 << 5) | rd;
    let movk = |rd: u32| 0xf280_0000 | (g0 << 5) | rd;
    const NOP: u32 = 0xd503_201f;
    Ok(match r_type {
        // Descriptor: `adrp x0` / `ldr x1, [x0]` / `add x0, x0` / `blr x1`.
        // The ABI fixes the result register to x0.
        elf::R_AARCH64_TLSDESC_ADR_PAGE21 => movz_lsl16(0),
        elf::R_AARCH64_TLSDESC_LD64_LO12 => {
            anyhow::ensure!((insn >> 5) & 0x1f == 0, "a TLS descriptor load not based on x0 ({insn:#010x})");
            movk(0)
        }
        elf::R_AARCH64_TLSDESC_ADD_LO12 | elf::R_AARCH64_TLSDESC_CALL | elf::R_AARCH64_TLSDESC_LDR => NOP,
        // Initial-exec: `adrp xN, :gottprel:` / `ldr xN, [xN, :gottprel_lo12:]`.
        elf::R_AARCH64_TLSIE_ADR_GOTTPREL_PAGE21 => movz_lsl16(insn & 0x1f),
        elf::R_AARCH64_TLSIE_LD64_GOTTPREL_LO12_NC => movk(insn & 0x1f),
        // Local-exec: fill the immediates the linker would have.
        elf::R_AARCH64_TLSLE_ADD_TPREL_HI12 => {
            anyhow::ensure!(tprel < 1 << 24, "thread-local offset {tprel:#x} out of add range");
            (insn & !(0xfff << 10)) | ((((tprel >> 12) & 0xfff) as u32) << 10)
        }
        elf::R_AARCH64_TLSLE_ADD_TPREL_LO12 | elf::R_AARCH64_TLSLE_ADD_TPREL_LO12_NC => {
            (insn & !(0xfff << 10)) | (((tprel & 0xfff) as u32) << 10)
        }
        elf::R_AARCH64_TLSLE_MOVW_TPREL_G1 | elf::R_AARCH64_TLSLE_MOVW_TPREL_G1_NC => (insn & !(0xffff << 5)) | (g1 << 5),
        elf::R_AARCH64_TLSLE_MOVW_TPREL_G0 | elf::R_AARCH64_TLSLE_MOVW_TPREL_G0_NC => (insn & !(0xffff << 5)) | (g0 << 5),
        other => bail!("thread-local reached through relocation type {other}, which a patch cannot relax"),
    })
}

/// Apply the binding to one object. `None` when nothing changed.
fn rewrite(
    bytes: &[u8],
    obj: &Elf,
    target: &dyn Fn(&Sym) -> Option<Target>,
    ran: &dyn Fn(&str) -> bool,
    tls_block: Option<u64>,
    slide: u64,
    bindings: &mut Bindings,
) -> Result<Option<Vec<u8>>> {
    let mut data = bytes.to_vec();
    let mut new_syms: Vec<String> = Vec::new();
    let mut new_index: HashMap<String, u32> = HashMap::new();
    let first_new = obj.syms.len() as u32;
    let mut add_sym = |name: String, new_syms: &mut Vec<String>| -> u32 {
        *new_index.entry(name.clone()).or_insert_with(|| {
            new_syms.push(name);
            first_new + new_syms.len() as u32 - 1
        })
    };
    let dormant = dormant_code(bytes, obj, ran);
    let mut changed = false;

    for rela in obj.sections.iter().filter(|s| s.sh_type == elf::SHT_RELA) {
        let Some(home) = obj.sections.get(rela.info as usize) else { continue };
        // Debug info may keep naming the patch's own copies.
        if home.flags & u64::from(elf::SHF_ALLOC) == 0 {
            continue;
        }
        let ctors = home.sh_type == elf::SHT_INIT_ARRAY;
        for i in 0..(rela.size as usize / RELA_SIZE) {
            let at = rela.offset as usize + i * RELA_SIZE;
            let offset = rd64(bytes, at);
            let info = rd64(bytes, at + 8);
            let addend = rd64(bytes, at + 16) as i64;
            let (sym_index, r_type) = ((info >> 32) as usize, info as u32);
            let Some((sym, inner)) = obj.referent(sym_index, addend) else { continue };
            let insn_at = home.offset as usize + offset as usize;

            if ctors {
                if r_type == elf::R_AARCH64_ABS64 && inner == 0 && sym.is_defined() && ran(&sym.name) {
                    let s = add_sym(CTOR_RAN.to_string(), &mut new_syms);
                    wr64(&mut data, at + 8, (u64::from(s) << 32) | u64::from(r_type));
                    wr64(&mut data, at + 16, 0);
                    bindings.ctor_ran = true;
                    changed = true;
                }
                continue;
            }
            let Some(t) = target(sym) else { continue };
            let in_dormant =
                dormant.iter().any(|&(sec, start, end)| sec as usize == home.index && (start..end).contains(&offset));
            if in_dormant && matches!(t, Target::Renamed { .. } | Target::Tls { .. }) && sym.is_defined() {
                // Never runs; the patch's own copy satisfies the linker.
                continue;
            }
            let name = &sym.name;
            match t {
                Target::Tls { host } => {
                    let block = tls_block.context("the running binary has no TLS segment")?;
                    let tprel = block + host + inner as u64;
                    let insn = rd32(bytes, insn_at);
                    let relaxed = relax_tls(r_type, insn, tprel)
                        .with_context(|| format!("relaxing a reference to thread-local `{name}`"))?;
                    wr32(&mut data, insn_at, relaxed);
                    // R_AARCH64_NONE, against no symbol.
                    wr64(&mut data, at + 8, 0);
                    wr64(&mut data, at + 16, 0);
                    changed = true;
                }
                Target::Renamed { host } | Target::InPlace { host } => {
                    let address = (host as i64 + inner) as u64;
                    let new_type = match r_type {
                        elf::R_AARCH64_ABS64 => {
                            // The address itself, written where the linker
                            // would have; the relocation goes.
                            wr64(&mut data, insn_at, address.wrapping_add(slide));
                            wr64(&mut data, at + 8, 0);
                            wr64(&mut data, at + 16, 0);
                            changed = true;
                            continue;
                        }
                        elf::R_AARCH64_ADR_PREL_PG_HI21
                        | elf::R_AARCH64_ADR_PREL_PG_HI21_NC
                        | elf::R_AARCH64_ADR_GOT_PAGE => elf::R_AARCH64_ADR_PREL_PG_HI21,
                        elf::R_AARCH64_ADD_ABS_LO12_NC => {
                            let insn = rd32(bytes, insn_at);
                            // ADD (immediate), 64-bit, unshifted.
                            if insn & 0xffc0_0000 != 0x9100_0000 {
                                bail!("`{name}`'s address is added by an unexpected instruction {insn:#010x}");
                            }
                            let (rd, rn) = (insn & 0x1f, (insn >> 5) & 0x1f);
                            if rd == 31 {
                                bail!("`{name}`'s address is added into sp");
                            }
                            wr32(&mut data, insn_at, 0xf940_0000 | (rn << 5) | rd);
                            elf::R_AARCH64_LDST64_ABS_LO12_NC
                        }
                        elf::R_AARCH64_LD64_GOT_LO12_NC => elf::R_AARCH64_LDST64_ABS_LO12_NC,
                        elf::R_AARCH64_LDST8_ABS_LO12_NC
                        | elf::R_AARCH64_LDST16_ABS_LO12_NC
                        | elf::R_AARCH64_LDST32_ABS_LO12_NC
                        | elf::R_AARCH64_LDST64_ABS_LO12_NC
                        | elf::R_AARCH64_LDST128_ABS_LO12_NC => bail!(
                            "a load or store addresses `{name}` directly; a patch can only reach the \
                             running binary's copy through an address it materializes"
                        ),
                        other => bail!("`{name}` is the target of relocation type {other}, which cannot be bound"),
                    };
                    let cell = statics::host_name(name, inner);
                    bindings.absolute.insert(cell.clone(), address);
                    let s = add_sym(cell, &mut new_syms);
                    wr64(&mut data, at + 8, (u64::from(s) << 32) | u64::from(new_type));
                    wr64(&mut data, at + 16, 0);
                    changed = true;
                }
            }
        }
    }
    if !changed {
        return Ok(None);
    }
    if !new_syms.is_empty() {
        append_undefined(&mut data, obj, &new_syms)?;
    }
    Ok(Some(data))
}

/// Append `names` as global undefined symbols: copy the symbol and string
/// tables to the end of the file with the new entries and point their
/// section headers at the copies.
fn append_undefined(data: &mut Vec<u8>, obj: &Elf, names: &[String]) -> Result<()> {
    let symtab = &obj.sections[obj.symtab];
    let strtab = &obj.sections[symtab.link as usize];
    let mut strings = data[strtab.offset as usize..(strtab.offset + strtab.size) as usize].to_vec();
    let mut table = data[symtab.offset as usize..(symtab.offset + symtab.size) as usize].to_vec();
    for name in names {
        let at = strings.len() as u32;
        strings.extend_from_slice(name.as_bytes());
        strings.push(0);
        table.extend_from_slice(&at.to_le_bytes());
        table.push((elf::STB_GLOBAL << 4) | elf::STT_NOTYPE);
        table.push(elf::STV_DEFAULT);
        table.extend_from_slice(&elf::SHN_UNDEF.to_le_bytes());
        table.extend_from_slice(&0u64.to_le_bytes());
        table.extend_from_slice(&0u64.to_le_bytes());
    }
    while data.len() % 8 != 0 {
        data.push(0);
    }
    let table_at = data.len() as u64;
    data.extend_from_slice(&table);
    let strings_at = data.len() as u64;
    data.extend_from_slice(&strings);
    wr64(data, symtab.header_at + 24, table_at);
    wr64(data, symtab.header_at + 32, table.len() as u64);
    wr64(data, strtab.header_at + 24, strings_at);
    wr64(data, strtab.header_at + 32, strings.len() as u64);
    // One word per symbol, or the linker rejects the table.
    if let Some(table) = obj.symtab_shndx.map(|i| &obj.sections[i]) {
        let mut words = data[table.offset as usize..(table.offset + table.size) as usize].to_vec();
        words.resize(words.len() + names.len() * 4, 0);
        let words_at = data.len() as u64;
        data.extend_from_slice(&words);
        wr64(data, table.header_at + 24, words_at);
        wr64(data, table.header_at + 32, words.len() as u64);
    }
    Ok(())
}

// ── roots ────────────────────────────────────────────────────────────

/// Set [`SHF_GNU_RETAIN`] on the section of every symbol named in `roots`,
/// in place, so `--gc-sections` keeps each redirected function even when
/// nothing in the patch calls it. Marks the object GNU-ABI so the linker
/// reads the flag ([`ELFOSABI_GNU`]).
pub fn retain_roots(bytes: &mut [u8], roots: &std::collections::BTreeSet<String>) -> Result<()> {
    let obj = Elf::parse(bytes)?;
    let sections: HashSet<u32> =
        obj.syms.iter().filter(|s| s.is_defined() && roots.contains(&s.name)).map(|s| s.shndx).collect();
    if sections.is_empty() {
        return Ok(());
    }
    match bytes[EI_OSABI] {
        elf::ELFOSABI_NONE => bytes[EI_OSABI] = ELFOSABI_GNU,
        ELFOSABI_GNU => {}
        other => bail!("the object declares OS ABI {other}, under which the linker may not keep its roots"),
    }
    for index in sections {
        let Some(section) = obj.section(index) else { continue };
        let flags = section.flags | SHF_GNU_RETAIN;
        wr64(bytes, section.header_at + 8, flags);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use object::{Object as _, ObjectSection as _, ObjectSymbol as _};

    /// An arm64 object with more sections than `e_shnum` can hold, as a large
    /// codegen unit is: `func` (with an address of `STATE` materialized, the
    /// way a patch's code reaches a static) and `.bss.STATE` both sit past
    /// section 0xff00, so their symbols need `SHT_SYMTAB_SHNDX`.
    fn object_past_16_bit_sections() -> Vec<u8> {
        use object::write::{Object, Relocation, Symbol, SymbolSection};
        use object::{RelocationFlags, SectionKind, SymbolFlags, SymbolKind, SymbolScope};
        let mut obj = Object::new(object::BinaryFormat::Elf, object::Architecture::Aarch64, object::Endianness::Little);
        for i in 0..0x1_0100 {
            obj.add_section(Vec::new(), format!(".text.filler{i}").into_bytes(), SectionKind::Text);
        }
        let bss = obj.add_section(Vec::new(), b".bss.STATE".to_vec(), SectionKind::UninitializedData);
        let offset = obj.append_section_bss(bss, 8, 8);
        let state = obj.add_symbol(Symbol {
            name: b"STATE".to_vec(),
            value: offset,
            size: 8,
            kind: SymbolKind::Data,
            scope: SymbolScope::Linkage,
            weak: false,
            section: SymbolSection::Section(bss),
            flags: SymbolFlags::None,
        });
        let text = obj.add_section(Vec::new(), b".text.func".to_vec(), SectionKind::Text);
        // adrp x8, STATE; add x8, x8, :lo12:STATE; ret
        let code: Vec<u8> = [0x9000_0008u32, 0x9100_0108, 0xd65f_03c0].iter().flat_map(|w| w.to_le_bytes()).collect();
        let at = obj.append_section_data(text, &code, 4);
        obj.add_symbol(Symbol {
            name: b"func".to_vec(),
            value: at,
            size: code.len() as u64,
            kind: SymbolKind::Text,
            scope: SymbolScope::Linkage,
            weak: false,
            section: SymbolSection::Section(text),
            flags: SymbolFlags::None,
        });
        for (offset, r_type) in [(0, elf::R_AARCH64_ADR_PREL_PG_HI21), (4, elf::R_AARCH64_ADD_ABS_LO12_NC)] {
            obj.add_relocation(
                text,
                Relocation { offset: at + offset, symbol: state, addend: 0, flags: RelocationFlags::Elf { r_type } },
            )
            .unwrap();
        }
        obj.write().unwrap()
    }

    /// CrewForge A17: `crewforge-api-server` compiles to a codegen unit of
    /// 66,224 sections, and `e_shnum` is 0 in a file that size. The parser
    /// read no sections at all, and every patch of the crate failed with "no
    /// symbol table" after its whole replay.
    #[test]
    fn regression_an_object_with_more_sections_than_e_shnum_holds_parses() {
        let bytes = object_past_16_bit_sections();
        assert_eq!(rd16(&bytes, 0x3c), 0, "the fixture must need extended numbering");
        let obj = Elf::parse(&bytes).expect("extended section numbering");
        assert!(obj.sections.len() > 0xff00);
        let state = obj.syms.iter().find(|s| s.name == "STATE").expect("STATE");
        assert!(state.is_defined(), "a section index past 0xff00 is a definition, not a reserved value");
        assert_eq!(obj.section(state.shndx).unwrap().name, ".bss.STATE");
        let func = obj.syms.iter().find(|s| s.name == "func").expect("func");
        assert_eq!(obj.section(func.shndx).unwrap().name, ".text.func");
        // What a mis-read index would silently do: leave the patch its own
        // copy of the static, so base code and patched code disagree.
        assert!(obj.is_state(state), "a static past 0xff00 must be bound like any other");
    }

    /// Binding appends symbols to the table, and `SHT_SYMTAB_SHNDX` has a word
    /// per symbol: left short, the linker rejects the edited object.
    #[test]
    fn regression_binding_past_16_bit_sections_keeps_the_object_valid() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("big.rcgu.o");
        std::fs::write(&input, object_past_16_bit_sections()).unwrap();
        let mut cache = HostBinCache::default();
        cache.format = object::BinaryFormat::Elf;
        cache.symbols.insert(
            "STATE".into(),
            crate::hotpatch::cache::CachedSymbol { address: 0x4000, kind: object::SymbolKind::Data, size: 8, is_weak: false },
        );
        cache.state_symbols = statics::ProgramStatics::new(["STATE"]);
        let (out, bindings) = bind(&[input], &cache, &dir.path().join("bound"), 0x1000, &|_| None).expect("bind");
        assert_eq!(bindings.absolute.get(&statics::host_name("STATE", 0)), Some(&0x4000));

        let bytes = std::fs::read(&out[0]).unwrap();
        let read = object::File::parse(&*bytes).expect("the edited object still parses");
        let cell = read.symbols().find(|s| s.name() == Ok("STATE$idealyst_host")).expect("the cell's symbol");
        assert!(cell.is_undefined());
        let func = read.symbols().find(|s| s.name() == Ok("func")).unwrap();
        let section = read.section_by_index(func.section_index().unwrap()).unwrap();
        assert_eq!(section.name(), Ok(".text.func"), "a symbol past 0xff00 kept its section");
        let parsed = Elf::parse(&bytes).unwrap();
        let table = &parsed.sections[parsed.symtab_shndx.expect("the table survives")];
        assert_eq!(table.size as usize / 4, parsed.syms.len(), "one SYMTAB_SHNDX word per symbol");
    }

    /// A reference through a section's own symbol lands in the named symbol
    /// covering the offset; the innermost when several start before it.
    #[test]
    fn a_section_symbols_reference_finds_the_item_at_its_offset() {
        let sym = |name: &str, info: u8, value: u64, size: u64| Sym { name: name.into(), info, shndx: 7, value, size };
        let syms = vec![
            sym("", elf::STT_SECTION, 0, 0),
            sym("$d", elf::STT_NOTYPE, 0, 0),
            sym("outer", elf::STT_OBJECT, 0, 64),
            sym("inner", elf::STT_OBJECT, 16, 8),
            sym("later", elf::STT_OBJECT, 128, 8),
        ];
        let items = HashMap::from([(7, vec![2, 3, 4])]);
        let obj = Elf { sections: Vec::new(), symtab: 0, symtab_shndx: None, syms, items };
        let at = |addend| obj.referent(0, addend).map(|(n, inner)| (n.name.clone(), inner));
        assert_eq!(at(4), Some(("outer".to_string(), 4)));
        assert_eq!(at(20), Some(("inner".to_string(), 4)));
        assert_eq!(at(32), Some(("outer".to_string(), 32)), "past the inner one, still in the outer");
        assert_eq!(at(100), None);
        assert_eq!(at(130), Some(("later".to_string(), 2)));
    }

    /// A function past section 0xff00 is a root like any other: unretained, the
    /// linker drops the very code the patch exists to carry.
    #[test]
    fn regression_a_root_past_16_bit_sections_is_retained() {
        let mut bytes = object_past_16_bit_sections();
        retain_roots(&mut bytes, &std::collections::BTreeSet::from(["func".to_string()])).unwrap();
        let read = object::File::parse(&*bytes).unwrap();
        let func = read.symbols().find(|s| s.name() == Ok("func")).unwrap();
        let section = read.section_by_index(func.section_index().unwrap()).unwrap();
        match section.flags() {
            object::SectionFlags::Elf { sh_flags } => assert_ne!(sh_flags & SHF_GNU_RETAIN, 0),
            other => panic!("{other:?}"),
        }
    }

    /// The unwinder's personality word and arm64 mapping symbols sit in
    /// writable data on both sides; pairing either by name bound the patch
    /// to an unrelated (or unreachable) word of the binary.
    #[test]
    fn regression_mapping_symbols_and_personality_words_are_never_bound() {
        assert!(!is_shared_static("$d"));
        assert!(!is_shared_static("$x.12"));
        assert!(!is_shared_static("DW.ref.rust_eh_personality"));
        assert!(is_shared_static("_RNvCs1a_6shared5CALLS"));
    }

    #[test]
    fn the_executable_tls_block_follows_the_tcb_at_its_alignment() {
        assert_eq!(tls_block_offset(8), 16);
        assert_eq!(tls_block_offset(16), 16);
        assert_eq!(tls_block_offset(64), 64);
        assert_eq!(tls_block_offset(0), 16);
    }

    #[test]
    fn state_sections_are_writable_data_zero_fill_and_tls() {
        for s in [".data", ".data.foo", ".bss", ".bss._RNvC1a_1x5COUNT", ".tdata", ".tbss.x"] {
            assert!(is_state_section(s), "{s}");
        }
        for s in [".data.rel.ro", ".data.rel.ro..Lanon.1", ".rodata", ".text", ".dataX", ".init_array"] {
            assert!(!is_state_section(s), "{s}");
        }
    }

    /// The descriptor sequence becomes `movz x0, #g1, lsl 16; movk x0,
    /// #g0; nop; nop` — GNU ld's own TLSDESC → local-exec relaxation.
    #[test]
    fn a_tls_descriptor_relaxes_to_the_binarys_thread_pointer_offset() {
        let tprel = 0x0012_0040;
        assert_eq!(relax_tls(elf::R_AARCH64_TLSDESC_ADR_PAGE21, 0x9000_0000, tprel).unwrap(), 0xd2a0_0240);
        // ldr x1, [x0, #0]
        assert_eq!(relax_tls(elf::R_AARCH64_TLSDESC_LD64_LO12, 0xf940_0001, tprel).unwrap(), 0xf280_0800);
        assert_eq!(relax_tls(elf::R_AARCH64_TLSDESC_ADD_LO12, 0x9100_0000, tprel).unwrap(), 0xd503_201f);
        assert_eq!(relax_tls(elf::R_AARCH64_TLSDESC_CALL, 0xd63f_0020, tprel).unwrap(), 0xd503_201f);
    }

    #[test]
    fn initial_and_local_exec_forms_relax_too() {
        // adrp x8 / ldr x8, [x8]
        assert_eq!(relax_tls(elf::R_AARCH64_TLSIE_ADR_GOTTPREL_PAGE21, 0x9000_0008, 0x40).unwrap(), 0xd2a0_0008);
        assert_eq!(relax_tls(elf::R_AARCH64_TLSIE_LD64_GOTTPREL_LO12_NC, 0xf940_0108, 0x40).unwrap(), 0xf280_0808);
        // add x8, x8, #0, lsl 12 / add x8, x8, #0
        assert_eq!(relax_tls(elf::R_AARCH64_TLSLE_ADD_TPREL_HI12, 0x9140_0108, 0x1040).unwrap(), 0x9140_0508);
        assert_eq!(relax_tls(elf::R_AARCH64_TLSLE_ADD_TPREL_LO12_NC, 0x9100_0108, 0x1040).unwrap(), 0x9101_0108);
    }

    #[test]
    fn a_descriptor_load_off_x0_or_a_global_dynamic_call_is_refused() {
        assert!(relax_tls(elf::R_AARCH64_TLSDESC_LD64_LO12, 0xf940_0021, 0x40).is_err());
        assert!(relax_tls(elf::R_AARCH64_TLSGD_ADR_PAGE21, 0x9000_0000, 0x40).is_err());
        assert!(relax_tls(elf::R_AARCH64_TLSDESC_ADR_PAGE21, 0x9000_0000, 1 << 32).is_err());
    }
}
