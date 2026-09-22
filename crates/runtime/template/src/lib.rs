//! Template **descriptors**: the static half of a `ui!` site, as a
//! BUILD-TIME artifact.
//!
//! A descriptor is the data form of one `ui!` call site — the node tree,
//! tags, attribute names, literal values, child order, and a [`SlotSig`]
//! for the dynamic expressions it does not carry. Its purpose is to let
//! a dev-time overlay say "node 7 of that site now has `content =
//! "Sign in."`" without recompiling.
//!
//! # Descriptors are not in the binary
//!
//! They used to be: `ui!` emitted a `static Descriptor` per site and
//! registered it at build time. That was measured on a real app
//! (CrewForge, ~256 CGUs) and cost about **1.4 s on every one-edit
//! rebuild** — a 27% tax on the loop the feature exists to speed up,
//! almost all of it in `macro_expand_crate` and `serialize_dep_graph`.
//! The same measurement with the descriptor compiled out and only the
//! node TAGS kept was within noise of the feature being off entirely.
//! See `docs/ui-layer.md` for the table.
//!
//! So the descriptor is produced from SOURCE at build time, by the same
//! split pass the macro uses, and written next to the build. The binary
//! carries only what cannot be recovered from source:
//!
//! - a [`site_key`] and a node index on each `Element` the site builds
//!   (`runtime_scene::NodeTag`), and
//! - one crate-level [`SPLIT_VERSION`] marker.
//!
//! Everything else — which node is which, what its props were, which
//! slots it references — is in the JSON the build wrote. That also puts
//! validation where it belongs: a differ has BOTH source versions in
//! hand, so it can check that an edit did not disturb the slot list
//! before it ever sends a patch. And an over-the-air path archives each
//! build's descriptor set keyed by build id rather than reading it back
//! out of a shipped app.
//!
//! # What this crate owns
//!
//! The data types, the [`Registry`], and [`validate`]. It knows nothing
//! about builders, handlers or transports, and depends only on serde.
//! That narrowness is the point: a descriptor is a portable artifact.
//! It serializes; it can be validated with no renderer in the graph.
//!
//! ```
//! use std::borrow::Cow;
//! use runtime_template::*;
//!
//! let desc = Descriptor {
//!     site: SiteId {
//!         package: Cow::Borrowed("my-app"),
//!         file: Cow::Borrowed("src/screen.rs"),
//!         line: 12,
//!         col: 5,
//!     },
//!     slots: SlotSig { slots: Cow::Borrowed(&[]) },
//!     nodes: Cow::Borrowed(&[Node::Prim {
//!         kind: Cow::Borrowed("text"),
//!         props: Cow::Borrowed(&[PropEntry {
//!             name: Cow::Borrowed("content"),
//!             value: PropValue::Lit(LiteralValue::Str(Cow::Borrowed("hello"))),
//!         }]),
//!         children: Cow::Borrowed(&[]),
//!     }]),
//!     roots: Cow::Borrowed(&[0]),
//! };
//!
//! assert_eq!(desc.nodes.len(), 1);
//! assert_eq!(desc.site.key(), site_key("my-app", "src/screen.rs", 12, 5));
//! ```
//!
//! Every collection is a [`Cow`] so a descriptor read from bytes and one
//! built in memory are the same type.
//!
//! # Node indices, not nesting
//!
//! [`Descriptor::nodes`] is FLAT and children are `u32` indices into it.
//! Nesting `Node` inside `Node` would need a `Box` per level; a flat
//! array also lets a nested template (an `if` branch's body) live in the
//! same descriptor as its parent, addressed by root index. The indices
//! are assigned by the split pass in emission order, which is exactly
//! the order the macro numbers its tags — that correspondence is the
//! whole addressing scheme, and it is tested over the parity corpus.

#![forbid(unsafe_code)]

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// A `'static`-or-owned string. Borrowed in a compiled-in descriptor,
/// owned in a deserialized one.
pub type Text = Cow<'static, str>;

/// A `'static`-or-owned slice. See [`Text`].
pub type List<T> = Cow<'static, [T]>;

// ===========================================================================
// Site identity
// ===========================================================================

