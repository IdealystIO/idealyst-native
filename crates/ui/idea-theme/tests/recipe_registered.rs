//! Catalog-registration test for the `dark_mode_toggle` recipe.
//!
//! The recipe's *content* is compile-checked by the `recipe!` macro
//! itself (the fn is emitted verbatim under `--features catalog`).
//! What that alone does NOT prove is the **wiring**: that this crate's
//! `catalog` feature actually forwards to `runtime-core/catalog` →
//! `runtime-macros/catalog` so the macro emits a registration (rather
//! than silently expanding to nothing), and that the `inventory`
//! distributed-slice entry links into the catalog. A mis-forwarded
//! feature would compile clean and serve nothing — exactly the failure
//! this test pins. (Regression guard for the arena themed-settings
//! run-0 finding: `list_recipes` offered no theming recipe.)
#![cfg(feature = "catalog")]

#[test]
fn dark_mode_toggle_recipe_registers_in_catalog() {
    // Reference the crate under test so the linker pulls idea-theme's
    // objects into the test binary at all — an integration test that
    // never names the crate links none of its inventory ctors.
    let _ = idea_theme::theme_installed();
    let entry = runtime_core::__mcp::recipes()
        .find(|r| r.name == "dark_mode_toggle")
        .expect(
            "dark_mode_toggle recipe missing from the catalog slice — \
             is idea-theme's `catalog` feature forwarding to runtime-core/catalog?",
        );
    assert_eq!(entry.target, "install_theme");
    // The served source must be the real, self-contained example.
    assert!(entry.source.contains("set_theme"));
    assert!(entry.source.contains("stylesheet!"));
    assert!(!entry.docs.is_empty(), "recipe should carry prose docs");
}
