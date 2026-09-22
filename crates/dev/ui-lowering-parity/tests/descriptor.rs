//! The build-time descriptor and the compiled tags must agree.
//!
//! Two halves produce the overlay's addressing, and they never run
//! together in a real build:
//!
//! - `runtime-macros` expands a `ui!` body inside rustc and tags each
//!   `Element` with the node's number;
//! - `runtime-macros-parse::describe` reads the same body from a SOURCE
//!   FILE at build time and produces the descriptor a differ compares,
//!   whose node indices must mean the same thing.
//!
//! If they ever disagree by one node, every patch after that node edits
//! the wrong element — silently, because both sides are internally
//! consistent. Neither half can catch it alone. This suite is the only
//! place both run on the same input, which is what the fixture corpus's
//! `stringify!`-ed body exists for.
//!
//! The numbering comes from `runtime_macros_parse::number`, which stamps
//! the parsed tree once; `describe` re-walks and checks its own counter
//! against every stamp it meets. So a drift is caught twice: as a
//! `StampMismatch` inside the library, and here as a tag that lands on
//! the wrong kind of node.

#![cfg(feature = "ui-overlay")]

use runtime_macros_parse::{describe, Ui};
use runtime_template::{Node, SiteId};
use ui_lowering_parity::{fixtures, Mode};

fn descriptor_of(body: &str) -> runtime_template::Descriptor {
    let mut ui: Ui = syn::parse_str(body).expect("fixture body re-parses");
    describe(
        SiteId {
            package: "ui-lowering-parity".into(),
            file: "tests/descriptor.rs".into(),
            line: 1,
            col: 1,
        },
        &mut ui,
    )
    .expect("the stamping walk and the describing walk agree")
}

/// Every tag a fixture's OWN site produces must land on a descriptor
/// node that is element-shaped.
///
/// "Element-shaped" is `Prim`, `Component`, or an `Opaque` with no
/// defining expression — the last being a node carrying a trailing
/// `.method(…)` chain, which is still one built `Element`. An `Opaque`
/// WITH an expression is control flow (`if`, `for`, `match`, a bare
/// expression child): it occupies an index and never becomes a tagged
/// element, so a tag landing on one means the two walks have drifted.
///
/// That is what makes this a real check rather than a bounds check. A
/// tree of nothing but primitives would survive any consistent
/// off-by-one; a corpus with control flow interleaved between elements
/// does not.
#[test]
fn every_tag_lands_on_the_node_the_descriptor_gives_that_number() {
    let mut checked = 0;
    for fixture in fixtures::all() {
        let descriptor = descriptor_of(fixture.body);
        let tags = (fixture.direct)(Mode::Spliced).tags;

        // The fixture's own site is the one its ROOT tags carry — a tag
        // with no tagged ancestor. Deeper tags may belong to a
        // `#[component]`'s own `ui!`, which is a different site with its
        // own descriptor.
        let Some(site) = tags.iter().find(|(p, _)| p.is_none()).map(|(_, t)| t.site) else {
            continue;
        };

        for (_, tag) in tags.iter().filter(|(_, t)| t.site == site) {
            let node = descriptor.node(tag.node).unwrap_or_else(|| {
                panic!(
                    "{}: node {} is tagged but the descriptor has only {} nodes",
                    fixture.name,
                    tag.node,
                    descriptor.nodes.len()
                )
            });
            assert!(
                !matches!(node, Node::Opaque { expr: Some(_), .. }),
                "{}: node {} is tagged as a built element, but the descriptor calls it \
                 control flow — the two walks have drifted:\n{node:?}",
                fixture.name,
                tag.node,
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "no fixture produced a tag to check");
}

/// A fixture whose body contains control flow must actually produce
/// control-flow nodes in its descriptor.
///
/// Without this the test above could pass vacuously on a describe()
/// that called everything a `Prim`. It pins the other direction: the
/// corpus does exercise the interleaving that makes the agreement
/// non-trivial.
#[test]
fn the_corpus_interleaves_control_flow_with_elements() {
    let with_control_flow = fixtures::all()
        .iter()
        .filter(|f| {
            descriptor_of(f.body)
                .nodes
                .iter()
                .any(|n| matches!(n, Node::Opaque { expr: Some(_), .. }))
        })
        .count();
    assert!(
        with_control_flow >= 10,
        "only {with_control_flow} fixtures describe any control flow; the agreement \
         test above would be close to vacuous"
    );
}

/// Every descriptor the corpus produces must be internally consistent —
/// every child and root index in range, every slot reference declared.
///
/// `describe` builds these indices itself, so this is where its own
/// arithmetic is checked, on 49 real trees rather than hand-written
/// ones.
#[test]
fn every_descriptor_the_corpus_produces_is_well_formed() {
    for fixture in fixtures::all() {
        let descriptor = descriptor_of(fixture.body);
        assert_eq!(
            runtime_template::check_well_formed(&descriptor),
            Ok(()),
            "{}: malformed descriptor",
            fixture.name
        );
    }
}
