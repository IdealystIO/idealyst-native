//! Phase 2: runtime name-resolution + adjacency.
//!
//! Composes edges emitted by `#[component]` (see `EdgeRef`) carry the
//! bare ident the author wrote at the `ui!` / `jsx!` call site. Bare
//! idents are ambiguous in two ways:
//!
//! 1. **Case convention**: `ui!` / `jsx!` lower `PascalCase` call
//!    sites to `snake_case` to find the per-component `macro_rules!`
//!    shim (`PrimaryButton()` → `primary_button!()`). A composes edge
//!    `name = "PrimaryButton"` should therefore resolve to the entry
//!    whose `name = "primary_button"`. We normalize both sides via
//!    `pascal_to_snake` for matching — idempotent on already-snake
//!    input, so snake_case call sites keep working too.
//!
//! 2. **Same short-name across modules**: two crates / submodules may
//!    each declare `#[component] fn card`. We disambiguate by
//!    source-module proximity (spec §6): same module first, then the
//!    deepest ancestor module, then anywhere in the workspace.
//!    Anything still ambiguous after that is surfaced as
//!    [`EdgeStatus::Ambiguous`] with the candidate list intact — the
//!    runtime hands the choice back to the user / `mcp --check`.
//!
//! Resolution runs *once* over the global catalog via
//! [`ResolvedCatalog::build`]; the forward and reverse adjacency maps
//! are then constant-time lookups. Reverse adjacency gives
//! `find_uses(name)` — who composes me? — in O(1) after the one-pass
//! build.

use std::collections::HashMap;

use crate::ComponentEntry;

/// A `(module_path, name)` pair, the canonical identity for a
/// `ComponentEntry`. Two entries with the same pair would be a
/// duplicate registration — we treat them as identical here.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct EntryRef {
    pub module_path: &'static str,
    pub name: &'static str,
}

impl EntryRef {
    pub fn of(entry: &ComponentEntry) -> Self {
        EntryRef { module_path: entry.module_path, name: entry.name }
    }

    /// Fully-qualified name as a heap string: `module_path::name`.
    pub fn fqn(&self) -> String {
        format!("{}::{}", self.module_path, self.name)
    }
}

/// One composition edge with its resolution result attached.
#[derive(Debug, Clone)]
pub struct ResolvedEdge {
    /// Bare ident as written at the `ui!` / `jsx!` call site.
    pub raw_name: &'static str,
    /// Source line the macro recorded (0 on stable Rust; see
    /// `framework_mcp::EdgeRef` doc-comment).
    pub line: u32,
    pub status: EdgeStatus,
}

/// Outcome of attempting to resolve a composes edge.
#[derive(Debug, Clone)]
pub enum EdgeStatus {
    /// Exactly one match (possibly after proximity tie-break).
    Resolved { target: EntryRef },
    /// Zero matches anywhere in the workspace.
    NoMatch,
    /// More than one match at the same proximity level. Spec §6 says
    /// the runtime surfaces these to the user — `mcp --check` is the
    /// intended consumer.
    Ambiguous { candidates: Vec<EntryRef> },
}

/// Resolved view over the catalog.
///
/// Built once at startup. Holds the entries (drained from `inventory`),
/// the forward edges (host → resolved edges), and the reverse map
/// (target → hosts that compose it). All three are populated in a
/// single pass.
#[derive(Debug)]
pub struct ResolvedCatalog {
    entries: Vec<&'static ComponentEntry>,
    forward: HashMap<EntryRef, Vec<ResolvedEdge>>,
    reverse: HashMap<EntryRef, Vec<EntryRef>>,
}

impl ResolvedCatalog {
    /// Build over the global `inventory` slice.
    pub fn build() -> Self {
        let entries: Vec<&'static ComponentEntry> = crate::entries().collect();
        Self::build_from(entries)
    }

