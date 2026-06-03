//! Build-time catalog extraction.
//!
//! The docs site renders the **framework's** catalog. If it
//! self-introspected its own wasm `inventory` at runtime, wasm DCE would
//! prune every `#[component]` / table ctor the app doesn't transitively
//! reference — the page would show only ~half the components and no
//! primitives/utilities/guides (anything `app()` doesn't touch gets
//! stripped during the wasm size pass).
//!
//! So we extract the catalog HERE instead. `build.rs` is always
//! host-compiled, where `inventory`'s `#[used]` ctors survive and run, so
//! `catalog_json()` sees the FULL catalog. We serialize it to
//! `OUT_DIR/catalog.json`, which `src/catalog.rs` embeds via
//! `include_str!` and loads with `ResolvedCatalog::build_from_json` —
//! DCE-proof and identical on every target.
//!
//! Note this deliberately does NOT include catalog-docs's *own* chrome
//! components (`app`, `Section`, `CodePanel`, …) — build.rs can't link
//! the crate it builds — which is correct: those are docs-app internals,
//! not framework API.

// Force-link the catalog-bearing crates so their `inventory::submit!`
// ctors are present in this build-script binary. `extern crate ... as _`
// is the link-for-side-effects idiom — the same mechanism the scaffolded
// `catalog` bin uses via `use <lib> as _`.
extern crate idea_ui as _;
extern crate drawer_navigator as _;
extern crate table as _;
extern crate codeblock as _;

fn main() {
    let cat = mcp_catalog::catalog_json();
    let component_count = cat["components"].as_array().map(|a| a.len()).unwrap_or(0);
    // A near-empty catalog means the inventory ctors didn't link — surface
    // it loudly rather than shipping an empty-looking docs site.
    if component_count < 20 {
        println!(
            "cargo:warning=catalog-docs: only {component_count} components extracted into \
             catalog.json — inventory ctors may not be linking; the docs site will look sparse"
        );
    }

    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    std::fs::write(
        out.join("catalog.json"),
        serde_json::to_string(&cat).expect("serialize catalog"),
    )
    .expect("write catalog.json");

    println!("cargo:rerun-if-changed=build.rs");
}
