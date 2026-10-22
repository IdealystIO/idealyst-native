//! Verifying an ADDITIVE patch after its replay.
//!
//! The dev loop lets a save whose shape moved only by new items be a hot
//! patch (`runtime_macros_parse::additive`, `dev_overlay`'s
//! `decide_additive`). Source can show that every existing item is
//! unchanged; it cannot show that every existing item still COMPILES to
//! the symbol it compiled to in the base. That is what the jump table
//! pairs by, so it is what this checks, against the base module itself.
//!
//! # Why a symbol can move without its item changing
//!
//! A symbol is a path, and the anonymous parts of a path are numbered in
//! source order: `{impl#3}`, an anonymous `const _`, a fn-local item's
//! block. An item inserted ahead of them — or a macro expansion queued
//! ahead of theirs — renumbers them. A renamed function has no partner in
//! the base, so the jump table leaves the base's copy in place: base code
//! keeps calling it directly while the patch calls the new copy. Harmless
//! while the two are the same code, and a silent stale edit the moment
//! the same save changed that function's body too.
//!
//! # What is checked
//!
//! Functions are grouped by their path with every number erased
//! ([`build_runtime_server::hotpatch::statics::static_key`]): an item
//! keeps its group however it is renumbered. Over every group of the
//! edited crate's items the base has (its own items, its impls —
//! leaving out closures, shims and generic instantiations, which a body
//! edit legitimately renumbers or drops), the patch must define no name
//! the base does not:
//!
//! - a renumbered item shows up as exactly that: its group in the patch
//!   holds a name the base never had (and usually lacks the old one);
//! - a group that GREW holds a name that may now belong to a different
//!   function of the same path.
//!
//! A base name the patch simply does not define is fine: an edit removed
//! the last call to a private fn, and rustc never compiled it. Nothing in
//! the patched program calls it, and the base's copy is unchanged. Every
//! crate that can name the edited one is in the patch, so a name the base
//! has and the patch lacks is never one a dependent's new code needs.
//! `inventory::submit!`'s `__ctor` groups are left out: the page never
//! runs a patch's constructors.
//!
//! And over the edited crate's statics, that it defines no new WRITABLE
//! one ([`crate::hotpatch_statics::new_state_statics`]): a later patch
//! could not keep its state. A refusal is a rebuild.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use build_runtime_server::hotpatch::statics::static_key;
use wasmparser::{Parser, Payload, SymbolFlags, SymbolInfo};

use crate::hotpatch_aliases::AliasMap;