    /// Build from an explicit entry list — the path tests use to
    /// exercise resolution without touching the global slice.
    pub fn build_from(entries: Vec<&'static ComponentEntry>) -> Self {
        // Bucket every entry under its name's snake_case form so the
        // resolution path is a single hash lookup. Snake-casing here
        // mirrors `ui!` / `jsx!`'s dispatch convention (see module
        // doc-comment): the call site's PascalCase is converted to
        // snake_case to find the per-component macro shim, and we do
        // the same here to find the entry it points at.
        let mut by_lower_name: HashMap<String, Vec<&'static ComponentEntry>> =
            HashMap::new();
        for e in &entries {
            by_lower_name
                .entry(pascal_to_snake(e.name))
                .or_default()
                .push(e);
        }

        let mut forward: HashMap<EntryRef, Vec<ResolvedEdge>> = HashMap::new();
        let mut reverse: HashMap<EntryRef, Vec<EntryRef>> = HashMap::new();

        for host in &entries {
            let host_ref = EntryRef::of(host);
            let mut edges = Vec::with_capacity(host.composes.len());
            for edge in host.composes {
                let key = pascal_to_snake(edge.name);
                let candidates = by_lower_name
                    .get(&key)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let status = resolve_one(host.module_path, candidates);
                if let EdgeStatus::Resolved { target } = &status {
                    reverse.entry(*target).or_default().push(host_ref);
                }
                edges.push(ResolvedEdge {
                    raw_name: edge.name,
                    line: edge.line,
                    status,
                });
            }
            forward.insert(host_ref, edges);
        }

        // Stable order in `uses()` output makes tests + diffs sane.
        for v in reverse.values_mut() {
            v.sort_by_key(|r| (r.module_path, r.name));
            v.dedup();
        }

        ResolvedCatalog { entries, forward, reverse }
    }

    pub fn entries(&self) -> &[&'static ComponentEntry] {
        &self.entries
    }

    /// Forward edges: what does `host` compose?
    pub fn dependencies(&self, host: &EntryRef) -> &[ResolvedEdge] {
        self.forward.get(host).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Reverse edges: who composes `target`?
    pub fn uses(&self, target: &EntryRef) -> &[EntryRef] {
        self.reverse.get(target).map(Vec::as_slice).unwrap_or(&[])
    }
}

/// Apply spec §6's proximity rules to a candidate set whose lowercased
/// names all match the edge's lowercased ident. Returns:
/// - `NoMatch` if `candidates` is empty.
/// - `Resolved` if exactly one candidate wins after tie-break.
/// - `Ambiguous` if multiple candidates remain at the winning level.
fn resolve_one(
    host_module: &'static str,
    candidates: &[&'static ComponentEntry],
) -> EdgeStatus {
    if candidates.is_empty() {
        return EdgeStatus::NoMatch;
    }

    // 1. Same-module match wins outright.
    let same: Vec<&ComponentEntry> = candidates
        .iter()
        .copied()
        .filter(|c| c.module_path == host_module)
        .collect();
    if let Some(unique) = single(&same) {
        return EdgeStatus::Resolved { target: EntryRef::of(unique) };
    }
    if same.len() > 1 {
        return EdgeStatus::Ambiguous {
            candidates: same.iter().map(|e| EntryRef::of(e)).collect(),
        };
    }

    // 2. Closest ancestor module wins next. "Closest" = the candidate
    //    whose module_path is the longest strict prefix of the host's.
    let mut best_len: usize = 0;
    let mut best: Vec<&ComponentEntry> = Vec::new();
    for c in candidates {
        if is_ancestor_module(c.module_path, host_module) {
            let len = c.module_path.len();
            #[allow(clippy::comparison_chain)]
            if len > best_len {
                best_len = len;
                best.clear();
                best.push(c);
            } else if len == best_len {
                best.push(c);
            }
        }
    }
    if let Some(unique) = single(&best) {
        return EdgeStatus::Resolved { target: EntryRef::of(unique) };
    }
    if best.len() > 1 {
        return EdgeStatus::Ambiguous {
            candidates: best.iter().map(|e| EntryRef::of(e)).collect(),
        };
    }

    // 3. Anywhere else in the workspace. One match → resolved; many →
    //    ambiguous with the full candidate list.
    if let Some(unique) = single(candidates) {
        return EdgeStatus::Resolved { target: EntryRef::of(unique) };
    }
    EdgeStatus::Ambiguous {
        candidates: candidates.iter().map(|e| EntryRef::of(e)).collect(),
    }
}

fn single<'a, T: Copy>(slice: &'a [T]) -> Option<T> {
    if slice.len() == 1 { Some(slice[0]) } else { None }
}

/// True iff `maybe_ancestor` is a strict ancestor module of
/// `descendant`. `crate::a` is an ancestor of `crate::a::b`; equal
/// paths are *not* ancestors (the same-module case is handled
/// separately in `resolve_one`).
/// Convert PascalCase to snake_case, mirroring `framework-macros`'s
/// `case::pascal_to_snake`. Duplicated here so the resolver can
/// normalize without taking a dep on `framework-macros` (a proc-macro
/// crate). Keep the two implementations in sync — see
/// `crates/framework/macros/src/case.rs` for the canonical form and
/// the unit tests covering acronym handling.
fn pascal_to_snake(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            let prev = chars[i - 1];
            let prev_lowerish = prev.is_ascii_lowercase() || prev.is_ascii_digit();
            let acronym_to_word = prev.is_ascii_uppercase()
                && chars
                    .get(i + 1)
                    .map(|n| n.is_ascii_lowercase())
                    .unwrap_or(false);
            if (prev_lowerish || acronym_to_word) && !out.ends_with('_') {
                out.push('_');
            }
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

fn is_ancestor_module(maybe_ancestor: &str, descendant: &str) -> bool {
    if maybe_ancestor == descendant {
        return false;
    }
    if !descendant.starts_with(maybe_ancestor) {
        return false;
    }
    // The next chars after the prefix must be `::` — otherwise
    // `crate::foo` would falsely "ancestor" `crate::foobar`.
    descendant[maybe_ancestor.len()..].starts_with("::")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EdgeRef;

    fn leak_entry(
        module_path: &'static str,
        name: &'static str,
        composes: &'static [EdgeRef],
    ) -> &'static ComponentEntry {
        Box::leak(Box::new(ComponentEntry {
            name,
            module_path,
            file: "synthetic.rs",
            line: 0,
            docs: "",
            composes,
            params: &[],
        }))
    }

    fn leak_edges(edges: Vec<EdgeRef>) -> &'static [EdgeRef] {
        Box::leak(edges.into_boxed_slice())
    }

