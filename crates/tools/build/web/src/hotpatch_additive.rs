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
//! ahead of theirs — renumbers them. Left under its new name, a function
//! has no partner in the base: the jump table leaves the base's copy in
//! place, and a dependent's call by the old name imports it, while the
//! patch's own code calls the new copy. Harmless while the two are the
//! same code, and a silent stale edit the moment the same save changed
//! that function's body too.
//!
//! # What is checked, and what is paired
//!
//! Functions are grouped by their path with every number erased
//! ([`build_runtime_server::hotpatch::statics::static_key`]): an item
//! keeps its group however it is renumbered. Over every group of the
//! edited crate's items the base has (its own items, its impls —
//! leaving out closures, shims and generic instantiations, which a body
//! edit legitimately renumbers or drops), a name the base does not have
//! means one of two things:
//!
//! - the group holds exactly one function in the base and exactly one in
//!   the patch: one item, renumbered. The patch's function is PAIRED with
//!   the base's, and [`verify`] returns the pair. The caller renames it in
//!   the patch's objects before anything else reads them, so from there
//!   on the patch simply defines the base's name: the digests compare it
//!   against its own baseline, a dependent's call by the old name links to
//!   the new body, and the jump table pairs it by name as it pairs any
//!   other function;
//! - anything else is a refusal. A group that GREW holds a name that may
//!   now belong to a different function of the same path, and with
//!   several on either side nothing says which is which.
//!
//! A closure inside a paired item is renumbered with it, and so is a
//! shim or a generic instantiation that names the item or the closure.
//! Those pair on their whole path with their own numbers kept (`{closure#0}`
//! with `{closure#0}`), so a fn pointer or vtable the page already holds
//! reaches the new code as it does after a body edit: such a pointer can
//! be the instantiation, whose base copy calls the base's closure directly.
//! One with no such partner is new code, which is what it is after a body
//! edit too.
//!
//! The one-to-one pairing cannot hand existing code the wrong function.
//! Suppose the patch's function is a new item and the base's an old one
//! that no longer compiled. What dropped out of the compile is private
//! (a `pub` item is always compiled, and a group of two is refused), and
//! so is called only from this crate: whichever caller stopped calling
//! it was edited, and is redirected. The check that remains is the
//! signature, which [`crate::hotpatch_build`] compares against the base's
//! slot once the patch is linked: unlike a mangled name shared by both
//! sides, a pair of two names promises nothing about it.
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
use crate::hotpatch_statics::Renames;

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
    /// patch that the base does not, and the two sides do not pair one to
    /// one: the group grew, or holds several functions that were
    /// renumbered.
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
                "the patch compiles `{key}` to `{symbol}`, a name the running program does not have, \
                 and that path holds more than one function, so nothing says which one it is: an item \
                 added ahead of them renumbered them, or one with the same path was added"
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
    /// Closures, shims and generic instantiations, by whole path, numbers
    /// kept.
    numbered: BTreeMap<String, BTreeSet<String>>,
}

impl BaseItems {
    /// `crate_name`'s items among the base's function names (its table).
    pub fn of(crate_name: &str, base: impl IntoIterator<Item = impl AsRef<str>>) -> Self {
        let mut out = Self::default();
        for name in base {
            let name = name.as_ref();
            match classify(name, crate_name) {
                Some(Owned::Item) => {
                    out.groups.entry(static_key(name)).or_default().insert(name.to_string());
                }
                Some(Owned::Numbered(path)) => {
                    out.numbered.entry(path).or_default().insert(name.to_string());
                }
                None => {}
            }
        }
        out
    }
}

