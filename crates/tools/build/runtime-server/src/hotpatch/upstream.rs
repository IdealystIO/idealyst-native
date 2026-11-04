//! The running binary's own build products, for what a patch cannot get
//! from the binary itself: the rlibs cargo linked it from.
//!
//! The binary was linked with dead code stripped, so it holds less than its
//! crates compiled. Two questions only the rlibs can answer:
//!
//! - **A generic instantiation the patch's live code needs and the binary
//!   stripped.** rustc at a low `opt-level` shares instantiations between
//!   crates (`-Zshare-generics`, on by default there): a crate calling
//!   `<[f32]>::get::<usize>` refers to the copy an upstream crate already
//!   compiled, named for that crate, and the linker keeps it only if base
//!   code used it. A body edit that adds such a call needs a copy nothing
//!   linked. [`Rlibs::pull`] takes it from the rlib, under rules that make
//!   the copy the same code the binary would have run (see [`vet`]).
//! - **Whether a static missing from the binary was stripped or renamed.**
//!   [`Rlibs::vouch`] answers from the crate's own rlib, which holds every
//!   static it compiled ([`super::statics::pair_vouched`]).
//!
//! An rlib speaks for the base only if the base's link came after it: a
//! later compile (a session check, a rebuild that failed further up) leaves
//! a newer rlib and an unchanged binary, and then neither answer is known.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use anyhow::{bail, Context, Result};
use object::read::archive::ArchiveFile;
use object::{Object, ObjectSection, ObjectSymbol, ReadCache, SymbolKind};

use super::statics::ProgramStatics;

/// The rlibs a patch may read. Cheap to create; everything is read on demand,
/// because most patches never ask.
pub struct Rlibs {
    /// `-L dependency=` directories of the replayed crates, then the
    /// toolchain's own rlibs.
    dirs: Vec<PathBuf>,
    /// When the base was linked. `None`: unknown, and no rlib speaks for it.
    base_linked: Option<SystemTime>,
    /// Each replayed crate's statics by its rlib, read at most once.
    statics: Mutex<HashMap<PathBuf, Option<ProgramStatics>>>,
}

/// One object taken out of an rlib.
pub struct Pulled {
    pub path: PathBuf,
}

impl Rlibs {
    pub fn new(dirs: Vec<PathBuf>, base_linked: Option<SystemTime>) -> Self {
        Self { dirs, base_linked, statics: Mutex::default() }
    }

    fn speaks_for_base(&self, rlib: &Path) -> bool {
        let built = std::fs::metadata(rlib).and_then(|m| m.modified()).ok();
        matches!((built, self.base_linked), (Some(built), Some(linked)) if built <= linked)
    }

    /// Whether a toolchain rlib, which no build of the project rewrites.
    fn in_toolchain(&self, rlib: &Path) -> bool {
        rlib.components().any(|c| c.as_os_str() == "rustlib")
    }

    /// Every rlib of the search directories, those `crate_hint` names first.
    fn candidates(&self, crate_hint: Option<&str>) -> Vec<PathBuf> {
        let mut named = Vec::new();
        let mut rest = Vec::new();
        for dir in &self.dirs {
            let Ok(entries) = std::fs::read_dir(dir) else { continue };
            for e in entries.flatten() {
                let path = e.path();
                let Some(file) = path.file_name().and_then(|f| f.to_str()) else { continue };
                if !file.ends_with(".rlib") {
                    continue;
                }
                let stem = file.trim_start_matches("lib").trim_end_matches(".rlib");
                let krate = stem.split('-').next().unwrap_or(stem);
                if crate_hint == Some(krate) {
                    named.push(path);
                } else {
                    rest.push(path);
                }
            }
        }
        named.extend(rest);
        named
    }