    #[test]
    fn ancestor_check_is_strict_and_segment_aligned() {
        assert!(is_ancestor_module("crate::a", "crate::a::b"));
        assert!(!is_ancestor_module("crate::a", "crate::a"));
        assert!(!is_ancestor_module("crate::foo", "crate::foobar"));
        assert!(is_ancestor_module("crate", "crate::a"));
    }

    #[test]
    fn multi_word_pascal_matches_snake_entry() {
        // The framework convention: fn names are snake_case, call
        // sites are PascalCase. The resolver normalizes both via
        // `pascal_to_snake` so a `PrimaryButton` edge resolves to
        // `primary_button`.
        let target = leak_entry("crate", "primary_button", &[]);
        let host = leak_entry(
            "crate",
            "host",
            leak_edges(vec![EdgeRef { name: "PrimaryButton", line: 0 }]),
        );
        let cat = ResolvedCatalog::build_from(vec![target, host]);
        let edges = cat.dependencies(&EntryRef::of(host));
        match &edges[0].status {
            EdgeStatus::Resolved { target } => assert_eq!(target.name, "primary_button"),
            other => panic!("expected Resolved, got {:?}", other),
        }
    }

    #[test]
    fn case_insensitive_match_handles_pascal_to_snake() {
        // Host at root composes `Vignette` → should resolve to
        // `vignette` (case-folded), simulating `ui!`'s dispatch.
        let target = leak_entry("crate::components::vignette", "vignette", &[]);
        let host = leak_entry(
            "crate",
            "app",
            leak_edges(vec![EdgeRef { name: "Vignette", line: 0 }]),
        );
        let cat = ResolvedCatalog::build_from(vec![target, host]);
        let edges = cat.dependencies(&EntryRef::of(host));
        assert_eq!(edges.len(), 1);
        match &edges[0].status {
            EdgeStatus::Resolved { target: t } => {
                assert_eq!(t.name, "vignette");
            }
            other => panic!("expected Resolved, got {:?}", other),
        }
    }

    #[test]
    fn same_module_beats_ancestor() {
        // Two `card` entries: one at `crate`, one at `crate::a::b`.
        // A host at `crate::a::b` composes `card` → should pick the
        // same-module `crate::a::b::card`, not the ancestor's.
        let root_card = leak_entry("crate", "card", &[]);
        let local_card = leak_entry("crate::a::b", "card", &[]);
        let host = leak_entry(
            "crate::a::b",
            "host",
            leak_edges(vec![EdgeRef { name: "card", line: 0 }]),
        );
        let cat = ResolvedCatalog::build_from(vec![root_card, local_card, host]);
        let edges = cat.dependencies(&EntryRef::of(host));
        match &edges[0].status {
            EdgeStatus::Resolved { target } => {
                assert_eq!(target.module_path, "crate::a::b");
            }
            other => panic!("expected same-module Resolved; got {:?}", other),
        }
    }

