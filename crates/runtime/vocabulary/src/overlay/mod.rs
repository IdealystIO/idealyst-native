//! The dev-time **overlay**: the runtime half of `ui-overlay`.
//!
//! `ui!` has one lowering. What this adds, behind a cargo feature that
//! is off by default, is the ability to *address* what that lowering
//! built: every node a `ui!` site produces carries a [`NodeTag`] naming
//! the site and the node's index within it. A patch names the same pair
//! and carries the new values inline; applying it edits the built
//! `Element` in place.
//!
//! # The binary carries numbers, not descriptions
//!
//! Nothing here holds a descriptor, and none is compiled into the app.
//! A descriptor — what node 7 of that site *is*, which props it had,
//! which slots it references — is produced from SOURCE at build time and
//! written beside the build, because measuring the alternative on a real
//! app showed a `static` descriptor per site cost +1.4 s on every
//! one-edit rebuild while tags alone cost nothing measurable
//! (`runtime_macros`' `ui_overlay` module docs have the table).
//!
//! That also puts validation in the right place. "Does this edit disturb
//! a slot the compiled code supplies?" is answerable by whoever has BOTH
//! source versions — the CLI or the dev server, which is where the diff
//! runs. It was never answerable well here, where only one side exists.
//!
//! What stays in the binary is:
//!
//! - [`tag`] calls, two integer literals each, and
//! - [`SPLIT_VERSION`], once per program, so a differ can refuse a
//!   binary whose node numbering it does not understand.
//!
//! # Why a feature and not `cfg(debug_assertions)`
//!
//! An over-the-air path would want this in a release build. A feature
//! can be turned on there; `debug_assertions` cannot.
//!
//! # How a patch reaches the tree
//!
//! Through [`tag`], which every node of every `ui!` site already passes
//! through as it is built. A staged patch is applied there, on the spot,
//! to the `Element` that node just produced. There is no separate walk
//! and no "apply once" step — which is the point: a site's `Element` is
//! rebuilt whenever its reactive scope re-runs, and a patch applied once
//! to a tree that is then rebuilt from the compiled code would silently
//! revert.
//!
//! A `#[component]`'s props are the exception, because they are consumed
//! before the node exists. The emission asks [`staged_props`] for them
//! at the call site and applies them to the props struct through the
//! generated `__apply_literal`, which has to resolve INHERENTLY on the
//! concrete type — hence a call site and not a function here.
//!
//! # Why tags and not positions
//!
//! A reactive branch, a keyed row or a `for` changes how many siblings
//! exist, so "the n-th child of this parent" is not a stable identity
//! across rebuilds. The tag is: it names the node of the site's
//! descriptor that produced this `Element`. Everything here looks nodes
//! up by tag — dynamic slots above all, which is what makes "patch the
//! static half, leave the code alone" sound.
//!
//! [`NodeTag`]: runtime_scene::NodeTag

mod apply;
mod construct;
pub mod live;
mod prims;
pub mod rebuild;

use std::cell::RefCell;
use std::collections::HashMap;

use runtime_scene::Element;
use runtime_template::{Edit, LiteralValue, Patch, SiteId};

pub use apply::Outcome;
pub use live::{apply_live, apply_live_to};
pub use rebuild::{OverlayProps, Probe, Rebuilder, ViaClone, ViaFallback};
pub use construct::{ctor_count, register_ctor, ComponentCtor};

thread_local! {
    /// Patches staged against sites, by the key the macro spells into
    /// each tag — so a tagged `Element` finds its edits without
    /// reconstructing a `SiteId`.
    ///
    /// Thread-local, not global: a scene is single-threaded (the whole
    /// reactive kernel is).
    static STAGED: RefCell<HashMap<u64, Vec<Edit>>> = RefCell::new(HashMap::new());
}

/// Stage a patch. Every subsequent build of that site applies it.
///
/// Replaces any patch already staged for the site: a patch is the whole
/// difference from the compiled source, not an increment, so merging two
/// would mean guessing which of a pair of edits to one prop is current.
pub fn stage(patch: Patch) {
    stage_key(patch.key(), patch.edits.into_owned());
}

