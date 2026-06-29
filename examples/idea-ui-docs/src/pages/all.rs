//! "All Components" — every catalog entry on one page.
//!
//! This is the cross-platform render-**parity fixture**: each component's demo
//! is wrapped in a `test_id`-anchored `view` whose id is the component's route
//! name (`button`, `checkbox`, …). Those stable anchors let
//! `native_parity::align` line every component up exactly across web and macОS,
//! so a single capture of `/all` covers the whole library at once. See
//! `tests/parity.rs` + `idealyst test --parity web,macos examples/idea-ui-docs`.
//!
//! The page is data-driven: it iterates [`crate::routes::CATALOG`] and reuses
//! each entry's existing `body` builder, so it stays complete automatically as
//! components are added — no per-component maintenance here.

use runtime_core::{ui, Element};
use idea_ui::{Stack, StackGap};

use crate::pages::body;
use crate::routes::{Entry, ALL_ROUTE, CATALOG};
use crate::shell::H3;

/// The `test_id` the parity harness scopes to — wraps the whole page body so
/// the comparison covers the content and excludes the (per-platform) navigator
/// chrome. Pass this as `root` to `robot_test::parity::compare` / the MCP
/// `compare_native_parity` tool.
pub const PARITY_ROOT: &str = "all-components";

/// Every catalog component, each in a `test_id`-anchored section, all under one
/// content anchor ([`PARITY_ROOT`]).
pub fn all() -> Element {
    let sections: Vec<Element> = CATALOG
        .iter()
        .flat_map(|g| g.entries.iter().map(move |e| (g.label, e)))
        // Skip THIS page so it doesn't render itself recursively.
        .filter(|(_, e)| e.route.name() != ALL_ROUTE.name())
        .map(section)
        .collect();
    let content = body(sections);
    ui! {
        view(test_id = PARITY_ROOT) {
            content
        }
    }
}

/// One component's demo under a `test_id`-anchored heading.
///
/// A plain file-local helper, NOT a `#[component]`: the parity anchor must be a
/// `&'static str` `test_id`, and passing a `&'static str` through a component
/// prop drops the value (the dispatch only carries owned types like `String`,
/// so the anchor would default to `""` and register no id). A local binding —
/// the same shape `examples/container-demo` uses — registers correctly.
fn section((group, e): (&'static str, &'static Entry)) -> Element {
    let anchor: &'static str = e.route.name();
    let label = format!("{group} · {}", e.name);
    let demo = (e.body)();
    ui! {
        view(test_id = anchor) {
            Stack(gap = StackGap::Md) {
                H3(content = label)
                demo
            }
        }
    }
}
