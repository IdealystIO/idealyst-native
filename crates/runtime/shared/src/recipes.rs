//! Core-primitive usage **recipes** for the MCP catalog — static-data
//! registrations that survive both cores (idea-lite migration, wave 2b).
//!
//! ## Why these live here as text, not as compiled `recipe!` fns
//!
//! They used to be `recipe!(...)` invocations in `runtime_core::recipes`.
//! That anchoring had two fatal properties for the migration:
//!
//! 1. `recipe!` compiles its fn body, and the bodies use `ui!` — whose
//!    emission is decided **graph-wide** by `runtime-macros/new-core`
//!    (proc-macro feature unification). In a new-core graph the emission
//!    targets `runtime_vocabulary::glue`, which runtime-core doesn't
//!    (and must not) depend on — so `runtime-core/catalog` could not be
//!    enabled in `--new-core` dev sessions and the whole catalog
//!    inventory vanished from them.
//! 2. runtime-core is scheduled for deletion (P7); the recipe *data*
//!    must not die with it.
//!
//! So the recipe **descriptions** are static data here (runtime-shared
//! is the permanent substrate both cores link), with each `source`
//! pulled via `include_str!` from `crates/runtime/shared/recipes/*.rs`.
//! Those files are real Rust: they stay compile-checked against the
//! live author surface — on BOTH cores — by
//! `crates/dev/newcore-app/tests/recipes_compile.rs`, which `include!`s
//! them under the old core (default) and under the facade alias
//! (`--features new-core`). A prop change that breaks a recipe still
//! fails a test; it just fails in the dual-core gate crate instead of
//! inside runtime-core.
//!
//! The `docs` literals below mirror each file's leading `///` block
//! (the file keeps the comment so the served `source` reads complete).
//! Entries register into the same `mcp_catalog::RecipeEntry` inventory
//! slice `recipe!` uses, so `list_recipes` / `describe_recipe` /
//! `get_catalog` are unchanged.

// One `inventory::submit!` per recipe. `module_path!()` names this
// module (recipe resolution is proximity-based and these target core
// primitives, which resolve by name, so the exact module path is
// informational).
macro_rules! core_recipe {
    ($name:literal, $target:literal, $file:literal, $docs:literal, uses: [$($use_:literal),* $(,)?]) => {
        mcp_catalog::inventory::submit! {
            mcp_catalog::RecipeEntry {
                name: $name,
                target: $target,
                module_path: module_path!(),
                file: concat!("crates/runtime/shared/recipes/", $file),
                line: 1,
                docs: $docs,
                source: include_str!(concat!("../recipes/", $file)),
                uses: &[$($use_),*],
            }
        }
    };
}

core_recipe!(
    "input_with_submit",
    "text_input",
    "input_with_submit.rs",
    "A single-line input with Enter-to-submit and a button sharing the same\nhandler. The `value` signal is the input's source of truth; `on_change`\nwrites it back; `on_key_down` turns Enter into submit (returning\n`PreventDefault` so the platform's own Enter behaviour is suppressed).",
    uses: ["button", "text", "text_input", "view"]
);

core_recipe!(
    "keyed_list_add_remove",
    "ui",
    "keyed_list_add_remove.rs",
    "A reactive list with add and remove. The keyed `for` iterates the\n`Signal<Vec<T>>` ITSELF (not `.get()`): writing a new Vec back with\n`.set(..)` re-renders, and `key =` drives reconciliation so rows\nmove/reuse instead of rebuilding. Stable ids (not indexes) make\nremoval correct. The item type derives `PartialEq` — signal writes\nare equality-guarded, so the stored type must be comparable.",
    uses: ["button", "text", "view"]
);

core_recipe!(
    "animated_toast",
    "presence",
    "animated_toast.rs",
    "A `signal(false)`-driven toast that animates in and out with\n`presence` — the primitive that owns mount/unmount *timing* so an\nenter animation can play on appearance and an exit animation can\nfinish before the subtree is torn down (a plain `if`/`when` would\ntear it down instantly, with no window for the exit to play).\n\n`presence(child)` returns a builder; chain `.present(...)` (the\nopen/close predicate), `.enter(...)` and `.exit(...)`. On enter,\n`enter.state` is applied *before* the first paint then interpolated\nback to rest over `enter.duration_ms`; on exit, `exit.state` is\ninterpolated *toward* and the subtree is held mounted for\n`exit.duration_ms` before it drops. `PresenceState` carries only the\nfour universally-interpolatable properties (opacity + 2D translate +\nuniform scale) — here a fade-and-slide, mirrored both ways. Finish\nwith `.into_element()` and splat the result into the tree.",
    uses: ["button", "text", "view"]
);

