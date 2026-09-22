//! Template **descriptors**: the static half of a `ui!` site.
//!
//! `ui!` has two lowerings. The *direct* one emits builder calls inline.
//! The *template* one splits each site into
//!
//! 1. a [`Descriptor`] — data: the node tree, tags, attribute names,
//!    literal values, child order, and a [`SlotSig`] for the dynamic
//!    expressions it does not carry; and
//! 2. an ordered **slot list** — the dynamic expressions themselves,
//!    evaluated at the call site,
//!
//! and hands both to `runtime_vocabulary::template::build`, which
//! constructs the `Element`.
//!
//! This crate owns (1). It knows nothing about builders, handlers or
//! transports, and depends only on `runtime-scene` and `serde`. That
//! narrowness is the point: a descriptor is a portable artifact. It
//! serializes; it can be [`validate`]d against a [`Registry`] with no
//! renderer in the graph; and a source other than "compiled into the
//! binary" can slot in behind [`TemplateSource`] without this crate
//! learning about transports. **The over-the-air path is not built** —
//! only the seam it would occupy, and [`CompiledIn`] is the single
//! implementation.
//!
//! # Const-constructible
//!
//! Every collection is a [`Cow`], so the compiled-in form is a plain
//! `static`:
//!
//! ```
//! use std::borrow::Cow;
//! use runtime_template::*;
//!
//! static DESC: Descriptor = Descriptor {
//!     site: SiteId { module: Cow::Borrowed("my_app::screen"), hash: Cow::Borrowed("0a1b2c3d") },
//!     slots: SlotSig { slots: Cow::Borrowed(&[]) },
//!     nodes: Cow::Borrowed(&[Node::Prim {
//!         kind: PrimKind::Text,
//!         props: Cow::Borrowed(&[PropEntry {
//!             name: Cow::Borrowed("content"),
//!             value: PropValue::Lit(LiteralValue::Str(Cow::Borrowed("hello"))),
//!         }]),
//!         children: Cow::Borrowed(&[]),
//!     }]),
//!     roots: Cow::Borrowed(&[0]),
//! };
//!
//! assert_eq!(DESC.nodes.len(), 1);
//! ```
//!
//! The same type round-trips through serde, so a descriptor read from
//! bytes is indistinguishable from one the compiler baked in.
//!
//! # Node indices, not nesting
//!
//! [`Descriptor::nodes`] is FLAT and children are `u32` indices into it.
//! Nesting `Node` inside `Node` would need a `Box` per level, which is
//! not const-constructible; a flat array also lets a nested template
//! (an `if` branch's body) live in the same descriptor as its parent,
//! addressed by root index.

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
/// `module` is the expansion's `module_path!()` and `hash` is a stable
/// digest of the site's body tokens. Together they survive a rebuild
/// that does not touch the site, and change when it does — which is what
/// makes a descriptor addressable across two compilations of the same
/// program.
///
/// Source line is deliberately NOT part of the identity: adding a blank
/// line above a site would otherwise re-key it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SiteId {
    pub module: Text,
    pub hash: Text,
}

impl fmt::Display for SiteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", self.module, self.hash)
    }
}

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

/// The builtin primitives the template builder constructs from the
/// descriptor directly.
///
/// This is deliberately a CLOSED set, and deliberately smaller than the
/// builtin vocabulary. A primitive is listed here only once the builder
/// drives its glue wrapper from data; everything else reaches the scene
/// through [`Node::Escape`], which is correct but opaque to a static
/// edit. Widening the list is additive — a new variant plus its arm in
/// the builder — and the parity suite's coverage report is what says
/// whether it is worth it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PrimKind {
    View,
    Text,
    Button,
    Image,
    ActivityIndicator,
    ScrollView,
}

