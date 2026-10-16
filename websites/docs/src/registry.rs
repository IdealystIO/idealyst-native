//! Static registry of every documentation page.
//!
//! Hand-maintained: adding a new page is one line here plus the
//! page module. The MCP server (and any future text exporter or
//! search indexer) walks this list to find pages.
//!
//! The convention: each page module exports `pub static PAGE_META:
//! PageMeta` (emitted by the `docs!` macro) and `pub fn page() ->
//! Element` (also emitted by `docs!`). Routes wire `page()` into
//! the navigator; this registry wires `PAGE_META` into MCP queries.

use crate::meta::{DocConcept, PageCategory, PageMeta};

/// Every page in the docs, in display order.
///
/// **Cookbook recipes belong here too** — [`non_cookbook`] /
/// [`cookbook`] split them by `PageCategory::Cookbook`, so the same
/// registry powers both the regular page index and the cookbook
/// surface.
pub static PAGES: &[&'static PageMeta] = &[
    &crate::pages::introduction::PAGE_META,
    &crate::pages::overview::PAGE_META,
    &crate::pages::quickstart::PAGE_META,
    &crate::pages::primitives::PAGE_META,
    &crate::pages::reactivity::PAGE_META,
    &crate::pages::reactive_text_bindings::PAGE_META,
    &crate::pages::async_reactivity::PAGE_META,
    &crate::pages::server_functions::PAGE_META,
    &crate::pages::styles::PAGE_META,
    &crate::pages::animation::PAGE_META,
    &crate::pages::components::PAGE_META,
    &crate::pages::robot::PAGE_META,
    &crate::pages::backends::PAGE_META,
    &crate::pages::dev_tools::PAGE_META,
    &crate::pages::navigation::PAGE_META,
    &crate::pages::lists::PAGE_META,
    &crate::pages::icons::PAGE_META,
    &crate::pages::refs::PAGE_META,
    &crate::pages::net::PAGE_META,
    &crate::pages::portal::PAGE_META,
    &crate::pages::writing_a_backend::PAGE_META,
    &crate::pages::third_party_primitives::PAGE_META,
    &crate::pages::wgpu_native_api::PAGE_META,
    &crate::pages::building_a_theme_system::PAGE_META,
];

/// Find a page by slug. Returns `None` if no page with that slug is
/// registered.
pub fn find(slug: &str) -> Option<&'static PageMeta> {
    PAGES.iter().copied().find(|p| p.slug == slug)
}

/// Pages in a given category, in registration order.
pub fn by_category(cat: PageCategory) -> impl Iterator<Item = &'static PageMeta> {
    PAGES.iter().copied().filter(move |p| p.category == cat)
}

/// Pages outside the Cookbook category — the default "what
/// documentation exists" surface.
pub fn non_cookbook() -> impl Iterator<Item = &'static PageMeta> {
    PAGES
        .iter()
        .copied()
        .filter(|p| p.category != PageCategory::Cookbook)
}

/// Cookbook recipes only.
pub fn cookbook() -> impl Iterator<Item = &'static PageMeta> {
    PAGES
        .iter()
        .copied()
        .filter(|p| p.category == PageCategory::Cookbook)
}

/// Pages where the given concept appears in `concepts` — the
/// authoritative explainer(s) for that concept (the reverse index).
pub fn pages_about(concept: DocConcept) -> impl Iterator<Item = &'static PageMeta> {
    PAGES
        .iter()
        .copied()
        .filter(move |p| p.concepts.contains(&concept))
}

/// Union of every page's `concepts` list, deduplicated. The full
/// vocabulary the framework documents.
pub fn all_concepts() -> Vec<DocConcept> {
    let mut seen = Vec::new();
    for page in PAGES {
        for &c in page.concepts {
            if !seen.contains(&c) {
                seen.push(c);
            }
        }
    }
    seen
}