core_recipe!(
    "confirm_dialog_overlay",
    "overlay",
    "confirm_dialog_overlay.rs",
    "A keyed list whose rows delete through a CONFIRM dialog rendered as a\nviewport `overlay`. A `signal(Option<u64>)` holds the id awaiting\nconfirmation; while it's `Some`, an `if let` INSIDE `ui!` (the\nstandard conditional — never a `Vec::push` before the macro) mounts a\ncentered confirm panel over a scrim.\n\n`overlay(placement = Center, backdrop = Dismiss, on_dismiss = …)`\ncenters the panel and renders a dismiss-on-tap backdrop: a tap on the\nscrim (or Escape / back) fires `on_dismiss`, which clears the pending\nid — the cancel path. The panel's own Confirm button both removes the\nrow and clears the id; Cancel just clears it. `Signal` is `Copy`, so\neach closure captures its own copy without a `.clone()`.",
    uses: ["button", "overlay", "text", "view"]
);

// ---------------------------------------------------------------------------
// Navigation recipes.
//
// These used to be `recipe!(…)` invocations inside the navigator SDKs
// (`swap-navigator/src/recipes.rs`, `stack-navigator/src/recipes.rs`),
// gated `#[cfg(all(feature = "catalog", not(feature = "new-core")))]` —
// so on the new core the framework served NO navigation recipe at all,
// which is a product-surface regression, not just a test gap (the arena
// nav-notes feedback, run-0, is exactly an agent reconstructing this
// skeleton by grepping source because `list_recipes` had none).
//
// They move here for the same two reasons the core-primitive recipes did
// (see the module docs): a `recipe!` body's `ui!` lowering is decided
// build-graph-wide, and the SDK crates carry an unconditional
// `runtime_core` dependency, so `::runtime_core::Element` inside an
// SDK-hosted recipe means the OLD Element even in a new-core graph — the
// body could not compile on both cores from in there. As static data
// with the source `include_str!`d, the served text is core-neutral
// (`::runtime_core::…` is whatever root the reader's app aliases) and the
// compile check happens where a core CAN be selected:
// `crates/dev/newcore-app/tests/recipes_compile.rs`, which builds both
// files on both legs against the real navigator SDKs.
//
// `uses` lists the primitives each body reaches for, matching the
// core-primitive entries' convention (the navigator itself is the
// `target`, not a "use").
// ---------------------------------------------------------------------------

core_recipe!(
    "swap_three_screens_tab_bar",
    "SwapNavigator",
    "swap_three_screens_tab_bar.rs",
    "A three-screen swap navigator with an author tab bar. `swap` has no\npush/pop depth — selecting a screen SWAPS the one visible screen — and\nthe \"tab bar\" is just ordinary author layout wrapped around the\nnavigator's single `{ nav.outlet }` (the analog of react-router's\n`<Outlet/>`). The `use swap_navigator::{…}` line matters: `SwapBuilder`\n(`.screen`/`.layout`/`.bind`) is an extension trait on the navigator\nbuilder — it must be in scope or the builder calls fail to resolve.\n\nIn the layout: splat `{ nav.outlet }` BARE (it ships its own\n`flex: 1 1 0; min-height: 0` fill rules; a styled wrapper replaces them\nand collapses grow-based screens) and keep the tab bar in a non-growing\nsibling slot so only the outlet grows. Each tab button calls\n`nav.on_select(\"<route name>\")` to switch; read `nav.active_route` to\nhighlight the live tab.",
    uses: ["button", "text", "view"]
);

