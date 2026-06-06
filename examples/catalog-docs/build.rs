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
// icons-lucide (catalog feature) self-registers its `IconSetEntry`; link
// it so the pack appears in catalog.json's `icon_sets` slice.
extern crate icons_lucide as _;

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

    // Codegen the live-recipe-render map. The catalog only carries each
    // recipe's *source text*, not a callable — and the runtime inventory
    // thunks are DCE-pruned on wasm. The single way to render a recipe's
    // component on web is an explicit, statically-linked fn reference. So
    // here, on the host, we emit a `recipe_renderer(module_path, name)`
    // that matches the recipes we CAN address and returns their fn
    // pointer; the app `include!`s it.
    //
    // We only emit arms for recipes that are (a) defined in `idea_ui`
    // (the only crate whose recipe fns we made `pub` + whose path this
    // app links) and (b) zero-arg — a recipe whose wrapper fn takes
    // params is defining props, not a standalone renderable, so it's
    // excluded (returns `None` → no live preview, source still shown).
    // Overlay components mount through a portal to a document-level root,
    // so a rendered overlay escapes ANY container — it can't be held
    // inside the inline preview box; an "open" one (e.g. `modal_confirm`)
    // would cover the whole docs page with a stuck backdrop. So a recipe
    // that builds an overlay gets source-only (no live preview). Keyed off
    // the recipe's `uses` (the components its `ui!` body references).
    const OVERLAY_COMPONENTS: &[&str] =
        &["Modal", "Menu", "SubMenu", "Popover", "Tooltip", "ToastHost"];

    let mut arms = String::new();
    if let Some(recipes) = cat["recipes"].as_array() {
        for r in recipes {
            let (Some(name), Some(module_path), Some(source)) = (
                r["name"].as_str(),
                r["module_path"].as_str(),
                r["source"].as_str(),
            ) else {
                continue;
            };
            // Only crates this app links and whose recipe fns are `pub`.
            if !module_path.starts_with("idea_ui") {
                continue;
            }
            // Skip portal/overlay recipes — they can't be previewed inline.
            let uses_overlay = r["uses"]
                .as_array()
                .is_some_and(|us| us.iter().any(|u| OVERLAY_COMPONENTS.contains(&u.as_str().unwrap_or_default())));
            if uses_overlay {
                continue;
            }
            // Zero-arg only: the formatted source renders an empty arg
            // list as `fn <name>()`. A recipe with params reads
            // `fn <name>(arg: …)`, so the empty-parens check excludes it.
            if !source.contains(&format!("fn {name}()")) {
                continue;
            }
            arms.push_str(&format!(
                "        ({module_path:?}, {name:?}) => \
                 Some({module_path}::{name} as fn() -> ::runtime_core::Element),\n"
            ));
        }
    }
    let renderers = format!(
        "// @generated by build.rs — recipe live-render map. Do not edit.\n\
         #[allow(clippy::match_single_binding, unused)]\n\
         pub fn recipe_renderer(\n\
         \x20   module_path: &str,\n\
         \x20   name: &str,\n\
         ) -> Option<fn() -> ::runtime_core::Element> {{\n\
         \x20   match (module_path, name) {{\n\
         {arms}\
         \x20       _ => None,\n\
         \x20   }}\n\
         }}\n"
    );
    std::fs::write(out.join("recipe_renderers.rs"), renderers).expect("write recipe_renderers.rs");

    println!("cargo:rerun-if-changed=build.rs");
}