/// Identifies one `ui!` call site.
///
/// The site is named by WHERE it is written, because that is the one
/// thing both halves of the system can see: the proc macro reads it off
/// its own call span, and the build-time producer reads it off the file
/// it is parsing. Exactly four things feed it, and nothing else:
///
/// - `package` — `CARGO_PKG_NAME` of the crate being compiled. Two
///   crates both having a `src/lib.rs` would otherwise collide.
/// - `file` — the source path RELATIVE to that package's
///   `CARGO_MANIFEST_DIR`, `/`-separated (`src/screens/login.rs`). The
///   absolute prefix is stripped so a descriptor produced on one
///   machine addresses a binary built on another.
/// - `line`, `col` — 1-based position of the `ui!` invocation, as the
///   macro's call span reports it.
///
/// [`key`](SiteId::key) folds those four into the `u64` the compiled
/// code actually carries in each node's tag. The full `SiteId` stays in
/// the build artifact, where a human can read it.
///
/// # Why position is part of the identity
///
/// It means inserting a line above a site re-keys it. That is a real
/// cost and it is deliberate: the alternative — hashing the site's
/// tokens — re-keys the site on exactly the edits the overlay exists to
/// serve (change a literal, change the key), which is worse. A moved
/// site simply looks to the differ like one site gone and another
/// arrived, and the answer is a normal rebuild. A literal edit, the case
/// that matters, does not move anything.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SiteId {
    pub package: Text,
    pub file: Text,
    pub line: u32,
    pub col: u32,
}

impl SiteId {
    /// The `u64` the binary carries for this site. See [`site_key`].
    pub fn key(&self) -> u64 {
        site_key(&self.package, &self.file, self.line, self.col)
    }
}

impl fmt::Display for SiteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}:{}:{}", self.package, self.file, self.line, self.col)
    }
}

/// FNV-1a 64-bit offset basis and prime. FNV and not a cryptographic
/// hash: this is an addressing key, not a signature, and it has to be
/// computable in a `const fn` with no dependencies on either side of the
/// build.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Fold a site's four identifying parts into the key the compiled code
/// carries.
///
/// `const fn` so a caller can place the key in a `const`; the proc macro
/// computes it at expansion time and splices the resulting integer
/// literal, so the compiled code contains a number and no hashing code.
///
/// The hashed bytes are exactly `package`, `0x1f`, `file`, `0x1f`, then
/// `line` and `col` little-endian — separated so `("ab", "c")` and
/// `("a", "bc")` cannot collide.
pub const fn site_key(package: &str, file: &str, line: u32, col: u32) -> u64 {
    /// Unit separator: cannot occur in a package name or a path.
    const SEP: u8 = 0x1f;

    let mut hash = FNV_OFFSET_BASIS;
    hash = fold_bytes(hash, package.as_bytes());
    hash = fold_byte(hash, SEP);
    hash = fold_bytes(hash, file.as_bytes());
    hash = fold_byte(hash, SEP);
    hash = fold_u32(hash, line);
    fold_u32(hash, col)
}

const fn fold_byte(hash: u64, byte: u8) -> u64 {
    (hash ^ byte as u64).wrapping_mul(FNV_PRIME)
}

const fn fold_bytes(mut hash: u64, bytes: &[u8]) -> u64 {
    let mut i = 0;
    while i < bytes.len() {
        hash = fold_byte(hash, bytes[i]);
        i += 1;
    }
    hash
}

const fn fold_u32(mut hash: u64, value: u32) -> u64 {
    let bytes = value.to_le_bytes();
    let mut i = 0;
    while i < bytes.len() {
        hash = fold_byte(hash, bytes[i]);
        i += 1;
    }
    hash
}

/// The split pass's numbering version.
///
/// A descriptor addresses a binary by node INDEX, and the indices come
/// from the split pass's walk. Change the walk and every index in a
/// previously produced descriptor means something else — a patch built
/// against the old numbering would silently edit the wrong node, which
/// is the worst failure this system can have. So the number goes into
/// the build artifact and into the binary (as
/// `runtime_vocabulary::overlay::SPLIT_VERSION`), and a differ refuses a
/// pair that disagrees.
///
/// **Bump this whenever node numbering changes.** The parity suite pins
/// the corpus's numbering, so a change that needs a bump fails there
/// first with a message saying so.
pub const SPLIT_VERSION: u32 = 1;

