//! Catalog-emission suite — the whole catalog contract, run against the
//! crate that defines it.
//!
//! One compilation exercises BOTH `__mcp` inventory anchors and proves
//! they land in the same catalog:
//!
//! - `#[component]` → `::runtime_vocabulary::glue::__mcp` (its emission
//!   passes through `runtime_macros::finish`, which rewrites absolute
//!   `::runtime_core::` path heads);
//! - `#[derive(IdealystSchema)]` / `#[idealyst_tool]` / `recipe!` /
//!   `doc_scope!` → `::runtime_core::__mcp`, resolved through the alias
//!   below (those entry points do NOT go through `finish`).
//!
//! Both anchors re-export this crate, so exactly one inventory must
//! result — which is what [`catalog_inventory_is_identical_across_cores`]
//! pins, via a sorted fingerprint of every macro-emitted slice whose
//! expected value is a literal in the suite source.
//!
//! The test-target name is load-bearing: `module_path!()` for an
//! integration test is the *target* name, and the fingerprint compares
//! module paths — so this file must stay `registers_component.rs`.
//!
//! Hosting: the suite body still lives at
//! `crates/dev/newcore-catalog/tests/shared/catalog_emission.rs`, which
//! is where it was parked while the pre-v2 core still owned the other
//! anchor and mcp-catalog could not name the facade (a dev-dep could not
//! be made optional, and an unconditional one would have flipped the
//! macro lowering for the dying leg too). With one core that constraint
//! is gone — mcp-catalog takes the facade as a dev-dep directly, cycle
//! and all (cargo permits cycles through dev-dependencies). The body
//! should be folded back in here and `crates/dev/newcore-catalog`
//! removed; until then this `include!` is the single source.
//!
//! Invocation: `cargo test -p mcp-catalog`.

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../dev/newcore-catalog/tests/shared/catalog_emission.rs"
));

/// Reading the catalog document back with `CatalogParts::from_json` and
/// writing it again reproduces it byte for byte. The CLI's catalog
/// scanner depends on this: it re-serializes a dependency extractor's
/// whole document through `CatalogParts` when it merges the scanned
/// workspace entries in, so any field the reader drops or the writer
/// derives differently would silently change the catalog.
#[test]
fn catalog_parts_json_round_trip_is_exact() {
    let original = mcp_catalog::catalog_json();
    assert!(
        !original["components"].as_array().unwrap().is_empty(),
        "the suite registers components; an empty catalog proves nothing"
    );
    let parts = mcp_catalog::CatalogParts::from_json(&original).expect("read the document back");
    assert_eq!(
        serde_json::to_string_pretty(&parts.to_json()).unwrap(),
        serde_json::to_string_pretty(&original).unwrap()
    );
}

/// The CLI's catalog scanner reads entries from source by running the
/// catalog macros' own expansion; this pins that it reads exactly what a
/// compiled extractor registers. The suite's fixture is both: compiled
/// into this binary (its entries are in the inventory under this test
/// target's name) and handed to the scanner as a crate root under the
/// same name. Every macro-emitted slice must come out identical — names,
/// params, schemas, composes edges and their lines, `file!()`, `line!()`,
/// recipe source — in the catalog document both are written to.
#[test]
fn source_scan_reads_exactly_what_the_compiled_macros_register() {
    use mcp_catalog::{origin_crate, CatalogParts};

    const CRATE: &str = "registers_component";
    // The path rustc reports through `file!()` for the `include!`d
    // fixture: the include's argument as written, `..` and all.
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/../../dev/newcore-catalog/tests/shared/catalog_emission.rs");

    let ours = |m: &str| origin_crate(m) == CRATE;
    let all = CatalogParts::registered();
    let compiled = CatalogParts {
        components: all.components.iter().copied().filter(|e| ours(e.module_path)).collect(),
        props_schemas: all.props_schemas.iter().copied().filter(|e| ours(e.module_path)).collect(),
        methods: all.methods.iter().copied().filter(|e| ours(e.parent_module_path)).collect(),
        animations: all.animations.iter().copied().filter(|e| ours(e.parent_module_path)).collect(),
        types: all.types.iter().copied().filter(|e| ours(e.module_path)).collect(),
        values: all.values.iter().copied().filter(|e| ours(e.module_path)).collect(),
        tools: all.tools.iter().copied().filter(|e| ours(e.module_path)).collect(),
        recipes: all.recipes.iter().copied().filter(|e| ours(e.module_path)).collect(),
        scopes: all.scopes.iter().copied().filter(|e| ours(e.module_path)).collect(),
        ..Default::default()
    };
    // Every slice the scan reads must actually be exercised here, or the
    // comparison below proves nothing about it.
    for (slice, n) in [
        ("components", compiled.components.len()),
        ("props_schemas", compiled.props_schemas.len()),
        ("methods", compiled.methods.len()),
        ("animations", compiled.animations.len()),
        ("types", compiled.types.len()),
        ("tools", compiled.tools.len()),
        ("recipes", compiled.recipes.len()),
        ("scopes", compiled.scopes.len()),
    ] {
        assert!(n > 0, "the fixture registers no {slice}");
    }

    let cfg = catalog_scan::Cfg::host_in(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))).expect("rustc --print cfg");
    let krate = catalog_scan::ScanCrate { name: CRATE.into(), root: fixture.into(), cfg };
    let scanned = catalog_scan::scan(&[krate], &[]);
    assert!(scanned.refused.is_empty() && scanned.skipped.is_empty(), "{:?} {:?}", scanned.refused, scanned.skipped);
    let scanned = scanned.parts;

    let pretty = |p: &CatalogParts| serde_json::to_string_pretty(&p.to_json()).unwrap();
    let (compiled, scanned) = (pretty(&compiled), pretty(&scanned));
    if compiled != scanned {
        let first = compiled.lines().zip(scanned.lines()).position(|(a, b)| a != b).unwrap_or(0);
        let window = |s: &str| s.lines().skip(first.saturating_sub(8)).take(16).collect::<Vec<_>>().join("\n");
        panic!(
            "scan differs from the compiled registrations at line {first}\n--- compiled ---\n{}\n--- scanned ---\n{}",
            window(&compiled),
            window(&scanned)
        );
    }
}