/// Every function `objects` define, by symbol name.
pub fn defined_functions(objects: &[impl AsRef<Path>]) -> Result<HashSet<String>> {
    let mut out = HashSet::new();
    for path in objects {
        let path = path.as_ref();
        let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        for payload in Parser::new(0).parse_all(&bytes) {
            let Payload::CustomSection(c) = payload? else { continue };
            if c.name() != "linking" {
                continue;
            }
            let reader = wasmparser::LinkingSectionReader::new(wasmparser::BinaryReader::new(
                c.data(),
                c.data_offset(),
            ))?;
            for sub in reader.subsections() {
                let wasmparser::Linking::SymbolTable(table) = sub? else { continue };
                for s in table {
                    if let SymbolInfo::Func { flags, name: Some(name), .. } = s? {
                        if !flags.contains(SymbolFlags::UNDEFINED) {
                            out.insert(name.to_string());
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Why an additive patch was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// A group of same-path functions the base has holds a name in the
    /// patch that the base does not: an item was renumbered, or the
    /// group grew.
    Renamed { key: String, symbol: String },
    /// The crate defines writable statics the running program does not
    /// have.
    NewState { statics: Vec<String> },
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Renamed { key, symbol } => write!(
                f,
                "the patch compiles `{key}` to `{symbol}`, a name the running program does not have: an \
                 item added ahead of it renumbered it, so base code would keep calling the old copy"
            ),
            Refusal::NewState { statics } => write!(
                f,
                "the added code declares writable static(s) the running program does not have ({}); \
                 each later patch would give them a fresh copy",
                statics.join(", ")
            ),
        }
    }
}

/// The base's functions of one crate's items, grouped by path with the
/// numbers erased: what [`verify`] compares a patch against. The base
/// does not change between patches, so the dev loop builds one per crate
/// and keeps it for the builder's life.
#[derive(Debug, Default, Clone)]
pub struct BaseItems {
    groups: BTreeMap<String, BTreeSet<String>>,
}

impl BaseItems {
    /// `crate_name`'s items among the base's function names (its table).
    pub fn of(crate_name: &str, base: impl IntoIterator<Item = impl AsRef<str>>) -> Self {
        let mut groups: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for name in base {
            let name = name.as_ref();
            if owned_item(name, crate_name) {
                groups.entry(static_key(name)).or_default().insert(name.to_string());
            }
        }
        Self { groups }
    }
}

/// The checks of the module docs for `crate_name`: `base` its items in
/// the base ([`BaseItems::of`]), `in_base` whether the base has a
/// function of that name at all, `patch` every function the patch's
/// objects define, `aliases` the base's alias map.
///
/// Only a name the base does NOT have can be refused, so a name it has
/// is passed over without being demangled — nearly all of a patch's tens
/// of thousands.
pub fn verify<'a>(
    crate_name: &str,
    base: &BaseItems,
    in_base: &dyn Fn(&str) -> bool,
    aliases: &AliasMap,
    patch: impl IntoIterator<Item = &'a str>,
) -> Result<(), Refusal> {
    let mut names: Vec<&str> = patch
        .into_iter()
        .filter(|n| !in_base(n))
        // A patch function defined under another name of the same base
        // function counts as that function.
        .map(|n| aliases.get(n).map_or(n, String::as_str))
        .filter(|n| !in_base(n) && owned_item(n, crate_name))
        .collect();
    names.sort_unstable();
    for name in names {
        let key = static_key(name);
        if key.ends_with("::_::__ctor") {
            continue;
        }
        if base.groups.get(&key).is_some_and(|group| !group.contains(name)) {
            return Err(Refusal::Renamed { key, symbol: name.to_string() });
        }
    }
    Ok(())
}

/// Whether `symbol` is a function of an item a source edit could rename
/// in `crate_name`: its path mentions the crate, and it is not a
/// closure, a shim or a generic instantiation.
fn owned_item(symbol: &str, crate_name: &str) -> bool {
    // Both manglings spell every path segment as `<len><name>`, the crate
    // root included, so a symbol that never names the crate cannot be one
    // of its items, and is ruled out without being demangled.
    let mut needle = crate_name.len().to_string();
    needle.push_str(crate_name);
    if !symbol.contains(&needle) {
        return false;
    }
    let Ok(d) = rustc_demangle::try_demangle(symbol) else { return false };
    let full = format!("{d}");
    let alt = format!("{d:#}");
    // v0 prints a crate root with its disambiguator (`app[1a2b]::…`);
    // legacy mangling has none, and its path starts at the crate.
    let mentions = full.contains(&format!("{crate_name}["))
        || alt.starts_with(&format!("{crate_name}::"))
        || alt.starts_with(&format!("<{crate_name}::"));
    mentions && !numbered_or_generic(&alt)
}

/// A closure, a shim or a generic instantiation, as `{:#}` prints it.
fn numbered_or_generic(alt: &str) -> bool {
    if alt.contains("{closure") || alt.contains("{shim") || alt.contains("{{closure}}") || alt.contains("::<") {
        return true;
    }
    // `<Self as Trait>::f` / `<Self>::f`: generic when `Self` has
    // arguments of its own.
    if let Some(rest) = alt.strip_prefix('<') {
        let mut depth = 1usize;
        for (i, c) in rest.char_indices() {
            match c {
                '<' => depth += 1,
                '>' => {
                    depth -= 1;
                    if depth == 0 {
                        let self_ty = rest[..i].split(" as ").next().unwrap_or("");
                        return self_ty.contains('<');
                    }
                }
                _ => {}
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// v0 for `probe::helper`, `probe::helper` in the impl numbered next,
    /// `<probe::Row>::total` in `{impl#0}` and `{impl#1}`, a closure in
    /// `probe::run`, and an inventory constructor.
    const HELPER: &str = "_RNvCsfcO1eJHpGpx_5probe6helper";
    const OTHER: &str = "_RNvCsfcO1eJHpGpx_5probe5other";
    const TOTAL_IMPL0: &str = "_RNvMCsfcO1eJHpGpx_5probeNtB2_3Row5total";
    const TOTAL_IMPL1: &str = "_RNvMs_CsfcO1eJHpGpx_5probeNtB4_3Row5total";
    const CLOSURE: &str = "_RNCNvCsfcO1eJHpGpx_5probe3run0";
    const FOREIGN: &str = "_RNvCs1234567890_5other6helper";

    fn set(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_symbols_used_here_demangle_as_described() {
        assert!(owned_item(HELPER, "probe"));
        assert!(owned_item(TOTAL_IMPL0, "probe"));
        assert_eq!(static_key(TOTAL_IMPL0), static_key(TOTAL_IMPL1));
        assert_ne!(TOTAL_IMPL0, TOTAL_IMPL1);
        assert!(!owned_item(CLOSURE, "probe"), "a closure is renumbered by body edits");
        assert!(!owned_item(FOREIGN, "probe"));
    }

    #[test]
    fn an_appended_function_passes() {
        let base = [HELPER, TOTAL_IMPL0, CLOSURE];
        let patch = set(&[HELPER, TOTAL_IMPL0, OTHER]);
        assert_eq!(verify("probe", &BaseItems::of("probe", base), &|n: &str| base.contains(&n), &AliasMap::new(), patch.iter().map(String::as_str)), Ok(()));
    }

    /// An `impl` inserted ahead of `impl Row` renumbers it: the patch
    /// compiles `total` under a name the base does not have.
    #[test]
    fn a_renumbered_method_is_refused() {
        let base = [HELPER, TOTAL_IMPL0];
        let patch = set(&[HELPER, TOTAL_IMPL1]);
        assert_eq!(
            verify("probe", &BaseItems::of("probe", base), &|n: &str| base.contains(&n), &AliasMap::new(), patch.iter().map(String::as_str)),
            Err(Refusal::Renamed { key: static_key(TOTAL_IMPL0), symbol: TOTAL_IMPL1.into() })
        );
    }

    /// Both names present, but the group grew: `{impl#0}::total` may be
    /// a different function now.
    #[test]
    fn a_grown_group_is_refused() {
        let base = [TOTAL_IMPL0];
        let patch = set(&[TOTAL_IMPL0, TOTAL_IMPL1]);
        assert!(matches!(verify("probe", &BaseItems::of("probe", base), &|n: &str| base.contains(&n), &AliasMap::new(), patch.iter().map(String::as_str)), Err(Refusal::Renamed { .. })));
    }

    /// An edit that drops the last call to a private fn leaves rustc
    /// nothing to compile it for. Nothing patched calls it: not a rename.
    #[test]
    fn a_function_no_longer_compiled_is_not_a_rename() {
        let base = [HELPER, TOTAL_IMPL0];
        assert_eq!(verify("probe", &BaseItems::of("probe", base), &|n: &str| base.contains(&n), &AliasMap::new(), [HELPER]), Ok(()));
    }

    #[test]
    fn closures_and_other_crates_are_not_checked() {
        let base = [HELPER, CLOSURE, FOREIGN];
        let patch = set(&[HELPER]);
        assert_eq!(verify("probe", &BaseItems::of("probe", base), &|n: &str| base.contains(&n), &AliasMap::new(), patch.iter().map(String::as_str)), Ok(()));
    }

    #[test]
    fn a_function_the_patch_defines_under_an_alias_is_present() {
        let alias = "_RNvCsfcO1eJHpGpx_5probe7helper2";
        let aliases = AliasMap::from([(alias.to_string(), HELPER.to_string())]);
        assert_eq!(verify("probe", &BaseItems::of("probe", [HELPER]), &|n: &str| n == HELPER, &aliases, [alias]), Ok(()));
    }

    #[test]
    fn generic_instantiations_are_not_checked() {
        assert!(numbered_or_generic("probe::wrap::<u32>"));
        assert!(numbered_or_generic("<probe::Grid<u8>>::get"));
        assert!(numbered_or_generic("<alloc::vec::Vec<probe::Row> as core::ops::drop::Drop>::drop"));
        assert!(!numbered_or_generic("<probe::Row as core::convert::From<u32>>::from"));
        assert!(!numbered_or_generic("<probe::Row>::total"));
    }
}
