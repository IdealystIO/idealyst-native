//! Which of a patch's statics are the running program's statics.
//!
//! A patch re-emits whole crates, so it DEFINES every `static`,
//! `static mut` and `thread_local!` those crates declare, function-local
//! ones included. Left alone, each would get storage of its own in the
//! patch: code the patch redirected would write one copy while the base
//! code still running read the other (CrewForge's kiosk signed in from a
//! patched screen and then sent requests built by base code that read
//! "Not signed in"). So a static the running program already has is
//! BOUND to it: the patch's references are pointed at the program's
//! storage, and the patch's own copy goes unused. Only a static the
//! program does not have gets fresh storage.
//!
//! This module is the pairing decision, shared by the native
//! (`super::bind`) and wasm (`build-web`'s `hotpatch_statics`) object
//! rewriters; each of them decides what counts as a writable static in
//! its own object format.
//!
//! # What is bound
//!
//! Writable statics only: anything in a writable data or zero-fill
//! section, and thread-locals. An immutable `static` without interior
//! mutability lives in read-only data, holds no state to share, and
//! keeps a fresh copy per patch — which `stylesheet!` relies on: each
//! sheet keys the stylesheet cache by the address of an immutable
//! function-local `static`, so a patched `<name>_style()` misses the
//! cache and builds the sheet from its new rules.
//!
//! # Pairing is by mangled name — and refused when the name could lie
//!
//! A static's symbol is its path: crate, enclosing items, and for one
//! inside a closure, the closure's index in its function. Two statics of
//! one name in one scope (two blocks of a function, or two expansions of
//! a macro that declares one) differ only by a disambiguator that counts
//! them in source order. A body edit leaves names alone unless it adds,
//! removes or reorders such siblings, or adds a closure ahead of the one
//! a static sits in — and then a name can match a DIFFERENT static of the
//! running program, possibly of another type. Binding to it would be
//! memory corruption, so [`pair`] groups statics by [`static_key`] (the
//! demangled path with every disambiguator and closure index erased) and
//! refuses the patch when a group's members do not all pair by exact
//! name. A refusal is a rebuild.
//!
//! User-written statics inside function bodies are part of a file's
//! shape (`runtime_macros_parse::shape_of`), so adding, removing or
//! editing one rebuilds before a patch is ever built; what reaches this
//! module is a body edit that leaves every static's declaration alone,
//! plus statics a macro expands to.

use std::collections::{BTreeMap, HashMap, HashSet};

/// The suffix of the symbol a patch's reference to a bound static is
/// pointed at; the stub (or, on the web, the import plan) gives it the
/// running program's address. A NEW name rather than the static's own:
/// the patch still defines the original name, and the linker would
/// resolve a reference to it against that definition.
pub const HOST_SUFFIX: &str = "$idealyst_host";

/// The name a bound static's references are rewritten to, with the
/// reference's addend folded in (an addend cannot ride on a GOT load).
pub fn host_name(name: &str, addend: i64) -> String {
    if addend == 0 {
        format!("{name}{HOST_SUFFIX}")
    } else {
        format!("{name}{HOST_SUFFIX}{addend:+}")
    }
}

/// The grouping key of a static: its demangled path, without the hash
/// (legacy mangling) and with every closure index erased. Two statics of
/// one name in one scope share a key; a static whose enclosing closure
/// moved keeps its key while its symbol changes. A name that is not a
/// Rust symbol is its own key.
pub fn static_key(symbol: &str) -> String {
    let Ok(demangled) = rustc_demangle::try_demangle(symbol) else {
        return symbol.to_string();
    };
    // `{:#}` drops the legacy hash and v0's crate disambiguators.
    let text = format!("{demangled:#}");
    // `{closure#3}` → `{closure}` (likewise `{shim#…}` and friends).
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(at) = rest.find('#') {
        out.push_str(&rest[..at]);
        let tail = &rest[at + 1..];
        let digits = tail.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && tail[digits..].starts_with('}') {
            rest = &tail[digits..];
        } else {
            out.push('#');
            rest = tail;
        }
    }
    out.push_str(rest);
    out
}

/// Why a patch's statics cannot be paired with the running program's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ambiguous {
    /// The demangled path whose statics did not pair.
    pub key: String,
    pub in_patch: usize,
    pub in_program: usize,
}