/// As [`stage`], addressed by the key a tag carries rather than by a
/// [`SiteId`].
///
/// A dev server knows the site by name and uses [`stage`]. A caller that
/// read the key off a built tree — a test, a dev-tools probe — has only
/// the number, and hashing a `SiteId` backwards is not possible.
pub fn stage_key(site: u64, edits: Vec<Edit>) {
    STAGED.with(|s| s.borrow_mut().insert(site, edits));
}

/// Drop a site's patch, reverting it to its compiled form on the next
/// build.
pub fn unstage(site: &SiteId) {
    unstage_key(site.key());
}

/// As [`unstage`], by the key a tag carries.
pub fn unstage_key(site: u64) {
    STAGED.with(|s| s.borrow_mut().remove(&site));
}

/// How many sites have a patch staged.
pub fn staged_count() -> usize {
    STAGED.with(|s| s.borrow().len())
}

/// Forget every patch and every registered component constructor.
///
/// Test support: the state is thread-local and per-process, so a suite
/// that stages a patch in one case must not leak it into the next.
pub fn reset() {
    STAGED.with(|s| s.borrow_mut().clear());
    AMBIENT.with(|a| a.borrow_mut().clear());
    REBUILDER.with(|r| r.borrow_mut().clear());
    live::release_all();
    rebuild::release_all();
    // The scene's live-instance registry too: it is thread-local and
    // per-process, so a suite that mounts a tree in one case would
    // otherwise leave its instances matching in the next.
    runtime_scene::live::reset();
    construct::clear_ctors();
}

thread_local! {
    /// The `ui!` (site, node) whose component is being built, innermost
    /// last.
    ///
    /// A stack, because a component's children are built while its own
    /// props struct literal is being evaluated — `enter(A)`, then the
    /// children `enter(B)`/`exit(B)`, then `build(A_props)`. The top of
    /// the stack at the moment `build` runs is A, which is correct.
    ///
    /// `Option` per frame, and `take_current` TAKES: a component built
    /// by hand inside another's children (a bare
    /// `BuildElement::build(..)`, not a `ui!` tag) has no `enter` of its
    /// own, and taking means it finds nothing rather than silently
    /// consuming its parent's address and applying that node's patch to
    /// itself. A missed patch is recoverable; a patch applied to the
    /// wrong node is not.
    static AMBIENT: RefCell<Vec<Option<(u64, u32)>>> = const { RefCell::new(Vec::new()) };

    /// The rebuilder for each open frame, parallel to `AMBIENT`.
    ///
    /// Separate from `AMBIENT` because they are taken at different
    /// moments: the address is consumed by the component's `build`, the
    /// rebuilder by `exit` once the element exists.
    static REBUILDER: RefCell<Vec<Option<rebuild::Rebuilder>>> = const {
        RefCell::new(Vec::new())
    };
}

/// Announce which node of which site is about to be built.
///
/// Emitted by `ui!` immediately before a `#[component]`'s build
/// expression, and paired with [`exit`]. Deliberately a free function
/// taking two integers: it is emitted at every component call site in
/// the program, and anything generic — or anything requiring a trait in
/// scope — is resolution work multiplied by thousands of call sites. See
/// `runtime_macros`' `ui_overlay` for what that cost measured.
pub fn enter(site: u64, node: u32) {
    AMBIENT.with(|a| a.borrow_mut().push(Some((site, node))));
    REBUILDER.with(|r| r.borrow_mut().push(None));
}

/// Give the open frame a way to build this instance again.
///
/// Called from a props type's generated `build`, where the type is
/// concrete and the probe for `Clone` costs one resolution per TYPE
/// rather than per call site. [`exit`] attaches it to the element,
/// realize carries it to the mounted node, and a live patch to a
/// component's prop runs the component again from it. See
/// [`rebuild`](crate::overlay::rebuild).
pub fn set_rebuilder(rebuilder: Option<rebuild::Rebuilder>) {
    REBUILDER.with(|r| {
        if let Some(top) = r.borrow_mut().last_mut() {
            *top = rebuilder;
        }
    });
}

