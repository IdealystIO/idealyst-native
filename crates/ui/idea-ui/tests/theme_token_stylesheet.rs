//! Field report: a custom app `stylesheet!` should track theme tokens by
//! name without restating a fallback hex. `theme_token!` / `theme_length!`
//! provide that — but they expand to a braced block (`{ const _…; expr }`)
//! for the compile-time name check, so this test's real job is to prove
//! that shape *parses as a `stylesheet!` field value* (a pure compile check)
//! AND that the referenced token flows through to the resolved rules with
//! idea-theme's canonical value as its fallback, no hex written here.

use idea_ui::{install_idea_theme, light_theme, theme_length, theme_token};

use runtime_core::{resolve_style, stylesheet, StyleApplication, Tokenized};

// A custom, app-authored sheet — the "custom sidebar" from the report.
// Every theme-dependent value is a token reference by name; the file
// contains no palette hex at all. `theme_token!` appears in both the base
// block and a variant block to exercise both parse positions.
stylesheet! {
    pub Sidebar<()> {
        base(_t) {
            background: theme_token!("color-surface"),
            gap: theme_length!("spacing-lg"),
        }
        variant emphasis {
            #[default]
            normal(_t) { background: theme_token!("color-surface") }
            accent(_t) { background: theme_token!("intent-primary-soft-bg") }
        }
    }
}

/// The STATIC sheet application off a builder (the token reference is a
/// constant `Tokenized`, so the built style must be the static arm).
fn static_app(b: impl runtime_core::IntoStyleSource) -> StyleApplication {
    match b.into_style_prop() {
        runtime_vocabulary::StyleProp::Sheet(app) => *app,
        _ => panic!("a constant token reference must produce a static application"),
    }
}

fn background(app: StyleApplication) -> Tokenized<runtime_core::Color> {
    resolve_style(&app)
        .background
        .clone()
        .expect("sidebar base sets a background token")
}

#[test]
fn theme_token_references_flow_through_a_stylesheet() {
    idea_theme::testing::with_test_world(|| {
    // Base: references `color-surface` by name. Its fallback must be
    // idea-theme's canonical base value — restated nowhere in this file.
    let base_bg = background(static_app(Sidebar()));
    assert_eq!(base_bg.name(), Some("color-surface"));
    let canonical_surface = light_theme().colors.surface.value().0.clone();
    assert_eq!(
        base_bg.value().0, canonical_surface,
        "fallback must equal idea-theme's canonical surface, not a hardcoded hex"
    );

    // Install the theme, then the SAME by-name reference resolves to the
    // installed value — a reskin re-flows this sheet with no edits to it.
    install_idea_theme(light_theme());
    assert_eq!(
        base_bg.resolve().0, canonical_surface,
        "installed theme value resolves through the token reference"
    );

    // The accent variant references a different canonical token.
    let accent_bg = background(static_app(Sidebar().emphasis(SidebarEmphasis::Accent)));
    assert_eq!(accent_bg.name(), Some("intent-primary-soft-bg"));
    });
}
