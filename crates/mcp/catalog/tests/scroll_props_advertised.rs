//! The scroll family's behavioural props are in the table.
//!
//! Regression: `scroll_view` and the virtualizer advertised only their
//! children / items plus the three common fields — no `horizontal`, no
//! `on_scroll`, no `on_end_reached` — so an agent working from the MCP
//! could not learn that a scroller reports its end. The drift audit checks
//! that an ENTRY exists per primitive, not that its props are complete, so
//! these pin the props that matter most.

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

/// The entry is found by the tag an author writes. It was catalogued
/// under the internal name `virtualizer`, so `describe_primitive
/// flat_list` answered "not found".
#[test]
fn regression_flat_list_is_found_by_its_tag() {
    assert!(lookup_primitive("flat_list").is_some());
    assert!(lookup_primitive("FlatList").is_some());
}

#[test]
fn flat_list_advertises_paging_and_layout_props() {
    let names = prop_names("flat_list");
    for want in [
        "data",
        "render",
        "key",
        "size",
        "axis",
        "lanes",
        "gap",
        "main_spacing",
        "cross_spacing",
        "overscan",
        "safe_area",
        "on_scroll",
        "on_end_reached",
        "end_reached_threshold",
    ] {
        assert!(names.contains(&want), "flat_list is missing `{want}`; has {names:?}");
    }
}

/// `on_end_reached` is not answered everywhere, and the entry has to say
/// so where an agent will read it — a list that only grows this way
/// stops growing on a backend that ignores it.
#[test]
fn on_end_reached_names_the_backends_that_answer() {
    for prim in ["scroll_view", "flat_list"] {
        let p = lookup_primitive(prim).unwrap();
        let f = p.props.iter().find(|f| f.name == "on_end_reached").unwrap();
        for b in ["ios", "web", "macos"] {
            assert!(f.constraint.contains(b), "{prim}: constraint should name `{b}`: {}", f.constraint);
        }
        assert!(f.doc.contains("Android"), "{prim}: the doc should say Android is a no-op: {}", f.doc);
    }
}
