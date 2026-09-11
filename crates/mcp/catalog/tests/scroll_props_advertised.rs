//! The scroll family's behavioural props are in the table.
//!
//! `scroll_view` and the virtualizer used to advertise their children /
//! items plus the three common fields and nothing else: no `horizontal`,
//! no `on_scroll`, no `on_end_reached`. An agent working from the MCP
//! could not learn that a scroller reports its end — and one commit
//! (47d014a2) exists because an author could not find the setter and
//! reached for a spelling that did nothing. The drift audit checks that
//! an ENTRY exists per primitive, not that its props are complete, which
//! is how this stayed invisible. These pin the props that matter most.

use mcp_catalog::lookup_primitive;

fn prop_names(prim: &str) -> Vec<&'static str> {
    lookup_primitive(prim)
        .unwrap_or_else(|| panic!("{prim} has a catalog entry"))
        .props
        .iter()
        .map(|p| p.name)
        .collect()
}

#[test]
fn scroll_view_advertises_its_behavioural_props() {
    let names = prop_names("scroll_view");
    for want in [
        "horizontal",
        "on_scroll",
        "on_end_reached",
        "end_reached_threshold",
        "bounces",
        "always_bounce",
        "safe_area",
    ] {
        assert!(names.contains(&want), "scroll_view is missing `{want}`; has {names:?}");
    }
}

#[test]
fn virtualizer_advertises_paging_and_layout_props() {
    let names = prop_names("virtualizer");
    for want in [
        "data",
        "render",
        "key",
        "size",
        "axis",
        "lanes",
        "gap",
        "overscan",
        "safe_area",
        "on_scroll",
        "on_end_reached",
        "end_reached_threshold",
    ] {
        assert!(names.contains(&want), "virtualizer is missing `{want}`; has {names:?}");
    }
}

/// `on_end_reached` is not answered everywhere, and the entry has to say
/// so where an agent will read it — a list that only grows this way
/// stops growing on a backend that ignores it.
#[test]
fn on_end_reached_names_the_backends_that_answer() {
    for prim in ["scroll_view", "virtualizer"] {
        let p = lookup_primitive(prim).unwrap();
        let f = p.props.iter().find(|f| f.name == "on_end_reached").unwrap();
        for b in ["ios", "web", "macos"] {
            assert!(f.constraint.contains(b), "{prim}: constraint should name `{b}`: {}", f.constraint);
        }
        assert!(f.doc.contains("Android"), "{prim}: the doc should say Android is a no-op: {}", f.doc);
    }
}