impl PrimKind {
    /// The `ui!` tag that lowers to this kind.
    pub fn tag(self) -> &'static str {
        match self {
            PrimKind::View => "view",
            PrimKind::Text => "text",
            PrimKind::Button => "button",
            PrimKind::Image => "image",
            PrimKind::ActivityIndicator => "activity_indicator",
            PrimKind::ScrollView => "scroll_view",
        }
    }

    /// Parse a canonical `ui!` primitive tag. `None` for a primitive the
    /// descriptor does not model — the emission escapes those.
    pub fn from_tag(tag: &str) -> Option<PrimKind> {
        Some(match tag {
            "view" => PrimKind::View,
            "text" => PrimKind::Text,
            "button" => PrimKind::Button,
            "image" => PrimKind::Image,
            "activity_indicator" => PrimKind::ActivityIndicator,
            "scroll_view" => PrimKind::ScrollView,
            _ => return None,
        })
    }
}

/// One node of a descriptor's flat node array.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Node {
    /// A builtin primitive the builder constructs itself.
    Prim {
        kind: PrimKind,
        props: List<PropEntry>,
        /// Indices into [`Descriptor::nodes`], in child order.
        children: List<u32>,
    },
    /// A `#[component]` tag.
    ///
    /// The builder cannot name the props type, so the site supplies a
    /// typed constructor in slot `ctor`; the builder hands it the
    /// `literals` (which the component's generated `__apply_literal`
    /// applies by name) and the built children. `tag` is recorded for
    /// diagnostics and for a descriptor diff, not for dispatch.
    Component {
        tag: Text,
        ctor: u32,
        literals: List<PropEntry>,
        children: List<u32>,
    },
    /// A reactive `if`: a `Fn() -> bool` in slot `cond` selects between
    /// two `Fn() -> Element` branch thunks. Each branch is its own
    /// NESTED TEMPLATE — the thunk builds from this same descriptor,
    /// which is why the node array is flat.
    Dyn {
        cond: u32,
        then: u32,
        otherwise: u32,
    },
    /// A subtree the descriptor does not model: the site built it and
    /// left the finished `Element`(s) in slot `slot`.
    ///
    /// Correct, but opaque — an escaped subtree's literals are compiled
    /// in, so a static edit inside one needs a rebuild. Every `ui!`
    /// construct is expressible this way, which is what lets the
    /// template lowering be complete from day one while the
    /// descriptor-native set grows.
    Escape { slot: u32 },
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
                Node::Prim { props, .. } => {
                    for p in props.iter() {
                        if let PropValue::Slot(s) = p.value {
                            out.push(s);
                        }
                    }
                }
                Node::Component { ctor, literals, .. } => {
                    out.push(*ctor);
                    for p in literals.iter() {
                        if let PropValue::Slot(s) = p.value {
                            out.push(s);
                        }
                    }
                }
                Node::Dyn { cond, then, otherwise } => {
                    out.push(*cond);
                    out.push(*then);
                    out.push(*otherwise);
                }
                Node::Escape { slot } => out.push(*slot),
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// How many nodes are descriptor-native (anything but
    /// [`Node::Escape`]). The parity suite reports this per fixture so
    /// the descriptor-native boundary is visible rather than assumed.
    pub fn native_node_count(&self) -> usize {
        self.nodes.iter().filter(|n| !matches!(n, Node::Escape { .. })).count()
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
            Node::Prim { props, children, .. } => {
                for p in props.iter() {
                    if let PropValue::Slot(s) = p.value {
                        slot_ok(s)?;
                    }
                }
                for &c in children.iter() {
                    node_ok(c)?;
                }
            }
            Node::Component { ctor, literals, children, .. } => {
                slot_ok(*ctor)?;
                for p in literals.iter() {
                    if let PropValue::Slot(s) = p.value {
                        slot_ok(s)?;
                    }
                }
                for &c in children.iter() {
                    node_ok(c)?;
                }
            }
            Node::Dyn { cond, then, otherwise } => {
                slot_ok(*cond)?;
                slot_ok(*then)?;
                slot_ok(*otherwise)?;
            }
            Node::Escape { slot } => slot_ok(*slot)?,
        }
    }
    Ok(())
}