// ===========================================================================
// Slots
// ===========================================================================

/// What one slot feeds, as recorded by the split pass.
///
/// `kind` is a SYNTACTIC label (`"closure"`, `"path"`, `"call"`, …), not
/// a Rust type: a proc macro has tokens, never resolved types. It exists
/// so a patch's slot list can be checked for shape drift against the
/// compiled code that will supply the values — a descriptor whose slot 3
/// was a `closure` cannot be replaced by one expecting a `path` there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotInfo {
    /// The prop name this slot feeds, when it has one.
    ///
    /// Unused today. It is what a later patch protocol would match a
    /// slot against by NAME rather than by position, so a descriptor
    /// that reorders props stays compatible with already-compiled slot
    /// code.
    pub name: Option<Text>,
    /// `"prop"`, `"text"`, `"child"`, `"cond"`, `"scrutinee"`, `"iter"`,
    /// `"key"` — see `runtime_macros`' `SlotRole`.
    pub role: Text,
    pub kind: Text,
}

/// The ordered slot list a descriptor expects.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotSig {
    pub slots: List<SlotInfo>,
}

impl SlotSig {
    /// How many slots the site supplies.
    pub fn count(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&SlotInfo> {
        self.slots.get(index)
    }
}

// ===========================================================================
// Values
// ===========================================================================

/// A literal the descriptor carries as data. These are exactly the
/// values a static edit should be able to change without recompiling.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum LiteralValue {
    Str(Text),
    Int(i64),
    Float(f64),
    Bool(bool),
    /// An enum-like path (`tone::Danger`) or a style-token accessor
    /// (`t.color.text()`), recorded as its source text.
    ///
    /// The *value* is not reconstructible from the text by generated
    /// code — a props struct cannot build an arbitrary variant of an
    /// arbitrary type from a string without a `FromStr`-shaped bound on
    /// every prop type. So an applier that meets a `Path` returns
    /// "not applied" and the emission keeps a site-local resolver for
    /// the paths it actually saw. The text is still what a descriptor
    /// diff compares, which is why it is recorded at all.
    Path(Text),
}

impl LiteralValue {
    /// A stable label for the variant, for error messages and shape
    /// comparison.
    pub fn kind(&self) -> &'static str {
        match self {
            LiteralValue::Str(_) => "str",
            LiteralValue::Int(_) => "int",
            LiteralValue::Float(_) => "float",
            LiteralValue::Bool(_) => "bool",
            LiteralValue::Path(_) => "path",
        }
    }
}

/// A prop's value: descriptor data, or a reference into the slot list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PropValue {
    Lit(LiteralValue),
    Slot(u32),
}

/// One `name = value` pair on a node.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PropEntry {
    pub name: Text,
    pub value: PropValue,
}

// ===========================================================================
// Nodes
// ===========================================================================