/// The checks of the module docs for `crate_name`: `base` its items in
/// the base ([`BaseItems::of`]), `in_base` whether the base has a
/// function of that name at all, `patch` every function the patch's
/// objects define, `aliases` the base's alias map. What it returns are the
/// pairs: each renumbered function's name in the patch, and its name in
/// the base.
///
/// Only a name the base does NOT have can be paired or refused, so a name
/// it has is passed over without being demangled — nearly all of a
/// patch's tens of thousands.
pub fn verify<'a>(
    crate_name: &str,
    base: &BaseItems,
    in_base: &dyn Fn(&str) -> bool,
    aliases: &AliasMap,
    patch: impl IntoIterator<Item = &'a str>,
) -> Result<Renames, Refusal> {
    let patch: Vec<&str> = patch.into_iter().collect();
    let mut items: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    let mut numbered: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for &name in &patch {
        if in_base(name) {
            continue;
        }
        // A patch function defined under another name of the same base
        // function counts as that function.
        let name = aliases.get(name).map_or(name, String::as_str);
        if in_base(name) {
            continue;
        }
        match classify(name, crate_name) {
            Some(Owned::Item) => items.entry(static_key(name)).or_default().push(name),
            Some(Owned::Numbered(path)) => numbered.entry(path).or_default().push(name),
            None => {}
        }
    }
    let mut renames = Renames::new();
    if items.is_empty() && numbered.is_empty() {
        return Ok(renames);
    }

    // Whether the patch also defines a base name: then the name it has
    // that the base lacks is a second function, not this one renumbered.
    let defined: HashSet<&str> =
        patch.iter().map(|n| aliases.get(*n).map_or(*n, String::as_str)).filter(|n| in_base(n)).collect();
    let partner = |group: &'_ BTreeSet<String>, names: &[&str]| -> Option<String> {
        let mut only = group.iter();
        match (only.next(), only.next(), names) {
            (Some(b), None, [_]) if !defined.contains(b.as_str()) => Some(b.clone()),
            _ => None,
        }
    };

    for (key, mut names) in items {
        if key.ends_with("::_::__ctor") {
            continue;
        }
        let Some(group) = base.groups.get(&key) else { continue };
        names.sort_unstable();
        names.dedup();
        let Some(b) = partner(group, &names) else {
            return Err(Refusal::Renamed { key, symbol: names[0].to_string() });
        };
        renames.insert(names[0].to_string(), b);
    }
    for (path, mut names) in numbered {
        let Some(group) = base.numbered.get(&path) else { continue };
        names.sort_unstable();
        names.dedup();
        if let Some(b) = partner(group, &names) {
            renames.insert(names[0].to_string(), b);
        }
    }
    Ok(renames)
}

/// What a function of `crate_name` is to these checks.
enum Owned {
    /// An item a source edit could rename.
    Item,
    /// A closure, a shim or a generic instantiation, with its whole path,
    /// numbers kept.
    Numbered(String),
}

/// How `symbol` belongs to `crate_name`: `None` when its path does not
/// mention the crate.
fn classify(symbol: &str, crate_name: &str) -> Option<Owned> {
    // Both manglings spell every path segment as `<len><name>`, the crate
    // root included, so a symbol that never names the crate cannot be one
    // of its items, and is ruled out without being demangled.
    let mut needle = crate_name.len().to_string();
    needle.push_str(crate_name);
    if !symbol.contains(&needle) {
        return None;
    }
    let d = rustc_demangle::try_demangle(symbol).ok()?;
    let full = format!("{d}");
    let alt = format!("{d:#}");
    // v0 prints a crate root with its disambiguator (`app[1a2b]::…`);
    // legacy mangling has none, and its path starts at the crate.
    let mentions = full.contains(&format!("{crate_name}["))
        || alt.starts_with(&format!("{crate_name}::"))
        || alt.starts_with(&format!("<{crate_name}::"));
    if !mentions {
        return None;
    }
    if !numbered_or_generic(&alt) {
        return Some(Owned::Item);
    }
    Some(Owned::Numbered(alt))
}

