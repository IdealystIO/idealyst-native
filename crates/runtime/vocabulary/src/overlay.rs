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

use runtime_scene::Element;

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
    runtime_scene::with_tag(
        crate::glue::IntoElement::into_element(element),
        runtime_scene::NodeTag { site, node },
    )
}

/// Every tagged node in a built tree, each paired with its nearest
/// tagged ANCESTOR, in depth-first order.
///
/// Test support and dev-tools readout: it is how the parity suite
/// asserts that a site's nodes are addressable, and how a dev server
/// answers "what did this site actually build".
///
/// The ancestor is what makes the list checkable. A flat list of tags
/// cannot distinguish "the emission numbered these correctly" from
/// "the emission numbered these at random", because a tree legitimately
/// contains several sites (a `#[component]`'s own `ui!` is a second
/// site inside the first's tree) and legitimately repeats one (site,
/// node) pair (every row of a `for` builds the same node of the same
/// site). What is invariant is the ancestor relation: within one site, a
/// node is always numbered before everything under it.
pub fn tag_tree(element: &Element) -> Vec<(Option<runtime_scene::NodeTag>, runtime_scene::NodeTag)> {
    let mut out = Vec::new();
    walk_tags(element, None, &mut out);
    out
}

/// Every tag in a built tree, in depth-first order.
pub fn tags(element: &Element) -> Vec<runtime_scene::NodeTag> {
    tag_tree(element).into_iter().map(|(_, t)| t).collect()
}

type TagEdge = (Option<runtime_scene::NodeTag>, runtime_scene::NodeTag);

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