/// One node of a descriptor's flat node array.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Node {
    /// A builtin primitive.
    ///
    /// `kind` is the CANONICAL primitive name — exactly the string
    /// `runtime_macros`' `canonical_primitive` yields (`"view"`,
    /// `"text"`, `"anchored_overlay"`, …). A string and not a closed
    /// enum on purpose: the overlay never CONSTRUCTS a tree from a
    /// descriptor, it addresses one that already exists — so every
    /// primitive has to be nameable, including ones no applier knows how
    /// to build. A closed enum would make "addressable" and
    /// "constructible" the same set, and they are not.
    Prim {
        kind: Text,
        props: List<PropEntry>,
        /// Indices into [`Descriptor::nodes`], in child order.
        children: List<u32>,
    },
    /// A `#[component]` invocation.
    ///
    /// `path` is the tag as written at the call site (PascalCase), which
    /// doubles as the props-type path: `#[component]` emits a
    /// `pub type Tag = TagProps` alias, so the tag resolves to the type.
    Component {
        path: Text,
        props: List<PropEntry>,
        children: List<u32>,
        /// Props the site passes as CODE, by name.
        ///
        /// A dynamic prop's value lives in the compiled emission, not
        /// here. Recording the NAMES is what keeps the descriptor
        /// honest: a reader — and [`validate`] — can see which of a
        /// component's props an edit could change and which are baked
        /// in.
        dynamic: List<Text>,
    },
    /// A shape the descriptor addresses as a UNIT but cannot patch
    /// inside: a generic `flat_list` / `link(route =)`, an `if let`, a
    /// `match` arm that binds, a `for`, a node carrying a trailing
    /// `.method(…)` chain.
    ///
    /// Each is irreducibly code — a type parameter, a pattern binding,
    /// an open-ended builder call — so the descriptor records that
    /// something is here, which slot defines it, and what hangs under
    /// it, and stops.
    ///
    /// `children` is NOT always empty: a `for`'s row body keeps its
    /// nested-template nodes under this one, so a row template's
    /// literals stay patchable even though the iteration is not.
    Opaque {
        /// The construct's defining expression — a condition, an
        /// iterable, a scrutinee, a bare expression child — as
        /// whitespace-squashed source text. `None` where there is no
        /// single such expression (a node whose only irreducible part is
        /// a trailing `.method(…)` chain).
        ///
        /// Text and not a slot index: the only consumer is a DIFFER,
        /// which needs to know whether the expression changed between
        /// two source versions, and text answers that exactly. A slot
        /// index would answer a question nobody asks — the expression is
        /// compiled code either way, so an edit to it is a rebuild.
        expr: Option<Text>,
        children: List<u32>,
    },
}

// ===========================================================================
// Descriptor
// ===========================================================================

/// One `ui!` site, as data.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Descriptor {
    pub site: SiteId,
    pub slots: SlotSig,
    /// Flat node array; children are indices into it.
    pub nodes: List<Node>,
    /// The site's top-level nodes, in order.
    pub roots: List<u32>,
}

impl Descriptor {
    pub fn node(&self, index: u32) -> Option<&Node> {
        self.nodes.get(index as usize)
    }

    /// Every slot index the tree references, lowest first. Useful for
    /// asserting a descriptor uses the slots its `SlotSig` declares.
    pub fn referenced_slots(&self) -> Vec<u32> {
        let mut out = Vec::new();
        for node in self.nodes.iter() {
            match node {
                Node::Prim { props, .. } | Node::Component { props, .. } => {
                    for p in props.iter() {
                        if let PropValue::Slot(s) = p.value {
                            out.push(s);
                        }
                    }
                }
                Node::Opaque { .. } => {}
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// How many nodes are PATCHABLE (anything but [`Node::Opaque`]).
    /// Reported per fixture by the overlay suite, so the boundary stays
    /// visible rather than assumed.
    pub fn patchable_node_count(&self) -> usize {
        self.nodes.iter().filter(|n| !matches!(n, Node::Opaque { .. })).count()
    }
}

// ===========================================================================
// Registry
// ===========================================================================

/// The descriptors a program knows about, keyed by site.
///
/// Registration is what makes a site patchable: [`validate`] refuses a
/// patch for a site it has never seen, because there is nothing to check
/// its slot signature against.
#[derive(Default, Debug)]
pub struct Registry {
    sites: HashMap<SiteId, Descriptor>,
}

impl Registry {
    pub fn new() -> Registry {
        Registry::default()
    }

    /// Register (or replace) `descriptor` under its own site id,
    /// returning any descriptor it displaced.
    pub fn register(&mut self, descriptor: Descriptor) -> Option<Descriptor> {
        self.sites.insert(descriptor.site.clone(), descriptor)
    }

    pub fn get(&self, site: &SiteId) -> Option<&Descriptor> {
        self.sites.get(site)
    }

    pub fn contains(&self, site: &SiteId) -> bool {
        self.sites.contains_key(site)
    }

    pub fn len(&self) -> usize {
        self.sites.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sites.is_empty()
    }

    pub fn sites(&self) -> impl Iterator<Item = &SiteId> {
        self.sites.keys()
    }
}

// ===========================================================================
// Well-formedness
// ===========================================================================

/// Why a [`Descriptor`] is malformed.
///
/// These are the errors a descriptor can have ON ITS OWN. Whether one
/// descriptor may REPLACE another — whether an edit disturbed a slot the
/// compiled code supplies — is a question about two source versions, and
/// it is answered by the differ, which has both. It was never answerable
/// here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Malformed {
    /// A child / root index points past the node array.
    NodeIndexOutOfRange { index: u32, nodes: usize },
    /// A prop references a slot the signature does not declare.
    SlotIndexOutOfRange { index: u32, slots: usize },
    /// The descriptor has nodes but no roots, so nothing reaches them.
    /// A descriptor with NEITHER is legal — that is an empty `ui! {}`.
    UnreachableNodes,
}

impl fmt::Display for Malformed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Malformed::NodeIndexOutOfRange { index, nodes } => {
                write!(f, "node index {index} out of range ({nodes} nodes)")
            }
            Malformed::SlotIndexOutOfRange { index, slots } => {
                write!(f, "slot index {index} out of range ({slots} slots)")
            }
            Malformed::UnreachableNodes => write!(f, "descriptor has nodes but no roots"),
        }
    }
}