impl std::fmt::Display for Ambiguous {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the patch has {} static(s) at `{}` and the running program {}, under names that \
             do not all match: a static of that name was added, removed or reordered, or a \
             closure ahead of it was. Binding by name could hand one static another's storage, \
             so this rebuilds",
            self.in_patch, self.key, self.in_program,
        )
    }
}

impl std::error::Error for Ambiguous {}

/// The running program's statics, grouped by [`static_key`]. Built once
/// per base build: demangling every data symbol of a large program is
/// the expensive half of pairing, and the program does not change between
/// patches.
#[derive(Debug, Default, Clone)]
pub struct ProgramStatics {
    groups: HashMap<String, HashSet<String>>,
    names: HashSet<String>,
}

impl ProgramStatics {
    pub fn new<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        let mut groups: HashMap<String, HashSet<String>> = HashMap::new();
        for name in names {
            groups.entry(static_key(name)).or_default().insert(name.to_string());
        }
        let names = groups.values().flatten().cloned().collect();
        Self { groups, names }
    }

    /// Whether the program has a static of exactly this name — answered
    /// without demangling it.
    pub fn has_name(&self, name: &str) -> bool {
        self.names.contains(name)
    }

    /// Whether the program has any static under this [`static_key`].
    pub fn has_key(&self, key: &str) -> bool {
        self.groups.contains_key(key)
    }
}

/// Whether a [`static_key`] is one of `inventory::submit!`'s: the
/// registry node (`__INVENTORY`) and the constructor pointer (`__CTOR`)
/// every submission declares in an anonymous `const _` of its own.
///
/// These are the one group of same-named statics an ADDITIVE patch can
/// grow (a new `#[component]` brings a catalog submission), and the one
/// group whose pairing cannot mislead on the web: the page never runs a
/// patch's constructors, so no code in a patch reads or writes a node,
/// and the running program's registry stays the base's list. See
/// [`pair_registrations_open`].
pub fn is_registration_key(key: &str) -> bool {
    key.ends_with("::_::__INVENTORY") || key.ends_with("::_::__CTOR")
}

/// [`pair`], except that a group of [`is_registration_key`] statics may
/// differ from the program's: its members that match by exact name are
/// bound, and the rest keep storage of their own.
///
/// Only for a patch whose constructors never run — the web's. A native
/// loader runs a constructor new to the binary, and a node bound to the
/// wrong storage would be pushed onto a registry list it is already on;
/// that side keeps [`pair`].
pub fn pair_registrations_open<'a>(
    patch: impl IntoIterator<Item = &'a str>,
    program: &ProgramStatics,
) -> Result<HashSet<String>, Ambiguous> {
    let (open, strict): (Vec<&str>, Vec<&str>) =
        patch.into_iter().partition(|name| is_registration_key(&static_key(name)));
    let mut bound = pair(strict, program)?;
    for name in open {
        if program.groups.get(&static_key(name)).is_some_and(|g| g.contains(name)) {
            bound.insert(name.to_string());
        }
    }
    Ok(bound)
}

/// Decide which of the patch's statics are bound to the running
/// program's.
///
/// `patch` is every static the patch defines that the caller considers;
/// `program` the running program's, in the same spelling. Returns the
/// names to bind; the rest of `patch` are new to the program and keep
/// storage of their own. See the module docs for the rule.
pub fn pair<'a>(
    patch: impl IntoIterator<Item = &'a str>,
    program: &ProgramStatics,
) -> Result<HashSet<String>, Ambiguous> {
    let mut patch_groups: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for name in patch {
        patch_groups.entry(static_key(name)).or_default().push(name);
    }
    let mut bind = HashSet::new();
    for (key, mut members) in patch_groups {
        members.sort_unstable();
        members.dedup();
        let Some(host) = program.groups.get(&key) else {
            continue; // new to the program: fresh storage
        };
        if members.len() != host.len() || members.iter().any(|m| !host.contains(*m)) {
            return Err(Ambiguous { key, in_patch: members.len(), in_program: host.len() });
        }
        bind.extend(members.into_iter().map(str::to_string));
    }
    Ok(bind)
}

#[cfg(test)]
mod tests {
    use super::*;

