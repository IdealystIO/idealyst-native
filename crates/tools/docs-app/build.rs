//! Build-time catalog extraction.
//!
//! The docs site renders a **catalog**. If it self-introspected its own
//! wasm `inventory` at runtime, wasm DCE would prune every `#[component]`
//! / table ctor the app doesn't transitively reference — the page would
//! show only ~half the components and no primitives/utilities/guides
//! (anything `app()` doesn't touch gets stripped during the wasm size
//! pass).
//!
//! So we extract the catalog HERE instead. `build.rs` is always
//! host-compiled, where `inventory`'s `#[used]` ctors survive and run, so
//! `catalog_json()` sees the FULL catalog. We serialize it to
//! `OUT_DIR/catalog.json`, which `src/catalog.rs` embeds via
//! `include_str!` and loads with `ResolvedCatalog::build_from_json` —
//! DCE-proof and identical on every target.
//!
//! ## Two catalog sources
//!
//! - **Default (no env):** extract the FULL framework catalog natively
//!   from the crates force-linked below.
//! - **`IDEALYST_DOCS_CATALOG=<path>`:** embed that file verbatim instead.
//!   The `idealyst docs <project>` command extracts an arbitrary project's
//!   catalog (via the same ephemeral `catalog` bin `idealyst mcp` runs) and
//!   points us here, so one renderer crate can document any project.
//!
//! Note this deliberately does NOT include docs-app's *own* chrome
//! components (`app`, `Section`, `CodePanel`, …) — build.rs can't link
//! the crate it builds — which is correct: those are docs-app internals,
//! not framework API.

// Force-link the catalog-bearing crates so their `inventory::submit!`
// ctors are present in this build-script binary. `extern crate ... as _`
// is the link-for-side-effects idiom — the same mechanism the scaffolded
// `catalog` bin uses via `use <lib> as _`.
extern crate idea_ui as _;
extern crate swap_navigator as _;
extern crate idea_ui_nav as _;
extern crate table as _;
extern crate codeblock as _;
// icons-lucide (catalog feature) self-registers its `IconSetEntry`; link
// it so the pack appears in catalog.json's `icon_sets` slice.
extern crate icons_lucide as _;

// The live-preview codegen — a plain module shared with the crate's unit
// tests (see its docs).
#[path = "src/preview_codegen.rs"]
mod preview_codegen;
// Each component's source for "View source" (see its docs).
#[path = "src/source_extract.rs"]
mod source_extract;

fn main() {
    // Re-run when the injected-catalog override changes (path or contents),
    // so `idealyst docs <project>` re-embeds a freshly extracted catalog.
    println!("cargo:rerun-if-env-changed=IDEALYST_DOCS_CATALOG");

    // Catalog source — native framework extraction by default, or an
    // externally-injected JSON file when `IDEALYST_DOCS_CATALOG` names one
    // (see the module note above). The injected JSON is the exact shape
    // `mcp_catalog::catalog_json()` produces, so the recipe-codegen pass
    // below works identically on either source.
    let (cat, injected): (serde_json::Value, bool) = match std::env::var_os("IDEALYST_DOCS_CATALOG") {
        Some(path) if !path.is_empty() => {
            let path = std::path::PathBuf::from(path);
            println!("cargo:rerun-if-changed={}", path.display());
            let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
                panic!(
                    "IDEALYST_DOCS_CATALOG points at {} which could not be read: {e}",
                    path.display()
                )
            });
            let v = serde_json::from_str(&text).unwrap_or_else(|e| {
                panic!(
                    "IDEALYST_DOCS_CATALOG file {} is not valid catalog JSON: {e}",
                    path.display()
                )
            });
            (v, true)
        }
        _ => (mcp_catalog::catalog_json(), false),
    };

    let component_count = cat["components"].as_array().map(|a| a.len()).unwrap_or(0);
    let primitive_count = cat["primitives"].as_array().map(|a| a.len()).unwrap_or(0);
    if injected {
        // An injected project catalog is legitimately small (it documents
        // only that project's surface), so a low component count is NOT a
        // bug. Only a catalog with no entries at all signals a broken
        // extraction worth surfacing.
        if component_count == 0 && primitive_count == 0 {
            println!(
                "cargo:warning=docs-app: the injected catalog (IDEALYST_DOCS_CATALOG) is empty \
                 — the project's catalog extraction may have failed; the docs site will be bare"
            );
        }
    } else if component_count < 20 {
        // Native framework extraction: <20 components means the inventory
        // `#[used]` ctors didn't link — surface it loudly rather than
        // shipping an empty-looking framework docs site.
        println!(
            "cargo:warning=docs-app: only {component_count} components extracted natively into \
             catalog.json — inventory ctors may not be linking; the docs site will look sparse"
        );
    }

    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    std::fs::write(
        out.join("catalog.json"),
        serde_json::to_string(&cat).expect("serialize catalog"),
    )
    .expect("write catalog.json");

    // The live-preview lookups: an `extern` block over every preview and
    // recipe symbol the catalog records, plus symbol → fn maps. See
    // `src/preview_codegen.rs`.
    std::fs::write(out.join("previews.rs"), preview_codegen::previews_source(&cat)).expect("write previews.rs");
    println!("cargo:rerun-if-changed=src/preview_codegen.rs");

    // Component sources for "View source": `module_path::Name` → the fn
    // as written. Read from the files the catalog names; one that can't be
    // read (a catalog extracted elsewhere) just has no source.
    let sources = source_extract::component_sources(&cat);
    std::fs::write(out.join("sources.json"), serde_json::to_string(&sources).expect("serialize sources"))
        .expect("write sources.json");
    for file in cat["components"].as_array().into_iter().flatten().filter_map(|c| c["file"].as_str()) {
        println!("cargo:rerun-if-changed={file}");
    }
    println!("cargo:rerun-if-changed=src/source_extract.rs");

    println!("cargo:rerun-if-changed=build.rs");
}