#[cfg(test)]
fn owned_item(symbol: &str, crate_name: &str) -> bool {
    matches!(classify(symbol, crate_name), Some(Owned::Item))
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
        assert_eq!(verify("probe", &BaseItems::of("probe", base), &|n: &str| base.contains(&n), &AliasMap::new(), patch.iter().map(String::as_str)), Ok(Renames::new()));
    }

    /// An `impl` inserted ahead of `impl Row` renumbers it: the patch
    /// compiles `total` under a name the base does not have, and it is
    /// the only function of that path on either side.
    #[test]
    fn a_renumbered_method_pairs_with_its_old_name() {
        let base = [HELPER, TOTAL_IMPL0];
        let patch = set(&[HELPER, TOTAL_IMPL1]);
        assert_eq!(
            verify("probe", &BaseItems::of("probe", base), &|n: &str| base.contains(&n), &AliasMap::new(), patch.iter().map(String::as_str)),
            Ok(Renames::from([(TOTAL_IMPL1.to_string(), TOTAL_IMPL0.to_string())]))
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

    /// Two new names for one base function, or one for two: nothing says
    /// which is which.
    #[test]
    fn a_group_of_several_is_refused_not_paired() {
        const TOTAL_IMPL2: &str = "_RNvMs0_CsfcO1eJHpGpx_5probeNtB5_3Row5total";
        assert_eq!(static_key(TOTAL_IMPL2), static_key(TOTAL_IMPL0));
        let check = |base: &[&str], patch: &[&str]| {
            verify("probe", &BaseItems::of("probe", base), &|n: &str| base.contains(&n), &AliasMap::new(), patch.iter().copied())
        };
        assert!(matches!(check(&[TOTAL_IMPL0], &[TOTAL_IMPL1, TOTAL_IMPL2]), Err(Refusal::Renamed { .. })));
        assert!(matches!(check(&[TOTAL_IMPL0, TOTAL_IMPL1], &[TOTAL_IMPL2]), Err(Refusal::Renamed { .. })));
    }

    /// The closure in a renumbered method pairs on its whole path, its own
    /// number kept; one numbered differently is new code.
    #[test]
    fn a_closure_in_a_renumbered_method_pairs_on_its_own_number() {
        const IN_IMPL0: &str = "_RNCNvMCsfcO1eJHpGpx_5probeNtB4_3Row5total0";
        const IN_IMPL1: &str = "_RNCNvMs_CsfcO1eJHpGpx_5probeNtB6_3Row5total0";
        const SECOND_IN_IMPL1: &str = "_RNCNvMs_CsfcO1eJHpGpx_5probeNtB6_3Row5totals_0";
        let base = [TOTAL_IMPL0, IN_IMPL0];
        let renames = verify(
            "probe",
            &BaseItems::of("probe", base),
            &|n: &str| base.contains(&n),
            &AliasMap::new(),
            [TOTAL_IMPL1, IN_IMPL1, SECOND_IN_IMPL1],
        )
        .unwrap();
        assert_eq!(renames.get(IN_IMPL1).map(String::as_str), Some(IN_IMPL0), "{renames:?}");
        assert_eq!(renames.get(SECOND_IN_IMPL1), None, "{renames:?}");
    }

    /// An edit that drops the last call to a private fn leaves rustc
    /// nothing to compile it for. Nothing patched calls it: not a rename.
    #[test]
    fn a_function_no_longer_compiled_is_not_a_rename() {
        let base = [HELPER, TOTAL_IMPL0];
        assert_eq!(verify("probe", &BaseItems::of("probe", base), &|n: &str| base.contains(&n), &AliasMap::new(), [HELPER]), Ok(Renames::new()));
    }

    #[test]
    fn closures_and_other_crates_are_not_checked() {
        let base = [HELPER, CLOSURE, FOREIGN];
        let patch = set(&[HELPER]);
        assert_eq!(verify("probe", &BaseItems::of("probe", base), &|n: &str| base.contains(&n), &AliasMap::new(), patch.iter().map(String::as_str)), Ok(Renames::new()));
    }

    #[test]
    fn a_function_the_patch_defines_under_an_alias_is_present() {
        let alias = "_RNvCsfcO1eJHpGpx_5probe7helper2";
        let aliases = AliasMap::from([(alias.to_string(), HELPER.to_string())]);
        assert_eq!(verify("probe", &BaseItems::of("probe", [HELPER]), &|n: &str| n == HELPER, &aliases, [alias]), Ok(Renames::new()));
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