impl std::error::Error for Malformed {}

/// Check a descriptor's internal consistency: every child and root index
/// in range, every slot reference declared.
///
/// Run on the producer's own output over the whole parity corpus, so it
/// is checked against real trees rather than hand-written ones.
pub fn check_well_formed(descriptor: &Descriptor) -> Result<(), Malformed> {
    let nodes = descriptor.nodes.len();
    let slots = descriptor.slots.count();
    if descriptor.roots.is_empty() && nodes > 0 {
        return Err(Malformed::UnreachableNodes);
    }
    let node_ok = |i: u32| -> Result<(), Malformed> {
        if (i as usize) < nodes {
            Ok(())
        } else {
            Err(Malformed::NodeIndexOutOfRange { index: i, nodes })
        }
    };
    let slot_ok = |i: u32| -> Result<(), Malformed> {
        if (i as usize) < slots {
            Ok(())
        } else {
            Err(Malformed::SlotIndexOutOfRange { index: i, slots })
        }
    };
    for &r in descriptor.roots.iter() {
        node_ok(r)?;
    }
    for node in descriptor.nodes.iter() {
        match node {
            Node::Prim { props, children, .. } | Node::Component { props, children, .. } => {
                for p in props.iter() {
                    if let PropValue::Slot(s) = p.value {
                        slot_ok(s)?;
                    }
                }
                for &c in children.iter() {
                    node_ok(c)?;
                }
            }
            Node::Opaque { children, .. } => {
                for &c in children.iter() {
                    node_ok(c)?;
                }
            }
        }
    }
    Ok(())
}

// ===========================================================================
// Patch
// ===========================================================================

/// What a patch asks an applier to change about one node.
///
/// Every edit names a node by the index the split pass gave it, which is
/// the index the compiled code tags the built `Element` with. Nothing
/// here refers to a descriptor: the applier has only the running tree
/// and this list, and it must be able to act on the pair alone. That is
/// why every new value is inline.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Edit {
    /// Give a node's prop a new literal value.
    ///
    /// The prop must be one the node carries as DATA. A prop whose value
    /// is a slot is compiled code; changing it is a rebuild, and a
    /// differ never emits this edit for one.
    SetProp { node: u32, name: Text, value: LiteralValue },
    /// Replace a node's children with these subtrees.
    ///
    /// Insert, remove and reorder are all this one edit, because all
    /// three are "the child list is now that" — and expressing them
    /// separately would mean the applier reconciling positions against a
    /// descriptor it does not have.
    ///
    /// Only legal where every current child is a plain node. An applier
    /// that finds a reactive region, a keyed list or a component
    /// boundary among them refuses the edit: those are code, and their
    /// position in the list is decided at runtime.
    SetChildren { node: u32, children: List<NewNode> },
}

