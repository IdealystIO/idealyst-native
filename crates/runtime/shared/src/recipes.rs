//! Core-primitive usage **recipes** for the MCP catalog, as static data.
//!
//! ## Why these live here as text, not as compiled `recipe!` fns
//!
//! A `recipe!` fn compiles its body, and the bodies use `ui!`, whose
//! expansion targets `runtime_vocabulary::glue` — a crate this substrate
//! cannot depend on. So the recipe **descriptions** are static data here,
//! with each `source` pulled via `include_str!` from
//! `crates/runtime/shared/recipes/*.rs`. Those files are real Rust: they
//! stay compile-checked against the live author surface by
//! `crates/dev/newcore-app/tests/recipes_compile.rs`, which `include!`s
//! them and mounts each against the scene-parity mock host. A prop change
//! that breaks a recipe still fails a test.
//!
//! The `docs` literals below mirror each file's leading `///` block
//! (the file keeps the comment so the served `source` reads complete).
//! Entries register into the same `mcp_catalog::RecipeEntry` inventory
//! slice `recipe!` uses, so `list_recipes` / `describe_recipe` /
//! `get_catalog` serve them like any other recipe.

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

core_recipe!(
    "keyboard_game_controls",
    "on_key",
    "keyboard_game_controls.rs",
    "Keyboard controls for a game: move a player with WASD / arrow keys\nwhile they're HELD, and jump on Space. Two complementary APIs:\n\n- `key_state()` returns a handle you POLL from a frame loop —\n  `is_down` / `any_down` take physical key codes (`\"KeyA\"`,\n  `\"ArrowLeft\"`, `\"Space\"`), so WASD stays in place on any keyboard\n  layout and a key released after Shift lets go still reads as up.\n- `on_key` delivers every key DOWN and UP as an event; `e.is_press()`\n  is \"down and not an auto-repeat\". Returning `PreventDefault` stops\n  the platform default (Space scrolling the page on web).\n\nBoth live exactly as long as the component (`raf_loop_scoped` too),\nand held keys are released automatically when the window loses\nfocus, so nothing stays stuck after an alt-tab.",
    uses: ["text", "view"]
);

core_recipe!(
    "pointer_lock_game_controls",
    "on_pointer",
    "pointer_lock_game_controls.rs",
    "Mouse controls for a first-person game: click to lock the pointer,\nturn with mouse movement, fire with the left button, switch weapons\nwith the wheel, and pause automatically when the window loses focus.\n\n- `request_pointer_lock()` must be called from a click or key handler\n  (browsers refuse it otherwise). `pointer_locked()` is a signal: it\n  flips back to `false` when the user presses Esc or alt-tabs, so the\n  game shows its \"click to resume\" overlay from that alone.\n- `pointer_state()` is polled from the frame loop. `take_movement()`\n  returns the mouse motion since the last frame — while locked that is\n  the only motion there is (the cursor doesn't move). Each handle keeps\n  its own accumulator.\n- `on_pointer` sees buttons anywhere in the app; returning\n  `PreventDefault` for the right button suppresses the context menu.\n- `app_focused()` drives auto-pause.",
    uses: ["button", "text", "view"]
);

// ---------------------------------------------------------------------------
// Navigation recipes.
//
// These register here rather than in the navigator SDKs for the same
// reason as the core-primitive recipes (see the module docs): as static
// data with the source `include_str!`d, the served text needs no
// compiled `ui!` body, and the compile check happens in
// `crates/dev/newcore-app/tests/recipes_compile.rs`, against the real
// navigator SDKs.
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
    /// with source text and docs intact. The catalog data must exist
    /// with no author-surface crate linked (this crate's `catalog`
    /// feature alone links + populates it).
    #[test]
    fn core_recipes_register_into_the_catalog_slice() {
        let names: Vec<&str> = mcp_catalog::recipes().map(|r| r.name).collect();
        for expected in [
            "input_with_submit",
            "keyed_list_add_remove",
            "animated_toast",
            "confirm_dialog_overlay",
            "keyboard_game_controls",
            "pointer_lock_game_controls",
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
    /// navigator SDKs, so `list_recipes` always serves them.
    ///
    /// The assertions on the served source pin the load-bearing pieces
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
