//! Catalog-recipe coverage: the `stack_two_screens` recipe must register a
//! [`mcp_catalog::RecipeEntry`] whose served source keeps the parts the
//! arena nav-notes feedback (run-0) showed agents failing without — above
//! all the `use stack_navigator::{StackBuilder, StackScreenExt, …}` import
//! line (both are extension traits; omitting `StackBuilder` breaks
//! `.screen(...)` resolution) and the bare `{ nav.outlet }` splat.
//!
//! Runs only with `--features catalog` (the recipe doesn't exist otherwise)
//! and only on native hosts (mcp-catalog is a native-only dev-dep here).

#![cfg(all(feature = "catalog", not(target_arch = "wasm32")))]

#[test]
fn stack_two_screens_recipe_registers_with_import_line() {
    // Call the recipe fn so this test binary references the `recipes` module
    // — its `inventory::submit!` then survives dev-profile codegen-unit DCE
    // (same force-link rationale as the navigator `register` hooks). Also
    // proves the recipe's element tree builds outside a mounted app.
    let _el: runtime_core::Element = stack_navigator::recipes::stack_two_screens();

    let entry = mcp_catalog::recipes()
        .find(|r| r.name == "stack_two_screens")
        .expect("stack_two_screens recipe registered in the catalog");
    assert_eq!(entry.target, "StackNavigator");

    // The served source must be self-contained and keep the load-bearing
    // pieces an agent copy-pastes.
    for needle in [
        "use stack_navigator::",
        "StackBuilder",
        "StackScreenExt",
        "Route::<NoteId>::new(\"detail\", \"/note/:slug\")",
        "nav.outlet",
    ] {
        assert!(
            entry.source.contains(needle),
            "recipe source should contain `{needle}`; got:\n{}",
            entry.source,
        );
    }
}