/// A subtree a patch asks to be CONSTRUCTED.
///
/// Fully static by construction: `props` carries literals only. A
/// subtree that referenced a slot would be code, and code cannot be
/// built from data — a differ that meets one rejects the edit rather
/// than emitting a `NewNode` that no applier could honour.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NewNode {
    /// A canonical primitive name (`"view"`, `"text"`) or a
    /// `#[component]` tag. Whether it can actually be built is the
    /// applier's table to answer; an unknown name is a refused edit,
    /// never a panic.
    pub kind: Text,
    pub props: List<PropEntry>,
    pub children: List<NewNode>,
}

/// One site's worth of edits.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Patch {
    pub site: SiteId,
    pub edits: List<Edit>,
}

impl Patch {
    /// The site key the compiled code tags with — what an applier
    /// stores this patch under.
    pub fn key(&self) -> u64 {
        self.site.key()
    }

    /// Every node index this patch touches, lowest first.
    pub fn nodes(&self) -> Vec<u32> {
        let mut out: Vec<u32> = self
            .edits
            .iter()
            .map(|e| match e {
                Edit::SetProp { node, .. } | Edit::SetChildren { node, .. } => *node,
            })
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_site_key_folds_all_four_parts() {
        let base = site_key("app", "src/a.rs", 10, 4);
        assert_ne!(base, site_key("other", "src/a.rs", 10, 4), "package must count");
        assert_ne!(base, site_key("app", "src/b.rs", 10, 4), "file must count");
        assert_ne!(base, site_key("app", "src/a.rs", 11, 4), "line must count");
        assert_ne!(base, site_key("app", "src/a.rs", 10, 5), "column must count");
        assert_eq!(base, site_key("app", "src/a.rs", 10, 4), "and it is a function");
    }

    /// The separator is the reason `("ab", "c")` and `("a", "bc")` are
    /// different sites. Without it they would hash the same byte run and
    /// a patch for one would address the other.
    #[test]
    fn a_site_key_separates_its_parts() {
        assert_ne!(site_key("ab", "c", 1, 1), site_key("a", "bc", 1, 1));
    }

    #[test]
    fn a_site_id_agrees_with_the_free_function() {
        let id = SiteId {
            package: Cow::Borrowed("app"),
            file: Cow::Borrowed("src/screens/login.rs"),
            line: 42,
            col: 9,
        };
        assert_eq!(id.key(), site_key("app", "src/screens/login.rs", 42, 9));
        assert_eq!(id.to_string(), "app/src/screens/login.rs:42:9");
    }

    fn site(file: &'static str) -> SiteId {
        SiteId {
            package: Cow::Borrowed("test"),
            file: Cow::Borrowed(file),
            line: 1,
            col: 1,
        }
    }

    fn sig(roles: &[(&'static str, &'static str)]) -> SlotSig {
        SlotSig {
            slots: Cow::Owned(
                roles
                    .iter()
                    .map(|(role, kind)| SlotInfo {
                        name: None,
                        role: Cow::Borrowed(role),
                        kind: Cow::Borrowed(kind),
                    })
                    .collect(),
            ),
        }
    }

    fn text_node(content: &'static str) -> Node {
        Node::Prim {
            kind: Cow::Borrowed("text"),
            props: Cow::Owned(vec![PropEntry {
                name: Cow::Borrowed("content"),
                value: PropValue::Lit(LiteralValue::Str(Cow::Borrowed(content))),
            }]),
            children: Cow::Borrowed(&[]),
        }
    }

    fn one_text(hash: &'static str) -> Descriptor {
        Descriptor {
            site: site(hash),
            slots: sig(&[]),
            nodes: Cow::Owned(vec![text_node("hi")]),
            roots: Cow::Owned(vec![0]),
        }
    }

    #[test]
    fn round_trips_through_serde() {
        let d = one_text("a");
        let json = serde_json::to_string(&d).unwrap();
        let back: Descriptor = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn registry_registers_by_site() {
        let mut reg = Registry::new();
        assert!(reg.is_empty());
        assert!(reg.register(one_text("a")).is_none());
        assert_eq!(reg.len(), 1);
        assert!(reg.contains(&site("a")));
        // Re-registering the same site replaces, and hands back the old.
        assert!(reg.register(one_text("a")).is_some());
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn a_child_index_past_the_node_array_is_malformed() {
        let mut d = one_text("a");
        d.roots = Cow::Owned(vec![7]);
        assert_eq!(
            check_well_formed(&d),
            Err(Malformed::NodeIndexOutOfRange { index: 7, nodes: 1 })
        );
    }

    #[test]
    fn a_prop_referencing_an_undeclared_slot_is_malformed() {
        let mut d = one_text("a");
        d.nodes = Cow::Owned(vec![Node::Prim {
            kind: Cow::Borrowed("view"),
            props: Cow::Owned(vec![PropEntry {
                name: Cow::Borrowed("style"),
                value: PropValue::Slot(2),
            }]),
            children: Cow::Borrowed(&[]),
        }]);
        assert_eq!(
            check_well_formed(&d),
            Err(Malformed::SlotIndexOutOfRange { index: 2, slots: 0 })
        );
    }

    /// Nodes with no roots are unreachable; NEITHER is `ui! {}`, which
    /// is legal and must not be reported.
    #[test]
    fn nodes_without_roots_are_unreachable_but_emptiness_is_legal() {
        let mut d = one_text("a");
        d.roots = Cow::Borrowed(&[]);
        assert_eq!(check_well_formed(&d), Err(Malformed::UnreachableNodes));

        let mut empty = one_text("a");
        empty.nodes = Cow::Borrowed(&[]);
        empty.roots = Cow::Borrowed(&[]);
        assert_eq!(check_well_formed(&empty), Ok(()));
    }

    /// A patch names nodes and carries its new values inline. It must
    /// survive the trip through JSON a dev server puts it on, and it
    /// must be able to say "these edits touch nodes 1 and 4" without
    /// anything else in hand — that is all an applier gets.
    #[test]
    fn a_patch_round_trips_and_names_the_nodes_it_touches() {
        let patch = Patch {
            site: site("src/screen.rs"),
            edits: Cow::Owned(vec![
                Edit::SetProp {
                    node: 4,
                    name: Cow::Borrowed("content"),
                    value: LiteralValue::Str(Cow::Borrowed("Sign in.")),
                },
                Edit::SetProp {
                    node: 1,
                    name: Cow::Borrowed("label"),
                    value: LiteralValue::Int(3),
                },
                Edit::SetChildren {
                    node: 1,
                    children: Cow::Owned(vec![NewNode {
                        kind: Cow::Borrowed("text"),
                        props: Cow::Owned(vec![PropEntry {
                            name: Cow::Borrowed("content"),
                            value: PropValue::Lit(LiteralValue::Str(Cow::Borrowed("new"))),
                        }]),
                        children: Cow::Borrowed(&[]),
                    }]),
                },
            ]),
        };
        assert_eq!(patch.nodes(), vec![1, 4]);
        assert_eq!(patch.key(), patch.site.key());

        let json = serde_json::to_string(&patch).expect("serialize");
        let back: Patch = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, patch);
    }

    #[test]
    fn referenced_slots_and_native_counts() {
        let d = Descriptor {
            site: site("a"),
            slots: sig(&[("prop", "path"), ("child", "call")]),
            nodes: Cow::Owned(vec![
                Node::Prim {
                    kind: Cow::Borrowed("view"),
                    props: Cow::Owned(vec![PropEntry {
                        name: Cow::Borrowed("style"),
                        value: PropValue::Slot(0),
                    }]),
                    children: Cow::Owned(vec![1, 2]),
                },
                Node::Prim {
                    kind: Cow::Borrowed("text"),
                    props: Cow::Owned(vec![PropEntry {
                        name: Cow::Borrowed("content"),
                        value: PropValue::Slot(1),
                    }]),
                    children: Cow::Borrowed(&[]),
                },
                Node::Opaque { expr: Some(Cow::Borrowed("items.get()")), children: Cow::Borrowed(&[]) },
            ]),
            roots: Cow::Owned(vec![0]),
        };
        assert_eq!(d.referenced_slots(), vec![0, 1]);
        assert_eq!(d.patchable_node_count(), 2);
        assert_eq!(check_well_formed(&d), Ok(()));
    }

}