core_recipe!(
    "stack_two_screens",
    "StackNavigator",
    "stack_two_screens.rs",
    "A list → detail stack: two screens, a typed `Route` param that\nround-trips through the URL (`/note/:slug`), and an author\n`.layout(...)` shell around the outlet. The `use stack_navigator::{…}`\nline matters: `StackBuilder` (`.screen`/`.layout`/`.bind`) and\n`StackScreenExt` (`.title`) are extension traits — both must be in\nscope or the builder calls fail to resolve. In the layout, splat\n`{ nav.outlet }` BARE (it ships its own `flex: 1 1 0; min-height: 0`\nfill rules; a styled wrapper replaces them and collapses grow-based\nscreens) and keep chrome in a non-growing sibling slot.",
    uses: ["button", "text", "view"]
);

#[cfg(test)]
mod tests {
    /// The four core recipes register into the catalog's recipe slice
    /// with source text and docs intact. Regression for the wave-2b
    /// re-anchoring: the catalog data must exist WITHOUT runtime-core
    /// (this crate's `catalog` feature alone links + populates it).
    #[test]
    fn core_recipes_register_into_the_catalog_slice() {
        let names: Vec<&str> = mcp_catalog::recipes().map(|r| r.name).collect();
        for expected in [
            "input_with_submit",
            "keyed_list_add_remove",
            "animated_toast",
            "confirm_dialog_overlay",
        ] {
            assert!(
                names.contains(&expected),
                "recipe {expected:?} missing from the inventory slice; got {names:?}"
            );
        }
        let toast = mcp_catalog::recipes()
            .find(|r| r.name == "animated_toast")
            .expect("animated_toast entry");
        assert_eq!(toast.target, "presence");
        assert!(toast.source.contains("fn animated_toast()"));
        assert!(toast.docs.contains("presence"));
        // The full catalog JSON (what `get_catalog` serves) carries them.
        let json = mcp_catalog::catalog_json();
        let recipes = json
            .get("recipes")
            .and_then(|r| r.as_array())
            .expect("catalog_json recipes slice");
        assert!(recipes
            .iter()
            .any(|r| r.get("name").and_then(|n| n.as_str()) == Some("confirm_dialog_overlay")));
    }

    /// The two navigation recipes register here rather than in the
    /// navigator SDKs, so they exist on BOTH cores (they used to be
    /// `cfg(not(new-core))` inside the SDKs — a served-content regression
    /// on the new core, not merely a test gap).
    ///
    /// The assertions on the served source are ported verbatim from the
    /// deleted `stack-navigator/tests/recipes.rs`: the load-bearing pieces
    /// an agent copy-pastes. Both extension-trait import lines are
    /// load-bearing — omitting `StackBuilder` breaks `.screen(...)`
    /// resolution, which is precisely what the arena feedback caught.
    #[test]
    fn navigation_recipes_register_with_their_import_lines() {
        let swap = mcp_catalog::recipes()
            .find(|r| r.name == "swap_three_screens_tab_bar")
            .expect("swap_three_screens_tab_bar recipe registered in the catalog");
        assert_eq!(swap.target, "SwapNavigator");
        for needle in [
            "use swap_navigator::",
            "SwapBuilder",
            "Route::<()>::new(\"search\", \"/search\")",
            "nav.outlet",
            "nav.on_select",
        ] {
            assert!(
                swap.source.contains(needle),
                "swap recipe source should contain `{needle}`; got:\n{}",
                swap.source,
            );
        }

        let stack = mcp_catalog::recipes()
            .find(|r| r.name == "stack_two_screens")
            .expect("stack_two_screens recipe registered in the catalog");
        assert_eq!(stack.target, "StackNavigator");
        for needle in [
            "use stack_navigator::",
            "StackBuilder",
            "StackScreenExt",
            "Route::<NoteId>::new(\"detail\", \"/note/:slug\")",
            "nav.outlet",
        ] {
            assert!(
                stack.source.contains(needle),
                "stack recipe source should contain `{needle}`; got:\n{}",
                stack.source,
            );
        }

        // `recipes_for` is what `describe_recipe`/`list_recipes` join on:
        // the navigator TYPE name must resolve to its recipe.
        let cat = mcp_catalog::ResolvedCatalog::build();
        assert!(
            cat.recipes_for("StackNavigator")
                .iter()
                .any(|r| r.name == "stack_two_screens"),
            "recipes_for(StackNavigator) must surface the stack recipe"
        );
        assert!(
            cat.recipes_for("SwapNavigator")
                .iter()
                .any(|r| r.name == "swap_three_screens_tab_bar"),
            "recipes_for(SwapNavigator) must surface the swap recipe"
        );
    }
}
