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
        /// The slot carrying the construct's defining expression — a
        /// condition, an iterable, a scrutinee. `None` where there is no
        /// single such expression (a chained node, an escaped
        /// primitive).
        slot: Option<u32>,
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
                Node::Opaque { slot, .. } => out.extend(*slot),
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
// Patch + validation
// ===========================================================================

/// A replacement descriptor for one site.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Patch {
    pub site: SiteId,
    pub descriptor: Descriptor,
}

/// Why a [`Patch`] cannot be applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValidationError {
    /// The patch's own `site` and its descriptor's `site` disagree.
    SiteIdMismatch { patch: SiteId, descriptor: SiteId },
    /// Nothing registered under that site — there is no signature to
    /// check the patch against, so accepting it would be a guess.
    UnknownSite(SiteId),
    /// The patch expects a different number of slots than the compiled
    /// site supplies.
    SlotCountMismatch { expected: usize, found: usize },
    /// A slot's recorded shape drifted. The compiled code supplies a
    /// value of one shape; the patch would use it as another.
    SlotShapeMismatch { index: usize, expected: SlotInfo, found: SlotInfo },
    /// A child / root index points past the node array.
    NodeIndexOutOfRange { index: u32, nodes: usize },
    /// A prop or thunk references a slot the signature does not declare.
    SlotIndexOutOfRange { index: u32, slots: usize },
    /// The descriptor has nodes but no roots, so nothing reaches them.
    /// A descriptor with NEITHER is legal — that is an empty `ui! {}`,
    /// which the builder renders as an empty `view`.
    UnreachableNodes,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValidationError::SiteIdMismatch { patch, descriptor } => {
                write!(f, "patch site {patch} does not match its descriptor's site {descriptor}")
            }
            ValidationError::UnknownSite(s) => write!(f, "no descriptor registered for site {s}"),
            ValidationError::SlotCountMismatch { expected, found } => {
                write!(f, "slot count mismatch: compiled site supplies {expected}, patch wants {found}")
            }
            ValidationError::SlotShapeMismatch { index, expected, found } => write!(
                f,
                "slot {index} shape mismatch: compiled site supplies {:?}/{:?}, patch wants {:?}/{:?}",
                expected.role, expected.kind, found.role, found.kind
            ),
            ValidationError::NodeIndexOutOfRange { index, nodes } => {
                write!(f, "node index {index} out of range ({nodes} nodes)")
            }
            ValidationError::SlotIndexOutOfRange { index, slots } => {
                write!(f, "slot index {index} out of range ({slots} slots)")
            }
            ValidationError::UnreachableNodes => {
                write!(f, "descriptor has nodes but no roots")
            }
        }
    }
}

impl std::error::Error for ValidationError {}

/// Check that `patch` could replace the descriptor registered for its
/// site.
///
/// Two classes of check, and both matter:
///
/// - **internal consistency** — every node/root index is in range and
///   every slot reference is declared. A descriptor that fails these is
///   malformed whatever it replaces.
/// - **slot-signature compatibility** — the patch must expect exactly
///   the slot list the compiled site supplies, shape for shape. The
///   slots are CODE: they were compiled into the binary and cannot be
///   patched. A descriptor that used slot 3 as a condition where the
///   binary supplies a text value would type-confuse the builder, so it
///   is rejected here rather than discovered at build time.
pub fn validate(patch: &Patch, registry: &Registry) -> Result<(), ValidationError> {
    if patch.site != patch.descriptor.site {
        return Err(ValidationError::SiteIdMismatch {
            patch: patch.site.clone(),
            descriptor: patch.descriptor.site.clone(),
        });
    }
    let compiled = registry
        .get(&patch.site)
        .ok_or_else(|| ValidationError::UnknownSite(patch.site.clone()))?;

    let expected = &compiled.slots;
    let found = &patch.descriptor.slots;
    if expected.count() != found.count() {
        return Err(ValidationError::SlotCountMismatch {
            expected: expected.count(),
            found: found.count(),
        });
    }
    for (index, (e, f)) in expected.slots.iter().zip(found.slots.iter()).enumerate() {
        // `name` is not compared: it is reserved for a name-matched
        // protocol that does not exist yet, and comparing it now would
        // reject harmless descriptor edits.
        if e.role != f.role || e.kind != f.kind {
            return Err(ValidationError::SlotShapeMismatch {
                index,
                expected: e.clone(),
                found: f.clone(),
            });
        }
    }

    check_well_formed(&patch.descriptor)
}