    #[test]
    fn closest_ancestor_wins() {
        // Two `card`s, both ancestors of `crate::a::b::c`. The deeper
        // ancestor (`crate::a`) should win over the shallower (`crate`).
        let deep_card = leak_entry("crate::a", "card", &[]);
        let shallow_card = leak_entry("crate", "card", &[]);
        let host = leak_entry(
            "crate::a::b::c",
            "host",
            leak_edges(vec![EdgeRef { name: "card", line: 0 }]),
        );
        let cat = ResolvedCatalog::build_from(vec![deep_card, shallow_card, host]);
        let edges = cat.dependencies(&EntryRef::of(host));
        match &edges[0].status {
            EdgeStatus::Resolved { target } => {
                assert_eq!(target.module_path, "crate::a");
            }
            other => panic!("expected closest-ancestor Resolved; got {:?}", other),
        }
    }

    #[test]
    fn ambiguous_when_two_candidates_at_same_depth() {
        // Two `card`s, neither in same module nor ancestor of the host.
        // No proximity preference → ambiguous, both surfaced.
        let card_x = leak_entry("crate::x", "card", &[]);
        let card_y = leak_entry("crate::y", "card", &[]);
        let host = leak_entry(
            "crate::host_mod",
            "host",
            leak_edges(vec![EdgeRef { name: "card", line: 0 }]),
        );
        let cat = ResolvedCatalog::build_from(vec![card_x, card_y, host]);
        let edges = cat.dependencies(&EntryRef::of(host));
        match &edges[0].status {
            EdgeStatus::Ambiguous { candidates } => {
                assert_eq!(candidates.len(), 2);
            }
            other => panic!("expected Ambiguous; got {:?}", other),
        }
    }

    #[test]
    fn no_match_when_name_absent() {
        let host = leak_entry(
            "crate",
            "host",
            leak_edges(vec![EdgeRef { name: "ghost", line: 0 }]),
        );
        let cat = ResolvedCatalog::build_from(vec![host]);
        let edges = cat.dependencies(&EntryRef::of(host));
        assert!(matches!(edges[0].status, EdgeStatus::NoMatch));
    }

    #[test]
    fn reverse_lookup_lists_all_hosts() {
        // Three hosts all composing the same target. `uses(target)`
        // should list every host, sorted for determinism.
        let target = leak_entry("crate::lib", "panel", &[]);
        let h1 = leak_entry(
            "crate::a",
            "host_a",
            leak_edges(vec![EdgeRef { name: "panel", line: 0 }]),
        );
        let h2 = leak_entry(
            "crate::b",
            "host_b",
            leak_edges(vec![EdgeRef { name: "Panel", line: 0 }]),
        );
        let h3 = leak_entry(
            "crate::c",
            "host_c",
            leak_edges(vec![EdgeRef { name: "panel", line: 0 }]),
        );
        let cat = ResolvedCatalog::build_from(vec![target, h1, h2, h3]);
        let users = cat.uses(&EntryRef::of(target));
        assert_eq!(users.len(), 3);
        let names: Vec<&str> = users.iter().map(|r| r.name).collect();
        assert!(names.contains(&"host_a"));
        assert!(names.contains(&"host_b"));
        assert!(names.contains(&"host_c"));
    }

    #[test]
    fn ambiguous_in_same_module() {
        // Two entries with the same lowercased name in the same
        // module — shouldn't happen in well-formed code, but the
        // resolver must surface it rather than silently picking one.
        let a = leak_entry("crate", "card", &[]);
        let b = leak_entry("crate", "Card", &[]);
        let host = leak_entry(
            "crate",
            "host",
            leak_edges(vec![EdgeRef { name: "Card", line: 0 }]),
        );
        let cat = ResolvedCatalog::build_from(vec![a, b, host]);
        let edges = cat.dependencies(&EntryRef::of(host));
        match &edges[0].status {
            EdgeStatus::Ambiguous { candidates } => {
                assert_eq!(candidates.len(), 2);
            }
            other => panic!("expected Ambiguous, got {:?}", other),
        }
    }
}