    // v0 symbols rustc 1.97 emitted for `probe::visits::VISITS`, a second
    // `VISITS` in another block of the same function (disambiguator `s_`),
    // and a `STATE` inside closures #0 and #1 of `probe::run`.
    const VISITS: &str = "_RNvNvCsfcO1eJHpGpx_5probe6visits6VISITS";
    const VISITS_2: &str = "_RNvNvCsfcO1eJHpGpx_5probe6visitss_6VISITS";
    const IN_CLOSURE_0: &str = "_RNvNCNvCsfcO1eJHpGpx_5probe3run05STATE";
    const IN_CLOSURE_1: &str = "_RNvNCNvCsfcO1eJHpGpx_5probe3runs_05STATE";

    #[test]
    fn the_key_erases_disambiguators_and_closure_indices() {
        assert_eq!(static_key(VISITS), "probe::visits::VISITS");
        assert_eq!(static_key(VISITS_2), "probe::visits::VISITS");
        assert_eq!(static_key(IN_CLOSURE_0), static_key(IN_CLOSURE_1));
        assert_eq!(static_key(&format!("_{VISITS}")), "probe::visits::VISITS", "Mach-O spelling");
        assert_eq!(static_key("_ZN5probe6visits6VISITS17h0123456789abcdefE"), "probe::visits::VISITS");
        assert_eq!(static_key("plain_c_symbol"), "plain_c_symbol");
    }

    /// `inventory::submit!` nodes, `probe::_::__INVENTORY`, in the
    /// anonymous consts `_`, `_`#1 and `_`#2 of one module.
    fn inventory_node(disambiguator: &str) -> String {
        format!("_RNvNvCsfcO1eJHpGpx_5probe{disambiguator}1__11___INVENTORY")
    }

    #[test]
    fn a_grown_registration_group_binds_what_matches_and_nothing_else() {
        let (a, b, c) = (inventory_node(""), inventory_node("s_"), inventory_node("s0_"));
        assert_eq!(static_key(&a), "probe::_::__INVENTORY");
        assert_eq!(static_key(&c), "probe::_::__INVENTORY");
        assert!(is_registration_key(&static_key(&a)));
        let program = ProgramStatics::new([a.as_str(), b.as_str()]);
        assert!(pair([a.as_str(), b.as_str(), c.as_str()], &program).is_err(), "strict pairing refuses growth");
        let bound = pair_registrations_open([a.as_str(), b.as_str(), c.as_str()], &program).unwrap();
        assert!(bound.contains(&a) && bound.contains(&b));
        assert!(!bound.contains(&c), "a new node is bound to nothing");
        // Every other group stays strict.
        assert!(pair_registrations_open([VISITS, VISITS_2], &ProgramStatics::new([VISITS])).is_err());
    }

    #[test]
    fn a_static_the_program_has_is_bound_and_a_new_one_is_not() {
        let fresh = "_RNvNvCsfcO1eJHpGpx_5probe6visits5FRESH";
        let bound = pair([VISITS, fresh], &ProgramStatics::new([VISITS])).unwrap();
        assert!(bound.contains(VISITS));
        assert!(!bound.contains(fresh));
    }

    /// The tracing-callsite shape: a macro that declares a static, used
    /// once more ahead of an existing use. The new expansion takes the
    /// old one's name; binding it would hand it the old one's storage.
    #[test]
    fn regression_a_same_named_static_added_in_one_scope_refuses() {
        let err = pair([VISITS, VISITS_2], &ProgramStatics::new([VISITS])).unwrap_err();
        assert_eq!(err.key, "probe::visits::VISITS");
        assert_eq!((err.in_patch, err.in_program), (2, 1));
    }

    /// A closure inserted ahead of the one holding a static renames the
    /// static. Pairing by name would make it a new static — the very
    /// two-copies bug this module exists to stop — so it refuses.
    #[test]
    fn regression_a_static_whose_closure_moved_refuses() {
        assert!(pair([IN_CLOSURE_1], &ProgramStatics::new([IN_CLOSURE_0])).is_err());
    }

    #[test]
    fn an_unchanged_group_of_siblings_binds_whole() {
        let bound = pair([VISITS, VISITS_2], &ProgramStatics::new([VISITS_2, VISITS])).unwrap();
        assert_eq!(bound.len(), 2);
    }

    #[test]
    fn host_names_fold_the_addend() {
        assert_eq!(host_name("_S", 0), "_S$idealyst_host");
        assert_eq!(host_name("_S", 8), "_S$idealyst_host+8");
        assert_eq!(host_name("_S", -4), "_S$idealyst_host-4");
    }
}
