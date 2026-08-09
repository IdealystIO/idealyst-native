//! The author-facing spelling of the screen-state surface.
//!
//! `crates/mcp/catalog/guides/navigation.md` tells authors to write
//! `use runtime_core::{QueryParams, ScreenState, screen_state};`. That path
//! reaches the definitions in `runtime-shared` through two hops
//! (`glue` → `runtime_core::*`), either of which can silently drop a name
//! during a re-export edit — the docs would keep claiming a path that no
//! longer resolves. This test fails to COMPILE if that happens.

use runtime_core::{screen_query, screen_state, QueryParams, ScreenState};

#[derive(Debug, Default, PartialEq)]
struct Filters {
    tab: String,
    page: u32,
}

impl ScreenState for Filters {
    fn to_query(&self) -> QueryParams {
        QueryParams::new()
            .with("tab", self.tab.clone())
            .with("page", self.page.to_string())
    }
    fn from_query(q: &QueryParams) -> Option<Self> {
        Some(Filters {
            tab: q.get("tab").unwrap_or("all").to_string(),
            page: q.get_as("page").unwrap_or(1),
        })
    }
}

#[test]
fn documented_author_paths_resolve_and_round_trip() {
    let original = Filters { tab: "starred".into(), page: 2 };
    let encoded = original.to_query();
    assert_eq!(encoded.to_query_string(), "tab=starred&page=2");
    assert_eq!(Filters::from_query(&encoded), Some(original));

    // Reachable outside a screen build: empty, never a panic.
    assert!(screen_query().is_empty());
    assert_eq!(
        screen_state::<Filters>(),
        Some(Filters { tab: "all".into(), page: 1 }),
        "outside a screen build the defaults decode from an empty query"
    );
}
