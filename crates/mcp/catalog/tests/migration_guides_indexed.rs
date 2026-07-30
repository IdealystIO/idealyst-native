//! The migration index and the migration guides must agree.
//!
//! `guides/migrations.md` is the index an agent lands on when it asks
//! "how do I upgrade" — a migration guide that exists on disk but is
//! missing from that table is unreachable in practice (the `[[slug]]`
//! convention is resolved by the client, so nothing else points at it),
//! and a table row naming a slug that does not exist is a dead link the
//! client cannot resolve. This suite pins both directions.
//!
//! It caught the real drift it was written for: `migration-0-5-0-to-1-0-0`
//! shipped in `docs/` for the 1.0 release but was never added to the
//! catalog, so `list_guides` stopped at 0.4 → 0.5.
//!
//! Invocation: `cargo test -p mcp-catalog`.

/// Slugs referenced from the index's `## Guides` table via the
/// `[[slug]]` convention, in file order. Scoped to that one section on
/// purpose — the authoring template further down the page carries a
/// literal `[[link]]` placeholder that is not a slug.
fn indexed_slugs() -> Vec<String> {
    let index =
        mcp_catalog::lookup_guide("migrations").expect("migrations index guide is registered");
    let table_start = index
        .body
        .find("## Guides")
        .expect("migrations.md has a `## Guides` section");
    let table = &index.body[table_start..];
    let table_end = table[1..]
        .find("\n## ")
        .map(|i| i + 1)
        .unwrap_or(table.len());

    let mut out = Vec::new();
    let mut rest = &table[..table_end];
    while let Some(open) = rest.find("[[") {
        rest = &rest[open + 2..];
        let close = rest.find("]]").expect("unterminated [[link]] in migrations.md");
        out.push(rest[..close].to_string());
        rest = &rest[close + 2..];
    }
    out
}

#[test]
fn migration_index_links_resolve_to_real_guides() {
    for slug in indexed_slugs() {
        assert!(
            mcp_catalog::lookup_guide(&slug).is_some(),
            "guides/migrations.md links [[{slug}]], but no guide with that slug is registered — \
             `read_guide` would fail on it"
        );
    }
}

#[test]
fn every_migration_guide_is_listed_in_the_index() {
    let indexed = indexed_slugs();
    for g in mcp_catalog::guides() {
        // The index itself carries the `migration` tag; it does not list
        // itself.
        if g.slug == "migrations" || !g.tags.contains(&"migration") {
            continue;
        }
        assert!(
            indexed.iter().any(|s| s == g.slug),
            "guide `{}` is tagged `migration` but is missing from the guides table in \
             guides/migrations.md — add a `[[{}]]` row",
            g.slug,
            g.slug
        );
    }
}