    /// Take the objects defining `missing` out of the rlibs into `out_dir`.
    /// `done` holds the members already taken. An `Err` is a refusal naming
    /// the first symbol with no safe copy.
    pub fn pull(&self, missing: &[String], done: &mut HashSet<(PathBuf, String)>, out_dir: &Path) -> Result<Vec<Pulled>> {
        std::fs::create_dir_all(out_dir).with_context(|| format!("create {}", out_dir.display()))?;
        let mut out = Vec::new();
        for symbol in missing {
            let (rlib, offset, member) = self
                .locate(symbol)?
                .with_context(|| format!("`{symbol}` is in no rlib this build linked"))?;
            let key = (rlib.clone(), member.clone());
            if done.contains(&key) {
                bail!("`{symbol}` is still undefined with its rlib member `{member}` in the patch");
            }
            if !self.in_toolchain(&rlib) && !self.speaks_for_base(&rlib) {
                bail!(
                    "`{symbol}` is in {}, which was compiled after the running binary was linked",
                    rlib.display()
                );
            }
            let file = std::fs::File::open(&rlib).with_context(|| format!("open {}", rlib.display()))?;
            let cache = ReadCache::new(file);
            let archive = ArchiveFile::parse(&cache).with_context(|| format!("parse {}", rlib.display()))?;
            let entry = archive.member(object::read::archive::ArchiveOffset(offset))?;
            let bytes = entry.data(&cache).with_context(|| format!("read `{member}` of {}", rlib.display()))?;
            vet(bytes).with_context(|| format!("`{symbol}` is in `{member}` of {}", rlib.display()))?;
            let path = out_dir.join(format!("{:04}-{}", done.len(), member.replace('/', "_")));
            std::fs::write(&path, bytes).with_context(|| format!("write {}", path.display()))?;
            done.insert(key);
            out.push(Pulled { path });
        }
        Ok(out)
    }

    /// The rlib, archive offset and member name defining `symbol`.
    fn locate(&self, symbol: &str) -> Result<Option<(PathBuf, u64, String)>> {
        for rlib in self.candidates(instantiating_crate(symbol)) {
            let Ok(file) = std::fs::File::open(&rlib) else { continue };
            let cache = ReadCache::new(file);
            let Ok(archive) = ArchiveFile::parse(&cache) else { continue };
            let Ok(Some(mut symbols)) = archive.symbols() else { continue };
            let found = symbols.find_map(|s| {
                let s = s.ok()?;
                (s.name() == symbol.as_bytes()).then_some(s.offset())
            });
            if let Some(offset) = found {
                let entry = archive.member(offset)?;
                let member = String::from_utf8_lossy(entry.name()).into_owned();
                return Ok(Some((rlib.clone(), offset.0, member)));
            }
        }
        Ok(None)
    }

    /// The statics `rlib` holds under [`super::statics::static_key`] `key`,
    /// if it speaks for the base.
    pub fn vouch(&self, rlib: &Path, key: &str) -> Option<HashSet<String>> {
        if !self.speaks_for_base(rlib) {
            return None;
        }
        let mut statics = self.statics.lock().unwrap();
        let read = statics.entry(rlib.to_path_buf()).or_insert_with(|| read_statics(rlib).ok());
        read.as_ref()?.group(key).cloned()
    }
}

/// The crate whose rlib a v0 instantiation's symbol names: the path it ends
/// in, `…C<disambiguator>_<len><name>`.
fn instantiating_crate(symbol: &str) -> Option<&str> {
    let digits_end = symbol.len() - symbol.bytes().rev().take_while(|b| b.is_ascii_alphanumeric() || *b == b'_').count();
    let tail = &symbol[digits_end..];
    // The name is the longest suffix whose decimal length prefix matches.
    (0..tail.len()).rev().find_map(|at| {
        let name = &tail[at..];
        let before = &tail[..at];
        let run = before.bytes().rev().take_while(u8::is_ascii_digit).count();
        (1..=run).find_map(|n| {
            let len: usize = before[before.len() - n..].parse().ok()?;
            (len == name.len() && !name.is_empty() && !name.as_bytes()[0].is_ascii_digit()).then_some(name)
        })
    })
}

/// Whether a patch may carry its own copy of the code in this object: it
/// must run exactly what the binary's copy would have.
///
/// - **Every function in it is one a patch can never change.** The decision
///   refuses an edit to a generic body, and only plain functions are ever
///   redirected ([`super::detour::plain_function`]). A copy of a plain
///   function would keep running the base's body after a patch redirected
///   the original, so an object holding one is refused. rustc keeps
///   instantiations in codegen units of their own under incremental, which
///   is why this rarely refuses in a dev build.
/// - **It has no constructor.** The binary never pulled this object if it
///   lacks the symbol, or stripped what it pulled; a constructor loaded now
///   would register something the running program never had.
///
/// Statics need no rule here: the patch binds the ones the binary has, and
/// one the binary does not have has no other copy to disagree with. Nor does
/// address identity: Rust does not promise one address per instantiation,
/// and a build above `opt-level = 1` has a copy per crate already.
fn vet(bytes: &[u8]) -> Result<()> {
    let file = object::File::parse(bytes).context("parse the object")?;
    for section in file.sections() {
        let name = section.name().unwrap_or("");
        if name.starts_with(".init_array") || name.starts_with(".ctors") || name == "__mod_init_func" {
            bail!("the object has a constructor ({name})");
        }
    }
    for sym in file.symbols() {
        if sym.is_undefined() || sym.kind() != SymbolKind::Text {
            continue;
        }
        let Ok(name) = sym.name() else { continue };
        if super::detour::plain_function(name) {
            bail!("the object also defines the plain function `{name}`, which a copy would freeze at its base body");
        }
    }
    Ok(())
}