/// The internal-consistency half of [`validate`], usable on its own (the
/// emission's own descriptors are checked with it in tests).
pub fn check_well_formed(descriptor: &Descriptor) -> Result<(), ValidationError> {
    let nodes = descriptor.nodes.len();
    let slots = descriptor.slots.count();
    if descriptor.roots.is_empty() && nodes > 0 {
        return Err(ValidationError::UnreachableNodes);
    }
    let node_ok = |i: u32| -> Result<(), ValidationError> {
        if (i as usize) < nodes {
            Ok(())
        } else {
            Err(ValidationError::NodeIndexOutOfRange { index: i, nodes })
        }
    };
    let slot_ok = |i: u32| -> Result<(), ValidationError> {
        if (i as usize) < slots {
            Ok(())
        } else {
            Err(ValidationError::SlotIndexOutOfRange { index: i, slots })
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
            Node::Opaque { slot, children } => {
                if let Some(s) = slot {
                    slot_ok(*s)?;
                }
                for &c in children.iter() {
                    node_ok(c)?;
                }
            }
        }
    }
    Ok(())
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
    fn validate_accepts_a_matching_patch() {
        let mut reg = Registry::new();
        reg.register(one_text("a"));
        let mut patched = one_text("a");
        patched.nodes = Cow::Owned(vec![text_node("bye")]);
        let patch = Patch { site: site("a"), descriptor: patched };
        assert_eq!(validate(&patch, &reg), Ok(()));
    }

    #[test]
    fn validate_rejects_an_unregistered_site() {
        let reg = Registry::new();
        let patch = Patch { site: site("a"), descriptor: one_text("a") };
        assert_eq!(validate(&patch, &reg), Err(ValidationError::UnknownSite(site("a"))));
    }

    #[test]
    fn validate_rejects_a_site_id_mismatch() {
        let reg = Registry::new();
        let patch = Patch { site: site("a"), descriptor: one_text("b") };
        assert!(matches!(
            validate(&patch, &reg),
            Err(ValidationError::SiteIdMismatch { .. })
        ));
    }

    /// The slots are CODE: compiled into the binary and unpatchable. A
    /// patch that wants a different number of them cannot be honoured.
    #[test]
    fn validate_rejects_a_slot_count_change() {
        let mut reg = Registry::new();
        reg.register(one_text("a"));
        let mut patched = one_text("a");
        patched.slots = sig(&[("prop", "path")]);
        patched.nodes = Cow::Owned(vec![Node::Opaque { slot: Some(0), children: Cow::Borrowed(&[]) }]);
        let patch = Patch { site: site("a"), descriptor: patched };
        assert_eq!(
            validate(&patch, &reg),
            Err(ValidationError::SlotCountMismatch { expected: 0, found: 1 })
        );
    }

    #[test]
    fn validate_rejects_a_slot_shape_change() {
        let mut reg = Registry::new();
        let mut compiled = one_text("a");
        compiled.slots = sig(&[("prop", "path")]);
        compiled.nodes = Cow::Owned(vec![Node::Opaque { slot: Some(0), children: Cow::Borrowed(&[]) }]);
        reg.register(compiled);

        let mut patched = one_text("a");
        patched.slots = sig(&[("cond", "closure")]);
        patched.nodes = Cow::Owned(vec![Node::Opaque { slot: Some(0), children: Cow::Borrowed(&[]) }]);
        let patch = Patch { site: site("a"), descriptor: patched };
        assert!(matches!(
            validate(&patch, &reg),
            Err(ValidationError::SlotShapeMismatch { index: 0, .. })
        ));
    }

    /// A slot's `name` is reserved for a name-matched protocol that does
    /// not exist yet, so it must NOT participate in compatibility —
    /// otherwise renaming a prop in a descriptor edit would be rejected
    /// for no reason.
    #[test]
    fn validate_ignores_slot_names() {
        let mut reg = Registry::new();
        let mut compiled = one_text("a");
        compiled.slots = SlotSig {
            slots: Cow::Owned(vec![SlotInfo {
                name: Some(Cow::Borrowed("label")),
                role: Cow::Borrowed("prop"),
                kind: Cow::Borrowed("path"),
            }]),
        };
        compiled.nodes = Cow::Owned(vec![Node::Opaque { slot: Some(0), children: Cow::Borrowed(&[]) }]);
        reg.register(compiled);

        let mut patched = one_text("a");
        patched.slots = SlotSig {
            slots: Cow::Owned(vec![SlotInfo {
                name: Some(Cow::Borrowed("title")),
                role: Cow::Borrowed("prop"),
                kind: Cow::Borrowed("path"),
            }]),
        };
        patched.nodes = Cow::Owned(vec![Node::Opaque { slot: Some(0), children: Cow::Borrowed(&[]) }]);
        let patch = Patch { site: site("a"), descriptor: patched };
        assert_eq!(validate(&patch, &reg), Ok(()));
    }

    #[test]
    fn validate_rejects_out_of_range_indices() {
        let mut reg = Registry::new();
        reg.register(one_text("a"));

        let mut bad_root = one_text("a");
        bad_root.roots = Cow::Owned(vec![7]);
        assert_eq!(
            validate(&Patch { site: site("a"), descriptor: bad_root }, &reg),
            Err(ValidationError::NodeIndexOutOfRange { index: 7, nodes: 1 })
        );

        let mut bad_slot = one_text("a");
        bad_slot.nodes = Cow::Owned(vec![Node::Opaque { slot: Some(2), children: Cow::Borrowed(&[]) }]);
        assert_eq!(
            validate(&Patch { site: site("a"), descriptor: bad_slot }, &reg),
            Err(ValidationError::SlotIndexOutOfRange { index: 2, slots: 0 })
        );

        let mut no_roots = one_text("a");
        no_roots.roots = Cow::Borrowed(&[]);
        assert_eq!(
            validate(&Patch { site: site("a"), descriptor: no_roots }, &reg),
            Err(ValidationError::UnreachableNodes)
        );

        // An EMPTY descriptor is legal — that is `ui! {}`.
        let mut empty = one_text("a");
        empty.nodes = Cow::Borrowed(&[]);
        empty.roots = Cow::Borrowed(&[]);
        assert_eq!(validate(&Patch { site: site("a"), descriptor: empty }, &reg), Ok(()));
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
                    children: Cow::Owned(vec![1]),
                },
                Node::Opaque { slot: Some(1), children: Cow::Borrowed(&[]) },
            ]),
            roots: Cow::Owned(vec![0]),
        };
        assert_eq!(d.referenced_slots(), vec![0, 1]);
        assert_eq!(d.patchable_node_count(), 1);
        assert_eq!(check_well_formed(&d), Ok(()));
    }

}