// ===========================================================================
// TemplateSource
// ===========================================================================

/// Where the descriptor a site builds from comes from.
///
/// The seam exists so that "compiled into the binary" is not the only
/// possible answer — a later over-the-air path would be another
/// implementation, resolving a site against descriptors received at
/// runtime and falling back to the compiled one. **That path is not
/// built**: [`CompiledIn`] is the only implementation, and it is what
/// the emission uses.
///
/// Keeping the seam here, in the data crate, is what stops the idea from
/// leaking into the builder: a source hands back a `&Descriptor` and the
/// builder never learns where it came from.
pub trait TemplateSource {
    /// The descriptor to build `site` from, given the one the compiler
    /// baked in. Must return a descriptor whose slot signature is
    /// compatible with `compiled`'s — [`validate`] is how a source
    /// establishes that before it starts handing out replacements.
    fn resolve<'a>(&'a self, site: &SiteId, compiled: &'a Descriptor) -> &'a Descriptor;
}

/// The only [`TemplateSource`]: always the compiled-in descriptor.
#[derive(Clone, Copy, Debug, Default)]
pub struct CompiledIn;

impl TemplateSource for CompiledIn {
    fn resolve<'a>(&'a self, _site: &SiteId, compiled: &'a Descriptor) -> &'a Descriptor {
        compiled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(hash: &'static str) -> SiteId {
        SiteId { module: Cow::Borrowed("test"), hash: Cow::Borrowed(hash) }
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
            kind: PrimKind::Text,
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
    fn prim_kinds_round_trip_their_tags() {
        for kind in [
            PrimKind::View,
            PrimKind::Text,
            PrimKind::Button,
            PrimKind::Image,
            PrimKind::ActivityIndicator,
            PrimKind::ScrollView,
        ] {
            assert_eq!(PrimKind::from_tag(kind.tag()), Some(kind));
        }
        assert_eq!(PrimKind::from_tag("overlay"), None);
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
        patched.nodes = Cow::Owned(vec![Node::Escape { slot: 0 }]);
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
        compiled.nodes = Cow::Owned(vec![Node::Escape { slot: 0 }]);
        reg.register(compiled);

        let mut patched = one_text("a");
        patched.slots = sig(&[("cond", "closure")]);
        patched.nodes = Cow::Owned(vec![Node::Escape { slot: 0 }]);
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
        compiled.nodes = Cow::Owned(vec![Node::Escape { slot: 0 }]);
        reg.register(compiled);

        let mut patched = one_text("a");
        patched.slots = SlotSig {
            slots: Cow::Owned(vec![SlotInfo {
                name: Some(Cow::Borrowed("title")),
                role: Cow::Borrowed("prop"),
                kind: Cow::Borrowed("path"),
            }]),
        };
        patched.nodes = Cow::Owned(vec![Node::Escape { slot: 0 }]);
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
        bad_slot.nodes = Cow::Owned(vec![Node::Escape { slot: 2 }]);
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
                    kind: PrimKind::View,
                    props: Cow::Owned(vec![PropEntry {
                        name: Cow::Borrowed("style"),
                        value: PropValue::Slot(0),
                    }]),
                    children: Cow::Owned(vec![1]),
                },
                Node::Escape { slot: 1 },
            ]),
            roots: Cow::Owned(vec![0]),
        };
        assert_eq!(d.referenced_slots(), vec![0, 1]);
        assert_eq!(d.native_node_count(), 1);
        assert_eq!(check_well_formed(&d), Ok(()));
    }

    #[test]
    fn compiled_in_source_always_returns_the_compiled_descriptor() {
        let d = one_text("a");
        let src = CompiledIn;
        assert!(std::ptr::eq(src.resolve(&site("a"), &d), &d));
    }
}