/// End the frame [`enter`] opened, returning the element unchanged.
///
/// Takes and returns the element so the pair brackets an EXPRESSION
/// rather than needing a block with a temporary, which keeps the
/// emission one statement wider instead of three.
pub fn exit(element: Element) -> Element {
    AMBIENT.with(|a| {
        a.borrow_mut().pop();
    });
    let rebuilder = REBUILDER.with(|r| r.borrow_mut().pop().flatten());
    match rebuild::erase(rebuilder) {
        Some(erased) => runtime_scene::with_rebuild(element, erased),
        None => element,
    }
}

/// Take the innermost address, if this build has one.
///
/// Called from a props type's generated `BuildElement::build`. See
/// [`AMBIENT`] on why it takes rather than reads.
pub fn take_current() -> Option<(u64, u32)> {
    AMBIENT.with(|a| a.borrow_mut().last_mut().and_then(|f| f.take()))
}

/// The literal prop edits staged for one node.
///
/// Called by the emission at every `#[component]` call site, BEFORE the
/// props struct is built into an `Element` — a component's props do not
/// survive into the tree, so this is the only moment they can be
/// changed. Returns an empty `Vec` (no allocation beyond the empty one)
/// when nothing is staged, which is every build of every site in the
/// normal case.
pub fn staged_props(site: u64, node: u32) -> Vec<(String, LiteralValue)> {
    STAGED.with(|s| {
        let s = s.borrow();
        let Some(edits) = s.get(&site) else { return Vec::new() };
        edits
            .iter()
            .filter_map(|e| match e {
                Edit::SetProp { node: n, name, value } if *n == node => {
                    Some((name.to_string(), value.clone()))
                }
                _ => None,
            })
            .collect()
    })
}

/// The split pass's numbering version, as this program was built with
/// it.
///
/// `#[no_mangle]` and `#[used]`: this is a marker a TOOL reads out of
/// the built artifact by symbol name, so it must survive both dead-code
/// elimination and the linker. Exactly one definition exists per
/// program, here, because every tagged node in the program was numbered
/// by the one proc-macro crate this one ships with.
///
/// A differ compares it against the version recorded in the descriptor
/// set it is diffing and refuses the pair on a mismatch, rather than
/// mis-addressing a patch to a node that has been renumbered under it.
#[no_mangle]
#[used]
pub static IDEALYST_UI_SPLIT_VERSION: u32 = runtime_template::SPLIT_VERSION;

/// The same value, reachable as a constant from Rust.
pub const SPLIT_VERSION: u32 = runtime_template::SPLIT_VERSION;

/// Attach an origin tag to a freshly built node.
///
/// Emitted around every node's expression under `ui-overlay`. Delegates
/// to `runtime_scene::with_tag`, which knows which `Element` variants
/// are nodes and which are structure.
///
/// `site` is `runtime_template::site_key` of the `ui!` invocation's
/// package, package-relative file, line and column; `node` is the index
/// the split pass gives this node. The macro splices both as integer
/// literals — there is no hashing, no string, and no allocation at
/// runtime.
///
/// Takes `impl IntoElement`, not `Element`: a primitive's expression is
/// still its BUILDER at that point (`GlueText`, `GlueView`, …) and only
/// coerces at the site's edge. Tagging has to accept the builder and do
/// the coercion itself — which does mean a tagged node reaches its
/// parent already an `Element` rather than a builder. That is invisible
/// to authors (every consuming position takes `impl IntoElement` or
/// `ChildList`) and is the one shape difference the feature makes.
pub fn tag(element: impl crate::glue::IntoElement, site: u64, node: u32) -> Element {
    let mut element = crate::glue::IntoElement::into_element(element);
    // Apply here, not in a separate pass: this is the one point every
    // node passes through on EVERY build of its site, which is what
    // makes a patch survive a rebuild.
    if let Some(edits) = edits_for(site, node) {
        let _ = apply::apply(&mut element, &edits);
    }
    runtime_scene::with_tag(element, runtime_scene::NodeTag { site, node })
}