/// Every static an rlib's objects define, grouped for pairing.
fn read_statics(rlib: &Path) -> Result<ProgramStatics> {
    let file = std::fs::File::open(rlib).with_context(|| format!("open {}", rlib.display()))?;
    let cache = ReadCache::new(file);
    let archive = ArchiveFile::parse(&cache).with_context(|| format!("parse {}", rlib.display()))?;
    let mut names = Vec::new();
    for member in archive.members() {
        let member = member?;
        if !member.name().ends_with(b".o") {
            continue;
        }
        let bytes = member.data(&cache)?;
        let Ok(object) = object::File::parse(bytes) else { continue };
        names.extend(super::cache::state_names(&object));
    }
    Ok(ProgramStatics::new(names.iter().map(String::as_str)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_v0_instantiation_names_the_crate_that_holds_it() {
        let elf = "_RINvMNtCsfo0Ls1ZyrQZ_4core5sliceSf3getjECsjXQs0CU8SdR_14runtime_shared";
        assert_eq!(instantiating_crate(elf), Some("runtime_shared"));
        assert_eq!(instantiating_crate(&format!("_{elf}")), Some("runtime_shared"));
        let residual = "_RNvXs_NtNtCsfo0Ls1ZyrQZ_4core3ops12control_flowINtB4_11ControlFlowINtNtB6_9try_trait17NeverShortCircuitjEjEINtB15_12FromResidualIBK_B12_NtNtB8_7convert10InfallibleEE13from_residualCs11hk5gXEwaX_13data_encoding";
        assert_eq!(instantiating_crate(residual), Some("data_encoding"));
    }

    fn assemble(dir: &Path, name: &str, source: &str) -> Option<PathBuf> {
        let s = dir.join(format!("{name}.s"));
        let o = dir.join(format!("{name}.o"));
        std::fs::write(&s, source).unwrap();
        let ok = std::process::Command::new("cc").arg("-c").arg(&s).arg("-o").arg(&o).status().ok()?;
        ok.success().then_some(o)
    }

    /// The object shapes [`vet`] tells apart. A plain function's copy would
    /// keep the base body once a patch redirected the original (a wrong
    /// program, silently), and a new constructor runs code the running
    /// program never ran.
    #[cfg(target_arch = "aarch64")]
    #[test]
    fn only_code_a_patch_cannot_change_may_be_copied() {
        let dir = tempfile::tempdir().unwrap();
        let under = if cfg!(target_os = "macos") { "_" } else { "" };
        // `core::option::Option<usize>::map::<…>`-shaped, and a plain `app::f`.
        let generic = "_RINvMNtCs1_4core6optionINtB3_6OptionjE3mapjECs2_2up";
        let plain = "_RNvCs3_3app1f";
        let func = |name: &str| format!(".text\n.globl {under}{name}\n.p2align 2\n{under}{name}:\n  ret\n");
        let vetted = |source: &str| {
            let o = assemble(dir.path(), "probe", source).expect("cc assembles");
            vet(&std::fs::read(o).unwrap())
        };
        assert!(vetted(&func(generic)).is_ok(), "an instantiation is the same code wherever it lives");
        let err = vetted(&func(plain)).unwrap_err();
        assert!(format!("{err:#}").contains(plain), "{err:#}");
        let ctor = if cfg!(target_os = "macos") {
            format!("{}.section __DATA,__mod_init_func,mod_init_funcs\n.p2align 3\n.quad {under}{generic}\n", func(generic))
        } else {
            format!("{}.section .init_array,\"aw\"\n.p2align 3\n.quad {generic}\n", func(generic))
        };
        let err = vetted(&ctor).unwrap_err();
        assert!(format!("{err:#}").contains("constructor"), "{err:#}");
    }
}