/// This node's edits, or `None` — the fast path, and the one every
/// untouched node in every build takes.
fn edits_for(site: u64, node: u32) -> Option<Vec<Edit>> {
    STAGED.with(|s| {
        let s = s.borrow();
        let mine = edits_addressed_to(s.get(&site)?, node);
        (!mine.is_empty()).then_some(mine)
    })
}

/// Apply a patch's edits to an already-built tree, by tag.
///
/// The staged path above is what a running app uses. This is for a
/// caller holding a tree it built itself — a test, a dev-tools preview —
/// that wants to see one patch applied without staging it.
pub fn apply_to(element: &mut Element, patch: &Patch) -> Outcome {
    let key = patch.key();
    let mut outcome = Outcome::default();
    apply_by_tag(element, key, &patch.edits, &mut outcome);
    outcome
}

fn apply_by_tag(element: &mut Element, site: u64, edits: &[Edit], outcome: &mut Outcome) {
    match element {
        Element::Item { .. } => {
            let tag = match element {
                Element::Item { tag, .. } => *tag,
                _ => None,
            };
            if let Some(t) = tag.filter(|t| t.site == site) {
                let mine = edits_addressed_to(edits, t.node);
                if !mine.is_empty() {
                    let o = apply::apply(element, &mine);
                    outcome.applied += o.applied;
                    outcome.refused += o.refused;
                }
            }
            // Borrowed AFTER applying: `SetChildren` replaces the list,
            // and the new children are the ones to recurse into.
            if let Element::Item { children, .. } = element {
                for child in children.iter_mut() {
                    apply_by_tag(child, site, edits, outcome);
                }
            }
        }
        Element::Fragment(children) => {
            for child in children.iter_mut() {
                apply_by_tag(child, site, edits, outcome);
            }
        }
        Element::Owned { element, .. } => apply_by_tag(element, site, edits, outcome),
        _ => {}
    }
}

fn edits_addressed_to(edits: &[Edit], node: u32) -> Vec<Edit> {
    edits
        .iter()
        .filter(|e| match e {
            Edit::SetProp { node: n, .. } | Edit::SetChildren { node: n, .. } => *n == node,
        })
        .cloned()
        .collect()
}

/// Every tagged node in a built tree, each paired with its nearest
/// tagged ANCESTOR, in depth-first order.
///
/// Test support and dev-tools readout: it is how the parity suite
/// asserts that a site's nodes are addressable, and how a dev server
/// answers "what did this site actually build".
///
/// The ancestor is what makes the list checkable. A flat list of tags
/// cannot distinguish "the emission numbered these correctly" from "the
/// emission numbered these at random", because a tree legitimately
/// contains several sites (a `#[component]`'s own `ui!` is a second site
/// inside the first's tree) and legitimately repeats one (site, node)
/// pair (every row of a `for` builds the same node of the same site).
/// What is invariant is the ancestor relation: within one site, a node
/// is always numbered before everything under it.
pub fn tag_tree(element: &Element) -> Vec<TagEdge> {
    let mut out = Vec::new();
    walk_tags(element, None, &mut out);
    out
}

/// Every tag in a built tree, in depth-first order.
pub fn tags(element: &Element) -> Vec<runtime_scene::NodeTag> {
    tag_tree(element).into_iter().map(|(_, t)| t).collect()
}

/// A tagged node and its nearest tagged ancestor.
pub type TagEdge = (Option<runtime_scene::NodeTag>, runtime_scene::NodeTag);

fn walk_tags(element: &Element, parent: Option<runtime_scene::NodeTag>, out: &mut Vec<TagEdge>) {
    match element {
        Element::Item { children, tag, .. } => {
            let parent = match tag {
                Some(t) => {
                    out.push((parent, *t));
                    Some(*t)
                }
                None => parent,
            };
            for c in children {
                walk_tags(c, parent, out);
            }
        }
        Element::Fragment(children) => {
            for c in children {
                walk_tags(c, parent, out);
            }
        }
        Element::Owned { element, .. } => walk_tags(element, parent, out),
        _ => {}
    }
}
